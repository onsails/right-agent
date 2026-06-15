# Curator Observability & Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the already-shipped skill curator provable (run-history telemetry + dashboard) and safe (real circuit breaker, live idle gate, opt-in report-only mode), without changing the consolidation logic itself.

**Architecture:** The curator runs as a per-agent 60s ticker (`crates/bot/src/lib.rs`) calling `learning_curator::run_if_due`. We add (A1) an append-only `curator_runs` history table + dashboard panel; (B1) a circuit breaker that finally writes `circuit_open_until`; (B2) feed the existing `IdleTimestamp` into the ticker; (B3) a `curator_mode: apply | report_only` config that runs a read-only LLM "plan" invocation and persists proposed actions instead of writing. The archive-not-delete invariant is preserved everywhere.

**Tech Stack:** Rust (edition 2024, tokio, turso via `right-db`, `thiserror`/`anyhow`), Vue 3 + Vitest (`crates/right-dashboard/frontend`), pnpm.

**Spec:** `docs/superpowers/specs/2026-06-15-curator-observability-and-safety-design.md`
**Related issues:** recover from archive onsails/right-agent#134; consolidation-quality onsails/right-agent#132.

**Conventions for every task below:**
- All Rust commands run under devenv: `devenv shell -- cargo …`.
- Frontend commands run in `crates/right-dashboard/frontend`: `devenv shell -- pnpm …`.
- TDD: write the failing test, run it red, implement, run it green, commit. Do **not** run the full workspace suite between tasks — only the targeted command named in the task. The final full-workspace run is Task 16.
- Known-flaky under parallel load (re-run isolated before blaming your change): a `cc/invocation` pid-race test and a `right-dashboard` warn-count test.

---

## Phase 0 — Baseline

### Task 1: Record a clean baseline

**Files:** none (verification only)

- [ ] **Step 1: Build + targeted curator tests**

Run:
```bash
devenv shell -- cargo build -p right-bot -p right-db -p right-agent-config -p right-dashboard -p right-lifecycle
devenv shell -- cargo nextest run -p right-bot learning_curator
devenv shell -- cargo nextest run -p right-agent-config
```
Expected: build succeeds; curator + config tests pass. Record any pre-existing failures in the PR description; if a failure is unrelated to this plan, note it and proceed.

- [ ] **Step 2: Frontend baseline**

Run:
```bash
cd crates/right-dashboard/frontend && devenv shell -- pnpm install --frozen-lockfile && devenv shell -- pnpm test
```
Expected: existing vitest suite passes.

---

## Phase 1 — B2: live idle gate

The gate already checks `min_idle_hours` in `cheap_skip`; the ticker just feeds it `None` (`lib.rs:1091`). We add a pure conversion helper and wire the existing `IdleTimestamp` Arc into the ticker.

### Task 2: Pure helper `idle_secs_to_activity`

**Files:**
- Modify: `crates/bot/src/learning_curator.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/bot/src/learning_curator.rs`:
```rust
    #[test]
    fn idle_secs_zero_or_negative_is_none() {
        assert_eq!(idle_secs_to_activity(0), None);
        assert_eq!(idle_secs_to_activity(-5), None);
    }

    #[test]
    fn idle_secs_positive_converts_to_utc() {
        let got = idle_secs_to_activity(1_700_000_000).unwrap();
        assert_eq!(got, DateTime::from_timestamp(1_700_000_000, 0).unwrap());
    }
```

- [ ] **Step 2: Run red**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator::tests::idle_secs`
Expected: FAIL — `cannot find function idle_secs_to_activity`.

- [ ] **Step 3: Implement**

Add near the top of `learning_curator.rs` (after the `use chrono::…` import, before `CuratorState`):
```rust
/// Convert the delivery idle timestamp (unix seconds, from `IdleTimestamp`)
/// into the chat-activity instant the curator idle gate consumes. 0/negative
/// means "uninitialized" → `None` (gate treats absence as "idle enough").
pub(crate) fn idle_secs_to_activity(secs: i64) -> Option<DateTime<Utc>> {
    if secs <= 0 {
        None
    } else {
        DateTime::from_timestamp(secs, 0)
    }
}
```

- [ ] **Step 4: Run green**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator::tests::idle_secs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/learning_curator.rs
git commit -m "feat(curator): pure idle_secs_to_activity helper for the idle gate"
```

### Task 3: Feed `IdleTimestamp` into the ticker

**Files:**
- Modify: `crates/bot/src/lib.rs:1037-1095` (the curator ticker block)

- [ ] **Step 1: Clone the idle Arc into the curator block**

In the curator-ticker block (`crates/bot/src/lib.rs`, the `{ … }` starting near line 1039), add alongside the other `let curator_* = …clone();` bindings:
```rust
        let curator_idle_ts = std::sync::Arc::clone(&idle_timestamp);
```
(`idle_timestamp` is defined at `lib.rs:960` as `Arc<IdleTimestamp>`.)

- [ ] **Step 2: Replace the `None` argument with a real activity timestamp**

Replace:
```rust
                // latest_user_activity_at hookup deferred — pass None for v1.
                crate::learning_curator::run_if_due(ctx, None).await;
```
with:
```rust
                let latest_activity = crate::learning_curator::idle_secs_to_activity(
                    curator_idle_ts
                        .0
                        .load(std::sync::atomic::Ordering::Relaxed),
                );
                crate::learning_curator::run_if_due(ctx, latest_activity).await;
```

- [ ] **Step 3: Verify it compiles**

Run: `devenv shell -- cargo build -p right-bot`
Expected: success. (If `IdleTimestamp`'s field is not `pub`, make the tuple field `pub(crate)` in its definition — it is already read as `idle_ts.0` in `async_delivery.rs`, so it is accessible within the crate.)

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "feat(curator): wire IdleTimestamp into the ticker so min_idle_hours works"
```

---

## Phase 2 — Config: `CuratorMode` + circuit knobs

`LearningConfig` lives in `crates/right-agent-config/src/lib.rs` and is re-exported via `pub use right_agent_config::*;` (`crates/right-agent/src/agent/types.rs:1`), so it is the single source of truth. We add three fields now (consumed in later phases) and keep current behavior as the default.

### Task 4: Add `CuratorMode` enum + three `LearningConfig` fields

**Files:**
- Modify: `crates/right-agent-config/src/lib.rs`
- Modify: `crates/right/src/wizard.rs` (struct literal only — prompts come in Task 15)

- [ ] **Step 1: Write the failing default test**

Extend the existing defaults test in `crates/right-agent-config/src/lib.rs` (the test asserting `cfg.curator_interval_hours == 168` near line 904). Add:
```rust
        assert_eq!(cfg.curator_circuit_failure_threshold, 3);
        assert_eq!(cfg.curator_circuit_cooldown_hours, 24);
        assert_eq!(cfg.curator_mode, CuratorMode::Apply);
```

- [ ] **Step 2: Run red**

Run: `devenv shell -- cargo nextest run -p right-agent-config`
Expected: FAIL — unknown field / `CuratorMode` not found.

- [ ] **Step 3: Add the enum**

In `crates/right-agent-config/src/lib.rs`, just above the `LearningConfig` struct definition (before line 451), add:
```rust
/// Curator execution mode. `Apply` (default) writes consolidations to disk and
/// the lifecycle DB; `ReportOnly` runs a read-only LLM pass that proposes
/// consolidations without writing anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CuratorMode {
    #[default]
    Apply,
    ReportOnly,
}
```
(If `Serialize`/`Deserialize` are not already imported at the top of the file, add `use serde::{Deserialize, Serialize};` — they are used by the surrounding config types, so the import almost certainly exists.)

- [ ] **Step 4: Add the default fns**

Next to the other `default_curator_*` fns (around line 130):
```rust
fn default_curator_circuit_failure_threshold() -> u32 {
    3
}
fn default_curator_circuit_cooldown_hours() -> u32 {
    24
}
```

- [ ] **Step 5: Add the struct fields**

In `LearningConfig`, immediately after `pub curator_min_cooldown_hours: u32,` (line 526) and before the baseline fields:
```rust
    /// Consecutive failed curator passes before the circuit opens.
    #[serde(
        default = "default_curator_circuit_failure_threshold",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_circuit_failure_threshold: u32,
    /// Fixed cooldown (hours) the circuit stays open once tripped.
    #[serde(
        default = "default_curator_circuit_cooldown_hours",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_circuit_cooldown_hours: u32,
    /// `apply` (write) or `report_only` (propose without writing).
    #[serde(default)]
    pub curator_mode: CuratorMode,
