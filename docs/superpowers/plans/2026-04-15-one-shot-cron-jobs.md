# One-Shot Cron Jobs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add one-shot job scheduling (absolute `run_at` time and `recurring: false` cron) alongside existing recurring cron jobs.

**Architecture:** Extend `cron_specs` table with `recurring` and `run_at` columns. `ScheduleKind` enum in Rust maps the three variants. Reconcile loop fires `run_at` specs on each tick, deletes after execution. `cron_update` becomes partial (only update fields that are passed).

**Tech Stack:** rusqlite, chrono, serde/schemars, tokio

---

### Task 1: Migration — add `recurring` and `run_at` columns

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v13_one_shot_cron.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`

- [ ] **Step 1: Write the migration test**

Add to `crates/rightclaw/src/memory/migrations.rs` in the `tests` module:

```rust
#[test]
fn v13_one_shot_cron_columns() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('cron_specs')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(cols.contains(&"recurring".to_string()), "recurring column missing");
    assert!(cols.contains(&"run_at".to_string()), "run_at column missing");
}

#[test]
fn v13_idempotent_when_columns_already_exist() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 12).unwrap();
    conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN recurring INTEGER NOT NULL DEFAULT 1").unwrap();
    conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN run_at TEXT").unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('cron_specs')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(cols.contains(&"recurring".to_string()));
    assert!(cols.contains(&"run_at".to_string()));
}

