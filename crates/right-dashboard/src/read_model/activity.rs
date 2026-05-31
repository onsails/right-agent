use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader};

use crate::api_types::{
    ActiveActivity, CronCard, ForegroundActivity, LogExcerpt, OverviewResponse, OverviewSummary,
    RunDetailResponse, RunSummary,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use right_db::{Connection, OptionalExtension, params};

use super::run_summary::{RUN_SUMMARY_COLUMNS, RUN_SUMMARY_FROM, run_summary_from_row};
use super::{ReadModelError, parse_utc};

pub struct ActivityOverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
    pub foreground: Vec<ForegroundActivity>,
}

const ACTIVE_BACKGROUND_RUN_LIMIT: usize = 50;

pub async fn activity_overview(
    conn: &Connection,
    input: ActivityOverviewInput,
) -> Result<OverviewResponse, ReadModelError> {
    let mut crons_stmt = conn.prepare(
        "SELECT job_name, schedule, recurring, run_at, target_chat_id, target_thread_id,
                max_budget_usd
         FROM cron_specs
         ORDER BY job_name",
    )?;
    let cron_rows = crons_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, f64>(6)?,
            ))
        })
        .await?
        .collect::<Result<Vec<_>, _>>()?;

    let mut crons = Vec::with_capacity(cron_rows.len());
    for (job_name, schedule, recurring, run_at, target_chat_id, target_thread_id, max_budget_usd) in
        cron_rows
    {
        let recent_runs = cron_runs(conn, &job_name).await?;
        crons.push(CronCard {
            job_name,
            schedule,
            recurring,
            run_at,
            target_chat_id,
            target_thread_id,
            max_budget_usd,
            last_run: recent_runs.first().cloned(),
            recent_runs,
        });
    }

    let active_background = active_background_runs(conn).await?;
    let today_cost_usd = today_cost_usd(conn, &input.generated_at).await?;
    let active_cron_count = crons
        .iter()
        .filter(|cron| {
            cron.recent_runs
                .iter()
                .any(|run| is_active_status(&run.status))
        })
        .count();
    let (failed_recent_cron_count, failed_runs) =
        failed_cron_runs(conn, &input.generated_at).await?;

    Ok(OverviewResponse {
        agent: input.agent,
        generated_at: input.generated_at,
        refresh_interval_secs: input.refresh_interval_secs,
        summary: OverviewSummary {
            cron_count: crons.len(),
            active_cron_count,
            failed_recent_cron_count,
            today_cost_usd,
        },
        crons,
        failed_runs,
        active: ActiveActivity {
            foreground: input.foreground,
            background: active_background,
        },
    })
}

pub async fn activity_run_detail(
    conn: &Connection,
    run_id: &str,
    max_lines: usize,
) -> Result<Option<RunDetailResponse>, ReadModelError> {
    let sql = format!(
        "SELECT {RUN_SUMMARY_COLUMNS}, ar.delivery_json, ar.error_json, ar.log_path
         {RUN_SUMMARY_FROM}
         WHERE ar.id = ?1"
    );
    let Some((run, delivery_json, error_json, log_path)) = conn
        .query_row(&sql, params![run_id], |row| {
            Ok((
                run_summary_from_row(row)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
            ))
        })
        .await
        .optional()?
    else {
        return Ok(None);
    };
    let run_note = run.run_note.clone();
    let (delivery, delivery_error) = parse_delivery_json(delivery_json);
    let error_message = extract_error_message(error_json);

    Ok(Some(RunDetailResponse {
        run,
        run_note,
        delivery,
        delivery_error,
        error_message,
        log: read_log_excerpt(log_path, max_lines)?,
    }))
}

