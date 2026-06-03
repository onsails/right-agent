use super::*;
use uuid::Uuid;

#[tokio::test]
async fn validate_job_name_valid() {
    assert!(validate_job_name("health-check").is_ok());
    assert!(validate_job_name("a").is_ok());
    assert!(validate_job_name("deploy-check-123").is_ok());
}

#[tokio::test]
async fn validate_job_name_invalid() {
    assert!(validate_job_name("").is_err());
    assert!(validate_job_name("-leading").is_err());
    assert!(validate_job_name("UPPER").is_err());
    assert!(validate_job_name("has space").is_err());
    assert!(validate_job_name("under_score").is_err());
}

#[tokio::test]
async fn validate_schedule_valid_no_warning() {
    assert!(validate_schedule("17 9 * * 1-5").unwrap().is_none());
    assert!(validate_schedule("43 */4 * * *").unwrap().is_none());
    assert!(validate_schedule("7,23,47 * * * *").unwrap().is_none());
}

#[tokio::test]
async fn validate_schedule_invalid() {
    assert!(validate_schedule("not a cron").is_err());
    assert!(validate_schedule("").is_err());
}

#[tokio::test]
async fn validate_schedule_peak_minute_warning() {
    // Literal :00 and :30 minutes
    assert!(validate_schedule("0 9 * * *").unwrap().is_some());
    assert!(validate_schedule("30 9 * * *").unwrap().is_some());
    assert!(validate_schedule("00 9 * * *").unwrap().is_some());
    // Step expressions that hit :00 and/or :30
    assert!(validate_schedule("*/30 * * * *").unwrap().is_some());
    assert!(validate_schedule("*/15 * * * *").unwrap().is_some());
    assert!(validate_schedule("*/10 * * * *").unwrap().is_some());
    assert!(validate_schedule("*/5 * * * *").unwrap().is_some());
    // Lists that include :00 or :30
    assert!(validate_schedule("0,30 * * * *").unwrap().is_some());
    assert!(validate_schedule("17,30 * * * *").unwrap().is_some());
    // Wildcard fires every minute → hits both peaks
    assert!(validate_schedule("* * * * *").unwrap().is_some());
}

#[tokio::test]
async fn validate_schedule_peak_minute_offsets_pass() {
    // Step expressions that never hit :00 or :30
    assert!(validate_schedule("7-59/15 * * * *").unwrap().is_none());
    // Single non-round literal
    assert!(validate_schedule("17 * * * *").unwrap().is_none());
    assert!(validate_schedule("43 * * * *").unwrap().is_none());
}

#[tokio::test]
async fn validate_lock_ttl_valid() {
    assert!(validate_lock_ttl("30m").is_ok());
    assert!(validate_lock_ttl("1h").is_ok());
}

#[tokio::test]
async fn validate_lock_ttl_invalid() {
    assert!(validate_lock_ttl("bad").is_err());
    assert!(validate_lock_ttl("30").is_err());
    assert!(validate_lock_ttl("").is_err());
}

async fn setup_db() -> (tempfile::TempDir, right_db::Connection) {
    right_db::test_support::migrated_connection().await
}

#[allow(clippy::too_many_arguments)]
async fn insert_async_cron_run(
    conn: &right_db::Connection,
    id: &str,
    job_name: &str,
    started_at: &str,
    finished_at: Option<&str>,
    exit_code: Option<i64>,
    status: &str,
    target_chat_id: i64,
    target_thread_id: Option<i64>,
    delivery_status: &str,
) {
    let delivery_required = i64::from(delivery_status != "none");
    let delivery_json = (delivery_status != "none").then_some("{}");
    let delivered_at = (delivery_status == "delivered").then_some("2026-01-01T00:05:00Z");
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
            started_at, finished_at, exit_code, status, log_path, delivery_json,
            delivery_required, delivery_status, delivered_at, created_at, updated_at
         ) VALUES (
            ?1, 'cron', ?2, ?1, ?3, ?4,
            ?5, ?6, ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?5, ?5
         )",
        right_db::params![
            id,
            job_name,
            target_chat_id,
            target_thread_id,
            started_at,
            finished_at,
            exit_code,
            status,
            format!("/tmp/{id}.log"),
            delivery_json,
            delivery_required,
            delivery_status,
            delivered_at,
        ],
    )
    .await
    .expect("insert async cron run");
}

#[tokio::test]
async fn create_spec_success() {
    let (_dir, conn) = setup_db().await;
    // Use an offset minute that never lands on :00 or :30 so the peak-minute
    // warning does not trip — this test asserts a clean creation path.
    let result = create_spec(&conn, "my-job", "7-59/15 * * * *", "do stuff", None, None)
        .await
        .unwrap();
    assert!(result.message.contains("Created"));
    assert!(result.warning.is_none());

    // Verify row exists.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cron_specs WHERE job_name = 'my-job'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn create_spec_with_warning() {
    let (_dir, conn) = setup_db().await;
    let result = create_spec(&conn, "my-job", "0 9 * * *", "do stuff", None, None)
        .await
        .unwrap();
    assert!(result.warning.is_some());
}

