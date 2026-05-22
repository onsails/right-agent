# Prefilter Classifier + Curator State Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the per-turn skill-learning pipeline so the Haiku prefilter emits a three-way structured decision (Skip / PatchExisting / CreateNew) informed by per-agent turn-stat baselines, the probe-writer accepts those decisions as directed hints, and the curator's state migrates from a JSON file to `data.db` with a multi-signal trigger gate.

**Architecture:** A new `turn_baseline` module in `right-agent::usage` computes per-agent P50/P90/P99 percentiles for foreground turns over the last 14 days. The prefilter Haiku invocation gets restructured around a 3-way enum with conditional JSON-schema requireds. Probe-writer takes the decision as a hint and may refuse. Curator state moves to a new `curator_state` singleton row in per-agent `data.db`; the trigger gate grows three OR-conditions (cost spike vs 14d P50, skill-change count, 168h fallback) plus a cooldown floor.

**Tech Stack:** Rust 2024, rusqlite + rusqlite_migration, serde, chrono, tokio. Existing patterns from prior `2026-05-22-skill-learning-writer-curator` work (lifecycle module, FleetView codegen contract, Telegram worker fire-and-forget).

**Spec:** `docs/superpowers/specs/2026-05-22-prefilter-classifier-and-curator-state-design.md`

---

## File Map

**New files:**

- `crates/right-agent/src/usage/turn_baseline.rs` — `TurnBaselines`, `BaselineMetric<T>`, `compute()`.
- `crates/right-db/src/sql/v28_usage_wall_elapsed.sql` — empty schema (the v28 migration is hook-only; conditional ALTER).
- `crates/right-db/src/sql/v29_curator_state.sql` — `CREATE TABLE IF NOT EXISTS curator_state`.

**Modified files:**

- `crates/right-db/src/migrations.rs` — register v28 + v29; add hook for the conditional `wall_elapsed_ms` ALTER.
- `crates/right-agent/src/usage/mod.rs` — `UsageBreakdown.wall_elapsed_ms` field; re-export `turn_baseline`.
- `crates/right-agent/src/usage/insert.rs` — thread `wall_elapsed_ms` through `insert_row`.
- `crates/right-agent-config/src/lib.rs` — new `LearningConfig` fields with `#[serde(default)]`.
- `crates/bot/src/cc/stream.rs` — `UsageBreakdown` parsing adds `wall_elapsed_ms` from outside-the-stream (worker-supplied).
- `crates/bot/src/telegram/worker.rs` — extend `ProbeAnchor`; measure wall-elapsed; parse `use_skill` receipts.
- `crates/bot/src/learning_prefilter.rs` — reshape `PrefilterDecision` to 3-way enum; new schema; baselines in prompt; conditional validation in parser.
- `crates/bot/src/learning_probe_writer.rs` — `ProbeWriterContext.incoming_hint`; prompt branches.
- `crates/bot/src/learning_curator.rs` — DB-backed state; multi-signal gate; evidence capture.
- `crates/right/src/right_backend.rs` — `skill_learning_finish` accepts `hint_outcome`.
- `crates/right/src/wizard.rs` — prompts for new `LearningConfig` fields.
- `crates/right-codegen/src/agent_def.rs` — update embedded `PREFILTER_SCHEMA_JSON` constant if mirrored.
- `ARCHITECTURE.md` — update "Skill learning loop" subsection.
- `PROMPT_SYSTEM.md` — describe new `PREFILTER_SCHEMA_JSON`; TURN STATS rendering; hint-aware prompts.

---

## Verification Cadence

Per `AGENTS.md`:

- **Worktree start (Task 0):** one targeted baseline test on the crates this plan touches; record pre-existing failures.
- **Per-task:** narrowest useful test (`devenv shell -- cargo test -p <crate> <filter>`).
- **End of plan (final task):** `devenv shell -- cargo test --workspace` is mandatory.

Do **not** run full-workspace tests after every task.

---

### Task 0: Worktree baseline

**Files:** none

- [ ] **Step 1: Verify worktree state and run targeted baseline**

Run from inside the worktree (`.worktrees/learning-fork-probe`):

```bash
devenv shell -- cargo test -p right-db -p right-agent -p right-agent-config -p right-bot --no-fail-fast 2>&1 | tail -50
```

Expected: all tests pass (this branch was previously clean per `2026-05-22-skill-learning-writer-curator` plan). If any are failing pre-existing, record them in a one-line note here for later comparison.

- [ ] **Step 2: Confirm spec is committed**

Run: `git log --oneline -1 docs/superpowers/specs/2026-05-22-prefilter-classifier-and-curator-state-design.md`

Expected: shows the spec commit.

---

### Task 1: SQLite migration v28 — `usage_events.wall_elapsed_ms`

**Files:**
- Create: `crates/right-db/src/sql/v28_usage_wall_elapsed.sql` (empty body — pure hook migration)
- Modify: `crates/right-db/src/migrations.rs`
- Test: same file (`crates/right-db/src/migrations.rs::tests`)

- [ ] **Step 1: Write failing test**

Append to `crates/right-db/src/migrations.rs` test module:

```rust
#[test]
fn v28_adds_wall_elapsed_ms_column_idempotently() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 27).unwrap();
    let pre: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('usage_events') WHERE name = ?1",
            ["wall_elapsed_ms"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pre, 0, "wall_elapsed_ms must not exist at v27");

    MIGRATIONS.to_version(&mut conn, 28).unwrap();
    let post: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('usage_events') WHERE name = ?1",
            ["wall_elapsed_ms"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(post, 1, "wall_elapsed_ms must exist at v28");

    let notnull: i64 = conn
        .query_row(
            "SELECT \"notnull\" FROM pragma_table_info('usage_events') WHERE name = ?1",
            ["wall_elapsed_ms"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(notnull, 0, "wall_elapsed_ms must be nullable");

    // Re-run is no-op.
    MIGRATIONS.to_version(&mut conn, 28).unwrap();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-db v28_adds_wall_elapsed_ms_column_idempotently`

Expected: FAIL (migration v28 not registered).

- [ ] **Step 3: Create empty schema file**

```bash
touch crates/right-db/src/sql/v28_usage_wall_elapsed.sql
```

The hook does the conditional ALTER; the SQL file exists for symmetry with other registry slots.

- [ ] **Step 4: Add hook in `migrations.rs`**

Append the hook (place it after `v27_skill_nudge_signals_source`, before `MIGRATIONS`):

```rust
/// v28: Add nullable `wall_elapsed_ms` column to `usage_events`.
///
/// Idempotent — checks pragma_table_info before ALTER. Foreground worker
/// turns populate this; non-foreground sources leave NULL.
fn v28_usage_wall_elapsed_ms(tx: &Transaction) -> Result<(), HookError> {
    let count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('usage_events') WHERE name = ?1",
        ["wall_elapsed_ms"],
        |r| r.get(0),
    )?;
    if count == 0 {
        tx.execute_batch("ALTER TABLE usage_events ADD COLUMN wall_elapsed_ms INTEGER")?;
    }
    Ok(())
}
```

Register in the `MIGRATIONS` vec (after `v27_skill_nudge_signals_source`):

```rust
        M::up_with_hook("", v28_usage_wall_elapsed_ms),
```

- [ ] **Step 5: Verify test passes**

Run: `devenv shell -- cargo test -p right-db v28_adds_wall_elapsed_ms_column_idempotently`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-db/src/sql/v28_usage_wall_elapsed.sql crates/right-db/src/migrations.rs
git commit -m "feat(db): v28 migration adds wall_elapsed_ms to usage_events"
```

---

### Task 2: SQLite migration v29 — `curator_state` singleton table

**Files:**
- Create: `crates/right-db/src/sql/v29_curator_state.sql`
- Modify: `crates/right-db/src/migrations.rs`

- [ ] **Step 1: Write failing test**

Append to `crates/right-db/src/migrations.rs` test module:

```rust
#[test]
fn v29_creates_curator_state_singleton_table_idempotently() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 28).unwrap();
    let pre: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='curator_state'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pre, 0);

    MIGRATIONS.to_version(&mut conn, 29).unwrap();
    let post: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='curator_state'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(post, 1);

    // Singleton CHECK constraint: id=2 must fail.
    let err = conn.execute(
        "INSERT INTO curator_state (agent_singleton_id, last_run_at) VALUES (2, NULL)",
        [],
    );
    assert!(err.is_err(), "CHECK constraint must reject id != 1");

    // id=1 must succeed.
    conn.execute(
        "INSERT INTO curator_state (agent_singleton_id, last_run_at) VALUES (1, '2026-05-22T00:00:00Z')",
        [],
    )
    .unwrap();

    // Re-run is no-op.
    MIGRATIONS.to_version(&mut conn, 29).unwrap();
}
```

- [ ] **Step 2: Run test, verify FAIL**

Run: `devenv shell -- cargo test -p right-db v29_creates_curator_state`

Expected: FAIL (table not created).

- [ ] **Step 3: Create schema file**

Write `crates/right-db/src/sql/v29_curator_state.sql`:

```sql
CREATE TABLE IF NOT EXISTS curator_state (
    agent_singleton_id        INTEGER PRIMARY KEY CHECK (agent_singleton_id = 1),
    last_run_at               TEXT,
    last_run_status           TEXT,
    consecutive_failures      INTEGER NOT NULL DEFAULT 0,
    circuit_open_until        TEXT,
    last_spike_evidence_json  TEXT
);
```

- [ ] **Step 4: Wire into `migrations.rs`**

Add the include + register entry (mirroring how `V27_SCHEMA` is loaded). Near the top of the file with other `const V<N>_SCHEMA`:

```rust
const V29_SCHEMA: &str = include_str!("sql/v29_curator_state.sql");
```

In the `MIGRATIONS` vec, after `M::up_with_hook("", v28_usage_wall_elapsed_ms)`:

```rust
        M::up(V29_SCHEMA),
```

- [ ] **Step 5: Verify test passes**

Run: `devenv shell -- cargo test -p right-db v29_creates_curator_state`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-db/src/sql/v29_curator_state.sql crates/right-db/src/migrations.rs
git commit -m "feat(db): v29 migration adds curator_state singleton table"
```

---

### Task 3: `UsageBreakdown.wall_elapsed_ms` + insert plumbing

**Files:**
- Modify: `crates/right-agent/src/usage/mod.rs`
- Modify: `crates/right-agent/src/usage/insert.rs`

- [ ] **Step 1: Write failing test**

Append to `crates/right-agent/src/usage/insert.rs::tests`:

```rust
#[test]
fn insert_threads_wall_elapsed_ms_when_set() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    let mut b = sample_breakdown();
    b.wall_elapsed_ms = Some(12345);
    insert_interactive(&conn, &b, 1, 0).unwrap();

    let elapsed: Option<i64> = conn
        .query_row(
            "SELECT wall_elapsed_ms FROM usage_events LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(elapsed, Some(12345));
}

#[test]
fn insert_keeps_wall_elapsed_ms_null_when_none() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    insert_learning_curator(&conn, &sample_breakdown()).unwrap();
    let elapsed: Option<i64> = conn
        .query_row(
            "SELECT wall_elapsed_ms FROM usage_events LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(elapsed, None);
}
```

Update `sample_breakdown()` to include the new field with `wall_elapsed_ms: None`.

- [ ] **Step 2: Run tests, verify FAIL**

Run: `devenv shell -- cargo test -p right-agent insert_threads_wall_elapsed_ms_when_set`

Expected: FAIL (compile error — `wall_elapsed_ms` does not exist).

- [ ] **Step 3: Add field to `UsageBreakdown`**

In `crates/right-agent/src/usage/mod.rs`, in the `UsageBreakdown` struct:

