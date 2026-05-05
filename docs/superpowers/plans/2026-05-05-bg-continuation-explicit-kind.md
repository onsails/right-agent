# Background Continuation: Explicit Kind & Mandatory Notify — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lift bg-continuation jobs into a typed `ScheduleKind::BackgroundContinuation { fork_from }` variant, enforce non-silent agent output via a dedicated JSON schema, and surface in-flight bg jobs to the main session via a `<background-jobs>` marker.

**Architecture:** New enum variant + sentinel encoding `@bg:<uuid>` in the existing `cron_specs.schedule` column (no DDL). New `BG_CONTINUATION_SCHEMA_JSON` selected when the variant is matched in `cron::execute_job` via a new `select_schema_and_fork` helper. New helper `insert_background_continuation` replaces `insert_immediate_cron`. Marker reads `cron_runs` for the current chat and is appended to `composite-memory.md` next to `<memory-status>`. One-time startup migration rewrites legacy `@immediate` + `X-FORK-FROM:` rows into the new shape.

**Tech Stack:** Rust 2024, rusqlite, tokio, teloxide, anyhow/thiserror, uuid.

**Spec:** `docs/superpowers/specs/2026-05-05-bg-continuation-explicit-kind-design.md`.

---

## Task ordering rationale

Each task leaves the system buildable and consistent. The order is chosen to avoid intermediate states where bg rows would be created but the dispatch path or reconciler isn't ready yet. Specifically:

1. Define the type (variant) and the schema before any caller depends on them.
2. Wire `select_schema_and_fork` and the kind-driven dispatch in `cron::execute_job` BEFORE the reconciler can fire bg-kind rows.
3. Extend the reconcile filters to fire bg-kind rows BEFORE the worker can create them.
4. THEN switch the worker to produce bg-kind rows (atomic with deletion of the old `insert_immediate_cron` path).

This keeps every commit consistent end-to-end.

## File Map

### Modified (no new files)

- `crates/right-agent/src/cron_spec.rs` — variant, `from_db_row`, `insert_background_continuation`, drop `insert_immediate_cron`.
- `crates/right-agent/src/cron_spec_tests.rs` — variant tests, replace `insert_immediate_cron` tests with `insert_background_continuation` tests.
- `crates/right-agent/src/codegen/agent_def.rs` — `BG_CONTINUATION_SCHEMA_JSON` constant.
- `crates/right-agent/src/codegen/agent_def_tests.rs` — schema shape tests.
- `crates/bot/src/cron.rs` — `select_schema_and_fork` helper, drop X-FORK-FROM block, extend reconcile filters, `migrate_legacy_bg_continuation`, tests.
- `crates/bot/src/telegram/worker.rs` — update `build_continuation_prompt`, rewrite `enqueue_background_job`, add `build_bg_marker_for_chat`, wire into pre-CC flow, tests.
- `crates/bot/src/telegram/prompt.rs` — extend `deploy_composite_memory` signature with `bg_marker`.
- `crates/bot/tests/cron_immediate.rs` — migrate to `insert_background_continuation`.
- `crates/bot/src/lib.rs` — call `migrate_legacy_bg_continuation` on bot startup.
- `ARCHITECTURE.md` — replace X-FORK-FROM convention section.
- `docs/architecture/sessions.md` — update if it carries the same wording.

### Deviation from spec

The spec proposes routing `insert_background_continuation` through `create_spec_v2` with a new `bg_fork_from: Option<Uuid>` parameter. `create_spec_v2` already has 11 args; adding a 12th is unmaintainable. This plan instead has `insert_background_continuation` build its INSERT directly, reusing the existing validators (`validate_job_name`) explicitly. Net DB behavior is identical.

---

## Task 0: Setup feature branch

**Files:**
- (working tree only)

- [ ] **Step 1: Inspect the current state**

```bash
git status --short
```

If unrelated modifications are present (`cron.rs`, `worker.rs`, etc. from prior work), halt and ask the user. Do not stash unilaterally.

- [ ] **Step 2: Create feature branch**

```bash
git checkout -b feat/bg-continuation-explicit-kind
```

- [ ] **Step 3: Verify the codebase builds before starting**

```bash
cargo check --workspace
```

Expected: green. If errors exist before our changes, halt and report.

---

## Task 1: Extract `ScheduleKind::from_db_row` (refactor, no behavior change)

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs:74-97` (impl block) and `:707-721` (inline match in `load_specs_from_db`).
- Modify: `crates/right-agent/src/cron_spec_tests.rs` (append tests).

- [ ] **Step 1: Write failing tests**

Append to `crates/right-agent/src/cron_spec_tests.rs` inside the existing `mod tests`:

```rust
#[test]
fn from_db_row_recurring() {
    let kind = ScheduleKind::from_db_row("*/5 * * * *", None, 1).unwrap();
    assert!(matches!(kind, ScheduleKind::Recurring(s) if s == "*/5 * * * *"));
}

#[test]
fn from_db_row_one_shot_cron() {
    let kind = ScheduleKind::from_db_row("0 9 * * *", None, 0).unwrap();
    assert!(matches!(kind, ScheduleKind::OneShotCron(s) if s == "0 9 * * *"));
}

#[test]
fn from_db_row_run_at() {
    let kind = ScheduleKind::from_db_row("", Some("2026-12-25T00:00:00Z"), 0).unwrap();
    assert!(matches!(kind, ScheduleKind::RunAt(_)));
}

#[test]
fn from_db_row_immediate_sentinel() {
    let kind = ScheduleKind::from_db_row(IMMEDIATE_SENTINEL, None, 0).unwrap();
    assert!(matches!(kind, ScheduleKind::Immediate));
}

