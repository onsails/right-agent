# Background Async Runs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move background continuations out of cron scheduling, introduce a unified `async_runs` execution history, and prevent the race where the next foreground message enters the main session before the background fork.

**Architecture:** `cron_specs` remains the schedule-definition table. New `async_runs` becomes the only execution/result/delivery table for cron and background. Background handoff is owned by the Telegram worker: it gates the thread, cancels foreground, starts the background fork immediately, waits for a confirmed fork handoff signal, then releases the thread.

**Tech Stack:** Rust 2024, SQLite via `rusqlite`, `rusqlite_migration`, Tokio tasks, Teloxide, Claude Code stream-json, existing `devenv shell -- cargo ...` workflow.

---

## Scope Check

The design touches four tightly coupled surfaces: DB schema, cron run history API, delivery, and Telegram background handoff. They are not independent enough to split into separate specs because the invariant depends on one unified runtime path: no `cron_runs` fallback and no cron-backed background execution.

Do not start implementation without first loading the repository-required Rust skill if it is available in the session:

```text
rust-dev:rust-dev
```

If that skill is unavailable, record that fact in the implementation notes before editing Rust and continue only under the user's current repository instruction fallback.

## File Structure

- Create `crates/right-db/src/sql/v22_async_runs.sql`: schema for `async_runs` and indexes.
- Modify `crates/right-db/src/migrations.rs`: add v22 copy/drop migration and migration tests.
- Modify `crates/right-db/tests/smoke.rs`: replace `cron_runs` smoke assertions with `async_runs`.
- Create `crates/right-agent/src/async_runs.rs`: shared SQL helpers and row-to-JSON conversion used by bot and MCP servers.
- Modify `crates/right-agent/src/lib.rs`: export `async_runs`.
- Modify `crates/right-agent/src/cron_spec.rs` and `crates/right-agent/src/cron_spec_tests.rs`: remove background continuation as a cron schedule kind, update cron run history lookups to `async_runs`.
- Modify `crates/bot/src/cron.rs`: write cron execution records to `async_runs`.
- Rename `crates/bot/src/cron_delivery.rs` to `crates/bot/src/async_delivery.rs`: delivery reads `async_runs` and supports cron/background instruction variants.
- Modify `crates/bot/src/lib.rs`: import and start `async_delivery`.
- Create `crates/bot/src/background.rs`: immediate background continuation executor and handoff result logic.
- Modify `crates/bot/src/telegram/mod.rs`: add `BgHandoffGates`.
- Modify `crates/bot/src/telegram/handler.rs`: Background callback sets the handoff gate before cancelling.
- Modify `crates/bot/src/telegram/dispatch.rs`: pass `BgHandoffGates` into handlers and workers.
- Modify `crates/bot/src/telegram/worker.rs`: replace cron enqueue with immediate background executor, wait on gates before foreground work, update background marker queries to `async_runs`.
- Modify `crates/right/src/memory_server.rs`, `crates/right/src/right_backend.rs`, and related tests: cron MCP reads from `async_runs WHERE kind='cron'`.
- Update `docs/architecture/sessions.md`, `docs/architecture/lifecycle.md`, `ARCHITECTURE.md` if load-bearing rules change, and `PROMPT_SYSTEM.md` if agent-facing text/schema changes.

## Task 0: Baseline

**Files:**
- Read: `docs/superpowers/specs/2026-05-18-background-async-runs-design.md`
- Read: `AGENTS.md`
- No code changes.

- [ ] **Step 1: Confirm clean starting point**

Run:

```bash
devenv shell -- git status --short
```

Expected: no output, or only user-owned unrelated changes that are recorded before continuing.

- [ ] **Step 2: Run targeted baseline checks**

Run:

```bash
devenv shell -- cargo test -p right-db
devenv shell -- cargo test -p right-agent cron_spec
devenv shell -- cargo test -p right memory_server_mcp_tests
devenv shell -- cargo test -p bot cron_delivery
devenv shell -- cargo test -p bot background_continuation_tests
```

Expected: all pass. If any fail, record the exact failing test names and do not mix unrelated fixes into this branch.

## Task 1: Add `async_runs` Schema and Drop `cron_runs`

**Files:**
- Create: `crates/right-db/src/sql/v22_async_runs.sql`
- Modify: `crates/right-db/src/migrations.rs`
- Modify: `crates/right-db/tests/smoke.rs`

- [ ] **Step 1: Write failing migration tests**

Add these tests to `crates/right-db/src/migrations.rs` inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn v22_creates_async_runs_and_drops_cron_runs() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let async_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='async_runs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(async_count, 1, "async_runs table should exist");

    let cron_runs_exists = conn.prepare("SELECT 1 FROM cron_runs LIMIT 1").is_ok();
    assert!(!cron_runs_exists, "cron_runs must be dropped after v22");
}

#[test]
fn v22_migrates_cron_runs_to_async_runs() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 21).unwrap();

    conn.execute(
        "INSERT INTO cron_runs (
            id, job_name, started_at, finished_at, exit_code, status, log_path,
            summary, notify_json, delivered_at, delivery_status, no_notify_reason,
            target_chat_id, target_thread_id
         ) VALUES (
            'run-1', 'morning', '2026-05-18T01:00:00Z', '2026-05-18T01:01:00Z',
            0, 'success', '/log/run-1.ndjson', 'summary', '{\"content\":\"hi\"}',
            NULL, 'pending', NULL, -100, 7
         )",
        [],
    )
    .unwrap();

    MIGRATIONS.to_latest(&mut conn).unwrap();

    let row: (String, String, String, Option<String>, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT kind, producer_ref, delivery_status, notify_json, target_chat_id, target_thread_id
             FROM async_runs WHERE id = 'run-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(row.0, "cron");
    assert_eq!(row.1, "morning");
    assert_eq!(row.2, "pending");
    assert_eq!(row.3.as_deref(), Some("{\"content\":\"hi\"}"));
    assert_eq!(row.4, Some(-100));
    assert_eq!(row.5, Some(7));
}

#[test]
fn v22_maps_silent_delivery_status_to_none() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 21).unwrap();

    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
         VALUES ('silent-1', 'quiet', '2026-05-18T02:00:00Z', 'success', '/log', 'silent')",
        [],
    )
    .unwrap();

    MIGRATIONS.to_latest(&mut conn).unwrap();

    let delivery: (i64, String) = conn
        .query_row(
            "SELECT delivery_required, delivery_status FROM async_runs WHERE id = 'silent-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(delivery, (0, "none".to_string()));
}
```

Update `crates/right-db/tests/smoke.rs` by replacing the existing `schema_has_cron_runs_table` and `cron_runs_insert_and_update` tests with:

```rust
#[test]
fn schema_has_async_runs_table_and_no_cron_runs_table() {
    let dir = tempdir().expect("tempdir");
    let conn = right_db::open_connection(dir.path(), true).expect("open db");

    let async_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='async_runs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(async_count, 1, "async_runs table should exist");

    let cron_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='cron_runs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cron_count, 0, "cron_runs table should be removed");
}

