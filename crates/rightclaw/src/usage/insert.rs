//! Insert path — called by worker (interactive) and cron (cron).

use crate::usage::{UsageBreakdown, UsageError};
use chrono::Utc;
use rusqlite::{Connection, params};

/// Insert a row for an interactive (Telegram worker) invocation.
///
/// `thread_id` is 0 when the message has no thread. `chat_id` may be any valid
/// Telegram chat id (including negative ids for groups).
pub fn insert_interactive(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(conn, b, "interactive", Some(chat_id), Some(thread_id), None)
}

/// Insert a row for a cron (or cron-delivery) invocation.
pub fn insert_cron(
    conn: &Connection,
    b: &UsageBreakdown,
    job_name: &str,
) -> Result<(), UsageError> {
    insert_row(conn, b, "cron", None, None, Some(job_name))
}

fn insert_row(
    conn: &Connection,
    b: &UsageBreakdown,
    source: &str,
    chat_id: Option<i64>,
    thread_id: Option<i64>,
    job_name: Option<&str>,
) -> Result<(), UsageError> {
    let ts = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name,
            session_uuid, total_cost_usd, num_turns,
            input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens,
            web_search_requests, web_fetch_requests,
            model_usage_json
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, ?7, ?8,
            ?9, ?10,
            ?11, ?12,
            ?13, ?14,
            ?15
         )",
        params![
            ts,
            source,
            chat_id,
            thread_id,
            job_name,
            b.session_uuid,
            b.total_cost_usd,
            b.num_turns as i64,
            b.input_tokens as i64,
            b.output_tokens as i64,
            b.cache_creation_tokens as i64,
            b.cache_read_tokens as i64,
            b.web_search_requests as i64,
            b.web_fetch_requests as i64,
            b.model_usage_json,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::open_connection;
    use tempfile::tempdir;

    fn sample_breakdown() -> UsageBreakdown {
        UsageBreakdown {
            session_uuid: "uuid-1".into(),
            total_cost_usd: 0.05,
            num_turns: 3,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 100,
            cache_read_tokens: 200,
            web_search_requests: 1,
            web_fetch_requests: 2,
            model_usage_json: r#"{"claude-sonnet-4-6":{"costUSD":0.05}}"#.into(),
            api_key_source: "none".into(),
        }
    }

    #[test]
    fn insert_interactive_writes_row() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &sample_breakdown(), 42, 0).unwrap();

        let (source, chat_id, thread_id, job_name, cost): (String, Option<i64>, Option<i64>, Option<String>, f64) =
            conn.query_row(
                "SELECT source, chat_id, thread_id, job_name, total_cost_usd FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            ).unwrap();
        assert_eq!(source, "interactive");
        assert_eq!(chat_id, Some(42));
        assert_eq!(thread_id, Some(0));
        assert_eq!(job_name, None);
        assert!((cost - 0.05).abs() < 1e-9);
    }

    #[test]
    fn insert_cron_writes_row_with_null_chat() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_cron(&conn, &sample_breakdown(), "my-job").unwrap();

        let (source, chat_id, job_name): (String, Option<i64>, Option<String>) =
            conn.query_row(
                "SELECT source, chat_id, job_name FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).unwrap();
        assert_eq!(source, "cron");
        assert_eq!(chat_id, None);
        assert_eq!(job_name, Some("my-job".into()));
    }

    #[test]
    fn insert_preserves_all_token_counts() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &sample_breakdown(), 1, 0).unwrap();
        let (inp, out, cc, cr, ws, wf): (i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, web_search_requests, web_fetch_requests FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!((inp, out, cc, cr, ws, wf), (10, 20, 100, 200, 1, 2));
    }
}
