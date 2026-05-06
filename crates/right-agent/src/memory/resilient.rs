//! Resilient wrapper around `HindsightClient`: circuit breaker + classified retry
//! + retain queue + status watch.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::time::Instant;

use super::MemoryError;
use super::circuit::{Breaker, Outcome};
use super::classify::ErrorKind;
use super::hindsight::{
    BankProfile, HindsightClient, RecallResult, ReflectResponse, RetainItem, RetainResponse,
};
use super::status::MemoryStatus;

/// Error returned by the resilient wrapper.
#[derive(Debug, thiserror::Error)]
pub enum ResilientError {
    #[error("upstream error: {0}")]
    Upstream(#[from] MemoryError),
    #[error("memory circuit open; retry after {retry_after:?}")]
    CircuitOpen { retry_after: Option<Duration> },
}

/// Per-operation retry policy.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub per_attempt: Duration,
    /// Total attempts = attempts + 1 (i.e. 0 means single try, no retry).
    pub attempts: u32,
}

pub const POLICY_BLOCKING_RECALL: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(3),
    attempts: 0,
};
pub const POLICY_AUTO_RETAIN: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(10),
    attempts: 2,
};
pub const POLICY_PREFETCH: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(5),
    attempts: 1,
};
pub const POLICY_MCP_RETAIN: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(10),
    attempts: 1,
};
pub const POLICY_MCP_RECALL: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(5),
    attempts: 0,
};
pub const POLICY_MCP_REFLECT: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(15),
    attempts: 0,
};
pub const POLICY_STARTUP_BANK: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(10),
    attempts: 3,
};

/// In-memory rolling window for Client-kind drop timestamps.
const DROP_WINDOW: Duration = Duration::from_secs(3600);
const DROP_WINDOW_24H: Duration = Duration::from_secs(86_400);
pub const CLIENT_FLOOD_THRESHOLD: usize = 20;

pub struct ResilientHindsight {
    inner: HindsightClient,
    agent_db_path: PathBuf,
    breaker: Mutex<Breaker>,
    status_tx: watch::Sender<MemoryStatus>,
    client_drops: Mutex<VecDeque<Instant>>,
    /// "bot" or "aggregator" — tags `pending_retains.source`.
    source: String,
}

impl ResilientHindsight {
    pub fn new(inner: HindsightClient, agent_db_path: PathBuf, source: impl Into<String>) -> Self {
        let (tx, _rx) = watch::channel(MemoryStatus::Healthy);
        Self {
            inner,
            agent_db_path,
            breaker: Mutex::new(Breaker::new()),
            status_tx: tx,
            client_drops: Mutex::new(VecDeque::new()),
            source: source.into(),
        }
    }

    pub fn inner(&self) -> &HindsightClient {
        &self.inner
    }

    pub fn agent_db_path(&self) -> &Path {
        &self.agent_db_path
    }

    pub fn status(&self) -> MemoryStatus {
        *self.status_tx.borrow()
    }

    pub fn subscribe_status(&self) -> watch::Receiver<MemoryStatus> {
        self.status_tx.subscribe()
    }

    /// Count of Client-kind drops in the last 24h. Evicts stale entries in place.
    pub async fn client_drops_24h(&self) -> usize {
        let mut q = self.client_drops.lock().await;
        let cutoff = Instant::now() - DROP_WINDOW_24H;
        while q.front().is_some_and(|t| *t < cutoff) {
            q.pop_front();
        }
        q.len()
    }

    /// Count of Client-kind drops in the last 1h (for flood alert). Read-only.
    pub async fn client_drops_1h(&self) -> usize {
        let q = self.client_drops.lock().await;
        let cutoff = Instant::now() - DROP_WINDOW;
        q.iter().filter(|t| **t >= cutoff).count()
    }

    pub async fn bump_client_drop(&self) {
        let mut q = self.client_drops.lock().await;
        let now = Instant::now();
        q.push_back(now);
        let cutoff = now - DROP_WINDOW_24H;
        while q.front().is_some_and(|t| *t < cutoff) {
            q.pop_front();
        }
    }