#[test]
fn from_db_row_invalid_run_at_returns_err() {
    let err = ScheduleKind::from_db_row("", Some("not-a-date"), 0);
    assert!(err.is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p right-agent --lib from_db_row_
```

Expected: build error — `ScheduleKind::from_db_row` does not exist.

- [ ] **Step 3: Implement `from_db_row`**

In `crates/right-agent/src/cron_spec.rs`, inside the existing `impl ScheduleKind` block (cron_spec.rs:74-87), add:

```rust
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
    if schedule == IMMEDIATE_SENTINEL {
        return Ok(Self::Immediate);
    }
    if recurring == 0 {
        Ok(Self::OneShotCron(schedule.to_string()))
    } else {
        Ok(Self::Recurring(schedule.to_string()))
    }
}
```

- [ ] **Step 4: Replace inline match in `load_specs_from_db`**

Replace `cron_spec.rs:707-721` (the `let schedule_kind = if let Some(...)` block) with:

```rust
        let schedule_kind = match ScheduleKind::from_db_row(&schedule, run_at.as_deref(), recurring) {
            Ok(k) => k,
            Err(e) => {
                tracing::error!(job = %job_name, "failed to parse schedule kind: {e}");
                continue;
            }
        };
```

- [ ] **Step 5: Run all cron-spec tests**

```bash
cargo test -p right-agent --lib cron_spec
```

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs crates/right-agent/src/cron_spec_tests.rs
git commit -m "refactor(cron-spec): extract ScheduleKind::from_db_row from inline match"
```

---

## Task 2: Add `BackgroundContinuation` variant + parser + Display

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (variant, `from_db_row`, `Display`, `is_one_shot`, `cron_schedule`).
- Modify: `crates/right-agent/src/cron_spec_tests.rs` (append tests).

- [ ] **Step 1: Confirm `uuid` is in scope**

```bash
rg "^uuid" crates/right-agent/Cargo.toml
```

Expected: present as a regular dependency (it is — used by other code).

- [ ] **Step 2: Write failing tests**

Append to `crates/right-agent/src/cron_spec_tests.rs`:

```rust
#[test]
fn from_db_row_bg_continuation() {
    let main = uuid::Uuid::new_v4();
    let kind = ScheduleKind::from_db_row(&format!("@bg:{main}"), None, 0).unwrap();
    match kind {
        ScheduleKind::BackgroundContinuation { fork_from } => {
            assert_eq!(fork_from, main);
        }
        other => panic!("expected BackgroundContinuation, got {other:?}"),
    }
}

#[test]
fn from_db_row_bg_invalid_uuid_errors() {
    let err = ScheduleKind::from_db_row("@bg:not-a-uuid", None, 0);
    assert!(err.is_err());
    assert!(err.unwrap_err().contains("invalid"));
}

#[test]
fn from_db_row_bg_missing_uuid_errors() {
    let err = ScheduleKind::from_db_row("@bg:", None, 0);
    assert!(err.is_err());
}

#[test]
fn bg_kind_display_round_trips() {
    let main = uuid::Uuid::new_v4();
    let kind = ScheduleKind::BackgroundContinuation { fork_from: main };
    let s = format!("{kind}");
    assert_eq!(s, format!("@bg:{main}"));
    let parsed = ScheduleKind::from_db_row(&s, None, 0).unwrap();
    assert_eq!(kind, parsed);
}

#[test]
fn bg_kind_is_one_shot() {
    let kind = ScheduleKind::BackgroundContinuation {
        fork_from: uuid::Uuid::new_v4(),
    };
    assert!(kind.is_one_shot());
}

#[test]
fn bg_kind_no_cron_schedule() {
    let kind = ScheduleKind::BackgroundContinuation {
        fork_from: uuid::Uuid::new_v4(),
    };
    assert!(kind.cron_schedule().is_none());
}

#[test]
fn immediate_kind_still_parses_after_bg_addition() {
    let kind = ScheduleKind::from_db_row(IMMEDIATE_SENTINEL, None, 0).unwrap();
    assert!(matches!(kind, ScheduleKind::Immediate));
}
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p right-agent --lib cron_spec
```

Expected: build error — variant `BackgroundContinuation` does not exist.

- [ ] **Step 4: Add the variant and update impls**

In `crates/right-agent/src/cron_spec.rs`, near top imports add:

```rust
use uuid::Uuid;
```

After the `IMMEDIATE_SENTINEL` constant (cron_spec.rs:32), add:

```rust
/// Sentinel prefix for `BackgroundContinuation` schedule encoding.
/// Stored as `@bg:<fork_from-uuid>` in the `schedule` column.
pub(crate) const BG_SENTINEL_PREFIX: &str = "@bg:";
```

Replace `enum ScheduleKind` (cron_spec.rs:60-72) with:

```rust
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
    /// Bot-internal background continuation: fires on the next reconcile
    /// tick with `--resume <fork_from> --fork-session --session-id <run_id>`,
    /// auto-deletes. Stored as `'@bg:<fork_from-uuid>'`.
    BackgroundContinuation { fork_from: Uuid },
}
```

Replace `cron_schedule`:

```rust
pub fn cron_schedule(&self) -> Option<&str> {
    match self {
        Self::Recurring(s) | Self::OneShotCron(s) => Some(s),
        Self::RunAt(_) | Self::Immediate | Self::BackgroundContinuation { .. } => None,
    }
}
```

Replace `is_one_shot`:

```rust
pub fn is_one_shot(&self) -> bool {
    matches!(
        self,
        Self::OneShotCron(_)
            | Self::RunAt(_)
            | Self::Immediate
            | Self::BackgroundContinuation { .. }
    )
}
```

Replace `Display`:

```rust
impl std::fmt::Display for ScheduleKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Recurring(s) | Self::OneShotCron(s) => f.write_str(s),
            Self::RunAt(dt) => write!(f, "{}", dt.to_rfc3339()),
            Self::Immediate => f.write_str(IMMEDIATE_SENTINEL),
            Self::BackgroundContinuation { fork_from } => {
                write!(f, "{BG_SENTINEL_PREFIX}{fork_from}")
            }
        }
    }
}
```

Update `from_db_row` (added in Task 1) to recognise the bg sentinel — insert the bg branch BEFORE the `IMMEDIATE_SENTINEL` check:

```rust
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
        let fork_from = Uuid::parse_str(rest)
            .map_err(|e| format!("invalid bg fork_from UUID '{rest}': {e}"))?;
        return Ok(Self::BackgroundContinuation { fork_from });
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
```

If the test file lacks `use uuid::Uuid;`, add it at the top.

- [ ] **Step 5: Run tests**

```bash
cargo test -p right-agent --lib cron_spec
cargo build --workspace
```

Expected: all bg tests pass; existing tests unchanged. Workspace builds (the new variant doesn't yet appear in any production `match` outside cron_spec.rs, but compiler permits as long as `_` arms exist or `matches!` is used).

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs crates/right-agent/src/cron_spec_tests.rs
git commit -m "feat(cron-spec): add ScheduleKind::BackgroundContinuation variant"
```

---

## Task 3: Add `BG_CONTINUATION_SCHEMA_JSON` constant + tests

**Files:**
- Modify: `crates/right-agent/src/codegen/agent_def.rs` (append constant).
- Modify: `crates/right-agent/src/codegen/agent_def_tests.rs` (append tests).

- [ ] **Step 1: Write failing tests**

Append to `crates/right-agent/src/codegen/agent_def_tests.rs`:

```rust
#[test]
fn bg_continuation_schema_requires_notify() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let required = v["required"].as_array().unwrap();
    let names: Vec<&str> = required.iter().filter_map(|x| x.as_str()).collect();
    assert!(names.contains(&"notify"), "notify must be required");
    assert!(names.contains(&"summary"), "summary must be required");
}

#[test]
fn bg_continuation_schema_notify_is_non_nullable_object() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let notify_type = &v["properties"]["notify"]["type"];
    assert_eq!(notify_type.as_str(), Some("object"),
               "notify must be non-nullable; got {:?}", notify_type);
}

#[test]
fn bg_continuation_schema_content_min_length_one() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let min_len = &v["properties"]["notify"]["properties"]["content"]["minLength"];
    assert_eq!(min_len.as_i64(), Some(1));
}

#[test]
fn bg_continuation_schema_no_notify_reason_field_absent() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let props = v["properties"].as_object().unwrap();
    assert!(!props.contains_key("no_notify_reason"),
            "no_notify_reason must not be in bg schema");
}

#[test]
fn bg_continuation_schema_attachments_item_has_media_group_id() {
    let items = attachments_item_schema(
        BG_CONTINUATION_SCHEMA_JSON,
        &["properties", "notify", "properties", "attachments", "items"],
    );
    assert_has_nullable_media_group_id(&items);
}
```

If `BG_CONTINUATION_SCHEMA_JSON` is not in scope of the test file, copy the import pattern that brings `CRON_SCHEMA_JSON` in (look at existing imports near top of `agent_def_tests.rs`).

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p right-agent --lib agent_def
```

Expected: build error — `BG_CONTINUATION_SCHEMA_JSON` does not exist.

