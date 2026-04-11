# Cron Specs to DB Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move cron spec storage from YAML files to SQLite, with MCP CRUD tools for agents.

**Architecture:** New `cron_specs` table (V6 migration). 4 MCP tools (create/update/delete/list) write to DB. `load_specs()` replaced with `load_specs_from_db()` reading from same table. Reconcile loop unchanged. Validation helpers in core crate for reuse across MCP and engine.

**Tech Stack:** Rust, rusqlite, rmcp, schemars, cron crate (for schedule validation)

---

### Task 1: V6 migration — `cron_specs` table

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v6_cron_specs.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`
- Modify: `crates/rightclaw/src/memory/mod.rs` (update version test)

- [ ] **Step 1: Write failing test**

In `crates/rightclaw/src/memory/migrations.rs`, add test:

```rust
#[test]
fn migrations_apply_cleanly_to_v6() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('cron_specs')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(cols.contains(&"job_name".to_string()), "job_name column missing");
    assert!(cols.contains(&"schedule".to_string()), "schedule column missing");
    assert!(cols.contains(&"prompt".to_string()), "prompt column missing");
    assert!(cols.contains(&"lock_ttl".to_string()), "lock_ttl column missing");
    assert!(cols.contains(&"max_budget_usd".to_string()), "max_budget_usd column missing");
    assert!(cols.contains(&"created_at".to_string()), "created_at column missing");
    assert!(cols.contains(&"updated_at".to_string()), "updated_at column missing");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw migrations_apply_cleanly_to_v6`

- [ ] **Step 3: Create migration SQL**

Create `crates/rightclaw/src/memory/sql/v6_cron_specs.sql`:

```sql
-- V6: Cron spec storage in DB (replaces crons/*.yaml files).
CREATE TABLE cron_specs (
    job_name       TEXT PRIMARY KEY,
    schedule       TEXT NOT NULL,
    prompt         TEXT NOT NULL,
    lock_ttl       TEXT,
    max_budget_usd REAL NOT NULL DEFAULT 1.0,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);
```

- [ ] **Step 4: Wire migration**

In `crates/rightclaw/src/memory/migrations.rs`:

```rust
const V6_SCHEMA: &str = include_str!("sql/v6_cron_specs.sql");
```

Add `M::up(V6_SCHEMA)` to the MIGRATIONS vec.

- [ ] **Step 5: Update version test in mod.rs**

In `crates/rightclaw/src/memory/mod.rs`, update `user_version_is_5` test to `user_version_is_6` with assertion `version == 6`.

- [ ] **Step 6: Run tests**

Run: `cargo test -p rightclaw migrations`

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v6_cron_specs.sql crates/rightclaw/src/memory/migrations.rs crates/rightclaw/src/memory/mod.rs
git commit -m "feat(cron): V6 migration — cron_specs table"
```

---

### Task 2: Cron spec validation helpers in core crate

**Files:**
- Create: `crates/rightclaw/src/cron_spec.rs`
- Modify: `crates/rightclaw/src/lib.rs` (add module)
- Modify: `crates/rightclaw/Cargo.toml` (add `cron` dep)

Validation logic needs to be shared between MCP tools (in rightclaw-cli) and the cron engine (in rightclaw-bot). Put it in the core `rightclaw` crate.

- [ ] **Step 1: Add `cron` dependency to rightclaw core**

In `crates/rightclaw/Cargo.toml`, add under `[dependencies]`:

```toml
cron = { workspace = true }
```

- [ ] **Step 2: Write failing tests**

Create `crates/rightclaw/src/cron_spec.rs`:

```rust
use std::collections::HashMap;

/// Cron spec as stored in DB. Same fields as the YAML CronSpec in bot crate.
#[derive(Debug, Clone, PartialEq)]
pub struct CronSpec {
    pub schedule: String,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: f64,
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
        let result = validate_schedule("0 9 * * *").unwrap();
        assert!(result.is_some(), "should warn on :00");
        let result = validate_schedule("30 9 * * *").unwrap();
        assert!(result.is_some(), "should warn on :30");
        let result = validate_schedule("17 9 * * *").unwrap();
        assert!(result.is_none(), "no warn on :17");
    }

    #[test]
    fn load_specs_from_db_empty() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();
        let specs = load_specs_from_db(&conn);
        assert!(specs.is_empty());
    }

    #[test]
    fn load_specs_from_db_returns_all() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('job1', '*/5 * * * *', 'do stuff', 0.5, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at) \
             VALUES ('job2', '17 9 * * *', 'other', '1h', 1.0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        ).unwrap();
        let specs = load_specs_from_db(&conn);
        assert_eq!(specs.len(), 2);
        assert_eq!(specs["job1"].schedule, "*/5 * * * *");
        assert_eq!(specs["job1"].max_budget_usd, 0.5);
        assert_eq!(specs["job2"].lock_ttl.as_deref(), Some("1h"));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rightclaw cron_spec`

- [ ] **Step 4: Implement validation functions and load_specs_from_db**

In `crates/rightclaw/src/cron_spec.rs`, add:

```rust
/// Validate a job name: lowercase alphanumeric + hyphens, no leading hyphen.
pub fn validate_job_name(name: &str) -> Result<(), String> {
    let re = regex_lite::Regex::new(r"^[a-z0-9][a-z0-9-]*$").unwrap();
    if !re.is_match(name) {
        return Err(format!(
            "invalid job name '{name}': must match [a-z0-9][a-z0-9-]*"
        ));
    }
    Ok(())
}

/// Validate a 5-field cron expression. Returns Ok(Some(warning)) for round minutes,
/// Ok(None) for valid with no warning, Err for invalid.
pub fn validate_schedule(schedule: &str) -> Result<Option<String>, String> {
    use cron::Schedule;
    use std::str::FromStr;

    let seven_field = format!("0 {} *", schedule.trim());
    Schedule::from_str(&seven_field)
        .map_err(|e| format!("invalid cron schedule '{schedule}': {e}"))?;

    let minute_field = schedule.split_whitespace().next().unwrap_or("");
    let warning = if matches!(minute_field, "0" | "00" | "30") {
        Some(format!(
            "schedule uses :{minute_field} minutes — consider offset to avoid API rate limit spikes"
        ))
    } else {
        None
    };
    Ok(warning)
}

/// Load all cron specs from the database.
pub fn load_specs_from_db(conn: &rusqlite::Connection) -> HashMap<String, CronSpec> {
    let mut map = HashMap::new();
    let mut stmt = match conn.prepare(
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd FROM cron_specs",
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to prepare cron_specs query: {e:#}");
            return map;
        }
    };
    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            CronSpec {
                schedule: row.get(1)?,
                prompt: row.get(2)?,
                lock_ttl: row.get(3)?,
                max_budget_usd: row.get(4)?,
            },
        ))
    }) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("failed to query cron_specs: {e:#}");
            return map;
        }
    };
    for row in rows.flatten() {
        let (name, spec) = row;
        if let Ok(Some(warning)) = validate_schedule(&spec.schedule) {
            tracing::warn!(job = %name, "{warning}");
        }
        map.insert(name, spec);
    }
    map
}
```

Also add `regex-lite` to `crates/rightclaw/Cargo.toml` if not already present. Check first — if `regex` or `regex-lite` is already a dependency, use it. Otherwise add `regex-lite = "0.1"` to workspace deps and wire it.

**Alternative:** If adding regex feels heavy for one check, use a manual char-by-char validation:

```rust
pub fn validate_job_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("job name cannot be empty".into());
    }
    let first = name.as_bytes()[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!("invalid job name '{name}': must start with [a-z0-9]"));
    }
    if !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err(format!("invalid job name '{name}': must match [a-z0-9-]"));
    }
    Ok(())
}
```

Use this manual approach — no regex dep needed.

- [ ] **Step 5: Register the module**

In `crates/rightclaw/src/lib.rs`, add:

```rust
pub mod cron_spec;
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p rightclaw cron_spec`

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/cron_spec.rs crates/rightclaw/src/lib.rs crates/rightclaw/Cargo.toml
git commit -m "feat(cron): validation helpers and load_specs_from_db in core crate"
```

