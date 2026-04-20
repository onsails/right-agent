# /usage Command Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rework `/usage` to show human-readable output — prompt-cache effectiveness, subscription-vs-API billing split, optional raw-tokens detail mode — plus fix the slash-menu autocomplete regression.

**Architecture:** Additive schema change (new `api_key_source` column, v16). New `pricing` module provides per-model rates for cache-savings math. Capture `apiKeySource` from the CC `system/init` stream event and thread it through to the insert. Aggregate grows two sum columns for the billing split. `format.rs` is rewritten. Handler accepts optional `detail` argument. Bot startup pre-deletes language-scoped Telegram command lists to prevent stale-override shadowing.

**Tech Stack:** Rust 2024, rusqlite + rusqlite_migration, teloxide, serde_json, chrono.

**Spec:** [docs/superpowers/specs/2026-04-20-usage-command-redesign-design.md](../specs/2026-04-20-usage-command-redesign-design.md)

---

## Task 1: v16 migration — add `api_key_source` column

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v16_usage_api_key_source.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`
- Modify: `crates/rightclaw/src/memory/mod.rs:145-153`
- Modify: `crates/rightclaw/src/doctor.rs:963`

- [ ] **Step 1: Create the SQL file**

Create `crates/rightclaw/src/memory/sql/v16_usage_api_key_source.sql`:

```sql
-- v16: Add api_key_source to usage_events.
-- 'none' = OAuth/setup-token (subscription), other values = API key.
-- Column is added via a Rust hook with a pragma_table_info guard for
-- idempotency (SQLite lacks ADD COLUMN IF NOT EXISTS).
-- The DEFAULT also backfills existing rows to 'none' — a safe assumption
-- since current RightClaw deployments all use setup-token auth.
ALTER TABLE usage_events ADD COLUMN api_key_source TEXT NOT NULL DEFAULT 'none';
```

- [ ] **Step 2: Write the failing migration test**

Append to `crates/rightclaw/src/memory/migrations.rs` tests module (just before the closing `}` of `mod tests`):

```rust
    #[test]
    fn v16_usage_events_has_api_key_source() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('usage_events')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"api_key_source".to_string()),
            "api_key_source column missing"
        );
    }

    #[test]
    fn v16_backfills_existing_rows_to_none() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Apply up to v15, insert a row without api_key_source.
        MIGRATIONS.to_version(&mut conn, 15).unwrap();
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, session_uuid, total_cost_usd, num_turns, model_usage_json
             ) VALUES ('2026-04-20T00:00:00Z','interactive','s',0.0,1,'{}')",
            [],
        )
        .unwrap();
        // Apply v16.
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let src: String = conn
            .query_row(
                "SELECT api_key_source FROM usage_events LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(src, "none");
    }

    #[test]
    fn v16_idempotent_when_column_already_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 15).unwrap();
        conn.execute_batch(
            "ALTER TABLE usage_events ADD COLUMN api_key_source TEXT NOT NULL DEFAULT 'none'",
        )
        .unwrap();
        // v16 must succeed even though the column already exists.
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('usage_events')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"api_key_source".to_string()));
    }
```

- [ ] **Step 3: Run the tests — must fail**

Run: `cargo test -p rightclaw --lib memory::migrations::tests::v16_ -- --nocapture`
Expected: FAIL — "api_key_source column missing" / row missing column / etc.

- [ ] **Step 4: Register the v16 migration**

Modify `crates/rightclaw/src/memory/migrations.rs`:

After the `const V15_SCHEMA: &str = include_str!("sql/v15_usage_events.sql");` line, add:

```rust
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V16_SCHEMA: &str = include_str!("sql/v16_usage_api_key_source.sql");
```

Right after the `fn v13_one_shot_cron(...)` function, add:

```rust
/// v16: Add api_key_source column to usage_events.
///
/// Idempotent — checks pragma_table_info before ALTER. Column defaults
/// to 'none' which matches the setup-token (subscription) auth mode all
/// current RightClaw deployments use.
fn v16_usage_api_key_source(tx: &Transaction) -> Result<(), HookError> {
    let has_column: bool = tx
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('usage_events') WHERE name = ?1",
            ["api_key_source"],
            |r| r.get(0),
        )?;
    if !has_column {
        tx.execute_batch(
            "ALTER TABLE usage_events ADD COLUMN api_key_source TEXT NOT NULL DEFAULT 'none'",
        )?;
    }
    Ok(())
}
```

In the `MIGRATIONS` static, after `M::up(V15_SCHEMA),` add:

```rust
        M::up_with_hook("", v16_usage_api_key_source),
```

- [ ] **Step 5: Bump user_version constants**

Modify `crates/rightclaw/src/memory/mod.rs` — locate the `user_version_is_15` test (around line 145):

```rust
    #[test]
    fn user_version_is_16() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 16, "user_version should be 16 after V16 migration");
    }
```

(Rename `user_version_is_15` → `user_version_is_16`; change `15` → `16` in both the literal and the message.)

Modify `crates/rightclaw/src/doctor.rs:963`:

```rust
    let expected: u32 = 16;
```

- [ ] **Step 6: Run the tests — must pass**

Run: `cargo test -p rightclaw --lib memory::`
Expected: PASS — all migration + user_version tests green.

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v16_usage_api_key_source.sql \
        crates/rightclaw/src/memory/migrations.rs \
        crates/rightclaw/src/memory/mod.rs \
        crates/rightclaw/src/doctor.rs
git commit -m "feat(memory): v16 migration adds api_key_source to usage_events"
```

---

## Task 2: Pricing module

**Files:**
- Create: `crates/rightclaw/src/usage/pricing.rs`
- Modify: `crates/rightclaw/src/usage/mod.rs:7-10`

- [ ] **Step 1: Write the failing tests**

Create `crates/rightclaw/src/usage/pricing.rs` with tests module stub:

```rust
//! Per-model Anthropic pricing table used for cache-savings estimation.
//!
//! Rates source: https://www.anthropic.com/pricing
//! Update when Anthropic changes published per-token rates.

/// Per-million-token input and output rates for a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

/// Look up published rates for a model. Returns `None` for unknown models —
/// callers must render gracefully (e.g. omit the dollar portion of a
/// cache-savings line).
pub fn lookup(model: &str) -> Option<ModelPricing> {
    // Implementation added in Step 3.
    let _ = model;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sonnet_4_6_known() {
        let p = lookup("claude-sonnet-4-6").expect("must be known");
        assert!((p.input_per_mtok - 3.0).abs() < 1e-9);
        assert!((p.output_per_mtok - 15.0).abs() < 1e-9);
    }

    #[test]
    fn opus_4_7_known() {
        let p = lookup("claude-opus-4-7").expect("must be known");
        assert!((p.input_per_mtok - 15.0).abs() < 1e-9);
        assert!((p.output_per_mtok - 75.0).abs() < 1e-9);
    }

    #[test]
    fn haiku_dated_variant_matches() {
        let p = lookup("claude-haiku-4-5-20251001").expect("dated haiku must match");
        assert!((p.input_per_mtok - 0.80).abs() < 1e-9);
        assert!((p.output_per_mtok - 4.0).abs() < 1e-9);
    }

    #[test]
    fn haiku_undated_variant_matches() {
        let p = lookup("claude-haiku-4-5").expect("undated haiku must match");
        assert!((p.input_per_mtok - 0.80).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(lookup("claude-fake-9-0").is_none());
        assert!(lookup("").is_none());
    }
}
```

- [ ] **Step 2: Wire the module into the crate**

Modify `crates/rightclaw/src/usage/mod.rs` — change the module-declaration block at the top:

```rust
pub mod aggregate;
pub mod error;
pub mod format;
pub mod insert;
pub mod pricing;
```

- [ ] **Step 3: Run the tests — must fail**

Run: `cargo test -p rightclaw --lib usage::pricing::tests`
Expected: FAIL — all `expect("must be known")` panics trigger, `unknown_model_returns_none` passes.

- [ ] **Step 4: Implement `lookup`**

Replace the stub `lookup` body in `crates/rightclaw/src/usage/pricing.rs`:

```rust
pub fn lookup(model: &str) -> Option<ModelPricing> {
    if model == "claude-sonnet-4-6" {
        return Some(ModelPricing { input_per_mtok: 3.0, output_per_mtok: 15.0 });
    }
    if model == "claude-opus-4-7" {
        return Some(ModelPricing { input_per_mtok: 15.0, output_per_mtok: 75.0 });
    }
    if model.starts_with("claude-haiku-4-5") {
        return Some(ModelPricing { input_per_mtok: 0.80, output_per_mtok: 4.0 });
    }
    None
}
```

- [ ] **Step 5: Run the tests — must pass**

Run: `cargo test -p rightclaw --lib usage::pricing::tests`
Expected: PASS — all 5 tests green.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/usage/pricing.rs crates/rightclaw/src/usage/mod.rs
git commit -m "feat(usage): pricing table for cache-savings estimation"
```

---

## Task 3: Extend `UsageBreakdown` and `WindowSummary`

**Files:**
- Modify: `crates/rightclaw/src/usage/mod.rs:17-56`

- [ ] **Step 1: Write the failing test**

Append to `crates/rightclaw/src/usage/mod.rs` (below the existing type definitions, inside a new `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_breakdown_has_api_key_source_field() {
        let b = UsageBreakdown {
            session_uuid: "s".into(),
            total_cost_usd: 0.0,
            num_turns: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests: 0,
            web_fetch_requests: 0,
            model_usage_json: "{}".into(),
            api_key_source: "none".into(),
        };
        assert_eq!(b.api_key_source, "none");
    }

    #[test]
    fn window_summary_has_billing_split_fields() {
        let w = WindowSummary {
            source: "interactive".into(),
            cost_usd: 1.0,
            subscription_cost_usd: 0.6,
            api_cost_usd: 0.4,
            turns: 5,
            invocations: 3,
            input_tokens: 100,
            output_tokens: 200,
            cache_creation_tokens: 50,
            cache_read_tokens: 400,
            web_search_requests: 0,
            web_fetch_requests: 0,
            per_model: BTreeMap::new(),
        };
        assert!((w.subscription_cost_usd - 0.6).abs() < 1e-9);
        assert!((w.api_cost_usd - 0.4).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: Run — must fail**

Run: `cargo test -p rightclaw --lib usage::tests`
Expected: FAIL — "missing field `api_key_source`", "missing field `subscription_cost_usd`", etc.

- [ ] **Step 3: Add fields to types**

Modify `crates/rightclaw/src/usage/mod.rs` — inside `UsageBreakdown`, append a field:

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
    /// Raw `modelUsage` sub-object as JSON string (preserves per-model fields).
    pub model_usage_json: String,
    /// `apiKeySource` captured from the CC `system/init` event.
    /// 'none' = OAuth/setup-token (subscription). Other values = API key mode.
    pub api_key_source: String,
}
```

Inside `WindowSummary`, after `pub cost_usd: f64,`, add two fields:

```rust
pub struct WindowSummary {
    pub source: String,
    pub cost_usd: f64,
    pub subscription_cost_usd: f64,
    pub api_cost_usd: f64,
    pub turns: u64,
    // ... rest unchanged
}
```

- [ ] **Step 4: Fix downstream call sites that construct these structs**

The compiler will flag every construction. Patch them:

`crates/rightclaw/src/usage/insert.rs` — `sample_breakdown` at line 81:

```rust
    fn sample_breakdown() -> UsageBreakdown {
        UsageBreakdown {
            session_uuid: "uuid-1".into(),
            total_cost_usd: 0.05,
            num_turns: 3,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 100,
            cache_read_tokens: 200,
            web_search_requests: 1,
            web_fetch_requests: 2,
            model_usage_json: r#"{"claude-sonnet-4-6":{"costUSD":0.05}}"#.into(),
            api_key_source: "none".into(),
        }
    }
```

`crates/rightclaw/src/usage/aggregate.rs` — `breakdown` helper at line 111:

```rust
    fn breakdown(cost: f64, model: &str) -> UsageBreakdown {
        UsageBreakdown {
            session_uuid: "s".into(),
            total_cost_usd: cost,
            num_turns: 1,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 30,
            cache_read_tokens: 40,
            web_search_requests: 1,
            web_fetch_requests: 1,
            model_usage_json: format!(
                r#"{{"{model}":{{"costUSD":{cost},"inputTokens":10,"outputTokens":20,"cacheCreationInputTokens":30,"cacheReadInputTokens":40}}}}"#
            ),
            api_key_source: "none".into(),
        }
    }
```

`crates/bot/src/telegram/stream.rs` — inside `parse_usage_full`, add `api_key_source: "none".into(),` to the returned struct (worker overwrites it later; default to "none" is safe):

```rust
    Some(UsageBreakdown {
        session_uuid,
        total_cost_usd,
        num_turns,
        input_tokens: get_u64("/usage/input_tokens"),
        output_tokens: get_u64("/usage/output_tokens"),
        cache_creation_tokens: get_u64("/usage/cache_creation_input_tokens"),
        cache_read_tokens: get_u64("/usage/cache_read_input_tokens"),
        web_search_requests: get_u64("/usage/server_tool_use/web_search_requests"),
        web_fetch_requests: get_u64("/usage/server_tool_use/web_fetch_requests"),
        model_usage_json,
        api_key_source: "none".into(),
    })
```

For the `stream.rs` tests that construct `UsageBreakdown` directly — inspect each with `rg -n "UsageBreakdown \{" crates/bot/src/telegram/stream.rs` and add `api_key_source: "none".into(),` before the closing `}`.

`crates/rightclaw/src/usage/format.rs` tests — the `with` helper constructs `WindowSummary`. Add the two new fields:

```rust
    fn with(source: &str, cost: f64, invocations: u64, model: &str, model_cost: f64) -> WindowSummary {
        let mut per_model = BTreeMap::new();
        per_model.insert(model.to_string(), ModelTotals {
            cost_usd: model_cost,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        });
        WindowSummary {
            source: source.into(),
            cost_usd: cost,
            subscription_cost_usd: cost,
            api_cost_usd: 0.0,
            turns: 3,
            invocations,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests: 0,
            web_fetch_requests: 0,
            per_model,
        }
    }
