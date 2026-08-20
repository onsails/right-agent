# Per-Cron Delivery Target Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Each cron carries its own `target_chat_id` (+ optional `target_thread_id`); delivery routes to that single target instead of fanning out to a legacy chat list. Removes `notify_chat_ids` from cron_delivery and OAuth paths.

**Architecture:** Two nullable columns on `cron_specs`. MCP `cron_create` / `cron_update` validate target against `allowlist.yaml` (read on demand from the aggregator process, no cache). Delivery JOINs cron_specs at fetch time. NULL target → log WARN and mark `delivery_status='no_target'`. OAuth notifications switch to `AllowlistHandle` users-only.

**Tech Stack:** rusqlite (rusqlite_migration), serde + schemars (MCP tool params), tokio (delivery loop), notify_debouncer_mini (allowlist watcher already in place — read-only here), teloxide.

**Spec:** `docs/superpowers/specs/2026-04-21-cron-per-target-delivery-design.md` (commit `aa82550`).

---

## Conventions

- Workspace root: `<repo root>`. All paths below are workspace-relative.
- All Rust crates use edition 2024.
- Tests run with `cargo test -p <crate>` (single crate scope keeps cycle short). Final smoke: `cargo build --workspace`.
- Commits: conventional commits (`feat`, `fix`, `refactor`, `test`, `docs`). Match the recent history style on the `reflect` branch.
- Never use `#[ignore]`. Tests that need a live OpenShell sandbox use `rightclaw::test_support::TestSandbox::create("<test-name>")` per `ARCHITECTURE.md`.

---

## File Map

**Created:**
- `crates/rightclaw/src/memory/sql/v17_cron_target.sql` — migration SQL constant text (used by hook for idempotent ADD COLUMN).

**Modified:**
- `crates/rightclaw/src/memory/migrations.rs` — register v17 hook, update tests.
- `crates/rightclaw/src/agent/allowlist.rs` — add `is_chat_allowed(chat_id) -> bool` on `AllowlistState`.
- `crates/rightclaw/src/cron_spec.rs` — `create_spec_v2`, `update_spec_partial`, `list_specs`, validation helper.
- `crates/rightclaw-cli/src/memory_server.rs` — `CronCreateParams`, `CronUpdateParams` add target fields; `deserialize_double_option_i32` helper.
- `crates/rightclaw-cli/src/right_backend.rs` — `call_cron_create` / `call_cron_update` validate target against allowlist (read `allowlist.yaml` on demand).
- `crates/bot/src/telegram/attachments.rs` — `format_cc_input` always emits `chat: { kind, id }` (DM gets `kind: dm`).
- `crates/bot/src/cron_delivery.rs` — `PendingCronResult` adds target fields; `fetch_pending` / `deduplicate_job` JOIN cron_specs; `run_delivery_loop` / `deliver_through_session` route per-target; `notify_chat_ids` removed.
- `crates/bot/src/lib.rs` — drop `notify_chat_ids` / `delivery_chat_ids` locals; pass `AllowlistHandle` to OAuth state and delivery loop.
- `crates/bot/src/telegram/oauth_callback.rs` — `OAuthCallbackState.notify_chat_ids` → `allowlist: AllowlistHandle`; notify users only.
- `crates/rightclaw/src/doctor.rs` — bump expected schema version to 17; add `check_cron_targets(agent_dir)`.
- `templates/right/prompt/OPERATING_INSTRUCTIONS.md` — add `target_chat_id` guidance for cron tools.
- `skills/rightcron/SKILL.md` — same guidance.

---

## Task 1: Schema migration v17 — add target columns

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v17_cron_target.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`

- [ ] **Step 1: Write the failing test**

Append at the bottom of the existing `tests` module in `crates/rightclaw/src/memory/migrations.rs`:

```rust
#[test]
fn v17_adds_cron_target_columns() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let chat_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = 'target_chat_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(chat_present, 1, "target_chat_id column missing");
    let thread_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = 'target_thread_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(thread_present, 1, "target_thread_id column missing");
}

#[test]
fn v17_is_idempotent_on_rerun() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    // Manually re-run the v17 hook; it must not error.
    let tx = conn.transaction().unwrap();
    super::v17_cron_target(&tx).unwrap();
    tx.commit().unwrap();
}