---

### Task 3: Replace `load_specs()` with DB query in cron engine

**Files:**
- Modify: `crates/bot/src/cron.rs`

- [ ] **Step 1: Replace `load_specs` calls with `load_specs_from_db`**

In `crates/bot/src/cron.rs`:

1. Remove the old `load_specs()` function (lines 122-159) entirely.

2. Remove `CronSpec` struct definition (lines 6-16) — it now lives in `rightclaw::cron_spec::CronSpec`. Add `use rightclaw::cron_spec::CronSpec;` at the top.

3. Remove the `default_cron_max_budget_usd` function.

4. In `reconcile_jobs()`, change the call:

```rust
// Before:
let new_specs = load_specs(agent_dir);

// After:
let new_specs = rightclaw::cron_spec::load_specs_from_db(&conn);
```

5. Thread a `&rusqlite::Connection` through `run_cron_task` → `reconcile_jobs`. In `run_cron_task`, open the connection once:

```rust
let conn = match rightclaw::memory::open_connection(&agent_dir) {
    Ok(c) => c,
    Err(e) => {
        tracing::error!(agent = %agent_name, "cron task: DB open failed: {e:#}");
        return;
    }
};
```

Pass `&conn` to `reconcile_jobs`.

6. Update `reconcile_jobs` signature to accept `conn: &rusqlite::Connection`.

