use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};

/// Default budget cap per cron invocation (USD).
pub const DEFAULT_CRON_BUDGET_USD: f64 = 5.0;

/// Sentinel value stored in the `schedule` column for Immediate cron jobs.
pub(crate) const IMMEDIATE_SENTINEL: &str = "@immediate";

/// Single source of truth for cron timings shown to users and the agent.
///
/// Engine tick interval is intentionally omitted from this surface — it is an
/// implementation detail that feels real-time and should never appear in user-
/// facing copy or tool descriptions.
///
/// `IDLE_THRESHOLD_SECS` is user-meaningful: it answers "why didn't the cron
/// notification arrive yet?" — we hold pending notifications until the chat has
/// been idle for this long (within CC's 5-min prompt cache TTL).
pub const IDLE_THRESHOLD_SECS: i64 = 180;

/// Human-readable form for prose ("3 min" reads better than "180 s").
pub const IDLE_THRESHOLD_MIN: i64 = IDLE_THRESHOLD_SECS / 60;

/// Description string for the `cron_trigger` MCP tool. Built at compile time
/// from `IDLE_THRESHOLD_MIN` so the number cannot drift from the runtime.
pub const TRIGGER_TOOL_DESC: &str = const_format::formatcp!(
    "Trigger a cron job for immediate execution. Lock check applies — if the \
     job is currently running, the trigger is skipped. Delivery is conditional: \
     the cron itself decides whether to notify (sets `notify` in its structured \
     output), and any notification is held until the chat has been idle for {} \
     minutes. Use `cron_list_runs` to inspect `delivery_status` and \
     `no_notify_reason`.",
    IDLE_THRESHOLD_MIN,
);

/// How a cron job is scheduled.
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleKind {
    /// 5-field cron expression, fires repeatedly.
    Recurring(String),
    /// 5-field cron expression, fires once then auto-deletes.
    OneShotCron(String),
    /// Absolute UTC time, fires once then auto-deletes.
    RunAt(DateTime<Utc>),
    /// Fires on the next reconcile tick, then auto-deletes.
    /// Stored as the `'@immediate'` sentinel in the `schedule` column.
    Immediate,
}

impl ScheduleKind {
    /// Extract the cron schedule string, if this is a cron-based variant.
    pub fn cron_schedule(&self) -> Option<&str> {
        match self {
            Self::Recurring(s) | Self::OneShotCron(s) => Some(s),
            Self::RunAt(_) | Self::Immediate => None,
        }
    }

    /// Whether this is a one-shot job (fires once then deletes).
    pub fn is_one_shot(&self) -> bool {
        matches!(self, Self::OneShotCron(_) | Self::RunAt(_) | Self::Immediate)
    }
}

/// A cron job specification loaded from the database.
#[derive(Debug, Clone)]
pub struct CronSpec {
    pub schedule_kind: ScheduleKind,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: f64,
    pub triggered_at: Option<String>,
}

