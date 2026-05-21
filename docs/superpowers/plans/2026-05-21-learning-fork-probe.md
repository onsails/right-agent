# Learning Fork-Probe Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Stage 2 background learning pipeline (episode selector + reviewer + drain scheduler) with a synchronous fork-probe fired after each foreground user-turn, while keeping the legacy code behind an opt-in flag.

**Architecture:** A new `bot::learning_probe` module spawns `claude -p --resume <main> --fork-session --tools "" --model <main>` as a fire-and-forget task after each foreground Telegram reply. The probe emits JSON matching a `FORK_PROBE_SCHEMA`; non-null `learning_signal` / `skill_issue_signal` is persisted to `skill_nudge_signals` with `source = 'fork_probe'`. The legacy `DrainScheduler` + `learning_episode.rs` + `learning_review.rs` are gated behind `learning.background_review_enabled` (default `false`).

**Tech Stack:** Rust 2024, Tokio, rusqlite + rusqlite_migration, teloxide, serde, chrono. Edits across crates: `right-db`, `right-agent`, `right-agent-config`, `right-codegen`, `right-dashboard`, `bot`, `right`.

**Spec:** `docs/superpowers/specs/2026-05-21-learning-fork-probe-design.md`

**Verification cadence:** Per `AGENTS.md` — targeted package tests during dev, one `devenv shell -- cargo test --workspace` at the end. Per `AGENTS.rust.md` — `thiserror` in libs, `anyhow` in bin/tests, FAIL-FAST error propagation (`?` everywhere), `format!("{:#}", e)` when stringifying `anyhow::Error`.

---

## File Structure

| File | New / Modified | Responsibility |
|---|---|---|
| `crates/right-db/src/sql/v27_skill_nudge_signals_source.sql` | New | Doc-only SQL describing v27 ALTER. Implementation lives in Rust hook for idempotency. |
| `crates/right-db/src/migrations.rs` | Modified | Register v27 Rust hook `v27_skill_nudge_signals_source`; bump `LATEST_SCHEMA_VERSION` to 27. |
| `crates/right-agent/src/usage/mod.rs` | Modified | Extend `LEARNING_SOURCES` with `"learning_fork_probe"`. |
| `crates/right-agent/src/usage/insert.rs` | Modified | Add `insert_learning_fork_probe` helper. |
| `crates/right-agent/src/learned_skills.rs` | Modified | Add `NudgeSignalSource` enum; extend `NudgeSignalRecord` with `source` field; extend `record_nudge_signal` to persist it. |
| `crates/right-agent-config/src/lib.rs` | Modified | Add `probe_model`, `fork_probe_enabled`, `background_review_enabled` fields to `LearningConfig` with backward-compatible defaults. |
| `crates/right-codegen/src/agent_def.rs` | Modified | Add `FORK_PROBE_SCHEMA_JSON` + `FORK_PROBE_PROMPT` constants. |
| `crates/bot/src/learning_probe.rs` | New | Probe logic: `should_run_probe`, `build_probe_invocation`, `parse_probe_output`, async `spawn_probe`. |
| `crates/bot/src/lib.rs` | Modified | `pub(crate) mod learning_probe;` |
| `crates/bot/src/telegram/worker.rs` | Modified | Pass `NudgeSignalSource::ReplyField` at existing insert site; after Telegram send for foreground turn, spawn `learning_probe::spawn_probe`. |
| `crates/bot/src/telegram/dispatch.rs` | Modified | Gate `DrainScheduler::spawn` on `agent.learning.background_review_enabled`. |
| `crates/bot/src/cron.rs` | Modified | Same gating where `DrainScheduler` is wired. |
| `crates/bot/src/lib.rs` (bot crate) | Modified | At bot startup, scan for legacy `learning_episodes` activity and WARN if `background_review_enabled = false`. |
| `crates/right-dashboard/src/api_types.rs` | Modified | Add `SignalsBySourceResponse` + `SignalsSourceBucket`. |
| `crates/right-dashboard/src/read_model/learning.rs` | Modified | Add `signals_by_source_24h` read-model fn. |
| `crates/right/src/wizard.rs` | Modified | Wizard prompts for `probe_model`, `fork_probe_enabled`, `background_review_enabled`. |

---

## Task 1: v27 migration — add `source` column to `skill_nudge_signals`

**Files:**
- Create: `crates/right-db/src/sql/v27_skill_nudge_signals_source.sql`
- Modify: `crates/right-db/src/migrations.rs`

- [ ] **Step 1.1: Write the failing migration test**

Append to `crates/right-db/src/migrations.rs` `mod tests` block (search for any existing `skill_review_reports_migration_creates_report_table` to follow style):

```rust
#[test]
fn v27_adds_source_column_to_skill_nudge_signals() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let has_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_signals') WHERE name = ?1",
            ["source"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_column, 1, "source column must exist");
    let not_null: i64 = conn
        .query_row(
            "SELECT \"notnull\" FROM pragma_table_info('skill_nudge_signals') WHERE name = ?1",
            ["source"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(not_null, 1, "source column must be NOT NULL");
}

#[test]
fn v27_is_idempotent_on_databases_already_at_v27() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    // Re-run by calling the migration registry again should not error.
    MIGRATIONS.to_latest(&mut conn).unwrap();
}

#[test]
fn v27_index_on_source_column_exists() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='index' AND name='idx_skill_nudge_signals_source'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists, 1, "idx_skill_nudge_signals_source must exist");
}
```

- [ ] **Step 1.2: Run test to verify it fails**

```bash
devenv shell -- cargo test -p right-db v27_
```

Expected: 3 tests FAIL — `v27_adds_source_column_to_skill_nudge_signals`, `v27_is_idempotent_on_databases_already_at_v27`, `v27_index_on_source_column_exists`. Failure reason: column / index do not exist; migration not registered.

- [ ] **Step 1.3: Create doc-only SQL file**

Create `crates/right-db/src/sql/v27_skill_nudge_signals_source.sql`:

```sql
-- v27: Track the origin of each accepted learning signal.
--
-- Source values:
--   'reply_field' — agent emitted the signal in its structured reply.
--   'fork_probe'  — post-turn fork-classifier identified the signal.
--
-- Implementation lives in Rust hook `v27_skill_nudge_signals_source`
-- because SQLite lacks `ADD COLUMN IF NOT EXISTS`. This file is doc-only.

ALTER TABLE skill_nudge_signals
  ADD COLUMN source TEXT NOT NULL DEFAULT 'reply_field';

CREATE INDEX IF NOT EXISTS idx_skill_nudge_signals_source
  ON skill_nudge_signals(source);
```

- [ ] **Step 1.4: Add the Rust migration hook and register it**

In `crates/right-db/src/migrations.rs`:

Add the const near other `#[allow(dead_code)]` constants (around line 32):

```rust
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V27_SCHEMA: &str = include_str!("sql/v27_skill_nudge_signals_source.sql");
```

Bump `LATEST_SCHEMA_VERSION`:

```rust
pub const LATEST_SCHEMA_VERSION: u32 = 27;
```

Add the hook function next to `v26_skill_nudge_circuit_breaker`:

```rust
/// v27: Add `source` column + index to `skill_nudge_signals`.
///
/// Idempotent — checks pragma_table_info before ALTER. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`. The CREATE INDEX uses `IF NOT EXISTS`.
fn v27_skill_nudge_signals_source(tx: &Transaction) -> Result<(), HookError> {
    let has_column: i64 = tx.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_signals') WHERE name = ?1",
        ["source"],
        |r| r.get(0),
    )?;
    if has_column == 0 {
        tx.execute_batch(
            "ALTER TABLE skill_nudge_signals ADD COLUMN source TEXT NOT NULL DEFAULT 'reply_field'",
        )?;
    }
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_skill_nudge_signals_source \
         ON skill_nudge_signals(source)",
    )?;
    Ok(())
}
```

Register inside the `MIGRATIONS` vec (after the v26 entry):

```rust
M::up_with_hook("", v27_skill_nudge_signals_source),
```

- [ ] **Step 1.5: Run tests to verify they pass**

```bash
devenv shell -- cargo test -p right-db v27_
```

Expected: 3 tests PASS.

- [ ] **Step 1.6: Commit**

```bash
git add crates/right-db/src/sql/v27_skill_nudge_signals_source.sql \
        crates/right-db/src/migrations.rs