7. Remove `is_round_minutes()` function from cron.rs — it's now handled inside `validate_schedule` / `load_specs_from_db`.

8. Keep `parse_lock_ttl()`, `to_7field()`, `is_lock_fresh()` — these are runtime helpers, not spec loading.

- [ ] **Step 2: Remove unused imports**

Remove `walkdir` and `serde_saphyr` usage from cron.rs. Check if they're used elsewhere in the bot crate — if not, remove from `crates/bot/Cargo.toml` too.

- [ ] **Step 3: Update tests**

The old `load_specs` tests (`test_load_specs_empty_dir`, `test_load_specs_valid_yaml`, etc.) need to be replaced with DB-based equivalents. But those tests now live in `crates/rightclaw/src/cron_spec.rs` (Task 2). Remove the old filesystem-based tests from cron.rs.

Also remove `test_is_round_minutes_*` tests — the function moved to core crate.

Keep: `test_to_7field_*`, `test_parse_lock_ttl_*`, `test_is_lock_fresh_*`, `parse_cron_output_*` tests.

- [ ] **Step 4: Verify compilation and tests**

Run: `cargo check -p rightclaw-bot`
Run: `cargo test -p rightclaw-bot cron`

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): replace load_specs filesystem scan with load_specs_from_db"
```

---

### Task 4: MCP tools — stdio transport (`memory_server.rs`)

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server.rs`

- [ ] **Step 1: Add parameter structs**