async fn cron_runs(conn: &Connection, job_name: &str) -> Result<Vec<RunSummary>, ReadModelError> {
    let sql = format!(
        "SELECT {RUN_SUMMARY_COLUMNS}
         {RUN_SUMMARY_FROM}
         WHERE ar.kind = 'cron' AND ar.producer_ref = ?1
         ORDER BY COALESCE(ar.started_at, ar.created_at) DESC, ar.created_at DESC
         LIMIT 5"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![job_name], run_summary_from_row)
        .await?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

async fn active_background_runs(conn: &Connection) -> Result<Vec<RunSummary>, ReadModelError> {
    let sql = format!(
        "SELECT {RUN_SUMMARY_COLUMNS}
         {RUN_SUMMARY_FROM}
         WHERE ar.kind = 'background' AND ar.status IN ('queued', 'running')
         ORDER BY COALESCE(ar.started_at, ar.created_at) DESC, ar.created_at DESC
         LIMIT ?1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(
            params![ACTIVE_BACKGROUND_RUN_LIMIT as i64],
            run_summary_from_row,
        )
        .await?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(super) async fn today_cost_usd(
    conn: &Connection,
    generated_at: &str,
) -> Result<f64, ReadModelError> {
    // Writers emit `chrono::Utc::now().to_rfc3339()` (e.g.
    // `2026-05-20T08:01:00.123456789+00:00`). SQLite compares timestamps as
    // strings, so the threshold must use the same format. A literal
    // `YYYY-MM-DDT00:00:00Z` excludes rows at start-of-day because `.` < `Z`.
    let generated_at_utc: DateTime<Utc> =
        DateTime::parse_from_rfc3339(generated_at)?.with_timezone(&Utc);
    let date = generated_at_utc.date_naive();
    let start_of_day = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ReadModelError::InvalidStartOfDay(generated_at.to_owned()))?;
    let since = Utc.from_utc_datetime(&start_of_day).to_rfc3339();
    let cost = conn
        .query_row(
            "SELECT COALESCE(SUM(total_cost_usd), 0.0)
         FROM usage_events
         WHERE ts >= ?1",
            params![since],
            |row| row.get(0),
        )
        .await?;
    Ok(cost)
}

fn parse_delivery_json(json: Option<String>) -> (Option<serde_json::Value>, Option<String>) {
    let Some(json) = json else {
        return (None, None);
    };
    match serde_json::from_str(&json) {
        Ok(delivery) => (Some(delivery), None),
        Err(error) => {
            tracing::warn!(error = %error, "dashboard: malformed delivery_json");
            (
                None,
                Some(format!("failed to parse delivery_json: {error}")),
            )
        }
    }
}

/// Extract a human-readable error message from an `async_runs.error_json`
/// row. Returns `None` when there is no error_json. When the value parses as
/// JSON with a string `reason` field, returns that reason; otherwise returns
/// the raw stored text so debuggability is preserved.
fn extract_error_message(json: Option<String>) -> Option<String> {
    let json = json?;
    match serde_json::from_str::<serde_json::Value>(&json) {
        Ok(value) => value
            .get("reason")
            .and_then(|reason| reason.as_str())
            .map(|reason| reason.to_owned())
            .or(Some(json)),
        Err(_) => Some(json),
    }
}

fn read_log_excerpt(path: Option<String>, max_lines: usize) -> Result<LogExcerpt, ReadModelError> {
    let Some(path) = path else {
        return Ok(LogExcerpt {
            available: false,
            path: None,
            lines: Vec::new(),
            truncated: false,
        });
    };

    if max_lines == 0 {
        let available = std::fs::metadata(&path).is_ok();
        return Ok(LogExcerpt {
            available,
            path: Some(path),
            lines: Vec::new(),
            truncated: false,
        });
    }

    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LogExcerpt {
                available: false,
                path: Some(path),
                lines: Vec::new(),
                truncated: false,
            });
        }
        Err(error) => return Err(ReadModelError::from(error)),
    };
    let mut tail = VecDeque::with_capacity(max_lines);
    let mut line_count = 0usize;
    for line in BufReader::new(file).lines() {
        let line = line?;
        line_count += 1;
        if tail.len() == max_lines {
            tail.pop_front();
        }
        tail.push_back(line);
    }

    Ok(LogExcerpt {
        available: true,
        path: Some(path),
        lines: tail.into_iter().collect(),
        truncated: line_count > max_lines,
    })
}

async fn failed_cron_runs(
    conn: &Connection,
    generated_at: &str,
) -> Result<(usize, Vec<RunSummary>), ReadModelError> {
    let now = parse_utc(generated_at)?;
    let since = now - Duration::days(7);
    super::run_summary::failed_runs_in_window(conn, &now, &since, Some("cron")).await
}