#[tokio::test]
async fn create_spec_duplicate_error() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "dup", "*/5 * * * *", "prompt", None, None)
        .await
        .unwrap();
    let err = create_spec(&conn, "dup", "*/5 * * * *", "prompt", None, None)
        .await
        .unwrap_err();
    assert!(err.contains("already exists"));
}

#[tokio::test]
async fn create_spec_validation_errors() {
    let (_dir, conn) = setup_db().await;
    // Bad job name
    assert!(
        create_spec(&conn, "BAD NAME", "*/5 * * * *", "p", None, None)
            .await
            .is_err()
    );
    // Empty prompt
    assert!(
        create_spec(&conn, "ok", "*/5 * * * *", "  ", None, None)
            .await
            .is_err()
    );
    // Bad schedule
    assert!(
        create_spec(&conn, "ok", "not-cron", "p", None, None)
            .await
            .is_err()
    );
    // Bad lock_ttl
    assert!(
        create_spec(&conn, "ok", "*/5 * * * *", "p", Some("bad"), None)
            .await
            .is_err()
    );
    // Negative budget
    assert!(
        create_spec(&conn, "ok", "*/5 * * * *", "p", None, Some(-1.0))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn update_spec_success() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "upd", "*/5 * * * *", "old", None, None)
        .await
        .unwrap();
    let result = update_spec(
        &conn,
        "upd",
        "17 9 * * *",
        "new prompt",
        Some("1h"),
        Some(2.0),
    )
    .await
    .unwrap();
    assert!(result.message.contains("Updated"));

    let prompt: String = conn
        .query_row(
            "SELECT prompt FROM cron_specs WHERE job_name = 'upd'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(prompt, "new prompt");
}

#[tokio::test]
async fn update_spec_not_found() {
    let (_dir, conn) = setup_db().await;
    let err = update_spec(&conn, "ghost", "*/5 * * * *", "prompt", None, None)
        .await
        .unwrap_err();
    assert!(err.contains("not found"));
}

#[tokio::test]
async fn delete_spec_success() {
    let (_dir, conn) = setup_db().await;
    let tmp = tempfile::tempdir().unwrap();
    create_spec(&conn, "del", "*/5 * * * *", "p", None, None)
        .await
        .unwrap();
    let msg = delete_spec(&conn, "del", tmp.path()).await.unwrap();
    assert!(msg.contains("Deleted"));

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cron_specs WHERE job_name = 'del'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn delete_spec_not_found() {
    let (_dir, conn) = setup_db().await;
    let tmp = tempfile::tempdir().unwrap();
    let err = delete_spec(&conn, "nope", tmp.path()).await.unwrap_err();
    assert!(err.contains("not found"));
}

#[tokio::test]
async fn list_specs_json() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "a-job", "*/5 * * * *", "prompt a", None, None)
        .await
        .unwrap();
    create_spec(
        &conn,
        "b-job",
        "17 9 * * *",
        "prompt b",
        Some("30m"),
        Some(2.5),
    )
    .await
    .unwrap();
    let output = list_specs(&conn).await.unwrap();
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0]["job_name"], "a-job");
    assert_eq!(parsed[1]["job_name"], "b-job");
    assert_eq!(parsed[1]["max_budget_usd"], 2.5);
    // No runs yet — latest-run fields should be null.
    assert!(parsed[0]["last_run_id"].is_null());
    assert!(parsed[0]["last_run_at"].is_null());
    assert!(parsed[0]["last_status"].is_null());
    assert!(parsed[1]["last_run_id"].is_null());
    assert!(parsed[1]["last_run_at"].is_null());
    assert!(parsed[1]["last_status"].is_null());
}

#[tokio::test]
async fn list_specs_reads_last_run_from_async_runs() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "a-job", "*/5 * * * *", "prompt a", None, None)
        .await
        .unwrap();
    insert_async_cron_run(
        &conn,
        "run-old",
        "a-job",
        "2026-01-01T00:00:00Z",
        Some("2026-01-01T00:01:00Z"),
        Some(0),
        "success",
        -100,
        None,
        "none",
    )
    .await;
    insert_async_cron_run(
        &conn,
        "run-new",
        "a-job",
        "2026-01-02T00:00:00Z",
        Some("2026-01-02T00:01:00Z"),
        Some(1),
        "failed",
        -100,
        None,
        "none",
    )
    .await;
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, source_session_id, run_session_id, target_chat_id,
            started_at, status, delivery_required, delivery_status, created_at, updated_at
         ) VALUES (
            'bg-newer', 'background', 'a-job', 'main', 'bg-session', -100,
            '2026-01-03T00:00:00Z', 'success', 1, 'pending',
            '2026-01-03T00:00:00Z', '2026-01-03T00:00:00Z'
         )",
        [],
    )
    .await
    .unwrap();
    let output = list_specs(&conn).await.unwrap();
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["last_run_id"], "run-new");
    assert_eq!(parsed[0]["last_run_at"], "2026-01-02T00:00:00Z");
    assert_eq!(parsed[0]["last_status"], "failed");
}

#[tokio::test]
async fn load_specs_from_db_empty() {
    let (_dir, conn) = setup_db().await;
    let specs = load_specs_from_db(&conn).await.unwrap();
    assert!(specs.is_empty());
}

