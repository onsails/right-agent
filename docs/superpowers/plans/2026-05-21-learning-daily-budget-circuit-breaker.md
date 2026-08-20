# Learning Daily Budget and Circuit Breaker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the count-based learning review gate with a per-day USD budget, add a consecutive-failure circuit breaker, surface learning costs in the usage dashboard, and emit a Telegram alert when the circuit opens.

**Architecture:** Single migration adds two columns to `skill_nudge_state`. Three new `usage_events.source` values (`learning_selector`, `learning_reviewer`, `learning_skill_review`) replace both the per-call cap and the daily count. Gate logic now sums today's learning spend from `usage_events` and consults a circuit-breaker window before allowing a new review. Failure path increments a consecutive-failure counter; success resets it.

**Tech Stack:** Rust (edition 2024), rusqlite_migration, tokio, teloxide.

**Spec:** `docs/superpowers/specs/2026-05-21-learning-daily-budget-circuit-breaker-design.md`.

---

## File Structure

### Created
- `crates/right-db/src/sql/v25_skill_nudge_circuit_breaker.sql` — doc-only marker (Rust hook is the real migration).
- `crates/bot/src/telegram/alerts.rs` — shared `should_fire` / `record_fire` dedup helpers extracted from `memory_alerts.rs`.
- `crates/bot/src/telegram/learning_alerts.rs` — `maybe_alert_circuit_open`.

### Modified
- `crates/right-db/src/migrations.rs` — register `v25_skill_nudge_circuit_breaker` hook, bump `LATEST_SCHEMA_VERSION` to 25.
- `crates/right-agent/src/usage/mod.rs` — `pub const LEARNING_SOURCES: &[&str]`.
- `crates/right-agent/src/usage/insert.rs` — `insert_learning_selector`, `insert_learning_reviewer`, `insert_learning_skill_review`.
- `crates/right-agent/src/learned_skills.rs` — gate types, gate decision, `try_mark_review_started`, `mark_review_finished*`, new `record_review_failure`.
- `crates/right-agent-config/src/lib.rs` — `LearningConfig` adds `max_daily_budget_usd`, `circuit_failure_threshold`, `circuit_cooldown_minutes`; soft-deprecates `episode_selector_max_budget_usd`.
- `crates/bot/src/learning_episode.rs` — pass new `ReviewGateInput`; call `record_review_failure` on failure; record `usage_events` on success; remove `LEARNING_EPISODE_REVIEW_DAILY_LIMIT`.
- `crates/bot/src/telegram/worker.rs` — same gate-input change for the worker skill review path; remove `LEARNED_SKILL_REVIEW_DAILY_LIMIT`; record `usage_events` on success; failure path uses `record_review_failure`.
- `crates/bot/src/telegram/memory_alerts.rs` — use shared `alerts.rs`.
- `crates/bot/src/telegram/mod.rs` — register `alerts`, `learning_alerts` modules.
- `crates/right-dashboard/src/read_model/usage.rs` — expand `SOURCES`; test the new entries.
- `crates/right-dashboard/src/read_model/dashboard_overview.rs` — if it also uses an inline source whitelist, sync it.
- `ARCHITECTURE.md` — short note on the new gate contract.

### Deleted
None. Soft-deprecation only.

---

## Task 1: Schema migration

**Files:**
- Create: `crates/right-db/src/sql/v25_skill_nudge_circuit_breaker.sql` (doc-only)
- Modify: `crates/right-db/src/migrations.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/right-db/src/migrations.rs` test module (find the existing `mod tests` block):

```rust
#[test]
fn migration_v25_adds_circuit_breaker_columns_idempotently() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 24).unwrap();
    // v24 ends without the new columns.
    let pre_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') \
             WHERE name IN ('consecutive_review_failures', 'review_circuit_open_until')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pre_count, 0, "preconditions: columns not yet present");

    MIGRATIONS.to_version(&mut conn, 25).unwrap();
    let post_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') \
             WHERE name IN ('consecutive_review_failures', 'review_circuit_open_until')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(post_count, 2, "both columns present after v25");

    // Re-running v25 is a no-op — verifies idempotency.
    MIGRATIONS.to_version(&mut conn, 25).unwrap();
    let post_count_again: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') \
             WHERE name IN ('consecutive_review_failures', 'review_circuit_open_until')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(post_count_again, 2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-db migration_v25_adds_circuit_breaker_columns_idempotently`
Expected: FAIL with "to_version(25) — no migration registered" or similar.

- [ ] **Step 3: Create the SQL file (doc only)**

Create `crates/right-db/src/sql/v25_skill_nudge_circuit_breaker.sql`:

```sql
-- v25: Add circuit-breaker columns to skill_nudge_state.
--
-- This file is a doc-only placeholder; the actual migration uses a Rust hook
-- (see `v25_skill_nudge_circuit_breaker` in migrations.rs) so that the column
-- additions can be guarded by `pragma_table_info` for idempotency. SQLite
-- has no `ADD COLUMN IF NOT EXISTS`.
--
-- ALTER TABLE skill_nudge_state ADD COLUMN consecutive_review_failures INTEGER NOT NULL DEFAULT 0;
-- ALTER TABLE skill_nudge_state ADD COLUMN review_circuit_open_until TEXT;
```

- [ ] **Step 4: Register hook + bump version**

In `crates/right-db/src/migrations.rs`:

```rust
// Near the other constant `const V25_SCHEMA` declarations:
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V25_SCHEMA: &str = include_str!("sql/v25_skill_nudge_circuit_breaker.sql");

// Replace `pub const LATEST_SCHEMA_VERSION: u32 = 24;` with:
pub const LATEST_SCHEMA_VERSION: u32 = 25;

// Add this hook function near the other vNN_* helpers:
fn v25_skill_nudge_circuit_breaker(tx: &Transaction) -> Result<(), HookError> {
    let has_column = |col: &str| -> Result<bool, rusqlite::Error> {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') WHERE name = ?1",
            [col],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    };

    if !has_column("consecutive_review_failures")? {
        tx.execute_batch(
            "ALTER TABLE skill_nudge_state ADD COLUMN consecutive_review_failures INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    if !has_column("review_circuit_open_until")? {
        tx.execute_batch("ALTER TABLE skill_nudge_state ADD COLUMN review_circuit_open_until TEXT")?;
    }
    Ok(())
}

// Inside the `Migrations::new(vec![...])` block, append after the v24 entry:
//         M::up_with_hook("", v24_learning_episodes),
//         M::up_with_hook("", v25_skill_nudge_circuit_breaker),  // ← add
//     ])
```

- [ ] **Step 5: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-db migration_v25_adds_circuit_breaker_columns_idempotently`
Expected: PASS.

- [ ] **Step 6: Run other migration tests to confirm nothing else broke**

Run: `devenv shell -- cargo test -p right-db`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/right-db/src/sql/v25_skill_nudge_circuit_breaker.sql crates/right-db/src/migrations.rs
git commit -m "feat(db): add circuit-breaker fields to skill_nudge_state"
```

---

## Task 2: `LEARNING_SOURCES` constant in `right-agent`

**Files:**
- Modify: `crates/right-agent/src/usage/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/right-agent/src/usage/mod.rs` test module (or create one if missing):

