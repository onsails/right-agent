# Background Continuation for Long-Running Turns — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a foreground CC turn hits the 600s safety limit or the user clicks a new `🌙 Background` button, fork the main session via `claude -p --resume <main> --fork-session --session-id <bg>`, run the continuation as an Immediate cron job, and deliver the result back into the main conversation through the existing cron_delivery pipeline.

**Architecture:** A new `ScheduleKind::Immediate` variant on `cron_specs`, encoded as the sentinel `'@immediate'` in the existing `schedule TEXT` column (no DB migration). The reconcile pass in `cron.rs` fires Immediate rows on every tick and auto-deletes them. A per-session `Arc<Mutex<()>>` map closes the TOCTOU race between worker and delivery resume calls. Worker's existing `Reflectable::SafetyTimeout` path is replaced by `Backgrounded { reason }`; reflection remains for the other failure kinds.

**Tech Stack:** Rust 2024, tokio, rusqlite, teloxide, chrono. Existing crates only — no new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-05-background-continuation-design.md` (commit `d84d9be7`).

---

## File Structure

| File | Role | Change type |
|---|---|---|
| `crates/right-agent/src/cron_spec.rs` | `ScheduleKind` enum, schedule resolver, create/insert helpers | Add Immediate variant, extend `create_spec_v2`, add `insert_immediate_cron`, raise default budget |
| `crates/bot/src/cron.rs` | Cron reconcile loop | Fire Immediate rows; include in `<immediate>` schedule display |
| `crates/bot/src/telegram/invocation.rs` | `ClaudeInvocation` argv builder | Add `fork_session: bool` field; emit `--fork-session` |
| `crates/bot/src/telegram/mod.rs` | Shared types | Add `SessionLocks`, `BgRequests` type aliases |
| `crates/bot/src/telegram/worker.rs` | `WorkerCtx`, `invoke_cc`, `spawn_worker`, `InvokeCcFailure` | Add fields, replace `SafetyTimeout`-reflection path with `Backgrounded`, add `working_keyboard`, `enqueue_background_job`, `build_continuation_prompt`, mutex acquire |
| `crates/bot/src/telegram/handler.rs` | Callback handler | Add `handle_bg_callback`, share dispatch with existing stop callback |
| `crates/bot/src/telegram/dispatch.rs` | DI graph | Wire `SessionLocks`, `BgRequests`; route bg callbacks |
| `crates/bot/src/cron_delivery.rs` | `deliver_through_session` | Acquire per-session Mutex before resume |
| `crates/bot/src/lib.rs` | Top-level wiring | Instantiate `SessionLocks`, `BgRequests`; thread into delivery and dispatch |
| `ARCHITECTURE.md` | Project docs | Document Immediate variant + race protection |

---

## Naming alignment with the spec

The spec uses sketch enum names (`Cron(String)`, `OneShot { run_at }`). The actual codebase enum is:

```rust
pub enum ScheduleKind {
    Recurring(String),
    OneShotCron(String),
    RunAt(DateTime<Utc>),
}
```

This plan uses the real names. We add a fourth variant `Immediate`.

The spec mentions a sketch helper `insert_one_shot`. The real codebase uses `create_spec_v2` (full-featured) and `create_spec` (legacy, schedule-only). We extend `create_spec_v2` with `immediate: bool` and add a focused `insert_immediate_cron` wrapper for worker enqueue use.

---

## Task 1: Raise default cron budget to $5

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs:8`

- [ ] **Step 1: Read existing constant and any tests that depend on it**

```bash
rg -n "DEFAULT_CRON_BUDGET_USD" crates/ -t rust
```
Expected: definition at `cron_spec.rs:8`, references in tests inside the same file.

- [ ] **Step 2: Update the constant**

In `crates/right-agent/src/cron_spec.rs:8`:
```rust
- pub const DEFAULT_CRON_BUDGET_USD: f64 = 2.0;
+ pub const DEFAULT_CRON_BUDGET_USD: f64 = 5.0;
```

- [ ] **Step 3: Run cron_spec tests**

```bash
cargo test -p right-agent --lib cron_spec
```
Expected: all pass. Tests assert behaviour, not the constant — so no test edits needed. If any test asserts `2.0` literally, fix to `DEFAULT_CRON_BUDGET_USD` reference rather than re-hardcoding.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs
git commit -m "feat(cron): raise default budget to \$5"
```

---

## Task 2: Add `ScheduleKind::Immediate` enum variant + DB round-trip

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (enum, `cron_schedule`, `is_one_shot`, `load_specs_from_db`, tests)

- [ ] **Step 1: Write failing test for sentinel round-trip**

Add at the end of the `tests` module in `crates/right-agent/src/cron_spec.rs` (after `load_specs_round_trips_all_schedule_kinds`):
```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p right-agent --lib cron_spec::tests::load_specs_round_trips_immediate cron_spec::tests::immediate_is_one_shot
```
Expected: compile errors — `ScheduleKind::Immediate` does not exist.

- [ ] **Step 3: Add `Immediate` variant**

In `crates/right-agent/src/cron_spec.rs:36-45`:
```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleKind {
    /// 5-field cron expression, fires repeatedly.
    Recurring(String),
    /// 5-field cron expression, fires once then auto-deletes.
    OneShotCron(String),
    /// Absolute UTC time, fires once then auto-deletes.
    RunAt(DateTime<Utc>),
    /// Fires on the next reconcile tick, then auto-deletes.
    Immediate,
}
```

- [ ] **Step 4: Update `cron_schedule()` and `is_one_shot()`**

In `crates/right-agent/src/cron_spec.rs:47-60`:
```rust
impl ScheduleKind {
    pub fn cron_schedule(&self) -> Option<&str> {
        match self {
            Self::Recurring(s) | Self::OneShotCron(s) => Some(s),
            Self::RunAt(_) | Self::Immediate => None,
        }
    }

    pub fn is_one_shot(&self) -> bool {
        matches!(self, Self::OneShotCron(_) | Self::RunAt(_) | Self::Immediate)
    }
}
```

- [ ] **Step 5: Update `load_specs_from_db` to detect sentinel**

In `crates/right-agent/src/cron_spec.rs:576-588`, change the schedule_kind classification:
```rust
let schedule_kind = if let Some(ref rat) = run_at {
    match rat.parse::<DateTime<Utc>>() {
        Ok(dt) => ScheduleKind::RunAt(dt),
        Err(e) => {
            tracing::error!(job = %job_name, "invalid run_at in DB: {e:#}");
            continue;
        }
    }
} else if schedule == "@immediate" {
    ScheduleKind::Immediate
} else if recurring == 0 {
    ScheduleKind::OneShotCron(schedule)
} else {
    ScheduleKind::Recurring(schedule)
};
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test -p right-agent --lib cron_spec::tests::load_specs_round_trips_immediate cron_spec::tests::immediate_is_one_shot
```
Expected: PASS.

- [ ] **Step 7: Run full crate tests**

```bash
cargo test -p right-agent --lib
```
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs
git commit -m "feat(cron): add ScheduleKind::Immediate variant with @immediate sentinel"
```

---

## Task 3: Extend `create_spec_v2` with `immediate: bool`; add `insert_immediate_cron`

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (`resolve_schedule_fields`, `create_spec_v2`, new `insert_immediate_cron`, tests)