/// Compare only the spec fields that define the job configuration.
/// `triggered_at` is transient state (set/cleared by trigger flow) and must NOT
/// participate in equality — otherwise the reconciler aborts running jobs on every
/// trigger because the in-memory snapshot differs from the DB snapshot.
impl PartialEq for CronSpec {
    fn eq(&self, other: &Self) -> bool {
        self.schedule_kind == other.schedule_kind
            && self.prompt == other.prompt
            && self.lock_ttl == other.lock_ttl
            && self.max_budget_usd == other.max_budget_usd
    }
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
        return Err(format!("lock_ttl must end with 'm' or 'h', got '{s}'"));
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

/// Resolve schedule/recurring/run_at/immediate into DB column values.
///
/// Returns `(schedule_str, recurring_int, run_at_str, optional_warning)`.
/// Exactly one of `schedule`, `run_at`, `immediate=true` must be provided.
fn resolve_schedule_fields(
    schedule: Option<&str>,
    recurring: Option<bool>,
    run_at: Option<&str>,
    immediate: bool,
) -> Result<(String, i64, Option<String>, Option<String>), String> {
    let provided_count =
        (schedule.is_some() as u8) + (run_at.is_some() as u8) + (immediate as u8);
    if provided_count > 1 {
        return Err(
            "schedule, run_at, and immediate are mutually exclusive — provide exactly one".into(),
        );
    }
    if provided_count == 0 {
        return Err("one of schedule, run_at, or immediate must be provided".into());
    }

    if immediate {
        return Ok((IMMEDIATE_SENTINEL.to_string(), 0, None, None));
    }

    match (schedule, run_at) {
        (Some(sched), None) => {
            let warning = validate_schedule(sched)?;
            let rec = if recurring.unwrap_or(true) { 1 } else { 0 };
            Ok((sched.to_string(), rec, None, warning))
        }
        (None, Some(rat)) => {
            let dt = rat
                .parse::<DateTime<Utc>>()
                .map_err(|e| format!("invalid run_at datetime '{rat}': {e}"))?;
            let warning = if dt <= Utc::now() {
                Some("run_at is in the past — job will fire on next reconcile tick".into())
            } else {
                None
            };
            Ok(("".to_string(), 0, Some(rat.to_string()), warning))
        }
        _ => unreachable!("provided_count guarantees one of these arms"),
    }
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
    let schedule_warning =
        validate_spec_inputs(job_name, schedule, prompt, lock_ttl, max_budget_usd)?;

    let now = chrono::Utc::now().to_rfc3339();
    let budget = max_budget_usd.unwrap_or(DEFAULT_CRON_BUDGET_USD);
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

/// Create a cron spec with one-shot support.
///
/// Exactly one of `schedule`, `run_at`, or `immediate=true` must be provided.
/// `run_at` implies `recurring=false`. `immediate=true` fires on the next
/// reconcile tick then auto-deletes.
#[allow(clippy::too_many_arguments)]
pub fn create_spec_v2(
    conn: &rusqlite::Connection,
    job_name: &str,
    schedule: Option<&str>,
    prompt: &str,
    lock_ttl: Option<&str>,
    max_budget_usd: Option<f64>,
    recurring: Option<bool>,
    run_at: Option<&str>,
    target_chat_id: Option<i64>,
    target_thread_id: Option<i64>,
    immediate: bool,
) -> Result<CronSpecResult, String> {
    validate_job_name(job_name)?;
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

    let (db_schedule, db_recurring, db_run_at, schedule_warning) =
        resolve_schedule_fields(schedule, recurring, run_at, immediate)?;

    let now = chrono::Utc::now().to_rfc3339();
    let budget = max_budget_usd.unwrap_or(DEFAULT_CRON_BUDGET_USD);
    let result = conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![job_name, db_schedule, prompt, lock_ttl, budget, db_recurring, db_run_at, target_chat_id, target_thread_id, now, now],
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

/// Insert a one-shot Immediate cron job. Bot-internal use (background continuation).
///
/// Fires on the next reconcile tick, then auto-deletes. Validates the job name
/// and budget like `create_spec_v2`. `max_budget_usd = None` uses the project
/// default (`DEFAULT_CRON_BUDGET_USD`).
pub fn insert_immediate_cron(
    conn: &rusqlite::Connection,
    job_name: &str,
    prompt: &str,
    target_chat_id: i64,
    target_thread_id: Option<i64>,
    max_budget_usd: Option<f64>,
) -> Result<CronSpecResult, String> {
    create_spec_v2(
        conn,
        job_name,
        None,
        prompt,
        None,
        max_budget_usd,
        None,
        None,
        Some(target_chat_id),
        target_thread_id,
        true,
    )
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
    let schedule_warning =
        validate_spec_inputs(job_name, schedule, prompt, lock_ttl, max_budget_usd)?;

    let now = chrono::Utc::now().to_rfc3339();
    let budget = max_budget_usd.unwrap_or(DEFAULT_CRON_BUDGET_USD);
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

/// Update a cron spec partially — only provided fields are changed.
///
/// - `schedule` set → clears `run_at`, sets `recurring` (default true if not provided)
/// - `run_at` set → clears `schedule`, forces `recurring=false`
/// - Both set → error
/// - No fields → error
#[allow(clippy::too_many_arguments)]
pub fn update_spec_partial(
    conn: &rusqlite::Connection,
    job_name: &str,
    schedule: Option<&str>,
    run_at: Option<&str>,
    prompt: Option<&str>,
    recurring: Option<bool>,
    lock_ttl: Option<&str>,
    max_budget_usd: Option<f64>,
    target_chat_id: Option<i64>,
    target_thread_id: Option<Option<i64>>,
) -> Result<CronSpecResult, String> {
    validate_job_name(job_name)?;

    if schedule.is_none()
        && run_at.is_none()
        && prompt.is_none()
        && recurring.is_none()
        && lock_ttl.is_none()
        && max_budget_usd.is_none()
        && target_chat_id.is_none()
        && target_thread_id.is_none()
    {
        return Err("at least one field must be provided to update".into());
    }

    if schedule.is_some() && run_at.is_some() {
        return Err("schedule and run_at are mutually exclusive — provide one or the other".into());
    }

    let mut schedule_warning = None;
    if let Some(sched) = schedule {
        schedule_warning = validate_schedule(sched)?;
    }
    if let Some(rat) = run_at {
        rat.parse::<DateTime<Utc>>()
            .map_err(|e| format!("invalid run_at datetime '{rat}': {e}"))?;
    }
    if let Some(p) = prompt
        && p.trim().is_empty()
    {
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

    // Build dynamic UPDATE
    let mut sets = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(sched) = schedule {
        sets.push("schedule = ?");
        params.push(Box::new(sched.to_string()));
        sets.push("run_at = NULL");
        let rec = if recurring.unwrap_or(true) {
            1i64
        } else {
            0i64
        };
        sets.push("recurring = ?");
        params.push(Box::new(rec));
    } else if let Some(rat) = run_at {
        sets.push("run_at = ?");
        params.push(Box::new(rat.to_string()));
        sets.push("schedule = ''");
        sets.push("recurring = 0");
    } else if let Some(rec) = recurring {
        sets.push("recurring = ?");
        params.push(Box::new(if rec { 1i64 } else { 0i64 }));
    }

    if let Some(p) = prompt {
        sets.push("prompt = ?");
        params.push(Box::new(p.to_string()));
    }
    if let Some(ttl) = lock_ttl {
        sets.push("lock_ttl = ?");
        params.push(Box::new(ttl.to_string()));
    }
    if let Some(budget) = max_budget_usd {
        sets.push("max_budget_usd = ?");
        params.push(Box::new(budget));
    }
    if let Some(chat) = target_chat_id {
        sets.push("target_chat_id = ?");
        params.push(Box::new(chat));
    }
    if let Some(thread_opt) = target_thread_id {
        match thread_opt {
            Some(t) => {
                sets.push("target_thread_id = ?");
                params.push(Box::new(t));
            }
            None => {
                sets.push("target_thread_id = NULL");
            }
        }
    }

    sets.push("updated_at = ?");
    params.push(Box::new(chrono::Utc::now().to_rfc3339()));

    params.push(Box::new(job_name.to_string()));

    let sql = format!(
        "UPDATE cron_specs SET {} WHERE job_name = ?",
        sets.join(", ")
    );

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = conn
        .execute(&sql, param_refs.as_slice())
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

    // Remove lock file if present.
    let lock_path = agent_dir
        .join("crons")
        .join(".locks")
        .join(format!("{job_name}.json"));
    if lock_path.exists()
        && let Err(e) = std::fs::remove_file(&lock_path)
    {
        tracing::warn!(job = %job_name, "failed to remove lock file: {e:#}");
    }

    Ok(format!("Deleted cron job '{job_name}'."))
}

/// List all cron specs as a JSON string.
pub fn list_specs(conn: &rusqlite::Connection) -> Result<String, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.job_name, s.schedule, s.prompt, s.lock_ttl, s.max_budget_usd, \
                    s.created_at, s.updated_at, s.recurring, s.run_at, \
                    s.target_chat_id, s.target_thread_id, \
                    r.started_at, r.status \
             FROM cron_specs s \
             LEFT JOIN ( \
                 SELECT job_name, started_at, status, \
                        ROW_NUMBER() OVER (PARTITION BY job_name ORDER BY started_at DESC) AS rn \
                 FROM cron_runs \
             ) r ON r.job_name = s.job_name AND r.rn = 1 \
             ORDER BY s.job_name",
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
                "recurring": row.get::<_, i64>(7)? != 0,
                "run_at": row.get::<_, Option<String>>(8)?,
                "target_chat_id": row.get::<_, Option<i64>>(9)?,
                "target_thread_id": row.get::<_, Option<i64>>(10)?,
                "last_run_at": row.get::<_, Option<String>>(11)?,
                "last_status": row.get::<_, Option<String>>(12)?,
            }))
        })
        .map_err(|e| format!("query failed: {e:#}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("row read failed: {e:#}"))?;
    serde_json::to_string_pretty(&rows).map_err(|e| format!("serialization error: {e:#}"))
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
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, recurring, run_at FROM cron_specs",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;

    for row in rows {
        let (job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, recurring, run_at) =
            row?;

        let schedule_kind = if let Some(ref rat) = run_at {
            match rat.parse::<DateTime<Utc>>() {
                Ok(dt) => ScheduleKind::RunAt(dt),
                Err(e) => {
                    tracing::error!(job = %job_name, "invalid run_at in DB: {e:#}");
                    continue;
                }
            }
        } else if schedule == IMMEDIATE_SENTINEL {
            ScheduleKind::Immediate
        } else if recurring == 0 {
            ScheduleKind::OneShotCron(schedule)
        } else {
            ScheduleKind::Recurring(schedule)
        };

        specs.insert(
            job_name,
            CronSpec {
                schedule_kind,
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
    Ok(format!("Triggered job '{job_name}'."))
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
    pub recurring: bool,
    pub run_at: Option<String>,
}

/// Fetch full detail for a single cron spec by name.
pub fn get_spec_detail(
    conn: &rusqlite::Connection,
    job_name: &str,
) -> Result<Option<CronSpecDetail>, String> {
    let result = conn.query_row(
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, created_at, updated_at, recurring, run_at \
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
                recurring: row.get::<_, i64>(8)? != 0,
                run_at: row.get(9)?,
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
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("row read failed: {e:#}"))?;
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
        assert_ne!(base, changed_schedule);
        assert_ne!(base, changed_prompt);
        assert_ne!(base, changed_budget);
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
}