```

- [ ] **Step 6: Add to the `Default for LearningConfig` impl**

After `curator_min_cooldown_hours: default_curator_min_cooldown_hours(),` (line 589):
```rust
            curator_circuit_failure_threshold: default_curator_circuit_failure_threshold(),
            curator_circuit_cooldown_hours: default_curator_circuit_cooldown_hours(),
            curator_mode: CuratorMode::default(),
```

- [ ] **Step 7: Keep the wizard compiling (passthrough)**

In `crates/right/src/wizard.rs`, the `LearningConfig { … }` struct literal (near line 1289), after `curator_min_cooldown_hours,` add:
```rust
        curator_circuit_failure_threshold: existing.curator_circuit_failure_threshold,
        curator_circuit_cooldown_hours: existing.curator_circuit_cooldown_hours,
        curator_mode: existing.curator_mode,
```

- [ ] **Step 8: Run green**

Run:
```bash
devenv shell -- cargo nextest run -p right-agent-config
devenv shell -- cargo build -p right
```
Expected: PASS + build succeeds.

- [ ] **Step 9: Commit**

```bash
git add crates/right-agent-config/src/lib.rs crates/right/src/wizard.rs
git commit -m "feat(config): CuratorMode + circuit threshold/cooldown knobs (defaults preserve behavior)"
```

---

## Phase 3 — B1: circuit breaker

### Task 5: Pure `next_circuit_open_until`

**Files:**
- Modify: `crates/bot/src/learning_curator.rs`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `learning_curator.rs`:
```rust
    #[test]
    fn circuit_stays_closed_below_threshold() {
        let now = dt("2026-05-22T00:00:00Z");
        assert_eq!(next_circuit_open_until(2, 3, 24, now), None);
    }

    #[test]
    fn circuit_opens_at_threshold() {
        let now = dt("2026-05-22T00:00:00Z");
        let got = next_circuit_open_until(3, 3, 24, now).unwrap();
        assert_eq!(got, now + Duration::hours(24));
    }

    #[test]
    fn circuit_stays_open_above_threshold_fixed_cooldown() {
        let now = dt("2026-05-22T00:00:00Z");
        // 5 failures still opens for the same fixed 24h (not exponential).
        let got = next_circuit_open_until(5, 3, 24, now).unwrap();
        assert_eq!(got, now + Duration::hours(24));
    }
```

- [ ] **Step 2: Run red**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator::tests::circuit`
Expected: FAIL — `next_circuit_open_until` not found.

- [ ] **Step 3: Implement**

In `learning_curator.rs`, near `should_run_now`:
```rust
/// Circuit-breaker decision: once `consecutive_failures >= threshold`, the
/// circuit opens for a FIXED `cooldown_hours` (not exponential). Failures
/// persist across opens, so a permanently-broken curator re-opens at this
/// cadence rather than hammering every cooldown. Returns the new
/// `circuit_open_until`, or `None` to leave the circuit closed.
pub(crate) fn next_circuit_open_until(
    consecutive_failures: u32,
    threshold: u32,
    cooldown_hours: u32,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if consecutive_failures >= threshold {
        Some(now + Duration::hours(cooldown_hours as i64))
    } else {
        None
    }
}
```

- [ ] **Step 4: Run green**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator::tests::circuit`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/learning_curator.rs
git commit -m "feat(curator): pure next_circuit_open_until circuit-breaker decision"
```

### Task 6: Carry circuit knobs into `CuratorConfig` and the ticker map

**Files:**
- Modify: `crates/bot/src/learning_curator.rs` (`CuratorConfig` struct + test `cfg()`)
- Modify: `crates/bot/src/lib.rs` (ticker `CuratorConfig { … }` map near line 1077)

- [ ] **Step 1: Extend `CuratorConfig`**

In `learning_curator.rs`, add to `struct CuratorConfig` (after `skill_change_threshold: u32,`):
```rust
    pub circuit_failure_threshold: u32,
    pub circuit_cooldown_hours: u32,
```

- [ ] **Step 2: Update the test `cfg()` helper**

In `mod tests`, add to the `CuratorConfig { … }` returned by `fn cfg()`:
```rust
            circuit_failure_threshold: 3,
            circuit_cooldown_hours: 24,
```

- [ ] **Step 3: Map from `LearningConfig` in the ticker**

In `crates/bot/src/lib.rs`, the `CuratorConfig { … }` built in the ticker (near line 1077), add:
```rust
                        circuit_failure_threshold: curator_learning
                            .curator_circuit_failure_threshold,
                        circuit_cooldown_hours: curator_learning.curator_circuit_cooldown_hours,
```

- [ ] **Step 4: Verify compile**

Run: `devenv shell -- cargo build -p right-bot`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/learning_curator.rs crates/bot/src/lib.rs
git commit -m "feat(curator): thread circuit knobs into CuratorConfig and the ticker"
```

### Task 7: Open the circuit on the failure path

**Files:**
- Modify: `crates/bot/src/learning_curator.rs` (`run_if_due` state-update-on-failure)

The current code increments `consecutive_failures` in several failure branches and at the end (`learning_curator.rs:418-424, 447-453, 504-514`). Add a single helper that both increments and applies the circuit decision, and call it from each failure branch so the logic is single-sourced.

- [ ] **Step 1: Add the failure-state helper**

In `learning_curator.rs`:
```rust
/// Apply a failed-pass state transition: bump `consecutive_failures`, stamp
/// `last_run_at`/`last_run_status`, and open the circuit when the threshold is
/// reached (B1). Mutates `state` in place; the caller persists it.
fn mark_failed_run(state: &mut CuratorState, config: CuratorConfig, now: DateTime<Utc>) {
    state.last_run_at = Some(now.to_rfc3339());
    state.last_run_status = Some("failed".to_owned());
    state.consecutive_failures += 1;
    if let Some(open_until) = next_circuit_open_until(
        state.consecutive_failures,
        config.circuit_failure_threshold,
        config.circuit_cooldown_hours,
        now,
    ) {
        state.circuit_open_until = Some(open_until.to_rfc3339());
    }
}
```

- [ ] **Step 2: Use it in the failure branches**

Replace the three manual failure blocks in `run_if_due` (invocation-registration failure ~418, command-build failure ~447, and the final `else` after the spawn match ~509-514) so each does:
```rust
            mark_failed_run(&mut state, ctx.config, now);
