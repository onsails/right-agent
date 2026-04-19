//! Resilient wrapper around `HindsightClient`: circuit breaker + classified retry
//! + retain queue + status watch.
//!
//! This module is the Task 9 skeleton. Several fields (`inner`, `agent_db_path`,
//! `breaker`, `source`) are populated here but unused until Task 10 wires up the
//! call methods (recall, retain, reflect). The file-level `dead_code` allow keeps
//! the skeleton warning-free until Task 10 lands in the same branch.

#![allow(dead_code)] // fields and retry policies come online in Task 10

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::time::Instant;

use super::MemoryError;
use super::circuit::Breaker;
use super::hindsight::HindsightClient;
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
    /// "bot" or "aggregator" — used by Task 10 to tag `pending_retains.source`.
    source: String,
}

impl ResilientHindsight {
    pub fn new(
        inner: HindsightClient,
        agent_db_path: PathBuf,
        source: impl Into<String>,
    ) -> Self {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