#[test]
fn async_runs_insert_and_update() {
    let dir = tempdir().expect("tempdir");
    let conn = right_db::open_connection(dir.path(), true).expect("open db");

    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id, status,
            delivery_required, delivery_status, created_at, updated_at
         ) VALUES (
            'run-1', 'cron', 'deploy-check', 'run-1', -100, 'running',
            0, 'none', '2026-04-01T00:00:00Z', '2026-04-01T00:00:00Z'
         )",
        [],
    )
    .unwrap();

    conn.execute(
        "UPDATE async_runs
         SET finished_at='2026-04-01T00:01:00Z', exit_code=0, status='success'
         WHERE id='run-1'",
        [],
    )
    .unwrap();

    let row: (Option<String>, Option<i64>, String) = conn
        .query_row(
            "SELECT finished_at, exit_code, status FROM async_runs WHERE id='run-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row.0.as_deref(), Some("2026-04-01T00:01:00Z"));
    assert_eq!(row.1, Some(0));
    assert_eq!(row.2, "success");
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-db v22_
devenv shell -- cargo test -p right-db schema_has_async_runs_table_and_no_cron_runs_table
```

Expected: failures mentioning missing `async_runs` table or still-present `cron_runs`.

- [ ] **Step 3: Add v22 migration**

Create `crates/right-db/src/sql/v22_async_runs.sql`:

```sql
CREATE TABLE IF NOT EXISTS async_runs (
    id                  TEXT PRIMARY KEY,
    kind                TEXT NOT NULL,
    producer_ref         TEXT,
    source_session_id    TEXT,
    run_session_id       TEXT NOT NULL,
    target_chat_id       INTEGER NOT NULL,
    target_thread_id     INTEGER,
    status              TEXT NOT NULL,
    handoff_state        TEXT,
    started_at           TEXT,
    finished_at          TEXT,
    exit_code            INTEGER,
    log_path             TEXT,
    summary              TEXT,
    notify_json          TEXT,
    no_notify_reason     TEXT,
    error_json           TEXT,
    delivery_required    INTEGER NOT NULL,
    delivery_status      TEXT NOT NULL,
    delivery_attempts    INTEGER NOT NULL DEFAULT 0,
    delivered_at         TEXT,
    last_delivery_error  TEXT,
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_async_runs_kind_producer_started
    ON async_runs(kind, producer_ref, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_async_runs_delivery
    ON async_runs(delivery_required, delivery_status, status, finished_at);

CREATE INDEX IF NOT EXISTS idx_async_runs_target_status
    ON async_runs(target_chat_id, target_thread_id, status);

CREATE INDEX IF NOT EXISTS idx_async_runs_run_session
    ON async_runs(run_session_id);
```

In `crates/right-db/src/migrations.rs`, add:

```rust
const V22_SCHEMA: &str = include_str!("sql/v22_async_runs.sql");
```

Add a hook after the v18 hook:

```rust
fn v22_async_runs(tx: &Transaction) -> Result<(), HookError> {
    tx.execute_batch(V22_SCHEMA)?;
    tx.execute_batch(
        "INSERT INTO async_runs (
            id, kind, producer_ref, source_session_id, run_session_id,
            target_chat_id, target_thread_id, status, handoff_state,
            started_at, finished_at, exit_code, log_path, summary,
            notify_json, no_notify_reason, error_json, delivery_required,
            delivery_status, delivery_attempts, delivered_at,
            last_delivery_error, created_at, updated_at
         )
         SELECT
            cr.id,
            CASE WHEN cr.job_name LIKE 'bg-%' THEN 'background' ELSE 'cron' END,
            cr.job_name,
            CASE
              WHEN cs.schedule LIKE '@bg:%' THEN substr(cs.schedule, 5)
              ELSE NULL
            END,
            cr.id,
            COALESCE(cr.target_chat_id, cs.target_chat_id, 0),
            COALESCE(cr.target_thread_id, cs.target_thread_id),
            cr.status,
            CASE WHEN cr.job_name LIKE 'bg-%' THEN 'spawned' ELSE NULL END,
            cr.started_at,
            cr.finished_at,
            cr.exit_code,
            cr.log_path,
            cr.summary,
            cr.notify_json,
            cr.no_notify_reason,
            NULL,
            CASE WHEN cr.notify_json IS NULL THEN 0 ELSE 1 END,
            CASE
              WHEN cr.delivery_status = 'silent' THEN 'none'
              WHEN cr.delivery_status IS NULL AND cr.notify_json IS NULL THEN 'none'
              WHEN cr.delivery_status IS NULL AND cr.notify_json IS NOT NULL THEN 'pending'
              ELSE cr.delivery_status
            END,
            0,
            cr.delivered_at,
            NULL,
            cr.started_at,
            COALESCE(cr.finished_at, cr.started_at)
         FROM cron_runs cr
         LEFT JOIN cron_specs cs ON cs.job_name = cr.job_name",
    )?;
    tx.execute_batch(
        "INSERT INTO async_runs (
            id, kind, producer_ref, source_session_id, run_session_id,
            target_chat_id, target_thread_id, status, handoff_state,
            started_at, finished_at, exit_code, log_path, summary,
            notify_json, no_notify_reason, error_json, delivery_required,
            delivery_status, delivery_attempts, delivered_at,
            last_delivery_error, created_at, updated_at
         )
         SELECT
            lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
            lower(hex(randomblob(2))) || '-' || lower(hex(randomblob(2))) || '-' ||
            lower(hex(randomblob(6))),
            'background',
            cs.job_name,
            substr(cs.schedule, 5),
            lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
            lower(hex(randomblob(2))) || '-' || lower(hex(randomblob(2))) || '-' ||
            lower(hex(randomblob(6))),
            COALESCE(cs.target_chat_id, 0),
            cs.target_thread_id,
            'failed',
            'queued',
            COALESCE(cs.triggered_at, cs.created_at),
            strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            NULL,
            NULL,
            'background handoff interrupted by async_runs migration',
            '{\"content\":\"Background work was interrupted during an upgrade before it could be started.\"}',
            NULL,
            '{\"error\":\"legacy background cron spec removed before execution\"}',
            1,
            'pending',
            0,
            NULL,
            NULL,
            COALESCE(cs.created_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         FROM cron_specs cs
         WHERE cs.schedule LIKE '@bg:%'
           AND NOT EXISTS (SELECT 1 FROM cron_runs cr WHERE cr.job_name = cs.job_name)",
    )?;
    tx.execute("DELETE FROM cron_specs WHERE schedule LIKE '@bg:%'", [])?;
    tx.execute_batch("DROP TABLE cron_runs")?;
    Ok(())
}
```

In `MIGRATIONS`, replace `M::up(V21_SCHEMA),` with:

```rust
M::up(V21_SCHEMA),
M::up_with_hook("", v22_async_runs),
```

- [ ] **Step 4: Update old migration tests that inspect `cron_runs`**

In `crates/right-db/src/migrations.rs`, update older tests named around v5, v12, v18, and v19 so they inspect the final `async_runs` schema or use `to_version(&mut conn, 21)` when they intentionally test pre-v22 migration behavior. Example:

```rust
#[test]
fn v22_async_runs_has_delivery_columns() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('async_runs')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for col in [
        "summary",
        "notify_json",
        "delivered_at",
        "delivery_status",
        "no_notify_reason",
        "target_chat_id",
        "target_thread_id",
    ] {
        assert!(cols.contains(&col.to_string()), "{col} column missing");
    }
}
```

- [ ] **Step 5: Run targeted DB tests**

Run:

```bash
devenv shell -- cargo test -p right-db
```

Expected: all `right-db` tests pass.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/right-db/src/sql/v22_async_runs.sql crates/right-db/src/migrations.rs crates/right-db/tests/smoke.rs
devenv shell -- git commit -m "feat(db): add async runs table"
```

Expected: commit succeeds.

## Task 2: Add Shared `async_runs` Helpers

**Files:**
- Create: `crates/right-agent/src/async_runs.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Write helper tests**

Create `crates/right-agent/src/async_runs.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> rusqlite::Connection {
        let dir = tempfile::tempdir().unwrap();
        right_db::open_connection(dir.path(), true).unwrap()
    }

    #[test]
    fn insert_running_cron_run_sets_none_delivery() {
        let conn = setup();
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: Some(-100),
                target_thread_id: Some(7),
            },
        )
        .unwrap();

        let row: (String, String, i64, String) = conn
            .query_row(
                "SELECT kind, producer_ref, delivery_required, delivery_status FROM async_runs WHERE id='run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, ("cron".into(), "job-a".into(), 0, "none".into()));
    }

    #[test]
    fn mark_run_output_with_notify_makes_delivery_pending() {
        let conn = setup();
        insert_running_cron_run(
            &conn,
            NewCronRun {
                id: "run-1",
                job_name: "job-a",
                started_at: "2026-05-18T10:00:00Z",
                log_path: "/log/run-1.ndjson",
                target_chat_id: Some(-100),
                target_thread_id: None,
            },
        )
        .unwrap();

        persist_run_output(
            &conn,
            "run-1",
            RunOutput {
                summary: Some("summary"),
                notify_json: Some("{\"content\":\"hi\"}"),
                no_notify_reason: None,
                error_json: None,
                delivery_required: true,
            },
        )
        .unwrap();

        let row: (i64, String, String) = conn
            .query_row(
                "SELECT delivery_required, delivery_status, notify_json FROM async_runs WHERE id='run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, (1, "pending".into(), "{\"content\":\"hi\"}".into()));
    }

    #[test]
    fn cron_run_to_json_parses_notify() {
        let row = CronRunJsonRow {
            id: "run-1".into(),
            job_name: "job-a".into(),
            started_at: "2026-05-18T10:00:00Z".into(),
            finished_at: None,
            exit_code: None,
            status: "success".into(),
            log_path: Some("/log".into()),
            summary: Some("summary".into()),
            notify_json: Some("{\"content\":\"hello\"}".into()),
            delivered_at: None,
            delivery_status: Some("pending".into()),
            no_notify_reason: None,
        };

        let json = cron_run_to_json(&row);
        assert_eq!(json["job_name"], "job-a");
        assert_eq!(json["notify"]["content"], "hello");
    }
}
```

- [ ] **Step 2: Run tests and verify they fail to compile**

Run:

```bash
devenv shell -- cargo test -p right-agent async_runs
```

Expected: compile failures for missing `NewCronRun`, `RunOutput`, and helper functions.

- [ ] **Step 3: Implement helper module**

At the top of `crates/right-agent/src/async_runs.rs`, add:

```rust
#[derive(Debug, Clone, Copy)]
pub struct NewCronRun<'a> {
    pub id: &'a str,
    pub job_name: &'a str,
    pub started_at: &'a str,
    pub log_path: &'a str,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct NewBackgroundRun<'a> {
    pub id: &'a str,
    pub source_session_id: &'a str,
    pub run_session_id: &'a str,
    pub target_chat_id: i64,
    pub target_thread_id: Option<i64>,
    pub created_at: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct RunOutput<'a> {
    pub summary: Option<&'a str>,
    pub notify_json: Option<&'a str>,
    pub no_notify_reason: Option<&'a str>,
    pub error_json: Option<&'a str>,
    pub delivery_required: bool,
}