#[tokio::test]
async fn load_specs_from_db_returns_all() {
    let (_dir, conn) = setup_db().await;
    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('job1', '*/5 * * * *', 'do stuff', 0.5, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .await
        .unwrap();
    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at) \
             VALUES ('job2', '17 9 * * *', 'other', '1h', 1.0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .await
        .unwrap();
    let specs = load_specs_from_db(&conn).await.unwrap();
    assert_eq!(specs.len(), 2);
    assert_eq!(
        specs["job1"].schedule_kind.cron_schedule().unwrap(),
        "*/5 * * * *"
    );
    assert_eq!(specs["job1"].max_budget_usd, 0.5);
    assert_eq!(specs["job2"].lock_ttl.as_deref(), Some("1h"));
}

#[tokio::test]
async fn load_specs_skips_legacy_bg_schedule_rows() {
    let (_dir, conn) = setup_db().await;
    let main = Uuid::new_v4();
    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, created_at, updated_at) \
             VALUES ('legacy-bg', ?1, 'old background prompt', 1.0, 0, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            right_db::params![format!("@bg:{main}")],
        )
        .await
        .unwrap();
    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, created_at, updated_at) \
             VALUES ('normal', '*/5 * * * *', 'normal cron prompt', 1.0, 1, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .await
        .unwrap();

    let specs = load_specs_from_db(&conn).await.unwrap();

    assert!(!specs.contains_key("legacy-bg"));
    assert!(matches!(
        specs["normal"].schedule_kind,
        ScheduleKind::Recurring(_)
    ));
}

#[tokio::test]
async fn load_specs_skips_non_bg_malformed_rows() {
    let (_dir, conn) = setup_db().await;
    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, created_at, updated_at) \
             VALUES ('bad-run-at', '', 'bad run_at prompt', 1.0, 0, 'not-a-date', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .await
        .unwrap();
    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, created_at, updated_at) \
             VALUES ('normal', '*/5 * * * *', 'normal cron prompt', 1.0, 1, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .await
        .unwrap();

    let specs = load_specs_from_db(&conn).await.unwrap();

    assert!(!specs.contains_key("bad-run-at"));
    assert!(matches!(
        specs["normal"].schedule_kind,
        ScheduleKind::Recurring(_)
    ));
}

#[tokio::test]
async fn trigger_spec_sets_timestamp() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "trig-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();
    let msg = trigger_spec(&conn, "trig-job", false).await.unwrap();
    assert!(msg.contains("Triggered"));
    let ts: Option<String> = conn
        .query_row(
            "SELECT triggered_at FROM cron_specs WHERE job_name = 'trig-job'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert!(ts.is_some(), "triggered_at should be set");
}

#[tokio::test]
async fn trigger_spec_nonexistent_job() {
    let (_dir, conn) = setup_db().await;
    let err = trigger_spec(&conn, "ghost", false).await.unwrap_err();
    assert!(err.contains("not found"));
}

#[tokio::test]
async fn trigger_spec_idempotent() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "idem-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();
    trigger_spec(&conn, "idem-job", false).await.unwrap();
    trigger_spec(&conn, "idem-job", false).await.unwrap();
    let ts: Option<String> = conn
        .query_row(
            "SELECT triggered_at FROM cron_specs WHERE job_name = 'idem-job'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert!(ts.is_some());
}

#[tokio::test]
async fn clear_triggered_at_clears() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "clr-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();
    trigger_spec(&conn, "clr-job", false).await.unwrap();
    clear_triggered_at(&conn, "clr-job").await.unwrap();
    let ts: Option<String> = conn
        .query_row(
            "SELECT triggered_at FROM cron_specs WHERE job_name = 'clr-job'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert!(ts.is_none(), "triggered_at should be cleared");
}

#[tokio::test]
async fn describe_schedule_returns_description() {
    let desc = describe_schedule("*/5 * * * *");
    assert!(!desc.is_empty());
}

#[tokio::test]
async fn describe_schedule_fallback_on_invalid() {
    let desc = describe_schedule("not-valid-cron");
    assert_eq!(desc, "not-valid-cron");
}

#[tokio::test]
async fn get_spec_detail_found() {
    let (_dir, conn) = setup_db().await;
    create_spec(
        &conn,
        "detail-job",
        "*/5 * * * *",
        "do stuff",
        Some("1h"),
        Some(2.5),
    )
    .await
    .unwrap();
    let detail = get_spec_detail(&conn, "detail-job").await.unwrap().unwrap();
    assert_eq!(detail.job_name, "detail-job");
    assert_eq!(detail.schedule, "*/5 * * * *");
    assert_eq!(detail.prompt, "do stuff");
    assert_eq!(detail.lock_ttl.as_deref(), Some("1h"));
    assert!((detail.max_budget_usd - 2.5).abs() < f64::EPSILON);
}

#[tokio::test]
async fn get_spec_detail_not_found() {
    let (_dir, conn) = setup_db().await;
    let detail = get_spec_detail(&conn, "ghost").await.unwrap();
    assert!(detail.is_none());
}

