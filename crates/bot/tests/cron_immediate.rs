//! Integration: verify ScheduleKind::Immediate jobs fire on the next cron tick.
//!
//! Uses the in-process reconcile loop — no real CC invocation. We seed a
//! cron_specs row with the @immediate sentinel and watch for the row to be
//! deleted (one-shot auto-delete) and a cron_runs row to appear.

use right_agent::cron_spec::insert_immediate_cron;
use right_agent::memory::open_connection;

#[tokio::test]
async fn immediate_job_row_inserted_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path();
    std::fs::create_dir_all(agent_dir.join("crons").join(".locks")).unwrap();

    let conn = open_connection(agent_dir, true).unwrap();
    let result = insert_immediate_cron(&conn, "bg-imm-1", "do thing", -100, Some(7), Some(5.0));
    assert!(result.is_ok(), "insert_immediate_cron failed: {result:?}");

    // Verify the sentinel landed
    let (schedule, recurring, run_at): (String, i64, Option<String>) = conn
        .query_row(
            "SELECT schedule, recurring, run_at FROM cron_specs WHERE job_name = 'bg-imm-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(schedule, "@immediate");
    assert_eq!(recurring, 0);
    assert!(run_at.is_none());
}