```rust
pub struct UsageBreakdown {
    pub session_uuid: String,
    pub total_cost_usd: f64,
    pub num_turns: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub model_usage_json: String,
    pub api_key_source: String,
    /// Wall-clock latency from CC spawn to result event. Populated only for
    /// foreground worker turns; other sources leave it `None` and the column
    /// remains NULL.
    pub wall_elapsed_ms: Option<u64>,
}
```

Update the `usage_breakdown_has_api_key_source_field` test fixture (add `wall_elapsed_ms: None`).

- [ ] **Step 4: Update `insert_row`**

In `crates/right-agent/src/usage/insert.rs`, update the SQL and `params!` block:

```rust
fn insert_row(
    conn: &Connection,
    b: &UsageBreakdown,
    source: &str,
    chat_id: Option<i64>,
    thread_id: Option<i64>,
    job_name: Option<&str>,
) -> Result<(), UsageError> {
    let ts = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name,
            session_uuid, total_cost_usd, num_turns,
            input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens,
            web_search_requests, web_fetch_requests,
            model_usage_json, api_key_source,
            wall_elapsed_ms
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, ?7, ?8,
            ?9, ?10,
            ?11, ?12,
            ?13, ?14,
            ?15, ?16,
            ?17
         )",
        params![
            ts,
            source,
            chat_id,
            thread_id,
            job_name,
            b.session_uuid,
            b.total_cost_usd,
            b.num_turns as i64,
            b.input_tokens as i64,
            b.output_tokens as i64,
            b.cache_creation_tokens as i64,
            b.cache_read_tokens as i64,
            b.web_search_requests as i64,
            b.web_fetch_requests as i64,
            b.model_usage_json,
            b.api_key_source,
            b.wall_elapsed_ms.map(|v| v as i64),
        ],
    )?;
    Ok(())
}
```

- [ ] **Step 5: Update existing `sample_breakdown` and parsing callsites**

In `crates/bot/src/cc/stream.rs` — the `UsageBreakdown` factories that construct via field names need `wall_elapsed_ms: None` added. Use `cargo check` to find:

```bash
devenv shell -- cargo check -p right-agent -p right-bot 2>&1 | grep -i "missing field\|wall_elapsed_ms"
```

Fix each callsite by adding `wall_elapsed_ms: None`. Stream parsers leave it None — worker injects later.

- [ ] **Step 6: Verify tests pass**

Run: `devenv shell -- cargo test -p right-agent -p right-bot --no-fail-fast`

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/right-agent/src/usage/mod.rs crates/right-agent/src/usage/insert.rs crates/bot/src/cc/stream.rs
git commit -m "feat(usage): UsageBreakdown carries wall_elapsed_ms (foreground-only)"
```

---

### Task 4: `turn_baseline` module — percentile computation

**Files:**
- Create: `crates/right-agent/src/usage/turn_baseline.rs`
- Modify: `crates/right-agent/src/usage/mod.rs` (add `pub mod turn_baseline;`)

- [ ] **Step 1: Write failing tests**

Create `crates/right-agent/src/usage/turn_baseline.rs` with only the test module + stubs:

```rust
//! Per-agent statistical baselines for foreground turn metrics.

use crate::usage::error::UsageError;
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq)]
pub struct TurnBaselines {
    pub sample_size: u32,
    pub window_days: u32,
    pub cost_usd: BaselineMetric<f64>,
    pub num_turns: BaselineMetric<u32>,
    pub wall_elapsed_ms: BaselineMetric<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BaselineMetric<T> {
    Insufficient { sample_size: u32 },
    Available { p50: T, p90: T, p99: T },
}

pub fn compute(
    _conn: &Connection,
    _window_days: u32,
    _min_sample: u32,
) -> Result<TurnBaselines, UsageError> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::UsageBreakdown;
    use crate::usage::insert::insert_interactive;
    use right_db::open_connection;
    use tempfile::tempdir;

    fn sample(cost: f64, turns: u32, elapsed: Option<u64>) -> UsageBreakdown {
        UsageBreakdown {
            session_uuid: "s".into(),
            total_cost_usd: cost,
            num_turns: turns,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests: 0,
            web_fetch_requests: 0,
            model_usage_json: "{}".into(),
            api_key_source: "none".into(),
            wall_elapsed_ms: elapsed,
        }
    }

    #[test]
    fn compute_returns_insufficient_when_below_min_sample() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        for i in 0..5 {
            insert_interactive(&conn, &sample(0.01, 1, Some(i * 100)), 1, 0).unwrap();
        }
        let b = compute(&conn, 14, 20).unwrap();
        assert_eq!(b.sample_size, 5);
        assert_eq!(b.window_days, 14);
        assert!(matches!(
            b.cost_usd,
            BaselineMetric::Insufficient { sample_size: 5 }
        ));
        assert!(matches!(b.num_turns, BaselineMetric::Insufficient { .. }));
        assert!(matches!(
            b.wall_elapsed_ms,
            BaselineMetric::Insufficient { .. }
        ));
    }

    #[test]
    fn compute_returns_available_when_at_least_min_sample() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        for i in 0..50 {
            let cost = 0.01 * (i + 1) as f64;
            insert_interactive(&conn, &sample(cost, i as u32 + 1, Some((i + 1) * 100)), 1, 0).unwrap();
        }
        let b = compute(&conn, 14, 20).unwrap();
        assert_eq!(b.sample_size, 50);
        let cost_available = matches!(b.cost_usd, BaselineMetric::Available { .. });
        assert!(cost_available);
        if let BaselineMetric::Available { p50, p90, p99 } = b.cost_usd {
            assert!((p50 - 0.255).abs() < 1e-3, "p50: {p50}");
            assert!((p90 - 0.455).abs() < 1e-3, "p90: {p90}");
            assert!((p99 - 0.5).abs() < 1e-3 || (p99 - 0.495).abs() < 1e-3, "p99: {p99}");
        }
    }

    #[test]
    fn compute_excludes_non_foreground_sources() {
        use crate::usage::insert::{insert_cron, insert_learning_curator};
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        // 25 foreground + 25 cron + 25 curator; baseline counts only foreground.
        for _ in 0..25 {
            insert_interactive(&conn, &sample(0.10, 5, Some(1000)), 1, 0).unwrap();
        }
        for _ in 0..25 {
            insert_cron(&conn, &sample(0.99, 99, None), "j").unwrap();
        }
        for _ in 0..25 {
            insert_learning_curator(&conn, &sample(0.99, 99, None)).unwrap();
        }
        let b = compute(&conn, 14, 20).unwrap();
        assert_eq!(b.sample_size, 25);
        if let BaselineMetric::Available { p50, .. } = b.cost_usd {
            assert!((p50 - 0.10).abs() < 1e-9, "foreground only; p50 must be 0.10, got {p50}");
        } else {
            panic!("expected Available");
        }
    }

    #[test]
    fn compute_excludes_null_wall_elapsed_from_elapsed_baseline_only() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        // 30 rows total, 10 with NULL elapsed, 20 with elapsed.
        for _ in 0..10 {
            insert_interactive(&conn, &sample(0.05, 2, None), 1, 0).unwrap();
        }
        for i in 0..20 {
            insert_interactive(&conn, &sample(0.05, 2, Some((i + 1) * 100)), 1, 0).unwrap();
        }
        let b = compute(&conn, 14, 20).unwrap();
        assert_eq!(b.sample_size, 30);
        assert!(matches!(b.cost_usd, BaselineMetric::Available { .. }));
        // Elapsed baseline has 20 samples; passes min_sample=20.
        assert!(matches!(b.wall_elapsed_ms, BaselineMetric::Available { .. }));
    }
}
```

Add `pub mod turn_baseline;` to `crates/right-agent/src/usage/mod.rs`.

- [ ] **Step 2: Run tests, verify FAIL**

Run: `devenv shell -- cargo test -p right-agent turn_baseline`

Expected: FAIL — `compute` is `unimplemented!`.

- [ ] **Step 3: Implement `compute`**

Replace the `unimplemented!()` body:

```rust
pub fn compute(
    conn: &Connection,
    window_days: u32,
    min_sample: u32,
) -> Result<TurnBaselines, UsageError> {
    let window_cutoff = (chrono::Utc::now() - chrono::Duration::days(window_days as i64))
        .to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT total_cost_usd, num_turns, wall_elapsed_ms \
         FROM usage_events \
         WHERE source = 'interactive' AND ts >= ?1",
    )?;
    let rows = stmt.query_map([&window_cutoff], |r| {
        Ok((
            r.get::<_, f64>(0)?,
            r.get::<_, i64>(1)? as u32,
            r.get::<_, Option<i64>>(2)?.map(|v| v as u64),
        ))
    })?;
    let mut costs: Vec<f64> = Vec::new();
    let mut turns: Vec<u32> = Vec::new();
    let mut elapsed: Vec<u64> = Vec::new();
    for row in rows {
        let (c, t, e) = row?;
        costs.push(c);
        turns.push(t);
        if let Some(v) = e {
            elapsed.push(v);
        }
    }
    let sample_size = costs.len() as u32;
    let elapsed_sample_size = elapsed.len() as u32;
    Ok(TurnBaselines {
        sample_size,
        window_days,
        cost_usd: percentile_metric(&mut costs, sample_size, min_sample),
        num_turns: percentile_metric(&mut turns, sample_size, min_sample),
        wall_elapsed_ms: percentile_metric(&mut elapsed, elapsed_sample_size, min_sample),
    })
}