#[tokio::test]
async fn get_recent_runs_returns_ordered() {
    let (_dir, conn) = setup_db().await;
    insert_async_cron_run(
        &conn,
        "r1",
        "runs-job",
        "2026-01-01T00:00:00Z",
        Some("2026-01-01T00:01:00Z"),
        Some(0),
        "success",
        -100,
        None,
        "none",
    )
    .await;
    insert_async_cron_run(
        &conn,
        "r2",
        "runs-job",
        "2026-01-01T01:00:00Z",
        Some("2026-01-01T01:01:00Z"),
        Some(1),
        "failed",
        -100,
        None,
        "none",
    )
    .await;
    let runs = get_recent_runs(&conn, "runs-job", 5).await.unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].id, "r2");
    assert_eq!(runs[1].id, "r1");
    assert_eq!(runs[0].status, "failed");
}

#[tokio::test]
async fn get_recent_runs_empty() {
    let (_dir, conn) = setup_db().await;
    let runs = get_recent_runs(&conn, "no-such-job", 5).await.unwrap();
    assert!(runs.is_empty());
}

#[tokio::test]
async fn get_recent_runs_respects_limit() {
    let (_dir, conn) = setup_db().await;
    for i in 0..10 {
        insert_async_cron_run(
            &conn,
            &format!("r{i}"),
            "limit-job",
            &format!("2026-01-01T{i:02}:00:00Z"),
            None,
            None,
            "success",
            -100,
            None,
            "none",
        )
        .await;
    }
    let runs = get_recent_runs(&conn, "limit-job", 3).await.unwrap();
    assert_eq!(runs.len(), 3);
}

/// Regression: triggered_at must NOT affect CronSpec equality.
/// The reconciler compares old vs new specs to detect config changes.
/// If triggered_at participates in PartialEq, triggering a job causes the
/// reconciler to abort and respawn the job scheduler in an infinite loop.
#[tokio::test]
async fn triggered_at_does_not_affect_equality() {
    let base = CronSpec {
        schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into()),
        prompt: "do stuff".into(),
        lock_ttl: None,
        max_budget_usd: 1.0,
        triggered_at: None,
        trigger_force_notify: false,
        target_chat_id: None,
        target_thread_id: None,
        model: None,
    };
    let triggered = CronSpec {
        triggered_at: Some("2026-04-15T12:00:00Z".into()),
        ..base.clone()
    };
    assert_eq!(base, triggered, "triggered_at must not affect equality");
}

#[tokio::test]
async fn spec_equality_detects_real_changes() {
    let base = CronSpec {
        schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into()),
        prompt: "do stuff".into(),
        lock_ttl: None,
        max_budget_usd: 1.0,
        triggered_at: None,
        trigger_force_notify: false,
        target_chat_id: None,
        target_thread_id: None,
        model: None,
    };
    let changed_schedule = CronSpec {
        schedule_kind: ScheduleKind::Recurring("*/10 * * * *".into()),
        ..base.clone()
    };
    let changed_prompt = CronSpec {
        prompt: "different".into(),
        ..base.clone()
    };
    let changed_budget = CronSpec {
        max_budget_usd: 2.0,
        ..base.clone()
    };
    let changed_target = CronSpec {
        target_chat_id: Some(-12345),
        ..base.clone()
    };
    assert_ne!(base, changed_schedule);
    assert_ne!(base, changed_prompt);
    assert_ne!(base, changed_budget);
    assert_ne!(
        base, changed_target,
        "target_chat_id change must be a real change"
    );
}

#[tokio::test]
async fn load_specs_includes_triggered_at() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "tr-load", "*/5 * * * *", "p", None, None)
        .await
        .unwrap();
    trigger_spec(&conn, "tr-load", false).await.unwrap();
    let specs = load_specs_from_db(&conn).await.unwrap();
    assert!(specs["tr-load"].triggered_at.is_some());
}

#[tokio::test]
async fn load_specs_from_db_carries_target_fields() {
    let (_dir, conn) = setup_db().await;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('with-target', '*/5 * * * *', 'p', NULL, 1.0, 1, -555, 9, ?1, ?1)",
            [&now],
        )
        .await
        .unwrap();
    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, created_at, updated_at) \
             VALUES ('no-target', '*/5 * * * *', 'p', NULL, 1.0, 1, ?1, ?1)",
            [&now],
        )
        .await
        .unwrap();

    let specs = load_specs_from_db(&conn).await.unwrap();
    let with = &specs["with-target"];
    assert_eq!(with.target_chat_id, Some(-555));
    assert_eq!(with.target_thread_id, Some(9));

    let without = &specs["no-target"];
    assert_eq!(without.target_chat_id, None);
    assert_eq!(without.target_thread_id, None);
}

#[tokio::test]
async fn create_spec_v2_with_run_at_succeeds() {
    let (_dir, conn) = setup_db().await;
    let result = create_spec_v2(
        &conn,
        "run-at-job",
        None,
        "do stuff at specific time",
        None,
        None,
        None,
        Some("2026-12-25T15:30:00Z"),
        None,
        None,
        false,
    )
    .await
    .unwrap();
    assert!(result.message.contains("Created"));
}