```

- [ ] **Step 5: Run the workspace build**

Run: `cargo build --workspace`
Expected: clean build.

Run: `cargo test -p rightclaw --lib usage::`
Expected: PASS on all usage tests (insert, aggregate, format, mod::tests).

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/usage/mod.rs \
        crates/rightclaw/src/usage/insert.rs \
        crates/rightclaw/src/usage/aggregate.rs \
        crates/rightclaw/src/usage/format.rs \
        crates/bot/src/telegram/stream.rs
git commit -m "feat(usage): add api_key_source and billing split fields"
```

---

## Task 4: Persist `api_key_source` on insert

**Files:**
- Modify: `crates/rightclaw/src/usage/insert.rs:38-73`

- [ ] **Step 1: Write the failing test**

Append inside `tests` module in `crates/rightclaw/src/usage/insert.rs`:

```rust
    #[test]
    fn insert_persists_api_key_source() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        let mut b = sample_breakdown();
        b.api_key_source = "ANTHROPIC_API_KEY".into();
        insert_interactive(&conn, &b, 1, 0).unwrap();

        let src: String = conn
            .query_row(
                "SELECT api_key_source FROM usage_events LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(src, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn insert_default_api_key_source_is_none() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        // sample_breakdown sets api_key_source="none".
        insert_interactive(&conn, &sample_breakdown(), 1, 0).unwrap();

        let src: String = conn
            .query_row(
                "SELECT api_key_source FROM usage_events LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(src, "none");
    }
```

- [ ] **Step 2: Run — must fail**

Run: `cargo test -p rightclaw --lib usage::insert::tests::insert_persists_api_key_source`
Expected: FAIL — returned value is `"none"` (from column default), not `"ANTHROPIC_API_KEY"`, because INSERT statement never binds the field.

- [ ] **Step 3: Bind api_key_source in `insert_row`**

Modify `crates/rightclaw/src/usage/insert.rs` — rewrite `insert_row`:

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
            model_usage_json, api_key_source
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, ?7, ?8,
            ?9, ?10,
            ?11, ?12,
            ?13, ?14,
            ?15, ?16
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
        ],
    )?;
    Ok(())
}
```

- [ ] **Step 4: Run — must pass**

Run: `cargo test -p rightclaw --lib usage::insert::tests`
Expected: PASS on all insert tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/usage/insert.rs
git commit -m "feat(usage): persist api_key_source on insert"
```

---

## Task 5: Aggregate subscription vs API cost

**Files:**
- Modify: `crates/rightclaw/src/usage/aggregate.rs:9-56`

- [ ] **Step 1: Write the failing test**

Append inside `tests` module in `crates/rightclaw/src/usage/aggregate.rs`:

```rust
    fn breakdown_with_src(cost: f64, model: &str, src: &str) -> UsageBreakdown {
        let mut b = breakdown(cost, model);
        b.api_key_source = src.into();
        b
    }

    #[test]
    fn aggregate_splits_subscription_and_api_costs() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown_with_src(0.10, "sonnet", "none"), 1, 0).unwrap();
        insert_interactive(&conn, &breakdown_with_src(0.20, "sonnet", "none"), 1, 0).unwrap();
        insert_interactive(&conn, &breakdown_with_src(0.05, "sonnet", "ANTHROPIC_API_KEY"), 1, 0).unwrap();

        let s = aggregate(&conn, None, "interactive").unwrap();
        assert!((s.cost_usd - 0.35).abs() < 1e-9);
        assert!((s.subscription_cost_usd - 0.30).abs() < 1e-9);
        assert!((s.api_cost_usd - 0.05).abs() < 1e-9);
    }

    #[test]
    fn aggregate_subscription_only_has_zero_api_cost() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown_with_src(0.10, "sonnet", "none"), 1, 0).unwrap();

        let s = aggregate(&conn, None, "interactive").unwrap();
        assert!((s.subscription_cost_usd - 0.10).abs() < 1e-9);
        assert_eq!(s.api_cost_usd, 0.0);
    }

    #[test]
    fn aggregate_api_only_has_zero_subscription_cost() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown_with_src(0.10, "sonnet", "ANTHROPIC_API_KEY"), 1, 0).unwrap();

        let s = aggregate(&conn, None, "interactive").unwrap();
        assert_eq!(s.subscription_cost_usd, 0.0);
        assert!((s.api_cost_usd - 0.10).abs() < 1e-9);
    }
```

- [ ] **Step 2: Run — must fail**

Run: `cargo test -p rightclaw --lib usage::aggregate::tests::aggregate_splits_subscription_and_api_costs`
Expected: FAIL — current aggregate returns `subscription_cost_usd = 0.0`, `api_cost_usd = 0.0` (`WindowSummary` defaults).

- [ ] **Step 3: Extend the aggregate SQL**

Modify `aggregate` in `crates/rightclaw/src/usage/aggregate.rs`:

```rust
pub fn aggregate(
    conn: &Connection,
    since: Option<DateTime<Utc>>,
    source: &str,
) -> Result<WindowSummary, UsageError> {
    let since_str = since.map(|t| t.to_rfc3339());

    let (
        cost_usd, sub_cost, api_cost,
        turns, invocations,
        input, output, cache_c, cache_r,
        web_s, web_f,
    ): (f64, f64, f64, i64, i64, i64, i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(total_cost_usd), 0.0),
                COALESCE(SUM(CASE WHEN api_key_source = 'none' THEN total_cost_usd ELSE 0.0 END), 0.0),
                COALESCE(SUM(CASE WHEN api_key_source != 'none' THEN total_cost_usd ELSE 0.0 END), 0.0),
                COALESCE(SUM(num_turns), 0),
                COUNT(*),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_creation_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COALESCE(SUM(web_search_requests), 0),
                COALESCE(SUM(web_fetch_requests), 0)
             FROM usage_events
             WHERE source = ?1
               AND (?2 IS NULL OR ts >= ?2)",
            rusqlite::params![source, since_str],
            |r| Ok((
                r.get(0)?, r.get(1)?, r.get(2)?,
                r.get(3)?, r.get(4)?,
                r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?,
                r.get(9)?, r.get(10)?,
            )),
        )?;

    let per_model = aggregate_per_model(conn, &since_str, source)?;

    Ok(WindowSummary {
        source: source.to_string(),
        cost_usd,
        subscription_cost_usd: sub_cost,
        api_cost_usd: api_cost,
        turns: turns as u64,
        invocations: invocations as u64,
        input_tokens: input as u64,
        output_tokens: output as u64,
        cache_creation_tokens: cache_c as u64,
        cache_read_tokens: cache_r as u64,
        web_search_requests: web_s as u64,
        web_fetch_requests: web_f as u64,
        per_model,
    })
}
```

- [ ] **Step 4: Run — must pass**