```
instead of the inline `state.last_run_at = …; state.last_run_status = …; state.consecutive_failures += 1;` lines. For the final post-spawn block, keep the existing success branch (`consecutive_failures = 0; circuit_open_until = None;`) and call `mark_failed_run` in the failure branch. Delete the stale `TODO(Phase-2)` comment at `learning_curator.rs:510-512`.

- [ ] **Step 3: Add a state-level test**

Add to `mod tests`:
```rust
    #[test]
    fn mark_failed_run_opens_circuit_at_threshold() {
        let now = dt("2026-05-22T00:00:00Z");
        let mut s = CuratorState {
            consecutive_failures: 2,
            ..Default::default()
        };
        mark_failed_run(&mut s, cfg(), now);
        assert_eq!(s.consecutive_failures, 3);
        assert_eq!(
            s.circuit_open_until.as_deref(),
            Some((now + Duration::hours(24)).to_rfc3339().as_str())
        );
        assert_eq!(s.last_run_status.as_deref(), Some("failed"));
    }

    #[test]
    fn mark_failed_run_keeps_circuit_closed_below_threshold() {
        let now = dt("2026-05-22T00:00:00Z");
        let mut s = CuratorState::default();
        mark_failed_run(&mut s, cfg(), now);
        assert_eq!(s.consecutive_failures, 1);
        assert_eq!(s.circuit_open_until, None);
    }
```

- [ ] **Step 4: Run green**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator`
Expected: PASS (existing curator tests still green).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/learning_curator.rs
git commit -m "feat(curator): open circuit_open_until on repeated failures (B1)"
```

---

## Phase 4 — A1 backend: `curator_runs` history

### Task 8: v48 migration — `curator_runs` table

**Files:**
- Create: `crates/right-db/src/sql/v48_curator_runs.sql`
- Modify: `crates/right-db/src/migrations.rs` (const + `LATEST_SCHEMA_VERSION` + array entry)

- [ ] **Step 1: Write the migration SQL**

Create `crates/right-db/src/sql/v48_curator_runs.sql`:
```sql
CREATE TABLE IF NOT EXISTS curator_runs (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    run_at                 TEXT NOT NULL,
    trigger                TEXT NOT NULL,          -- cost_spike | skill_change | time_fallback
    trigger_evidence_json  TEXT,
    mode                   TEXT NOT NULL,          -- apply | report_only
    status                 TEXT NOT NULL,          -- success | failed | proposed
    cost_usd               REAL NOT NULL DEFAULT 0,
    cache_read             INTEGER NOT NULL DEFAULT 0,
    cache_creation         INTEGER NOT NULL DEFAULT 0,
    consolidations         INTEGER NOT NULL DEFAULT 0,  -- skills merged (absorbed_into set); subset of archives
    archives               INTEGER NOT NULL DEFAULT 0,  -- total skills archived this pass
    summary                TEXT,
    actions_json           TEXT NOT NULL DEFAULT '[]',
    invocation_id          TEXT,
    created_at             TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_curator_runs_run_at ON curator_runs(run_at);
```

- [ ] **Step 2: Register the migration**

In `crates/right-db/src/migrations.rs`:
1. After line 39 (`const V47_CRON_SKILL_LINKS …`):
```rust
const V48_CURATOR_RUNS: &str = include_str!("sql/v48_curator_runs.sql");
```
2. Bump line 41:
```rust
pub const LATEST_SCHEMA_VERSION: u32 = 48;
```
3. After the v47 array entry (`learning_curator`-style block ending at line 1094), add inside `migrations: &[ … ]`:
```rust
        Migration {
            version: 48,
            sql: V48_CURATOR_RUNS,
            hook: None,
        },
```

- [ ] **Step 3: Write the idempotency/round-trip test**

In the `migrations.rs` `#[cfg(test)]` module, add:
```rust
    #[tokio::test]
    async fn v48_curator_runs_table_exists_and_is_idempotent() {
        let mut conn = test_conn().await; // mirror the existing helper used by sibling tests
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        // Insert + read back.
        conn.execute(
            "INSERT INTO curator_runs (run_at, trigger, mode, status) \
             VALUES ('2026-06-15T00:00:00Z','time_fallback','apply','success')",
            (),
        )
        .await
        .unwrap();
        let n: i64 = conn
            .query_one("SELECT COUNT(*) FROM curator_runs", (), |r| r.get(0))
            .await
            .unwrap();
        assert_eq!(n, 1);
        // Re-running to_latest is a no-op (CREATE TABLE IF NOT EXISTS).
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
    }
```
(Use whatever in-test connection constructor the neighboring migration tests use — e.g. the same `Connection::open_in_memory()` + helper pattern visible in the file. Match the existing tests' setup verbatim.)

- [ ] **Step 4: Run red→green**

Run: `devenv shell -- cargo nextest run -p right-db v48_curator_runs`
Expected: PASS after the changes (and a clean red if you run it before editing `migrations.rs`).

- [ ] **Step 5: Commit**

```bash
git add crates/right-db/src/sql/v48_curator_runs.sql crates/right-db/src/migrations.rs
git commit -m "feat(db): v48 curator_runs history table"
```

### Task 9: `CuratorRunRecord` + `insert_curator_run`

**Files:**
- Modify: `crates/bot/src/learning_curator.rs`

- [ ] **Step 1: Write the failing test**

Add to `mod tests` (uses the existing `open_test_conn()` helper):
```rust
    #[tokio::test]
    async fn insert_curator_run_round_trips() {
        let conn = open_test_conn().await;
        let rec = CuratorRunRecord {
            run_at: "2026-06-15T00:00:00Z".into(),
            trigger: "time_fallback".into(),
            trigger_evidence_json: Some("{\"trigger\":\"time_fallback\"}".into()),
            mode: "apply".into(),
            status: "success".into(),
            cost_usd: 0.42,
            cache_read: 10,
            cache_creation: 5,
            consolidations: 1,
            archives: 2,
            summary: Some("merged 1, archived 2".into()),
            actions_json: "[]".into(),
            invocation_id: Some("inv-1".into()),
        };
        insert_curator_run(&conn, &rec).await.unwrap();
        let (trigger, status, archives): (String, String, i64) = conn
            .query_row(
                "SELECT trigger, status, archives FROM curator_runs WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!((trigger.as_str(), status.as_str(), archives), ("time_fallback", "success", 2));
    }
```

- [ ] **Step 2: Run red**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator::tests::insert_curator_run`
Expected: FAIL — `CuratorRunRecord`/`insert_curator_run` not found.

- [ ] **Step 3: Implement**

In `learning_curator.rs` (near `save_state_db`):
```rust
/// One append-only `curator_runs` history row (A1 observability).
#[derive(Debug, Clone)]
pub(crate) struct CuratorRunRecord {
    pub run_at: String,
    pub trigger: String,
    pub trigger_evidence_json: Option<String>,
    pub mode: String,
    pub status: String,
    pub cost_usd: f64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub consolidations: i64,
    pub archives: i64,
    pub summary: Option<String>,
    pub actions_json: String,
    pub invocation_id: Option<String>,
}

/// Append a curator run-history row. Best-effort at the learning boundary —
/// callers log-and-continue on error; never abort a pass over telemetry.
pub(crate) async fn insert_curator_run(
    conn: &right_db::Connection,
    rec: &CuratorRunRecord,
) -> Result<(), right_db::DbError> {
    conn.execute(
        "INSERT INTO curator_runs \
            (run_at, trigger, trigger_evidence_json, mode, status, cost_usd, \
             cache_read, cache_creation, consolidations, archives, summary, \
             actions_json, invocation_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        right_db::params![
            rec.run_at.as_str(),
            rec.trigger.as_str(),
            rec.trigger_evidence_json.as_deref(),
            rec.mode.as_str(),
            rec.status.as_str(),
            rec.cost_usd,
            rec.cache_read,
            rec.cache_creation,
            rec.consolidations,
            rec.archives,
            rec.summary.as_deref(),
            rec.actions_json.as_str(),
            rec.invocation_id.as_deref(),
        ],
    )
    .await?;
    Ok(())
}

/// Map a `CuratorTrigger` to its `curator_runs.trigger` string.
fn trigger_label(trigger: &CuratorTrigger) -> &'static str {
    match trigger {
        CuratorTrigger::CostSpike(_) => "cost_spike",
        CuratorTrigger::SkillChangeCount { .. } => "skill_change",
        CuratorTrigger::TimeFallback { .. } => "time_fallback",
    }
}
```

- [ ] **Step 4: Run green**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator::tests::insert_curator_run`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/learning_curator.rs
git commit -m "feat(curator): CuratorRunRecord + insert_curator_run writer"
```

### Task 10: Write a `curator_runs` row at the end of an apply pass

**Files:**
- Modify: `crates/bot/src/learning_curator.rs` (`run_if_due` apply tail)

The apply pass already computes `trigger`, `archived_skill_names`, the usage breakdown `b` (inside the `Completed` arm at ~464), and `run_status`. Hoist the breakdown so it survives the match, query each archived skill's `absorbed_into`, then write one row.

- [ ] **Step 1: Hoist the usage breakdown**

Before the `let run_status = match right_process::ProcessGroupChild::spawn(cmd) { … }` block, add:
```rust
    let mut usage_for_run: Option<right_agent::usage::UsageBreakdown> = None;