After the existing `CronShowRunParams` struct (around line 53), add:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronCreateParams {
    #[schemars(description = "Job name (lowercase alphanumeric and hyphens, e.g. 'health-check')")]
    pub job_name: String,
    #[schemars(description = "5-field cron expression in UTC (e.g. '17 9 * * 1-5')")]
    pub schedule: String,
    #[schemars(description = "Task prompt that Claude executes when the cron fires")]
    pub prompt: String,
    #[schemars(description = "Lock TTL duration (e.g. '30m', '1h'). Default: 30m")]
    pub lock_ttl: Option<String>,
    #[schemars(description = "Maximum dollar spend per invocation. Default: 1.0")]
    pub max_budget_usd: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronDeleteParams {
    #[schemars(description = "Job name to delete")]
    pub job_name: String,
}
```

Note: `CronUpdateParams` uses the same fields as `CronCreateParams` — reuse it via type alias or just use `CronCreateParams` for both.

- [ ] **Step 2: Add `cron_create` tool**

Add after the existing `cron_show_run` method:

```rust
#[tool(description = "Create a new cron job spec. The runtime picks up new specs within ~60 seconds. Returns warning if schedule uses :00 or :30 minutes.")]
async fn cron_create(
    &self,
    Parameters(params): Parameters<CronCreateParams>,
) -> Result<CallToolResult, McpError> {
    // Validate
    rightclaw::cron_spec::validate_job_name(&params.job_name)
        .map_err(|e| McpError::invalid_params(e, None))?;
    let warning = rightclaw::cron_spec::validate_schedule(&params.schedule)
        .map_err(|e| McpError::invalid_params(e, None))?;
    if params.prompt.trim().is_empty() {
        return Err(McpError::invalid_params("prompt cannot be empty", None));
    }
    if let Some(ref ttl) = params.lock_ttl {
        rightclaw::cron_spec::validate_lock_ttl(ttl)
            .map_err(|e| McpError::invalid_params(e, None))?;
    }
    let budget = params.max_budget_usd.unwrap_or(1.0);
    if budget <= 0.0 {
        return Err(McpError::invalid_params("max_budget_usd must be > 0", None));
    }

    let conn = self.conn.lock()
        .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![params.job_name, params.schedule, params.prompt, params.lock_ttl, budget, now, now],
    ).map_err(|e| {
        if let rusqlite::Error::SqliteFailure(ref err, _) = e {
            if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY {
                return McpError::invalid_params(
                    format!("cron job '{}' already exists — use cron_update to modify", params.job_name),
                    None,
                );
            }
        }
        McpError::internal_error(format!("insert failed: {e:#}"), None)
    })?;

    let mut msg = format!("Created cron job '{}' (schedule: {})", params.job_name, params.schedule);
    if let Some(ref w) = warning {
        msg.push_str(&format!("\n⚠️ {w}"));
    }
    Ok(CallToolResult::success(vec![Content::text(msg)]))
}
```

- [ ] **Step 3: Add `cron_update` tool**

```rust
#[tool(description = "Update an existing cron job spec (full replacement). Changes take effect within ~60 seconds.")]
async fn cron_update(
    &self,
    Parameters(params): Parameters<CronCreateParams>,
) -> Result<CallToolResult, McpError> {
    // Same validation as cron_create
    rightclaw::cron_spec::validate_job_name(&params.job_name)
        .map_err(|e| McpError::invalid_params(e, None))?;
    let warning = rightclaw::cron_spec::validate_schedule(&params.schedule)
        .map_err(|e| McpError::invalid_params(e, None))?;
    if params.prompt.trim().is_empty() {
        return Err(McpError::invalid_params("prompt cannot be empty", None));
    }
    if let Some(ref ttl) = params.lock_ttl {
        rightclaw::cron_spec::validate_lock_ttl(ttl)
            .map_err(|e| McpError::invalid_params(e, None))?;
    }
    let budget = params.max_budget_usd.unwrap_or(1.0);
    if budget <= 0.0 {
        return Err(McpError::invalid_params("max_budget_usd must be > 0", None));
    }

    let conn = self.conn.lock()
        .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
    let now = chrono::Utc::now().to_rfc3339();

    let updated = conn.execute(
        "UPDATE cron_specs SET schedule=?1, prompt=?2, lock_ttl=?3, max_budget_usd=?4, updated_at=?5 \
         WHERE job_name=?6",
        rusqlite::params![params.schedule, params.prompt, params.lock_ttl, budget, now, params.job_name],
    ).map_err(|e| McpError::internal_error(format!("update failed: {e:#}"), None))?;

    if updated == 0 {
        return Err(McpError::invalid_params(
            format!("cron job '{}' not found — use cron_create to add it", params.job_name),
            None,
        ));
    }

    let mut msg = format!("Updated cron job '{}' (schedule: {})", params.job_name, params.schedule);
    if let Some(ref w) = warning {
        msg.push_str(&format!("\n⚠️ {w}"));
    }
    Ok(CallToolResult::success(vec![Content::text(msg)]))
}
```

- [ ] **Step 4: Add `cron_delete` tool**

```rust
#[tool(description = "Delete a cron job spec. The runtime stops the job within ~60 seconds.")]
async fn cron_delete(
    &self,
    Parameters(params): Parameters<CronDeleteParams>,
) -> Result<CallToolResult, McpError> {
    let conn = self.conn.lock()
        .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;

    let deleted = conn.execute(
        "DELETE FROM cron_specs WHERE job_name = ?1",
        rusqlite::params![params.job_name],
    ).map_err(|e| McpError::internal_error(format!("delete failed: {e:#}"), None))?;

    if deleted == 0 {
        return Err(McpError::invalid_params(
            format!("cron job '{}' not found", params.job_name),
            None,
        ));
    }

    // Remove lock file if present
    let lock_path = self.agent_dir
        .join("crons")
        .join(".locks")
        .join(format!("{}.json", params.job_name));
    let _ = std::fs::remove_file(&lock_path);

    Ok(CallToolResult::success(vec![Content::text(
        format!("Deleted cron job '{}'", params.job_name),
    )]))
}
```

- [ ] **Step 5: Add `cron_list` tool**

```rust
#[tool(description = "List all cron job specs (schedules, prompts, budgets). Returns current specs, not run history.")]
async fn cron_list(
    &self,
) -> Result<CallToolResult, McpError> {
    let conn = self.conn.lock()
        .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;

    let mut stmt = conn.prepare(
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd FROM cron_specs ORDER BY job_name",
    ).map_err(|e| McpError::internal_error(format!("prepare failed: {e:#}"), None))?;

    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "job_name": row.get::<_, String>(0)?,
                "schedule": row.get::<_, String>(1)?,
                "prompt": row.get::<_, String>(2)?,
                "lock_ttl": row.get::<_, Option<String>>(3)?,
                "max_budget_usd": row.get::<_, f64>(4)?,
            }))
        })
        .map_err(|e| McpError::internal_error(format!("query failed: {e:#}"), None))?
        .filter_map(|r| r.ok())
        .collect();

    let output = serde_json::to_string_pretty(&rows)
        .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(output)]))
}
```

- [ ] **Step 6: Update `with_instructions()`**

Find the `with_instructions()` call and update the Cron section:

```
## Cron\n\
- cron_create: Create a new cron job spec (schedule, prompt, budget)\n\
- cron_update: Update an existing cron job spec (full replacement)\n\
- cron_delete: Delete a cron job spec\n\
- cron_list: List all current cron job specs\n\
- cron_list_runs: List recent cron job executions\n\
- cron_show_run: Get details of a specific cron run\n\n\
```

- [ ] **Step 7: Add `validate_lock_ttl` to core crate**

In `crates/rightclaw/src/cron_spec.rs`, add:

```rust
/// Validate a lock_ttl string ("30m", "1h").
pub fn validate_lock_ttl(s: &str) -> Result<(), String> {
    if let Some(mins) = s.strip_suffix('m') {
        mins.trim().parse::<i64>()
            .map_err(|_| format!("invalid lock_ttl '{s}': expected e.g. '30m' or '1h'"))?;
        return Ok(());
    }
    if let Some(hrs) = s.strip_suffix('h') {
        hrs.trim().parse::<i64>()
            .map_err(|_| format!("invalid lock_ttl '{s}': expected e.g. '30m' or '1h'"))?;
        return Ok(());
    }
    Err(format!("invalid lock_ttl '{s}': expected e.g. '30m' or '1h'"))
}
```

Add test:

```rust
#[test]
fn validate_lock_ttl_valid() {
    assert!(validate_lock_ttl("30m").is_ok());
    assert!(validate_lock_ttl("1h").is_ok());
    assert!(validate_lock_ttl("2h").is_ok());
}

