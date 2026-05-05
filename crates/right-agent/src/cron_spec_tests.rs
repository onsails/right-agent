    use super::*;

    #[test]
    fn validate_job_name_valid() {
        assert!(validate_job_name("health-check").is_ok());
        assert!(validate_job_name("a").is_ok());
        assert!(validate_job_name("deploy-check-123").is_ok());
    }

    #[test]
    fn validate_job_name_invalid() {
        assert!(validate_job_name("").is_err());
        assert!(validate_job_name("-leading").is_err());
        assert!(validate_job_name("UPPER").is_err());
        assert!(validate_job_name("has space").is_err());
        assert!(validate_job_name("under_score").is_err());
    }

    #[test]
    fn validate_schedule_valid() {
        assert!(validate_schedule("*/5 * * * *").is_ok());
        assert!(validate_schedule("17 9 * * 1-5").is_ok());
    }

    #[test]
    fn validate_schedule_invalid() {
        assert!(validate_schedule("not a cron").is_err());
        assert!(validate_schedule("").is_err());
    }

    #[test]
    fn validate_schedule_round_minutes_warning() {
        assert!(validate_schedule("0 9 * * *").unwrap().is_some());
        assert!(validate_schedule("30 9 * * *").unwrap().is_some());
        assert!(validate_schedule("17 9 * * *").unwrap().is_none());
    }

    #[test]
    fn validate_lock_ttl_valid() {
        assert!(validate_lock_ttl("30m").is_ok());
        assert!(validate_lock_ttl("1h").is_ok());
    }

    #[test]
    fn validate_lock_ttl_invalid() {
        assert!(validate_lock_ttl("bad").is_err());
        assert!(validate_lock_ttl("30").is_err());
        assert!(validate_lock_ttl("").is_err());
    }

    fn setup_db() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::migrations::MIGRATIONS
            .to_latest(&mut conn)
            .unwrap();
        conn
    }

    #[test]
    fn create_spec_success() {
        let conn = setup_db();
        let result = create_spec(&conn, "my-job", "*/5 * * * *", "do stuff", None, None).unwrap();
        assert!(result.message.contains("Created"));
        assert!(result.warning.is_none());

        // Verify row exists.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cron_specs WHERE job_name = 'my-job'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn create_spec_with_warning() {
        let conn = setup_db();
        let result = create_spec(&conn, "my-job", "0 9 * * *", "do stuff", None, None).unwrap();
        assert!(result.warning.is_some());
    }

    #[test]
    fn create_spec_duplicate_error() {
        let conn = setup_db();
        create_spec(&conn, "dup", "*/5 * * * *", "prompt", None, None).unwrap();
        let err = create_spec(&conn, "dup", "*/5 * * * *", "prompt", None, None).unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn create_spec_validation_errors() {
        let conn = setup_db();
        // Bad job name
        assert!(create_spec(&conn, "BAD NAME", "*/5 * * * *", "p", None, None).is_err());
        // Empty prompt
        assert!(create_spec(&conn, "ok", "*/5 * * * *", "  ", None, None).is_err());
        // Bad schedule
        assert!(create_spec(&conn, "ok", "not-cron", "p", None, None).is_err());
        // Bad lock_ttl
        assert!(create_spec(&conn, "ok", "*/5 * * * *", "p", Some("bad"), None).is_err());
        // Negative budget
        assert!(create_spec(&conn, "ok", "*/5 * * * *", "p", None, Some(-1.0)).is_err());
    }

    #[test]
    fn update_spec_success() {
        let conn = setup_db();
        create_spec(&conn, "upd", "*/5 * * * *", "old", None, None).unwrap();
        let result = update_spec(
            &conn,
            "upd",
            "17 9 * * *",
            "new prompt",
            Some("1h"),
            Some(2.0),
        )
        .unwrap();
        assert!(result.message.contains("Updated"));

        let prompt: String = conn
            .query_row(
                "SELECT prompt FROM cron_specs WHERE job_name = 'upd'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(prompt, "new prompt");
    }

    #[test]
    fn update_spec_not_found() {
        let conn = setup_db();
        let err = update_spec(&conn, "ghost", "*/5 * * * *", "prompt", None, None).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn delete_spec_success() {
        let conn = setup_db();
        let tmp = tempfile::tempdir().unwrap();
        create_spec(&conn, "del", "*/5 * * * *", "p", None, None).unwrap();
        let msg = delete_spec(&conn, "del", tmp.path()).unwrap();
        assert!(msg.contains("Deleted"));

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cron_specs WHERE job_name = 'del'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn delete_spec_not_found() {
        let conn = setup_db();
        let tmp = tempfile::tempdir().unwrap();
        let err = delete_spec(&conn, "nope", tmp.path()).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn list_specs_json() {
        let conn = setup_db();
        create_spec(&conn, "a-job", "*/5 * * * *", "prompt a", None, None).unwrap();
        create_spec(
            &conn,
            "b-job",
            "17 9 * * *",
            "prompt b",
            Some("30m"),
            Some(2.5),
        )
        .unwrap();
        let output = list_specs(&conn).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["job_name"], "a-job");
        assert_eq!(parsed[1]["job_name"], "b-job");
        assert_eq!(parsed[1]["max_budget_usd"], 2.5);
        // No runs yet — last_run_at and last_status should be null
        assert!(parsed[0]["last_run_at"].is_null());
        assert!(parsed[0]["last_status"].is_null());
        assert!(parsed[1]["last_run_at"].is_null());
        assert!(parsed[1]["last_status"].is_null());
    }

    #[test]
    fn list_specs_includes_last_run() {
        let conn = setup_db();
        create_spec(&conn, "a-job", "*/5 * * * *", "prompt a", None, None).unwrap();
        // Insert two runs — only the latest should appear
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path) \
             VALUES ('run-old', 'a-job', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 0, 'success', '/tmp/old.log')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path) \
             VALUES ('run-new', 'a-job', '2026-01-02T00:00:00Z', '2026-01-02T00:01:00Z', 1, 'failed', '/tmp/new.log')",
            [],
        )
        .unwrap();
        let output = list_specs(&conn).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["last_run_at"], "2026-01-02T00:00:00Z");
        assert_eq!(parsed[0]["last_status"], "failed");
    }

    #[test]
    fn load_specs_from_db_empty() {
        let conn = setup_db();
        let specs = load_specs_from_db(&conn).unwrap();
        assert!(specs.is_empty());
    }

    #[test]
    fn load_specs_from_db_returns_all() {
        let conn = setup_db();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('job1', '*/5 * * * *', 'do stuff', 0.5, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at) \
             VALUES ('job2', '17 9 * * *', 'other', '1h', 1.0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let specs = load_specs_from_db(&conn).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(
            specs["job1"].schedule_kind.cron_schedule().unwrap(),
            "*/5 * * * *"
        );
        assert_eq!(specs["job1"].max_budget_usd, 0.5);
        assert_eq!(specs["job2"].lock_ttl.as_deref(), Some("1h"));
    }

    #[test]
    fn trigger_spec_sets_timestamp() {
        let conn = setup_db();
        create_spec(&conn, "trig-job", "*/5 * * * *", "do stuff", None, None).unwrap();
        let msg = trigger_spec(&conn, "trig-job").unwrap();
        assert!(msg.contains("Triggered"));
        let ts: Option<String> = conn
            .query_row(
                "SELECT triggered_at FROM cron_specs WHERE job_name = 'trig-job'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ts.is_some(), "triggered_at should be set");
    }

    #[test]
    fn trigger_spec_nonexistent_job() {
        let conn = setup_db();
        let err = trigger_spec(&conn, "ghost").unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn trigger_spec_idempotent() {
        let conn = setup_db();
        create_spec(&conn, "idem-job", "*/5 * * * *", "do stuff", None, None).unwrap();
        trigger_spec(&conn, "idem-job").unwrap();
        trigger_spec(&conn, "idem-job").unwrap();
        let ts: Option<String> = conn
            .query_row(
                "SELECT triggered_at FROM cron_specs WHERE job_name = 'idem-job'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ts.is_some());
    }

    #[test]
    fn clear_triggered_at_clears() {
        let conn = setup_db();
        create_spec(&conn, "clr-job", "*/5 * * * *", "do stuff", None, None).unwrap();
        trigger_spec(&conn, "clr-job").unwrap();
        clear_triggered_at(&conn, "clr-job").unwrap();
        let ts: Option<String> = conn
            .query_row(
                "SELECT triggered_at FROM cron_specs WHERE job_name = 'clr-job'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ts.is_none(), "triggered_at should be cleared");
    }

    #[test]
    fn describe_schedule_returns_description() {
        let desc = describe_schedule("*/5 * * * *");
        assert!(!desc.is_empty());
    }

    #[test]
    fn describe_schedule_fallback_on_invalid() {
        let desc = describe_schedule("not-valid-cron");
        assert_eq!(desc, "not-valid-cron");
    }

    #[test]
    fn get_spec_detail_found() {
        let conn = setup_db();
        create_spec(
            &conn,
            "detail-job",
            "*/5 * * * *",
            "do stuff",
            Some("1h"),
            Some(2.5),
        )
        .unwrap();
        let detail = get_spec_detail(&conn, "detail-job").unwrap().unwrap();
        assert_eq!(detail.job_name, "detail-job");
        assert_eq!(detail.schedule, "*/5 * * * *");
        assert_eq!(detail.prompt, "do stuff");
        assert_eq!(detail.lock_ttl.as_deref(), Some("1h"));
        assert!((detail.max_budget_usd - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn get_spec_detail_not_found() {
        let conn = setup_db();
        let detail = get_spec_detail(&conn, "ghost").unwrap();
        assert!(detail.is_none());
    }

    #[test]
    fn get_recent_runs_returns_ordered() {
        let conn = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path) \
             VALUES ('r1', 'runs-job', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 0, 'success', '/tmp/r1.txt')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path) \
             VALUES ('r2', 'runs-job', '2026-01-01T01:00:00Z', '2026-01-01T01:01:00Z', 1, 'failed', '/tmp/r2.txt')",
            [],
        )
        .unwrap();
        let runs = get_recent_runs(&conn, "runs-job", 5).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "r2");
        assert_eq!(runs[1].id, "r1");
        assert_eq!(runs[0].status, "failed");
    }

    #[test]
    fn get_recent_runs_empty() {
        let conn = setup_db();
        let runs = get_recent_runs(&conn, "no-such-job", 5).unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn get_recent_runs_respects_limit() {
        let conn = setup_db();
        for i in 0..10 {
            conn.execute(
                "INSERT INTO cron_runs (id, job_name, started_at, status, log_path) \
                 VALUES (?1, 'limit-job', ?2, 'success', '/tmp/r.txt')",
                rusqlite::params![format!("r{i}"), format!("2026-01-01T{i:02}:00:00Z")],
            )
            .unwrap();
        }
        let runs = get_recent_runs(&conn, "limit-job", 3).unwrap();
        assert_eq!(runs.len(), 3);
    }

    /// Regression: triggered_at must NOT affect CronSpec equality.
    /// The reconciler compares old vs new specs to detect config changes.
    /// If triggered_at participates in PartialEq, triggering a job causes the
    /// reconciler to abort and respawn the job scheduler in an infinite loop.
    #[test]
    fn triggered_at_does_not_affect_equality() {
        let base = CronSpec {
            schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "do stuff".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            target_chat_id: None,
            target_thread_id: None,
        };
        let triggered = CronSpec {
            triggered_at: Some("2026-04-15T12:00:00Z".into()),
            ..base.clone()
        };
        assert_eq!(base, triggered, "triggered_at must not affect equality");
    }

    #[test]
    fn spec_equality_detects_real_changes() {
        let base = CronSpec {
            schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "do stuff".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            target_chat_id: None,
            target_thread_id: None,
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
        assert_ne!(base, changed_target, "target_chat_id change must be a real change");
    }

    #[test]
    fn load_specs_includes_triggered_at() {
        let conn = setup_db();
        create_spec(&conn, "tr-load", "*/5 * * * *", "p", None, None).unwrap();
        trigger_spec(&conn, "tr-load").unwrap();
        let specs = load_specs_from_db(&conn).unwrap();
        assert!(specs["tr-load"].triggered_at.is_some());
    }

    #[test]
    fn load_specs_from_db_carries_target_fields() {
        let conn = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('with-target', '*/5 * * * *', 'p', NULL, 1.0, 1, -555, 9, ?1, ?1)",
            [&now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, created_at, updated_at) \
             VALUES ('no-target', '*/5 * * * *', 'p', NULL, 1.0, 1, ?1, ?1)",
            [&now],
        )
        .unwrap();

        let specs = load_specs_from_db(&conn).unwrap();
        let with = &specs["with-target"];
        assert_eq!(with.target_chat_id, Some(-555));
        assert_eq!(with.target_thread_id, Some(9));

        let without = &specs["no-target"];
        assert_eq!(without.target_chat_id, None);
        assert_eq!(without.target_thread_id, None);
    }

    #[test]
    fn create_spec_v2_with_run_at_succeeds() {
        let conn = setup_db();
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
        .unwrap();
        assert!(result.message.contains("Created"));
    }

    #[test]
    fn create_spec_v2_with_both_schedule_and_run_at_fails() {
        let conn = setup_db();
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
        .unwrap_err();
        assert!(err.contains("mutually exclusive"));
    }

    #[test]
    fn create_spec_v2_with_neither_schedule_nor_run_at_fails() {
        let conn = setup_db();
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
        .unwrap_err();
        assert!(err.contains("one of"));
    }

    #[test]
    fn create_spec_v2_with_invalid_run_at_fails() {
        let conn = setup_db();
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
        .unwrap_err();
        assert!(err.contains("invalid"));
    }

    #[test]
    fn create_spec_v2_with_past_run_at_succeeds() {
        let conn = setup_db();
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
        .unwrap();
        assert!(result.message.contains("Created"));
    }

    #[test]
    fn create_spec_v2_recurring_false_stored_as_one_shot_cron() {
        let conn = setup_db();
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
        .unwrap();
        let specs = load_specs_from_db(&conn).unwrap();
        assert!(matches!(
            specs["oneshot-cron"].schedule_kind,
            ScheduleKind::OneShotCron(_)
        ));
    }

    #[test]
    fn load_specs_round_trips_all_schedule_kinds() {
        let conn = setup_db();
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
        .unwrap();

        let specs = load_specs_from_db(&conn).unwrap();
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

    #[test]
    fn update_spec_partial_prompt_only() {
        let conn = setup_db();
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
        .unwrap();
        let detail = get_spec_detail(&conn, "partial").unwrap().unwrap();
        assert_eq!(detail.prompt, "new prompt");
        assert_eq!(detail.schedule, "*/5 * * * *");
        assert!((detail.max_budget_usd - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn update_spec_partial_schedule_clears_run_at() {
        let conn = setup_db();
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
        .unwrap();
        let specs = load_specs_from_db(&conn).unwrap();
        assert!(matches!(
            specs["switch"].schedule_kind,
            ScheduleKind::Recurring(_)
        ));
    }

    #[test]
    fn update_spec_partial_run_at_clears_schedule() {
        let conn = setup_db();
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
        .unwrap();
        let specs = load_specs_from_db(&conn).unwrap();
        assert!(matches!(
            specs["switch2"].schedule_kind,
            ScheduleKind::RunAt(_)
        ));
    }

    #[test]
    fn update_spec_partial_both_schedule_and_run_at_fails() {
        let conn = setup_db();
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
        .unwrap_err();
        assert!(err.contains("mutually exclusive"));
    }

    #[test]
    fn update_spec_partial_no_fields_fails() {
        let conn = setup_db();
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
        .unwrap();
        let err = update_spec_partial(
            &conn, "empty", None, None, None, None, None, None, None, None,
        )
        .unwrap_err();
        assert!(err.contains("at least one"));
    }

    #[test]
    fn update_spec_partial_not_found() {
        let conn = setup_db();
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
        .unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn create_spec_v2_persists_target_fields() {
        let conn = setup_db();
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
        .unwrap();

        let (chat, thread): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT target_chat_id, target_thread_id FROM cron_specs WHERE job_name = 'with-target'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(chat, Some(-100));
        assert_eq!(thread, Some(7));
    }

    #[test]
    fn create_spec_v2_persists_null_target_when_omitted() {
        let conn = setup_db();
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
        .unwrap();

        let (chat, thread): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT target_chat_id, target_thread_id FROM cron_specs WHERE job_name = 'no-target'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(chat.is_none());
        assert!(thread.is_none());
    }

    #[test]
    fn update_spec_partial_sets_target_chat_id() {
        let conn = setup_db();
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
        .unwrap();
        let chat: Option<i64> = conn
            .query_row(
                "SELECT target_chat_id FROM cron_specs WHERE job_name='j1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(chat, Some(-555));
    }

    #[test]
    fn update_spec_partial_clears_target_thread_id() {
        let conn = setup_db();
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
        .unwrap();
        let thread: Option<i64> = conn
            .query_row(
                "SELECT target_thread_id FROM cron_specs WHERE job_name='j1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(thread.is_none(), "thread must be cleared");
    }

    #[test]
    fn update_spec_partial_leaves_target_when_omitted() {
        let conn = setup_db();
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
        .unwrap();
        let (chat, thread): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT target_chat_id, target_thread_id FROM cron_specs WHERE job_name='j1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(chat, Some(-1));
        assert_eq!(thread, Some(42));
    }

    #[test]
    fn load_specs_round_trips_immediate() {
        let conn = setup_db();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, created_at, updated_at) \
             VALUES ('imm', '@immediate', 'do it now', 5.0, 0, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let specs = load_specs_from_db(&conn).unwrap();
        assert!(matches!(specs["imm"].schedule_kind, ScheduleKind::Immediate));
    }

    #[test]
    fn immediate_is_one_shot() {
        assert!(ScheduleKind::Immediate.is_one_shot());
        assert!(ScheduleKind::Immediate.cron_schedule().is_none());
    }

    #[test]
    fn list_specs_includes_target_fields() {
        let conn = setup_db();
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
        .unwrap();
        let json = list_specs(&conn).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let row = &value.as_array().unwrap()[0];
        assert_eq!(row["target_chat_id"].as_i64(), Some(-100));
        assert_eq!(row["target_thread_id"].as_i64(), Some(5));
    }

    #[test]
    fn resolve_schedule_fields_immediate_mutex() {
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

    #[test]
    fn create_spec_v2_immediate_inserts_sentinel() {
        let conn = setup_db();
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
        .unwrap();
        let stored: (String, i64, Option<String>, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT schedule, recurring, run_at, target_chat_id, target_thread_id FROM cron_specs WHERE job_name = 'bg-test'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(stored.0, IMMEDIATE_SENTINEL);
        assert_eq!(stored.1, 0);
        assert!(stored.2.is_none());
        assert_eq!(stored.3, Some(-100));
        assert_eq!(stored.4, Some(7));
    }

    #[test]
    fn insert_immediate_cron_uses_default_budget_when_none() {
        let conn = setup_db();
        insert_immediate_cron(&conn, "bg-2", "prompt", -42, Some(0), None).unwrap();
        let budget: f64 = conn
            .query_row(
                "SELECT max_budget_usd FROM cron_specs WHERE job_name = 'bg-2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((budget - DEFAULT_CRON_BUDGET_USD).abs() < f64::EPSILON);
    }

    /// `insert_immediate_cron` substitutes [`IMMEDIATE_DEFAULT_LOCK_TTL`] when
    /// the caller passes no explicit lock_ttl. This prevents the reader-side
    /// `unwrap_or("30m")` default from letting the reconciler spawn a duplicate
    /// `execute_job` against a long-running bg-continuation spec after 30 min.
    #[test]
    fn insert_immediate_cron_defaults_lock_ttl_to_six_hours() {
        let conn = setup_db();
        insert_immediate_cron(&conn, "bg-3", "prompt", -42, None, None).unwrap();
        let lock_ttl: Option<String> = conn
            .query_row(
                "SELECT lock_ttl FROM cron_specs WHERE job_name = 'bg-3'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lock_ttl.as_deref(), Some(IMMEDIATE_DEFAULT_LOCK_TTL));
        assert_eq!(lock_ttl.as_deref(), Some("6h"));
    }

    /// Regression: changing `target_chat_id` via `update_spec_partial` must
    /// also redirect any already-finished-but-undelivered `cron_runs` rows
    /// to the new chat. Pre-snapshot the delivery loop re-read the spec via
    /// LEFT JOIN, so a target change took effect immediately. Now that
    /// `cron_runs` carries its own snapshot we have to propagate the update
    /// alongside the spec write to preserve that user-visible behavior.
    /// Delivered runs (delivered_at IS NOT NULL) must NOT be rewritten —
    /// they reflect what was actually sent.
    #[test]
    fn update_spec_partial_propagates_target_to_undelivered_runs() {
        let conn = setup_db();

        // Insert spec with original target chat 100.
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('redirect', '*/5 * * * *', 'p', 1.0, 1, NULL, 100, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        // Undelivered run (status='success', notify_json present, delivered_at NULL),
        // snapshotted at insert time with target_chat_id=100.
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path, notify_json, delivered_at, target_chat_id, target_thread_id) \
             VALUES ('run-undelivered', 'redirect', '2026-01-01T00:01:00Z', '2026-01-01T00:02:00Z', 0, 'success', '/tmp/log', '{\"reply\":\"hi\"}', NULL, 100, NULL)",
            [],
        )
        .unwrap();

        // Already-delivered run with the old target — must remain at 100.
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path, notify_json, delivered_at, target_chat_id, target_thread_id) \
             VALUES ('run-delivered', 'redirect', '2026-01-01T00:00:30Z', '2026-01-01T00:00:45Z', 0, 'success', '/tmp/log', '{\"reply\":\"hi\"}', '2026-01-01T00:00:50Z', 100, NULL)",
            [],
        )
        .unwrap();

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
        .unwrap();

        // Spec target updated.
        let spec_chat: Option<i64> = conn
            .query_row(
                "SELECT target_chat_id FROM cron_specs WHERE job_name = 'redirect'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(spec_chat, Some(200));

        // Undelivered run redirected.
        let undelivered_chat: Option<i64> = conn
            .query_row(
                "SELECT target_chat_id FROM cron_runs WHERE id = 'run-undelivered'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            undelivered_chat,
            Some(200),
            "undelivered run must be redirected to the new chat"
        );

        // Delivered run untouched.
        let delivered_chat: Option<i64> = conn
            .query_row(
                "SELECT target_chat_id FROM cron_runs WHERE id = 'run-delivered'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            delivered_chat,
            Some(100),
            "delivered run must keep its historical target snapshot"
        );
    }

    /// Updating only `target_thread_id` (chat unchanged) must propagate the
    /// new thread to undelivered runs while preserving the spec's chat.
    #[test]
    fn update_spec_partial_propagates_thread_only_change() {
        let conn = setup_db();

        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('thr', '*/5 * * * *', 'p', 1.0, 1, NULL, 500, 7, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path, notify_json, delivered_at, target_chat_id, target_thread_id) \
             VALUES ('run-thr', 'thr', '2026-01-01T00:01:00Z', '2026-01-01T00:02:00Z', 0, 'success', '/tmp/log', '{}', NULL, 500, 7)",
            [],
        )
        .unwrap();

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
        .unwrap();

        let (run_chat, run_thread): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT target_chat_id, target_thread_id FROM cron_runs WHERE id = 'run-thr'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(run_chat, Some(500), "chat must be preserved");
        assert_eq!(run_thread, Some(99), "thread must be redirected");
    }

    /// Updates that don't touch target columns (e.g. prompt-only) must NOT
    /// rewrite cron_runs — those rows should retain the run-time snapshot
    /// untouched.
    #[test]
    fn update_spec_partial_non_target_change_leaves_runs_alone() {
        let conn = setup_db();

        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('np', '*/5 * * * *', 'p', 1.0, 1, NULL, 100, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        // Run snapshotted with a *different* (stale) target — simulates a run
        // that captured the spec target before some hypothetical earlier
        // redirect. A prompt-only update must not normalize it.
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path, notify_json, delivered_at, target_chat_id, target_thread_id) \
             VALUES ('run-np', 'np', '2026-01-01T00:01:00Z', '2026-01-01T00:02:00Z', 0, 'success', '/tmp/log', '{}', NULL, 77, 3)",
            [],
        )
        .unwrap();

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
        .unwrap();

        let (run_chat, run_thread): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT target_chat_id, target_thread_id FROM cron_runs WHERE id = 'run-np'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(run_chat, Some(77));
        assert_eq!(run_thread, Some(3));
    }
