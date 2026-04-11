# Cron Telegram Commands & Trigger — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add read-only `/cron` Telegram commands for monitoring, an MCP `cron_trigger` tool for manual execution, and unified trigger handling in the cron engine.

**Architecture:** New `triggered_at` column in `cron_specs` table (migration V7). Cron engine checks `triggered_at OR schedule_matches` on each tick — single code path. `/cron` Telegram command follows `/mcp` pattern in handler.rs. `cron_trigger` MCP tool is a thin wrapper over shared `trigger_spec()` in `cron_spec.rs`. Human-readable schedules via `cron-descriptor` crate.

**Tech Stack:** Rust (edition 2024), rusqlite, teloxide, rmcp, cron-descriptor 0.1.1

---

## File Structure

| File | Role |
|------|------|
| `crates/rightclaw/src/memory/sql/v7_cron_trigger.sql` | **Create** — migration adding `triggered_at` column |
| `crates/rightclaw/src/memory/migrations.rs` | **Modify** — wire V7 migration |
| `crates/rightclaw/src/cron_spec.rs` | **Modify** — add `trigger_spec()`, `clear_triggered_at()`, `get_spec_detail()`, `get_recent_runs()`, `describe_schedule()` |
| `Cargo.toml` | **Modify** — add `cron-descriptor` to workspace deps |
| `crates/rightclaw/Cargo.toml` | **Modify** — add `cron-descriptor` dependency |
| `crates/bot/src/cron.rs` | **Modify** — add `triggered_at` to `CronSpec` loading, unified trigger check in engine |
| `crates/bot/src/telegram/dispatch.rs` | **Modify** — add `BotCommand::Cron(String)` variant + branch |
| `crates/bot/src/telegram/handler.rs` | **Modify** — add `handle_cron`, `handle_cron_list`, `handle_cron_detail` |
| `crates/rightclaw-cli/src/memory_server.rs` | **Modify** — add `CronTriggerParams`, `cron_trigger` tool, update `with_instructions()` |
| `crates/rightclaw-cli/src/memory_server_http.rs` | **Modify** — add `cron_trigger` tool, update `with_instructions()` |
| `skills/rightcron/SKILL.md` | **Modify** — document `cron_trigger` |

---

### Task 1: Migration V7 — add `triggered_at` column

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v7_cron_trigger.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`

- [ ] **Step 1: Write the migration test**

Add to `crates/rightclaw/src/memory/migrations.rs` test module:

```rust
#[test]
fn migrations_apply_cleanly_to_v7() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('cron_specs')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(cols.contains(&"triggered_at".to_string()), "triggered_at column missing");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib memory::migrations::tests::migrations_apply_cleanly_to_v7`
Expected: FAIL — `triggered_at` column doesn't exist yet.

- [ ] **Step 3: Create the migration SQL**

Create `crates/rightclaw/src/memory/sql/v7_cron_trigger.sql`:

```sql
ALTER TABLE cron_specs ADD COLUMN triggered_at TEXT;
```

- [ ] **Step 4: Wire V7 in migrations.rs**

In `crates/rightclaw/src/memory/migrations.rs`, add:

```rust
const V7_SCHEMA: &str = include_str!("sql/v7_cron_trigger.sql");
```

And add `M::up(V7_SCHEMA)` to the `Migrations::new(vec![...])` list after V6.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rightclaw --lib memory::migrations::tests::migrations_apply_cleanly_to_v7`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v7_cron_trigger.sql crates/rightclaw/src/memory/migrations.rs
git commit -m "feat(cron): add V7 migration — triggered_at column in cron_specs"
```

---

### Task 2: Add `cron-descriptor` dependency

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/rightclaw/Cargo.toml`

- [ ] **Step 1: Add workspace dependency**

In root `Cargo.toml`, under `[workspace.dependencies]`, add:

```toml
cron-descriptor = "0.1"
```

- [ ] **Step 2: Add to rightclaw crate**

In `crates/rightclaw/Cargo.toml`, under `[dependencies]`, add:

```toml
cron-descriptor = { workspace = true }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rightclaw`
Expected: compiles cleanly.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/rightclaw/Cargo.toml
git commit -m "chore: add cron-descriptor dependency"
```

---

### Task 3: Shared helpers in `cron_spec.rs` — trigger, detail, runs, schedule description

**Files:**
- Modify: `crates/rightclaw/src/cron_spec.rs`

This task adds five functions to `cron_spec.rs`: `trigger_spec()`, `clear_triggered_at()`, `describe_schedule()`, `get_spec_detail()`, `get_recent_runs()`. All are shared helpers used by both MCP servers and the Telegram handler.

- [ ] **Step 1: Write tests for `trigger_spec`**

Add to the `#[cfg(test)] mod tests` block in `crates/rightclaw/src/cron_spec.rs`:

```rust
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
    // Second trigger overwrites — should succeed, not error
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
```

- [ ] **Step 2: Write tests for `clear_triggered_at`**

```rust
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
```

- [ ] **Step 3: Write tests for `describe_schedule`**

```rust
#[test]
fn describe_schedule_returns_description() {
    let desc = describe_schedule("*/5 * * * *");
    // Should contain something meaningful, not just the raw expression
    assert!(!desc.is_empty());
    // Fallback: if cron-descriptor fails, returns raw expression
}

#[test]
fn describe_schedule_fallback_on_invalid() {
    let desc = describe_schedule("not-valid-cron");
    assert_eq!(desc, "not-valid-cron");
}
```

- [ ] **Step 4: Write tests for `get_spec_detail`**

```rust
#[test]
fn get_spec_detail_found() {
    let conn = setup_db();
    create_spec(&conn, "detail-job", "*/5 * * * *", "do stuff", Some("1h"), Some(2.5)).unwrap();
    let detail = get_spec_detail(&conn, "detail-job").unwrap();
    assert!(detail.is_some());
    let d = detail.unwrap();
    assert_eq!(d.job_name, "detail-job");
    assert_eq!(d.schedule, "*/5 * * * *");
    assert_eq!(d.prompt, "do stuff");
    assert_eq!(d.lock_ttl.as_deref(), Some("1h"));
    assert!((d.max_budget_usd - 2.5).abs() < f64::EPSILON);
}

#[test]
fn get_spec_detail_not_found() {
    let conn = setup_db();
    let detail = get_spec_detail(&conn, "ghost").unwrap();
    assert!(detail.is_none());
}
```

- [ ] **Step 5: Write tests for `get_recent_runs`**

```rust
#[test]
fn get_recent_runs_returns_ordered() {
    let conn = setup_db();
    // Insert runs directly
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path) \
         VALUES ('r1', 'runs-job', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 0, 'success', '/tmp/r1.txt')",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path) \
         VALUES ('r2', 'runs-job', '2026-01-01T01:00:00Z', '2026-01-01T01:01:00Z', 1, 'failed', '/tmp/r2.txt')",
        [],
    ).unwrap();

    let runs = get_recent_runs(&conn, "runs-job", 5).unwrap();
    assert_eq!(runs.len(), 2);
    // Most recent first
    assert_eq!(runs[0].id, "r2");
    assert_eq!(runs[1].id, "r1");
    assert_eq!(runs[0].status, "failed");
    assert_eq!(runs[1].status, "success");
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
        ).unwrap();
    }
    let runs = get_recent_runs(&conn, "limit-job", 3).unwrap();
    assert_eq!(runs.len(), 3);
}
```