#[tokio::test]
async fn trigger_spec_force_notify_sets_both_columns() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "fn-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();

    trigger_spec(&conn, "fn-job", true).await.unwrap();

    let (triggered_at, force): (Option<String>, i64) = conn
        .query_row(
            "SELECT triggered_at, trigger_force_notify FROM cron_specs WHERE job_name = ?1",
            right_db::params!["fn-job"],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert!(triggered_at.is_some(), "triggered_at must be set");
    assert_eq!(force, 1, "trigger_force_notify must be set");
}

#[tokio::test]
async fn trigger_spec_without_force_notify_leaves_flag_zero() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "plain-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();

    trigger_spec(&conn, "plain-job", false).await.unwrap();

    let force: i64 = conn
        .query_row(
            "SELECT trigger_force_notify FROM cron_specs WHERE job_name = ?1",
            right_db::params!["plain-job"],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(force, 0);
}

#[tokio::test]
async fn clear_triggered_at_resets_force_notify() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "clr-fn-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();
    trigger_spec(&conn, "clr-fn-job", true).await.unwrap();

    clear_triggered_at(&conn, "clr-fn-job").await.unwrap();

    let (triggered_at, force): (Option<String>, i64) = conn
        .query_row(
            "SELECT triggered_at, trigger_force_notify FROM cron_specs WHERE job_name = ?1",
            right_db::params!["clr-fn-job"],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert!(triggered_at.is_none(), "triggered_at must be cleared");
    assert_eq!(force, 0, "trigger_force_notify must be reset");
}

#[tokio::test]
async fn load_specs_carries_force_notify() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "load-fn-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();
    trigger_spec(&conn, "load-fn-job", true).await.unwrap();

    let specs = load_specs_from_db(&conn).await.unwrap();
    assert!(
        specs["load-fn-job"].trigger_force_notify,
        "loaded spec must carry trigger_force_notify"
    );
}

#[tokio::test]
async fn create_spec_v2_with_both_schedule_and_run_at_fails() {
    let (_dir, conn) = setup_db().await;
    let err = create_spec_v2(
        &conn,
        "both-job",
        Some("*/5 * * * *"),
        "prompt",
        None,
        None,
        None,
        Some("2026-12-25T15:30:00Z"),
        None,
        None,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.contains("mutually exclusive"));
}

#[tokio::test]
async fn create_spec_v2_with_neither_schedule_nor_run_at_fails() {
    let (_dir, conn) = setup_db().await;
    let err = create_spec_v2(
        &conn,
        "neither-job",
        None,
        "prompt",
        None,
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.contains("one of"));
}

#[tokio::test]
async fn create_spec_v2_with_invalid_run_at_fails() {
    let (_dir, conn) = setup_db().await;
    let err = create_spec_v2(
        &conn,
        "bad-time",
        None,
        "prompt",
        None,
        None,
        None,
        Some("not-a-datetime"),
        None,
        None,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.contains("invalid"));
}

#[tokio::test]
async fn create_spec_v2_with_past_run_at_succeeds() {
    let (_dir, conn) = setup_db().await;
    let result = create_spec_v2(
        &conn,
        "past-job",
        None,
        "prompt",
        None,
        None,
        None,
        Some("2020-01-01T00:00:00Z"),
        None,
        None,
        false,
    )
    .await
    .unwrap();
    assert!(result.message.contains("Created"));
}

#[tokio::test]
async fn create_spec_v2_recurring_false_stored_as_one_shot_cron() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "oneshot-cron",
        Some("30 15 * * *"),
        "prompt",
        None,
        None,
        Some(false),
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();
    let specs = load_specs_from_db(&conn).await.unwrap();
    assert!(matches!(
        specs["oneshot-cron"].schedule_kind,
        ScheduleKind::OneShotCron(_)
    ));
}

#[tokio::test]
async fn load_specs_round_trips_all_schedule_kinds() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "recurring",
        Some("*/5 * * * *"),
        "p",
        None,
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();
    create_spec_v2(
        &conn,
        "oneshot",
        Some("17 15 * * *"),
        "p",
        None,
        None,
        Some(false),
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();
    create_spec_v2(
        &conn,
        "runat",
        None,
        "p",
        None,
        None,
        None,
        Some("2026-12-25T15:30:00Z"),
        None,
        None,
        false,
    )
    .await
    .unwrap();

    let specs = load_specs_from_db(&conn).await.unwrap();
    assert!(matches!(
        specs["recurring"].schedule_kind,
        ScheduleKind::Recurring(_)
    ));
    assert!(matches!(
        specs["oneshot"].schedule_kind,
        ScheduleKind::OneShotCron(_)
    ));
    assert!(matches!(
        specs["runat"].schedule_kind,
        ScheduleKind::RunAt(_)
    ));
}

#[tokio::test]
async fn update_spec_partial_prompt_only() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "partial",
        Some("*/5 * * * *"),
        "old",
        None,
        Some(1.5),
        None,
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();
    update_spec_partial(
        &conn,
        "partial",
        None,
        None,
        Some("new prompt"),
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let detail = get_spec_detail(&conn, "partial").await.unwrap().unwrap();
    assert_eq!(detail.prompt, "new prompt");
    assert_eq!(detail.schedule, "*/5 * * * *");
    assert!((detail.max_budget_usd - 1.5).abs() < f64::EPSILON);
}

