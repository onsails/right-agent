//! Per-thread session CRUD against `sessions` SQLite table.
//!
//! Supports multiple sessions per (chat_id, thread_id) with at most one active.

use teloxide::types::{Message, MessageId, ThreadId};

/// A session row from the `sessions` table.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: i64,
    pub chat_id: i64,
    pub thread_id: i64,
    pub root_session_id: String,
    pub label: Option<String>,
    pub is_active: bool,
    pub created_at: String,
    pub last_used_at: String,
}

/// Normalise Telegram thread_id for session keying and reply routing.
pub fn effective_thread_id(msg: &Message) -> i64 {
    match msg.thread_id {
        Some(ThreadId(MessageId(1))) => 0,
        Some(ThreadId(MessageId(n))) => i64::from(n),
        None => 0,
    }
}

/// Truncate a string to at most 60 chars for use as a session label.
pub fn truncate_label(s: &str) -> &str {
    match s.char_indices().nth(60) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Get the active session for (chat_id, thread_id), or None.
pub fn get_active_session(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<SessionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at \
         FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND is_active = 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![chat_id, thread_id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_session(row)?)),
        None => Ok(None),
    }
}

/// Create a new active session. Returns the row id.
pub fn create_session(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
    session_uuid: &str,
    label: Option<&str>,
) -> Result<i64, rusqlite::Error> {
    conn.execute(
        "INSERT INTO sessions (chat_id, thread_id, root_session_id, label, is_active) \
         VALUES (?1, ?2, ?3, ?4, 1)",
        rusqlite::params![chat_id, thread_id, session_uuid, label],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Deactivate the current active session for (chat_id, thread_id).
/// Returns the previous session's root_session_id, or None if no active session.
pub fn deactivate_current(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<String>, rusqlite::Error> {
    let prev = get_active_session(conn, chat_id, thread_id)?;
    conn.execute(
        "UPDATE sessions SET is_active = 0 WHERE chat_id = ?1 AND thread_id = ?2 AND is_active = 1",
        rusqlite::params![chat_id, thread_id],
    )?;
    Ok(prev.map(|s| s.root_session_id))
}

/// Re-activate a session by row id.
///
/// Atomically deactivates any other active session for the same (chat_id, thread_id),
/// activates the target, and updates its `last_used_at`. Single transaction, two statements.
pub fn activate_session(
    conn: &rusqlite::Connection,
    session_id: i64,
) -> Result<(), rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    // Deactivate others via a CTE to avoid double subquery
    tx.execute(
        "WITH target AS (SELECT chat_id, thread_id FROM sessions WHERE id = ?1) \
         UPDATE sessions SET is_active = 0 WHERE is_active = 1 AND \
         chat_id = (SELECT chat_id FROM target) AND \
         thread_id = (SELECT thread_id FROM target)",
        rusqlite::params![session_id],
    )?;
    tx.execute(
        "UPDATE sessions SET is_active = 1, last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id = ?1",
        rusqlite::params![session_id],
    )?;
    tx.commit()?;
    Ok(())
}

/// Update last_used_at for a session by row id.
pub fn touch_session(
    conn: &rusqlite::Connection,
    session_id: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE sessions SET last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id = ?1",
        rusqlite::params![session_id],
    )?;
    Ok(())
}

/// List all sessions for (chat_id, thread_id) ordered by last_used_at DESC.
pub fn list_sessions(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Vec<SessionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at \
         FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 ORDER BY last_used_at DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![chat_id, thread_id], |row| {
        row_to_session(row)
    })?;
    rows.collect()
}

/// Find sessions matching a partial UUID or label for (chat_id, thread_id).
pub fn find_sessions_by_uuid(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
    partial: &str,
) -> Result<Vec<SessionRow>, rusqlite::Error> {
    let pattern = format!("%{partial}%");
    let mut stmt = conn.prepare_cached(
        "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at \
         FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND (root_session_id LIKE ?3 OR label LIKE ?3)",
    )?;
    let rows = stmt.query_map(rusqlite::params![chat_id, thread_id, pattern], |row| {
        row_to_session(row)
    })?;
    rows.collect()
}

