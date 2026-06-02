# Force-notify cron trigger Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `cron_trigger(job_name, notify=true)` so an agent can force-run an existing cron and guarantee a prompt Telegram report, overriding the job's silent decision and the delivery idle gate — retiring the watcher-cron hack.

**Architecture:** A transient `trigger_force_notify` flag on `cron_specs` (set/cleared with `triggered_at`) rides on the in-memory `CronSpec` the reconciler hands to `execute_job`. When set, the forced run gets an "always notify" prompt directive, its `async_runs` row is stamped `force_notify=1`, delivery is forced even if the run chose silent (delivering the silent reason as content), and the delivery loop skips the idle gate for that row.

**Tech Stack:** Rust (edition 2024), Turso/SQLite via `right-db`, `rmcp` `#[tool]` macros, tokio. Build/test through `devenv shell -- cargo …`.

**Spec:** `docs/superpowers/specs/2026-06-02-cron-force-notify-trigger-design.md`

**Implementation note (refinement of spec):** The spec described "`execute_job` gains a `force_notify` parameter." In implementation the flag travels on the `CronSpec` the reconciler already clones into the spawn (`cron.rs:1742 let sp = spec.clone()`), so `execute_job` and `insert_running_run` read `spec.trigger_force_notify` directly — no extra parameter threading. `persist_successful_cron_output` takes an explicit `force_notify: bool` derived from the spec at the call site. This is equivalent to and simpler than a new parameter.

**Baseline check (run once before Task 1):**
```
devenv shell -- cargo test -p right-db -p right-agent -p right-bot -p right
```
Record any pre-existing failures (see memory: two workspace tests flake under parallel load — re-run isolated before blaming a change).

---

### Task 1: Migration v41 — add the two columns

**Files:**
- Modify: `crates/right-db/src/migrations.rs` (const at `:37`, registry tail at `:934`, hook fn near `:634`, test module at `:944`)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/right-db/src/migrations.rs`:

```rust
    #[tokio::test]
    async fn v41_adds_force_notify_columns() {
        let conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&conn).await.unwrap();

        let spec_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = 'trigger_force_notify'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(spec_col, 1, "cron_specs.trigger_force_notify must exist");

        let run_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('async_runs') WHERE name = 'force_notify'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(run_col, 1, "async_runs.force_notify must exist");

        // Idempotent: re-running to_latest is a no-op, not an error.
        MIGRATIONS.to_latest(&conn).await.unwrap();
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-db v41_adds_force_notify_columns`
Expected: FAIL — either a compile error (no version 41 yet, `LATEST_SCHEMA_VERSION` mismatch in the existing `to_latest` final-version test) or the COUNT assertions return 0.

- [ ] **Step 3: Bump the schema version constant**

In `crates/right-db/src/migrations.rs:37`, change:
```rust
pub const LATEST_SCHEMA_VERSION: u32 = 40;
```
to:
```rust
pub const LATEST_SCHEMA_VERSION: u32 = 41;
```

- [ ] **Step 4: Add the migration hook function**

Add near the other hook fns (e.g. after `v27_skill_nudge_signals_source` around `:656`):

```rust
/// v41: Force-notify trigger support.
///
/// `cron_specs.trigger_force_notify` is set together with `triggered_at` by a
/// force-notify trigger and cleared together. `async_runs.force_notify` marks a
/// run whose delivery overrides the silent decision and the idle gate.
///
/// Idempotent — checks pragma_table_info before each ALTER. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`.
fn v41_cron_force_notify(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "cron_specs", "trigger_force_notify").await? {
            conn.execute_batch(
                "ALTER TABLE cron_specs ADD COLUMN trigger_force_notify INTEGER NOT NULL DEFAULT 0",
            )
            .await?;
        }
        if !column_exists(conn, "async_runs", "force_notify").await? {
            conn.execute_batch(
                "ALTER TABLE async_runs ADD COLUMN force_notify INTEGER NOT NULL DEFAULT 0",
            )
            .await?;
        }
        Ok(())
    })
}
```

- [ ] **Step 5: Register the migration**

In the `MIGRATIONS` registry, immediately after the `version: 40` entry (`crates/right-db/src/migrations.rs:934-939`), add:

```rust
        Migration {
            version: 41,
            sql: "",
            hook: Some(v41_cron_force_notify),
        },
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `devenv shell -- cargo test -p right-db v41_adds_force_notify_columns`
Expected: PASS.

- [ ] **Step 7: Run the migration-runner suite (guards the version bump)**