```rust
#[cfg(test)]
mod sources_const_tests {
    use super::*;

    #[test]
    fn learning_sources_contains_expected_three_entries() {
        assert_eq!(
            LEARNING_SOURCES,
            &["learning_selector", "learning_reviewer", "learning_skill_review"]
        );
    }
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `devenv shell -- cargo test -p right-agent learning_sources_contains_expected_three_entries`
Expected: FAIL with "cannot find value `LEARNING_SOURCES` in this scope".

- [ ] **Step 3: Add the constant**

In `crates/right-agent/src/usage/mod.rs`, near the top of the module, add:

```rust
/// Canonical list of `usage_events.source` values produced by the learning
/// pipeline (Stage 2 episode selector + reviewer + worker-side skill review).
///
/// Single source of truth shared between the review gate's daily-budget query
/// (`right_agent::learned_skills`) and the dashboard's `SOURCES` array
/// (`right_dashboard::read_model::usage`). New learning-adjacent sources must
/// be added here; the dashboard test asserts every entry is rendered.
pub const LEARNING_SOURCES: &[&str] = &[
    "learning_selector",
    "learning_reviewer",
    "learning_skill_review",
];
```

- [ ] **Step 4: Run test, verify PASS**

Run: `devenv shell -- cargo test -p right-agent learning_sources_contains_expected_three_entries`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/usage/mod.rs
git commit -m "feat(agent): add LEARNING_SOURCES single source of truth"
```

---

## Task 3: `insert_learning_*` helpers

**Files:**
- Modify: `crates/right-agent/src/usage/insert.rs`

- [ ] **Step 1: Write failing tests for all three helpers**

Append to the `#[cfg(test)] mod tests` block in `crates/right-agent/src/usage/insert.rs`:

```rust
#[test]
fn insert_learning_selector_writes_row_with_correct_source_and_job_name() {
    let dir = tempdir().unwrap();
    let conn = right_db::open_connection(dir.path(), true).unwrap();
    let b = sample_breakdown();
    insert_learning_selector(&conn, &b, 42).unwrap();
    let (source, job_name): (String, String) = conn
        .query_row(
            "SELECT source, job_name FROM usage_events WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(source, "learning_selector");
    assert_eq!(job_name, "42");
}

#[test]
fn insert_learning_reviewer_writes_row_with_correct_source_and_job_name() {
    let dir = tempdir().unwrap();
    let conn = right_db::open_connection(dir.path(), true).unwrap();
    let b = sample_breakdown();
    insert_learning_reviewer(&conn, &b, 99).unwrap();
    let (source, job_name): (String, String) = conn
        .query_row(
            "SELECT source, job_name FROM usage_events WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(source, "learning_reviewer");
    assert_eq!(job_name, "99");
}

#[test]
fn insert_learning_skill_review_writes_row_with_correct_source_chat_thread() {
    let dir = tempdir().unwrap();
    let conn = right_db::open_connection(dir.path(), true).unwrap();
    let b = sample_breakdown();
    insert_learning_skill_review(&conn, &b, -1001, 7).unwrap();
    let (source, chat_id, thread_id): (String, i64, i64) = conn
        .query_row(
            "SELECT source, chat_id, thread_id FROM usage_events WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(source, "learning_skill_review");
    assert_eq!(chat_id, -1001);
    assert_eq!(thread_id, 7);
}
```

- [ ] **Step 2: Run, verify FAIL**

Run: `devenv shell -- cargo test -p right-agent insert_learning_`
Expected: FAIL (functions not defined).

- [ ] **Step 3: Implement the three helpers**

Add to `crates/right-agent/src/usage/insert.rs` (below the existing `insert_reflection_cron`):

```rust
/// Insert a row for a Stage 2 learning-episode selector invocation.
/// `episode_id` is stored in the `job_name` column so the dashboard can link
/// the usage row back to the episode detail view without a separate column.
pub fn insert_learning_selector(
    conn: &Connection,
    b: &UsageBreakdown,
    episode_id: i64,
) -> Result<(), UsageError> {
    let job = episode_id.to_string();
    insert_row(conn, b, "learning_selector", None, None, Some(job.as_str()))
}

/// Insert a row for a Stage 2 learning-episode reviewer invocation.
/// `episode_id` is stored in `job_name` (see `insert_learning_selector`).
pub fn insert_learning_reviewer(
    conn: &Connection,
    b: &UsageBreakdown,
    episode_id: i64,
) -> Result<(), UsageError> {
    let job = episode_id.to_string();
    insert_row(conn, b, "learning_reviewer", None, None, Some(job.as_str()))
}

/// Insert a row for a worker-side learned-skill review invocation.
pub fn insert_learning_skill_review(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(
        conn,
        b,
        "learning_skill_review",
        Some(chat_id),
        Some(thread_id),
        None,
    )
}
```

- [ ] **Step 4: Run, verify PASS**

