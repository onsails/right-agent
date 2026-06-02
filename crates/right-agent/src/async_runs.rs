use chrono::Utc;
use right_db::{Connection, DbError, params};

#[derive(Debug, Clone, Copy)]
pub struct NewCronRun<'a> {
    pub id: &'a str,
    pub job_name: &'a str,
    pub started_at: &'a str,
    pub log_path: &'a str,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
    pub force_notify: bool,
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
    pub run_note: Option<&'a str>,
    pub delivery_json: Option<&'a str>,
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
    pub run_note: Option<String>,
    pub delivery_json: Option<String>,
    pub delivered_at: Option<String>,
    pub delivery_status: Option<String>,
}

fn require_updated(rows: usize) -> Result<(), DbError> {
    if rows == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn insert_running_cron_run(
    conn: &Connection,
    run: NewCronRun<'_>,
) -> Result<(), DbError> {
    let target_chat_id = run
        .target_chat_id
        .ok_or_else(|| DbError::InvalidParameter("target_chat_id is required".into()))?;

    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
            status, started_at, log_path, delivery_required, delivery_status,
            force_notify, created_at, updated_at
         ) VALUES (
            ?1, 'cron', ?2, ?1, ?3, ?4,
            'running', ?5, ?6, 0, 'none',
            ?7, ?5, ?5
         )",
        params![
            run.id,
            run.job_name,
            target_chat_id,
            run.target_thread_id,
            run.started_at,
            run.log_path,
            run.force_notify,
        ],
    )
    .await?;
    Ok(())
}

pub async fn insert_queued_background_run(
    conn: &Connection,
    run: NewBackgroundRun<'_>,
) -> Result<(), DbError> {
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
    )
    .await?;
    Ok(())
}

pub async fn mark_background_spawned(
    conn: &Connection,
    run_id: &str,
    started_at: &str,
    log_path: &str,
) -> Result<(), DbError> {
    let rows = conn
        .execute(
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
        )
        .await?;
    require_updated(rows)
}

pub async fn persist_run_output(
    conn: &Connection,
    run_id: &str,
    output: RunOutput<'_>,
) -> Result<(), DbError> {
    if output.delivery_required && output.delivery_json.is_none() {
        return Err(DbError::InvalidParameter(
            "delivery_json is required when delivery_required is true".into(),
        ));
    }

    let now = Utc::now().to_rfc3339();
    let delivery_status = if output.delivery_required {
        "pending"
    } else {
        "none"
    };

    let rows = conn
        .execute(
            "UPDATE async_runs
         SET run_note = ?2,
             delivery_json = ?3,
             error_json = ?4,
             delivery_required = ?5,
             delivery_status = ?6,
             updated_at = ?7
         WHERE id = ?1",
            params![
                run_id,
                output.run_note,
                output.delivery_json,
                output.error_json,
                output.delivery_required,
                delivery_status,
                now,
            ],
        )
        .await?;
    require_updated(rows)
}

