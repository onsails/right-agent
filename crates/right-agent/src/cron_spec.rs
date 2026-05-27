use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use right_db::params::ParamsBuilder;
use right_db::{Connection, DbError, OptionalExtension, params};
use right_platform_knobs::IDLE_THRESHOLD_MIN;

/// Default budget cap per cron invocation (USD).
pub const DEFAULT_CRON_BUDGET_USD: f64 = 5.0;

/// Default `lock_ttl` for `Immediate` cron jobs when the caller does not
/// supply one.
///
/// Immediate jobs run on the next cron reconcile tick. They can legitimately
/// last for hours.
///
/// The lock heartbeat is written ONCE at job start and never refreshed, so
/// `is_lock_fresh` is the only guard against the reconciler spawning a
/// duplicate `execute_job` against a still-running spec on the next 5-second
/// tick. The previous reader-side default of `"30m"` was tighter than the
/// realistic upper bound for a long immediate turn, which let duplicates
/// sneak through. `"6h"` is generous enough to cover any plausible single-turn
/// execution while still bounding runaway specs.
///
/// This is the duplicate-prevention guard, NOT a wall-clock execution
/// budget — that is `max_budget_usd` plus `--max-turns` plus the process
/// timeout in the worker.
pub const IMMEDIATE_DEFAULT_LOCK_TTL: &str = "6h";

/// Sentinel value stored in the `schedule` column for Immediate cron jobs.
pub const IMMEDIATE_SENTINEL: &str = "@immediate";

/// Legacy sentinel prefix for removed cron-backed background continuations.
pub(crate) const BG_SENTINEL_PREFIX: &str = "@bg:";

/// Description string for the `cron_trigger` MCP tool. Built at compile time
/// from `IDLE_THRESHOLD_MIN` so the number cannot drift from the runtime.
pub const TRIGGER_TOOL_DESC: &str = const_format::formatcp!(
    "Trigger a cron job for immediate execution. Lock check applies — if the \
     job is currently running, the trigger is skipped. Delivery is conditional: \
     the cron itself decides whether to notify (sets `delivery` in its structured \
     output), and any notification is held until the chat has been idle for {} \
     minutes. Use `cron_list_runs` to inspect `delivery_status` and \
     `delivery`.",
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
        matches!(
            self,
            Self::OneShotCron(_) | Self::RunAt(_) | Self::Immediate
        )
    }

    /// Parse a (`schedule`, `run_at`, `recurring`) row tuple from `cron_specs`
    /// into the typed kind. Single source of truth — `load_specs_from_db`
    /// calls this for every row. Returns `Err(message)` on malformed input.
    pub fn from_db_row(
        schedule: &str,
        run_at: Option<&str>,
        recurring: i64,
    ) -> Result<Self, String> {
        if let Some(rat) = run_at {
            let dt = rat
                .parse::<DateTime<Utc>>()
                .map_err(|e| format!("invalid run_at datetime '{rat}': {e}"))?;
            return Ok(Self::RunAt(dt));
        }
        if let Some(rest) = schedule.strip_prefix(BG_SENTINEL_PREFIX) {
            return Err(format!(
                "legacy background continuation schedule '{BG_SENTINEL_PREFIX}{rest}' is no longer schedulable"
            ));
        }
        if schedule == IMMEDIATE_SENTINEL {
            return Ok(Self::Immediate);
        }
        if recurring == 0 {
            Ok(Self::OneShotCron(schedule.to_string()))
        } else {
            Ok(Self::Recurring(schedule.to_string()))
        }
    }
}

impl std::fmt::Display for ScheduleKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Recurring(s) | Self::OneShotCron(s) => f.write_str(s),
            Self::RunAt(dt) => write!(f, "{}", dt.to_rfc3339()),
            Self::Immediate => f.write_str(IMMEDIATE_SENTINEL),
        }
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
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
}