#[derive(Debug, Clone)]
pub struct CronRunJsonRow {
    pub id: String,
    pub job_name: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub status: String,
    pub log_path: Option<String>,
    pub summary: Option<String>,
    pub notify_json: Option<String>,
    pub delivered_at: Option<String>,
    pub delivery_status: Option<String>,
    pub no_notify_reason: Option<String>,
}
```

Add functions:

```rust
pub fn insert_running_cron_run(
    conn: &rusqlite::Connection,
    run: NewCronRun<'_>,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
            status, started_at, log_path, delivery_required, delivery_status,
            created_at, updated_at
         ) VALUES (?1, 'cron', ?2, ?1, ?3, ?4, 'running', ?5, ?6, 0, 'none', ?5, ?5)",
        rusqlite::params![
            run.id,
            run.job_name,
            run.target_chat_id,
            run.target_thread_id,
            run.started_at,
            run.log_path,
        ],
    )?;
    Ok(())
}

pub fn insert_queued_background_run(
    conn: &rusqlite::Connection,
    run: NewBackgroundRun<'_>,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, source_session_id, run_session_id, target_chat_id, target_thread_id,
            status, handoff_state, delivery_required, delivery_status, created_at, updated_at
         ) VALUES (?1, 'background', ?2, ?3, ?4, ?5, 'queued', 'queued', 0, 'none', ?6, ?6)",
        rusqlite::params![
            run.id,
            run.source_session_id,
            run.run_session_id,
            run.target_chat_id,
            run.target_thread_id,
            run.created_at,
        ],
    )?;
    Ok(())
}

pub fn mark_background_spawned(
    conn: &rusqlite::Connection,
    run_id: &str,
    started_at: &str,
    log_path: Option<&str>,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE async_runs
         SET status='running', handoff_state='spawned', started_at=?1, log_path=?2,
             delivery_required=1, delivery_status='pending', updated_at=?1
         WHERE id=?3",
        rusqlite::params![started_at, log_path, run_id],
    )?;
    Ok(())
}

pub fn persist_run_output(
    conn: &rusqlite::Connection,
    run_id: &str,
    output: RunOutput<'_>,
) -> Result<(), rusqlite::Error> {
    let delivery_status = if output.delivery_required {
        "pending"
    } else {
        "none"
    };
    conn.execute(
        "UPDATE async_runs
         SET summary=?1, notify_json=?2, no_notify_reason=?3, error_json=?4,
             delivery_required=?5, delivery_status=?6, updated_at=?7
         WHERE id=?8",
        rusqlite::params![
            output.summary,
            output.notify_json,
            output.no_notify_reason,
            output.error_json,
            if output.delivery_required { 1 } else { 0 },
            delivery_status,
            chrono::Utc::now().to_rfc3339(),
            run_id,
        ],
    )?;
    Ok(())
}

pub fn finish_run(
    conn: &rusqlite::Connection,
    run_id: &str,
    exit_code: Option<i32>,
    status: &str,
) -> Result<(), rusqlite::Error> {
    let finished_at = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE async_runs
         SET finished_at=?1, exit_code=?2, status=?3, updated_at=?1
         WHERE id=?4",
        rusqlite::params![finished_at, exit_code, status, run_id],
    )?;
    Ok(())
}

pub fn cron_run_to_json(row: &CronRunJsonRow) -> serde_json::Value {
    let mut val = serde_json::json!({
        "id": row.id,
        "job_name": row.job_name,
        "started_at": row.started_at,
        "finished_at": row.finished_at,
        "exit_code": row.exit_code,
        "status": row.status,
        "log_path": row.log_path,
        "delivered_at": row.delivered_at,
        "delivery_status": row.delivery_status,
        "no_notify_reason": row.no_notify_reason,
    });
    if let Some(s) = &row.summary {
        val["summary"] = serde_json::Value::String(s.clone());
    }
    if let Some(nj) = &row.notify_json
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(nj)
    {
        val["notify"] = parsed;
    }
    val
}
```

In `crates/right-agent/src/lib.rs`, add:

```rust
pub mod async_runs;
```

- [ ] **Step 4: Run helper tests**

Run:

```bash
devenv shell -- cargo test -p right-agent async_runs
```

Expected: all `async_runs` tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
devenv shell -- git add crates/right-agent/src/async_runs.rs crates/right-agent/src/lib.rs
devenv shell -- git commit -m "feat(agent): add async run helpers"
```

Expected: commit succeeds.

## Task 3: Move Cron MCP History to `async_runs`

**Files:**
- Modify: `crates/right/src/memory_server.rs`
- Modify: `crates/right/src/right_backend.rs`
- Modify: `crates/right/src/memory_server_mcp_tests.rs`
- Modify: `crates/right/src/right_backend_tests.rs`
- Modify: `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` if they mention `cron_runs`

- [ ] **Step 1: Update MCP tests to insert `async_runs`**

In `crates/right/src/memory_server_mcp_tests.rs`, replace `insert_cron_run` with:

```rust
fn insert_cron_run(
    server: &MemoryServer,
    id: &str,
    job_name: &str,
    started_at: &str,
    status: &str,
) {
    let conn = server.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id,
            started_at, status, log_path, delivery_required, delivery_status,
            created_at, updated_at
         ) VALUES (?1, 'cron', ?2, ?1, -100, ?3, ?4, ?5, 0, 'none', ?3, ?3)",
        rusqlite::params![id, job_name, started_at, status, format!("/tmp/{id}.log")],
    )
    .expect("insert async cron run");
}
```

Add this test:

```rust
#[tokio::test]
async fn test_cron_list_runs_excludes_background_rows() {
    let (server, _dir) = setup_server();
    insert_cron_run(
        &server,
        "cron-001",
        "job-a",
        "2026-04-01T10:00:00Z",
        "success",
    );
    {
        let conn = server.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, source_session_id, run_session_id, target_chat_id,
                started_at, status, delivery_required, delivery_status, created_at, updated_at
             ) VALUES (
                'bg-001', 'background', 'main', 'bg-session', -100,
                '2026-04-01T11:00:00Z', 'success', 1, 'pending',
                '2026-04-01T11:00:00Z', '2026-04-01T11:00:00Z'
             )",
            [],
        )
        .unwrap();
    }

    let result = server
        .cron_list_runs(Parameters(CronListRunsParams {
            job_name: None,
            limit: None,
        }))
        .await
        .expect("cron_list_runs ok");
    let text = call_result_text(result);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["id"], "cron-001");
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
devenv shell -- cargo test -p right memory_server_mcp_tests::test_cron_list_runs_excludes_background_rows
```