- [ ] **Step 6: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib cron_spec::tests`
Expected: FAIL — functions don't exist yet.

- [ ] **Step 7: Implement `describe_schedule`**

Add to `crates/rightclaw/src/cron_spec.rs`:

```rust
/// Convert a 5-field cron expression to a human-readable description.
///
/// Uses `cron-descriptor` for the conversion. Falls back to the raw expression
/// if the crate can't parse it (e.g. non-standard extensions).
pub fn describe_schedule(schedule: &str) -> String {
    // cron-descriptor expects 5 or 6 fields (with optional seconds).
    // Our schedules are 5-field, which it handles directly.
    match cron_descriptor::cronparser::cron_expression_descriptor::get_description_cron(schedule) {
        Ok(desc) => desc,
        Err(_) => schedule.to_string(),
    }
}
```

- [ ] **Step 8: Implement `trigger_spec` and `clear_triggered_at`**

```rust
/// Queue a cron job for immediate execution by setting its `triggered_at` timestamp.
///
/// The cron engine checks `triggered_at` on each tick and executes if set.
/// Returns error if job doesn't exist.
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
    Ok(format!("Triggered job '{job_name}'. Will execute on next engine tick (≤30s)."))
}

/// Clear the `triggered_at` flag after the cron engine has picked up the trigger.
pub fn clear_triggered_at(conn: &rusqlite::Connection, job_name: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE cron_specs SET triggered_at = NULL WHERE job_name = ?1",
        rusqlite::params![job_name],
    )
    .map_err(|e| format!("clear trigger failed: {e:#}"))?;
    Ok(())
}
```

- [ ] **Step 9: Implement `get_spec_detail` and `get_recent_runs`**

Add the `CronSpecDetail` and `CronRunSummary` structs and their query functions:

```rust
/// Full detail of a cron spec for Telegram display.
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

/// Summary of a cron run for Telegram display.
#[derive(Debug)]
pub struct CronRunSummary {
    pub id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub status: String,
}

/// Get full detail for a single cron spec. Returns None if not found.
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

/// Get the most recent N runs for a given job, ordered newest first.
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
```

- [ ] **Step 10: Update `load_specs_from_db` to include `triggered_at`**

The `CronSpec` struct needs a new field. Modify the struct at the top of the file:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct CronSpec {
    pub schedule: String,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: f64,
    pub triggered_at: Option<String>,
}
```

Update `load_specs_from_db` query to `SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at FROM cron_specs` and include the new field in the struct construction:

```rust
// In the query_map closure:
row.get::<_, Option<String>>(5)?,  // triggered_at

// In the struct construction:
CronSpec {
    schedule,
    prompt,
    lock_ttl,
    max_budget_usd,
    triggered_at,
}
```

- [ ] **Step 11: Fix existing tests that construct `CronSpec` directly**

The `reconcile_jobs` tests in `cron.rs` and any tests that construct `CronSpec` literals need the new `triggered_at: None` field. Check `crates/bot/src/cron.rs` tests and add `triggered_at: None` to any `CronSpec { ... }` literals.

- [ ] **Step 12: Run all tests**

Run: `cargo test -p rightclaw --lib cron_spec::tests`
Expected: All PASS.

- [ ] **Step 13: Commit**

```bash
git add crates/rightclaw/src/cron_spec.rs
git commit -m "feat(cron): add trigger, detail, runs, describe helpers to cron_spec.rs"
```

---

### Task 4: Cron engine — unified trigger check

**Files:**
- Modify: `crates/bot/src/cron.rs`