#[test]
fn v17_existing_rows_get_null_target() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
         VALUES ('legacy', '*/5 * * * *', 'p', 1.0, ?1, ?1)",
        [&now],
    )
    .unwrap();
    let target: Option<i64> = conn
        .query_row(
            "SELECT target_chat_id FROM cron_specs WHERE job_name = 'legacy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(target.is_none(), "legacy row should have NULL target_chat_id");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib memory::migrations::tests::v17_ -- --nocapture`
Expected: 3 failures — column missing / function `v17_cron_target` not in scope.

- [ ] **Step 3: Create the SQL constant file**

Create `crates/rightclaw/src/memory/sql/v17_cron_target.sql`:

```sql
-- V17: Per-cron delivery target. Columns are nullable for back-compat with
-- rows that existed before this migration; the MCP layer enforces presence
-- on new inserts. NULL rows are surfaced by `doctor::check_cron_targets`.
ALTER TABLE cron_specs ADD COLUMN target_chat_id   INTEGER;
ALTER TABLE cron_specs ADD COLUMN target_thread_id INTEGER;
```

- [ ] **Step 4: Add the idempotent hook + register the migration**

In `crates/rightclaw/src/memory/migrations.rs`, after the existing `v16_usage_api_key_source` function (around line 115), add:

```rust
/// v17: Add target_chat_id and target_thread_id to cron_specs.
///
/// Idempotent — checks pragma_table_info before each ALTER. Both columns
/// are nullable; the MCP layer validates presence on new rows. NULL on
/// existing rows is surfaced by `doctor::check_cron_targets`.
fn v17_cron_target(tx: &Transaction) -> Result<(), HookError> {
    let has_column = |col: &str| -> Result<bool, rusqlite::Error> {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = ?1",
            [col],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    };

    if !has_column("target_chat_id")? {
        tx.execute_batch("ALTER TABLE cron_specs ADD COLUMN target_chat_id INTEGER")?;
    }
    if !has_column("target_thread_id")? {
        tx.execute_batch("ALTER TABLE cron_specs ADD COLUMN target_thread_id INTEGER")?;
    }
    Ok(())
}
```

Then in the `MIGRATIONS` `LazyLock` block, append after the v16 line:

```rust
        M::up_with_hook("", v17_cron_target),
```

The block becomes:

```rust
pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> = std::sync::LazyLock::new(|| {
    Migrations::new(vec![
        M::up(V1_SCHEMA),
        M::up(V2_SCHEMA),
        M::up(V3_SCHEMA),
        M::up(V4_SCHEMA),
        M::up(V5_SCHEMA),
        M::up(V6_SCHEMA),
        M::up(V7_SCHEMA),
        M::up(V8_SCHEMA),
        M::up(V9_SCHEMA),
        M::up(V10_SCHEMA),
        M::up_with_hook("", v12_cron_diagnostics),
        M::up_with_hook("", v13_one_shot_cron),
        M::up(V14_SCHEMA),
        M::up(V15_SCHEMA),
        M::up_with_hook("", v16_usage_api_key_source),
        M::up_with_hook("", v17_cron_target),
    ])
});
```

(The `V17_SCHEMA` SQL file is not directly included via `include_str!` because the hook does the column-existence check itself. The `.sql` file is documentation-only — same pattern v13 uses.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib memory::migrations::tests::v17_ -- --nocapture`
Expected: 3 passes.

Also run the full migrations suite to catch regressions:

`cargo test -p rightclaw --lib memory::migrations`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v17_cron_target.sql crates/rightclaw/src/memory/migrations.rs
git commit -m "feat(schema): v17 — add target_chat_id/target_thread_id to cron_specs"
```

---

## Task 2: AllowlistState — `is_chat_allowed` helper

**Files:**
- Modify: `crates/rightclaw/src/agent/allowlist.rs`
- Modify: `crates/rightclaw/src/agent/allowlist_tests.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/rightclaw/src/agent/allowlist_tests.rs`:

```rust
#[test]
fn is_chat_allowed_matches_user_or_group() {
    let now = chrono::Utc::now();
    let mut state = AllowlistState::default();
    state.add_user(AllowedUser { id: 100, label: None, added_by: None, added_at: now });
    state.add_group(AllowedGroup { id: -200, label: None, opened_by: None, opened_at: now });

    assert!(state.is_chat_allowed(100), "trusted user must match");
    assert!(state.is_chat_allowed(-200), "open group must match");
    assert!(!state.is_chat_allowed(999), "stranger must not match");
    assert!(!state.is_chat_allowed(-999), "unknown group must not match");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib agent::allowlist::tests::is_chat_allowed_matches_user_or_group`
Expected: FAIL — method not found.

- [ ] **Step 3: Implement the helper**

In `crates/rightclaw/src/agent/allowlist.rs`, in the `impl AllowlistState` block (around line 197, just after `is_group_open`), add:

```rust
    /// True iff the given chat id is either a trusted user or an opened group.
    /// Used by cron target validation and by anything else that needs a
    /// uniform "is this a valid Telegram destination for this agent?" check.
    pub fn is_chat_allowed(&self, chat_id: i64) -> bool {
        self.is_user_trusted(chat_id) || self.is_group_open(chat_id)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rightclaw --lib agent::allowlist::tests::is_chat_allowed_matches_user_or_group`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/agent/allowlist.rs crates/rightclaw/src/agent/allowlist_tests.rs
git commit -m "feat(allowlist): is_chat_allowed unifies user + group lookup"
```

---

## Task 3: cron_spec — extend `create_spec_v2` with target fields

**Files:**
- Modify: `crates/rightclaw/src/cron_spec.rs`

The existing `create_spec_v2` signature already has `#[allow(clippy::too_many_arguments)]` so adding two more is fine. Validation against allowlist is done by callers (the MCP layer in `right_backend.rs`); this function only persists.

- [ ] **Step 1: Write the failing test**

Append inside the `#[cfg(test)] mod tests { ... }` block of `crates/rightclaw/src/cron_spec.rs`:

```rust
#[test]
fn create_spec_v2_persists_target_fields() {
    let conn = rightclaw::memory::open_connection(tempfile::tempdir().unwrap().path(), true).unwrap();
    create_spec_v2(
        &conn,
        "with-target",
        Some("*/5 * * * *"),
        "do thing",
        None,
        None,
        None,
        None,
        Some(-100),         // target_chat_id
        Some(7),            // target_thread_id
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
    let conn = rightclaw::memory::open_connection(tempfile::tempdir().unwrap().path(), true).unwrap();
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
```

(The `tempfile` crate is already a dev-dep — confirmed by existing tests in this module.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib cron_spec::tests::create_spec_v2_persists_`
Expected: FAILs — `create_spec_v2` takes 8 args, callers pass 10.

- [ ] **Step 3: Update `create_spec_v2`**

In `crates/rightclaw/src/cron_spec.rs` line 224, change the function:

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
        resolve_schedule_fields(schedule, recurring, run_at)?;

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

- [ ] **Step 4: Update existing call sites in tests**

Within `crates/rightclaw/src/cron_spec.rs` `mod tests`, every existing `create_spec_v2(...)` call needs two trailing `None, None` args. There are several. Search-and-replace each occurrence in the test module to add `, None, None` immediately before the closing paren.

The existing calls are at (approximate line numbers): 1099, 1109, 1119, 1129, 1139, 1149, 1160, 1161, 1162, 1173, 1184, 1193, 1202, 1213. Update every one. (Run the failing test below to confirm none were missed.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib cron_spec::tests`
Expected: all green, including the two new tests.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/cron_spec.rs
git commit -m "feat(cron_spec): create_spec_v2 persists target_chat_id/target_thread_id"
```

---

## Task 4: cron_spec — extend `update_spec_partial` with target fields

**Files:**
- Modify: `crates/rightclaw/src/cron_spec.rs`

`target_chat_id` is `Option<i64>` (omitted → leave as is; provided → overwrite). `target_thread_id` is `Option<Option<i64>>` (outer = "field present?"; inner = "set vs clear to NULL"). The double-Option lets callers explicitly clear the thread without losing the "no change" signal.

- [ ] **Step 1: Write the failing tests**

Append inside the `mod tests` block:

```rust
#[test]
fn update_spec_partial_sets_target_chat_id() {
    let conn = rightclaw::memory::open_connection(tempfile::tempdir().unwrap().path(), true).unwrap();
    create_spec_v2(&conn, "j1", Some("*/5 * * * *"), "p", None, None, None, None, None, None).unwrap();
    update_spec_partial(
        &conn, "j1", None, None, None, None, None, None,
        Some(-555), None,
    ).unwrap();
    let chat: Option<i64> = conn
        .query_row("SELECT target_chat_id FROM cron_specs WHERE job_name='j1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(chat, Some(-555));
}

#[test]
fn update_spec_partial_clears_target_thread_id() {
    let conn = rightclaw::memory::open_connection(tempfile::tempdir().unwrap().path(), true).unwrap();
    create_spec_v2(&conn, "j1", Some("*/5 * * * *"), "p", None, None, None, None, Some(-1), Some(42)).unwrap();
    // Outer Some = field present; inner None = clear to NULL.
    update_spec_partial(
        &conn, "j1", None, None, None, None, None, None,
        None, Some(None),
    ).unwrap();
    let thread: Option<i64> = conn
        .query_row("SELECT target_thread_id FROM cron_specs WHERE job_name='j1'", [], |r| r.get(0))
        .unwrap();
    assert!(thread.is_none(), "thread must be cleared");
}

#[test]
fn update_spec_partial_leaves_target_when_omitted() {
    let conn = rightclaw::memory::open_connection(tempfile::tempdir().unwrap().path(), true).unwrap();
    create_spec_v2(&conn, "j1", Some("*/5 * * * *"), "p", None, None, None, None, Some(-1), Some(42)).unwrap();
    // Update only the prompt; targets must stay.
    update_spec_partial(
        &conn, "j1", None, None, Some("new prompt"), None, None, None,
        None, None,
    ).unwrap();
    let (chat, thread): (Option<i64>, Option<i64>) = conn
        .query_row("SELECT target_chat_id, target_thread_id FROM cron_specs WHERE job_name='j1'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(chat, Some(-1));
    assert_eq!(thread, Some(42));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib cron_spec::tests::update_spec_partial_`
Expected: FAILs — wrong arity.

- [ ] **Step 3: Update `update_spec_partial`**

In `crates/rightclaw/src/cron_spec.rs` line 309, modify the signature and the dynamic UPDATE builder:

```rust
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
```

- [ ] **Step 4: Update existing test call sites**

Search the `mod tests` block for all `update_spec_partial(...)` calls and append `, None, None` before the closing paren on each. Approximate line numbers: 1173, 1184, 1193, 1202, 1213, plus any test added since this plan was written.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib cron_spec::tests`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/cron_spec.rs
git commit -m "feat(cron_spec): update_spec_partial supports target_chat_id/thread_id"
```

---

## Task 5: cron_spec — `list_specs` exposes target columns

**Files:**
- Modify: `crates/rightclaw/src/cron_spec.rs`

- [ ] **Step 1: Write the failing test**

Append inside the `mod tests` block:

```rust
#[test]
fn list_specs_includes_target_fields() {
    let conn = rightclaw::memory::open_connection(tempfile::tempdir().unwrap().path(), true).unwrap();
    create_spec_v2(&conn, "j1", Some("*/5 * * * *"), "p", None, None, None, None, Some(-100), Some(5)).unwrap();
    let json = list_specs(&conn).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let row = &value.as_array().unwrap()[0];
    assert_eq!(row["target_chat_id"].as_i64(), Some(-100));
    assert_eq!(row["target_thread_id"].as_i64(), Some(5));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib cron_spec::tests::list_specs_includes_target_fields`
Expected: FAIL — fields missing in JSON.

- [ ] **Step 3: Extend `list_specs`**

In `crates/rightclaw/src/cron_spec.rs` line 449, update both the SELECT and the JSON-row construction:

```rust
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
    serde_json::to_string_pretty(&rows)
        .map_err(|e| format!("serialization error: {e:#}"))
}
```

Note the column index shift for `last_run_at` / `last_status` (now 11/12, were 9/10).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rightclaw --lib cron_spec::tests::list_specs_includes_target_fields`
Expected: PASS.

Run the full module to catch index-shift bugs: `cargo test -p rightclaw --lib cron_spec::tests`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/cron_spec.rs
git commit -m "feat(cron_spec): list_specs surfaces target fields"
```

---

## Task 6: MCP — `CronCreateParams` + allowlist validation

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server.rs`
- Modify: `crates/rightclaw-cli/src/right_backend.rs`
- Test: `crates/rightclaw-cli/src/right_backend.rs` (`#[cfg(test)]` module — extend if present, else add one)

The aggregator process reads `~/.rightclaw/agents/<name>/allowlist.yaml` on demand via `rightclaw::agent::allowlist::read_file()`. No cache, no watcher.

- [ ] **Step 1: Write the failing test**

In `crates/rightclaw-cli/src/right_backend.rs`, find or add a `#[cfg(test)] mod tests { ... }` at the bottom of the file. Add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rightclaw::agent::allowlist::{AllowedUser, AllowlistFile};

    fn write_allowlist(agent_dir: &std::path::Path, users: &[i64], groups: &[i64]) {
        let now = chrono::Utc::now();
        let mut file = AllowlistFile::default();
        for &id in users {
            file.users.push(AllowedUser { id, label: None, added_by: None, added_at: now });
        }
        for &id in groups {
            file.groups.push(rightclaw::agent::allowlist::AllowedGroup {
                id, label: None, opened_by: None, opened_at: now,
            });
        }
        rightclaw::agent::allowlist::write_file(agent_dir, &file).unwrap();
    }

    #[tokio::test]
    async fn cron_create_rejects_target_not_in_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().to_path_buf();
        let agent_dir = agents_dir.join("a1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        write_allowlist(&agent_dir, &[100], &[]);
        // Initialize the agent's data.db so get_conn succeeds.
        rightclaw::memory::open_connection(&agent_dir, true).unwrap();

        let backend = RightBackend::new(agents_dir.clone(), None);
        let args = serde_json::json!({
            "job_name": "j1",
            "schedule": "*/5 * * * *",
            "prompt": "p",
            "target_chat_id": -999,
        });
        let result = backend.tools_call("a1", &agent_dir, "cron_create", args).await.unwrap();
        // CallToolResult::error path renders the error string in `content[0].text`.
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        assert!(
            text.contains("not in allowlist") || text.contains("-999"),
            "expected allowlist rejection, got: {text}"
        );
    }

    #[tokio::test]
    async fn cron_create_accepts_target_in_allowlist_group() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().to_path_buf();
        let agent_dir = agents_dir.join("a1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        write_allowlist(&agent_dir, &[], &[-200]);
        rightclaw::memory::open_connection(&agent_dir, true).unwrap();

        let backend = RightBackend::new(agents_dir.clone(), None);
        let args = serde_json::json!({
            "job_name": "j1",
            "schedule": "*/5 * * * *",
            "prompt": "p",
            "target_chat_id": -200,
            "target_thread_id": 7,
        });
        let result = backend.tools_call("a1", &agent_dir, "cron_create", args).await.unwrap();
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        assert!(text.contains("Created"), "got: {text}");
    }

    #[tokio::test]
    async fn cron_create_rejects_missing_target_chat_id() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().to_path_buf();
        let agent_dir = agents_dir.join("a1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        write_allowlist(&agent_dir, &[100], &[]);
        rightclaw::memory::open_connection(&agent_dir, true).unwrap();

        let backend = RightBackend::new(agents_dir.clone(), None);
        let args = serde_json::json!({
            "job_name": "j1",
            "schedule": "*/5 * * * *",
            "prompt": "p",
            // target_chat_id deliberately omitted
        });
        let result = backend.tools_call("a1", &agent_dir, "cron_create", args).await;
        assert!(result.is_err(), "missing required field must surface as error");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-cli --lib right_backend::tests`
Expected: FAILs (or compile errors — `target_chat_id` field absent on `CronCreateParams`).

- [ ] **Step 3: Add fields to `CronCreateParams`**

In `crates/rightclaw-cli/src/memory_server.rs` line 30 (`CronCreateParams`), append:

```rust
    #[schemars(
        description = "Telegram chat id to deliver this cron's results to. Required. For DMs use the user_id; for groups use the negative chat id. Must be present in the agent's allowlist (allowlist.yaml). Read this from the `chat.id` field in the incoming message YAML unless the user explicitly asks for a different chat."
    )]
    pub target_chat_id: i64,
    #[schemars(
        description = "Optional supergroup topic (message_thread_id). Set only when the cron should reply to a specific topic; leave unset for ordinary chat delivery."
    )]
    pub target_thread_id: Option<i64>,
```

- [ ] **Step 4: Validate target in `call_cron_create`**

In `crates/rightclaw-cli/src/right_backend.rs`, replace `call_cron_create` (around line 154) with:

```rust
    fn call_cron_create(
        &self,
        agent_name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronCreateParams =
            serde_json::from_value(args.clone()).context("invalid cron_create params")?;
        let agent_dir = self.agents_dir.join(agent_name);
        if let Err(msg) = validate_target_against_allowlist(&agent_dir, params.target_chat_id) {
            return Ok(CallToolResult::error(vec![Content::text(msg)]));
        }
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
            Some(params.target_chat_id),
            params.target_thread_id,
        )
        .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(
            rightclaw::cron_spec::format_result(&result),
        )]))
    }