- [ ] **Step 3: Add the constant**

Append to `crates/right-agent/src/codegen/agent_def.rs` (right after `CRON_SCHEMA_JSON` at line 34):

```rust
/// Structured-output schema for background-continuation cron runs.
///
/// `notify` is required and non-null; `notify.content` must be a non-empty
/// string. `summary` is required (kept for log/analytics parity with
/// `CRON_SCHEMA_JSON`). `no_notify_reason` is absent — silence is not a
/// valid outcome for this job kind, since the user is waiting for the
/// foreground answer that was sent to background.
pub const BG_CONTINUATION_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"notify":{"type":"object","properties":{"content":{"type":"string","minLength":1},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]},"summary":{"type":"string"}},"required":["summary","notify"]}"#;
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p right-agent --lib agent_def
```

Expected: green.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/codegen/agent_def.rs crates/right-agent/src/codegen/agent_def_tests.rs
git commit -m "feat(codegen): add BG_CONTINUATION_SCHEMA_JSON for forked bg turns"
```

---

## Task 4: Add `select_schema_and_fork` helper

**Files:**
- Modify: `crates/bot/src/cron.rs` (add helper above `execute_job`).
- Modify: `crates/bot/src/cron.rs::tests` (append).

- [ ] **Step 1: Write failing tests**

Find the `#[cfg(test)] mod tests` block in `crates/bot/src/cron.rs` (look for the existing `parse_cron_output_*` tests). Append:

```rust
#[test]
fn select_schema_for_recurring_uses_cron_schema() {
    let spec = right_agent::cron_spec::CronSpec {
        schedule_kind: right_agent::cron_spec::ScheduleKind::Recurring("*/5 * * * *".into()),
        prompt: "p".into(),
        lock_ttl: None,
        max_budget_usd: 1.0,
        triggered_at: None,
        target_chat_id: None,
        target_thread_id: None,
    };
    let (schema, fork) = select_schema_and_fork(&spec);
    assert_eq!(schema, right_agent::codegen::CRON_SCHEMA_JSON);
    assert!(fork.is_none());
}

#[test]
fn select_schema_for_immediate_uses_cron_schema() {
    let spec = right_agent::cron_spec::CronSpec {
        schedule_kind: right_agent::cron_spec::ScheduleKind::Immediate,
        prompt: "p".into(),
        lock_ttl: None,
        max_budget_usd: 1.0,
        triggered_at: None,
        target_chat_id: None,
        target_thread_id: None,
    };
    let (schema, fork) = select_schema_and_fork(&spec);
    assert_eq!(schema, right_agent::codegen::CRON_SCHEMA_JSON);
    assert!(fork.is_none());
}

#[test]
fn select_schema_for_bg_uses_bg_schema_and_fork_from() {
    let main = uuid::Uuid::new_v4();
    let spec = right_agent::cron_spec::CronSpec {
        schedule_kind: right_agent::cron_spec::ScheduleKind::BackgroundContinuation { fork_from: main },
        prompt: "p".into(),
        lock_ttl: None,
        max_budget_usd: 1.0,
        triggered_at: None,
        target_chat_id: None,
        target_thread_id: None,
    };
    let (schema, fork) = select_schema_and_fork(&spec);
    assert_eq!(schema, right_agent::codegen::BG_CONTINUATION_SCHEMA_JSON);
    assert_eq!(fork.as_deref(), Some(main.to_string().as_str()));
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p right-bot --lib select_schema
```

Expected: build error — `select_schema_and_fork` doesn't exist.

- [ ] **Step 3: Implement the helper**

In `crates/bot/src/cron.rs`, just above `async fn execute_job` (around line 237), add:

```rust
/// Pick the JSON schema and (optional) `--fork-session` source for a cron run.
///
/// `BackgroundContinuation` is the only kind that runs against
/// [`right_agent::codegen::BG_CONTINUATION_SCHEMA_JSON`] — its forked turn
/// MUST reply (notify required + non-null) because the user is waiting for
/// the foreground answer sent to background. All other kinds use
/// [`right_agent::codegen::CRON_SCHEMA_JSON`] where `notify: null` (silent)
/// is a valid outcome.
fn select_schema_and_fork(
    spec: &right_agent::cron_spec::CronSpec,
) -> (&'static str, Option<String>) {
    match &spec.schedule_kind {
        right_agent::cron_spec::ScheduleKind::BackgroundContinuation { fork_from } => (
            right_agent::codegen::BG_CONTINUATION_SCHEMA_JSON,
            Some(fork_from.to_string()),
        ),
        _ => (right_agent::codegen::CRON_SCHEMA_JSON, None),
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p right-bot --lib select_schema
```

Expected: 3 green.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): add select_schema_and_fork helper for kind-aware invocation"
```

---

## Task 5: Wire `select_schema_and_fork` into `execute_job`; delete X-FORK-FROM block

**Files:**
- Modify: `crates/bot/src/cron.rs:320-376` (delete X-FORK-FROM block, replace `json_schema` source).

After this task, `cron::execute_job` is kind-aware:
- `BackgroundContinuation` → bg schema + `--fork-session`.
- All other kinds → cron schema, no fork.

Legacy `@immediate` + `X-FORK-FROM:` rows (if any exist before Task 8 migration) become plain `Immediate` kind — they fire as ordinary one-shots without bg semantics. This is the intended degraded behaviour: legacy rows get migrated by Task 8 on next bot startup, after which they dispatch correctly.

- [ ] **Step 1: Confirm exact block lines**

```bash
rg -n "X-FORK-FROM|fork_from_main_session|let \(fork_from_main_session" crates/bot/src/cron.rs
```

The block is the comment `// Optional X-FORK-FROM header in the prompt:` (≈ cron.rs:320) through the closing `};` of the `let (fork_from_main_session, prompt_for_cc) = …` chain (≈ cron.rs:358).

- [ ] **Step 2: Replace the block**

Delete the entire X-FORK-FROM parsing block (cron.rs:320-358) and replace with:

```rust
    // Schema and (optional) --fork-session source come from spec.schedule_kind.
    // BackgroundContinuation produces both a stricter schema (bg) and a
    // resume-target main session UUID; everything else gets the regular
    // cron schema and no fork.
    let (json_schema_str, fork_from_main_session) = select_schema_and_fork(spec);
    let prompt_for_cc = spec.prompt.clone();
```

In the `ClaudeInvocation { … }` literal a few lines below, change:

```rust
        json_schema: Some(right_agent::codegen::CRON_SCHEMA_JSON.into()),
```

to:

```rust
        json_schema: Some(json_schema_str.into()),
```

Verify that `resume_session_id` and `fork_session` fields keep using `fork_from_main_session.clone()` and `fork_from_main_session.is_some()` — they should already.

- [ ] **Step 3: Build and test**

```bash
cargo build --workspace
cargo test -p right-bot --lib cron
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "refactor(cron): replace X-FORK-FROM prompt parsing with kind-driven dispatch"
```

---

## Task 6: Extend reconcile match arms to fire `BackgroundContinuation`

**Files:**
- Modify: `crates/bot/src/cron.rs:1142-1146` (immediate-fire filter).
- Modify: `crates/bot/src/cron.rs:1182-1186` (skip-from-run_job_loop filter).
- Modify: `crates/bot/src/cron.rs::tests` (append).

After this task, `BackgroundContinuation` rows are eligible for firing on every reconcile tick AND are excluded from the spawn-recurring-handle loop. This is the gate that enables Task 7 to actually create such rows.