Run: `devenv shell -- cargo test -p right-db migration`
Expected: PASS (the existing `…final user_version must equal LATEST_SCHEMA_VERSION` test now expects 41).

- [ ] **Step 8: Commit**

```bash
git add crates/right-db/src/migrations.rs
git commit -m "feat(db): v41 — cron_specs.trigger_force_notify + async_runs.force_notify"
```

---

### Task 2: `CronSpec.trigger_force_notify` + trigger/clear/load (right-agent)

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (struct `:122`, load `:705-760`, `trigger_spec` `:776`, `clear_triggered_at` `:793`)
- Test: `crates/right-agent/src/cron_spec_tests.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/right-agent/src/cron_spec_tests.rs` (uses the existing `setup_db()` and `create_spec` helpers in that file):

```rust
#[tokio::test]
async fn trigger_spec_force_notify_sets_both_columns() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "fn-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();

    trigger_spec(&conn, "fn-job", true).await.unwrap();

    let (triggered_at, force): (Option<String>, i64) = conn
        .query_row(
            "SELECT triggered_at, trigger_force_notify FROM cron_specs WHERE job_name = ?1",
            right_db::params!["fn-job"],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert!(triggered_at.is_some(), "triggered_at must be set");
    assert_eq!(force, 1, "trigger_force_notify must be set");
}

#[tokio::test]
async fn trigger_spec_without_force_notify_leaves_flag_zero() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "plain-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();

    trigger_spec(&conn, "plain-job", false).await.unwrap();

    let force: i64 = conn
        .query_row(
            "SELECT trigger_force_notify FROM cron_specs WHERE job_name = ?1",
            right_db::params!["plain-job"],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(force, 0);
}

#[tokio::test]
async fn clear_triggered_at_resets_force_notify() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "clr-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();
    trigger_spec(&conn, "clr-job", true).await.unwrap();

    clear_triggered_at(&conn, "clr-job").await.unwrap();

    let (triggered_at, force): (Option<String>, i64) = conn
        .query_row(
            "SELECT triggered_at, trigger_force_notify FROM cron_specs WHERE job_name = ?1",
            right_db::params!["clr-job"],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert!(triggered_at.is_none(), "triggered_at must be cleared");
    assert_eq!(force, 0, "trigger_force_notify must be reset");
}

#[tokio::test]
async fn load_specs_carries_force_notify() {
    let (_dir, conn) = setup_db().await;
    create_spec(&conn, "load-job", "*/5 * * * *", "do stuff", None, None)
        .await
        .unwrap();
    trigger_spec(&conn, "load-job", true).await.unwrap();

    let specs = load_specs_from_db(&conn).await.unwrap();
    assert!(
        specs["load-job"].trigger_force_notify,
        "loaded spec must carry trigger_force_notify"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-agent force_notify`
Expected: FAIL — compile error (`trigger_spec` takes 2 args, no `trigger_force_notify` field).

- [ ] **Step 3: Add the struct field**

In `crates/right-agent/src/cron_spec.rs:122`, add `trigger_force_notify` to `CronSpec`:

```rust
pub struct CronSpec {
    pub schedule_kind: ScheduleKind,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: f64,
    pub triggered_at: Option<String>,
    pub trigger_force_notify: bool,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
}
```

Leave the `PartialEq` impl (`:138`) unchanged — like `triggered_at`, this transient flag must NOT participate in equality, so the reconciler does not abort running jobs when it toggles.

- [ ] **Step 4: Read and map the column in `load_specs_from_db`**

In `crates/right-agent/src/cron_spec.rs`, extend the SELECT and row tuple. Change the query string (`:706`) to append the column:

```rust
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, trigger_force_notify, recurring, run_at, target_chat_id, target_thread_id FROM cron_specs",
```

Add the new column to the row closure (after the `triggered_at` `Option<String>` at index 5), shifting the later indices by one:

```rust
        |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, i64>(6)?,          // trigger_force_notify
            row.get::<_, i64>(7)?,          // recurring
            row.get::<_, Option<String>>(8)?, // run_at
            row.get::<_, Option<i64>>(9)?,  // target_chat_id
            row.get::<_, Option<i64>>(10)?, // target_thread_id
        ))
    })
```

Update the destructuring `let (...) = row;` and the `CronSpec { ... }` construction to include `trigger_force_notify`:

```rust
        let (
            job_name,
            schedule,
            prompt,
            lock_ttl,
            max_budget_usd,
            triggered_at,
            trigger_force_notify,
            recurring,
            run_at,
            target_chat_id,
            target_thread_id,
        ) = row;
```
```rust
        specs.insert(
            job_name,
            CronSpec {
                schedule_kind,
                prompt,
                lock_ttl,
                max_budget_usd,
                triggered_at,
                trigger_force_notify: trigger_force_notify != 0,
                target_chat_id,
                target_thread_id,
            },
        );
```

- [ ] **Step 5: Add `force_notify` param to `trigger_spec`**

Replace `trigger_spec` (`crates/right-agent/src/cron_spec.rs:776`):

```rust
/// Mark a cron spec for immediate execution on the next engine tick.
///
/// When `force_notify` is set, the resulting run delivers a report regardless
/// of the job's own silent decision and bypasses the delivery idle gate.
pub async fn trigger_spec(
    conn: &Connection,
    job_name: &str,
    force_notify: bool,
) -> Result<String, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let rows = conn
        .execute(
            "UPDATE cron_specs SET triggered_at = ?2, trigger_force_notify = ?3 WHERE job_name = ?1",
            params![job_name, now, force_notify],
        )
        .await
        .map_err(|e| format!("trigger failed: {e:#}"))?;
    if rows == 0 {
        return Err(format!("job '{job_name}' not found"));
    }
    Ok(format!("Triggered job '{job_name}'."))
}
```

- [ ] **Step 6: Reset the flag in `clear_triggered_at`**

Replace the UPDATE in `clear_triggered_at` (`crates/right-agent/src/cron_spec.rs:793`):

```rust
    conn.execute(
        "UPDATE cron_specs SET triggered_at = NULL, trigger_force_notify = 0 WHERE job_name = ?1",
        params![job_name],
    )
```

- [ ] **Step 7: Fix any other `CronSpec { … }` literals and `trigger_spec(` callers**

Run: `devenv shell -- cargo build -p right-agent 2>&1 | head -40`
Add `trigger_force_notify: false` to any `CronSpec { … }` struct literal the compiler flags (notably test fixtures in `crates/bot/src/cron.rs` at `:1950, :2087, :2141, :2459, :2493, :2527, :2570` — those are fixed in Task 4). Within right-agent, fix only right-agent literals here.

- [ ] **Step 8: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-agent force_notify`
Expected: PASS (4 tests).

- [ ] **Step 9: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs crates/right-agent/src/cron_spec_tests.rs
git commit -m "feat(cron): trigger_force_notify on CronSpec + trigger_spec(force_notify)"
```

---

### Task 3: Stamp `force_notify` on the run row (right-agent async_runs)

**Files:**
- Modify: `crates/right-agent/src/async_runs.rs` (`NewCronRun` `:5`, `insert_running_cron_run` `:55`)
- Test: `crates/right-agent/src/async_runs.rs` (add a `#[cfg(test)]` test, or its existing test module if present)

- [ ] **Step 1: Write the failing test**

Add to a test module in `crates/right-agent/src/async_runs.rs` (create `#[cfg(test)] mod tests { … }` at the end if none exists; `right_db::open_connection` with `migrate=true` builds the schema):

```rust
#[cfg(test)]
mod force_notify_tests {
    use super::*;

    #[tokio::test]
    async fn insert_running_cron_run_persists_force_notify() {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();

        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-fn",
                job_name: "job",
                started_at: "2026-06-02T00:00:00Z",
                log_path: "/log",
                target_chat_id: Some(42),
                target_thread_id: None,
                force_notify: true,
            },
        )
        .await
        .unwrap();

        let force: i64 = conn
            .query_row(
                "SELECT force_notify FROM async_runs WHERE id = 'run-fn'",
                right_db::params![],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(force, 1);
    }
}
```

(If `tempfile` is not already a dev-dependency of `right-agent`, add it: `devenv shell -- cargo add --dev --package right-agent tempfile` — it is already used in `cron_spec_tests.rs`, so it should be present.)

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-agent insert_running_cron_run_persists_force_notify`
Expected: FAIL — compile error (`NewCronRun` has no `force_notify` field).

- [ ] **Step 3: Add the field to `NewCronRun`**

In `crates/right-agent/src/async_runs.rs:5`:

```rust
#[derive(Debug, Clone, Copy)]
pub struct NewCronRun<'a> {
    pub id: &'a str,
    pub job_name: &'a str,
    pub started_at: &'a str,
    pub log_path: &'a str,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
    pub force_notify: bool,
}
```

- [ ] **Step 4: Write the column in `insert_running_cron_run`**

Replace the INSERT in `insert_running_cron_run` (`crates/right-agent/src/async_runs.rs:63`):

```rust
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
            status, started_at, log_path, delivery_required, delivery_status,
            force_notify, created_at, updated_at
         ) VALUES (
            ?1, 'cron', ?2, ?1, ?3, ?4,
            'running', ?5, ?6, 0, 'none',
            ?7, ?5, ?5
         )",
        params![
            run.id,
            run.job_name,
            target_chat_id,
            run.target_thread_id,
            run.started_at,
            run.log_path,
            run.force_notify,
        ],
    )
    .await?;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-agent insert_running_cron_run_persists_force_notify`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/async_runs.rs
git commit -m "feat(cron): persist force_notify on running cron run row"
```