Run: `cargo test -p rightclaw --lib usage::aggregate::tests`
Expected: PASS on all aggregate tests (existing + 3 new).

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/usage/aggregate.rs
git commit -m "feat(usage): aggregate splits subscription vs API cost"
```

---

## Task 6: `parse_api_key_source` helper

**Files:**
- Modify: `crates/bot/src/telegram/stream.rs`

- [ ] **Step 1: Write the failing tests**

Append inside the `tests` module in `crates/bot/src/telegram/stream.rs`:

```rust
    #[test]
    fn parse_api_key_source_happy_path() {
        let line = r#"{"type":"system","subtype":"init","cwd":"/x","session_id":"s","tools":[],"mcp_servers":[],"model":"claude-sonnet-4-6","permissionMode":"bypassPermissions","slash_commands":[],"apiKeySource":"none"}"#;
        assert_eq!(parse_api_key_source(line).as_deref(), Some("none"));
    }

    #[test]
    fn parse_api_key_source_api_key_mode() {
        let line = r#"{"type":"system","subtype":"init","apiKeySource":"ANTHROPIC_API_KEY"}"#;
        assert_eq!(parse_api_key_source(line).as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn parse_api_key_source_wrong_type_returns_none() {
        // Result event has apiKeySource-adjacent fields but different type.
        let line = r#"{"type":"result","apiKeySource":"none"}"#;
        assert!(parse_api_key_source(line).is_none());
    }

    #[test]
    fn parse_api_key_source_wrong_subtype_returns_none() {
        let line = r#"{"type":"system","subtype":"other","apiKeySource":"none"}"#;
        assert!(parse_api_key_source(line).is_none());
    }

    #[test]
    fn parse_api_key_source_missing_field_returns_none() {
        let line = r#"{"type":"system","subtype":"init"}"#;
        assert!(parse_api_key_source(line).is_none());
    }

    #[test]
    fn parse_api_key_source_malformed_json_returns_none() {
        assert!(parse_api_key_source("not json").is_none());
    }
```

- [ ] **Step 2: Run — must fail**

Run: `cargo test -p rightclaw-bot --lib telegram::stream::tests::parse_api_key_source_`
Expected: FAIL — "cannot find function `parse_api_key_source` in this scope".

- [ ] **Step 3: Implement the helper**

Modify `crates/bot/src/telegram/stream.rs` — just below `parse_usage_full`:

```rust
/// Parse `apiKeySource` from the CC `system/init` NDJSON line.
///
/// Returns `None` when:
/// - line is not valid JSON
/// - `type` is not `"system"` or `subtype` is not `"init"`
/// - `apiKeySource` key is absent
///
/// Callers fall back to `"none"` (subscription) if `None` is returned —
/// matching the column default in the `usage_events` table.
pub fn parse_api_key_source(init_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(init_json).ok()?;
    if v.get("type")?.as_str()? != "system" {
        return None;
    }
    if v.get("subtype")?.as_str()? != "init" {
        return None;
    }
    v.get("apiKeySource")?.as_str().map(|s| s.to_string())
}
```

- [ ] **Step 4: Run — must pass**

Run: `cargo test -p rightclaw-bot --lib telegram::stream::tests::parse_api_key_source_`
Expected: PASS on all 6 new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/stream.rs
git commit -m "feat(bot): parse_api_key_source extracts auth mode from init event"
```

---

## Task 7: Worker captures `apiKeySource` and threads into insert

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:1200-1250`

- [ ] **Step 1: Stash `apiKeySource` from init, pass to insert**

Modify `crates/bot/src/telegram/worker.rs` — locate the block around line 1200 that initialises the stream-loop locals and the result-branch that inserts.

Before the `loop {` that reads stream lines, add a new local. Find the existing:

```rust
    let mut ring_buffer = super::stream::EventRingBuffer::new(5);
    let mut usage = super::stream::StreamUsage::default();
    let mut result_line: Option<String> = None;
```

Add directly after:

```rust
    let mut api_key_source: Option<String> = None;
```

Inside the `Ok(Some(line))` branch of the loop, AFTER writing to `stream_log` (the `let _ = writeln!(log, "{line}");` block) but BEFORE `let event = super::stream::parse_stream_event(&line);`, add:

```rust
                        if api_key_source.is_none()
                            && let Some(src) = super::stream::parse_api_key_source(&line)
                        {
                            api_key_source = Some(src);
                        }
```

Then update the `StreamEvent::Result(json)` branch where `parse_usage_full` is called. Replace the existing block:

```rust
                            super::stream::StreamEvent::Result(json) => {
                                usage = super::stream::parse_usage(json);
                                result_line = Some(json.clone());

                                match super::stream::parse_usage_full(json) {
                                    Some(breakdown) => {
                                        if let Err(e) =
                                            rightclaw::usage::insert::insert_interactive(
                                                &conn,
                                                &breakdown,
                                                chat_id,
                                                eff_thread_id,
                                            )
                                        {
                                            tracing::warn!(
                                                ?chat_id,
                                                "usage insert failed: {e:#}"
                                            );
                                        }
                                    }
                                    None => tracing::warn!(
                                        ?chat_id,
                                        "result event missing required usage fields"
                                    ),
                                }
                            }
```

with (overrides `api_key_source` on the breakdown before inserting):

```rust
                            super::stream::StreamEvent::Result(json) => {
                                usage = super::stream::parse_usage(json);
                                result_line = Some(json.clone());

                                match super::stream::parse_usage_full(json) {
                                    Some(mut breakdown) => {
                                        breakdown.api_key_source = api_key_source
                                            .clone()
                                            .unwrap_or_else(|| "none".into());
                                        if let Err(e) =
                                            rightclaw::usage::insert::insert_interactive(
                                                &conn,
                                                &breakdown,
                                                chat_id,
                                                eff_thread_id,
                                            )
                                        {
                                            tracing::warn!(
                                                ?chat_id,
                                                "usage insert failed: {e:#}"
                                            );
                                        }
                                    }
                                    None => tracing::warn!(
                                        ?chat_id,
                                        "result event missing required usage fields"
                                    ),
                                }
                            }
```

- [ ] **Step 2: Build**

Run: `cargo build -p rightclaw-bot`
Expected: clean build.

- [ ] **Step 3: Run bot tests to confirm no regression**

Run: `cargo test -p rightclaw-bot --lib telegram::stream::`
Expected: PASS.

Run: `cargo build --workspace`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): worker threads apiKeySource from init into usage insert"
```

---

## Task 8: Cron captures `apiKeySource` and threads into insert

**Files:**
- Modify: `crates/bot/src/cron.rs:611-622`

- [ ] **Step 1: Modify the cron result-line hook**

Modify `crates/bot/src/cron.rs` — replace the existing block (around lines 611–622):

```rust
    if let Some(result_line) = find_last_result_line(&collected_lines) {
        match crate::telegram::stream::parse_usage_full(result_line) {
            Some(breakdown) => {
                if let Err(e) = rightclaw::usage::insert::insert_cron(&conn, &breakdown, job_name) {
                    tracing::warn!(job = %job_name, "usage insert failed: {e:#}");
                }
            }
            None => {
                tracing::warn!(job = %job_name, "result event missing required usage fields");
            }
        }
    }
```

with (scan `collected_lines` for the init event and thread the value through):

```rust
    if let Some(result_line) = find_last_result_line(&collected_lines) {
        match crate::telegram::stream::parse_usage_full(result_line) {
            Some(mut breakdown) => {
                // Scan all lines for the init event (first line that matches wins).
                breakdown.api_key_source = collected_lines
                    .iter()
                    .find_map(|l| crate::telegram::stream::parse_api_key_source(l))
                    .unwrap_or_else(|| "none".into());
                if let Err(e) = rightclaw::usage::insert::insert_cron(&conn, &breakdown, job_name) {
                    tracing::warn!(job = %job_name, "usage insert failed: {e:#}");
                }
            }
            None => {
                tracing::warn!(job = %job_name, "result event missing required usage fields");
            }
        }
    }
```

- [ ] **Step 2: Build**

Run: `cargo build -p rightclaw-bot`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(bot): cron threads apiKeySource from init into usage insert"
```

---

## Task 9: Rewrite `format.rs` — new layout, cache line, detail mode

**Files:**
- Modify: `crates/rightclaw/src/usage/format.rs` (full rewrite)

- [ ] **Step 1: Write the failing tests**

Replace the entire contents of the existing `tests` module in `crates/rightclaw/src/usage/format.rs` with this new set (existing tests stay where they still apply; new ones extend coverage):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{ModelTotals, WindowSummary};
    use std::collections::BTreeMap;

    fn empty(source: &str) -> WindowSummary {
        WindowSummary { source: source.into(), ..Default::default() }
    }

    fn sub_only(source: &str, cost: f64, model: &str) -> WindowSummary {
        let mut per_model = BTreeMap::new();
        per_model.insert(model.to_string(), ModelTotals {
            cost_usd: cost,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 50,
            cache_read_tokens: 300,
        });
        WindowSummary {
            source: source.into(),
            cost_usd: cost,
            subscription_cost_usd: cost,
            api_cost_usd: 0.0,
            turns: 3,
            invocations: 1,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 50,
            cache_read_tokens: 300,
            web_search_requests: 0,
            web_fetch_requests: 0,
            per_model,
        }
    }

    fn api_only(source: &str, cost: f64, model: &str) -> WindowSummary {
        let mut w = sub_only(source, cost, model);
        w.subscription_cost_usd = 0.0;
        w.api_cost_usd = cost;
        w
    }

    fn all_empty() -> AllWindows {
        AllWindows {
            today_interactive: empty("interactive"),
            today_cron: empty("cron"),
            week_interactive: empty("interactive"),
            week_cron: empty("cron"),
            month_interactive: empty("interactive"),
            month_cron: empty("cron"),
            all_interactive: empty("interactive"),
            all_cron: empty("cron"),
        }
    }

    #[test]
    fn empty_db_returns_no_usage_line() {
        assert_eq!(format_summary_message(&all_empty(), false), "No usage recorded yet.");
    }

    #[test]
    fn empty_window_shows_no_activity() {
        let mut w = all_empty();
        w.week_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.month_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("Today"));
        assert!(msg.contains("(no activity)"));
        assert!(msg.contains("Last 7 days"));
    }

    #[test]
    fn default_mode_omits_raw_tokens_line() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        assert!(!msg.contains("Tokens:"), "default mode must not include raw tokens");
    }

    #[test]
    fn detail_mode_includes_raw_tokens_line() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, true);
        assert!(msg.contains("Tokens:"));
        assert!(msg.contains("new in"));
        assert!(msg.contains("cache-created"));
        assert!(msg.contains("cache-read"));
    }

    #[test]
    fn cache_line_renders_hit_rate_and_dollar_savings_for_known_model() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        // cache_read=300, total_input=10+50+300=360 → 83%. At $3/Mtok × 0.9 × 300 = $0.00081 → "<$0.01".
        assert!(msg.contains("Cache:"));
        assert!(msg.contains("83%"));
        assert!(msg.contains("saved"));
    }

    #[test]
    fn cache_line_without_dollar_for_unknown_model() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-fake-unknown");
        w.all_interactive = sub_only("interactive", 0.1, "claude-fake-unknown");
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("83%"));
        assert!(!msg.contains("saved"), "unknown model must not render 'saved' clause");
    }

    #[test]
    fn cache_line_omitted_when_no_cache_reads() {
        let mut ws = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        // Zero out cache reads on both the window totals and per-model.
        ws.cache_read_tokens = 0;
        ws.per_model.get_mut("claude-sonnet-4-6").unwrap().cache_read_tokens = 0;
        let mut w = all_empty();
        w.today_interactive = ws.clone();
        w.all_interactive = ws;
        let msg = format_summary_message(&w, false);
        assert!(!msg.contains("Cache:"), "cache line must be omitted when cache_read=0");
    }

    #[test]
    fn subscription_only_window_has_subscription_footnote() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("Subscription covers this"));
    }

    #[test]
    fn api_only_window_labels_block_and_has_api_footnote() {
        let mut w = all_empty();
        w.today_interactive = api_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = api_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("(API-billed)"));
        assert!(msg.contains("Billed via API key"));
        assert!(!msg.contains("Subscription covers this"));
    }

    #[test]
    fn mixed_billing_window_shows_split_footer() {
        let mut ws = sub_only("interactive", 0.30, "claude-sonnet-4-6");
        ws.subscription_cost_usd = 0.20;
        ws.api_cost_usd = 0.10;
        let mut w = all_empty();
        w.today_interactive = ws.clone();
        w.all_interactive = ws;
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("(Mixed)"));
        assert!(msg.contains("Subscription: $0.20"));
        assert!(msg.contains("API-billed: $0.10"));
    }

    #[test]
    fn total_footer_plain_when_single_mode() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("Total retail: $0.10"));
        assert!(!msg.contains("subscription:"), "single-mode footer must not show split");
    }

    #[test]
    fn total_footer_splits_when_both_modes_present() {
        let mut ws_sub = sub_only("interactive", 0.10, "claude-sonnet-4-6");
        let mut ws_api = api_only("cron", 0.05, "claude-sonnet-4-6");
        // All-time window needs combined totals for the footer line.
        let mut ws_all = ws_sub.clone();
        ws_all.source = "interactive".into();
        ws_all.cost_usd = 0.10;
        ws_all.subscription_cost_usd = 0.10;
        ws_all.api_cost_usd = 0.0;
        let mut ws_all_cron = ws_api.clone();
        ws_all_cron.cost_usd = 0.05;
        ws_all_cron.subscription_cost_usd = 0.0;
        ws_all_cron.api_cost_usd = 0.05;

        let w = AllWindows {
            today_interactive: ws_sub.clone(),
            today_cron: ws_api.clone(),
            week_interactive: ws_sub.clone(),
            week_cron: ws_api.clone(),
            month_interactive: ws_sub,
            month_cron: ws_api,
            all_interactive: ws_all,
            all_cron: ws_all_cron,
        };
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("Total retail: $0.15"));
        assert!(msg.contains("subscription: $0.10"));
        assert!(msg.contains("API-billed: $0.05"));
    }

    #[test]
    fn cost_below_one_cent_shown_as_less_than() {
        assert_eq!(format_cost(0.003), "&lt;$0.01");
        assert_eq!(format_cost(0.0), "$0.00");
        assert_eq!(format_cost(1.234), "$1.23");
    }

    #[test]
    fn counts_use_k_and_m_suffix() {
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(1_234), "1.2k");
        assert_eq!(format_count(1_234_567), "1.2M");
    }

    #[test]
    fn html_escape_applied_to_model_names() {
        let mut ws = sub_only("interactive", 0.1, "foo<script>");
        // Need cache_read=0 to skip pricing.lookup on the nonsense name.
        ws.cache_read_tokens = 0;
        ws.per_model.get_mut("foo<script>").unwrap().cache_read_tokens = 0;
        let mut w = all_empty();
        w.today_interactive = ws.clone();
        w.all_interactive = ws;
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("foo&lt;script&gt;"));
        assert!(!msg.contains("<script>"));
    }
}
```

- [ ] **Step 2: Run — must fail**

Run: `cargo test -p rightclaw --lib usage::format::tests`
Expected: FAIL on most tests — current format does not accept `detail` arg, does not show subscription footnote, etc.

- [ ] **Step 3: Rewrite format.rs body**

Replace the **pre-tests** portion of `crates/rightclaw/src/usage/format.rs` (everything from the top through the last `fn` above `#[cfg(test)]`) with:

```rust
//! Telegram HTML message rendering for `/usage`.

use crate::usage::{ModelTotals, WindowSummary, pricing};

/// All windows × sources, as produced by the handler before rendering.
pub struct AllWindows {
    pub today_interactive: WindowSummary,
    pub today_cron: WindowSummary,
    pub week_interactive: WindowSummary,
    pub week_cron: WindowSummary,
    pub month_interactive: WindowSummary,
    pub month_cron: WindowSummary,
    pub all_interactive: WindowSummary,
    pub all_cron: WindowSummary,
}

/// Render the complete `/usage` summary as Telegram HTML. When `detail` is
/// true each source block also renders a raw-tokens line.
pub fn format_summary_message(w: &AllWindows, detail: bool) -> String {
    let total_invocations = w.all_interactive.invocations + w.all_cron.invocations;
    if total_invocations == 0 {
        return "No usage recorded yet.".to_string();
    }

    let total_cost = w.all_interactive.cost_usd + w.all_cron.cost_usd;
    let total_sub = w.all_interactive.subscription_cost_usd + w.all_cron.subscription_cost_usd;
    let total_api = w.all_interactive.api_cost_usd + w.all_cron.api_cost_usd;

    let mut out = String::new();
    out.push_str("\u{1f4ca} <b>Usage Summary</b> (UTC)\n\n");
    out.push_str(&render_window("Today", &w.today_interactive, &w.today_cron, detail));
    out.push_str(&render_window("Last 7 days", &w.week_interactive, &w.week_cron, detail));
    out.push_str(&render_window("Last 30 days", &w.month_interactive, &w.month_cron, detail));
    out.push_str(&render_window("All time", &w.all_interactive, &w.all_cron, detail));

    // Total footer: plain when single mode, split when both present.
    if total_sub > 0.0 && total_api > 0.0 {
        out.push_str(&format!(
            "\n<b>Total retail:</b> {} · subscription: {} · API-billed: {}\n",
            format_cost(total_cost),
            format_cost(total_sub),
            format_cost(total_api),
        ));
    } else {
        out.push_str(&format!("\n<b>Total retail:</b> {}\n", format_cost(total_cost)));
    }
    out
}

fn render_window(title: &str, interactive: &WindowSummary, cron: &WindowSummary, detail: bool) -> String {
    let mut s = format!("\u{2501}\u{2501} <b>{}</b> \u{2501}\u{2501}\n", html_escape(title));
    if interactive.invocations == 0 && cron.invocations == 0 {
        s.push_str("(no activity)\n\n");
        return s;
    }
    if interactive.invocations > 0 {
        s.push_str(&render_source("\u{1f4ac} Interactive", interactive, "sessions", detail));
    }
    if cron.invocations > 0 {
        s.push_str(&render_source("\u{23f0} Cron", cron, "runs", detail));
    }
    let web_s = interactive.web_search_requests + cron.web_search_requests;
    let web_f = interactive.web_fetch_requests + cron.web_fetch_requests;
    if web_s > 0 || web_f > 0 {
        s.push_str(&format!("\u{1f50e} Web: {web_s} searches, {web_f} fetches\n"));
    }

    // Footer per window.
    let sub = interactive.subscription_cost_usd + cron.subscription_cost_usd;
    let api = interactive.api_cost_usd + cron.api_cost_usd;
    if sub > 0.0 && api > 0.0 {
        s.push_str(&format!(
            "Subscription: {} · API-billed: {}\n",
            format_cost(sub),
            format_cost(api),
        ));
    } else if api == 0.0 && sub > 0.0 {
        s.push_str("Subscription covers this (Claude subscription via setup-token)\n");
    } else if sub == 0.0 && api > 0.0 {
        s.push_str("Billed via API key\n");
    }
    s.push('\n');
    s
}

fn render_source(label: &str, w: &WindowSummary, unit: &str, detail: bool) -> String {
    let billing_tag = match (w.subscription_cost_usd > 0.0, w.api_cost_usd > 0.0) {
        (true, true) => " (Mixed)",
        (false, true) => " (API-billed)",
        _ => "",
    };
    let mut s = format!(
        "{label}{billing_tag}: {cost} retail · {turns} turns · {count} {unit}\n",
        cost = format_cost(w.cost_usd),
        turns = w.turns,
        count = w.invocations,
    );

    // Per-model lines sorted by cost desc for readability.
    let mut models: Vec<_> = w.per_model.iter().collect();
    models.sort_by(|a, b| b.1.cost_usd.partial_cmp(&a.1.cost_usd).unwrap_or(std::cmp::Ordering::Equal));
    for (name, totals) in &models {
        s.push_str(&format!(
            "  {} \u{2014} {}\n",
            html_escape(name),
            format_cost(totals.cost_usd),
        ));
    }

    // Cache effectiveness line (omitted when no cache reads in this window).
    if let Some(line) = format_cache_line(&w.per_model) {
        s.push_str("  ");
        s.push_str(&line);
        s.push('\n');
    }

    // Detail mode: raw-tokens line.
    if detail {
        s.push_str(&format!(
            "  Tokens: {} new in, {} out, {} cache-created, {} cache-read\n",
            format_count(w.input_tokens),
            format_count(w.output_tokens),
            format_count(w.cache_creation_tokens),
            format_count(w.cache_read_tokens),
        ));
    }
    s
}

/// Build the "Cache: H% hit rate, saved ~$C.CC" line for a window's per-model
/// map. Returns `None` when every model has zero cache reads (nothing to say).
/// When some models are priced and others aren't, the dollar savings still
/// represents only the priced portion — accepted as "estimate", not audit.
fn format_cache_line(per_model: &std::collections::BTreeMap<String, ModelTotals>) -> Option<String> {
    let mut total_cache_read: u64 = 0;
    let mut total_input_bearing: u64 = 0; // input + cache_creation + cache_read
    let mut total_savings_usd: f64 = 0.0;
    let mut any_priced = false;

    for (model, t) in per_model {
        total_cache_read = total_cache_read.saturating_add(t.cache_read_tokens);
        total_input_bearing = total_input_bearing
            .saturating_add(t.input_tokens)
            .saturating_add(t.cache_creation_tokens)
            .saturating_add(t.cache_read_tokens);
        if let Some(p) = pricing::lookup(model) {
            any_priced = true;
            // Cached reads cost 10% of regular input, so 90% of the fresh-input
            // rate is saved per cached token.
            total_savings_usd += t.cache_read_tokens as f64 * p.input_per_mtok * 0.9 / 1_000_000.0;
        }
    }

    if total_cache_read == 0 {
        return None;
    }

    let hit_rate = if total_input_bearing == 0 {
        0.0
    } else {
        total_cache_read as f64 / total_input_bearing as f64
    };
    let pct = (hit_rate * 100.0).round() as u32;

    if any_priced {
        Some(format!("Cache: {pct}% hit rate, saved ~{}", format_cost(total_savings_usd)))
    } else {
        Some(format!("Cache: {pct}% hit rate"))
    }
}

fn format_cost(v: f64) -> String {
    if v > 0.0 && v < 0.01 {
        "&lt;$0.01".to_string()
    } else {
        format!("${v:.2}")
    }
}

fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
```

- [ ] **Step 4: Run — must pass**

Run: `cargo test -p rightclaw --lib usage::format::tests`
Expected: PASS on all format tests (existing + new).

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/usage/format.rs
git commit -m "feat(usage): redesigned /usage rendering — cache line, billing split, detail mode"
```

---

## Task 10: `/usage detail` argument + handler flag

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs:65-67`
- Modify: `crates/bot/src/telegram/handler.rs:1459-1514`

- [ ] **Step 1: Change `BotCommand::Usage` to take a String**

Modify `crates/bot/src/telegram/dispatch.rs` — replace the `Usage` variant:

```rust
    #[command(description = "Show usage summary (add 'detail' for raw tokens)")]
    Usage(String),
```

Update the branch that wires the handler — find the existing `.branch(dptree::case![BotCommand::Usage].endpoint(handle_usage))` and replace with:

```rust
        .branch(dptree::case![BotCommand::Usage(arg)].endpoint(handle_usage))
```

- [ ] **Step 2: Change `handle_usage` signature + plumb the flag**

Modify `crates/bot/src/telegram/handler.rs`. Replace `handle_usage`:

```rust
/// Handle the /usage command — aggregate and render a usage summary.
/// `arg` is the trailing text after `/usage`; accepts `"detail"` or `"d"` to
/// include raw-tokens lines, anything else → default (no detail).
pub async fn handle_usage(
    bot: BotType,
    msg: Message,
    arg: String,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "usage", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    let detail = matches!(arg.trim().to_ascii_lowercase().as_str(), "detail" | "d");
    let text = match build_usage_summary(&agent_dir.0, detail).await {
        Ok(t) => t,
        Err(e) => format!("Failed to read usage: {e:#}"),
    };
    let eff_thread_id = effective_thread_id(&msg);
    send_html_reply(&bot, msg.chat.id, eff_thread_id, &text).await?;
    Ok(())
}
```

Replace `build_usage_summary` — add `detail: bool` param, thread into format call:

```rust
async fn build_usage_summary(agent_dir: &Path, detail: bool) -> Result<String, miette::Report> {
    use chrono::{Duration, Utc};
    use rightclaw::usage::aggregate::aggregate;
    use rightclaw::usage::format::{AllWindows, format_summary_message};

    let conn = rightclaw::memory::open_connection(agent_dir, false)
        .map_err(|e| miette::miette!("open_connection: {e:#}"))?;

    let now = Utc::now();
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let week_start = now - Duration::days(7);
    let month_start = now - Duration::days(30);

    let windows = AllWindows {
        today_interactive: aggregate(&conn, Some(today_start), "interactive")
            .map_err(|e| miette::miette!("aggregate today/interactive: {e:#}"))?,
        today_cron: aggregate(&conn, Some(today_start), "cron")
            .map_err(|e| miette::miette!("aggregate today/cron: {e:#}"))?,
        week_interactive: aggregate(&conn, Some(week_start), "interactive")
            .map_err(|e| miette::miette!("aggregate week/interactive: {e:#}"))?,
        week_cron: aggregate(&conn, Some(week_start), "cron")
            .map_err(|e| miette::miette!("aggregate week/cron: {e:#}"))?,
        month_interactive: aggregate(&conn, Some(month_start), "interactive")
            .map_err(|e| miette::miette!("aggregate month/interactive: {e:#}"))?,
        month_cron: aggregate(&conn, Some(month_start), "cron")
            .map_err(|e| miette::miette!("aggregate month/cron: {e:#}"))?,
        all_interactive: aggregate(&conn, None, "interactive")
            .map_err(|e| miette::miette!("aggregate all/interactive: {e:#}"))?,
        all_cron: aggregate(&conn, None, "cron")
            .map_err(|e| miette::miette!("aggregate all/cron: {e:#}"))?,
    };

    Ok(format_summary_message(&windows, detail))
}
```

- [ ] **Step 3: Build**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 4: Run affected tests**

Run: `cargo test -p rightclaw-bot --lib telegram::dispatch`
Expected: PASS (dispatcher_builds_without_panic in particular — type checks the dptree wiring).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs
git commit -m "feat(bot): /usage detail argument toggles raw-tokens rendering"
```

---

## Task 11: Slash-menu fix — pre-delete language-scoped commands

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs:209-226`

- [ ] **Step 1: Add the pre-delete pass**

Modify `crates/bot/src/telegram/dispatch.rs` — locate the existing `Register commands in three overlapping scopes...` block. Before the `let commands = BotCommand::bot_commands();` line, insert a new block:

```rust
    // Pre-delete any language-scoped command lists from prior deployments.
    // Telegram's resolution order is: scope+language wins over scope-only, so
    // stale language-scoped entries shadow our fresh per-scope set. Best-effort,
    // errors ignored (e.g. the slot was never populated).
    for scope in [
        teloxide::types::BotCommandScope::Default,
        teloxide::types::BotCommandScope::AllPrivateChats,
        teloxide::types::BotCommandScope::AllGroupChats,
    ] {
        for lang in ["en", "ru"] {
            let _ = bot
                .delete_my_commands()
                .scope(scope.clone())
                .language_code(lang.to_string())
                .await;
        }
    }
```

- [ ] **Step 2: Build**

Run: `cargo build -p rightclaw-bot`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/dispatch.rs
git commit -m "fix(bot): pre-delete language-scoped commands to prevent menu-shadow"
```

---

## Task 12: Workspace build + clippy + full test run

**Files:** none (quality gate)

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: clean build, no warnings on new code.

- [ ] **Step 2: Clippy on the touched crates**

Run: `cargo clippy -p rightclaw -p rightclaw-bot -- -D warnings`
Expected: clean.

- [ ] **Step 3: Full test run**

Run: `cargo test --workspace`
Expected: all tests green. Any new failure must be investigated — no `#[ignore]` escapes.

- [ ] **Step 4: Verify migration on a real existing DB (dry run)**

```bash
cp ~/.rightclaw/agents/<pick-an-agent>/data.db /tmp/usage-migration-test.db
sqlite3 /tmp/usage-migration-test.db 'PRAGMA user_version;'
# Expected: 15 (pre-upgrade state)

cargo run -p rightclaw-cli --bin rightclaw -- doctor 2>&1 | head -30
# OK for the doctor to report schema mismatch; the real migration happens on bot start.
```

No commit for this task — it's a quality gate, not a code change.

---

## Task 13: Manual verification checklist

**Files:** none (user-driven)

- [ ] **Step 1: Restart bot on a real agent**

```bash
rightclaw down
rightclaw up --detach
```

Tail the log:

```bash
tail -F ~/.rightclaw/logs/<agent>.log | grep -i -E 'migration|usage'
```

Expected: v16 migration applied, no errors.

- [ ] **Step 2: Verify schema on host DB**

```bash
sqlite3 ~/.rightclaw/agents/<agent>/data.db 'PRAGMA table_info(usage_events);'
```

Expected: `api_key_source` column present, type TEXT, default `'none'`.

```bash
sqlite3 ~/.rightclaw/agents/<agent>/data.db "SELECT api_key_source, COUNT(*) FROM usage_events GROUP BY api_key_source;"
```

Expected: all existing rows grouped under `'none'`.

- [ ] **Step 3: Interactive exercise**

- Send a Telegram message to the agent; wait for reply.
- `/usage` — expect: new layout, no `cache_c`/`cache_r` line, `Cache:` line with hit rate (and savings for Sonnet), `Subscription covers this` footnote.
- `/usage detail` — expect: same as above, plus a `Tokens:` line per source block.

- [ ] **Step 4: Cron exercise**

- Wait for a cron job to run (or force-trigger one via `/cron run <job>`).
- `/usage` — expect: `⏰ Cron` block populated with cache line.

- [ ] **Step 5: Verify slash-menu**

- In Telegram, type `/` in the agent DM. Menu should show `/usage`.
- If not, restart Telegram client once (client-side cache). Menu should appear on second try.

- [ ] **Step 6: (Optional) API key mode exercise**

- Temporarily start the bot with `ANTHROPIC_API_KEY=sk-... rightclaw up`.
- Send a message; run `/usage`.
- Expect: `(API-billed)` tag on the Interactive block, `Billed via API key` footnote on that window.
- Revert env var.

---

## Notes

- No backward-compatibility shims are needed — `api_key_source` defaults to `'none'` on existing rows, so agents upgrading in place don't need recreation.
- `cron_delivery.rs` is intentionally unchanged. Delivery uses `--output-format json` (non-streaming), so there's no init event to parse; retain the existing TODO comment.
- When Anthropic updates published rates, bump `crates/rightclaw/src/usage/pricing.rs`. The update is purely additive and doesn't touch stored data.
- Telegram's client-side command cache is outside our control — expect one manual client restart after the bot upgrade for some users.
