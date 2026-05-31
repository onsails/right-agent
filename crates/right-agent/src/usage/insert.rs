//! Insert path — called by worker (interactive) and cron (cron).

use crate::usage::{UsageBreakdown, UsageError};
use chrono::Utc;
use right_db::{Connection, params};

/// Insert a row for an interactive (Telegram worker) invocation.
///
/// `thread_id` is 0 when the message has no thread. `chat_id` may be any valid
/// Telegram chat id (including negative ids for groups).
pub async fn insert_interactive(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(conn, b, "interactive", Some(chat_id), Some(thread_id), None).await
}

/// Insert a row for a cron (or cron-delivery) invocation.
pub async fn insert_cron(
    conn: &Connection,
    b: &UsageBreakdown,
    job_name: &str,
) -> Result<(), UsageError> {
    insert_row(conn, b, "cron", None, None, Some(job_name)).await
}

/// Insert a row for a reflection invocation whose parent was a Telegram worker turn.
pub async fn insert_reflection_worker(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(conn, b, "reflection", Some(chat_id), Some(thread_id), None).await
}

/// Insert a row for a reflection invocation whose parent was a cron job.
pub async fn insert_reflection_cron(
    conn: &Connection,
    b: &UsageBreakdown,
    job_name: &str,
) -> Result<(), UsageError> {
    insert_row(conn, b, "reflection", None, None, Some(job_name)).await
}

/// Insert a row for a per-turn prefilter invocation (Haiku classifier).
///
/// `chat_id` and `thread_id` carry the originating foreground turn so the
/// dashboard can group prefilter spend by chat.
pub async fn insert_learning_prefilter(
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
    .await
}

/// Insert a row for a post-turn probe-writer fork (writes skill files).
///
/// `chat_id` and `thread_id` carry the originating foreground turn so the
/// dashboard can group probe-writer spend by chat.
pub async fn insert_learning_probe_writer(
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
    .await
}

/// Insert a row for a periodic curator pass (no chat context).
pub async fn insert_learning_curator(
    conn: &Connection,
    b: &UsageBreakdown,
) -> Result<(), UsageError> {
    insert_row(conn, b, "learning_curator", None, None, None).await
}

/// Shared INSERT for the `skill_spend` ledger. Bind order:
/// skill_name, kind, cost_usd, cache_read, cache_creation, invocation_id, ts.
const SKILL_SPEND_INSERT_SQL: &str = "INSERT INTO skill_spend \
     (skill_name, kind, cost_usd, cache_read, cache_creation, invocation_id, ts) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

/// Insert one per-skill spend ledger row (create/patch/maintain/usage).
pub async fn insert_skill_spend(
    conn: &Connection,
    skill_name: &str,
    kind: &str,
    cost_usd: f64,
    cache_read: i64,
    cache_creation: i64,
    invocation_id: Option<&str>,
) -> Result<(), UsageError> {
    let ts = Utc::now().to_rfc3339();
    conn.execute(
        SKILL_SPEND_INSERT_SQL,
        params![
            skill_name,
            kind,
            cost_usd,
            cache_read,
            cache_creation,
            invocation_id,
            ts
        ],
    )
    .await?;
    Ok(())
}

