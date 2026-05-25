use crate::{Connection, DbError};

type Result<T> = std::result::Result<T, DbError>;
const SEARCH_SNIPPET_MAX_CHARS: usize = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationRole {
    User,
    Assistant,
}

impl ConversationRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

pub struct ConversationMessage<'a> {
    pub platform: &'a str,
    pub chat_id: i64,
    pub thread_id: i64,
    pub message_id: Option<i32>,
    pub sender_user_id: Option<i64>,
    pub sender_name: Option<&'a str>,
    pub addressed_to_bot: bool,
    pub routed_to_agent: bool,
    pub root_session_id: Option<&'a str>,
    pub turn_id: Option<u64>,
    pub role: ConversationRole,
    pub content: &'a str,
}

pub struct ConversationSearchResult {
    pub id: i64,
    pub role: String,
    pub snippet: String,
    pub sender_user_id: Option<i64>,
    pub sender_name: Option<String>,
    pub created_at: String,
    pub thread_id: i64,
    pub message_id: Option<i32>,
    pub root_session_id: Option<String>,
}

pub fn archive_message(conn: &Connection, message: ConversationMessage<'_>) -> Result<i64> {
    let content = trimmed_content(message.content)?;
    let role = message.role.as_str();
    let turn_id = checked_turn_id(message.turn_id)?;
    let addressed_to_bot = i64::from(message.addressed_to_bot);
    let routed_to_agent = i64::from(message.routed_to_agent);

    if matches!(message.role, ConversationRole::Assistant) || message.message_id.is_none() {
        return conn.query_one(
            "INSERT INTO conversation_messages (
                platform, chat_id, thread_id, message_id, sender_user_id, sender_name,
                addressed_to_bot, routed_to_agent, root_session_id, turn_id, role, content
             ) VALUES (
                ?, ?, ?, NULL, ?, ?,
                ?, ?, ?, ?, ?, ?
             )
             RETURNING id",
            crate::params![
                message.platform,
                message.chat_id,
                message.thread_id,
                message.sender_user_id,
                message.sender_name,
                addressed_to_bot,
                routed_to_agent,
                message.root_session_id,
                turn_id,
                role,
                content,
            ],
            |r| r.get(0),
        );
    }

    conn.query_one(
        "INSERT INTO conversation_messages (
            platform, chat_id, thread_id, message_id, sender_user_id, sender_name,
            addressed_to_bot, routed_to_agent, root_session_id, turn_id, role, content
         ) VALUES (
            ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?
         )
         ON CONFLICT(platform, chat_id, message_id, role)
         DO UPDATE SET
            thread_id = excluded.thread_id,
            sender_user_id = excluded.sender_user_id,
            sender_name = excluded.sender_name,
            addressed_to_bot = excluded.addressed_to_bot,
            routed_to_agent = conversation_messages.routed_to_agent OR excluded.routed_to_agent,
            root_session_id = COALESCE(excluded.root_session_id, conversation_messages.root_session_id),
            turn_id = COALESCE(excluded.turn_id, conversation_messages.turn_id),
            content = excluded.content
         RETURNING id",
        crate::params![
            message.platform,
            message.chat_id,
            message.thread_id,
            message.message_id,
            message.sender_user_id,
            message.sender_name,
            addressed_to_bot,
            routed_to_agent,
            message.root_session_id,
            turn_id,
            role,
            content,
        ],
        |r| r.get(0),
    )
}

// UPSERT, not UPDATE: closes the race where worker turn-start beats the
// async archive write to the row. The stub carries content='', which indexes
// no searchable terms. Once the archive INSERT lands, ON CONFLICT DO UPDATE
// replaces content while OR/COALESCE in archive_message preserves routing
// fields regardless of which write wins.
pub fn mark_routed(
    conn: &Connection,
    platform: &str,
    chat_id: i64,
    thread_id: i64,
    message_id: i32,
    root_session_id: &str,
    turn_id: u64,
) -> Result<usize> {
    let turn_id = checked_turn_id(Some(turn_id))?;
    conn.execute(
        "INSERT INTO conversation_messages (
            platform, chat_id, thread_id, message_id,
            addressed_to_bot, routed_to_agent, root_session_id, turn_id,
            role, content
         ) VALUES (
            ?, ?, ?, ?,
            0, 1, ?, ?,
            'user', ''
         )
         ON CONFLICT(platform, chat_id, message_id, role)
         DO UPDATE SET
            routed_to_agent = 1,
            root_session_id = excluded.root_session_id,
            turn_id = excluded.turn_id",
        crate::params![
            platform,
            chat_id,
            thread_id,
            message_id,
            root_session_id,
            turn_id
        ],
    )
}

