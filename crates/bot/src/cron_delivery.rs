use rusqlite::OptionalExtension as _;

/// A pending cron result ready for delivery.
#[derive(Debug)]
pub struct PendingCronResult {
    pub id: String,
    pub job_name: String,
    pub notify_json: String,
    pub summary: String,
    pub finished_at: String,
}

/// Query the oldest undelivered cron result with a non-null notify_json.
pub fn fetch_pending(
    conn: &rusqlite::Connection,
) -> Result<Option<PendingCronResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, job_name, notify_json, summary, finished_at FROM cron_runs \
         WHERE status = 'success' AND notify_json IS NOT NULL AND delivered_at IS NULL \
         ORDER BY finished_at ASC LIMIT 1",
    )?;
    let result = stmt.query_row([], |row| {
        Ok(PendingCronResult {
            id: row.get(0)?,
            job_name: row.get(1)?,
            notify_json: row.get(2)?,
            summary: row.get(3)?,
            finished_at: row.get(4)?,
        })
    });
    match result {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Fetch a specific cron result by ID.
pub fn fetch_by_id(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<PendingCronResult>, rusqlite::Error> {
    let result = conn.query_row(
        "SELECT id, job_name, notify_json, summary, finished_at FROM cron_runs WHERE id = ?1",
        rusqlite::params![id],
        |row| {
            Ok(PendingCronResult {
                id: row.get(0)?,
                job_name: row.get(1)?,
                notify_json: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                summary: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                finished_at: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            })
        },
    );
    match result {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Mark a cron run as delivered.
pub fn mark_delivered(
    conn: &rusqlite::Connection,
    run_id: &str,
) -> Result<(), rusqlite::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE cron_runs SET delivered_at = ?1 WHERE id = ?2",
        rusqlite::params![now, run_id],
    )?;
    Ok(())
}

/// Deduplicate: for a given job, find the latest undelivered result and mark all
/// older undelivered results as delivered. Returns (latest_id, skipped_count).
pub fn deduplicate_job(
    conn: &rusqlite::Connection,
    job_name: &str,
) -> Result<Option<(String, u32)>, rusqlite::Error> {
    let latest_id: Option<String> = conn
        .query_row(
            "SELECT id FROM cron_runs \
             WHERE job_name = ?1 AND status = 'success' AND notify_json IS NOT NULL AND delivered_at IS NULL \
             ORDER BY finished_at DESC LIMIT 1",
            rusqlite::params![job_name],
            |row| row.get(0),
        )
        .optional()?;

    let Some(latest_id) = latest_id else {
        return Ok(None);
    };

    let now = chrono::Utc::now().to_rfc3339();
    let count = conn.execute(
        "UPDATE cron_runs SET delivered_at = ?1 \
         WHERE job_name = ?2 AND id != ?3 \
         AND status = 'success' AND notify_json IS NOT NULL AND delivered_at IS NULL",
        rusqlite::params![now, job_name, latest_id],
    )?;

    Ok(Some((latest_id, count as u32)))
}

/// Format a pending cron result as YAML for the main CC session.
pub fn format_cron_yaml(pending: &PendingCronResult, skipped: u32) -> String {
    let total = skipped + 1;
    let mut yaml = String::new();
    yaml.push_str("cron_result:\n");
    yaml.push_str(&format!("  job: {}\n", pending.job_name));
    yaml.push_str(&format!("  runs_total: {total}\n"));
    if skipped > 0 {
        yaml.push_str(&format!("  skipped_runs: {skipped}\n"));
    }

    if let Ok(notify) = serde_json::from_str::<serde_json::Value>(&pending.notify_json) {
        yaml.push_str("  result:\n");
        yaml.push_str("    notify:\n");
        if let Some(content) = notify.get("content").and_then(|v| v.as_str()) {
            yaml.push_str(&format!(
                "      content: \"{}\"\n",
                content.replace('"', "\\\"")
            ));
        }
        if let Some(atts) = notify.get("attachments").and_then(|v| v.as_array()) {
            if !atts.is_empty() {
                yaml.push_str("      attachments:\n");
                for att in atts {
                    let att_type = att
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("document");
                    let path = att.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    yaml.push_str(&format!("        - type: {att_type}\n"));
                    yaml.push_str(&format!("          path: {path}\n"));
                    if let Some(caption) = att.get("caption").and_then(|v| v.as_str()) {
                        yaml.push_str(&format!(
                            "          caption: \"{}\"\n",
                            caption.replace('"', "\\\"")
                        ));
                    }
                }
            }
        }
        yaml.push_str(&format!(
            "    summary: \"{}\"\n",
            pending.summary.replace('"', "\\\"")
        ));
    }

    yaml
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = rightclaw::memory::open_connection(dir.path()).unwrap();
        (dir, conn)
    }

    #[test]
    fn fetch_pending_empty_db() {
        let (_dir, conn) = setup_db();
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn fetch_pending_returns_oldest() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum1', '{\"content\":\"first\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job1', '2026-01-01T00:05:00Z', '2026-01-01T00:06:00Z', 'success', '/log', 'sum2', '{\"content\":\"second\"}')",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.id, "a", "should return oldest first");
    }

    #[test]
    fn fetch_pending_skips_null_notify() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'silent')",
            [],
        )
        .unwrap();
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn fetch_pending_skips_delivered() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json, delivered_at) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"done\"}', '2026-01-01T00:10:00Z')",
            [],
        ).unwrap();
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn deduplicate_keeps_latest_marks_older() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum1', '{\"content\":\"old\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job1', '2026-01-01T00:05:00Z', '2026-01-01T00:06:00Z', 'success', '/log', 'sum2', '{\"content\":\"new\"}')",
            [],
        ).unwrap();
        let (latest_id, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest_id, "b");
        assert_eq!(skipped, 1);
        let delivered: Option<String> = conn
            .query_row(
                "SELECT delivered_at FROM cron_runs WHERE id = 'a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(delivered.is_some());
        let not_delivered: Option<String> = conn
            .query_row(
                "SELECT delivered_at FROM cron_runs WHERE id = 'b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(not_delivered.is_none());
    }

    #[test]
    fn deduplicate_does_not_touch_other_jobs() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job2', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"y\"}')",
            [],
        ).unwrap();
        let (latest_id, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest_id, "a");
        assert_eq!(skipped, 0);
    }

    #[test]
    fn format_cron_yaml_basic() {
        let pending = PendingCronResult {
            id: "abc".into(),
            job_name: "health-check".into(),
            notify_json: r#"{"content":"BTC up 2%"}"#.into(),
            summary: "Checked 5 pairs".into(),
            finished_at: "2026-01-01T00:01:00Z".into(),
        };
        let yaml = format_cron_yaml(&pending, 2);
        assert!(yaml.contains("job: health-check"));
        assert!(yaml.contains("runs_total: 3"));
        assert!(yaml.contains("skipped_runs: 2"));
        assert!(yaml.contains("BTC up 2%"));
        assert!(yaml.contains("Checked 5 pairs"));
    }

    #[test]
    fn format_cron_yaml_no_skipped() {
        let pending = PendingCronResult {
            id: "abc".into(),
            job_name: "job1".into(),
            notify_json: r#"{"content":"hello"}"#.into(),
            summary: "done".into(),
            finished_at: "2026-01-01T00:01:00Z".into(),
        };
        let yaml = format_cron_yaml(&pending, 0);
        assert!(yaml.contains("runs_total: 1"));
        assert!(!yaml.contains("skipped_runs"));
    }
}