- [ ] **Step 1: Write failing test (regression guard)**

Append to `crates/bot/src/cron.rs::tests`:

```rust
#[test]
fn reconcile_immediate_filter_includes_bg_continuation() {
    use right_agent::cron_spec::{CronSpec, ScheduleKind};
    let main = uuid::Uuid::new_v4();
    let specs: std::collections::HashMap<String, CronSpec> = [(
        "bg-1".to_string(),
        CronSpec {
            schedule_kind: ScheduleKind::BackgroundContinuation { fork_from: main },
            prompt: "p".into(),
            lock_ttl: Some("6h".into()),
            max_budget_usd: 5.0,
            triggered_at: None,
            target_chat_id: Some(-1),
            target_thread_id: None,
        },
    )]
    .into_iter()
    .collect();

    // Mirror the production filter exactly. If it drifts, this test fails.
    let immediate: Vec<(String, CronSpec)> = specs
        .iter()
        .filter(|(_, spec)| {
            matches!(
                &spec.schedule_kind,
                ScheduleKind::Immediate | ScheduleKind::BackgroundContinuation { .. }
            )
        })
        .map(|(name, spec)| (name.clone(), spec.clone()))
        .collect();
    assert_eq!(immediate.len(), 1, "bg row must match the immediate-fire filter");
}

#[test]
fn reconcile_skip_filter_excludes_bg_from_run_job_loop() {
    use right_agent::cron_spec::ScheduleKind;
    let kind = ScheduleKind::BackgroundContinuation { fork_from: uuid::Uuid::new_v4() };
    assert!(matches!(
        kind,
        ScheduleKind::RunAt(_) | ScheduleKind::Immediate | ScheduleKind::BackgroundContinuation { .. }
    ));
}
```

These tests assert the exact `matches!` patterns we're about to install. They'll pass on their own — the value is regression guarding (someone removing `BackgroundContinuation` from the production filter will get a test failure here mirroring it).

- [ ] **Step 2: Run tests (should pass without prod change)**

```bash
cargo test -p right-bot --lib reconcile_
```

Expected: 2 green.

- [ ] **Step 3: Update production filter at cron.rs:1142-1146**

Replace:

```rust
        .filter(|(_, spec)| {
            matches!(
                &spec.schedule_kind,
                right_agent::cron_spec::ScheduleKind::Immediate
            )
        })
```

with:

```rust
        .filter(|(_, spec)| {
            matches!(
                &spec.schedule_kind,
                right_agent::cron_spec::ScheduleKind::Immediate
                    | right_agent::cron_spec::ScheduleKind::BackgroundContinuation { .. }
            )
        })
```

Update the corresponding `tracing::info!` label `"immediate"` (at the call to `fire_one_shot_specs(immediate, "immediate", ...)`) to:

```rust
        "immediate-or-bg",
```

- [ ] **Step 4: Update production filter at cron.rs:1182-1186**

Replace:

```rust
        if matches!(
            spec.schedule_kind,
            right_agent::cron_spec::ScheduleKind::RunAt(_)
                | right_agent::cron_spec::ScheduleKind::Immediate
        ) {
            continue;
        }
```

with:

```rust
        if matches!(
            spec.schedule_kind,
            right_agent::cron_spec::ScheduleKind::RunAt(_)
                | right_agent::cron_spec::ScheduleKind::Immediate
                | right_agent::cron_spec::ScheduleKind::BackgroundContinuation { .. }
        ) {
            continue;
        }
```

- [ ] **Step 5: Build and run all bot tests**

```bash
cargo build --workspace
cargo test -p right-bot --lib
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): extend reconcile filters to fire BackgroundContinuation jobs"
```

---