pub async fn finish_run(
    conn: &Connection,
    run_id: &str,
    exit_code: Option<i32>,
    status: &str,
) -> Result<(), DbError> {
    let now = Utc::now().to_rfc3339();
    let exit_code = exit_code.map(i64::from);

    let rows = conn
        .execute(
            "UPDATE async_runs
         SET finished_at = ?2,
             exit_code = ?3,
             status = ?4,
             updated_at = ?2
         WHERE id = ?1",
            params![run_id, now, exit_code, status],
        )
        .await?;
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
    });

    if let Some(run_note) = &row.run_note {
        val["run_note"] = serde_json::Value::String(run_note.clone());
    }

    if let Some(delivery_json) = &row.delivery_json
        && let Ok(delivery) = serde_json::from_str::<serde_json::Value>(delivery_json)
    {
        val["delivery"] = delivery;
    }

    val
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> right_db::Connection {
        let dir = tempfile::tempdir().unwrap();
        right_db::open_connection(dir.path(), true).await.unwrap()
    }

    #[tokio::test]
    async fn insert_running_cron_run_sets_none_delivery() {
        let conn = setup().await;
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: Some(-100),
                target_thread_id: Some(7),
                force_notify: false,
            },
        )
        .await
        .unwrap();

        let row: (String, String, i64, String) = conn
            .query_row(
                "SELECT kind, producer_ref, delivery_required, delivery_status FROM async_runs WHERE id='run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row, ("cron".into(), "job-a".into(), 0, "none".into()));
    }

    #[tokio::test]
    async fn insert_running_cron_run_requires_target_chat_id() {
        let conn = setup().await;
        let err = insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: None,
                target_thread_id: None,
                force_notify: false,
            },
        )
        .await
        .expect_err("missing target_chat_id should fail");

        assert!(matches!(
            err,
            right_db::DbError::InvalidParameter(ref name)
                if name == "target_chat_id is required"
        ));

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM async_runs", [], |r| r.get(0))
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn mark_run_output_with_delivery_makes_delivery_pending() {
        let conn = setup().await;
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: Some(-100),
                target_thread_id: None,
                force_notify: false,
            },
        )
        .await
        .unwrap();

        persist_run_output(
            &conn,
            "run-1",
            RunOutput {
                run_note: Some("summary"),
                delivery_json: Some(r#"{"kind":"notify","content":"hi"}"#),
                error_json: None,
                delivery_required: true,
            },
        )
        .await
        .unwrap();

        let row: (i64, String, String) = conn
            .query_row(
                "SELECT delivery_required, delivery_status, delivery_json FROM async_runs WHERE id='run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(
            row,
            (
                1,
                "pending".into(),
                r#"{"kind":"notify","content":"hi"}"#.into()
            )
        );
    }

    #[tokio::test]
    async fn persist_run_output_requires_delivery_json_when_delivery_required() {
        let conn = setup().await;
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: Some(-100),
                target_thread_id: None,
                force_notify: false,
            },
        )
        .await
        .unwrap();

        let err = persist_run_output(
            &conn,
            "run-1",
            RunOutput {
                run_note: Some("note"),
                delivery_json: None,
                error_json: None,
                delivery_required: true,
            },
        )
        .await
        .expect_err("delivery_required without delivery_json should fail");

        assert!(matches!(
            err,
            right_db::DbError::InvalidParameter(ref name)
                if name == "delivery_json is required when delivery_required is true"
        ));

        let row: (i64, String, Option<String>) = conn
            .query_row(
                "SELECT delivery_required, delivery_status, delivery_json FROM async_runs WHERE id='run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(row, (0, "none".into(), None));
    }

    #[tokio::test]
    async fn cron_run_to_json_parses_delivery() {
        let row = CronRunJsonRow {
            id: "run-1".into(),
            job_name: "job-a".into(),
            started_at: "2026-05-18T10:00:00Z".into(),
            finished_at: None,
            exit_code: None,
            status: "success".into(),
            log_path: Some("/log".into()),
            run_note: Some("summary".into()),
            delivery_json: Some(r#"{"kind":"notify","content":"hello"}"#.into()),
            delivered_at: None,
            delivery_status: Some("pending".into()),
        };

        let json = cron_run_to_json(&row);
        assert_eq!(json["job_name"], "job-a");
        assert_eq!(json["run_note"], "summary");
        assert_eq!(json["delivery"]["content"], "hello");
        assert!(json.get("notify").is_none());
    }

    #[tokio::test]
    async fn update_helpers_return_err_for_missing_run_id() {
        let conn = setup().await;
        let spawned_err =
            mark_background_spawned(&conn, "missing", "2026-05-18T10:01:00Z", "/log/bg-1.ndjson")
                .await
                .expect_err("missing background run should fail");
        assert!(matches!(spawned_err, right_db::DbError::NotFound));

        let output_err = persist_run_output(
            &conn,
            "missing",
            RunOutput {
                run_note: Some("summary"),
                delivery_json: Some(r#"{"kind":"notify","content":"hi"}"#),
                error_json: None,
                delivery_required: true,
            },
        )
        .await
        .expect_err("missing output run should fail");
        assert!(matches!(output_err, right_db::DbError::NotFound));

        let finish_err = finish_run(&conn, "missing", Some(0), "success")
            .await
            .expect_err("missing finished run should fail");
        assert!(matches!(finish_err, right_db::DbError::NotFound));
    }

    #[tokio::test]
    async fn insert_queued_background_run_then_mark_spawned_sets_pending_delivery() {
        let conn = setup().await;
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
        .await
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
            .await
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

        mark_background_spawned(&conn, "bg-1", "2026-05-18T10:01:00Z", "/log/bg-1.ndjson")
            .await
            .unwrap();

        let spawned: (String, String, String, i64, String) = conn
            .query_row(
                "SELECT status, handoff_state, log_path, delivery_required, delivery_status FROM async_runs WHERE id='bg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .await
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

    #[tokio::test]
    async fn insert_running_cron_run_persists_force_notify() {
        let conn = setup().await;
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-fn",
                job_name: "job",
                started_at: "2026-06-02T00:00:00Z",
                log_path: "/log",
                target_chat_id: Some(42),
                target_thread_id: None,
                force_notify: true,
            },
        )
        .await
        .unwrap();

        let force: i64 = conn
            .query_row(
                "SELECT force_notify FROM async_runs WHERE id = 'run-fn'",
                right_db::params![],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(force, 1);
    }

    #[tokio::test]
    async fn finish_run_sets_terminal_fields() {
        let conn = setup().await;
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: Some(-100),
                target_thread_id: None,
                force_notify: false,
            },
        )
        .await
        .unwrap();

        finish_run(&conn, "run-1", Some(0), "success")
            .await
            .unwrap();

        let row: (Option<String>, Option<i64>, String, String) = conn
            .query_row(
                "SELECT finished_at, exit_code, status, updated_at FROM async_runs WHERE id='run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row.1, Some(0));
        assert_eq!(row.2, "success");
        assert_eq!(row.0, Some(row.3));
    }
}
