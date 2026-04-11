use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

/// A cron job specification loaded from the database.
#[derive(Debug, Clone, PartialEq)]
pub struct CronSpec {
    pub schedule: String,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: f64,
    pub triggered_at: Option<String>,
}

/// Result of a cron spec create/update operation.
#[derive(Debug)]
pub struct CronSpecResult {
    pub message: String,
    pub warning: Option<String>,
}

/// Validate a cron job name: must match `^[a-z0-9][a-z0-9-]*$`.
pub fn validate_job_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("job name must not be empty".into());
    }
    let first = name.as_bytes()[0];
    if first == b'-' {
        return Err("job name must not start with a hyphen".into());
    }
    for ch in name.chars() {
        if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
            return Err(format!(
                "job name contains invalid character '{ch}': only lowercase alphanumeric and hyphens allowed"
            ));
        }
    }
    Ok(())
}

/// Validate a 5-field cron schedule expression.
///
/// Returns `Ok(Some(warning))` if the minute field is a round value (0 or 30),
/// `Ok(None)` if valid with no warning, or `Err` if the expression is invalid.
pub fn validate_schedule(schedule: &str) -> Result<Option<String>, String> {
    let trimmed = schedule.trim();
    if trimmed.is_empty() {
        return Err("schedule must not be empty".into());
    }

    // Convert 5-field to 7-field for the cron crate (seconds + year)
    let seven_field = format!("0 {} *", trimmed);
    cron::Schedule::from_str(&seven_field)
        .map_err(|e| format!("invalid cron schedule '{trimmed}': {e}"))?;

    // Check for round-minute warning
    let minute_field = trimmed.split_whitespace().next().unwrap_or("");
    let is_round = matches!(minute_field, "0" | "00" | "30");
    if is_round {
        Ok(Some(format!(
            "schedule runs at minute {minute_field} — consider offsetting to reduce thundering-herd"
        )))
    } else {
        Ok(None)
    }
}

/// Validate a lock TTL string (e.g. "30m", "1h").
pub fn validate_lock_ttl(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("lock_ttl must not be empty".into());
    }
    let (num_part, suffix) = s.split_at(s.len() - 1);
    if !matches!(suffix, "m" | "h") {
        return Err(format!(
            "lock_ttl must end with 'm' or 'h', got '{s}'"
        ));
    }
    num_part
        .parse::<i64>()
        .map_err(|_| format!("lock_ttl numeric part '{num_part}' is not a valid integer"))?;
    Ok(())
}

/// Validate all cron spec inputs. Returns schedule warning if any.
fn validate_spec_inputs(
    job_name: &str,
    schedule: &str,
    prompt: &str,
    lock_ttl: Option<&str>,
    max_budget_usd: Option<f64>,
) -> Result<Option<String>, String> {
    validate_job_name(job_name)?;
    let schedule_warning = validate_schedule(schedule)?;
    if prompt.trim().is_empty() {
        return Err("prompt must not be empty".into());
    }
    if let Some(ttl) = lock_ttl {
        validate_lock_ttl(ttl)?;
    }
    if let Some(budget) = max_budget_usd
        && budget <= 0.0
    {
        return Err("max_budget_usd must be greater than 0".into());
    }
    Ok(schedule_warning)
}

/// Insert a new cron spec into DB. Returns error message if job exists.
pub fn create_spec(
    conn: &rusqlite::Connection,
    job_name: &str,
    schedule: &str,
    prompt: &str,
    lock_ttl: Option<&str>,
    max_budget_usd: Option<f64>,
) -> Result<CronSpecResult, String> {
    let schedule_warning = validate_spec_inputs(job_name, schedule, prompt, lock_ttl, max_budget_usd)?;

    let now = chrono::Utc::now().to_rfc3339();
    let budget = max_budget_usd.unwrap_or(1.0);
    let result = conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![job_name, schedule, prompt, lock_ttl, budget, now, now],
    );

    match result {
        Ok(_) => Ok(CronSpecResult {
            message: format!("Created cron job '{job_name}'."),
            warning: schedule_warning,
        }),
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ffi::ErrorCode::ConstraintViolation =>
        {
            Err(format!("job '{job_name}' already exists"))
        }
        Err(e) => Err(format!("insert failed: {e:#}")),
    }
}