---

### Task 4: Forced-run behavior in `execute_job` (bot/cron.rs)

**Files:**
- Modify: `crates/bot/src/cron.rs` (`insert_running_run` `:294`, `execute_job` prompt `:537`, delivery_json build `:840`, `persist_successful_cron_output` `:411`, persist call `:887`, test fixtures)
- Test: `crates/bot/src/cron.rs` test module

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/bot/src/cron.rs`. This targets the pure persist mapping. It reuses the test helpers already present near `:2078` (`setup_db`-style). If the module has a local DB helper named differently, use it; otherwise use `right_db::open_connection(dir.path(), true)`:

```rust
    #[tokio::test]
    async fn persist_force_notify_silent_delivers_pending() {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
        right_agent::async_runs::insert_running_cron_run(
            &conn,
            right_agent::async_runs::NewCronRun {
                id: "run-fns",
                job_name: "job",
                started_at: "2026-06-02T00:00:00Z",
                log_path: "/log",
                target_chat_id: Some(7),
                target_thread_id: None,
                force_notify: true,
            },
        )
        .await
        .unwrap();

        let cron_output = CronReplyOutput {
            delivery: CronDeliveryDecision::Silent {
                reason: "no changes".into(),
            },
            run_note: "checked".into(),
        };
        // Forced silent → delivery_json is synthesized as notify carrying the reason.
        let delivery_json = notify_delivery_json("Verification run — nothing to report. no changes", None).unwrap();

        let status = persist_successful_cron_output(&conn, "run-fns", &cron_output, &delivery_json, true)
            .await
            .unwrap();
        assert_eq!(status, "pending");

        let required: i64 = conn
            .query_row(
                "SELECT delivery_required FROM async_runs WHERE id = 'run-fns'",
                right_db::params![],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(required, 1, "forced silent run must require delivery");
    }

    #[tokio::test]
    async fn persist_non_forced_silent_stays_none() {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
        right_agent::async_runs::insert_running_cron_run(
            &conn,
            right_agent::async_runs::NewCronRun {
                id: "run-ns",
                job_name: "job",
                started_at: "2026-06-02T00:00:00Z",
                log_path: "/log",
                target_chat_id: Some(7),
                target_thread_id: None,
                force_notify: false,
            },
        )
        .await
        .unwrap();

        let cron_output = CronReplyOutput {
            delivery: CronDeliveryDecision::Silent {
                reason: "no changes".into(),
            },
            run_note: "checked".into(),
        };
        let delivery_json = serde_json::to_string(&cron_output.delivery).unwrap();

        let status = persist_successful_cron_output(&conn, "run-ns", &cron_output, &delivery_json, false)
            .await
            .unwrap();
        assert_eq!(status, "none", "non-forced silent run stays silent (regression)");
    }
```

Note: confirm the exact field set of `CronReplyOutput` by reading it near the top of `cron.rs` (it wraps `delivery: CronDeliveryDecision` and `run_note: String`). Adjust the literal if it has more fields.

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot persist_force_notify_silent_delivers_pending persist_non_forced_silent_stays_none`
Expected: FAIL — compile error (`persist_successful_cron_output` takes 4 args; `NewCronRun` field set just changed so fixtures may also fail to build — fixed in Step 6).

- [ ] **Step 3: Add `force_notify` to `persist_successful_cron_output`**

Replace `persist_successful_cron_output` (`crates/bot/src/cron.rs:411`):

```rust
async fn persist_successful_cron_output(
    conn: &right_db::Connection,
    run_id: &str,
    cron_output: &CronReplyOutput,
    delivery_json: &str,
    force_notify: bool,
) -> Result<&'static str, right_db::DbError> {
    let (delivery_required, delivery_status) = match cron_output.delivery {
        CronDeliveryDecision::Notify { .. } => (true, "pending"),
        CronDeliveryDecision::Silent { .. } if force_notify => (true, "pending"),
        CronDeliveryDecision::Silent { .. } => (false, "none"),
    };
    right_agent::async_runs::persist_run_output(
        conn,
        run_id,
        right_agent::async_runs::RunOutput {
            run_note: Some(&cron_output.run_note),
            delivery_json: Some(delivery_json),
            error_json: None,
            delivery_required,
        },
    )
    .await?;
    Ok(delivery_status)
}
```

- [ ] **Step 4: Prepend the always-notify directive and stamp the run row in `execute_job`**

In `crates/bot/src/cron.rs`, replace `let prompt_for_cc = spec.prompt.clone();` (`:537`):

```rust
    let prompt_for_cc = if spec.trigger_force_notify {
        format!(
            "⟨⟨SYSTEM_NOTICE⟩⟩ Manual verification trigger: always emit \
             delivery.kind=\"notify\" with a complete report of what you found; \
             do not go silent. ⟨⟨/SYSTEM_NOTICE⟩⟩\n\n{}",
            spec.prompt
        )
    } else {
        spec.prompt.clone()
    };
```

- [ ] **Step 5: Pass `force_notify` from `insert_running_run`**

In `crates/bot/src/cron.rs`, update the `NewCronRun` literal inside `insert_running_run` (`:308`):

```rust
    right_agent::async_runs::insert_running_cron_run(
        conn,
        right_agent::async_runs::NewCronRun {
            id: run_id,
            job_name,
            started_at,
            log_path,
            target_chat_id: Some(target_chat_id),
            target_thread_id: spec.target_thread_id,
            force_notify: spec.trigger_force_notify,
        },
    )
    .await
```

- [ ] **Step 6: Override silent delivery_json + pass force_notify at the persist call site**

In the success path (`crates/bot/src/cron.rs:840`), replace the `other =>` arm of the `delivery_json` match so a forced silent run delivers its reason as notify content:

```rust
                    other => {
                        if spec.trigger_force_notify
                            && let CronDeliveryDecision::Silent { reason } = other
                        {
                            notify_delivery_json(
                                &format!("Verification run — nothing to report. {reason}"),
                                None,
                            )
                            .map_err(|e| {
                                tracing::error!(job = %job_name, "failed to serialize forced-notify delivery_json: {e:#}");
                            })
                            .ok()
                        } else {
                            serde_json::to_string(other)
                                .map_err(|e| {
                                    tracing::error!(job = %job_name, "failed to serialize delivery_json: {e:#}");
                                })
                                .ok()
                        }
                    }
```

Then update the `persist_successful_cron_output` call (`:889`) to pass the flag:

```rust
                            let delivery_status = persist_successful_cron_output(
                                &tx,
                                &run_id,
                                &cron_output,
                                &delivery_json,
                                spec.trigger_force_notify,
                            )
                            .await?;
```

- [ ] **Step 7: Fix `CronSpec { … }` test fixtures in this file**

The new `trigger_force_notify` field (Task 2) breaks `CronSpec { … }` literals in `cron.rs` tests. Add `trigger_force_notify: false,` to each (the compiler lists them; known sites near `:1950, :2087, :2141, :2459, :2493, :2527, :2570`). Likewise add `force_notify: false,` to any `NewCronRun { … }` test literal the compiler flags.

Run: `devenv shell -- cargo build -p right-bot 2>&1 | head -40`
Fix every flagged literal until it builds.

- [ ] **Step 8: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot persist_force_notify_silent_delivers_pending persist_non_forced_silent_stays_none`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): forced-notify run — always-notify directive, silent override, delivery_required"
```

---

### Task 5: Idle-gate bypass in the delivery loop (bot/async_delivery.rs)

**Files:**
- Modify: `crates/bot/src/async_delivery.rs` (`PendingAsyncResult` `:13`, `pending_from_row` `:52`, `fetch_pending_batch` SELECT `:36`, `deduplicate_job` SELECT `:126`, idle gate `:411`, add `should_hold_delivery` helper near `:329`)
- Test: `crates/bot/src/async_delivery.rs` test module

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/bot/src/async_delivery.rs`:

```rust
    #[test]
    fn force_notify_skips_idle_gate() {
        // Non-forced, recently active chat → held.
        assert!(should_hold_delivery(false, DeliveryMode::Normal, 10));
        // Forced → never held, even when active.
        assert!(!should_hold_delivery(true, DeliveryMode::Normal, 10));
        // Idle long enough → not held regardless.
        assert!(!should_hold_delivery(false, DeliveryMode::Normal, IDLE_THRESHOLD_SECS + 1));
    }

    #[tokio::test]
    async fn fetch_pending_reads_force_notify() {
        let (_dir, conn) = setup_db().await;
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
                status, started_at, finished_at, log_path, run_note, delivery_json,
                delivery_required, delivery_status, force_notify, created_at, updated_at
             ) VALUES (
                'r-fn', 'cron', 'job', 'r-fn', 5, NULL,
                'success', '2026-06-02T00:00:00Z', '2026-06-02T00:01:00Z', '/log', 'note',
                '{\"kind\":\"notify\",\"content\":\"hi\"}',
                1, 'pending', 1, '2026-06-02T00:00:00Z', '2026-06-02T00:01:00Z'
             )",
            right_db::params![],
        )
        .await
        .unwrap();

        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        assert_eq!(pending.id, "r-fn");
        assert!(pending.force_notify, "force_notify must be read from the row");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot force_notify_skips_idle_gate fetch_pending_reads_force_notify`
Expected: FAIL — `should_hold_delivery` undefined; `PendingAsyncResult` has no `force_notify`.

- [ ] **Step 3: Add the field to `PendingAsyncResult`**

In `crates/bot/src/async_delivery.rs:13`:

```rust
pub(crate) struct PendingAsyncResult {
    pub id: String,
    pub kind: String,
    pub producer_ref: Option<String>,
    pub delivery_json: String,
    pub run_note: String,
    pub status: String,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
    pub force_notify: bool,
}
```

- [ ] **Step 4: Read the column in `pending_from_row` and both SELECTs**

In `pending_from_row` (`:52`), add the read (new index 8):

```rust
fn pending_from_row(row: &right_db::row::Row<'_>) -> Result<PendingAsyncResult, right_db::DbError> {
    Ok(PendingAsyncResult {
        id: row.get(0)?,
        kind: row.get(1)?,
        producer_ref: row.get(2)?,
        delivery_json: row.get(3)?,
        run_note: row.get(4)?,
        status: row.get(5)?,
        target_chat_id: row.get(6)?,
        target_thread_id: row.get(7)?,
        force_notify: row.get::<_, i64>(8)? != 0,
    })
}
```

In `fetch_pending_batch` (`:36`), append `force_notify` to the column list:

```rust
        "SELECT id, kind, producer_ref, delivery_json, COALESCE(run_note, ''), status, \
                NULLIF(target_chat_id, 0), target_thread_id, force_notify \
         FROM async_runs \
         WHERE delivery_required = 1 \
           AND delivery_status IN ('pending', 'retryable') \
           AND status IN ('success', 'failed') \
           AND delivery_json IS NOT NULL \
         ORDER BY finished_at ASC \
         LIMIT ?1",