The cron engine needs to: (1) load `triggered_at` from specs (already done via Task 3's `CronSpec` change), (2) check trigger alongside schedule in the per-job loop, (3) clear trigger after spawning execution.

- [ ] **Step 1: Write test for triggered job execution**

Add to `crates/bot/src/cron.rs` test module:

```rust
#[test]
fn test_triggered_at_loaded_from_db() {
    let dir = tempdir().unwrap();
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    rightclaw::memory::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();

    // Create a spec and trigger it
    rightclaw::cron_spec::create_spec(&conn, "trig-test", "*/5 * * * *", "test prompt", None, None).unwrap();
    rightclaw::cron_spec::trigger_spec(&conn, "trig-test").unwrap();

    let specs = rightclaw::cron_spec::load_specs_from_db(&conn).unwrap();
    assert!(specs["trig-test"].triggered_at.is_some(), "triggered_at should be loaded");
}

#[test]
fn test_clear_triggered_at_works() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    rightclaw::memory::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();

    rightclaw::cron_spec::create_spec(&conn, "clr-test", "*/5 * * * *", "test", None, None).unwrap();
    rightclaw::cron_spec::trigger_spec(&conn, "clr-test").unwrap();
    rightclaw::cron_spec::clear_triggered_at(&conn, "clr-test").unwrap();

    let specs = rightclaw::cron_spec::load_specs_from_db(&conn).unwrap();
    assert!(specs["clr-test"].triggered_at.is_none(), "triggered_at should be cleared");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot --lib cron::tests`
Expected: FAIL — `triggered_at` field not in CronSpec yet (if Task 3 not applied), or tests need wiring.

- [ ] **Step 3: Modify `run_job_loop` to check `triggered_at`**

In `crates/bot/src/cron.rs`, the `run_job_loop` function currently only checks the cron schedule. We need to also handle triggered jobs. The key insight: `reconcile_jobs` already re-runs every 60s. For triggered jobs, the simplest approach is to check `triggered_at` in `reconcile_jobs` itself before spawning the per-job loop.

Modify `reconcile_jobs` in `crates/bot/src/cron.rs`. After the existing spawn loop (the `for (name, spec) in &new_specs` block), add a trigger check:

```rust
// Check for triggered jobs (manual trigger via cron_trigger MCP tool)
for (name, spec) in &new_specs {
    if spec.triggered_at.is_some() {
        // Clear trigger immediately to prevent re-firing on next tick
        if let Err(e) = rightclaw::cron_spec::clear_triggered_at(conn, name) {
            tracing::error!(job = %name, "failed to clear triggered_at: {e}");
            continue;
        }

        // Check lock — if locked, skip (trigger lost, same as schedule miss while locked)
        let lock_ttl = spec.lock_ttl.as_deref().unwrap_or("30m");
        if is_lock_fresh(agent_dir, name, lock_ttl) {
            tracing::info!(job = %name, "triggered but locked — skipping");
            continue;
        }

        let jn = name.clone();
        let sp = spec.clone();
        let ad = agent_dir.to_path_buf();
        let an = agent_name.to_string();
        let md = model.clone();
        let sc = ssh_config_path.clone();
        tracing::info!(job = %name, "executing triggered job");
        tokio::spawn(async move {
            execute_job(&jn, &sp, &ad, &an, md.as_deref(), sc.as_deref()).await;
        });
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rightclaw-bot --lib cron::tests`
Expected: All PASS.

- [ ] **Step 5: Build workspace**

Run: `cargo build --workspace`
Expected: compiles cleanly.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): unified trigger check in reconcile_jobs"
```

---

### Task 5: MCP `cron_trigger` tool — both servers

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server.rs`
- Modify: `crates/rightclaw-cli/src/memory_server_http.rs`

- [ ] **Step 1: Add `CronTriggerParams` to `memory_server.rs`**

Add after the existing `CronListParams` struct (around line 76):

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronTriggerParams {
    #[schemars(description = "Job name to trigger for immediate execution")]
    pub job_name: String,
}
```

- [ ] **Step 2: Add `cron_trigger` tool to stdio server**

Add inside the `#[tool_router] impl MemoryServer` block, after `cron_list`:

```rust
#[tool(description = "Trigger a cron job for immediate execution. The job is queued and will run on the next engine tick (≤30s). Lock check still applies — if the job is currently running, the trigger is skipped.")]
async fn cron_trigger(
    &self,
    Parameters(params): Parameters<CronTriggerParams>,
) -> Result<CallToolResult, McpError> {
    let conn = self
        .conn
        .lock()
        .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
    let msg = rightclaw::cron_spec::trigger_spec(&conn, &params.job_name)
        .map_err(|e| McpError::invalid_params(e, None))?;
    Ok(CallToolResult::success(vec![Content::text(msg)]))
}
```

- [ ] **Step 3: Update `with_instructions()` in stdio server**

In the `get_info()` method of `MemoryServer`, add to the Cron section:

```
                 - cron_trigger: Trigger a cron job for immediate execution\n\
```

Add this line after the `cron_list` entry in the instructions string.

- [ ] **Step 4: Add `cron_trigger` tool to HTTP server**

In `crates/rightclaw-cli/src/memory_server_http.rs`, add inside the `#[tool_router] impl HttpMemoryServer` block, after `cron_list`. Also add `CronTriggerParams` to the import from `memory_server`:

```rust
#[tool(description = "Trigger a cron job for immediate execution. The job is queued and will run on the next engine tick (≤30s). Lock check still applies — if the job is currently running, the trigger is skipped.")]
async fn cron_trigger(
    &self,
    Extension(parts): Extension<http::request::Parts>,
    Parameters(params): Parameters<CronTriggerParams>,
) -> Result<CallToolResult, McpError> {
    let agent = Self::agent_from_parts(&parts)?;
    let conn_arc = self.get_conn_for_agent(&agent)?;
    let conn = conn_arc.lock()
        .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
    let msg = rightclaw::cron_spec::trigger_spec(&conn, &params.job_name)
        .map_err(|e| McpError::invalid_params(e, None))?;
    Ok(CallToolResult::success(vec![Content::text(msg)]))
}
```

- [ ] **Step 5: Update `with_instructions()` in HTTP server**

Same change as Step 3 — add `cron_trigger` line to the Cron section.

- [ ] **Step 6: Update import in `memory_server_http.rs`**

Add `CronTriggerParams` to the import list from `crate::memory_server`:

```rust
use crate::memory_server::{
    CronCreateParams, CronDeleteParams, CronListParams, CronListRunsParams, CronShowRunParams,
    CronTriggerParams,  // <-- add this
    DeleteRecordParams, McpAddParams, McpAuthParams, McpListParams, McpRemoveParams,
    QueryRecordsParams, SearchRecordsParams, StoreRecordParams, cron_run_to_json, entry_to_json,
};
```

- [ ] **Step 7: Build workspace**

Run: `cargo build --workspace`
Expected: compiles cleanly.

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server.rs crates/rightclaw-cli/src/memory_server_http.rs
git commit -m "feat(cron): add cron_trigger MCP tool to both servers"
```

---

### Task 6: Telegram `/cron` command — dispatch + handlers

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Add `BotCommand::Cron` variant**

In `crates/bot/src/telegram/dispatch.rs`, add to the `BotCommand` enum:

```rust
#[command(description = "Cron job status (list or detail)")]
Cron(String),
```

- [ ] **Step 2: Add dispatch branch**

In the `command_handler` chain in `run_telegram()`, add after the Doctor branch:

```rust
.branch(dptree::case![BotCommand::Cron(args)].endpoint(handle_cron))
```

- [ ] **Step 3: Add `handle_cron` import**

Add `handle_cron` to the import from `super::handler` in `dispatch.rs`.

- [ ] **Step 4: Implement `handle_cron` dispatcher**

In `crates/bot/src/telegram/handler.rs`, add after the `/mcp` handler section:

```rust
// ---------------------------------------------------------------------------
// /cron command handler
// ---------------------------------------------------------------------------

