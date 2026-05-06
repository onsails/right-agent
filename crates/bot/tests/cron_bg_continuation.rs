//! Integration: verify ScheduleKind::BackgroundContinuation rows are inserted
//! correctly by the bot-internal helper.

use right_agent::cron_spec::insert_background_continuation;
use right_db::open_connection;
use uuid::Uuid;

#[tokio::test]
async fn bg_continuation_row_inserted_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path();
    std::fs::create_dir_all(agent_dir.join("crons").join(".locks")).unwrap();

    let conn = open_connection(agent_dir, true).unwrap();
    let fork_from = Uuid::new_v4();
    let result = insert_background_continuation(
        &conn,
        "bg-imm-1",
        "do thing",
        fork_from,
        -100,
        Some(7),
        Some(5.0),
    );
    assert!(
        result.is_ok(),
        "insert_background_continuation failed: {result:?}"
    );

    let (schedule, recurring, run_at): (String, i64, Option<String>) = conn
        .query_row(
            "SELECT schedule, recurring, run_at FROM cron_specs WHERE job_name = 'bg-imm-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(schedule, format!("@bg:{fork_from}"));
    assert_eq!(recurring, 0);
    assert!(run_at.is_none());
}