/// Update an existing cron spec. Returns error if job not found.
pub fn update_spec(
    conn: &rusqlite::Connection,
    job_name: &str,
    schedule: &str,
    prompt: &str,
    lock_ttl: Option<&str>,
    max_budget_usd: Option<f64>,
) -> Result<CronSpecResult, String> {
    let schedule_warning = validate_spec_inputs(job_name, schedule, prompt, lock_ttl, max_budget_usd)?;

    let now = chrono::Utc::now().to_rfc3339();
    let budget = max_budget_usd.unwrap_or(1.0);
    let rows = conn
        .execute(
            "UPDATE cron_specs SET schedule = ?2, prompt = ?3, lock_ttl = ?4, max_budget_usd = ?5, updated_at = ?6 \
             WHERE job_name = ?1",
            rusqlite::params![job_name, schedule, prompt, lock_ttl, budget, now],
        )
        .map_err(|e| format!("update failed: {e:#}"))?;

    if rows == 0 {
        return Err(format!("job '{job_name}' not found"));
    }

    Ok(CronSpecResult {
        message: format!("Updated cron job '{job_name}'."),
        warning: schedule_warning,
    })
}

/// Delete a cron spec and its lock file. Returns error if not found.
pub fn delete_spec(
    conn: &rusqlite::Connection,
    job_name: &str,
    agent_dir: &Path,
) -> Result<String, String> {
    let rows = conn
        .execute(
            "DELETE FROM cron_specs WHERE job_name = ?1",
            rusqlite::params![job_name],
        )
        .map_err(|e| format!("delete failed: {e:#}"))?;

    if rows == 0 {
        return Err(format!("job '{job_name}' not found"));
    }

    // Best-effort lock file removal.
    let lock_path = agent_dir
        .join("crons")
        .join(".locks")
        .join(format!("{job_name}.json"));
    if lock_path.exists() {
        let _ = std::fs::remove_file(&lock_path);
    }

    Ok(format!("Deleted cron job '{job_name}'."))
}

/// List all cron specs as a JSON string.
pub fn list_specs(conn: &rusqlite::Connection) -> Result<String, String> {
    let mut stmt = conn
        .prepare(
            "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at \
             FROM cron_specs ORDER BY job_name",
        )
        .map_err(|e| format!("prepare failed: {e:#}"))?;
    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "job_name": row.get::<_, String>(0)?,
                "schedule": row.get::<_, String>(1)?,
                "prompt": row.get::<_, String>(2)?,
                "lock_ttl": row.get::<_, Option<String>>(3)?,
                "max_budget_usd": row.get::<_, f64>(4)?,
                "created_at": row.get::<_, String>(5)?,
                "updated_at": row.get::<_, String>(6)?,
            }))
        })
        .map_err(|e| format!("query failed: {e:#}"))?
        .filter_map(|r| r.ok())
        .collect();
    serde_json::to_string_pretty(&rows)
        .map_err(|e| format!("serialization error: {e:#}"))
}

/// Format a [`CronSpecResult`] into a single message string with optional warning.
pub fn format_result(result: &CronSpecResult) -> String {
    let mut msg = result.message.clone();
    if let Some(ref w) = result.warning {
        msg.push_str(&format!(" Warning: {w}"));
    }
    msg
}

/// Load all cron specs from the `cron_specs` table.
///
/// Logs warnings for schedules that hit round minutes.
pub fn load_specs_from_db(
    conn: &rusqlite::Connection,
) -> Result<HashMap<String, CronSpec>, rusqlite::Error> {
    let mut specs = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at FROM cron_specs",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;

    for row in rows {
        let (job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at) = row?;

        if let Ok(Some(warning)) = validate_schedule(&schedule) {
            tracing::warn!(job = %job_name, "{warning}");
        }

        specs.insert(
            job_name,
            CronSpec {
                schedule,
                prompt,
                lock_ttl,
                max_budget_usd,
                triggered_at,
            },
        );
    }

    Ok(specs)
}

/// Convert a 5-field cron expression to a human-readable description.
/// Falls back to the raw expression if cron-descriptor can't parse it.
pub fn describe_schedule(schedule: &str) -> String {
    match cron_descriptor::cronparser::cron_expression_descriptor::get_description_cron(schedule) {
        Ok(desc) => desc,
        Err(_) => schedule.to_string(),
    }
}