fn percentile_metric<T: Copy + PartialOrd>(
    values: &mut [T],
    sample_size: u32,
    min_sample: u32,
) -> BaselineMetric<T> {
    if sample_size < min_sample {
        return BaselineMetric::Insufficient { sample_size };
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(values, 0.50);
    let p90 = percentile(values, 0.90);
    let p99 = percentile(values, 0.99);
    BaselineMetric::Available { p50, p90, p99 }
}

fn percentile<T: Copy + PartialOrd>(sorted: &[T], q: f64) -> T {
    debug_assert!(!sorted.is_empty(), "percentile requires non-empty slice");
    let n = sorted.len();
    let idx = ((q * (n as f64 - 1.0)).round() as usize).min(n - 1);
    sorted[idx]
}
```

Crate-internal note: this code path uses `source = 'interactive'`. The plan's spec calls this "foreground" — confirm the variant `insert_interactive` writes `source = 'interactive'` (it does, per `insert.rs:17`). The dashboard surfaces this row class as "foreground"; the storage tag is `interactive`.

- [ ] **Step 4: Run tests, verify PASS**

Run: `devenv shell -- cargo test -p right-agent turn_baseline`

Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/usage/turn_baseline.rs crates/right-agent/src/usage/mod.rs
git commit -m "feat(usage): turn_baseline module computes per-agent P50/P90/P99"
```

---

### Task 5: Extend `LearningConfig` with new knobs

**Files:**
- Modify: `crates/right-agent-config/src/lib.rs`

- [ ] **Step 1: Write failing test**

Append to the `LearningConfig` test module (`crates/right-agent-config/src/lib.rs::tests`, near `default_learning_has_known_six_fields` or equivalent):

```rust
#[test]
fn default_learning_has_new_curator_trigger_fields() {
    let cfg = LearningConfig::default();
    assert!((cfg.curator_cost_spike_k - 3.0).abs() < 1e-9);
    assert_eq!(cfg.curator_cost_spike_baseline_days, 14);
    assert!((cfg.curator_cost_spike_min_floor_usd - 0.05).abs() < 1e-9);
    assert_eq!(cfg.curator_skill_change_threshold, 3);
    assert_eq!(cfg.curator_min_cooldown_hours, 12);
    assert_eq!(cfg.baseline_window_days, 14);
    assert_eq!(cfg.baseline_min_sample, 20);
}

#[test]
fn learning_yaml_accepts_missing_new_fields_via_defaults() {
    let yaml = "prefilter_enabled: true";
    let cfg: LearningConfig = serde_saphyr::from_str(yaml).unwrap();
    assert_eq!(cfg.curator_skill_change_threshold, 3);
    assert!((cfg.curator_cost_spike_k - 3.0).abs() < 1e-9);
}
```

- [ ] **Step 2: Run test, verify FAIL**

Run: `devenv shell -- cargo test -p right-agent-config default_learning_has_new_curator_trigger_fields`

Expected: FAIL (compile errors for missing fields).

- [ ] **Step 3: Add defaults + fields**

In `crates/right-agent-config/src/lib.rs`, add default fns near other `default_*`:

```rust
fn default_curator_cost_spike_k() -> f64 {
    3.0
}
fn default_curator_cost_spike_baseline_days() -> u32 {
    14
}
fn default_curator_cost_spike_min_floor_usd() -> f64 {
    0.05
}
fn default_curator_skill_change_threshold() -> u32 {
    3
}
fn default_curator_min_cooldown_hours() -> u32 {
    12
}
fn default_baseline_window_days() -> u32 {
    14
}
fn default_baseline_min_sample() -> u32 {
    20
}
```

Insert into the `LearningConfig` struct (placement after the existing curator fields, before `max_daily_budget_usd`):

```rust
    /// Multiplier on 14-day P50 probe-writer cost; ≥ k * P50 in last 24h
    /// triggers an early curator run.
    #[serde(default = "default_curator_cost_spike_k")]
    pub curator_cost_spike_k: f64,
    #[serde(
        default = "default_curator_cost_spike_baseline_days",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_cost_spike_baseline_days: u32,
    /// Absolute floor on 24h probe-writer spend below which the cost-spike
    /// trigger never fires — protects low-activity agents.
    #[serde(default = "default_curator_cost_spike_min_floor_usd")]
    pub curator_cost_spike_min_floor_usd: f64,
    /// Skills created/patched since last curator run; ≥ threshold triggers a
    /// run.
    #[serde(
        default = "default_curator_skill_change_threshold",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_skill_change_threshold: u32,
    /// Hard cooldown between curator runs — gates every trigger including the
    /// 168h fallback.
    #[serde(
        default = "default_curator_min_cooldown_hours",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_min_cooldown_hours: u32,

    /// Window for prefilter per-agent turn baselines.
    #[serde(
        default = "default_baseline_window_days",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub baseline_window_days: u32,
    /// Minimum sample size in window for prefilter baselines to be considered
    /// sufficient. Below this, the prompt notes "baseline insufficient."
    #[serde(
        default = "default_baseline_min_sample",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub baseline_min_sample: u32,
```

Update the `Default` impl to add these fields with the same defaults.

- [ ] **Step 4: Run tests, verify PASS**

Run: `devenv shell -- cargo test -p right-agent-config`

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent-config/src/lib.rs
git commit -m "feat(config): LearningConfig adds curator trigger + baseline knobs"
```

---

### Task 6: Extend `ProbeAnchor` with stats + receipts

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/learning_prefilter.rs` (test fixture)

- [ ] **Step 1: Update struct**

In `crates/bot/src/telegram/worker.rs` near line 232, replace the `ProbeAnchor` struct:

```rust
/// Snapshot of one foreground turn, captured after the assistant reply was
/// sent. Consumed by the prefilter and (if it returns non-Skip) the
/// probe-writer.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ProbeAnchor {
    pub user_msg_text: String,
    pub assistant_reply_text: String,
    pub main_session_uuid: String,
    pub captured_at: DateTime<Utc>,
    pub chat_id: i64,
    pub thread_id: i64,
    /// num_turns from the foreground CC `result` event.
    pub num_turns: u32,
    /// total_cost_usd from the foreground CC `result` event.
    pub total_cost_usd: f64,
    /// Wall-clock from CC spawn to result event in milliseconds.
    pub wall_elapsed_ms: u64,
    /// `rightx-<slug>` skill names the foreground turn used (extracted from
    /// `mcp__right__use_skill` tool calls in the stream).
    pub used_skill_receipts: Vec<String>,
}
```

- [ ] **Step 2: Update `learning_prefilter.rs` test fixture**

In `crates/bot/src/learning_prefilter.rs::tests`, update `fn anchor(user: &str, assistant: &str)`:

```rust
fn anchor(user: &str, assistant: &str) -> ProbeAnchor {
    ProbeAnchor {
        user_msg_text: user.into(),
        assistant_reply_text: assistant.into(),
        main_session_uuid: "uuid-main".into(),
        captured_at: chrono::Utc::now(),
        chat_id: 1,
        thread_id: 0,
        num_turns: 1,
        total_cost_usd: 0.0,
        wall_elapsed_ms: 0,
        used_skill_receipts: Vec::new(),
    }
}
```

- [ ] **Step 3: Update other `ProbeAnchor` construction sites**

Run `cargo check` to find:

```bash
devenv shell -- cargo check -p right-bot 2>&1 | grep "missing field" | head -20
```

For each callsite, add the new fields. The primary construction is in `worker.rs:1463` inside `post_turn_probe_anchor = Some(ProbeAnchor { ... })`. Update it to populate from `usage`:

```rust
post_turn_probe_anchor = Some(ProbeAnchor {
    user_msg_text: user_msg_text.clone(),
    assistant_reply_text: assistant_reply_text.clone(),
    main_session_uuid: main_session_uuid.clone(),
    captured_at: chrono::Utc::now(),
    chat_id,
    thread_id,
    num_turns: usage.num_turns,
    total_cost_usd: usage.total_cost_usd,
    wall_elapsed_ms: turn_wall_elapsed_ms,  // captured earlier — see Task 7
    used_skill_receipts: used_skill_receipts.clone(), // collected in Task 7
});
```

For now, since `turn_wall_elapsed_ms` and `used_skill_receipts` aren't yet captured, supply placeholders to keep the build green:

```rust
    num_turns: usage.num_turns,
    total_cost_usd: usage.total_cost_usd,
    wall_elapsed_ms: 0,
    used_skill_receipts: Vec::new(),
```

These get wired in Task 7.

- [ ] **Step 4: Run tests, verify PASS (existing tests)**

Run: `devenv shell -- cargo test -p right-bot learning_prefilter`

Expected: existing prefilter tests pass (no new behavior asserted yet).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs crates/bot/src/learning_prefilter.rs
git commit -m "feat(bot): ProbeAnchor carries turn stats + used skill receipts"
```

---

### Task 7: Worker measures wall-elapsed + parses use_skill receipts

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/cc/stream.rs` (if receipt parsing belongs here)

- [ ] **Step 1: Inspect stream event shape for `use_skill`**

Run: `grep -n "use_skill\|skill_use\|mcp__right__use_skill" crates/bot/src/cc/stream.rs crates/bot/src/cc/worker_reply.rs | head -10`

The receipts are already detected (`worker_reply.rs::append_used_skill_receipts`). Find where the worker collects them — likely a `HashSet<String>` of skill names accumulated during stream processing.

- [ ] **Step 2: Locate and reuse the receipt collector**

In `crates/bot/src/telegram/worker.rs`, find the variable that accumulates `rightx-*` skill names. If named `used_skills` or similar, capture it where the anchor is built. Replace the placeholder `used_skill_receipts: Vec::new()` from Task 6:

```rust
used_skill_receipts: used_skills.iter().cloned().collect::<Vec<_>>(),
```

If the worker does not yet maintain a `used_skills` set per turn, add one. Inside the stream loop, when handling a `tool_use` event with name starting with `mcp__right__use_skill`, extract the `name` parameter and insert into the per-turn set. Reset at turn start.

```rust
// In the per-turn scope, before the stream loop:
let mut used_skill_names_this_turn: std::collections::BTreeSet<String> = Default::default();

// Inside the loop, when processing tool_use:
if name == "mcp__right__use_skill"
    && let Some(skill_name) = parameters
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| s.starts_with("rightx-"))
{
    used_skill_names_this_turn.insert(skill_name.to_owned());
}
```

The exact field paths depend on the stream-event JSON shape; inspect `stream.rs::parse_*` helpers. The set then feeds `used_skill_receipts`.

- [ ] **Step 3: Measure wall-elapsed**

In the same per-turn scope, capture the start instant:

```rust
let turn_started_at = std::time::Instant::now();
// ... drive the CC subprocess ...
// when result event arrives:
let turn_wall_elapsed_ms = turn_started_at.elapsed().as_millis() as u64;
```

Use `turn_wall_elapsed_ms` for the anchor field and pass it to the `insert_interactive` callsite so the new column is populated.

- [ ] **Step 4: Update `usage` insert callsite to carry elapsed**

The worker reads `usage` from the `result` event. After parsing, set:

```rust
let mut usage = parsed_usage;
usage.wall_elapsed_ms = Some(turn_wall_elapsed_ms);
insert_interactive(&conn, &usage, chat_id, thread_id)?;
```

The `result` parser leaves `wall_elapsed_ms = None`; only the worker fills it for foreground turns. Cron/learning/curator paths leave `None`.

- [ ] **Step 5: Write integration test**

Append to `crates/bot/src/telegram/worker.rs::tests` (or a sibling test file if the worker has one):

```rust
#[test]
fn used_skill_receipts_filter_only_rightx_names() {
    // Pure-function test for the filter predicate, if extracted as a helper.
    // If filter is inline, this test lives where the helper lives.
    fn is_rightx_skill(name: &str) -> bool {
        name.starts_with("rightx-")
    }
    assert!(is_rightx_skill("rightx-foo"));
    assert!(!is_rightx_skill("foo"));
    assert!(!is_rightx_skill("rightx"));
}
```

Extract a `is_rightx_skill` const helper if not already present.

- [ ] **Step 6: Run tests, verify PASS**

Run: `devenv shell -- cargo test -p right-bot used_skill_receipts_filter_only_rightx_names`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/worker.rs crates/bot/src/cc/stream.rs
git commit -m "feat(bot): worker measures wall_elapsed_ms + accumulates rightx receipts per turn"
```

---

### Task 8: New `PrefilterDecision` enum + schema + parser

**Files:**
- Modify: `crates/bot/src/learning_prefilter.rs`

- [ ] **Step 1: Write failing tests**

Replace the existing decision/parser tests in `crates/bot/src/learning_prefilter.rs::tests` with:

```rust
#[test]
fn parses_skip_decision() {
    let stdout = wrap_cc_envelope(r#"{"decision":"skip","reason":"trivial echo"}"#);
    let d = parse_output(&stdout);
    assert!(matches!(d, PrefilterDecision::Skip { reason } if reason == "trivial echo"));
}

#[test]
fn parses_patch_existing_decision_with_target() {
    let stdout = wrap_cc_envelope(
        r#"{"decision":"patch_existing","target_skill":"rightx-foo","reason":"missed step"}"#,
    );
    let d = parse_output(&stdout);
    match d {
        PrefilterDecision::PatchExisting { target_skill, reason } => {
            assert_eq!(target_skill, "rightx-foo");
            assert_eq!(reason, "missed step");
        }
        _ => panic!("expected PatchExisting"),
    }
}

#[test]
fn parses_create_new_decision_with_topic_hint() {
    let stdout = wrap_cc_envelope(
        r#"{"decision":"create_new","topic_hint":"git rebase recovery","reason":"new procedure"}"#,
    );
    let d = parse_output(&stdout);
    match d {
        PrefilterDecision::CreateNew { topic_hint, reason } => {
            assert_eq!(topic_hint, "git rebase recovery");
            assert_eq!(reason, "new procedure");
        }
        _ => panic!("expected CreateNew"),
    }
}

#[test]
fn patch_without_target_returns_skip() {
    let stdout = wrap_cc_envelope(r#"{"decision":"patch_existing","reason":"vague"}"#);
    let d = parse_output(&stdout);
    assert!(matches!(d, PrefilterDecision::Skip { .. }));
}

#[test]
fn create_without_topic_hint_returns_skip() {
    let stdout = wrap_cc_envelope(r#"{"decision":"create_new","reason":"vague"}"#);
    let d = parse_output(&stdout);
    assert!(matches!(d, PrefilterDecision::Skip { .. }));
}

#[test]
fn target_skill_not_rightx_returns_skip() {
    let stdout = wrap_cc_envelope(
        r#"{"decision":"patch_existing","target_skill":"foo-bar","reason":"x"}"#,
    );
    let d = parse_output(&stdout);
    assert!(matches!(d, PrefilterDecision::Skip { .. }));
}

#[test]
fn malformed_json_returns_skip() {
    let d = parse_output("not json");
    assert!(matches!(d, PrefilterDecision::Skip { .. }));
}

/// Wrap raw JSON in the CC `--output-format json` envelope the parser
/// expects (`result` field). Implementation borrows from
/// `learning_review::unwrap_structured_output_payload`.
fn wrap_cc_envelope(inner_json: &str) -> String {
    serde_json::json!({
        "type": "result",
        "result": inner_json,
    })
    .to_string()
}
```

- [ ] **Step 2: Run tests, verify FAIL**

Run: `devenv shell -- cargo test -p right-bot -- learning_prefilter::tests`

Expected: compile errors + failures (enum variants don't exist yet).

- [ ] **Step 3: Replace the enum and schema**

In `crates/bot/src/learning_prefilter.rs`, replace the existing `PrefilterDecision`, schema constant, and parser:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrefilterDecision {
    Skip {
        reason: String,
    },
    PatchExisting {
        target_skill: String,
        reason: String,
    },
    CreateNew {
        topic_hint: String,
        reason: String,
    },
}

pub(crate) const PREFILTER_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "decision": {
      "type": "string",
      "enum": ["skip", "patch_existing", "create_new"]
    },
    "target_skill": {
      "type": "string",
      "pattern": "^rightx-[a-z0-9-]+$"
    },
    "topic_hint": {
      "type": "string",
      "maxLength": 120
    },
    "reason": {
      "type": "string",
      "maxLength": 400
    }
  },
  "required": ["decision", "reason"]
}"#;

pub(crate) fn parse_output(stdout: &str) -> PrefilterDecision {
    let inner =
        match crate::learning_review::unwrap_structured_output_payload(stdout, "prefilter") {
            Ok(v) => v,
            Err(_) => return PrefilterDecision::Skip {
                reason: "envelope parse failed".into(),
            },
        };

    let decision = inner.get("decision").and_then(|v| v.as_str());
    let reason = inner
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();

    match decision {
        Some("skip") => PrefilterDecision::Skip { reason },
        Some("patch_existing") => {
            let target = inner
                .get("target_skill")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if target.is_empty() || !target.starts_with("rightx-") {
                tracing::warn!(
                    target = %target,
                    "prefilter patch_existing missing/invalid target_skill"
                );
                return PrefilterDecision::Skip {
                    reason: "patch_existing without valid target_skill".into(),
                };
            }
            PrefilterDecision::PatchExisting {
                target_skill: target.into(),
                reason,
            }
        }
        Some("create_new") => {
            let hint = inner
                .get("topic_hint")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if hint.is_empty() {
                tracing::warn!("prefilter create_new missing topic_hint");
                return PrefilterDecision::Skip {
                    reason: "create_new without topic_hint".into(),
                };
            }
            PrefilterDecision::CreateNew {
                topic_hint: hint.into(),
                reason,
            }
        }
        other => {
            tracing::warn!(decision = ?other, "prefilter unknown decision");
            PrefilterDecision::Skip {
                reason: "unknown decision".into(),
            }
        }
    }
}
```

- [ ] **Step 4: Run tests, verify PASS**

Run: `devenv shell -- cargo test -p right-bot -- learning_prefilter::tests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/learning_prefilter.rs
git commit -m "feat(bot): prefilter returns three-mode PrefilterDecision with schema validation"
```

---

### Task 9: Skill index summary + new prefilter prompt

**Files:**
- Modify: `crates/bot/src/learning_prefilter.rs`
- Modify: `crates/bot/src/learning_review.rs` (add `summary` projection alongside existing index)

- [ ] **Step 1: Add `skill_index_summary` projection**

In `crates/bot/src/learning_review.rs`, near `collect_host_rightx_skill_index`, add:

```rust
/// One-line-per-skill projection of `collect_host_rightx_skill_index`.
/// Each line is `rightx-<name>: <one-line description>`. Description is taken
/// from the `description` field in the SKILL.md frontmatter; if missing or
/// multi-line, only the first 120 chars of the first line are kept.
pub(crate) fn render_skill_index_summary(skills: &[CollectedSkill]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for sk in skills {
        let desc_line = sk
            .description
            .as_deref()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(120)
            .collect::<String>();
        let _ = writeln!(s, "- {name}: {desc_line}", name = sk.name);
    }
    s
}
```

`CollectedSkill` already carries `name` and may carry `description`; check the existing struct. If not, add a `pub description: Option<String>` field and populate from SKILL.md frontmatter parsing.

- [ ] **Step 2: Add tests for the projection**

Append to `crates/bot/src/learning_review_tests.rs`:

```rust
#[test]
fn render_skill_index_summary_outputs_one_line_per_skill() {
    let skills = vec![
        crate::learning_review::CollectedSkill {
            name: "rightx-a".into(),
            description: Some("Does the A thing".into()),
            // ... fill required fields based on actual struct shape
        },
        crate::learning_review::CollectedSkill {
            name: "rightx-b".into(),
            description: Some("Multi\nline\ndesc".into()),
            // ... fill required fields
        },
    ];
    let s = crate::learning_review::render_skill_index_summary(&skills);
    assert!(s.contains("rightx-a: Does the A thing"));
    assert!(s.contains("rightx-b: Multi"));
    assert!(!s.contains("\nline"));
}
```

Use the actual struct shape — inspect `CollectedSkill` first.

- [ ] **Step 3: Rewrite `build_prompt` in prefilter**

In `crates/bot/src/learning_prefilter.rs`, replace `build_prompt`:

```rust
pub(crate) fn build_prompt(
    anchor: &ProbeAnchor,
    baselines: &right_agent::usage::turn_baseline::TurnBaselines,
    skill_index_summary: &str,
) -> String {
    let user: String = anchor.user_msg_text.chars().take(2000).collect();
    let assistant: String = anchor.assistant_reply_text.chars().take(4000).collect();
    let stats = render_turn_stats(anchor, baselines);
    let receipts_section = if anchor.used_skill_receipts.is_empty() {
        "USED SKILLS: none".to_owned()
    } else {
        format!(
            "USED SKILLS:\n{}",
            anchor
                .used_skill_receipts
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let framing = if anchor.used_skill_receipts.is_empty() {
        "No existing skill was used in this turn. Decide whether the turn \
exposed a reusable procedure that warrants a *new* skill, or whether it \
was trivial (Skip)."
    } else {
        "One or more existing skills were used. Decide whether any of them \
needs a *patch* (because the turn exposed a gap or correction), whether a \
*new* skill should be created for a procedure beyond the cited skills' \
scope, or whether the turn was a clean application of existing skills \
(Skip)."
    };
    format!(
        "Decide whether the just-finished turn produced material worth \
spawning the probe-writer (Sonnet) over. Reply JSON per schema.

{stats}

{receipts_section}

EXISTING SKILLS (abbreviated):
{skill_index_summary}

USER: {user}
ASSISTANT: {assistant}

{framing}

Set:
- decision=\"skip\" if the turn was trivial chat/echo or already \
well-covered by existing skills.
- decision=\"patch_existing\" with target_skill=\"rightx-<name>\" if a \
used skill needs a focused update (gap, correction, missing edge case).
- decision=\"create_new\" with topic_hint=\"<short topic>\" if the turn \
exposes a reusable procedure not covered by any existing skill.

reason is a short justification (max 400 chars)."
    )
}

fn render_turn_stats(
    anchor: &ProbeAnchor,
    b: &right_agent::usage::turn_baseline::TurnBaselines,
) -> String {
    use right_agent::usage::turn_baseline::BaselineMetric;
    let n = b.sample_size;
    let window = b.window_days;
    if matches!(b.cost_usd, BaselineMetric::Insufficient { .. }) {
        return format!(
            "TURN STATS (baseline insufficient, only n={n} prior turns):\n  \
num_turns: {turns}, cost: ${cost:.3}, elapsed: {elapsed_s}s",
            turns = anchor.num_turns,
            cost = anchor.total_cost_usd,
            elapsed_s = anchor.wall_elapsed_ms / 1000,
        );
    }
    let cost_line = match b.cost_usd {
        BaselineMetric::Available { p50, p90, p99 } => format!(
            "  cost:       ${cur:.3}   (P50=${p50:.3}, P90=${p90:.3}, P99=${p99:.3})",
            cur = anchor.total_cost_usd
        ),
        _ => format!("  cost:       ${:.3}", anchor.total_cost_usd),
    };
    let turns_line = match b.num_turns {
        BaselineMetric::Available { p50, p90, p99 } => format!(
            "  num_turns:  {cur}      (P50={p50}, P90={p90}, P99={p99})",
            cur = anchor.num_turns
        ),
        _ => format!("  num_turns:  {}", anchor.num_turns),
    };
    let elapsed_line = match b.wall_elapsed_ms {
        BaselineMetric::Available { p50, p90, p99 } => format!(
            "  elapsed:    {cur}s     (P50={p50}s, P90={p90}s, P99={p99}s)",
            cur = anchor.wall_elapsed_ms / 1000,
            p50 = p50 / 1000,
            p90 = p90 / 1000,
            p99 = p99 / 1000
        ),
        _ => format!("  elapsed:    {}s", anchor.wall_elapsed_ms / 1000),
    };
    format!(
        "TURN STATS (this turn vs agent's {window}d foreground baseline, n={n}):\n\
{turns_line}\n{cost_line}\n{elapsed_line}"
    )
}
```

- [ ] **Step 4: Tests for prompt rendering**

Add to `crates/bot/src/learning_prefilter.rs::tests`:

```rust
#[test]
fn build_prompt_includes_create_new_framing_when_receipts_empty() {
    let mut a = anchor("hello", "hi");
    a.used_skill_receipts.clear();
    let bs = baselines_insufficient(8);
    let p = build_prompt(&a, &bs, "- rightx-foo: foo desc");
    assert!(p.contains("No existing skill was used"));
    assert!(p.contains("USED SKILLS: none"));
}

#[test]
fn build_prompt_includes_patch_framing_when_receipts_present() {
    let mut a = anchor("hello", "hi");
    a.used_skill_receipts = vec!["rightx-foo".into()];
    let bs = baselines_insufficient(8);
    let p = build_prompt(&a, &bs, "- rightx-foo: foo desc");
    assert!(p.contains("One or more existing skills were used"));
    assert!(p.contains("- rightx-foo"));
}

#[test]
fn build_prompt_renders_percentiles_when_baseline_available() {
    let a = anchor("hello", "hi");
    let bs = baselines_available();
    let p = build_prompt(&a, &bs, "");
    assert!(p.contains("vs agent's"));
    assert!(p.contains("P50="));
    assert!(p.contains("P90="));
    assert!(p.contains("P99="));
}

#[test]
fn build_prompt_renders_insufficient_baseline_message() {
    let a = anchor("hello", "hi");
    let bs = baselines_insufficient(8);
    let p = build_prompt(&a, &bs, "");
    assert!(p.contains("baseline insufficient"));
    assert!(p.contains("n=8"));
}

fn baselines_insufficient(n: u32) -> right_agent::usage::turn_baseline::TurnBaselines {
    use right_agent::usage::turn_baseline::{BaselineMetric, TurnBaselines};
    TurnBaselines {
        sample_size: n,
        window_days: 14,
        cost_usd: BaselineMetric::Insufficient { sample_size: n },
        num_turns: BaselineMetric::Insufficient { sample_size: n },
        wall_elapsed_ms: BaselineMetric::Insufficient { sample_size: n },
    }
}

fn baselines_available() -> right_agent::usage::turn_baseline::TurnBaselines {
    use right_agent::usage::turn_baseline::{BaselineMetric, TurnBaselines};
    TurnBaselines {
        sample_size: 50,
        window_days: 14,
        cost_usd: BaselineMetric::Available { p50: 0.03, p90: 0.18, p99: 0.95 },
        num_turns: BaselineMetric::Available { p50: 4, p90: 12, p99: 24 },
        wall_elapsed_ms: BaselineMetric::Available {
            p50: 6_000,
            p90: 22_000,
            p99: 58_000,
        },
    }
}
```

- [ ] **Step 5: Verify**

Run: `devenv shell -- cargo test -p right-bot learning_prefilter`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/learning_prefilter.rs crates/bot/src/learning_review.rs crates/bot/src/learning_review_tests.rs
git commit -m "feat(bot): prefilter prompt renders baselines + receipts + skill index summary"
```

---

### Task 10: `prefilter::run()` wires baseline + summary

**Files:**
- Modify: `crates/bot/src/learning_prefilter.rs`

- [ ] **Step 1: Update `PrefilterContext` and `run`**

Replace the existing `run` and add fields to `PrefilterContext`:

```rust
#[derive(Debug, Clone)]
pub(crate) struct PrefilterContext {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub model: String,
    pub chat_id: i64,
    pub thread_id: i64,
    /// Window for baseline percentiles (passes through to `turn_baseline::compute`).
    pub baseline_window_days: u32,
    /// Minimum sample size for the baseline to be `Available`.
    pub baseline_min_sample: u32,
}

pub(crate) async fn run(ctx: PrefilterContext, anchor: ProbeAnchor) -> PrefilterDecision {
    use crate::cc::invocation::{ClaudeInvocation, OutputFormat, build_claude_command};

    let conn = match right_db::open_connection(&ctx.agent_db_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "prefilter open_connection failed: {e:#}");
            return PrefilterDecision::Skip {
                reason: "db open failed".into(),
            };
        }
    };
    let baselines = match right_agent::usage::turn_baseline::compute(
        &conn,
        ctx.baseline_window_days,
        ctx.baseline_min_sample,
    ) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "prefilter baseline compute failed: {e:#}");
            return PrefilterDecision::Skip {
                reason: "baseline compute failed".into(),
            };
        }
    };
    drop(conn);

    let skills = match crate::learning_review::collect_host_rightx_skill_index(&ctx.agent_dir) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "prefilter skill index failed: {e:#}");
            return PrefilterDecision::Skip {
                reason: "skill index failed".into(),
            };
        }
    };
    let summary = crate::learning_review::render_skill_index_summary(&skills);

    let prompt = build_prompt(&anchor, &baselines, &summary);
    let invocation = ClaudeInvocation {
        mcp_config_path: None,
        json_schema: Some(PREFILTER_SCHEMA_JSON.into()),
        output_format: OutputFormat::Json,
        model: Some(ctx.model.clone()),
        max_budget_usd: None,
        max_turns: Some(1),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: crate::cc::invocation::disable_all_tools_args(),
        prompt: Some(prompt),
        debug_flag: None,
    };
    let args = invocation.into_args();
    let mut cmd = build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    );
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = match tokio::time::timeout(PREFILTER_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            tracing::warn!(agent = %ctx.agent_name, "prefilter spawn failed: {e:#}");
            return PrefilterDecision::Skip {
                reason: "spawn failed".into(),
            };
        }
        Err(_) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "prefilter timed out after {}s",
                PREFILTER_TIMEOUT.as_secs()
            );
            return PrefilterDecision::Skip {
                reason: "timed out".into(),
            };
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if let Some(b) = crate::cc::stream::parse_usage_full(&stdout)
        && let Ok(conn) = right_db::open_connection(&ctx.agent_db_dir, false)
        && let Err(e) =
            right_agent::usage::insert::insert_learning_prefilter(&conn, &b, ctx.chat_id, ctx.thread_id)
    {
        tracing::warn!(agent = %ctx.agent_name, "prefilter usage insert failed: {e:#}");
    }
    parse_output(&stdout)
}
```

- [ ] **Step 2: Update worker callsite**

In `crates/bot/src/telegram/worker.rs:1828`, where `PrefilterContext` is constructed, add the new fields from `config.learning`:

```rust
let prefilter_ctx = crate::learning_prefilter::PrefilterContext {
    // ... existing fields
    baseline_window_days: config.learning.baseline_window_days,
    baseline_min_sample: config.learning.baseline_min_sample,
};
```

- [ ] **Step 3: Build**

Run: `devenv shell -- cargo check -p right-bot`

Expected: clean.

- [ ] **Step 4: Verify tests still pass**

Run: `devenv shell -- cargo test -p right-bot learning_prefilter`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/learning_prefilter.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): prefilter::run computes baselines and renders skill summary"
```