```
Inside the `Completed(output)` arm where `parse_usage_full` runs (~464), after the existing `if let Some(b) = …` block, store a clone:
```rust
                    usage_for_run = crate::cc::stream::parse_usage_full(&stdout);
```
(Place this so it captures the same parsed breakdown; reuse the already-parsed value rather than parsing twice if convenient.)

- [ ] **Step 2: Compute consolidations and the actions blob**

After `run_status` is known and **after** the `state` save block at the end of `run_if_due`, add:
```rust
    // A1: append a curator_runs history row for this executed apply pass.
    let absorbed: Vec<(String, Option<String>)> = match conn
        .query_all(
            "SELECT skill_name, absorbed_into FROM skill_lifecycle WHERE archived_at = ?1",
            right_db::params![now.to_rfc3339()],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator_runs absorbed query failed: {e:#}");
            Vec::new()
        }
    };
    let consolidations = absorbed.iter().filter(|(_, t)| t.is_some()).count() as i64;
    let archives = absorbed.len() as i64;
    let actions_json = serde_json::to_string(
        &absorbed
            .iter()
            .map(|(name, target)| {
                serde_json::json!({
                    "kind": if target.is_some() { "merge" } else { "archive" },
                    "skills": [name],
                    "target": target,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_owned());
    let (cost_usd, cache_read, cache_creation) = usage_for_run
        .as_ref()
        .map(|b| {
            (
                b.total_cost_usd,
                b.cache_read_tokens as i64,
                b.cache_creation_tokens as i64,
            )
        })
        .unwrap_or((0.0, 0, 0));
    let record = CuratorRunRecord {
        run_at: now.to_rfc3339(),
        trigger: trigger_label(&trigger).to_owned(),
        trigger_evidence_json: state.last_spike_evidence_json.clone(),
        mode: "apply".to_owned(),
        status: run_status.clone(),
        cost_usd,
        cache_read,
        cache_creation,
        consolidations,
        archives,
        summary: Some(format!("merged {consolidations}, archived {archives}")),
        actions_json,
        invocation_id: None,
    };
    if let Err(e) = insert_curator_run(&conn, &record).await {
        tracing::warn!(agent = %ctx.agent_name, "curator_runs insert failed: {e:#}");
    }
```

- [ ] **Step 3: Verify compile + curator suite**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator`
Expected: PASS. (No new unit test here — the writer is covered by Task 9; this is wiring verified by compile + the existing gate tests. An integration test that runs a full apply pass requires a live CC fork and is out of scope for unit tests.)

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/learning_curator.rs
git commit -m "feat(curator): record a curator_runs row per executed apply pass"
```

---

## Phase 5 — A1 dashboard

### Task 11: Backend read-model — curator runs + consolidation lineage

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs` (new response types + extend `LearningOverviewResponse`)
- Modify: `crates/right-dashboard/src/read_model/learning.rs` (new read fns + wire into `learning_overview`)

- [ ] **Step 1: Add response types**

In `crates/right-dashboard/src/api_types.rs`, add:
```rust
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CuratorRunSummary {
    pub run_at: String,
    pub trigger: String,
    pub mode: String,
    pub status: String,
    pub cost_usd: f64,
    pub consolidations: i64,
    pub archives: i64,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CuratorConsolidation {
    /// Skill that was absorbed (archived).
    pub absorbed: String,
    /// Umbrella it was merged into.
    pub umbrella: String,
}
```
Add two fields to `LearningOverviewResponse`:
```rust
    pub curator_runs: Vec<CuratorRunSummary>,
    pub curator_consolidations: Vec<CuratorConsolidation>,
```

- [ ] **Step 2: Write the read-model test**

In `crates/right-dashboard/src/read_model/learning.rs` `#[cfg(test)]` (mirror the in-memory `open_connection` + `MIGRATIONS` setup the sibling tests use):
```rust
    #[tokio::test]
    async fn curator_runs_and_lineage_projection() {
        let conn = test_conn().await;
        conn.execute(
            "INSERT INTO curator_runs (run_at, trigger, mode, status, cost_usd, consolidations, archives, summary) \
             VALUES ('2026-06-15T00:00:00Z','time_fallback','apply','success',0.5,1,2,'merged 1, archived 2')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO skill_lifecycle (skill_name, state, created_by, created_at, absorbed_into) \
             VALUES ('rightx-a','archived','curator','t','rightx-umbrella')",
            (),
        ).await.unwrap();
        let now = parse_utc("2026-06-15T01:00:00Z").unwrap();
        let runs = curator_runs(&conn, 20).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].archives, 2);
        let lineage = curator_consolidations(&conn, &now).await.unwrap();
        assert_eq!(lineage, vec![CuratorConsolidation {
            absorbed: "rightx-a".into(),
            umbrella: "rightx-umbrella".into(),
        }]);
    }
```

- [ ] **Step 3: Run red**

Run: `devenv shell -- cargo nextest run -p right-dashboard curator_runs_and_lineage_projection`
Expected: FAIL — `curator_runs`/`curator_consolidations` not found.

- [ ] **Step 4: Implement the read fns**

In `read_model/learning.rs` (import `CuratorConsolidation, CuratorRunSummary` from `crate::api_types`):
```rust
async fn curator_runs(
    conn: &Connection,
    limit: i64,
) -> Result<Vec<CuratorRunSummary>, ReadModelError> {
    let rows = conn
        .query_all(
            "SELECT run_at, trigger, mode, status, cost_usd, consolidations, archives, summary \
             FROM curator_runs ORDER BY run_at DESC LIMIT ?1",
            params![limit],
            |r| {
                Ok(CuratorRunSummary {
                    run_at: r.get(0)?,
                    trigger: r.get(1)?,
                    mode: r.get(2)?,
                    status: r.get(3)?,
                    cost_usd: r.get(4)?,
                    consolidations: r.get(5)?,
                    archives: r.get(6)?,
                    summary: r.get(7)?,
                })
            },
        )
        .await?;
    Ok(rows)
}

async fn curator_consolidations(
    conn: &Connection,
    _now: &DateTime<Utc>,
) -> Result<Vec<CuratorConsolidation>, ReadModelError> {
    let rows = conn
        .query_all(
            "SELECT skill_name, absorbed_into FROM skill_lifecycle \
             WHERE state = 'archived' AND absorbed_into IS NOT NULL \
             ORDER BY archived_at DESC LIMIT 50",
            (),
            |r| {
                Ok(CuratorConsolidation {
                    absorbed: r.get(0)?,
                    umbrella: r.get(1)?,
                })
            },
        )
        .await?;
    Ok(rows)
}
```
Then in `learning_overview` (before the `Ok(LearningOverviewResponse { … })`), add:
```rust
    let curator_runs = curator_runs(conn, 20).await?;
    let curator_consolidations = curator_consolidations(conn, &generated_at_utc).await?;
```
and include both fields in the returned struct literal.

- [ ] **Step 5: Run green**

Run: `devenv shell -- cargo nextest run -p right-dashboard curator_runs_and_lineage_projection`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model/learning.rs
git commit -m "feat(dashboard): project curator_runs history + consolidation lineage"
```

### Task 12: Frontend types + pure label helper + SSR panel

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts` (add the two interfaces + the two `LearningOverviewResponse` fields)
- Create: `crates/right-dashboard/frontend/src/views/learning/curatorRuns.ts` (pure helper)
- Create: `crates/right-dashboard/frontend/src/views/learning/curatorRuns.test.ts`
- Create: `crates/right-dashboard/frontend/src/views/learning/CuratorRunsPanel.vue`
- Modify: `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue` (render the panel)

- [ ] **Step 1: Add TS types**

In `crates/right-dashboard/frontend/src/types.ts`, add:
```typescript
export interface CuratorRunSummary {
  run_at: string
  trigger: string
  mode: string
  status: string
  cost_usd: number
  consolidations: number
  archives: number
  summary: string | null
}

export interface CuratorConsolidation {
  absorbed: string
  umbrella: string
}
```
Add to the `LearningOverviewResponse` interface:
```typescript
  curator_runs: CuratorRunSummary[]
  curator_consolidations: CuratorConsolidation[]
```

- [ ] **Step 2: Write the failing helper test**

Create `crates/right-dashboard/frontend/src/views/learning/curatorRuns.test.ts`:
```typescript
import { describe, expect, it } from 'vitest'

import { curatorRunStatusTone, curatorRunHeadline } from './curatorRuns'

describe('curatorRunStatusTone', () => {
  it('maps proposed to info', () => {
    expect(curatorRunStatusTone('proposed')).toBe('info')
  })
  it('maps failed to bad', () => {
    expect(curatorRunStatusTone('failed')).toBe('bad')
  })
  it('maps success to ok', () => {
    expect(curatorRunStatusTone('success')).toBe('ok')
  })
})

describe('curatorRunHeadline', () => {
  it('summarises an apply run', () => {
    expect(
      curatorRunHeadline({
        run_at: '2026-06-15T00:00:00Z',
        trigger: 'time_fallback',
        mode: 'apply',
        status: 'success',
        cost_usd: 0.5,
        consolidations: 1,
        archives: 2,
        summary: null,
      }),
    ).toBe('merged 1, archived 2')
  })
  it('labels a report-only run as proposed', () => {
    expect(
      curatorRunHeadline({
        run_at: '2026-06-15T00:00:00Z',
        trigger: 'cost_spike',
        mode: 'report_only',
        status: 'proposed',
        cost_usd: 0.1,
        consolidations: 0,
        archives: 0,
        summary: '3 proposals',
      }),
    ).toBe('3 proposals')
  })
})
```

- [ ] **Step 3: Run red**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm vitest run src/views/learning/curatorRuns.test.ts`
Expected: FAIL — module not found.

- [ ] **Step 4: Implement the helper**

Create `crates/right-dashboard/frontend/src/views/learning/curatorRuns.ts`:
```typescript
import type { CuratorRunSummary } from '../../types'

export type Tone = 'ok' | 'bad' | 'info'

export function curatorRunStatusTone(status: string): Tone {
  if (status === 'failed') {
    return 'bad'
  }
  if (status === 'success') {
    return 'ok'
  }
  return 'info'
}

/** Prefer the backend summary; otherwise synthesise from the counts. */
export function curatorRunHeadline(run: CuratorRunSummary): string {
  if (run.summary && run.summary.length > 0) {
    return run.summary
  }
  return `merged ${run.consolidations}, archived ${run.archives}`
}
```

- [ ] **Step 5: Run green**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm vitest run src/views/learning/curatorRuns.test.ts`
Expected: PASS.

- [ ] **Step 6: Build the panel component**

Create `crates/right-dashboard/frontend/src/views/learning/CuratorRunsPanel.vue`:
```vue
<script setup lang="ts">
import { computed } from 'vue'

import AsyncState from '../../components/AsyncState.vue'
import CollapsibleSection from '../../components/CollapsibleSection.vue'
import type { CuratorConsolidation, CuratorRunSummary } from '../../types'
import { curatorRunHeadline, curatorRunStatusTone } from './curatorRuns'

const props = defineProps<{
  runs: CuratorRunSummary[] | null
  consolidations: CuratorConsolidation[] | null
}>()

const runs = computed(() => props.runs ?? [])
const consolidations = computed(() => props.consolidations ?? [])
const loading = computed(() => props.runs === null)
</script>

<template>
  <section class="panel">
    <h3>Curator</h3>
    <AsyncState :loading="loading" :error="null" :empty="runs.length === 0" empty-text="No curator runs yet">
      <CollapsibleSection title="Recent runs" :count="runs.length" :default-open="true">
        <ul class="curator-runs">
          <li v-for="run in runs" :key="run.run_at" :class="curatorRunStatusTone(run.status)">
            <span class="when">{{ run.run_at }}</span>
            <span class="trigger">{{ run.trigger }}</span>
            <span class="mode">{{ run.mode }}</span>
            <span class="headline">{{ curatorRunHeadline(run) }}</span>
            <span class="cost">${{ run.cost_usd.toFixed(3) }}</span>
          </li>
        </ul>
      </CollapsibleSection>
      <CollapsibleSection title="Consolidations" :count="consolidations.length">
        <ul class="curator-lineage">
          <li v-for="c in consolidations" :key="c.absorbed">
            {{ c.absorbed }} → {{ c.umbrella }}
          </li>
        </ul>
      </CollapsibleSection>
    </AsyncState>
  </section>
</template>
```

- [ ] **Step 7: Render it in ReportsView**

In `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue`, import and render the panel, passing the new fields:
```vue
import CuratorRunsPanel from './CuratorRunsPanel.vue'
```
In the template, add:
```vue
    <CuratorRunsPanel
      :runs="learning?.curator_runs ?? null"
      :consolidations="learning?.curator_consolidations ?? null"
    />
```

- [ ] **Step 8: SSR component test**

Create the SSR test alongside (append to `curatorRuns.test.ts` or a new `CuratorRunsPanel.test.ts` — mirror `components/AsyncState.test.ts`'s `createSSRApp` + `renderToString` pattern):
```typescript
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import CuratorRunsPanel from './CuratorRunsPanel.vue'

describe('CuratorRunsPanel', () => {
  it('renders the empty state when there are no runs', async () => {
    const app = createSSRApp({ render: () => h(CuratorRunsPanel, { runs: [], consolidations: [] }) })
    expect(await renderToString(app)).toContain('No curator runs yet')
  })
  it('renders a run headline and a lineage arrow', async () => {
    const app = createSSRApp({
      render: () =>
        h(CuratorRunsPanel, {
          runs: [{ run_at: '2026-06-15T00:00:00Z', trigger: 'time_fallback', mode: 'apply', status: 'success', cost_usd: 0.5, consolidations: 1, archives: 2, summary: null }],
          consolidations: [{ absorbed: 'rightx-a', umbrella: 'rightx-umbrella' }],
        }),
    })
    const html = await renderToString(app)
    expect(html).toContain('merged 1, archived 2')
    expect(html).toContain('rightx-a → rightx-umbrella')
  })
})
```

- [ ] **Step 9: Run green**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test`
Expected: PASS (all vitest, including the new SSR test).

- [ ] **Step 10: Commit**

```bash
git add crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/views/learning/
git commit -m "feat(dashboard): curator runs + consolidation lineage panel"
```

---

## Phase 6 — B3: `report_only` mode

### Task 13: Curator plan schema + report-only prompt

**Files:**
- Modify: `crates/right-codegen/src/agent_def.rs` (two new consts)
- Modify: `crates/right-codegen/src/lib.rs` (re-export the new consts if siblings are re-exported there)

- [ ] **Step 1: Add the consts**

In `crates/right-codegen/src/agent_def.rs`, after `CURATOR_SYSTEM_PROMPT` (ends line 181):
```rust
/// JSON schema for the read-only `report_only` curator pass: a list of proposed
/// consolidation actions. The model writes nothing; it returns this plan.
pub const CURATOR_PLAN_SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "actions": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "properties": {
          "kind": { "type": "string", "enum": ["merge", "demote", "archive"] },
          "skills": { "type": "array", "items": { "type": "string" } },
          "target": { "type": ["string", "null"] },
          "rationale": { "type": "string" }
        },
        "required": ["kind", "skills", "rationale"]
      }
    }
  },
  "required": ["actions"]
}"#;

/// System prompt for the read-only `report_only` curator pass. Same analysis as
/// `CURATOR_SYSTEM_PROMPT`, but the model PROPOSES instead of writing.
pub const CURATOR_REPORT_PROMPT: &str = "\
You are the Right Agent skill CURATOR in REPORT-ONLY mode. Analyze the \
inventory and propose consolidations of agent-created `rightx-*` skills. \
Prefer broader umbrella skills over narrow near-duplicates.

You MUST NOT write, move, archive, or edit any file. Use only the `Read` tool \
to inspect specific `SKILL.md` bodies. Return your plan as JSON matching the \
provided schema: a list of proposed actions, each with `kind` \
(merge|demote|archive), the `skills` involved, an optional umbrella `target`, \
and a one-sentence `rationale`. Do NOT propose touching skills with \
`created_by=\"foreground\"`, `\"bundled\"`, or `pinned=true`.
";
```
If `agent_def`'s sibling consts (e.g. `CURATOR_SYSTEM_PROMPT`) are re-exported in `crates/right-codegen/src/lib.rs`, add `CURATOR_PLAN_SCHEMA, CURATOR_REPORT_PROMPT` to that re-export list in the same style.

- [ ] **Step 2: Add a schema sanity test**

In `crates/right-codegen/src/agent_def_tests.rs` (mirror the existing curator prompt assertions):
```rust
    #[test]
    fn curator_plan_schema_is_valid_json_with_actions() {
        let v: serde_json::Value =
            serde_json::from_str(right_codegen::CURATOR_PLAN_SCHEMA).unwrap();
        assert!(v["properties"]["actions"].is_object());
    }

    #[test]
    fn curator_report_prompt_forbids_writes() {
        let p = right_codegen::CURATOR_REPORT_PROMPT;
        assert!(p.contains("MUST NOT write"));
        assert!(p.contains("Read"));
    }
```

- [ ] **Step 3: Run red→green**

Run: `devenv shell -- cargo nextest run -p right-codegen curator_plan_schema curator_report_prompt`
Expected: PASS after Step 1.

- [ ] **Step 4: Commit**

```bash
git add crates/right-codegen/src/agent_def.rs crates/right-codegen/src/lib.rs crates/right-codegen/src/agent_def_tests.rs
git commit -m "feat(codegen): curator report-only plan schema + prompt"
```

### Task 14: Report-only branch in `run_if_due`

**Files:**
- Modify: `crates/bot/src/cc/stream.rs` (shared `last_result_line`)
- Modify: `crates/bot/src/learning_curator.rs` (mode dispatch + report-only pass + plan parse)

- [ ] **Step 1: Add `last_result_line` to `cc::stream`**

In `crates/bot/src/cc/stream.rs`:
```rust
/// Return the last `{"type":"result", …}` NDJSON line from a stream-json
/// stdout, if any. (A private copy exists in `learning_probe_writer`; that one
/// is pre-existing and left untouched.)
pub(crate) fn last_result_line(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .rfind(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|t| t == "result"))
                .unwrap_or(false)
        })
        .map(ToOwned::to_owned)
}
```

- [ ] **Step 2: Write the failing plan-parse test**

Add to `learning_curator.rs` `mod tests`:
```rust
    #[test]
    fn parse_curator_plan_extracts_actions_from_result_line() {
        let stdout = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\"}\n",
            "{\"type\":\"result\",\"structured_output\":{\"actions\":[{\"kind\":\"merge\",\"skills\":[\"rightx-a\",\"rightx-b\"],\"target\":\"rightx-u\",\"rationale\":\"dupes\"}]}}\n",
        );
        let plan = parse_curator_plan(stdout).unwrap();
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].kind, "merge");
        assert_eq!(plan.actions[0].target.as_deref(), Some("rightx-u"));
    }

    #[test]
    fn parse_curator_plan_none_when_no_result() {
        assert!(parse_curator_plan("{\"type\":\"system\"}\n").is_none());
    }
