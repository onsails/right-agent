//! SQLite-backed queue of pending retain calls, drained by the bot.

use std::future::Future;
use std::time::Duration;

use rusqlite::{Connection, params};

use super::{ErrorKind, MemoryError};

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
pub fn enqueue(
    conn: &Connection,
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

    let tx = conn.unchecked_transaction()?;

    // Delete (count - (cap - 1)) oldest rows if over-cap, so we're at cap-1 before insert.
    tx.execute(
        "DELETE FROM pending_retains WHERE id IN (
            SELECT id FROM pending_retains ORDER BY created_at ASC
                LIMIT MAX(0, (SELECT COUNT(*) FROM pending_retains) - ?1)
         )",
        [(QUEUE_CAP as i64) - 1],
    )?;

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
    )?;

    tx.commit()?;
    Ok(())
}

/// Current row count.
pub fn count(conn: &Connection) -> Result<usize, MemoryError> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0))?;
    Ok(n as usize)
}

/// Age of the oldest row (None if queue empty).
pub fn oldest_age(conn: &Connection) -> Result<Option<Duration>, MemoryError> {
    let iso: Option<String> =
        conn.query_row("SELECT MIN(created_at) FROM pending_retains", [], |r| {
            r.get(0)
        })?;
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

/// Run one drain tick.
///
/// `call` is invoked with a single-item batch. The closure returns
/// `Err(ErrorKind)` on failure (already classified by caller) or `Ok(())` on success.
pub async fn drain_tick<F, Fut>(conn: &Connection, mut call: F) -> DrainReport
where
    F: FnMut(Vec<PendingRetain>) -> Fut,
    Fut: Future<Output = Result<(), ErrorKind>>,
{
    let mut report = DrainReport::default();

    let batch = match load_batch(conn, DRAIN_BATCH) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("drain: load_batch failed: {e:#}");
            return report;
        }
    };
    if batch.is_empty() {
        return report;
    }

    let now = chrono::Utc::now();

    for entry in batch {
        // Age cap check. If created_at unparseable we log and fall through to the call
        // path rather than silently evicting (a bad timestamp is not the same as an old row).
        let created = match chrono::DateTime::parse_from_rfc3339(&entry.created_at) {
            Ok(dt) => Some(dt.with_timezone(&chrono::Utc)),
            Err(e) => {
                tracing::warn!(id = entry.id, error = %e, "drain: unparseable created_at");
                None
            }
        };
        if let Some(c) = created
            && now.signed_duration_since(c) > MAX_AGE
        {
            match conn.execute("DELETE FROM pending_retains WHERE id = ?1", [entry.id]) {
                Ok(_) => {
                    tracing::warn!(id = entry.id, "retain dropped: >24h");
                    report.dropped_age += 1;
                }
                Err(e) => {
                    tracing::error!(id = entry.id, error = %e, "drain: age-cap DELETE failed");
                }
            }
            continue;
        }

        match call(vec![entry.clone()]).await {
            Ok(()) => match conn.execute("DELETE FROM pending_retains WHERE id = ?1", [entry.id]) {
                Ok(_) => report.deleted += 1,
                Err(e) => {
                    tracing::error!(id = entry.id, error = %e, "drain: success DELETE failed");
                }
            },
            Err(ErrorKind::Client) => {
                match conn.execute("DELETE FROM pending_retains WHERE id = ?1", [entry.id]) {
                    Ok(_) => {
                        tracing::error!(id = entry.id, "retain dropped on 4xx: {entry:?}");
                        report.dropped_client += 1;
                    }
                    Err(e) => {
                        tracing::error!(id = entry.id, error = %e, "drain: client-drop DELETE failed");
                    }
                }
                continue;
            }
            Err(ErrorKind::Auth) => {
                // Should not happen (Auth never enqueues), but defensively stop.
                tracing::warn!(id = entry.id, "drain encountered Auth; stopping");
                break;
            }
            Err(_) => {
                if let Err(e) = conn.execute(
                    "UPDATE pending_retains SET attempts = attempts + 1, \
                       last_attempt_at = ?1, last_error = ?2 WHERE id = ?3",
                    params![now.to_rfc3339(), "classified_transient", entry.id],
                ) {
                    tracing::error!(id = entry.id, error = %e, "drain: attempts UPDATE failed");
                }
                report.bumped_attempts += 1;
                break; // don't storm
            }
        }
    }

    report
}