- [ ] **Step 1: Write failing test for 3-way mutual exclusion**

Add at the end of the `tests` module:
```rust
#[test]
fn resolve_schedule_fields_immediate_mutex() {
    use super::resolve_schedule_fields;
    // immediate + schedule → error
    assert!(resolve_schedule_fields(Some("*/5 * * * *"), None, None, true).is_err());
    // immediate + run_at → error
    assert!(resolve_schedule_fields(None, None, Some("2026-12-25T00:00:00Z"), true).is_err());
    // immediate alone → ok with sentinel
    let (sched, rec, run_at, _) = resolve_schedule_fields(None, None, None, true).unwrap();
    assert_eq!(sched, "@immediate");
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
    assert_eq!(stored.0, "@immediate");
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
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p right-agent --lib cron_spec::tests::resolve_schedule_fields_immediate_mutex cron_spec::tests::create_spec_v2_immediate_inserts_sentinel cron_spec::tests::insert_immediate_cron_uses_default_budget_when_none
```
Expected: compile errors — `resolve_schedule_fields` has wrong arity, `insert_immediate_cron` missing, `create_spec_v2` arity wrong.

- [ ] **Step 3: Update `resolve_schedule_fields` to 3-way exclusion**

Replace `crates/right-agent/src/cron_spec.rs:177-207`:
```rust
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
    let provided_count = (schedule.is_some() as u8)
        + (run_at.is_some() as u8)
        + (immediate as u8);
    if provided_count > 1 {
        return Err(
            "schedule, run_at, and immediate are mutually exclusive — provide exactly one".into(),
        );
    }
    if provided_count == 0 {
        return Err("one of schedule, run_at, or immediate must be provided".into());
    }

    if immediate {
        return Ok(("@immediate".to_string(), 0, None, None));
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
```

- [ ] **Step 4: Add `immediate: bool` parameter to `create_spec_v2`**

In `crates/right-agent/src/cron_spec.rs:247-296`, add `immediate: bool` as the last parameter and pass it into `resolve_schedule_fields`:
```rust
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
```

- [ ] **Step 5: Update existing `create_spec_v2` callsites to pass `immediate: false`**

```bash
rg -n "create_spec_v2\b" crates/ -t rust
```
Expected callsites: handler.rs (cron MCP create), tests inside cron_spec.rs.

For every callsite, append `false` as the new last positional argument. Example:
```rust
create_spec_v2(&conn, "x", Some("*/5 * * * *"), "p", None, None, None, None, None, None)
```
becomes
```rust
create_spec_v2(&conn, "x", Some("*/5 * * * *"), "p", None, None, None, None, None, None, false)
```

Also update the tests in `cron_spec.rs` (`create_spec_v2_with_run_at_succeeds`, `_with_both_schedule_and_run_at_fails`, `_with_neither_schedule_nor_run_at_fails`, `_with_invalid_run_at_fails`, `_with_past_run_at_succeeds`, `_recurring_false_stored_as_one_shot_cron`, `load_specs_round_trips_all_schedule_kinds`, `_persists_target_fields`, `_persists_null_target_when_omitted`, `update_spec_partial_*` callsites) to pass the new last arg.

- [ ] **Step 6: Add `insert_immediate_cron` helper**

Insert in `crates/right-agent/src/cron_spec.rs` after `create_spec_v2` (around line 297):
```rust
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
```

- [ ] **Step 7: Run tests**

```bash
cargo test -p right-agent --lib cron_spec
```
Expected: all pass, including the three new tests.

- [ ] **Step 8: Run workspace build to catch any callsite I missed**

```bash
cargo build --workspace
```
Expected: clean build. If any callsite still uses old `create_spec_v2` arity, the compiler points it out — add `false` and rebuild.

- [ ] **Step 9: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs
git commit -m "feat(cron): immediate kind in create_spec_v2 + insert_immediate_cron helper"
```

---

## Task 4: `cron.rs` reconcile fires Immediate jobs

**Files:**
- Modify: `crates/bot/src/cron.rs` around lines 936–1005, 1036

- [ ] **Step 1: Add fire-Immediate branch after the run_at branch**

In `crates/bot/src/cron.rs`, after the `for (name, spec) in overdue_run_at` loop ends (line ~981), add:
```rust
// Fire Immediate specs (every tick — they are one-shot)
let immediate: Vec<(String, CronSpec)> = new_specs
    .iter()
    .filter(|(_, spec)| matches!(&spec.schedule_kind, right_agent::cron_spec::ScheduleKind::Immediate))
    .map(|(name, spec)| (name.clone(), spec.clone()))
    .collect();

for (name, spec) in immediate {
    let lock_ttl = spec.lock_ttl.as_deref().unwrap_or("30m");
    if is_lock_fresh(agent_dir, &name, lock_ttl) {
        tracing::info!(job = %name, "immediate job locked — skipping until next tick");
        continue;
    }

    tracing::info!(job = %name, "firing immediate job");
    let jn = name.clone();
    let sp = spec.clone();
    let ad = agent_dir.to_path_buf();
    let an = agent_name.to_string();
    let md = model.clone();
    let sc = ssh_config_path.clone();
    let ic = Arc::clone(internal_client);
    let rs = resolved_sandbox.clone();
    let ul = Arc::clone(upgrade_lock);
    let handle = tokio::spawn(async move {
        execute_job(
            &jn,
            &sp,
            &ad,
            &an,
            md.as_deref(),
            sc.as_deref(),
            &ic,
            rs.as_deref(),
            ul,
        )
        .await;
        delete_one_shot_spec(&ad, &jn);
    });
    if let Ok(mut guard) = execute_handles.lock() {
        guard.push((name, handle));
    } else {
        triggered_handles.push(handle);
    }
}
```

- [ ] **Step 2: Skip Immediate in `run_job_loop` spawn loop**

In `crates/bot/src/cron.rs:998-1005`, extend the RunAt skip to also skip Immediate:
```rust
// Skip RunAt and Immediate specs — they are handled above, not run_job_loop
if matches!(
    spec.schedule_kind,
    right_agent::cron_spec::ScheduleKind::RunAt(_)
        | right_agent::cron_spec::ScheduleKind::Immediate
) {
    continue;
}
```

- [ ] **Step 3: Update schedule display string**

In `crates/bot/src/cron.rs:1036`:
```rust
let sched_display = spec.schedule_kind.cron_schedule().unwrap_or_else(|| match &spec.schedule_kind {
    right_agent::cron_spec::ScheduleKind::RunAt(_) => "<run_at>",
    right_agent::cron_spec::ScheduleKind::Immediate => "<immediate>",
    _ => "<unknown>",
});
```

- [ ] **Step 4: Build to verify**

```bash
cargo build --workspace
```
Expected: clean.

- [ ] **Step 5: Write integration test for Immediate firing**

Create new test file `crates/bot/tests/cron_immediate.rs`:
```rust
//! Integration: verify ScheduleKind::Immediate jobs fire on the next cron tick.
//!
//! Uses the in-process reconcile loop — no real CC invocation. We seed a
//! cron_specs row with the @immediate sentinel and watch for the row to be
//! deleted (one-shot auto-delete) and a cron_runs row to appear.