```

- [ ] **Step 3: Run red**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator::tests::parse_curator_plan`
Expected: FAIL — `parse_curator_plan`/`CuratorPlan` not found.

- [ ] **Step 4: Implement the plan types + parser**

In `learning_curator.rs`:
```rust
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct CuratorPlanAction {
    pub kind: String,
    pub skills: Vec<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct CuratorPlan {
    pub actions: Vec<CuratorPlanAction>,
}

/// Parse the structured curator plan from a report-only fork's stdout. Reads the
/// terminal result line and prefers `structured_output`, falling back to
/// `result` (same convention as `worker_reply::parse_reply_output`).
pub(crate) fn parse_curator_plan(stdout: &str) -> Option<CuratorPlan> {
    let line = crate::cc::stream::last_result_line(stdout)?;
    let v: serde_json::Value = serde_json::from_str(&line).ok()?;
    let plan_val = v
        .get("structured_output")
        .filter(|x| !x.is_null())
        .or_else(|| v.get("result"))?;
    serde_json::from_value::<CuratorPlan>(plan_val.clone()).ok()
}
```

- [ ] **Step 5: Run green (parser)**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator::tests::parse_curator_plan`
Expected: PASS.

- [ ] **Step 6: Add `mode` to `CuratorConfig` + ticker map**

In `learning_curator.rs` `CuratorConfig`, add:
```rust
    pub mode: right_agent_config::CuratorMode,
