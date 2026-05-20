use std::fs;
use std::io;

use crate::api_types::{
    ActiveActivity, CronCard, ForegroundActivity, LogExcerpt, OverviewResponse, OverviewSummary,
    RunDetailResponse, RunSummary,
};
use rusqlite::Connection;
use rusqlite::{OptionalExtension, params};

pub struct OverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
    pub foreground: Vec<ForegroundActivity>,
}

pub fn smoke_read_model(_conn: &Connection) -> usize {
    0
}

pub fn overview(conn: &Connection, input: OverviewInput) -> rusqlite::Result<OverviewResponse> {
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
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut crons = Vec::with_capacity(cron_rows.len());
    for (job_name, schedule, recurring, run_at, target_chat_id, target_thread_id, max_budget_usd) in
        cron_rows
    {
        let recent_runs = cron_runs(conn, &job_name)?;
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

    let active_background = active_background_runs(conn)?;
    let today_cost_usd = today_cost_usd(conn, &input.generated_at)?;
    let active_cron_count = crons
        .iter()
        .filter(|cron| {
            cron.recent_runs
                .iter()
                .any(|run| is_active_status(&run.status))
        })
        .count();
    let failed_recent_cron_count = crons
        .iter()
        .filter(|cron| cron.recent_runs.iter().any(|run| run.status == "failed"))
        .count();

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
        active: ActiveActivity {
            foreground: input.foreground,
            background: active_background,
        },
    })
}

pub fn run_detail(
    conn: &Connection,
    run_id: &str,
    max_lines: usize,
) -> rusqlite::Result<Option<RunDetailResponse>> {
    let row = conn
        .query_row(
            "SELECT ar.id, ar.kind, ar.producer_ref, ar.status, ar.started_at, ar.finished_at,
                    ar.exit_code, ar.delivery_status, costs.cost_usd, ar.summary,
                    ar.notify_json, ar.no_notify_reason, ar.log_path
             FROM async_runs ar
             LEFT JOIN (
                SELECT session_uuid, SUM(total_cost_usd) AS cost_usd
                FROM usage_events
                GROUP BY session_uuid
             ) costs ON costs.session_uuid = ar.run_session_id
             WHERE ar.id = ?1",
            params![run_id],
            |row| {
                Ok((
                    RunSummary {
                        id: row.get(0)?,
                        kind: row.get(1)?,
                        producer_ref: row.get(2)?,
                        status: row.get(3)?,
                        started_at: row.get(4)?,
                        finished_at: row.get(5)?,
                        exit_code: row.get(6)?,
                        delivery_status: row.get(7)?,
                        cost_usd: row.get(8)?,
                    },
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                ))
            },
        )
        .optional()?;

    let Some((run, summary, notify_json, no_notify_reason, log_path)) = row else {
        return Ok(None);
    };
    let notify_json = notify_json.and_then(|json| serde_json::from_str(&json).ok());

    Ok(Some(RunDetailResponse {
        run,
        summary,
        notify_json,
        no_notify_reason,
        log: read_log_excerpt(log_path, max_lines)?,
    }))
}

fn cron_runs(conn: &Connection, job_name: &str) -> rusqlite::Result<Vec<RunSummary>> {
    let mut stmt = conn.prepare(
        "SELECT ar.id, ar.kind, ar.producer_ref, ar.status, ar.started_at, ar.finished_at,
                ar.exit_code, ar.delivery_status, costs.cost_usd
         FROM async_runs ar
         LEFT JOIN (
            SELECT session_uuid, SUM(total_cost_usd) AS cost_usd
            FROM usage_events
            GROUP BY session_uuid
         ) costs ON costs.session_uuid = ar.run_session_id
         WHERE ar.kind = 'cron' AND ar.producer_ref = ?1
         ORDER BY COALESCE(ar.started_at, ar.created_at) DESC, ar.created_at DESC
         LIMIT 5",
    )?;
    stmt.query_map(params![job_name], run_summary_from_row)?
        .collect()
}

fn active_background_runs(conn: &Connection) -> rusqlite::Result<Vec<RunSummary>> {
    let mut stmt = conn.prepare(
        "SELECT ar.id, ar.kind, ar.producer_ref, ar.status, ar.started_at, ar.finished_at,
                ar.exit_code, ar.delivery_status, costs.cost_usd
         FROM async_runs ar
         LEFT JOIN (
            SELECT session_uuid, SUM(total_cost_usd) AS cost_usd
            FROM usage_events
            GROUP BY session_uuid
         ) costs ON costs.session_uuid = ar.run_session_id
         WHERE ar.kind = 'background' AND ar.status IN ('queued', 'running')
         ORDER BY ar.started_at DESC, ar.created_at DESC",
    )?;
    stmt.query_map([], run_summary_from_row)?.collect()
}

fn run_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunSummary> {
    Ok(RunSummary {
        id: row.get(0)?,
        kind: row.get(1)?,
        producer_ref: row.get(2)?,
        status: row.get(3)?,
        started_at: row.get(4)?,
        finished_at: row.get(5)?,
        exit_code: row.get(6)?,
        delivery_status: row.get(7)?,
        cost_usd: row.get(8)?,
    })
}

fn today_cost_usd(conn: &Connection, generated_at: &str) -> rusqlite::Result<f64> {
    let since = if let Some((date, _)) = generated_at.split_once('T') {
        format!("{date}T00:00:00Z")
    } else {
        generated_at.to_owned()
    };
    conn.query_row(
        "SELECT COALESCE(SUM(total_cost_usd), 0.0)
         FROM usage_events
         WHERE ts >= ?1",
        params![since],
        |row| row.get(0),
    )
}

fn read_log_excerpt(path: Option<String>, max_lines: usize) -> rusqlite::Result<LogExcerpt> {
    let Some(path) = path else {
        return Ok(LogExcerpt {
            available: false,
            path: None,
            lines: Vec::new(),
            truncated: false,
        });
    };

    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LogExcerpt {
                available: false,
                path: Some(path),
                lines: Vec::new(),
                truncated: false,
            });
        }
        Err(error) => return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error))),
    };
    let lines = content.lines().map(str::to_owned).collect::<Vec<_>>();
    let truncated = lines.len() > max_lines;
    let start = lines.len().saturating_sub(max_lines);

    Ok(LogExcerpt {
        available: true,
        path: Some(path),
        lines: lines[start..].to_vec(),
        truncated,
    })
}

fn is_active_status(status: &str) -> bool {
    matches!(status, "queued" | "running")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_types::ForegroundActivity;
    use tempfile::{TempDir, tempdir};

    fn fixture() -> (TempDir, Connection) {
        let dir = tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true).expect("open db");

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
        .expect("insert usage event");

        (dir, conn)
    }

    #[test]
    fn overview_builds_cron_cards_and_active_background() {
        let (_dir, conn) = fixture();
        let response = overview(
            &conn,
            OverviewInput {
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
        .unwrap();

        assert_eq!(response.summary.cron_count, 1);
        assert_eq!(response.summary.today_cost_usd, 0.25);
        assert_eq!(response.crons[0].last_run.as_ref().unwrap().id, "run-1");
        assert_eq!(response.active.background[0].id, "bg-1");
    }

    #[test]
    fn run_detail_handles_missing_runs_and_missing_logs() {
        let (_dir, conn) = fixture();

        assert!(run_detail(&conn, "missing", 20).unwrap().is_none());
        assert!(
            !run_detail(&conn, "run-1", 20)
                .unwrap()
                .unwrap()
                .log
                .available
        );
    }
}