```

Add the helper at the bottom of `impl RightBackend` (or as a free fn at module scope — pick free fn for testability):

```rust
/// Validate that `chat_id` is in the agent's allowlist (users or groups).
/// Reads `allowlist.yaml` on demand from `agent_dir`.
fn validate_target_against_allowlist(agent_dir: &Path, chat_id: i64) -> Result<(), String> {
    let file = match rightclaw::agent::allowlist::read_file(agent_dir) {
        Ok(Some(f)) => f,
        Ok(None) => {
            return Err(format!(
                "target_chat_id {chat_id} cannot be validated: allowlist.yaml does not exist for this agent"
            ));
        }
        Err(e) => {
            return Err(format!(
                "target_chat_id {chat_id} cannot be validated: failed to read allowlist.yaml: {e}"
            ));
        }
    };
    let state = rightclaw::agent::allowlist::AllowlistState::from_file(file);
    if state.is_chat_allowed(chat_id) {
        Ok(())
    } else {
        Err(format!(
            "target_chat_id {chat_id} is not in allowlist; use /allow (DM) or /allow_all (group) from a trusted account first"
        ))
    }
}
```

(`Path` is already in scope at the top of the file — line 9.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw-cli --lib right_backend::tests`
Expected: 3 passes.

Then run the wider suite:

`cargo test -p rightclaw-cli --lib`
Expected: all green (existing memory_server tests already pass `CronCreateParams` JSON — they will fail if we forgot to update; if so, add `"target_chat_id": <some allowlist id>` to those JSON payloads).

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server.rs crates/rightclaw-cli/src/right_backend.rs
git commit -m "feat(mcp): cron_create requires + validates target_chat_id"
```

---

## Task 7: MCP — `CronUpdateParams` + double-Option for thread

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server.rs`
- Modify: `crates/rightclaw-cli/src/right_backend.rs`

The double-`Option<Option<i64>>` distinguishes "field absent" (no change) from "explicit `null`" (clear to NULL). We use a small `deserialize_some` helper.

- [ ] **Step 1: Write the failing tests**

In the same `right_backend::tests` module, append:

```rust
    #[tokio::test]
    async fn cron_update_changes_target_chat_id_with_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().to_path_buf();
        let agent_dir = agents_dir.join("a1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        write_allowlist(&agent_dir, &[100], &[-200, -300]);
        rightclaw::memory::open_connection(&agent_dir, true).unwrap();

        let backend = RightBackend::new(agents_dir.clone(), None);
        backend
            .tools_call(
                "a1",
                &agent_dir,
                "cron_create",
                serde_json::json!({
                    "job_name": "j1",
                    "schedule": "*/5 * * * *",
                    "prompt": "p",
                    "target_chat_id": -200,
                }),
            )
            .await
            .unwrap();

        let result = backend
            .tools_call(
                "a1",
                &agent_dir,
                "cron_update",
                serde_json::json!({
                    "job_name": "j1",
                    "target_chat_id": -300,
                }),
            )
            .await
            .unwrap();
        let text = result.content.first().and_then(|c| c.as_text()).map(|t| t.text.clone()).unwrap_or_default();
        assert!(text.contains("Updated"), "got: {text}");

        // Reject change to non-allowlisted chat
        let denied = backend
            .tools_call(
                "a1",
                &agent_dir,
                "cron_update",
                serde_json::json!({
                    "job_name": "j1",
                    "target_chat_id": -999,
                }),
            )
            .await
            .unwrap();
        let denied_text = denied.content.first().and_then(|c| c.as_text()).map(|t| t.text.clone()).unwrap_or_default();
        assert!(denied_text.contains("not in allowlist"), "got: {denied_text}");
    }

    #[tokio::test]
    async fn cron_update_clears_target_thread_id_with_explicit_null() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().to_path_buf();
        let agent_dir = agents_dir.join("a1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        write_allowlist(&agent_dir, &[], &[-200]);
        rightclaw::memory::open_connection(&agent_dir, true).unwrap();

        let backend = RightBackend::new(agents_dir.clone(), None);
        backend
            .tools_call(
                "a1",
                &agent_dir,
                "cron_create",
                serde_json::json!({
                    "job_name": "j1",
                    "schedule": "*/5 * * * *",
                    "prompt": "p",
                    "target_chat_id": -200,
                    "target_thread_id": 7,
                }),
            )
            .await
            .unwrap();

        backend
            .tools_call(
                "a1",
                &agent_dir,
                "cron_update",
                serde_json::json!({
                    "job_name": "j1",
                    "target_thread_id": null,
                }),
            )
            .await
            .unwrap();

        let conn = rightclaw::memory::open_connection(&agent_dir, false).unwrap();
        let thread: Option<i64> = conn
            .query_row("SELECT target_thread_id FROM cron_specs WHERE job_name='j1'", [], |r| r.get(0))
            .unwrap();
        assert!(thread.is_none(), "explicit null must clear the column");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-cli --lib right_backend::tests::cron_update_`