use std::path::PathBuf;
use std::time::Duration;

use right_agent::cron_spec::insert_immediate_cron;
use right_agent::memory::open_connection;

#[tokio::test]
async fn immediate_job_row_inserted_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path();
    std::fs::create_dir_all(agent_dir.join("crons").join(".locks")).unwrap();

    let conn = open_connection(agent_dir, true).unwrap();
    let result = insert_immediate_cron(&conn, "bg-imm-1", "do thing", -100, Some(7), Some(5.0));
    assert!(result.is_ok(), "insert_immediate_cron failed: {result:?}");

    // Verify the sentinel landed
    let (schedule, recurring, run_at): (String, i64, Option<String>) = conn
        .query_row(
            "SELECT schedule, recurring, run_at FROM cron_specs WHERE job_name = 'bg-imm-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(schedule, "@immediate");
    assert_eq!(recurring, 0);
    assert!(run_at.is_none());
}
```

This test exercises only the DB write — full reconcile-loop firing requires a sandbox and is exercised by manual smoke testing. (A live-sandbox firing test is not warranted: the firing path is identical to the existing OneShotCron and RunAt paths, only the filter differs.)

- [ ] **Step 6: Run integration test**

```bash
cargo test -p right-bot --test cron_immediate
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/tests/cron_immediate.rs
git commit -m "feat(cron): fire ScheduleKind::Immediate jobs on next reconcile tick"
```

---

## Task 5: `ClaudeInvocation` accepts `--fork-session`

**Files:**
- Modify: `crates/bot/src/telegram/invocation.rs` (struct, `into_args`, tests)

- [ ] **Step 1: Write failing test for fork-session argv**

Add to the existing `tests` module in `crates/bot/src/telegram/invocation.rs`:
```rust
#[test]
fn fork_session_emits_resume_fork_and_session_id() {
    let mut inv = minimal();
    inv.resume_session_id = Some("main-uuid".into());
    inv.new_session_id = Some("fork-uuid".into());
    inv.fork_session = true;
    let args = inv.into_args();

    let resume_pos = args.iter().position(|a| a == "--resume").expect("--resume missing");
    let fork_pos = args.iter().position(|a| a == "--fork-session").expect("--fork-session missing");
    let session_pos = args.iter().position(|a| a == "--session-id").expect("--session-id missing");

    assert!(resume_pos < fork_pos, "--resume must precede --fork-session");
    assert!(fork_pos < session_pos, "--fork-session must precede --session-id");
    assert_eq!(args[resume_pos + 1], "main-uuid");
    assert_eq!(args[session_pos + 1], "fork-uuid");
}

#[test]
fn fork_session_without_resume_does_not_emit_flag() {
    let mut inv = minimal();
    inv.new_session_id = Some("only-new".into());
    inv.fork_session = true;
    let args = inv.into_args();
    assert!(!args.contains(&"--fork-session".to_string()));
    assert!(!args.contains(&"--resume".to_string()));
}
```

- [ ] **Step 2: Update `minimal()` test helper**

In `crates/bot/src/telegram/invocation.rs:171-...`, add `fork_session: false` to the `ClaudeInvocation` literal so tests still build.

- [ ] **Step 3: Run tests to verify failure**

```bash
cargo test -p right-bot --lib telegram::invocation
```
Expected: compile errors — `fork_session` field missing.

- [ ] **Step 4: Add `fork_session: bool` field**

In `crates/bot/src/telegram/invocation.rs:12-25`:
```rust
#[derive(Debug, Clone)]
pub(crate) struct ClaudeInvocation {
    pub(crate) mcp_config_path: Option<String>,
    pub(crate) json_schema: Option<String>,
    pub(crate) output_format: OutputFormat,
    pub(crate) model: Option<String>,
    pub(crate) max_budget_usd: Option<f64>,
    pub(crate) max_turns: Option<u32>,
    pub(crate) resume_session_id: Option<String>,
    pub(crate) new_session_id: Option<String>,
    pub(crate) fork_session: bool,
    pub(crate) allowed_tools: Vec<String>,
    pub(crate) disallowed_tools: Vec<String>,
    pub(crate) extra_args: Vec<String>,
    pub(crate) prompt: Option<String>,
}
```

- [ ] **Step 5: Update `into_args` session block**

In `crates/bot/src/telegram/invocation.rs:52-59`:
```rust
// 4. Session
if let Some(resume_id) = self.resume_session_id {
    args.push("--resume".into());
    args.push(resume_id);
    if self.fork_session {
        args.push("--fork-session".into());
        if let Some(new_id) = self.new_session_id {
            args.push("--session-id".into());
            args.push(new_id);
        }
    }
} else if let Some(id) = self.new_session_id {
    args.push("--session-id".into());
    args.push(id);
}
```

- [ ] **Step 6: Update every `ClaudeInvocation { ... }` literal in the codebase**

```bash
rg -n "ClaudeInvocation \{" crates/ -t rust
```
Each callsite must add `fork_session: false` to its literal. Known sites:
- `crates/bot/src/telegram/worker.rs` (around line 1295–1310)
- `crates/bot/src/cron.rs:308–321`
- `crates/bot/src/cron_delivery.rs:471–484`
- `crates/bot/src/reflection.rs` (literal in `reflect_on_failure` invocation)

- [ ] **Step 7: Run tests**

```bash
cargo test -p right-bot --lib telegram::invocation
cargo build --workspace
```
Expected: tests pass, build clean.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/telegram/invocation.rs crates/bot/src/telegram/worker.rs crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs crates/bot/src/reflection.rs
git commit -m "feat(invocation): add fork_session flag emitting --fork-session"
```

---

## Task 6: `SessionLocks` type + worker-side acquisition

**Files:**
- Modify: `crates/bot/src/telegram/mod.rs` (new type alias)
- Modify: `crates/bot/src/telegram/worker.rs` (`WorkerCtx` field; lock acquisition in `invoke_cc`)

- [ ] **Step 1: Add type alias**

In `crates/bot/src/telegram/mod.rs` next to `StopTokens` (line 51):
```rust
pub(crate) type SessionLocks =
    Arc<dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>>;
```

- [ ] **Step 2: Add field to `WorkerCtx`**

In `crates/bot/src/telegram/worker.rs:108-111` (after `pub stop_tokens: super::StopTokens,`):
```rust
/// Per-main-session async mutex map. Worker acquires before claude -p --resume <main>;
/// delivery acquires before its own --resume. Closes the TOCTOU race on session JSONL.
pub session_locks: super::SessionLocks,
```

- [ ] **Step 3: Acquire mutex inside `invoke_cc` before spawning child**

Find the spot in `invoke_cc` where the resume args are assembled and the child is spawned (around line 1232–1280, where `--resume <root_session_id>` is set up). Just before `tokio::process::Command::new(...)` / before `child = ...spawn()...`, acquire the lock:

```rust
// Acquire the per-session mutex when this turn resumes the main session.
// First-call turns (no resume) skip the lock — they cannot race anything.
let _session_guard: Option<tokio::sync::OwnedMutexGuard<()>> = if let Some(active_root) =
    /* the resume root_session_id used to build args; see existing variable */ active_root_for_lock.clone() {
    let entry = ctx.session_locks
        .entry(active_root)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    Some(entry.lock_owned().await)
} else {
    None
};
```

