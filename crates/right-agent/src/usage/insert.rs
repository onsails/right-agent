//! Insert path — called by worker (interactive) and cron (cron).

use crate::usage::{UsageBreakdown, UsageError};
use chrono::Utc;
use right_db::{Connection, params};

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

/// Insert a row for a reflection invocation whose parent was a Telegram worker turn.
pub fn insert_reflection_worker(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(conn, b, "reflection", Some(chat_id), Some(thread_id), None)
}

/// Insert a row for a reflection invocation whose parent was a cron job.
pub fn insert_reflection_cron(
    conn: &Connection,
    b: &UsageBreakdown,
    job_name: &str,
) -> Result<(), UsageError> {
    insert_row(conn, b, "reflection", None, None, Some(job_name))
}

/// Insert a row for a Stage 2 learning-episode selector invocation.
/// `episode_id` is stored in `job_name` so the dashboard can link the usage
/// row back to the episode detail view without a separate column.
pub fn insert_learning_selector(
    conn: &Connection,
    b: &UsageBreakdown,
    episode_id: i64,
) -> Result<(), UsageError> {
    let job = episode_id.to_string();
    insert_row(conn, b, "learning_selector", None, None, Some(job.as_str()))
}

/// Insert a row for a Stage 2 learning-episode reviewer invocation.
/// `episode_id` is stored in `job_name` (see `insert_learning_selector`).
pub fn insert_learning_reviewer(
    conn: &Connection,
    b: &UsageBreakdown,
    episode_id: i64,
) -> Result<(), UsageError> {
    let job = episode_id.to_string();
    insert_row(conn, b, "learning_reviewer", None, None, Some(job.as_str()))
}

/// Insert a row for a worker-side learned-skill review invocation.
pub fn insert_learning_skill_review(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(
        conn,
        b,
        "learning_skill_review",
        Some(chat_id),
        Some(thread_id),
        None,
    )
}

/// Insert a row for a per-turn prefilter invocation (Haiku classifier).
///
/// `chat_id` and `thread_id` carry the originating foreground turn so the
/// dashboard can group prefilter spend by chat.
pub fn insert_learning_prefilter(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(
        conn,
        b,
        "learning_prefilter",
        Some(chat_id),
        Some(thread_id),
        None,
    )
}

/// Insert a row for a post-turn probe-writer fork (writes skill files).
///
/// `chat_id` and `thread_id` carry the originating foreground turn so the
/// dashboard can group probe-writer spend by chat.
pub fn insert_learning_probe_writer(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(
        conn,
        b,
        "learning_probe_writer",
        Some(chat_id),
        Some(thread_id),
        None,
    )
}