git commit -m "feat(db): add source column to skill_nudge_signals (v27 migration)"
```

---

## Task 2: Extend `LEARNING_SOURCES` + add fork-probe usage insert helper

**Files:**
- Modify: `crates/right-agent/src/usage/mod.rs`
- Modify: `crates/right-agent/src/usage/insert.rs`

- [ ] **Step 2.1: Write failing tests**

In `crates/right-agent/src/usage/mod.rs`, replace the existing `learning_sources_contains_expected_three_entries` test with:

```rust
#[test]
fn learning_sources_contains_expected_four_entries() {
    assert_eq!(
        LEARNING_SOURCES,
        &[
            "learning_selector",
            "learning_reviewer",
            "learning_skill_review",
            "learning_fork_probe",
        ]
    );
}
```

In `crates/right-agent/src/usage/insert.rs`, append to `#[cfg(test)] mod tests`:

```rust
#[test]
fn insert_learning_fork_probe_writes_row_with_correct_source() {
    let conn = test_conn();
    insert_learning_fork_probe(&conn, &sample_breakdown(), 1234, 0).unwrap();
    let source: String = conn
        .query_row(
            "SELECT source FROM usage_events WHERE session_uuid = ?1",
            ["test-session"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(source, "learning_fork_probe");
    let chat_id: i64 = conn
        .query_row(
            "SELECT chat_id FROM usage_events WHERE session_uuid = ?1",
            ["test-session"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(chat_id, 1234);
}
```

(The `test_conn()` and `sample_breakdown()` helpers already exist in the file; verify their names before relying on them.)

- [ ] **Step 2.2: Run tests to verify they fail**

```bash
devenv shell -- cargo test -p right-agent learning_sources_ insert_learning_fork_probe_
```

Expected: 2 tests FAIL — both with compile or assertion errors.

- [ ] **Step 2.3: Extend `LEARNING_SOURCES`**

In `crates/right-agent/src/usage/mod.rs`:

```rust
pub const LEARNING_SOURCES: &[&str] = &[
    "learning_selector",
    "learning_reviewer",
    "learning_skill_review",
    "learning_fork_probe",
];
```

- [ ] **Step 2.4: Add the insert helper**

In `crates/right-agent/src/usage/insert.rs`, add (after `insert_learning_skill_review`):

```rust
/// Insert a row for a fork-probe invocation (post-turn classifier).
///
/// `chat_id` and `thread_id` carry the originating foreground turn so the
/// dashboard can group probe spend by chat.
pub fn insert_learning_fork_probe(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(
        conn,
        b,
        "learning_fork_probe",
        Some(chat_id),
        Some(thread_id),
        None,
    )
}
```

- [ ] **Step 2.5: Run tests to verify they pass**

```bash
devenv shell -- cargo test -p right-agent learning_sources_ insert_learning_fork_probe_
```

Expected: 2 tests PASS.

- [ ] **Step 2.6: Check cross-crate consistency test (right-dashboard)**

```bash
devenv shell -- cargo test -p right-dashboard usage_overview_sources_match_learning_sources_constant
```

Expected: test PASSES because the dashboard `SOURCES` array picks up `learning_fork_probe` automatically via the iter chain (verify this in `crates/right-dashboard/src/read_model/usage.rs:9-16` — if `SOURCES` literals do not iterate `LEARNING_SOURCES`, that callsite needs the union update too).

If `SOURCES` is a hard-coded `[&str; 6]`, expand it to `[&str; 7]` and add `"learning_fork_probe"`:

```rust
const SOURCES: [&str; 7] = [
    "interactive",
    "cron",
    "reflection",
    "learning_selector",
    "learning_reviewer",
    "learning_skill_review",
    "learning_fork_probe",
];
```

Rerun the cross-crate test.

- [ ] **Step 2.7: Commit**

```bash
git add crates/right-agent/src/usage/mod.rs \
        crates/right-agent/src/usage/insert.rs \
        crates/right-dashboard/src/read_model/usage.rs
git commit -m "feat(usage): add learning_fork_probe source + insert helper"
```

---

## Task 3: Add `NudgeSignalSource` enum and extend `record_nudge_signal`

**Files:**
- Modify: `crates/right-agent/src/learned_skills.rs`

- [ ] **Step 3.1: Write failing tests**

Append to `crates/right-agent/src/learned_skills.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn nudge_signal_source_as_str_returns_kebab_case_literals() {
    assert_eq!(NudgeSignalSource::ReplyField.as_str(), "reply_field");
    assert_eq!(NudgeSignalSource::ForkProbe.as_str(), "fork_probe");
}

#[test]
fn record_nudge_signal_persists_source_reply_field() {
    let conn = open_test_conn();
    record_nudge_signal(
        &conn,
        &NudgeSignalRecord {
            invocation_id: "inv-reply".to_owned(),
            agent_name: "right".to_owned(),
            root_session_id: Some("s-1".to_owned()),
            chat_id: Some(10),
            thread_id: Some(0),
            signal_kind: NudgeSignalKind::Learning,
            payload_json: serde_json::json!({"kind":"create_candidate"}),
            source: NudgeSignalSource::ReplyField,
        },
    )
    .unwrap();
    let source: String = conn
        .query_row(
            "SELECT source FROM skill_nudge_signals WHERE invocation_id=?1",
            ["inv-reply"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(source, "reply_field");
}

#[test]
fn record_nudge_signal_persists_source_fork_probe() {
    let conn = open_test_conn();
    record_nudge_signal(
        &conn,
        &NudgeSignalRecord {
            invocation_id: "inv-probe".to_owned(),
            agent_name: "right".to_owned(),
            root_session_id: Some("s-2".to_owned()),
            chat_id: Some(20),
            thread_id: Some(0),
            signal_kind: NudgeSignalKind::SkillIssue,
            payload_json: serde_json::json!({"kind":"update_candidate"}),
            source: NudgeSignalSource::ForkProbe,
        },
    )
    .unwrap();
    let source: String = conn
        .query_row(
            "SELECT source FROM skill_nudge_signals WHERE invocation_id=?1",
            ["inv-probe"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(source, "fork_probe");
}
```

(`open_test_conn` is the existing helper at the top of the test module that runs `MIGRATIONS.to_latest`.)

- [ ] **Step 3.2: Run tests to verify they fail**

```bash
devenv shell -- cargo test -p right-agent nudge_signal_source_ record_nudge_signal_persists_source_
```

Expected: 3 tests FAIL with "no `NudgeSignalSource` in scope" or "no `source` field on `NudgeSignalRecord`".

- [ ] **Step 3.3: Implement enum + extend struct + persist field**

In `crates/right-agent/src/learned_skills.rs`:

Add near `NudgeSignalKind` (around line 60-90):

```rust
/// Where an accepted learning signal originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeSignalSource {
    /// Agent emitted the signal in its structured `learning_signal`
    /// or `skill_issue_signal` reply field.
    ReplyField,
    /// Post-turn fork-classifier identified the signal.
    ForkProbe,
}

impl NudgeSignalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReplyField => "reply_field",
            Self::ForkProbe => "fork_probe",
        }
    }
}
```

