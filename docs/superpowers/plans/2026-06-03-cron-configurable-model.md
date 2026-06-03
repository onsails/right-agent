# MCP-configurable per-cron model — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the agent session that creates a cron pick that cron's model (`haiku`|`sonnet`|`opus`) via the `cron_create`/`cron_update` MCP tools; omitting it inherits the agent's global `/model` (unchanged behavior).

**Architecture:** A new nullable `cron_specs.model TEXT` column stores the chosen tier alias. The MCP param is a 3-value enum mapped to the bare CC alias and passed straight to `--model`. At cron fire time, `resolve_cron_model(spec, global)` prefers the spec's model and falls back to the global snapshot. The `right-cron` skill teaches the complexity → tier heuristic; the runtime never classifies.

**Tech Stack:** Rust (edition 2024), `right-db` (Turso) migrations, `schemars`/`serde` for MCP param schemas, `rmcp` tool router, `arc_swap` for the hot-reloadable global model.

**Spec:** `docs/superpowers/specs/2026-06-03-cron-configurable-model-design.md`

**Verification cadence:** Targeted per-crate tests after each task (`devenv shell -- cargo test -p <crate> <filter>`). One mandatory full `devenv shell -- cargo test --workspace` at the end (Task 7). All commands are prefixed with `devenv shell --` per project convention.

**Crate map (where each change lands):**
- `right-db` — migration v42 (Task 1).
- `right-agent` — `CronSpec` field, persist/load/list (Tasks 2, 4).
- `right` — `CronModel` enum + MCP params + tool handlers (Tasks 3, 4).
- `bot` — `resolve_cron_model` + cron fire sites (Task 5).
- `right-codegen` — `right-cron` SKILL.md (Task 6).

---

### Task 1: DB migration v42 — add `cron_specs.model` column

**Files:**
- Modify: `crates/right-db/src/migrations.rs` (add `v42_cron_model` hook near `v41_cron_force_notify` ~line 750; register entry in `MIGRATIONS.migrations` after the `version: 41` entry ~line 987)
- Test: `crates/right-db/src/migrations.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add this test inside the existing `mod tests` block in `crates/right-db/src/migrations.rs` (mirror the style of the existing `v41`/`v40` column tests — find one with `column_exists` to copy the connection setup):

```rust
#[tokio::test]
async fn v42_adds_cron_specs_model_column() {
    let conn = Connection::open_in_memory().await.unwrap();
    MIGRATIONS.to_latest(&conn).await.unwrap();
    // Column must exist and be nullable: inserting a row without `model` succeeds,
    // and an explicit model value round-trips.
    conn.execute_batch(
        "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, created_at, updated_at) \
         VALUES ('j-null', '17 9 * * *', 'p', 5.0, 1, '2026-06-03T00:00:00Z', '2026-06-03T00:00:00Z'); \
         INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, model, created_at, updated_at) \
         VALUES ('j-set', '17 9 * * *', 'p', 5.0, 1, 'sonnet', '2026-06-03T00:00:00Z', '2026-06-03T00:00:00Z');",
    )
    .await
    .unwrap();
    let got: Option<String> = conn
        .query_row(
            "SELECT model FROM cron_specs WHERE job_name = 'j-set'",
            (),
            |row| row.get(0),
        )
        .await
        .unwrap();
    assert_eq!(got.as_deref(), Some("sonnet"));
    let null_model: Option<String> = conn
        .query_row(
            "SELECT model FROM cron_specs WHERE job_name = 'j-null'",
            (),
            |row| row.get(0),
        )
        .await
        .unwrap();
    assert_eq!(null_model, None);
}
```

If `Connection::open_in_memory`/`query_row` signatures differ from this, copy the exact setup from the nearest existing migration test in the same file (e.g. the test asserting a v40/v41 column).

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-db v42_adds_cron_specs_model_column`
Expected: FAIL — `no such column: model` (the INSERT with `model` errors).

- [ ] **Step 3: Add the migration hook**

Add this function just above `pub static MIGRATIONS` (next to `v41_cron_force_notify`), mirroring v41's table-existence guard:

```rust
/// v42: Add a per-cron `model` column to `cron_specs`.
///
/// Nullable TEXT holding a CC model alias (`haiku`/`sonnet`/`opus`). NULL =
/// inherit the agent's global `/model` (the prior behavior), so existing rows
/// keep working unchanged. Idempotent — checks `pragma_table_info` before the
/// ALTER. Guards on table existence via `sqlite_master` like v41, because the
/// synthetic legacy-v33 test fixture lacks `cron_specs`.
fn v42_cron_model(conn: &dyn MigrationConnection) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        let cron_specs_exists = conn
            .query_i64(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cron_specs'",
                MigrationParams::Empty,
            )
            .await?;
        if cron_specs_exists > 0 && !column_exists(conn, "cron_specs", "model").await? {
            conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN model TEXT")
                .await?;
        }
        Ok(())
    })
}
```

- [ ] **Step 4: Register the migration**

In the `MIGRATIONS.migrations` array, immediately after the `version: 41` entry, add:

```rust
        Migration {
            version: 42,
            sql: "",
            hook: Some(v42_cron_model),
        },
```

- [ ] **Step 5: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-db v42`
Expected: PASS. Also run `devenv shell -- cargo test -p right-db` to confirm no other migration test regressed.

- [ ] **Step 6: Commit**

```bash
git add crates/right-db/src/migrations.rs
git commit -m "feat(cron): add cron_specs.model column (migration v42)"
```

---

### Task 2: `CronSpec.model` field + load/list reads

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (`CronSpec` struct ~line 126; `PartialEq` ~line 145; `load_specs_from_db` ~line 707; `list_specs` ~line 655)
- Test: `crates/right-agent/src/cron_spec_tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/right-agent/src/cron_spec_tests.rs` (use the same in-memory DB + migration setup the other tests in this file use — copy from an existing test such as one that raw-`INSERT`s into `cron_specs` and calls `load_specs_from_db`):

```rust
#[tokio::test]
async fn load_specs_reads_model_column() {
    let conn = test_conn().await; // reuse this file's existing helper that runs MIGRATIONS.to_latest
    conn.execute_batch(
        "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, model, created_at, updated_at) \
         VALUES ('with-model', '17 9 * * *', 'p', 5.0, 1, 'haiku', '2026-06-03T00:00:00Z', '2026-06-03T00:00:00Z'); \
         INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, created_at, updated_at) \
         VALUES ('no-model', '17 9 * * *', 'p', 5.0, 1, '2026-06-03T00:00:00Z', '2026-06-03T00:00:00Z');",
    )
    .await
    .unwrap();
    let specs = super::load_specs_from_db(&conn).await.unwrap();
    assert_eq!(specs["with-model"].model.as_deref(), Some("haiku"));
    assert_eq!(specs["no-model"].model, None);
}

#[test]
fn cron_spec_eq_reacts_to_model_change() {
    let base = super::CronSpec {
        schedule_kind: super::ScheduleKind::Recurring("17 9 * * *".into()),
        prompt: "p".into(),
        lock_ttl: None,
        max_budget_usd: 5.0,
        triggered_at: None,
        trigger_force_notify: false,
        target_chat_id: None,
        target_thread_id: None,
        model: Some("sonnet".into()),
    };
    let mut other = base.clone();
    assert_eq!(base, other);
    other.model = Some("haiku".into());
    assert_ne!(base, other, "changing model must make specs unequal so the reconciler reacts");
}
```

If this file has no `test_conn()` helper, copy the exact connection/migration setup from the nearest existing async test in the same file.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-agent load_specs_reads_model_column cron_spec_eq_reacts_to_model_change`
Expected: FAIL to compile — `CronSpec` has no field `model`.

- [ ] **Step 3: Add the field + PartialEq + reads**

In `crates/right-agent/src/cron_spec.rs`:

a) Add the field to `CronSpec` (after `target_thread_id`):

```rust
    pub target_thread_id: Option<i64>,
    /// Per-cron model alias (`haiku`/`sonnet`/`opus`), or `None` to inherit the
    /// agent's global `/model` at fire time. Config (not transient state) — see
    /// `PartialEq` below.
    pub model: Option<String>,
```