Expected: failure because `cron_list_runs` still queries `cron_runs`.

- [ ] **Step 3: Update `memory_server.rs` queries**

In `crates/right/src/memory_server.rs`, replace `cron_list_runs` SQL with:

```rust
"SELECT id, producer_ref, started_at, finished_at, exit_code, status, log_path,
        summary, notify_json, delivered_at, delivery_status, no_notify_reason
 FROM async_runs
 WHERE kind = 'cron'
   AND (?1 IS NULL OR producer_ref = ?1)
 ORDER BY started_at DESC
 LIMIT ?2"
```

Replace `cron_show_run` SQL with:

```rust
"SELECT id, producer_ref, started_at, finished_at, exit_code, status, log_path,
        summary, notify_json, delivered_at, delivery_status, no_notify_reason
 FROM async_runs
 WHERE kind = 'cron' AND id = ?1"
```

Replace local `cron_run_to_json` with a wrapper around `right_agent::async_runs::cron_run_to_json`:

```rust
pub(crate) fn cron_run_to_json(
    id: &str,
    job_name: &str,
    started_at: &str,
    finished_at: Option<&str>,
    exit_code: Option<i64>,
    status: &str,
    log_path: Option<&str>,
    summary: Option<&str>,
    notify_json: Option<&str>,
    delivered_at: Option<&str>,
    delivery_status: Option<&str>,
    no_notify_reason: Option<&str>,
) -> serde_json::Value {
    right_agent::async_runs::cron_run_to_json(&right_agent::async_runs::CronRunJsonRow {
        id: id.to_owned(),
        job_name: job_name.to_owned(),
        started_at: started_at.to_owned(),
        finished_at: finished_at.map(str::to_owned),
        exit_code,
        status: status.to_owned(),
        log_path: log_path.map(str::to_owned),
        summary: summary.map(str::to_owned),
        notify_json: notify_json.map(str::to_owned),
        delivered_at: delivered_at.map(str::to_owned),
        delivery_status: delivery_status.map(str::to_owned),
        no_notify_reason: no_notify_reason.map(str::to_owned),
    })
}
```

- [ ] **Step 4: Update backend queries**

In `crates/right/src/right_backend.rs`, update `call_cron_list_runs` and `call_cron_show_run` with the same `async_runs WHERE kind='cron'` SQL. Keep output JSON shape unchanged.

- [ ] **Step 5: Update tests that still insert `cron_runs`**

Run:

```bash
devenv shell -- rg -n "cron_runs" crates/right/src
```

For each test insert, replace `cron_runs` with an `async_runs` insert matching Step 1. Leave docs strings alone only if they describe the historical migration; MCP-facing text should say "cron run history" rather than table names.

- [ ] **Step 6: Run targeted MCP/backend tests**

Run:

```bash
devenv shell -- cargo test -p right memory_server_mcp_tests
devenv shell -- cargo test -p right right_backend
```

Expected: all selected tests pass.

- [ ] **Step 7: Commit**

Run:

```bash
devenv shell -- git add crates/right/src/memory_server.rs crates/right/src/right_backend.rs crates/right/src/memory_server_mcp_tests.rs crates/right/src/right_backend_tests.rs crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md
devenv shell -- git commit -m "refactor(mcp): read cron history from async runs"
```

Expected: commit succeeds. If one of the prompt files was not changed, omit it from `git add`.

## Task 4: Make Cron Executor Write `async_runs`

**Files:**
- Modify: `crates/bot/src/cron.rs`
- Modify: `crates/bot/src/cron_delivery.rs` tests only if they construct cron rows before Task 5
- Modify: `crates/bot/src/telegram/worker.rs` background marker tests if they construct cron rows before Task 7

- [ ] **Step 1: Add failing cron executor test**

In `crates/bot/src/cron.rs` tests, update or add a unit test around `insert_running_run`:

```rust
#[test]
fn insert_running_run_writes_async_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(tmp.path(), true).unwrap();
    let spec = right_agent::cron_spec::CronSpec {
        job_name: "job-a".into(),
        schedule: "* * * * *".into(),
        prompt: "ping".into(),
        lock_ttl: "30m".into(),
        max_budget_usd: 1.0,
        created_at: "2026-05-18T10:00:00Z".into(),
        updated_at: "2026-05-18T10:00:00Z".into(),
        triggered_at: None,
        recurring: true,
        run_at: None,
        target_chat_id: Some(-100),
        target_thread_id: Some(7),
        schedule_kind: right_agent::cron_spec::ScheduleKind::Recurring("* * * * *".into()),
    };

    insert_running_run(&conn, "run-1", "job-a", "2026-05-18T10:00:00Z", "/log", &spec).unwrap();

    let row: (String, String, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT kind, producer_ref, target_chat_id, target_thread_id FROM async_runs WHERE id='run-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row, ("cron".into(), "job-a".into(), Some(-100), Some(7)));
}
```

- [ ] **Step 2: Run test and verify it fails**

Run:

```bash
devenv shell -- cargo test -p bot insert_running_run_writes_async_runs
```

Expected: failure because `insert_running_run` still inserts into `cron_runs`.

- [ ] **Step 3: Update cron insert/update helpers**

In `crates/bot/src/cron.rs`, replace `insert_running_run` body with:

```rust
right_agent::async_runs::insert_running_cron_run(
    conn,
    right_agent::async_runs::NewCronRun {
        id: run_id,
        job_name,
        started_at,
        log_path,
        target_chat_id: spec.target_chat_id,
        target_thread_id: spec.target_thread_id,
    },
)
```

Replace output persistence:

```rust
right_agent::async_runs::persist_run_output(
    &conn,
    &run_id,
    right_agent::async_runs::RunOutput {
        summary: Some(&cron_output.summary),
        notify_json: notify_json.as_deref(),
        no_notify_reason: cron_output.no_notify_reason.as_deref(),
        error_json: None,
        delivery_required: cron_output.notify.is_some(),
    },
)
```

Replace failure notify persistence:

```rust
right_agent::async_runs::persist_run_output(
    &conn,
    &run_id,
    right_agent::async_runs::RunOutput {
        summary: Some("failed"),
        notify_json: Some(&json),
        no_notify_reason: None,
        error_json: None,
        delivery_required: true,
    },
)
```

Replace `update_run_record` body with:

```rust
if let Err(e) = right_agent::async_runs::finish_run(conn, run_id, exit_code, status) {
    tracing::error!("DB update for run {run_id} failed: {e:#}");
}
```

Use `delivery_status = "none"` in logs for silent cron output instead of `"silent"`.

- [ ] **Step 4: Keep cron-backed background compiling until Task 8**

Do not remove `BackgroundContinuation` from cron in this task. At this point the
worker still calls the old enqueue path, so removing it would create a broken
intermediate branch. The final removal happens in Task 10 after the worker uses
the immediate background executor.

Update any existing cron tests that insert or inspect run records so they use
`async_runs`. Keep tests around `select_schema_and_fork` temporarily if they
still compile; Task 10 deletes them with the old scheduler path.

- [ ] **Step 5: Run cron tests**

Run:

```bash
devenv shell -- cargo test -p bot cron
```

Expected: all bot cron tests pass or fail only where tests still mention `cron_runs`; update those tests to use `async_runs`.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/cron.rs
devenv shell -- git commit -m "refactor(cron): persist runs as async runs"
```

Expected: commit succeeds.

## Task 5: Move Cron Spec Utilities to `async_runs`

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs`
- Modify: `crates/right-agent/src/cron_spec_tests.rs`

- [ ] **Step 1: Write failing tests for spec utilities**

In `crates/right-agent/src/cron_spec_tests.rs`, update tests that currently
insert into `cron_runs` so they insert into `async_runs`. Add this regression
test for last-run status:

```rust
#[test]
fn list_specs_reads_last_run_from_async_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(tmp.path(), true).unwrap();
    conn.execute(
        "INSERT INTO cron_specs (
            job_name, schedule, prompt, lock_ttl, max_budget_usd,
            created_at, updated_at, recurring, target_chat_id
         ) VALUES (
            'morning', '0 9 * * *', 'prompt', '30m', 2.0, '2026-05-18T10:00:00Z',
            '2026-05-18T10:00:00Z', 0, -100
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id,
            started_at, finished_at, status, log_path, delivery_required,
            delivery_status, created_at, updated_at
         ) VALUES (
            'run-1', 'cron', 'morning', 'run-1', -100,
            '2026-05-18T10:30:00Z', '2026-05-18T10:31:00Z', 'success',
            '/log/run-1.ndjson', 0, 'none',
            '2026-05-18T10:30:00Z', '2026-05-18T10:31:00Z'
         )",
        [],
    )
    .unwrap();

    let text = right_agent::cron_spec::list_specs(&conn).unwrap();
    assert!(text.contains("morning"), "list output: {text}");
    assert!(text.contains("success"), "list output should include last run status: {text}");
    assert!(text.contains("run-1"), "list output should include last run id: {text}");
}
```

Add this regression for target propagation:

```rust
#[test]
fn update_spec_target_propagates_to_undelivered_async_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(tmp.path(), true).unwrap();
    right_agent::cron_spec::create_spec_v2(
        &conn,
        "morning",
        Some("0 9 * * *"),
        "prompt",
        "30m",
        Some(2.0),
        None,
        false,
        Some(-100),
        Some(7),
    )
    .unwrap();
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
            started_at, finished_at, status, notify_json, delivery_required,
            delivery_status, created_at, updated_at
         ) VALUES (
            'run-undelivered', 'cron', 'morning', 'run-undelivered', -100, 7,
            '2026-05-18T10:00:00Z', '2026-05-18T10:01:00Z', 'success',
            '{\"content\":\"hi\"}', 1, 'pending',
            '2026-05-18T10:00:00Z', '2026-05-18T10:01:00Z'
         )",
        [],
    )
    .unwrap();

    right_agent::cron_spec::update_spec(
        &conn,
        "morning",
        None,
        None,
        None,
        None,
        None,
        Some(false),
        Some(200),
        Some(0),
    )
    .unwrap();

    let target: (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT target_chat_id, target_thread_id FROM async_runs WHERE id='run-undelivered'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(target, (Some(200), None));
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-agent list_specs_reads_last_run_from_async_runs
devenv shell -- cargo test -p right-agent update_spec_target_propagates_to_undelivered_async_runs
```

Expected: failures because `cron_spec.rs` still queries or updates `cron_runs`.

- [ ] **Step 3: Update `cron_spec.rs` SQL**

In `crates/right-agent/src/cron_spec.rs`:

- replace last-run lookup SQL with:

```sql
SELECT id, started_at, finished_at, exit_code, status, log_path
FROM async_runs
WHERE kind = 'cron' AND producer_ref = ?1
ORDER BY started_at DESC
LIMIT 1
```

- replace target propagation SQL with:

```sql
UPDATE async_runs
SET target_chat_id = ?1,
    target_thread_id = ?2
WHERE kind = 'cron'
  AND producer_ref = ?3
  AND delivery_status IN ('pending', 'retryable')
```

- replace `cron_runs` detail queries used by `cron_show_run`-style helpers with
  `async_runs WHERE kind='cron'`.

Keep `ScheduleKind::BackgroundContinuation` and `insert_background_continuation`
unchanged in this task. They are removed after the immediate background executor
is wired.

- [ ] **Step 4: Update remaining tests in `cron_spec_tests.rs`**

Run:

```bash
devenv shell -- rg -n "cron_runs" crates/right-agent/src/cron_spec.rs crates/right-agent/src/cron_spec_tests.rs
```

Expected remaining matches are only comments or tests explicitly constructing
pre-v22 migration input. Active helper tests should insert `async_runs`.

- [ ] **Step 5: Run cron spec tests**

Run:

```bash
devenv shell -- cargo test -p right-agent cron_spec
```

Expected: all cron spec tests pass.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/right-agent/src/cron_spec.rs crates/right-agent/src/cron_spec_tests.rs
devenv shell -- git commit -m "refactor(cron): read spec run state from async runs"
```

Expected: commit succeeds.

## Task 6: Generalize Delivery to `async_runs`

**Files:**
- Move: `crates/bot/src/cron_delivery.rs` to `crates/bot/src/async_delivery.rs`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/async_delivery.rs`

- [ ] **Step 1: Rename module**

Run:

```bash
devenv shell -- git mv crates/bot/src/cron_delivery.rs crates/bot/src/async_delivery.rs
```

In `crates/bot/src/lib.rs`, change:

```rust
pub(crate) mod cron_delivery;
```

to:

```rust
pub(crate) mod async_delivery;
```

Change the spawn site from:

```rust
cron_delivery::run_delivery_loop(
```

to:

```rust
async_delivery::run_delivery_loop(
```

- [ ] **Step 2: Update delivery tests to use `async_runs` and add background coverage**

In `crates/bot/src/async_delivery.rs`, rename `PendingCronResult` to `PendingAsyncResult`:

```rust
#[derive(Debug)]
pub(crate) struct PendingAsyncResult {
    pub id: String,
    pub kind: String,
    pub producer_ref: Option<String>,
    pub notify_json: String,
    pub summary: String,
    pub status: String,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
}
```

Update tests to insert rows into `async_runs`. Example helper:

```rust
fn insert_async_result(
    conn: &rusqlite::Connection,
    id: &str,
    kind: &str,
    producer_ref: Option<&str>,
    finished_at: &str,
    status: &str,
    notify_json: Option<&str>,
    delivery_required: bool,
    delivery_status: &str,
) {
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id,
            started_at, finished_at, status, summary, notify_json,
            delivery_required, delivery_status, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?1, -100, ?4, ?4, ?5, 'summary', ?6, ?7, ?8, ?4, ?4)",
        rusqlite::params![
            id,
            kind,
            producer_ref,
            finished_at,
            status,
            notify_json,
            if delivery_required { 1 } else { 0 },
            delivery_status,
        ],
    )
    .unwrap();
}
```

Add tests:

```rust
#[test]
fn fetch_pending_reads_async_runs_and_skips_none_delivery() {
    let (_dir, conn) = setup_db();
    insert_async_result(
        &conn,
        "silent",
        "cron",
        Some("job"),
        "2026-05-18T10:00:00Z",
        "success",
        None,
        false,
        "none",
    );
    insert_async_result(
        &conn,
        "notify",
        "cron",
        Some("job"),
        "2026-05-18T10:01:00Z",
        "success",
        Some("{\"content\":\"hi\"}"),
        true,
        "pending",
    );

    let pending = fetch_pending(&conn).unwrap().unwrap();
    assert_eq!(pending.id, "notify");
}

#[test]
fn format_async_yaml_has_background_instruction() {
    let pending = PendingAsyncResult {
        id: "bg-1".into(),
        kind: "background".into(),
        producer_ref: None,
        notify_json: "{\"content\":\"done\"}".into(),
        summary: "summary".into(),
        status: "success".into(),
        target_chat_id: Some(-100),
        target_thread_id: None,
    };

    let output = format_async_yaml(&pending, 0);
    assert!(output.contains("background"));
    assert!(output.contains("done"));
}
```

- [ ] **Step 3: Run delivery tests and verify failures**

Run:

```bash
devenv shell -- cargo test -p bot async_delivery
```

Expected: compile/query failures until the module uses `async_runs`.

- [ ] **Step 4: Update fetch, dedupe, and outcome SQL**

Replace `fetch_pending` SQL with:

```rust
"SELECT id, kind, producer_ref, notify_json, COALESCE(summary, ''), status,
        target_chat_id, target_thread_id
 FROM async_runs
 WHERE delivery_required = 1
   AND delivery_status IN ('pending', 'retryable')
   AND status IN ('success', 'failed')
 ORDER BY finished_at ASC
 LIMIT 1"
```

Update `mark_delivery_outcome`:

```rust
conn.execute(
    "UPDATE async_runs
     SET delivery_status = ?1, delivered_at = ?2, updated_at = ?2
     WHERE id = ?3",
    rusqlite::params![status, now, run_id],
)?;
```

For dedupe, only dedupe cron rows with a `producer_ref`. Background rows must not supersede each other:

```rust
if pending.kind != "cron" {
    return Ok(Some((pending, 0)));
}
```

Older cron rows are superseded with:

```sql
UPDATE async_runs
SET delivered_at = ?1, delivery_status = 'superseded', updated_at = ?1
WHERE kind = 'cron'
  AND producer_ref = ?2
  AND id != ?3
  AND delivery_required = 1
  AND delivery_status IN ('pending', 'retryable')
  AND status IN ('success', 'failed')
```

- [ ] **Step 5: Add instruction variants**

In `crates/bot/src/async_delivery.rs`, add constants:

```rust
const DELIVERY_INSTRUCTION_BACKGROUND_SUCCESS: &str = "\
You are delivering the result of work the user moved to background execution.
The `content` field below is the FINAL user-facing answer to that earlier request.
Send it naturally as the answer to the user. Do not invent details.
Ignore the attachments field because attachments are sent separately.

Here is the YAML report of the background run:
";

const DELIVERY_INSTRUCTION_BACKGROUND_FAILURE: &str = "\
The background continuation below failed. The `content` field contains the
platform-generated failure report. Relay it to the user in natural prose while
preserving the facts. Do not invent details.

Here is the YAML report of the background run:
";
```

Replace `format_cron_yaml` with `format_async_yaml` that chooses instruction by `(kind, status)`:

```rust
let instruction = match (pending.kind.as_str(), pending.status.as_str()) {
    ("background", "failed") => DELIVERY_INSTRUCTION_BACKGROUND_FAILURE,
    ("background", _) => DELIVERY_INSTRUCTION_BACKGROUND_SUCCESS,
    ("cron", "failed") => DELIVERY_INSTRUCTION_FAILURE,
    _ => DELIVERY_INSTRUCTION_SUCCESS,
};
```

- [ ] **Step 6: Make delivery success depend on Telegram sends**

Change `deliver_through_session` to return a struct:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeliverySendReport {
    pub text_messages_sent: usize,
    pub attachment_batches_sent: usize,
}
```

Return `Err("delivery produced no content or attachments")` when both `reply.content` and `reply.attachments` are empty. Increment `text_messages_sent` only after an HTML send or plain fallback succeeds. If an HTML send fails and the fallback also fails, return `Err(format!("telegram send failed: {e2:#}"))`. Treat attachment send failure as `Err`.

In the loop, mark delivered only for `Ok(report)` where:

```rust
report.text_messages_sent + report.attachment_batches_sent > 0
```

- [ ] **Step 7: Run delivery tests**

Run:

```bash
devenv shell -- cargo test -p bot async_delivery
```

Expected: all async delivery tests pass.

- [ ] **Step 8: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/async_delivery.rs crates/bot/src/lib.rs
devenv shell -- git commit -m "refactor(bot): generalize async delivery"
```

Expected: commit succeeds.

## Task 7: Add Background Handoff Gates

**Files:**
- Modify: `crates/bot/src/telegram/mod.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add gate type and tests**

In `crates/bot/src/telegram/mod.rs`, add:

```rust
/// Per-(chat, thread) handoff gate set before a foreground turn is moved to
/// background. Workers wait while a gate is present so the next foreground turn
/// cannot mutate the main session before the background fork is confirmed.
pub(crate) type BgHandoffGates = Arc<DashMap<(i64, i64), Arc<tokio::sync::Notify>>>;
```

Add helpers:

```rust
pub(crate) fn set_bg_handoff_gate(gates: &BgHandoffGates, key: (i64, i64)) {
    gates
        .entry(key)
        .or_insert_with(|| Arc::new(tokio::sync::Notify::new()));
}

pub(crate) fn release_bg_handoff_gate(gates: &BgHandoffGates, key: (i64, i64)) {
    if let Some((_, notify)) = gates.remove(&key) {
        notify.notify_waiters();
    }
}

pub(crate) async fn wait_for_bg_handoff_gate(gates: &BgHandoffGates, key: (i64, i64)) {
    loop {
        let Some(notify) = gates.get(&key).map(|entry| entry.value().clone()) else {
            return;
        };
        let notified = notify.notified();
        if gates.contains_key(&key) {
            notified.await;
        }
    }
}
```

Add tests in `mod.rs` tests:

```rust
#[tokio::test]
async fn bg_handoff_wait_blocks_until_release() {
    let gates: BgHandoffGates = Arc::new(DashMap::new());
    let key = (1, 0);
    set_bg_handoff_gate(&gates, key);

    let gates_for_task = Arc::clone(&gates);
    let wait = tokio::spawn(async move {
        wait_for_bg_handoff_gate(&gates_for_task, key).await;
        true
    });

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(!wait.is_finished(), "waiter must remain blocked while gate exists");
    release_bg_handoff_gate(&gates, key);
    assert!(wait.await.unwrap());
}
```

- [ ] **Step 2: Run gate test**

Run:

```bash
devenv shell -- cargo test -p bot bg_handoff_wait_blocks_until_release
```

Expected: pass after helper implementation.

- [ ] **Step 3: Wire gates through dependencies**

Add `bg_handoff_gates` to `WorkerControlDeps`:

```rust
pub(crate) bg_handoff_gates: BgHandoffGates,
```

Add it to `WorkerContext`:

```rust
pub bg_handoff_gates: super::BgHandoffGates,
```

In `crates/bot/src/telegram/dispatch.rs`, create and pass the shared map next to `bg_requests`:

```rust
let bg_handoff_gates: super::BgHandoffGates = Arc::new(DashMap::new());
```

Pass `Arc::clone(&worker_ctl.bg_handoff_gates)` into new workers.

- [ ] **Step 4: Set gate in Background callback**

In `handle_bg_callback` in `crates/bot/src/telegram/handler.rs`, before `token.cancel()`:

```rust
super::set_bg_handoff_gate(&worker_ctl.bg_handoff_gates, key);
worker_ctl.bg_requests.insert(key, *turn_id);
token.cancel();
```

If there is no stop token, do not set a gate.

- [ ] **Step 5: Wait before foreground processing**

In `spawn_worker`, after `collect_batch(first, &mut rx).await` and before attachment downloads or any DB/session work:

```rust
super::wait_for_bg_handoff_gate(&ctx.bg_handoff_gates, key).await;
```

This keeps queued messages accepted but prevents the next foreground invocation from starting.

- [ ] **Step 6: Run Telegram worker/handler tests**

Run:

```bash
devenv shell -- cargo test -p bot telegram::tests
devenv shell -- cargo test -p bot bg_request_race_tests
```

Expected: all selected tests pass.

- [ ] **Step 7: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/mod.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/worker.rs
devenv shell -- git commit -m "feat(bot): gate background handoff"
```

Expected: commit succeeds.

## Task 8: Add Immediate Background Executor

**Files:**
- Create: `crates/bot/src/background.rs`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/cron.rs` if output parsing helpers need visibility changes

- [ ] **Step 1: Add module skeleton and pure tests**

Create `crates/bot/src/background.rs`:

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct BackgroundRunRequest {
    pub run_id: String,
    pub source_session_id: String,
    pub target_chat_id: i64,
    pub target_thread_id: Option<i64>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HandoffStatus {
    Spawned,
    Failed(String),
}

pub(crate) fn bg_log_path(agent_dir: &Path, run_id: &str) -> PathBuf {
    agent_dir.join("background").join("logs").join(format!("{run_id}.ndjson"))
}
```