Run: `devenv shell -- cargo test -p right-agent insert_learning_`
Expected: PASS (all three tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/usage/insert.rs
git commit -m "feat(agent): record learning costs in usage_events"
```

---

## Task 4: `LearningConfig` new fields with soft deprecation

**Files:**
- Modify: `crates/right-agent-config/src/lib.rs`

- [ ] **Step 1: Write failing tests for defaults and deprecation log**

Add to `crates/right-agent-config/src/lib.rs` test module (find the existing `#[cfg(test)] mod tests` block; if there isn't one near `LearningConfig`, create one):

```rust
#[cfg(test)]
mod learning_config_tests {
    use super::*;

    #[test]
    fn learning_config_default_max_daily_budget_is_five_dollars() {
        let cfg = LearningConfig::default();
        assert!((cfg.max_daily_budget_usd - 5.00).abs() < f64::EPSILON);
    }

    #[test]
    fn learning_config_default_circuit_breaker_knobs() {
        let cfg = LearningConfig::default();
        assert_eq!(cfg.circuit_failure_threshold, 5);
        assert_eq!(cfg.circuit_cooldown_minutes, 60);
    }

    #[test]
    fn learning_config_accepts_yaml_with_overrides() {
        let yaml = r#"
max_daily_budget_usd: 12.5
circuit_failure_threshold: 8
circuit_cooldown_minutes: 30
"#;
        let cfg: LearningConfig = serde_yaml::from_str(yaml).unwrap();
        assert!((cfg.max_daily_budget_usd - 12.5).abs() < f64::EPSILON);
        assert_eq!(cfg.circuit_failure_threshold, 8);
        assert_eq!(cfg.circuit_cooldown_minutes, 30);
    }

    #[test]
    fn learning_config_accepts_legacy_episode_selector_max_budget_usd_without_error() {
        // Soft deprecation: old yaml keeps loading.
        let yaml = "episode_selector_max_budget_usd: 0.10\n";
        let cfg: LearningConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.episode_selector_max_budget_usd, Some(0.10));
        // New fields take their default.
        assert!((cfg.max_daily_budget_usd - 5.00).abs() < f64::EPSILON);
    }

    #[test]
    fn learning_config_rejects_zero_or_negative_daily_budget() {
        let bad = r#"max_daily_budget_usd: 0.0"#;
        assert!(serde_yaml::from_str::<LearningConfig>(bad).is_err());
        let bad = r#"max_daily_budget_usd: -1.0"#;
        assert!(serde_yaml::from_str::<LearningConfig>(bad).is_err());
    }

    #[test]
    fn learning_config_rejects_zero_failure_threshold() {
        let bad = r#"circuit_failure_threshold: 0"#;
        assert!(serde_yaml::from_str::<LearningConfig>(bad).is_err());
    }
}
```

- [ ] **Step 2: Run, verify FAIL**

Run: `devenv shell -- cargo test -p right-agent-config learning_config_`
Expected: FAIL — new fields don't exist yet.

- [ ] **Step 3: Update `LearningConfig`**

In `crates/right-agent-config/src/lib.rs`:

```rust
fn default_max_daily_budget_usd() -> f64 {
    5.00
}

fn default_circuit_failure_threshold() -> u32 {
    5
}

fn default_circuit_cooldown_minutes() -> u32 {
    60
}

fn deserialize_positive_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    if value > 0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom("value must be greater than 0"))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LearningConfig {
    /// Optional selector model. None means inherit the agent model.
    pub episode_selector_model: Option<String>,

    /// Soft-deprecated. Kept for backward-compatibility with existing
    /// agent.yaml files. Not read by any code. A warn-log is emitted at
    /// agent load time when present (see `LearningConfig::warn_on_deprecated`).
    /// Slated for removal in a future release.
    pub episode_selector_max_budget_usd: Option<f64>,

    /// Delay after seed evidence before selecting the episode boundary.
    #[serde(
        default = "default_episode_settle_seconds",
        deserialize_with = "deserialize_positive_u64"
    )]
    pub episode_settle_seconds: u64,

    /// Daily $ budget across all learning invocations (selector + reviewer +
    /// skill review). Replaces the previous per-call cap + per-day count.
    #[serde(
        default = "default_max_daily_budget_usd",
        deserialize_with = "deserialize_positive_finite_f64"
    )]
    pub max_daily_budget_usd: f64,

    /// Consecutive failures that trip the circuit breaker.
    #[serde(
        default = "default_circuit_failure_threshold",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub circuit_failure_threshold: u32,

    /// How long the circuit stays open after tripping (minutes).
    #[serde(
        default = "default_circuit_cooldown_minutes",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub circuit_cooldown_minutes: u32,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            episode_selector_model: None,
            episode_selector_max_budget_usd: None,
            episode_settle_seconds: default_episode_settle_seconds(),
            max_daily_budget_usd: default_max_daily_budget_usd(),
            circuit_failure_threshold: default_circuit_failure_threshold(),
            circuit_cooldown_minutes: default_circuit_cooldown_minutes(),
        }
    }
}

impl LearningConfig {
    /// Emit a warning log if a deprecated field is set in `agent.yaml`.
    /// Call at agent load time.
    pub fn warn_on_deprecated(&self, agent_name: &str) {
        if let Some(value) = self.episode_selector_max_budget_usd {
            tracing::warn!(
                agent = %agent_name,
                value,
                "agent.yaml: `episode_selector_max_budget_usd` is deprecated and ignored; \
                 use `max_daily_budget_usd` instead. The deprecated field will be removed \
                 in a future release."
            );
        }
    }
}

// Remove `default_episode_selector_max_budget_usd` (no longer used).
// Remove the line `pub episode_selector_max_budget_usd: f64` from the prior struct.
```

- [ ] **Step 4: Add a `serde_yaml` dev-dep if missing**

Run: `devenv shell -- cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "right-agent-config") | .dependencies[] | .name' | grep -q '^serde_yaml$' && echo OK || echo NEED_DEV_DEP`

If `NEED_DEV_DEP`, add to `crates/right-agent-config/Cargo.toml` under `[dev-dependencies]`:

```toml
serde_yaml = "0.9"
```

Then run `devenv shell -- cargo build -p right-agent-config --tests`.

- [ ] **Step 5: Run tests, verify PASS**

Run: `devenv shell -- cargo test -p right-agent-config learning_config_`
Expected: PASS (all 6 tests).

- [ ] **Step 6: Find the agent-load call site and call `warn_on_deprecated`**

Run: `grep -rn "LearningConfig" crates/right-agent/src/agent/ crates/bot/src/ 2>/dev/null | grep -v test | head`

Find where the per-agent config is loaded from `agent.yaml`. Add a call to `cfg.learning.warn_on_deprecated(&agent_name)` right after the load. The exact call site depends on the current agent-load helper (likely `crates/right-agent/src/agent/loader.rs` or similar — verify before editing).

- [ ] **Step 7: Run workspace check**

Run: `devenv shell -- cargo check --workspace`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/right-agent-config/src/lib.rs crates/right-agent-config/Cargo.toml crates/right-agent/src/
git commit -m "feat(config): max_daily_budget_usd and circuit knobs in LearningConfig"
```

---

## Task 5: `ReviewGateInput` and `ReviewGateDecision` type changes

**Files:**
- Modify: `crates/right-agent/src/learned_skills.rs`

- [ ] **Step 1: Update the test helper to use the new shape**

Find `fn review_gate_input` near the bottom of `crates/right-agent/src/learned_skills.rs` and replace:

```rust
fn review_gate_input(signal_trigger: Option<ReviewTriggerKind>) -> ReviewGateInput<'static> {
    ReviewGateInput {
        signal_trigger,
        now_utc: "2026-05-18T12:00:00Z",
        daily_budget_usd: 5.00,
    }
}
```

This file currently has `today: "2026-05-18"` and `daily_limit: 12`. Both go away.

- [ ] **Step 2: Update `ReviewGateInput`**

Find `pub struct ReviewGateInput<'a>` and replace its body:

```rust
pub struct ReviewGateInput<'a> {
    pub signal_trigger: Option<ReviewTriggerKind>,
    /// Current UTC time in RFC3339 strict format (e.g. "2026-05-21T03:14:15Z").
    /// Used for the daily-budget date filter and the circuit-window comparison.
    pub now_utc: &'a str,
    pub daily_budget_usd: f64,
}
```

- [ ] **Step 3: Update `ReviewGateDecision`**

Find `pub enum ReviewSkipReason` and replace:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewSkipReason {
    AlreadyRunning,
    CircuitOpen,
    DailyBudget,
    BelowThreshold,
}
```

(`DailyLimit` is removed.)

- [ ] **Step 4: Run a build to surface every break**

Run: `devenv shell -- cargo check --workspace 2>&1 | head -80`
Expected: Multiple errors at `learning_episode.rs`, `worker.rs`, and any test that still passes `daily_limit` / `today`. Note the file:line:col locations.

- [ ] **Step 5: Commit the type rename without fixing callers yet**

The intermediate state does not build. Stash and continue, or batch into the next task. We pick the latter — proceed to Task 6 without committing.

---

## Task 6: Rewrite `review_gate_decision_in_tx`

**Files:**
- Modify: `crates/right-agent/src/learned_skills.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/right-agent/src/learned_skills.rs` test module:

```rust
fn insert_usage(conn: &rusqlite::Connection, ts: &str, source: &str, cost: f64) {
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name, session_uuid,
            total_cost_usd, num_turns, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, web_search_requests,
            web_fetch_requests, model_usage_json, api_key_source
         ) VALUES (?1, ?2, NULL, NULL, NULL, 's', ?3, 1, 0, 0, 0, 0, 0, 0, '{}', 'none')",
        rusqlite::params![ts, source, cost],
    )
    .unwrap();
}

fn ensure_agent_nudge_state(conn: &rusqlite::Connection, agent: &str) {
    let tx = conn.unchecked_transaction().unwrap();
    ensure_nudge_state(&tx, agent).unwrap();
    tx.commit().unwrap();
}

#[test]
fn gate_skips_when_daily_budget_exceeded() {
    let conn = conn();
    ensure_agent_nudge_state(&conn, "him");
    // 5.50 spent across today's learning sources — over $5 budget.
    insert_usage(&conn, "2026-05-21T01:00:00Z", "learning_selector", 2.50);
    insert_usage(&conn, "2026-05-21T02:00:00Z", "learning_reviewer", 3.00);

    let input = ReviewGateInput {
        signal_trigger: Some(ReviewTriggerKind::EffortThreshold),
        now_utc: "2026-05-21T03:00:00Z",
        daily_budget_usd: 5.00,
    };
    let decision = try_mark_review_started(&conn, "him", input).unwrap();
    assert_eq!(
        decision,
        ReviewGateDecision::Skip(ReviewSkipReason::DailyBudget)
    );
}

#[test]
fn gate_ignores_non_learning_sources_and_yesterdays_spend() {
    let conn = conn();
    ensure_agent_nudge_state(&conn, "him");
    // Yesterday — must be ignored.
    insert_usage(&conn, "2026-05-20T23:59:00Z", "learning_selector", 10.00);
    // Non-learning source today — must be ignored.
    insert_usage(&conn, "2026-05-21T01:00:00Z", "interactive", 10.00);
    // Today, learning, under budget.
    insert_usage(&conn, "2026-05-21T02:00:00Z", "learning_selector", 1.00);

    let input = ReviewGateInput {
        signal_trigger: Some(ReviewTriggerKind::EffortThreshold),
        now_utc: "2026-05-21T03:00:00Z",
        daily_budget_usd: 5.00,
    };
    let decision = try_mark_review_started(&conn, "him", input).unwrap();
    assert!(matches!(decision, ReviewGateDecision::Start(_)));
}