## Task 7: Add `insert_background_continuation`, rewrite `enqueue_background_job`, delete `insert_immediate_cron`

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs` (replace `insert_immediate_cron` with `insert_background_continuation`).
- Modify: `crates/right-agent/src/cron_spec_tests.rs` (replace tests for `insert_immediate_cron`).
- Modify: `crates/bot/src/telegram/worker.rs:486-508` (rewrite `enqueue_background_job`).
- Modify: `crates/bot/src/telegram/worker.rs::tests` (update existing test `enqueue_background_job_inserts_immediate_with_target`).
- Modify: `crates/bot/tests/cron_immediate.rs` (migrate to `insert_background_continuation`).

This is the atomic worker-side switch. After this commit, the worker writes `@bg:<uuid>` rows that:
- The reconciler picks up (Task 6).
- `execute_job` dispatches with bg schema + fork-session (Task 5).
- The continuation prompt has NO `X-FORK-FROM:` line — the variant carries `fork_from`.

- [ ] **Step 1: Identify all sites that reference `insert_immediate_cron`**

```bash
rg -n "insert_immediate_cron" --type rust
```

Expected sites:
- `crates/right-agent/src/cron_spec.rs:358` — the function (delete).
- `crates/right-agent/src/cron_spec_tests.rs:1117,1135,1137` — old tests (replace).
- `crates/bot/src/telegram/worker.rs:506` — the only production caller (rewrite).
- `crates/bot/tests/cron_immediate.rs:7,17,18` — integration test (migrate).

If any other site appears, halt and inspect.

- [ ] **Step 2: Write failing tests (cron_spec side)**

In `crates/right-agent/src/cron_spec_tests.rs`, **delete** the two tests:
- `insert_immediate_cron_uses_default_budget_when_none` (≈ :1117)
- `insert_immediate_cron_defaults_lock_ttl_to_six_hours` (≈ :1135)

Append:

```rust
#[test]
fn insert_background_continuation_writes_bg_sentinel() {
    let conn = setup_db();
    let main = Uuid::new_v4();
    insert_background_continuation(&conn, "bg-x1", "do thing", main, -100, Some(7), Some(5.0)).unwrap();
    let (schedule, recurring, run_at, chat, thread): (String, i64, Option<String>, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT schedule, recurring, run_at, target_chat_id, target_thread_id FROM cron_specs WHERE job_name = 'bg-x1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(schedule, format!("@bg:{main}"));
    assert_eq!(recurring, 0);
    assert!(run_at.is_none());
    assert_eq!(chat, Some(-100));
    assert_eq!(thread, Some(7));
}

#[test]
fn insert_background_continuation_uses_default_budget_when_none() {
    let conn = setup_db();
    insert_background_continuation(&conn, "bg-x2", "prompt", Uuid::new_v4(), -42, None, None).unwrap();
    let budget: f64 = conn
        .query_row("SELECT max_budget_usd FROM cron_specs WHERE job_name = 'bg-x2'", [], |r| r.get(0))
        .unwrap();
    assert!((budget - DEFAULT_CRON_BUDGET_USD).abs() < f64::EPSILON);
}

#[test]
fn insert_background_continuation_defaults_lock_ttl_to_six_hours() {
    let conn = setup_db();
    insert_background_continuation(&conn, "bg-x3", "prompt", Uuid::new_v4(), -42, None, None).unwrap();
    let lock_ttl: Option<String> = conn
        .query_row("SELECT lock_ttl FROM cron_specs WHERE job_name = 'bg-x3'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(lock_ttl.as_deref(), Some(IMMEDIATE_DEFAULT_LOCK_TTL));
}

#[test]
fn insert_background_continuation_round_trips_through_load() {
    let conn = setup_db();
    let main = Uuid::new_v4();
    insert_background_continuation(&conn, "bg-x4", "prompt", main, -42, None, None).unwrap();
    let specs = load_specs_from_db(&conn).unwrap();
    let spec = specs.get("bg-x4").expect("spec must load");
    match &spec.schedule_kind {
        ScheduleKind::BackgroundContinuation { fork_from } => assert_eq!(*fork_from, main),
        other => panic!("expected BackgroundContinuation, got {other:?}"),
    }
}
```

- [ ] **Step 3: Update worker test**

In `crates/bot/src/telegram/worker.rs::tests`, find `enqueue_background_job_inserts_immediate_with_target` (≈ :3070):

```bash
rg -n "enqueue_background_job_inserts" crates/bot/src/telegram/worker.rs
```

Replace its body with:

```rust
#[test]
fn enqueue_background_job_inserts_bg_kind_with_target() {
    // Use the same in-memory connection style other tests in this module use.
    // If they call `right_agent::memory::open_connection` against a tempdir,
    // do that. Otherwise use rusqlite::Connection::open_in_memory() and
    // run migrations via the same path.
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_agent::memory::open_connection(tmp.path(), true).unwrap();
    let main = uuid::Uuid::new_v4().to_string();
    let job = enqueue_background_job(&conn, -42, 7, &main, BgReason::AutoTimeout)
        .expect("enqueue ok");
    let (schedule, prompt, chat, thread): (String, String, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT schedule, prompt, target_chat_id, target_thread_id FROM cron_specs WHERE job_name = ?1",
            rusqlite::params![job],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(schedule, format!("@bg:{main}"));
    assert!(!prompt.starts_with("X-FORK-FROM:"), "X-FORK-FROM header must NOT be in prompt; got {prompt:?}");
    assert!(prompt.contains("SYSTEM_NOTICE"), "continuation notice must be in prompt body; got {prompt:?}");
    assert_eq!(chat, Some(-42));
    assert_eq!(thread, Some(7));
}
```

If the existing test name was different, keep the existing name and rename in the same edit.

- [ ] **Step 4: Update `crates/bot/tests/cron_immediate.rs`**

Replace the entire file with:

```rust
//! Integration: verify ScheduleKind::BackgroundContinuation rows are inserted
//! correctly by the bot-internal helper.

use right_agent::cron_spec::insert_background_continuation;
use right_agent::memory::open_connection;
use uuid::Uuid;

#[tokio::test]
async fn bg_continuation_row_inserted_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path();
    std::fs::create_dir_all(agent_dir.join("crons").join(".locks")).unwrap();

    let conn = open_connection(agent_dir, true).unwrap();
    let fork_from = Uuid::new_v4();
    let result = insert_background_continuation(&conn, "bg-imm-1", "do thing", fork_from, -100, Some(7), Some(5.0));
    assert!(result.is_ok(), "insert_background_continuation failed: {result:?}");

    let (schedule, recurring, run_at): (String, i64, Option<String>) = conn
        .query_row(
            "SELECT schedule, recurring, run_at FROM cron_specs WHERE job_name = 'bg-imm-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(schedule, format!("@bg:{fork_from}"));
    assert_eq!(recurring, 0);
    assert!(run_at.is_none());
}
```

- [ ] **Step 5: Run tests to verify they fail**

```bash
cargo test -p right-agent --lib insert_background_continuation
cargo test -p right-bot --lib enqueue_background_job_inserts_bg_kind
cargo test -p right-bot --test cron_immediate
```

Expected: build errors — function and references don't exist yet.

- [ ] **Step 6: Implement `insert_background_continuation`; delete `insert_immediate_cron`**

In `crates/right-agent/src/cron_spec.rs`, **delete** `insert_immediate_cron` (lines 348-379) entirely.

Add new function in its place:

```rust
/// Insert a one-shot `BackgroundContinuation` cron job. Bot-internal use only —
/// produced by the worker when foreground turns are sent to background, never
/// constructible from CLI/MCP.
///
/// Stores `schedule = '@bg:<fork_from>'`, `recurring = 0`, `run_at = NULL`,
/// `lock_ttl = IMMEDIATE_DEFAULT_LOCK_TTL` (`"6h"`).
/// `max_budget_usd = None` uses [`DEFAULT_CRON_BUDGET_USD`].
pub fn insert_background_continuation(
    conn: &rusqlite::Connection,
    job_name: &str,
    prompt: &str,
    fork_from: Uuid,
    target_chat_id: i64,
    target_thread_id: Option<i64>,
    max_budget_usd: Option<f64>,
) -> Result<CronSpecResult, String> {
    validate_job_name(job_name)?;
    if prompt.trim().is_empty() {
        return Err("prompt must not be empty".into());
    }
    if let Some(budget) = max_budget_usd
        && budget <= 0.0
    {
        return Err("max_budget_usd must be greater than 0".into());
    }

    let now = chrono::Utc::now().to_rfc3339();
    let budget = max_budget_usd.unwrap_or(DEFAULT_CRON_BUDGET_USD);
    let schedule = format!("{BG_SENTINEL_PREFIX}{fork_from}");
    let lock_ttl = IMMEDIATE_DEFAULT_LOCK_TTL;

    let result = conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL, ?6, ?7, ?8, ?9)",
        rusqlite::params![job_name, schedule, prompt, lock_ttl, budget, target_chat_id, target_thread_id, now, now],
    );

    match result {
        Ok(_) => Ok(CronSpecResult {
            message: format!("Created bg-continuation job '{job_name}'."),
            warning: None,
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

- [ ] **Step 7: Rewrite `enqueue_background_job` in worker.rs**

Replace `crates/bot/src/telegram/worker.rs:486-508` with:

```rust
/// Enqueue a one-shot `BackgroundContinuation` cron job that will fork from
/// `main_session_id` and continue the interrupted turn. Job name is
/// `bg-<HHMMSS>-<8hex>` — timestamped for human scanning, uuid-suffixed for
/// collision-free PK insert. The `fork_from` UUID is carried structurally in
/// the schedule kind, NOT as a header in the prompt body.
fn enqueue_background_job(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
    main_session_id: &str,
    reason: BgReason,
) -> Result<String, String> {
    const JOB_SUFFIX_HEX_CHARS: usize = 8;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let job_name = format!(
        "bg-{}-{}",
        chrono::Utc::now().format("%H%M%S"),
        &suffix[..JOB_SUFFIX_HEX_CHARS]
    );
    let prompt = build_continuation_prompt(reason);
    let fork_from = uuid::Uuid::parse_str(main_session_id)
        .map_err(|e| format!("main_session_id '{main_session_id}' is not a UUID: {e:#}"))?;
    let target_thread = if thread_id == 0 { None } else { Some(thread_id) };
    right_agent::cron_spec::insert_background_continuation(
        conn,
        &job_name,
        &prompt,
        fork_from,
        chat_id,
        target_thread,
        None,
    )?;
    Ok(job_name)
}
```

- [ ] **Step 8: Run all tests**

```bash
cargo test -p right-agent --lib cron_spec
cargo test -p right-bot --lib worker
cargo test -p right-bot --test cron_immediate
cargo build --workspace
```

Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs \
        crates/right-agent/src/cron_spec_tests.rs \
        crates/bot/src/telegram/worker.rs \
        crates/bot/tests/cron_immediate.rs
git commit -m "feat(worker): produce BackgroundContinuation rows; drop X-FORK-FROM prefix"
```

---

## Task 8: Continuation prompt forbids silence

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:465-481` (`build_continuation_prompt`).
- Modify: `crates/bot/src/telegram/worker.rs::tests` (append).

- [ ] **Step 1: Add failing test**

Append to the worker tests module (next to existing `build_continuation_prompt_*` tests near :3055):

```rust
#[test]
fn build_continuation_prompt_forbids_silence() {
    let p = build_continuation_prompt(BgReason::AutoTimeout);
    assert!(
        p.contains("Silence is not a valid outcome"),
        "must explicitly forbid silent output; got {p:?}"
    );
    let q = build_continuation_prompt(BgReason::UserRequested);
    assert!(q.contains("Silence is not a valid outcome"));
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p right-bot --lib build_continuation_prompt_forbids
```

Expected: fail.

- [ ] **Step 3: Update the prompt builder**

Replace `crates/bot/src/telegram/worker.rs:465-481`:

```rust
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
\n\
You MUST produce a non-empty notify.content. Silence is not a valid outcome\n\
for this turn — the user is waiting for an answer.\n\
\u{27e8}\u{27e8}/SYSTEM_NOTICE\u{27e9}\u{27e9}"
    )
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p right-bot --lib build_continuation_prompt
```

Expected: 3 green (`auto_timeout`, `user_requested`, `forbids_silence`).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(worker): instruct bg fork that silence is not a valid outcome"
```

---

## Task 9: Startup migration for legacy `@immediate` + `X-FORK-FROM` rows

**Files:**
- Modify: `crates/bot/src/cron.rs` (add `migrate_legacy_bg_continuation`).
- Modify: `crates/bot/src/cron.rs::tests` (append).
- Modify: `crates/bot/src/lib.rs` (call migration on bot startup).

- [ ] **Step 1: Write failing tests**

Append to `crates/bot/src/cron.rs::tests`:

```rust
#[test]
fn migrate_legacy_bg_rewrites_at_immediate_with_x_fork_from() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_agent::memory::open_connection(tmp.path(), true).unwrap();
    let main = uuid::Uuid::new_v4();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
         VALUES ('bg-old', '@immediate', ?1, '6h', 5.0, 0, NULL, -100, NULL, ?2, ?2)",
        rusqlite::params![format!("X-FORK-FROM: {main}\nbody continues here"), now],
    ).unwrap();

    let migrated = migrate_legacy_bg_continuation(&conn).unwrap();
    assert_eq!(migrated, 1);

    let (schedule, prompt): (String, String) = conn
        .query_row(
            "SELECT schedule, prompt FROM cron_specs WHERE job_name = 'bg-old'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(schedule, format!("@bg:{main}"));
    assert_eq!(prompt, "body continues here");
}

#[test]
fn migrate_legacy_bg_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_agent::memory::open_connection(tmp.path(), true).unwrap();
    let main = uuid::Uuid::new_v4();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
         VALUES ('bg-old', '@immediate', ?1, '6h', 5.0, 0, NULL, -100, NULL, ?2, ?2)",
        rusqlite::params![format!("X-FORK-FROM: {main}\nbody"), now],
    ).unwrap();

    let first = migrate_legacy_bg_continuation(&conn).unwrap();
    let second = migrate_legacy_bg_continuation(&conn).unwrap();
    assert_eq!(first, 1);
    assert_eq!(second, 0, "second pass must migrate zero rows");
}

#[test]
fn migrate_legacy_bg_skips_invalid_uuid() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_agent::memory::open_connection(tmp.path(), true).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
         VALUES ('bg-bad', '@immediate', 'X-FORK-FROM: not-a-uuid\nbody', '6h', 5.0, 0, NULL, -100, NULL, ?1, ?1)",
        rusqlite::params![now],
    ).unwrap();

    let migrated = migrate_legacy_bg_continuation(&conn).unwrap();
    assert_eq!(migrated, 0);

    let schedule: String = conn
        .query_row("SELECT schedule FROM cron_specs WHERE job_name = 'bg-bad'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(schedule, "@immediate", "row with invalid UUID must be untouched");
}

#[test]
fn migrate_legacy_bg_skips_immediate_without_header() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_agent::memory::open_connection(tmp.path(), true).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, run_at, target_chat_id, target_thread_id, created_at, updated_at) \
         VALUES ('plain-imm', '@immediate', 'just a prompt', '6h', 5.0, 0, NULL, -100, NULL, ?1, ?1)",
        rusqlite::params![now],
    ).unwrap();

    let migrated = migrate_legacy_bg_continuation(&conn).unwrap();
    assert_eq!(migrated, 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p right-bot --lib migrate_legacy_bg
```

Expected: build error — function does not exist.

- [ ] **Step 3: Implement the migration**

Add to `crates/bot/src/cron.rs` as a top-level public fn (near other top-level helpers):

```rust
/// One-time startup migration: rewrite legacy `@immediate` + `X-FORK-FROM:`
/// rows produced by the old bg-continuation convention into the new
/// `@bg:<uuid>` sentinel + clean prompt body. Idempotent — rows already in
/// the new form are filtered out by `WHERE schedule = '@immediate'`.
/// Invalid UUIDs in the legacy header leave the row untouched (logged at
/// WARN). Returns the number of rows rewritten.
pub fn migrate_legacy_bg_continuation(
    conn: &rusqlite::Connection,
) -> Result<usize, rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    let mut migrated = 0usize;
    {
        let mut stmt = tx.prepare(
            "SELECT job_name, prompt FROM cron_specs WHERE schedule = '@immediate'",
        )?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(Result::ok)
            .collect();
        for (name, prompt) in rows {
            let Some(rest) = prompt.strip_prefix("X-FORK-FROM: ") else {
                continue;
            };
            let Some((sess, body)) = rest.split_once('\n') else {
                continue;
            };
            let Ok(fork_from) = uuid::Uuid::parse_str(sess) else {
                tracing::warn!(job = %name, "legacy @immediate row has invalid UUID in X-FORK-FROM; skipping");
                continue;
            };
            tx.execute(
                "UPDATE cron_specs SET schedule = ?1, prompt = ?2 WHERE job_name = ?3",
                rusqlite::params![format!("@bg:{fork_from}"), body, name],
            )?;
            migrated += 1;
        }
    }
    tx.commit()?;
    Ok(migrated)
}
```

- [ ] **Step 4: Wire migration into bot startup**

Find the per-agent startup hook in `crates/bot/src/lib.rs`:

```bash
rg -n "open_connection.*true|migrate.*startup|run_bot|run_single_agent|fn run_bot" crates/bot/src/lib.rs | head
```

Look for the place where `right_agent::memory::open_connection(...)` is called with `migrate=true` (per-agent DB). Just after that connection is opened (and migrations have run) and before the cron reconcile loop starts, add:

```rust
match crate::cron::migrate_legacy_bg_continuation(&conn) {
    Ok(0) => {}
    Ok(n) => tracing::info!(agent = %agent_name, "migrated {n} legacy bg-continuation rows"),
    Err(e) => tracing::error!(agent = %agent_name, "legacy bg-continuation migration failed: {e:#}"),
}
```

If the surrounding scope uses a different name for `agent_name` (e.g., `name`, `cfg.name`), adapt. If `conn` isn't kept after the migration, the migration may need its own short-lived connection — open another via `right_agent::memory::open_connection(&agent_dir, false)`. Inspect the existing pattern first.

- [ ] **Step 5: Run tests and build**

```bash
cargo test -p right-bot --lib migrate_legacy_bg
cargo build --workspace
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/src/lib.rs
git commit -m "feat(cron): startup migration for legacy @immediate+X-FORK-FROM rows"
```

---

## Task 10: Extend `deploy_composite_memory` signature with `bg_marker`

**Files:**
- Modify: `crates/bot/src/telegram/prompt.rs:132-155`.
- Modify: `crates/bot/src/telegram/worker.rs:1687-1717` (caller; add stub `build_bg_marker_for_chat`).

- [ ] **Step 1: Update the signature in prompt.rs**

Replace `crates/bot/src/telegram/prompt.rs:132-155`:

```rust
pub(crate) async fn deploy_composite_memory(
    content: &str,
    label: &str,
    agent_dir: &std::path::Path,
    resolved_sandbox: Option<&str>,
    status_marker: Option<&str>,
    bg_marker: Option<&str>,
) -> Result<(), DeployError> {
    let status_tail = status_marker
        .map(|m| format!("\n\n{m}"))
        .unwrap_or_default();
    let bg_tail = bg_marker.map(|m| format!("\n\n{m}")).unwrap_or_default();
    let fenced = format!(
        "<memory-context>\n[System: recalled memory context, {label}.]\n\n{content}\n</memory-context>{status_tail}{bg_tail}"
    );
    let host_path = agent_dir.join(".claude").join("composite-memory.md");
    tokio::fs::write(&host_path, &fenced)
        .await
        .map_err(DeployError::Write)?;
    if let Some(sandbox) = resolved_sandbox {
        right_agent::openshell::upload_file(sandbox, &host_path, "/sandbox/.claude/")
            .await
            .map_err(|e| DeployError::Upload(format!("{e:#}")))?;
    }
    Ok(())
}
```

- [ ] **Step 2: Update the caller in worker.rs**

In `crates/bot/src/telegram/worker.rs:1686-1717`:

Add a placeholder for the bg-marker builder at the end of the file (full impl in Task 11):

```rust
/// Placeholder — full implementation in Task 11.
fn build_bg_marker_for_chat(_agent_dir: &std::path::Path, _chat_id: i64) -> Option<String> {
    None
}
```

Change the block starting at `let marker = build_memory_marker(...)`:

Old:

```rust
        let marker = build_memory_marker(wrapper_status, client_drops_24h);
        match (recall_content.as_deref(), marker.as_deref()) {
            (None, None) => {
                // ... remove_composite_memory branch ...
            }
            (content, marker_str) => {
                // ... deploy_composite_memory call ...
            }
        }