#[test]
fn v13_existing_specs_get_recurring_true() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 12).unwrap();
    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
         VALUES ('old-job', '*/5 * * * *', 'do stuff', 1.0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    ).unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let recurring: i64 = conn
        .query_row("SELECT recurring FROM cron_specs WHERE job_name = 'old-job'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(recurring, 1, "existing specs must default to recurring=1");
    let run_at: Option<String> = conn
        .query_row("SELECT run_at FROM cron_specs WHERE job_name = 'old-job'", [], |r| r.get(0))
        .unwrap();
    assert!(run_at.is_none(), "existing specs must have run_at=NULL");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw v13 -- --nocapture`
Expected: FAIL — migration v13 doesn't exist yet.

- [ ] **Step 3: Create the migration SQL file**

Create `crates/rightclaw/src/memory/sql/v13_one_shot_cron.sql` — this file won't be used directly since we need a Rust hook for idempotency, but we keep it for documentation consistency. The actual migration is the Rust hook below.

- [ ] **Step 4: Write the migration hook in migrations.rs**

Add to `crates/rightclaw/src/memory/migrations.rs`:

```rust
/// v13: Add recurring and run_at columns to cron_specs for one-shot job support.
///
/// Idempotent — checks pragma_table_info before each ALTER.
fn v13_one_shot_cron(tx: &Transaction) -> Result<(), HookError> {
    let has_column = |col: &str| -> Result<bool, rusqlite::Error> {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = ?1",
            [col],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    };

    if !has_column("recurring")? {
        tx.execute_batch("ALTER TABLE cron_specs ADD COLUMN recurring INTEGER NOT NULL DEFAULT 1")?;
    }
    if !has_column("run_at")? {
        tx.execute_batch("ALTER TABLE cron_specs ADD COLUMN run_at TEXT")?;
    }

    Ok(())
}
```

Add to the `MIGRATIONS` vector after `v12`:

```rust
M::up_with_hook("", v13_one_shot_cron),
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw v13 -- --nocapture`
Expected: All 3 v13 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v13_one_shot_cron.sql crates/rightclaw/src/memory/migrations.rs
git commit -m "feat(cron): add v13 migration — recurring and run_at columns"
```

---

### Task 2: ScheduleKind enum and CronSpec refactor

**Files:**
- Modify: `crates/rightclaw/src/cron_spec.rs`

- [ ] **Step 1: Write failing tests for ScheduleKind round-trip**

Add to the `tests` module in `crates/rightclaw/src/cron_spec.rs`:

```rust
#[test]
fn create_spec_with_run_at_succeeds() {
    let conn = setup_db();
    let result = create_spec_v2(
        &conn,
        "run-at-job",
        None,                                    // no schedule
        "do stuff at specific time",
        None,                                    // no lock_ttl
        None,                                    // default budget
        None,                                    // recurring ignored for run_at
        Some("2026-12-25T15:30:00Z"),            // run_at
    ).unwrap();
    assert!(result.message.contains("Created"));
}

#[test]
fn create_spec_with_both_schedule_and_run_at_fails() {
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
    ).unwrap_err();
    assert!(err.contains("mutually exclusive"));
}

#[test]
fn create_spec_with_neither_schedule_nor_run_at_fails() {
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
    ).unwrap_err();
    assert!(err.contains("one of"));
}

#[test]
fn create_spec_with_invalid_run_at_fails() {
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
    ).unwrap_err();
    assert!(err.contains("invalid"));
}

#[test]
fn create_spec_with_past_run_at_succeeds() {
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
    ).unwrap();
    assert!(result.message.contains("Created"));
}

#[test]
fn create_spec_recurring_false_stored_as_one_shot_cron() {
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
    ).unwrap();
    let specs = load_specs_from_db(&conn).unwrap();
    assert!(matches!(specs["oneshot-cron"].schedule_kind, ScheduleKind::OneShotCron(_)));
}

#[test]
fn load_specs_round_trips_all_schedule_kinds() {
    let conn = setup_db();
    // Recurring
    create_spec_v2(&conn, "recurring", Some("*/5 * * * *"), "p", None, None, None, None).unwrap();
    // OneShotCron
    create_spec_v2(&conn, "oneshot", Some("30 15 * * *"), "p", None, None, Some(false), None).unwrap();
    // RunAt
    create_spec_v2(&conn, "runat", None, "p", None, None, None, Some("2026-12-25T15:30:00Z")).unwrap();

    let specs = load_specs_from_db(&conn).unwrap();
    assert!(matches!(specs["recurring"].schedule_kind, ScheduleKind::Recurring(_)));
    assert!(matches!(specs["oneshot"].schedule_kind, ScheduleKind::OneShotCron(_)));
    assert!(matches!(specs["runat"].schedule_kind, ScheduleKind::RunAt(_)));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw create_spec_with_run_at -- --nocapture`
Expected: FAIL — `create_spec_v2` and `ScheduleKind` don't exist.

- [ ] **Step 3: Add ScheduleKind enum and update CronSpec**

In `crates/rightclaw/src/cron_spec.rs`, add the `ScheduleKind` enum and update `CronSpec`:

```rust
use chrono::{DateTime, Utc};

/// How a cron job is scheduled.
#[derive(Debug, Clone)]
pub enum ScheduleKind {
    /// 5-field cron expression, fires repeatedly.
    Recurring(String),
    /// 5-field cron expression, fires once then auto-deletes.
    OneShotCron(String),
    /// Absolute UTC time, fires once then auto-deletes.
    RunAt(DateTime<Utc>),
}

impl ScheduleKind {
    /// Extract the cron schedule string, if this is a cron-based variant.
    pub fn cron_schedule(&self) -> Option<&str> {
        match self {
            ScheduleKind::Recurring(s) | ScheduleKind::OneShotCron(s) => Some(s),
            ScheduleKind::RunAt(_) => None,
        }
    }

    /// Whether this is a one-shot job (fires once then deletes).
    pub fn is_one_shot(&self) -> bool {
        matches!(self, ScheduleKind::OneShotCron(_) | ScheduleKind::RunAt(_))
    }
}
```

Update `CronSpec` to use `ScheduleKind`:

```rust
pub struct CronSpec {
    pub schedule_kind: ScheduleKind,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: f64,
    pub triggered_at: Option<String>,
}
```

Update `PartialEq` for `CronSpec` — compare by schedule_kind fields:

```rust
impl PartialEq for CronSpec {
    fn eq(&self, other: &Self) -> bool {
        self.schedule_kind_eq(&other.schedule_kind)
            && self.prompt == other.prompt
            && self.lock_ttl == other.lock_ttl
            && self.max_budget_usd == other.max_budget_usd
    }
}

impl CronSpec {
    fn schedule_kind_eq(&self, other: &ScheduleKind) -> bool {
        match (&self.schedule_kind, other) {
            (ScheduleKind::Recurring(a), ScheduleKind::Recurring(b)) => a == b,
            (ScheduleKind::OneShotCron(a), ScheduleKind::OneShotCron(b)) => a == b,
            (ScheduleKind::RunAt(a), ScheduleKind::RunAt(b)) => a == b,
            _ => false,
        }
    }
}
```

- [ ] **Step 4: Implement `create_spec_v2`**

Add a new function `create_spec_v2` that accepts optional `schedule`, `recurring`, and `run_at`. Keep `create_spec` intact temporarily for backward compat (callers will be migrated in Task 4).

```rust
/// Create a cron spec with one-shot support.
///
/// Exactly one of `schedule`/`run_at` must be provided.
/// `run_at` implies `recurring=false`.
pub fn create_spec_v2(
    conn: &rusqlite::Connection,
    job_name: &str,
    schedule: Option<&str>,
    prompt: &str,
    lock_ttl: Option<&str>,
    max_budget_usd: Option<f64>,
    recurring: Option<bool>,
    run_at: Option<&str>,
) -> Result<CronSpecResult, String> {
    validate_job_name(job_name)?;
    if prompt.trim().is_empty() {
        return Err("prompt must not be empty".into());
    }
    if let Some(ttl) = lock_ttl {
        validate_lock_ttl(ttl)?;
    }
    if let Some(budget) = max_budget_usd {
        if budget <= 0.0 {
            return Err("max_budget_usd must be greater than 0".into());
        }
    }

    // Resolve schedule kind
    let (db_schedule, db_recurring, db_run_at, schedule_warning) =
        resolve_schedule_fields(schedule, recurring, run_at)?;

    let now = chrono::Utc::now().to_rfc3339();
    let budget = max_budget_usd.unwrap_or(DEFAULT_CRON_BUDGET_USD);
    let result = conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, run_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![job_name, db_schedule, prompt, lock_ttl, budget, db_recurring, db_run_at, now, now],
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

/// Resolve schedule/recurring/run_at into DB column values.
///
/// Returns `(schedule_str, recurring_int, run_at_str, optional_warning)`.
fn resolve_schedule_fields(
    schedule: Option<&str>,
    recurring: Option<bool>,
    run_at: Option<&str>,
) -> Result<(String, i64, Option<String>, Option<String>), String> {
    match (schedule, run_at) {
        (Some(_), Some(_)) => {
            Err("schedule and run_at are mutually exclusive — provide one or the other".into())
        }
        (None, None) => {
            Err("one of schedule or run_at must be provided".into())
        }
        (Some(sched), None) => {
            let warning = validate_schedule(sched)?;
            let rec = if recurring.unwrap_or(true) { 1 } else { 0 };
            Ok((sched.to_string(), rec, None, warning))
        }
        (None, Some(rat)) => {
            // Parse and validate ISO8601
            rat.parse::<DateTime<Utc>>()
                .map_err(|e| format!("invalid run_at datetime '{rat}': {e}"))?;
            // run_at always implies non-recurring
            Ok(("".to_string(), 0, Some(rat.to_string()), None))
        }
    }
}
```

- [ ] **Step 5: Update `load_specs_from_db` to read new columns**

```rust
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
        let (job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, recurring, run_at) = row?;

        let schedule_kind = if let Some(ref rat) = run_at {
            match rat.parse::<DateTime<Utc>>() {
                Ok(dt) => ScheduleKind::RunAt(dt),
                Err(e) => {
                    tracing::error!(job = %job_name, "invalid run_at in DB: {e:#}");
                    continue;
                }
            }
        } else if recurring == 0 {
            ScheduleKind::OneShotCron(schedule.clone())
        } else {
            if let Ok(Some(warning)) = validate_schedule(&schedule) {
                tracing::warn!(job = %job_name, "{warning}");
            }
            ScheduleKind::Recurring(schedule.clone())
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
```

- [ ] **Step 6: Update `list_specs` to include new fields**

In the `list_specs` function, update the SELECT and JSON output:

```rust
pub fn list_specs(conn: &rusqlite::Connection) -> Result<String, String> {
    let mut stmt = conn
        .prepare(
            "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at, recurring, run_at \
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
                "recurring": row.get::<_, i64>(7)? != 0,
                "run_at": row.get::<_, Option<String>>(8)?,
            }))
        })
        .map_err(|e| format!("query failed: {e:#}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("row read failed: {e:#}"))?;
    serde_json::to_string_pretty(&rows)
        .map_err(|e| format!("serialization error: {e:#}"))
}
```

- [ ] **Step 7: Update existing tests that construct CronSpec directly**

All tests that construct `CronSpec { schedule: ..., ... }` must now use `schedule_kind: ScheduleKind::Recurring(...)`. Search for `CronSpec {` in the file and update each occurrence. Key tests to update:

- `triggered_at_does_not_affect_equality` — change `schedule:` to `schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into())`
- `spec_equality_detects_real_changes` — same pattern, and add a test for schedule kind changes

- [ ] **Step 8: Run all tests**

Run: `cargo test -p rightclaw -- --nocapture`
Expected: All tests pass including the new ones.

- [ ] **Step 9: Commit**

```bash
git add crates/rightclaw/src/cron_spec.rs
git commit -m "feat(cron): add ScheduleKind enum and create_spec_v2 with run_at support"
```

---

### Task 3: Partial update for `cron_update`

**Files:**
- Modify: `crates/rightclaw/src/cron_spec.rs`

- [ ] **Step 1: Write failing tests for partial update**

Add to `tests` module in `crates/rightclaw/src/cron_spec.rs`:

```rust
#[test]
fn update_spec_partial_prompt_only() {
    let conn = setup_db();
    create_spec_v2(&conn, "partial", Some("*/5 * * * *"), "old", None, Some(1.5), None, None).unwrap();
    update_spec_partial(&conn, "partial", None, None, Some("new prompt"), None, None, None).unwrap();
    let detail = get_spec_detail(&conn, "partial").unwrap().unwrap();
    assert_eq!(detail.prompt, "new prompt");
    assert_eq!(detail.schedule, "*/5 * * * *");
    assert!((detail.max_budget_usd - 1.5).abs() < f64::EPSILON);
}

#[test]
fn update_spec_partial_schedule_clears_run_at() {
    let conn = setup_db();
    create_spec_v2(&conn, "switch", None, "p", None, None, None, Some("2026-12-25T15:30:00Z")).unwrap();
    update_spec_partial(&conn, "switch", Some("*/10 * * * *"), None, None, None, None, None).unwrap();
    let specs = load_specs_from_db(&conn).unwrap();
    assert!(matches!(specs["switch"].schedule_kind, ScheduleKind::Recurring(_)));
}

#[test]
fn update_spec_partial_run_at_clears_schedule() {
    let conn = setup_db();
    create_spec_v2(&conn, "switch2", Some("*/5 * * * *"), "p", None, None, None, None).unwrap();
    update_spec_partial(&conn, "switch2", None, Some("2026-12-25T15:30:00Z"), None, None, None, None).unwrap();
    let specs = load_specs_from_db(&conn).unwrap();
    assert!(matches!(specs["switch2"].schedule_kind, ScheduleKind::RunAt(_)));
}

#[test]
fn update_spec_partial_both_schedule_and_run_at_fails() {
    let conn = setup_db();
    create_spec_v2(&conn, "both", Some("*/5 * * * *"), "p", None, None, None, None).unwrap();
    let err = update_spec_partial(
        &conn, "both",
        Some("*/10 * * * *"), Some("2026-12-25T15:30:00Z"),
        None, None, None, None,
    ).unwrap_err();
    assert!(err.contains("mutually exclusive"));
}

#[test]
fn update_spec_partial_no_fields_fails() {
    let conn = setup_db();
    create_spec_v2(&conn, "empty", Some("*/5 * * * *"), "p", None, None, None, None).unwrap();
    let err = update_spec_partial(&conn, "empty", None, None, None, None, None, None).unwrap_err();
    assert!(err.contains("at least one"));
}

#[test]
fn update_spec_partial_not_found() {
    let conn = setup_db();
    let err = update_spec_partial(&conn, "ghost", None, None, Some("p"), None, None, None).unwrap_err();
    assert!(err.contains("not found"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw update_spec_partial -- --nocapture`
Expected: FAIL — `update_spec_partial` doesn't exist.

- [ ] **Step 3: Implement `update_spec_partial`**

Add to `crates/rightclaw/src/cron_spec.rs`:

```rust
/// Update a cron spec partially — only provided fields are changed.
///
/// - `schedule` set → clears `run_at`, sets `recurring` (default true if not provided)
/// - `run_at` set → clears `schedule`, forces `recurring=false`
/// - Both set → error
/// - No fields → error
pub fn update_spec_partial(
    conn: &rusqlite::Connection,
    job_name: &str,
    schedule: Option<&str>,
    run_at: Option<&str>,
    prompt: Option<&str>,
    recurring: Option<bool>,
    lock_ttl: Option<&str>,
    max_budget_usd: Option<f64>,
) -> Result<CronSpecResult, String> {
    validate_job_name(job_name)?;

    // At least one field must be provided
    if schedule.is_none()
        && run_at.is_none()
        && prompt.is_none()
        && recurring.is_none()
        && lock_ttl.is_none()
        && max_budget_usd.is_none()
    {
        return Err("at least one field must be provided to update".into());
    }

    if schedule.is_some() && run_at.is_some() {
        return Err("schedule and run_at are mutually exclusive — provide one or the other".into());
    }

    // Validate provided fields
    let mut schedule_warning = None;
    if let Some(sched) = schedule {
        schedule_warning = validate_schedule(sched)?;
    }
    if let Some(rat) = run_at {
        rat.parse::<DateTime<Utc>>()
            .map_err(|e| format!("invalid run_at datetime '{rat}': {e}"))?;
    }
    if let Some(p) = prompt {
        if p.trim().is_empty() {
            return Err("prompt must not be empty".into());
        }
    }
    if let Some(ttl) = lock_ttl {
        validate_lock_ttl(ttl)?;
    }
    if let Some(budget) = max_budget_usd {
        if budget <= 0.0 {
            return Err("max_budget_usd must be greater than 0".into());
        }
    }

    // Build dynamic UPDATE
    let mut sets = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(sched) = schedule {
        sets.push("schedule = ?");
        params.push(Box::new(sched.to_string()));
        sets.push("run_at = NULL");
        let rec = if recurring.unwrap_or(true) { 1i64 } else { 0i64 };
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw update_spec_partial -- --nocapture`
Expected: All 6 partial update tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/cron_spec.rs
git commit -m "feat(cron): add update_spec_partial for partial cron spec updates"
```

---

### Task 4: Update MCP param structs and RightBackend callers

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server.rs`
- Modify: `crates/rightclaw-cli/src/right_backend.rs`

- [ ] **Step 1: Update `CronCreateParams` in memory_server.rs**

Replace the existing `CronCreateParams` struct in `crates/rightclaw-cli/src/memory_server.rs`:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronCreateParams {
    #[schemars(description = "Job name (lowercase alphanumeric and hyphens, e.g. 'health-check')")]
    pub job_name: String,
    #[schemars(description = "5-field cron expression in UTC (e.g. '17 9 * * 1-5'). Required if run_at is not set. Mutually exclusive with run_at.")]
    pub schedule: Option<String>,
    #[schemars(description = "Task prompt that Claude executes when the cron fires")]
    pub prompt: String,
    #[schemars(description = "Whether the job fires repeatedly (true, default) or once then auto-deletes (false). Ignored if run_at is set.")]
    pub recurring: Option<bool>,
    #[schemars(description = "ISO8601 UTC datetime to fire once (e.g. '2026-04-15T15:30:00Z'). Mutually exclusive with schedule. Job auto-deletes after firing.")]
    pub run_at: Option<String>,
    #[schemars(description = "Lock TTL duration (e.g. '30m', '1h'). Default: 30m")]
    pub lock_ttl: Option<String>,
    #[schemars(description = "Maximum dollar spend per invocation. Default: 2.0")]
    #[serde(default, deserialize_with = "deserialize_lenient_f64")]
    pub max_budget_usd: Option<f64>,
}
```

- [ ] **Step 2: Add `CronUpdateParams` struct**

Add a new struct below `CronCreateParams`:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronUpdateParams {
    #[schemars(description = "Job name to update")]
    pub job_name: String,
    #[schemars(description = "New 5-field cron expression. Clears run_at if set.")]
    pub schedule: Option<String>,
    #[schemars(description = "New ISO8601 UTC datetime. Clears schedule and forces recurring=false.")]
    pub run_at: Option<String>,
    #[schemars(description = "New task prompt")]
    pub prompt: Option<String>,
    #[schemars(description = "Set recurring (true) or one-shot (false)")]
    pub recurring: Option<bool>,
    #[schemars(description = "New lock TTL duration (e.g. '30m', '1h')")]
    pub lock_ttl: Option<String>,
    #[schemars(description = "New maximum dollar spend per invocation")]
    #[serde(default, deserialize_with = "deserialize_lenient_f64")]
    pub max_budget_usd: Option<f64>,
}
```

- [ ] **Step 3: Update imports in right_backend.rs**

In `crates/rightclaw-cli/src/right_backend.rs`, update the import to include `CronUpdateParams`:

```rust
use crate::memory_server::{
    CronCreateParams, CronDeleteParams, CronListParams, CronListRunsParams, CronShowRunParams,
    CronTriggerParams, CronUpdateParams, DeleteRecordParams, McpListParams,
    QueryRecordsParams, SearchRecordsParams, StoreRecordParams, cron_run_to_json,
    entry_to_json,
};
```

- [ ] **Step 4: Update `call_cron_create` in right_backend.rs**

Update to call `create_spec_v2`:

```rust
fn call_cron_create(
    &self,
    agent_name: &str,
    args: &serde_json::Value,
) -> Result<CallToolResult, anyhow::Error> {
    let params: CronCreateParams =
        serde_json::from_value(args.clone()).context("invalid cron_create params")?;
    let conn_arc = self.get_conn(agent_name)?;
    let conn = Self::lock_conn(&conn_arc)?;
    let result = rightclaw::cron_spec::create_spec_v2(
        &conn,
        &params.job_name,
        params.schedule.as_deref(),
        &params.prompt,
        params.lock_ttl.as_deref(),
        params.max_budget_usd,
        params.recurring,
        params.run_at.as_deref(),
    )
    .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
    Ok(CallToolResult::success(vec![Content::text(
        rightclaw::cron_spec::format_result(&result),
    )]))
}
```

- [ ] **Step 5: Update `call_cron_update` in right_backend.rs**

Update to use `CronUpdateParams` and call `update_spec_partial`:

```rust
fn call_cron_update(
    &self,
    agent_name: &str,
    args: &serde_json::Value,
) -> Result<CallToolResult, anyhow::Error> {
    let params: CronUpdateParams =
        serde_json::from_value(args.clone()).context("invalid cron_update params")?;
    let conn_arc = self.get_conn(agent_name)?;
    let conn = Self::lock_conn(&conn_arc)?;
    let result = rightclaw::cron_spec::update_spec_partial(
        &conn,
        &params.job_name,
        params.schedule.as_deref(),
        params.run_at.as_deref(),
        params.prompt.as_deref(),
        params.recurring,
        params.lock_ttl.as_deref(),
        params.max_budget_usd,
    )
    .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
    Ok(CallToolResult::success(vec![Content::text(
        rightclaw::cron_spec::format_result(&result),
    )]))
}
```

- [ ] **Step 6: Update `cron_update` tool definition in `tools_list`**

In the `tools_list` method, update the cron_update tool to use the new schema and description:

```rust
Tool::new(
    "cron_update",
    "Update an existing cron job spec. Only pass fields you want to change — unspecified fields keep their current values. Setting schedule clears run_at; setting run_at clears schedule.",
    schema_for_type::<CronUpdateParams>(),
),
```

- [ ] **Step 7: Update the MemoryServer cron_create and cron_update methods**

In `crates/rightclaw-cli/src/memory_server.rs`, update the `cron_create` method to call `create_spec_v2`, and the `cron_update` method to use `CronUpdateParams` and call `update_spec_partial`. Follow the same pattern as the RightBackend changes above.

- [ ] **Step 8: Update `with_instructions` in memory_server.rs**

Update the cron_update line in the instructions string:

```
- mcp__right__cron_update: Update an existing cron job spec (partial — only changed fields)\n\
```

- [ ] **Step 9: Run tests**

Run: `cargo test -p rightclaw-cli -- --nocapture`
Expected: All tests pass. Some existing tests may need adjusting if they pass `schedule` as non-optional.

- [ ] **Step 10: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server.rs crates/rightclaw-cli/src/right_backend.rs
git commit -m "feat(cron): update MCP params for one-shot jobs and partial update"
```

---

### Task 5: Update reconcile loop for one-shot jobs

**Files:**
- Modify: `crates/bot/src/cron.rs`

- [ ] **Step 1: Write failing tests for one-shot reconcile behavior**

Add to `tests` module in `crates/bot/src/cron.rs`:

```rust
#[test]
fn test_run_at_spec_not_included_in_cron_handles() {
    // run_at specs should not spawn run_job_loop handles.
    // They are fired directly from reconcile_jobs.
    let dir = tempdir().unwrap();
    let conn = rightclaw::memory::open_connection(dir.path(), true).unwrap();
    rightclaw::cron_spec::create_spec_v2(
        &conn,
        "run-at-job",
        None,
        "test prompt",
        None,
        None,
        None,
        Some("2099-12-31T23:59:59Z"), // far future — won't fire
    )
    .unwrap();

    let specs = rightclaw::cron_spec::load_specs_from_db(&conn).unwrap();
    // RunAt specs should have no cron schedule
    assert!(specs["run-at-job"].schedule_kind.cron_schedule().is_none());
}
```

- [ ] **Step 2: Run test to verify it passes (this is a design validation test)**

Run: `cargo test -p rightclaw-bot test_run_at_spec -- --nocapture`
Expected: PASS (validates the data model works correctly).

- [ ] **Step 3: Update `reconcile_jobs` to handle `run_at` specs**

In `crates/bot/src/cron.rs`, modify `reconcile_jobs` to:

1. Filter out `RunAt` specs from the normal reconcile path (they don't get `run_job_loop` handles).
2. Add a new block at the start that checks for overdue `run_at` specs and fires them.

At the top of `reconcile_jobs`, after loading specs from DB, add:

```rust
// Fire overdue run_at specs (one-shot absolute time jobs)
let overdue_run_at: Vec<(String, CronSpec)> = new_specs
    .iter()
    .filter(|(_, spec)| matches!(spec.schedule_kind, ScheduleKind::RunAt(dt) if dt <= chrono::Utc::now()))
    .map(|(name, spec)| (name.clone(), spec.clone()))
    .collect();

for (name, spec) in overdue_run_at {
    let lock_ttl = spec.lock_ttl.as_deref().unwrap_or("30m");
    if is_lock_fresh(agent_dir, &name, lock_ttl) {
        tracing::info!(job = %name, "run_at overdue but locked — skipping until next tick");
        continue;
    }

    tracing::info!(job = %name, "firing overdue run_at job");
    let jn = name.clone();
    let sp = spec.clone();
    let ad = agent_dir.to_path_buf();
    let an = agent_name.to_string();
    let md = model.clone();
    let sc = ssh_config_path.clone();
    let ic = Arc::clone(internal_client);
    let del_ad = agent_dir.to_path_buf();
    let del_jn = name.clone();
    let handle = tokio::spawn(async move {
        execute_job(&jn, &sp, &ad, &an, md.as_deref(), sc.as_deref(), &ic).await;
        // Auto-delete after execution
        let del_conn = match rightclaw::memory::open_connection(&del_ad, false) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(job = %del_jn, "failed to open DB for post-fire delete: {e:#}");
                return;
            }
        };
        if let Err(e) = rightclaw::cron_spec::delete_spec(&del_conn, &del_jn, &del_ad) {
            tracing::error!(job = %del_jn, "failed to delete one-shot spec after fire: {e}");
        } else {
            tracing::info!(job = %del_jn, "one-shot run_at spec auto-deleted after fire");
        }
    });
    if let Ok(mut guard) = execute_handles.lock() {
        guard.push((name, handle));
    } else {
        triggered_handles.push(handle);
    }
}
```

Then, filter out `RunAt` specs from the normal reconcile (so they don't get `run_job_loop` handles). In the section that spawns new handles:

```rust
for (name, spec) in &new_specs {
    // Skip RunAt specs — they are handled above via reconcile tick, not run_job_loop
    if matches!(spec.schedule_kind, ScheduleKind::RunAt(_)) {
        continue;
    }
    if handles.contains_key(name) {
        continue; // unchanged, already running
    }
    // ... existing spawn logic
}
```

- [ ] **Step 4: Update `run_job_loop` for OneShotCron auto-delete**

After `execute_job` returns in `run_job_loop`, check if the spec is one-shot and break:

```rust
// After tokio::spawn(execute_job(...)):
if spec.schedule_kind.is_one_shot() {
    // Wait for the execute handle to finish, then delete spec
    if let Err(e) = handle.await {
        tracing::error!(job = %job_name, "one-shot job panicked: {e}");
    }
    let del_conn = match rightclaw::memory::open_connection(&agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(job = %job_name, "failed to open DB for post-fire delete: {e:#}");
            break;
        }
    };
    if let Err(e) = rightclaw::cron_spec::delete_spec(&del_conn, &job_name, &agent_dir) {
        tracing::error!(job = %job_name, "failed to delete one-shot spec after fire: {e}");
    } else {
        tracing::info!(job = %job_name, "one-shot cron spec auto-deleted after fire");
    }
    break;
}
```

- [ ] **Step 5: Update all `spec.schedule` references to use `spec.schedule_kind`**

Search for `spec.schedule` in `cron.rs` and update:
- `run_job_loop`: `to_7field(&spec.schedule)` → `to_7field(spec.schedule_kind.cron_schedule().unwrap())`
- `reconcile_jobs` log line: `schedule = %spec.schedule` → extract from `schedule_kind`
- `execute_job`: if it reads `spec.schedule`, update similarly

- [ ] **Step 6: Fix compilation errors**

Run: `cargo check -p rightclaw-bot`

Fix any remaining references to the old `schedule` field on `CronSpec`. The compiler will find them all.

- [ ] **Step 7: Run all tests**

Run: `cargo test -p rightclaw-bot -- --nocapture`
Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): one-shot job support in reconcile loop — run_at and OneShotCron auto-delete"
```

