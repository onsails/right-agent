# Learning: Sandbox Skill Index + Skill-Spend Observability — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the per-turn skill-learning pipeline correct and observable: the prefilter sees skills that actually exist (in the sandbox), probe-writer cost/cache is recorded, per-skill spend (learn/fix/usage) and budget-blocked attempts are surfaced in the dashboard.

**Architecture:** A new `skill_spend` ledger and `learning_skip` table (migration v38) capture attribution separately from raw `usage_events`. The probe-writer's stdout drain is fixed to record usage and write create/patch spend (joined to the skill via the existing invocation-id linkage). The prefilter reads its skill index from the sandbox via gRPC `exec_in_sandbox`. The dashboard Knowledge and Usage views render the new data.

**Tech Stack:** Rust (edition 2024), `right-db` (turso), tokio, tonic/gRPC (`right-openshell`), Vue 3 + Vitest SSR (`right-dashboard` frontend).

**Spec:** `docs/superpowers/specs/2026-05-31-learning-skill-index-and-spend-observability-design.md`

**Verification cadence:** Targeted `-p <crate>` tests after each task (TDD red→green). One mandatory `devenv shell -- cargo test --workspace` at the end (Task 12). All `cargo`/`sqlite3` commands are prefixed `devenv shell --`.

**Canonical signatures (defined in Task 2, reused throughout — keep identical):**
```rust
// crates/right-agent/src/usage/insert.rs
pub async fn insert_skill_spend(
    conn: &Connection,
    skill_name: &str,
    kind: &str,                  // "create" | "patch" | "maintain" | "usage"
    cost_usd: f64,
    cache_read: i64,
    cache_creation: i64,
    invocation_id: Option<&str>,
) -> Result<(), UsageError>;

pub async fn insert_learning_skip(
    conn: &Connection,
    reason: &str,                // "budget"
    intended_kind: Option<&str>, // None now
    chat_id: Option<i64>,
    thread_id: Option<i64>,
) -> Result<(), UsageError>;
```

---

## Task 1: Migration v38 — `skill_spend` + `learning_skip` tables

**Files:**
- Create: `crates/right-db/src/sql/v38_skill_spend_and_learning_skip.sql`
- Modify: `crates/right-db/src/migrations.rs` (const, registry entry, `LATEST_SCHEMA_VERSION`)
- Test: `crates/right-db/src/migrations.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the SQL file**

Create `crates/right-db/src/sql/v38_skill_spend_and_learning_skip.sql`:
```sql
CREATE TABLE IF NOT EXISTS skill_spend (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  skill_name     TEXT NOT NULL,
  kind           TEXT NOT NULL CHECK (kind IN ('create','patch','maintain','usage')),
  cost_usd       REAL NOT NULL DEFAULT 0.0,
  cache_read     INTEGER NOT NULL DEFAULT 0,
  cache_creation INTEGER NOT NULL DEFAULT 0,
  invocation_id  TEXT,
  ts             TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_skill_spend_skill_kind ON skill_spend(skill_name, kind);
CREATE INDEX IF NOT EXISTS idx_skill_spend_ts ON skill_spend(ts);

CREATE TABLE IF NOT EXISTS learning_skip (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  reason        TEXT NOT NULL,
  intended_kind TEXT,
  chat_id       INTEGER,
  thread_id     INTEGER,
  ts            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_learning_skip_reason_ts ON learning_skip(reason, ts);
```

- [ ] **Step 2: Register the migration**

In `crates/right-db/src/migrations.rs`: add the const near the other `VNN_SCHEMA` consts (after `V36_SCHEMA`):
```rust
const V38_SCHEMA: &str = include_str!("sql/v38_skill_spend_and_learning_skip.sql");
```
Change `LATEST_SCHEMA_VERSION` from `37` to `38`:
```rust
pub const LATEST_SCHEMA_VERSION: u32 = 38;
```
Add the entry as the LAST element of the `MIGRATIONS.migrations` array (after the `version: 37` entry):
```rust
        Migration {
            version: 38,
            sql: V38_SCHEMA,
            hook: None,
        },
```

- [ ] **Step 3: Write the idempotency test**

Add to the `#[cfg(test)] mod tests` in `migrations.rs` (mirror `v37_deletes_legacy_learning_usage_sources`):
```rust
    #[tokio::test]
    async fn v38_creates_skill_spend_and_learning_skip_idempotently() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 37).await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        for table in ["skill_spend", "learning_skip"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .await
                .unwrap();
            assert_eq!(n, 1, "{table} must exist after v38");
        }

        // Insert + read back one row of each to confirm columns/CHECK.
        conn.execute(
            "INSERT INTO skill_spend (skill_name, kind, cost_usd, cache_read, cache_creation, invocation_id) \
             VALUES ('rightx-x','create',0.5,10,20,'inv1')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO learning_skip (reason, intended_kind, chat_id, thread_id) \
             VALUES ('budget', NULL, 7, 0)",
            (),
        )
        .await
        .unwrap();

        // Idempotent: re-running to_latest is a no-op and does not error.
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
    }
```

- [ ] **Step 4: Run the test (red→green in one shot; CREATE is the impl)**

Run: `devenv shell -- cargo test -p right-db v38_creates_skill_spend_and_learning_skip_idempotently -- --nocapture`
Expected: PASS. Also run the guard test that keeps version/registry in sync:
Run: `devenv shell -- cargo test -p right-db migration_runner_semantics_latest_schema_version_matches_highest_migration`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/right-db/src/sql/v38_skill_spend_and_learning_skip.sql crates/right-db/src/migrations.rs
git commit -m "feat(right-db): v38 skill_spend + learning_skip tables"
```

---

## Task 2: Insert helpers `insert_skill_spend` + `insert_learning_skip`

**Files:**
- Modify: `crates/right-agent/src/usage/insert.rs`
- Test: `crates/right-agent/src/usage/insert.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write failing tests**

Add to the tests module in `insert.rs` (mirror `insert_learning_curator_writes_row_with_null_chat`; reuse its in-memory DB setup helper — find it in the same module, e.g. a `fn test_conn()` / `open_in_memory` + migrations):
```rust
    #[tokio::test]
    async fn insert_skill_spend_writes_row() {
        let conn = test_conn().await; // same helper the other insert tests use
        insert_skill_spend(&conn, "rightx-foo", "create", 0.25, 100, 200, Some("inv-1"))
            .await
            .unwrap();
        let (name, kind, cost, cr, cc, inv): (String, String, f64, i64, i64, Option<String>) = conn
            .query_row(
                "SELECT skill_name, kind, cost_usd, cache_read, cache_creation, invocation_id \
                 FROM skill_spend LIMIT 1",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .await
            .unwrap();
        assert_eq!((name.as_str(), kind.as_str(), cost, cr, cc, inv.as_deref()),
                   ("rightx-foo", "create", 0.25, 100, 200, Some("inv-1")));
    }

    #[tokio::test]
    async fn insert_learning_skip_writes_budget_row_with_null_kind() {
        let conn = test_conn().await;
        insert_learning_skip(&conn, "budget", None, Some(42), Some(0)).await.unwrap();
        let (reason, kind, chat): (String, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT reason, intended_kind, chat_id FROM learning_skip LIMIT 1",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!((reason.as_str(), kind, chat), ("budget", None, Some(42)));
    }
```
> If the existing tests use an inline `open_connection`/`open_in_memory` rather than a `test_conn()` helper, copy that exact setup into these two tests instead.

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-agent insert_skill_spend_writes_row insert_learning_skip_writes`
Expected: FAIL (function not found).

- [ ] **Step 3: Implement the helpers**

Add to `insert.rs` (these write to the NEW tables directly — do NOT route through the private `insert_row`, which targets `usage_events`):
```rust
/// Insert one per-skill spend ledger row (create/patch/maintain/usage).
pub async fn insert_skill_spend(
    conn: &Connection,
    skill_name: &str,
    kind: &str,
    cost_usd: f64,
    cache_read: i64,
    cache_creation: i64,
    invocation_id: Option<&str>,
) -> Result<(), UsageError> {
    let ts = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO skill_spend \
         (skill_name, kind, cost_usd, cache_read, cache_creation, invocation_id, ts) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![skill_name, kind, cost_usd, cache_read, cache_creation, invocation_id, ts],
    )
    .await?;
    Ok(())
}

/// Record one learning attempt suppressed before it could run (e.g. budget).
pub async fn insert_learning_skip(
    conn: &Connection,
    reason: &str,
    intended_kind: Option<&str>,
    chat_id: Option<i64>,
    thread_id: Option<i64>,
) -> Result<(), UsageError> {
    let ts = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO learning_skip (reason, intended_kind, chat_id, thread_id, ts) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![reason, intended_kind, chat_id, thread_id, ts],
    )
    .await?;
    Ok(())
}
```
> `Connection`, `UsageError`, `params!`, and `Utc` are already imported at the top of `insert.rs` (used by `insert_row`). Confirm with the existing imports; add none new.

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-agent insert_skill_spend_writes_row insert_learning_skip_writes`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/right-agent/src/usage/insert.rs
git commit -m "feat(right-agent): insert_skill_spend + insert_learning_skip helpers"
```

---

## Task 3: Extend `StreamUsage` with cache token fields

**Why:** `kind='usage'` rows (Task 5) need per-turn cache, but `StreamUsage` (carried to the receipts point) only has `num_turns` + `cost_usd`.

**Files:**
- Modify: `crates/bot/src/cc/stream.rs` (struct + `parse_usage`)
- Modify: construction sites (test/helpers): `crates/bot/src/cc/stream.rs:780,812`; `crates/bot/src/telegram/worker.rs:4617,4633,4689`
- Test: `crates/bot/src/cc/stream.rs`

- [ ] **Step 1: Write failing test**

Add to `stream.rs` tests:
```rust
    #[test]
    fn parse_usage_captures_cache_tokens() {
        let json = r#"{"type":"result","num_turns":2,"total_cost_usd":0.1,
            "usage":{"cache_creation_input_tokens":30,"cache_read_input_tokens":40}}"#;
        let u = parse_usage(json);
        assert_eq!(u.cache_creation_tokens, 30);
        assert_eq!(u.cache_read_tokens, 40);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo test -p right-bot parse_usage_captures_cache_tokens`
Expected: FAIL (no field `cache_creation_tokens` on `StreamUsage`).

- [ ] **Step 3: Extend the struct and `parse_usage`**

In `crates/bot/src/cc/stream.rs`, change the struct (around line 44):
```rust
pub(crate) struct StreamUsage {
    pub num_turns: u32,
    pub cost_usd: f64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}
