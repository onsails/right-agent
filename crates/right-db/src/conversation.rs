use rusqlite::{Connection, Result, named_params};

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
        return conn.query_row(
            "INSERT INTO conversation_messages (
                platform, chat_id, thread_id, message_id, sender_user_id, sender_name,
                addressed_to_bot, routed_to_agent, root_session_id, turn_id, role, content
             ) VALUES (
                :platform, :chat_id, :thread_id, NULL, :sender_user_id, :sender_name,
                :addressed_to_bot, :routed_to_agent, :root_session_id, :turn_id, :role, :content
             )
             RETURNING id",
            named_params! {
                ":platform": message.platform,
                ":chat_id": message.chat_id,
                ":thread_id": message.thread_id,
                ":sender_user_id": message.sender_user_id,
                ":sender_name": message.sender_name,
                ":addressed_to_bot": addressed_to_bot,
                ":routed_to_agent": routed_to_agent,
                ":root_session_id": message.root_session_id,
                ":turn_id": turn_id,
                ":role": role,
                ":content": content,
            },
            |r| r.get(0),
        );
    }

    conn.query_row(
        "INSERT INTO conversation_messages (
            platform, chat_id, thread_id, message_id, sender_user_id, sender_name,
            addressed_to_bot, routed_to_agent, root_session_id, turn_id, role, content
         ) VALUES (
            :platform, :chat_id, :thread_id, :message_id, :sender_user_id, :sender_name,
            :addressed_to_bot, :routed_to_agent, :root_session_id, :turn_id, :role, :content
         )
         ON CONFLICT(platform, chat_id, message_id, role) WHERE message_id IS NOT NULL
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
        named_params! {
            ":platform": message.platform,
            ":chat_id": message.chat_id,
            ":thread_id": message.thread_id,
            ":message_id": message.message_id,
            ":sender_user_id": message.sender_user_id,
            ":sender_name": message.sender_name,
            ":addressed_to_bot": addressed_to_bot,
            ":routed_to_agent": routed_to_agent,
            ":root_session_id": message.root_session_id,
            ":turn_id": turn_id,
            ":role": role,
            ":content": content,
        },
        |r| r.get(0),
    )
}

pub fn mark_routed(
    conn: &Connection,
    platform: &str,
    chat_id: i64,
    message_id: i32,
    root_session_id: &str,
    turn_id: u64,
) -> Result<usize> {
    let turn_id = checked_turn_id(Some(turn_id))?;
    conn.execute(
        "UPDATE conversation_messages
         SET routed_to_agent = 1,
             root_session_id = ?1,
             turn_id = ?2
         WHERE platform = ?3
           AND chat_id = ?4
           AND message_id = ?5
           AND role = 'user'",
        (root_session_id, turn_id, platform, chat_id, message_id),
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
    let mut stmt = conn.prepare(
        "SELECT
            m.id,
            m.role,
            snippet(conversation_messages_fts, 0, '[', ']', '...', 12),
            m.sender_user_id,
            m.sender_name,
            m.created_at,
            m.thread_id,
            m.message_id,
            m.root_session_id
         FROM conversation_messages_fts
         JOIN conversation_messages m ON m.id = conversation_messages_fts.rowid
         WHERE conversation_messages_fts MATCH ?
           AND m.platform = 'telegram'
           AND m.chat_id = ?
           AND m.thread_id = ?
         ORDER BY m.created_at DESC, m.id DESC
         LIMIT ?",
    )?;
    collect_search_results(
        stmt.query_map((query, chat_id, thread_id, limit), search_result_from_row)?,
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
    let mut stmt = conn.prepare(
        "SELECT
            m.id,
            m.role,
            snippet(conversation_messages_fts, 0, '[', ']', '...', 12),
            m.sender_user_id,
            m.sender_name,
            m.created_at,
            m.thread_id,
            m.message_id,
            m.root_session_id
         FROM conversation_messages_fts
         JOIN conversation_messages m ON m.id = conversation_messages_fts.rowid
         WHERE conversation_messages_fts MATCH ?
           AND m.platform = 'telegram'
           AND m.chat_id = ?
         ORDER BY m.created_at DESC, m.id DESC
         LIMIT ?",
    )?;
    collect_search_results(stmt.query_map((query, chat_id, limit), search_result_from_row)?)
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

fn search_result_from_row(row: &rusqlite::Row<'_>) -> Result<ConversationSearchResult> {
    Ok(ConversationSearchResult {
        id: row.get(0)?,
        role: row.get(1)?,
        snippet: row.get(2)?,
        sender_user_id: row.get(3)?,
        sender_name: row.get(4)?,
        created_at: row.get(5)?,
        thread_id: row.get(6)?,
        message_id: row.get(7)?,
        root_session_id: row.get(8)?,
    })
}

fn collect_search_results<F>(
    rows: rusqlite::MappedRows<'_, F>,
) -> Result<Vec<ConversationSearchResult>>
where
    F: FnMut(&rusqlite::Row<'_>) -> Result<ConversationSearchResult>,
{
    rows.collect()
}

fn invalid_parameter(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn migrated_connection() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::MIGRATIONS.to_latest(&mut conn).unwrap();
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

    #[test]
    fn archive_message_is_idempotent_for_inbound_telegram_message() {
        let conn = migrated_connection();

        let first_id = archive_message(&conn, user_message(100, 10, 25, "first draft")).unwrap();
        let second_id = archive_message(&conn, user_message(100, 10, 25, "revised draft")).unwrap();

        assert_eq!(first_id, second_id);
        let (count, content): (i64, String) = conn
            .query_row(
                "SELECT COUNT(*), content FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                [],
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

        let changed = mark_routed(&conn, "telegram", 100, 25, "session-abc", 42).unwrap();

        assert_eq!(changed, 1);
        let routed: (i64, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT routed_to_agent, root_session_id, turn_id
                 FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(routed, (1, Some("session-abc".to_string()), Some(42)));
    }

    #[test]
    fn archive_message_preserves_existing_route_metadata_when_rearchived() {
        let conn = migrated_connection();
        archive_message(&conn, user_message(100, 10, 25, "route me")).unwrap();
        mark_routed(&conn, "telegram", 100, 25, "session-abc", 42).unwrap();

        archive_message(&conn, user_message(100, 10, 25, "route me edited")).unwrap();

        let routed: (i64, Option<String>, Option<i64>, String) = conn
            .query_row(
                "SELECT routed_to_agent, root_session_id, turn_id, content
                 FROM conversation_messages
                 WHERE platform='telegram' AND chat_id=100 AND message_id=25 AND role='user'",
                [],
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