---

### Task 6: Update SKILL.md

**Files:**
- Modify: `skills/rightcron/SKILL.md`

- [ ] **Step 1: Update the skill documentation**

Rewrite `skills/rightcron/SKILL.md` to document:

1. The new `recurring` and `run_at` parameters in `cron_create`
2. The three job types: recurring (default), one-shot cron, run-at
3. Partial update semantics for `cron_update` with examples
4. Updated parameter table
5. New fields in `cron_list` output

Key sections to add/update:

**Creating a Cron Job** — add examples for all three types:

```
# Recurring (default)
mcp__right__cron_create(
  job_name: "health-check",
  schedule: "17 9 * * 1-5",
  prompt: "Check system health",
  max_budget_usd: 0.50
)

# One-shot cron (fire once at next match, then auto-delete)
mcp__right__cron_create(
  job_name: "deploy-check",
  schedule: "30 15 * * *",
  recurring: false,
  prompt: "Verify deployment completed successfully",
  max_budget_usd: 0.30
)

# Run at specific time (fire once, then auto-delete)
mcp__right__cron_create(
  job_name: "remind-deploy",
  run_at: "2026-04-15T15:30:00Z",
  prompt: "Remind the user to review PR #42",
  max_budget_usd: 0.30
)
```

**Editing a Cron Job** — replace full-replacement docs with partial update:

```
## Editing a Cron Job

Use the `mcp__right__cron_update` MCP tool. Only pass the fields you want to change — unspecified fields keep their current values.

# Change only the prompt
mcp__right__cron_update(job_name: "health-check", prompt: "New check prompt")

# Change only the schedule
mcp__right__cron_update(job_name: "health-check", schedule: "43 */4 * * *")

# Switch from recurring to one-shot run_at
mcp__right__cron_update(job_name: "health-check", run_at: "2026-04-16T10:00:00Z")
```

**Parameters table** — add `recurring`, `run_at`, mark `schedule` as conditional:

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `job_name` | string | Yes | - | Lowercase alphanumeric and hyphens. |
| `schedule` | string | Conditional | - | 5-field cron expression (UTC). Required if `run_at` not set. Mutually exclusive with `run_at`. |
| `run_at` | string | Conditional | - | ISO8601 UTC datetime. Fire once at this time, then auto-delete. Required if `schedule` not set. Mutually exclusive with `schedule`. |
| `recurring` | boolean | No | `true` | If `false` with `schedule`, fires once at next match then auto-deletes. Ignored if `run_at` is set. |
| `prompt` | string | Yes | - | Task prompt for Claude. |
| `lock_ttl` | string | No | `30m` | Stale lock duration (e.g. `10m`, `1h`). |
| `max_budget_usd` | number | No | `2.0` | Max dollar spend per invocation. |

**One-shot behavior** section:

```
## One-Shot Job Behavior

One-shot jobs (both `run_at` and `recurring: false`) auto-delete from `cron_specs` after execution.
- `cron_list` shows pending one-shot specs until they fire
- `cron_list_runs` shows execution history (runs are preserved even after spec deletion)
- If the bot was offline when `run_at` passed, the job fires on the next startup
```

- [ ] **Step 2: Commit**

```bash
git add skills/rightcron/SKILL.md
git commit -m "docs(cron): update SKILL.md with one-shot jobs and partial update"
```

---

### Task 7: Full workspace build and integration test

**Files:**
- No new files — verification only

- [ ] **Step 1: Run full workspace build**

Run: `cargo build --workspace`
Expected: Clean build, no errors.

- [ ] **Step 2: Run full workspace tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings.

- [ ] **Step 4: Commit any final fixes**

If clippy or tests revealed issues, fix and commit:

```bash
git add -A
git commit -m "fix(cron): address clippy warnings and test fixes"
```