#[tokio::test]
async fn update_spec_partial_schedule_clears_run_at() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "switch",
        None,
        "p",
        None,
        None,
        None,
        Some("2026-12-25T15:30:00Z"),
        None,
        None,
        false,
    )
    .await
    .unwrap();
    update_spec_partial(
        &conn,
        "switch",
        Some("*/10 * * * *"),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let specs = load_specs_from_db(&conn).await.unwrap();
    assert!(matches!(
        specs["switch"].schedule_kind,
        ScheduleKind::Recurring(_)
    ));
}

#[tokio::test]
async fn update_spec_partial_run_at_clears_schedule() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "switch2",
        Some("*/5 * * * *"),
        "p",
        None,
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();
    update_spec_partial(
        &conn,
        "switch2",
        None,
        Some("2026-12-25T15:30:00Z"),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let specs = load_specs_from_db(&conn).await.unwrap();
    assert!(matches!(
        specs["switch2"].schedule_kind,
        ScheduleKind::RunAt(_)
    ));
}

#[tokio::test]
async fn update_spec_partial_both_schedule_and_run_at_fails() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "both",
        Some("*/5 * * * *"),
        "p",
        None,
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();
    let err = update_spec_partial(
        &conn,
        "both",
        Some("*/10 * * * *"),
        Some("2026-12-25T15:30:00Z"),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("mutually exclusive"));
}

#[tokio::test]
async fn update_spec_partial_no_fields_fails() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "empty",
        Some("*/5 * * * *"),
        "p",
        None,
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();
    let err = update_spec_partial(
        &conn, "empty", None, None, None, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("at least one"));
}

#[tokio::test]
async fn update_spec_partial_not_found() {
    let (_dir, conn) = setup_db().await;
    let err = update_spec_partial(
        &conn,
        "ghost",
        None,
        None,
        Some("p"),
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("not found"));
}

#[tokio::test]
async fn create_spec_v2_persists_target_fields() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "with-target",
        Some("*/5 * * * *"),
        "do thing",
        None,
        None,
        None,
        None,
        Some(-100),
        Some(7),
        false,
    )
    .await
    .unwrap();

    let (chat, thread): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT target_chat_id, target_thread_id FROM cron_specs WHERE job_name = 'with-target'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .await
            .unwrap();
    assert_eq!(chat, Some(-100));
    assert_eq!(thread, Some(7));
}

#[tokio::test]
async fn create_spec_v2_persists_null_target_when_omitted() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "no-target",
        Some("*/5 * * * *"),
        "do thing",
        None,
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();

    let (chat, thread): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT target_chat_id, target_thread_id FROM cron_specs WHERE job_name = 'no-target'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert!(chat.is_none());
    assert!(thread.is_none());
}

#[tokio::test]
async fn update_spec_partial_sets_target_chat_id() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "j1",
        Some("*/5 * * * *"),
        "p",
        None,
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();
    update_spec_partial(
        &conn,
        "j1",
        None,
        None,
        None,
        None,
        None,
        None,
        Some(-555),
        None,
    )
    .await
    .unwrap();
    let chat: Option<i64> = conn
        .query_row(
            "SELECT target_chat_id FROM cron_specs WHERE job_name='j1'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(chat, Some(-555));
}

#[tokio::test]
async fn update_spec_partial_clears_target_thread_id() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "j1",
        Some("*/5 * * * *"),
        "p",
        None,
        None,
        None,
        None,
        Some(-1),
        Some(42),
        false,
    )
    .await
    .unwrap();
    // Outer Some = field present; inner None = clear to NULL.
    update_spec_partial(
        &conn,
        "j1",
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(None),
    )
    .await
    .unwrap();
    let thread: Option<i64> = conn
        .query_row(
            "SELECT target_thread_id FROM cron_specs WHERE job_name='j1'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert!(thread.is_none(), "thread must be cleared");
}

#[tokio::test]
async fn update_spec_partial_leaves_target_when_omitted() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "j1",
        Some("*/5 * * * *"),
        "p",
        None,
        None,
        None,
        None,
        Some(-1),
        Some(42),
        false,
    )
    .await
    .unwrap();
    // Update only the prompt; targets must stay.
    update_spec_partial(
        &conn,
        "j1",
        None,
        None,
        Some("new prompt"),
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (chat, thread): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT target_chat_id, target_thread_id FROM cron_specs WHERE job_name='j1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert_eq!(chat, Some(-1));
    assert_eq!(thread, Some(42));
}

#[tokio::test]
async fn load_specs_round_trips_immediate() {
    let (_dir, conn) = setup_db().await;
    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, created_at, updated_at) \
             VALUES ('imm', '@immediate', 'do it now', 5.0, 0, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .await
        .unwrap();
    let specs = load_specs_from_db(&conn).await.unwrap();
    assert!(matches!(
        specs["imm"].schedule_kind,
        ScheduleKind::Immediate
    ));
}