b) In the `impl PartialEq for CronSpec`, add the model comparison (it is job config, like `target_*`):

```rust
            && self.target_chat_id == other.target_chat_id
            && self.target_thread_id == other.target_thread_id
            && self.model == other.model
```

c) In `load_specs_from_db`, add `model` to the SELECT (append to the column list) and to the row closure tuple:

```rust
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, trigger_force_notify, recurring, run_at, target_chat_id, target_thread_id, model FROM cron_specs",
```

Add a trailing element to the tuple returned by the row closure:

```rust
            row.get::<_, Option<i64>>(10)?,
            row.get::<_, Option<String>>(11)?,
```

Add `model` to the destructuring `let (...) = row;` (after `target_thread_id`) and to the `CronSpec { ... }` construction:

```rust
            target_thread_id,
            model,
        ) = row;
```

```rust
                target_chat_id,
                target_thread_id,
                model,
            },
```

d) In `list_specs`, add `s.model` to the SELECT column list and a `"model"` key to the JSON object:

```rust
                    s.target_chat_id, s.target_thread_id, s.model, \
```

Because a new column is inserted before the LEFT JOIN's `r.id`, the positional indexes for `r.*` shift by one. Update the JSON closure: keep `target_chat_id` = index 9, `target_thread_id` = index 10, add `"model": row.get::<_, Option<String>>(11)?,`, then bump `last_run_id` to 12, `last_run_at` to 13, `last_status` to 14:

```rust
                "target_chat_id": row.get::<_, Option<i64>>(9)?,
                "target_thread_id": row.get::<_, Option<i64>>(10)?,
                "model": row.get::<_, Option<String>>(11)?,
                "last_run_id": row.get::<_, Option<String>>(12)?,
                "last_run_at": row.get::<_, Option<String>>(13)?,
                "last_status": row.get::<_, Option<String>>(14)?,
```

- [ ] **Step 4: Fix other `CronSpec` constructors the compiler flags**

Adding a field breaks every struct-literal `CronSpec { ... }`. Run `devenv shell -- cargo build -p right-agent 2>&1 | rg "missing field|CronSpec"` and add `model: None,` to each flagged literal (e.g. existing tests in `cron_spec_tests.rs`). `load_specs_from_db` is already handled in Step 3.

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-agent`
Expected: PASS (new tests green; pre-existing cron tests still green).

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs crates/right-agent/src/cron_spec_tests.rs
git commit -m "feat(cron): read model column into CronSpec (load + list)"
```

---

### Task 3: `CronModel` enum + MCP params

**Files:**
- Modify: `crates/right/src/memory_server.rs` (`CronModel` enum + double-option deserializer near the existing `deserialize_double_option_i64` ~line 157; `CronCreateParams` ~line 32; `CronUpdateParams` ~line 68)
- Test: `crates/right/src/memory_server.rs` inline tests, or the existing `crates/right/src/memory_server_mcp_tests.rs`

- [ ] **Step 1: Write the failing test**

Add to the test module covering param parsing (use `crates/right/src/memory_server_mcp_tests.rs`; if param-deser tests live inline, add there instead):

```rust
#[test]
fn cron_create_params_parse_model_enum() {
    let p: super::CronCreateParams = serde_json::from_value(serde_json::json!({
        "job_name": "j", "schedule": "17 9 * * *", "prompt": "p",
        "target_chat_id": 1, "model": "sonnet"
    }))
    .unwrap();
    assert_eq!(p.model.map(|m| m.as_alias()), Some("sonnet"));

    let p_none: super::CronCreateParams = serde_json::from_value(serde_json::json!({
        "job_name": "j", "schedule": "17 9 * * *", "prompt": "p", "target_chat_id": 1
    }))
    .unwrap();
    assert!(p_none.model.is_none());
}

#[test]
fn cron_update_params_model_double_option() {
    // omitted → None (leave unchanged)
    let omit: super::CronUpdateParams =
        serde_json::from_value(serde_json::json!({ "job_name": "j" })).unwrap();
    assert!(omit.model.is_none());
    // explicit null → Some(None) (clear back to inherit-global)
    let clear: super::CronUpdateParams =
        serde_json::from_value(serde_json::json!({ "job_name": "j", "model": null })).unwrap();
    assert_eq!(clear.model, Some(None));
    // value → Some(Some(Haiku))
    let set: super::CronUpdateParams =
        serde_json::from_value(serde_json::json!({ "job_name": "j", "model": "haiku" })).unwrap();
    assert_eq!(set.model.flatten().map(|m| m.as_alias()), Some("haiku"));
}
```