Extend `NudgeSignalRecord`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct NudgeSignalRecord {
    pub invocation_id: String,
    pub agent_name: String,
    pub root_session_id: Option<String>,
    pub chat_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub signal_kind: NudgeSignalKind,
    pub payload_json: serde_json::Value,
    pub source: NudgeSignalSource,
}
```

Extend `record_nudge_signal` body:

```rust
pub fn record_nudge_signal(
    conn: &rusqlite::Connection,
    record: &NudgeSignalRecord,
) -> Result<(), rusqlite::Error> {
    let payload = serde_json::to_string(&record.payload_json)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO skill_nudge_state (agent_name) VALUES (?1)",
        [record.agent_name.as_str()],
    )?;
    tx.execute(
        "INSERT INTO skill_nudge_signals \
         (invocation_id, agent_name, root_session_id, chat_id, thread_id, signal_kind, payload_json, source) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            record.invocation_id,
            record.agent_name,
            record.root_session_id,
            record.chat_id,
            record.thread_id,
            record.signal_kind.as_str(),
            payload,
            record.source.as_str(),
        ],
    )?;
    if matches!(record.signal_kind, NudgeSignalKind::SkillIssue) {
        tx.execute(
            "UPDATE skill_nudge_state \
             SET skill_issue_hints_since_review = skill_issue_hints_since_review + 1 \
             WHERE agent_name = ?1",
            [record.agent_name.as_str()],
        )?;
    }
    tx.commit()?;
    Ok(())
}
```

Update the existing `record_nudge_signal_persists_payload_and_updates_counter` test (and any other tests in the file constructing `NudgeSignalRecord`) to set `source: NudgeSignalSource::ReplyField`. Search for `NudgeSignalRecord {` in the test module and add the field to every literal.

- [ ] **Step 3.4: Run tests to verify they pass**

```bash
devenv shell -- cargo test -p right-agent record_nudge_signal_ nudge_signal_source_
```

Expected: all green.

- [ ] **Step 3.5: Commit**

```bash
git add crates/right-agent/src/learned_skills.rs
git commit -m "feat(agent): add NudgeSignalSource enum and persist on record_nudge_signal"
```

---

## Task 4: Update worker reply-field insert site to pass `Source::ReplyField`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 4.1: Write failing test**

The existing test `crates/bot/src/telegram/worker.rs` `#[cfg(test)]` module already exercises reply-signal ingestion via the worker happy path; if it does NOT, defer adding a worker integration test here and rely on the unit test in Task 3. Instead, write a regression compile test: any callsite of `NudgeSignalRecord {...}` without `source:` should not compile.

Confirm the project will fail to compile by running:

```bash
devenv shell -- cargo build -p right-bot
```

Expected: FAILS with "missing field `source` in initializer of `NudgeSignalRecord`" pointing at `crates/bot/src/telegram/worker.rs:4327`.

- [ ] **Step 4.2: Fix the callsite**

In `crates/bot/src/telegram/worker.rs` around line 4327 (the `let record = NudgeSignalRecord { ... }` block), add the `source` field:

```rust
let record = NudgeSignalRecord {
    invocation_id: invocation_id.to_owned(),
    agent_name: ctx.agent_name.clone(),
    root_session_id: Some(session_uuid.clone()),
    chat_id: Some(chat_id),
    thread_id: Some(eff_thread_id),
    signal_kind,
    payload_json,
    source: right_agent::learned_skills::NudgeSignalSource::ReplyField,
};
```

Add the import at the top of `crates/bot/src/telegram/worker.rs` (the existing `use right_agent::learned_skills::{...}` block — append `NudgeSignalSource`):

```rust
use right_agent::learned_skills::{
    NudgeSignalKind, NudgeSignalRecord, NudgeSignalSource, ReviewGateDecision, ReviewGateInput,
    ReviewStatus,
};
```

- [ ] **Step 4.3: Run build**

```bash
devenv shell -- cargo build -p right-bot
```

Expected: builds cleanly.

- [ ] **Step 4.4: Run focused bot tests**

```bash
devenv shell -- cargo test -p right-bot worker_
```

Expected: existing worker tests still pass.

- [ ] **Step 4.5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): tag reply-field-sourced nudge signals at worker ingestion site"
```

---

## Task 5: Extend `LearningConfig` with probe + deprecation fields

**Files:**
- Modify: `crates/right-agent-config/src/lib.rs`

- [ ] **Step 5.1: Write failing tests**

Append to `crates/right-agent-config/src/lib.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn learning_config_defaults_set_fork_probe_on_and_background_off() {
    let cfg = LearningConfig::default();
    assert!(cfg.fork_probe_enabled, "fork_probe must default ON");
    assert!(
        !cfg.background_review_enabled,
        "background_review must default OFF"
    );
    assert!(
        cfg.probe_model.is_none(),
        "probe_model must default to None (inherit agent.model)"
    );
}

#[test]
fn learning_config_deserialises_minimal_yaml_with_new_defaults() {
    let yaml = "{}";
    let cfg: LearningConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(cfg.fork_probe_enabled);
    assert!(!cfg.background_review_enabled);
    assert!(cfg.probe_model.is_none());
}

#[test]
fn learning_config_deserialises_pre_v27_yaml_without_new_fields() {
    let yaml = "
episode_selector_model: claude-sonnet-4-6
episode_settle_seconds: 60
max_daily_budget_usd: 2.50
circuit_failure_threshold: 3
circuit_cooldown_minutes: 30
";
    let cfg: LearningConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(
        cfg.episode_selector_model.as_deref(),
        Some("claude-sonnet-4-6")
    );
    assert!(cfg.fork_probe_enabled);
    assert!(!cfg.background_review_enabled);
    assert_eq!(cfg.max_daily_budget_usd, 2.50);
}

#[test]
fn learning_config_accepts_probe_model_override() {
    let yaml = "probe_model: claude-haiku-4-5-20251001\n";
    let cfg: LearningConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(
        cfg.probe_model.as_deref(),
        Some("claude-haiku-4-5-20251001")
    );
}

#[test]
fn learning_config_accepts_background_review_opt_in() {
    let yaml = "background_review_enabled: true\n";
    let cfg: LearningConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(cfg.background_review_enabled);
}
```

- [ ] **Step 5.2: Run tests to verify they fail**

```bash
devenv shell -- cargo test -p right-agent-config learning_config_
```

Expected: 5 tests FAIL with "no field `fork_probe_enabled`", "no field `background_review_enabled`", "no field `probe_model`".

- [ ] **Step 5.3: Extend the struct**

In `crates/right-agent-config/src/lib.rs` `LearningConfig`:

Add new default fns near the other `default_*` helpers (around line 102):

```rust
fn default_fork_probe_enabled() -> bool {
    true
}

fn default_background_review_enabled() -> bool {
    false
}
```

Extend `LearningConfig` with three new fields:

```rust
pub struct LearningConfig {
    pub episode_selector_model: Option<String>,
    pub episode_selector_max_budget_usd: Option<f64>,

    #[serde(
        default = "default_episode_settle_seconds",
        deserialize_with = "deserialize_positive_u64"
    )]
    pub episode_settle_seconds: u64,

    #[serde(
        default = "default_max_daily_budget_usd",
        deserialize_with = "deserialize_positive_finite_f64_max_daily"
    )]
    pub max_daily_budget_usd: f64,

    #[serde(
        default = "default_circuit_failure_threshold",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub circuit_failure_threshold: u32,

    #[serde(
        default = "default_circuit_cooldown_minutes",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub circuit_cooldown_minutes: u32,

    /// Model used by the fork-probe. `None` = inherit `AgentConfig.model`.
    pub probe_model: Option<String>,

    /// Master switch for the fork-probe. Defaults `true`; set `false` to
    /// disable post-turn signal-classification probes for an agent.
    #[serde(default = "default_fork_probe_enabled")]
    pub fork_probe_enabled: bool,

    /// Deprecated Stage 2 background pipeline opt-in. Defaults `false`.
    /// When `true`, the legacy `DrainScheduler` + selector + reviewer
    /// run alongside fork-probe; daily budget covers both.
    #[serde(default = "default_background_review_enabled")]
    pub background_review_enabled: bool,
}
```

Extend `impl Default for LearningConfig`:

```rust
impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            episode_selector_model: None,
            episode_selector_max_budget_usd: None,
            episode_settle_seconds: default_episode_settle_seconds(),
            max_daily_budget_usd: default_max_daily_budget_usd(),
            circuit_failure_threshold: default_circuit_failure_threshold(),
            circuit_cooldown_minutes: default_circuit_cooldown_minutes(),
            probe_model: None,
            fork_probe_enabled: default_fork_probe_enabled(),
            background_review_enabled: default_background_review_enabled(),
        }
    }
}
```

- [ ] **Step 5.4: Run tests to verify they pass**

```bash
devenv shell -- cargo test -p right-agent-config learning_config_
```

Expected: 5 PASS.

- [ ] **Step 5.5: Verify dependent crates still build**

```bash
devenv shell -- cargo build -p right-agent -p right-bot -p right
```

Expected: builds. Any callers constructing `LearningConfig` literally will fail; if any exist, fix them by deferring to `..LearningConfig::default()` or by adding the three new fields explicitly.

- [ ] **Step 5.6: Commit**

```bash
git add crates/right-agent-config/src/lib.rs
git commit -m "feat(config): add probe_model, fork_probe_enabled, background_review_enabled"
```

---

## Task 6: Add `FORK_PROBE_SCHEMA_JSON` and `FORK_PROBE_PROMPT` constants

**Files:**
- Modify: `crates/right-codegen/src/agent_def.rs`

- [ ] **Step 6.1: Write failing tests**

Append to `crates/right-codegen/src/agent_def_tests.rs`:

```rust
#[test]
fn fork_probe_schema_is_valid_json_with_signal_fields() {
    let parsed: serde_json::Value = serde_json::from_str(FORK_PROBE_SCHEMA_JSON)
        .expect("FORK_PROBE_SCHEMA_JSON must be valid JSON");
    let properties = parsed
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("schema needs properties");
    assert!(properties.contains_key("workflow_complete"));
    assert!(properties.contains_key("learning_signal"));
    assert!(properties.contains_key("skill_issue_signal"));
    let required = parsed
        .get("required")
        .and_then(serde_json::Value::as_array)
        .expect("schema needs required");
    assert!(required.iter().any(|v| v == "workflow_complete"));
}