pub fn search_thread(
    conn: &Connection,
    query: &str,
    limit: usize,
    chat_id: i64,
    thread_id: i64,
) -> Result<Vec<ConversationSearchResult>> {
    let query = normalized_fts_query(query)?;
    let limit = clamped_limit(limit);
    conn.query_all(
        "SELECT
            m.id,
            m.role,
            m.content,
            m.sender_user_id,
            m.sender_name,
            m.created_at,
            m.thread_id,
            m.message_id,
            m.root_session_id
         FROM conversation_messages m
         WHERE m.content MATCH ?
           AND m.platform = 'telegram'
           AND m.chat_id = ?
           AND m.thread_id = ?
         ORDER BY m.created_at DESC, m.id DESC
         LIMIT ?",
        crate::params![query, chat_id, thread_id, limit],
        search_result_from_row,
    )
}

pub fn search_chat(
    conn: &Connection,
    query: &str,
    limit: usize,
    chat_id: i64,
) -> Result<Vec<ConversationSearchResult>> {
    let query = normalized_fts_query(query)?;
    let limit = clamped_limit(limit);
    conn.query_all(
        "SELECT
            m.id,
            m.role,
            m.content,
            m.sender_user_id,
            m.sender_name,
            m.created_at,
            m.thread_id,
            m.message_id,
            m.root_session_id
         FROM conversation_messages m
         WHERE m.content MATCH ?
           AND m.platform = 'telegram'
           AND m.chat_id = ?
         ORDER BY m.created_at DESC, m.id DESC
         LIMIT ?",
        crate::params![query, chat_id, limit],
        search_result_from_row,
    )
}

fn trimmed_content(content: &str) -> Result<&str> {
    let content = content.trim();
    if content.is_empty() {
        return Err(invalid_parameter("content must not be empty"));
    }
    Ok(content)
}

fn checked_turn_id(turn_id: Option<u64>) -> Result<Option<i64>> {
    turn_id
        .map(|value| i64::try_from(value).map_err(|_| invalid_parameter("turn_id out of range")))
        .transpose()
}

fn clamped_limit(limit: usize) -> i64 {
    limit.clamp(1, 50) as i64
}

fn normalized_fts_query(query: &str) -> Result<String> {
    let mut terms = Vec::new();
    let mut current = String::new();

    for ch in query.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            terms.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }

    if terms.is_empty() {
        return Err(invalid_parameter("search query must contain a term"));
    }

    Ok(terms
        .into_iter()
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>()
        .join(" AND "))
}

fn search_result_from_row(row: &crate::row::Row<'_>) -> Result<ConversationSearchResult> {
    let content: String = row.get(2)?;
    Ok(ConversationSearchResult {
        id: row.get(0)?,
        role: row.get(1)?,
        snippet: bounded_search_snippet(&content),
        sender_user_id: row.get(3)?,
        sender_name: row.get(4)?,
        created_at: row.get(5)?,
        thread_id: row.get(6)?,
        message_id: row.get(7)?,
        root_session_id: row.get(8)?,
    })
}

fn bounded_search_snippet(content: &str) -> String {
    let content = content.trim();
    let mut chars = content.chars();
    let snippet = chars
        .by_ref()
        .take(SEARCH_SNIPPET_MAX_CHARS)
        .collect::<String>();

    if chars.next().is_none() {
        return snippet;
    }

    format!("{}...", snippet.trim_end())
}