    async fn refresh_status(&self) {
        let st = {
            let mut b = self.breaker.lock().await;
            b.state()
        };
        // send_if_modified atomically reads-and-conditionally-writes, closing the
        // race window between borrow() and send_replace(). AuthFailed is sticky —
        // only the startup probe reset (or explicit recovery) clears it.
        self.status_tx.send_if_modified(|cur| {
            if matches!(*cur, MemoryStatus::AuthFailed { .. }) {
                return false;
            }
            let new = match st {
                crate::memory::circuit::CircuitState::Closed => MemoryStatus::Healthy,
                crate::memory::circuit::CircuitState::Open { .. }
                | crate::memory::circuit::CircuitState::HalfOpen => MemoryStatus::Degraded {
                    since: std::time::Instant::now(),
                },
            };
            if *cur != new {
                *cur = new;
                true
            } else {
                false
            }
        });
    }

    fn backoff(attempt: u32) -> Duration {
        // checked_shl prevents overflow at attempt >=64 (defensive; RetryPolicy caps attempts).
        let base_ms = 500u64.checked_shl(attempt).unwrap_or(u64::MAX);
        let jitter_ms = fastrand::u64(0..250);
        Duration::from_millis(base_ms.saturating_add(jitter_ms))
    }

    /// Wrap a single upstream call with per-attempt timeout + retry loop.
    async fn call_with_policy<F, Fut, T>(
        &self,
        policy: RetryPolicy,
        mut op: F,
    ) -> Result<T, ResilientError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, MemoryError>>,
    {
        for attempt in 0..=policy.attempts {
            // Breaker check.
            {
                let mut b = self.breaker.lock().await;
                if let Err(retry_after) = b.admit() {
                    drop(b);
                    self.refresh_status().await;
                    return Err(ResilientError::CircuitOpen {
                        retry_after: Some(retry_after),
                    });
                }
            }

            let call = op();
            let res = tokio::time::timeout(policy.per_attempt, call).await;
            let out = match res {
                Err(_) => Err(MemoryError::HindsightTimeout),
                Ok(r) => r,
            };

            match out {
                Ok(val) => {
                    {
                        let mut b = self.breaker.lock().await;
                        b.record(Outcome::Success);
                    }
                    self.refresh_status().await;
                    return Ok(val);
                }
                Err(e) => {
                    let kind = e.classify();
                    {
                        let mut b = self.breaker.lock().await;
                        b.record(Outcome::Failure(kind));
                    }
                    if matches!(kind, ErrorKind::Auth) {
                        // send_if_modified avoids waking watchers on persistent 401s.
                        self.status_tx.send_if_modified(|cur| {
                            if matches!(*cur, MemoryStatus::AuthFailed { .. }) {
                                false
                            } else {
                                *cur = MemoryStatus::AuthFailed {
                                    since: std::time::Instant::now(),
                                };
                                true
                            }
                        });
                        return Err(ResilientError::Upstream(e));
                    }
                    if matches!(kind, ErrorKind::Client | ErrorKind::Malformed) {
                        self.refresh_status().await;
                        return Err(ResilientError::Upstream(e));
                    }
                    if attempt == policy.attempts {
                        self.refresh_status().await;
                        return Err(ResilientError::Upstream(e));
                    }
                    tokio::time::sleep(Self::backoff(attempt)).await;
                }
            }
        }
        unreachable!("retry loop must return");
    }

    pub async fn recall(
        &self,
        query: &str,
        tags: Option<&[String]>,
        tags_match: Option<&str>,
        policy: RetryPolicy,
    ) -> Result<Vec<RecallResult>, ResilientError> {
        let inner = &self.inner;
        let tags_v = tags.map(|t| t.to_vec());
        self.call_with_policy(policy, || {
            let tv = tags_v.clone();
            let tm = tags_match.map(|s| s.to_owned());
            async move { inner.recall(query, tv.as_deref(), tm.as_deref()).await }
        })
        .await
    }