```
In `parse_usage` (around line 255-262), populate the two new fields the same way `parse_usage_full` does (it already computes these at lines 292-293 via a `get_u64("/usage/...")` accessor — reuse the same path):
```rust
    StreamUsage {
        num_turns: /* existing */,
        cost_usd: /* existing */,
        cache_creation_tokens: get_u64("/usage/cache_creation_input_tokens"),
        cache_read_tokens: get_u64("/usage/cache_read_input_tokens"),
    }
```
> Use whatever local accessor `parse_usage` has for reading `num_turns`/`cost_usd`. If `parse_usage` lacks a `get_u64` closure like `parse_usage_full`, add one identically (a closure over the parsed `serde_json::Value` returning `0` on absence).

- [ ] **Step 4: Fix the 5 construction sites (compile errors)**

Add `cache_creation_tokens: 0, cache_read_tokens: 0,` to each `StreamUsage { ... }` literal at: `stream.rs:780`, `stream.rs:812`, `worker.rs:4617`, `worker.rs:4633`, `worker.rs:4689`. (These are test/helper constructions; zeros are fine.)

- [ ] **Step 5: Run tests**

Run: `devenv shell -- cargo test -p right-bot parse_usage_captures_cache_tokens && devenv shell -- cargo test -p right-bot --lib cc::stream`
Expected: PASS.

- [ ] **Step 6: Commit**
```bash
git add crates/bot/src/cc/stream.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): carry cache tokens in StreamUsage"
```

---

## Task 4: Fix probe-writer stdout drain + record usage + skill_spend create/patch

**Why:** `wait_for_system_init` consumes & drops the stdout reader after init; the later `wait_with_output_or_kill` gets empty `output.stdout`, so usage is never parsed. Post-init stdout is also abandoned (pipe-fill hang risk). Fix: one reader drains the whole stream, capturing the final `result` line; usage + create/patch spend are written from it.

**Files:**
- Modify: `crates/bot/src/cc/invocation.rs` (expose `invocation_id()` for production)
- Modify: `crates/bot/src/learning_probe_writer.rs` (drain restructure + spend write + helpers)
- Modify: `crates/right-agent/src/learned_skills.rs` (finish-row lookup)
- Test: `crates/bot/src/learning_probe_writer.rs`

- [ ] **Step 1: Expose the invocation id (production accessor)**

In `crates/bot/src/cc/invocation.rs`, the `impl RegisteredNonForegroundInvocation` has (around line 153):
```rust
    #[cfg(test)]
    pub(crate) fn invocation_id(&self) -> &str {
        &self.invocation_id
    }