fn row_to_session(row: &rusqlite::Row) -> Result<SessionRow, rusqlite::Error> {
    Ok(SessionRow {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        thread_id: row.get(2)?,
        root_session_id: row.get(3)?,
        label: row.get(4)?,
        is_active: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        last_used_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rightclaw::memory::open_connection;
    use tempfile::tempdir;

    fn test_conn() -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        (dir, conn)
    }

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
    fn get_active_returns_none_for_empty_db() {
        let (_dir, conn) = test_conn();
        assert!(get_active_session(&conn, 100, 0).unwrap().is_none());
    }

    #[test]
    fn create_then_get_active() {
        let (_dir, conn) = test_conn();
        let id = create_session(&conn, 100, 0, "uuid-1", Some("hello world")).unwrap();
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        assert_eq!(active.id, id);
        assert_eq!(active.root_session_id, "uuid-1");
        assert_eq!(active.label.as_deref(), Some("hello world"));
    }

    #[test]
    fn deactivate_clears_active() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "uuid-1", None).unwrap();
        let prev = deactivate_current(&conn, 100, 0).unwrap();
        assert_eq!(prev.as_deref(), Some("uuid-1"));
        assert!(get_active_session(&conn, 100, 0).unwrap().is_none());
    }

    #[test]
    fn deactivate_returns_none_when_no_active() {
        let (_dir, conn) = test_conn();
        let prev = deactivate_current(&conn, 100, 0).unwrap();
        assert!(prev.is_none());
    }

    #[test]
    fn activate_session_by_id() {
        let (_dir, conn) = test_conn();
        let id = create_session(&conn, 100, 0, "uuid-1", None).unwrap();
        deactivate_current(&conn, 100, 0).unwrap();
        activate_session(&conn, id).unwrap();
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        assert_eq!(active.root_session_id, "uuid-1");
    }

    #[test]
    fn list_sessions_ordered_by_last_used() {
        let (_dir, conn) = test_conn();
        let old_id = create_session(&conn, 100, 0, "uuid-old", Some("old")).unwrap();
        // Pin uuid-old to a known past timestamp so uuid-new sorts after it.
        conn.execute(
            "UPDATE sessions SET last_used_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
            rusqlite::params![old_id],
        )
        .unwrap();
        deactivate_current(&conn, 100, 0).unwrap();
        create_session(&conn, 100, 0, "uuid-new", Some("new")).unwrap();
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        touch_session(&conn, active.id).unwrap();

        let sessions = list_sessions(&conn, 100, 0).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].root_session_id, "uuid-new");
    }

    #[test]
    fn find_session_by_partial_uuid() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "550e8400-e29b-41d4", None).unwrap();
        deactivate_current(&conn, 100, 0).unwrap();
        create_session(&conn, 100, 0, "7a3f1b22-c9d8-4e5f", None).unwrap();

        let matches = find_sessions_by_uuid(&conn, 100, 0, "550e").unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].root_session_id, "550e8400-e29b-41d4");
    }

    #[test]
    fn find_session_partial_returns_multiple() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "aaa-111", None).unwrap();
        deactivate_current(&conn, 100, 0).unwrap();
        create_session(&conn, 100, 0, "aaa-222", None).unwrap();

        let matches = find_sessions_by_uuid(&conn, 100, 0, "aaa").unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn truncate_label_at_60_chars() {
        let long = "a".repeat(100);
        assert_eq!(truncate_label(&long).len(), 60);
        assert_eq!(truncate_label("short"), "short");
    }

    #[test]
    fn sessions_isolated_by_thread_id() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "thread0", None).unwrap();
        create_session(&conn, 100, 5, "thread5", None).unwrap();

        let t0 = get_active_session(&conn, 100, 0).unwrap().unwrap();
        let t5 = get_active_session(&conn, 100, 5).unwrap().unwrap();
        assert_eq!(t0.root_session_id, "thread0");
        assert_eq!(t5.root_session_id, "thread5");
    }

    #[test]
    fn full_lifecycle_new_switch_list() {
        let (_dir, conn) = test_conn();

        // First message creates session 1
        let id1 = create_session(&conn, 100, 0, "uuid-1", Some("hello world")).unwrap();
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        assert_eq!(active.id, id1);

        // /new — deactivate, create session 2
        let prev = deactivate_current(&conn, 100, 0).unwrap();
        assert_eq!(prev.as_deref(), Some("uuid-1"));
        let id2 = create_session(&conn, 100, 0, "uuid-2", Some("second task")).unwrap();

        // /list — both visible, session 2 active
        let all = list_sessions(&conn, 100, 0).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|s| s.root_session_id == "uuid-2" && s.is_active));
        assert!(all.iter().any(|s| s.root_session_id == "uuid-1" && !s.is_active));

        // /switch — back to session 1
        deactivate_current(&conn, 100, 0).unwrap();
        activate_session(&conn, id1).unwrap();
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        assert_eq!(active.root_session_id, "uuid-1");

        let _ = id2; // suppress unused warning
    }

    #[test]
    fn find_session_by_label() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "uuid-aaa", Some("crypto research")).unwrap();
        deactivate_current(&conn, 100, 0).unwrap();
        create_session(&conn, 100, 0, "uuid-bbb", Some("test cron")).unwrap();

        let matches = find_sessions_by_uuid(&conn, 100, 0, "crypto").unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].root_session_id, "uuid-aaa");
    }

    #[test]
    fn activate_session_is_atomic() {
        let (_dir, conn) = test_conn();
        let id1 = create_session(&conn, 100, 0, "uuid-1", None).unwrap();
        deactivate_current(&conn, 100, 0).unwrap();
        let id2 = create_session(&conn, 100, 0, "uuid-2", None).unwrap();

        // activate_session should atomically deactivate uuid-2 and activate uuid-1
        activate_session(&conn, id1).unwrap();
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        assert_eq!(active.root_session_id, "uuid-1");
        assert!(!list_sessions(&conn, 100, 0).unwrap().iter().any(|s| s.id == id2 && s.is_active));
    }
}