Note: deriving `PartialEq` on `CronModel` (Step 3) makes `assert_eq!(clear.model, Some(None))` work.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right cron_create_params_parse_model_enum cron_update_params_model_double_option`
Expected: FAIL to compile — `CronModel`, `.model` field, and `.as_alias()` do not exist.

- [ ] **Step 3: Add the enum + deserializer**

In `crates/right/src/memory_server.rs`, near the other param helpers (after `deserialize_double_option_i64`), add:

```rust
/// Per-cron model tier chosen by the creating session. Mapped to the bare CC
/// alias and passed straight to `--model`. Kept local to this module per the
/// project's "no central registries" convention (`feedback_no_central_registries`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CronModel {
    Haiku,
    Sonnet,
    Opus,
}

impl CronModel {
    pub fn as_alias(self) -> &'static str {
        match self {
            Self::Haiku => "haiku",
            Self::Sonnet => "sonnet",
            Self::Opus => "opus",
        }
    }
}

/// Distinguish "field absent" (`None`) from "explicit null" (`Some(None)`) for
/// the nullable `model` on `cron_update`, so the agent can clear it back to
/// inherit-global. Mirrors `deserialize_double_option_i64`.
fn deserialize_double_option_cron_model<'de, D>(
    deserializer: D,
) -> Result<Option<Option<CronModel>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<CronModel>::deserialize(deserializer).map(Some)
}
```

- [ ] **Step 4: Add the fields to the param structs**

In `CronCreateParams`, after `target_thread_id`:

```rust
    #[schemars(
        description = "Model tier for this cron, chosen by complexity: 'haiku' (trivial request-and-format), 'sonnet' (mechanical multi-step — the usual choice), 'opus' (complex reasoning/research). Omit to inherit the agent's current /model. See the right-cron skill for the full heuristic."
    )]
    pub model: Option<CronModel>,
```

In `CronUpdateParams`, after `target_thread_id`:

```rust
    #[schemars(
        description = "New model tier ('haiku'|'sonnet'|'opus'). Pass null to clear back to inheriting the agent's /model. Omit to leave unchanged."
    )]
    #[serde(default, deserialize_with = "deserialize_double_option_cron_model")]
    pub model: Option<Option<CronModel>>,
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right cron_create_params_parse_model_enum cron_update_params_model_double_option`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right/src/memory_server.rs crates/right/src/memory_server_mcp_tests.rs
git commit -m "feat(cron): add model tier enum to cron_create/cron_update params"
```

---

### Task 4: Thread `model` through persist + tool handlers

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (`create_spec_v2` ~line 343; `update_spec_partial` ~line 419 incl. the empty-update guard ~line 451 and the SET builder ~line 540)
- Modify: `crates/right/src/right_backend.rs` (`call_cron_create` ~line 363; `call_cron_update` ~line 398)
- Modify: `crates/right/src/memory_server.rs` (`cron_create` handler ~line 292; `cron_update` handler ~line 320)
- Test: `crates/right/src/right_backend_tests.rs` (end-to-end create + clear)

- [ ] **Step 1: Write the failing test**

Add to `crates/right/src/right_backend_tests.rs` (mirror the existing `cron_update_clears_target_thread_id_with_explicit_null` test for setup — same backend/agent harness):

