use chrono::Utc;
use rusqlite::{Connection, params};

#[derive(Debug, Clone, Copy)]
pub struct NewCronRun<'a> {
    pub id: &'a str,
    pub job_name: &'a str,
    pub started_at: &'a str,
    pub log_path: &'a str,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct NewBackgroundRun<'a> {
    pub id: &'a str,
    pub producer_ref: Option<&'a str>,
    pub source_session_id: &'a str,
    pub run_session_id: &'a str,
    pub target_chat_id: i64,
    pub target_thread_id: Option<i64>,
    pub created_at: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct RunOutput<'a> {
    pub summary: Option<&'a str>,
    pub notify_json: Option<&'a str>,
    pub no_notify_reason: Option<&'a str>,
    pub error_json: Option<&'a str>,
    pub delivery_required: bool,
}

#[derive(Debug, Clone)]
pub struct CronRunJsonRow {
    pub id: String,
    pub job_name: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub status: String,
    pub log_path: Option<String>,
    pub summary: Option<String>,
    pub notify_json: Option<String>,
    pub delivered_at: Option<String>,
    pub delivery_status: Option<String>,
    pub no_notify_reason: Option<String>,
}

fn require_updated(rows: usize) -> rusqlite::Result<()> {
    if rows == 0 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    Ok(())
}

pub fn insert_running_cron_run(conn: &Connection, run: NewCronRun<'_>) -> rusqlite::Result<()> {
    let target_chat_id = run.target_chat_id.ok_or_else(|| {
        rusqlite::Error::InvalidParameterName("target_chat_id is required".into())
    })?;

    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
            status, started_at, log_path, delivery_required, delivery_status,
            created_at, updated_at
         ) VALUES (
            ?1, 'cron', ?2, ?1, ?3, ?4,
            'running', ?5, ?6, 0, 'none',
            ?5, ?5
         )",
        params![
            run.id,
            run.job_name,
            target_chat_id,
            run.target_thread_id,
            run.started_at,
            run.log_path,
        ],
    )?;
    Ok(())
}

pub fn insert_queued_background_run(
    conn: &Connection,
    run: NewBackgroundRun<'_>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, source_session_id, run_session_id, target_chat_id, target_thread_id,
            status, handoff_state, delivery_required, delivery_status,
            created_at, updated_at
         ) VALUES (
            ?1, 'background', ?2, ?3, ?4, ?5, ?6,
            'queued', 'queued', 0, 'none',
            ?7, ?7
         )",
        params![
            run.id,
            run.producer_ref,
            run.source_session_id,
            run.run_session_id,
            run.target_chat_id,
            run.target_thread_id,
            run.created_at,
        ],
    )?;
    Ok(())
}

pub fn mark_background_spawned(
    conn: &Connection,
    run_id: &str,
    started_at: &str,
    log_path: &str,
) -> rusqlite::Result<()> {
    let rows = conn.execute(
        "UPDATE async_runs
         SET status = 'running',
             handoff_state = 'spawned',
             started_at = ?2,
             log_path = ?3,
             delivery_required = 1,
             delivery_status = 'pending',
             updated_at = ?2
         WHERE id = ?1",
        params![run_id, started_at, log_path],
    )?;
    require_updated(rows)
}

pub fn persist_run_output(
    conn: &Connection,
    run_id: &str,
    output: RunOutput<'_>,
) -> rusqlite::Result<()> {
    if output.delivery_required && output.notify_json.is_none() {
        return Err(rusqlite::Error::InvalidParameterName(
            "notify_json is required when delivery_required is true".into(),
        ));
    }

    let now = Utc::now().to_rfc3339();
    let delivery_status = if output.delivery_required {
        "pending"
    } else {
        "none"
    };

    let rows = conn.execute(
        "UPDATE async_runs
         SET summary = ?2,
             notify_json = ?3,
             no_notify_reason = ?4,
             error_json = ?5,
             delivery_required = ?6,
             delivery_status = ?7,
             updated_at = ?8
         WHERE id = ?1",
        params![
            run_id,
            output.summary,
            output.notify_json,
            output.no_notify_reason,
            output.error_json,
            output.delivery_required,
            delivery_status,
            now,
        ],
    )?;
    require_updated(rows)
}