/// Mark a cron spec for immediate execution on the next engine tick.
pub fn trigger_spec(conn: &rusqlite::Connection, job_name: &str) -> Result<String, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let rows = conn
        .execute(
            "UPDATE cron_specs SET triggered_at = ?2 WHERE job_name = ?1",
            rusqlite::params![job_name, now],
        )
        .map_err(|e| format!("trigger failed: {e:#}"))?;
    if rows == 0 {
        return Err(format!("job '{job_name}' not found"));
    }
    Ok(format!(
        "Triggered job '{job_name}'. Will execute on next engine tick (≤30s)."
    ))
}

/// Clear the `triggered_at` timestamp after a triggered run completes.
pub fn clear_triggered_at(conn: &rusqlite::Connection, job_name: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE cron_specs SET triggered_at = NULL WHERE job_name = ?1",
        rusqlite::params![job_name],
    )
    .map_err(|e| format!("clear trigger failed: {e:#}"))?;
    Ok(())
}

/// Full detail of a cron spec including timestamps.
#[derive(Debug)]
pub struct CronSpecDetail {
    pub job_name: String,
    pub schedule: String,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: f64,
    pub triggered_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Fetch full detail for a single cron spec by name.
pub fn get_spec_detail(
    conn: &rusqlite::Connection,
    job_name: &str,
) -> Result<Option<CronSpecDetail>, String> {
    let result = conn.query_row(
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, created_at, updated_at \
         FROM cron_specs WHERE job_name = ?1",
        rusqlite::params![job_name],
        |row| {
            Ok(CronSpecDetail {
                job_name: row.get(0)?,
                schedule: row.get(1)?,
                prompt: row.get(2)?,
                lock_ttl: row.get(3)?,
                max_budget_usd: row.get(4)?,
                triggered_at: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        },
    );
    match result {
        Ok(detail) => Ok(Some(detail)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(format!("query failed: {e:#}")),
    }
}

/// Summary of a single cron run for display.
#[derive(Debug)]
pub struct CronRunSummary {
    pub id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub status: String,
}

/// Fetch recent runs for a job, ordered by most recent first.
pub fn get_recent_runs(
    conn: &rusqlite::Connection,
    job_name: &str,
    limit: i64,
) -> Result<Vec<CronRunSummary>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, started_at, finished_at, exit_code, status \
             FROM cron_runs WHERE job_name = ?1 \
             ORDER BY started_at DESC LIMIT ?2",
        )
        .map_err(|e| format!("prepare failed: {e:#}"))?;
    let rows = stmt
        .query_map(rusqlite::params![job_name, limit], |row| {
            Ok(CronRunSummary {
                id: row.get(0)?,
                started_at: row.get(1)?,
                finished_at: row.get(2)?,
                exit_code: row.get(3)?,
                status: row.get(4)?,
            })
        })
        .map_err(|e| format!("query failed: {e:#}"))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

#[cfg(test)]
mod tests {
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
        let result =
            create_spec(&conn, "my-job", "*/5 * * * *", "do stuff", None, None).unwrap();
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
        let result =
            create_spec(&conn, "my-job", "0 9 * * *", "do stuff", None, None).unwrap();
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
        let result =
            update_spec(&conn, "upd", "17 9 * * *", "new prompt", Some("1h"), Some(2.0))
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
        let err =
            update_spec(&conn, "ghost", "*/5 * * * *", "prompt", None, None).unwrap_err();
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
        create_spec(&conn, "b-job", "17 9 * * *", "prompt b", Some("30m"), Some(2.5)).unwrap();
        let output = list_specs(&conn).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["job_name"], "a-job");
        assert_eq!(parsed[1]["job_name"], "b-job");
        assert_eq!(parsed[1]["max_budget_usd"], 2.5);
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
        assert_eq!(specs["job1"].schedule, "*/5 * * * *");
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
        create_spec(&conn, "detail-job", "*/5 * * * *", "do stuff", Some("1h"), Some(2.5))
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

    #[test]
    fn load_specs_includes_triggered_at() {
        let conn = setup_db();
        create_spec(&conn, "tr-load", "*/5 * * * *", "p", None, None).unwrap();
        trigger_spec(&conn, "tr-load").unwrap();
        let specs = load_specs_from_db(&conn).unwrap();
        assert!(specs["tr-load"].triggered_at.is_some());
    }
}