```rust
#[tokio::test]
async fn cron_create_persists_model_and_update_clears_it() {
    let (backend, agent, dir) = test_backend_with_allowlisted_chat(7).await; // reuse this file's harness

    backend
        .tools_call(&agent, &dir, "cron_create", serde_json::json!({
            "job_name": "j1", "schedule": "17 9 * * *", "prompt": "p",
            "target_chat_id": 7, "model": "haiku"
        }), test_ctx())
        .await
        .expect("cron_create ok");

    let conn = backend.get_conn(&agent).await.unwrap();
    let c = conn.lock().await;
    let m: Option<String> = c
        .query_row("SELECT model FROM cron_specs WHERE job_name='j1'", (), |r| r.get(0))
        .await
        .unwrap();
    assert_eq!(m.as_deref(), Some("haiku"));
    drop(c);

    backend
        .tools_call(&agent, &dir, "cron_update", serde_json::json!({
            "job_name": "j1", "model": null
        }), test_ctx())
        .await
        .expect("cron_update ok");

    let c = conn.lock().await;
    let m2: Option<String> = c
        .query_row("SELECT model FROM cron_specs WHERE job_name='j1'", (), |r| r.get(0))
        .await
        .unwrap();
    assert_eq!(m2, None, "explicit null clears model back to inherit-global");
}
```

Adapt the harness calls (`test_backend_with_allowlisted_chat`, `test_ctx`) to whatever helpers this test file already defines — copy from the nearest existing `cron_create`/`cron_update` test.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right cron_create_persists_model_and_update_clears_it`
Expected: FAIL — model is not persisted (column stays NULL after create).

- [ ] **Step 3: Add `model` param to `create_spec_v2`**

In `crates/right-agent/src/cron_spec.rs`, change the signature to insert `model: Option<&str>` between `target_thread_id` and `immediate`:

```rust
    target_chat_id: Option<i64>,
    target_thread_id: Option<i64>,
    model: Option<&str>,
    immediate: bool,
) -> Result<CronSpecResult, String> {
```

Update the INSERT to include the `model` column and bind it:

```rust
    let result = conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, model, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![job_name, db_schedule, prompt, lock_ttl, budget, db_recurring, db_run_at, target_chat_id, target_thread_id, model, &now, &now],
    )
    .await;
```

- [ ] **Step 4: Add `model` param to `update_spec_partial`**

Change the signature to append `model: Option<Option<&str>>`:

```rust
    target_chat_id: Option<i64>,
    target_thread_id: Option<Option<i64>>,
    model: Option<Option<&str>>,
) -> Result<CronSpecResult, String> {
```

In the "at least one field" empty-update guard, add `&& model.is_none()` so a model-only update is accepted:

```rust
        && target_chat_id.is_none()
        && target_thread_id.is_none()
        && model.is_none()
    {
        return Err("at least one field must be provided to update".into());
    }
```

In the dynamic SET builder, after the `target_thread_id` block, add (mirrors the `target_thread_id` set/clear pattern):

```rust
    if let Some(model_opt) = model {
        match model_opt {
            Some(m) => {
                sets.push("model = ?");
                values
                    .push(m)
                    .map_err(|e| format!("invalid parameter: {e:#}"))?;
            }
            None => {
                sets.push("model = NULL");
            }
        }
    }
```

- [ ] **Step 5: Update the four tool-handler call sites**

In `crates/right/src/right_backend.rs` `call_cron_create`, add the model argument (between `params.target_thread_id` and `false`):

```rust
            params.target_thread_id,
            params.model.map(|m| m.as_alias()),
            false,
```

In `call_cron_update`, append the model argument after `params.target_thread_id`:

```rust
            params.target_thread_id,
            params.model.map(|o| o.map(|m| m.as_alias())),
        )
```

In `crates/right/src/memory_server.rs` `cron_create` handler, same insertion before `false`:

```rust
            params.target_thread_id,
            params.model.map(|m| m.as_alias()),
            false,
```

In the `cron_update` handler, append after `params.target_thread_id`:

```rust
            params.target_thread_id,
            params.model.map(|o| o.map(|m| m.as_alias())),
        )