```
Remove the `#[cfg(test)]` attribute so production code can read it:
```rust
    pub(crate) fn invocation_id(&self) -> &str {
        &self.invocation_id
    }
```

- [ ] **Step 2: Write failing unit tests for the pure helpers**

Add to `learning_probe_writer.rs` tests:
```rust
    #[test]
    fn finish_status_to_spend_kind_maps_created_and_updated() {
        assert_eq!(finish_status_to_spend_kind("created"), Some("create"));
        assert_eq!(finish_status_to_spend_kind("updated"), Some("patch"));
        assert_eq!(finish_status_to_spend_kind("aborted"), None);
        assert_eq!(finish_status_to_spend_kind("failed"), None);
    }

    #[test]
    fn last_result_line_picks_final_result_event() {
        let stream = "\
{\"type\":\"system\",\"subtype\":\"init\"}\n\
{\"type\":\"assistant\"}\n\
{\"type\":\"result\",\"num_turns\":3,\"total_cost_usd\":0.2,\"session_id\":\"s\"}\n";
        let line = last_result_line(stream).unwrap();
        assert!(line.contains("\"type\":\"result\""));
        assert!(line.contains("\"total_cost_usd\":0.2"));
    }
```

- [ ] **Step 3: Run to verify failure**

Run: `devenv shell -- cargo test -p right-bot finish_status_to_spend_kind last_result_line_picks`
Expected: FAIL (helpers not defined).

- [ ] **Step 4: Add the pure helpers**

Add to `learning_probe_writer.rs`:
```rust
/// Map a `skill_learning_events.status` (finish phase) to a `skill_spend.kind`.
/// Only successful create/update are spend-worthy; aborted/failed are not.
fn finish_status_to_spend_kind(status: &str) -> Option<&'static str> {
    match status {
        "created" => Some("create"),
        "updated" => Some("patch"),
        _ => None,
    }
}

/// Return the last `{"type":"result",...}` line from a stream-json stdout dump.
fn last_result_line(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .filter(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|t| t == "result"))
                .unwrap_or(false)
        })
        .next_back()
        .map(ToOwned::to_owned)
}
```

- [ ] **Step 5: Add a DB read for the finish row**

Add a helper to `crates/right-agent/src/learned_skills.rs` (same module that owns `insert_learning_event`), exported:
```rust
/// The skill_name + finish status for an invocation, if it wrote a finish event.
pub async fn finish_event_for_invocation(
    conn: &right_db::Connection,
    invocation_id: &str,
) -> Result<Option<(String, String)>, right_db::DbError> {
    conn.query_opt(
        "SELECT skill_name, status FROM skill_learning_events \
         WHERE invocation_id = ?1 AND phase = 'finish' AND status IS NOT NULL \
         ORDER BY created_at DESC LIMIT 1",
        [invocation_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .await
}
```
> Use the project's optional-row API. If `query_opt` does not exist, use `query_row` and map `DbError::NotFound` → `Ok(None)`, or `.optional()` as in `right-mcp/src/credentials.rs::get_auth_token`. Confirm the exact helper in `crates/right-db/src/connection.rs`.

- [ ] **Step 6: Restructure the drain in `run()`**

Replace lines ~204-271 of `learning_probe_writer.rs` with the body below. The new flow keeps ONE `Lines` reader: read until init (bounded, under mutex), then detach and drain the SAME reader to EOF while awaiting the child, capturing the last result line.
```rust
    let stdout = match child.stdout() {
        Some(s) => s,
        None => {
            tracing::warn!(agent = %ctx.agent_name, "probe-writer child has no stdout");
            let _ = child.kill().await;
            drop(child);
            drop(_guard);
            active_invocation.cleanup().await;
            return;
        }
    };
    let mut lines = tokio::io::BufReader::new(stdout).lines();

    // Phase 1: read until system/init (bounded), holding the session mutex.
    let init_observed = tokio::time::timeout(
        PROBE_WRITER_INIT_TIMEOUT,
        read_until_init(&mut lines, &probe_session_id),
    )
    .await
    .unwrap_or(false);
    drop(_guard);

    if !init_observed {
        tracing::warn!(agent = %ctx.agent_name, "probe-writer never emitted system/init, killing");
        let _ = child.kill().await;
        drop(child);
        active_invocation.cleanup().await;
        return;
    }

    // Phase 2: detached — drain the REST of the same reader to EOF (prevents
    // pipe-fill hang) while awaiting the process, capturing the final result.
    let agent_name = ctx.agent_name.clone();
    let agent_db_dir = ctx.agent_db_dir.clone();
    let chat_id = ctx.chat_id;
    let thread_id = ctx.thread_id;
    let invocation_id = active_invocation.invocation_id().to_owned();
    tokio::spawn(async move {
        let mut tail = String::new();
        let drain = async {
            while let Ok(Some(line)) = lines.next_line().await {
                if line.len() < 1_000_000 {
                    tail.push_str(&line);
                    tail.push('\n');
                }
            }
        };
        // Drain + wait for exit, bounded by PROBE_WRITER_TIMEOUT.
        let completed = tokio::time::timeout(PROBE_WRITER_TIMEOUT, async {
            tokio::join!(drain, child.wait())
        })
        .await;
        match completed {
            Ok((_, Ok(status))) => {
                if !status.success() {
                    tracing::warn!(agent = %agent_name, ?status, "probe-writer exited non-zero");
                }
            }
            Ok((_, Err(e))) => {
                tracing::warn!(agent = %agent_name, "probe-writer wait failed: {e:#}");
            }
            Err(_) => {
                tracing::warn!(
                    agent = %agent_name,
                    "probe-writer timed out after {}s", PROBE_WRITER_TIMEOUT.as_secs()
                );
                let _ = child.kill().await;
            }
        }

        // Record usage + per-skill create/patch spend from the captured result.
        if let Some(result_line) = last_result_line(&tail)
            && let Some(b) = crate::cc::stream::parse_usage_full(&result_line)
            && let Ok(conn) = right_db::open_connection(&agent_db_dir, false).await
        {
            if let Err(e) = right_agent::usage::insert::insert_learning_probe_writer(
                &conn, &b, chat_id, thread_id,
            )
            .await
            {
                tracing::warn!(agent = %agent_name, "probe-writer usage insert failed: {e:#}");
            }
            record_probe_writer_spend(&conn, &agent_name, &invocation_id, &b).await;
        }

        active_invocation.cleanup().await;
    });
}

/// Read lines until a `system/init` for `expected_session_id` is seen. Returns
/// `true` on init, `false` on EOF. Does NOT consume the reader (borrows it).
async fn read_until_init<R: tokio::io::AsyncRead + Unpin>(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<R>>,
    expected_session_id: &str,
) -> bool {
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) == Some("system")
            && v.get("subtype").and_then(|s| s.as_str()) == Some("init")
            && v.get("session_id").and_then(|s| s.as_str()) == Some(expected_session_id)
        {
            return true;
        }
    }
    false
}

/// Look up the skill created/patched in this invocation and write a skill_spend
/// row. No finish row (aborted/failed/timeout) → no spend row.
async fn record_probe_writer_spend(
    conn: &right_db::Connection,
    agent_name: &str,
    invocation_id: &str,
    b: &right_agent::usage::UsageBreakdown,
) {
    match right_agent::learned_skills::finish_event_for_invocation(conn, invocation_id).await {
        Ok(Some((skill_name, status))) => {
            if let Some(kind) = finish_status_to_spend_kind(&status) {
                if let Err(e) = right_agent::usage::insert::insert_skill_spend(
                    conn,
                    &skill_name,
                    kind,
                    b.total_cost_usd,
                    b.cache_read_tokens as i64,
                    b.cache_creation_tokens as i64,
                    Some(invocation_id),
                )
                .await
                {
                    tracing::warn!(agent = %agent_name, "probe-writer skill_spend insert failed: {e:#}");
                }
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(agent = %agent_name, "probe-writer finish lookup failed: {e:#}"),
    }
}
```
Then DELETE the now-unused `wait_for_system_init` / `wait_for_system_init_unbounded` functions (lines ~274-305) — `read_until_init` replaces them. If `wait_with_output_or_kill` is no longer used anywhere (check `crates/bot/src/` for other callers first), leave it in place; do not delete shared helpers other modules use.
> `child.wait()` — confirm `right_process::ProcessGroupChild` exposes `async fn wait(&mut self) -> io::Result<ExitStatus>`. If it only exposes `wait_with_output`, use `tokio::join!(drain, async { child.wait_with_output().await })` and read `output.status` (ignore its empty `stdout`).