Expected: FAILs.

- [ ] **Step 3: Add fields + helper to `CronUpdateParams`**

In `crates/rightclaw-cli/src/memory_server.rs` line 55 (`CronUpdateParams`), append fields and add the helper:

```rust
    #[schemars(
        description = "New target_chat_id. Must be in the agent's allowlist."
    )]
    pub target_chat_id: Option<i64>,
    #[schemars(
        description = "New target_thread_id. Pass `null` to clear (cron will deliver to the chat without a topic). Omit the field entirely to leave unchanged."
    )]
    #[serde(default, deserialize_with = "deserialize_double_option_i64")]
    pub target_thread_id: Option<Option<i64>>,
```

Add the deserializer below `deserialize_lenient_f64` (around line 118):

```rust
/// Distinguish between "field absent" (`None`) and "explicit null" (`Some(None)`)
/// for nullable optional integers. Required so `cron_update` can clear a field.
fn deserialize_double_option_i64<'de, D>(deserializer: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Option<i64>>::deserialize(deserializer).map(Some)
}
```

JSON Schema generation: since `Option<Option<i64>>` is `nullable` already, the default `JsonSchema` derive yields `{ "type": ["integer", "null"] }`, which is what we want. The `#[serde(default, deserialize_with = "...")]` plus `Option` outer makes "absent" round-trip cleanly.

- [ ] **Step 4: Update `call_cron_update`**

Replace `call_cron_update` in `crates/rightclaw-cli/src/right_backend.rs` (around line 179):

```rust
    fn call_cron_update(
        &self,
        agent_name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronUpdateParams =
            serde_json::from_value(args.clone()).context("invalid cron_update params")?;
        let agent_dir = self.agents_dir.join(agent_name);
        if let Some(chat) = params.target_chat_id
            && let Err(msg) = validate_target_against_allowlist(&agent_dir, chat)
        {
            return Ok(CallToolResult::error(vec![Content::text(msg)]));
        }
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
            params.target_chat_id,
            params.target_thread_id,
        )
        .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(
            rightclaw::cron_spec::format_result(&result),
        )]))
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw-cli --lib right_backend::tests`
Expected: all green.

`cargo test -p rightclaw-cli --lib`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server.rs crates/rightclaw-cli/src/right_backend.rs
git commit -m "feat(mcp): cron_update accepts target fields with explicit-null clear"
```

---

## Task 8: YAML input — DM emits `chat: { kind, id }`

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`

We always emit a `chat:` block. DM gets `kind: dm` + `id`; group keeps `kind: group` + `id` + optional `title`/`topic_id`. This guarantees the agent sees `chat.id` for both modes.

- [ ] **Step 1: Update the existing test that asserts "no chat block in DM"**

In `crates/bot/src/telegram/attachments.rs` test at `dm_single_message_emits_yaml_with_no_chat_block` (line ~2132), invert the assertion:

```rust
    #[test]
    fn dm_single_message_emits_yaml_with_chat_block() {
        // (rename the test if convenient; the key is the assertion change)
        let msgs = vec![InputMessage {
            message_id: 1,
            text: Some("hi".into()),
            timestamp: chrono::Utc::now(),
            attachments: vec![],
            author: MessageAuthor { name: "Andrey".into(), username: None, user_id: Some(42) },
            forward_info: None,
            reply_to_id: None,
            chat: ChatContext::Private,
            reply_to_body: None,
        }];
        let yaml = format_cc_input(&msgs).unwrap();
        assert!(yaml.contains("chat:"), "DM must now include a chat block, got:\n{yaml}");
        assert!(yaml.contains("kind: dm"), "DM block must mark kind: dm, got:\n{yaml}");
        assert!(yaml.contains("id: 42") || yaml.contains("id: "), "DM chat block must include id, got:\n{yaml}");
    }
```