```

- [ ] **Step 6: Fix other callers the compiler flags**

`create_spec_v2`/`update_spec_partial` signature changes break existing test callers (e.g. in `crates/right-agent/src/cron_spec_tests.rs`). Run `devenv shell -- cargo build --workspace 2>&1 | rg "this function takes|arguments were supplied|create_spec_v2|update_spec_partial"` and add the new argument to each: `None` for `create_spec_v2` (the inserted `model` arg, before the trailing `immediate` bool) and `None` for `update_spec_partial` (appended). Tests that don't exercise model pass `None`.

- [ ] **Step 7: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right cron_create_persists_model_and_update_clears_it`
Then: `devenv shell -- cargo test -p right-agent -p right`
Expected: PASS across both crates.

- [ ] **Step 8: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs crates/right/src/right_backend.rs crates/right/src/memory_server.rs crates/right/src/right_backend_tests.rs crates/right-agent/src/cron_spec_tests.rs
git commit -m "feat(cron): persist per-cron model via create/update tools"
```

---

### Task 5: Resolve per-cron model at fire time

**Files:**
- Modify: `crates/bot/src/cron.rs` (add `resolve_cron_model` helper; replace the three `let md: Option<String> = crate::snapshot_model(...)` lines at ~1777, ~1969, ~2066)
- Test: `crates/bot/src/cron.rs` inline `#[cfg(test)] mod tests` (extend near `cron_reads_current_model_from_arcswap` ~line 2228)

- [ ] **Step 1: Write the failing test**

Add to the test module in `crates/bot/src/cron.rs`:

```rust
#[test]
fn resolve_cron_model_prefers_spec_then_global() {
    let global = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(Some("opus".to_string())));

    let spec_with = CronSpec {
        schedule_kind: right_agent::cron_spec::ScheduleKind::Recurring("17 9 * * *".into()),
        prompt: "p".into(),
        lock_ttl: None,
        max_budget_usd: 5.0,
        triggered_at: None,
        trigger_force_notify: false,
        target_chat_id: None,
        target_thread_id: None,
        model: Some("haiku".into()),
    };
    assert_eq!(resolve_cron_model(&spec_with, &global).as_deref(), Some("haiku"));

    let mut spec_without = spec_with.clone();
    spec_without.model = None;
    assert_eq!(resolve_cron_model(&spec_without, &global).as_deref(), Some("opus"));

    let empty_global = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(None::<String>));
    assert_eq!(resolve_cron_model(&spec_without, &empty_global), None);
}
```

Use the `CronSpec` import path already used in this file (it imports `right_agent::cron_spec::CronSpec` — match the existing alias).

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p bot resolve_cron_model_prefers_spec_then_global`
Expected: FAIL to compile — `resolve_cron_model` not defined.

- [ ] **Step 3: Add the helper**

Add near the top of `crates/bot/src/cron.rs` (module-level fn, above `execute_job`):

```rust
/// Resolve the model for a cron firing: the spec's own model wins; otherwise
/// fall back to the agent's current global `/model` snapshot; otherwise `None`
/// (CC default). Snapshotting at fire time keeps `/model` hot-reload working.
fn resolve_cron_model(
    spec: &CronSpec,
    global: &arc_swap::ArcSwap<Option<String>>,
) -> Option<String> {
    spec.model
        .clone()
        .or_else(|| crate::snapshot_model(global))
}
```

- [ ] **Step 4: Use it at the three fire sites**

Replace each existing snapshot line with the resolver (the `spec` variable is in scope at all three):

At ~line 1777 (one-shot loop) and ~line 1969 (triggered loop), where `model: &Arc<ArcSwap<Option<String>>>`:

```rust
        let md: Option<String> = resolve_cron_model(spec, model);
```

At ~line 2066 (recurring loop), where `model: Arc<ArcSwap<Option<String>>>` is owned and currently passed as `&model`:

```rust
        let md: Option<String> = resolve_cron_model(spec, &model);