- [ ] **Step 7: Run targeted tests + compile check**

Run: `devenv shell -- cargo test -p right-bot finish_status_to_spend_kind last_result_line_picks && devenv shell -- cargo test -p right-bot --lib learning_probe_writer`
Expected: PASS.
Run: `devenv shell -- cargo check -p right-bot`
Expected: clean (no unused-import/function warnings from the deleted code).

- [ ] **Step 8: Commit**
```bash
git add crates/bot/src/learning_probe_writer.rs crates/bot/src/cc/invocation.rs crates/right-agent/src/learned_skills.rs
git commit -m "fix(bot): drain probe-writer stdout fully; record usage + create/patch spend"
```

---

## Task 5: `kind='usage'` spend per used skill (worker)

**Why:** Attribute each turn's cost/cache to the rightx skills it used. Hook: `record_used_skill_receipts` already runs per turn for rightx receipts and opens a conn.

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (`record_used_skill_receipts` signature + body; call site ~1571)
- Test: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write failing test**

Add to `worker.rs` tests (open a tempdir `data.db` via `right_db::open_connection(dir, true)` and run `MIGRATIONS.to_latest`):
```rust
    #[tokio::test]
    async fn record_used_skill_receipts_writes_usage_spend_per_rightx() {
        let dir = tempfile::tempdir().unwrap();
        { let mut c = right_db::open_connection(dir.path(), true).await.unwrap();
          right_db::migrations::MIGRATIONS.to_latest(&mut c).await.unwrap(); }
        let receipts = vec![
            UsedSkillReceipt { package_name: "rightx-a".into(), message: None },
            UsedSkillReceipt { package_name: "core-skill".into(), message: None }, // ignored
        ];
        record_used_skill_receipts(dir.path(), &receipts, chrono::Utc::now(), 0.30, 10, 20)
            .await
            .unwrap();
        let conn = right_db::open_connection(dir.path(), false).await.unwrap();
        let (n, name, cost): (i64, String, f64) = conn
            .query_row(
                "SELECT COUNT(*), MAX(skill_name), MAX(cost_usd) FROM skill_spend WHERE kind='usage'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!((n, name.as_str(), cost), (1, "rightx-a", 0.30));
    }
```
> Match `UsedSkillReceipt`'s real fields (see `crates/bot/src/cc/worker_reply.rs`); adjust the literal if it has more fields.

- [ ] **Step 2: Run to verify failure**

Run: `devenv shell -- cargo test -p right-bot record_used_skill_receipts_writes_usage_spend_per_rightx`
Expected: FAIL (arity mismatch).

- [ ] **Step 3: Extend `record_used_skill_receipts`**

Change the signature and body (currently `worker.rs:370-388`):
```rust
    async fn record_used_skill_receipts(
        agent_db_dir: &Path,
        receipts: &[UsedSkillReceipt],
        now_utc: DateTime<Utc>,
        turn_cost_usd: f64,
        turn_cache_read: u64,
        turn_cache_creation: u64,
    ) -> anyhow::Result<std::collections::BTreeSet<String>> {
        let used_skill_names = used_skill_names_from_receipts(receipts);
        if used_skill_names.is_empty() {
            return Ok(used_skill_names);
        }

        let conn = right_db::open_connection(agent_db_dir, false)
            .await
            .context("open lifecycle database")?;
        right_lifecycle::bump_use_many(&conn, &used_skill_names, now_utc)
            .await
            .context("bump lifecycle usage")?;

        // Attribute this turn's cost/cache to each used rightx skill (kind='usage').
        // Overlaps across skills when a turn used several — intentional; the
        // dashboard labels it attributed, not exact. Failure here is non-fatal.
        for name in &used_skill_names {
            if let Err(e) = right_agent::usage::insert::insert_skill_spend(
                &conn, name, "usage",
                turn_cost_usd, turn_cache_read as i64, turn_cache_creation as i64, None,
            )
            .await
            {
                tracing::warn!(skill = %name, "usage skill_spend insert failed: {e:#}");
            }
        }
        Ok(used_skill_names)
    }
```