/// Compare only the spec fields that define the job configuration.
/// `triggered_at` is transient state (set/cleared by trigger flow) and must NOT
/// participate in equality — otherwise the reconciler aborts running jobs on every
/// trigger because the in-memory snapshot differs from the DB snapshot.
/// `target_chat_id` and `target_thread_id` ARE config: changing them via
/// `cron_update` is a real change the reconciler must react to.
impl PartialEq for CronSpec {
    fn eq(&self, other: &Self) -> bool {
        self.schedule_kind == other.schedule_kind
            && self.prompt == other.prompt
            && self.lock_ttl == other.lock_ttl
            && self.max_budget_usd == other.max_budget_usd
            && self.target_chat_id == other.target_chat_id
            && self.target_thread_id == other.target_thread_id
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
/// Returns `Ok(Some(warning))` if the schedule fires at minute :00 or :30 in
/// any hour (peak minutes that cluster with other automated jobs and spike
/// API rate limits), `Ok(None)` if valid with no warning, or `Err` if the
/// expression is invalid.
pub fn validate_schedule(schedule: &str) -> Result<Option<String>, String> {
    let trimmed = schedule.trim();
    if trimmed.is_empty() {
        return Err("schedule must not be empty".into());
    }

    // Convert 5-field to 7-field for the cron crate (seconds + year)
    let seven_field = format!("0 {} *", trimmed);
    let parsed = cron::Schedule::from_str(&seven_field)
        .map_err(|e| format!("invalid cron schedule '{trimmed}': {e}"))?;

    // Peak-minute detection: walk the first N upcoming fires from a
    // deterministic anchor and check whether any lands on minute :00 or :30.
    // Catches `*/30`, `*/15`, `*/10`, `*/5`, `0,30 * * * *`, `0 9 * * *`,
    // literal `0`/`30` etc. — not just the literal minute field. N=60 is
    // enough: for sub-hourly schedules every distinct minute slot is observed
    // within the first 60 fires; for less frequent schedules (daily, monthly)
    // the minute value is fixed per spec so the first fire suffices.
    let anchor = chrono::TimeZone::with_ymd_and_hms(&Utc, 2030, 1, 1, 0, 0, 0)
        .single()
        .expect("anchor is a valid UTC datetime");
    let mut peak_hits: Vec<u32> = Vec::new();
    for dt in parsed.after(&anchor).take(60) {
        let m = chrono::Timelike::minute(&dt);
        if (m == 0 || m == 30) && !peak_hits.contains(&m) {
            peak_hits.push(m);
            if peak_hits.len() == 2 {
                break;
            }
        }
    }
    if peak_hits.is_empty() {
        Ok(None)
    } else {
        peak_hits.sort_unstable();
        let mins = peak_hits
            .iter()
            .map(|m| format!(":{m:02}"))
            .collect::<Vec<_>>()
            .join(" and ");
        Ok(Some(format!(
            "schedule fires at peak minute {mins}. Many automated jobs cluster on round minutes and spike API rate limits. \
             If the user did not explicitly ask for a round minute, update via mcp__right__cron_update to an offset like :17 or :43."
        )))
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
    match (schedule, run_at, immediate) {
        (Some(sched), None, false) => {
            let warning = validate_schedule(sched)?;
            let rec = if recurring.unwrap_or(true) { 1 } else { 0 };
            Ok((sched.to_string(), rec, None, warning))
        }
        (None, Some(rat), false) => {
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
        (None, None, true) => Ok((IMMEDIATE_SENTINEL.to_string(), 0, None, None)),
        (None, None, false) => Err("one of schedule, run_at, or immediate must be provided".into()),
        _ => Err(
            "schedule, run_at, and immediate are mutually exclusive — provide exactly one".into(),
        ),
    }
}

/// Insert a new cron spec into DB. Returns error message if job exists.
pub async fn create_spec(
    conn: &Connection,
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
        params![job_name, schedule, prompt, lock_ttl, budget, &now, &now],
    )
    .await;

    match result {
        Ok(_) => Ok(CronSpecResult {
            message: format!("Created cron job '{job_name}'."),
            warning: schedule_warning,
        }),
        Err(e) if e.is_constraint_violation() => Err(format!("job '{job_name}' already exists")),
        Err(e) => Err(format!("insert failed: {e:#}")),
    }
}

/// Create a cron spec with one-shot support.
///
/// Exactly one of `schedule`, `run_at`, or `immediate=true` must be provided.
/// `run_at` implies `recurring=false`. `immediate=true` fires on the next
/// reconcile tick then auto-deletes.
#[allow(clippy::too_many_arguments)]
pub async fn create_spec_v2(
    conn: &Connection,
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
        params![job_name, db_schedule, prompt, lock_ttl, budget, db_recurring, db_run_at, target_chat_id, target_thread_id, &now, &now],
    )
    .await;

    match result {
        Ok(_) => Ok(CronSpecResult {
            message: format!("Created cron job '{job_name}'."),
            warning: schedule_warning,
        }),
        Err(e) if e.is_constraint_violation() => Err(format!("job '{job_name}' already exists")),
        Err(e) => Err(format!("insert failed: {e:#}")),
    }
}

/// Update an existing cron spec. Returns error if job not found.
pub async fn update_spec(
    conn: &Connection,
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
            params![job_name, schedule, prompt, lock_ttl, budget, now],
        )
        .await
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
pub async fn update_spec_partial(
    conn: &Connection,
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
    let mut values = ParamsBuilder::new();

    if let Some(sched) = schedule {
        sets.push("schedule = ?");
        values
            .push(sched)
            .map_err(|e| format!("invalid parameter: {e:#}"))?;
        sets.push("run_at = NULL");
        let rec = if recurring.unwrap_or(true) {
            1i64
        } else {
            0i64
        };
        sets.push("recurring = ?");
        values
            .push(rec)
            .map_err(|e| format!("invalid parameter: {e:#}"))?;
    } else if let Some(rat) = run_at {
        sets.push("run_at = ?");
        values
            .push(rat)
            .map_err(|e| format!("invalid parameter: {e:#}"))?;
        sets.push("schedule = ''");
        sets.push("recurring = 0");
    } else if let Some(rec) = recurring {
        sets.push("recurring = ?");
        values
            .push(if rec { 1i64 } else { 0i64 })
            .map_err(|e| format!("invalid parameter: {e:#}"))?;
    }

    if let Some(p) = prompt {
        sets.push("prompt = ?");
        values
            .push(p)
            .map_err(|e| format!("invalid parameter: {e:#}"))?;
    }
    if let Some(ttl) = lock_ttl {
        sets.push("lock_ttl = ?");
        values
            .push(ttl)
            .map_err(|e| format!("invalid parameter: {e:#}"))?;
    }
    if let Some(budget) = max_budget_usd {
        sets.push("max_budget_usd = ?");
        values
            .push(budget)
            .map_err(|e| format!("invalid parameter: {e:#}"))?;
    }
    if let Some(chat) = target_chat_id {
        sets.push("target_chat_id = ?");
        values
            .push(chat)
            .map_err(|e| format!("invalid parameter: {e:#}"))?;
    }
    if let Some(thread_opt) = target_thread_id {
        match thread_opt {
            Some(t) => {
                sets.push("target_thread_id = ?");
                values
                    .push(t)
                    .map_err(|e| format!("invalid parameter: {e:#}"))?;
            }
            None => {
                sets.push("target_thread_id = NULL");
            }
        }
    }

    sets.push("updated_at = ?");
    values
        .push(chrono::Utc::now().to_rfc3339())
        .map_err(|e| format!("invalid parameter: {e:#}"))?;

    values
        .push(job_name)
        .map_err(|e| format!("invalid parameter: {e:#}"))?;

    let sql = format!(
        "UPDATE cron_specs SET {} WHERE job_name = ?",
        sets.join(", ")
    );

    // Determine if the caller is changing target columns. If so, propagate
    // the new values to pending/retryable async cron deliveries so queued
    // notifications route to the new chat/thread on the next delivery tick.
    let target_changed = target_chat_id.is_some() || target_thread_id.is_some();

    let tx = conn
        .transaction()
        .await
        .map_err(|e| format!("begin transaction failed: {e:#}"))?;

    let rows = tx
        .execute(&sql, values)
        .await
        .map_err(|e| format!("update failed: {e:#}"))?;

    if rows == 0 {
        return Err(format!("job '{job_name}' not found"));
    }

    if target_changed {
        // Read back the spec's authoritative target after the UPDATE; this
        // handles the case where the caller changed only one of the two
        // target columns (the unspecified column must keep its prior value
        // on undelivered runs, not be clobbered).
        let (new_chat, new_thread): (Option<i64>, Option<i64>) = tx
            .query_row(
                "SELECT target_chat_id, target_thread_id FROM cron_specs WHERE job_name = ?1",
                params![job_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .await
            .map_err(|e| format!("read-back failed: {e:#}"))?;
        tx.execute(
            "UPDATE async_runs SET target_chat_id = ?1, target_thread_id = ?2
             WHERE kind = 'cron'
               AND producer_ref = ?3
               AND delivery_status IN ('pending', 'retryable')",
            params![new_chat, new_thread, job_name],
        )
        .await
        .map_err(|e| format!("async cron run target propagation failed: {e:#}"))?;
    }

    tx.commit()
        .await
        .map_err(|e| format!("commit transaction failed: {e:#}"))?;

    Ok(CronSpecResult {
        message: format!("Updated cron job '{job_name}'."),
        warning: schedule_warning,
    })
}

/// Delete a cron spec and its lock file. Returns error if not found.
pub async fn delete_spec(
    conn: &Connection,
    job_name: &str,
    agent_dir: &Path,
) -> Result<String, String> {
    let rows = conn
        .execute(
            "DELETE FROM cron_specs WHERE job_name = ?1",
            params![job_name],
        )
        .await
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
pub async fn list_specs(conn: &Connection) -> Result<String, String> {
    let rows: Vec<serde_json::Value> = conn
        .query_all(
            "SELECT s.job_name, s.schedule, s.prompt, s.lock_ttl, s.max_budget_usd, \
                    s.created_at, s.updated_at, s.recurring, s.run_at, \
                    s.target_chat_id, s.target_thread_id, \
                    r.id, r.started_at, r.status \
             FROM cron_specs s \
             LEFT JOIN ( \
                 SELECT producer_ref, id, started_at, status, \
                        ROW_NUMBER() OVER (PARTITION BY producer_ref ORDER BY started_at DESC) AS rn \
                 FROM async_runs \
                 WHERE kind = 'cron' \
             ) r ON r.producer_ref = s.job_name AND r.rn = 1 \
             ORDER BY s.job_name",
            (),
            |row| {
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
                "last_run_id": row.get::<_, Option<String>>(11)?,
                "last_run_at": row.get::<_, Option<String>>(12)?,
                "last_status": row.get::<_, Option<String>>(13)?,
            }))
        })
        .await
        .map_err(|e| format!("query failed: {e:#}"))?;
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
pub async fn load_specs_from_db(conn: &Connection) -> Result<HashMap<String, CronSpec>, DbError> {
    let mut specs = HashMap::new();
    let rows = conn.query_all(
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, recurring, run_at, target_chat_id, target_thread_id FROM cron_specs",
        (),
        |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<i64>>(8)?,
            row.get::<_, Option<i64>>(9)?,
        ))
    })
    .await?;

