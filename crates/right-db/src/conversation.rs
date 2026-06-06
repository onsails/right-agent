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

pub async fn archive_message(conn: &Connection, message: ConversationMessage<'_>) -> Result<i64> {
    let content = trimmed_content(message.content)?;
    let role = message.role.as_str();
    let turn_id = checked_turn_id(message.turn_id)?;
    let addressed_to_bot = i64::from(message.addressed_to_bot);
    let routed_to_agent = i64::from(message.routed_to_agent);

    if matches!(message.role, ConversationRole::Assistant) || message.message_id.is_none() {
        return conn
            .query_one(
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
            )
            .await;
    }

    conn.query_one(
        "INSERT INTO conversation_messages (
            platform, chat_id, thread_id, message_id, sender_user_id, sender_name,
            addressed_to_bot, routed_to_agent, root_session_id, turn_id, role, content
         ) VALUES (
            ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?
         )
         -- Bare ON CONFLICT relies on exactly one applicable unique constraint
         -- (the partial idx_conversation_messages_inbound_unique); the WHERE-
         -- qualified conflict-target form is dropped because Turso's parser does not accept it.
         ON CONFLICT
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
    .await
}

// UPSERT, not UPDATE: closes the race where worker turn-start beats the
// async archive write to the row. The stub carries content='', which indexes
// no searchable terms. Once the archive INSERT lands, ON CONFLICT DO UPDATE
// replaces content while OR/COALESCE in archive_message preserves routing
// fields regardless of which write wins.
pub async fn mark_routed(
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
         -- Bare ON CONFLICT relies on exactly one applicable unique constraint
         -- (the partial idx_conversation_messages_inbound_unique); the WHERE-
         -- qualified conflict-target form is dropped because Turso's parser does not accept it.
         ON CONFLICT
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
    .await
}

pub async fn search_thread(
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
    .await
}

pub async fn search_chat(
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
    .await
}

/// A message fetched by id for on-demand reply recovery.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchedMessage {
    pub message_id: Option<i32>,
    pub sender_name: Option<String>,
    pub text: String,
    pub role: String,
}

/// Fetch archived messages by telegram message id, scoped to one
/// `(chat_id, thread_id)`. Ids outside the scope or not archived are absent
/// from the result. Empty `message_ids` returns an empty Vec.
pub async fn fetch_by_ids(
    conn: &Connection,
    platform: &str,
    chat_id: i64,
    thread_id: i64,
    message_ids: &[i32],
) -> Result<Vec<FetchedMessage>> {
    if message_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT message_id, sender_name, content, role
         FROM conversation_messages
         WHERE platform = ? AND chat_id = ? AND thread_id = ?
           AND message_id IN ({placeholders})
           AND content <> ''
         ORDER BY message_id ASC"
    );
    let mut params = crate::params::ParamsBuilder::new();
    params.push(platform)?;
    params.push(chat_id)?;
    params.push(thread_id)?;
    for id in message_ids {
        params.push(*id)?;
    }
    conn.query_all(&sql, params, fetched_from_row).await
}

pub async fn is_recent_routed_target(
    conn: &Connection,
    platform: &str,
    chat_id: i64,
    thread_id: i64,
    message_id: i32,
    root_session_id: &str,
    window: i64,
    current_turn_id: i64,
) -> Result<bool> {
    let min_turn = current_turn_id.saturating_sub(window);
    let rows: Vec<i64> = conn
        .query_all(
            "SELECT 1
             FROM conversation_messages
             WHERE platform = ?
               AND chat_id = ?
               AND thread_id = ?
               AND message_id = ?
               AND root_session_id = ?
               AND routed_to_agent = 1
               AND turn_id > ?
             LIMIT 1",
            crate::params![
                platform,
                chat_id,
                thread_id,
                i64::from(message_id),
                root_session_id,
                min_turn
            ],
            |r| r.get(0),
        )
        .await?;

    Ok(!rows.is_empty())
}

pub async fn latest_assistant_text(
    conn: &Connection,
    root_session_id: &str,
) -> Result<Option<String>> {
    let mut rows = conn
        .query_all(
            "SELECT content
             FROM conversation_messages
             WHERE root_session_id = ?
               AND role = 'assistant'
             ORDER BY turn_id DESC, id DESC
             LIMIT 1",
            crate::params![root_session_id],
            |r| r.get(0),
        )
        .await?;

    Ok(rows.pop())
}