Add tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bg_log_path_uses_background_logs_dir() {
        let path = bg_log_path(Path::new("/agent"), "run-1");
        assert_eq!(path, Path::new("/agent/background/logs/run-1.ndjson"));
    }
}
```

In `crates/bot/src/lib.rs`, add:

```rust
pub(crate) mod background;
```

- [ ] **Step 2: Run skeleton test**

Run:

```bash
devenv shell -- cargo test -p bot bg_log_path_uses_background_logs_dir
```

Expected: pass.

- [ ] **Step 3: Implement background row creation in worker**

In `crates/bot/src/telegram/worker.rs`, replace `enqueue_background_job` with:

```rust
fn create_background_run(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
    main_session_id: &str,
) -> Result<String, String> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    right_agent::async_runs::insert_queued_background_run(
        conn,
        right_agent::async_runs::NewBackgroundRun {
            id: &run_id,
            source_session_id: main_session_id,
            run_session_id: &run_id,
            target_chat_id: chat_id,
            target_thread_id: (thread_id != 0).then_some(thread_id),
            created_at: &now,
        },
    )
    .map_err(|e| format!("insert background async_run failed: {e:#}"))?;
    Ok(run_id)
}
```

Update tests under `background_continuation_tests` to assert `async_runs.kind='background'` instead of `cron_specs.schedule='@bg:...'`.

- [ ] **Step 4: Run worker background tests and verify failures before full executor**

Run:

```bash
devenv shell -- cargo test -p bot background_continuation_tests
```

Expected: tests compile after replacing cron enqueue assertions; any remaining failure should point to missing executor handoff.

- [ ] **Step 5: Implement immediate executor API**

In `crates/bot/src/background.rs`, add an async function with this signature:

```rust
#[allow(clippy::too_many_arguments)]
pub(crate) async fn spawn_background_continuation(
    request: BackgroundRunRequest,
    agent_dir: PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<PathBuf>,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<String>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    session_locks: crate::telegram::SessionLocks,
    debug: Arc<std::sync::atomic::AtomicBool>,
) -> HandoffStatus
```

The function must:

- open DB connection;
- acquire `upgrade_lock.read().await`;
- acquire `SessionLocks[source_session_id]`;
- build `ClaudeInvocation` with:

```rust
resume_session_id: Some(request.source_session_id.clone()),
new_session_id: Some(request.run_id.clone()),
fork_session: true,
json_schema: Some(right_codegen::BG_CONTINUATION_SCHEMA_JSON.into()),
output_format: crate::cc::invocation::OutputFormat::StreamJson,
prompt: Some(request.prompt.clone()),
```

- spawn the child;
- create a stdout reader task that writes stream lines to `bg_log_path`;
- wait for the first `system/init` event for `request.run_id`;
- call `right_agent::async_runs::mark_background_spawned`;
- return `HandoffStatus::Spawned`;
- continue reading in a detached task until completion, then call `parse_cron_output` and `finish_run`/`persist_run_output`.

If no init event arrives within 30 seconds:

```rust
let _ = child.kill().await;
right_agent::async_runs::finish_run(&conn, &request.run_id, None, "failed")?;
```

Return `HandoffStatus::Failed("background handoff timed out before init".into())`.

- [ ] **Step 6: Move parsing helpers if needed**

If `background.rs` cannot access `parse_cron_output`, `CronNotify`, or `CronReplyOutput`, change their visibility in `crates/bot/src/cron.rs` to `pub(crate)` or move shared structured-output parsing to a new module `crates/bot/src/async_output.rs`. Keep the move mechanical:

```rust
pub(crate) mod async_output;
```

and have both cron and background use:

```rust
crate::async_output::parse_async_output(&collected_lines)
```

- [ ] **Step 7: Wire executor into `Backgrounded` worker path**

In the `InvokeCcFailure::Backgrounded` arm in `crates/bot/src/telegram/worker.rs`:

1. create the `async_runs` row with `create_background_run`;
2. call `background::spawn_background_continuation(...).await`;
3. on `Spawned`, edit the thinking banner and release the gate;
4. on `Failed(message)`, edit/send immediate error and release the gate;
5. use a guard object or `scopeguard`-style manual `finally` pattern so the gate is released on every path.

Use this shape:

```rust
let handoff = crate::background::spawn_background_continuation(...).await;
match handoff {
    crate::background::HandoffStatus::Spawned => {
        // edit banner
    }
    crate::background::HandoffStatus::Failed(message) => {
        send_error_to_telegram(&ctx, tg_chat_id, eff_thread_id, &html_escape(&message)).await;
    }
}
super::release_bg_handoff_gate(&ctx.bg_handoff_gates, key);
```

- [ ] **Step 8: Add regression test for handoff gate release timing**

In `crates/bot/src/telegram/worker.rs` tests, add a pure or fake-executor test around gate helpers:

```rust
#[tokio::test]
async fn queued_foreground_waits_for_background_handoff_release() {
    let gates: super::super::BgHandoffGates = Arc::new(DashMap::new());
    let key = (42, 0);
    super::super::set_bg_handoff_gate(&gates, key);

    let gates_for_wait = Arc::clone(&gates);
    let waiter = tokio::spawn(async move {
        super::super::wait_for_bg_handoff_gate(&gates_for_wait, key).await;
        "foreground-can-start"
    });

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(!waiter.is_finished());

    super::super::release_bg_handoff_gate(&gates, key);
    assert_eq!(waiter.await.unwrap(), "foreground-can-start");
}
```

- [ ] **Step 9: Run targeted background tests**

Run:

```bash
devenv shell -- cargo test -p bot background
devenv shell -- cargo test -p bot background_continuation_tests
devenv shell -- cargo test -p bot queued_foreground_waits_for_background_handoff_release
```

Expected: all selected tests pass.

- [ ] **Step 10: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/background.rs crates/bot/src/lib.rs crates/bot/src/telegram/worker.rs crates/bot/src/cron.rs
devenv shell -- git commit -m "feat(bot): start background continuations immediately"
```

Expected: commit succeeds. If `cron.rs` was not touched in this task, omit it from `git add`.

## Task 9: Update Background Markers and Recovery

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/background.rs`
- Modify: `crates/bot/src/lib.rs` if startup recovery is wired there

- [ ] **Step 1: Update marker tests to use `async_runs`**

In `background_continuation_tests`, replace inserts into `cron_runs` with:

```rust
conn.execute(
    "INSERT INTO async_runs (
        id, kind, source_session_id, run_session_id, target_chat_id, target_thread_id,
        status, handoff_state, started_at, finished_at, delivery_required,
        delivery_status, created_at, updated_at
     ) VALUES (
        'run-A', 'background', 'main', 'run-A', -100, NULL,
        'running', 'spawned', ?1, NULL, 1, 'pending', ?1, ?1
     )",
    rusqlite::params![now],
)
.unwrap();
```

Add a test:

```rust
#[test]
fn build_bg_marker_excludes_cron_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = open_connection(tmp.path(), true).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id,
            status, started_at, delivery_required, delivery_status, created_at, updated_at
         ) VALUES (
            'cron-run', 'cron', 'job', 'cron-run', -100,
            'running', ?1, 0, 'none', ?1, ?1
         )",
        rusqlite::params![now],
    )
    .unwrap();
    drop(conn);
    let m = build_bg_marker_for_chat(tmp.path(), -100);
    assert!(m.is_none(), "cron rows must not appear in background marker; got {m:?}");
}
```

- [ ] **Step 2: Update marker query**

In `build_bg_marker_for_chat`, replace the `cron_runs` query with:

```sql
SELECT id, COALESCE(producer_ref, 'background'), started_at, status
FROM async_runs
WHERE kind = 'background'
  AND target_chat_id = ?1
  AND (
    status = 'running'
    OR (status IN ('success', 'failed') AND delivery_status IN ('pending', 'retryable'))
  )
ORDER BY started_at DESC
LIMIT 5
```

- [ ] **Step 3: Add recovery test**

In `crates/bot/src/background.rs`, add a pure DB helper:

```rust
pub(crate) fn mark_interrupted_handoffs(conn: &rusqlite::Connection) -> Result<usize, rusqlite::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE async_runs
         SET status='failed',
             finished_at=?1,
             error_json='{\"error\":\"background handoff interrupted before spawn confirmation\"}',
             notify_json='{\"content\":\"Background work was interrupted before it could be started.\"}',
             delivery_required=1,
             delivery_status='pending',
             updated_at=?1
         WHERE kind='background'
           AND status='queued'
           AND handoff_state='queued'",
        rusqlite::params![now],
    )
}
```

Test:

```rust
#[test]
fn mark_interrupted_handoffs_converts_queued_background_to_failed_pending_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(dir.path(), true).unwrap();
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, source_session_id, run_session_id, target_chat_id,
            status, handoff_state, delivery_required, delivery_status, created_at, updated_at
         ) VALUES (
            'bg-1', 'background', 'main', 'bg-1', -100,
            'queued', 'queued', 0, 'none', '2026-05-18T10:00:00Z', '2026-05-18T10:00:00Z'
         )",
        [],
    )
    .unwrap();

    assert_eq!(mark_interrupted_handoffs(&conn).unwrap(), 1);
    let row: (String, i64, String) = conn
        .query_row(
            "SELECT status, delivery_required, delivery_status FROM async_runs WHERE id='bg-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, ("failed".into(), 1, "pending".into()));
}
```

- [ ] **Step 4: Wire recovery on startup**

In `crates/bot/src/lib.rs`, after opening agent startup DB or near cron/delivery startup, call:

```rust
if let Ok(conn) = right_db::open_connection(&agent_dir, false) {
    if let Err(e) = crate::background::mark_interrupted_handoffs(&conn) {
        tracing::warn!("background handoff recovery failed: {e:#}");
    }
}
```

Keep stale-running recovery as a separate helper if the implementation can identify process ownership. If no reliable process ownership exists, mark only queued handoffs in this task and leave running recovery out of code rather than guessing.

- [ ] **Step 5: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p bot background_continuation_tests
devenv shell -- cargo test -p bot mark_interrupted_handoffs
```