---

### Task 11: Probe-writer accepts directed hint

**Files:**
- Modify: `crates/bot/src/learning_probe_writer.rs`
- Modify: `crates/bot/src/telegram/worker.rs` (worker selects path based on decision)

- [ ] **Step 1: Add `incoming_hint` to context and prompt branches**

In `crates/bot/src/learning_probe_writer.rs`, extend the context:

```rust
#[derive(Debug, Clone)]
pub(crate) enum ProbeWriterHint {
    PatchExisting { target_skill: String, reason: String },
    CreateNew { topic_hint: String, reason: String },
}

#[derive(Debug, Clone)]
pub(crate) struct ProbeWriterContext {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub model: String,
    pub debug_flag: Arc<AtomicBool>,
    pub session_locks: SessionLocks,
    pub chat_id: i64,
    pub thread_id: i64,
    pub incoming_hint: ProbeWriterHint,
}
```

Replace `build_user_prompt`:

```rust
pub(crate) fn build_user_prompt(
    anchor: &ProbeAnchor,
    skill_index: &str,
    hint: &ProbeWriterHint,
) -> String {
    let user: String = anchor.user_msg_text.chars().take(8000).collect();
    let assistant: String = anchor.assistant_reply_text.chars().take(12000).collect();
    let hint_block = match hint {
        ProbeWriterHint::PatchExisting { target_skill, reason } => format!(
            "PREFILTER HINT: patch_existing\n\
TARGET SKILL: {target_skill}\n\
REASON: {reason}\n\n\
Verify the gap described in REASON by reading {target_skill}/SKILL.md \
and the turn transcript below. If you confirm the gap, patch the skill. \
If the hint is mistaken (skill is already correct, or the gap is \
elsewhere), exit silently or create a new skill if a different procedure \
is exposed.",
            target_skill = target_skill,
            reason = reason,
        ),
        ProbeWriterHint::CreateNew { topic_hint, reason } => format!(
            "PREFILTER HINT: create_new\n\
TOPIC HINT: {topic_hint}\n\
REASON: {reason}\n\n\
Verify that no existing skill covers TOPIC HINT by scanning the index \
below. If a close-enough skill exists, patch it instead. If nothing \
matches, create a new rightx-* skill. If the hint is wrong (the turn \
does not expose a reusable procedure), exit silently.",
            topic_hint = topic_hint,
            reason = reason,
        ),
    };
    format!(
        "{hint_block}\n\n\
EXISTING SKILLS:\n{skill_index}\n\n\
TURN:\nUSER: {user}\nASSISTANT: {assistant}\n"
    )
}
```