#[test]
fn gate_skips_when_circuit_open() {
    let conn = conn();
    ensure_agent_nudge_state(&conn, "him");
    conn.execute(
        "UPDATE skill_nudge_state SET review_circuit_open_until = ?1 WHERE agent_name = 'him'",
        ["2026-05-21T04:00:00Z"],
    )
    .unwrap();

    let input = ReviewGateInput {
        signal_trigger: Some(ReviewTriggerKind::EffortThreshold),
        now_utc: "2026-05-21T03:00:00Z",
        daily_budget_usd: 5.00,
    };
    let decision = try_mark_review_started(&conn, "him", input).unwrap();
    assert_eq!(
        decision,
        ReviewGateDecision::Skip(ReviewSkipReason::CircuitOpen)
    );
}

#[test]
fn gate_clears_expired_circuit_and_resets_failure_count() {
    let conn = conn();
    ensure_agent_nudge_state(&conn, "him");
    conn.execute(
        "UPDATE skill_nudge_state SET \
            review_circuit_open_until = '2026-05-21T02:30:00Z', \
            consecutive_review_failures = 5 \
         WHERE agent_name = 'him'",
        [],
    )
    .unwrap();

    let input = ReviewGateInput {
        signal_trigger: Some(ReviewTriggerKind::EffortThreshold),
        now_utc: "2026-05-21T03:00:00Z",
        daily_budget_usd: 5.00,
    };
    let decision = try_mark_review_started(&conn, "him", input).unwrap();
    assert!(matches!(decision, ReviewGateDecision::Start(_)));

    let (open_until, count): (Option<String>, i64) = conn
        .query_row(
            "SELECT review_circuit_open_until, consecutive_review_failures \
             FROM skill_nudge_state WHERE agent_name = 'him'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(open_until, None);
    assert_eq!(count, 0);
}
```

- [ ] **Step 2: Run, verify FAIL**

Run: `devenv shell -- cargo test -p right-agent gate_ -- --include-ignored`
Expected: FAIL — old gate still uses `daily_review_count`.

- [ ] **Step 3: Rewrite `review_gate_decision_in_tx`**

In `crates/right-agent/src/learned_skills.rs`, replace the `review_gate_decision_in_tx` function and the `try_mark_review_started` body:

```rust
fn review_gate_decision_in_tx(
    tx: &rusqlite::Transaction<'_>,
    agent_name: &str,
    input: ReviewGateInput<'_>,
) -> Result<ReviewGateDecision, rusqlite::Error> {
    let (review_running, circuit_open_until, tool_iters, interval): (i64, Option<String>, i64, i64) =
        tx.query_row(
            "SELECT review_running, review_circuit_open_until, \
                    tool_iters_since_review, creation_review_interval \
             FROM skill_nudge_state WHERE agent_name = ?1",
            [agent_name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;

    if review_running != 0 {
        return Ok(ReviewGateDecision::Skip(ReviewSkipReason::AlreadyRunning));
    }
    if let Some(until) = circuit_open_until {
        if until.as_str() > input.now_utc {
            return Ok(ReviewGateDecision::Skip(ReviewSkipReason::CircuitOpen));
        }
        // Window expired. Clear both fields so the next attempt has a fresh
        // failure budget. Otherwise consecutive_review_failures stays elevated
        // and the next failure immediately reopens the circuit.
        tx.execute(
            "UPDATE skill_nudge_state SET \
                review_circuit_open_until = NULL, \
                consecutive_review_failures = 0 \
             WHERE agent_name = ?1",
            [agent_name],
        )?;
    }

    // Daily budget check: SUM(total_cost_usd) for today UTC across learning sources.
    // Build placeholder list dynamically from `crate::usage::LEARNING_SOURCES`.
    let today_start = format!("{}T00:00:00Z", &input.now_utc[..10]);
    let placeholders = (0..crate::usage::LEARNING_SOURCES.len())
        .map(|i| format!("?{}", i + 2))
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT COALESCE(SUM(total_cost_usd), 0.0) FROM usage_events \
         WHERE ts >= ?1 AND source IN ({placeholders})"
    );
    let mut stmt = tx.prepare(&query)?;
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&today_start];
    for source in crate::usage::LEARNING_SOURCES {
        params.push(source);
    }
    let spent: f64 = stmt.query_row(params.as_slice(), |r| r.get(0))?;
    if spent >= input.daily_budget_usd {
        return Ok(ReviewGateDecision::Skip(ReviewSkipReason::DailyBudget));
    }

    if let Some(trigger) = input.signal_trigger {
        return Ok(ReviewGateDecision::Start(trigger));
    }
    if interval > 0 && tool_iters >= interval {
        return Ok(ReviewGateDecision::Start(
            ReviewTriggerKind::EffortThreshold,
        ));
    }
    Ok(ReviewGateDecision::Skip(ReviewSkipReason::BelowThreshold))
}

pub fn try_mark_review_started(
    conn: &rusqlite::Connection,
    agent_name: &str,
    input: ReviewGateInput<'_>,
) -> Result<ReviewGateDecision, rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    ensure_nudge_state(&tx, agent_name)?;

    let decision = review_gate_decision_in_tx(&tx, agent_name, input)?;
    let ReviewGateDecision::Start(trigger) = decision else {
        tx.commit()?;
        return Ok(decision);
    };

    // Only flip `review_running`. The daily-budget guard is enforced by the
    // SUM query above — no counter to increment.
    let updated = tx.execute(
        "UPDATE skill_nudge_state \
         SET review_running = 1 \
         WHERE agent_name = ?1 AND review_running = 0",
        [agent_name],
    )?;
    if updated == 1 {
        tx.commit()?;
        return Ok(ReviewGateDecision::Start(trigger));
    }

    // Lost the race — another caller marked it running. Re-read.
    let decision = review_gate_decision_in_tx(&tx, agent_name, input)?;
    tx.commit()?;
    Ok(decision)
}
```

Remove the helpers `roll_over_daily_count_if_stale` (and any tests that reference it).

- [ ] **Step 4: Run, verify PASS**

Run: `devenv shell -- cargo test -p right-agent gate_`
Expected: PASS (all four tests).

- [ ] **Step 5: Commit (still no callers fixed; intermediate state OK in right-agent)**

```bash
git add crates/right-agent/src/learned_skills.rs
git commit -m "feat(agent): daily-budget gate with circuit-open skip"
```

---

## Task 7: `mark_review_finished` resets circuit state

**Files:**
- Modify: `crates/right-agent/src/learned_skills.rs`

- [ ] **Step 1: Write the failing test**

Add to test module:

```rust
#[test]
fn mark_review_finished_resets_circuit_and_failures() {
    let conn = conn();
    ensure_agent_nudge_state(&conn, "him");
    conn.execute(
        "UPDATE skill_nudge_state SET \
            review_running = 1, \
            consecutive_review_failures = 4, \
            review_circuit_open_until = '2026-05-21T05:00:00Z' \
         WHERE agent_name = 'him'",
        [],
    )
    .unwrap();

    mark_review_finished(
        &conn,
        "him",
        ReviewTriggerKind::EffortThreshold,
        ReviewStatus::NothingToLearn,
        false,
    )
    .unwrap();

    let (running, failures, open_until): (i64, i64, Option<String>) = conn
        .query_row(
            "SELECT review_running, consecutive_review_failures, review_circuit_open_until \
             FROM skill_nudge_state WHERE agent_name = 'him'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(running, 0);
    assert_eq!(failures, 0);
    assert_eq!(open_until, None);
}
```

- [ ] **Step 2: Run, verify FAIL**

Run: `devenv shell -- cargo test -p right-agent mark_review_finished_resets_circuit_and_failures`
Expected: FAIL — failure count not reset.

- [ ] **Step 3: Update `mark_review_finished_in_tx`**

In `crates/right-agent/src/learned_skills.rs`, find the `UPDATE skill_nudge_state SET review_running = 0 ...` statement inside `mark_review_finished_in_tx` and extend the SET list:

```rust
tx.execute(
    "UPDATE skill_nudge_state \
     SET review_running = 0, \
         tool_iters_since_review = CASE WHEN ?3 THEN 0 ELSE tool_iters_since_review END, \
         turns_since_review = CASE WHEN ?3 THEN 0 ELSE turns_since_review END, \
         skill_issue_hints_since_review = CASE WHEN ?4 THEN 0 ELSE skill_issue_hints_since_review END, \
         last_review_at = strftime('%Y-%m-%dT%H:%M:%SZ','now'), \
         last_review_status = ?2, \
         consecutive_review_failures = 0, \
         review_circuit_open_until = NULL \
     WHERE agent_name = ?1",
    rusqlite::params![
        agent_name,
        status.as_str(),
        if reset_activity_counters { 1_i64 } else { 0_i64 },
        if reset_issue_hints { 1_i64 } else { 0_i64 },
    ],
)?;
```

- [ ] **Step 4: Run, verify PASS**

Run: `devenv shell -- cargo test -p right-agent mark_review_finished_resets_circuit_and_failures`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/learned_skills.rs
git commit -m "feat(agent): reset circuit state on review success"
```

---

## Task 8: `record_review_failure` helper

**Files:**
- Modify: `crates/right-agent/src/learned_skills.rs`

- [ ] **Step 1: Write the failing tests**

Add to test module:

```rust
#[test]
fn record_review_failure_increments_and_returns_opened_false_below_threshold() {
    let conn = conn();
    ensure_agent_nudge_state(&conn, "him");
    conn.execute(
        "UPDATE skill_nudge_state SET review_running = 1, consecutive_review_failures = 2 WHERE agent_name = 'him'",
        [],
    )
    .unwrap();

    let (count, opened) = record_review_failure(
        &conn,
        "him",
        "2026-05-21T03:00:00Z",
        5,
        60,
    )
    .unwrap();
    assert_eq!(count, 3);
    assert!(!opened);

    let (running, open_until): (i64, Option<String>) = conn
        .query_row(
            "SELECT review_running, review_circuit_open_until FROM skill_nudge_state WHERE agent_name = 'him'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(running, 0);
    assert_eq!(open_until, None);
}

#[test]
fn record_review_failure_opens_circuit_exactly_at_threshold() {
    let conn = conn();
    ensure_agent_nudge_state(&conn, "him");
    conn.execute(
        "UPDATE skill_nudge_state SET review_running = 1, consecutive_review_failures = 4 WHERE agent_name = 'him'",
        [],
    )
    .unwrap();

    let (count, opened) = record_review_failure(
        &conn,
        "him",
        "2026-05-21T03:00:00Z",
        5,
        60,
    )
    .unwrap();
    assert_eq!(count, 5);
    assert!(opened);

    let open_until: Option<String> = conn
        .query_row(
            "SELECT review_circuit_open_until FROM skill_nudge_state WHERE agent_name = 'him'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(open_until.as_deref(), Some("2026-05-21T04:00:00Z"));
}

#[test]
fn record_review_failure_does_not_reopen_already_open_circuit() {
    let conn = conn();
    ensure_agent_nudge_state(&conn, "him");
    conn.execute(
        "UPDATE skill_nudge_state SET \
            review_running = 1, \
            consecutive_review_failures = 7, \
            review_circuit_open_until = '2026-05-21T05:00:00Z' \
         WHERE agent_name = 'him'",
        [],
    )
    .unwrap();

    let (count, opened) = record_review_failure(
        &conn,
        "him",
        "2026-05-21T03:00:00Z",
        5,
        60,
    )
    .unwrap();
    assert_eq!(count, 8);
    assert!(!opened); // already open — no transition.

    let open_until: Option<String> = conn
        .query_row(
            "SELECT review_circuit_open_until FROM skill_nudge_state WHERE agent_name = 'him'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Open-until unchanged.
    assert_eq!(open_until.as_deref(), Some("2026-05-21T05:00:00Z"));
}
```

- [ ] **Step 2: Run, verify FAIL**

Run: `devenv shell -- cargo test -p right-agent record_review_failure_`
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement `record_review_failure`**

Add to `crates/right-agent/src/learned_skills.rs` (near `clear_review_running`):

```rust
/// Record a learning review failure: increment `consecutive_review_failures`,
/// open the circuit if the threshold is reached. Atomic.
///
/// Returns `(new_failure_count, opened_circuit_now)`:
/// - `new_failure_count` is the updated counter value.
/// - `opened_circuit_now` is `true` iff this call transitioned the circuit
///   from closed to open. Useful for callers that need to emit a one-shot
///   Telegram alert without re-alerting on every subsequent failure while
///   the circuit stays open.
///
/// `now_utc` must be RFC3339 strict (e.g. "2026-05-21T03:14:15Z").
pub fn record_review_failure(
    conn: &rusqlite::Connection,
    agent_name: &str,
    now_utc: &str,
    threshold: u32,
    cooldown_minutes: u32,
) -> Result<(i64, bool), rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    ensure_nudge_state(&tx, agent_name)?;

    let (prev_count, prev_open_until): (i64, Option<String>) = tx.query_row(
        "SELECT consecutive_review_failures, review_circuit_open_until \
         FROM skill_nudge_state WHERE agent_name = ?1",
        [agent_name],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    let new_count = prev_count + 1;
    let circuit_already_open = prev_open_until
        .as_deref()
        .map(|s| s > now_utc)
        .unwrap_or(false);
    let should_open = new_count >= i64::from(threshold) && !circuit_already_open;
    let opened_now = should_open;

    let new_open_until: Option<String> = if should_open {
        // Compute now_utc + cooldown_minutes in Rust to avoid SQLite datetime
        // round-trips that drop the 'Z' suffix.
        let parsed = chrono::DateTime::parse_from_rfc3339(now_utc).map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid now_utc {now_utc:?}: {e}"),
            )))
        })?;
        let until = parsed
            .with_timezone(&chrono::Utc)
            + chrono::Duration::minutes(i64::from(cooldown_minutes));
        Some(until.format("%Y-%m-%dT%H:%M:%SZ").to_string())
    } else {
        prev_open_until
    };

    tx.execute(
        "UPDATE skill_nudge_state SET \
            review_running = 0, \
            consecutive_review_failures = ?2, \
            review_circuit_open_until = ?3 \
         WHERE agent_name = ?1",
        rusqlite::params![agent_name, new_count, new_open_until],
    )?;
    tx.commit()?;
    Ok((new_count, opened_now))
}
```

- [ ] **Step 4: Run, verify PASS**

Run: `devenv shell -- cargo test -p right-agent record_review_failure_`
Expected: PASS (all three tests).

- [ ] **Step 5: Run all right-agent tests to confirm nothing else broke**

Run: `devenv shell -- cargo test -p right-agent`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/learned_skills.rs
git commit -m "feat(agent): record_review_failure with circuit breaker"
```

---

## Task 9: Wire `bot/learning_episode.rs` to the new gate + usage recording

**Files:**
- Modify: `crates/bot/src/learning_episode.rs`

- [ ] **Step 1: Update the `try_mark_review_started` call site**

In `drain_ready_learning_episodes_once_with_selector_and_reviewer` (around line 604), replace:

```rust
let gate = match try_mark_review_started(
    &conn,
    &runtime.agent_name,
    ReviewGateInput {
        signal_trigger: review_trigger_for_episode(episode.seed_trigger_kind),
        today: &today,
        daily_limit: LEARNING_EPISODE_REVIEW_DAILY_LIMIT,
    },
)
```

with:

```rust
let gate = match try_mark_review_started(
    &conn,
    &runtime.agent_name,
    ReviewGateInput {
        signal_trigger: review_trigger_for_episode(episode.seed_trigger_kind),
        now_utc: &now_str,
        daily_budget_usd: runtime.learning.max_daily_budget_usd,
    },
)
```

Update the `match gate` arms — the `Skip(DailyLimit)` arm becomes:

```rust
ReviewGateDecision::Skip(
    ReviewSkipReason::AlreadyRunning
    | ReviewSkipReason::DailyBudget
    | ReviewSkipReason::CircuitOpen,
) => {
    requeue_episode_or_fail(
        &conn,
        &runtime.agent_name,
        episode.id,
        now,
        runtime.learning.episode_settle_seconds,
    )?;
    runtime.schedule_drain();
    return Ok(());
}
```

- [ ] **Step 2: Remove `LEARNING_EPISODE_REVIEW_DAILY_LIMIT`**

Find and delete the line:

```rust
const LEARNING_EPISODE_REVIEW_DAILY_LIMIT: i64 = 12;
```

at the top of `crates/bot/src/learning_episode.rs`.

- [ ] **Step 3: Update the failure path to use `record_review_failure`**

In `mark_claimed_episode_failed`, the current body calls `clear_review_running`. Replace with a call to `record_review_failure` and return its tuple to the caller. New signature:

```rust
fn mark_claimed_episode_failed(
    conn: &rusqlite::Connection,
    runtime: &LearningEpisodeRuntime,
    episode_id: i64,
    reason: &str,
    now_utc: &str,
    record_failure: bool,
) -> anyhow::Result<Option<(i64, bool)>> {
    let tx = conn.unchecked_transaction()?;
    right_agent::learning_episodes::mark_episode_failed(&tx, episode_id, reason)
        .with_context(|| format!("mark learning episode {episode_id} failed"))?;
    let outcome = if record_failure {
        let (count, opened) = right_agent::learned_skills::record_review_failure(
            &tx,
            &runtime.agent_name,
            now_utc,
            runtime.learning.circuit_failure_threshold,
            runtime.learning.circuit_cooldown_minutes,
        )
        .with_context(|| format!("record_review_failure for {}", runtime.agent_name))?;
        Some((count, opened))
    } else {
        None
    };
    tx.commit()?;
    Ok(outcome)
}
```

Update all callers in this file to pass `&runtime` instead of `&runtime.agent_name`, plus `&now_str`, and propagate the returned tuple. The `clear_gate: bool` boolean becomes `record_failure: bool` — same set of call sites.

- [ ] **Step 4: Record `usage_events` on successful selector / reviewer**

The selector currently parses `stdout` for the structured output. Extract the cost portion too. After `parse_selector_process_stdout(&stdout)` returns Ok, also parse a `UsageBreakdown` from the same JSON envelope.

Add a helper `extract_usage_from_cc_json(stdout: &str, source_session_uuid: &str) -> Option<UsageBreakdown>` to `crates/bot/src/cc/mod.rs` if not present (check first; the bot likely already has a similar helper used by the worker — reuse it).

Then in `run_episode_selector` after success:

```rust
if let Some(breakdown) = extract_usage_from_cc_json(&stdout, &session_uuid) {
    if let Err(e) = right_agent::usage::insert::insert_learning_selector(
        &conn,
        &breakdown,
        episode.id,
    ) {
        tracing::warn!(episode_id = episode.id, "insert_learning_selector failed: {e:#}");
    }
}
```

Mirror for `run_episode_review_invocation`: call `insert_learning_reviewer`.

Note: if `extract_usage_from_cc_json` does not exist, the existing CC final-JSON parser must be located (search for `total_cost_usd` in `crates/bot/src/cc/` and `crates/right-agent/src/usage/`). The breakdown produced for `interactive` usage is the model.

- [ ] **Step 5: Build, verify PASS**

Run: `devenv shell -- cargo build -p right-bot`
Expected: success.

- [ ] **Step 6: Run the bot's learning-episode tests**

Run: `devenv shell -- cargo test -p right-bot learning_episode`
Expected: PASS. Update any test that constructed the old `ReviewGateInput` shape.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/learning_episode.rs crates/bot/src/cc/
git commit -m "feat(bot): wire learning-episode drain to daily-budget gate and usage events"
```

---

## Task 10: Wire `bot/telegram/worker.rs` to the new gate + usage recording

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Update the `try_mark_review_started` call site**

In `crates/bot/src/telegram/worker.rs:2373`, replace:

```rust
let gate = match try_mark_review_started(
    conn,
    &ctx.agent_name,
    ReviewGateInput {
        signal_trigger,
        today: &today,
        daily_limit: LEARNED_SKILL_REVIEW_DAILY_LIMIT,
    },
)
```

with:

```rust
let now_utc = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
let gate = match try_mark_review_started(
    conn,
    &ctx.agent_name,
    ReviewGateInput {
        signal_trigger,
        now_utc: &now_utc,
        daily_budget_usd: ctx.learning.max_daily_budget_usd,
    },
)
```

If `ctx.learning` is not currently plumbed through the worker context, add it. Track this via a quick grep: `grep -n "learning" crates/bot/src/telegram/worker.rs | head`.

- [ ] **Step 2: Remove the constant**

Find and delete:

```rust
const LEARNED_SKILL_REVIEW_DAILY_LIMIT: i64 = 12;
```

- [ ] **Step 3: Update the worker's failure path**

Find the `clear_background_review_gate_on_shutdown` callsite at `worker.rs:2424`. There is also a finalization path where the review returns Err. In both error paths, call `record_review_failure` instead of `clear_review_running`. Pseudocode:

```rust
// On review error:
match record_review_failure(
    &db_conn,
    &agent_name,
    &chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    ctx.learning.circuit_failure_threshold,
    ctx.learning.circuit_cooldown_minutes,
) {
    Ok((count, opened)) => {
        if opened {
            // Task 13 will add the alert call here.
        }
    }
    Err(e) => tracing::warn!("record_review_failure failed: {e:#}"),
}
```

Keep `clear_background_review_gate_on_shutdown` calling `clear_review_running` (it covers the shutdown-during-review case, where no failure actually happened).

- [ ] **Step 4: Record `usage_events` on successful skill review**

In `run_background_learned_skill_review`, after CC returns Ok with a UsageBreakdown, insert via `insert_learning_skill_review(&conn, &breakdown, chat_id, eff_thread_id)`. Reuse the same `extract_usage_from_cc_json` helper from Task 9.

- [ ] **Step 5: Build, verify PASS**

Run: `devenv shell -- cargo build -p right-bot`
Expected: success.

- [ ] **Step 6: Run the worker tests**

Run: `devenv shell -- cargo test -p right-bot telegram::worker`
Expected: PASS. Update old `ReviewGateInput` constructions.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): wire worker skill review to daily-budget gate and usage events"
```

---

## Task 11: Split dedup helpers into `alerts.rs`

**Files:**
- Create: `crates/bot/src/telegram/alerts.rs`
- Modify: `crates/bot/src/telegram/memory_alerts.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Write a test asserting the shared helpers work**