    pub async fn retain(
        &self,
        content: &str,
        context: Option<&str>,
        document_id: Option<&str>,
        update_mode: Option<&str>,
        tags: Option<&[String]>,
        policy: RetryPolicy,
    ) -> Result<RetainResponse, ResilientError> {
        let res = self
            .call_with_policy(policy, || {
                let inner = &self.inner;
                async move {
                    inner
                        .retain(content, context, document_id, update_mode, tags)
                        .await
                }
            })
            .await;

        if let Err(ref err) = res {
            match err {
                ResilientError::Upstream(e) => match e.classify() {
                    ErrorKind::Transient | ErrorKind::RateLimited => {
                        self.enqueue_for_retry(content, context, document_id, update_mode, tags)
                            .await;
                    }
                    ErrorKind::Client | ErrorKind::Malformed => {
                        self.bump_client_drop().await;
                        tracing::error!(
                            "retain dropped ({:?}) — not enqueueing; content_preview={:?}",
                            e.classify(),
                            &content.chars().take(80).collect::<String>()
                        );
                    }
                    ErrorKind::Auth => {
                        // Don't enqueue; Auth resets on startup probe success.
                    }
                },
                ResilientError::CircuitOpen { .. } => {
                    // Don't enqueue when the breaker is open due to Auth — the queue
                    // would grow with entries that can't drain (drain gates on
                    // Healthy, which AuthFailed sticks against).
                    if !matches!(*self.status_tx.borrow(), MemoryStatus::AuthFailed { .. }) {
                        self.enqueue_for_retry(content, context, document_id, update_mode, tags)
                            .await;
                    }
                }
            }
        }
        res
    }

    async fn enqueue_for_retry(
        &self,
        content: &str,
        context: Option<&str>,
        document_id: Option<&str>,
        update_mode: Option<&str>,
        tags: Option<&[String]>,
    ) {
        // Open a fresh connection for each enqueue — cheap (WAL, same process),
        // and avoids holding the drain connection while we're on the error path.
        match right_db::open_connection(&self.agent_db_path, false) {
            Ok(conn) => {
                if let Err(e) = super::retain_queue::enqueue(
                    &conn,
                    &self.source,
                    content,
                    context,
                    document_id,
                    update_mode,
                    tags,
                ) {
                    tracing::error!("retain enqueue failed: {e:#}");
                }
            }
            Err(e) => {
                tracing::error!("retain enqueue: open_connection failed: {e:#}");
            }
        }
    }

    pub async fn reflect(
        &self,
        query: &str,
        policy: RetryPolicy,
    ) -> Result<ReflectResponse, ResilientError> {
        let inner = &self.inner;
        self.call_with_policy(policy, || async move { inner.reflect(query).await })
            .await
    }

    pub async fn get_or_create_bank(
        &self,
        policy: RetryPolicy,
    ) -> Result<BankProfile, ResilientError> {
        let inner = &self.inner;
        let out = self
            .call_with_policy(policy, || async move { inner.get_or_create_bank().await })
            .await;

        if out.is_ok() && matches!(*self.status_tx.borrow(), MemoryStatus::AuthFailed { .. }) {
            self.status_tx.send_replace(MemoryStatus::Healthy);
        }
        out
    }