- [ ] **Step 2: Update `run` signature and callsite**

`run` now takes the hint:

```rust
pub(crate) async fn run(
    ctx: ProbeWriterContext,
    anchor: ProbeAnchor,
    skill_index: String,
)
```

Already takes `ctx`, which now carries `incoming_hint`. Internally, the prompt builder passes `&ctx.incoming_hint`.

In `crates/bot/src/telegram/worker.rs`, the pipeline after prefilter changes from:

```rust
match prefilter_decision {
    PrefilterDecision::Probe => spawn probe_writer,
    PrefilterDecision::Skip => no-op,
}
```

to:

```rust
let hint = match prefilter_decision {
    PrefilterDecision::Skip { reason } => {
        tracing::debug!(reason = %reason, "prefilter skipped");
        return;
    }
    PrefilterDecision::PatchExisting { target_skill, reason } => {
        ProbeWriterHint::PatchExisting { target_skill, reason }
    }
    PrefilterDecision::CreateNew { topic_hint, reason } => {
        ProbeWriterHint::CreateNew { topic_hint, reason }
    }
};

let probe_ctx = ProbeWriterContext {
    // ... existing fields
    incoming_hint: hint,
};
crate::learning_probe_writer::run(probe_ctx, anchor, skill_index).await;
```

- [ ] **Step 3: Tests for prompt branches**

Append to `crates/bot/src/learning_probe_writer.rs::tests`:

```rust
fn anchor() -> ProbeAnchor {
    ProbeAnchor {
        user_msg_text: "u".into(),
        assistant_reply_text: "a".into(),
        main_session_uuid: "uuid".into(),
        captured_at: chrono::Utc::now(),
        chat_id: 1,
        thread_id: 0,
        num_turns: 1,
        total_cost_usd: 0.01,
        wall_elapsed_ms: 1000,
        used_skill_receipts: Vec::new(),
    }
}

#[test]
fn build_user_prompt_includes_patch_block_for_patch_hint() {
    let p = build_user_prompt(
        &anchor(),
        "- rightx-foo: ...",
        &ProbeWriterHint::PatchExisting {
            target_skill: "rightx-foo".into(),
            reason: "missed step".into(),
        },
    );
    assert!(p.contains("PREFILTER HINT: patch_existing"));
    assert!(p.contains("TARGET SKILL: rightx-foo"));
    assert!(p.contains("missed step"));
}

#[test]
fn build_user_prompt_includes_create_block_for_create_hint() {
    let p = build_user_prompt(
        &anchor(),
        "- rightx-foo: ...",
        &ProbeWriterHint::CreateNew {
            topic_hint: "git rebase recovery".into(),
            reason: "new procedure".into(),
        },
    );
    assert!(p.contains("PREFILTER HINT: create_new"));
    assert!(p.contains("TOPIC HINT: git rebase recovery"));
    assert!(p.contains("new procedure"));
}
```

- [ ] **Step 4: Verify**

Run: `devenv shell -- cargo test -p right-bot learning_probe_writer`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/learning_probe_writer.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): probe-writer accepts directed PrefilterHint and branches prompt"
```

---

### Task 12: `skill_learning_finish` reports `hint_outcome`

**Files:**
- Modify: `crates/right/src/right_backend.rs`
- Modify: `crates/right/src/skill_lifecycle.rs` (if needed)

- [ ] **Step 1: Locate `skill_learning_finish` handler**

Run: `grep -n "skill_learning_finish\|fn call_skill_learning_finish\|hint_outcome" crates/right/src/right_backend.rs`

- [ ] **Step 2: Extend params struct**

Add a new optional field `hint_outcome` to the params type for `skill_learning_finish`:

```rust
#[derive(Debug, Deserialize)]
struct SkillLearningFinishParams {
    // ... existing fields (status, skill_name, etc.)
    /// Optional. Probe-writer reports back whether the prefilter hint matched.
    /// One of: "applied_as_hinted", "applied_differently", "refused".
    #[serde(default)]
    hint_outcome: Option<String>,
}
```

In the handler body, log the value:

```rust
if let Some(ho) = params.hint_outcome.as_deref() {
    tracing::info!(
        agent = %ctx.agent_name,
        skill = %params.skill_name,
        hint_outcome = %ho,
        "probe-writer hint outcome"
    );
}
```

For now, only log. A future Phase-2 spec writes these to an outcome table.

- [ ] **Step 3: Update MCP tool description**

Update the tool's input schema/instructions in `with_instructions()` (`crates/right-mcp/src/memory_server.rs` and `aggregator.rs`) to mention `hint_outcome` as optional with three values. Conventions require both files stay in sync per `AGENTS.md`.

- [ ] **Step 4: Test**

Append to `crates/right/src/right_backend.rs::tests` (or sibling test file):

```rust
#[test]
fn skill_learning_finish_accepts_hint_outcome_field() {
    let json = r#"{"status":"created","skill_name":"rightx-foo","hint_outcome":"applied_as_hinted"}"#;
    let params: SkillLearningFinishParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.hint_outcome.as_deref(), Some("applied_as_hinted"));
}