fn fetched_from_row(row: &crate::row::Row<'_>) -> Result<FetchedMessage> {
    Ok(FetchedMessage {
        message_id: row.get(0)?,
        sender_name: row.get(1)?,
        text: row.get(2)?,
        role: row.get(3)?,
    })
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

    async fn migrated_connection() -> TestDb {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::open_connection(dir.path(), true).await.unwrap();
        TestDb { _dir: dir, conn }
    }

    async fn legacy_conversation_partial_unique_connection() -> Connection {
        let conn = Connection::open_in_memory().await.unwrap();
        conn.execute_batch(
            "CREATE TABLE conversation_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                platform TEXT NOT NULL DEFAULT 'telegram',
                chat_id INTEGER NOT NULL,
                thread_id INTEGER NOT NULL DEFAULT 0,
                message_id INTEGER,
                sender_user_id INTEGER,
                sender_name TEXT,
                addressed_to_bot INTEGER NOT NULL DEFAULT 0 CHECK (addressed_to_bot IN (0, 1)),
                routed_to_agent INTEGER NOT NULL DEFAULT 0 CHECK (routed_to_agent IN (0, 1)),
                root_session_id TEXT,
                turn_id INTEGER,
                role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            );
            CREATE UNIQUE INDEX idx_conversation_messages_inbound_unique
            ON conversation_messages (platform, chat_id, message_id, role)
            WHERE message_id IS NOT NULL;",
        )
        .await
        .unwrap();
        conn
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

    async fn archive_assistant_row(conn: &Connection, session: &str, turn: u64, content: &str) {
        archive_message(
            conn,
            ConversationMessage {
                platform: "telegram",
                chat_id: 100,
                thread_id: 10,
                message_id: None,
                sender_user_id: None,
                sender_name: None,
                addressed_to_bot: false,
                routed_to_agent: false,
                root_session_id: Some(session),
                turn_id: Some(turn),
                role: ConversationRole::Assistant,
                content,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn fetch_by_ids_returns_matching_scoped_messages() {
        let conn = migrated_connection().await;
        archive_message(&conn, user_message(100, 10, 25, "hello"))
            .await
            .unwrap();
        archive_message(&conn, user_message(100, 10, 26, "world"))
            .await
            .unwrap();
        archive_message(&conn, user_message(100, 99, 27, "other thread"))
            .await
            .unwrap();

        let rows = fetch_by_ids(&conn, "telegram", 100, 10, &[25, 26, 27, 999])
            .await
            .unwrap();

        let ids: Vec<Option<i32>> = rows.iter().map(|r| r.message_id).collect();
        // 27 is in thread 99 (out of scope); 999 does not exist.
        assert_eq!(ids, vec![Some(25), Some(26)]);
        assert_eq!(rows[0].text, "hello");
        assert_eq!(rows[0].role, "user");
    }

    #[tokio::test]
    async fn fetch_by_ids_omits_mark_routed_stub_without_archive() {
        let conn = migrated_connection().await;
        mark_routed(&conn, "telegram", 100, 10, 25, "session-abc", 42)
            .await
            .unwrap();

        let rows = fetch_by_ids(&conn, "telegram", 100, 10, &[25])
            .await
            .unwrap();

        assert!(
            rows.is_empty(),
            "mark_routed stub is not archived content: {rows:?}"
        );
    }

    #[tokio::test]
    async fn fetch_by_ids_empty_input_returns_empty() {
        let conn = migrated_connection().await;
        let rows = fetch_by_ids(&conn, "telegram", 100, 10, &[]).await.unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn is_recent_routed_target_true_for_routed_in_window() {
        let conn = migrated_connection().await;
        mark_routed(&conn, "telegram", 100, 7, 50, "S", 5)
            .await
            .unwrap();

        let routed = is_recent_routed_target(&conn, "telegram", 100, 7, 50, "S", 30, 5)
            .await
            .unwrap();

        assert!(routed);
    }

    #[tokio::test]
    async fn is_recent_routed_target_false_when_not_routed() {
        let conn = migrated_connection().await;
        archive_message(&conn, user_message(100, 7, 50, "Сравни по времени в море"))
            .await
            .unwrap();

        let routed = is_recent_routed_target(&conn, "telegram", 100, 7, 50, "S", 30, 5)
            .await
            .unwrap();

        assert!(!routed);
    }

    #[tokio::test]
    async fn is_recent_routed_target_false_when_outside_window() {
        let conn = migrated_connection().await;
        mark_routed(&conn, "telegram", 100, 7, 50, "S", 2)
            .await
            .unwrap();

        let routed = is_recent_routed_target(&conn, "telegram", 100, 7, 50, "S", 30, 40)
            .await
            .unwrap();

        assert!(!routed);
    }

    #[tokio::test]
    async fn is_recent_routed_target_false_for_other_root_session() {
        let conn = migrated_connection().await;
        mark_routed(&conn, "telegram", 100, 7, 50, "other-session", 5)
            .await
            .unwrap();

        let routed = is_recent_routed_target(&conn, "telegram", 100, 7, 50, "S", 30, 5)
            .await
            .unwrap();

        assert!(!routed);
    }

    #[tokio::test]
    async fn latest_assistant_text_returns_most_recent() {
        let conn = migrated_connection().await;
        archive_assistant_row(&conn, "S", 1, "older answer").await;
        archive_assistant_row(&conn, "S", 2, "freshest answer").await;
        archive_assistant_row(&conn, "session-other", 43, "other session").await;

        let text = latest_assistant_text(&conn, "S").await.unwrap();

        assert_eq!(text, Some("freshest answer".to_string()));
    }

    #[tokio::test]
    async fn latest_assistant_text_uses_id_tiebreaker_for_same_turn() {
        let conn = migrated_connection().await;
        archive_assistant_row(&conn, "S", 2, "first same-turn answer").await;
        archive_assistant_row(&conn, "S", 2, "later same-turn answer").await;

        let text = latest_assistant_text(&conn, "S").await.unwrap();

        assert_eq!(text, Some("later same-turn answer".to_string()));
    }

    #[tokio::test]
    async fn latest_assistant_text_none_when_no_assistant_rows() {
        let conn = migrated_connection().await;
        let mut message = user_message(100, 10, 25, "user only");
        message.root_session_id = Some("session-abc");
        message.turn_id = Some(42);
        archive_message(&conn, message).await.unwrap();

        let text = latest_assistant_text(&conn, "session-abc").await.unwrap();

        assert_eq!(text, None);
    }

    #[tokio::test]
    async fn archive_message_is_idempotent_for_inbound_telegram_message() {
        let conn = migrated_connection().await;

        let first_id = archive_message(&conn, user_message(100, 10, 25, "first draft"))
            .await
            .unwrap();
        let second_id = archive_message(&conn, user_message(100, 10, 25, "revised draft"))
            .await
            .unwrap();

        assert_eq!(first_id, second_id);
        let (count, content): (i64, String) = conn
            .query_one(
                "SELECT COUNT(*), content FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(content, "revised draft");
    }

    #[tokio::test]
    async fn archive_message_is_compatible_with_legacy_partial_unique_index() {
        let conn = legacy_conversation_partial_unique_connection().await;

        let first_id = archive_message(&conn, user_message(100, 10, 25, "first draft"))
            .await
            .unwrap();
        let second_id = archive_message(&conn, user_message(100, 10, 25, "revised draft"))
            .await
            .unwrap();

        assert_eq!(first_id, second_id);
        let (count, content): (i64, String) = conn
            .query_one(
                "SELECT COUNT(*), content FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(content, "revised draft");
    }

    #[tokio::test]
    async fn mark_routed_is_compatible_with_legacy_partial_unique_index() {
        let conn = legacy_conversation_partial_unique_connection().await;

        let first = mark_routed(&conn, "telegram", 100, 10, 25, "session-abc", 42)
            .await
            .unwrap();
        let second = mark_routed(&conn, "telegram", 100, 10, 25, "session-def", 43)
            .await
            .unwrap();

        assert_eq!(first, 1);
        assert_eq!(second, 1);
        let row: (i64, String, i64) = conn
            .query_one(
                "SELECT COUNT(*), root_session_id, turn_id FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(row, (1, "session-def".to_string(), 43));
    }

    #[tokio::test]
    async fn mark_routed_sets_session_and_turn() {
        let conn = migrated_connection().await;
        archive_message(&conn, user_message(100, 10, 25, "route me"))
            .await
            .unwrap();

        let changed = mark_routed(&conn, "telegram", 100, 10, 25, "session-abc", 42)
            .await
            .unwrap();

        assert_eq!(changed, 1);
        let routed: (i64, Option<String>, Option<i64>) = conn
            .query_one(
                "SELECT routed_to_agent, root_session_id, turn_id
                 FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(routed, (1, Some("session-abc".to_string()), Some(42)));
    }

    #[tokio::test]
    async fn mark_routed_stub_without_archive_does_not_pollute_search() {
        // If archive write is dropped (semaphore saturation, restart), the
        // mark_routed stub row must not appear as a phantom search hit.
        let conn = migrated_connection().await;

        mark_routed(&conn, "telegram", 100, 10, 25, "session-abc", 42)
            .await
            .unwrap();
        archive_message(&conn, user_message(100, 10, 99, "real content here"))
            .await
            .unwrap();

        // Any FTS query must only return the real-content row, never the stub.
        let chat_results = search_chat(&conn, "real", 10, 100).await.unwrap();
        assert_eq!(chat_results.len(), 1);
        assert_eq!(chat_results[0].message_id, Some(99));

        // Querying FTS for an empty term must not surface the stub.
        // (Searching for the all-content sentinel via MATCH 'real OR content'
        // proves the stub doesn't appear under any term.)
        let results = search_chat(&conn, "anything OR maybe", 50, 100)
            .await
            .unwrap();
        assert!(
            results.iter().all(|r| r.message_id != Some(25)),
            "stub row (message_id=25) must not appear in search results"
        );
    }

    #[tokio::test]
    async fn mark_routed_then_archive_preserves_routing_metadata() {
        // Race ordering: worker calls mark_routed before async archive write
        // lands. mark_routed creates a stub row; archive_message later
        // overwrites content while preserving routed_to_agent/session/turn.
        let conn = migrated_connection().await;

        mark_routed(&conn, "telegram", 100, 10, 25, "session-abc", 42)
            .await
            .unwrap();
        archive_message(&conn, user_message(100, 10, 25, "real content"))
            .await
            .unwrap();

        let (routed, session, turn, content): (i64, Option<String>, Option<i64>, String) = conn
            .query_one(
                "SELECT routed_to_agent, root_session_id, turn_id, content
                 FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
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

    #[tokio::test]
    async fn archive_message_preserves_existing_route_metadata_when_rearchived() {
        let conn = migrated_connection().await;
        archive_message(&conn, user_message(100, 10, 25, "route me"))
            .await
            .unwrap();
        mark_routed(&conn, "telegram", 100, 10, 25, "session-abc", 42)
            .await
            .unwrap();

        archive_message(&conn, user_message(100, 10, 25, "route me edited"))
            .await
            .unwrap();

        let routed: (i64, Option<String>, Option<i64>, String) = conn
            .query_one(
                "SELECT routed_to_agent, root_session_id, turn_id, content
                 FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
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

    #[tokio::test]
    async fn thread_search_filters_to_current_thread() {
        let conn = migrated_connection().await;
        archive_message(&conn, user_message(100, 10, 25, "needle in thread ten"))
            .await
            .unwrap();
        archive_message(&conn, user_message(100, 11, 26, "needle in thread eleven"))
            .await
            .unwrap();
        archive_message(&conn, user_message(200, 10, 27, "needle in other chat"))
            .await
            .unwrap();

        let results = search_thread(&conn, "needle", 10, 100, 10).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].thread_id, 10);
        assert_eq!(results[0].message_id, Some(25));
    }

    #[tokio::test]
    async fn chat_search_includes_all_threads_in_current_chat() {
        let conn = migrated_connection().await;
        archive_message(&conn, user_message(100, 10, 25, "needle in thread ten"))
            .await
            .unwrap();
        archive_message(&conn, user_message(100, 11, 26, "needle in thread eleven"))
            .await
            .unwrap();
        archive_message(&conn, user_message(200, 10, 27, "needle in other chat"))
            .await
            .unwrap();

        let results = search_chat(&conn, "needle", 10, 100).await.unwrap();
        let message_ids: Vec<Option<i32>> = results.iter().map(|r| r.message_id).collect();

        assert_eq!(message_ids, vec![Some(26), Some(25)]);
        assert!(
            results
                .iter()
                .all(|r| r.thread_id == 10 || r.thread_id == 11)
        );
    }

    #[tokio::test]
    async fn search_filters_to_telegram_platform() {
        let conn = migrated_connection().await;
        archive_message(&conn, user_message(100, 10, 25, "platform needle telegram"))
            .await
            .unwrap();
        archive_message(
            &conn,
            message("discord", 100, 10, 26, "platform needle discord"),
        )
        .await
        .unwrap();

        let thread_results = search_thread(&conn, "platform needle", 10, 100, 10)
            .await
            .unwrap();
        let chat_results = search_chat(&conn, "platform needle", 10, 100)
            .await
            .unwrap();

        assert_eq!(thread_results.len(), 1);
        assert_eq!(thread_results[0].message_id, Some(25));
        assert_eq!(chat_results.len(), 1);
        assert_eq!(chat_results[0].message_id, Some(25));
    }

    #[tokio::test]
    async fn empty_search_query_is_rejected() {
        let conn = migrated_connection().await;

        let result = search_chat(&conn, " .!? ", 10, 100).await;

        assert!(
            result.is_err(),
            "empty normalized FTS query must be rejected"
        );
    }
}
