//! Injected pending-retain queue interfaces.
//!
//! The single-owner architecture forbids `right-memory` from opening the
//! per-agent database itself: the Aggregator's `AgentDbOwner` is the sole
//! live connection owner. `ResilientHindsight` therefore receives a
//! [`PendingRetainSink`] for fire-and-forget enqueue, and the drain loop
//! drives a [`RetainLeaseQueue`] with lease-based claim/ack/nack operations.
//!
//! Two production implementations exist:
//!
//! - the bot's typed-IPC adapter (talks to the Aggregator over
//!   `internal.sock`), and
//! - the Aggregator-side owner adapter (executes the SQL primitives in
//!   [`crate::retain_queue`] inside the owner).
//!
//! [`InMemoryRetainQueue`] backs unit tests for the lease protocol: crash
//! after claim, lease expiry, duplicate-claim exclusion, and stale-token
//! rejection.

use std::collections::VecDeque;
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use super::error::MemoryError;
use super::retain_queue::{MAX_AGE, PendingRetain, QUEUE_CAP};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A retain payload to enqueue for later drain.
#[derive(Debug, Clone)]
pub struct NewPendingRetain {
    /// "bot" or "aggregator" — tags `pending_retains.source`.
    pub source: String,
    pub content: String,
    pub context: Option<String>,
    pub document_id: Option<String>,
    pub update_mode: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// A leased batch of pending retains.
///
/// `claim_token` guards every subsequent ack/nack: the queue applies a
/// transition only when the presented token still matches the row's lease, so
/// a crashed drainer's stale token can never delete or requeue rows claimed
/// by its successor. Rows whose lease expires are reclaimable by the next
/// [`RetainLeaseQueue::claim_batch`].
#[derive(Debug, Clone)]
pub struct RetainClaim {
    pub claim_token: String,
    /// RFC3339 lease expiry.
    pub lease_expires_at: String,
    pub items: Vec<PendingRetain>,
    /// Rows deleted during this claim because they exceeded the 24h age cap.
    pub dropped_age: usize,
}

/// Queue depth report (mirror of the SQL `count`/`oldest_age` helpers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainQueueStats {
    pub count: usize,
    pub oldest_age: Option<Duration>,
}

/// Fire-and-forget enqueue side of the queue (used by `ResilientHindsight`).
pub trait PendingRetainSink: Send + Sync + Debug {
    fn enqueue(&self, item: NewPendingRetain) -> BoxFuture<'static, Result<(), MemoryError>>;
}

/// Lease-based drain side of the queue (used by the drain loop).
pub trait RetainLeaseQueue: Send + Sync + Debug {
    /// Atomically drop stale rows, reclaim expired leases, and lease up to
    /// `limit` oldest unclaimed rows for `lease_ttl`.
    fn claim_batch(
        &self,
        limit: usize,
        lease_ttl: Duration,
    ) -> BoxFuture<'static, Result<RetainClaim, MemoryError>>;

    /// Delete row `id`, guarded by `claim_token`.
    ///
    /// Returns [`MemoryError::LeaseConflict`] when the token no longer matches
    /// (lease expired and the row was reclaimed by another claim).
    fn ack(&self, claim_token: &str, id: i64) -> BoxFuture<'static, Result<(), MemoryError>>;

    /// Release row `id` back to the queue (`retry = true`, bumping attempts)
    /// or drop it permanently (`retry = false`), guarded by `claim_token`.
    ///
    /// Same stale-token conflict semantics as [`RetainLeaseQueue::ack`].
    fn nack(
        &self,
        claim_token: &str,
        id: i64,
        retry: bool,
        error: &str,
    ) -> BoxFuture<'static, Result<(), MemoryError>>;

    fn stats(&self) -> BoxFuture<'static, Result<RetainQueueStats, MemoryError>>;
}

// ---------------------------------------------------------------------------
// In-memory implementation (tests)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct MemRow {
    id: i64,
    content: String,
    context: Option<String>,
    document_id: Option<String>,
    update_mode: Option<String>,
    tags: Option<Vec<String>>,
    created_at: String,
    attempts: i64,
    last_attempt_at: Option<String>,
    last_error: Option<String>,
    claim_token: Option<String>,
    claim_expires_at: Option<String>,
}

impl MemRow {
    fn as_pending(&self) -> PendingRetain {
        PendingRetain {
            id: self.id,
            content: self.content.clone(),
            context: self.context.clone(),
            document_id: self.document_id.clone(),
            update_mode: self.update_mode.clone(),
            tags: self.tags.clone(),
            created_at: self.created_at.clone(),
            attempts: self.attempts,
        }
    }
}

