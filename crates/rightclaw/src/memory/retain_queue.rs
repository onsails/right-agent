//! SQLite-backed queue of pending retain calls, drained by the bot.

use std::time::Duration;

use rusqlite::{params, Connection};

use super::MemoryError;

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

/// Enqueue a retain attempt for later drain. Evicts the oldest row if cap exceeded.
/// Uses a single transaction for the combined eviction + insert when needed.
pub fn enqueue(
    conn: &Connection,
    source: &str,
    content: &str,
    context: Option<&str>,
    document_id: Option<&str>,
    update_mode: Option<&str>,
    tags: Option<&[String]>,
) -> Result<(), MemoryError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pending_retains",
        [],
        |r| r.get(0),
    )?;

    let tx = conn.unchecked_transaction()?;

    if count as usize >= QUEUE_CAP {
        tx.execute(
            "DELETE FROM pending_retains WHERE id = (SELECT id FROM pending_retains ORDER BY created_at ASC LIMIT 1)",
            [],
        )?;
    }

    let tags_json = tags
        .map(|t| serde_json::to_string(t).unwrap_or_else(|_| "[]".into()));
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

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
    let iso: Option<String> = conn.query_row(
        "SELECT MIN(created_at) FROM pending_retains",
        [],
        |r| r.get(0),
    )?;
    let Some(iso) = iso else { return Ok(None) };
    let parsed = chrono::DateTime::parse_from_rfc3339(&iso).map_err(|e| {
        MemoryError::HindsightOther(format!("oldest_age parse: {e:#}"))
    })?;
    let now = chrono::Utc::now();
    let dur = now.signed_duration_since(parsed.with_timezone(&chrono::Utc));
    Ok(Some(Duration::from_secs(dur.num_seconds().max(0) as u64)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::open_connection;
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
        enqueue(&conn, "bot", "content", Some("ctx"), Some("doc"), Some("append"), None).unwrap();
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
        let oldest_content: String = conn.query_row(
            "SELECT content FROM pending_retains ORDER BY created_at ASC LIMIT 1",
            [], |r| r.get(0),
        ).unwrap();
        assert!(oldest_content.starts_with("content-"), "got {oldest_content}");
        // The first inserted entry ("content-0") must be evicted.
        let first_gone: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pending_retains WHERE content = 'content-0'",
            [], |r| r.get(0),
        ).unwrap();
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
        let json: String = conn.query_row(
            "SELECT tags_json FROM pending_retains LIMIT 1",
            [], |r| r.get(0),
        ).unwrap();
        let parsed: Vec<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tags);
    }
}