#[test]
fn fork_probe_prompt_contains_signal_field_names() {
    assert!(FORK_PROBE_PROMPT.contains("learning_signal"));
    assert!(FORK_PROBE_PROMPT.contains("skill_issue_signal"));
}
```

Add re-exports if the constants are not yet `pub`; the test file should be able to reach them via `use super::*;` if it shares the module, or via `right_codegen::{FORK_PROBE_SCHEMA_JSON, FORK_PROBE_PROMPT}` if external.

- [ ] **Step 6.2: Run tests to verify they fail**

```bash
devenv shell -- cargo test -p right-codegen fork_probe_
```

Expected: 2 tests FAIL: identifiers not found.

- [ ] **Step 6.3: Add the constants**

In `crates/right-codegen/src/agent_def.rs`, after `CRON_SCHEMA_JSON` / `BG_CONTINUATION_SCHEMA_JSON`:

```rust
/// JSON schema for the fork-probe classifier output.
///
/// Mirrors `learning_signal` and `skill_issue_signal` shapes from
/// `REPLY_SCHEMA_JSON`. `workflow_complete` is captured for telemetry
/// but does not gate signal ingestion in v1.
pub const FORK_PROBE_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "workflow_complete": { "type": "boolean" },
    "learning_signal": {
      "type": ["object", "null"],
      "properties": {
        "kind": { "const": "create_candidate" },
        "package_name_hint": { "type": "string" },
        "trigger": {
          "enum": ["explicit_user_request", "multi_step_workflow", "recovered_surprise", "user_correction", "repeated_tool_pattern"]
        },
        "reason_not_written": {
          "enum": ["conversation_still_evolving", "needs_full_context_review", "write_or_publish_failed", "needs_existing_skill_diff"]
        },
        "event_refs": {
          "type": "array",
          "items": { "type": "string" },
          "minItems": 1
        },
        "summary": { "type": "string" }
      },
      "required": ["kind", "package_name_hint", "trigger", "reason_not_written", "event_refs", "summary"]
    },
    "skill_issue_signal": {
      "type": ["object", "null"],
      "properties": {
        "kind": { "const": "update_candidate" },
        "skill_name": { "type": "string" },
        "issue": {
          "enum": ["missing_step", "stale_command", "wrong_api_assumption", "overbroad_activation", "broken_script", "unsafe_instruction"]
        },
        "reason_not_patched": {
          "enum": ["conversation_still_evolving", "needs_full_context_review", "write_or_publish_failed", "needs_existing_skill_diff"]
        },
        "observed_effect": {
          "enum": ["retry_after_tool_error", "retry_after_user_correction", "manual_override", "verified_alternative"]
        },
        "event_refs": {
          "type": "array",
          "items": { "type": "string" },
          "minItems": 1
        },
        "patch_hint": { "type": "string" }
      },
      "required": ["kind", "skill_name", "issue", "reason_not_patched", "observed_effect", "event_refs", "patch_hint"]
    }
  },
  "required": ["workflow_complete"]
}"#;

/// Prompt sent to the fork-probe classifier.
///
/// The probe inherits the foreground turn's transcript via
/// `claude -p --resume <main> --fork-session` and emits JSON per
/// `FORK_PROBE_SCHEMA_JSON`. Set signal fields to null when nothing qualifies.
pub const FORK_PROBE_PROMPT: &str = "\
Review the just-finished turn. \
Decide whether the workflow is complete and whether a reusable learning candidate \
(`learning_signal`) or a skill-issue worth recording (`skill_issue_signal`) exists. \
Emit JSON matching the provided schema. Set `learning_signal` and `skill_issue_signal` \
to null if nothing qualifies. Do not invoke any tool.";
```

- [ ] **Step 6.4: Run tests to verify they pass**

```bash
devenv shell -- cargo test -p right-codegen fork_probe_
```

Expected: 2 PASS.

- [ ] **Step 6.5: Commit**

```bash
git add crates/right-codegen/src/agent_def.rs crates/right-codegen/src/agent_def_tests.rs
git commit -m "feat(codegen): add FORK_PROBE_SCHEMA_JSON and FORK_PROBE_PROMPT"
```

---

## Task 7: Create `learning_probe` module — pure logic (no I/O)

**Files:**
- Create: `crates/bot/src/learning_probe.rs`
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 7.1: Write failing tests**

Create `crates/bot/src/learning_probe.rs` with TESTS FIRST (no implementation yet):

```rust
//! Post-turn fork-probe: classify whether the just-finished foreground turn
//! contains a learnable signal that the agent failed to emit in its
//! structured reply.
//!
//! Spec: docs/superpowers/specs/2026-05-21-learning-fork-probe-design.md

use right_agent::learned_skills::{NudgeSignalKind, NudgeSignalSource};

/// Decision returned by [`should_run_probe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeDecision {
    Run,
    SkipReplyHasSignal,
    SkipDisabled,
    SkipBudgetExceeded,
    SkipNonForeground,
}

/// Inputs to the probe gate.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProbeGateInput {
    pub fork_probe_enabled: bool,
    pub is_foreground: bool,
    pub reply_has_signal: bool,
    pub today_spend_usd: f64,
    pub daily_budget_usd: f64,
}

/// Pure decision function. No I/O.
pub(crate) fn should_run_probe(input: ProbeGateInput) -> ProbeDecision {
    if !input.fork_probe_enabled {
        return ProbeDecision::SkipDisabled;
    }
    if !input.is_foreground {
        return ProbeDecision::SkipNonForeground;
    }
    if input.reply_has_signal {
        return ProbeDecision::SkipReplyHasSignal;
    }
    if input.today_spend_usd >= input.daily_budget_usd {
        return ProbeDecision::SkipBudgetExceeded;
    }
    ProbeDecision::Run
}

/// Parsed fork-probe stdout JSON.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedProbe {
    pub workflow_complete: bool,
    pub learning_signal: Option<serde_json::Value>,
    pub skill_issue_signal: Option<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProbeParseError {
    #[error("probe stdout is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("probe JSON missing required field `workflow_complete`")]
    MissingWorkflowComplete,
    #[error("probe JSON `workflow_complete` must be a boolean")]
    WorkflowCompleteNotBool,
}

/// Parse the JSON document returned by `--output-format json`.
///
/// CC wraps the assistant reply in `{"result": {...}}` for non-stream output;
/// we tolerate both shapes (unwrapped object or `result`-wrapped).
pub(crate) fn parse_probe_output(stdout: &str) -> Result<ParsedProbe, ProbeParseError> {
    let value: serde_json::Value = serde_json::from_str(stdout)?;
    let body = match value.get("result") {
        Some(serde_json::Value::Object(_)) => &value["result"],
        _ => &value,
    };
    let workflow_complete = body
        .get("workflow_complete")
        .ok_or(ProbeParseError::MissingWorkflowComplete)?
        .as_bool()
        .ok_or(ProbeParseError::WorkflowCompleteNotBool)?;
    let learning_signal = body
        .get("learning_signal")
        .filter(|v| !v.is_null())
        .cloned();
    let skill_issue_signal = body
        .get("skill_issue_signal")
        .filter(|v| !v.is_null())
        .cloned();
    Ok(ParsedProbe {
        workflow_complete,
        learning_signal,
        skill_issue_signal,
    })
}

/// Choose which (kind, payload) to persist when probe returned both.
/// Prefers `learning_signal` over `skill_issue_signal`.
pub(crate) fn select_probe_signal(
    parsed: &ParsedProbe,
) -> Option<(NudgeSignalKind, serde_json::Value)> {
    if let Some(payload) = parsed.learning_signal.clone() {
        return Some((NudgeSignalKind::Learning, payload));
    }
    if let Some(payload) = parsed.skill_issue_signal.clone() {
        return Some((NudgeSignalKind::SkillIssue, payload));
    }
    None
}