```

New:

```rust
        let marker = build_memory_marker(wrapper_status, client_drops_24h);
        let bg_marker = build_bg_marker_for_chat(&ctx.agent_dir, chat_id);
        match (recall_content.as_deref(), marker.as_deref(), bg_marker.as_deref()) {
            (None, None, None) => {
                // existing remove_composite_memory branch — unchanged body
            }
            (content, marker_str, bg_marker_str) => {
                let body = content.unwrap_or("");
                if let Err(e) = super::prompt::deploy_composite_memory(
                    body,
                    "NOT new user input. Treat as background",
                    &ctx.agent_dir,
                    ctx.resolved_sandbox.as_deref(),
                    marker_str,
                    bg_marker_str,
                )
                .await
                {
                    tracing::warn!("composite-memory deploy failed: {e:#}");
                }
            }
        }
```

Keep the exact body of the `(None, None, None)` branch identical to the existing `(None, None)` branch.

- [ ] **Step 3: Build and test**

```bash
cargo build --workspace
cargo test -p right-bot --lib
```

Expected: green (no behavioural change yet — bg_marker is always `None`).

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/prompt.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(prompt): add bg_marker slot to deploy_composite_memory; stub builder"
```

---

## Task 11: Implement `build_bg_marker_for_chat`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (replace stub with full impl).
- Modify: `crates/bot/src/telegram/worker.rs::tests` (append).