Create `crates/bot/src/telegram/alerts.rs`:

```rust
//! Shared Telegram alert dedup helpers. Both `memory_alerts` and
//! `learning_alerts` use these to enforce a 24-hour dedup window per
//! `alert_type` key against the `memory_alerts` SQLite table.

use std::path::Path;

use chrono::Utc;

/// Returns true iff an alert of this type has NOT been sent in the last 24
/// hours (i.e., it is safe to send).
pub(crate) fn should_fire(db: &Path, alert_type: &str) -> bool {
    let conn = match right_db::open_connection(db, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("alerts::should_fire open failed: {e:#}");
            return false;
        }
    };
    let existing: Option<String> = match conn.query_row(
        "SELECT first_sent_at FROM memory_alerts WHERE alert_type = ?1",
        [alert_type],
        |r| r.get(0),
    ) {
        Ok(v) => Some(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => {
            tracing::warn!("alerts::should_fire query failed: {e:#}");
            return false;
        }
    };
    let Some(sent) = existing else {
        return true;
    };
    let parsed = match chrono::DateTime::parse_from_rfc3339(&sent) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("alerts::should_fire parse failed: {e:#}");
            return true;
        }
    };
    Utc::now().signed_duration_since(parsed.with_timezone(&Utc)) > chrono::Duration::hours(24)
}

/// Record that an alert of this type was sent. Idempotent via
/// `ON CONFLICT DO UPDATE`.
pub(crate) fn record_fire(db: &Path, alert_type: &str) {
    match right_db::open_connection(db, false) {
        Ok(conn) => {
            let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            if let Err(e) = conn.execute(
                "INSERT INTO memory_alerts(alert_type, first_sent_at) VALUES (?1, ?2) \
                 ON CONFLICT(alert_type) DO UPDATE SET first_sent_at = excluded.first_sent_at",
                [alert_type, &now],
            ) {
                tracing::warn!("alerts::record_fire failed: {e:#}");
            }
        }
        Err(e) => tracing::warn!("alerts::record_fire open failed: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn should_fire_true_when_no_row_exists() {
        let dir = tempdir().unwrap();
        let _ = right_db::open_connection(dir.path(), true).unwrap();
        assert!(should_fire(dir.path(), "test_type"));
    }

    #[test]
    fn record_fire_then_should_fire_is_false() {
        let dir = tempdir().unwrap();
        let _ = right_db::open_connection(dir.path(), true).unwrap();
        record_fire(dir.path(), "test_type");
        assert!(!should_fire(dir.path(), "test_type"));
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/bot/src/telegram/mod.rs`, add:

```rust
pub(crate) mod alerts;
```

- [ ] **Step 3: Switch `memory_alerts.rs` to the shared helpers**

In `crates/bot/src/telegram/memory_alerts.rs`, delete the private `fn should_fire` and `fn record_fire`. Replace their call sites with `super::alerts::should_fire(...)` and `super::alerts::record_fire(...)`.

- [ ] **Step 4: Build + run alert tests**

Run: `devenv shell -- cargo test -p right-bot alerts`
Expected: PASS.

Run: `devenv shell -- cargo test -p right-bot memory_alerts`
Expected: PASS (existing behavior preserved).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/alerts.rs crates/bot/src/telegram/memory_alerts.rs crates/bot/src/telegram/mod.rs
git commit -m "refactor(bot): split telegram alert dedup into reusable module"
```

---

## Task 12: `learning_alerts.rs` with circuit-open alert

**Files:**
- Create: `crates/bot/src/telegram/learning_alerts.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/bot/src/telegram/learning_alerts.rs`:

```rust
//! Telegram alert when the learning review circuit breaker opens.
//!
//! Fired reactively from the failure path in `learning_episode.rs` and the
//! worker skill review path when `record_review_failure` reports
//! `opened_circuit = true`. Dedup is 24 hours per agent (alert_type key
//! `"learning_circuit_open"`) via the shared `alerts` module.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use teloxide::Bot;
use teloxide::prelude::*;