#[test]
fn validate_lock_ttl_invalid() {
    assert!(validate_lock_ttl("bad").is_err());
    assert!(validate_lock_ttl("30").is_err());
    assert!(validate_lock_ttl("").is_err());
}
```

- [ ] **Step 8: Verify compilation**

Run: `cargo check -p rightclaw-cli`

- [ ] **Step 9: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server.rs crates/rightclaw/src/cron_spec.rs
git commit -m "feat(cron): MCP CRUD tools for cron specs (stdio transport)"
```

---

### Task 5: MCP tools — HTTP transport (`memory_server_http.rs`)

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server_http.rs`

Same 4 tools as Task 4, but with HTTP agent extraction pattern. Each method starts with:

```rust
let agent = Self::agent_from_parts(&parts)?;
let conn_arc = self.get_conn_for_agent(&agent)?;
let conn = conn_arc.lock()
    .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
```

- [ ] **Step 1: Add `cron_create` with HTTP agent extraction**

```rust
#[tool(description = "Create a new cron job spec. The runtime picks up new specs within ~60 seconds.")]
async fn cron_create(
    &self,
    Extension(parts): Extension<http::request::Parts>,
    Parameters(params): Parameters<CronCreateParams>,
) -> Result<CallToolResult, McpError> {
    let agent = Self::agent_from_parts(&parts)?;
    let conn_arc = self.get_conn_for_agent(&agent)?;
    let conn = conn_arc.lock()
        .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;

    // Same validation + insert logic as stdio version
    rightclaw::cron_spec::validate_job_name(&params.job_name)
        .map_err(|e| McpError::invalid_params(e, None))?;
    let warning = rightclaw::cron_spec::validate_schedule(&params.schedule)
        .map_err(|e| McpError::invalid_params(e, None))?;
    if params.prompt.trim().is_empty() {
        return Err(McpError::invalid_params("prompt cannot be empty", None));
    }
    if let Some(ref ttl) = params.lock_ttl {
        rightclaw::cron_spec::validate_lock_ttl(ttl)
            .map_err(|e| McpError::invalid_params(e, None))?;
    }
    let budget = params.max_budget_usd.unwrap_or(1.0);
    if budget <= 0.0 {
        return Err(McpError::invalid_params("max_budget_usd must be > 0", None));
    }

    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![params.job_name, params.schedule, params.prompt, params.lock_ttl, budget, now, now],
    ).map_err(|e| {
        if let rusqlite::Error::SqliteFailure(ref err, _) = e {
            if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY {
                return McpError::invalid_params(
                    format!("cron job '{}' already exists", params.job_name), None,
                );
            }
        }
        McpError::internal_error(format!("insert failed: {e:#}"), None)
    })?;

    let mut msg = format!("Created cron job '{}' (schedule: {})", params.job_name, params.schedule);
    if let Some(ref w) = warning {
        msg.push_str(&format!("\n⚠️ {w}"));
    }
    Ok(CallToolResult::success(vec![Content::text(msg)]))
}
```

- [ ] **Step 2: Add `cron_update` (HTTP)**

Same pattern: agent extraction, then same logic as stdio `cron_update`. Use `CronCreateParams`.

- [ ] **Step 3: Add `cron_delete` (HTTP)**

Same pattern: agent extraction, lock_path uses `self.agents_dir.join(&agent.name).join("crons").join(".locks")`.

- [ ] **Step 4: Add `cron_list` (HTTP)**

Same pattern: agent extraction, same SQL query.

- [ ] **Step 5: Update HTTP `with_instructions()`**

Same text as stdio version.

- [ ] **Step 6: Import `CronCreateParams` and `CronDeleteParams`**

These structs are defined in `memory_server.rs`. Import them:

```rust
use crate::memory_server::{CronCreateParams, CronDeleteParams};
```

Or define them in a shared module. Check existing pattern — if HTTP server already imports from stdio server, follow that. Otherwise duplicate the structs.

- [ ] **Step 7: Verify compilation**

Run: `cargo check -p rightclaw-cli`

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server_http.rs
git commit -m "feat(cron): MCP CRUD tools for cron specs (HTTP transport)"
```

