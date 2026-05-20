use crate::api_types::{DashboardOverviewResponse, OverviewDoctorStatus, OverviewSandboxStatus};
use chrono::Duration;
use rusqlite::{Connection, params};

use super::ReadModelError;
use super::learning::window_start;

pub struct DashboardOverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub foreground_active_count: i64,
    pub sandbox: OverviewSandboxStatus,
}

pub fn dashboard_overview(
    conn: &Connection,
    input: DashboardOverviewInput,
) -> Result<DashboardOverviewResponse, ReadModelError> {
    let active_runs = active_async_run_count(conn)? + input.foreground_active_count;
    let recent_failures = recent_failure_count(conn, &input.generated_at)?;
    let today_cost_usd = super::activity::today_cost_usd(conn, &input.generated_at)?;
    let learning_candidates_24h =
        learning_candidate_count(conn, &input.agent, &input.generated_at)?;

    Ok(DashboardOverviewResponse {
        agent: input.agent,
        generated_at: input.generated_at,
        active_runs,
        recent_failures,
        today_cost_usd,
        learning_candidates_24h,
        doctor: OverviewDoctorStatus {
            state: "not_loaded".to_string(),
            pass_count: 0,
            warn_count: 0,
            fail_count: 0,
            generated_at: None,
        },
        sandbox: input.sandbox,
    })
}

fn active_async_run_count(conn: &Connection) -> Result<i64, ReadModelError> {
    Ok(conn.query_row(
        "SELECT COUNT(*)
         FROM async_runs
         WHERE status IN ('queued', 'running')",
        [],
        |row| row.get(0),
    )?)
}

fn recent_failure_count(conn: &Connection, generated_at: &str) -> Result<i64, ReadModelError> {
    let since = window_start(generated_at, Duration::hours(24))?;
    Ok(conn.query_row(
        "SELECT COUNT(*)
         FROM async_runs
         WHERE status = 'failed'
           AND COALESCE(finished_at, updated_at, created_at) >= ?1",
        params![since],
        |row| row.get(0),
    )?)
}

fn learning_candidate_count(
    conn: &Connection,
    agent: &str,
    generated_at: &str,
) -> Result<i64, ReadModelError> {
    let since = window_start(generated_at, Duration::hours(24))?;
    Ok(conn.query_row(
        "SELECT COUNT(*)
         FROM skill_review_reports
         WHERE agent_name = ?1
           AND status IN ('create_candidate', 'update_candidate')
           AND created_at >= ?2",
        params![agent, since],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{TempDir, tempdir};

    fn fixture() -> (TempDir, rusqlite::Connection) {
        let dir = tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true).expect("open db");
        (dir, conn)
    }

    #[test]
    fn dashboard_overview_summarizes_activity_usage_and_learning() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, started_at, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-active', 'background', 'handoff', 'session-active', 123,
                'running', '2026-05-20T08:00:00Z', 1, 'pending',
                '2026-05-20T08:00:00Z', '2026-05-20T08:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, finished_at, exit_code, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-failed', 'cron', 'daily', 'session-failed', 123,
                'failed', '2026-05-20T07:00:00Z', 1, 1, 'pending',
                '2026-05-20T07:00:00Z', '2026-05-20T07:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, model_usage_json
             ) VALUES (
                '2026-05-20T08:30:00Z', 'cron', NULL, NULL, 'daily',
                'session-active', 1.25, 1, '[]'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, candidate_skill_name, evidence_refs_json,
                review_output_json, created_at
             ) VALUES (
                'alpha', 'inv-1', 'learning_signal', 'create_candidate',
                'high', 'rightx-debugging', '[]', '{}',
                '2026-05-20T09:00:00Z'
             )",
            [],
        )
        .unwrap();

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-20T10:00:00Z".to_string(),
                foreground_active_count: 2,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: Some("sandbox alpha".to_string()),
                },
            },
        )
        .unwrap();

        assert_eq!(response.agent, "alpha");
        assert_eq!(response.active_runs, 3);
        assert_eq!(response.recent_failures, 1);
        assert_eq!(response.today_cost_usd, 1.25);
        assert_eq!(response.learning_candidates_24h, 1);
        assert_eq!(response.doctor.state, "not_loaded");
        assert_eq!(response.sandbox.state, "configured");
    }
}