#[derive(Debug, Default)]
struct MemInner {
    next_id: i64,
    next_claim: u64,
    rows: VecDeque<MemRow>,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn is_stale(created_at: &str) -> bool {
    match chrono::DateTime::parse_from_rfc3339(created_at) {
        Ok(dt) => {
            chrono::Utc::now().signed_duration_since(dt.with_timezone(&chrono::Utc)) > MAX_AGE
        }
        // Unparseable timestamps are not the same as old rows: keep them.
        Err(_) => false,
    }
}

/// In-memory [`PendingRetainSink`] + [`RetainLeaseQueue`] for tests.
///
/// Mirrors the SQL lease semantics: claim marks rows with a unique token and
/// expiry, expired leases are reclaimed on the next claim, and ack/nack are
/// rejected with [`MemoryError::LeaseConflict`] on a stale token.
#[derive(Debug, Default)]
pub struct InMemoryRetainQueue {
    inner: Mutex<MemInner>,
}

impl InMemoryRetainQueue {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemInner> {
        self.inner.lock().expect("in-memory retain queue poisoned")
    }
}

impl PendingRetainSink for InMemoryRetainQueue {
    fn enqueue(&self, item: NewPendingRetain) -> BoxFuture<'static, Result<(), MemoryError>> {
        let mut inner = self.lock();
        while inner.rows.len() >= QUEUE_CAP {
            inner.rows.pop_front();
        }
        inner.next_id += 1;
        let id = inner.next_id;
        inner.rows.push_back(MemRow {
            id,
            content: item.content,
            context: item.context,
            document_id: item.document_id,
            update_mode: item.update_mode,
            tags: item.tags,
            created_at: now_rfc3339(),
            attempts: 0,
            last_attempt_at: None,
            last_error: None,
            claim_token: None,
            claim_expires_at: None,
        });
        Box::pin(async { Ok(()) })
    }
}

impl RetainLeaseQueue for InMemoryRetainQueue {
    fn claim_batch(
        &self,
        limit: usize,
        lease_ttl: Duration,
    ) -> BoxFuture<'static, Result<RetainClaim, MemoryError>> {
        let mut inner = self.lock();
        let now = chrono::Utc::now();
        let now_s = now_rfc3339();

        // Age cap: drop stale rows first.
        let before = inner.rows.len();
        inner.rows.retain(|row| !is_stale(&row.created_at));
        let dropped_age = before - inner.rows.len();

        // Reclaim expired leases.
        for row in &mut inner.rows {
            if let Some(expires) = &row.claim_expires_at
                && expires.as_str() <= now_s.as_str()
            {
                row.claim_token = None;
                row.claim_expires_at = None;
            }
        }

        inner.next_claim += 1;
        let token = format!("mem-claim-{}", inner.next_claim);
        let lease_expires_at =
            (now + lease_ttl).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let mut items = Vec::new();
        for row in &mut inner.rows {
            if items.len() >= limit {
                break;
            }
            if row.claim_token.is_none() {
                row.claim_token = Some(token.clone());
                row.claim_expires_at = Some(lease_expires_at.clone());
                items.push(row.as_pending());
            }
        }