#[test]
fn skill_learning_finish_accepts_missing_hint_outcome() {
    let json = r#"{"status":"created","skill_name":"rightx-foo"}"#;
    let params: SkillLearningFinishParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.hint_outcome, None);
}
```

- [ ] **Step 5: Probe-writer emits the field**

In `crates/bot/src/learning_probe_writer.rs`, the prompt instructs the writer to call `mcp__right__skill_learning_finish` with `hint_outcome` set. Add a note to the prompt:

```text
When you call mcp__right__skill_learning_finish, ALWAYS include the field
"hint_outcome" with one of:
  - "applied_as_hinted" — you patched/created exactly as the hint suggested.
  - "applied_differently" — you took action but not as hinted (e.g. patched a
    different skill, created instead of patched).
  - "refused" — you exited without writing because the hint was unjustified.
```

Inject this into the prompt body (in `build_user_prompt` after the hint block).

- [ ] **Step 6: Verify**

Run: `devenv shell -- cargo test -p right -p right-bot skill_learning_finish`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/right/src/right_backend.rs crates/right-mcp/src/memory_server.rs crates/right-mcp/src/aggregator.rs crates/bot/src/learning_probe_writer.rs
git commit -m "feat(mcp): skill_learning_finish accepts hint_outcome; probe-writer instructs"
```

---

### Task 13: Extend `CuratorState` struct + DB-backed load/save

**Files:**
- Modify: `crates/bot/src/learning_curator.rs`

- [ ] **Step 1: Write failing tests**

Append to `crates/bot/src/learning_curator.rs::tests`:

```rust
use tempfile::tempdir;

fn open_test_conn() -> rusqlite::Connection {
    let dir = tempdir().unwrap();
    right_db::open_connection(dir.path(), true).unwrap()
}

#[test]
fn db_load_state_returns_default_when_empty() {
    let conn = open_test_conn();
    let s = load_state_db(&conn).unwrap();
    assert!(s.last_run_at.is_none());
    assert_eq!(s.consecutive_failures, 0);
}

#[test]
fn db_save_then_load_round_trip() {
    let conn = open_test_conn();
    let s = CuratorState {
        last_run_at: Some("2026-05-22T00:00:00Z".to_owned()),
        last_run_status: Some("success".to_owned()),
        consecutive_failures: 2,
        circuit_open_until: None,
        last_spike_evidence_json: Some(r#"{"trigger":"cost_spike"}"#.to_owned()),
    };
    save_state_db(&conn, &s).unwrap();
    let loaded = load_state_db(&conn).unwrap();
    assert_eq!(loaded, s);
}

#[test]
fn db_save_replaces_existing_row() {
    let conn = open_test_conn();
    save_state_db(
        &conn,
        &CuratorState {
            last_run_at: Some("a".into()),
            ..Default::default()
        },
    )
    .unwrap();
    save_state_db(
        &conn,
        &CuratorState {
            last_run_at: Some("b".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM curator_state", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let loaded = load_state_db(&conn).unwrap();
    assert_eq!(loaded.last_run_at.as_deref(), Some("b"));
}
```

- [ ] **Step 2: Run, verify FAIL**

Run: `devenv shell -- cargo test -p right-bot db_load_state_returns_default_when_empty`

Expected: FAIL (helpers don't exist).

- [ ] **Step 3: Extend struct + add helpers**

Replace the existing `CuratorState`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CuratorState {
    pub last_run_at: Option<String>,
    pub last_run_status: Option<String>,
    pub consecutive_failures: u32,
    pub circuit_open_until: Option<String>,
    pub last_spike_evidence_json: Option<String>,
}

pub(crate) fn load_state_db(conn: &rusqlite::Connection) -> Result<CuratorState, rusqlite::Error> {
    let row = conn.query_row(
        "SELECT last_run_at, last_run_status, consecutive_failures, \
                circuit_open_until, last_spike_evidence_json \
         FROM curator_state WHERE agent_singleton_id = 1",
        [],
        |r| {
            Ok(CuratorState {
                last_run_at: r.get(0)?,
                last_run_status: r.get(1)?,
                consecutive_failures: r.get::<_, i64>(2)? as u32,
                circuit_open_until: r.get(3)?,
                last_spike_evidence_json: r.get(4)?,
            })
        },
    );
    match row {
        Ok(s) => Ok(s),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(CuratorState::default()),
        Err(e) => Err(e),
    }
}

pub(crate) fn save_state_db(
    conn: &rusqlite::Connection,
    state: &CuratorState,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO curator_state \
            (agent_singleton_id, last_run_at, last_run_status, \
             consecutive_failures, circuit_open_until, last_spike_evidence_json) \
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            state.last_run_at,
            state.last_run_status,
            state.consecutive_failures as i64,
            state.circuit_open_until,
            state.last_spike_evidence_json,
        ],
    )?;
    Ok(())
}
```

Remove the old file-based `load_state`, `save_state`, and `state_path` helpers — they're dead code after Task 14 swaps the caller. Suppress unused warnings if needed during the transition.

- [ ] **Step 4: Run tests, verify PASS**

Run: `devenv shell -- cargo test -p right-bot db_load_state_returns_default`

Expected: PASS (3 new tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/learning_curator.rs
git commit -m "feat(curator): CuratorState gains DB-backed load/save helpers"
```

---

### Task 14: Curator helpers — cost-spike + skill-change-count

**Files:**
- Modify: `crates/right-agent/src/usage/turn_baseline.rs` (or new sibling)
- Modify: `crates/bot/src/lifecycle/usage.rs` (count helper)

- [ ] **Step 1: Add cost-spike helper**

Append to `crates/right-agent/src/usage/turn_baseline.rs` (or factor into a new sibling `curator_signals.rs` if you prefer separation; keeping in `turn_baseline.rs` is fine since it shares the percentile machinery):