    for row in rows {
        let (
            job_name,
            schedule,
            prompt,
            lock_ttl,
            max_budget_usd,
            triggered_at,
            recurring,
            run_at,
            target_chat_id,
            target_thread_id,
        ) = row;

        let schedule_kind = match ScheduleKind::from_db_row(&schedule, run_at.as_deref(), recurring)
        {
            Ok(k) => k,
            Err(e) => {
                if schedule.starts_with(BG_SENTINEL_PREFIX) {
                    tracing::warn!(job = %job_name, "skipping legacy background schedule: {e}");
                } else {
                    tracing::error!(job = %job_name, schedule = %schedule, "skipping unparseable cron schedule: {e}");
                }
                continue;
            }
        };

        specs.insert(
            job_name,
            CronSpec {
                schedule_kind,
                prompt,
                lock_ttl,
                max_budget_usd,
                triggered_at,
                target_chat_id,
                target_thread_id,
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
pub async fn trigger_spec(conn: &Connection, job_name: &str) -> Result<String, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let rows = conn
        .execute(
            "UPDATE cron_specs SET triggered_at = ?2 WHERE job_name = ?1",
            params![job_name, now],
        )
        .await
        .map_err(|e| format!("trigger failed: {e:#}"))?;
    if rows == 0 {
        return Err(format!("job '{job_name}' not found"));
    }
    Ok(format!("Triggered job '{job_name}'."))
}

/// Clear the `triggered_at` timestamp after a triggered run completes.
pub async fn clear_triggered_at(conn: &Connection, job_name: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE cron_specs SET triggered_at = NULL WHERE job_name = ?1",
        params![job_name],
    )
    .await
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
pub async fn get_spec_detail(
    conn: &Connection,
    job_name: &str,
) -> Result<Option<CronSpecDetail>, String> {
    conn.query_row(
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, created_at, updated_at, recurring, run_at \
         FROM cron_specs WHERE job_name = ?1",
        params![job_name],
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
    )
    .await
    .optional()
    .map_err(|e| format!("query failed: {e:#}"))
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
pub async fn get_recent_runs(
    conn: &Connection,
    job_name: &str,
    limit: i64,
) -> Result<Vec<CronRunSummary>, String> {
    let rows = conn
        .query_all(
            "SELECT id, started_at, finished_at, exit_code, status \
             FROM async_runs \
             WHERE kind = 'cron' AND producer_ref = ?1 \
             ORDER BY started_at DESC LIMIT ?2",
            params![job_name, limit],
            |row| {
                Ok(CronRunSummary {
                    id: row.get(0)?,
                    started_at: row.get(1)?,
                    finished_at: row.get(2)?,
                    exit_code: row.get(3)?,
                    status: row.get(4)?,
                })
            },
        )
        .await
        .map_err(|e| format!("query failed: {e:#}"))?;
    Ok(rows)
}

#[cfg(test)]
#[path = "cron_spec_tests.rs"]
mod tests;
