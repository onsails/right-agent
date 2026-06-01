//! Per-agent registry of Telegram forum topics the agent has created or
//! managed. Authoritative source: results of the agent's own
//! create/edit/close/reopen tool calls. Rows are scoped by `chat_id`; the
//! MCP layer must always pass the server-resolved current chat id and never
//! an agent-supplied value.

use crate::{Connection, Row};

type Result<T> = std::result::Result<T, crate::DbError>;

/// One registry row, returned by [`list`].
#[derive(Debug, Clone, PartialEq)]
pub struct ForumTopicRow {
    pub message_thread_id: i64,
    pub name: Option<String>,
    pub icon_color: Option<i64>,
    pub icon_custom_emoji_id: Option<String>,
    pub state: String,
    pub updated_at: String,
}

/// Open/closed state of a forum topic. The only two states Telegram supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumTopicState {
    Open,
    Closed,
}

impl ForumTopicState {
    pub fn as_str(self) -> &'static str {
        match self {
            ForumTopicState::Open => "open",
            ForumTopicState::Closed => "closed",
        }
    }
}

fn row_to_topic(r: &Row<'_>) -> Result<ForumTopicRow> {
    Ok(ForumTopicRow {
        message_thread_id: r.get(0)?,
        name: r.get(1)?,
        icon_color: r.get(2)?,
        icon_custom_emoji_id: r.get(3)?,
        state: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

/// Upsert a topic the agent just created. Resets state to 'open' and
/// refreshes metadata. Single-statement write — no transaction needed.
pub async fn upsert_created(
    conn: &Connection,
    chat_id: i64,
    message_thread_id: i64,
    name: &str,
    icon_color: Option<i64>,
    icon_custom_emoji_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO forum_topics
            (chat_id, message_thread_id, name, icon_color, icon_custom_emoji_id, state, updated_at)
         VALUES (?, ?, ?, ?, ?, 'open', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         ON CONFLICT(chat_id, message_thread_id) DO UPDATE SET
            name = excluded.name,
            icon_color = excluded.icon_color,
            icon_custom_emoji_id = excluded.icon_custom_emoji_id,
            state = 'open',
            updated_at = excluded.updated_at",
        crate::params![
            chat_id,
            message_thread_id,
            name,
            icon_color,
            icon_custom_emoji_id
        ],
    )
    .await?;
    Ok(())
}

/// Update name/icon for an existing tracked topic. No-op (0 rows) if the
/// topic is not in the registry (e.g. a human-created topic). `None` fields
/// are left unchanged via COALESCE.
pub async fn update_edited(
    conn: &Connection,
    chat_id: i64,
    message_thread_id: i64,
    name: Option<&str>,
    icon_custom_emoji_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE forum_topics SET
            name = COALESCE(?, name),
            icon_custom_emoji_id = COALESCE(?, icon_custom_emoji_id),
            updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
         WHERE chat_id = ? AND message_thread_id = ?",
        crate::params![name, icon_custom_emoji_id, chat_id, message_thread_id],
    )
    .await?;
    Ok(())
}

/// Set open/closed state. No-op if the topic is not tracked.
pub async fn set_state(
    conn: &Connection,
    chat_id: i64,
    message_thread_id: i64,
    state: ForumTopicState,
) -> Result<()> {
    conn.execute(
        "UPDATE forum_topics SET
            state = ?,
            updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
         WHERE chat_id = ? AND message_thread_id = ?",
        crate::params![state.as_str(), chat_id, message_thread_id],
    )
    .await?;
    Ok(())
}

/// List all tracked topics for ONE chat, newest-updated first. The caller
/// MUST pass the server-resolved current chat id.
pub async fn list(conn: &Connection, chat_id: i64) -> Result<Vec<ForumTopicRow>> {
    conn.query_all(
        "SELECT message_thread_id, name, icon_color, icon_custom_emoji_id, state, updated_at
         FROM forum_topics
         WHERE chat_id = ?
         ORDER BY updated_at DESC, message_thread_id DESC",
        crate::params![chat_id],
        row_to_topic,
    )
    .await
}

#[cfg(test)]
#[path = "forum_topics_tests.rs"]
mod tests;