/// The source value to attach to a fork-probe-derived nudge signal.
pub(crate) fn probe_signal_source() -> NudgeSignalSource {
    NudgeSignalSource::ForkProbe
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(
        enabled: bool,
        fg: bool,
        has: bool,
        spend: f64,
        budget: f64,
    ) -> ProbeGateInput {
        ProbeGateInput {
            fork_probe_enabled: enabled,
            is_foreground: fg,
            reply_has_signal: has,
            today_spend_usd: spend,
            daily_budget_usd: budget,
        }
    }

    #[test]
    fn gate_returns_run_when_all_conditions_met() {
        assert_eq!(
            should_run_probe(gate(true, true, false, 0.0, 1.0)),
            ProbeDecision::Run
        );
    }

    #[test]
    fn gate_skips_when_disabled() {
        assert_eq!(
            should_run_probe(gate(false, true, false, 0.0, 1.0)),
            ProbeDecision::SkipDisabled
        );
    }

    #[test]
    fn gate_skips_when_not_foreground() {
        assert_eq!(
            should_run_probe(gate(true, false, false, 0.0, 1.0)),
            ProbeDecision::SkipNonForeground
        );
    }

    #[test]
    fn gate_skips_when_reply_already_has_signal() {
        assert_eq!(
            should_run_probe(gate(true, true, true, 0.0, 1.0)),
            ProbeDecision::SkipReplyHasSignal
        );
    }

    #[test]
    fn gate_skips_when_budget_met_or_exceeded() {
        assert_eq!(
            should_run_probe(gate(true, true, false, 1.0, 1.0)),
            ProbeDecision::SkipBudgetExceeded
        );
        assert_eq!(
            should_run_probe(gate(true, true, false, 1.50, 1.0)),
            ProbeDecision::SkipBudgetExceeded
        );
    }

    #[test]
    fn parse_probe_output_accepts_null_signals() {
        let stdout = r#"{"workflow_complete":true,"learning_signal":null,"skill_issue_signal":null}"#;
        let parsed = parse_probe_output(stdout).unwrap();
        assert!(parsed.workflow_complete);
        assert!(parsed.learning_signal.is_none());
        assert!(parsed.skill_issue_signal.is_none());
    }

    #[test]
    fn parse_probe_output_accepts_learning_signal() {
        let stdout = r#"{"workflow_complete":true,"learning_signal":{"kind":"create_candidate","package_name_hint":"x","trigger":"explicit_user_request","reason_not_written":"needs_full_context_review","event_refs":["e1"],"summary":"s"}}"#;
        let parsed = parse_probe_output(stdout).unwrap();
        assert!(parsed.learning_signal.is_some());
        assert!(parsed.skill_issue_signal.is_none());
    }

    #[test]
    fn parse_probe_output_unwraps_result_envelope() {
        let stdout = r#"{"result":{"workflow_complete":false,"learning_signal":null,"skill_issue_signal":null}}"#;
        let parsed = parse_probe_output(stdout).unwrap();
        assert!(!parsed.workflow_complete);
    }

    #[test]
    fn parse_probe_output_rejects_missing_required_field() {
        let stdout = r#"{}"#;
        let err = parse_probe_output(stdout).unwrap_err();
        assert!(matches!(err, ProbeParseError::MissingWorkflowComplete));
    }

    #[test]
    fn parse_probe_output_rejects_malformed_json() {
        let err = parse_probe_output("not json").unwrap_err();
        assert!(matches!(err, ProbeParseError::Json(_)));
    }

    #[test]
    fn select_probe_signal_prefers_learning_over_skill_issue() {
        let parsed = ParsedProbe {
            workflow_complete: true,
            learning_signal: Some(serde_json::json!({"kind":"create_candidate"})),
            skill_issue_signal: Some(serde_json::json!({"kind":"update_candidate"})),
        };
        let (kind, _) = select_probe_signal(&parsed).unwrap();
        assert_eq!(kind, NudgeSignalKind::Learning);
    }

    #[test]
    fn select_probe_signal_returns_none_when_both_null() {
        let parsed = ParsedProbe {
            workflow_complete: true,
            learning_signal: None,
            skill_issue_signal: None,
        };
        assert!(select_probe_signal(&parsed).is_none());
    }

    #[test]
    fn probe_signal_source_is_fork_probe() {
        assert_eq!(probe_signal_source(), NudgeSignalSource::ForkProbe);
    }
}
```

In `crates/bot/src/lib.rs`, register the module:

```rust
pub(crate) mod learning_probe;
```

- [ ] **Step 7.2: Run tests to verify they fail (compile)**

```bash
devenv shell -- cargo test -p right-bot --lib learning_probe
```

Expected: tests FAIL — first because the module file exists but the impl signatures don't match (or because `NudgeSignalKind::Learning` is the wrong PartialEq).

Adjust the impls in Step 7.1 — the file we created already has the implementations. So this step should actually PASS. Run it.

If tests pass on first run (because Step 7.1 included impl), commit and move on. If they don't, fix.

- [ ] **Step 7.3: Run tests**

```bash
devenv shell -- cargo test -p right-bot --lib learning_probe
```

Expected: 12 tests PASS.

- [ ] **Step 7.4: Commit**

```bash
git add crates/bot/src/learning_probe.rs crates/bot/src/lib.rs
git commit -m "feat(bot): add learning_probe module with pure gate and parse logic"
```

---

## Task 8: `learning_probe::spawn_probe` — async probe execution and persistence

**Files:**
- Modify: `crates/bot/src/learning_probe.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 8.1: Add the probe-running function with a mock-friendly seam**

In `crates/bot/src/learning_probe.rs`, append (above `#[cfg(test)] mod tests`):

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Bundle of inputs needed to spawn one fork-probe.
#[derive(Debug, Clone)]
pub(crate) struct ProbeContext {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub main_session_id: String,
    pub chat_id: i64,
    pub thread_id: i64,
    pub probe_model: Option<String>,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub debug_flag: Arc<std::sync::atomic::AtomicBool>,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Build the `ClaudeInvocation` for the fork-probe.
///
/// Session-bearing (preserves the contract invariant). Passes `--tools ""`
/// to block all tool use; MCP config is loaded but unused for tools.
pub(crate) fn build_probe_invocation(
    ctx: &ProbeContext,
    probe_session_id: &str,
) -> crate::cc::invocation::ClaudeInvocation {
    crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: Some(crate::cc::invocation::mcp_config_path(
            ctx.ssh_config_path.as_deref(),
            &ctx.agent_dir,
        )),
        json_schema: Some(right_codegen::FORK_PROBE_SCHEMA_JSON.into()),
        output_format: crate::cc::invocation::OutputFormat::Json,
        model: ctx.probe_model.clone(),
        max_budget_usd: None,
        max_turns: Some(1),
        resume_session_id: Some(ctx.main_session_id.clone()),
        new_session_id: Some(probe_session_id.to_owned()),
        fork_session: true,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: crate::cc::invocation::disable_all_tools_args(),
        prompt: Some(right_codegen::FORK_PROBE_PROMPT.into()),
        debug_flag: Some(Arc::clone(&ctx.debug_flag)),
    }
}

/// Fire-and-forget the probe. Spawned via `tokio::spawn` by the caller.
pub(crate) async fn run_probe(ctx: ProbeContext) {
    let probe_session_id = uuid::Uuid::new_v4().to_string();
    let invocation = build_probe_invocation(&ctx, &probe_session_id);
    let args = invocation.into_args();
    let mut cmd = crate::cc::invocation::build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    );
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = match tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                main_session = %ctx.main_session_id,
                "fork-probe spawn failed: {e:#}"
            );
            return;
        }
        Err(_) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                main_session = %ctx.main_session_id,
                "fork-probe timed out after {}s",
                PROBE_TIMEOUT.as_secs()
            );
            return;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            agent = %ctx.agent_name,
            main_session = %ctx.main_session_id,
            status = ?output.status,
            stderr = %stderr,
            "fork-probe exited non-zero"
        );
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    record_probe_result(&ctx, &probe_session_id, &stdout);
}

fn record_probe_result(ctx: &ProbeContext, probe_session_id: &str, stdout: &str) {
    let parsed = match parse_probe_output(stdout) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                main_session = %ctx.main_session_id,
                error = %e,
                stdout_excerpt = %stdout.chars().take(256).collect::<String>(),
                "fork-probe stdout parse failed"
            );
            return;
        }
    };

    let conn = match right_db::open_connection(&ctx.agent_db_dir, false) {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "fork-probe db open failed: {e:#}"
            );
            return;
        }
    };

    if let Some(breakdown) = crate::cc::stream::parse_usage_full(stdout) {
        if let Err(e) = right_agent::usage::insert::insert_learning_fork_probe(
            &conn,
            &breakdown,
            ctx.chat_id,
            ctx.thread_id,
        ) {
            tracing::warn!(
                agent = %ctx.agent_name,
                "fork-probe usage insert failed: {e:#}"
            );
        }
    }

    let Some((signal_kind, payload_json)) = select_probe_signal(&parsed) else {
        return;
    };

    let record = right_agent::learned_skills::NudgeSignalRecord {
        invocation_id: probe_session_id.to_owned(),
        agent_name: ctx.agent_name.clone(),
        root_session_id: Some(ctx.main_session_id.clone()),
        chat_id: Some(ctx.chat_id),
        thread_id: Some(ctx.thread_id),
        signal_kind,
        payload_json,
        source: NudgeSignalSource::ForkProbe,
    };
    if let Err(e) = right_agent::learned_skills::record_nudge_signal(&conn, &record) {
        tracing::warn!(
            agent = %ctx.agent_name,
            "fork-probe signal record failed: {e:#}"
        );
    }
}

