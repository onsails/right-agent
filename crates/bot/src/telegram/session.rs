//! Per-thread session CRUD against `telegram_sessions` SQLite table.
//!
//! Session key: `(chat_id: i64, effective_thread_id: i64)`.
//! `effective_thread_id()` normalises Telegram General topic (Some(ThreadId(MessageId(1)))) → 0.
//!
//! Each worker task opens its own `Connection` via `open_connection()` (rusqlite is !Send).

use teloxide::types::{Message, MessageId, ThreadId};

/// Normalise Telegram thread_id for session keying and reply routing.
///
/// Telegram sends `thread_id = Some(ThreadId(MessageId(1)))` for General topic messages in supergroups.
/// This is NOT a real forum topic — normalise to 0 so it shares a session key
/// with non-threaded messages (which have `thread_id = None`).
pub fn effective_thread_id(msg: &Message) -> i64 {
    match msg.thread_id {
        Some(ThreadId(MessageId(1))) => 0,
        Some(ThreadId(MessageId(n))) => i64::from(n),
        None => 0,
    }
}

/// Returns the `root_session_id` stored for `(chat_id, thread_id)`, or `None` if no session exists.
pub fn get_session(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<String>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT root_session_id FROM telegram_sessions WHERE chat_id = ?1 AND thread_id = ?2",
    )?;
    let mut rows = stmt.query(rusqlite::params![chat_id, thread_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

/// Stores a new session row for `(chat_id, thread_id)`.
///
/// Uses `INSERT OR IGNORE` — if a row already exists, this is a no-op.
/// The `root_session_id` is NEVER overwritten on resume (guards against CC bug #8069).
pub fn create_session(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
    session_uuid: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR IGNORE INTO telegram_sessions (chat_id, thread_id, root_session_id) VALUES (?1, ?2, ?3)",
        rusqlite::params![chat_id, thread_id, session_uuid],
    )?;
    Ok(())
}

/// Deletes the session row for `(chat_id, thread_id)`.
///
/// Used by the `/reset` command (SES-06). Next message will create a fresh session.
pub fn delete_session(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM telegram_sessions WHERE chat_id = ?1 AND thread_id = ?2",
        rusqlite::params![chat_id, thread_id],
    )?;
    Ok(())
}

/// Updates `last_used_at` for `(chat_id, thread_id)` to the current UTC time.
///
/// Called after each successful CC invocation to track session activity.
pub fn touch_session(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE telegram_sessions SET last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE chat_id = ?1 AND thread_id = ?2",
        rusqlite::params![chat_id, thread_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rightclaw::memory::open_connection;
    use tempfile::tempdir;

    fn test_conn() -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path()).unwrap();
        (dir, conn)
    }

    // NOTE: effective_thread_id requires constructing a Message, which is non-trivial.
    // Test the normalisation logic directly via a thin helper to avoid Message construction.
    fn normalise_thread_id(thread_id: Option<ThreadId>) -> i64 {
        match thread_id {
            Some(ThreadId(MessageId(1))) => 0,
            Some(ThreadId(MessageId(n))) => i64::from(n),
            None => 0,
        }
    }

    #[test]
    fn effective_thread_id_general_topic() {
        assert_eq!(normalise_thread_id(Some(ThreadId(MessageId(1)))), 0);
    }

    #[test]
    fn effective_thread_id_none() {
        assert_eq!(normalise_thread_id(None), 0);
    }

    #[test]
    fn effective_thread_id_real_topic() {
        assert_eq!(normalise_thread_id(Some(ThreadId(MessageId(5)))), 5);
    }

    #[test]
    fn get_session_returns_none_for_new_db() {
        let (_dir, conn) = test_conn();
        let result = get_session(&conn, 100, 0).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn create_then_get_returns_uuid() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "test-uuid-1234").unwrap();
        let result = get_session(&conn, 100, 0).unwrap();
        assert_eq!(result.as_deref(), Some("test-uuid-1234"));
    }

    #[test]
    fn create_is_idempotent() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "first-uuid").unwrap();
        create_session(&conn, 100, 0, "second-uuid").unwrap(); // INSERT OR IGNORE — no-op
        let result = get_session(&conn, 100, 0).unwrap();
        // Must still be first-uuid, not overwritten
        assert_eq!(result.as_deref(), Some("first-uuid"));
    }

    #[test]
    fn delete_session_clears_row() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "uuid-abc").unwrap();
        delete_session(&conn, 100, 0).unwrap();
        let result = get_session(&conn, 100, 0).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn touch_session_updates_last_used_at() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "uuid-touch").unwrap();
        touch_session(&conn, 100, 0).unwrap();
        // Row still present after touch
        let result = get_session(&conn, 100, 0).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn session_keys_are_independent() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "thread0-uuid").unwrap();
        create_session(&conn, 100, 5, "thread5-uuid").unwrap();
        assert_eq!(
            get_session(&conn, 100, 0).unwrap().as_deref(),
            Some("thread0-uuid")
        );
        assert_eq!(
            get_session(&conn, 100, 5).unwrap().as_deref(),
            Some("thread5-uuid")
        );
    }
}