/// Handle the /cron command — routes to list (no args) or detail (job name).
pub async fn handle_cron(
    bot: BotType,
    msg: Message,
    args: String,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let result = if args.trim().is_empty() {
        handle_cron_list(&bot, &msg, &agent_dir.0).await
    } else {
        handle_cron_detail(&bot, &msg, args.trim(), &agent_dir.0).await
    };
    result.map_err(|e| to_request_err(format!("{e:#}")))?;
    Ok(())
}
```

- [ ] **Step 5: Implement `handle_cron_list`**

```rust
/// `/cron` — list all cron jobs with human-readable schedule and last run status.
async fn handle_cron_list(
    bot: &BotType,
    msg: &Message,
    agent_dir: &Path,
) -> Result<(), RequestError> {
    let conn = rightclaw::memory::open_connection(agent_dir)
        .map_err(|e| to_request_err(format!("DB open failed: {e:#}")))?;

    let specs = rightclaw::cron_spec::load_specs_from_db(&conn)
        .map_err(|e| to_request_err(format!("load specs failed: {e:#}")))?;

    if specs.is_empty() {
        bot.send_message(msg.chat.id, "No cron jobs configured.").await?;
        return Ok(());
    }

    let mut text = String::from("Cron Jobs:\n\n");
    let mut names: Vec<&String> = specs.keys().collect();
    names.sort();

    for name in names {
        let spec = &specs[name];
        let desc = rightclaw::cron_spec::describe_schedule(&spec.schedule);

        // Get last run status
        let last_run = rightclaw::cron_spec::get_recent_runs(&conn, name, 1)
            .unwrap_or_default();

        let status_str = match last_run.first() {
            Some(run) => {
                let icon = match run.status.as_str() {
                    "success" => "\u{2705}",   // ✅
                    "failed" => "\u{274c}",    // ❌
                    "running" => "\u{23f3}",   // ⏳
                    _ => "?",
                };
                let ago = format_relative_time(&run.started_at);
                format!("last: {ago} {icon}")
            }
            None => "never run".to_string(),
        };

        text.push_str(&format!("\u{2022} {name} \u{2014} {desc} \u{2014} {status_str}\n"));
    }

    let eff_thread_id = effective_thread_id(&msg);
    send_html_reply(bot, msg.chat.id, eff_thread_id, &text).await?;
    Ok(())
}
```

- [ ] **Step 6: Implement `handle_cron_detail`**

```rust
/// `/cron <job-name>` — show job detail + last 5 runs.
async fn handle_cron_detail(
    bot: &BotType,
    msg: &Message,
    job_name: &str,
    agent_dir: &Path,
) -> Result<(), RequestError> {
    let conn = rightclaw::memory::open_connection(agent_dir)
        .map_err(|e| to_request_err(format!("DB open failed: {e:#}")))?;

    let detail = rightclaw::cron_spec::get_spec_detail(&conn, job_name)
        .map_err(|e| to_request_err(format!("query failed: {e:#}")))?;

    let Some(detail) = detail else {
        bot.send_message(msg.chat.id, format!("Cron job '{job_name}' not found.")).await?;
        return Ok(());
    };

    let desc = rightclaw::cron_spec::describe_schedule(&detail.schedule);
    let mut text = format!(
        "<b>{}</b>\nSchedule: {} (<code>{}</code>)\nBudget: ${:.2}",
        detail.job_name, desc, detail.schedule, detail.max_budget_usd,
    );
    if let Some(ref ttl) = detail.lock_ttl {
        text.push_str(&format!("\nLock TTL: {ttl}"));
    }
    if detail.triggered_at.is_some() {
        text.push_str("\n\u{26a1} Trigger pending");
    }

    // Recent runs
    let runs = rightclaw::cron_spec::get_recent_runs(&conn, job_name, 5)
        .unwrap_or_default();

    if runs.is_empty() {
        text.push_str("\n\nNo runs yet.");
    } else {
        text.push_str("\n\nRecent runs:");
        for (i, run) in runs.iter().enumerate() {
            let icon = match run.status.as_str() {
                "success" => "\u{2705}",
                "failed" => "\u{274c}",
                "running" => "\u{23f3}",
                _ => "?",
            };
            let ago = format_relative_time(&run.started_at);
            let duration = match (&run.finished_at, &run.started_at) {
                (Some(end), start) => format_duration(start, end),
                _ => String::new(),
            };
            text.push_str(&format!("\n  {}. {ago} \u{2014} {icon} {}{duration}", i + 1, run.status));
        }
    }

    let eff_thread_id = effective_thread_id(&msg);
    send_html_reply(bot, msg.chat.id, eff_thread_id, &text).await?;
    Ok(())
}
```

- [ ] **Step 7: Add helper `format_relative_time` and `format_duration`**

These may already exist in handler.rs. If not, add them:

```rust
/// Format an ISO 8601 timestamp as a relative time string (e.g. "2m ago", "3h ago").
fn format_relative_time(iso: &str) -> String {
    let Ok(ts) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let diff = chrono::Utc::now() - ts.to_utc();
    let secs = diff.num_seconds();
    if secs < 60 { return format!("{secs}s ago"); }
    if secs < 3600 { return format!("{}m ago", secs / 60); }
    if secs < 86400 { return format!("{}h ago", secs / 3600); }
    format!("{}d ago", secs / 86400)
}