fn is_active_status(status: &str) -> bool {
    matches!(status, "queued" | "running")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_types::ForegroundActivity;
    use serde_json::json;
    use std::fs;
    use tempfile::{TempDir, tempdir};

    async fn fixture() -> (TempDir, Connection) {
        let dir = tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true)
            .await
            .expect("open db");

        conn.execute(
            "INSERT INTO cron_specs (
                job_name, schedule, prompt, max_budget_usd, created_at, updated_at,
                recurring, target_chat_id, target_thread_id
             ) VALUES (
                'daily', '0 8 * * *', 'daily prompt', 2.5,
                '2026-05-20T00:00:00Z', '2026-05-20T00:00:00Z',
                1, 123, 456
             )",
            [],
        )
        .await
        .expect("insert cron spec");

        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                target_thread_id, status, started_at, finished_at, exit_code,
                delivery_required, delivery_status, created_at, updated_at
             ) VALUES (
                'run-1', 'cron', 'daily', 'run-1', 123, 456, 'success',
                '2026-05-20T08:00:00Z', '2026-05-20T08:01:00Z', 0,
                1, 'delivered', '2026-05-20T08:00:00Z', '2026-05-20T08:01:00Z'
             )",
            [],
        )
        .await
        .expect("insert completed cron run");

        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, started_at, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'bg-1', 'background', 'handoff', 'bg-session-1', 123,
                'running', '2026-05-20T09:00:00Z', 1, 'pending',
                '2026-05-20T09:00:00Z', '2026-05-20T09:00:00Z'
             )",
            [],
        )
        .await
        .expect("insert running background run");

        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, model_usage_json
             ) VALUES (
                '2026-05-20T08:01:00Z', 'cron', 123, 456, 'daily', 'run-1',
                0.25, 1, '{}'
             )",
            [],
        )
        .await
        .expect("insert usage event");

        (dir, conn)
    }

    #[tokio::test]
    async fn activity_overview_builds_cron_cards_and_active_background() {
        let (_dir, conn) = fixture().await;
        let response = activity_overview(
            &conn,
            ActivityOverviewInput {
                agent: "agent-a".to_owned(),
                generated_at: "2026-05-20T10:00:00Z".to_owned(),
                refresh_interval_secs: 30,
                foreground: vec![ForegroundActivity {
                    chat_id: 123,
                    thread_id: 456,
                    turn_id: 7,
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(response.summary.cron_count, 1);
        assert_eq!(response.summary.today_cost_usd, 0.25);
        assert_eq!(response.crons[0].last_run.as_ref().unwrap().id, "run-1");
        assert_eq!(response.crons[0].recent_runs[0].id, "run-1");
        assert_eq!(response.active.background[0].id, "bg-1");
    }

    #[tokio::test]
    async fn activity_overview_includes_cron_run_summary_delivery_and_note_fields() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "UPDATE async_runs
             SET delivery_json = ?1, run_note = ?2
             WHERE id = 'run-1'",
            (r#"{"kind":"notify","content":"done"}"#, "Checked 5 pairs"),
        )
        .await
        .unwrap();

        let response = activity_overview(
            &conn,
            ActivityOverviewInput {
                agent: "agent-a".to_owned(),
                generated_at: "2026-05-20T10:00:00Z".to_owned(),
                refresh_interval_secs: 30,
                foreground: vec![],
            },
        )
        .await
        .unwrap();

        let run = response.crons[0].last_run.as_ref().unwrap();
        assert!(run.delivery_required);
        assert_eq!(run.delivery_kind.as_deref(), Some("notify"));
        assert_eq!(run.run_note.as_deref(), Some("Checked 5 pairs"));
    }

    #[tokio::test]
    async fn activity_overview_ignores_malformed_cron_run_delivery_json_kind() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "UPDATE async_runs SET delivery_json = ?1 WHERE id = 'run-1'",
            [r#"{malformed"#],
        )
        .await
        .unwrap();

        let response = activity_overview(
            &conn,
            ActivityOverviewInput {
                agent: "agent-a".to_owned(),
                generated_at: "2026-05-20T10:00:00Z".to_owned(),
                refresh_interval_secs: 30,
                foreground: vec![],
            },
        )
        .await
        .unwrap();

        let run = response.crons[0].last_run.as_ref().unwrap();
        assert!(run.delivery_required);
        assert!(run.delivery_kind.is_none());
    }

    #[tokio::test]
    async fn activity_today_cost_includes_events_at_midnight_in_writer_format() {
        // Regression: the writer emits `chrono::Utc::now().to_rfc3339()`
        // (e.g. `2026-05-20T00:00:00.123456789+00:00`). A naive
        // `YYYY-MM-DDT00:00:00Z` threshold compares `.` < `Z` at position 19
        // and excludes start-of-day rows.
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, model_usage_json
             ) VALUES (
                '2026-05-20T00:00:00.123456789+00:00', 'cron', 123, 456, 'daily', 'run-midnight',
                0.50, 1, '{}'
             )",
            [],
        )
        .await
        .expect("insert midnight usage event");

        let response = activity_overview(
            &conn,
            ActivityOverviewInput {
                agent: "agent-a".to_owned(),
                generated_at: "2026-05-20T10:00:00+00:00".to_owned(),
                refresh_interval_secs: 30,
                foreground: vec![],
            },
        )
        .await
        .unwrap();

        // 0.25 (fixture) + 0.50 (midnight) = 0.75
        assert!(
            (response.summary.today_cost_usd - 0.75).abs() < 1e-9,
            "today_cost_usd = {}",
            response.summary.today_cost_usd
        );
    }

    #[tokio::test]
    async fn activity_run_detail_handles_missing_runs_and_missing_logs() {
        let (_dir, conn) = fixture().await;

        assert!(
            activity_run_detail(&conn, "missing", 20)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !activity_run_detail(&conn, "run-1", 20)
                .await
                .unwrap()
                .unwrap()
                .log
                .available
        );
    }

    #[tokio::test]
    async fn activity_run_detail_parses_valid_delivery_json() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "UPDATE async_runs SET delivery_json = ?1 WHERE id = 'run-1'",
            [r#"{"kind":"notify","content":"done"}"#],
        )
        .await
        .unwrap();

        let detail = activity_run_detail(&conn, "run-1", 20)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            detail.delivery,
            Some(json!({"kind": "notify", "content": "done"}))
        );
        assert!(detail.delivery_error.is_none());
    }

    #[tokio::test]
    async fn activity_run_detail_preserves_logs_for_malformed_delivery_json() {
        let (dir, conn) = fixture().await;
        let log_path = dir.path().join("run-1.log");
        fs::write(&log_path, "first\nsecond\n").unwrap();
        conn.execute(
            "UPDATE async_runs SET delivery_json = ?1, log_path = ?2 WHERE id = 'run-1'",
            (r#"{malformed"#, log_path.to_string_lossy().as_ref()),
        )
        .await
        .unwrap();

        let detail = activity_run_detail(&conn, "run-1", 20)
            .await
            .unwrap()
            .unwrap();

        assert!(detail.delivery.is_none());
        assert!(
            detail
                .delivery_error
                .as_deref()
                .is_some_and(|error| error.contains("failed to parse delivery_json")),
            "unexpected delivery_error: {:?}",
            detail.delivery_error
        );
        assert!(detail.log.available);
        assert_eq!(
            detail.log.lines,
            vec!["first".to_owned(), "second".to_owned()]
        );
    }

    #[tokio::test]
    async fn activity_run_detail_surfaces_error_message_from_error_json() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "UPDATE async_runs SET error_json = ?1 WHERE id = 'run-1'",
            [r#"{"kind":"cron_parse_failed","reason":"missing result stream"}"#],
        )
        .await
        .unwrap();

        let detail = activity_run_detail(&conn, "run-1", 20)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            detail.error_message.as_deref(),
            Some("missing result stream")
        );
        assert!(detail.delivery.is_none());
        assert!(detail.delivery_error.is_none());
    }

    #[tokio::test]
    async fn activity_run_detail_surfaces_raw_error_json_when_no_reason() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "UPDATE async_runs SET error_json = ?1 WHERE id = 'run-1'",
            [r#"not-json"#],
        )
        .await
        .unwrap();

        let detail = activity_run_detail(&conn, "run-1", 20)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(detail.error_message.as_deref(), Some("not-json"));
    }

    #[tokio::test]
    async fn activity_run_detail_omits_error_message_when_error_json_null() {
        let (_dir, conn) = fixture().await;

        let detail = activity_run_detail(&conn, "run-1", 20)
            .await
            .unwrap()
            .unwrap();

        assert!(detail.error_message.is_none());
    }

    #[tokio::test]
    async fn activity_run_detail_tails_existing_log_file() {
        let (dir, conn) = fixture().await;
        let log_path = dir.path().join("run-1.log");
        fs::write(&log_path, "one\ntwo\nthree\nfour\n").unwrap();
        conn.execute(
            "UPDATE async_runs SET log_path = ?1 WHERE id = 'run-1'",
            [log_path.to_string_lossy().as_ref()],
        )
        .await
        .unwrap();

        let detail = activity_run_detail(&conn, "run-1", 2)
            .await
            .unwrap()
            .unwrap();

        assert!(detail.log.available);
        assert_eq!(
            detail.log.lines,
            vec!["three".to_owned(), "four".to_owned()]
        );
        assert!(detail.log.truncated);
    }

    #[tokio::test]
    async fn failed_runs_lists_failed_cron_runs_in_window_and_matches_count() {
        let (_dir, conn) = fixture().await;

        // Two failed cron runs within the 7d window
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, finished_at, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-f1', 'cron', 'job-a', 'run-f1', 123,
                'failed', '2026-05-31T10:00:00Z', 0, 'none',
                '2026-05-31T10:00:00Z', '2026-05-31T10:00:00Z'
             )",
            [],
        )
        .await
        .expect("insert failed cron run 1");

        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, finished_at, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-f2', 'cron', 'job-a', 'run-f2', 123,
                'failed', '2026-05-31T11:00:00Z', 0, 'none',
                '2026-05-31T11:00:00Z', '2026-05-31T11:00:00Z'
             )",
            [],
        )
        .await
        .expect("insert failed cron run 2");

        // A completed cron run — must not appear
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, finished_at, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-c1', 'cron', 'job-b', 'run-c1', 123,
                'completed', '2026-05-31T11:30:00Z', 0, 'none',
                '2026-05-31T11:30:00Z', '2026-05-31T11:30:00Z'
             )",
            [],
        )
        .await
        .expect("insert completed cron run");

        let response = activity_overview(
            &conn,
            ActivityOverviewInput {
                agent: "agent-a".to_owned(),
                generated_at: "2026-05-31T12:00:00Z".to_owned(),
                refresh_interval_secs: 30,
                foreground: vec![],
            },
        )
        .await
        .unwrap();

        assert_eq!(response.failed_runs.len(), 2);
        assert_eq!(
            response.summary.failed_recent_cron_count,
            response.failed_runs.len()
        );
        // newest first
        assert_eq!(response.failed_runs[0].id, "run-f2");
    }

    #[tokio::test]
    async fn failed_cron_runs_caps_at_sample_limit_with_true_count() {
        let (_dir, conn) = fixture().await;
        // 51 failed cron runs in the 7d window (> FAILURE_SAMPLE_LIMIT = 50).
        for i in 0..51 {
            conn.execute(
                "INSERT INTO async_runs (
                    id, kind, producer_ref, run_session_id, target_chat_id,
                    status, finished_at, delivery_required, delivery_status,
                    created_at, updated_at
                 ) VALUES (?1, 'cron', 'job-a', ?2, 123,
                    'failed', '2026-05-31T11:00:00Z', 0, 'none',
                    '2026-05-31T11:00:00Z', '2026-05-31T11:00:00Z')",
                right_db::params![format!("run-{i:03}"), format!("session-{i:03}")],
            )
            .await
            .unwrap();
        }

        let response = activity_overview(
            &conn,
            ActivityOverviewInput {
                agent: "agent-a".to_owned(),
                generated_at: "2026-05-31T12:00:00Z".to_owned(),
                refresh_interval_secs: 30,
                foreground: vec![],
            },
        )
        .await
        .unwrap();

        assert_eq!(response.summary.failed_recent_cron_count, 51);
        assert_eq!(response.failed_runs.len(), 50);
    }
}