fn invalid_parameter(message: &str) -> DbError {
    DbError::InvalidParameter(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct TestDb {
        _dir: TempDir,
        conn: Connection,
    }

    impl std::ops::Deref for TestDb {
        type Target = Connection;

        fn deref(&self) -> &Self::Target {
            &self.conn
        }
    }

    fn migrated_connection() -> TestDb {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::open_connection(dir.path(), true).unwrap();
        TestDb { _dir: dir, conn }
    }

    fn user_message<'a>(
        chat_id: i64,
        thread_id: i64,
        message_id: i32,
        content: &'a str,
    ) -> ConversationMessage<'a> {
        message("telegram", chat_id, thread_id, message_id, content)
    }

    fn message<'a>(
        platform: &'a str,
        chat_id: i64,
        thread_id: i64,
        message_id: i32,
        content: &'a str,
    ) -> ConversationMessage<'a> {
        ConversationMessage {
            platform,
            chat_id,
            thread_id,
            message_id: Some(message_id),
            sender_user_id: Some(9001),
            sender_name: Some("Ada"),
            addressed_to_bot: true,
            routed_to_agent: false,
            root_session_id: None,
            turn_id: None,
            role: ConversationRole::User,
            content,
        }
    }

    #[test]
    fn archive_message_is_idempotent_for_inbound_telegram_message() {
        let conn = migrated_connection();

        let first_id = archive_message(&conn, user_message(100, 10, 25, "first draft")).unwrap();
        let second_id = archive_message(&conn, user_message(100, 10, 25, "revised draft")).unwrap();

        assert_eq!(first_id, second_id);
        let (count, content): (i64, String) = conn
            .query_one(
                "SELECT COUNT(*), content FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(content, "revised draft");
    }

    #[test]
    fn mark_routed_sets_session_and_turn() {
        let conn = migrated_connection();
        archive_message(&conn, user_message(100, 10, 25, "route me")).unwrap();

        let changed = mark_routed(&conn, "telegram", 100, 10, 25, "session-abc", 42).unwrap();

        assert_eq!(changed, 1);
        let routed: (i64, Option<String>, Option<i64>) = conn
            .query_one(
                "SELECT routed_to_agent, root_session_id, turn_id
                 FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(routed, (1, Some("session-abc".to_string()), Some(42)));
    }

    #[test]
    fn mark_routed_stub_without_archive_does_not_pollute_search() {
        // If archive write is dropped (semaphore saturation, restart), the
        // mark_routed stub row must not appear as a phantom search hit.
        let conn = migrated_connection();

        mark_routed(&conn, "telegram", 100, 10, 25, "session-abc", 42).unwrap();
        archive_message(&conn, user_message(100, 10, 99, "real content here")).unwrap();

        // Any FTS query must only return the real-content row, never the stub.
        let chat_results = search_chat(&conn, "real", 10, 100).unwrap();
        assert_eq!(chat_results.len(), 1);
        assert_eq!(chat_results[0].message_id, Some(99));

        // Querying FTS for an empty term must not surface the stub.
        // (Searching for the all-content sentinel via MATCH 'real OR content'
        // proves the stub doesn't appear under any term.)
        let results = search_chat(&conn, "anything OR maybe", 50, 100).unwrap();
        assert!(
            results.iter().all(|r| r.message_id != Some(25)),
            "stub row (message_id=25) must not appear in search results"
        );
    }

    #[test]
    fn mark_routed_then_archive_preserves_routing_metadata() {
        // Race ordering: worker calls mark_routed before async archive write
        // lands. mark_routed creates a stub row; archive_message later
        // overwrites content while preserving routed_to_agent/session/turn.
        let conn = migrated_connection();

        mark_routed(&conn, "telegram", 100, 10, 25, "session-abc", 42).unwrap();
        archive_message(&conn, user_message(100, 10, 25, "real content")).unwrap();

        let (routed, session, turn, content): (i64, Option<String>, Option<i64>, String) = conn
            .query_one(
                "SELECT routed_to_agent, root_session_id, turn_id, content
                 FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (routed, session, turn, content),
            (
                1,
                Some("session-abc".to_string()),
                Some(42),
                "real content".to_string()
            )
        );
    }

    #[test]
    fn archive_message_preserves_existing_route_metadata_when_rearchived() {
        let conn = migrated_connection();
        archive_message(&conn, user_message(100, 10, 25, "route me")).unwrap();
        mark_routed(&conn, "telegram", 100, 10, 25, "session-abc", 42).unwrap();

        archive_message(&conn, user_message(100, 10, 25, "route me edited")).unwrap();

        let routed: (i64, Option<String>, Option<i64>, String) = conn
            .query_one(
                "SELECT routed_to_agent, root_session_id, turn_id, content
                 FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            routed,
            (
                1,
                Some("session-abc".to_string()),
                Some(42),
                "route me edited".to_string()
            )
        );
    }

    #[test]
    fn thread_search_filters_to_current_thread() {
        let conn = migrated_connection();
        archive_message(&conn, user_message(100, 10, 25, "needle in thread ten")).unwrap();
        archive_message(&conn, user_message(100, 11, 26, "needle in thread eleven")).unwrap();
        archive_message(&conn, user_message(200, 10, 27, "needle in other chat")).unwrap();

        let results = search_thread(&conn, "needle", 10, 100, 10).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].thread_id, 10);
        assert_eq!(results[0].message_id, Some(25));
    }

    #[test]
    fn chat_search_includes_all_threads_in_current_chat() {
        let conn = migrated_connection();
        archive_message(&conn, user_message(100, 10, 25, "needle in thread ten")).unwrap();
        archive_message(&conn, user_message(100, 11, 26, "needle in thread eleven")).unwrap();
        archive_message(&conn, user_message(200, 10, 27, "needle in other chat")).unwrap();

        let results = search_chat(&conn, "needle", 10, 100).unwrap();
        let message_ids: Vec<Option<i32>> = results.iter().map(|r| r.message_id).collect();

        assert_eq!(message_ids, vec![Some(26), Some(25)]);
        assert!(
            results
                .iter()
                .all(|r| r.thread_id == 10 || r.thread_id == 11)
        );
    }

    #[test]
    fn search_filters_to_telegram_platform() {
        let conn = migrated_connection();
        archive_message(&conn, user_message(100, 10, 25, "platform needle telegram")).unwrap();
        archive_message(
            &conn,
            message("discord", 100, 10, 26, "platform needle discord"),
        )
        .unwrap();

        let thread_results = search_thread(&conn, "platform needle", 10, 100, 10).unwrap();
        let chat_results = search_chat(&conn, "platform needle", 10, 100).unwrap();

        assert_eq!(thread_results.len(), 1);
        assert_eq!(thread_results[0].message_id, Some(25));
        assert_eq!(chat_results.len(), 1);
        assert_eq!(chat_results[0].message_id, Some(25));
    }

    #[test]
    fn empty_search_query_is_rejected() {
        let conn = migrated_connection();

        let result = search_chat(&conn, " .!? ", 10, 100);

        assert!(
            result.is_err(),
            "empty normalized FTS query must be rejected"
        );
    }
}