```rust
use chrono::{DateTime, Utc};

/// Trigger evidence for `cost_spike` — populated when the gate fires due to
/// today's probe-writer cost exceeding the 14d P50 multiplier.
#[derive(Debug, Clone, PartialEq)]
pub struct CostSpikeEvidence {
    pub today_cost_usd: f64,
    pub baseline_p50_usd: f64,
    pub k: f64,
    pub min_floor_usd: f64,
}

/// Return `Some(evidence)` iff today's probe-writer spend exceeds both `k *
/// baseline_p50` and `min_floor_usd`. Both conditions must hold.
pub fn check_probe_writer_cost_spike(
    conn: &Connection,
    now: DateTime<Utc>,
    baseline_days: u32,
    k: f64,
    min_floor_usd: f64,
) -> Result<Option<CostSpikeEvidence>, UsageError> {
    let today_start = now.format("%Y-%m-%dT00:00:00Z").to_string();
    let today_cost: f64 = conn.query_row(
        "SELECT COALESCE(SUM(total_cost_usd), 0.0) FROM usage_events \
         WHERE source = 'learning_probe_writer' AND ts >= ?1",
        [&today_start],
        |r| r.get(0),
    )?;
    if today_cost < min_floor_usd {
        return Ok(None);
    }
    // Daily sums over the baseline window — group by date.
    let window_start = (now - chrono::Duration::days(baseline_days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();
    let mut stmt = conn.prepare(
        "SELECT SUM(total_cost_usd) FROM usage_events \
         WHERE source = 'learning_probe_writer' AND ts >= ?1 \
         GROUP BY substr(ts, 1, 10)",
    )?;
    let rows = stmt.query_map([&window_start], |r| r.get::<_, f64>(0))?;
    let mut daily: Vec<f64> = rows.collect::<Result<_, _>>()?;
    if daily.is_empty() {
        // No probe_writer history — fall back to floor-only check.
        return Ok(Some(CostSpikeEvidence {
            today_cost_usd: today_cost,
            baseline_p50_usd: 0.0,
            k,
            min_floor_usd,
        }));
    }
    daily.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = daily[daily.len() / 2];
    if today_cost >= k * p50.max(min_floor_usd) {
        Ok(Some(CostSpikeEvidence {
            today_cost_usd: today_cost,
            baseline_p50_usd: p50,
            k,
            min_floor_usd,
        }))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod cost_spike_tests {
    use super::*;
    use crate::usage::UsageBreakdown;
    use crate::usage::insert::insert_learning_probe_writer;
    use right_db::open_connection;
    use tempfile::tempdir;

    fn b(cost: f64) -> UsageBreakdown {
        UsageBreakdown {
            session_uuid: "s".into(),
            total_cost_usd: cost,
            num_turns: 1,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests: 0,
            web_fetch_requests: 0,
            model_usage_json: "{}".into(),
            api_key_source: "none".into(),
            wall_elapsed_ms: None,
        }
    }

    #[test]
    fn returns_none_when_today_below_floor() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_learning_probe_writer(&conn, &b(0.01), 1, 0).unwrap();
        let now = Utc::now();
        let r = check_probe_writer_cost_spike(&conn, now, 14, 3.0, 0.05).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn fires_when_today_above_floor_and_no_baseline() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_learning_probe_writer(&conn, &b(0.20), 1, 0).unwrap();
        let now = Utc::now();
        let r = check_probe_writer_cost_spike(&conn, now, 14, 3.0, 0.05).unwrap();
        // baseline empty + today > floor → fires
        assert!(r.is_some());
    }
}
```

- [ ] **Step 2: Add skill-change-count helper**

In `crates/bot/src/lifecycle/usage.rs`, add:

```rust
/// Count skills whose `created_at` OR `last_patched_at` is strictly after
/// `since`. Used by the curator's skill-change-count trigger.
pub(crate) fn count_changes_since(index: &Index, since: &str) -> u32 {
    let mut n = 0u32;
    for r in index.skills.values() {
        let created = r.created_at.as_deref().unwrap_or("");
        let patched = r.last_patched_at.as_deref().unwrap_or("");
        if created.as_bytes() > since.as_bytes() || patched.as_bytes() > since.as_bytes() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod count_tests {
    use super::*;

    #[test]
    fn counts_skills_created_after_since() {
        let mut idx = Index::default();
        let mut r = UsageRecord::default();
        r.created_at = Some("2026-05-22T12:00:00Z".into());
        idx.skills.insert("rightx-new".into(), r);
        let mut older = UsageRecord::default();
        older.created_at = Some("2026-05-20T12:00:00Z".into());
        idx.skills.insert("rightx-old".into(), older);
        assert_eq!(count_changes_since(&idx, "2026-05-21T00:00:00Z"), 1);
    }

    #[test]
    fn counts_skills_patched_after_since() {
        let mut idx = Index::default();
        let mut r = UsageRecord::default();
        r.created_at = Some("2026-04-01T00:00:00Z".into());
        r.last_patched_at = Some("2026-05-22T12:00:00Z".into());
        idx.skills.insert("rightx-patched".into(), r);
        assert_eq!(count_changes_since(&idx, "2026-05-21T00:00:00Z"), 1);
    }
}
```

The `as_bytes()` lexicographic comparison works because all timestamps are RFC3339 UTC.

- [ ] **Step 3: Verify**

Run: `devenv shell -- cargo test -p right-agent cost_spike_tests && devenv shell -- cargo test -p right-bot count_tests`

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/src/usage/turn_baseline.rs crates/bot/src/lifecycle/usage.rs
git commit -m "feat(curator): cost-spike + skill-change-count helpers for multi-signal gate"
```

---

### Task 15: Curator multi-signal gate — `should_run_now` rewrite + DB-backed `run_if_due`

**Files:**
- Modify: `crates/bot/src/learning_curator.rs`

- [ ] **Step 1: Extend `CuratorConfig`**

Replace `CuratorConfig`:

```rust
#[derive(Debug, Clone, Copy)]
pub(crate) struct CuratorConfig {
    pub enabled: bool,
    pub paused: bool,
    pub interval_hours: u32,
    pub min_idle_hours: u32,
    pub min_cooldown_hours: u32,
    pub stale_after_days: u32,
    pub archive_after_days: u32,
    pub cost_spike_k: f64,
    pub cost_spike_baseline_days: u32,
    pub cost_spike_min_floor_usd: f64,
    pub skill_change_threshold: u32,
}
```

- [ ] **Step 2: Extend `CuratorGateDecision`**

```rust
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CuratorGateDecision {
    Run { trigger: CuratorTrigger },
    SkipDisabled,
    SkipPaused,
    SkipCircuitOpen,
    SkipChatNotIdle,
    SkipCooldown,
    SkipNoTrigger,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CuratorTrigger {
    CostSpike(right_agent::usage::turn_baseline::CostSpikeEvidence),
    SkillChangeCount {
        count: u32,
        threshold: u32,
    },
    TimeFallback {
        interval_hours: u32,
    },
}
```

- [ ] **Step 3: Rewrite `should_run_now`**

Replace the pure gate function:

```rust
pub(crate) fn should_run_now(
    config: CuratorConfig,
    state: &CuratorState,
    now: DateTime<Utc>,
    latest_user_activity_at: Option<DateTime<Utc>>,
    cost_spike_evidence: Option<right_agent::usage::turn_baseline::CostSpikeEvidence>,
    skill_change_count: u32,
) -> CuratorGateDecision {
    if !config.enabled {
        return CuratorGateDecision::SkipDisabled;
    }
    if config.paused {
        return CuratorGateDecision::SkipPaused;
    }
    if let Some(open_until) = state.circuit_open_until.as_deref()
        && let Ok(dt) = DateTime::parse_from_rfc3339(open_until)
        && dt.with_timezone(&Utc) > now
    {
        return CuratorGateDecision::SkipCircuitOpen;
    }
    if let Some(latest) = latest_user_activity_at
        && now - latest < Duration::hours(config.min_idle_hours as i64)
    {
        return CuratorGateDecision::SkipChatNotIdle;
    }

    let last = state.last_run_at.as_deref().and_then(|s| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    });

    // Cooldown gate — applies to ALL triggers including time fallback.
    if let Some(last_dt) = last
        && now - last_dt < Duration::hours(config.min_cooldown_hours as i64)
    {
        return CuratorGateDecision::SkipCooldown;
    }

    // Trigger priority: cost spike > skill change count > time fallback.
    if let Some(ev) = cost_spike_evidence {
        return CuratorGateDecision::Run {
            trigger: CuratorTrigger::CostSpike(ev),
        };
    }
    if skill_change_count >= config.skill_change_threshold {
        return CuratorGateDecision::Run {
            trigger: CuratorTrigger::SkillChangeCount {
                count: skill_change_count,
                threshold: config.skill_change_threshold,
            },
        };
    }
    if let Some(last_dt) = last
        && now - last_dt >= Duration::hours(config.interval_hours as i64)
    {
        return CuratorGateDecision::Run {
            trigger: CuratorTrigger::TimeFallback {
                interval_hours: config.interval_hours,
            },
        };
    }
    if last.is_none() {
        // First-ever run with no triggers — keep Hermes defer pattern.
        return CuratorGateDecision::SkipNoTrigger;
    }
    CuratorGateDecision::SkipNoTrigger
}
```

- [ ] **Step 4: Write tests covering each trigger**

Replace the existing curator-gate tests with:

```rust
fn cfg() -> CuratorConfig {
    CuratorConfig {
        enabled: true,
        paused: false,
        interval_hours: 168,
        min_idle_hours: 2,
        min_cooldown_hours: 12,
        stale_after_days: 30,
        archive_after_days: 90,
        cost_spike_k: 3.0,
        cost_spike_baseline_days: 14,
        cost_spike_min_floor_usd: 0.05,
        skill_change_threshold: 3,
    }
}

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

#[test]
fn disabled_skips() {
    let mut c = cfg();
    c.enabled = false;
    assert_eq!(
        should_run_now(c, &CuratorState::default(), dt("2026-05-22T00:00:00Z"), None, None, 0),
        CuratorGateDecision::SkipDisabled
    );
}

#[test]
fn paused_skips() {
    let mut c = cfg();
    c.paused = true;
    assert_eq!(
        should_run_now(c, &CuratorState::default(), dt("2026-05-22T00:00:00Z"), None, None, 0),
        CuratorGateDecision::SkipPaused
    );
}

#[test]
fn circuit_open_in_future_skips() {
    let s = CuratorState {
        circuit_open_until: Some("2027-01-01T00:00:00Z".into()),
        ..Default::default()
    };
    assert_eq!(
        should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0),
        CuratorGateDecision::SkipCircuitOpen
    );
}

#[test]
fn cooldown_blocks_all_triggers() {
    let s = CuratorState {
        last_run_at: Some("2026-05-21T18:00:00Z".into()),
        ..Default::default()
    };
    let ev = right_agent::usage::turn_baseline::CostSpikeEvidence {
        today_cost_usd: 1.0,
        baseline_p50_usd: 0.1,
        k: 3.0,
        min_floor_usd: 0.05,
    };
    assert_eq!(
        should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, Some(ev), 5),
        CuratorGateDecision::SkipCooldown
    );
}

#[test]
fn cost_spike_fires_after_cooldown() {
    let s = CuratorState {
        last_run_at: Some("2026-05-21T00:00:00Z".into()),
        ..Default::default()
    };
    let ev = right_agent::usage::turn_baseline::CostSpikeEvidence {
        today_cost_usd: 1.0,
        baseline_p50_usd: 0.1,
        k: 3.0,
        min_floor_usd: 0.05,
    };
    let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, Some(ev.clone()), 0);
    assert!(matches!(
        d,
        CuratorGateDecision::Run {
            trigger: CuratorTrigger::CostSpike(_)
        }
    ));
}

#[test]
fn skill_change_count_fires_when_no_cost_spike() {
    let s = CuratorState {
        last_run_at: Some("2026-05-21T00:00:00Z".into()),
        ..Default::default()
    };
    let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 4);
    assert_eq!(
        d,
        CuratorGateDecision::Run {
            trigger: CuratorTrigger::SkillChangeCount {
                count: 4,
                threshold: 3
            }
        }
    );
}

#[test]
fn time_fallback_fires_when_no_other_trigger() {
    let s = CuratorState {
        last_run_at: Some("2026-05-01T00:00:00Z".into()),
        ..Default::default()
    };
    let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0);
    assert_eq!(
        d,
        CuratorGateDecision::Run {
            trigger: CuratorTrigger::TimeFallback { interval_hours: 168 }
        }
    );
}

#[test]
fn no_trigger_no_run() {
    let s = CuratorState {
        last_run_at: Some("2026-05-21T00:00:00Z".into()),
        ..Default::default()
    };
    let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0);
    // last_run_at 24h ago; cooldown 12h passed; no spike; no change-count; not 168h yet
    assert_eq!(d, CuratorGateDecision::SkipNoTrigger);
}

#[test]
fn first_ever_run_defers() {
    let s = CuratorState {
        last_run_at: None,
        ..Default::default()
    };
    let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0);
    assert_eq!(d, CuratorGateDecision::SkipNoTrigger);
}
```

- [ ] **Step 5: Rewrite `run_if_due` to use DB + new gate**

Replace the body:

```rust
pub(crate) async fn run_if_due(
    ctx: CuratorContext,
    latest_user_activity_at: Option<DateTime<Utc>>,
) {
    let conn = match right_db::open_connection(&ctx.agent_db_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator open_connection failed: {e:#}");
            return;
        }
    };
    let mut state = match load_state_db(&conn) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator load_state_db failed: {e:#}");
            return;
        }
    };

    // Seed first-run timestamp (Hermes defer).
    if state.last_run_at.is_none() {
        state.last_run_at = Some(Utc::now().to_rfc3339());
        if let Err(e) = save_state_db(&conn, &state) {
            tracing::warn!(agent = %ctx.agent_name, "curator seed state failed: {e:#}");
        }
        return;
    }

    let now = Utc::now();

    // Compute trigger signals.
    let cost_spike_evidence = match right_agent::usage::turn_baseline::check_probe_writer_cost_spike(
        &conn,
        now,
        ctx.config.cost_spike_baseline_days,
        ctx.config.cost_spike_k,
        ctx.config.cost_spike_min_floor_usd,
    ) {
        Ok(ev) => ev,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator cost spike check failed: {e:#}");
            None
        }
    };

    let skills_dir = ctx.agent_dir.join(".claude/skills");
    let usage_path = skills_dir.join(".usage.json");
    let index = match crate::lifecycle::usage::read_index(&usage_path) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator usage read failed: {e:#}");
            return;
        }
    };
    let since = state.last_run_at.as_deref().unwrap_or("");
    let change_count = crate::lifecycle::usage::count_changes_since(&index, since);

    let decision = should_run_now(
        ctx.config,
        &state,
        now,
        latest_user_activity_at,
        cost_spike_evidence.clone(),
        change_count,
    );

    let trigger = match decision {
        CuratorGateDecision::Run { trigger } => trigger,
        other => {
            tracing::debug!(agent = %ctx.agent_name, "curator gate: {:?}", other);
            return;
        }
    };

    // Capture evidence.
    state.last_spike_evidence_json =
        Some(serialize_evidence(&trigger, now));

    // ... existing backup / transitions / LLM fork unchanged ...
    // (Reuse the existing snapshot + transitions + invocation logic.)

    let backups_dir = ctx.agent_dir.join("curator_backups");
    let now_str = now.format("%Y%m%dT%H%M%SZ").to_string();
    if let Err(e) =
        crate::lifecycle::snapshot::snapshot_skills(&skills_dir, &backups_dir, &now_str)
    {
        tracing::warn!(agent = %ctx.agent_name, "curator snapshot failed: {e:#}");
    }

    let mut index_mut = index;
    let transition_changes = crate::lifecycle::transitions::apply_automatic_transitions(
        &mut index_mut,
        now,
        crate::lifecycle::transitions::TransitionConfig {
            stale_after_days: ctx.config.stale_after_days as i64,
            archive_after_days: ctx.config.archive_after_days as i64,
        },
    );
    if let Err(e) = crate::lifecycle::usage::write_index(&usage_path, &index_mut) {
        tracing::warn!(agent = %ctx.agent_name, "curator usage write failed: {e:#}");
    }
    tracing::info!(
        agent = %ctx.agent_name,
        transitions = transition_changes,
        trigger = ?trigger,
        "curator auto-transitions applied"
    );

    // LLM consolidation fork — unchanged from previous body.
    let invocation = build_curator_invocation(&ctx, &index_mut);
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

    let run_status = match tokio::time::timeout(CURATOR_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            if let Some(b) = crate::cc::stream::parse_usage_full(&stdout)
                && let Err(e) = right_agent::usage::insert::insert_learning_curator(&conn, &b)
            {
                tracing::warn!(agent = %ctx.agent_name, "curator usage insert failed: {e:#}");
            }
            if output.status.success() {
                "success".to_owned()
            } else {
                tracing::warn!(
                    agent = %ctx.agent_name,
                    status = ?output.status,
                    "curator exited non-zero"
                );
                "failed".to_owned()
            }
        }
        Ok(Err(e)) => {
            tracing::warn!(agent = %ctx.agent_name, "curator spawn failed: {e:#}");
            "failed".to_owned()
        }
        Err(_) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "curator timed out after {}s",
                CURATOR_TIMEOUT.as_secs()
            );
            "failed".to_owned()
        }
    };

    state.last_run_at = Some(now.to_rfc3339());
    state.last_run_status = Some(run_status.clone());
    if run_status == "success" {
        state.consecutive_failures = 0;
        state.circuit_open_until = None;
    } else {
        state.consecutive_failures += 1;
    }
    if let Err(e) = save_state_db(&conn, &state) {
        tracing::warn!(agent = %ctx.agent_name, "curator save state failed: {e:#}");
    }
}

fn serialize_evidence(trigger: &CuratorTrigger, now: DateTime<Utc>) -> String {
    let computed_at = now.to_rfc3339();
    match trigger {
        CuratorTrigger::CostSpike(ev) => serde_json::json!({
            "trigger": "cost_spike",
            "computed_at": computed_at,
            "details": {
                "today_cost_usd": ev.today_cost_usd,
                "baseline_p50_usd": ev.baseline_p50_usd,
                "k": ev.k,
                "min_floor_usd": ev.min_floor_usd
            }
        })
        .to_string(),
        CuratorTrigger::SkillChangeCount { count, threshold } => serde_json::json!({
            "trigger": "skill_change_count",
            "computed_at": computed_at,
            "details": { "count": count, "threshold": threshold }
        })
        .to_string(),
        CuratorTrigger::TimeFallback { interval_hours } => serde_json::json!({
            "trigger": "time_fallback",
            "computed_at": computed_at,
            "details": { "interval_hours": interval_hours }
        })
        .to_string(),
    }
}
```

- [ ] **Step 6: Update lib.rs ticker to pass new config fields**

In `crates/bot/src/lib.rs:1192` where `CuratorConfig` is built, add:

```rust
config: crate::learning_curator::CuratorConfig {
    enabled: curator_learning.curator_enabled,
    paused: curator_learning.curator_paused,
    interval_hours: curator_learning.curator_interval_hours,
    min_idle_hours: curator_learning.curator_min_idle_hours,
    min_cooldown_hours: curator_learning.curator_min_cooldown_hours,
    stale_after_days: curator_learning.curator_stale_after_days,
    archive_after_days: curator_learning.curator_archive_after_days,
    cost_spike_k: curator_learning.curator_cost_spike_k,
    cost_spike_baseline_days: curator_learning.curator_cost_spike_baseline_days,
    cost_spike_min_floor_usd: curator_learning.curator_cost_spike_min_floor_usd,
    skill_change_threshold: curator_learning.curator_skill_change_threshold,
},
```

- [ ] **Step 7: Run tests**

Run: `devenv shell -- cargo test -p right-bot learning_curator`

Expected: all curator-gate tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/learning_curator.rs crates/bot/src/lib.rs
git commit -m "feat(curator): multi-signal gate with cost spike + skill change + time fallback"
```

---

### Task 16: Wizard prompts for new `LearningConfig` fields

**Files:**
- Modify: `crates/right/src/wizard.rs`

- [ ] **Step 1: Locate the learning section of the wizard**

Run: `grep -n "fn cmd_agent_config\|curator_interval\|prefilter_enabled" crates/right/src/wizard.rs | head -10`

- [ ] **Step 2: Add prompts**

In the curator-related section of `cmd_agent_config`, add 7 new prompts (parse helpers reuse the existing `parse_u32_positive`, `parse_bool_yes_no`, and a new `parse_positive_finite_f64`):

```rust
// Cost-spike trigger
let curator_cost_spike_k = prompt_positive_finite_f64(
    "Curator cost-spike multiplier k (today's probe-writer cost ≥ k * 14d P50 → trigger)",
    cfg.learning.curator_cost_spike_k,
)?;
let curator_cost_spike_baseline_days = prompt_u32_positive(
    "Curator cost-spike baseline window (days)",
    cfg.learning.curator_cost_spike_baseline_days,
)?;
let curator_cost_spike_min_floor_usd = prompt_positive_finite_f64(
    "Curator cost-spike absolute floor (USD; below this, spike never fires)",
    cfg.learning.curator_cost_spike_min_floor_usd,
)?;

// Skill-change count
let curator_skill_change_threshold = prompt_u32_positive(
    "Curator skill-change count threshold (new/patched skills since last run → trigger)",
    cfg.learning.curator_skill_change_threshold,
)?;

// Cooldown
let curator_min_cooldown_hours = prompt_u32_positive(
    "Curator minimum cooldown (hours; blocks ALL triggers)",
    cfg.learning.curator_min_cooldown_hours,
)?;

// Prefilter baselines
let baseline_window_days = prompt_u32_positive(
    "Prefilter baseline window (days)",
    cfg.learning.baseline_window_days,
)?;
let baseline_min_sample = prompt_u32_positive(
    "Prefilter baseline minimum sample size",
    cfg.learning.baseline_min_sample,
)?;
```

Then assign these into the updated `cfg.learning` before write-back.

- [ ] **Step 3: Add `prompt_positive_finite_f64` helper if absent**

```rust
fn prompt_positive_finite_f64(label: &str, default_val: f64) -> Result<f64> {
    let s = right_ui::prompt::ask_default(label, &format!("{:.3}", default_val))?;
    let v: f64 = s.parse().map_err(|_| anyhow!("not a number: {s}"))?;
    if !v.is_finite() || v <= 0.0 {
        return Err(anyhow!("value must be finite and > 0: got {v}"));
    }
    Ok(v)
}
```

- [ ] **Step 4: Build**

Run: `devenv shell -- cargo check -p right`

Expected: clean.

- [ ] **Step 5: Test wizard parser helpers**

If `prompt_positive_finite_f64` is new, add a unit test:

```rust
#[test]
fn prompt_positive_finite_f64_rejects_zero_and_negative_and_nan() {
    // Test the parsing predicate directly if exposed; otherwise factor into
    // a parse-only helper for testability.
}
```

If extracting the predicate into a separate `fn parse_positive_finite_f64(s: &str)` is too invasive for this task, mark the test as a follow-up.

- [ ] **Step 6: Commit**

```bash
git add crates/right/src/wizard.rs
git commit -m "feat(wizard): prompts for curator trigger + prefilter baseline knobs"
```

---

### Task 17: Update docs — ARCHITECTURE + PROMPT_SYSTEM + successor note

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `PROMPT_SYSTEM.md`
- Modify: `docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md` (add successor note)

- [ ] **Step 1: Update `ARCHITECTURE.md` "Skill learning loop" subsection**

Find the "Skill learning loop" section. Replace the per-turn pipeline summary with:

```markdown
1. **Anchor capture** (`bot::telegram::worker`): after the foreground assistant
   reply is sent, the worker captures a `ProbeAnchor` (user text, assistant
   text, main session UUID, captured_at, chat/thread, **num_turns,
   total_cost_usd, wall_elapsed_ms, used_skill_receipts**) for downstream
   consumption.

2. **Prefilter** (`bot::learning_prefilter`): a Haiku classifier returns a
   structured three-way decision —
   `Skip{reason}` / `PatchExisting{target_skill, reason}` /
   `CreateNew{topic_hint, reason}`. The prompt embeds per-agent baselines
   (P50/P90/P99 over 14d foreground turns) for `num_turns`, `total_cost_usd`,
   and `wall_elapsed_ms`, plus a one-line-per-skill index summary. Baselines
   are computed on demand by `right_agent::usage::turn_baseline::compute`.

3. **Probe-writer** (`bot::learning_probe_writer`): when the prefilter
   returns non-Skip, the worker forks the main CC session with the decision
   as a directed hint. The writer verifies and may patch, create, or refuse.
   It reports `hint_outcome` (`applied_as_hinted` / `applied_differently` /
   `refused`) back via `mcp__right__skill_learning_finish`.

4. **Curator** (`bot::learning_curator`): per-agent 60s ticker reads state
   from the `curator_state` singleton row in `data.db`. The gate is
   multi-signal: cost spike (today's `learning_probe_writer` cost vs
   `k * 14d P50` with a floor), skill-change count (≥ N skills
   created/patched since last run), or the 168h time fallback. A
   `min_cooldown_hours` floor blocks all triggers including the time
   fallback. Trigger evidence is captured in `last_spike_evidence_json`.
```

Add this to the "Memory Schema (SQLite)" table list:

> `curator_state` (singleton; `agent_singleton_id` PRIMARY KEY CHECK = 1).

- [ ] **Step 2: Update `PROMPT_SYSTEM.md`**

Replace the `PREFILTER_SCHEMA_JSON` description with the new 3-mode schema. Add a section "Probe-writer hint propagation" explaining the prompt branches based on `PrefilterDecision`. Add a section "TURN STATS in prefilter prompt" with an example block (both Available and Insufficient cases). Note the `hint_outcome` field on `skill_learning_finish`.

- [ ] **Step 3: Successor note in prior spec**

Add to the top of `docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md`:

```markdown
> **Successor:** `docs/superpowers/specs/2026-05-22-prefilter-classifier-and-curator-state-design.md` refines the prefilter into a 3-mode classifier, migrates curator state to `data.db`, and adds a multi-signal curator trigger.
```

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md PROMPT_SYSTEM.md docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md
git commit -m "docs: ARCHITECTURE + PROMPT_SYSTEM updated for 3-mode prefilter + DB-backed curator state"
```

---

### Task 18: Final workspace verification

**Files:** none

- [ ] **Step 1: Full workspace build**

Run: `devenv shell -- cargo build --workspace 2>&1 | tail -20`

Expected: clean build, no warnings beyond pre-existing.

- [ ] **Step 2: Full workspace test**

Run: `devenv shell -- cargo test --workspace --no-fail-fast 2>&1 | tail -40`

Expected: all pass except any pre-existing failures recorded in Task 0.

- [ ] **Step 3: Clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets 2>&1 | tail -30`

Expected: no new warnings.

- [ ] **Step 4: Commit any rustfmt/clippy autofixes**

If pre-commit applied fixes, commit them:

```bash
git status --short
# if anything is modified:
git add -u
git commit -m "chore: rustfmt/clippy autofixes"
```

- [ ] **Step 5: Confirm spec is referenced in latest commits**

Run: `git log --oneline -20`

Expected: ~18 commits since baseline `da2488d3` (or equivalent) on this branch.

---

## Self-review checklist (run before declaring complete)

1. ✅ Spec §4.1 (turn baselines) → Task 4.
2. ✅ Spec §4.2 (ProbeAnchor extension) → Task 6 + Task 7.
3. ✅ Spec §4.3 (3-mode prefilter) → Tasks 8, 9, 10.
4. ✅ Spec §4.4 (probe-writer hints + hint_outcome) → Tasks 11, 12.
5. ✅ Spec §4.5 (`wall_elapsed_ms`) → Tasks 1, 3, 7.
6. ✅ Spec §4.6 (curator_state in DB) → Tasks 2, 13.
7. ✅ Spec §4.7 (multi-signal curator trigger) → Tasks 14, 15.
8. ✅ Spec §4.8 (LearningConfig additions) → Task 5.
9. ✅ Spec §5 (ASCII diagram) — purely documentary, no task needed; doc update lands in Task 17.
10. ✅ Spec §11 Phase-2 items — explicitly out of scope; not in plan.
