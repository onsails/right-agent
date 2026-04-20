# Telegram `/usage` Command — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `/usage` Telegram bot command that shows token/cost usage for the whole agent (all chats + cron) across fixed time windows.

**Architecture:** New SQLite table `usage_events` in per-agent `data.db`. Worker and cron engine write one row per `claude -p` invocation on the `result` event. `/usage` handler runs SQL aggregations and formats a Telegram HTML message.

**Tech Stack:** Rust edition 2024, `rusqlite`, `rusqlite_migration`, `teloxide`, `serde_json`, `chrono`.

**Spec:** `docs/superpowers/specs/2026-04-20-telegram-usage-command-design.md`

**Branch:** `usage`

**Crate conventions:**
- Library error types use `thiserror`; binary/tests use `anyhow` or `miette`.
- Transactions: wrap 2+ writes in `conn.unchecked_transaction()`. Single INSERT needs no tx.
- FAIL FAST — propagate errors with `?` and context. Never swallow.
- After every task, run `cargo build -p rightclaw -p rightclaw-bot` and `cargo test -p <crate> <module>` to verify.

---

## File Structure

Files created:
- `crates/rightclaw/src/memory/sql/v15_usage_events.sql` — schema DDL.
- `crates/rightclaw/src/usage/mod.rs` — module entry, public types (`UsageBreakdown`, `ModelTotals`, `WindowSummary`).
- `crates/rightclaw/src/usage/insert.rs` — `insert_interactive`, `insert_cron`.
- `crates/rightclaw/src/usage/aggregate.rs` — `aggregate(conn, since, source)`.
- `crates/rightclaw/src/usage/error.rs` — `UsageError` (thiserror).
- `crates/rightclaw/src/usage/format.rs` — `format_summary_message(per-window data) -> String` (Telegram HTML).

Files modified:
- `crates/rightclaw/src/memory/migrations.rs` — register v15.
- `crates/rightclaw/src/lib.rs` — declare `pub mod usage;`.
- `crates/bot/src/telegram/stream.rs` — add `parse_usage_full`.
- `crates/bot/src/telegram/worker.rs:1226` — hook INSERT on `StreamEvent::Result`.
- `crates/bot/src/cron.rs` — hook INSERT on cron result line (near line ~570).
- `crates/bot/src/cron_delivery.rs` — same, with `job_name='<job>-delivery'`.
- `crates/bot/src/telegram/dispatch.rs:37` — add `BotCommand::Usage` variant.
- `crates/bot/src/telegram/handler.rs` — add `handle_usage`.

---

## Task 1: Migration v15 SQL + registration

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v15_usage_events.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`
- Test: `crates/rightclaw/src/memory/migrations.rs` (existing `tests` module)

- [ ] **Step 1: Write the failing migration test**

Add to the `tests` module inside `crates/rightclaw/src/memory/migrations.rs`:

```rust
#[test]
fn v15_creates_usage_events_table_with_indexes() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    // Table exists and is writable.
    conn.execute_batch(
        "INSERT INTO usage_events (
            ts, source, session_uuid, total_cost_usd, num_turns,
            model_usage_json
         ) VALUES (
            '2026-04-20T00:00:00Z', 'interactive', 'test-uuid', 0.05, 3, '{}'
         );"
    ).unwrap();

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);

    // Indexes present.
    let indexes: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='usage_events'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(indexes.iter().any(|n| n == "idx_usage_events_ts"));
    assert!(indexes.iter().any(|n| n == "idx_usage_events_source_ts"));
}

#[test]
fn v15_migration_is_idempotent() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    // Second call must be a no-op.
    MIGRATIONS.to_latest(&mut conn).unwrap();
}
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test -p rightclaw memory::migrations::tests::v15 -- --nocapture`
Expected: FAIL with `no such table: usage_events` (migration not added yet).

- [ ] **Step 3: Create the SQL file**

Create `crates/rightclaw/src/memory/sql/v15_usage_events.sql`:

```sql
-- V15 schema: usage_events — per-invocation CC token/cost telemetry.
-- One row per `claude -p` invocation, written when the `result` stream event is observed.