/// Format duration between two ISO 8601 timestamps (e.g. " (12s)", " (2m 30s)").
fn format_duration(start_iso: &str, end_iso: &str) -> String {
    let Ok(start) = chrono::DateTime::parse_from_rfc3339(start_iso) else { return String::new() };
    let Ok(end) = chrono::DateTime::parse_from_rfc3339(end_iso) else { return String::new() };
    let secs = (end - start).num_seconds();
    if secs < 60 {
        format!(" ({secs}s)")
    } else {
        format!(" ({}m {}s)", secs / 60, secs % 60)
    }
}
```

- [ ] **Step 8: Build workspace**

Run: `cargo build --workspace`
Expected: compiles cleanly.

- [ ] **Step 9: Commit**

```bash
git add crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs
git commit -m "feat(cron): add /cron telegram command (list + detail)"
```

---

### Task 7: Update SKILL.md

**Files:**
- Modify: `skills/rightcron/SKILL.md`

- [ ] **Step 1: Add `cron_trigger` section**

After the "Removing a Cron Job" section and before "Listing Current Cron Jobs", add:

```markdown
## Triggering a Cron Job Manually

Use the `cron_trigger` MCP tool to run a job immediately:

```
cron_trigger(job_name: "health-check")
```

The job is queued and executes on the next engine tick (≤30s). Lock check still applies — if the job is currently running, the trigger is skipped. The result is delivered through the normal delivery loop.

Confirm: "Job triggered. Execution starts within ~30 seconds."
```

- [ ] **Step 2: Add `cron_trigger` to the Parameters table**

Add a row to the Parameters table:

```markdown
| `job_name` (trigger) | string | Yes | - | Job name to trigger for immediate execution. |
```

- [ ] **Step 3: Add trigger to the Constraints section**

Add after item 2:

```markdown
3. **Manual triggers**: `cron_trigger` queues the job; it runs on the next 60-second engine tick. If the job is locked (still running from a previous invocation), the trigger is skipped.
```

- [ ] **Step 4: Commit**

```bash
git add skills/rightcron/SKILL.md
git commit -m "docs: add cron_trigger to SKILL.md"
```

---

### Task 8: Build full workspace and run all tests

**Files:** None (verification only)

- [ ] **Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: Build release-like**

Run: `cargo build --workspace`
Expected: Clean build.

---

## Self-Review

**Spec coverage check:**
1. `/cron` Telegram list — Task 6, Step 5 ✅
2. `/cron <job-name>` detail — Task 6, Step 6 ✅
3. MCP `cron_trigger` — Task 5 ✅
4. Migration V7 — Task 1 ✅
5. Cron engine unified trigger — Task 4 ✅
6. SKILL.md update — Task 7 ✅
7. Comprehensive tests — Tasks 1, 3, 4 ✅

**Placeholder scan:** No TBD/TODO found. All code blocks complete.

**Type consistency check:**
- `CronSpec.triggered_at: Option<String>` — used in Task 3 (definition), Task 4 (engine check)
- `CronSpecDetail` — defined in Task 3, used in Task 6
- `CronRunSummary` — defined in Task 3, used in Task 6
- `trigger_spec()` → `Result<String, String>` — defined Task 3, called Tasks 4, 5
- `clear_triggered_at()` → `Result<(), String>` — defined Task 3, called Task 4
- `describe_schedule()` → `String` — defined Task 3, called Task 6
- `CronTriggerParams` — defined Task 5, used in both MCP servers

All consistent.