pub fn finish_run(
    conn: &Connection,
    run_id: &str,
    exit_code: Option<i32>,
    status: &str,
) -> rusqlite::Result<()> {
    let now = Utc::now().to_rfc3339();
    let exit_code = exit_code.map(i64::from);

    let rows = conn.execute(
        "UPDATE async_runs
         SET finished_at = ?2,
             exit_code = ?3,
             status = ?4,
             updated_at = ?2
         WHERE id = ?1",
        params![run_id, now, exit_code, status],
    )?;
    require_updated(rows)
}

pub fn cron_run_to_json(row: &CronRunJsonRow) -> serde_json::Value {
    let mut val = serde_json::json!({
        "id": row.id,
        "job_name": row.job_name,
        "started_at": row.started_at,
        "finished_at": row.finished_at,
        "exit_code": row.exit_code,
        "status": row.status,
        "log_path": row.log_path,
        "delivered_at": row.delivered_at,
        "delivery_status": row.delivery_status,
        "no_notify_reason": row.no_notify_reason,
    });

    if let Some(summary) = &row.summary {
        val["summary"] = serde_json::Value::String(summary.clone());
    }

    if let Some(notify_json) = &row.notify_json
        && let Ok(notify) = serde_json::from_str::<serde_json::Value>(notify_json)
    {
        val["notify"] = notify;
    }

    val
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> rusqlite::Connection {
        let dir = tempfile::tempdir().unwrap();
        right_db::open_connection(dir.path(), true).unwrap()
    }

    #[test]
    fn insert_running_cron_run_sets_none_delivery() {
        let conn = setup();
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: Some(-100),
                target_thread_id: Some(7),
            },
        )
        .unwrap();

        let row: (String, String, i64, String) = conn
            .query_row(
                "SELECT kind, producer_ref, delivery_required, delivery_status FROM async_runs WHERE id='run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, ("cron".into(), "job-a".into(), 0, "none".into()));
    }

    #[test]
    fn insert_running_cron_run_requires_target_chat_id() {
        let conn = setup();
        let err = insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: None,
                target_thread_id: None,
            },
        )
        .expect_err("missing target_chat_id should fail");

        assert!(matches!(
            err,
            rusqlite::Error::InvalidParameterName(ref name)
                if name == "target_chat_id is required"
        ));

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM async_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn mark_run_output_with_notify_makes_delivery_pending() {
        let conn = setup();
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: Some(-100),
                target_thread_id: None,
            },
        )
        .unwrap();

        persist_run_output(
            &conn,
            "run-1",
            RunOutput {
                summary: Some("summary"),
                notify_json: Some("{\"content\":\"hi\"}"),
                no_notify_reason: None,
                error_json: None,
                delivery_required: true,
            },
        )
        .unwrap();

        let row: (i64, String, String) = conn
            .query_row(
                "SELECT delivery_required, delivery_status, notify_json FROM async_runs WHERE id='run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, (1, "pending".into(), "{\"content\":\"hi\"}".into()));
    }

    #[test]
    fn persist_run_output_requires_notify_when_delivery_required() {
        let conn = setup();
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: Some(-100),
                target_thread_id: None,
            },
        )
        .unwrap();

        let err = persist_run_output(
            &conn,
            "run-1",
            RunOutput {
                summary: Some("summary"),
                notify_json: None,
                no_notify_reason: None,
                error_json: None,
                delivery_required: true,
            },
        )
        .expect_err("delivery_required without notify_json should fail");

        assert!(matches!(
            err,
            rusqlite::Error::InvalidParameterName(ref name)
                if name == "notify_json is required when delivery_required is true"
        ));

        let row: (i64, String, Option<String>) = conn
            .query_row(
                "SELECT delivery_required, delivery_status, notify_json FROM async_runs WHERE id='run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, (0, "none".into(), None));
    }

    #[test]
    fn cron_run_to_json_parses_notify() {
        let row = CronRunJsonRow {
            id: "run-1".into(),
            job_name: "job-a".into(),
            started_at: "2026-05-18T10:00:00Z".into(),
            finished_at: None,
            exit_code: None,
            status: "success".into(),
            log_path: Some("/log".into()),
            summary: Some("summary".into()),
            notify_json: Some("{\"content\":\"hello\"}".into()),
            delivered_at: None,
            delivery_status: Some("pending".into()),
            no_notify_reason: None,
        };

        let json = cron_run_to_json(&row);
        assert_eq!(json["job_name"], "job-a");
        assert_eq!(json["notify"]["content"], "hello");
    }

    #[test]
    fn update_helpers_return_err_for_missing_run_id() {
        let conn = setup();
        let spawned_err =
            mark_background_spawned(&conn, "missing", "2026-05-18T10:01:00Z", "/log/bg-1.ndjson")
                .expect_err("missing background run should fail");
        assert!(matches!(spawned_err, rusqlite::Error::QueryReturnedNoRows));

        let output_err = persist_run_output(
            &conn,
            "missing",
            RunOutput {
                summary: Some("summary"),
                notify_json: Some("{\"content\":\"hi\"}"),
                no_notify_reason: None,
                error_json: None,
                delivery_required: true,
            },
        )
        .expect_err("missing output run should fail");
        assert!(matches!(output_err, rusqlite::Error::QueryReturnedNoRows));

        let finish_err = finish_run(&conn, "missing", Some(0), "success")
            .expect_err("missing finished run should fail");
        assert!(matches!(finish_err, rusqlite::Error::QueryReturnedNoRows));
    }

    #[test]
    fn insert_queued_background_run_then_mark_spawned_sets_pending_delivery() {
        let conn = setup();
        insert_queued_background_run(
            &conn,
            NewBackgroundRun {
                id: "bg-1",
                producer_ref: Some("bg-job"),
                source_session_id: "main-session",
                run_session_id: "bg-session",
                target_chat_id: -100,
                target_thread_id: Some(7),
                created_at: "2026-05-18T10:00:00Z",
            },
        )
        .unwrap();

        let queued: (String, Option<String>, String, String, i64, String) = conn
            .query_row(
                "SELECT kind, producer_ref, source_session_id, handoff_state, delivery_required, delivery_status FROM async_runs WHERE id='bg-1'",
                [],
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
            .unwrap();
        assert_eq!(
            queued,
            (
                "background".into(),
                Some("bg-job".into()),
                "main-session".into(),
                "queued".into(),
                0,
                "none".into(),
            )
        );

        mark_background_spawned(&conn, "bg-1", "2026-05-18T10:01:00Z", "/log/bg-1.ndjson").unwrap();

        let spawned: (String, String, String, i64, String) = conn
            .query_row(
                "SELECT status, handoff_state, log_path, delivery_required, delivery_status FROM async_runs WHERE id='bg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            spawned,
            (
                "running".into(),
                "spawned".into(),
                "/log/bg-1.ndjson".into(),
                1,
                "pending".into(),
            )
        );
    }

    #[test]
    fn finish_run_sets_terminal_fields() {
        let conn = setup();
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: Some(-100),
                target_thread_id: None,
            },
        )
        .unwrap();

        finish_run(&conn, "run-1", Some(0), "success").unwrap();

        let row: (Option<String>, Option<i64>, String, String) = conn
            .query_row(
                "SELECT finished_at, exit_code, status, updated_at FROM async_runs WHERE id='run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.1, Some(0));
        assert_eq!(row.2, "success");
        assert_eq!(row.0, Some(row.3));
    }
}
