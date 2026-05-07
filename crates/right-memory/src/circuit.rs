//! Circuit breaker for outbound Hindsight calls.
//!
//! State transitions:
//! - Closed + N failures in 30s rolling window → Open { until = now + 30s }
//! - Open + now >= until → HalfOpen
//! - HalfOpen + success → Closed
//! - HalfOpen + failure → Open { until = now + 2 * previous_open_dur }, capped at 10 min
//! - Any state + Auth kind → Open { until = now + 1h }
//!
//! All methods except `new()` / `Default::default()` must be called from within a
//! tokio runtime context — they call `tokio::time::Instant::now()` which panics
//! outside a runtime.

use std::collections::VecDeque;
use std::time::Duration;
use tokio::time::Instant;

use super::ErrorKind;

pub(crate) const WINDOW: Duration = Duration::from_secs(30);
pub(crate) const TRIP_THRESHOLD: usize = 5;
pub(crate) const INITIAL_OPEN: Duration = Duration::from_secs(30);
pub(crate) const MAX_OPEN: Duration = Duration::from_secs(600); // 10 min
pub(crate) const AUTH_OPEN: Duration = Duration::from_secs(3600); // 1h

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CircuitState {
    Closed,
    Open { until: Instant },
    HalfOpen,
}

pub(crate) struct Breaker {
    state: CircuitState,
    failures: VecDeque<Instant>,
    last_open_duration: Duration,
}

impl Default for Breaker {
    fn default() -> Self {
        Self::new()
    }
}

impl Breaker {
    pub(crate) fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            failures: VecDeque::new(),
            last_open_duration: INITIAL_OPEN,
        }
    }

    pub(crate) fn state(&mut self) -> CircuitState {
        self.refresh();
        self.state
    }

    /// Call before each network attempt. Returns `Err(retry_after)` when call must be skipped.
    pub(crate) fn admit(&mut self) -> Result<(), Duration> {
        self.refresh();
        match self.state {
            CircuitState::Closed | CircuitState::HalfOpen => Ok(()),
            CircuitState::Open { until } => Err(until.saturating_duration_since(Instant::now())),
        }
    }

    /// Record an attempt outcome.
    pub(crate) fn record(&mut self, outcome: Outcome) {
        self.refresh();
        match outcome {
            Outcome::Success => {
                if matches!(self.state, CircuitState::HalfOpen) {
                    self.state = CircuitState::Closed;
                    self.failures.clear();
                    self.last_open_duration = INITIAL_OPEN;
                }
            }
            Outcome::Failure(ErrorKind::Auth) => {
                self.state = CircuitState::Open {
                    until: Instant::now() + AUTH_OPEN,
                };
                self.last_open_duration = AUTH_OPEN;
            }
            Outcome::Failure(ErrorKind::Client) => {
                // Client errors do not tick the breaker.
            }
            Outcome::Failure(_) => {
                self.push_failure(Instant::now());
                match self.state {
                    CircuitState::Closed if self.failures.len() >= TRIP_THRESHOLD => {
                        self.state = CircuitState::Open {
                            until: Instant::now() + INITIAL_OPEN,
                        };
                        self.last_open_duration = INITIAL_OPEN;
                    }
                    CircuitState::HalfOpen => {
                        let new_dur = (self.last_open_duration * 2).min(MAX_OPEN);
                        self.state = CircuitState::Open {
                            until: Instant::now() + new_dur,
                        };
                        self.last_open_duration = new_dur;
                    }
                    _ => {}
                }
            }
        }
    }

    fn refresh(&mut self) {
        if let CircuitState::Open { until } = self.state
            && Instant::now() >= until
        {
            self.state = CircuitState::HalfOpen;
        }
        // Evict stale failures.
        let cutoff = Instant::now() - WINDOW;
        while self.failures.front().is_some_and(|t| *t < cutoff) {
            self.failures.pop_front();
        }
    }

    fn push_failure(&mut self, at: Instant) {
        self.failures.push_back(at);
        let cutoff = at - WINDOW;
        while self.failures.front().is_some_and(|t| *t < cutoff) {
            self.failures.pop_front();
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Outcome {
    Success,
    Failure(ErrorKind),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(b: &mut Breaker, kind: ErrorKind) {
        b.record(Outcome::Failure(kind));
    }

    #[tokio::test(start_paused = true)]
    async fn closed_opens_after_threshold() {
        let mut b = Breaker::new();
        for _ in 0..TRIP_THRESHOLD {
            fail(&mut b, ErrorKind::Transient);
        }
        assert!(matches!(b.state(), CircuitState::Open { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn open_transitions_to_half_open() {
        let mut b = Breaker::new();
        for _ in 0..TRIP_THRESHOLD {
            fail(&mut b, ErrorKind::Transient);
        }
        tokio::time::advance(INITIAL_OPEN + Duration::from_millis(10)).await;
        assert_eq!(b.state(), CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn half_open_success_closes() {
        let mut b = Breaker::new();
        for _ in 0..TRIP_THRESHOLD {
            fail(&mut b, ErrorKind::Transient);
        }
        tokio::time::advance(INITIAL_OPEN + Duration::from_millis(10)).await;
        assert_eq!(b.state(), CircuitState::HalfOpen);
        b.record(Outcome::Success);
        assert_eq!(b.state(), CircuitState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn half_open_failure_doubles_backoff() {
        let mut b = Breaker::new();
        for _ in 0..TRIP_THRESHOLD {
            fail(&mut b, ErrorKind::Transient);
        }
        tokio::time::advance(INITIAL_OPEN + Duration::from_millis(10)).await;
        fail(&mut b, ErrorKind::Transient);
        match b.state() {
            CircuitState::Open { until } => {
                let dur = until.saturating_duration_since(Instant::now());
                assert!(dur > INITIAL_OPEN, "expected >30s, got {dur:?}");
                assert!(dur <= INITIAL_OPEN * 2 + Duration::from_millis(50));
            }
            other => panic!("expected Open, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn auth_opens_for_1h() {
        let mut b = Breaker::new();
        fail(&mut b, ErrorKind::Auth);
        match b.state() {
            CircuitState::Open { until } => {
                let dur = until.saturating_duration_since(Instant::now());
                assert!(dur >= AUTH_OPEN - Duration::from_millis(10));
            }
            other => panic!("expected Open, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn client_does_not_tick() {
        let mut b = Breaker::new();
        for _ in 0..(TRIP_THRESHOLD * 2) {
            fail(&mut b, ErrorKind::Client);
        }
        assert_eq!(b.state(), CircuitState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn stale_failures_evict_by_window() {
        let mut b = Breaker::new();
        for _ in 0..(TRIP_THRESHOLD - 1) {
            fail(&mut b, ErrorKind::Transient);
        }
        tokio::time::advance(WINDOW + Duration::from_secs(1)).await;
        // One more failure shouldn't trip because older ones expired.
        fail(&mut b, ErrorKind::Transient);
        assert_eq!(b.state(), CircuitState::Closed);
    }
}