/// Insert a row for a periodic curator pass (no chat context).
pub fn insert_learning_curator(conn: &Connection, b: &UsageBreakdown) -> Result<(), UsageError> {
    insert_row(conn, b, "learning_curator", None, None, None)
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
            model_usage_json, api_key_source,
            wall_elapsed_ms
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, ?7, ?8,
            ?9, ?10,
            ?11, ?12,
            ?13, ?14,
            ?15, ?16,
            ?17
         )",
        params![
            ts,
            source,
            chat_id,
            thread_id,
            job_name,
            b.session_uuid.as_str(),
            b.total_cost_usd,
            b.num_turns as i64,
            b.input_tokens as i64,
            b.output_tokens as i64,
            b.cache_creation_tokens as i64,
            b.cache_read_tokens as i64,
            b.web_search_requests as i64,
            b.web_fetch_requests as i64,
            b.model_usage_json.as_str(),
            b.api_key_source.as_str(),
            b.wall_elapsed_ms.map(|v| v as i64),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use right_db::open_connection;
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
            wall_elapsed_ms: None,
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
                (),
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

        let (source, chat_id, job_name): (String, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT source, chat_id, job_name FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(source, "cron");
        assert_eq!(chat_id, None);
        assert_eq!(job_name, Some("my-job".into()));
    }

    #[test]
    fn insert_persists_api_key_source() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        let mut b = sample_breakdown();
        b.api_key_source = "ANTHROPIC_API_KEY".into();
        insert_interactive(&conn, &b, 1, 0).unwrap();

        let src: String = conn
            .query_row("SELECT api_key_source FROM usage_events LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(src, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn insert_default_api_key_source_is_none() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        // sample_breakdown sets api_key_source="none".
        insert_interactive(&conn, &sample_breakdown(), 1, 0).unwrap();

        let src: String = conn
            .query_row("SELECT api_key_source FROM usage_events LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(src, "none");
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

    #[test]
    fn insert_reflection_from_worker_has_chat_id() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_reflection_worker(&conn, &sample_breakdown(), 42, 7).unwrap();

        let (source, chat_id, thread_id, job_name): (
            String,
            Option<i64>,
            Option<i64>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT source, chat_id, thread_id, job_name FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(source, "reflection");
        assert_eq!(chat_id, Some(42));
        assert_eq!(thread_id, Some(7));
        assert_eq!(job_name, None);
    }

    #[test]
    fn insert_reflection_from_cron_has_job_name() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_reflection_cron(&conn, &sample_breakdown(), "my-job").unwrap();

        let (source, chat_id, job_name): (String, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT source, chat_id, job_name FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(source, "reflection");
        assert_eq!(chat_id, None);
        assert_eq!(job_name, Some("my-job".to_string()));
    }

    #[test]
    fn insert_learning_selector_writes_row_with_correct_source_and_job_name() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_learning_selector(&conn, &sample_breakdown(), 42).unwrap();
        let (source, job_name): (String, Option<String>) = conn
            .query_row(
                "SELECT source, job_name FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(source, "learning_selector");
        assert_eq!(job_name.as_deref(), Some("42"));
    }

    #[test]
    fn insert_learning_reviewer_writes_row_with_correct_source_and_job_name() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_learning_reviewer(&conn, &sample_breakdown(), 99).unwrap();
        let (source, job_name): (String, Option<String>) = conn
            .query_row(
                "SELECT source, job_name FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(source, "learning_reviewer");
        assert_eq!(job_name.as_deref(), Some("99"));
    }

    #[test]
    fn insert_learning_skill_review_writes_row_with_correct_source_chat_thread() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_learning_skill_review(&conn, &sample_breakdown(), -1001, 7).unwrap();
        let (source, chat_id, thread_id): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT source, chat_id, thread_id FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(source, "learning_skill_review");
        assert_eq!(chat_id, Some(-1001));
        assert_eq!(thread_id, Some(7));
    }

    #[test]
    fn insert_learning_prefilter_writes_row_with_correct_source() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_learning_prefilter(&conn, &sample_breakdown(), 1234, 0).unwrap();
        let (source, chat_id, thread_id): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT source, chat_id, thread_id FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(source, "learning_prefilter");
        assert_eq!(chat_id, Some(1234));
        assert_eq!(thread_id, Some(0));
    }

    #[test]
    fn insert_learning_probe_writer_writes_row_with_correct_source() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_learning_probe_writer(&conn, &sample_breakdown(), 1234, 0).unwrap();
        let (source, chat_id, thread_id): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT source, chat_id, thread_id FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(source, "learning_probe_writer");
        assert_eq!(chat_id, Some(1234));
        assert_eq!(thread_id, Some(0));
    }

    #[test]
    fn insert_learning_curator_writes_row_with_null_chat() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_learning_curator(&conn, &sample_breakdown()).unwrap();
        let (source, chat_id, thread_id): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT source, chat_id, thread_id FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(source, "learning_curator");
        assert_eq!(chat_id, None);
        assert_eq!(thread_id, None);
    }

    #[test]
    fn insert_threads_wall_elapsed_ms_when_set() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        let mut b = sample_breakdown();
        b.wall_elapsed_ms = Some(12345);
        insert_interactive(&conn, &b, 1, 0).unwrap();

        let elapsed: Option<i64> = conn
            .query_row(
                "SELECT wall_elapsed_ms FROM usage_events LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(elapsed, Some(12345));
    }

    #[test]
    fn insert_keeps_wall_elapsed_ms_null_when_none() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_learning_curator(&conn, &sample_breakdown()).unwrap();
        let elapsed: Option<i64> = conn
            .query_row(
                "SELECT wall_elapsed_ms FROM usage_events LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(elapsed, None);
    }
}