/// Read today's spend across `LEARNING_SOURCES` to feed the budget gate.
pub(crate) fn today_spend_usd(
    conn: &rusqlite::Connection,
    now_utc: &str,
) -> Result<f64, rusqlite::Error> {
    let date_part = now_utc.split_once('T').map(|(d, _)| d).unwrap_or(now_utc);
    let today_start = format!("{date_part}T00:00:00Z");
    let placeholders = std::iter::repeat("?")
        .take(right_agent::usage::LEARNING_SOURCES.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT COALESCE(SUM(total_cost_usd), 0.0) FROM usage_events \
         WHERE ts >= ?1 AND source IN ({placeholders})"
    );
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&today_start];
    for s in right_agent::usage::LEARNING_SOURCES {
        params.push(s);
    }
    conn.query_row(&sql, params.as_slice(), |r| r.get::<_, f64>(0))
}
```

Add a focused unit test for `build_probe_invocation`:

```rust
#[test]
fn build_probe_invocation_emits_fork_and_disables_tools() {
    use std::sync::atomic::AtomicBool;
    let ctx = ProbeContext {
        agent_dir: PathBuf::from("/tmp/agent"),
        agent_db_dir: PathBuf::from("/tmp/agent"),
        agent_name: "right".into(),
        main_session_id: "main-uuid".into(),
        chat_id: 100,
        thread_id: 0,
        probe_model: Some("claude-opus-4-7".into()),
        ssh_config_path: None,
        resolved_sandbox: None,
        debug_flag: Arc::new(AtomicBool::new(false)),
    };
    let inv = build_probe_invocation(&ctx, "probe-uuid");
    let args = inv.into_args();
    assert!(args.iter().any(|a| a == "--fork-session"));
    let resume_pos = args.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(args[resume_pos + 1], "main-uuid");
    let sid_pos = args.iter().position(|a| a == "--session-id").unwrap();
    assert_eq!(args[sid_pos + 1], "probe-uuid");
    let tools_pos = args.iter().position(|a| a == "--tools").unwrap();
    assert_eq!(args[tools_pos + 1], "");
    let model_pos = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(args[model_pos + 1], "claude-opus-4-7");
    let max_turns_pos = args.iter().position(|a| a == "--max-turns").unwrap();
    assert_eq!(args[max_turns_pos + 1], "1");
}
```

Add `uuid = "1"` to `crates/bot/Cargo.toml` `[dependencies]` if not already present (search via `rg uuid crates/bot/Cargo.toml`).

- [ ] **Step 8.2: Run the test**

```bash
devenv shell -- cargo test -p right-bot --lib learning_probe::tests::build_probe_invocation_
```

Expected: PASS.

- [ ] **Step 8.3: Wire `run_probe` into worker.rs**

In `crates/bot/src/telegram/worker.rs`, find the location AFTER `archive_assistant_message(...)` is called for a successful foreground reply (search for `archive_assistant_message` around line 1388). Add:

```rust
// Fire-and-forget post-turn fork-probe. Must run after Telegram send so
// it never blocks user-visible latency.
if ctx.learning.fork_probe_enabled
    && matches!(prompt_mode, crate::cc::prompt::PromptMode::Normal)
    && accepted_review_signal.is_none()
{
    let now_utc = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let probe_conn = right_db::open_connection(&ctx.agent_db_dir, false);
    let today_spend = probe_conn
        .as_ref()
        .ok()
        .and_then(|conn| crate::learning_probe::today_spend_usd(conn, &now_utc).ok())
        .unwrap_or(0.0);
    let decision = crate::learning_probe::should_run_probe(
        crate::learning_probe::ProbeGateInput {
            fork_probe_enabled: ctx.learning.fork_probe_enabled,
            is_foreground: true,
            reply_has_signal: accepted_review_signal.is_some(),
            today_spend_usd: today_spend,
            daily_budget_usd: ctx.learning.max_daily_budget_usd,
        },
    );
    if matches!(decision, crate::learning_probe::ProbeDecision::Run) {
        let probe_ctx = crate::learning_probe::ProbeContext {
            agent_dir: ctx.agent_dir.clone(),
            agent_db_dir: ctx.agent_db_dir.clone(),
            agent_name: ctx.agent_name.clone(),
            main_session_id: session_uuid.clone(),
            chat_id,
            thread_id: eff_thread_id,
            probe_model: ctx
                .learning
                .probe_model
                .clone()
                .or_else(|| ctx.model.load().as_ref().clone()),
            ssh_config_path: ctx.ssh_config_path.clone(),
            resolved_sandbox: ctx.resolved_sandbox.clone(),
            debug_flag: Arc::clone(&ctx.debug),
        };
        tokio::spawn(async move {
            crate::learning_probe::run_probe(probe_ctx).await;
        });
    }
}
```

Notes about identifier names — verify exact field names by reading `crates/bot/src/telegram/worker.rs::WorkerContext`. If the model is `Arc<ArcSwap<Option<String>>>`, adapt the deref. If `ssh_config_path` lives elsewhere on the context, adapt. The shape is:

- `ctx.learning: LearningConfig`
- `ctx.agent_dir: PathBuf`
- `ctx.agent_db_dir: PathBuf`
- `ctx.agent_name: String`
- `ctx.model: Arc<ArcSwap<Option<String>>>`
- `ctx.debug: Arc<AtomicBool>`
- `ctx.ssh_config_path: Option<PathBuf>`
- `ctx.resolved_sandbox: Option<String>`

If `WorkerContext` does not expose `learning`, add a field carrying `LearningConfig` (clone-on-spawn is fine — it's small).

- [ ] **Step 8.4: Build the bot crate**

```bash
devenv shell -- cargo build -p right-bot
```

Expected: builds. Fix any name mismatches by reading the actual struct.

- [ ] **Step 8.5: Run focused tests**

```bash
devenv shell -- cargo test -p right-bot --lib learning_probe
```

Expected: PASS.

- [ ] **Step 8.6: Commit**

```bash
git add crates/bot/src/learning_probe.rs crates/bot/src/telegram/worker.rs crates/bot/Cargo.toml
git commit -m "feat(bot): spawn post-turn fork-probe and persist non-null signals"
```

---

## Task 9: Gate `DrainScheduler` on `background_review_enabled`

**Files:**
- Modify: `crates/bot/src/learning_episode.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/cron.rs`

- [ ] **Step 9.1: Write failing test**

Append to `crates/bot/src/learning_episode_tests.rs`:

```rust
#[tokio::test]
async fn drain_scheduler_noop_when_background_disabled() {
    use crate::learning_episode::DrainScheduler;
    use tokio_util::sync::CancellationToken;

    let cancel = CancellationToken::new();
    let scheduler = DrainScheduler::noop();
    scheduler.schedule_drain();
    // No panic, no work — the noop variant must be cheap.
    drop(scheduler);
    cancel.cancel();
}
```

- [ ] **Step 9.2: Run test to verify it fails**

```bash
devenv shell -- cargo test -p right-bot --lib drain_scheduler_noop_
```

Expected: FAIL — `DrainScheduler::noop` does not exist.

- [ ] **Step 9.3: Add the noop variant**

In `crates/bot/src/learning_episode.rs`, refactor `DrainScheduler` to either:

(a) Add a `noop: bool` field that short-circuits `schedule_drain` and `spawn_drain_loop`, or
(b) Wrap the public type in an enum `DrainScheduler::Active(ActiveScheduler) | DrainScheduler::Noop`.

Use option (a) — single struct, simplest change:

```rust
pub(crate) struct DrainScheduler {
    inner: Option<DrainSchedulerInner>,
}

struct DrainSchedulerInner {
    // ... whatever the existing struct had ...
}

impl DrainScheduler {
    pub(crate) fn noop() -> Self {
        Self { inner: None }
    }

    pub(crate) fn spawn(
        // ... existing args ...
    ) -> (Self, tokio::task::JoinHandle<()>) {
        // ... existing logic, returning Self { inner: Some(...) } ...
    }

    pub(crate) fn schedule_drain(&self) {
        if let Some(inner) = self.inner.as_ref() {
            inner.schedule_drain();
        }
        // else: noop
    }
}
```

Move existing body of `DrainScheduler::schedule_drain` to `DrainSchedulerInner::schedule_drain`. The `JoinHandle` returned from `spawn` already covers the active path; the noop variant returns a dummy handle. Caller sites in `dispatch.rs` and `cron.rs` that currently pattern `let (scheduler, handle) = DrainScheduler::spawn(...)` will need to construct the noop variant differently:

```rust
let (learning_drain_scheduler, _learning_drain_handle) =
    if agent.learning.background_review_enabled {
        let (s, h) = crate::learning_episode::DrainScheduler::spawn(
            // ... existing args ...
        );
        (Arc::new(s), Some(h))
    } else {
        (Arc::new(crate::learning_episode::DrainScheduler::noop()), None)
    };