```
In the test `cfg()` helper add `mode: right_agent_config::CuratorMode::Apply,`. In `crates/bot/src/lib.rs` ticker `CuratorConfig { … }` add:
```rust
                        mode: curator_learning.curator_mode,
```

- [ ] **Step 7: Build the report-only invocation + dispatch**

In `run_if_due`, immediately after the gate yields `trigger` (after `state.last_spike_evidence_json = …`), branch on mode:
```rust
    if ctx.config.mode == right_agent_config::CuratorMode::ReportOnly {
        run_report_only_pass(&ctx, &conn, &mut state, &trigger, now).await;
        return;
    }
```
Add the report-only pass function (registers a `Curator` invocation for the MCP config it must carry per the `ClaudeInvocation` invariant, runs a read-only fork, parses the plan, writes one `proposed` `curator_runs` row, and writes nothing to disk or `skill_lifecycle`):
```rust
async fn run_report_only_pass(
    ctx: &CuratorContext,
    conn: &right_db::Connection,
    state: &mut CuratorState,
    trigger: &CuratorTrigger,
    now: DateTime<Utc>,
) {
    let lifecycle_rows = match right_lifecycle::list_curator_candidates(conn).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "report-only candidate read failed: {e:#}");
            return;
        }
    };
    let active = match crate::cc::invocation::register_non_foreground_invocation(
        crate::cc::invocation::NonForegroundInvocationRegistration {
            agent_name: ctx.agent_name.clone(),
            agent_dir: ctx.agent_dir.clone(),
            ssh_config_path: ctx.ssh_config_path.clone(),
            resolved_sandbox: ctx.resolved_sandbox.clone(),
            internal_client: Arc::clone(&ctx.internal_client),
            kind: right_mcp::internal_client::ProgressInvocationKindDto::Curator,
            chat_id: None,
            thread_id: None,
        },
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "report-only registration failed: {e:#}");
            return;
        }
    };
    let invocation = build_report_only_invocation(
        ctx,
        &lifecycle_rows,
        active.mcp_config_path().to_owned(),
    );
    let args = invocation.into_args();
    let mut cmd = match crate::cc::invocation::build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "skipping report-only curator: {e:#}");
            active.cleanup().await;
            return;
        }
    };
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let (actions_json, action_count, cost, cache_r, cache_c) =
        match right_process::ProcessGroupChild::spawn(cmd) {
            Ok(child) => match crate::cc::invocation::wait_with_output_or_kill(child, CURATOR_TIMEOUT).await {
                Ok(crate::cc::invocation::ChildOutput::Completed(output)) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                    if let Some(b) = crate::cc::stream::parse_usage_full(&stdout)
                        && let Err(e) =
                            right_agent::usage::insert::insert_learning_curator(conn, &b).await
                    {
                        tracing::warn!(agent = %ctx.agent_name, "report-only usage insert failed: {e:#}");
                    }
                    let usage = crate::cc::stream::parse_usage_full(&stdout);
                    let plan = parse_curator_plan(&stdout).unwrap_or(CuratorPlan { actions: vec![] });
                    let count = plan.actions.len() as i64;
                    let json = serde_json::to_string(&plan.actions).unwrap_or_else(|_| "[]".to_owned());
                    let (cost, cr, cc) = usage
                        .map(|b| (b.total_cost_usd, b.cache_read_tokens as i64, b.cache_creation_tokens as i64))
                        .unwrap_or((0.0, 0, 0));
                    (json, count, cost, cr, cc)
                }
                _ => ("[]".to_owned(), 0, 0.0, 0, 0),
            },
            Err(e) => {
                tracing::warn!(agent = %ctx.agent_name, "report-only spawn failed: {e:#}");
                ("[]".to_owned(), 0, 0.0, 0, 0)
            }
        };
    active.cleanup().await;

    let record = CuratorRunRecord {
        run_at: now.to_rfc3339(),
        trigger: trigger_label(trigger).to_owned(),
        trigger_evidence_json: state.last_spike_evidence_json.clone(),
        mode: "report_only".to_owned(),
        status: "proposed".to_owned(),
        cost_usd: cost,
        cache_read: cache_r,
        cache_creation: cache_c,
        consolidations: 0,
        archives: 0,
        summary: Some(format!("{action_count} proposals")),
        actions_json,
        invocation_id: None,
    };
    if let Err(e) = insert_curator_run(conn, &record).await {
        tracing::warn!(agent = %ctx.agent_name, "report-only curator_runs insert failed: {e:#}");
    }
    // Report-only never writes lifecycle/disk; still advance the gate clock.
    state.last_run_at = Some(now.to_rfc3339());
    state.last_run_status = Some("proposed".to_owned());
    if let Err(e) = save_state_db(conn, state).await {
        tracing::warn!(agent = %ctx.agent_name, "report-only save state failed: {e:#}");
    }
}