- [ ] **Step 4: Update the call site (~1571)**

At the `record_used_skill_receipts(&ctx.agent_dir, receipts, chrono::Utc::now())` call, pass the turn usage (in scope as `cc_usage: StreamUsage`):
```rust
                            Some(receipts) => match record_used_skill_receipts(
                                &ctx.agent_dir,
                                receipts,
                                chrono::Utc::now(),
                                cc_usage.cost_usd,
                                cc_usage.cache_read_tokens,
                                cc_usage.cache_creation_tokens,
                            )
                            .await
```

- [ ] **Step 5: Run tests**

Run: `devenv shell -- cargo test -p right-bot record_used_skill_receipts_writes_usage_spend_per_rightx`
Expected: PASS.

- [ ] **Step 6: Commit**
```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): attribute per-turn cost/cache to used skills (skill_spend usage)"
```

---

## Task 6: Budget-gate writes a `learning_skip` row

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (the gate at ~2071)
- Test: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write failing test for a small extracted helper**

To make the gate testable without spawning CC, extract the skip write into a helper and test it:
```rust
    #[tokio::test]
    async fn budget_skip_records_learning_skip_row() {
        let dir = tempfile::tempdir().unwrap();
        { let mut c = right_db::open_connection(dir.path(), true).await.unwrap();
          right_db::migrations::MIGRATIONS.to_latest(&mut c).await.unwrap(); }
        let conn = right_db::open_connection(dir.path(), false).await.unwrap();
        record_budget_skip(&conn, "agent-x", 99, 0).await;
        let (n, reason, kind): (i64, String, Option<String>) = conn
            .query_row(
                "SELECT COUNT(*), MAX(reason), MAX(intended_kind) FROM learning_skip",
                (), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).await.unwrap();
        assert_eq!((n, reason.as_str(), kind), (1, "budget", None));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `devenv shell -- cargo test -p right-bot budget_skip_records_learning_skip_row`
Expected: FAIL (`record_budget_skip` not defined).

- [ ] **Step 3: Add the helper + wire the gate**

Add the helper near the learning block in `worker.rs`:
```rust
/// Record a budget-blocked learning attempt. Best-effort; logs and swallows.
async fn record_budget_skip(conn: &right_db::Connection, agent_name: &str, chat_id: i64, thread_id: i64) {
    if let Err(e) = right_agent::usage::insert::insert_learning_skip(
        conn, "budget", None, Some(chat_id), Some(thread_id),
    )
    .await
    {
        tracing::warn!(agent = %agent_name, "learning_skip insert failed: {e:#}");
    }
}
```
Replace the gate body (currently `worker.rs:2071-2079`) — `conn` is already open at that point (opened at ~2053):
```rust
                    if today_spend >= daily_budget {
                        tracing::debug!(
                            agent = %agent_name, spend = today_spend, budget = daily_budget,
                            "learning pipeline skipped: daily budget exhausted"
                        );
                        record_budget_skip(&conn, &agent_name, anchor.chat_id, anchor.thread_id).await;
                        return;
                    }
```

- [ ] **Step 4: Run tests**

Run: `devenv shell -- cargo test -p right-bot budget_skip_records_learning_skip_row`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): record learning_skip row on budget-exhausted gate"
```

---

## Task 7: Curator `maintain` spend

**Files:**
- Modify: `crates/bot/src/learning_curator.rs`
- Test: `crates/bot/src/learning_curator.rs` (or its `*_tests.rs`)

- [ ] **Step 1: Inspect the curator usage path**

Run: `devenv shell -- rg -n "insert_learning_curator|parse_usage|invocation|skill_name|for .* in" crates/bot/src/learning_curator.rs`
Find where the curator parses its run usage and which skill(s) it acted on. The curator may act on multiple skills per pass.

- [ ] **Step 2: Decide attribution (per spec)**

If a curator pass attributes work to specific skill(s), write one `skill_spend(kind='maintain')` row per skill it actually mutated, attributing the run cost as the curator already tracks per-skill work. If a pass cannot attribute to a single skill, write NO `skill_spend` row (the run cost stays in `usage_events` via `insert_learning_curator`). Do not invent numbers.

- [ ] **Step 3: Write a failing test**

Add a test that, given a curator pass that archives/absorbs skill `rightx-z` with a known run cost, a `skill_spend` row `(rightx-z, 'maintain')` exists. Model it on the existing curator lifecycle tests in `learning_curator_curator_lifecycle_tests.rs` (reuse their fixture/DB setup verbatim). Example assertion:
```rust
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skill_spend WHERE kind='maintain' AND skill_name='rightx-z'",
        (), |r| r.get(0)).await.unwrap();
    assert_eq!(n, 1);
```

- [ ] **Step 4: Run to verify failure**

Run: `devenv shell -- cargo test -p right-bot curator -- maintain`
Expected: FAIL.

- [ ] **Step 5: Implement**

At the point the curator records its usage (where it calls `insert_learning_curator`), also call `insert_skill_spend(&conn, &skill_name, "maintain", cost, cache_read as i64, cache_creation as i64, invocation_id.as_deref())` for each skill the pass mutated, using the cost/cache from the curator's parsed `UsageBreakdown`. If the pass has no single skill, skip.

- [ ] **Step 6: Run tests**

Run: `devenv shell -- cargo test -p right-bot curator`
Expected: PASS.

- [ ] **Step 7: Commit**
```bash
git add crates/bot/src/learning_curator.rs crates/bot/src/learning_curator_curator_lifecycle_tests.rs
git commit -m "feat(bot): record curator maintain spend per skill"
```

---

## Task 8: Prefilter + probe-writer skill index from the sandbox (gRPC)

**Why:** `collect_host_rightx_skill_index` reads the host, where learned skills never land. Read the sandbox (source of truth) instead, at both call sites. gRPC only — no ssh fallback.

**Files:**
- Modify: `crates/bot/src/learning_prefilter.rs` (new sandbox reader + shared dispatch; `run` uses it)
- Modify: `crates/bot/src/telegram/worker.rs` (probe-writer index call site ~2131 uses the shared reader)
- Test: `crates/bot/src/learning_prefilter.rs` (unit: parse) + an in-crate `#[ignore]` live test

- [ ] **Step 1: Write a failing unit test for the sandbox-output parser**