```

Apply equivalent at:
- `crates/bot/src/telegram/dispatch.rs:224` (real path)
- `crates/bot/src/telegram/dispatch.rs:694–710` (smoke-test path — keep using `spawn` since the test expects an active scheduler)
- `crates/bot/src/cron.rs:984`, `:1075–1087`, `:1147–1180` (real paths used by cron startup)

- [ ] **Step 9.4: Build**

```bash
devenv shell -- cargo build -p right-bot
```

Expected: builds.

- [ ] **Step 9.5: Run tests**

```bash
devenv shell -- cargo test -p right-bot --lib drain_scheduler_
```

Expected: existing scheduler tests still pass; new `drain_scheduler_noop_when_background_disabled` PASSES.

- [ ] **Step 9.6: Commit**

```bash
git add crates/bot/src/learning_episode.rs \
        crates/bot/src/telegram/dispatch.rs \
        crates/bot/src/cron.rs \
        crates/bot/src/learning_episode_tests.rs
git commit -m "feat(bot): gate DrainScheduler on background_review_enabled flag"
```

---

## Task 10: Bot-startup deprecation WARN

**Files:**
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 10.1: Write failing test**

This is a logged side-effect; testable via a unit function that produces the message rather than emitting it.

In `crates/bot/src/learning_episode.rs` (or new tiny module), add:

```rust
/// Build the deprecation WARN text for an agent whose `learning_episodes`
/// table has fresh activity but whose `background_review_enabled = false`.
pub(crate) fn deprecation_warn_message(agent_name: &str) -> String {
    format!(
        "agent {agent_name}: background learning is deprecated and disabled by default. \
         Set `learning.background_review_enabled: true` in agents/{agent_name}/agent.yaml \
         to restore the prior pipeline; otherwise post-turn fork-probe takes over. \
         Cleanup spec will drop the background code in a future release."
    )
}

/// Detect whether the agent has `learning_episodes` rows newer than 24h.
pub(crate) fn has_recent_legacy_activity(
    conn: &rusqlite::Connection,
) -> Result<bool, rusqlite::Error> {
    let now = chrono::Utc::now();
    let cutoff = (now - chrono::Duration::hours(24))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM learning_episodes WHERE created_at >= ?1",
        [cutoff.as_str()],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}
```

Append tests:

```rust
#[cfg(test)]
mod deprecation_warn_tests {
    use super::*;

    #[test]
    fn message_mentions_agent_name_and_yaml_key() {
        let msg = deprecation_warn_message("agent-b");
        assert!(msg.contains("agent-b"));
        assert!(msg.contains("background_review_enabled"));
        assert!(msg.contains("agents/agent-b/agent.yaml"));
    }
}
```

- [ ] **Step 10.2: Run test**

```bash
devenv shell -- cargo test -p right-bot deprecation_warn_
```

Expected: PASS.

- [ ] **Step 10.3: Call from bot startup**

In `crates/bot/src/lib.rs` `run_bot` (or whatever the agent-init entrypoint is — search for `LearningConfig` or `background_review_enabled` references after Task 5; the call point is right after the per-agent config is loaded and before the agent's main loop spawns):

```rust
if !agent.learning.background_review_enabled {
    if let Ok(conn) = right_db::open_connection(&agent_db_dir, false) {
        match crate::learning_episode::has_recent_legacy_activity(&conn) {
            Ok(true) => {
                tracing::warn!(
                    "{}",
                    crate::learning_episode::deprecation_warn_message(&agent.name)
                );
            }
            Ok(false) => {}
            Err(e) => tracing::warn!(
                agent = %agent.name,
                "deprecation activity check failed: {e:#}"
            ),
        }
    }
}
```

- [ ] **Step 10.4: Build and run tests**

```bash
devenv shell -- cargo build -p right-bot
devenv shell -- cargo test -p right-bot deprecation_
```

Expected: green.

- [ ] **Step 10.5: Commit**

```bash
git add crates/bot/src/learning_episode.rs crates/bot/src/lib.rs
git commit -m "feat(bot): warn at startup when background learning is deprecated but legacy rows exist"
```

---

## Task 11: Wizard prompts for new fields

**Files:**
- Modify: `crates/right/src/wizard.rs`

- [ ] **Step 11.1: Write failing tests**

Append to `crates/right/src/wizard.rs` `#[cfg(test)] mod tests` (or the closest test module):

```rust
#[test]
fn parse_probe_model_blank_returns_none() {
    assert_eq!(parse_probe_model(""), Ok(None));
}

#[test]
fn parse_probe_model_string_returns_some() {
    assert_eq!(
        parse_probe_model("claude-haiku-4-5-20251001"),
        Ok(Some("claude-haiku-4-5-20251001".to_owned()))
    );
}

#[test]
fn parse_fork_probe_enabled_accepts_yes_no_true_false() {
    assert_eq!(parse_fork_probe_enabled("yes"), Ok(true));
    assert_eq!(parse_fork_probe_enabled("no"), Ok(false));
    assert_eq!(parse_fork_probe_enabled("true"), Ok(true));
    assert_eq!(parse_fork_probe_enabled("false"), Ok(false));
    assert!(parse_fork_probe_enabled("maybe").is_err());
}

#[test]
fn parse_background_review_enabled_defaults_off() {
    assert_eq!(parse_background_review_enabled("no"), Ok(false));
    assert_eq!(parse_background_review_enabled(""), Ok(false));
    assert_eq!(parse_background_review_enabled("yes"), Ok(true));
}
```

- [ ] **Step 11.2: Run tests to verify they fail**

```bash
devenv shell -- cargo test -p right parse_probe_model_ parse_fork_probe_enabled_ parse_background_review_enabled_
```

Expected: FAIL — parsers do not exist.

- [ ] **Step 11.3: Implement parsers and wizard prompts**

In `crates/right/src/wizard.rs`:

```rust
pub(crate) fn parse_probe_model(input: &str) -> Result<Option<String>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_owned()))
    }
}

pub(crate) fn parse_fork_probe_enabled(input: &str) -> Result<bool, String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "yes" | "y" | "true" | "on" => Ok(true),
        "no" | "n" | "false" | "off" => Ok(false),
        other => Err(format!("invalid boolean: '{other}'")),
    }
}

pub(crate) fn parse_background_review_enabled(input: &str) -> Result<bool, String> {
    if input.trim().is_empty() {
        return Ok(false);
    }
    parse_fork_probe_enabled(input)
}
```

Then in `learning_setup` (the function that prompts user for `LearningConfig` fields — search file for `episode_settle_seconds` or `max_daily_budget_usd` to locate):

Add new prompts AFTER the existing budget prompt:

```rust
let probe_model = right_ui::prompt::ask_optional_string(
    "Fork-probe model (blank = inherit agent model)",
    existing.probe_model.as_deref(),
)?;
let probe_model = parse_probe_model(&probe_model).map_err(|e| anyhow::anyhow!(e))?;

let fork_probe_enabled = right_ui::prompt::ask_bool(
    "Enable fork-probe (post-turn signal classifier)?",
    existing.fork_probe_enabled,
)?;

let background_review_enabled = right_ui::prompt::ask_bool(
    "Enable deprecated background learning (advanced; off by default)?",
    existing.background_review_enabled,
)?;
```