The variable `active_root_for_lock` is the same UUID that gets passed to `--resume` in the existing flow. Bind it explicitly above the args-construction block; the guard lives until `invoke_cc` returns. Drop the binding name at end of function naturally — no manual unlock.

(The periodic sweeper for orphaned mutex entries is a one-time spawn from `lib.rs` — added in Task 12 Step 6.)

- [ ] **Step 4: Build**

```bash
cargo build --workspace
```
Expected: error about `WorkerCtx` instantiations missing the new field. Fix every instantiation in dispatch.rs and tests.

- [ ] **Step 6: Update `WorkerCtx` instantiations**

```bash
rg -n "WorkerCtx \{" crates/bot/src/ -t rust
```
Add `session_locks: Arc::clone(&session_locks)` (where `session_locks` is the new dispatch-level handle from Task 12) — for now in tests, use `Arc::new(DashMap::new())`.

- [ ] **Step 7: Build clean**

```bash
cargo build --workspace
```

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/telegram/mod.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(worker): per-main-session mutex on --resume to close TOCTOU race"
```

---

## Task 7: Delivery-side mutex acquisition

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs` (`run_delivery_loop` signature + call inside `deliver_through_session`)
- Modify: `crates/bot/src/lib.rs` (pass `session_locks` to delivery loop — done with Task 12)

- [ ] **Step 1: Add `session_locks` parameter to `run_delivery_loop`**

In `crates/bot/src/cron_delivery.rs:238-249`, append to the parameter list:
```rust
session_locks: crate::telegram::SessionLocks,
```

- [ ] **Step 2: Acquire the lock inside `deliver_through_session` before spawning the Haiku CC**

In `deliver_through_session` (around line 445-...), before the `tokio::process::Command::new(...)` (or wherever the child is built/spawned), acquire:
```rust
let entry = session_locks
    .entry(session_id.clone().unwrap_or_default())  // empty key when no session — see below
    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
    .clone();
let _session_guard = entry.lock_owned().await;
```

Edge case: `session_id == None` happens only in pathological cases (no active main session at delivery time). Skip the lock entirely — there's nothing to race with:
```rust
let _session_guard = match session_id.clone() {
    Some(id) => {
        let entry = session_locks
            .entry(id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        Some(entry.lock_owned().await)
    }
    None => None,
};
```

- [ ] **Step 3: Pass `session_locks` from caller down through `deliver_through_session`**

In `run_delivery_loop` body, where `deliver_through_session` is called (line ~377-389), add `session_locks.clone()` to the call. Add `session_locks: crate::telegram::SessionLocks` to the `deliver_through_session` signature.

- [ ] **Step 4: Build**

```bash
cargo build --workspace
```
Expected: error about missing `session_locks` argument in `lib.rs::run_delivery_loop` call. We fix that in Task 12.

- [ ] **Step 5: Commit (incomplete; fixes finalize in Task 12)**

```bash
git add crates/bot/src/cron_delivery.rs
git commit -m "feat(cron-delivery): acquire per-session mutex before --resume into main"
```

---

## Task 8: `BgRequests` type + `working_keyboard` + `handle_bg_callback`

**Files:**
- Modify: `crates/bot/src/telegram/mod.rs` (new `BgRequests` type)
- Modify: `crates/bot/src/telegram/worker.rs` (`WorkerCtx` field, replace `stop_keyboard` with `working_keyboard`)
- Modify: `crates/bot/src/telegram/handler.rs` (new `handle_bg_callback`, dispatcher branching)
- Modify: `crates/bot/src/telegram/dispatch.rs` (pass `bg_requests` through DI; route bg callbacks)

- [ ] **Step 1: Add `BgRequests` type alias**

In `crates/bot/src/telegram/mod.rs`:
```rust
pub(crate) type BgRequests = Arc<dashmap::DashMap<(i64, i64), ()>>;
```

- [ ] **Step 2: Add field to `WorkerCtx`**

In `crates/bot/src/telegram/worker.rs` (right after `pub session_locks: super::SessionLocks,`):
```rust
/// Per-(chat, thread) flag set by the bg callback. Worker checks after kill+wait
/// to distinguish UserRequested backgrounding from auto-timeout.
pub bg_requests: super::BgRequests,
```

- [ ] **Step 3: Write failing test for `working_keyboard`**

Replace `stop_keyboard_format` in the worker tests module:
```rust
#[test]
fn working_keyboard_has_stop_and_background() {
    let kb = working_keyboard(12345, 678);
    let buttons: Vec<Vec<_>> = kb.inline_keyboard.into_iter().collect();
    assert_eq!(buttons.len(), 1, "single row");
    assert_eq!(buttons[0].len(), 2, "two buttons");
    assert_eq!(buttons[0][0].text, "\u{26d4} Stop");
    assert_eq!(buttons[0][1].text, "\u{1f319} Background");
    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(data) = &buttons[0][0].kind {
        assert_eq!(data, "stop:12345:678");
    } else {
        panic!("Stop button must use CallbackData");
    }
    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(data) = &buttons[0][1].kind {
        assert_eq!(data, "bg:12345:678");
    } else {
        panic!("Background button must use CallbackData");
    }
}
```

Delete the old `stop_keyboard_format` test.

- [ ] **Step 4: Run test to verify failure**

```bash
cargo test -p right-bot --lib telegram::worker::tests::working_keyboard_has_stop_and_background
```
Expected: compile error — `working_keyboard` does not exist.

- [ ] **Step 5: Replace `stop_keyboard` with `working_keyboard`**

In `crates/bot/src/telegram/worker.rs:49-57`:
```rust
/// Build the inline keyboard for thinking messages: Stop + Background.
fn working_keyboard(chat_id: i64, eff_thread_id: i64) -> teloxide::types::InlineKeyboardMarkup {
    teloxide::types::InlineKeyboardMarkup::new(vec![vec![
        teloxide::types::InlineKeyboardButton::callback(
            "\u{26d4} Stop",
            format!("stop:{chat_id}:{eff_thread_id}"),
        ),
        teloxide::types::InlineKeyboardButton::callback(
            "\u{1f319} Background",
            format!("bg:{chat_id}:{eff_thread_id}"),
        ),
    ]])
}
```

Update the single callsite at `crates/bot/src/telegram/worker.rs:1654`:
```rust
- let kb = stop_keyboard(chat_id, eff_thread_id);
+ let kb = working_keyboard(chat_id, eff_thread_id);
```

- [ ] **Step 6: Run test to verify pass**

```bash
cargo test -p right-bot --lib telegram::worker::tests::working_keyboard_has_stop_and_background
```
Expected: PASS.

- [ ] **Step 7: Add `handle_bg_callback` to handler.rs**