use super::alerts;

const ALERT_TYPE: &str = "learning_circuit_open";

/// Send a circuit-open alert to the first chat in the agent's allowlist if
/// the 24-hour dedup window allows. No-op if the dedup window blocks the
/// alert or if no recipient can be resolved.
pub(crate) async fn maybe_alert_circuit_open(
    bot: Arc<Bot>,
    db: &Path,
    agent_name: &str,
    allowlist_path: &Path,
    last_failure_reason: &str,
    cooldown_minutes: u32,
    failure_threshold: u32,
) -> Result<()> {
    if !alerts::should_fire(db, ALERT_TYPE) {
        return Ok(());
    }
    let Some(chat_id) = first_allowlist_chat(allowlist_path)? else {
        tracing::warn!(
            agent = %agent_name,
            "learning circuit open but allowlist has no chat; skipping alert"
        );
        return Ok(());
    };

    let truncated = if last_failure_reason.len() > 200 {
        format!("{}…", &last_failure_reason[..200])
    } else {
        last_failure_reason.to_owned()
    };
    let body = format!(
        "❌ <b>Learning review circuit opened</b>\n\n\
         Selector failed {failure_threshold}× in a row. New reviews paused for {cooldown_minutes} minutes.\n\n\
         Last error: <code>{}</code>\n\n\
         ➡️ Check <code>~/.right/logs/{agent_name}.log</code> for details.",
        teloxide::utils::html::escape(&truncated),
    );

    bot.send_message(teloxide::types::ChatId(chat_id), body)
        .parse_mode(teloxide::types::ParseMode::Html)
        .await
        .context("send learning_circuit_open alert")?;
    alerts::record_fire(db, ALERT_TYPE);
    Ok(())
}

fn first_allowlist_chat(allowlist_path: &Path) -> Result<Option<i64>> {
    let content = std::fs::read_to_string(allowlist_path)
        .with_context(|| format!("read allowlist {}", allowlist_path.display()))?;
    let parsed: AllowlistFile = serde_yaml::from_str(&content)
        .with_context(|| format!("parse allowlist yaml {}", allowlist_path.display()))?;
    Ok(parsed.allowed_chat_ids.into_iter().next())
}

#[derive(serde::Deserialize)]
struct AllowlistFile {
    #[serde(default)]
    allowed_chat_ids: Vec<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn first_allowlist_chat_returns_first_id() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "allowed_chat_ids:\n- 100\n- 200\n").unwrap();
        let id = first_allowlist_chat(file.path()).unwrap();
        assert_eq!(id, Some(100));
    }

    #[test]
    fn first_allowlist_chat_none_when_empty() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "allowed_chat_ids: []\n").unwrap();
        let id = first_allowlist_chat(file.path()).unwrap();
        assert_eq!(id, None);
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/bot/src/telegram/mod.rs`:

```rust
pub(crate) mod learning_alerts;
```

- [ ] **Step 3: Build + test**

Run: `devenv shell -- cargo test -p right-bot learning_alerts`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/learning_alerts.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): telegram alert helper for learning circuit open"
```

---

## Task 13: Wire alert call into failure paths