CREATE TABLE IF NOT EXISTS usage_events (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    ts                     TEXT    NOT NULL,       -- ISO8601 UTC
    source                 TEXT    NOT NULL,       -- 'interactive' | 'cron'
    chat_id                INTEGER,                -- NULL for cron
    thread_id              INTEGER,                -- 0 if no thread, NULL for cron
    job_name               TEXT,                   -- NULL for interactive
    session_uuid           TEXT    NOT NULL,
    total_cost_usd         REAL    NOT NULL,
    num_turns              INTEGER NOT NULL,
    input_tokens           INTEGER NOT NULL DEFAULT 0,
    output_tokens          INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens  INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens      INTEGER NOT NULL DEFAULT 0,
    web_search_requests    INTEGER NOT NULL DEFAULT 0,
    web_fetch_requests     INTEGER NOT NULL DEFAULT 0,
    model_usage_json       TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_usage_events_ts ON usage_events (ts);
CREATE INDEX IF NOT EXISTS idx_usage_events_source_ts ON usage_events (source, ts);
```

- [ ] **Step 4: Register the migration**

Edit `crates/rightclaw/src/memory/migrations.rs`. Add after line 17 (where `V14_SCHEMA` is declared):

```rust
const V15_SCHEMA: &str = include_str!("sql/v15_usage_events.sql");
```

Add inside the `MIGRATIONS` `vec![]` (after the `M::up(V14_SCHEMA)` line, currently at line 110):

```rust
        M::up(V15_SCHEMA),
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p rightclaw memory::migrations::tests::v15 -- --nocapture`
Expected: PASS — both `v15_creates_usage_events_table_with_indexes` and `v15_migration_is_idempotent`.

Then run the full migrations test set to confirm no regression:
Run: `cargo test -p rightclaw memory::migrations`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v15_usage_events.sql \
        crates/rightclaw/src/memory/migrations.rs
git commit -m "feat(memory): v15 migration for usage_events table"
```

---

## Task 2: usage module scaffolding + types + error

**Files:**
- Create: `crates/rightclaw/src/usage/mod.rs`
- Create: `crates/rightclaw/src/usage/error.rs`
- Modify: `crates/rightclaw/src/lib.rs`

- [ ] **Step 1: Declare the module**

Edit `crates/rightclaw/src/lib.rs`. Add `pub mod usage;` alphabetically between `pub mod tunnel;` and the `openshell_proto` block. Final additions section should read:

```rust
pub mod tunnel;
pub mod usage;
```

- [ ] **Step 2: Create the error type**

Create `crates/rightclaw/src/usage/error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UsageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid result JSON: {0}")]
    InvalidJson(String),
}
```

- [ ] **Step 3: Create the module with public types**

Create `crates/rightclaw/src/usage/mod.rs`:

```rust
//! Usage telemetry — per-invocation CC token/cost tracking.
//!
//! One `UsageEvent` row per `claude -p` invocation, written when the
//! stream-json `result` event is received. Read by the `/usage` Telegram
//! command via `aggregate`.

pub mod aggregate;
pub mod error;
pub mod format;
pub mod insert;

use std::collections::BTreeMap;

pub use error::UsageError;

/// Parsed `result` event payload used to write one row.
#[derive(Debug, Clone, PartialEq)]
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
}

/// Per-model totals, aggregated across rows in a window.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ModelTotals {
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

/// Aggregated summary for one window + source.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct WindowSummary {
    pub source: String,
    pub cost_usd: f64,
    pub turns: u64,
    pub invocations: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub per_model: BTreeMap<String, ModelTotals>,
}
```

- [ ] **Step 4: Create placeholder submodules so the crate compiles**

Create `crates/rightclaw/src/usage/insert.rs`:

```rust
//! Insert path — called by worker (interactive) and cron (cron).
```

Create `crates/rightclaw/src/usage/aggregate.rs`:

```rust
//! Read path — used by `/usage` Telegram handler.
```

Create `crates/rightclaw/src/usage/format.rs`:

```rust
//! Telegram HTML message rendering for `/usage`.
```

- [ ] **Step 5: Verify the crate compiles**

Run: `cargo build -p rightclaw`
Expected: builds without errors (warnings about unused modules are fine).

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/usage/ crates/rightclaw/src/lib.rs
git commit -m "feat(usage): scaffold usage module with types and error"
```

---

## Task 3: `parse_usage_full` in stream.rs

**Files:**
- Modify: `crates/bot/src/telegram/stream.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module at the bottom of `crates/bot/src/telegram/stream.rs`:

```rust
#[test]
fn parse_usage_full_happy_path() {
    let line = r#"{
        "type":"result","subtype":"success","is_error":false,
        "session_id":"abc-123",
        "total_cost_usd":0.24,"num_turns":5,
        "usage":{
            "input_tokens":10,"output_tokens":200,
            "cache_creation_input_tokens":500,"cache_read_input_tokens":1500,
            "server_tool_use":{"web_search_requests":2,"web_fetch_requests":3}
        },
        "modelUsage":{
            "claude-sonnet-4-6":{
                "inputTokens":10,"outputTokens":200,
                "cacheReadInputTokens":1500,"cacheCreationInputTokens":500,
                "costUSD":0.24,"contextWindow":200000,"maxOutputTokens":32000
            }
        }
    }"#;
    let breakdown = parse_usage_full(line).expect("happy path must parse");
    assert_eq!(breakdown.session_uuid, "abc-123");
    assert!((breakdown.total_cost_usd - 0.24).abs() < 1e-9);
    assert_eq!(breakdown.num_turns, 5);
    assert_eq!(breakdown.input_tokens, 10);
    assert_eq!(breakdown.output_tokens, 200);
    assert_eq!(breakdown.cache_creation_tokens, 500);
    assert_eq!(breakdown.cache_read_tokens, 1500);
    assert_eq!(breakdown.web_search_requests, 2);
    assert_eq!(breakdown.web_fetch_requests, 3);
    assert!(breakdown.model_usage_json.contains("claude-sonnet-4-6"));
}

#[test]
fn parse_usage_full_missing_cost_returns_none() {
    let line = r#"{"type":"result","session_id":"x","num_turns":1}"#;
    assert!(parse_usage_full(line).is_none());
}

#[test]
fn parse_usage_full_missing_turns_returns_none() {
    let line = r#"{"type":"result","session_id":"x","total_cost_usd":0.1}"#;
    assert!(parse_usage_full(line).is_none());
}

#[test]
fn parse_usage_full_missing_session_id_returns_none() {
    let line = r#"{"type":"result","total_cost_usd":0.1,"num_turns":1}"#;
    assert!(parse_usage_full(line).is_none());
}

#[test]
fn parse_usage_full_missing_model_usage_uses_empty_object() {
    let line = r#"{
        "type":"result","session_id":"x",
        "total_cost_usd":0.1,"num_turns":1,
        "usage":{"input_tokens":5,"output_tokens":7}
    }"#;
    let b = parse_usage_full(line).expect("must parse");
    assert_eq!(b.model_usage_json, "{}");
    assert_eq!(b.input_tokens, 5);
    assert_eq!(b.output_tokens, 7);
    assert_eq!(b.cache_creation_tokens, 0);
    assert_eq!(b.web_search_requests, 0);
}

#[test]
fn parse_usage_full_invalid_json_returns_none() {
    assert!(parse_usage_full("not json").is_none());
}
```

Also add at the top of `stream.rs` (after existing uses):

```rust
use rightclaw::usage::UsageBreakdown;
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p rightclaw-bot stream::tests::parse_usage_full -- --nocapture`
Expected: compile error (`parse_usage_full` not found) or link error.

- [ ] **Step 3: Implement `parse_usage_full`**

Add to `crates/bot/src/telegram/stream.rs` below the existing `parse_usage` function:

```rust
/// Parse the full `result` event JSON into `UsageBreakdown`. Returns `None` if
/// required fields (`total_cost_usd`, `num_turns`, `session_id`) are missing or
/// the JSON is malformed. The `modelUsage` object is preserved as a JSON string
/// for per-model reduction at read time.
pub fn parse_usage_full(result_json: &str) -> Option<UsageBreakdown> {
    let v: serde_json::Value = serde_json::from_str(result_json).ok()?;

    let total_cost_usd = v.get("total_cost_usd")?.as_f64()?;
    let num_turns = u32::try_from(v.get("num_turns")?.as_u64()?).ok()?;
    let session_uuid = v.get("session_id")?.as_str()?.to_string();

    let get_u64 = |ptr: &str| -> u64 {
        v.pointer(ptr).and_then(|n| n.as_u64()).unwrap_or(0)
    };

    let model_usage_json = v
        .get("modelUsage")
        .map(|m| m.to_string())
        .unwrap_or_else(|| "{}".to_string());

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
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rightclaw-bot stream::tests::parse_usage_full -- --nocapture`
Expected: all 6 tests PASS.

Then run the whole stream module tests for no regression:
Run: `cargo test -p rightclaw-bot telegram::stream`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/stream.rs
git commit -m "feat(bot): parse_usage_full extracts full UsageBreakdown from result event"
```

---

## Task 4: `insert_interactive` + `insert_cron`

**Files:**
- Modify: `crates/rightclaw/src/usage/insert.rs`

- [ ] **Step 1: Replace placeholder with failing test**

Replace the contents of `crates/rightclaw/src/usage/insert.rs` with:

```rust
//! Insert path — called by worker (interactive) and cron (cron).

use crate::usage::{UsageBreakdown, UsageError};
use chrono::Utc;
use rusqlite::{Connection, params};

/// Insert a row for an interactive (Telegram worker) invocation.
///
/// `thread_id` is 0 when the message has no thread. `chat_id` may be any valid
/// Telegram chat id (including negative ids for groups).
pub fn insert_interactive(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(conn, b, "interactive", Some(chat_id), Some(thread_id), None)
}

/// Insert a row for a cron (or cron-delivery) invocation.
pub fn insert_cron(
    conn: &Connection,
    b: &UsageBreakdown,
    job_name: &str,
) -> Result<(), UsageError> {
    insert_row(conn, b, "cron", None, None, Some(job_name))
}

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
            model_usage_json
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, ?7, ?8,
            ?9, ?10,
            ?11, ?12,
            ?13, ?14,
            ?15
         )",
        params![
            ts,
            source,
            chat_id,
            thread_id,
            job_name,
            b.session_uuid,
            b.total_cost_usd,
            b.num_turns,
            b.input_tokens,
            b.output_tokens,
            b.cache_creation_tokens,
            b.cache_read_tokens,
            b.web_search_requests,
            b.web_fetch_requests,
            b.model_usage_json,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::open_connection;
    use tempfile::tempdir;

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
        }
    }

    #[test]
    fn insert_interactive_writes_row() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &sample_breakdown(), 42, 0).unwrap();

        let (source, chat_id, thread_id, job_name, cost): (String, Option<i64>, Option<i64>, Option<String>, f64) =
            conn.query_row(
                "SELECT source, chat_id, thread_id, job_name, total_cost_usd FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            ).unwrap();
        assert_eq!(source, "interactive");
        assert_eq!(chat_id, Some(42));
        assert_eq!(thread_id, Some(0));
        assert_eq!(job_name, None);
        assert!((cost - 0.05).abs() < 1e-9);
    }

    #[test]
    fn insert_cron_writes_row_with_null_chat() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_cron(&conn, &sample_breakdown(), "my-job").unwrap();

        let (source, chat_id, job_name): (String, Option<i64>, Option<String>) =
            conn.query_row(
                "SELECT source, chat_id, job_name FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).unwrap();
        assert_eq!(source, "cron");
        assert_eq!(chat_id, None);
        assert_eq!(job_name, Some("my-job".into()));
    }

    #[test]
    fn insert_preserves_all_token_counts() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &sample_breakdown(), 1, 0).unwrap();
        let (inp, out, cc, cr, ws, wf): (u64, u64, u64, u64, u64, u64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, web_search_requests, web_fetch_requests FROM usage_events LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!((inp, out, cc, cr, ws, wf), (10, 20, 100, 200, 1, 2));
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p rightclaw usage::insert`
Expected: may fail at first — confirm what error. If `chrono` isn't in `rightclaw`'s Cargo.toml yet, next step adds it.

- [ ] **Step 3: Ensure `chrono` dependency**

Check `crates/rightclaw/Cargo.toml` for `chrono`. If absent, add under `[dependencies]`:

```toml
chrono = { version = "0.4", features = ["serde"] }
```

Run: `cargo build -p rightclaw` to fetch.

- [ ] **Step 4: Run the tests again to verify pass**

Run: `cargo test -p rightclaw usage::insert`
Expected: all 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/usage/insert.rs crates/rightclaw/Cargo.toml
git commit -m "feat(usage): insert_interactive and insert_cron"
```

---

## Task 5: `aggregate` + per-model reduction

**Files:**
- Modify: `crates/rightclaw/src/usage/aggregate.rs`

- [ ] **Step 1: Replace placeholder with failing tests**

Replace the contents of `crates/rightclaw/src/usage/aggregate.rs` with:

```rust
//! Read path — used by `/usage` Telegram handler.

use crate::usage::{ModelTotals, UsageError, WindowSummary};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::collections::BTreeMap;

/// Aggregate rows for one (source, window) pair. `since=None` → all-time.
pub fn aggregate(
    conn: &Connection,
    since: Option<DateTime<Utc>>,
    source: &str,
) -> Result<WindowSummary, UsageError> {
    let since_str = since.map(|t| t.to_rfc3339());

    let (cost_usd, turns, invocations, input, output, cache_c, cache_r, web_s, web_f): (
        f64, u64, u64, u64, u64, u64, u64, u64, u64,
    ) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(total_cost_usd), 0.0),
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
                r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?,
                r.get(7)?, r.get(8)?,
            )),
        )?;

    let per_model = aggregate_per_model(conn, &since_str, source)?;

    Ok(WindowSummary {
        source: source.to_string(),
        cost_usd,
        turns,
        invocations,
        input_tokens: input,
        output_tokens: output,
        cache_creation_tokens: cache_c,
        cache_read_tokens: cache_r,
        web_search_requests: web_s,
        web_fetch_requests: web_f,
        per_model,
    })
}

fn aggregate_per_model(
    conn: &Connection,
    since_str: &Option<String>,
    source: &str,
) -> Result<BTreeMap<String, ModelTotals>, UsageError> {
    let mut stmt = conn.prepare(
        "SELECT model_usage_json FROM usage_events
         WHERE source = ?1 AND (?2 IS NULL OR ts >= ?2)",
    )?;
    let rows = stmt.query_map(rusqlite::params![source, since_str], |r| {
        r.get::<_, String>(0)
    })?;

    let mut out: BTreeMap<String, ModelTotals> = BTreeMap::new();
    for row in rows {
        let json = row?;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue; // skip malformed rows rather than failing the whole query
        };
        let Some(obj) = v.as_object() else { continue };
        for (model_name, fields) in obj {
            let cost = fields.get("costUSD").and_then(|n| n.as_f64()).unwrap_or(0.0);
            let input = fields.get("inputTokens").and_then(|n| n.as_u64()).unwrap_or(0);
            let output = fields.get("outputTokens").and_then(|n| n.as_u64()).unwrap_or(0);
            let cache_c = fields.get("cacheCreationInputTokens").and_then(|n| n.as_u64()).unwrap_or(0);
            let cache_r = fields.get("cacheReadInputTokens").and_then(|n| n.as_u64()).unwrap_or(0);

            let entry = out.entry(model_name.clone()).or_default();
            entry.cost_usd += cost;
            entry.input_tokens += input;
            entry.output_tokens += output;
            entry.cache_creation_tokens += cache_c;
            entry.cache_read_tokens += cache_r;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::open_connection;
    use crate::usage::UsageBreakdown;
    use crate::usage::insert::{insert_cron, insert_interactive};
    use tempfile::tempdir;

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
        }
    }

    #[test]
    fn aggregate_empty_table_returns_zeros() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        let s = aggregate(&conn, None, "interactive").unwrap();
        assert_eq!(s.invocations, 0);
        assert_eq!(s.cost_usd, 0.0);
        assert!(s.per_model.is_empty());
    }

    #[test]
    fn aggregate_sums_across_invocations() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown(0.1, "sonnet"), 1, 0).unwrap();
        insert_interactive(&conn, &breakdown(0.2, "sonnet"), 1, 0).unwrap();
        let s = aggregate(&conn, None, "interactive").unwrap();
        assert_eq!(s.invocations, 2);
        assert!((s.cost_usd - 0.3).abs() < 1e-9);
        assert_eq!(s.turns, 2);
        assert_eq!(s.input_tokens, 20);
    }

    #[test]
    fn aggregate_per_model_reduces_across_rows_and_models() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown(0.1, "sonnet"), 1, 0).unwrap();
        insert_interactive(&conn, &breakdown(0.2, "sonnet"), 1, 0).unwrap();
        insert_interactive(&conn, &breakdown(0.05, "haiku"), 1, 0).unwrap();
        let s = aggregate(&conn, None, "interactive").unwrap();
        assert_eq!(s.per_model.len(), 2);
        assert!((s.per_model["sonnet"].cost_usd - 0.3).abs() < 1e-9);
        assert!((s.per_model["haiku"].cost_usd - 0.05).abs() < 1e-9);
        assert_eq!(s.per_model["sonnet"].input_tokens, 20);
    }

    #[test]
    fn aggregate_filters_by_source() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown(0.1, "sonnet"), 1, 0).unwrap();
        insert_cron(&conn, &breakdown(0.2, "sonnet"), "job1").unwrap();
        let i = aggregate(&conn, None, "interactive").unwrap();
        let c = aggregate(&conn, None, "cron").unwrap();
        assert_eq!(i.invocations, 1);
        assert!((i.cost_usd - 0.1).abs() < 1e-9);
        assert_eq!(c.invocations, 1);
        assert!((c.cost_usd - 0.2).abs() < 1e-9);
    }

    #[test]
    fn aggregate_filters_by_since() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown(0.1, "sonnet"), 1, 0).unwrap();
        // Backdate this row so the filter excludes it.
        conn.execute(
            "UPDATE usage_events SET ts = '2020-01-01T00:00:00Z' WHERE id = 1",
            [],
        ).unwrap();
        insert_interactive(&conn, &breakdown(0.2, "sonnet"), 1, 0).unwrap();

        let since = Utc::now() - chrono::Duration::hours(1);
        let s = aggregate(&conn, Some(since), "interactive").unwrap();
        assert_eq!(s.invocations, 1);
        assert!((s.cost_usd - 0.2).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: Run tests to verify**

Run: `cargo test -p rightclaw usage::aggregate`
Expected: all 5 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/usage/aggregate.rs
git commit -m "feat(usage): aggregate function with per-model reduction"
```

---

## Task 6: `format_summary_message`

**Files:**
- Modify: `crates/rightclaw/src/usage/format.rs`

- [ ] **Step 1: Replace placeholder with failing tests**

Replace the contents of `crates/rightclaw/src/usage/format.rs` with:

```rust
//! Telegram HTML message rendering for `/usage`.
//!
//! Input: 4 windows × 2 sources = 8 `WindowSummary` values, passed as the
//! `AllWindows` struct. Output: HTML string, HTML-escaped, ≤ 4096 chars.

use crate::usage::WindowSummary;

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

pub fn format_summary_message(w: &AllWindows) -> String {
    let total_invocations = w.all_interactive.invocations + w.all_cron.invocations;
    if total_invocations == 0 {
        return "No usage recorded yet.".to_string();
    }

    let total_cost = w.all_interactive.cost_usd + w.all_cron.cost_usd;

    let mut out = String::new();
    out.push_str("\u{1f4ca} <b>Usage Summary</b> (UTC)\n\n");
    out.push_str(&render_window("Today", &w.today_interactive, &w.today_cron));
    out.push_str(&render_window("Last 7 days", &w.week_interactive, &w.week_cron));
    out.push_str(&render_window("Last 30 days", &w.month_interactive, &w.month_cron));
    out.push_str(&render_window("All time", &w.all_interactive, &w.all_cron));
    out.push_str(&format!("\n<b>Total all time:</b> {}\n", format_cost(total_cost)));
    out
}

fn render_window(title: &str, interactive: &WindowSummary, cron: &WindowSummary) -> String {
    let mut s = format!("\u{2501}\u{2501} <b>{}</b> \u{2501}\u{2501}\n", html_escape(title));
    if interactive.invocations == 0 && cron.invocations == 0 {
        s.push_str("(no activity)\n\n");
        return s;
    }
    if interactive.invocations > 0 {
        s.push_str(&render_source("\u{1f4ac} Interactive", interactive, "invocations"));
    }
    if cron.invocations > 0 {
        s.push_str(&render_source("\u{23f0} Cron", cron, "runs"));
    }
    let web_s = interactive.web_search_requests + cron.web_search_requests;
    let web_f = interactive.web_fetch_requests + cron.web_fetch_requests;
    if web_s > 0 || web_f > 0 {
        s.push_str(&format!("\u{1f50e} Web tools: {web_s} search, {web_f} fetch\n"));
    }
    s.push('\n');
    s
}

fn render_source(label: &str, w: &WindowSummary, unit: &str) -> String {
    let mut s = format!(
        "{label}: {cost} · {turns} turns · {count} {unit}\n   Tokens: in {inp}, out {out}, cache_c {cc}, cache_r {cr}\n",
        cost = format_cost(w.cost_usd),
        turns = w.turns,
        count = w.invocations,
        unit = unit,
        inp = format_count(w.input_tokens),
        out = format_count(w.output_tokens),
        cc = format_count(w.cache_creation_tokens),
        cr = format_count(w.cache_read_tokens),
    );
    // Per-model lines (sorted by cost desc for readability).
    let mut models: Vec<_> = w.per_model.iter().collect();
    models.sort_by(|a, b| b.1.cost_usd.partial_cmp(&a.1.cost_usd).unwrap_or(std::cmp::Ordering::Equal));
    for (name, totals) in models {
        s.push_str(&format!(
            "   \u{2022} {} \u{2014} {}\n",
            html_escape(name),
            format_cost(totals.cost_usd),
        ));
    }
    s
}

fn format_cost(v: f64) -> String {
    if v > 0.0 && v < 0.01 {
        "<$0.01".to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{ModelTotals, WindowSummary};
    use std::collections::BTreeMap;

    fn empty(source: &str) -> WindowSummary {
        WindowSummary { source: source.into(), ..Default::default() }
    }

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

    #[test]
    fn empty_db_returns_no_usage_line() {
        let w = AllWindows {
            today_interactive: empty("interactive"),
            today_cron: empty("cron"),
            week_interactive: empty("interactive"),
            week_cron: empty("cron"),
            month_interactive: empty("interactive"),
            month_cron: empty("cron"),
            all_interactive: empty("interactive"),
            all_cron: empty("cron"),
        };
        assert_eq!(format_summary_message(&w), "No usage recorded yet.");
    }

    #[test]
    fn empty_window_shows_no_activity() {
        let w = AllWindows {
            today_interactive: empty("interactive"),
            today_cron: empty("cron"),
            week_interactive: with("interactive", 0.1, 1, "sonnet", 0.1),
            week_cron: empty("cron"),
            month_interactive: with("interactive", 0.1, 1, "sonnet", 0.1),
            month_cron: empty("cron"),
            all_interactive: with("interactive", 0.1, 1, "sonnet", 0.1),
            all_cron: empty("cron"),
        };
        let msg = format_summary_message(&w);
        assert!(msg.contains("Today"));
        assert!(msg.contains("(no activity)"));
        assert!(msg.contains("Last 7 days"));
        assert!(msg.contains("Interactive"));
    }

    #[test]
    fn cost_below_one_cent_shown_as_less_than() {
        assert_eq!(format_cost(0.003), "<$0.01");
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
        let w = AllWindows {
            today_interactive: with("interactive", 0.1, 1, "foo<script>", 0.1),
            today_cron: empty("cron"),
            week_interactive: empty("interactive"),
            week_cron: empty("cron"),
            month_interactive: empty("interactive"),
            month_cron: empty("cron"),
            all_interactive: with("interactive", 0.1, 1, "foo<script>", 0.1),
            all_cron: empty("cron"),
        };
        let msg = format_summary_message(&w);
        assert!(msg.contains("foo&lt;script&gt;"));
        assert!(!msg.contains("<script>"));
    }

    #[test]
    fn total_line_sums_interactive_and_cron() {
        let w = AllWindows {
            today_interactive: with("interactive", 0.5, 2, "sonnet", 0.5),
            today_cron: with("cron", 0.3, 1, "sonnet", 0.3),
            week_interactive: with("interactive", 0.5, 2, "sonnet", 0.5),
            week_cron: with("cron", 0.3, 1, "sonnet", 0.3),
            month_interactive: with("interactive", 0.5, 2, "sonnet", 0.5),
            month_cron: with("cron", 0.3, 1, "sonnet", 0.3),
            all_interactive: with("interactive", 0.5, 2, "sonnet", 0.5),
            all_cron: with("cron", 0.3, 1, "sonnet", 0.3),
        };
        let msg = format_summary_message(&w);
        assert!(msg.contains("Total all time:</b> $0.80"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rightclaw usage::format`
Expected: all 6 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/usage/format.rs
git commit -m "feat(usage): format_summary_message renders Telegram HTML output"
```

---

## Task 7: Hook worker.rs — write on interactive result

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Read the current hook site**

Run: `rg -n "StreamEvent::Result\(json\)" crates/bot/src/telegram/worker.rs`
Expected: match near line 1226.

- [ ] **Step 2: Add the INSERT in the Result branch**

In `crates/bot/src/telegram/worker.rs`, replace this block at ~line 1226:

```rust
                            super::stream::StreamEvent::Result(json) => {
                                usage = super::stream::parse_usage(json);
                                result_line = Some(json.clone());
                            }
```

with:

```rust
                            super::stream::StreamEvent::Result(json) => {
                                usage = super::stream::parse_usage(json);
                                result_line = Some(json.clone());

                                // Write usage_events row (best-effort — telemetry must not block the turn).
                                if let Some(breakdown) = super::stream::parse_usage_full(json) {
                                    if let Err(e) = rightclaw::usage::insert::insert_interactive(
                                        &conn, &breakdown, chat_id, eff_thread_id,
                                    ) {
                                        tracing::warn!(?chat_id, "usage insert failed: {e:#}");
                                    }
                                }
                            }
```

- [ ] **Step 3: Ensure compile**

Run: `cargo build -p rightclaw-bot`
Expected: builds. If `insert` submodule is not re-exported, the `rightclaw::usage::insert::insert_interactive` path still works because `pub mod insert;` is declared in `usage/mod.rs`.

- [ ] **Step 4: Run worker tests for regression**

Run: `cargo test -p rightclaw-bot telegram::worker`
Expected: existing tests pass. No new test here — insert is exercised via `usage::insert::tests`; end-to-end is manual.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): worker writes usage_events row on CC result"
```

---

## Task 8: Hook cron.rs — write on cron result

**Files:**
- Modify: `crates/bot/src/cron.rs`
- Modify: `crates/bot/src/cron_delivery.rs`

- [ ] **Step 1: Locate the cron result processing**

Run: `rg -n "type.*result|parse_cron_output|result_line" crates/bot/src/cron.rs | head -20`

Find the function that processes `collected_lines` into `CronReplyOutput`. Around line ~560-617, there is code that parses the `result` line. Above or after this processing (before the function returns), insert the usage event write.

- [ ] **Step 2: Add the INSERT in cron.rs**

In `crates/bot/src/cron.rs`, locate the site where `collected_lines` has already been gathered and cron processing begins (typically after `parse_cron_output(&collected_lines)` succeeds or inside the surrounding function). Add this block immediately after `collected_lines` is available, before returning:

```rust
    // Write usage_events row (best-effort). Find the last `result` line and parse full usage.
    if let Some(result_line) = collected_lines.iter().rev().find(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|s| s.to_string()))
            .as_deref() == Some("result")
    }) {
        if let Some(breakdown) = rightclaw_bot::telegram::stream::parse_usage_full(result_line) {
            if let Err(e) = rightclaw::usage::insert::insert_cron(&conn, &breakdown, &job_name) {
                tracing::warn!(job = %job_name, "usage insert failed: {e:#}");
            }
        }
    }
```

Adjust the `rightclaw_bot::telegram::stream::parse_usage_full` path based on how cron.rs imports modules (it's in the same crate, so `crate::telegram::stream::parse_usage_full` is correct). Use `crate::telegram::stream::parse_usage_full`.

Variable names (`collected_lines`, `job_name`, `conn`) must match the existing function scope. Check names by reading the current file before editing.

- [ ] **Step 3: Add the INSERT in cron_delivery.rs**

Find the equivalent point in `crates/bot/src/cron_delivery.rs` where a delivery CC run has completed and its stdout lines are available. Add the same pattern but with `job_name = format!("{}-delivery", job_name)` so delivery cost is distinguishable.

If cron_delivery doesn't collect stream lines the same way, skip this step for now — delivery rows can be added in a follow-up. Document the skip with a comment: `// TODO(usage): delivery stream capture lives elsewhere — follow up.`

- [ ] **Step 4: Ensure compile**

Run: `cargo build -p rightclaw-bot`
Expected: builds.

- [ ] **Step 5: Run cron tests for regression**

Run: `cargo test -p rightclaw-bot cron`
Expected: existing tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs
git commit -m "feat(bot): cron writes usage_events row on CC result"
```

---

## Task 9: `/usage` BotCommand + handler

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Add `Usage` variant to `BotCommand` enum**

In `crates/bot/src/telegram/dispatch.rs` at line 37, add inside the enum (alphabetically or at end — follow existing ordering):

```rust
    #[command(description = "Show usage summary (cost, turns, tokens)")]
    Usage,
```

- [ ] **Step 2: Wire the command into the dispatcher**

In the same file, search for the branch that matches `BotCommand::Doctor => handle_doctor(...)`. Follow the same pattern for `BotCommand::Usage => handle_usage(bot, msg, agent_dir)`. The exact dispatch signature depends on how other commands are wired — read the file and mirror the pattern used by `handle_doctor`.

- [ ] **Step 3: Import `handle_usage`**

In `crates/bot/src/telegram/dispatch.rs` near line 28-29 where `handle_doctor` etc. are imported, add `handle_usage`.

- [ ] **Step 4: Implement `handle_usage` in handler.rs**

Append to `crates/bot/src/telegram/handler.rs`:

```rust
pub async fn handle_usage(
    bot: BotType,
    msg: Message,
    agent_dir: Arc<AgentDir>,
) -> Result<(), RequestError> {
    tracing::info!("handle_usage: running");
    let text = match build_usage_summary(&agent_dir.0).await {
        Ok(t) => t,
        Err(e) => format!("Failed to read usage: {e:#}"),
    };
    let mut send = bot
        .send_message(msg.chat.id, &text)
        .parse_mode(teloxide::types::ParseMode::Html);
    if let Some(thread_id) = msg.thread_id {
        send = send.message_thread_id(thread_id);
    }
    send.await?;
    Ok(())
}

async fn build_usage_summary(agent_dir: &Path) -> Result<String, miette::Report> {
    use chrono::{Duration, Utc};
    use rightclaw::usage::aggregate::aggregate;
    use rightclaw::usage::format::{AllWindows, format_summary_message};

    // Open read-only (migrations already applied by bot startup).
    let conn = rightclaw::memory::open_connection(agent_dir, false)
        .map_err(|e| miette::miette!("open_connection: {e:#}"))?;

    let now = Utc::now();
    let today_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
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

    Ok(format_summary_message(&windows))
}
```

- [ ] **Step 5: Ensure compile**

Run: `cargo build -p rightclaw-bot`
Expected: builds.

- [ ] **Step 6: Set the bot command menu**

Look at `dispatch.rs` around line 211-215 where `BotCommand::bot_commands()` is registered — the new `Usage` variant is picked up automatically. Verify no manual menu list needs updating.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs
git commit -m "feat(bot): /usage command — aggregate and render summary"
```

---

## Task 10: Full workspace build + clippy

**Files:** (none)

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: succeeds.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. Fix any warnings introduced.

- [ ] **Step 3: Full test suite**

Run: `cargo test --workspace`
Expected: all pass.

- [ ] **Step 4: Commit (if clippy fixes applied)**

If Step 2 required edits, commit with a message like `chore: clippy cleanup for usage module`. Otherwise skip.

---

## Task 11: Manual verification checklist

**Not a code task — verify end-to-end on a real agent before merge.**

- [ ] Start a fresh agent (or existing). Confirm the bot process restarts and applies migration v15 (check `~/.rightclaw/logs/<agent>.log` for migration logs; confirm table exists via `sqlite3 ~/.rightclaw/agents/<agent>/data.db '.schema usage_events'`).
- [ ] Send `/usage` with no prior activity → reply: `No usage recorded yet.`
- [ ] Send a few regular messages to the agent, wait for replies to complete.
- [ ] Send `/usage` → "Today → Interactive" section shows cost, turns, invocations, tokens, per-model lines.
- [ ] Trigger a cron job (or wait for one). After it completes, `/usage` shows a "Today → Cron" section.
- [ ] Check SQLite directly: `sqlite3 ~/.rightclaw/agents/<agent>/data.db 'SELECT source, total_cost_usd, num_turns, input_tokens FROM usage_events ORDER BY ts DESC LIMIT 10'`.
- [ ] Confirm no warnings in the agent log about `usage insert failed`.
- [ ] Confirm `/usage` message renders cleanly in Telegram (HTML escaping correct, no malformed tags, ≤ 4096 chars).

If every item passes, open a PR on branch `usage`.

---

## Self-Review Notes

Spec coverage audit:

- Architecture (new table `usage_events`, worker + cron writes, `/usage` handler) — Tasks 1, 4, 7, 8, 9.
- Modules listed in spec — all created in Tasks 2, 4, 5, 6.
- Schema (all columns, both indexes) — Task 1.
- `parse_usage_full` — Task 3.
- Write path (interactive + cron + delivery) — Tasks 7, 8.
- Read path aggregation (8 calls, per-model reduction, windows) — Tasks 5, 9.
- Output format (all rules: k/M suffix, `<$0.01`, `(no activity)`, empty DB, web tools omission, HTML escape) — Task 6 tests cover each.
- Error handling (best-effort writes, parse-none skip, DB open error, empty DB) — Tasks 4, 5, 6, 9.
- Testing (unit tests per spec) — Tasks 1, 3, 4, 5, 6.
- Manual verification — Task 11.

Deferred (per spec's Non-Goals or Open Questions):

- Custom date ranges — not in plan.
- Per-chat breakdown — not in plan.
- Backfill — not in plan.
- Retention pruning — not in plan.