(If `right_ui::prompt::ask_optional_string` or `ask_bool` don't exist with these names, use the existing project pattern — likely `inquire::Text::new(...).prompt()` or a project helper. Search for `existing.max_daily_budget_usd` to copy the surrounding idiom.)

Assemble the final `LearningConfig` using the new fields:

```rust
LearningConfig {
    episode_selector_model: existing.episode_selector_model.clone(),
    episode_selector_max_budget_usd: existing.episode_selector_max_budget_usd,
    episode_settle_seconds,
    max_daily_budget_usd,
    circuit_failure_threshold: existing.circuit_failure_threshold,
    circuit_cooldown_minutes: existing.circuit_cooldown_minutes,
    probe_model,
    fork_probe_enabled,
    background_review_enabled,
}
```

- [ ] **Step 11.4: Run tests**

```bash
devenv shell -- cargo test -p right parse_probe_model_ parse_fork_probe_enabled_ parse_background_review_enabled_
```

Expected: PASS.

- [ ] **Step 11.5: Commit**

```bash
git add crates/right/src/wizard.rs
git commit -m "feat(cli): wizard prompts for probe_model, fork_probe_enabled, background_review_enabled"
```

---

## Task 12: Dashboard read model — signals by source

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs`
- Modify: `crates/right-dashboard/src/read_model/learning.rs`

- [ ] **Step 12.1: Write failing test**

Append to `crates/right-dashboard/src/read_model/learning.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn signals_by_source_24h_returns_three_buckets_with_correct_counts() {
    use chrono::Utc;
    use rusqlite::Connection;

    let mut conn = Connection::open_in_memory().unwrap();
    right_db::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();

    let now = "2026-05-21T12:00:00Z";
    // 2 reply_field, 1 fork_probe, all within 24h.
    for (inv, src) in [
        ("inv-a", "reply_field"),
        ("inv-b", "reply_field"),
        ("inv-c", "fork_probe"),
    ] {
        conn.execute(
            "INSERT INTO skill_nudge_signals \
             (invocation_id, agent_name, root_session_id, chat_id, thread_id, signal_kind, payload_json, accepted_at, source) \
             VALUES (?1, 'right', 's', 1, 0, 'learning', '{}', ?2, ?3)",
            rusqlite::params![inv, "2026-05-21T11:00:00Z", src],
        )
        .unwrap();
    }
    // 1 background-review-equivalent report within 24h.
    conn.execute(
        "INSERT INTO skill_review_reports \
         (agent_name, source_invocation_id, root_session_id, chat_id, thread_id, trigger_kind, status, confidence, candidate_skill_name, candidate_summary, evidence_refs_json, review_output_json, telegram_notified, created_at) \
         VALUES ('right', 'inv-x', 's', 1, 0, 'learning_signal', 'create_candidate', 'medium', 'rightx-foo', NULL, '[]', '{}', 0, '2026-05-21T10:00:00Z')",
        [],
    )
    .unwrap();

    let result = signals_by_source_24h(&conn, "right", now).unwrap();
    assert_eq!(result.reply_field, 2);
    assert_eq!(result.fork_probe, 1);
    assert_eq!(result.background_review, 1);
}

#[test]
fn signals_by_source_24h_window_excludes_old_rows() {
    use rusqlite::Connection;

    let mut conn = Connection::open_in_memory().unwrap();
    right_db::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();

    conn.execute(
        "INSERT INTO skill_nudge_signals \
         (invocation_id, agent_name, root_session_id, chat_id, thread_id, signal_kind, payload_json, accepted_at, source) \
         VALUES ('old', 'right', 's', 1, 0, 'learning', '{}', '2026-05-19T00:00:00Z', 'fork_probe')",
        [],
    )
    .unwrap();

    let result = signals_by_source_24h(&conn, "right", "2026-05-21T12:00:00Z").unwrap();
    assert_eq!(result.fork_probe, 0);
}
```

- [ ] **Step 12.2: Run tests to verify they fail**

```bash
devenv shell -- cargo test -p right-dashboard signals_by_source_24h_
```

Expected: FAIL.

- [ ] **Step 12.3: Add response type + read-model fn**

In `crates/right-dashboard/src/api_types.rs`:

```rust
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalsBySourceResponse {
    pub agent: String,
    pub window_label: String,
    pub reply_field: i64,
    pub fork_probe: i64,
    pub background_review: i64,
}
```

In `crates/right-dashboard/src/read_model/learning.rs`:

```rust
use chrono::{DateTime, Duration, Utc};

pub fn signals_by_source_24h(
    conn: &rusqlite::Connection,
    agent: &str,
    now_utc: &str,
) -> Result<crate::api_types::SignalsBySourceResponse, super::ReadModelError> {
    let now = DateTime::parse_from_rfc3339(now_utc)?.with_timezone(&Utc);
    let cutoff = (now - Duration::hours(24)).to_rfc3339();

    let reply_field: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skill_nudge_signals \
         WHERE agent_name = ?1 AND accepted_at >= ?2 AND source = 'reply_field'",
        rusqlite::params![agent, cutoff.as_str()],
        |r| r.get(0),
    )?;
    let fork_probe: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skill_nudge_signals \
         WHERE agent_name = ?1 AND accepted_at >= ?2 AND source = 'fork_probe'",
        rusqlite::params![agent, cutoff.as_str()],
        |r| r.get(0),
    )?;
    let background_review: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skill_review_reports \
         WHERE agent_name = ?1 AND created_at >= ?2 \
           AND status IN ('create_candidate','update_candidate')",
        rusqlite::params![agent, cutoff.as_str()],
        |r| r.get(0),
    )?;

    Ok(crate::api_types::SignalsBySourceResponse {
        agent: agent.to_owned(),
        window_label: "24h".to_owned(),
        reply_field,
        fork_probe,
        background_review,
    })
}
```

If `ReadModelError` doesn't already wrap `chrono::ParseError`, add a variant. Verify the existing variant list before patching.

- [ ] **Step 12.4: Run tests**

```bash
devenv shell -- cargo test -p right-dashboard signals_by_source_24h_
```

Expected: PASS.

- [ ] **Step 12.5: Wire into the API route layer (bot side)**

This step depends on existing dashboard routing in `crates/bot/src/telegram/dashboard.rs`. Read that file to find the route mount points (search for existing `learning_episodes` or `nudge_signals` route handlers) and add a new route, e.g. `GET /api/v1/agents/<name>/learning/signals_by_source` that calls `signals_by_source_24h` with `chrono::Utc::now().to_rfc3339()`. Match the existing handler signature/style verbatim; do not invent a new pattern.

- [ ] **Step 12.6: Commit**

```bash
git add crates/right-dashboard/src/api_types.rs \
        crates/right-dashboard/src/read_model/learning.rs \
        crates/bot/src/telegram/dashboard.rs
git commit -m "feat(dashboard): expose learning signals_by_source_24h read model"
```

---

## Task 13: Final workspace test and self-review

- [ ] **Step 13.1: Full workspace build**

```bash
devenv shell -- cargo build --workspace
```

Expected: clean build, no warnings other than known pre-existing ones.

- [ ] **Step 13.2: Full workspace test**

```bash
devenv shell -- cargo test --workspace
```

Expected: 0 failures across all crates. If any failure is in a crate this plan did not touch, verify it was failing on master at the start of the worktree (`git stash; cargo test -p <crate>; git stash pop`); if pre-existing, note it and proceed.

- [ ] **Step 13.3: Live-CC smoke (optional, gated by environment)**

```bash
devenv shell -- cargo test --workspace -- --ignored ci_claude_
```

If the live-CC smoke test environment is present, ensure no fork-probe regressions; if absent, document as a follow-up.

- [ ] **Step 13.4: Run `cargo clippy --workspace`**

```bash
devenv shell -- cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean. Pre-existing warnings noted earlier (right-openshell, right-dashboard) may still be present; do not fix what this plan did not introduce.

- [ ] **Step 13.5: Sanity-check the deprecation path**

Manually verify by editing a local `agent.yaml` to set `background_review_enabled: true`, restarting the agent, and confirming the legacy `DrainScheduler` resumes (look for the existing log line emitted on first drain tick). Then flip to `false` and confirm the WARN fires when legacy episode rows are present.

- [ ] **Step 13.6: Final commit (if any nit fixes needed)**

```bash
git add -A
git diff --cached
git commit -m "chore(learning-fork-probe): post-test cleanup"
```

Only commit if Step 13.1–13.4 surfaced edits.

---

## Self-Review Notes

Spec coverage check (manually run while writing — leaving here as a marker for the implementer to re-check):

- [x] v27 migration → Task 1.
- [x] `source` column with `reply_field` / `fork_probe` → Tasks 1, 3.
- [x] `learning_fork_probe` usage source → Task 2.
- [x] `NudgeSignalSource` enum + record extension → Task 3.
- [x] Reply-field callsite tagging → Task 4.
- [x] `LearningConfig` new fields + backward-compat defaults → Task 5.
- [x] Probe schema + prompt constants → Task 6.
- [x] `learning_probe` module: gate, parse, build, run → Tasks 7, 8.
- [x] Worker spawns probe after Telegram send → Task 8.
- [x] `DrainScheduler` gated on flag → Task 9.
- [x] Bot-startup deprecation WARN → Task 10.
- [x] Wizard prompts → Task 11.
- [x] Dashboard signals_by_source widget → Task 12.
- [x] Final workspace tests → Task 13.

Open spec questions (deferred to implementation-time validation per spec):

- Fork inheritance under CC 2.1.x — verify in Task 13.3 via live-CC smoke.
- `--tools ""` interaction with `--mcp-config` — Task 8 unit test covers args; Task 13.3 verifies CC honors the empty allowlist at runtime.
- Probe wall-clock on heavy turns — defer; observe production.

No retain-tool ingestion was added; the spec correction (commits `2769822e`, `3a8b5514`, `fa14a8bc`) brought the spec back in line with the codebase. If retain-tool nudge ingestion is wanted later, it is a separate feature.

The `daily_review_count` / `daily_review_date` / `consecutive_review_failures` / `review_circuit_open_until` columns on `skill_nudge_state` are pre-existing dead (former Stage 2 gate fields) and are NOT dropped by this plan — per `CLAUDE.md` "remove only what your changes made unused". A future cleanup spec drops them together with the rest of the legacy episode tables.