Expected: all selected tests pass.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/worker.rs crates/bot/src/background.rs crates/bot/src/lib.rs
devenv shell -- git commit -m "fix(bot): recover interrupted background handoffs"
```

Expected: commit succeeds.

## Task 10: Remove Remaining `cron_runs` Runtime References

**Files:**
- Modify every Rust/doc prompt file reported by the search, excluding migration comments/tests that intentionally mention legacy `cron_runs`.
- Modify: `crates/right-agent/src/cron_spec.rs`
- Modify: `crates/right-agent/src/cron_spec_tests.rs`
- Modify: `crates/bot/src/cron.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Remove cron-backed background schedule kind**

In `crates/right-agent/src/cron_spec_tests.rs`, add:

```rust
#[test]
fn load_specs_skips_legacy_bg_schedule_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(tmp.path(), true).unwrap();
    conn.execute(
        "INSERT INTO cron_specs (
            job_name, schedule, prompt, lock_ttl, max_budget_usd,
            created_at, updated_at, recurring, target_chat_id
         ) VALUES (
            'bg-legacy', '@bg:00000000-0000-0000-0000-000000000000',
            'prompt', '6h', 2.0, '2026-05-18T10:00:00Z',
            '2026-05-18T10:00:00Z', 0, -100
         )",
        [],
    )
    .unwrap();

    let specs = right_agent::cron_spec::load_specs_from_db(&conn).unwrap();
    assert!(specs.iter().all(|s| s.job_name != "bg-legacy"));
}
```

In `crates/right-agent/src/cron_spec.rs`:

- remove `ScheduleKind::BackgroundContinuation`;
- remove `insert_background_continuation`;
- make `ScheduleKind::from_db_row` return an error for `@bg:`:

```rust
if schedule.starts_with("@bg:") {
    return Err("legacy background continuation cron specs are not schedulable".to_string());
}
```

In `crates/bot/src/cron.rs`:

- delete `select_schema_and_fork`;
- remove `BackgroundContinuation` matches from `is_reconcile_tick_kind` and
  `is_run_job_loop_skip_kind`;
- set cron invocations to regular cron schema and no fork:

```rust
let json_schema_str = right_codegen::CRON_SCHEMA_JSON;
let fork_from_main_session = None;
let fork_session = false;
```

In `crates/bot/src/telegram/worker.rs`, delete the old `enqueue_background_job`
helper if it still exists. Production background handoff must use
`create_background_run` plus `background::spawn_background_continuation`.

Run:

```bash
devenv shell -- cargo test -p right-agent load_specs_skips_legacy_bg_schedule_rows
devenv shell -- cargo test -p bot cron
```

Expected: tests pass after all old cron-backed background references are removed.

- [ ] **Step 2: Search for references**

Run:

```bash
devenv shell -- rg -n "cron_runs" crates docs ARCHITECTURE.md PROMPT_SYSTEM.md
```

Expected allowed matches:

- `crates/right-db/src/migrations.rs` legacy migration copy/drop comments/tests;
- `docs/superpowers/specs/...` historical specs;
- the new implementation plan and design spec.

Every runtime SQL query, active test insert, MCP prompt, and architecture doc should use `async_runs` or table-neutral wording.

- [ ] **Step 3: Update architecture docs**

In `docs/architecture/sessions.md`, replace the per-session mutex and BackgroundContinuation sections with rules matching this wording:

```markdown
Worker, async delivery, and background handoff all acquire `SessionLocks` when
they invoke Claude Code against a main session. Background handoff holds the
source session lock until the forked background session has emitted its confirmed
handoff signal.

Background continuations are not cron specs. The Telegram worker creates an
`async_runs(kind='background')` row and starts `claude -p --resume <main>
--fork-session --session-id <bg_run_id>` immediately. The thread gate for
`(chat_id, thread_id)` stays closed until the fork handoff is confirmed.
```

In `docs/architecture/lifecycle.md`, replace the old background flow with:

```markdown
If foreground exits via timeout or Background button:
  - set a per-thread handoff gate
  - cancel foreground Claude Code
  - create `async_runs(kind='background')`
  - start the background fork immediately
  - release the gate only after confirmed fork handoff
  - deliver the completed result through shared async delivery
```

In `ARCHITECTURE.md`, update load-bearing references from `cron_runs` to `async_runs` where they describe active schema or delivery.

- [ ] **Step 4: Update prompt docs if needed**

Run:

```bash
devenv shell -- rg -n "cron_runs|cron delivery|BackgroundContinuation|@bg" PROMPT_SYSTEM.md crates/right-codegen/templates skills
```

If agent-facing text names table internals, replace it with user-facing tool names:

```markdown
Use `mcp__right__cron_list_runs` to inspect cron execution history.
```

Do not expose `async_runs` as an agent-facing concept unless the existing prompt already exposes DB tables for debugging.

- [ ] **Step 5: Run reference search again**

Run:

```bash
devenv shell -- rg -n "FROM cron_runs|INSERT INTO cron_runs|UPDATE cron_runs|@bg:|BackgroundContinuation" crates
```

Expected: no production runtime matches. Legacy migration tests may still contain `INSERT INTO cron_runs` to prove migration.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/right-agent/src/cron_spec.rs crates/right-agent/src/cron_spec_tests.rs crates/bot/src/cron.rs crates/bot/src/telegram/worker.rs docs/architecture/sessions.md docs/architecture/lifecycle.md ARCHITECTURE.md PROMPT_SYSTEM.md crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md
devenv shell -- git commit -m "refactor(bot): remove cron-backed background runtime"
```

Expected: commit succeeds. If a listed file was not changed, omit it from `git add`.

## Task 11: Final Verification and Review

**Files:**
- No planned source edits unless verification reveals bugs introduced by this branch.

- [ ] **Step 1: Run targeted package checks**

Run:

```bash
devenv shell -- cargo test -p right-db
devenv shell -- cargo test -p right-agent
devenv shell -- cargo test -p right
devenv shell -- cargo test -p bot
```

Expected: all pass.

- [ ] **Step 2: Run full workspace test**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: all workspace tests pass. This is mandatory before claiming completion.

- [ ] **Step 3: Run final debug searches**

Run:

```bash
devenv shell -- rg -n "FROM cron_runs|INSERT INTO cron_runs|UPDATE cron_runs|delivery_status = 'silent'|BackgroundContinuation|@bg:" crates
```

Expected: no production runtime matches. Legacy migration tests may mention `cron_runs` as migration input.

- [ ] **Step 4: Run Rust review flow**

Use the repository-required Rust review subagent if it is available in the implementation session:

```text
rust-dev:review-rust-code
```

If unavailable, run this fallback review checklist manually:

```text
- No session lock is held until a long-running background job completes.
- Handoff gate is released on every success and failure path.
- No foreground turn can start between Background callback and confirmed background handoff.
- Delivery marks delivered only after a successful Telegram send/edit.
- Cron MCP never returns background rows.
- `cron_runs` is not referenced by runtime SQL.
```

- [ ] **Step 5: Fix review findings**

For each review finding, make the smallest code change, run the narrowest relevant test, then commit:

```bash
devenv shell -- git add <changed-files>
devenv shell -- git commit -m "fix(bot): address async run review finding"
```

Expected: each finding has either a fix commit or a written reason it is not a defect.

- [ ] **Step 6: Final status**

Run:

```bash
devenv shell -- git status --short
devenv shell -- git log --oneline -8
```

Expected: clean worktree and recent commits show the async-runs implementation sequence.