        let claim = RetainClaim {
            claim_token: token,
            lease_expires_at,
            items,
            dropped_age,
        };
        Box::pin(async move { Ok(claim) })
    }

    fn ack(&self, claim_token: &str, id: i64) -> BoxFuture<'static, Result<(), MemoryError>> {
        let mut inner = self.lock();
        let result = match inner
            .rows
            .iter()
            .position(|row| row.id == id && row.claim_token.as_deref() == Some(claim_token))
        {
            Some(pos) => {
                inner.rows.remove(pos);
                Ok(())
            }
            None => Err(MemoryError::LeaseConflict(format!(
                "ack id {id}: claim token no longer holds the lease"
            ))),
        };
        Box::pin(async move { result })
    }

    fn nack(
        &self,
        claim_token: &str,
        id: i64,
        retry: bool,
        error: &str,
    ) -> BoxFuture<'static, Result<(), MemoryError>> {
        let mut inner = self.lock();
        let result = match inner
            .rows
            .iter_mut()
            .find(|row| row.id == id && row.claim_token.as_deref() == Some(claim_token))
        {
            Some(row) if retry => {
                row.claim_token = None;
                row.claim_expires_at = None;
                row.attempts += 1;
                row.last_attempt_at = Some(now_rfc3339());
                row.last_error = Some(error.to_owned());
                Ok(())
            }
            Some(_) => {
                inner.rows.retain(|row| row.id != id);
                Ok(())
            }
            None => Err(MemoryError::LeaseConflict(format!(
                "nack id {id}: claim token no longer holds the lease"
            ))),
        };
        Box::pin(async move { result })
    }

    fn stats(&self) -> BoxFuture<'static, Result<RetainQueueStats, MemoryError>> {
        let inner = self.lock();
        let oldest = inner
            .rows
            .iter()
            .filter_map(|row| chrono::DateTime::parse_from_rfc3339(&row.created_at).ok())
            .map(|dt| {
                chrono::Utc::now()
                    .signed_duration_since(dt.with_timezone(&chrono::Utc))
                    .to_std()
                    .unwrap_or(Duration::ZERO)
            })
            .max();
        let stats = RetainQueueStats {
            count: inner.rows.len(),
            oldest_age: oldest,
        };
        Box::pin(async move { Ok(stats) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str) -> NewPendingRetain {
        NewPendingRetain {
            source: "bot".to_owned(),
            content: content.to_owned(),
            context: None,
            document_id: None,
            update_mode: None,
            tags: None,
        }
    }

    const LEASE: Duration = Duration::from_secs(60);

    #[tokio::test]
    async fn duplicate_claim_excludes_leased_rows() {
        let queue = InMemoryRetainQueue::new();
        queue.enqueue(item("a")).await.unwrap();
        queue.enqueue(item("b")).await.unwrap();

        let first = queue.claim_batch(10, LEASE).await.unwrap();
        assert_eq!(first.items.len(), 2);

        let second = queue.claim_batch(10, LEASE).await.unwrap();
        assert!(
            second.items.is_empty(),
            "leased rows must not be claimed twice"
        );
    }

    #[tokio::test]
    async fn crash_after_claim_requeues_on_lease_expiry() {
        let queue = InMemoryRetainQueue::new();
        queue.enqueue(item("a")).await.unwrap();

        // Drainer claims, then "crashes" (never acks/nacks).
        let crashed = queue
            .claim_batch(10, Duration::from_millis(20))
            .await
            .unwrap();
        assert_eq!(crashed.items.len(), 1);

        // Before expiry the row stays leased.
        let early = queue.claim_batch(10, LEASE).await.unwrap();
        assert!(early.items.is_empty());

        tokio::time::sleep(Duration::from_millis(40)).await;

        // After expiry the next claim reclaims the row.
        let reclaimed = queue.claim_batch(10, LEASE).await.unwrap();
        assert_eq!(reclaimed.items.len(), 1);
        assert_eq!(reclaimed.items[0].content, "a");
        assert_ne!(
            reclaimed.claim_token, crashed.claim_token,
            "reclaim must mint a fresh token"
        );
    }

    #[tokio::test]
    async fn stale_token_cannot_ack_or_nack() {
        let queue = InMemoryRetainQueue::new();
        queue.enqueue(item("a")).await.unwrap();

        let stale = queue
            .claim_batch(10, Duration::from_millis(20))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        let fresh = queue.claim_batch(10, LEASE).await.unwrap();

        let ack = queue.ack(&stale.claim_token, stale.items[0].id).await;
        assert!(
            matches!(ack, Err(MemoryError::LeaseConflict(_))),
            "stale ack must be rejected: {ack:?}"
        );
        let nack = queue
            .nack(&stale.claim_token, stale.items[0].id, true, "stale")
            .await;
        assert!(
            matches!(nack, Err(MemoryError::LeaseConflict(_))),
            "stale nack must be rejected: {nack:?}"
        );

        // The fresh claim still owns the row and can ack it.
        queue
            .ack(&fresh.claim_token, fresh.items[0].id)
            .await
            .unwrap();
        assert_eq!(queue.stats().await.unwrap().count, 0);
    }

    #[tokio::test]
    async fn nack_retry_requeues_with_attempt_bump() {
        let queue = InMemoryRetainQueue::new();
        queue.enqueue(item("a")).await.unwrap();
        let claim = queue.claim_batch(10, LEASE).await.unwrap();

        queue
            .nack(&claim.claim_token, claim.items[0].id, true, "transient")
            .await
            .unwrap();

        let re = queue.claim_batch(10, LEASE).await.unwrap();
        assert_eq!(re.items.len(), 1);
        assert_eq!(re.items[0].attempts, 1);
    }

    #[tokio::test]
    async fn nack_drop_removes_row() {
        let queue = InMemoryRetainQueue::new();
        queue.enqueue(item("a")).await.unwrap();
        let claim = queue.claim_batch(10, LEASE).await.unwrap();

        queue
            .nack(&claim.claim_token, claim.items[0].id, false, "client")
            .await
            .unwrap();
        assert_eq!(queue.stats().await.unwrap().count, 0);
    }

    #[tokio::test]
    async fn stale_rows_are_dropped_at_claim_time() {
        let queue = InMemoryRetainQueue::new();
        queue.enqueue(item("fresh")).await.unwrap();
        {
            let mut inner = queue.lock();
            let stale_at = (chrono::Utc::now() - MAX_AGE - chrono::Duration::hours(1))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            inner.rows.push_back(MemRow {
                id: 999,
                content: "stale".to_owned(),
                context: None,
                document_id: None,
                update_mode: None,
                tags: None,
                created_at: stale_at,
                attempts: 0,
                last_attempt_at: None,
                last_error: None,
                claim_token: None,
                claim_expires_at: None,
            });
        }

        let claim = queue.claim_batch(10, LEASE).await.unwrap();
        assert_eq!(claim.dropped_age, 1);
        assert_eq!(claim.items.len(), 1);
        assert_eq!(claim.items[0].content, "fresh");
    }
}