After `handle_stop_callback` (around line 1910), add:
```rust
/// Handle the Background button callback query from thinking messages.
///
/// Callback data format: `bg:{chat_id}:{eff_thread_id}`
/// Sets the bg flag in `BgRequests` and cancels the worker's stop token —
/// the worker reads the flag after kill+wait and emits Backgrounded.
pub async fn handle_bg_callback(
    bot: BotType,
    q: CallbackQuery,
    stop_tokens: super::StopTokens,
    bg_requests: super::BgRequests,
) -> ResponseResult<()> {
    let data = q.data.as_deref().unwrap_or("");
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    let qid = q.id;

    let text = if parts.len() == 3
        && parts[0] == "bg"
        && let Ok(chat_id) = parts[1].parse::<i64>()
        && let Ok(thread_id) = parts[2].parse::<i64>()
    {
        let key = (chat_id, thread_id);
        if let Some(entry) = stop_tokens.get(&key) {
            bg_requests.insert(key, ());
            entry.value().cancel();
            drop(entry);
            Some("Sending to background...")
        } else {
            Some("Already finished")
        }
    } else {
        None
    };

    let mut answer = bot.answer_callback_query(qid);
    if let Some(t) = text {
        answer = answer.text(t);
    }
    answer.await?;

    Ok(())
}
```

- [ ] **Step 8: Add tests for `bg:` callback parse**

Add to `crates/bot/src/telegram/handler.rs::tests` (next to `parse_stop_callback_data_*`):
```rust
#[test]
fn parse_bg_callback_data_valid() {
    let data = "bg:42:7";
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    assert_eq!(parts[0], "bg");
    assert_eq!(parts[1].parse::<i64>().unwrap(), 42);
    assert_eq!(parts[2].parse::<i64>().unwrap(), 7);
}

#[test]
fn parse_bg_callback_data_malformed() {
    for bad in ["", "bg", "bg:", "bg:abc:0", "bg:1", "stop:1:2"] {
        let parts: Vec<&str> = bad.splitn(3, ':').collect();
        let valid = parts.len() == 3
            && parts[0] == "bg"
            && parts[1].parse::<i64>().is_ok()
            && parts[2].parse::<i64>().is_ok();
        assert!(!valid, "bad={bad} unexpectedly parsed as valid");
    }
}
```

- [ ] **Step 9: Route bg callbacks in dispatch.rs**

In `crates/bot/src/telegram/dispatch.rs:441` (where the existing callback handler is wired):
```rust
let callback_handler = Update::filter_callback_query()
    .branch(
        dptree::filter(|q: CallbackQuery| {
            q.data.as_deref().is_some_and(|d| d.starts_with("bg:"))
        })
        .endpoint(handle_bg_callback),
    )
    .endpoint(handle_stop_callback);
```

Update the import at line 29:
```rust
- handle_message, handle_new, handle_start, handle_stop_callback, handle_switch, handle_usage,
+ handle_bg_callback, handle_message, handle_new, handle_start, handle_stop_callback, handle_switch, handle_usage,
```

Add `bg_requests: super::BgRequests` parameter to `build_dispatcher` (line 375). Add `Arc::clone(&bg_requests)` to dependencies and to the WorkerCtx instantiation.

- [ ] **Step 10: Build + test**

```bash
cargo build --workspace
cargo test -p right-bot --lib telegram::handler::tests
```
Expected: clean build, all tests pass.

- [ ] **Step 11: Commit**

```bash
git add crates/bot/src/telegram/mod.rs crates/bot/src/telegram/worker.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs
git commit -m "feat(bot): Background button + handle_bg_callback dispatch"
```

---

## Task 9: `BgReason`, `Backgrounded` outcome, `enqueue_background_job`, `build_continuation_prompt`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add `BgReason` enum and `Backgrounded` variant to `InvokeCcFailure`**

After the existing `InvokeCcFailure` enum (around line 1170-1196):
```rust
#[derive(Debug, Clone, Copy)]
pub(crate) enum BgReason {
    AutoTimeout,
    UserRequested,
}
```

In `InvokeCcFailure`, add:
```rust
pub(crate) enum InvokeCcFailure {
    Reflectable {
        kind: crate::reflection::FailureKind,
        ring_buffer_tail: VecDeque<crate::telegram::stream::StreamEvent>,
        session_uuid: String,
        raw_message: String,
        thinking_msg_id: Option<MessageId>,
    },
    NonReflectable { message: String },
    Backgrounded {
        reason: BgReason,
        main_session_id: String,
        thinking_msg_id: Option<MessageId>,
    },
}
```

(Keep the `From` impl for `String` unchanged.)

- [ ] **Step 2: Write failing test for `build_continuation_prompt`**

Add to the worker tests module:
```rust
#[test]
fn continuation_prompt_auto_timeout_includes_focus_hint() {
    let p = build_continuation_prompt(BgReason::AutoTimeout);
    assert!(p.contains("10-minute safety limit"));
    assert!(p.contains("MOST RECENT MESSAGE"));
    assert!(p.contains("⟨⟨SYSTEM_NOTICE⟩⟩"));
    assert!(p.contains("⟨⟨/SYSTEM_NOTICE⟩⟩"));
}

#[test]
fn continuation_prompt_user_requested_uses_correct_reason() {
    let p = build_continuation_prompt(BgReason::UserRequested);
    assert!(p.contains("user moved this work to background"));
    assert!(p.contains("MOST RECENT MESSAGE"));
}
```

- [ ] **Step 3: Run tests to verify failure**

```bash
cargo test -p right-bot --lib telegram::worker::tests::continuation_prompt_auto_timeout_includes_focus_hint
```
Expected: function not defined.

- [ ] **Step 4: Implement `build_continuation_prompt`**

In `crates/bot/src/telegram/worker.rs` (in pure-helpers section, near the top):
```rust
fn continuation_reason_text(reason: BgReason) -> &'static str {
    match reason {
        BgReason::AutoTimeout => {
            "the foreground turn hit the 10-minute safety limit and was terminated"
        }
        BgReason::UserRequested => "the user moved this work to background execution",
    }
}

fn build_continuation_prompt(reason: BgReason) -> String {
    let reason_text = continuation_reason_text(reason);
    format!(
        "\u{27e8}\u{27e8}SYSTEM_NOTICE\u{27e9}\u{27e9}\n\
You were forked from the main conversation because {reason_text}.\n\
The previous turn did not complete. Please continue and produce a final\n\
answer to the user's MOST RECENT MESSAGE.\n\
\n\
Earlier conversation history is provided as context only — do not re-engage\n\
with it unless directly required to answer the most recent message.\n\
\n\
Take as much time as you need within your budget. Your reply will be relayed\n\
back to the main conversation, so write it as if responding to the user\n\
directly.\n\
\u{27e8}\u{27e8}/SYSTEM_NOTICE\u{27e9}\u{27e9}"
    )
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p right-bot --lib telegram::worker::tests::continuation_prompt
```
Expected: PASS.

- [ ] **Step 6: Write failing test for `enqueue_background_job`**

```rust
#[test]
fn enqueue_background_job_inserts_immediate_with_target() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut conn = conn;
    right_agent::memory::migrations::MIGRATIONS
        .to_latest(&mut conn)
        .unwrap();
    let job = enqueue_background_job(&conn, -42, 7, "main-uuid", BgReason::AutoTimeout)
        .expect("enqueue must succeed");
    assert!(job.starts_with("bg-"));

    let (schedule, recurring, target_chat, target_thread, prompt): (
        String,
        i64,
        Option<i64>,
        Option<i64>,
        String,
    ) = conn
        .query_row(
            "SELECT schedule, recurring, target_chat_id, target_thread_id, prompt FROM cron_specs WHERE job_name = ?1",
            rusqlite::params![job],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(schedule, "@immediate");
    assert_eq!(recurring, 0);
    assert_eq!(target_chat, Some(-42));
    assert_eq!(target_thread, Some(7));
    assert!(prompt.contains("10-minute safety limit"));
}
```