fn build_report_only_invocation(
    ctx: &CuratorContext,
    lifecycle_rows: &[right_lifecycle::SkillLifecycleRow],
    mcp_config_path: String,
) -> crate::cc::invocation::ClaudeInvocation {
    use crate::cc::invocation::{ClaudeInvocation, OutputFormat};
    let session_id = uuid::Uuid::new_v4().to_string();
    let user_prompt = format!(
        "{system}\n\n{candidates}",
        system = right_codegen::CURATOR_REPORT_PROMPT,
        candidates = render_candidate_list(lifecycle_rows),
    );
    ClaudeInvocation {
        mcp_config_path: Some(mcp_config_path),
        json_schema: Some(right_codegen::CURATOR_PLAN_SCHEMA.to_owned()),
        output_format: OutputFormat::StreamJson,
        model: Some(ctx.model.clone()),
        max_budget_usd: None,
        max_turns: Some(CURATOR_MAX_TURNS),
        resume_session_id: None,
        new_session_id: Some(session_id),
        fork_session: false,
        allowed_tools: vec!["Read".into()],
        disallowed_tools: vec![],
        extra_args: vec![],
        prompt: Some(user_prompt),
        debug_flag: Some(Arc::clone(&ctx.debug_flag)),
    }
}
```

- [ ] **Step 8: Verify compile + curator suite**

Run: `devenv shell -- cargo nextest run -p right-bot learning_curator`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/bot/src/cc/stream.rs crates/bot/src/learning_curator.rs crates/bot/src/lib.rs
git commit -m "feat(curator): report_only mode — read-only plan pass, proposed curator_runs row, no writes"
```