fn load_batch(conn: &Connection, limit: usize) -> Result<Vec<PendingRetain>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, content, context, document_id, update_mode, tags_json, created_at, attempts
           FROM pending_retains ORDER BY created_at ASC LIMIT ?1",
    )?;
    let rows: Result<Vec<PendingRetain>, rusqlite::Error> = stmt
        .query_map([limit as i64], |row| {
            let tags_json: Option<String> = row.get(5)?;
            let tags = match tags_json {
                Some(s) => match serde_json::from_str::<Vec<String>>(&s) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::warn!(error = %e, "drain: tags_json parse failed; treating as None");
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
        })?
        .collect();
    Ok(rows?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use right_db::open_connection;
    use tempfile::tempdir;

    fn fresh_db() -> Connection {
        let dir = tempdir().unwrap();
        // Keep the tempdir alive inside the test scope (leak is fine for test).
        let path = dir.keep();
        open_connection(&path, true).unwrap()
    }

    #[test]
    fn enqueue_inserts_row() {
        let conn = fresh_db();
        enqueue(
            &conn,
            "bot",
            "content",
            Some("ctx"),
            Some("doc"),
            Some("append"),
            None,
        )
        .unwrap();
        assert_eq!(count(&conn).unwrap(), 1);
    }

    #[test]
    fn enqueue_cap_evicts_oldest() {
        let conn = fresh_db();
        for i in 0..(QUEUE_CAP + 5) {
            let c = format!("content-{i}");
            enqueue(&conn, "bot", &c, None, None, None, None).unwrap();
        }
        assert_eq!(count(&conn).unwrap(), QUEUE_CAP);
        // Oldest remaining rows should not include the first 5.
        let oldest_content: String = conn
            .query_row(
                "SELECT content FROM pending_retains ORDER BY created_at ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            oldest_content.starts_with("content-"),
            "got {oldest_content}"
        );
        // The first inserted entry ("content-0") must be evicted.
        let first_gone: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pending_retains WHERE content = 'content-0'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(first_gone, 0);
    }

    #[test]
    fn oldest_age_returns_none_when_empty() {
        let conn = fresh_db();
        assert!(oldest_age(&conn).unwrap().is_none());
    }

    #[test]
    fn tags_serialize_as_json_array() {
        let conn = fresh_db();
        let tags = vec!["chat:42".to_string(), "user:7".to_string()];
        enqueue(&conn, "bot", "c", None, None, None, Some(&tags)).unwrap();
        let json: String = conn
            .query_row("SELECT tags_json FROM pending_retains LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let parsed: Vec<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tags);
    }

    use super::super::ErrorKind;

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

    #[tokio::test]
    async fn drain_success_deletes_entry() {
        let conn = fresh_db();
        enqueue(&conn, "bot", "c1", None, None, None, None).unwrap();
        let fake = FakeOutcome::default();
        fake.push(None);

        let report = drain_tick(&conn, |items| {
            let kind = fake.next(&items[0]);
            async move { kind }
        })
        .await;

        assert_eq!(report.deleted, 1);
        assert_eq!(count(&conn).unwrap(), 0);
    }

    #[tokio::test]
    async fn drain_client_error_deletes_and_continues() {
        let conn = fresh_db();
        enqueue(&conn, "bot", "poison", None, None, None, None).unwrap();
        enqueue(&conn, "bot", "good", None, None, None, None).unwrap();
        let fake = FakeOutcome::default();
        fake.push(Some(ErrorKind::Client));
        fake.push(None);

        let report = drain_tick(&conn, |items| {
            let kind = fake.next(&items[0]);
            async move { kind }
        })
        .await;

        assert_eq!(report.dropped_client, 1);
        assert_eq!(report.deleted, 1);
        assert_eq!(count(&conn).unwrap(), 0);
    }

    #[tokio::test]
    async fn drain_transient_updates_attempts_and_breaks() {
        let conn = fresh_db();
        enqueue(&conn, "bot", "first", None, None, None, None).unwrap();
        enqueue(&conn, "bot", "second", None, None, None, None).unwrap();
        let fake = FakeOutcome::default();
        fake.push(Some(ErrorKind::Transient));

        let report = drain_tick(&conn, |items| {
            let kind = fake.next(&items[0]);
            async move { kind }
        })
        .await;

        assert_eq!(report.deleted, 0);
        assert_eq!(report.bumped_attempts, 1);
        let attempts: i64 = conn
            .query_row(
                "SELECT attempts FROM pending_retains WHERE content = 'first'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 1);
        assert_eq!(count(&conn).unwrap(), 2);
    }

    #[tokio::test]
    async fn drain_age_cap_drops_stale_rows() {
        let conn = fresh_db();
        enqueue(&conn, "bot", "old", None, None, None, None).unwrap();
        // Overwrite created_at with a real RFC3339 timestamp 48h in the past so the
        // parser accepts it (SQLite's datetime() format is not RFC3339 and would
        // fail to parse, which would fall through to the call path).
        let t = (chrono::Utc::now() - chrono::Duration::hours(48))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        conn.execute("UPDATE pending_retains SET created_at = ?1", [t])
            .unwrap();

        let report = drain_tick(&conn, |_items| async move {
            panic!("should not call upstream for stale entries");
        })
        .await;

        assert_eq!(report.dropped_age, 1);
        assert_eq!(count(&conn).unwrap(), 0);
    }

    #[tokio::test]
    async fn drain_does_not_block_concurrent_enqueue() {
        // Verifies the drain loop does not hold a write lock across the closure await.
        // Without the fix, this test deadlocks (drain holds tx; enqueue waits on busy_timeout).
        let dir = tempdir().unwrap();
        let path = dir.keep();
        let drain_conn = right_db::open_connection(&path, true).unwrap();
        let enq_conn = right_db::open_connection(&path, false).unwrap();

        // Seed with one row to drain.
        enqueue(&drain_conn, "bot", "first", None, None, None, None).unwrap();

        // Trigger drain where the closure blocks on a oneshot until the concurrent
        // enqueue succeeds; if a tx was held, the enqueue would starve.
        let (tx_unblock, rx_unblock) = tokio::sync::oneshot::channel::<()>();
        let (tx_entered, rx_entered) = tokio::sync::oneshot::channel::<()>();
        let mut tx_entered_opt = Some(tx_entered);
        let mut rx_unblock_opt = Some(rx_unblock);

        let drain_fut = drain_tick(&drain_conn, |_items| {
            let signal = tx_entered_opt.take();
            let wait = rx_unblock_opt.take();
            async move {
                if let Some(s) = signal {
                    let _ = s.send(());
                }
                // Wait for the other task to finish its enqueue.
                if let Some(w) = wait {
                    let _ = w.await;
                }
                Ok(())
            }
        });

        let enqueue_fut = async move {
            // Wait until drain is mid-await.
            rx_entered.await.unwrap();
            // This must succeed even though drain_tick is suspended in its closure.
            enqueue(&enq_conn, "bot", "concurrent", None, None, None, None).unwrap();
            let _ = tx_unblock.send(());
        };

        let (report, _) = tokio::join!(drain_fut, enqueue_fut);

        assert_eq!(report.deleted, 1);
        // After drain: "first" gone, "concurrent" still enqueued
        assert_eq!(count(&drain_conn).unwrap(), 1);
    }
}