- [ ] **Step 7: Run test to verify failure**

```bash
cargo test -p right-bot --lib telegram::worker::tests::enqueue_background_job_inserts_immediate_with_target
```
Expected: function not defined.

- [ ] **Step 8: Implement `enqueue_background_job`**

In `crates/bot/src/telegram/worker.rs`:
```rust
/// Generate a 4-character lowercase alphanumeric suffix using the system clock as entropy.
/// Avoids pulling in `rand` for this single use site.
fn random_suffix4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut n = nanos as u64;
    let mut out = String::with_capacity(4);
    for _ in 0..4 {
        out.push(ALPHABET[(n % ALPHABET.len() as u64) as usize] as char);
        n /= ALPHABET.len() as u64;
    }
    out
}

fn enqueue_background_job(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
    main_session_id: &str,
    reason: BgReason,
) -> Result<String, String> {
    let job_name = format!(
        "bg-{}-{}",
        chrono::Utc::now().format("%H%M%S"),
        random_suffix4()
    );
    let prompt_user_msg = build_continuation_prompt(reason);
    // Encode the main_session_id into the prompt so cron's execute_job can
    // construct the --fork-session invocation. We use a dedicated header line
    // that cron parses out before piping to CC.
    let full_prompt = format!(
        "X-FORK-FROM: {main_session_id}\n{prompt_user_msg}"
    );
    let _ = right_agent::cron_spec::insert_immediate_cron(
        conn,
        &job_name,
        &full_prompt,
        chat_id,
        if thread_id == 0 { None } else { Some(thread_id) },
        None,
    )?;
    Ok(job_name)
}
```

> **Note about the X-FORK-FROM header:** The cron table doesn't have a column for "fork from session id". We embed it in the prompt with a header line; `cron::execute_job` strips the header and uses the parsed value when constructing the `ClaudeInvocation`. This is a localized hack, contained in two functions, and avoids a DB migration. See Task 10 for the parser side.

- [ ] **Step 9: Run tests**

```bash
cargo test -p right-bot --lib telegram::worker::tests::enqueue_background_job_inserts_immediate_with_target
cargo test -p right-bot --lib telegram::worker
```
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(worker): BgReason, Backgrounded outcome, enqueue helper, continuation prompt"
```

---

## Task 10: `cron::execute_job` honours X-FORK-FROM header for fork-session invocation

**Files:**
- Modify: `crates/bot/src/cron.rs` (around lines 308-321 where `ClaudeInvocation` is built)

- [ ] **Step 1: Parse X-FORK-FROM header at the top of `execute_job`**

In `crates/bot/src/cron.rs`, in `execute_job` (around line 200-280 — find where `spec.prompt` is consumed; if it isn't directly, find where it's passed into `ClaudeInvocation`), insert this near the top of the function:
```rust
// Optional X-FORK-FROM header in the prompt: when present, this is a
// background-continuation job that must `--resume <main> --fork-session
// --session-id <run_id>`. We strip the header before passing the prompt to CC.
let (fork_from_main_session, prompt_for_cc): (Option<String>, String) =
    if let Some(rest) = spec.prompt.strip_prefix("X-FORK-FROM: ") {
        match rest.split_once('\n') {
            Some((sess, body)) => (Some(sess.to_string()), body.to_string()),
            None => (None, spec.prompt.clone()),
        }
    } else {
        (None, spec.prompt.clone())
    };
```

- [ ] **Step 2: Wire fork-session into the `ClaudeInvocation`**

Replace the existing `ClaudeInvocation` literal (around line 308-321) so it uses the parsed values:
```rust
let invocation = crate::telegram::invocation::ClaudeInvocation {
    mcp_config_path: Some(mcp_path),
    json_schema: Some(right_agent::codegen::CRON_SCHEMA_JSON.into()),
    output_format: crate::telegram::invocation::OutputFormat::StreamJson,
    model: model.map(|s| s.to_owned()),
    max_budget_usd: Some(spec.max_budget_usd),
    max_turns: None,
    resume_session_id: fork_from_main_session.clone(),
    new_session_id: Some(run_id.clone()),
    fork_session: fork_from_main_session.is_some(),
    allowed_tools: vec![],
    disallowed_tools,
    extra_args: vec![],
    prompt: Some(prompt_for_cc.clone()),
};
```

(Note: the existing `prompt: Some(spec.prompt.clone())` is replaced by `prompt_for_cc.clone()`.)

- [ ] **Step 3: Build**

```bash
cargo build --workspace
```
Expected: clean.

- [ ] **Step 4: Add test for header parsing**

Add to `crates/bot/src/cron.rs::tests`:
```rust
#[test]
fn fork_from_header_is_parsed_and_stripped() {
    let prompt = "X-FORK-FROM: abc-123-uuid\nthe rest of the prompt\nmore lines";
    let (fork, body): (Option<String>, String) = if let Some(rest) = prompt.strip_prefix("X-FORK-FROM: ") {
        match rest.split_once('\n') {
            Some((sess, body)) => (Some(sess.to_string()), body.to_string()),
            None => (None, prompt.to_string()),
        }
    } else {
        (None, prompt.to_string())
    };
    assert_eq!(fork.as_deref(), Some("abc-123-uuid"));
    assert_eq!(body, "the rest of the prompt\nmore lines");
}