---

## Phase 7 — Wizard, docs, final verification

### Task 15: Wizard prompts for the three knobs

**Files:**
- Modify: `crates/right/src/wizard.rs`

Replace the three passthrough assignments from Task 4 Step 7 with real prompts, following the existing `curator_*` prompt pattern (each wrapped in `right_agent::init::inquire_back`).

- [ ] **Step 1: Add the prompts**

Before the `Ok(Some(LearningConfig { … }))` literal, add (mirroring `curator_skill_change_threshold` / `curator_min_cooldown_hours` prompt style):
```rust
    // curator_circuit_failure_threshold
    let Some(curator_circuit_failure_threshold_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator circuit: consecutive failures before the circuit opens:")
            .with_default(&existing.curator_circuit_failure_threshold.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_circuit_failure_threshold = parse_u32_positive(
        curator_circuit_failure_threshold_input.trim(),
        existing.curator_circuit_failure_threshold,
        "curator circuit failure threshold",
    )?;

    // curator_circuit_cooldown_hours
    let Some(curator_circuit_cooldown_hours_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator circuit: cooldown hours while open:")
            .with_default(&existing.curator_circuit_cooldown_hours.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_circuit_cooldown_hours = parse_u32_positive(
        curator_circuit_cooldown_hours_input.trim(),
        existing.curator_circuit_cooldown_hours,
        "curator circuit cooldown hours",
    )?;

    // curator_mode
    let mode_default = match existing.curator_mode {
        right_agent::agent::types::CuratorMode::Apply => "apply",
        right_agent::agent::types::CuratorMode::ReportOnly => "report_only",
    };
    let Some(curator_mode_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator mode (apply | report_only):")
            .with_default(mode_default)
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_mode = match curator_mode_input.trim() {
        "report_only" => right_agent::agent::types::CuratorMode::ReportOnly,
        _ => right_agent::agent::types::CuratorMode::Apply,
    };
```

- [ ] **Step 2: Replace the struct-literal passthrough with the parsed values**

In the `LearningConfig { … }` literal, change the three Task-4 lines to:
```rust
        curator_circuit_failure_threshold,
        curator_circuit_cooldown_hours,
        curator_mode,
```

- [ ] **Step 3: Verify build**

Run: `devenv shell -- cargo build -p right`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/wizard.rs
git commit -m "feat(wizard): prompt for curator circuit knobs + mode"
```

### Task 16: Docs sync + final full-workspace verification

**Files:**
- Modify: `docs/architecture/learning.md` (curator section)
- Modify: `PROMPT_SYSTEM.md` (report-only plan schema/prompt, if it documents curator prompts)
- Modify: `ARCHITECTURE.md` (only if a new contract line is warranted — keep ≤3 sentences; check the 40k budget)

- [ ] **Step 1: Update `docs/architecture/learning.md`**

In the curator subsection, document: the `curator_runs` history table (append-only, one row per executed pass, distinct from the `curator_state` singleton); the live circuit breaker (`curator_circuit_failure_threshold`/`_cooldown_hours`, failures persist across opens); the idle gate now fed by `IdleTimestamp`; and `curator_mode: apply | report_only` (read-only plan pass writing a `proposed` `curator_runs` row, archive-not-delete preserved).

- [ ] **Step 2: Update `PROMPT_SYSTEM.md`**

If `PROMPT_SYSTEM.md` enumerates curator prompts, add `CURATOR_REPORT_PROMPT` + `CURATOR_PLAN_SCHEMA` (report-only, read-only `Read`-only, returns a plan; writes nothing). Run `rg -n -i 'curator' PROMPT_SYSTEM.md` to find the right section; if curator prompts are not enumerated there, skip this step and note it in the commit.

- [ ] **Step 3: Consider `ARCHITECTURE.md`**

Only if a load-bearing contract changed: add a ≤3-sentence note that `curator_mode = report_only` MUST NOT write to disk or `skill_lifecycle` (proposes via `curator_runs`). Check `wc -c ARCHITECTURE.md` stays < 40000; if it would exceed, put the detail in `docs/architecture/learning.md` and link by plain path instead.

- [ ] **Step 4: Final full-workspace verification (mandatory)**

Run:
```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo nextest run --workspace
devenv shell -- cargo test --doc --workspace
cd crates/right-dashboard/frontend && devenv shell -- pnpm test
```
Expected: all green. Re-run any parallel-load-flaky test in isolation before concluding (the `cc/invocation` pid-race test and the `right-dashboard` warn-count test are known-flaky, unrelated to this change).

- [ ] **Step 5: Commit**

```bash
git add docs/architecture/learning.md PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "docs(curator): document run history, circuit breaker, idle gate, report-only mode"
```

---

## Self-review checklist (completed by plan author)

- **Spec coverage:** A1 → Tasks 8-12; B1 → Tasks 5-7; B2 → Tasks 2-3; B3 → Tasks 13-14; config/wizard → Tasks 4, 15; archive-not-delete invariant → preserved (report-only writes nothing; apply path unchanged); docs sync → Task 16. Non-goals (A2, per-item approval, recover #134, quality #132) are not implemented, by design.
- **Open questions resolved:** `actions_json` shape (Task 10 apply: `{kind:merge|archive, skills, target}`; Task 14 report-only: the plan's actions array); plan JSON schema (Task 13); apply-mode counts (Task 10: `archives` = all archived this pass, `consolidations` = subset with `absorbed_into` — a subset, never additive, so no double-count).
- **Type consistency:** `CuratorMode` (config) used verbatim in `CuratorConfig` and the wizard; `CuratorRunRecord`/`insert_curator_run` defined in Task 9 and reused in Tasks 10/14; `CuratorRunSummary`/`CuratorConsolidation` defined in Task 11 (Rust) and Task 12 (TS) with matching field names; `last_result_line` added once in `cc::stream` and reused; `next_circuit_open_until`/`mark_failed_run` defined Task 5/7 and consistent.
- **No placeholders:** every code step carries complete code; verification steps carry exact commands and expected outcomes.