- [ ] **Step 1: Write failing tests**

Append to the worker tests module:

```rust
#[test]
fn build_bg_marker_returns_none_when_no_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let _conn = right_agent::memory::open_connection(tmp.path(), true).unwrap();
    let m = build_bg_marker_for_chat(tmp.path(), -100);
    assert!(m.is_none(), "no rows → no marker; got {m:?}");
}

#[test]
fn build_bg_marker_includes_running_run_for_chat() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_agent::memory::open_connection(tmp.path(), true).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, target_chat_id, target_thread_id) \
         VALUES ('run-A', 'bg-job-A', ?1, 'running', '/log', -100, NULL)",
        rusqlite::params![now],
    ).unwrap();
    drop(conn);
    let m = build_bg_marker_for_chat(tmp.path(), -100).expect("marker present");
    assert!(m.starts_with("<background-jobs>"), "got {m:?}");
    assert!(m.contains("bg-job-A"));
    assert!(m.contains("run-A"));
    assert!(m.contains("running"));
}

#[test]
fn build_bg_marker_includes_undelivered_success_run() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_agent::memory::open_connection(tmp.path(), true).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, target_chat_id, target_thread_id, delivery_status) \
         VALUES ('run-B', 'bg-job-B', ?1, ?1, 'success', '/log', -100, NULL, 'pending')",
        rusqlite::params![now],
    ).unwrap();
    drop(conn);
    let m = build_bg_marker_for_chat(tmp.path(), -100).expect("marker present");
    assert!(m.contains("bg-job-B"));
    assert!(m.contains("success"));
}

#[test]
fn build_bg_marker_excludes_other_chat() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_agent::memory::open_connection(tmp.path(), true).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, target_chat_id, target_thread_id) \
         VALUES ('run-other', 'bg-other', ?1, 'running', '/log', -999, NULL)",
        rusqlite::params![now],
    ).unwrap();
    drop(conn);
    let m = build_bg_marker_for_chat(tmp.path(), -100);
    assert!(m.is_none(), "row for other chat must not appear; got {m:?}");
}

#[test]
fn build_bg_marker_excludes_delivered_run() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_agent::memory::open_connection(tmp.path(), true).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, target_chat_id, target_thread_id, delivered_at, delivery_status) \
         VALUES ('run-D', 'bg-D', ?1, ?1, 'success', '/log', -100, NULL, ?1, 'delivered')",
        rusqlite::params![now],
    ).unwrap();
    drop(conn);
    let m = build_bg_marker_for_chat(tmp.path(), -100);
    assert!(m.is_none(), "delivered run must not appear; got {m:?}");
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p right-bot --lib build_bg_marker
```

Expected: 4 of 5 tests fail (the empty-no-runs case passes by accident — the stub returns None).

- [ ] **Step 3: Replace the stub**

Replace `build_bg_marker_for_chat` at the bottom of `worker.rs` with:

```rust
/// Build the `<background-jobs>` marker tail for `composite-memory.md`.
///
/// Surfaces in-flight bg/cron runs targeted at this chat so the foreground
/// agent is aware of work pending in the background. Two states qualify:
/// - `status = 'running'` — job currently executing.
/// - `status = 'success' AND delivered_at IS NULL` — job finished, answer
///   queued for delivery (held by `IDLE_THRESHOLD_SECS` until the chat
///   goes idle).
///
/// Returns `None` if the DB cannot be opened or no rows match. Errors do
/// not propagate (best-effort marker — failure leaves composite-memory
/// without the marker rather than blocking the turn).
fn build_bg_marker_for_chat(agent_dir: &std::path::Path, target_chat_id: i64) -> Option<String> {
    let conn = right_agent::memory::open_connection(agent_dir, false).ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, job_name, started_at, status \
             FROM cron_runs \
             WHERE target_chat_id = ?1 \
               AND ((status = 'running') OR (status = 'success' AND delivered_at IS NULL)) \
             ORDER BY started_at",
        )
        .ok()?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([target_chat_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .ok()?
        .filter_map(Result::ok)
        .collect();
    if rows.is_empty() {
        return None;
    }
    let body = rows
        .iter()
        .map(|(id, name, ts, st)| format!("{name} (run {id}) — started {ts}, {st}"))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("<background-jobs>\n{body}\n</background-jobs>"))
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p right-bot --lib build_bg_marker
```

Expected: all 5 green.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(worker): build_bg_marker_for_chat surfaces in-flight bg runs to main session"
```

---

## Task 12: ARCHITECTURE.md and docs/architecture/sessions.md updates

**Files:**
- Modify: `ARCHITECTURE.md` (replace X-FORK-FROM section).
- Modify: `docs/architecture/sessions.md` (update if it carries the same wording).

- [ ] **Step 1: Locate the section in ARCHITECTURE.md**

```bash
rg -n "X-FORK-FROM|background continuation:" ARCHITECTURE.md
```

Identify the section heading `### Background continuation: X-FORK-FROM convention`.

- [ ] **Step 2: Replace it**

Replace the section (heading + body, ~15 lines) with:

```markdown
### Background continuation: `BackgroundContinuation` schedule kind

A background-continuation cron job is identified by its
`ScheduleKind::BackgroundContinuation { fork_from: Uuid }` variant,
encoded in the `cron_specs.schedule` column as `@bg:<fork_from-uuid>`.
`cron::execute_job` matches the variant via `select_schema_and_fork`
and emits a CC invocation with `--resume <fork_from> --fork-session
--session-id <run_id>`. The forked session inherits the main session's
full history; the prompt body is a short SYSTEM_NOTICE
(`build_continuation_prompt`) asking the agent to finish answering
the user's most recent message.

Bot-internal: only `worker::enqueue_background_job` constructs this
variant via `cron_spec::insert_background_continuation`. Agents
cannot hijack `--resume` because the variant carries `fork_from` as
typed data, and the `cron_create` MCP path never sets it.
`select_schema_and_fork` co-derives the JSON schema
(`BG_CONTINUATION_SCHEMA_JSON`, which forbids silent output) and the
`fork_from` source from the same variant — drift between the two
effects is impossible by construction.

A one-time startup migration `cron::migrate_legacy_bg_continuation`
rewrites pre-existing rows that used the deprecated
`@immediate` + `X-FORK-FROM:` convention into the new form.
```

- [ ] **Step 3: Update sessions.md if needed**

```bash
rg -n "X-FORK-FROM|background continuation" docs/architecture/sessions.md
```

If matches exist, mirror the same prose adapted to that file's tone.

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md docs/architecture/sessions.md
git commit -m "docs(arch): document BackgroundContinuation schedule kind"
```

---

## Task 13: Workspace build, clippy, and final test

**Files:**
- (none — verification only)

- [ ] **Step 1: Full workspace build**

```bash
cargo build --workspace
```

Expected: green.

- [ ] **Step 2: Clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: zero warnings. If new warnings surface from the new code, fix and re-run.

- [ ] **Step 3: Full test suite**

```bash
cargo test --workspace
```

Expected: all green. Integration tests that need OpenShell create real sandboxes — make sure OpenShell is running locally (per project convention, no `#[ignore]` on these tests).

- [ ] **Step 4: Manual smoke test (recommended)**

If a dev-mode bot is running:

1. Restart the agent: `right restart <agent>`.
2. Watch `~/.right/logs/<agent>.log` on startup for `migrated N legacy bg-continuation rows` (zero is fine — there were no legacy rows).
3. Trigger a background turn: send a message that hits the 600s timeout, OR press `🌙 Background` mid-flight on the working keyboard.
4. Watch the log for:
   - `firing one-shot job ... kind="immediate-or-bg"`
   - `cron job completed ... status=success`
   - `cron output persisted to DB ... has_notify=true delivery_status="pending"`
5. Wait for `IDLE_THRESHOLD_SECS` (3 min) of chat idle. Expect a Telegram delivery.
6. While the bg job is running, send a new foreground message. Read `composite-memory.md`:
   - Sandboxed: `ssh ... cat /sandbox/.claude/composite-memory.md` via the agent's ssh-config.
   - Non-sandbox: `cat ~/.right/agents/<agent>/.claude/composite-memory.md`.
   Confirm the `<background-jobs>` block is present and lists the bg job by name + run id.

- [ ] **Step 5: Cleanup commit if anything was tweaked**

```bash
git status --short
```

If clean, skip. Otherwise:

```bash
git commit -am "chore: clippy / formatting fixes"
```

---

## Self-Review

Before marking the implementation complete, walk through the spec one more time:

1. **Spec coverage map:**
   - Defect 1 (silent bg drops answer) → Tasks 3, 4, 5 (schema + dispatch + helper).
   - Defect 2 (main session unaware) → Tasks 10, 11 (signature + builder + wiring).
   - Defect 3 (string-magic kind) → Tasks 1, 2, 5, 7 (refactor + variant + dispatch + worker rewrite).
   - Drop X-FORK-FROM convention → Tasks 5, 7 (cron.rs + worker.rs).
   - Forbid-silence prompt line → Task 8.
   - Startup migration → Task 9.
   - Reconcile match-arm extension → Task 6.
   - Docs update → Task 12.

2. **Type & name consistency:**
   - `ScheduleKind::BackgroundContinuation { fork_from: Uuid }` — same field name everywhere.
   - `select_schema_and_fork(&CronSpec) -> (&'static str, Option<String>)` — same signature in test and impl.
   - `build_bg_marker_for_chat(&Path, i64) -> Option<String>` — same in stub (Task 10) and impl (Task 11).
   - `migrate_legacy_bg_continuation(&Connection) -> Result<usize, rusqlite::Error>` — same in tests, impl, caller.
   - `insert_background_continuation(conn, job_name, prompt, fork_from, target_chat_id, target_thread_id, max_budget_usd)` — order matches between tests, impl, and worker caller.
   - `deploy_composite_memory` gains `bg_marker: Option<&str>` as the 6th positional arg — caller passes it last.

3. **Intermediate-state consistency:** Each task leaves the system buildable AND behaviourally consistent end-to-end:
   - Tasks 1-4 add types/constants/helpers — no production behaviour change.
   - Task 5 makes `execute_job` kind-aware — but no `BackgroundContinuation` rows exist yet.
   - Task 6 makes the reconciler fire bg rows — but no rows exist yet.
   - Task 7 starts producing bg rows — at this point reconcile, dispatch, and prompt all match.
   - Tasks 8-12 are independent enrichments.

4. **No `#[ignore]` added** (CLAUDE.md convention; integration tests run against live OpenShell).

5. **No silent error handling.** `enqueue_background_job` propagates UUID parse errors, `migrate_legacy_bg_continuation` propagates SQLite errors, `build_bg_marker_for_chat` returns `None` on best-effort failures (acceptable — marker is non-critical, failure leaves composite-memory without the marker).