---

### Task 6: Rewrite rightcron SKILL.md

**Files:**
- Modify: `skills/rightcron/SKILL.md`

- [ ] **Step 1: Rewrite the skill**

Replace the entire file with:

```markdown
---
name: rightcron
description: >-
  Manages cron jobs for this RightClaw agent via MCP tools. Creates, updates,
  and deletes cron specs stored in the agent database. The Rust runtime handles
  scheduling and execution automatically. Use when the user mentions cron
  jobs, scheduled tasks, RightCron, or recurring tasks.
version: 2.0.0
---

# /rightcron -- Cron Job Manager

## When to Activate

Activate this skill when:
- The user mentions "cron", "cron jobs", "scheduled tasks", or "RightCron"
- The user asks to schedule, create, remove, or change a recurring task
- The user asks about cron run history or why a job failed

## How It Works

Cron specs are stored in the agent database. The Rust runtime polls specs every 60 seconds and schedules jobs automatically. Use MCP tools to manage specs — no file creation needed.

## Creating a Cron Job

Use the `cron_create` MCP tool:

```
cron_create(
  job_name: "health-check",
  schedule: "17 9 * * 1-5",
  prompt: "Check system health and report status",
  max_budget_usd: 0.50
)
```

Confirm to the user: "Job created. The runtime picks up new specs within ~60 seconds."

## Editing a Cron Job

Use the `cron_update` MCP tool (full replacement — all fields required):

```
cron_update(
  job_name: "health-check",
  schedule: "43 */4 * * *",
  prompt: "Check system health, alert on degradation",
  max_budget_usd: 0.75
)
```

Confirm: "Job updated. Changes take effect within ~60 seconds."

## Removing a Cron Job

Use the `cron_delete` MCP tool:

```
cron_delete(job_name: "health-check")
```

Confirm: "Job removed. The runtime drops it within ~60 seconds."

## Listing Current Cron Jobs

Use the `cron_list` MCP tool to see all configured jobs:

```
cron_list()
```

Returns: job_name, schedule, prompt, lock_ttl, max_budget_usd for each job.

## Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `job_name` | string | Yes | - | Lowercase alphanumeric and hyphens (e.g. `health-check`). |
| `schedule` | string | Yes | - | Standard 5-field cron expression (minute hour day-of-month month day-of-week). Evaluated in **UTC**. |
| `prompt` | string | Yes | - | The task prompt that Claude executes when the cron fires. |
| `lock_ttl` | string | No | `30m` | Duration after which a lock is considered stale (e.g. `10m`, `1h`). |
| `max_budget_usd` | number | No | `1.0` | Maximum dollar spend per invocation. Claude stops gracefully when budget is reached. |

### Schedule Guidelines

When the user doesn't specify exact minutes, **avoid :00 and :30** — these are peak times when many automated jobs fire simultaneously, causing API rate limit spikes. Use odd minutes like `:17`, `:43`, `:07`, `:53` to spread load.

The tool returns a warning when it detects `:00` or `:30` in the minute field.

## Checking Run History

Use the `rightclaw` MCP server tools to check cron job execution history.

### cron_list_runs

Returns recent runs sorted by `started_at` descending.

Parameters:
- `job_name` (optional string) — filter by job name; omit to return all jobs
- `limit` (optional integer) — max runs to return; default 20

Each run record contains: `id`, `job_name`, `started_at`, `finished_at`, `exit_code`, `status`, `log_path`

### cron_show_run

Returns full metadata for a single run.

Parameters:
- `run_id` (string, UUID) — run to retrieve

### Reading logs

The `log_path` field in each run record points to the log file. Read it directly:

```
cat <log_path>
```

### Debugging example

```
User: "Why did morning-briefing fail?"