The sandbox dump emits delimited frontmatter; the Rust side parses it into `LearnedSkillSummary { name, excerpt }` (existing struct in `learning_prefilter.rs`). Add:
```rust
    #[test]
    fn parse_sandbox_skill_dump_extracts_name_and_excerpt() {
        let dump = "\n@@@SKILL rightx-foo\n---\nname: rightx-foo\ndescription: does foo\n---\n# body\n\
                    \n@@@SKILL rightx-bar\n---\nname: rightx-bar\ndescription: does bar\n---\n";
        let got = parse_sandbox_skill_dump(dump);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "rightx-foo");
        assert!(got[0].excerpt.contains("does foo"));
        assert_eq!(got[1].name, "rightx-bar");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `devenv shell -- cargo test -p right-bot parse_sandbox_skill_dump_extracts_name_and_excerpt`
Expected: FAIL.

- [ ] **Step 3: Implement the parser + the sandbox reader + shared dispatch**

In `learning_prefilter.rs`:
```rust
/// Shell command run inside the sandbox to dump rightx skill frontmatter.
/// The `[ -f ... ]` guard makes a no-match glob emit nothing (no error).
const SANDBOX_SKILL_DUMP_CMD: &str =
    "for d in /sandbox/.claude/skills/rightx-*/; do \
       [ -f \"$d/SKILL.md\" ] || continue; \
       printf '\\n@@@SKILL %s\\n' \"$(basename \"$d\")\"; \
       head -c 4096 \"$d/SKILL.md\"; \
     done";

/// Parse the delimited dump into per-skill summaries (reuses bounding helpers).
fn parse_sandbox_skill_dump(dump: &str) -> Vec<LearnedSkillSummary> {
    let mut out = Vec::new();
    for chunk in dump.split("\n@@@SKILL ") {
        let chunk = chunk.trim_start_matches("@@@SKILL ");
        let Some((name_line, body)) = chunk.split_once('\n') else { continue };
        let name = name_line.trim();
        if !is_rightx_skill(name) {
            continue;
        }
        out.push(LearnedSkillSummary {
            name: name.to_owned(),
            excerpt: bounded_skill_excerpt(body), // existing helper
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Read the rightx skill index from inside the sandbox via gRPC.
async fn collect_sandbox_rightx_skill_index(
    sandbox_name: &str,
) -> anyhow::Result<Vec<LearnedSkillSummary>> {
    use anyhow::Context as _;
    let mtls_dir = match right_openshell::openshell::preflight_check() {
        right_openshell::openshell::OpenShellStatus::Ready(dir) => dir,
        status => anyhow::bail!("openshell preflight not ready: {status:?}"),
    };
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .map_err(|e| anyhow::anyhow!("connect_grpc: {e:#}"))?;
    let sandbox_id = right_openshell::openshell::resolve_sandbox_id(&mut client, sandbox_name)
        .await
        .map_err(|e| anyhow::anyhow!("resolve_sandbox_id: {e:#}"))?;
    let (out, _exit) = right_openshell::openshell::exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["sh", "-lc", SANDBOX_SKILL_DUMP_CMD],
        right_openshell::openshell::DEFAULT_EXEC_TIMEOUT_SECS,
    )
    .await
    .map_err(|e| anyhow::anyhow!("exec_in_sandbox: {e:#}"))
    .context("read sandbox skill index")?;
    Ok(parse_sandbox_skill_dump(&out))
}

/// Shared skill-index reader: sandbox via gRPC, or host for `mode: none`.
pub(crate) async fn collect_rightx_skill_index(
    resolved_sandbox: Option<&str>,
    agent_dir: &Path,
) -> anyhow::Result<Vec<LearnedSkillSummary>> {
    match resolved_sandbox {
        Some(name) => collect_sandbox_rightx_skill_index(name).await,
        None => Ok(collect_host_rightx_skill_index(agent_dir)?),
    }
}
```
> Confirm `bounded_skill_excerpt` and `is_rightx_skill` are in scope in `learning_prefilter.rs` (the file already uses both). Add `use std::path::Path;` if not present.

- [ ] **Step 4: Use the shared reader in `learning_prefilter::run`**

Replace the body's `collect_host_rightx_skill_index(&ctx.agent_dir)` call (~line 505) with:
```rust
    let skills = match collect_rightx_skill_index(ctx.resolved_sandbox.as_deref(), &ctx.agent_dir).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "skill index failed: {e:#}");
            return PrefilterDecision::Skip { reason: "skill index failed".into() };
        }
    };
```
> Per the spec FAIL-FAST rule: a read error returns `Skip`, NOT an empty index (empty would re-trigger duplicate creation).

- [ ] **Step 5: Use the shared reader at the probe-writer call site**

In `worker.rs` (~2131), replace `collect_host_rightx_skill_index(&agent_dir)` with the async shared reader. `resolved` (`Option<String>`) is in scope:
```rust
                    let skill_index = match crate::learning_prefilter::collect_rightx_skill_index(
                        resolved.as_deref(), &agent_dir,
                    )
                    .await
                    {
                        Ok(entries) => entries
                            .into_iter()
                            .map(|s| format!("- {}: {}", s.name, summary_first_line(&s.excerpt)))
                            .collect::<Vec<_>>()
                            .join("\n"),
                        Err(e) => {
                            tracing::warn!(agent = %agent_name, "skill index failed: {e:#}");
                            String::new()
                        }
                    };
```
> Here an empty index is acceptable (the probe-writer still has the prefilter's decision hint); only the PREFILTER must hard-Skip on read failure.

- [ ] **Step 6: Add a live integration test (ci_openshell)**

Add an in-crate test (so it can call the `pub(crate)` reader directly). Per `ARCHITECTURE.md` → "Integration Tests Using Live Sandboxes" + the ci-ignore contract, use `#[ignore = "ci-openshell: ..."]` and a `ci_openshell_` name prefix, and `right_openshell::test_support::TestSandbox` (enable the crate's `test-support` dev-dependency feature if not already):
```rust
    #[tokio::test]
    #[ignore = "ci-openshell: creates a live sandbox, writes a SKILL.md, reads the index"]
    async fn ci_openshell_sandbox_skill_index_reads_rightx() {
        let sb = right_openshell::test_support::TestSandbox::create("skill-index").await;
        let (_o, code) = sb.exec(&["sh", "-lc",
            "mkdir -p /sandbox/.claude/skills/rightx-demo && \
             printf '---\\nname: rightx-demo\\ndescription: demo skill\\n---\\n' \
             > /sandbox/.claude/skills/rightx-demo/SKILL.md"]).await;
        assert_eq!(code, 0);
        let idx = collect_sandbox_rightx_skill_index(sb.name()).await.unwrap();
        assert!(idx.iter().any(|s| s.name == "rightx-demo" && s.excerpt.contains("demo skill")));
    }
```