#[tokio::test]
async fn immediate_is_one_shot() {
    assert!(ScheduleKind::Immediate.is_one_shot());
    assert!(ScheduleKind::Immediate.cron_schedule().is_none());
}

#[tokio::test]
async fn list_specs_includes_target_fields() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "j1",
        Some("*/5 * * * *"),
        "p",
        None,
        None,
        None,
        None,
        Some(-100),
        Some(5),
        false,
    )
    .await
    .unwrap();
    let json = list_specs(&conn).await.unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let row = &value.as_array().unwrap()[0];
    assert_eq!(row["target_chat_id"].as_i64(), Some(-100));
    assert_eq!(row["target_thread_id"].as_i64(), Some(5));
}

#[tokio::test]
async fn resolve_schedule_fields_immediate_mutex() {
    use super::resolve_schedule_fields;
    // immediate + schedule → error
    assert!(resolve_schedule_fields(Some("*/5 * * * *"), None, None, true).is_err());
    // immediate + run_at → error
    assert!(resolve_schedule_fields(None, None, Some("2026-12-25T00:00:00Z"), true).is_err());
    // immediate alone → ok with sentinel
    let (sched, rec, run_at, _) = resolve_schedule_fields(None, None, None, true).unwrap();
    assert_eq!(sched, IMMEDIATE_SENTINEL);
    assert_eq!(rec, 0);
    assert!(run_at.is_none());
}