```

In `deduplicate_job` (`:126`), append `force_notify` to that SELECT's column list identically:

```rust
            "SELECT id, kind, producer_ref, delivery_json, COALESCE(run_note, ''), status, \
                    NULLIF(target_chat_id, 0), target_thread_id, force_notify \
             FROM async_runs \
             WHERE kind = 'cron' \
               AND producer_ref = ?1 \
               AND delivery_required = 1 \
               AND delivery_status IN ('pending', 'retryable') \
               AND status IN ('success', 'failed') \
               AND delivery_json IS NOT NULL \
             ORDER BY finished_at DESC \
             LIMIT 1",
```

- [ ] **Step 5: Add the `should_hold_delivery` helper and use it at the gate**

In `crates/bot/src/async_delivery.rs`, just below `should_wait_for_idle` (`:331`):

```rust
/// Whether a pending result must wait before delivery. Force-notify runs are
/// never held — they bypass the idle gate so a forced verification result lands
/// promptly.
fn should_hold_delivery(force_notify: bool, mode: DeliveryMode, idle_for: i64) -> bool {
    !force_notify && should_wait_for_idle(mode, idle_for)
}
```

In `run_delivery_once`, replace the idle check (`:411`):

```rust
    if should_hold_delivery(pending.force_notify, mode, idle_for) {
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot force_notify_skips_idle_gate fetch_pending_reads_force_notify`
Expected: PASS.

- [ ] **Step 7: Run the async_delivery suite (regression)**

Run: `devenv shell -- cargo test -p right-bot async_delivery`
Expected: PASS — existing idle-gate and fetch tests still green.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/async_delivery.rs
git commit -m "feat(cron): delivery loop bypasses idle gate for force_notify runs"
```

---

### Task 6: MCP surface — `notify` param + descriptions (right crate)

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (`TRIGGER_TOOL_DESC` `:40`)
- Modify: `crates/right/src/memory_server.rs` (`CronTriggerParams` `:109`, `cron_trigger` desc+body `:365`, instruction block `:521`)
- Modify: `crates/right/src/right_backend.rs` (`call_cron_trigger` `:529`)

- [ ] **Step 1: Write the failing test**

`memory_server.rs` already has `cron_trigger_description_matches_const` (`:639`) enforcing the literal == `TRIGGER_TOOL_DESC`. Add a param-default test to `memory_server.rs` tests:

```rust
    #[test]
    fn cron_trigger_params_notify_defaults_false() {
        let p: CronTriggerParams =
            serde_json::from_value(serde_json::json!({ "job_name": "j" })).unwrap();
        assert!(!p.notify);
        let p2: CronTriggerParams =
            serde_json::from_value(serde_json::json!({ "job_name": "j", "notify": true })).unwrap();
        assert!(p2.notify);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right cron_trigger`
Expected: FAIL — `CronTriggerParams` has no `notify`; and once the const changes, the description-match test fails until the literal is updated in lockstep.

- [ ] **Step 3: Update `TRIGGER_TOOL_DESC`**

Replace `TRIGGER_TOOL_DESC` (`crates/right-agent/src/cron_spec.rs:40`):

```rust
pub const TRIGGER_TOOL_DESC: &str = const_format::formatcp!(
    "Trigger a cron job for immediate execution. Lock check applies — if the \
     job is currently running, the trigger is skipped. By default delivery is \
     conditional: the cron decides whether to notify (sets `delivery` in its \
     structured output), and any notification is held until the chat has been \
     idle for {} minutes. Set `notify=true` to force a verification report — it \
     overrides a silent decision and skips the idle gate, so the user is sure to \
     receive the result promptly. Use `notify=true` to check a job instead of \
     creating a second cron to watch it. Use `cron_list_runs` to inspect \
     `delivery_status` and `delivery`.",
    IDLE_THRESHOLD_MIN,
);
```

- [ ] **Step 4: Add `notify` to `CronTriggerParams`**

In `crates/right/src/memory_server.rs:109`:

```rust
pub struct CronTriggerParams {
    #[schemars(description = "Job name to trigger for immediate execution")]
    pub job_name: String,
    #[serde(default)]
    #[schemars(
        description = "Force a verification report: override a silent decision and skip the idle gate so the user receives the result promptly. Default false."
    )]
    pub notify: bool,
}
```

- [ ] **Step 5: Update the `#[tool(description = …)]` literal and body in `memory_server.rs`**

The `#[tool(description = "…")]` literal on `cron_trigger` (`:366`) must stay byte-for-byte equal to the new `TRIGGER_TOOL_DESC`. Replace the literal with the exact resolved text (substituting the `IDLE_THRESHOLD_MIN` value `2`):

```rust
    #[tool(
        description = "Trigger a cron job for immediate execution. Lock check applies — if the job is currently running, the trigger is skipped. By default delivery is conditional: the cron decides whether to notify (sets `delivery` in its structured output), and any notification is held until the chat has been idle for 2 minutes. Set `notify=true` to force a verification report — it overrides a silent decision and skips the idle gate, so the user is sure to receive the result promptly. Use `notify=true` to check a job instead of creating a second cron to watch it. Use `cron_list_runs` to inspect `delivery_status` and `delivery`."
    )]
```

Update the body call (`:374`) to pass `params.notify`:

```rust
        let msg = right_agent::cron_spec::trigger_spec(&conn, &params.job_name, params.notify)
            .await
            .map_err(|e| McpError::invalid_params(e, None))?;
```

- [ ] **Step 6: Update the instruction-block line**

In the tool-list instruction block (`crates/right/src/memory_server.rs:521`), replace the `cron_trigger` line:

```rust
                 - mcp__right__cron_trigger: Trigger a cron job for immediate execution; pass notify=true to force a verification report (overrides silent + idle gate) instead of creating a watcher cron\n\n\
```

- [ ] **Step 7: Update the aggregator backend dispatch**

In `crates/right/src/right_backend.rs`, `call_cron_trigger` (`:538`), pass the flag:

```rust
        let msg = right_agent::cron_spec::trigger_spec(&conn, &params.job_name, params.notify)
            .await
            .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
```

No description edit is needed in `right_backend.rs`: its `cron_trigger` registration (`:158`) references `right_agent::cron_spec::TRIGGER_TOOL_DESC` directly, so it picks up the new text automatically. Only `params.notify` threading (above) is required here.

- [ ] **Step 8: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right cron_trigger`
Expected: PASS (`cron_trigger_params_notify_defaults_false` + `cron_trigger_description_matches_const`).

- [ ] **Step 9: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs crates/right/src/memory_server.rs crates/right/src/right_backend.rs
git commit -m "feat(mcp): cron_trigger notify=true force-notify flag + guidance"
```

---

### Task 7: Docs sync (PROMPT_SYSTEM.md + sessions.md)

**Files:**
- Modify: `PROMPT_SYSTEM.md`
- Modify: `docs/architecture/sessions.md`

- [ ] **Step 1: Update `PROMPT_SYSTEM.md`**

Find the section that lists/describes the `cron_trigger` MCP tool (grep: `rg -n "cron_trigger" PROMPT_SYSTEM.md`). Update its description to mention the `notify=true` force-notify behavior, matching the new tool description. If `cron_trigger` is not individually described there, add one sentence under the cron tools summary:

> `cron_trigger` accepts `notify=true` to force a verification report — it overrides the run's silent decision and skips the delivery idle gate, replacing the pattern of creating a second cron to watch a job.

- [ ] **Step 2: Update `docs/architecture/sessions.md`**

In the cron section (grep: `rg -n "cron_trigger|triggered_at|Immediate" docs/architecture/sessions.md`), add a short subsection describing the force-notify trigger:

```markdown
### Force-notify trigger

`cron_trigger(job_name, notify=true)` force-runs a job and guarantees a
prompt report. It sets `cron_specs.trigger_force_notify` alongside
`triggered_at` (both cleared together by `clear_triggered_at`). The
reconciler's triggered branch passes the flag via the in-memory `CronSpec`
to `execute_job`, which (1) prepends an "always notify, don't go silent"
`⟨⟨SYSTEM_NOTICE⟩⟩` directive to the run prompt, and (2) stamps
`async_runs.force_notify = 1`. `persist_successful_cron_output` then forces
`delivery_required = 1` even on a silent decision (delivering the silent
reason as content), and the delivery loop's `should_hold_delivery` skips the
idle gate for force-notify rows. Force-trigger while the job is locked is
dropped, same as a plain trigger; the flag is transient, so scheduled runs of
a recurring job are unaffected.
```

- [ ] **Step 3: Commit**

```bash
git add PROMPT_SYSTEM.md docs/architecture/sessions.md
git commit -m "docs(cron): document force-notify trigger"
```

---

### Task 8: Final workspace verification

- [ ] **Step 1: Full workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: clean build.

- [ ] **Step 2: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. If `cc/invocation` pid-race or the dashboard warn-count test flakes (known parallel-load flakes per project memory), re-run those isolated before attributing failure to this change.

- [ ] **Step 3: Clippy on touched crates**

Run: `devenv shell -- cargo clippy -p right-db -p right-agent -p right-bot -p right -- -D warnings`
Expected: no warnings. Fix any introduced by the new code (e.g. the `if let … && let …` chain in Task 4 requires the let-chains pattern already used elsewhere in `cron.rs`; if clippy objects, restructure to nested `if let`).

- [ ] **Step 4: Final commit (if clippy fixes were needed)**

```bash
git add -A
git commit -m "chore(cron): clippy + final verification for force-notify trigger"
```

---

## Self-review notes

- **Spec coverage:** MCP param (Task 6) · two columns + idempotent migration (Task 1) · `CronSpec` field/load/trigger/clear (Task 2) · run-row stamp (Task 3) · always-notify directive + silent override + delivery_required (Task 4) · idle bypass (Task 5) · agent guidance in `TRIGGER_TOOL_DESC`/instruction block (Task 6) · `PROMPT_SYSTEM.md`/`sessions.md` (Task 7) · known limits documented (Task 7). All spec requirements mapped.
- **Type consistency:** `trigger_spec(conn, job_name, force_notify)` used identically in Tasks 2/6. `NewCronRun.force_notify` (Task 3) set in Task 4. `PendingAsyncResult.force_notify` + `should_hold_delivery` (Task 5). `persist_successful_cron_output(.., force_notify)` (Task 4).
- **Ordering:** right-agent (Tasks 2-3) lands the API before bot consumers (Tasks 4-5); MCP surface (Task 6) after `trigger_spec` arity change.