- [ ] **Step 7: Run tests**

Run: `devenv shell -- cargo test -p right-bot parse_sandbox_skill_dump_extracts_name_and_excerpt`
Expected: PASS.
Run (live): `devenv shell -- cargo test -p right-bot ci_openshell_sandbox_skill_index_reads_rightx -- --ignored`
Expected: PASS (requires local OpenShell). Record the result.

- [ ] **Step 8: Commit**
```bash
git add crates/bot/src/learning_prefilter.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): read learned-skill index from the sandbox via gRPC"
```

---

## Task 9: Dashboard read models + api_types (per-skill spend + budget-skip count)

**Files:**
- Modify: `crates/right-dashboard/src/read_model/learning.rs` (or the skills read model)
- Modify: `crates/right-dashboard/src/read_model/usage.rs`
- Modify: `crates/right-dashboard/src/api_types.rs`
- Test: same files (`#[cfg(test)]`)

- [ ] **Step 1: Locate the skills list read model**

Run: `devenv shell -- rg -n "SkillsResponse|SkillSummary|apply_lifecycle" crates/right-dashboard/src`
Identify the function that builds the `Vec<SkillSummary>` (groups core/learned/other) and the `SkillSummary::apply_lifecycle` merge hook (`api_types.rs:417`).

- [ ] **Step 2: Extend `SkillSummary` (api_types.rs)**

Add fields (after `last_patched_at`):
```rust
    #[serde(default)]
    pub learn_cost_usd: f64,
    #[serde(default)]
    pub fix_cost_usd: f64,
    #[serde(default)]
    pub usage_cost_usd: f64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_creation_tokens: i64,
```
Add an `apply_spend` method next to `apply_lifecycle`, plus the aggregate struct:
```rust
impl SkillSummary {
    pub fn apply_spend(&mut self, s: &SkillSpendAgg) {
        self.learn_cost_usd = s.learn_cost_usd;
        self.fix_cost_usd = s.fix_cost_usd;
        self.usage_cost_usd = s.usage_cost_usd;
        self.cache_read_tokens = s.cache_read_tokens;
        self.cache_creation_tokens = s.cache_creation_tokens;
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillSpendAgg {
    pub learn_cost_usd: f64,
    pub fix_cost_usd: f64,
    pub usage_cost_usd: f64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
}
```

- [ ] **Step 3: Write a failing read-model test for the spend aggregate**

