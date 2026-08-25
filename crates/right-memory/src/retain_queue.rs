//! SQLite-backed queue of pending retain calls.
//!
//! The queue is owned by the Aggregator's `AgentDbOwner`; these functions are
//! the scoped SQL primitives the owner executes. Drainers never touch this
//! module directly — they use the lease-based
//! [`crate::retain_sink::RetainLeaseQueue`] interface and [`drain_claimed`].

use std::time::Duration;

use right_db::{Connection, Transaction, params};

use super::classify::ErrorKind;
use super::error::MemoryError;
use super::retain_sink::{RetainClaim, RetainLeaseQueue};

pub const QUEUE_CAP: usize = 1000;

/// A queued retain payload (mirrors `HindsightClient::retain_many` item inputs).
#[derive(Debug, Clone)]
pub struct PendingRetain {
    pub id: i64,
    pub content: String,
    pub context: Option<String>,
    pub document_id: Option<String>,
    pub update_mode: Option<String>,
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub attempts: i64,
}

/// Enqueue a retain attempt for later drain. Evicts oldest rows if cap exceeded.
/// Cap enforcement and insert happen atomically in one transaction so concurrent
/// enqueuers can never blow past the cap.
pub async fn enqueue(
    conn: &Connection,
    source: &str,
    content: &str,
    context: Option<&str>,
    document_id: Option<&str>,
    update_mode: Option<&str>,
    tags: Option<&[String]>,
) -> Result<(), MemoryError> {
    let tx = conn.transaction().await?;
    enqueue_in_transaction(
        &tx,
        source,
        content,
        context,
        document_id,
        update_mode,
        tags,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Enqueue within a caller-owned transaction without committing it.
///
/// This lets an owner atomically persist the queue row together with its
/// idempotency response record.
pub async fn enqueue_in_transaction(
    tx: &Transaction<'_>,
    source: &str,
    content: &str,
    context: Option<&str>,
    document_id: Option<&str>,
    update_mode: Option<&str>,
    tags: Option<&[String]>,
) -> Result<(), MemoryError> {
    let tags_json = tags
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| MemoryError::HindsightOther(format!("tags_json: {e:#}")))?;
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    reclaim_expired(tx).await?;
    // Delete (count - (cap - 1)) oldest rows if over-cap, so we're at cap-1 before insert.
    tx.execute(
        "DELETE FROM pending_retains WHERE id IN (
            SELECT id FROM pending_retains ORDER BY created_at ASC
                LIMIT MAX(0, (SELECT COUNT(*) FROM pending_retains) - ?1)
         )",
        [(QUEUE_CAP as i64) - 1],
    )
    .await?;

    tx.execute(
        "INSERT INTO pending_retains
            (content, context, document_id, update_mode, tags_json, created_at, attempts, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
        params![
            content,
            context,
            document_id,
            update_mode,
            tags_json,
            created_at,
            source,
        ],
    )
    .await?;
    Ok(())
}

/// Current row count.
pub async fn count(conn: &Connection) -> Result<usize, MemoryError> {
    let n: i64 = conn
        .query_one("SELECT COUNT(*) FROM pending_retains", (), |r| r.get(0))
        .await?;
    Ok(n as usize)
}

/// Age of the oldest row (None if queue empty).
pub async fn oldest_age(conn: &Connection) -> Result<Option<Duration>, MemoryError> {
    let iso: Option<String> = conn
        .query_one("SELECT MIN(created_at) FROM pending_retains", (), |r| {
            r.get(0)
        })
        .await?;
    let Some(iso) = iso else { return Ok(None) };
    let parsed = chrono::DateTime::parse_from_rfc3339(&iso)
        .map_err(|e| MemoryError::HindsightOther(format!("oldest_age parse: {e:#}")))?;
    let now = chrono::Utc::now();
    let dur = now.signed_duration_since(parsed.with_timezone(&chrono::Utc));
    Ok(Some(Duration::from_secs(dur.num_seconds().max(0) as u64)))
}

pub const DRAIN_BATCH: usize = 20;
pub const MAX_AGE: chrono::Duration = chrono::Duration::hours(24);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DrainReport {
    pub deleted: usize,         // successfully retained + removed
    pub dropped_age: usize,     // removed due to 24h age cap
    pub dropped_client: usize,  // removed due to Client-kind error
    pub bumped_attempts: usize, // attempts incremented (Transient/RateLimited/Malformed)
}

/// Lease time granted to one drain claim. Generous enough for a slow upstream
/// (20 items × ~10s each is well under this); a crashed drainer's rows become
/// reclaimable once it lapses.
pub const CLAIM_LEASE_TTL: Duration = Duration::from_secs(900);

fn new_claim_token() -> String {
    format!("{:016x}-{:016x}", fastrand::u64(..), fastrand::u64(..))
}

/// Atomically reclaim expired leases, drop stale rows, and lease up to
/// `limit` oldest unclaimed rows under a fresh claim token.
///
/// Runs in one immediate transaction (current transaction boundary: the
/// owner serializes all operations for an agent on one connection, so the
/// claim is race-free by construction).
pub async fn claim_batch(
    conn: &Connection,
    limit: usize,
    lease_ttl: Duration,
) -> Result<RetainClaim, MemoryError> {
    let now = chrono::Utc::now();
    let lease_expires_at = (now + lease_ttl).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let claim_token = new_claim_token();

    let tx = conn.transaction().await?;
    let result: Result<RetainClaim, MemoryError> = async {
        reclaim_expired(&tx).await?;

        let candidates = load_unclaimed(&tx, limit).await?;
        let mut dropped_age = 0usize;
        let mut items = Vec::new();
        for entry in candidates {
            // Age cap. Unparseable timestamps are not the same as old rows:
            // lease them like fresh rows rather than silently evicting.
            let stale = chrono::DateTime::parse_from_rfc3339(&entry.created_at)
                .map(|dt| now.signed_duration_since(dt.with_timezone(&chrono::Utc)) > MAX_AGE)
                .unwrap_or(false);
            if stale {
                tx.execute("DELETE FROM pending_retains WHERE id = ?1", [entry.id])
                    .await?;
                tracing::warn!(id = entry.id, "retain dropped: >24h");
                dropped_age += 1;
            } else {
                tx.execute(
                    "UPDATE pending_retains SET claim_token = ?1, claim_expires_at = ?2 \
                     WHERE id = ?3 AND claim_token IS NULL",
                    params![claim_token.as_str(), lease_expires_at.as_str(), entry.id],
                )
                .await?;
                items.push(entry);
            }
        }
        Ok(RetainClaim {
            claim_token,
            lease_expires_at,
            items,
            dropped_age,
        })
    }
    .await;
    match result {
        Ok(claim) => {
            tx.commit().await?;
            Ok(claim)
        }
        Err(err) => {
            if let Err(rollback_err) = tx.rollback().await {
                tracing::warn!(
                    operation_error = format!("{err:#}"),
                    rollback_error = format!("{rollback_err:#}"),
                    "retain claim rollback failed; returning original claim error",
                );
            }
            Err(err)
        }
    }
}

/// Clear leases whose expiry has passed. Called inside [`claim_batch`] and
/// available standalone for owner startup recovery. Returns rows reclaimed.
pub async fn reclaim_expired(conn: &Connection) -> Result<usize, MemoryError> {
    let now_s = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let n = conn
        .execute(
            "UPDATE pending_retains SET claim_token = NULL, claim_expires_at = NULL \
             WHERE claim_expires_at IS NOT NULL AND claim_expires_at <= ?1",
            [now_s.as_str()],
        )
        .await?;
    Ok(n as usize)
}

/// Delete row `id`, guarded by `claim_token`. A stale token (lease expired
/// and reclaimed by a newer claim) matches no row and is a conflict.
pub async fn ack(conn: &Connection, claim_token: &str, id: i64) -> Result<(), MemoryError> {
    let rows = conn
        .execute(
            "DELETE FROM pending_retains WHERE id = ?1 AND claim_token = ?2",
            params![id, claim_token],
        )
        .await?;
    if rows == 0 {
        return Err(MemoryError::LeaseConflict(format!(
            "ack id {id}: claim token no longer holds the lease"
        )));
    }
    Ok(())
}

/// Release row `id` back to the queue (`retry = true`, bumping attempts) or
/// drop it permanently (`retry = false`), guarded by `claim_token`.
pub async fn nack(
    conn: &Connection,
    claim_token: &str,
    id: i64,
    retry: bool,
    error: &str,
) -> Result<(), MemoryError> {
    let rows = if retry {
        let now_s = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE pending_retains SET claim_token = NULL, claim_expires_at = NULL, \
                 attempts = attempts + 1, last_attempt_at = ?1, last_error = ?2 \
             WHERE id = ?3 AND claim_token = ?4",
            params![now_s, error, id, claim_token],
        )
        .await?
    } else {
        conn.execute(
            "DELETE FROM pending_retains WHERE id = ?1 AND claim_token = ?2",
            params![id, claim_token],
        )
        .await?
    };
    if rows == 0 {
        return Err(MemoryError::LeaseConflict(format!(
            "nack id {id}: claim token no longer holds the lease"
        )));
    }
    Ok(())
}