1. cron_list_runs(job_name="morning-briefing", limit=5)
   -> Find the failed run (status="failed")
2. cron_show_run(run_id="<run_id from step 1>")
   -> Get full metadata including log_path
3. cat <log_path>
   -> Read the subprocess output to diagnose the failure
```

## Constraints

1. **UTC schedules**: Cron expressions are evaluated in UTC by the Rust runtime.
2. **60-second polling**: The runtime re-reads specs every 60 seconds. After creating, editing, or deleting a spec, changes take effect within ~1 minute.
```

- [ ] **Step 2: Commit**

```bash
git add skills/rightcron/SKILL.md
git commit -m "feat(cron): rewrite rightcron skill to use MCP tools"
```

---

### Task 7: Build workspace and run all tests

**Files:** None (verification only)

- [ ] **Step 1: Build**

Run: `cargo build --workspace`

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace -- -D warnings`

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix: address build/clippy issues from cron specs to DB"
```

---

### Task 8: Update ARCHITECTURE.md

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update Memory Schema section**

Add `cron_specs` to the schema listing:

```
cron_specs      (job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at)
```

- [ ] **Step 2: Update module description for cron.rs**

Change:
```
├── cron.rs             # Cron engine: load specs, lock check, invoke CC, persist results to DB
```
To:
```
├── cron.rs             # Cron engine: load specs from DB, lock check, invoke CC, persist results
```

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: update ARCHITECTURE.md for cron specs in DB"
```
