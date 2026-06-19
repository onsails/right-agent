//! Per-thread session CRUD against `sessions` SQLite table.
//!
//! Supports multiple sessions per (chat_id, thread_id) with at most one active.

use frankenstein::types::Message;

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
    super::msg_ext::effective_thread_id(msg)
}

/// Truncate a string to at most 60 chars for use as a session label.
pub fn truncate_label(s: &str) -> &str {
    match s.char_indices().nth(60) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Get the active session for (chat_id, thread_id), or None.
pub async fn get_active_session(
    conn: &right_db::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<SessionRow>, right_db::DbError> {
    use right_db::OptionalExtension as _;
    conn.query_row(
        "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at \
         FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND is_active = 1 LIMIT 1",
        right_db::params![chat_id, thread_id],
        row_to_session,
    )
    .await
    .optional()
}

/// Create a new active session. Returns the row id.
pub async fn create_session(
    conn: &right_db::Connection,
    chat_id: i64,
    thread_id: i64,
    session_uuid: &str,
    label: Option<&str>,
) -> Result<i64, right_db::DbError> {
    conn.execute(
        "INSERT INTO sessions (chat_id, thread_id, root_session_id, label, is_active) \
         VALUES (?1, ?2, ?3, ?4, 1)",
        right_db::params![chat_id, thread_id, session_uuid, label],
    )
    .await?;
    Ok(conn.last_insert_rowid())
}

/// Deactivate the current active session for (chat_id, thread_id).
/// Returns the previous session's root_session_id, or None if no active session.
pub async fn deactivate_current(
    conn: &right_db::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<String>, right_db::DbError> {
    let prev = get_active_session(conn, chat_id, thread_id).await?;
    conn.execute(
        "UPDATE sessions SET is_active = 0 WHERE chat_id = ?1 AND thread_id = ?2 AND is_active = 1",
        right_db::params![chat_id, thread_id],
    )
    .await?;
    Ok(prev.map(|s| s.root_session_id))
}

/// Re-activate a session by row id.
///
/// Atomically deactivates any other active session for the same (chat_id, thread_id),
/// activates the target, and updates its `last_used_at`. Single transaction, two statements.
pub async fn activate_session(
    conn: &right_db::Connection,
    session_id: i64,
) -> Result<(), right_db::DbError> {
    let tx = conn.transaction().await?;
    // Deactivate others via a CTE to avoid double subquery
    tx.execute(
        "WITH target AS (SELECT chat_id, thread_id FROM sessions WHERE id = ?1) \
         UPDATE sessions SET is_active = 0 WHERE is_active = 1 AND \
         chat_id = (SELECT chat_id FROM target) AND \
         thread_id = (SELECT thread_id FROM target)",
        right_db::params![session_id],
    )
    .await?;
    tx.execute(
        "UPDATE sessions SET is_active = 1, last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id = ?1",
        right_db::params![session_id],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Update last_used_at for a session by row id.
pub async fn touch_session(
    conn: &right_db::Connection,
    session_id: i64,
) -> Result<(), right_db::DbError> {
    conn.execute(
        "UPDATE sessions SET last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id = ?1",
        right_db::params![session_id],
    )
    .await?;
    Ok(())
}

/// List all sessions for (chat_id, thread_id) ordered by last_used_at DESC.
pub async fn list_sessions(
    conn: &right_db::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Vec<SessionRow>, right_db::DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at \
         FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 ORDER BY last_used_at DESC",
    )?;
    let rows = stmt
        .query_map(right_db::params![chat_id, thread_id], |row| {
            row_to_session(row)
        })
        .await?;
    rows.collect()
}

/// Find sessions matching a partial UUID or label for (chat_id, thread_id).
pub async fn find_sessions_by_uuid(
    conn: &right_db::Connection,
    chat_id: i64,
    thread_id: i64,
    partial: &str,
) -> Result<Vec<SessionRow>, right_db::DbError> {
    let pattern = format!("%{partial}%");
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at \
         FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND (root_session_id LIKE ?3 OR label LIKE ?3)",
    )?;
    let rows = stmt
        .query_map(right_db::params![chat_id, thread_id, pattern], |row| {
            row_to_session(row)
        })
        .await?;
    rows.collect()
}

fn row_to_session(row: &right_db::row::Row) -> Result<SessionRow, right_db::DbError> {
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
    use right_db::open_connection;
    use tempfile::tempdir;

    async fn test_conn() -> (tempfile::TempDir, right_db::Connection) {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        (dir, conn)
    }

    fn normalise_thread_id(thread_id: Option<i32>) -> i64 {
        match thread_id {
            Some(1) | None => 0,
            Some(n) => i64::from(n),
        }
    }

    #[tokio::test]
    async fn effective_thread_id_general_topic() {
        assert_eq!(normalise_thread_id(Some(1)), 0);
    }

    #[tokio::test]
    async fn effective_thread_id_none() {
        assert_eq!(normalise_thread_id(None), 0);
    }

    #[tokio::test]
    async fn effective_thread_id_real_topic() {
        assert_eq!(normalise_thread_id(Some(5)), 5);
    }

    #[tokio::test]
    async fn get_active_returns_none_for_empty_db() {
        let (_dir, conn) = test_conn().await;
        assert!(get_active_session(&conn, 100, 0).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn create_then_get_active() {
        let (_dir, conn) = test_conn().await;
        let id = create_session(&conn, 100, 0, "uuid-1", Some("hello world"))
            .await
            .unwrap();
        let active = get_active_session(&conn, 100, 0).await.unwrap().unwrap();
        assert_eq!(active.id, id);
        assert_eq!(active.root_session_id, "uuid-1");
        assert_eq!(active.label.as_deref(), Some("hello world"));
    }

    #[tokio::test]
    async fn deactivate_clears_active() {
        let (_dir, conn) = test_conn().await;
        create_session(&conn, 100, 0, "uuid-1", None).await.unwrap();
        let prev = deactivate_current(&conn, 100, 0).await.unwrap();
        assert_eq!(prev.as_deref(), Some("uuid-1"));
        assert!(get_active_session(&conn, 100, 0).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn deactivate_returns_none_when_no_active() {
        let (_dir, conn) = test_conn().await;
        let prev = deactivate_current(&conn, 100, 0).await.unwrap();
        assert!(prev.is_none());
    }

    #[tokio::test]
    async fn activate_session_by_id() {
        let (_dir, conn) = test_conn().await;
        let id = create_session(&conn, 100, 0, "uuid-1", None).await.unwrap();
        deactivate_current(&conn, 100, 0).await.unwrap();
        activate_session(&conn, id).await.unwrap();
        let active = get_active_session(&conn, 100, 0).await.unwrap().unwrap();
        assert_eq!(active.root_session_id, "uuid-1");
    }

    #[tokio::test]
    async fn list_sessions_ordered_by_last_used() {
        let (_dir, conn) = test_conn().await;
        let old_id = create_session(&conn, 100, 0, "uuid-old", Some("old"))
            .await
            .unwrap();
        // Pin uuid-old to a known past timestamp so uuid-new sorts after it.
        conn.execute(
            "UPDATE sessions SET last_used_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
            right_db::params![old_id],
        )
        .await
        .unwrap();
        deactivate_current(&conn, 100, 0).await.unwrap();
        create_session(&conn, 100, 0, "uuid-new", Some("new"))
            .await
            .unwrap();
        let active = get_active_session(&conn, 100, 0).await.unwrap().unwrap();
        touch_session(&conn, active.id).await.unwrap();

        let sessions = list_sessions(&conn, 100, 0).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].root_session_id, "uuid-new");
    }

    #[tokio::test]
    async fn find_session_by_partial_uuid() {
        let (_dir, conn) = test_conn().await;
        create_session(&conn, 100, 0, "550e8400-e29b-41d4", None)
            .await
            .unwrap();
        deactivate_current(&conn, 100, 0).await.unwrap();
        create_session(&conn, 100, 0, "7a3f1b22-c9d8-4e5f", None)
            .await
            .unwrap();

        let matches = find_sessions_by_uuid(&conn, 100, 0, "550e").await.unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].root_session_id, "550e8400-e29b-41d4");
    }

    #[tokio::test]
    async fn find_session_partial_returns_multiple() {
        let (_dir, conn) = test_conn().await;
        create_session(&conn, 100, 0, "aaa-111", None)
            .await
            .unwrap();
        deactivate_current(&conn, 100, 0).await.unwrap();
        create_session(&conn, 100, 0, "aaa-222", None)
            .await
            .unwrap();

        let matches = find_sessions_by_uuid(&conn, 100, 0, "aaa").await.unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[tokio::test]
    async fn truncate_label_at_60_chars() {
        let long = "a".repeat(100);
        assert_eq!(truncate_label(&long).len(), 60);
        assert_eq!(truncate_label("short"), "short");
    }

    #[tokio::test]
    async fn sessions_isolated_by_thread_id() {
        let (_dir, conn) = test_conn().await;
        create_session(&conn, 100, 0, "thread0", None)
            .await
            .unwrap();
        create_session(&conn, 100, 5, "thread5", None)
            .await
            .unwrap();

        let t0 = get_active_session(&conn, 100, 0).await.unwrap().unwrap();
        let t5 = get_active_session(&conn, 100, 5).await.unwrap().unwrap();
        assert_eq!(t0.root_session_id, "thread0");
        assert_eq!(t5.root_session_id, "thread5");
    }

    #[tokio::test]
    async fn full_lifecycle_new_switch_list() {
        let (_dir, conn) = test_conn().await;

        // First message creates session 1
        let id1 = create_session(&conn, 100, 0, "uuid-1", Some("hello world"))
            .await
            .unwrap();
        let active = get_active_session(&conn, 100, 0).await.unwrap().unwrap();
        assert_eq!(active.id, id1);

        // /new — deactivate, create session 2
        let prev = deactivate_current(&conn, 100, 0).await.unwrap();
        assert_eq!(prev.as_deref(), Some("uuid-1"));
        let id2 = create_session(&conn, 100, 0, "uuid-2", Some("second task"))
            .await
            .unwrap();

        // /list — both visible, session 2 active
        let all = list_sessions(&conn, 100, 0).await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(
            all.iter()
                .any(|s| s.root_session_id == "uuid-2" && s.is_active)
        );
        assert!(
            all.iter()
                .any(|s| s.root_session_id == "uuid-1" && !s.is_active)
        );

        // /switch — back to session 1
        deactivate_current(&conn, 100, 0).await.unwrap();
        activate_session(&conn, id1).await.unwrap();
        let active = get_active_session(&conn, 100, 0).await.unwrap().unwrap();
        assert_eq!(active.root_session_id, "uuid-1");

        let _ = id2; // suppress unused warning
    }

    #[tokio::test]
    async fn find_session_by_label() {
        let (_dir, conn) = test_conn().await;
        create_session(&conn, 100, 0, "uuid-aaa", Some("crypto research"))
            .await
            .unwrap();
        deactivate_current(&conn, 100, 0).await.unwrap();
        create_session(&conn, 100, 0, "uuid-bbb", Some("test cron"))
            .await
            .unwrap();

        let matches = find_sessions_by_uuid(&conn, 100, 0, "crypto")
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].root_session_id, "uuid-aaa");
    }

    #[tokio::test]
    async fn activate_session_is_atomic() {
        let (_dir, conn) = test_conn().await;
        let id1 = create_session(&conn, 100, 0, "uuid-1", None).await.unwrap();
        deactivate_current(&conn, 100, 0).await.unwrap();
        let id2 = create_session(&conn, 100, 0, "uuid-2", None).await.unwrap();

        // activate_session should atomically deactivate uuid-2 and activate uuid-1
        activate_session(&conn, id1).await.unwrap();
        let active = get_active_session(&conn, 100, 0).await.unwrap().unwrap();
        assert_eq!(active.root_session_id, "uuid-1");
        assert!(
            !list_sessions(&conn, 100, 0)
                .await
                .unwrap()
                .iter()
                .any(|s| s.id == id2 && s.is_active)
        );
    }
}