/// Insert one `usage` spend row per skill for a single turn, in one immediate
/// transaction (Transaction Rule: 2+ writes go through one transaction). Every
/// row carries the same turn cost/cache — attributed, not exact, when a turn
/// used several skills. `invocation_id` is NULL because foreground usage has no
/// learning invocation.
pub async fn insert_usage_skill_spend_many<I, S>(
    conn: &Connection,
    skill_names: I,
    cost_usd: f64,
    cache_read: i64,
    cache_creation: i64,
) -> Result<(), UsageError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let ts = Utc::now().to_rfc3339();
    let tx = conn.transaction().await?;
    for name in skill_names {
        tx.execute(
            SKILL_SPEND_INSERT_SQL,
            params![
                name.as_ref(),
                "usage",
                cost_usd,
                cache_read,
                cache_creation,
                None::<&str>,
                ts.as_str()
            ],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Record one learning attempt suppressed before it could run (e.g. budget).
pub async fn insert_learning_skip(
    conn: &Connection,
    reason: &str,
    intended_kind: Option<&str>,
    chat_id: Option<i64>,
    thread_id: Option<i64>,
) -> Result<(), UsageError> {
    let ts = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO learning_skip (reason, intended_kind, chat_id, thread_id, ts) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![reason, intended_kind, chat_id, thread_id, ts],
    )
    .await?;
    Ok(())
}

async fn insert_row(
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
    )
    .await?;
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

    #[tokio::test]
    async fn insert_interactive_writes_row() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_interactive(&conn, &sample_breakdown(), 42, 0)
            .await
            .unwrap();

        let (source, chat_id, thread_id, job_name, cost): (String, Option<i64>, Option<i64>, Option<String>, f64) =
            conn.query_row(
                "SELECT source, chat_id, thread_id, job_name, total_cost_usd FROM usage_events LIMIT 1",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            ).await.unwrap();
        assert_eq!(source, "interactive");
        assert_eq!(chat_id, Some(42));
        assert_eq!(thread_id, Some(0));
        assert_eq!(job_name, None);
        assert!((cost - 0.05).abs() < 1e-9);
    }

    #[tokio::test]
    async fn insert_cron_writes_row_with_null_chat() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_cron(&conn, &sample_breakdown(), "my-job")
            .await
            .unwrap();

        let (source, chat_id, job_name): (String, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT source, chat_id, job_name FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(source, "cron");
        assert_eq!(chat_id, None);
        assert_eq!(job_name, Some("my-job".into()));
    }

    #[tokio::test]
    async fn insert_persists_api_key_source() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        let mut b = sample_breakdown();
        b.api_key_source = "ANTHROPIC_API_KEY".into();
        insert_interactive(&conn, &b, 1, 0).await.unwrap();

        let src: String = conn
            .query_row("SELECT api_key_source FROM usage_events LIMIT 1", [], |r| {
                r.get(0)
            })
            .await
            .unwrap();
        assert_eq!(src, "ANTHROPIC_API_KEY");
    }

    #[tokio::test]
    async fn insert_default_api_key_source_is_none() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        // sample_breakdown sets api_key_source="none".
        insert_interactive(&conn, &sample_breakdown(), 1, 0)
            .await
            .unwrap();

        let src: String = conn
            .query_row("SELECT api_key_source FROM usage_events LIMIT 1", [], |r| {
                r.get(0)
            })
            .await
            .unwrap();
        assert_eq!(src, "none");
    }

    #[tokio::test]
    async fn insert_preserves_all_token_counts() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_interactive(&conn, &sample_breakdown(), 1, 0)
            .await
            .unwrap();
        let (inp, out, cc, cr, ws, wf): (i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, web_search_requests, web_fetch_requests FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .await
            .unwrap();
        assert_eq!((inp, out, cc, cr, ws, wf), (10, 20, 100, 200, 1, 2));
    }

    #[tokio::test]
    async fn insert_reflection_from_worker_has_chat_id() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_reflection_worker(&conn, &sample_breakdown(), 42, 7)
            .await
            .unwrap();

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
            .await
            .unwrap();
        assert_eq!(source, "reflection");
        assert_eq!(chat_id, Some(42));
        assert_eq!(thread_id, Some(7));
        assert_eq!(job_name, None);
    }

    #[tokio::test]
    async fn insert_reflection_from_cron_has_job_name() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_reflection_cron(&conn, &sample_breakdown(), "my-job")
            .await
            .unwrap();

        let (source, chat_id, job_name): (String, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT source, chat_id, job_name FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(source, "reflection");
        assert_eq!(chat_id, None);
        assert_eq!(job_name, Some("my-job".to_string()));
    }

    #[tokio::test]
    async fn insert_learning_prefilter_writes_row_with_correct_source() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_learning_prefilter(&conn, &sample_breakdown(), 1234, 0)
            .await
            .unwrap();
        let (source, chat_id, thread_id): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT source, chat_id, thread_id FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(source, "learning_prefilter");
        assert_eq!(chat_id, Some(1234));
        assert_eq!(thread_id, Some(0));
    }

    #[tokio::test]
    async fn insert_learning_probe_writer_writes_row_with_correct_source() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_learning_probe_writer(&conn, &sample_breakdown(), 1234, 0)
            .await
            .unwrap();
        let (source, chat_id, thread_id): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT source, chat_id, thread_id FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(source, "learning_probe_writer");
        assert_eq!(chat_id, Some(1234));
        assert_eq!(thread_id, Some(0));
    }

    #[tokio::test]
    async fn insert_learning_curator_writes_row_with_null_chat() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_learning_curator(&conn, &sample_breakdown())
            .await
            .unwrap();
        let (source, chat_id, thread_id): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT source, chat_id, thread_id FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(source, "learning_curator");
        assert_eq!(chat_id, None);
        assert_eq!(thread_id, None);
    }

    #[tokio::test]
    async fn insert_threads_wall_elapsed_ms_when_set() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        let mut b = sample_breakdown();
        b.wall_elapsed_ms = Some(12345);
        insert_interactive(&conn, &b, 1, 0).await.unwrap();

        let elapsed: Option<i64> = conn
            .query_row(
                "SELECT wall_elapsed_ms FROM usage_events LIMIT 1",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(elapsed, Some(12345));
    }

    #[tokio::test]
    async fn insert_keeps_wall_elapsed_ms_null_when_none() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_learning_curator(&conn, &sample_breakdown())
            .await
            .unwrap();
        let elapsed: Option<i64> = conn
            .query_row(
                "SELECT wall_elapsed_ms FROM usage_events LIMIT 1",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(elapsed, None);
    }

    #[tokio::test]
    async fn insert_skill_spend_writes_row() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_skill_spend(&conn, "rightx-foo", "create", 0.25, 100, 200, Some("inv-1"))
            .await
            .unwrap();
        let (name, kind, cost, cr, cc, inv): (String, String, f64, i64, i64, Option<String>) = conn
            .query_row(
                "SELECT skill_name, kind, cost_usd, cache_read, cache_creation, invocation_id \
                 FROM skill_spend LIMIT 1",
                (),
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .await
            .unwrap();
        assert_eq!(
            (name.as_str(), kind.as_str(), cost, cr, cc, inv.as_deref()),
            ("rightx-foo", "create", 0.25, 100, 200, Some("inv-1"))
        );
    }

    #[tokio::test]
    async fn insert_usage_skill_spend_many_writes_one_usage_row_per_skill() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_usage_skill_spend_many(&conn, ["rightx-alpha", "rightx-beta"], 0.12, 10, 20)
            .await
            .unwrap();

        let rows: Vec<(String, String, f64, i64, i64, Option<String>)> = conn
            .query_all(
                "SELECT skill_name, kind, cost_usd, cache_read, cache_creation, invocation_id \
                 FROM skill_spend ORDER BY skill_name",
                (),
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        for (i, name) in ["rightx-alpha", "rightx-beta"].iter().enumerate() {
            let (skill, kind, cost, cr, cc, inv) = &rows[i];
            assert_eq!(skill, name);
            assert_eq!(kind, "usage");
            assert!((cost - 0.12).abs() < 1e-9);
            assert_eq!((*cr, *cc), (10, 20));
            assert_eq!(inv.as_deref(), None);
        }
    }

    #[tokio::test]
    async fn insert_usage_skill_spend_many_empty_writes_nothing() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_usage_skill_spend_many(&conn, Vec::<String>::new(), 0.5, 1, 2)
            .await
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM skill_spend", (), |r| r.get(0))
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn insert_learning_skip_writes_budget_row_with_null_kind() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_learning_skip(&conn, "budget", None, Some(42), Some(0))
            .await
            .unwrap();
        let (reason, kind, chat): (String, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT reason, intended_kind, chat_id FROM learning_skip LIMIT 1",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!((reason.as_str(), kind, chat), ("budget", None, Some(42)));
    }
}