(Note: in DM the `chat_id` Telegram-side equals the user's `id`. If `InputMessage` already carries chat id via author, use that; otherwise extend the struct in the next step.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw-bot --lib telegram::attachments::tests::dm_single_message_emits_yaml_with_chat_block`
Expected: FAIL — `ChatContext::Private` produces no chat block.

- [ ] **Step 3: Carry chat_id on `ChatContext::Private`**

Modify `ChatContext` in `crates/bot/src/telegram/attachments.rs` (line ~401):

```rust
#[derive(Debug, Clone)]
pub enum ChatContext {
    Private { id: i64 },
    Group {
        id: i64,
        title: Option<String>,
        topic_id: Option<i64>,
    },
}
```

Find every constructor of `ChatContext::Private` in the workspace (`grep -nR "ChatContext::Private"`) and pass the chat id. The primary call site is in the worker / handler where messages are first wrapped — populate from `msg.chat.id.0`.

For `format_cc_input`, replace the chat-block branch (line ~478):

```rust
        // Chat block — always present.
        out.push_str("    chat:\n");
        match &m.chat {
            ChatContext::Private { id } => {
                writeln!(out, "      kind: dm").expect("infallible");
                writeln!(out, "      id: {id}").expect("infallible");
            }
            ChatContext::Group { id, title, topic_id } => {
                writeln!(out, "      kind: group").expect("infallible");
                writeln!(out, "      id: {id}").expect("infallible");
                if let Some(t) = title {
                    writeln!(out, "      title: \"{}\"", yaml_escape_string(t)).expect("infallible");
                }
                if let Some(tid) = topic_id {
                    writeln!(out, "      topic_id: {tid}").expect("infallible");
                }
            }
        }
```

- [ ] **Step 4: Update the existing group test**

The test `dm_single_message_emits_yaml_with_no_chat_block` was renamed in Step 1; the existing group test (line ~2178) should still pass since we kept its YAML shape. Re-run to confirm:

`cargo test -p rightclaw-bot --lib telegram::attachments`
Expected: all green.

- [ ] **Step 5: Fix downstream compile errors**

Run `cargo build -p rightclaw-bot` — expect compile errors at every `ChatContext::Private` constructor. Fix each one (e.g. `ChatContext::Private { id: msg.chat.id.0 }`). Likely sites:
- `crates/bot/src/telegram/handler.rs` — message intake.
- `crates/bot/src/telegram/worker.rs` — debounce buffer.
- Any test fixtures that build `InputMessage`.

- [ ] **Step 6: Run the bot crate tests**

Run: `cargo test -p rightclaw-bot --lib`
Expected: all green (or only failures unrelated to this task — fix any introduced).

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(yaml): always emit chat:{kind,id} block (DM + group)"
```

---

## Task 9: Delivery — `PendingCronResult` carries target fields

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs`

`fetch_pending` and `deduplicate_job` JOIN `cron_specs` to read the (current) target. JOIN-time values mean: if the operator updated the target between the cron run and its delivery, the new target wins. That matches the operator's intent (they just changed where they want it).

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block in `crates/bot/src/cron_delivery.rs`:

```rust
    #[test]
    fn fetch_pending_carries_target_fields() {
        let (_dir, conn) = setup_db();
        // Seed cron_specs with target so the JOIN finds it.
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('job1', '*/5 * * * *', 'p', 1.0, -555, 9, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.target_chat_id, Some(-555));
        assert_eq!(pending.target_thread_id, Some(9));
    }

    #[test]
    fn fetch_pending_returns_none_target_when_spec_has_none() {
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('legacy', '*/5 * * * *', 'p', 1.0, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'legacy', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert!(pending.target_chat_id.is_none());
        assert!(pending.target_thread_id.is_none());
    }

    #[test]
    fn deduplicate_job_carries_target_fields() {
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('job1', '*/5 * * * *', 'p', 1.0, -100, NULL, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum1', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job1', '2026-01-01T00:05:00Z', '2026-01-01T00:06:00Z', 'success', '/log', 'sum2', '{\"content\":\"y\"}')",
            [],
        ).unwrap();
        let (latest, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest.id, "b");
        assert_eq!(skipped, 1);
        assert_eq!(latest.target_chat_id, Some(-100));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot --lib cron_delivery::tests::fetch_pending_carries cron_delivery::tests::fetch_pending_returns_none_target cron_delivery::tests::deduplicate_job_carries`
Expected: FAILs — `target_chat_id` not on `PendingCronResult`.

- [ ] **Step 3: Extend `PendingCronResult` and the queries**

In `crates/bot/src/cron_delivery.rs` (line ~12), modify the struct:

```rust
#[derive(Debug)]
pub struct PendingCronResult {
    pub id: String,
    pub job_name: String,
    pub notify_json: String,
    pub summary: String,
    pub finished_at: String,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
}
```

Replace `fetch_pending` (line ~21):

```rust
pub fn fetch_pending(
    conn: &rusqlite::Connection,
) -> Result<Option<PendingCronResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT cr.id, cr.job_name, cr.notify_json, cr.summary, cr.finished_at, \
                cs.target_chat_id, cs.target_thread_id \
         FROM cron_runs cr \
         LEFT JOIN cron_specs cs ON cs.job_name = cr.job_name \
         WHERE cr.status IN ('success', 'failed') AND cr.notify_json IS NOT NULL AND cr.delivered_at IS NULL \
         ORDER BY cr.finished_at ASC LIMIT 1",
    )?;
    let result = stmt.query_row([], |row| {
        Ok(PendingCronResult {
            id: row.get(0)?,
            job_name: row.get(1)?,
            notify_json: row.get(2)?,
            summary: row.get(3)?,
            finished_at: row.get(4)?,
            target_chat_id: row.get(5)?,
            target_thread_id: row.get(6)?,
        })
    });
    match result {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}
```

Replace `deduplicate_job` (line ~63) similarly — update the SELECT to add the JOIN + target columns, and the row mapper to populate them. Remember the `summary`/`finished_at` fallbacks the existing function uses (`Option<String>::unwrap_or_default()`):

```rust
pub fn deduplicate_job(
    conn: &rusqlite::Connection,
    job_name: &str,
) -> Result<Option<(PendingCronResult, u32)>, rusqlite::Error> {
    let latest = conn
        .query_row(
            "SELECT cr.id, cr.job_name, cr.notify_json, cr.summary, cr.finished_at, \
                    cs.target_chat_id, cs.target_thread_id \
             FROM cron_runs cr \
             LEFT JOIN cron_specs cs ON cs.job_name = cr.job_name \
             WHERE cr.job_name = ?1 AND cr.status IN ('success', 'failed') AND cr.notify_json IS NOT NULL AND cr.delivered_at IS NULL \
             ORDER BY cr.finished_at DESC LIMIT 1",
            rusqlite::params![job_name],
            |row| {
                Ok(PendingCronResult {
                    id: row.get(0)?,
                    job_name: row.get(1)?,
                    notify_json: row.get(2)?,
                    summary: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    finished_at: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    target_chat_id: row.get(5)?,
                    target_thread_id: row.get(6)?,
                })
            },
        )
        .optional()?;

    let Some(latest) = latest else {
        return Ok(None);
    };

    let now = chrono::Utc::now().to_rfc3339();
    let count = conn.execute(
        "UPDATE cron_runs SET delivered_at = ?1, delivery_status = 'superseded' \
         WHERE job_name = ?2 AND id != ?3 \
         AND status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL",
        rusqlite::params![now, job_name, latest.id],
    )?;

    Ok(Some((latest, count as u32)))
}
```

- [ ] **Step 4: Update existing tests that build `PendingCronResult` literally**

Tests `format_cron_yaml_basic` and `format_cron_yaml_no_skipped` (lines ~707, ~729) construct `PendingCronResult { ... }` directly. Add `target_chat_id: None, target_thread_id: None,` to each.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot --lib cron_delivery::tests`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron_delivery.rs
git commit -m "feat(cron_delivery): JOIN cron_specs to surface target fields"
```

---

## Task 10: Delivery — route per-target, drop `notify_chat_ids`

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs`

The loop now branches on `target_chat_id`:
- `None` → log WARN with `cron_update` hint; `mark_delivery_outcome(.., "no_target")`.
- `Some(id)` not in allowlist → log WARN; `mark_delivery_outcome(.., "denied")`.
- `Some(id)` in allowlist → call `deliver_through_session` with single `(chat_id, thread_id)`.

`deliver_through_session` loses `notify_chat_ids: &[i64]` and gains `target_chat_id: i64, target_thread_id: Option<i64>`. The two `for &cid in notify_chat_ids` loops collapse to single sends.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests`:

```rust
    #[test]
    fn null_target_marks_no_target_status() {
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('legacy', '*/5 * * * *', 'p', 1.0, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'legacy', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        // Function under test: a small helper extracted from run_delivery_loop —
        // see Step 3 below.
        let outcome = classify_pending_target(&conn, &fetch_pending(&conn).unwrap().unwrap(),
            &fake_allowlist(&[], &[])).unwrap();
        assert!(matches!(outcome, TargetClassification::NoTarget), "got: {outcome:?}");
    }

    #[test]
    fn target_not_in_allowlist_marks_denied() {
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, target_chat_id, created_at, updated_at) \
             VALUES ('agenda', '*/5 * * * *', 'p', 1.0, -777, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'agenda', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        let outcome = classify_pending_target(&conn, &fetch_pending(&conn).unwrap().unwrap(),
            &fake_allowlist(&[100], &[-200])).unwrap();
        assert!(matches!(outcome, TargetClassification::Denied), "got: {outcome:?}");
    }

    #[test]
    fn target_in_allowlist_returns_ready() {
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('agenda', '*/5 * * * *', 'p', 1.0, -200, 5, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'agenda', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        let outcome = classify_pending_target(&conn, &fetch_pending(&conn).unwrap().unwrap(),
            &fake_allowlist(&[], &[-200])).unwrap();
        assert!(matches!(outcome, TargetClassification::Ready { chat_id: -200, thread_id: Some(5) }), "got: {outcome:?}");
    }

    fn fake_allowlist(users: &[i64], groups: &[i64]) -> rightclaw::agent::allowlist::AllowlistState {
        use rightclaw::agent::allowlist::{AllowedGroup, AllowedUser, AllowlistState};
        let now = chrono::Utc::now();
        let mut state = AllowlistState::default();
        for &id in users {
            state.add_user(AllowedUser { id, label: None, added_by: None, added_at: now });
        }
        for &id in groups {
            state.add_group(AllowedGroup { id, label: None, opened_by: None, opened_at: now });
        }
        state
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot --lib cron_delivery::tests::null_target cron_delivery::tests::target_not_in cron_delivery::tests::target_in_allowlist`
Expected: FAILs — `classify_pending_target` / `TargetClassification` don't exist.

- [ ] **Step 3: Add the classification helper**

In `crates/bot/src/cron_delivery.rs`, above `run_delivery_loop`, add:

```rust
/// Outcome of resolving a pending cron's delivery target against the live allowlist.
#[derive(Debug)]
pub enum TargetClassification {
    NoTarget,
    Denied,
    Ready {
        chat_id: i64,
        thread_id: Option<i64>,
    },
}

/// Classify a pending cron result. Pure function; no side effects.
pub fn classify_pending_target(
    _conn: &rusqlite::Connection,
    pending: &PendingCronResult,
    allowlist: &rightclaw::agent::allowlist::AllowlistState,
) -> Result<TargetClassification, rusqlite::Error> {
    match pending.target_chat_id {
        None => Ok(TargetClassification::NoTarget),
        Some(id) if !allowlist.is_chat_allowed(id) => Ok(TargetClassification::Denied),
        Some(id) => Ok(TargetClassification::Ready {
            chat_id: id,
            thread_id: pending.target_thread_id,
        }),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot --lib cron_delivery::tests::null_target cron_delivery::tests::target_not_in cron_delivery::tests::target_in_allowlist`
Expected: PASS.

- [ ] **Step 5: Rewrite `run_delivery_loop` and `deliver_through_session` signatures**

Modify the function bodies. New signatures:

```rust
#[allow(clippy::too_many_arguments)]
pub async fn run_delivery_loop(
    agent_dir: PathBuf,
    agent_name: String,
    bot: crate::telegram::BotType,
    allowlist: rightclaw::agent::allowlist::AllowlistHandle,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    internal_client: std::sync::Arc<rightclaw::mcp::internal_client::InternalClient>,
    shutdown: tokio_util::sync::CancellationToken,
    resolved_sandbox: Option<String>,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
)
```

Inside the loop, after the existing `idle_for` gate and `deduplicate_job` call:

```rust
        let allowlist_snapshot = {
            let guard = allowlist.0.read().expect("allowlist lock poisoned");
            guard.clone()
        };

        let classification = match classify_pending_target(&conn, &to_deliver, &allowlist_snapshot) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(run_id = %to_deliver.id, "classify_pending_target failed: {e:#}");
                continue;
            }
        };

        let (target_chat_id, target_thread_id) = match classification {
            TargetClassification::NoTarget => {
                tracing::warn!(
                    job = %to_deliver.job_name,
                    run_id = %to_deliver.id,
                    "cron has no target_chat_id — call cron_update to set one or recreate the cron in the desired chat"
                );
                if let Err(e) = mark_delivery_outcome(&conn, &to_deliver.id, "no_target") {
                    tracing::error!(run_id = %to_deliver.id, "mark no_target failed: {e:#}");
                }
                continue;
            }
            TargetClassification::Denied => {
                tracing::warn!(
                    job = %to_deliver.job_name,
                    run_id = %to_deliver.id,
                    target_chat_id = ?to_deliver.target_chat_id,
                    "cron target chat is not in allowlist — skipping delivery"
                );
                if let Err(e) = mark_delivery_outcome(&conn, &to_deliver.id, "denied") {
                    tracing::error!(run_id = %to_deliver.id, "mark denied failed: {e:#}");
                }
                continue;
            }
            TargetClassification::Ready { chat_id, thread_id } => (chat_id, thread_id),
        };

        let session_id = match crate::telegram::session::get_active_session(&conn, target_chat_id, target_thread_id.unwrap_or(0)) {
            Ok(s) => s.map(|s| s.root_session_id),
            Err(e) => {
                tracing::error!("cron delivery: session lookup failed: {e:#}");
                None
            }
        };

        let yaml = format_cron_yaml(&to_deliver, skipped);
        tracing::info!(
            job = %to_deliver.job_name,
            run_id = %to_deliver.id,
            skipped,
            target_chat_id,
            ?target_thread_id,
            "delivering cron result through main session"
        );

        match deliver_through_session(
            &yaml,
            &agent_dir,
            &agent_name,
            &bot,
            target_chat_id,
            target_thread_id,
            ssh_config_path.as_deref(),
            session_id,
            &internal_client,
            resolved_sandbox.as_deref(),
            &upgrade_lock,
        )
        .await
        {
            Ok(()) => {
                if let Err(e) = mark_delivery_outcome(&conn, &to_deliver.id, "delivered") {
                    tracing::error!(run_id = %to_deliver.id, "delivery DB update failed: {e:#}");
                    delivered_in_memory.insert(to_deliver.id.clone());
                }
                let outbox_dir = agent_dir.join("outbox").join("cron").join(&to_deliver.id);
                if outbox_dir.exists()
                    && let Err(e) = std::fs::remove_dir_all(&outbox_dir)
                {
                    tracing::warn!(run_id = %to_deliver.id, "outbox cleanup failed: {e:#}");
                }
                idle_ts.0.store(
                    chrono::Utc::now().timestamp(),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            Err(e) => {
                let attempts = attempt_counts
                    .entry(to_deliver.id.clone())
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
                tracing::error!(
                    job = %to_deliver.job_name,
                    run_id = %to_deliver.id,
                    attempt = *attempts,
                    max = MAX_DELIVERY_ATTEMPTS,
                    "cron delivery failed: {e:#}"
                );
                if *attempts >= MAX_DELIVERY_ATTEMPTS {
                    tracing::warn!(
                        job = %to_deliver.job_name,
                        run_id = %to_deliver.id,
                        "giving up after {MAX_DELIVERY_ATTEMPTS} attempts, marking as delivered"
                    );
                    if let Err(e) = mark_delivery_outcome(&conn, &to_deliver.id, "failed") {
                        tracing::error!(run_id = %to_deliver.id, "delivery-failure DB update failed: {e:#}");
                        delivered_in_memory.insert(to_deliver.id.clone());
                    }
                    attempt_counts.remove(&to_deliver.id);
                }
            }
        }
```

Replace `deliver_through_session`:

```rust
#[allow(clippy::too_many_arguments)]
async fn deliver_through_session(
    yaml_input: &str,
    agent_dir: &Path,
    agent_name: &str,
    bot: &crate::telegram::BotType,
    target_chat_id: i64,
    target_thread_id: Option<i64>,
    ssh_config_path: Option<&Path>,
    session_id: Option<String>,
    internal_client: &rightclaw::mcp::internal_client::InternalClient,
    resolved_sandbox: Option<&str>,
    upgrade_lock: &tokio::sync::RwLock<()>,
) -> Result<(), String> {
    use std::process::Stdio;

    // Block while upgrade is running.
    let _upgrade_guard = upgrade_lock.read().await;

    // (The CC subprocess assembly is unchanged from the existing implementation —
    //  copy the existing block from the previous version verbatim, removing the
    //  `notify_chat_ids.is_empty()` guard at the top.)
    //
    // Replace the two `for &cid in notify_chat_ids` loops near the bottom with
    // single sends keyed off `target_chat_id`/`target_thread_id`:

    /* ... existing setup that builds the CC `cmd` ... */

    /* ... existing wait_with_output and reply parsing ... */

    if let Some(ref content) = reply.content {
        use teloxide::prelude::Requester as _;
        use teloxide::types::{ChatId, MessageId, ThreadId};
        let html = crate::telegram::markdown::md_to_telegram_html(content);
        let parts = crate::telegram::markdown::split_html_message(&html);
        let chat_id = ChatId(target_chat_id);
        for part in &parts {
            let mut send = bot
                .send_message(chat_id, part)
                .parse_mode(teloxide::types::ParseMode::Html);
            if let Some(t) = target_thread_id {
                send = send.message_thread_id(ThreadId(MessageId(t as i32)));
            }
            if let Err(e) = send.await {
                tracing::warn!(
                    chat_id = target_chat_id,
                    "cron delivery: HTML send failed, retrying plain: {e:#}"
                );
                let plain = crate::telegram::markdown::strip_html_tags(part);
                let mut fallback = bot.send_message(chat_id, &plain);
                if let Some(t) = target_thread_id {
                    fallback = fallback.message_thread_id(ThreadId(MessageId(t as i32)));
                }
                if let Err(e2) = fallback.await {
                    tracing::error!(
                        chat_id = target_chat_id,
                        "cron delivery: plain text fallback also failed: {e2:#}"
                    );
                }
            }
        }
    }

    if let Some(ref atts) = reply.attachments
        && !atts.is_empty()
    {
        if let Err(e) = crate::telegram::attachments::send_attachments(
            atts,
            bot,
            teloxide::types::ChatId(target_chat_id),
            target_thread_id.unwrap_or(0),
            agent_dir,
            ssh_config_path,
            resolved_sandbox,
        )
        .await
        {
            tracing::error!(
                chat_id = target_chat_id,
                "cron delivery: attachment send failed: {e:#}"
            );
        }
    }

    Ok(())
}
```

(Engineer note: the comment block `/* ... existing setup ... */` is shorthand for the unchanged middle of the function — keep the existing CC-invocation, MCP instructions, system prompt assembly, `child.wait_with_output`, and `parse_reply_output` lines verbatim. Only the **send** parts at the bottom change, plus the signature and the dropped `notify_chat_ids.is_empty()` early-return at the top.)

- [ ] **Step 6: Run all bot tests**

Run: `cargo test -p rightclaw-bot --lib`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/cron_delivery.rs
git commit -m "feat(cron_delivery): route per-target with allowlist gate; drop notify_chat_ids fan-out"
```

---

## Task 11: lib.rs — pass `AllowlistHandle` to delivery loop, drop legacy locals

**Files:**
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Update the delivery loop spawn (line ~635)**

Replace the `delivery_chat_ids` local and adjust the `run_delivery_loop` call:

```rust
    // Cron delivery loop
    let delivery_agent_dir = agent_dir.clone();
    let delivery_agent_name = args.agent.clone();
    let delivery_bot = telegram::bot::build_bot(token.clone());
    let delivery_allowlist = allowlist.clone();
    let delivery_idle_ts = Arc::clone(&idle_timestamp);
    let delivery_ssh_config = ssh_config_path.clone();
    let delivery_internal_client = Arc::clone(&internal_client);
    let delivery_shutdown = shutdown.clone();
    let delivery_sandbox = resolved_sandbox.clone();
    let delivery_upgrade_lock = Arc::clone(&upgrade_lock);
    let delivery_handle = tokio::spawn(async move {
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
        )
        .await;
    });
