# Cron `then` continuation + ad-hoc trigger instructions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three foreground-orchestration features to the `cron_trigger` MCP tool — an ephemeral per-run `extra_instruction`, a runtime-guaranteed structured `then` continuation that forks the triggered run's session, and origin-chat resolution so a `then` can report back to the chat it was triggered from.

**Architecture:** Per-trigger inputs are stored as transient columns on `cron_specs` (same lifecycle as the existing `trigger_force_notify`), loaded into the in-memory `CronSpec` snapshot by the reconciler, then read by `execute_job` and its completion handler (which keeps `spec` in scope through completion — no `async_runs` schema change). The `then` continuation reuses the existing `spawn_background_continuation` path (`fork_session: true`, `--resume` the triggered run's session). Origin chat is resolved server-side from the foreground invocation's conversation scope.

**Tech Stack:** Rust 2024, `turso`-backed `right-db` migrations, `rmcp`/aggregator MCP tooling, `serde`/`schemars`, `tokio`.

**Spec:** `docs/superpowers/specs/2026-06-14-cron-then-continuation-design.md`

**Verification cadence:** Targeted `-p right-db` / `-p right-agent` / `-p right` / `-p bot` tests during each task's red/green loop. ONE full `cargo nextest run --workspace` + `cargo test --doc --workspace` at the end (Task E4). Do not run the full workspace after every edit.

Run all commands prefixed with `devenv shell --` from the worktree root.

---

## Reference: shared type shapes (defined in Phase B/C, referenced later)

`right_agent::cron_spec::ThenSpec` (Phase B, Task B1):

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ThenSpec {
    pub instruction: String,
    pub run_on: RunOn,
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub target_chat_id: Option<i64>,
    #[serde(default)]
    pub target_thread_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOn {
    Success,
    Failure,
    Always,
}
```

`RunOn` has **no `Default`** and `ThenSpec::run_on` has **no `#[serde(default)]`**, so JSON missing `run_on` fails to deserialize — this is how "`run_on` is required" is enforced.

---

## Phase A — `right-db` migration

### Task A1: Add migration v45 (cron_specs transient columns)

**Files:**
- Modify: `crates/right-db/src/migrations.rs` (registry + `LATEST_SCHEMA_VERSION`, currently `44`)
- Test: `crates/right-db/src/migrations.rs` (existing `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing test**

Add to the migrations test module:

```rust
#[tokio::test]
async fn v45_adds_cron_trigger_transient_columns() {
    let conn = crate::test_support::open_migrated_memory().await;
    for col in [
        "trigger_extra_instruction",
        "trigger_then_json",
        "trigger_origin_chat_id",
        "trigger_origin_thread_id",
    ] {
        let n = conn
            .query_i64(
                "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = ?",
                crate::migrations::MigrationParams::one_text(col),
            )
            .await
            .unwrap();
        assert_eq!(n, 1, "missing cron_specs.{col}");
    }
}
```

> Use whatever helper the existing migration tests use to open a fully-migrated in-memory DB (grep the test module for the current pattern, e.g. `open_migrated_memory` / `open_connection(":memory:", true)`); match it exactly. `MigrationParams::one_text` mirrors the `two_text` helper used by `column_exists` (`migrations.rs:225-237`) — if a one-arg variant doesn't exist, use the existing `query_i64` + params pattern from a neighbouring test.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right-db v45_adds_cron_trigger_transient_columns`
Expected: FAIL (columns absent; `LATEST_SCHEMA_VERSION` still 44).

- [ ] **Step 3: Add the migration hook and register it**

Add the hook (mirror the v42 single-table template at `migrations.rs:792-806`):

```rust
fn v45_cron_trigger_transient(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        let cron_specs_exists = conn
            .query_i64(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cron_specs'",
                MigrationParams::Empty,
            )
            .await?;
        if cron_specs_exists == 0 {
            return Ok(());
        }
        for (col, ddl) in [
            ("trigger_extra_instruction", "ALTER TABLE cron_specs ADD COLUMN trigger_extra_instruction TEXT"),
            ("trigger_then_json", "ALTER TABLE cron_specs ADD COLUMN trigger_then_json TEXT"),
            ("trigger_origin_chat_id", "ALTER TABLE cron_specs ADD COLUMN trigger_origin_chat_id INTEGER"),
            ("trigger_origin_thread_id", "ALTER TABLE cron_specs ADD COLUMN trigger_origin_thread_id INTEGER"),
        ] {
            if !column_exists(conn, "cron_specs", col).await? {
                conn.execute_batch(ddl).await?;
            }
        }
        Ok(())
    })
}
```

Register it in the `MIGRATIONS` array (after the `version: 44` entry):

```rust
    Migration { version: 45, sql: "", hook: Some(v45_cron_trigger_transient) },
```

Bump the constant (`migrations.rs:39`):

```rust
pub const LATEST_SCHEMA_VERSION: u32 = 45;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo nextest run -p right-db v45 migration_runner_semantics_latest_schema_version_matches_highest_migration`
Expected: PASS (new column test + the version-constant invariant test).

- [ ] **Step 5: Commit**

```bash
git add crates/right-db/src/migrations.rs
git commit -m "feat(db): v45 cron_specs trigger transient columns (extra_instruction, then, origin)"
```

---

## Phase B — `right-agent` types & `cron_spec`

### Task B1: `ThenSpec` / `RunOn` types

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (add types near the top, after `ScheduleKind`)
- Test: `crates/right-agent/src/cron_spec_tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `cron_spec_tests.rs`:

```rust
#[test]
fn then_spec_roundtrips_and_run_on_is_required() {
    use crate::cron_spec::{RunOn, ThenSpec};

    let then = ThenSpec {
        instruction: "summarize how it went".into(),
        run_on: RunOn::Always,
        notify: true,
        target_chat_id: Some(42),
        target_thread_id: None,
    };
    let json = serde_json::to_string(&then).unwrap();
    let back: ThenSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(then, back);
    assert!(json.contains("\"run_on\":\"always\""));

    // run_on missing -> deserialization fails (required field)
    let no_run_on = r#"{"instruction":"x"}"#;
    assert!(serde_json::from_str::<ThenSpec>(no_run_on).is_err());

    // notify / targets default when absent
    let minimal = r#"{"instruction":"x","run_on":"success"}"#;
    let parsed: ThenSpec = serde_json::from_str(minimal).unwrap();
    assert!(!parsed.notify);
    assert_eq!(parsed.target_chat_id, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right-agent then_spec_roundtrips`
Expected: FAIL (types don't exist).

- [ ] **Step 3: Add the types**

Insert into `cron_spec.rs` (after the `ScheduleKind` impl block, before `CronSpec`):

```rust
/// When a `then` continuation fires, relative to the triggered run's
/// terminal status. Required on every `ThenSpec` (no default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOn {
    Success,
    Failure,
    Always,
}

impl RunOn {
    /// Whether a continuation should fire given the triggered run's outcome.
    pub fn fires_on(&self, success: bool) -> bool {
        match self {
            RunOn::Always => true,
            RunOn::Success => success,
            RunOn::Failure => !success,
        }
    }
}

/// A runtime-guaranteed follow-up that resumes (forks) the triggered run's
/// session after it reaches a terminal state matching `run_on`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ThenSpec {
    pub instruction: String,
    pub run_on: RunOn,
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub target_chat_id: Option<i64>,
    #[serde(default)]
    pub target_thread_id: Option<i64>,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p right-agent then_spec_roundtrips`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs crates/right-agent/src/cron_spec_tests.rs
git commit -m "feat(cron): ThenSpec + RunOn types with required run_on"
```

### Task B2: `CronSpec` transient fields (excluded from `PartialEq`)

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (`CronSpec` struct `124-139`, `PartialEq` impl `148-158`)
- Test: `crates/right-agent/src/cron_spec_tests.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn cron_spec_eq_ignores_transient_trigger_fields() {
    use crate::cron_spec::{CronSpec, RunOn, ScheduleKind, ThenSpec};

    let base = CronSpec {
        schedule_kind: ScheduleKind::Recurring("17 9 * * *".into()),
        prompt: "p".into(),
        lock_ttl: None,
        max_budget_usd: 2.0,
        triggered_at: None,
        trigger_force_notify: false,
        target_chat_id: Some(1),
        target_thread_id: None,
        model: None,
        trigger_extra_instruction: None,
        then: None,
        trigger_origin_chat_id: None,
        trigger_origin_thread_id: None,
    };
    let mut triggered = base.clone();
    triggered.trigger_extra_instruction = Some("focus on X".into());
    triggered.then = Some(ThenSpec {
        instruction: "go".into(),
        run_on: RunOn::Success,
        notify: false,
        target_chat_id: None,
        target_thread_id: None,
    });
    triggered.trigger_origin_chat_id = Some(99);
    // Transient trigger state must NOT affect equality (reconciler relies on this).
    assert_eq!(base, triggered);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right-agent cron_spec_eq_ignores_transient`
Expected: FAIL (struct lacks the new fields).

- [ ] **Step 3: Add the fields, keep them out of `PartialEq`**

In the `CronSpec` struct (after `model: Option<String>,` at line 138), add:

```rust
    /// Ephemeral, set only for a triggered run; not config — excluded from
    /// `PartialEq` like `triggered_at`/`trigger_force_notify`.
    pub trigger_extra_instruction: Option<String>,
    pub then: Option<ThenSpec>,
    pub trigger_origin_chat_id: Option<i64>,
    pub trigger_origin_thread_id: Option<i64>,
```

The `PartialEq` impl (lines 148-158) already lists only config fields — **do not add the new fields to it**. Leave it unchanged so they are ignored. (Add a comment pointing at this invariant.)

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p right-agent cron_spec_eq_ignores_transient`
Expected: PASS. The crate will not compile until every `CronSpec { .. }` literal in `right-agent` includes the new fields — fix those construction sites (the test modules in `cron_spec_tests.rs` and any `load_specs_from_db` caller) by adding the four fields set to `None`.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs crates/right-agent/src/cron_spec_tests.rs
git commit -m "feat(cron): CronSpec transient then/extra/origin fields (excluded from eq)"
```

### Task B3: `trigger_spec` writes the new transient columns

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (`trigger_spec` `813-830`)
- Test: `crates/right-agent/src/cron_spec_tests.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn trigger_spec_writes_then_and_origin() {
    let conn = crate::test_support_open_migrated().await; // use the crate's existing helper
    crate::cron_spec::create_spec(&conn, "j", "17 9 * * *", "p", None, None)
        .await
        .unwrap();

    crate::cron_spec::trigger_spec(
        &conn,
        "j",
        true,
        Some("focus on X"),
        Some(r#"{"instruction":"go","run_on":"always"}"#),
        Some(555),
        Some(7),
    )
    .await
    .unwrap();

    let specs = crate::cron_spec::load_specs_from_db(&conn).await.unwrap();
    let s = specs.get("j").unwrap();
    assert_eq!(s.trigger_extra_instruction.as_deref(), Some("focus on X"));
    assert_eq!(s.then.as_ref().unwrap().instruction, "go");
    assert_eq!(s.trigger_origin_chat_id, Some(555));
    assert_eq!(s.trigger_origin_thread_id, Some(7));
    assert!(s.trigger_force_notify);
}
```

> Use the crate's existing test DB helper (grep `cron_spec_tests.rs` for how current `#[tokio::test]`s open a migrated connection); replace `test_support_open_migrated` with that.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right-agent trigger_spec_writes_then_and_origin`
Expected: FAIL (compile error — `trigger_spec` has 3 params; `load_specs_from_db` doesn't populate new fields yet — Task B5).

- [ ] **Step 3: Extend `trigger_spec`**

Replace `trigger_spec` (813-830) with:

```rust
#[allow(clippy::too_many_arguments)]
pub async fn trigger_spec(
    conn: &Connection,
    job_name: &str,
    force_notify: bool,
    extra_instruction: Option<&str>,
    then_json: Option<&str>,
    origin_chat_id: Option<i64>,
    origin_thread_id: Option<i64>,
) -> Result<String, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let rows = conn
        .execute(
            "UPDATE cron_specs SET triggered_at = ?2, trigger_force_notify = ?3, \
             trigger_extra_instruction = ?4, trigger_then_json = ?5, \
             trigger_origin_chat_id = ?6, trigger_origin_thread_id = ?7 \
             WHERE job_name = ?1",
            params![
                job_name,
                now,
                force_notify as i64,
                extra_instruction,
                then_json,
                origin_chat_id,
                origin_thread_id
            ],
        )
        .await
        .map_err(|e| format!("trigger failed: {e:#}"))?;
    if rows == 0 {
        return Err(format!("job '{job_name}' not found"));
    }
    Ok(format!("Triggered job '{job_name}'."))
}
```

> This breaks the existing call in `crates/right/src/right_backend.rs` (Phase C, Task C3) and any test caller — they'll be fixed in their own tasks. If a different crate calls `trigger_spec` before C3, pass `None, None, None, None` to keep it compiling.

- [ ] **Step 4: Run test to verify it passes** (after B5 lands `load_specs_from_db`)

This test depends on Task B5; if you implement B3→B5 in order, run after B5:
Run: `devenv shell -- cargo nextest run -p right-agent trigger_spec_writes_then_and_origin`
Expected: PASS.

- [ ] **Step 5: Commit** (may be combined with B5)

```bash
git add crates/right-agent/src/cron_spec.rs
git commit -m "feat(cron): trigger_spec persists extra_instruction/then/origin"
```

### Task B4: `clear_triggered_at` clears the new columns

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (`clear_triggered_at` `835-843`)
- Test: `crates/right-agent/src/cron_spec_tests.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn clear_triggered_at_wipes_all_transient_fields() {
    let conn = crate::test_support_open_migrated().await;
    crate::cron_spec::create_spec(&conn, "j", "17 9 * * *", "p", None, None)
        .await
        .unwrap();
    crate::cron_spec::trigger_spec(
        &conn, "j", true, Some("x"),
        Some(r#"{"instruction":"go","run_on":"success"}"#), Some(5), Some(1),
    )
    .await
    .unwrap();

    crate::cron_spec::clear_triggered_at(&conn, "j").await.unwrap();

    let s = crate::cron_spec::load_specs_from_db(&conn).await.unwrap();
    let j = s.get("j").unwrap();
    assert_eq!(j.trigger_extra_instruction, None);
    assert!(j.then.is_none());
    assert_eq!(j.trigger_origin_chat_id, None);
    assert_eq!(j.trigger_origin_thread_id, None);
    assert!(!j.trigger_force_notify);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right-agent clear_triggered_at_wipes_all`
Expected: FAIL (new columns not cleared).

- [ ] **Step 3: Extend `clear_triggered_at`**

```rust
pub async fn clear_triggered_at(conn: &Connection, job_name: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE cron_specs SET triggered_at = NULL, trigger_force_notify = 0, \
         trigger_extra_instruction = NULL, trigger_then_json = NULL, \
         trigger_origin_chat_id = NULL, trigger_origin_thread_id = NULL \
         WHERE job_name = ?1",
        params![job_name],
    )
    .await
    .map_err(|e| format!("clear trigger failed: {e:#}"))?;
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p right-agent clear_triggered_at_wipes_all`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs
git commit -m "feat(cron): clear_triggered_at wipes new transient columns"
```

### Task B5: `load_specs_from_db` loads + parses the new columns

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (`load_specs_from_db` `729-798`)
- Test: covered by B3/B4 tests above.

- [ ] **Step 1: Tests already written** (B3, B4 depend on this).

- [ ] **Step 2: Extend the SELECT and row mapping**

In `load_specs_from_db`, extend the SELECT column list (line 732) to append the four columns:

```rust
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, \
         trigger_force_notify, recurring, run_at, target_chat_id, target_thread_id, model, \
         trigger_extra_instruction, trigger_then_json, trigger_origin_chat_id, trigger_origin_thread_id \
         FROM cron_specs",
```

Extend the row closure to read indices 12-15:

```rust
            row.get::<_, Option<String>>(11)?, // model (existing)
            row.get::<_, Option<String>>(12)?, // trigger_extra_instruction
            row.get::<_, Option<String>>(13)?, // trigger_then_json
            row.get::<_, Option<i64>>(14)?,    // trigger_origin_chat_id
            row.get::<_, Option<i64>>(15)?,    // trigger_origin_thread_id
```

Destructure the new values in the `for row in rows` loop and build `CronSpec`:

```rust
        let then = match trigger_then_json.as_deref() {
            Some(j) => match serde_json::from_str::<ThenSpec>(j) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::error!(job = %job_name, "ignoring unparseable trigger_then_json: {e}");
                    None
                }
            },
            None => None,
        };
        // ... in the CronSpec { .. } literal, add:
        trigger_extra_instruction,
        then,
        trigger_origin_chat_id,
        trigger_origin_thread_id,
```

> A malformed `then_json` is logged and dropped (not fatal) — a single bad row must not break loading of all specs, matching the existing schedule-parse `continue` behavior.

- [ ] **Step 3: Run the B3 + B4 + B2 tests**

Run: `devenv shell -- cargo nextest run -p right-agent cron_spec`
Expected: PASS for `trigger_spec_writes_then_and_origin`, `clear_triggered_at_wipes_all`, `cron_spec_eq_ignores_transient`, and existing cron_spec tests.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs
git commit -m "feat(cron): load_specs_from_db carries trigger transient fields"
```

---

## Phase C — MCP tool surface (`right` crate)

### Task C1: `cron_trigger` input params (`extra_instruction`, `then`)

**Files:**
- Modify: `crates/right/src/memory_server.rs` (`CronTriggerParams` struct)
- Test: `crates/right/src/memory_server_mcp_tests.rs` (or the nearest existing test module in the `right` crate)

The MCP input type stays in the `right` crate (schemars-deriving). The execution side deserializes from DB JSON into `right_agent::cron_spec::ThenSpec`; a parity test guards field-name drift.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn cron_then_params_json_matches_then_spec() {
    use crate::memory_server::{CronThenParams, RunOnDto};
    let p = CronThenParams {
        instruction: "go".into(),
        run_on: RunOnDto::Success,
        notify: true,
        target_chat_id: Some(9),
        target_thread_id: None,
    };
    // Serialized CronThenParams must deserialize into right_agent's ThenSpec.
    let json = serde_json::to_string(&p).unwrap();
    let spec: right_agent::cron_spec::ThenSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(spec.instruction, "go");
    assert_eq!(spec.run_on, right_agent::cron_spec::RunOn::Success);
    assert!(spec.notify);
    assert_eq!(spec.target_chat_id, Some(9));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right cron_then_params_json_matches`
Expected: FAIL (types don't exist).

- [ ] **Step 3: Add the input types and extend `CronTriggerParams`**

In `memory_server.rs`, add near `CronTriggerParams`:

```rust
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunOnDto {
    Success,
    Failure,
    Always,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CronThenParams {
    #[schemars(description = "Instruction for the follow-up turn. It resumes (forks) THIS run's session, so it can reference what the run just did.")]
    pub instruction: String,
    #[schemars(description = "When the follow-up fires relative to this run's outcome. REQUIRED.")]
    pub run_on: RunOnDto,
    #[serde(default)]
    #[schemars(description = "Force the follow-up's report to the user (skip silent/idle gate). Default false.")]
    pub notify: bool,
    #[serde(default)]
    #[schemars(description = "Override the follow-up's delivery chat. Defaults to the chat this trigger was issued from.")]
    pub target_chat_id: Option<i64>,
    #[serde(default)]
    pub target_thread_id: Option<i64>,
}
```

Extend `CronTriggerParams` with:

```rust
    #[serde(default)]
    #[schemars(description = "Extra instruction prepended to THIS run only; does not change the stored prompt.")]
    pub extra_instruction: Option<String>,
    #[serde(default)]
    #[schemars(description = "Runtime-guaranteed follow-up that resumes this run's session after it finishes.")]
    pub then: Option<CronThenParams>,
```

> `run_on` is non-`Option` with no `#[serde(default)]`, so a `then` without `run_on` fails deserialization → the handler returns a params error. `RunOnDto`'s `snake_case` serialization matches `RunOn`, and field names match `ThenSpec`, so the JSON round-trips (guarded by the test).

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p right cron_then_params_json_matches`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/memory_server.rs crates/right/src/memory_server_mcp_tests.rs
git commit -m "feat(mcp): cron_trigger extra_instruction + then input params"
```

### Task C2: foreground-scoped origin accessor on `ProgressRegistry`

**Files:**
- Modify: `crates/right/src/progress.rs` (add method near `foreground_scope` `300-325`)
- Test: `crates/right/src/progress.rs` test module (mirror an existing `foreground_scope` test)

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn conversation_scope_opt_returns_some_for_foreground_none_for_cron() {
    // Build a registry, register a Foreground invocation with a scope and a
    // Cron invocation with a scope, mirroring existing register tests.
    // (Copy the setup from the nearest existing foreground_scope test.)
    let reg = ProgressRegistry::new();
    reg.register(ProgressRegistration {
        invocation_id: "fg".into(),
        kind: ProgressInvocationKind::Foreground,
        bot_socket_path: "/tmp/x".into(),
        bot_send_token: "t".into(),
        conversation_scope: Some(ConversationScope { chat_id: 5, thread_id: 2 }),
    })
    .await;
    reg.register(ProgressRegistration {
        invocation_id: "cron".into(),
        kind: ProgressInvocationKind::Cron,
        bot_socket_path: "/tmp/x".into(),
        bot_send_token: "t".into(),
        conversation_scope: Some(ConversationScope { chat_id: 9, thread_id: 0 }),
    })
    .await;

    assert_eq!(
        reg.conversation_scope_opt("fg").await,
        Some(ConversationScope { chat_id: 5, thread_id: 2 })
    );
    assert_eq!(reg.conversation_scope_opt("cron").await, None);
    assert_eq!(reg.conversation_scope_opt("missing").await, None);
}
```

> Match `ProgressRegistration`/`ProgressRegistry::new`/`register` to their real shapes (grep `progress.rs`); adjust field names if they differ. `ConversationScope` must derive `PartialEq` for the assert — add `#[derive(PartialEq)]` if missing (it's a plain `{chat_id, thread_id}` struct).

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right conversation_scope_opt_returns_some`
Expected: FAIL (method missing).

- [ ] **Step 3: Add the accessor**

```rust
    /// Origin conversation scope for a foreground invocation, or `None` for a
    /// non-foreground invocation or unknown id. Used by `cron_trigger` to learn
    /// the chat it was triggered from — origin exists iff the trigger came from
    /// a live foreground turn.
    pub(crate) async fn conversation_scope_opt(
        &self,
        invocation_id: &str,
    ) -> Option<ConversationScope> {
        let inner = self.inner.lock().await;
        let invocation = inner.get(invocation_id)?;
        if !matches!(invocation.kind, ProgressInvocationKind::Foreground) {
            return None;
        }
        invocation.conversation_scope
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p right conversation_scope_opt_returns_some`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/progress.rs
git commit -m "feat(mcp): foreground-scoped conversation_scope_opt accessor"
```

### Task C3: wire `ToolCallContext` + origin into `call_cron_trigger`

**Files:**
- Modify: `crates/right/src/right_backend.rs` (dispatch `298`, handler `591-604`)
- Modify: `crates/right/src/memory_server.rs` (parallel rmcp handler `414-426`, if it also calls `trigger_spec`)
- Test: `crates/right/src/right_backend_tests.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn cron_trigger_with_then_persists_then_json_and_origin() {
    // Build a RightBackend over a temp agent dir with a migrated data.db and a
    // cron spec "j" already created (reuse the harness in right_backend_tests.rs).
    // Register a Foreground invocation "inv-1" with scope {chat_id: 77, thread_id: 0}
    // in the backend's ProgressRegistry.
    // Call tools_call("cron_trigger", { job_name: "j", then: { instruction: "go", run_on: "success" } },
    //                 ToolCallContext { invocation_id: Some("inv-1".into()) }).
    // Then load_specs_from_db and assert the spec's `then` is Some, run_on Success,
    // and trigger_origin_chat_id == Some(77).
    // (Model this on the existing cron_create/cron_trigger tests in this file.)
}
```

> Flesh out the body using the existing `RightBackend` test harness in `right_backend_tests.rs` (there are already `cron_create`/`cron_trigger` tests — copy their setup verbatim and add the registry registration + assertions). Keep the assertions above.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right cron_trigger_with_then_persists`
Expected: FAIL (handler ignores `then`/origin; `trigger_spec` call passes only 3 args / won't compile).

- [ ] **Step 3: Thread context and resolve origin in the handler**

Change the dispatch arm (`right_backend.rs:298`) to pass context (match how `call_send_message` receives it at `300-303`):

```rust
            "cron_trigger" => self.call_cron_trigger(agent_name, &args, &context).await,
```

Rewrite `call_cron_trigger` (591-604):

```rust
    async fn call_cron_trigger(
        &self,
        agent_name: &str,
        args: &serde_json::Value,
        context: &crate::progress::ToolCallContext,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronTriggerParams =
            serde_json::from_value(args.clone()).context("invalid cron_trigger params")?;

        // Resolve origin chat from the foreground invocation that issued this call.
        // `None` for cron-turn callers (legacy hand-off) — then falls back to the
        // job's standing target.
        let origin = match &context.invocation_id {
            Some(id) => self.progress.conversation_scope_opt(id).await,
            None => None,
        };
        let (origin_chat, origin_thread) = match origin {
            Some(s) => (Some(s.chat_id), Some(s.thread_id)),
            None => (None, None),
        };

        // Serialize `then` (input shape) into the JSON ThenSpec stored in DB.
        let then_json = match &params.then {
            Some(t) => Some(serde_json::to_string(t).context("serialize then")?),
            None => None,
        };

        let conn_arc = self.get_conn(agent_name).await?;
        let conn = conn_arc.lock().await;
        let msg = right_agent::cron_spec::trigger_spec(
            &conn,
            &params.job_name,
            params.notify,
            params.extra_instruction.as_deref(),
            then_json.as_deref(),
            origin_chat,
            origin_thread,
        )
        .await
        .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }
```

> `self.progress` is the `ProgressRegistry` handle the backend already uses for `call_send_message` — match its real field name. `ToolCallContext` import path mirrors `call_send_message`. If `memory_server.rs`'s parallel `cron_trigger` handler (`414-426`) also calls `trigger_spec`, update it to pass `params... None/None/None/None` for the four new args (the rmcp-macro path has no `ToolCallContext`/registry, so origin is always absent there) — keeping it compiling; the live path is `right_backend.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo nextest run -p right cron_trigger`
Expected: PASS (new test + existing cron_trigger tests + `cron_trigger_description_matches_const`).

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/right_backend.rs crates/right/src/memory_server.rs crates/right/src/right_backend_tests.rs
git commit -m "feat(mcp): cron_trigger resolves origin + persists then/extra_instruction"
```

---

## Phase D — execution wiring (`bot` crate, `cron.rs`)

### Task D1: prepend `extra_instruction` in `execute_job`

**Files:**
- Modify: `crates/bot/src/cron.rs` (`604-613`)
- Test: `crates/bot/src/cron.rs` test module (a pure prompt-composition helper makes this unit-testable)

- [ ] **Step 1: Write the failing test**

Extract prompt composition into a pure function and test it:

```rust
#[test]
fn compose_run_prompt_orders_force_notify_then_extra_then_prompt() {
    let p = compose_run_prompt("BODY", true, Some("focus on X"));
    let fn_idx = p.find("Manual verification trigger").unwrap();
    let extra_idx = p.find("focus on X").unwrap();
    let body_idx = p.find("BODY").unwrap();
    assert!(fn_idx < extra_idx && extra_idx < body_idx);

    // No force-notify, no extra -> body unchanged.
    assert_eq!(compose_run_prompt("BODY", false, None), "BODY");

    // Extra only.
    let e = compose_run_prompt("BODY", false, Some("X"));
    assert!(e.contains("X") && e.ends_with("BODY"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p bot compose_run_prompt_orders`
Expected: FAIL (function not defined).

- [ ] **Step 3: Add the helper and use it in `execute_job`**

Add near `execute_job`:

```rust
/// Compose a triggered run's prompt: force-notify notice, then this-run-only
/// extra instruction, then the stored prompt. Each layer is optional.
fn compose_run_prompt(prompt: &str, force_notify: bool, extra_instruction: Option<&str>) -> String {
    let mut out = String::new();
    if force_notify {
        out.push_str(
            "⟨⟨SYSTEM_NOTICE⟩⟩ Manual verification trigger: always emit \
             delivery.kind=\"notify\" with a complete report of what you found; \
             do not go silent. ⟨⟨/SYSTEM_NOTICE⟩⟩\n\n",
        );
    }
    if let Some(extra) = extra_instruction.filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!(
            "⟨⟨SYSTEM_NOTICE⟩⟩ Extra instruction for this run only: {extra} ⟨⟨/SYSTEM_NOTICE⟩⟩\n\n"
        ));
    }
    out.push_str(prompt);
    out
}
```

Replace the inline `prompt_for_cc` block (604-613) with:

```rust
    let prompt_for_cc = compose_run_prompt(
        &spec.prompt,
        spec.trigger_force_notify,
        spec.trigger_extra_instruction.as_deref(),
    );
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p bot compose_run_prompt_orders`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): prepend ephemeral extra_instruction to triggered run"
```

### Task D2: spawn the `then` continuation on completion

**Files:**
- Modify: `crates/bot/src/cron.rs` (success branch ~`1035-1042`, failure branch ~`1160-1276`, plus a new helper)
- Test: `crates/bot/src/cron.rs` test module (pure decision fn) + reuse the mock-CC harness for an integration check

This splits into a **pure decision function** (unit-tested) and the **spawn wiring** (integration).

- [ ] **Step 1: Write the failing test for the decision fn**

```rust
#[test]
fn then_action_respects_run_on_and_target_precedence() {
    use right_agent::cron_spec::{RunOn, ThenSpec};

    let mk = |run_on, then_target: Option<i64>, origin: Option<i64>, standing: Option<i64>| {
        let mut s = sample_cron_spec(); // helper returning a minimal CronSpec
        s.target_chat_id = standing;
        s.trigger_origin_chat_id = origin;
        s.then = Some(ThenSpec {
            instruction: "go".into(),
            run_on,
            notify: false,
            target_chat_id: then_target,
            target_thread_id: None,
        });
        s
    };

    // run_on=success fires only on success
    assert!(resolve_then_action(&mk(RunOn::Success, None, Some(1), Some(2)), true).is_some());
    assert!(resolve_then_action(&mk(RunOn::Success, None, Some(1), Some(2)), false).is_none());
    // run_on=failure fires only on failure
    assert!(resolve_then_action(&mk(RunOn::Failure, None, Some(1), Some(2)), false).is_some());
    // run_on=always fires both
    assert!(resolve_then_action(&mk(RunOn::Always, None, Some(1), Some(2)), true).is_some());

    // target precedence: then.target_chat_id > origin > standing
    assert_eq!(resolve_then_action(&mk(RunOn::Always, Some(9), Some(1), Some(2)), true).unwrap().target_chat_id, 9);
    assert_eq!(resolve_then_action(&mk(RunOn::Always, None, Some(1), Some(2)), true).unwrap().target_chat_id, 1);
    assert_eq!(resolve_then_action(&mk(RunOn::Always, None, None, Some(2)), true).unwrap().target_chat_id, 2);
    // no target anywhere -> None (cannot deliver)
    assert!(resolve_then_action(&mk(RunOn::Always, None, None, None), true).is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p bot then_action_respects_run_on`
Expected: FAIL (fn + helper not defined).

- [ ] **Step 3: Add the decision fn**

```rust
/// What a `then` continuation should deliver, if it fires.
pub(crate) struct ThenAction {
    pub target_chat_id: i64,
    pub target_thread_id: Option<i64>,
    pub prompt: String,
}

/// Decide whether/where a `then` continuation fires for a finished triggered run.
/// Target precedence: explicit `then.target_chat_id` > resolved origin >
/// the job's standing `target_chat_id`. Returns `None` when there is no `then`,
/// the `run_on` does not match, or no deliverable chat is known.
pub(crate) fn resolve_then_action(spec: &CronSpec, success: bool) -> Option<ThenAction> {
    let then = spec.then.as_ref()?;
    if !then.run_on.fires_on(success) {
        return None;
    }
    let target_chat_id = then
        .target_chat_id
        .or(spec.trigger_origin_chat_id)
        .or(spec.target_chat_id)?;
    let target_thread_id = if then.target_chat_id.is_some() {
        then.target_thread_id
    } else if spec.trigger_origin_chat_id.is_some() {
        spec.trigger_origin_thread_id
    } else {
        spec.target_thread_id
    };
    let prompt = if then.notify {
        format!(
            "⟨⟨SYSTEM_NOTICE⟩⟩ Scheduled follow-up of the job you just ran. Always emit \
             delivery.kind=\"notify\" with a complete report. ⟨⟨/SYSTEM_NOTICE⟩⟩\n\n{}",
            then.instruction
        )
    } else {
        then.instruction.clone()
    };
    Some(ThenAction { target_chat_id, target_thread_id, prompt })
}
```

> Add a `sample_cron_spec()` test helper if the module lacks one (a `CronSpec` literal with all fields, `then: None`, targets `None`).
> NOTE (v1 limitation): `then.notify` is realized via the prompt directive above (the continuation chooses `notify`). Forcing delivery through the background row's `force_notify` column + idle-gate skip is a follow-up; see the spec's delivery section. Do not claim idle-gate skip works until that wiring exists.

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p bot then_action_respects_run_on`
Expected: PASS.

- [ ] **Step 5: Add the spawn wiring (no new test; exercised by Step 7 integration)**

Add a helper that builds the background request and spawns it, mirroring the worker call site (`worker.rs:2280-2331`) and reflection's session-guard acquisition (`worker.rs:3373-3380`):

```rust
#[allow(clippy::too_many_arguments)]
async fn spawn_then_continuation(
    action: ThenAction,
    source_session_id: String, // the triggered run's run_id == its session id
    agent_dir: &Path,
    agent_name: &str,
    model: Option<&str>,
    ssh_config_path: Option<&Path>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<&str>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    session_locks: &crate::telegram::SessionLocks,
    debug: Arc<std::sync::atomic::AtomicBool>,
) {
    let new_run_id = uuid::Uuid::new_v4().to_string();
    // Insert the queued background row so delivery + recovery treat it normally.
    {
        let conn = match right_db::open_connection(agent_dir, false).await {
            Ok(c) => c,
            Err(e) => { tracing::error!("then: open db failed: {e:#}"); return; }
        };
        let now = chrono::Utc::now().to_rfc3339();
        if let Err(e) = right_agent::async_runs::insert_queued_background_run(
            &conn,
            right_agent::async_runs::NewBackgroundRun {
                id: &new_run_id,
                producer_ref: Some("cron_then"),
                source_session_id: &source_session_id,
                run_session_id: &new_run_id,
                target_chat_id: action.target_chat_id,
                target_thread_id: action.target_thread_id,
                created_at: &now,
            },
        ).await {
            tracing::error!("then: insert background row failed: {e:#}");
            return;
        }
    }
    // Acquire the per-session mutex on the SOURCE session (we --resume/fork it).
    let session_guard = {
        let entry = session_locks
            .entry(source_session_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        entry.lock_owned().await
    };
    let status = crate::background::spawn_background_continuation(
        crate::background::BackgroundRunRequest {
            run_id: new_run_id,
            source_session_id,
            target_chat_id: action.target_chat_id,
            target_thread_id: action.target_thread_id,
            prompt: action.prompt,
        },
        agent_dir.to_path_buf(),
        agent_name.to_string(),
        model.map(|s| s.to_owned()),
        ssh_config_path.map(|p| p.to_path_buf()),
        Arc::clone(internal_client),
        resolved_sandbox.map(|s| s.to_owned()),
        upgrade_lock,
        session_guard,
        debug,
    )
    .await;
    tracing::info!(?status, "cron then continuation handoff");
}
```

Call it from both completion branches. In the **success** branch after the atomic commit logs (~`1035-1042`):

```rust
            if let Some(action) = resolve_then_action(spec, true) {
                spawn_then_continuation(
                    action,
                    run_id.clone(),
                    agent_dir, agent_name, model, ssh_config_path,
                    internal_client, resolved_sandbox,
                    Arc::clone(&upgrade_lock), session_locks, Arc::clone(&debug),
                )
                .await;
            }
```

In the **failure** branch after the failure record is persisted (~`1255-1275`, after reflection): same call with `resolve_then_action(spec, false)`.

> `upgrade_lock` is the `Arc<RwLock<()>>` param of `execute_job`; `Arc::clone` it for each call so the function keeps its own. `spawn_background_continuation` only read-locks it, so concurrent reads are fine. The call is `.await`ed inline (it returns after the continuation's `system/init` confirmation, like the worker), which is acceptable for a completion-time hand-off; if cron throughput suffers, wrap the call in `tokio::spawn` moving the cloned handles + guard in (the guard is `Send`).

- [ ] **Step 6: Build to verify wiring compiles**

Run: `devenv shell -- cargo check -p bot`
Expected: clean compile.

- [ ] **Step 7: Add an integration test using the mock-CC harness**

Model on the existing mock-CC cron test (`cron.rs:3231` uses a `remote_script` that prints a `{"type":"result",...}` line). Add a test that:
1. Creates a spec, triggers it with a `then { run_on: "success" }` and an origin chat.
2. Runs the triggered job via the same entry the other mock tests use.
3. Asserts an `async_runs` row with `kind='background'`, `producer_ref='cron_then'`, `source_session_id == <triggered run_id>`, and `target_chat_id == origin` was created.

```rust
#[tokio::test]
async fn triggered_run_with_then_success_spawns_continuation() {
    // Reuse the mock-CC scaffolding from the existing cron tests in this file.
    // After the triggered run completes successfully, query async_runs:
    //   SELECT kind, producer_ref, source_session_id, target_chat_id
    //   FROM async_runs WHERE producer_ref = 'cron_then'
    // and assert the values above. Also assert run_on=failure variant does NOT
    // create a cron_then row on a successful run.
}
```

> Fill the body from the nearest existing end-to-end mock cron test; keep the assertions. If spawning a real continuation subprocess is impractical in-test, assert up to the `insert_queued_background_run` row creation (factor `spawn_then_continuation` so the row insert is observable before the subprocess spawn — it already is).

- [ ] **Step 8: Run the integration test**

Run: `devenv shell -- cargo nextest run -p bot triggered_run_with_then_success_spawns_continuation`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): runtime-guaranteed then continuation forks the run's session"
```

---

## Phase E — skills, prompts, docs, final verification

### Task E1: update `right-cron` SKILL.md

**Files:**
- Modify: `crates/right-codegen/skills/right-cron/SKILL.md`

- [ ] **Step 1: Edit the skill**

- In "Chaining Jobs and Multi-Step Pipelines": replace the soft "schedule one delayed one-shot that triggers B with notify=true" guidance with the structured `then` on `cron_trigger` — runtime-guaranteed, resumes the run's session, so "B" sees what "A" did. Keep "prefer the fewest jobs."
- Add a short "One-off tweaks" note: use `extra_instruction` on `cron_trigger` for a single-run change instead of `cron_update` (which mutates the stored prompt).
- Add a "Report back to this chat" note: attach a `then` (e.g. `run_on: "always"`, `notify: true`, `instruction: "summarize how it went"`); its delivery defaults to the chat you triggered from.
- Document the new `cron_trigger` params in the Parameters table: `extra_instruction`, `then { instruction, run_on (required), notify, target_chat_id, target_thread_id }`.
- Bump `version` in the frontmatter (e.g. `3.5.0` → `3.6.0`).

- [ ] **Step 2: Verify codegen/skill tests still pass**

Run: `devenv shell -- cargo nextest run -p right-codegen`
Expected: PASS (no test pins the old wording; if a snapshot test fails, update the snapshot intentionally).

- [ ] **Step 3: Commit**

```bash
git add crates/right-codegen/skills/right-cron/SKILL.md
git commit -m "docs(skill): right-cron then continuation, extra_instruction, report-here"
```

### Task E2: prompt-tier descriptions

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (`TRIGGER_TOOL_DESC` `40-51`)
- Modify: `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Edit `TRIGGER_TOOL_DESC`**

Append one sentence (prompt-tier brevity): `extra_instruction` adds a one-off note to this run; `then` schedules a guaranteed follow-up that resumes this run's session (set `run_on`).

- [ ] **Step 2: Edit `CRON_INSTRUCTIONS.md`** — one or two sentences noting `then` is the sanctioned way to chain dependent work and to report back to the triggering chat; no second watcher cron.

- [ ] **Step 3: Sync `PROMPT_SYSTEM.md`** — reflect the new `cron_trigger` params in the prompting-system description.

- [ ] **Step 4: Verify**

Run: `devenv shell -- cargo nextest run -p right-agent cron_trigger_description`
Expected: PASS (the const + parallel copy stay in sync; update both if the sync test points at `memory_server.rs`).

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md PROMPT_SYSTEM.md
git commit -m "docs(prompt): cron_trigger extra_instruction + then descriptions"
```

### Task E3: architecture docs (cite-on-touch)

**Files:**
- Modify: `ARCHITECTURE.md` (MCP Aggregator scope-enforcement list)
- Modify: `docs/architecture/sessions.md` (force-notify-trigger / background-continuation narration)

- [ ] **Step 1: ARCHITECTURE.md** — add one line to the "scope comes from the registered invocation, never agent args" set: `cron_trigger` resolves the origin chat for a `then` from the foreground invocation's conversation scope; agents never pass it.

- [ ] **Step 2: sessions.md** — extend the Force-notify trigger / background-continuation section with the `then` flow: triggered run carries transient `then`/origin on the in-memory `CronSpec`; on terminal completion `resolve_then_action` + `spawn_then_continuation` fork the run's session via `spawn_background_continuation`; depth capped at 1 (continuation carries no `then`).

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md docs/architecture/sessions.md
git commit -m "docs(arch): cron then continuation + origin scope resolution"
```

### Task E4: final full-workspace verification

- [ ] **Step 1: Full test suite**

Run: `devenv shell -- cargo nextest run --workspace`
Expected: PASS (note any pre-existing flaky tests from memory — re-run isolated before blaming this change).

- [ ] **Step 2: Doctests**

Run: `devenv shell -- cargo test --doc --workspace`
Expected: PASS.

- [ ] **Step 3: Build**

Run: `devenv shell -- cargo build --workspace`
Expected: clean.

- [ ] **Step 4: Registry completeness (no new codegen output added, but confirm)**

Run: `devenv shell -- cargo nextest run registry_covers_all_per_agent_writes`
Expected: PASS (this feature adds no new per-agent codegen file).

- [ ] **Step 5: Commit any fixups**

```bash
git add -A
git commit -m "test(cron): final workspace verification for then continuation"
```

---

## Self-review notes

- **Spec coverage:** Feature 1 (extra_instruction) → A1, B2-B5, C1, C3, D1. Feature 2 (`then`) → B1-B5, C1, C3, D2. Feature 3 (origin) → C2, C3, D2. Migration → A1. Skill/prompt/docs → E1-E3. `run_on` required → B1 (+ C1). Single-hop cap → D2 (continuation row carries no `then`). Delivery mechanism (row target, not output) → D2 `resolve_then_action` target precedence.
- **Known v1 limitation (flagged):** `then.notify` is realized via a prompt directive, not the background row's `force_notify` column / idle-gate skip — called out in D2 Step 3 and the spec.
- **Cross-task type parity:** `RunOn`/`RunOnDto` snake_case + `ThenSpec`/`CronThenParams` field names are guarded by the C1 parity test; `trigger_spec`'s 7-arg signature is consistent across B3, C3, and any `memory_server.rs` caller.
- **Excluded from `PartialEq`:** the four transient `CronSpec` fields (B2) — load-bearing for the reconciler; guarded by `cron_spec_eq_ignores_transient`.
