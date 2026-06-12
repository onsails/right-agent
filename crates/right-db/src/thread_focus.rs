//! Per-conversation standing "focus" text, keyed by `(chat_id, thread_id)`
//! where `thread_id` is the bot's `effective_thread_id` (DM and General
//! normalize to 0). Two trust-separated columns: `operator_focus` is set by
//! the operator via the dashboard; `agent_focus` is set by the agent via the
//! `thread_focus_set` MCP tool. The MCP layer must always pass the
//! server-resolved current scope and never an agent-supplied value.

use crate::{Connection, Row};

type Result<T> = std::result::Result<T, crate::DbError>;

/// One focus row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadFocus {
    pub operator_focus: Option<String>,
    pub agent_focus: Option<String>,
    pub updated_at: String,
}

fn row_to_focus(r: &Row<'_>) -> Result<ThreadFocus> {
    Ok(ThreadFocus {
        operator_focus: r.get(0)?,
        agent_focus: r.get(1)?,
        updated_at: r.get(2)?,
    })
}

/// Fetch the focus row for one scope, or `None` if unset.
pub async fn get(conn: &Connection, chat_id: i64, thread_id: i64) -> Result<Option<ThreadFocus>> {
    let rows = conn
        .query_all(
            "SELECT operator_focus, agent_focus, updated_at
             FROM thread_focus
             WHERE chat_id = ? AND thread_id = ?",
            crate::params![chat_id, thread_id],
            row_to_focus,
        )
        .await?;
    Ok(rows.into_iter().next())
}

/// Upsert the operator column only. `None` clears it. Single-statement write.
pub async fn set_operator(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
    value: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO thread_focus (chat_id, thread_id, operator_focus, updated_at)
         VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         ON CONFLICT(chat_id, thread_id) DO UPDATE SET
            operator_focus = excluded.operator_focus,
            updated_at = excluded.updated_at",
        crate::params![chat_id, thread_id, value],
    )
    .await?;
    Ok(())
}

/// Upsert the agent column only. `None` clears it. Single-statement write.
pub async fn set_agent(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
    value: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO thread_focus (chat_id, thread_id, agent_focus, updated_at)
         VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         ON CONFLICT(chat_id, thread_id) DO UPDATE SET
            agent_focus = excluded.agent_focus,
            updated_at = excluded.updated_at",
        crate::params![chat_id, thread_id, value],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "thread_focus_tests.rs"]
mod tests;