```

- [ ] **Step 2: Drop the legacy `notify_chat_ids` local (line ~416)**

Replace lines 415-431 with the OAuth state construction that uses the allowlist (final shape lands in Task 12 — for now, leave a temporary copy that still compiles):

```rust
    let notify_bot = teloxide::Bot::new(token.clone());
    let agent_name = args.agent.clone();

    let internal_socket = home.join("run/internal.sock");
    let internal_client = Arc::new(rightclaw::mcp::internal_client::InternalClient::new(
        internal_socket,
    ));

    let oauth_state = OAuthCallbackState {
        pending_auth: Arc::clone(&pending_auth),
        agent_name: agent_name.clone(),
        bot: notify_bot,
        allowlist: allowlist.clone(), // wired up in Task 12
        internal_client: Arc::clone(&internal_client),
    };
```

- [ ] **Step 3: Build to confirm intermediate state**

Run: `cargo build -p rightclaw-bot`
Expected: compile error in `oauth_callback.rs` (`allowlist` field missing) — this is fine, Task 12 finishes it. To unblock the next tasks: temporarily keep the old `notify_chat_ids: vec![]` field on `OAuthCallbackState` until Task 12 swaps it.

If you prefer one atomic commit: skip Step 3 here and commit lib.rs + oauth_callback together at the end of Task 12.

- [ ] **Step 4: Commit (only if Step 3 compiles)**

```bash
git add crates/bot/src/lib.rs
git commit -m "refactor(bot): pass AllowlistHandle to cron delivery loop"
```

---

## Task 12: OAuth callback — switch to `AllowlistHandle`

**Files:**
- Modify: `crates/bot/src/telegram/oauth_callback.rs`

- [ ] **Step 1: Write the failing test**

In `crates/bot/src/telegram/oauth_callback.rs`, find the existing `#[cfg(test)]` block (a small one at the bottom — line ~325 references `notify_chat_ids: vec![]`). Replace the assertion with one that exercises the new `allowlist` field:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rightclaw::agent::allowlist::{AllowedUser, AllowlistHandle, AllowlistState};

    #[tokio::test]
    async fn oauth_callback_state_uses_allowlist_users() {
        let now = chrono::Utc::now();
        let mut state = AllowlistState::default();
        state.add_user(AllowedUser { id: 100, label: None, added_by: None, added_at: now });
        let handle = AllowlistHandle::new(state);
        let cb_state = OAuthCallbackState {
            pending_auth: Arc::new(tokio::sync::Mutex::new(Default::default())),
            agent_name: "test".into(),
            bot: teloxide::Bot::new("123:abc"),
            allowlist: handle.clone(),
            internal_client: Arc::new(rightclaw::mcp::internal_client::InternalClient::new("/nonexistent.sock")),
        };
        let user_ids: Vec<i64> = cb_state
            .allowlist
            .0
            .read()
            .unwrap()
            .users()
            .iter()
            .map(|u| u.id)
            .collect();
        assert_eq!(user_ids, vec![100]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw-bot --lib telegram::oauth_callback::tests`
Expected: FAIL — `allowlist` field missing.

- [ ] **Step 3: Update `OAuthCallbackState` and notify call sites**

In `crates/bot/src/telegram/oauth_callback.rs`, replace the struct (line ~42):

```rust
#[derive(Clone)]
pub struct OAuthCallbackState {
    pub pending_auth: PendingAuthMap,
    pub agent_name: String,
    pub bot: teloxide::Bot,
    pub allowlist: rightclaw::agent::allowlist::AllowlistHandle,
    pub internal_client: Arc<InternalClient>,
}
```

Replace both `notify_telegram(&cb_state.bot, &cb_state.notify_chat_ids, &msg)` invocations with snapshot reads from the allowlist:

```rust
            let chat_ids: Vec<i64> = cb_state
                .allowlist
                .0
                .read()
                .expect("allowlist lock poisoned")
                .users()
                .iter()
                .map(|u| u.id)
                .collect();
            notify_telegram(&cb_state.bot, &chat_ids, &msg).await;
```

(Apply the same pattern to the failure branch around line 236-244.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rightclaw-bot --lib telegram::oauth_callback::tests`
Expected: PASS.

`cargo build --workspace` — should now build cleanly across all crates.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/oauth_callback.rs crates/bot/src/lib.rs
git commit -m "refactor(oauth): notify trusted users from allowlist (no more notify_chat_ids)"
```

---

## Task 13: Doctor — check cron targets + bump expected schema version

**Files:**
- Modify: `crates/rightclaw/src/doctor.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/rightclaw/src/doctor.rs` (or its existing tests module):

```rust
#[cfg(test)]
mod cron_target_tests {
    use super::*;
    use rightclaw::agent::allowlist::{AllowedGroup, AllowedUser, AllowlistFile};

    fn write_allowlist(agent_dir: &std::path::Path, users: &[i64], groups: &[i64]) {
        let now = chrono::Utc::now();
        let mut file = AllowlistFile::default();
        for &id in users {
            file.users.push(AllowedUser { id, label: None, added_by: None, added_at: now });
        }
        for &id in groups {
            file.groups.push(AllowedGroup { id, label: None, opened_by: None, opened_at: now });
        }
        rightclaw::agent::allowlist::write_file(agent_dir, &file).unwrap();
    }

    fn seed_cron(conn: &rusqlite::Connection, name: &str, target_chat_id: Option<i64>) {
        let now = chrono::Utc::now().to_rfc3339();
        match target_chat_id {
            Some(id) => conn.execute(
                "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, target_chat_id, created_at, updated_at) \
                 VALUES (?1, '*/5 * * * *', 'p', 1.0, ?2, ?3, ?3)",
                rusqlite::params![name, id, now],
            ).unwrap(),
            None => conn.execute(
                "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
                 VALUES (?1, '*/5 * * * *', 'p', 1.0, ?2, ?2)",
                rusqlite::params![name, now],
            ).unwrap(),
        };
    }

    #[test]
    fn null_target_warns() {
        let dir = tempfile::tempdir().unwrap();
        write_allowlist(dir.path(), &[100], &[]);
        let conn = rightclaw::memory::open_connection(dir.path(), true).unwrap();
        seed_cron(&conn, "j1", None);
        drop(conn);

        let checks = check_cron_targets(dir.path());
        let warns: Vec<_> = checks.iter().filter(|c| c.status == CheckStatus::Warn).collect();
        assert_eq!(warns.len(), 1, "expected 1 warn, got {checks:?}");
        assert!(warns[0].detail.contains("j1"));
    }

    #[test]
    fn target_outside_allowlist_warns() {
        let dir = tempfile::tempdir().unwrap();
        write_allowlist(dir.path(), &[100], &[]);
        let conn = rightclaw::memory::open_connection(dir.path(), true).unwrap();
        seed_cron(&conn, "j1", Some(-999));
        drop(conn);

        let checks = check_cron_targets(dir.path());
        let warns: Vec<_> = checks.iter().filter(|c| c.status == CheckStatus::Warn).collect();
        assert_eq!(warns.len(), 1, "expected 1 warn, got {checks:?}");
        assert!(warns[0].detail.contains("-999"));
    }

    #[test]
    fn valid_target_passes() {
        let dir = tempfile::tempdir().unwrap();
        write_allowlist(dir.path(), &[], &[-200]);
        let conn = rightclaw::memory::open_connection(dir.path(), true).unwrap();
        seed_cron(&conn, "j1", Some(-200));
        drop(conn);

        let checks = check_cron_targets(dir.path());
        let warns: Vec<_> = checks.iter().filter(|c| c.status == CheckStatus::Warn).collect();
        assert!(warns.is_empty(), "expected no warns, got {checks:?}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib doctor::cron_target_tests`
Expected: FAIL — `check_cron_targets` not defined.

- [ ] **Step 3: Implement `check_cron_targets` and bump expected version**

In `crates/rightclaw/src/doctor.rs`, change line 963 to:

```rust
    let expected: u32 = 17;
```

Then add the new check function (place it near `check_memory`, around line 919):

```rust
/// Validate cron `target_chat_id` values for a single agent.
///
/// Surfaces:
/// - cron_specs rows with `target_chat_id IS NULL` → WARN (operator must `cron_update`)
/// - cron_specs rows whose `target_chat_id` is no longer in `allowlist.yaml` → WARN
///
/// Returns one `DoctorCheck` per problem found, plus a single Pass when the agent
/// has crons and all of them are healthy. Returns an empty Vec if the agent has no crons.
pub fn check_cron_targets(agent_dir: &Path) -> Vec<DoctorCheck> {
    let mut out = Vec::new();

    let conn = match crate::memory::open_connection(agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            out.push(DoctorCheck {
                name: "cron targets".into(),
                status: CheckStatus::Fail,
                detail: format!("open data.db: {e:#}"),
                fix: None,
            });
            return out;
        }
    };

    let allowlist_state = match crate::agent::allowlist::read_file(agent_dir) {
        Ok(Some(file)) => crate::agent::allowlist::AllowlistState::from_file(file),
        Ok(None) => {
            out.push(DoctorCheck {
                name: "cron targets".into(),
                status: CheckStatus::Warn,
                detail: "allowlist.yaml is missing — cron targets cannot be validated".into(),
                fix: Some("run `rightclaw agent allow <user_id>` from a trusted account".into()),
            });
            return out;
        }
        Err(e) => {
            out.push(DoctorCheck {
                name: "cron targets".into(),
                status: CheckStatus::Fail,
                detail: format!("read allowlist.yaml: {e}"),
                fix: None,
            });
            return out;
        }
    };

    let mut stmt = match conn.prepare("SELECT job_name, target_chat_id FROM cron_specs") {
        Ok(s) => s,
        Err(e) => {
            out.push(DoctorCheck {
                name: "cron targets".into(),
                status: CheckStatus::Fail,
                detail: format!("prepare query: {e:#}"),
                fix: None,
            });
            return out;
        }
    };

    let rows = match stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
    }) {
        Ok(r) => r,
        Err(e) => {
            out.push(DoctorCheck {
                name: "cron targets".into(),
                status: CheckStatus::Fail,
                detail: format!("query: {e:#}"),
                fix: None,
            });
            return out;
        }
    };

    let mut total = 0usize;
    let mut warned = 0usize;
    for row in rows {
        let (job_name, target) = match row {
            Ok(v) => v,
            Err(e) => {
                out.push(DoctorCheck {
                    name: "cron targets".into(),
                    status: CheckStatus::Fail,
                    detail: format!("row read: {e:#}"),
                    fix: None,
                });
                continue;
            }
        };
        total += 1;
        match target {
            None => {
                warned += 1;
                out.push(DoctorCheck {
                    name: "cron targets".into(),
                    status: CheckStatus::Warn,
                    detail: format!("cron '{job_name}' has no target_chat_id"),
                    fix: Some(format!("call cron_update job_name={job_name} target_chat_id=<chat_id>; or recreate the cron in the desired chat")),
                });
            }
            Some(id) if !allowlist_state.is_chat_allowed(id) => {
                warned += 1;
                out.push(DoctorCheck {
                    name: "cron targets".into(),
                    status: CheckStatus::Warn,
                    detail: format!("cron '{job_name}' targets chat {id} which is no longer in allowlist"),
                    fix: Some(format!("call cron_update job_name={job_name} target_chat_id=<chat_id>; or `rightclaw agent allow_all {id}` to re-open")),
                });
            }
            Some(_) => {}
        }
    }

    if total > 0 && warned == 0 {
        out.push(DoctorCheck {
            name: "cron targets".into(),
            status: CheckStatus::Pass,
            detail: format!("{total} cron(s) with valid targets"),
            fix: None,
        });
    }
    out
}
```

- [ ] **Step 4: Wire `check_cron_targets` into the per-agent run**

Find the function that calls `check_memory(agent_dir)` for each agent (search for `check_memory(` in `doctor.rs`). Add `out.extend(check_cron_targets(agent_dir));` immediately after each `check_memory` call.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib doctor::cron_target_tests`
Expected: 3 passes.

Run: `cargo test -p rightclaw --lib doctor`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/doctor.rs
git commit -m "feat(doctor): check_cron_targets + bump expected schema to v17"
```

---

## Task 14: Agent-facing docs — `target_chat_id` guidance

**Files:**
- Modify: `templates/right/prompt/OPERATING_INSTRUCTIONS.md`
- Modify: `skills/rightcron/SKILL.md`

- [ ] **Step 1: Locate the existing cron section**

```bash
grep -n "cron_create" templates/right/prompt/OPERATING_INSTRUCTIONS.md
grep -n "cron_create" skills/rightcron/SKILL.md
```

- [ ] **Step 2: Add the target guidance to OPERATING_INSTRUCTIONS.md**

In the section that documents `cron_create`, append a paragraph (or new bullet, matching the surrounding style):

```markdown
**Always pass `target_chat_id`** equal to the `chat.id` value from the incoming message YAML, unless the user explicitly asks for the cron to deliver elsewhere. The MCP tool will reject any chat that is not in the agent's allowlist. For supergroup topics, also pass `target_thread_id` from the message's `chat.topic_id`. To change a cron's destination later, call `cron_update` with the new `target_chat_id`.
```

- [ ] **Step 3: Add the same guidance to skills/rightcron/SKILL.md**

Append the same paragraph (lightly adapted to the SKILL voice) in the relevant `cron_create` / `cron_update` section.

- [ ] **Step 4: Commit**

```bash
git add templates/right/prompt/OPERATING_INSTRUCTIONS.md skills/rightcron/SKILL.md
git commit -m "docs(prompt): explain target_chat_id contract for cron tools"
```

---

## Task 15: Final workspace build + smoke check

**Files:** none (verification only).

- [ ] **Step 1: Full workspace debug build**

Run: `cargo build --workspace`
Expected: clean.

- [ ] **Step 2: Full workspace test**

Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 3: Manual smoke against the `him` agent**

(Run from your dev machine with the bot stopped; do not commit any state changes.)

```bash
# Inspect the existing cron — should have NULL target after migration.
sqlite3 ~/.rightclaw/agents/him/data.db "SELECT job_name, target_chat_id, target_thread_id FROM cron_specs;"
```
Expected: existing rows show `NULL | NULL`.

- [ ] **Step 4: Doctor smoke test**

Run: `cargo run -p rightclaw-cli -- doctor --agent him`
Expected: a `cron targets` warn line for each NULL-target cron, with the `cron_update` fix hint.

- [ ] **Step 5: Manual fix the agenda cron via `cron_update`**

This step is for the operator to run after deploy — document it in the release notes:

```
В Telegram-чате с him: попроси агента вызвать
  cron_update job_name=<имя крона> target_chat_id=-1001234567890
для перенацеливания agenda на группу aibots.
```

(No commit — this is operator runbook content, not code.)

---

## Spec Coverage Self-Review

The spec lists these requirements; map each to a task:

- Schema migration v17 (nullable columns) → **Task 1**
- `chat:` block always emitted (X choice) → **Task 8**
- MCP `cron_create` requires + validates target → **Task 6**
- MCP `cron_update` supports target changes (with explicit-null clear) → **Task 7**
- MCP `cron_list` shows targets → **Task 5**
- Aggregator reads `allowlist.yaml` on demand → **Task 6** (`validate_target_against_allowlist`)
- Delivery loop: NULL → no_target, denied → denied, ready → single send → **Tasks 9, 10**
- New `delivery_status` values `'no_target'`, `'denied'` → **Task 10** (string literals; no migration needed since column is `TEXT`)
- `notify_chat_ids` removed from cron_delivery + lib.rs → **Tasks 10, 11**
- OAuth callback uses `AllowlistHandle` (DM users only) → **Task 12**
- Doctor `cron_targets` check → **Task 13**
- Agent-facing instructions updated → **Task 14**
- Tests at every layer → embedded in each task

Backward compat path (existing rows → NULL → operator runs `cron_update`) is covered by Task 15 Step 5 in the operator runbook.

## Notes for the implementer

- `CronSpec` (the in-memory struct used by the cron *engine*, `crates/rightclaw/src/cron_spec.rs:38`) does **not** need target fields — the engine only consumes the schedule + prompt to fire the job. Targets live in the DB and are read by `cron_delivery::fetch_pending` via the JOIN. Keeping `CronSpec` lean avoids the `PartialEq` reconciler-restart subtlety described in the same file (line 50-57).
- The aggregator-side allowlist read uses `rightclaw::agent::allowlist::read_file` directly — no new `AllowlistHandle` instance needed inside `right_backend`. The bot keeps its own watcher-backed handle for routing; the aggregator's on-demand reads stay current because the bot is the sole writer and writes atomically.
- If you split `dm_single_message_emits_yaml_with_no_chat_block` rename across multiple commits, double-check no stray `assert!(!yaml.contains("chat:"))` survives — that was the original assertion shape.
- Existing `notify_chat_ids` in any `docs/superpowers/plans/*.md` are historical and should not be touched.