    /// Drain helper invoked by the bot drain task. Uses `retain_many` for single-item POST.
    pub async fn drain_retain_item(&self, item: &RetainItem) -> Result<(), ErrorKind> {
        let inner = &self.inner;
        let policy = RetryPolicy {
            per_attempt: Duration::from_secs(10),
            attempts: 0,
        };
        let res = self
            .call_with_policy(policy, || {
                let batch = vec![item.clone()];
                async move { inner.retain_many(&batch).await.map(|_| ()) }
            })
            .await;
        match res {
            Ok(()) => Ok(()),
            Err(ResilientError::Upstream(e)) => Err(e.classify()),
            Err(ResilientError::CircuitOpen { .. }) => Err(ErrorKind::Transient),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use right_db::open_connection;
    use tempfile::tempdir;

    #[tokio::test]
    async fn bump_client_drop_records_timestamp() {
        let client = HindsightClient::new("hs_x", "b", "high", 1024, Some("http://127.0.0.1:1"));
        let w = ResilientHindsight::new(client, PathBuf::from("/tmp"), "bot");
        assert_eq!(w.client_drops_24h().await, 0);
        w.bump_client_drop().await;
        w.bump_client_drop().await;
        assert_eq!(w.client_drops_24h().await, 2);
    }

    #[tokio::test]
    async fn status_starts_healthy() {
        let client = HindsightClient::new("hs_x", "b", "high", 1024, Some("http://127.0.0.1:1"));
        let w = ResilientHindsight::new(client, PathBuf::from("/tmp"), "bot");
        assert!(matches!(w.status(), MemoryStatus::Healthy));
    }

    /// Mock HTTP server that responds to each incoming connection with the given
    /// status + body. Loops forever so the wrapper can retry or make multiple calls
    /// against the same URL.
    async fn mock(hs_body: &str, status: u16) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let body = hs_body.to_owned();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body,
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (handle, url)
    }

    fn wrap(url: &str) -> ResilientHindsight {
        let dir = tempdir().unwrap().keep();
        let _ = open_connection(&dir, true).unwrap();
        let client = HindsightClient::new("hs_x", "bank-1", "high", 1024, Some(url));
        ResilientHindsight::new(client, dir, "bot")
    }

    #[tokio::test]
    async fn recall_success_returns_results() {
        let (_h, url) = mock(r#"{"results": [{"text": "hi", "score": 0.9}]}"#, 200).await;
        let w = wrap(&url);
        let policy = RetryPolicy {
            per_attempt: Duration::from_secs(2),
            attempts: 0,
        };
        let results = w.recall("q", None, None, policy).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "hi");
        assert!(matches!(w.status(), MemoryStatus::Healthy));
    }

    #[tokio::test]
    async fn recall_auth_sets_status_auth_failed_and_returns_upstream_err() {
        let (_h, url) = mock(r#"{"error": "unauthorized"}"#, 401).await;
        let w = wrap(&url);
        let policy = RetryPolicy {
            per_attempt: Duration::from_secs(2),
            attempts: 0,
        };
        let err = w.recall("q", None, None, policy).await.unwrap_err();
        assert!(matches!(err, ResilientError::Upstream(_)));
        assert!(matches!(w.status(), MemoryStatus::AuthFailed { .. }));
    }

    #[tokio::test]
    async fn recall_circuit_open_skips_http_call() {
        // No mock server at this port — if breaker let us through we'd get a connect error.
        let w = wrap("http://127.0.0.1:1");
        // Force breaker open by feeding it an Auth failure.
        {
            let mut b = w.breaker.lock().await;
            b.record(Outcome::Failure(ErrorKind::Auth));
        }
        let policy = RetryPolicy {
            per_attempt: Duration::from_secs(2),
            attempts: 0,
        };
        let err = w.recall("q", None, None, policy).await.unwrap_err();
        assert!(
            matches!(err, ResilientError::CircuitOpen { .. }),
            "expected CircuitOpen, got {err:?}"
        );
    }

    #[tokio::test]
    async fn retain_enqueues_on_transient_error() {
        let (_h, url) = mock(r#"{"error": "upstream down"}"#, 503).await;
        let w = wrap(&url);
        let policy = RetryPolicy {
            per_attempt: Duration::from_secs(2),
            attempts: 0,
        };
        let err = w
            .retain("content-1", None, None, None, None, policy)
            .await
            .unwrap_err();
        assert!(matches!(err, ResilientError::Upstream(_)));
        // Row should now be in pending_retains.
        let conn = open_connection(w.agent_db_path(), false).unwrap();
        let cnt = crate::memory::retain_queue::count(&conn).unwrap();
        assert_eq!(cnt, 1, "expected row enqueued on transient 503");
    }

    #[tokio::test]
    async fn retain_does_not_enqueue_on_client_error() {
        let (_h, url) = mock(r#"{"error": "bad payload"}"#, 400).await;
        let w = wrap(&url);
        let policy = RetryPolicy {
            per_attempt: Duration::from_secs(2),
            attempts: 0,
        };
        let err = w
            .retain("poison", None, None, None, None, policy)
            .await
            .unwrap_err();
        assert!(matches!(err, ResilientError::Upstream(_)));
        let conn = open_connection(w.agent_db_path(), false).unwrap();
        let cnt = crate::memory::retain_queue::count(&conn).unwrap();
        assert_eq!(cnt, 0, "4xx must not enqueue");
        // Client drop counter must have been bumped.
        assert_eq!(w.client_drops_24h().await, 1);
    }
}