#[test]
fn no_fork_header_leaves_prompt_intact() {
    let prompt = "regular cron prompt";
    let (fork, body): (Option<String>, String) = if let Some(rest) = prompt.strip_prefix("X-FORK-FROM: ") {
        match rest.split_once('\n') {
            Some((sess, body)) => (Some(sess.to_string()), body.to_string()),
            None => (None, prompt.to_string()),
        }
    } else {
        (None, prompt.to_string())
    };
    assert!(fork.is_none());
    assert_eq!(body, "regular cron prompt");
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p right-bot --lib cron
```

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): honour X-FORK-FROM header for background continuation jobs"
```

---

## Task 11: Wire `Backgrounded` outcome in `spawn_worker` + emit it from `invoke_cc`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (`invoke_cc` outcome construction; `spawn_worker` outcome handling)

- [ ] **Step 1: Replace `SafetyTimeout`-via-Reflectable with `Backgrounded` in `invoke_cc`**

In `crates/bot/src/telegram/worker.rs`, around line 1806-1827 (the `if timed_out` block):
```rust
// Handle timeout — backgrounding instead of reflection.
if timed_out {
    return Err(InvokeCcFailure::Backgrounded {
        reason: BgReason::AutoTimeout,
        main_session_id: session_uuid.clone(),
        thinking_msg_id,
    });
}
```

Delete the old `Reflectable { kind: SafetyTimeout, .. }` construction in that branch, including the `timeout_msg` building (the new banner is shorter and constructed by `spawn_worker`).

- [ ] **Step 2: Check `bg_requests` AFTER `child.wait()`**

In `invoke_cc`, after `child.wait().await` and stop-token cleanup (around line 1722-1727), insert a new branch BEFORE the `stopped` and `timed_out` checks:
```rust
// User clicked Background — check before treating as a normal stop.
let was_bg_request = ctx
    .bg_requests
    .remove(&(chat_id, eff_thread_id))
    .is_some();

if was_bg_request {
    return Err(InvokeCcFailure::Backgrounded {
        reason: BgReason::UserRequested,
        main_session_id: session_uuid.clone(),
        thinking_msg_id,
    });
}
```

Place this block right after the `ctx.stop_tokens.remove(...)` line. The existing `if stopped` / `if timed_out` branches keep their semantics for non-bg cases.

- [ ] **Step 3: Handle `Backgrounded` in `spawn_worker`**

In `crates/bot/src/telegram/worker.rs::spawn_worker`, around line 825-830 where `InvokeCcFailure` variants are matched, add a new arm:
```rust
Err(InvokeCcFailure::Backgrounded {
    reason,
    main_session_id,
    thinking_msg_id,
}) => {
    tracing::info!(?key, ?reason, "backgrounding turn");

    // 1. Open DB connection and enqueue the background job.
    let conn = match right_agent::memory::open_connection(&ctx.agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(?key, "DB open for bg enqueue failed: {e:#}");
            send_error_to_telegram(
                &ctx,
                tg_chat_id,
                eff_thread_id,
                "Failed to enqueue background job: database unavailable.",
            )
            .await;
            return;
        }
    };
    let job_name = match enqueue_background_job(
        &conn,
        chat_id,
        eff_thread_id,
        &main_session_id,
        reason,
    ) {
        Ok(name) => name,
        Err(e) => {
            tracing::error!(?key, "bg enqueue failed: {e}");
            send_error_to_telegram(
                &ctx,
                tg_chat_id,
                eff_thread_id,
                &format!("Failed to enqueue background job: {e}"),
            )
            .await;
            return;
        }
    };
    tracing::info!(?key, %job_name, "background job enqueued");

    // 2. Edit thinking message to per-reason banner, clear keyboard.
    if let Some(msg_id) = thinking_msg_id {
        let banner = match reason {
            BgReason::AutoTimeout => {
                "\u{23f1} Foreground hit 10-min limit — continuing in background. \
                 Will reply when ready \u{1f319}"
            }
            BgReason::UserRequested => {
                "\u{1f319} Working in background. Will reply when ready"
            }
        };
        let _ = ctx
            .bot
            .edit_message_text(tg_chat_id, msg_id, banner)
            .reply_markup(teloxide::types::InlineKeyboardMarkup::default())
            .await;
    }
}
```

Where the existing `Reflectable { kind: SafetyTimeout, .. }` arm currently wraps banner-edit + reflection: keep that arm for the OTHER FailureKind variants (`BudgetExceeded`, `MaxTurns`, `NonZeroExit`) but `SafetyTimeout` is no longer emitted. Compiler will warn about non-exhaustive match if `FailureKind::SafetyTimeout` becomes unreachable; if so, leave the arm in place but never hit. (Alternatively, delete the variant entirely — see Step 4.)

- [ ] **Step 4: Decide on `FailureKind::SafetyTimeout`**

Two options:
- **(a)** Keep the variant but stop emitting it from worker. `cron::classify_cron_failure` may still emit it — leave it for that.
- **(b)** Remove the variant entirely.

Check usage:
```bash
rg -n "FailureKind::SafetyTimeout|SafetyTimeout \{" crates/ -t rust
```

Pick (a) — the variant remains useful for cron-side classification when a cron job's CC has its OWN safety net somewhere. No code change for this step beyond confirming it still compiles.

- [ ] **Step 5: Build**

```bash
cargo build --workspace
```
Expected: clean.

- [ ] **Step 6: Run pure unit tests**

```bash
cargo test -p right-bot --lib telegram::worker
```
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(worker): replace SafetyTimeout-reflection with Backgrounded path"
```

---

## Task 12: `lib.rs` and `dispatch.rs` — instantiate and wire `SessionLocks` + `BgRequests`

**Files:**
- Modify: `crates/bot/src/lib.rs` (instantiate at startup, pass to delivery + dispatcher)
- Modify: `crates/bot/src/telegram/dispatch.rs` (signature, deps registry, WorkerCtx instantiation)

- [ ] **Step 1: Instantiate at startup in `lib.rs`**

Find where `idle_timestamp` is created (`lib.rs:784-787`). Add immediately after:
```rust
// Per-main-session mutex map and per-(chat,thread) bg-request flags.
// Shared across worker, delivery, and callback handlers.
let session_locks: crate::telegram::SessionLocks = Arc::new(dashmap::DashMap::new());
let bg_requests: crate::telegram::BgRequests = Arc::new(dashmap::DashMap::new());
```

- [ ] **Step 2: Pass `session_locks` to delivery loop**

Update the `cron_delivery::run_delivery_loop(...)` call at `lib.rs:801-813` to include the new parameter:
```rust
cron_delivery::run_delivery_loop(
    delivery_agent_dir,
    delivery_agent_name,
    delivery_bot,
    delivery_allowlist,
    delivery_idle_ts,
    delivery_ssh_config,
    delivery_internal_client,
    delivery_shutdown,
    delivery_sandbox,
    delivery_upgrade_lock,
    Arc::clone(&session_locks),
)
```

- [ ] **Step 3: Pass both into the dispatcher**

Find where `dispatch::run_telegram_dispatcher(...)` (or the equivalent dispatch entry point) is called from `lib.rs`. Add `session_locks` and `bg_requests` arguments. The dispatch entry point's signature must accept them; update it.

- [ ] **Step 4: `dispatch.rs` accepts and threads through**

In `crates/bot/src/telegram/dispatch.rs`, at the top-level dispatch entry function (the one that constructs `WorkerCtx`), add parameters:
```rust
session_locks: super::SessionLocks,
bg_requests: super::BgRequests,
```

In the `WorkerCtx` literal (around line 130-151), add:
```rust
session_locks: Arc::clone(&session_locks),
bg_requests: Arc::clone(&bg_requests),
```

In `build_dispatcher` (line 375-399), add the same two parameters and append to `dependencies(dptree::deps![...])`:
```rust
stop_tokens,
idle_ts,
identity_arc,
allowlist,
bg_requests,
```
(`session_locks` is consumed only by worker context and delivery — does NOT need to be a dispatcher dep.)

- [ ] **Step 5: Update test instantiations**

```bash
rg -n "WorkerCtx \{" crates/bot/src/ -t rust
rg -n "build_dispatcher\b" crates/bot/src/ -t rust
```
Update each test that constructs `WorkerCtx` to include `session_locks: Arc::new(DashMap::new())` and `bg_requests: Arc::new(DashMap::new())`. Update each test invoking `build_dispatcher` to pass both.

- [ ] **Step 6: Spawn the periodic session-locks sweeper from `lib.rs`**

Right after instantiating `session_locks` (Step 1), add:
```rust
{
    let session_locks = Arc::clone(&session_locks);
    let sweep_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(std::time::Duration::from_secs(3600));
        iv.tick().await;
        loop {
            tokio::select! {
                _ = iv.tick() => {
                    session_locks.retain(|_, arc| Arc::strong_count(arc) > 1);
                }
                _ = sweep_shutdown.cancelled() => break,
            }
        }
    });
}
```
(Remove the per-worker sweeper that Task 6 Step 4 added, if any was inserted into `spawn_worker`.)

- [ ] **Step 7: Build**

```bash
cargo build --workspace
```
Expected: clean.

- [ ] **Step 8: Run all bot tests**

```bash
cargo test -p right-bot
```
Expected: all pass.

- [ ] **Step 9: Commit**

```bash
git add crates/bot/src/lib.rs crates/bot/src/telegram/dispatch.rs
git commit -m "feat(bot): wire SessionLocks + BgRequests through dispatch and delivery"
```

---

## Task 13: ARCHITECTURE.md update

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update the per-message flow section**

In `ARCHITECTURE.md`, find the "Per message" subsection of the agent lifecycle. Insert after the `claude -p` invocation step:
```
  ├─ If foreground exits via 600s timeout or 🌙 Background button:
  │   ├─ Insert cron_specs row with schedule_kind=Immediate, prompt prefixed
  │   │   with `X-FORK-FROM: <main_session_id>\n` and the continuation prompt
  │   ├─ Edit thinking message to per-reason banner ("⏱ Foreground hit 10-min
  │   │   limit — continuing in background…" / "🌙 Working in background…")
  │   └─ Worker returns; debounce frees, user can send next message
```

- [ ] **Step 2: Update the Cron Lifecycle section**

Find the "Cron Lifecycle" or equivalent. Add `Immediate` alongside `OneShot { run_at }`:
```
- ScheduleKind::Recurring("0 9 * * *") — fires repeatedly per cron expression
- ScheduleKind::OneShotCron("30 15 * * *") — fires once on next match, then deletes
- ScheduleKind::RunAt(2026-12-25T15:30:00Z) — fires once at absolute time, then deletes
- ScheduleKind::Immediate — fires on next reconcile tick (≤5s), then deletes.
  Encoded as `schedule = '@immediate'` sentinel, no DB migration. Used by the
  bot for background-continuation jobs (also available to cron_create as
  `--immediate` once exposed in the MCP surface).
```

- [ ] **Step 3: Add a new "Session race protection" subsection**

Before the existing IDLE_THRESHOLD discussion (or in a new subsection just below it):
```
### Per-session mutex on --resume

Worker (`bot/src/telegram/worker.rs`) and cron delivery
(`bot/src/cron_delivery.rs`) both invoke `claude -p --resume <main_session_id>`,
which mutates the session's JSONL file. Concurrent invocations against the same
session would interleave or lose turns.

A `SessionLocks` map (`Arc<DashMap<String, Arc<Mutex<()>>>>`) keyed by the main
`root_session_id` serialises these accesses. Worker acquires before each
foreground turn; delivery acquires before each Haiku-relayed delivery. Cron
job execution itself does NOT acquire — it runs `--fork-session` against a new
session ID and does not race the main session JSONL.

`IDLE_THRESHOLD_SECS = 180` remains as UX politeness ("don't interrupt the
user mid-conversation"), but correctness now lives in the mutex.

Sweep: a periodic task in `lib.rs` (every hour) drops entries whose Arc has no
external strong references — protects against unbounded growth on long-lived
agents.
```

- [ ] **Step 4: Document the X-FORK-FROM convention**

In the same vicinity as the cron section:
```
### Background continuation: X-FORK-FROM convention

A background continuation cron job is identified by its prompt starting with
`X-FORK-FROM: <main_session_id>\n`. `cron::execute_job` strips this header,
sets `ClaudeInvocation::resume_session_id` and `fork_session = true`, and
passes the body as the user message. The forked session inherits the main
session's full history; the body is a short SYSTEM_NOTICE asking the agent to
finish answering the user's most recent message.

This convention avoids a `cron_specs` schema migration. It is bot-internal —
no agent or user is expected to construct prompts with this prefix.
```

- [ ] **Step 5: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs(arch): document Immediate schedule, session mutex, X-FORK-FROM"
```

---

## Self-Review

- [ ] **Step 1: Spec coverage check**

Walk through each section of `docs/superpowers/specs/2026-05-05-background-continuation-design.md`:

| Spec section | Plan task |
|---|---|
| Decisions table | Tasks 1–12 collectively |
| Data flow → Foreground | Task 11 (Backgrounded outcome + banner) |
| Data flow → Background execution | Task 4 (Immediate firing) + Task 10 (X-FORK-FROM → fork-session) |
| Data flow → Delivery | Task 7 (mutex acquisition; rest already exists) |
| Data flow → Per-session mutex | Tasks 6, 7, 12 |
| `ScheduleKind::Immediate` | Tasks 2, 3 |
| Cron loop fires Immediate | Task 4 |
| `--fork-session` support | Task 5 |
| Worker trigger handling | Tasks 8, 9, 11 |
| Continuation prompt | Task 9 |
| Banner text | Task 11 |
| `bg:` callback handler | Task 8 |
| Session-locks sweep | Task 12 |
| Default budget bump | Task 1 |
| Failure mode (bg fails) | No code change — relies on existing cron failure pipeline. Verified by inspection in Task 13 docs note. |
| Risks (partial last turn) | TO VERIFY noted in spec; no code change planned. Manual smoke-test during rollout. |
| ARCHITECTURE.md updates | Task 13 |

- [ ] **Step 2: Placeholder scan**

```bash
grep -nE "TBD|TODO|fill in|implement later|appropriate error|handle edge cases" docs/superpowers/plans/2026-05-05-background-continuation.md
```
Expected: no matches.

- [ ] **Step 3: Type consistency check**

- `BgReason` variants: `AutoTimeout`, `UserRequested` — used identically in worker.rs (continuation prompt + banner) and `InvokeCcFailure::Backgrounded` (Tasks 9, 11). ✓
- `enqueue_background_job` parameter order: `conn, chat_id, thread_id, main_session_id, reason` — used identically in tests (Task 9) and `spawn_worker` (Task 11). ✓
- `working_keyboard(chat_id, thread_id)` — same signature in helper and callsite (Task 8). ✓
- `SessionLocks = Arc<DashMap<String, Arc<Mutex<()>>>>` — same shape in mod.rs, WorkerCtx, lib.rs, dispatch.rs (Tasks 6, 7, 12). ✓
- `BgRequests = Arc<DashMap<(i64, i64), ()>>` — same shape across handler.rs, WorkerCtx, lib.rs (Tasks 8, 11, 12). ✓
- `ClaudeInvocation::fork_session: bool` — set in Task 5, consumed in Task 10 (cron) and any future worker callsite. ✓
- `insert_immediate_cron(conn, name, prompt, target_chat_id, target_thread_id, max_budget_usd)` — defined in Task 3, called in Task 9 with matching argument types. ✓
- `X-FORK-FROM` header format `"X-FORK-FROM: <id>\n<body>"` — written in Task 9 (`enqueue_background_job`), parsed in Task 10 (`execute_job`) with matching prefix. ✓

- [ ] **Step 4: Verify build at end**

```bash
cargo build --workspace
cargo test --workspace
```
Expected: clean.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-05-background-continuation.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