**Files:**
- Modify: `crates/bot/src/learning_episode.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add bot + allowlist plumbing to `LearningEpisodeRuntime`**

In `crates/bot/src/learning_episode.rs`, extend `LearningEpisodeRuntime`:

```rust
pub(crate) struct LearningEpisodeRuntime {
    // ...existing fields...
    pub(crate) bot: Arc<teloxide::Bot>,
    pub(crate) allowlist_path: PathBuf,
}
```

Add these to the constructor and to every call site that builds a runtime. Search for `LearningEpisodeRuntime::new` and `LearningEpisodeRuntime {` to find them.

- [ ] **Step 2: Call the alert on circuit transition in `mark_claimed_episode_failed`**

Update the caller in `drain_ready_learning_episodes_once_with_selector_and_reviewer`:

```rust
let outcome = mark_claimed_episode_failed(
    &conn,
    &runtime,
    episode.id,
    &reason,
    &now_str,
    true,
)?;
if let Some((_count, opened)) = outcome
    && opened
{
    let bot = Arc::clone(&runtime.bot);
    let db = runtime.agent_db_dir.clone();
    let agent = runtime.agent_name.clone();
    let allowlist = runtime.allowlist_path.clone();
    let reason_clone = reason.clone();
    let threshold = runtime.learning.circuit_failure_threshold;
    let cooldown = runtime.learning.circuit_cooldown_minutes;
    tokio::spawn(async move {
        if let Err(e) = crate::telegram::learning_alerts::maybe_alert_circuit_open(
            bot,
            &db,
            &agent,
            &allowlist,
            &reason_clone,
            cooldown,
            threshold,
        )
        .await
        {
            tracing::warn!("maybe_alert_circuit_open failed: {e:#}");
        }
    });
}
```

- [ ] **Step 3: Wire equivalent path in worker.rs**

In `worker.rs` failure path, after `record_review_failure` returns `(count, opened)`, replicate the spawn — use the existing `ctx.bot`, `ctx.agent_dir.join("allowlist.yaml")`, etc.

- [ ] **Step 4: Build, run targeted tests**

Run: `devenv shell -- cargo build -p right-bot`
Run: `devenv shell -- cargo test -p right-bot learning_episode`
Run: `devenv shell -- cargo test -p right-bot telegram::worker`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/learning_episode.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): alert on learning circuit open"
```

---

## Task 14: Dashboard `SOURCES` expansion

**Files:**
- Modify: `crates/right-dashboard/src/read_model/usage.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/right-dashboard/src/read_model/usage.rs` test module:

```rust
#[test]
fn usage_overview_includes_learning_sources() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    insert_usage(&conn, "2026-05-21T01:00:00Z", "learning_selector", 0.10, "sonnet");
    insert_usage(&conn, "2026-05-21T02:00:00Z", "learning_reviewer", 0.20, "sonnet");
    insert_usage(&conn, "2026-05-21T03:00:00Z", "learning_skill_review", 0.30, "sonnet");

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "him".to_owned(),
            generated_at: "2026-05-21T05:00:00Z".to_owned(),
        },
    )
    .unwrap();
    let today = response.windows.iter().find(|w| w.key == "today").unwrap();
    let names: Vec<&str> = today.sources.iter().map(|s| s.source.as_str()).collect();
    assert!(names.contains(&"learning_selector"));
    assert!(names.contains(&"learning_reviewer"));
    assert!(names.contains(&"learning_skill_review"));

    let selector = today
        .sources
        .iter()
        .find(|s| s.source == "learning_selector")
        .unwrap();
    assert!((selector.cost_usd - 0.10).abs() < 1e-9);
}

#[test]
fn usage_overview_sources_match_learning_sources_constant() {
    for source in right_agent::usage::LEARNING_SOURCES {
        assert!(
            SOURCES.contains(source),
            "dashboard SOURCES is missing learning source `{source}`"
        );
    }
}
```

- [ ] **Step 2: Run, verify FAIL**

Run: `devenv shell -- cargo test -p right-dashboard usage_overview_includes_learning_sources`
Expected: FAIL — learning sources not in `SOURCES`.

- [ ] **Step 3: Expand `SOURCES`**

In `crates/right-dashboard/src/read_model/usage.rs`:

```rust
const SOURCES: [&str; 6] = [
    "interactive",
    "cron",
    "reflection",
    "learning_selector",
    "learning_reviewer",
    "learning_skill_review",
];
```

- [ ] **Step 4: Run, verify PASS**

Run: `devenv shell -- cargo test -p right-dashboard usage_`
Expected: PASS.

- [ ] **Step 5: Audit other dashboard read models for inline source lists**

Run: `grep -n 'interactive.*cron.*reflection\|cron.*reflection.*interactive' crates/right-dashboard/src/read_model/*.rs`

If `activity.rs` or `dashboard_overview.rs` has a similar hardcoded list of sources, add the three new entries there too. Re-run their tests if so.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/src/read_model/
git commit -m "feat(dashboard): include learning sources in usage overview"
```

---

## Task 15: Documentation updates

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Find the relevant section**

Search `ARCHITECTURE.md` for the words "learning" and "review gate" — the new contract should be referenced where the current gate behavior is documented. If no such section exists yet, add a short subsection under "Data Flow" or near the memory architecture pointer.

- [ ] **Step 2: Add a paragraph**

Add (adjust prose to match existing voice):

```markdown
### Learning review gate

`try_mark_review_started` (`crates/right-agent/src/learned_skills.rs`) is the
shared gate for two learning flows: Stage 2 episode selector/reviewer
(`crates/bot/src/learning_episode.rs`) and worker-side skill review
(`crates/bot/src/telegram/worker.rs`). The gate enforces:

- `Skip(AlreadyRunning)` — `skill_nudge_state.review_running = 1`.
- `Skip(CircuitOpen)` — `consecutive_review_failures >= circuit_failure_threshold`
  opened `review_circuit_open_until` in the future. Auto-clears with the
  failure counter when the window expires.
- `Skip(DailyBudget)` — `SUM(usage_events.total_cost_usd)` for today UTC across
  `right_agent::usage::LEARNING_SOURCES` (`learning_selector`,
  `learning_reviewer`, `learning_skill_review`) is at or above
  `LearningConfig.max_daily_budget_usd` (default $5).

Failure path calls `record_review_failure`, which increments the counter and
opens the circuit on threshold crossing. Success path
(`mark_review_finished`) resets both. Adding a new learning-adjacent
invocation requires adding its source string to `LEARNING_SOURCES` so both
the gate query and the dashboard `SOURCES` array pick it up.
```

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs(architecture): document learning review gate contract"
```

---

## Task 16: Final workspace verification

- [ ] **Step 1: Full workspace tests**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS, no failures.

- [ ] **Step 2: Lint**

Run: `devenv shell -- cargo clippy --workspace -- -D warnings`
Expected: clean.

- [ ] **Step 3: Build all binaries**

Run: `devenv shell -- cargo build --workspace`
Expected: success.

- [ ] **Step 4: Note the operational cleanup SQL for `him`**

The spec lists the one-time SQL block to free `him`'s stuck pending episodes and reset gate state. It is NOT run by code — apply manually after the new bot is deployed:

```sql
UPDATE learning_episodes
SET status = 'no_episode',
    selector_output_json = json_object('status', 'no_episode', 'reason', 'stale_cleanup'),
    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')
WHERE agent_name = 'him'
  AND status = 'pending'
  AND created_at < datetime('now', '-24 hours');

UPDATE skill_nudge_state SET
    daily_review_count = 0,
    daily_review_date = NULL,
    consecutive_review_failures = 0,
    review_circuit_open_until = NULL,
    review_running = 0
WHERE agent_name = 'him';
```

Run this with `sqlite3 ~/.right/agents/him/data.db < cleanup.sql` before the first `right restart him` after deploy.

---

## Self-review checklist (run before handing off)

- Schema migration: ✓ Task 1.
- `LEARNING_SOURCES` constant: ✓ Task 2.
- `insert_learning_*` helpers (3): ✓ Task 3.
- `LearningConfig` new fields + deprecation: ✓ Task 4.
- Gate type changes (`ReviewGateInput`, `ReviewSkipReason`): ✓ Task 5.
- Gate decision rewrite (CircuitOpen + DailyBudget + auto-clear): ✓ Task 6.
- `mark_review_finished` resets circuit: ✓ Task 7.
- `record_review_failure` helper with transition detection: ✓ Task 8.
- Wire `learning_episode.rs`: ✓ Task 9 (gate input, failure helper, usage recording, constant removal).
- Wire `worker.rs`: ✓ Task 10 (gate input, failure helper, usage recording, constant removal).
- Dedup helper split (`alerts.rs`): ✓ Task 11.
- `learning_alerts.rs`: ✓ Task 12.
- Wire alert call: ✓ Task 13.
- Dashboard `SOURCES`: ✓ Task 14 (+ audit of sibling read models).
- Docs: ✓ Task 15.
- Final workspace verification: ✓ Task 16.

No placeholders, no "implement later", every step has either concrete code or a concrete command + expected outcome.

## Known follow-ups (out of scope here)

- If the selector still exits 1 silently after this lands, add `--session-id` plumbing so `--debug-file` lands predictably. Separate spec.
- After 1–2 releases, drop the soft-deprecated `episode_selector_max_budget_usd` config field and the dead `daily_review_count` / `daily_review_date` columns from `skill_nudge_state`.