#[tokio::test]
async fn create_spec_v2_immediate_inserts_sentinel() {
    let (_dir, conn) = setup_db().await;
    create_spec_v2(
        &conn,
        "bg-test",
        None,
        "do it now",
        None,
        Some(5.0),
        None,
        None,
        Some(-100),
        Some(7),
        true,
    )
    .await
    .unwrap();
    let stored: (String, i64, Option<String>, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT schedule, recurring, run_at, target_chat_id, target_thread_id FROM cron_specs WHERE job_name = 'bg-test'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .await
            .unwrap();
    assert_eq!(stored.0, IMMEDIATE_SENTINEL);
    assert_eq!(stored.1, 0);
    assert!(stored.2.is_none());
    assert_eq!(stored.3, Some(-100));
    assert_eq!(stored.4, Some(7));
}

/// Regression: changing `target_chat_id` via `update_spec_partial` must
/// redirect pending/retryable async cron deliveries to the new chat.
/// Completed delivery rows keep the target snapshot that was actually sent.
#[tokio::test]
async fn update_spec_target_propagates_to_undelivered_async_runs() {
    let (_dir, conn) = setup_db().await;

    // Insert spec with original target chat 100.
    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('redirect', '*/5 * * * *', 'p', 1.0, 1, NULL, 100, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .await
        .unwrap();

    insert_async_cron_run(
        &conn,
        "run-pending",
        "redirect",
        "2026-01-01T00:01:00Z",
        Some("2026-01-01T00:02:00Z"),
        Some(0),
        "success",
        100,
        None,
        "pending",
    )
    .await;
    insert_async_cron_run(
        &conn,
        "run-retryable",
        "redirect",
        "2026-01-01T00:00:45Z",
        Some("2026-01-01T00:01:00Z"),
        Some(0),
        "success",
        100,
        None,
        "retryable",
    )
    .await;
    insert_async_cron_run(
        &conn,
        "run-delivered",
        "redirect",
        "2026-01-01T00:00:30Z",
        Some("2026-01-01T00:00:45Z"),
        Some(0),
        "success",
        100,
        None,
        "delivered",
    )
    .await;
    insert_async_cron_run(
        &conn,
        "run-none",
        "redirect",
        "2026-01-01T00:00:15Z",
        Some("2026-01-01T00:00:20Z"),
        Some(0),
        "success",
        100,
        None,
        "none",
    )
    .await;

    update_spec_partial(
        &conn,
        "redirect",
        None,
        None,
        None,
        None,
        None,
        None,
        Some(200),
        None,
    )
    .await
    .unwrap();

    // Spec target updated.
    let spec_chat: Option<i64> = conn
        .query_row(
            "SELECT target_chat_id FROM cron_specs WHERE job_name = 'redirect'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(spec_chat, Some(200));

    let pending_chat: Option<i64> = conn
        .query_row(
            "SELECT target_chat_id FROM async_runs WHERE id = 'run-pending'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(
        pending_chat,
        Some(200),
        "pending run must be redirected to the new chat"
    );

    let retryable_chat: Option<i64> = conn
        .query_row(
            "SELECT target_chat_id FROM async_runs WHERE id = 'run-retryable'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(
        retryable_chat,
        Some(200),
        "retryable run must be redirected to the new chat"
    );

    let delivered_chat: Option<i64> = conn
        .query_row(
            "SELECT target_chat_id FROM async_runs WHERE id = 'run-delivered'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(
        delivered_chat,
        Some(100),
        "delivered run must keep its historical target snapshot"
    );

    let none_chat: Option<i64> = conn
        .query_row(
            "SELECT target_chat_id FROM async_runs WHERE id = 'run-none'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(
        none_chat,
        Some(100),
        "non-delivery run must keep its historical target snapshot"
    );
}

/// Updating only `target_thread_id` (chat unchanged) must propagate the
/// new thread to undelivered runs while preserving the spec's chat.
#[tokio::test]
async fn update_spec_partial_propagates_thread_only_change() {
    let (_dir, conn) = setup_db().await;

    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('thr', '*/5 * * * *', 'p', 1.0, 1, NULL, 500, 7, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .await
        .unwrap();
    insert_async_cron_run(
        &conn,
        "run-thr",
        "thr",
        "2026-01-01T00:01:00Z",
        Some("2026-01-01T00:02:00Z"),
        Some(0),
        "success",
        500,
        Some(7),
        "pending",
    )
    .await;

    update_spec_partial(
        &conn,
        "thr",
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(Some(99)),
    )
    .await
    .unwrap();

    let (run_chat, run_thread): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT target_chat_id, target_thread_id FROM async_runs WHERE id = 'run-thr'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert_eq!(run_chat, Some(500), "chat must be preserved");
    assert_eq!(run_thread, Some(99), "thread must be redirected");
}

/// Updates that don't touch target columns (e.g. prompt-only) must NOT
/// rewrite async cron runs — those rows should retain the run-time snapshot
/// untouched.
#[tokio::test]
async fn update_spec_partial_non_target_change_leaves_runs_alone() {
    let (_dir, conn) = setup_db().await;

    conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('np', '*/5 * * * *', 'p', 1.0, 1, NULL, 100, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .await
        .unwrap();
    // Run snapshotted with a *different* (stale) target — simulates a run
    // that captured the spec target before some hypothetical earlier
    // redirect. A prompt-only update must not normalize it.
    insert_async_cron_run(
        &conn,
        "run-np",
        "np",
        "2026-01-01T00:01:00Z",
        Some("2026-01-01T00:02:00Z"),
        Some(0),
        "success",
        77,
        Some(3),
        "pending",
    )
    .await;

    update_spec_partial(
        &conn,
        "np",
        None,
        None,
        Some("new prompt"),
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let (run_chat, run_thread): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT target_chat_id, target_thread_id FROM async_runs WHERE id = 'run-np'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert_eq!(run_chat, Some(77));
    assert_eq!(run_thread, Some(3));
}

#[tokio::test]
async fn from_db_row_recurring() {
    let kind = ScheduleKind::from_db_row("*/5 * * * *", None, 1).unwrap();
    assert!(matches!(kind, ScheduleKind::Recurring(s) if s == "*/5 * * * *"));
}

#[tokio::test]
async fn from_db_row_one_shot_cron() {
    let kind = ScheduleKind::from_db_row("0 9 * * *", None, 0).unwrap();
    assert!(matches!(kind, ScheduleKind::OneShotCron(s) if s == "0 9 * * *"));
}

#[tokio::test]
async fn from_db_row_run_at() {
    let kind = ScheduleKind::from_db_row("", Some("2026-12-25T00:00:00Z"), 0).unwrap();
    assert!(matches!(kind, ScheduleKind::RunAt(_)));
}

#[tokio::test]
async fn from_db_row_immediate_sentinel() {
    let kind = ScheduleKind::from_db_row(IMMEDIATE_SENTINEL, None, 0).unwrap();
    assert!(matches!(kind, ScheduleKind::Immediate));
}

#[tokio::test]
async fn from_db_row_invalid_run_at_returns_err() {
    let err = ScheduleKind::from_db_row("", Some("not-a-date"), 0);
    assert!(err.is_err());
}

#[tokio::test]
async fn from_db_row_legacy_bg_schedule_errors() {
    let main = uuid::Uuid::new_v4();
    let err = ScheduleKind::from_db_row(&format!("@bg:{main}"), None, 0).unwrap_err();
    assert!(err.contains("no longer schedulable"));
}

#[tokio::test]
async fn load_specs_reads_model_column() {
    let (_dir, conn) = setup_db().await;
    conn.execute_batch(
        "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, model, created_at, updated_at) \
         VALUES ('with-model', '17 9 * * *', 'p', 5.0, 1, 'haiku', '2026-06-03T00:00:00Z', '2026-06-03T00:00:00Z'); \
         INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, created_at, updated_at) \
         VALUES ('no-model', '17 9 * * *', 'p', 5.0, 1, '2026-06-03T00:00:00Z', '2026-06-03T00:00:00Z');",
    )
    .await
    .unwrap();
    let specs = super::load_specs_from_db(&conn).await.unwrap();
    assert_eq!(specs["with-model"].model.as_deref(), Some("haiku"));
    assert_eq!(specs["no-model"].model, None);
}

#[test]
fn cron_spec_eq_reacts_to_model_change() {
    let base = super::CronSpec {
        schedule_kind: super::ScheduleKind::Recurring("17 9 * * *".into()),
        prompt: "p".into(),
        lock_ttl: None,
        max_budget_usd: 5.0,
        triggered_at: None,
        trigger_force_notify: false,
        target_chat_id: None,
        target_thread_id: None,
        model: Some("sonnet".into()),
    };
    let mut other = base.clone();
    assert_eq!(base, other);
    other.model = Some("haiku".into());
    assert_ne!(
        base, other,
        "changing model must make specs unequal so the reconciler reacts"
    );
}