```

(Deref coercion turns `&Arc<ArcSwap<_>>` into `&ArcSwap<_>`, matching the helper signature — same coercion `snapshot_model(model)` already relied on.) No change inside `execute_job` is needed: it already forwards `model` (now the resolved value) into `ClaudeInvocation.model`, the learning `probe_writer_model_fallback`, and the reflection context.

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p bot resolve_cron_model_prefers_spec_then_global cron_reads_current_model_from_arcswap`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): prefer per-cron model over global at fire time"
```

---

### Task 6: Teach the `right-cron` skill the model heuristic

**Files:**
- Modify: `crates/right-codegen/skills/right-cron/SKILL.md` (frontmatter `version`; new "Choosing the Model" section; `model` row in the Parameters table)

- [ ] **Step 1: Bump the skill version**

In the frontmatter, change `version: 3.4.0` to `version: 3.5.0`.

- [ ] **Step 2: Add the model row to the Parameters table**

In the `## Parameters` table, add a row after `max_budget_usd`:

```markdown
| `model` | enum | No | inherit | `haiku` \| `sonnet` \| `opus`. Picks the model for this cron by complexity (see "Choosing the Model"). Omit to inherit the agent's current `/model`. |
```

- [ ] **Step 3: Add the "Choosing the Model" section**

Insert this section just after the `## Writing Cron Prompts` section:

```markdown
## Choosing the Model

The session that creates a cron picks that cron's model by judging the task's
complexity — the runtime never decides for you. Set `model:` on `cron_create`
(and `cron_update`) deliberately:

- **`haiku`** — trivial: one request or tool call plus mechanical formatting of
  the result (fetch a page, extract a field, report it). No reasoning, no
  multi-step decisions.
- **`sonnet`** — mechanical multi-step with light judgment: health checks,
  summaries, scheduled briefings, status polls. **This is the right default for
  most crons.**
- **`opus`** — genuinely complex: multi-source research, nuanced analysis,
  anything you'd want your strongest model for.

Omit `model` only when you deliberately want the cron to track the agent's
current `/model`. Otherwise set it explicitly — a mechanical cron left on an
Opus global wastes budget and latency.

To change a cron's model later: `mcp__right__cron_update(job_name: "...", model: "sonnet")`.
Pass `model: null` to clear it back to inheriting the agent's `/model`.
```

- [ ] **Step 4: Verify the skill still builds into the codegen bundle**

Run: `devenv shell -- cargo test -p right-codegen`
Expected: PASS (the include_dir embed + any skill-install test still green).

- [ ] **Step 5: Commit**

```bash
git add crates/right-codegen/skills/right-cron/SKILL.md
git commit -m "docs(cron): teach right-cron the model-by-complexity heuristic"
```

---

### Task 7: Final full-workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full workspace test suite (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. If a known-flaky test fails (see `project_flaky_tests_parallel_load` — cc/invocation pid race, dashboard warn-count), re-run it isolated before attributing the failure to this change.

- [ ] **Step 2: Build check**

Run: `devenv shell -- cargo build --workspace`
Expected: clean build.

- [ ] **Step 3: Cite-on-touch doc check**

Skim `docs/architecture/sessions.md` for any `CronSpec`-field listing; if it enumerates spec fields, add `model` (per the AGENTS.md cite-on-touch rule). No `ARCHITECTURE.md` change is needed (new optional param, not a new invariant). If a doc edit was made, commit it:

```bash
git add docs/architecture/sessions.md
git commit -m "docs(cron): note per-cron model field in sessions satellite"
```

---

## Self-review notes (for the implementer)

- **Spec coverage:** migration (Task 1) ↔ spec §3; CronSpec/load/list (Task 2) ↔ §2; enum+params (Task 3) ↔ §1; persist+handlers (Task 4) ↔ §1–2; resolve at fire time (Task 5) ↔ §4; skill (Task 6) ↔ §5; docs+tests (Task 7) ↔ §6 + Testing.
- **Out of scope (do NOT add):** per-run recorded model in `async_runs`; `cron_trigger` one-off override; centralizing `MODEL_CHOICES`; `[1m]` variants.
- **Type consistency:** `CronModel::as_alias(self) -> &'static str`; `CronSpec.model: Option<String>`; `create_spec_v2(..., model: Option<&str>, immediate: bool)`; `update_spec_partial(..., model: Option<Option<&str>>)`; `resolve_cron_model(&CronSpec, &arc_swap::ArcSwap<Option<String>>) -> Option<String>`. The same names are used in every task that references them.