async fn load_unclaimed(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<PendingRetain>, MemoryError> {
    conn.query_all(
        "SELECT id, content, context, document_id, update_mode, tags_json, created_at, attempts
           FROM pending_retains WHERE claim_token IS NULL ORDER BY created_at ASC LIMIT ?1",
        [limit as i64],
        pending_from_row,
    )
    .await
    .map_err(Into::into)
}

/// Run one drain tick against an injected lease queue.
///
/// `call` is invoked with a single-item batch. The closure returns
/// `Err(ErrorKind)` on failure (already classified by caller) or `Ok(())` on
/// success. Outcomes: success → ack; Client → drop via nack; Transient /
/// RateLimited / Malformed → retry-nack with attempts bump and stop the tick
/// (don't storm); Auth / Quota → stop (the rows stay leased and become
/// reclaimable when the lease lapses, matching the previous behaviour of
/// leaving them queued).
pub async fn drain_claimed<F, Fut>(queue: &dyn RetainLeaseQueue, mut call: F) -> DrainReport
where
    F: FnMut(Vec<PendingRetain>) -> Fut,
    Fut: Future<Output = Result<(), ErrorKind>>,
{
    let mut report = DrainReport::default();

    let claim = match queue.claim_batch(DRAIN_BATCH, CLAIM_LEASE_TTL).await {
        Ok(claim) => claim,
        Err(e) => {
            tracing::warn!("drain: claim_batch failed: {e:#}");
            return report;
        }
    };
    report.dropped_age = claim.dropped_age;
    if claim.items.is_empty() {
        return report;
    }
    let token = claim.claim_token;

    for entry in claim.items {
        match call(vec![entry.clone()]).await {
            Ok(()) => match queue.ack(&token, entry.id).await {
                Ok(()) => report.deleted += 1,
                Err(e) => {
                    tracing::error!(id = entry.id, error = %e, "drain: ack failed");
                }
            },
            Err(ErrorKind::Client) => {
                match queue
                    .nack(&token, entry.id, false, "classified_client")
                    .await
                {
                    Ok(()) => {
                        tracing::error!(id = entry.id, "retain dropped on 4xx: {entry:?}");
                        report.dropped_client += 1;
                    }
                    Err(e) => {
                        tracing::error!(id = entry.id, error = %e, "drain: client-drop nack failed");
                    }
                }
                continue;
            }
            Err(ErrorKind::Auth) => {
                // Should not happen (Auth never enqueues), but defensively stop.
                tracing::warn!(id = entry.id, "drain encountered Auth; stopping");
                break;
            }
            Err(ErrorKind::Quota) => {
                // Should not happen (Quota never enqueues). Defensive stop:
                // 402 will not self-heal until the user tops up.
                tracing::warn!(
                    id = entry.id,
                    "drain encountered Quota; stopping until quota is restored"
                );
                break;
            }
            Err(_) => {
                match queue
                    .nack(&token, entry.id, true, "classified_transient")
                    .await
                {
                    Ok(()) => report.bumped_attempts += 1,
                    Err(e) => {
                        tracing::error!(id = entry.id, error = %e, "drain: retry nack failed");
                    }
                }
                break; // don't storm
            }
        }
    }

    report
}

fn pending_from_row(row: &right_db::row::Row<'_>) -> Result<PendingRetain, right_db::DbError> {
    let tags_json: Option<String> = row.get(5)?;
    let tags = match tags_json {
        Some(s) => match serde_json::from_str::<Vec<String>>(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(error = %e, "pending_retain tags_json parse failed; treating as None");
                None
            }
        },
        None => None,
    };
    Ok(PendingRetain {
        id: row.get(0)?,
        content: row.get(1)?,
        context: row.get(2)?,
        document_id: row.get(3)?,
        update_mode: row.get(4)?,
        tags,
        created_at: row.get(6)?,
        attempts: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retain_sink::{BoxFuture, RetainQueueStats};
    use right_db::open_connection;
    use tempfile::tempdir;

    async fn fresh_db() -> (tempfile::TempDir, Connection) {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        (dir, conn)
    }

    #[tokio::test]
    async fn enqueue_inserts_row() {
        let (_dir, conn) = fresh_db().await;
        enqueue(
            &conn,
            "bot",
            "content",
            Some("ctx"),
            Some("doc"),
            Some("append"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(count(&conn).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn enqueue_in_transaction_rolls_back_with_caller_transaction() {
        let (_dir, conn) = fresh_db().await;
        let tx = conn.transaction().await.unwrap();
        enqueue_in_transaction(
            &tx,
            "bot",
            "content",
            Some("ctx"),
            Some("doc"),
            Some("append"),
            None,
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();

        assert_eq!(count(&conn).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn enqueue_cap_evicts_oldest() {
        let (_dir, conn) = fresh_db().await;
        for i in 0..(QUEUE_CAP + 5) {
            let c = format!("content-{i}");
            enqueue(&conn, "bot", &c, None, None, None, None)
                .await
                .unwrap();
        }
        assert_eq!(count(&conn).await.unwrap(), QUEUE_CAP);
        // Oldest remaining rows should not include the first 5.
        let oldest_content: String = conn
            .query_one(
                "SELECT content FROM pending_retains ORDER BY created_at ASC LIMIT 1",
                (),
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert!(
            oldest_content.starts_with("content-"),
            "got {oldest_content}"
        );
        // The first inserted entry ("content-0") must be evicted.
        let first_gone: i64 = conn
            .query_one(
                "SELECT COUNT(*) FROM pending_retains WHERE content = 'content-0'",
                (),
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(first_gone, 0);
    }

    #[tokio::test]
    async fn oldest_age_returns_none_when_empty() {
        let (_dir, conn) = fresh_db().await;
        assert!(oldest_age(&conn).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn tags_serialize_as_json_array() {
        let (_dir, conn) = fresh_db().await;
        let tags = vec!["chat:42".to_string(), "user:7".to_string()];
        enqueue(&conn, "bot", "c", None, None, None, Some(&tags))
            .await
            .unwrap();
        let json: String = conn
            .query_one("SELECT tags_json FROM pending_retains LIMIT 1", (), |r| {
                r.get(0)
            })
            .await
            .unwrap();
        let parsed: Vec<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tags);
    }

    use crate::ErrorKind;

    #[derive(Default)]
    struct FakeOutcome {
        queue: std::sync::Mutex<std::collections::VecDeque<Option<ErrorKind>>>,
        calls: std::sync::Mutex<Vec<PendingRetain>>,
    }

    impl FakeOutcome {
        fn push(&self, outcome: Option<ErrorKind>) {
            self.queue.lock().unwrap().push_back(outcome);
        }
        fn next(&self, item: &PendingRetain) -> Result<(), ErrorKind> {
            self.calls.lock().unwrap().push(item.clone());
            match self.queue.lock().unwrap().pop_front().flatten() {
                None => Ok(()),
                Some(kind) => Err(kind),
            }
        }
    }

    /// Test adapter driving [`drain_claimed`] against the SQL lease primitives.
    #[derive(Debug)]
    struct SqlQueue(std::sync::Arc<Connection>);

    impl RetainLeaseQueue for SqlQueue {
        fn claim_batch(
            &self,
            limit: usize,
            lease_ttl: Duration,
        ) -> BoxFuture<'static, Result<RetainClaim, MemoryError>> {
            let conn = std::sync::Arc::clone(&self.0);
            Box::pin(async move { claim_batch(&conn, limit, lease_ttl).await })
        }

        fn ack(&self, claim_token: &str, id: i64) -> BoxFuture<'static, Result<(), MemoryError>> {
            let conn = std::sync::Arc::clone(&self.0);
            let token = claim_token.to_owned();
            Box::pin(async move { ack(&conn, &token, id).await })
        }

        fn nack(
            &self,
            claim_token: &str,
            id: i64,
            retry: bool,
            error: &str,
        ) -> BoxFuture<'static, Result<(), MemoryError>> {
            let conn = std::sync::Arc::clone(&self.0);
            let token = claim_token.to_owned();
            let error = error.to_owned();
            Box::pin(async move { nack(&conn, &token, id, retry, &error).await })
        }

        fn stats(&self) -> BoxFuture<'static, Result<RetainQueueStats, MemoryError>> {
            let conn = std::sync::Arc::clone(&self.0);
            Box::pin(async move {
                Ok(RetainQueueStats {
                    count: count(&conn).await?,
                    oldest_age: oldest_age(&conn).await?,
                })
            })
        }
    }

    fn sql_queue(conn: Connection) -> SqlQueue {
        SqlQueue(std::sync::Arc::new(conn))
    }

    #[tokio::test]
    async fn drain_success_deletes_entry() {
        let (_dir, conn) = fresh_db().await;
        enqueue(&conn, "bot", "c1", None, None, None, None)
            .await
            .unwrap();
        let queue = sql_queue(conn);
        let fake = FakeOutcome::default();
        fake.push(None);

        let report = drain_claimed(&queue, |items| {
            let kind = fake.next(&items[0]);
            async move { kind }
        })
        .await;

        assert_eq!(report.deleted, 1);
        assert_eq!(count(&queue.0).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn drain_client_error_deletes_and_continues() {
        let (_dir, conn) = fresh_db().await;
        enqueue(&conn, "bot", "poison", None, None, None, None)
            .await
            .unwrap();
        enqueue(&conn, "bot", "good", None, None, None, None)
            .await
            .unwrap();
        let queue = sql_queue(conn);
        let fake = FakeOutcome::default();
        fake.push(Some(ErrorKind::Client));
        fake.push(None);

        let report = drain_claimed(&queue, |items| {
            let kind = fake.next(&items[0]);
            async move { kind }
        })
        .await;

        assert_eq!(report.dropped_client, 1);
        assert_eq!(report.deleted, 1);
        assert_eq!(count(&queue.0).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn drain_transient_updates_attempts_and_breaks() {
        let (_dir, conn) = fresh_db().await;
        enqueue(&conn, "bot", "first", None, None, None, None)
            .await
            .unwrap();
        enqueue(&conn, "bot", "second", None, None, None, None)
            .await
            .unwrap();
        let queue = sql_queue(conn);
        let fake = FakeOutcome::default();
        fake.push(Some(ErrorKind::Transient));

        let report = drain_claimed(&queue, |items| {
            let kind = fake.next(&items[0]);
            async move { kind }
        })
        .await;

        assert_eq!(report.deleted, 0);
        assert_eq!(report.bumped_attempts, 1);
        let attempts: i64 = queue
            .0
            .query_one(
                "SELECT attempts FROM pending_retains WHERE content = 'first'",
                (),
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(attempts, 1);
        // The retry-nacked row is immediately reclaimable. The untouched
        // remainder of the already-claimed batch stays leased until expiry,
        // preventing another drainer from duplicating it.
        let reclaim = claim_batch(&queue.0, 10, CLAIM_LEASE_TTL).await.unwrap();
        assert_eq!(reclaim.items.len(), 1);
    }

    #[tokio::test]
    async fn drain_age_cap_drops_stale_rows() {
        let (_dir, conn) = fresh_db().await;
        enqueue(&conn, "bot", "old", None, None, None, None)
            .await
            .unwrap();
        // Overwrite created_at with a real RFC3339 timestamp 48h in the past so the
        // parser accepts it (SQLite's datetime() format is not RFC3339 and would
        // fail to parse, which would fall through to the call path).
        let t = (chrono::Utc::now() - chrono::Duration::hours(48))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        conn.execute("UPDATE pending_retains SET created_at = ?1", [t])
            .await
            .unwrap();

        let queue = sql_queue(conn);
        let report = drain_claimed(&queue, |_items| async move {
            panic!("should not call upstream for stale entries");
        })
        .await;

        assert_eq!(report.dropped_age, 1);
        assert_eq!(count(&queue.0).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn claim_excludes_leased_rows_until_expiry() {
        let (_dir, conn) = fresh_db().await;
        enqueue(&conn, "bot", "a", None, None, None, None)
            .await
            .unwrap();

        let first = claim_batch(&conn, 10, Duration::from_millis(20))
            .await
            .unwrap();
        assert_eq!(first.items.len(), 1);

        let second = claim_batch(&conn, 10, CLAIM_LEASE_TTL).await.unwrap();
        assert!(
            second.items.is_empty(),
            "leased rows must not be claimed twice"
        );

        // Crash-after-claim: no ack. After the lease lapses the next claim
        // reclaims the row under a fresh token.
        tokio::time::sleep(Duration::from_millis(40)).await;
        let reclaimed = claim_batch(&conn, 10, CLAIM_LEASE_TTL).await.unwrap();
        assert_eq!(reclaimed.items.len(), 1);
        assert_ne!(reclaimed.claim_token, first.claim_token);

        // The stale token can neither ack nor nack.
        let stale_ack = ack(&conn, &first.claim_token, first.items[0].id).await;
        assert!(
            matches!(stale_ack, Err(MemoryError::LeaseConflict(_))),
            "stale ack must conflict: {stale_ack:?}"
        );
        let stale_nack = nack(&conn, &first.claim_token, first.items[0].id, true, "stale").await;
        assert!(
            matches!(stale_nack, Err(MemoryError::LeaseConflict(_))),
            "stale nack must conflict: {stale_nack:?}"
        );

        // The fresh token still owns the row.
        ack(&conn, &reclaimed.claim_token, reclaimed.items[0].id)
            .await
            .unwrap();
        assert_eq!(count(&conn).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn reclaim_expired_releases_only_expired_leases() {
        let (_dir, conn) = fresh_db().await;
        enqueue(&conn, "bot", "short", None, None, None, None)
            .await
            .unwrap();
        enqueue(&conn, "bot", "long", None, None, None, None)
            .await
            .unwrap();

        // Lease the short row with a short TTL and the long row with a long TTL.
        let short = claim_batch(&conn, 1, Duration::from_millis(20))
            .await
            .unwrap();
        let long = claim_batch(&conn, 1, CLAIM_LEASE_TTL).await.unwrap();
        assert_eq!(short.items.len(), 1);
        assert_eq!(long.items.len(), 1);

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(reclaim_expired(&conn).await.unwrap(), 1);

        let next = claim_batch(&conn, 10, CLAIM_LEASE_TTL).await.unwrap();
        assert_eq!(next.items.len(), 1);
        assert_eq!(next.items[0].content, "short");
    }
    #[tokio::test]
    async fn drain_does_not_hold_queue_lock_across_upstream_await() {
        use crate::retain_sink::{InMemoryRetainQueue, NewPendingRetain, PendingRetainSink};

        let queue = InMemoryRetainQueue::new();
        let item = |content: &str| NewPendingRetain {
            source: "bot".to_owned(),
            content: content.to_owned(),
            context: None,
            document_id: None,
            update_mode: None,
            tags: None,
        };
        queue.enqueue(item("first")).await.unwrap();

        let (tx_unblock, rx_unblock) = tokio::sync::oneshot::channel::<()>();
        let (tx_entered, rx_entered) = tokio::sync::oneshot::channel::<()>();
        let mut tx_entered_opt = Some(tx_entered);
        let mut rx_unblock_opt = Some(rx_unblock);

        let drain_fut = drain_claimed(&queue, |_items| {
            let signal = tx_entered_opt.take();
            let wait = rx_unblock_opt.take();
            async move {
                if let Some(s) = signal {
                    let _ = s.send(());
                }
                if let Some(w) = wait {
                    let _ = w.await;
                }
                Ok(())
            }
        });

        let enqueue_fut = async {
            rx_entered.await.unwrap();
            // Must not wait on a queue lock held by the upstream call.
            queue.enqueue(item("concurrent")).await.unwrap();
            let _ = tx_unblock.send(());
        };

        let (report, _) = tokio::join!(drain_fut, enqueue_fut);
        assert_eq!(report.deleted, 1);
        assert_eq!(queue.stats().await.unwrap().count, 1);
    }
}