Add to the read-model file (mirror existing read-model tests' DB setup — insert into `skill_spend`, then call the aggregate):
```rust
    #[tokio::test]
    async fn skill_spend_agg_buckets_by_kind() {
        let conn = test_conn().await; // existing dashboard read-model test helper
        for (k, c) in [("create", 0.5), ("patch", 0.1), ("patch", 0.2), ("usage", 0.9)] {
            conn.execute(
                "INSERT INTO skill_spend (skill_name, kind, cost_usd, cache_read, cache_creation) \
                 VALUES ('rightx-a', ?1, ?2, 5, 7)",
                right_db::params![k, c],
            ).await.unwrap();
        }
        let map = skill_spend_by_skill(&conn).await.unwrap();
        let a = map.get("rightx-a").unwrap();
        assert!((a.learn_cost_usd - 0.5).abs() < 1e-9);
        assert!((a.fix_cost_usd - 0.3).abs() < 1e-9);   // patch summed
        assert!((a.usage_cost_usd - 0.9).abs() < 1e-9);
        assert_eq!(a.cache_read_tokens, 20);            // 4 rows * 5
    }
```

- [ ] **Step 4: Run to verify failure**

Run: `devenv shell -- cargo test -p right-dashboard skill_spend_agg_buckets_by_kind`
Expected: FAIL.

- [ ] **Step 5: Implement the aggregate + wire into the skills read model**

Add to the read-model file:
```rust
pub(crate) async fn skill_spend_by_skill(
    conn: &right_db::Connection,
) -> Result<std::collections::HashMap<String, crate::api_types::SkillSpendAgg>, ReadModelError> {
    let rows = conn
        .query_map(
            "SELECT skill_name, \
               COALESCE(SUM(CASE WHEN kind='create' THEN cost_usd END),0), \
               COALESCE(SUM(CASE WHEN kind IN ('patch','maintain') THEN cost_usd END),0), \
               COALESCE(SUM(CASE WHEN kind='usage' THEN cost_usd END),0), \
               COALESCE(SUM(cache_read),0), COALESCE(SUM(cache_creation),0) \
             FROM skill_spend GROUP BY skill_name",
            (),
            |r| Ok((
                r.get::<_, String>(0)?,
                crate::api_types::SkillSpendAgg {
                    learn_cost_usd: r.get(1)?, fix_cost_usd: r.get(2)?, usage_cost_usd: r.get(3)?,
                    cache_read_tokens: r.get(4)?, cache_creation_tokens: r.get(5)?,
                },
            )),
        )
        .await?;
    Ok(rows.into_iter().collect())
}
```
> Use the project's row-collecting API (`query_map`/`query_all` — check `right-db/src/connection.rs`; match what other dashboard read models use). In the function that builds the skills list, after `apply_lifecycle`, fetch the map once and `if let Some(agg) = map.get(&summary.name) { summary.apply_spend(agg); }`.

- [ ] **Step 6: Usage tab — budget-skip count**

Add to `api_types.rs` `UsageWindow`:
```rust
    #[serde(default)]
    pub budget_skip_count: i64,
```
In `usage.rs`, add a count query over `learning_skip` for the window and set it on the window:
```rust
async fn budget_skip_count(conn: &Connection, since: &str, until: &str) -> Result<i64, ReadModelError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM learning_skip WHERE reason='budget' AND ts >= ?1 AND ts <= ?2",
        [since, until], |r| r.get(0)).await?)
}
```
Call it in `build_window` and set `budget_skip_count`. Add a test that inserts two `learning_skip` rows in-window and asserts the window count is 2.

- [ ] **Step 7: Run tests**

Run: `devenv shell -- cargo test -p right-dashboard skill_spend_agg_buckets_by_kind && devenv shell -- cargo test -p right-dashboard usage`
Expected: PASS (including `usage_overview_sources_match_learning_sources_constant`, unchanged).

- [ ] **Step 8: Commit**
```bash
git add crates/right-dashboard/src/read_model/learning.rs crates/right-dashboard/src/read_model/usage.rs crates/right-dashboard/src/api_types.rs
git commit -m "feat(dashboard): per-skill spend aggregate + budget-skip count read models"
```

---

## Task 10: Dashboard frontend — Knowledge spend + Usage cache/skip columns

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/SkillsView.vue` (detail panel meta-grid)
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.vue` (per-source cache cols + skip count)
- Test: `*.test.ts` (SSR via `@vue/server-renderer`)

- [ ] **Step 1: Write failing SSR test for SkillsView spend rows**

Create/extend `crates/right-dashboard/frontend/src/views/SkillsView.test.ts` (mirror `components/AsyncState.test.ts`'s `createSSRApp` + `renderToString` pattern). Render with a selected skill carrying `learn_cost_usd: 0.5` and assert the rendered HTML contains the formatted value and a "Learn" label.

- [ ] **Step 2: Run to verify failure**

Run: `devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && npx vitest run SkillsView'`
Expected: FAIL.

- [ ] **Step 3: Add the meta-grid rows (SkillsView.vue)**

In the detail panel `<dl class="meta-grid compact">`, add (after the existing Uses/Patches rows):
```html
<div><dt>Learn</dt><dd>{{ money(selected.learn_cost_usd) }}</dd></div>
<div><dt>Fix</dt><dd>{{ money(selected.fix_cost_usd) }}</dd></div>
<div><dt>Usage</dt><dd>{{ money(selected.usage_cost_usd) }}</dd></div>
<div><dt>Cache r/w</dt><dd>{{ selected.cache_read_tokens }} / {{ selected.cache_creation_tokens }}</dd></div>
```
> Use the same `money()` formatter UsageView uses; import or duplicate the tiny helper from the shared util.

- [ ] **Step 4: Usage view — cache columns + skip count**

In `UsageView.vue`, in the per-window `v-for="source in windowRows(window)"` row, add cache cells:
```html
<span class="cell">{{ source.cache_read_tokens }}</span>
<span class="cell">{{ source.cache_creation_tokens }}</span>
```
And near the window header add a budget-skip line:
```html
<p v-if="window.budget_skip_count > 0" class="muted">
  Budget-blocked learning attempts: {{ window.budget_skip_count }}
</p>
```
Add an SSR test asserting the skip line renders when `budget_skip_count > 0` and is absent when `0`.

- [ ] **Step 5: Run tests**

Run: `devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && npx vitest run SkillsView UsageView'`
Expected: PASS. Then check whether the bundle is embedded at build time:
Run: `devenv shell -- rg -n "build.rs|include_dir|frontend/dist" crates/right-dashboard`
If a rebuild step is needed, run the project's frontend build (e.g. `npm run build` in `frontend/`).

- [ ] **Step 6: Commit**
```bash
git add crates/right-dashboard/frontend/src/views/SkillsView.vue crates/right-dashboard/frontend/src/views/UsageView.vue crates/right-dashboard/frontend/src/views/*.test.ts
git commit -m "feat(dashboard): show per-skill spend + usage cache/skip columns"
```

---

## Task 11: Docs

**Files:**
- Modify: `docs/architecture/learning.md`
- Modify: `ARCHITECTURE.md` (only if a new invariant must be stated; keep under 40k)
- Modify: `PROMPT_SYSTEM.md` (only if agent-facing prompt/schema changed — it did NOT here; confirm and skip)

- [ ] **Step 1: Update `docs/architecture/learning.md`**

Add: (a) the prefilter/probe-writer skill index is read from `/sandbox/.claude/skills/rightx-*` via gRPC `exec_in_sandbox`; host read is `mode: none` only. (b) `skill_spend` ledger (kinds create/patch/maintain/usage) attributes cost/cache per skill, separate from `usage_events`. (c) `learning_skip(reason='budget')` counts budget-blocked attempts; single $1 gate; `intended_kind` always NULL. (d) probe-writer stdout is drained in one pass; usage + create/patch spend recorded via the invocation-id → `skill_learning_events` finish-row join.

- [ ] **Step 2: Update `ARCHITECTURE.md` Skill-learning section (one line each)**

Under "### Skill learning", add the skill-index-from-sandbox rule and the `skill_spend`/`learning_skip` tables as one-line facts pointing to `docs/architecture/learning.md`. Verify the file stays under 40k chars:
Run: `devenv shell -- wc -c ARCHITECTURE.md`
Expected: < 40000.

- [ ] **Step 3: Commit**
```bash
git add docs/architecture/learning.md ARCHITECTURE.md
git commit -m "docs(learning): sandbox skill index, skill_spend ledger, learning_skip"
```

---

## Task 12: Final verification

- [ ] **Step 1: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. (Live `ci_openshell_*` tests stay `#[ignore]` and are not run here; Task 8 Step 7 already exercised the sandbox path locally.)

- [ ] **Step 2: Full debug build**

Run: `devenv shell -- cargo build --workspace`
Expected: PASS.

- [ ] **Step 3: Frontend tests (if not covered by workspace)**

Run: `devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && npx vitest run'`
Expected: PASS.

- [ ] **Step 4: Record results**

Note any pre-existing failures unrelated to this work (e.g. `activity_overview_returns_current_cron_payload` was dirty in a prior session) vs. failures this work introduced. Only the latter block completion.

---

## Self-Review Checklist (run before handing off)

- **Spec coverage:** Part 1 → Task 8; Part 2 → Task 4; Part 3 (skill_spend) → Tasks 1,2,4,5,7; Part 4 (Knowledge) → Tasks 9,10; Part 5 (Usage) → Tasks 9,10; Part 6 (budget skip) → Tasks 1,2,6,9,10. Migration v38 → Task 1. ✔
- **Type consistency:** `insert_skill_spend` / `insert_learning_skip` signatures identical in Task 2 (def) and Tasks 4/5/6/7 (use). `SkillSpendAgg` fields match between api_types (Task 9 Step 2) and the aggregate query (Task 9 Step 5). `StreamUsage` cache fields (Task 3) consumed in Task 5. ✔
- **Placeholder scan:** the "locate via rg" steps (Task 7 Step 1, Task 9 Step 1, Task 10 Step 5) are concrete discovery steps, each followed by exact edits — not deferred work. ✔
