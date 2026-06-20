# Agent Self-Continuation Primitive — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give an agent an explicit, idempotent, bounded way to resume its own foreground session later to finish/collect out-of-turn async work (e.g. browser-use cloud sessions), instead of ending the turn and silently halting.

**Architecture:** A new `kind='continuation'` row on the existing `async_runs` table carries a stable task id, a `scheduled_at`, bounding state, and the resume instruction. The existing cron reconciler (5s tick) fires due rows; a new `continuation` execution module forks the foreground session per hop (fresh CC session id, full conversation context), and the hop emits a structured decision — **report** (deliver via the existing delivery path), **wait** (re-arm the same row), or **abandon** (silent finalize). Three foreground-only MCP tools (`continue_later`/`continue_cancel`/`continue_list`) arm, cancel, and list continuations. Idempotency is by agent judgment (the hop sees the full conversation); cancellation is explicit by task id. Hard ceilings (wall-clock, hops, cumulative budget) bound the chain; the give-up hop is forced to notify.

**Tech Stack:** Rust (edition 2024), `turso`-backed `right-db`, `tokio`, `rmcp` (MCP), `serde`/`schemars`. Tests via `cargo nextest`. Project conventions in `AGENTS.md`/`AGENTS.rust.md`/`ARCHITECTURE.md`.

**Spec:** `docs/superpowers/specs/2026-06-21-agent-self-continuation-design.md`

---

## Design refinements vs. the spec (read first)

The spec §5 proposed `continue_*` tools usable inside the continuation hop for re-arm. Code research changed this:

1. **Background-style runs have NO MCP scope** — foreground/scope tools are disabled and return `conversation_scope_unavailable` (`crates/bot/src/cc/invocation.rs` disallow chain; `crates/bot/src/background.rs:79`). So a hop cannot call tools to re-arm. **Re-arm/report/abandon is expressed in the hop's structured output**, and `continue_*` are **foreground-only** (arm/cancel/list).
2. **Each hop needs a fresh CC fork session id** (CC session ids must be unique); the stable continuation/task id is the `async_runs.id`. The hop's CC session id is a fresh uuid stored in `run_session_id` per fire. Completion semantics (report/wait/abandon) differ from `complete_background_run` (which mandates notify), so continuation gets its **own execution module**, not a reuse of `spawn_background_continuation`.

Everything else matches the spec.

## File Structure

**Modify:**
- `crates/right-db/src/migrations.rs` — register migration v49; add `v49_continuation_columns` hook (adds columns + index to `async_runs`).
- `crates/right-agent/src/async_runs.rs` — continuation DB API (insert/fetch-due/mark-running/rearm/finalize/cancel/list/accumulate-cost) + `active_root_session_id` lookup + a `ContinuationRow` struct.
- `crates/right-codegen/src/agent_def.rs` — add `CONTINUATION_SCHEMA_JSON` const (next to `BG_CONTINUATION_SCHEMA_JSON`).
- `crates/right-mcp/src/internal_client.rs` — add `CONTINUE_*_TOOL` / `CONTINUE_*_MCP_TOOL` name constants.
- `crates/right/src/right_backend.rs` — three params structs, three handlers, `tools_list` + `tools_call` entries.
- `crates/right/src/memory_server.rs` + `crates/right/src/aggregator.rs` — `with_instructions` "## Continuation" section (convention: both).
- `crates/bot/src/cc/invocation.rs` — `disallow_continue_tools` + wire into `disallow_foreground_only_tools_keep_learning`.
- `crates/bot/src/cron.rs` — `fire_due_continuations`, called from `reconcile_jobs`.
- `crates/bot/src/async_delivery.rs` — `kind='continuation'` branches (instruction, YAML label, outbox subdir).
- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` — continuation guidance.
- `PROMPT_SYSTEM.md` — sync.

**Create:**
- `crates/bot/src/continuation.rs` — continuation execution: `build_continuation_resume_prompt`, `ContinuationDecision`/`ContinuationOutput` parse types, `fire_continuation_hop`, completion mapping. Registered in `crates/bot/src/lib.rs` (`mod continuation;`).

## Constants (single source — `crates/right-agent/src/cron_spec.rs` neighbours or a new `continuation` const block)

Put these in `crates/right-agent/src/async_runs.rs` (top of file) so both the bot and the MCP handler crates can read them:

```rust
/// Continuation scheduling/bounding ceilings. Tunable; the budget cap is the
/// meaningful runaway guard, wall-clock is a generous backstop.
pub const CONTINUE_MIN_CHECK_IN_SECS: i64 = 30;
pub const CONTINUE_DEFAULT_CHECK_IN_SECS: i64 = 90;
pub const CONTINUE_MAX_WALLCLOCK_SECS: i64 = 6 * 3600; // 6h hard ceiling
pub const CONTINUE_DEFAULT_DEADLINE_SECS: i64 = 30 * 60; // 30m default if unset
pub const CONTINUE_MAX_HOPS: i64 = 120;
pub const CONTINUE_DEFAULT_BUDGET_USD: f64 = 5.0;
pub const CONTINUE_MAX_BUDGET_USD: f64 = 20.0;
pub const CONTINUE_PER_HOP_BUDGET_USD: f64 = 0.50;
```

---

## Task 1: Migration v49 — continuation columns + index

**Files:**
- Modify: `crates/right-db/src/migrations.rs`
- Test: `crates/right-db/src/migrations.rs` (inline `#[cfg(test)]`) or the existing migration test module.

First confirm the next version number: run `rg -n "version: 4[0-9]" crates/right-db/src/migrations.rs`. The current highest is **48**; use **49**. If a 49 already exists, use the next free integer and adjust all references below.

- [ ] **Step 1: Write the failing test**

Add to the migrations test module (mirror existing migration tests that open an in-memory/temp DB and assert columns):

```rust
#[tokio::test]
async fn v49_adds_continuation_columns() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::open_connection(dir.path(), true).await.unwrap();
    for col in [
        "scheduled_at", "deadline_at", "hop_count", "check_in_secs",
        "budget_usd", "cost_usd_accum", "instruction",
    ] {
        let n = conn
            .query_one(
                "SELECT COUNT(*) FROM pragma_table_info('async_runs') WHERE name = ?1",
                crate::params![col],
                |r| r.get::<i64>(0),
            )
            .await
            .unwrap();
        assert_eq!(n, 1, "async_runs missing column {col}");
    }
}
```

(Confirm the exact `open_connection` signature and `Row::get` form against a neighbouring migration test; match it.)

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right-db v49_adds_continuation_columns`
Expected: FAIL (columns absent).

- [ ] **Step 3: Add the migration hook**

After the `v41_cron_force_notify` function, add (mirrors v41's `column_exists` + `sqlite_master` guard exactly):

```rust
/// v49: agent self-continuation columns on async_runs.
///
/// `kind='continuation'` rows reuse the async_runs delivery/status machinery
/// and add scheduling + bounding state: `scheduled_at` (next fire),
/// `deadline_at` (hard give-up), `hop_count`, `check_in_secs` (re-arm cadence),
/// `budget_usd` (chain cap), `cost_usd_accum` (spent), `instruction` (resume
/// directive). Idempotent — pragma_table_info guard, sqlite_master guard like v41.
fn v49_continuation_columns(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        let exists = conn
            .query_i64(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='async_runs'",
                MigrationParams::Empty,
            )
            .await?;
        if exists == 0 {
            return Ok(());
        }
        let adds: &[(&str, &str)] = &[
            ("scheduled_at", "ALTER TABLE async_runs ADD COLUMN scheduled_at TEXT"),
            ("deadline_at", "ALTER TABLE async_runs ADD COLUMN deadline_at TEXT"),
            ("hop_count", "ALTER TABLE async_runs ADD COLUMN hop_count INTEGER NOT NULL DEFAULT 0"),
            ("check_in_secs", "ALTER TABLE async_runs ADD COLUMN check_in_secs INTEGER NOT NULL DEFAULT 90"),
            ("budget_usd", "ALTER TABLE async_runs ADD COLUMN budget_usd REAL NOT NULL DEFAULT 0"),
            ("cost_usd_accum", "ALTER TABLE async_runs ADD COLUMN cost_usd_accum REAL NOT NULL DEFAULT 0"),
            ("instruction", "ALTER TABLE async_runs ADD COLUMN instruction TEXT"),
        ];
        for (col, ddl) in adds {
            if !column_exists(conn, "async_runs", col).await? {
                conn.execute_batch(ddl).await?;
            }
        }
        // Due-scan index for the reconciler. CREATE INDEX IF NOT EXISTS is idempotent.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_async_runs_continuation_due \
             ON async_runs(kind, status, scheduled_at)",
        )
        .await?;
        Ok(())
    })
}
```

Register it in the `MIGRATIONS` array (after v48):

```rust
        Migration {
            version: 49,
            sql: "",
            hook: Some(v49_continuation_columns),
        },
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p right-db v49_adds_continuation_columns`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-db/src/migrations.rs
git commit -m "feat(right-db): v49 async_runs continuation columns"
```

---

## Task 2: Continuation DB API on `async_runs`

**Files:**
- Modify: `crates/right-agent/src/async_runs.rs`
- Test: `crates/right-agent/src/async_runs.rs` (`#[cfg(test)]` module; if the file exceeds the size rule, extract to `async_runs_tests.rs` per AGENTS.rust.md).

This task adds: a `NewContinuation` input struct, a `ContinuationRow` struct, and functions `insert_continuation`, `fetch_due_continuations`, `mark_continuation_running`, `rearm_continuation`, `finalize_continuation_report`, `finalize_continuation_silent`, `finalize_continuation_failed`, `cancel_continuation`, `list_continuations`, and `active_root_session_id`. All take `&Connection` (or `&Transaction`). Multi-write functions use a single immediate transaction (`AGENTS.rust.md` transaction rule).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod continuation_tests {
    use super::*;

    async fn mem() -> Connection {
        // mirror existing async_runs tests' in-memory open helper
        crate::test_support::open_memory_db().await
    }

    #[tokio::test]
    async fn insert_then_fetch_due() {
        let conn = mem().await;
        insert_continuation(&conn, NewContinuation {
            id: "c1",
            source_session_id: "s1",
            target_chat_id: 42,
            target_thread_id: None,
            instruction: "check browser-use",
            scheduled_at: "2020-01-01T00:00:00+00:00",
            deadline_at: "2999-01-01T00:00:00+00:00",
            check_in_secs: 90,
            budget_usd: 5.0,
            created_at: "2020-01-01T00:00:00+00:00",
        }).await.unwrap();

        let due = fetch_due_continuations(&conn, "2021-01-01T00:00:00+00:00").await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "c1");
        assert_eq!(due[0].source_session_id, "s1");

        // not due yet
        let none = fetch_due_continuations(&conn, "2019-01-01T00:00:00+00:00").await.unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn rearm_updates_same_row_and_bumps_hop() {
        let conn = mem().await;
        insert_continuation(&conn, NewContinuation {
            id: "c1", source_session_id: "s1", target_chat_id: 42, target_thread_id: None,
            instruction: "x", scheduled_at: "2020-01-01T00:00:00+00:00",
            deadline_at: "2999-01-01T00:00:00+00:00", check_in_secs: 90, budget_usd: 5.0,
            created_at: "2020-01-01T00:00:00+00:00",
        }).await.unwrap();
        mark_continuation_running(&conn, "c1", "fork-1", "2020-01-01T00:00:01+00:00", "/log").await.unwrap();
        rearm_continuation(&conn, "c1", "2020-01-01T00:05:00+00:00", 120, 0.03).await.unwrap();

        let due = fetch_due_continuations(&conn, "2999-01-01T00:00:00+00:00").await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].hop_count, 1);
        assert_eq!(due[0].check_in_secs, 120);
        assert!((due[0].cost_usd_accum - 0.03).abs() < 1e-9);
    }

    #[tokio::test]
    async fn report_sets_deliverable_success() {
        let conn = mem().await;
        insert_continuation(&conn, NewContinuation {
            id: "c1", source_session_id: "s1", target_chat_id: 42, target_thread_id: None,
            instruction: "x", scheduled_at: "2020-01-01T00:00:00+00:00",
            deadline_at: "2999-01-01T00:00:00+00:00", check_in_secs: 90, budget_usd: 5.0,
            created_at: "2020-01-01T00:00:00+00:00",
        }).await.unwrap();
        finalize_continuation_report(&conn, "c1", r#"{"content":"done","attachments":null}"#, "ran", 0.02, "2020-01-01T00:01:00+00:00").await.unwrap();

        let (status, dr, ds, dj): (String, i64, String, Option<String>) = conn.query_one(
            "SELECT status, delivery_required, delivery_status, delivery_json FROM async_runs WHERE id='c1'",
            crate::params![],
            |r| Ok((r.get::<String>(0)?, r.get::<i64>(1)?, r.get::<String>(2)?, r.get::<Option<String>>(3)?)),
        ).await.unwrap();
        assert_eq!(status, "success");
        assert_eq!(dr, 1);
        assert_eq!(ds, "pending");
        assert!(dj.unwrap().contains("done"));
    }

    #[tokio::test]
    async fn abandon_is_silent_success_no_delivery() {
        let conn = mem().await;
        insert_continuation(&conn, NewContinuation {
            id: "c1", source_session_id: "s1", target_chat_id: 42, target_thread_id: None,
            instruction: "x", scheduled_at: "2020-01-01T00:00:00+00:00",
            deadline_at: "2999-01-01T00:00:00+00:00", check_in_secs: 90, budget_usd: 5.0,
            created_at: "2020-01-01T00:00:00+00:00",
        }).await.unwrap();
        finalize_continuation_silent(&conn, "c1", "already handled", 0.01, "2020-01-01T00:01:00+00:00").await.unwrap();
        let (status, dr): (String, i64) = conn.query_one(
            "SELECT status, delivery_required FROM async_runs WHERE id='c1'",
            crate::params![], |r| Ok((r.get::<String>(0)?, r.get::<i64>(1)?)),
        ).await.unwrap();
        assert_eq!(status, "success");
        assert_eq!(dr, 0);
    }

    #[tokio::test]
    async fn cancel_only_scheduled_in_scope() {
        let conn = mem().await;
        insert_continuation(&conn, NewContinuation {
            id: "c1", source_session_id: "s1", target_chat_id: 42, target_thread_id: None,
            instruction: "x", scheduled_at: "2020-01-01T00:00:00+00:00",
            deadline_at: "2999-01-01T00:00:00+00:00", check_in_secs: 90, budget_usd: 5.0,
            created_at: "2020-01-01T00:00:00+00:00",
        }).await.unwrap();
        // wrong chat: no-op
        let n0 = cancel_continuation(&conn, "c1", 999, None, "2020-01-01T00:02:00+00:00").await.unwrap();
        assert_eq!(n0, 0);
        // right scope: cancelled
        let n1 = cancel_continuation(&conn, "c1", 42, None, "2020-01-01T00:02:00+00:00").await.unwrap();
        assert_eq!(n1, 1);
        // already terminal: no-op
        let n2 = cancel_continuation(&conn, "c1", 42, None, "2020-01-01T00:03:00+00:00").await.unwrap();
        assert_eq!(n2, 0);
    }
}
```

(Adapt `mem()`/`open_memory_db` and `Row::get` to the exact helpers used by the existing async_runs tests — check the file first.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo nextest run -p right-agent continuation_tests`
Expected: FAIL (functions/structs not defined).

- [ ] **Step 3: Implement the API**

Add to `crates/right-agent/src/async_runs.rs` (uses the same `Connection`/`params!` patterns already in the file):

```rust
#[derive(Debug, Clone, Copy)]
pub struct NewContinuation<'a> {
    pub id: &'a str,
    pub source_session_id: &'a str,
    pub target_chat_id: i64,
    pub target_thread_id: Option<i64>,
    pub instruction: &'a str,
    pub scheduled_at: &'a str,
    pub deadline_at: &'a str,
    pub check_in_secs: i64,
    pub budget_usd: f64,
    pub created_at: &'a str,
}

#[derive(Debug, Clone)]
pub struct ContinuationRow {
    pub id: String,
    pub source_session_id: String,
    pub target_chat_id: i64,
    pub target_thread_id: Option<i64>,
    pub instruction: String,
    pub deadline_at: String,
    pub check_in_secs: i64,
    pub budget_usd: f64,
    pub cost_usd_accum: f64,
    pub hop_count: i64,
}

/// Insert a scheduled continuation. `run_session_id` is initialised to the row
/// id (NOT NULL); each fire overwrites it with that hop's fresh fork session id.
pub async fn insert_continuation(
    conn: &Connection,
    c: NewContinuation<'_>,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, run_session_id, source_session_id, target_chat_id, target_thread_id,
            status, instruction, scheduled_at, deadline_at, check_in_secs, budget_usd,
            cost_usd_accum, hop_count, delivery_required, delivery_status, created_at, updated_at
         ) VALUES (
            ?1, 'continuation', ?1, ?2, ?3, ?4,
            'scheduled', ?5, ?6, ?7, ?8, ?9,
            0, 0, 0, 'none', ?10, ?10
         )",
        params![
            c.id, c.source_session_id, c.target_chat_id, c.target_thread_id,
            c.instruction, c.scheduled_at, c.deadline_at, c.check_in_secs, c.budget_usd,
            c.created_at,
        ],
    )
    .await?;
    Ok(())
}

/// Continuation rows due to fire (`scheduled` and `scheduled_at <= now`).
pub async fn fetch_due_continuations(
    conn: &Connection,
    now_rfc3339: &str,
) -> Result<Vec<ContinuationRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, source_session_id, target_chat_id, target_thread_id, \
                COALESCE(instruction, ''), COALESCE(deadline_at, ''), check_in_secs, \
                budget_usd, cost_usd_accum, hop_count \
         FROM async_runs \
         WHERE kind = 'continuation' AND status = 'scheduled' AND scheduled_at <= ?1 \
         ORDER BY scheduled_at ASC",
    )?;
    stmt.query_map(params![now_rfc3339], |row| {
        Ok(ContinuationRow {
            id: row.get::<String>(0)?,
            source_session_id: row.get::<String>(1)?,
            target_chat_id: row.get::<i64>(2)?,
            target_thread_id: row.get::<Option<i64>>(3)?,
            instruction: row.get::<String>(4)?,
            deadline_at: row.get::<String>(5)?,
            check_in_secs: row.get::<i64>(6)?,
            budget_usd: row.get::<f64>(7)?,
            cost_usd_accum: row.get::<f64>(8)?,
            hop_count: row.get::<i64>(9)?,
        })
    })
    .await?
    .collect()
}

/// Transition a due row to running for this hop, stamping the fork session id.
/// Returns rows affected (0 if it was cancelled/superseded between scan and fire).
pub async fn mark_continuation_running(
    conn: &Connection,
    id: &str,
    fork_session_id: &str,
    started_at: &str,
    log_path: &str,
) -> Result<usize, DbError> {
    conn.execute(
        "UPDATE async_runs SET status='running', run_session_id=?2, started_at=?3, \
                log_path=?4, updated_at=?3 \
         WHERE id=?1 AND kind='continuation' AND status='scheduled'",
        params![id, fork_session_id, started_at, log_path],
    )
    .await
}

/// Re-arm the same row: back to scheduled with a new fire time and cadence,
/// accumulate this hop's cost, bump the hop counter.
pub async fn rearm_continuation(
    conn: &Connection,
    id: &str,
    next_scheduled_at: &str,
    check_in_secs: i64,
    hop_cost_usd: f64,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE async_runs SET status='scheduled', scheduled_at=?2, check_in_secs=?3, \
                cost_usd_accum = cost_usd_accum + ?4, hop_count = hop_count + 1, updated_at=?2 \
         WHERE id=?1 AND kind='continuation'",
        params![id, next_scheduled_at, check_in_secs, hop_cost_usd],
    )
    .await?;
    Ok(())
}

/// Finalise as a deliverable success: the existing delivery loop will relay it.
pub async fn finalize_continuation_report(
    conn: &Connection,
    id: &str,
    delivery_json: &str,
    run_note: &str,
    hop_cost_usd: f64,
    finished_at: &str,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE async_runs SET status='success', finished_at=?5, run_note=?3, \
                delivery_json=?2, delivery_required=1, delivery_status='pending', \
                cost_usd_accum = cost_usd_accum + ?4, updated_at=?5 \
         WHERE id=?1 AND kind='continuation'",
        params![id, delivery_json, run_note, hop_cost_usd, finished_at],
    )
    .await?;
    Ok(())
}

/// Finalise silently (agent judged it already handled / moot): success, no delivery.
pub async fn finalize_continuation_silent(
    conn: &Connection,
    id: &str,
    reason: &str,
    hop_cost_usd: f64,
    finished_at: &str,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE async_runs SET status='success', finished_at=?4, run_note=?2, \
                delivery_required=0, delivery_status='none', \
                cost_usd_accum = cost_usd_accum + ?3, updated_at=?4 \
         WHERE id=?1 AND kind='continuation'",
        params![id, reason, hop_cost_usd, finished_at],
    )
    .await?;
    Ok(())
}

/// Finalise as a failure WITH a delivered notice (never leave the user silent).
pub async fn finalize_continuation_failed(
    conn: &Connection,
    id: &str,
    delivery_json: &str,
    run_note: &str,
    hop_cost_usd: f64,
    finished_at: &str,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE async_runs SET status='failed', finished_at=?5, run_note=?3, \
                delivery_json=?2, delivery_required=1, delivery_status='pending', \
                cost_usd_accum = cost_usd_accum + ?4, updated_at=?5 \
         WHERE id=?1 AND kind='continuation'",
        params![id, delivery_json, run_note, hop_cost_usd, finished_at],
    )
    .await?;
    Ok(())
}

/// Cancel a scheduled continuation in the given scope. Returns rows affected.
pub async fn cancel_continuation(
    conn: &Connection,
    id: &str,
    chat_id: i64,
    thread_id: Option<i64>,
    now: &str,
) -> Result<usize, DbError> {
    conn.execute(
        "UPDATE async_runs SET status='cancelled', updated_at=?4 \
         WHERE id=?1 AND kind='continuation' AND status='scheduled' \
           AND target_chat_id=?2 AND target_thread_id IS ?3",
        params![id, chat_id, thread_id, now],
    )
    .await
}

/// List scheduled continuations in the given scope.
pub async fn list_continuations(
    conn: &Connection,
    chat_id: i64,
    thread_id: Option<i64>,
) -> Result<Vec<ContinuationRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, source_session_id, target_chat_id, target_thread_id, \
                COALESCE(instruction, ''), COALESCE(deadline_at, ''), check_in_secs, \
                budget_usd, cost_usd_accum, hop_count \
         FROM async_runs \
         WHERE kind='continuation' AND status='scheduled' \
           AND target_chat_id=?1 AND target_thread_id IS ?2 \
         ORDER BY scheduled_at ASC",
    )?;
    stmt.query_map(params![chat_id, thread_id], |row| {
        Ok(ContinuationRow {
            id: row.get::<String>(0)?,
            source_session_id: row.get::<String>(1)?,
            target_chat_id: row.get::<i64>(2)?,
            target_thread_id: row.get::<Option<i64>>(3)?,
            instruction: row.get::<String>(4)?,
            deadline_at: row.get::<String>(5)?,
            check_in_secs: row.get::<i64>(6)?,
            budget_usd: row.get::<f64>(7)?,
            cost_usd_accum: row.get::<f64>(8)?,
            hop_count: row.get::<i64>(9)?,
        })
    })
    .await?
    .collect()
}

/// Active root session id for a (chat, thread), or None. Used to resolve the
/// foreground session the continuation must fork.
pub async fn active_root_session_id(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<String>, DbError> {
    let res = conn
        .query_one(
            "SELECT root_session_id FROM sessions \
             WHERE chat_id=?1 AND thread_id=?2 AND is_active=1 LIMIT 1",
            params![chat_id, thread_id],
            |r| r.get::<String>(0),
        )
        .await;
    match res {
        Ok(s) => Ok(Some(s)),
        Err(DbError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}
```

Confirm `conn.prepare(...).query_map(...)` and `Row::get::<T>(i)` exactly match the forms already used in `async_runs.rs` (the file shows `params!` + `conn.execute`; check whether reads use `query_one`/`prepare`+`query_map`). Adjust to the file's actual idiom. `target_thread_id IS ?3` matches NULL when bound `None` (SQLite `IS`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo nextest run -p right-agent continuation_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/async_runs.rs
git commit -m "feat(right-agent): continuation async_runs DB API"
```

---

## Task 3: Continuation output schema + parse types

**Files:**
- Modify: `crates/right-codegen/src/agent_def.rs` (add `CONTINUATION_SCHEMA_JSON`; re-export from `right_codegen` like `BG_CONTINUATION_SCHEMA_JSON`).
- Create: `crates/bot/src/continuation.rs` (decision types + parse).
- Modify: `crates/bot/src/lib.rs` (add `mod continuation;`).
- Test: in `crates/bot/src/continuation.rs` `#[cfg(test)]`.

The hop must emit exactly one of three decisions. Schema (mirrors the style of `BG_CONTINUATION_SCHEMA_JSON`):

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_report() {
        let v = r#"{"decision":{"action":"report","content":"all done","attachments":null},"run_note":"collected"}"#;
        let out: ContinuationOutput = serde_json::from_str(v).unwrap();
        match out.decision {
            ContinuationDecision::Report { content, .. } => assert_eq!(content, "all done"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_wait() {
        let v = r#"{"decision":{"action":"wait","check_in_seconds":120,"reason":"still running"},"run_note":"polled"}"#;
        let out: ContinuationOutput = serde_json::from_str(v).unwrap();
        match out.decision {
            ContinuationDecision::Wait { check_in_seconds, .. } => assert_eq!(check_in_seconds, 120),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_abandon() {
        let v = r#"{"decision":{"action":"abandon","reason":"user changed direction"},"run_note":"moot"}"#;
        let out: ContinuationOutput = serde_json::from_str(v).unwrap();
        assert!(matches!(out.decision, ContinuationDecision::Abandon { .. }));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p bot continuation::tests`
Expected: FAIL (module/types absent).

- [ ] **Step 3: Implement schema + types**

In `crates/right-codegen/src/agent_def.rs` (next to `BG_CONTINUATION_SCHEMA_JSON`):

```rust
/// Structured output for a continuation hop: exactly one of report/wait/abandon.
pub const CONTINUATION_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"decision":{"type":"object","oneOf":[{"properties":{"action":{"const":"report"},"content":{"type":"string","minLength":1},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["action","content"]},{"properties":{"action":{"const":"wait"},"check_in_seconds":{"type":"integer","minimum":1},"reason":{"type":"string"}},"required":["action","check_in_seconds"]},{"properties":{"action":{"const":"abandon"},"reason":{"type":"string"}},"required":["action","reason"]}]},"run_note":{"type":"string"}},"required":["decision","run_note"]}"#;
```

Ensure it's re-exported wherever `BG_CONTINUATION_SCHEMA_JSON` is (`pub use` in the crate root — check `crates/right-codegen/src/lib.rs`).

Create `crates/bot/src/continuation.rs` (start with types; execution added in Task 5):

```rust
//! Agent self-continuation: scheduled, idempotent, bounded resume of a
//! foreground session to finish/collect out-of-turn async work.

use crate::cron::CronNotify; // reuse the notify shape for delivery_json

/// The hop's structured decision.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum ContinuationDecision {
    /// Work is done (or give-up): deliver this to the user.
    Report {
        content: String,
        #[serde(default)]
        attachments: Option<Vec<crate::telegram::attachments::OutboundAttachment>>,
    },
    /// Not done yet: re-arm to check again.
    Wait {
        check_in_seconds: i64,
        #[serde(default)]
        reason: Option<String>,
    },
    /// Already handled elsewhere / moot: finish silently.
    Abandon { reason: String },
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ContinuationOutput {
    pub(crate) decision: ContinuationDecision,
    pub(crate) run_note: String,
}
```

Confirm the exact path/type of `OutboundAttachment` used by `CronNotify` (the research shows `CronNotify { content, attachments: Option<Vec<OutboundAttachment>> }` in `crates/bot/src/cron.rs`); import the SAME `OutboundAttachment` type so `delivery_json` serialised from a `Report` matches what delivery expects. Add `mod continuation;` to `crates/bot/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p bot continuation::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-codegen/src/agent_def.rs crates/right-codegen/src/lib.rs crates/bot/src/continuation.rs crates/bot/src/lib.rs
git commit -m "feat: continuation output schema + decision types"
```

---

## Task 4: Continuation resume-prompt builder

**Files:**
- Modify: `crates/bot/src/continuation.rs`
- Test: same file `#[cfg(test)]`.

The hop's prompt wraps the agent's stored instruction in a SYSTEM_NOTICE, states the idempotency directive, and on the final hop forbids `wait`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn prompt_mentions_idempotency_and_final() {
    let p = build_continuation_resume_prompt("check browser-use sessions", false, "tok");
    assert!(p.contains("check browser-use sessions"));
    assert!(p.to_lowercase().contains("already"));
    assert!(p.contains("wait")); // non-final allows wait

    let f = build_continuation_resume_prompt("check browser-use sessions", true, "tok");
    assert!(f.to_lowercase().contains("final"));
    assert!(f.to_lowercase().contains("must")); // must report
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p bot continuation::tests::prompt_mentions_idempotency_and_final`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
pub(crate) fn build_continuation_resume_prompt(
    instruction: &str,
    is_final: bool,
    notice_token: &str,
) -> String {
    let finality = if is_final {
        "This is the FINAL check — the wait budget/deadline is exhausted. You MUST \
         choose action=\"report\" with a user-facing summary of what you have (even if \
         the work did not finish). action=\"wait\" is not allowed on this hop."
    } else {
        "If the awaited work is still in progress, choose action=\"wait\" with a sensible \
         check_in_seconds to be resumed again."
    };
    let body = format!(
        "You are a scheduled continuation of this conversation, resumed to follow up on \
work you started earlier that finishes out of turn.\n\
\n\
Your standing instruction for this follow-up:\n\
{instruction}\n\
\n\
You can see the full conversation since you armed this continuation. Before doing \
anything, CHECK whether this was ALREADY handled — e.g. a later message already \
reported the result, the user redirected, or the task became moot. If so, choose \
action=\"abandon\" with a short reason and stay silent (no message is sent).\n\
\n\
Otherwise: if the work is finished, choose action=\"report\" with the user-facing \
content. {finality}\n\
\n\
Emit exactly one decision via the structured output. Only action=\"report\" sends a \
message to the user; \"wait\" and \"abandon\" are silent."
    );
    crate::cc::system_notice::wrap_system_notice(notice_token, &body)
}
```

Confirm `wrap_system_notice` path (research: `crate::cc::system_notice::wrap_system_notice(token, &body)`).

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p bot continuation::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/continuation.rs
git commit -m "feat: continuation resume-prompt builder"
```

---

## Task 5: Continuation hop execution + completion mapping

**Files:**
- Modify: `crates/bot/src/continuation.rs`
- Test: deferred to the live integration test in Task 11 (this is process-spawning glue; unit-test the pure decision→DB mapping helper here).

This mirrors `crates/bot/src/background.rs` (`spawn_background_continuation` + `complete_background_run`) but: uses `CONTINUATION_SCHEMA_JSON`, sets `max_budget_usd` per hop, computes `is_final`, and maps the parsed `ContinuationOutput` to the Task 2 finalisers. Extract the pure mapping into a testable function.

- [ ] **Step 1: Write the failing test (pure mapping)**

```rust
#[derive(Debug, PartialEq)]
pub(crate) enum HopOutcome {
    Report { delivery_json: String, run_note: String },
    Wait { check_in_secs: i64 },
    Abandon { reason: String },
}

#[test]
fn final_hop_wait_is_coerced_to_report() {
    let out = ContinuationOutput {
        decision: ContinuationDecision::Wait { check_in_seconds: 60, reason: Some("still running".into()) },
        run_note: "polled".into(),
    };
    let outcome = resolve_hop_outcome(out, /*is_final=*/ true).unwrap();
    match outcome {
        HopOutcome::Report { run_note, .. } => assert!(run_note.contains("still running") || !run_note.is_empty()),
        _ => panic!("final wait must coerce to report"),
    }
}

#[test]
fn nonfinal_wait_clamps_check_in() {
    let out = ContinuationOutput {
        decision: ContinuationDecision::Wait { check_in_seconds: 1, reason: None },
        run_note: "polled".into(),
    };
    let outcome = resolve_hop_outcome(out, false).unwrap();
    assert_eq!(outcome, HopOutcome::Wait { check_in_secs: right_agent::async_runs::CONTINUE_MIN_CHECK_IN_SECS });
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p bot continuation::tests`
Expected: FAIL.

- [ ] **Step 3: Implement mapping + execution**

Pure mapping:

```rust
pub(crate) fn resolve_hop_outcome(
    out: ContinuationOutput,
    is_final: bool,
) -> Result<HopOutcome, String> {
    match out.decision {
        ContinuationDecision::Report { content, attachments } => {
            let notify = crate::cron::CronNotify { content, attachments };
            let delivery_json = serde_json::to_string(&notify).map_err(|e| e.to_string())?;
            Ok(HopOutcome::Report { delivery_json, run_note: out.run_note })
        }
        ContinuationDecision::Abandon { reason } => Ok(HopOutcome::Abandon { reason }),
        ContinuationDecision::Wait { check_in_seconds, reason } => {
            if is_final {
                // Coerce: the agent ignored the "final" directive. Synthesize a report.
                let msg = reason.unwrap_or_else(|| {
                    "I stopped waiting on the background task — it had not finished in time.".into()
                });
                let notify = crate::cron::CronNotify { content: msg, attachments: None };
                let delivery_json = serde_json::to_string(&notify).map_err(|e| e.to_string())?;
                Ok(HopOutcome::Report { delivery_json, run_note: out.run_note })
            } else {
                let clamped = check_in_seconds.max(right_agent::async_runs::CONTINUE_MIN_CHECK_IN_SECS);
                Ok(HopOutcome::Wait { check_in_secs: clamped })
            }
        }
    }
}
```

Execution (mirror `background.rs` for process spawn + reader; cite those functions, do not reinvent the streaming reader). Signature and body:

```rust
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) struct ContinuationHop {
    pub row: right_agent::async_runs::ContinuationRow,
    pub agent_dir: PathBuf,
    pub agent_name: String,
    pub model: Option<String>,
    pub ssh_config_path: Option<PathBuf>,
    pub internal_client: Arc<right_mcp::internal_client::InternalClient>,
    pub resolved_sandbox: Option<String>,
    pub upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    pub session_guard: tokio::sync::OwnedMutexGuard<()>,
    pub debug: Arc<std::sync::atomic::AtomicBool>,
    pub notice_token: String,
}

/// Fire one continuation hop: fork the source session, run CC with the
/// continuation schema, map the decision to a DB finalise/re-arm.
pub(crate) async fn fire_continuation_hop(hop: ContinuationHop) {
    let row = hop.row;
    let now = chrono::Utc::now();
    // Ceiling check → is this the last hop?
    let deadline = chrono::DateTime::parse_from_rfc3339(&row.deadline_at)
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| now);
    let remaining_budget =
        (row.budget_usd - row.cost_usd_accum).max(0.0);
    let next_fire = now + chrono::Duration::seconds(row.check_in_secs);
    let is_final = row.hop_count + 1 >= right_agent::async_runs::CONTINUE_MAX_HOPS
        || next_fire >= deadline
        || remaining_budget <= 0.0;

    let fork_session_id = uuid::Uuid::new_v4().to_string();
    let log_path = crate::background::bg_log_path(&hop.agent_dir, &fork_session_id);
    let started_at = now.to_rfc3339();

    // Transition scheduled → running. If 0 rows, it was cancelled/superseded — stop.
    let conn = match right_db::open_connection(&hop.agent_dir, false).await {
        Ok(c) => c,
        Err(e) => { tracing::error!("continuation: open db failed: {e:#}"); return; }
    };
    match right_agent::async_runs::mark_continuation_running(
        &conn, &row.id, &fork_session_id, &started_at, &log_path.to_string_lossy(),
    ).await {
        Ok(0) => { tracing::info!(id=%row.id, "continuation no longer scheduled; skip"); return; }
        Ok(_) => {}
        Err(e) => { tracing::error!("continuation: mark running failed: {e:#}"); return; }
    }

    let per_hop_budget = remaining_budget.min(right_agent::async_runs::CONTINUE_PER_HOP_BUDGET_USD).max(0.01);
    let prompt = build_continuation_resume_prompt(&row.instruction, is_final, &hop.notice_token);

    let invocation = crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: Some(crate::cc::invocation::mcp_config_path(
            hop.ssh_config_path.as_deref(),
            &hop.agent_dir,
        )),
        json_schema: Some(right_codegen::CONTINUATION_SCHEMA_JSON.into()),
        output_format: crate::cc::invocation::OutputFormat::StreamJson,
        model: hop.model.clone(),
        max_budget_usd: Some(per_hop_budget),
        max_turns: None,
        resume_session_id: Some(row.source_session_id.clone()),
        new_session_id: Some(fork_session_id.clone()),
        fork_session: true,
        allowed_tools: vec![],
        disallowed_tools: crate::cc::invocation::disallow_foreground_only_tools(
            crate::cc::invocation::baseline_disallowed_tools(),
        ),
        extra_args: vec![],
        prompt: Some(prompt),
        debug_flag: Some(Arc::clone(&hop.debug)),
    };

    // Spawn + collect stdout exactly as background.rs does (process group, reader
    // task, HANDOFF_INIT wait). Reuse crate::background helpers where pub(crate);
    // otherwise replicate the spawn/read block from background.rs::complete_background_run.
    // The collected stdout lines feed parse below.
    let lines: Vec<String> = match run_forked_cc_collect(&invocation, &hop, &log_path).await {
        Ok(l) => l,
        Err(e) => {
            finalize_hop_failure(&conn, &row.id, &format!("continuation hop failed to run: {e}")).await;
            return;
        }
    };

    // Parse cost + the terminal result line.
    let (result_text, _turns, hop_cost) =
        crate::cron::parse_result_stats(&lines).unwrap_or((String::new(), 0, 0.0));

    let finished_at = chrono::Utc::now().to_rfc3339();
    let parsed: Result<ContinuationOutput, _> = serde_json::from_str(&result_text);
    let outcome = match parsed {
        Ok(out) => resolve_hop_outcome(out, is_final),
        Err(e) => Err(format!("invalid continuation output: {e}")),
    };

    match outcome {
        Ok(HopOutcome::Report { delivery_json, run_note }) => {
            let _ = right_agent::async_runs::finalize_continuation_report(
                &conn, &row.id, &delivery_json, &run_note, hop_cost, &finished_at,
            ).await;
        }
        Ok(HopOutcome::Abandon { reason }) => {
            let _ = right_agent::async_runs::finalize_continuation_silent(
                &conn, &row.id, &reason, hop_cost, &finished_at,
            ).await;
        }
        Ok(HopOutcome::Wait { check_in_secs }) => {
            let next = (chrono::Utc::now() + chrono::Duration::seconds(check_in_secs)).to_rfc3339();
            let _ = right_agent::async_runs::rearm_continuation(
                &conn, &row.id, &next, check_in_secs, hop_cost,
            ).await;
        }
        Err(msg) => {
            finalize_hop_failure(&conn, &row.id, &msg).await;
        }
    }
}

async fn finalize_hop_failure(conn: &right_db::Connection, id: &str, msg: &str) {
    let notify = crate::cron::CronNotify {
        content: format!("A background follow-up could not complete: {msg}"),
        attachments: None,
    };
    let dj = serde_json::to_string(&notify).unwrap_or_else(|_| "{}".into());
    let finished_at = chrono::Utc::now().to_rfc3339();
    if let Err(e) = right_agent::async_runs::finalize_continuation_failed(
        conn, id, &dj, msg, 0.0, &finished_at,
    ).await {
        tracing::error!("continuation: finalize_failed write failed: {e:#}");
    }
}
```

`run_forked_cc_collect` is the spawn+stream-read helper: if `crate::background` exposes a reusable spawn/reader, call it; otherwise lift the process-group spawn + reader-task + `HANDOFF_INIT_TIMEOUT` block from `background.rs::complete_background_run` into a shared `pub(crate)` helper and call it from both. Note in the commit which path you took. Make `crate::cron::CronNotify` and `crate::cron::parse_result_stats` `pub(crate)` if not already.

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo nextest run -p bot continuation`
Expected: PASS (pure-mapping tests). Then `devenv shell -- cargo build -p bot` to confirm the execution compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/continuation.rs crates/bot/src/background.rs crates/bot/src/cron.rs
git commit -m "feat: continuation hop execution + decision mapping"
```

---

## Task 6: Reconciler fires due continuations

**Files:**
- Modify: `crates/bot/src/cron.rs` (add `fire_due_continuations`, call it from `reconcile_jobs`; it already has `conn`, the spawn params, `session_locks`, and `triggered_handles`).
- Test: live integration in Task 11; here, a focused unit test on due selection already exists (Task 2 `fetch_due_continuations`). Add a guard test that a non-due/cancelled row is skipped (covered in Task 2). No new unit test required for the wiring; verify by build + Task 11.

- [ ] **Step 1: Implement `fire_due_continuations`**

Add to `crates/bot/src/cron.rs` (mirror `spawn_then_continuation`'s lock + spawn shape; spawn each hop as a detached task pushed to `triggered_handles` so the 5s loop never blocks):

```rust
#[allow(clippy::too_many_arguments)]
async fn fire_due_continuations(
    triggered_handles: &mut Vec<tokio::task::JoinHandle<()>>,
    conn: &right_db::Connection,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: Option<&str>,
    ssh_config_path: Option<&std::path::Path>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<&str>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    session_locks: &crate::telegram::SessionLocks,
    debug: Arc<std::sync::atomic::AtomicBool>,
    notice_token: &str,
) {
    let now = chrono::Utc::now().to_rfc3339();
    let due = match right_agent::async_runs::fetch_due_continuations(conn, &now).await {
        Ok(d) => d,
        Err(e) => { tracing::error!("continuation: fetch_due failed: {e:#}"); return; }
    };
    for row in due {
        // Acquire the per-session lock on the SOURCE session (we fork/--resume it),
        // serialising against live foreground turns and delivery.
        let entry = session_locks
            .entry(row.source_session_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let session_guard = entry.lock_owned().await;

        let hop = crate::continuation::ContinuationHop {
            row,
            agent_dir: agent_dir.to_path_buf(),
            agent_name: agent_name.to_string(),
            model: model.map(|s| s.to_owned()),
            ssh_config_path: ssh_config_path.map(|p| p.to_path_buf()),
            internal_client: Arc::clone(internal_client),
            resolved_sandbox: resolved_sandbox.map(|s| s.to_owned()),
            upgrade_lock: Arc::clone(&upgrade_lock),
            session_guard,
            debug: Arc::clone(&debug),
            notice_token: notice_token.to_string(),
        };
        triggered_handles.push(tokio::spawn(async move {
            crate::continuation::fire_continuation_hop(hop).await;
        }));
    }
}
```

Call it from `reconcile_jobs` right after the Immediate-spec firing block (`crates/bot/src/cron.rs` ~line 2258). The `reconcile_jobs` signature already carries `conn`, `agent_dir`, `agent_name`, `model`, `ssh_config_path`, `internal_client`, `resolved_sandbox`, `upgrade_lock`, `session_locks`, `debug` (verify; thread any missing one through from `run_cron_task`). The notice token is the per-agent token used by `wrap_system_notice`; obtain it the same way cron/worker do (verify the source — likely loaded from the agent's config/db; reuse that accessor).

```rust
    fire_due_continuations(
        triggered_handles, conn, &agent_dir, &agent_name,
        model.as_deref(), ssh_config_path.as_deref(), &internal_client,
        resolved_sandbox.as_deref(), Arc::clone(&upgrade_lock),
        &session_locks, Arc::clone(&debug), &notice_token,
    )
    .await;
```

- [ ] **Step 2: Build**

Run: `devenv shell -- cargo build -p bot`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat: reconciler fires due continuations"
```

---

## Task 7: MCP tools — continue_later / continue_cancel / continue_list

**Files:**
- Modify: `crates/right-mcp/src/internal_client.rs` (tool-name constants).
- Modify: `crates/right/src/right_backend.rs` (params structs, handlers, `tools_list`, `tools_call`).
- Test: `crates/right/src/right_backend.rs` `#[cfg(test)]` (handler logic against a temp agent DB) — or a dedicated `right_backend_continuation_tests.rs`.

- [ ] **Step 1: Add tool-name constants**

In `crates/right-mcp/src/internal_client.rs` (next to `SEND_MESSAGE_TOOL`):

```rust
pub const CONTINUE_LATER_TOOL: &str = "continue_later";
pub const CONTINUE_CANCEL_TOOL: &str = "continue_cancel";
pub const CONTINUE_LIST_TOOL: &str = "continue_list";
pub const CONTINUE_LATER_MCP_TOOL: &str = "mcp__right__continue_later";
pub const CONTINUE_CANCEL_MCP_TOOL: &str = "mcp__right__continue_cancel";
pub const CONTINUE_LIST_MCP_TOOL: &str = "mcp__right__continue_list";
```

- [ ] **Step 2: Write the failing test**

```rust
#[tokio::test]
async fn continue_later_arms_in_scope() {
    // Build a RightBackend over a temp agents dir with one agent DB migrated,
    // with an active session row for (chat 42, thread 0) and a registered
    // foreground invocation scope. (Mirror how existing right_backend tests
    // set up self.progress + an agent DB.)
    let (backend, agent, _tmp, invocation_id) = test_backend_with_foreground_scope(42, 0, "sess-1").await;
    let args = serde_json::json!({
        "instruction": "check browser-use",
        "check_in_seconds": 90
    });
    let ctx = crate::progress::ToolCallContext { invocation_id: Some(invocation_id) };
    let res = backend.tools_call(&agent, /*agent_dir*/&_tmp.path().join(&agent), "continue_later", args, ctx).await.unwrap();
    // success result carries a continuation_id
    let text = result_text(&res);
    assert!(text.contains("continuation_id"));

    // a continuation row now exists, scheduled, source_session_id resolved
    let conn = right_db::open_connection(&_tmp.path().join(&agent), false).await.unwrap();
    let rows = right_agent::async_runs::list_continuations(&conn, 42, None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source_session_id, "sess-1");
}

#[tokio::test]
async fn continue_later_without_foreground_scope_errors() {
    let (backend, agent, tmp, _id) = test_backend_with_foreground_scope(42, 0, "sess-1").await;
    let ctx = crate::progress::ToolCallContext { invocation_id: None };
    let res = backend.tools_call(&agent, &tmp.path().join(&agent), "continue_later",
        serde_json::json!({"instruction":"x"}), ctx).await.unwrap();
    assert!(result_text(&res).contains("foreground"));
}
```

(Reuse/extend the existing right_backend test scaffolding for `self.progress` registration and a migrated agent DB. If none exists, add a minimal `test_backend_with_foreground_scope` helper that constructs `RightBackend::new`, registers a `Foreground` invocation with a `ConversationScope`, opens+migrates the agent DB, and inserts a `sessions` row.)

- [ ] **Step 3: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right continue_later`
Expected: FAIL.

- [ ] **Step 4: Implement params, handlers, registration**

Params (in `right_backend.rs`, near the handlers):

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ContinueLaterParams {
    /// What to do when you are resumed (e.g. "check the browser-use sessions and
    /// report findings, or wait if still running").
    pub(crate) instruction: String,
    /// Seconds until the first check. Clamped to a 30s floor; default 90.
    #[serde(default)]
    pub(crate) check_in_seconds: Option<i64>,
    /// Hard give-up after this many seconds (clamped to a 6h ceiling). Default 30m.
    #[serde(default)]
    pub(crate) give_up_after_seconds: Option<i64>,
    /// Cumulative USD budget across the whole wait (clamped). Default 5.
    #[serde(default)]
    pub(crate) max_budget_usd: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ContinueCancelParams {
    /// The continuation_id returned by continue_later.
    pub(crate) continuation_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ContinueListParams {}
```

Handlers (foreground-only; resolve scope via `self.progress.conversation_scope`, which enforces foreground and errors otherwise):

```rust
async fn call_continue_later(
    &self,
    agent_name: &str,
    agent_dir: &Path,
    context: &crate::progress::ToolCallContext,
    args: &serde_json::Value,
) -> Result<CallToolResult, anyhow::Error> {
    use right_agent::async_runs as ar;
    let params: ContinueLaterParams = match serde_json::from_value(args.clone()) {
        Ok(p) => p,
        Err(e) => return Ok(tool_error("invalid_argument", format!("invalid continue_later params: {e:#}"), None)),
    };
    let Some(invocation_id) = context.invocation_id.as_deref() else {
        return Ok(conversation_scope_unavailable());
    };
    let scope = match self.progress.conversation_scope(invocation_id).await {
        Ok(s) => s,
        Err(_) => return Ok(conversation_scope_unavailable()),
    };
    let conn_arc = self.get_conn(agent_name).await?;
    let conn = conn_arc.lock().await;
    let Some(source) = ar::active_root_session_id(&conn, scope.chat_id, scope.thread_id).await? else {
        return Ok(tool_error("no_active_session", "no active session to continue in this chat", None));
    };
    let now = chrono::Utc::now();
    let check_in = params.check_in_seconds.unwrap_or(ar::CONTINUE_DEFAULT_CHECK_IN_SECS)
        .clamp(ar::CONTINUE_MIN_CHECK_IN_SECS, ar::CONTINUE_MAX_WALLCLOCK_SECS);
    let give_up = params.give_up_after_seconds.unwrap_or(ar::CONTINUE_DEFAULT_DEADLINE_SECS)
        .clamp(check_in, ar::CONTINUE_MAX_WALLCLOCK_SECS);
    let budget = params.max_budget_usd.unwrap_or(ar::CONTINUE_DEFAULT_BUDGET_USD)
        .clamp(0.01, ar::CONTINUE_MAX_BUDGET_USD);
    let id = uuid::Uuid::new_v4().to_string();
    let scheduled_at = (now + chrono::Duration::seconds(check_in)).to_rfc3339();
    let deadline_at = (now + chrono::Duration::seconds(give_up)).to_rfc3339();
    let created_at = now.to_rfc3339();
    let eff_thread = (scope.thread_id != 0).then_some(scope.thread_id);
    ar::insert_continuation(&conn, ar::NewContinuation {
        id: &id, source_session_id: &source, target_chat_id: scope.chat_id,
        target_thread_id: eff_thread, instruction: &params.instruction,
        scheduled_at: &scheduled_at, deadline_at: &deadline_at,
        check_in_secs: check_in, budget_usd: budget, created_at: &created_at,
    }).await?;
    let _ = agent_dir; // agent_dir available if needed for logging
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::json!({
            "status": "armed",
            "continuation_id": id,
            "first_check_at": scheduled_at,
            "give_up_at": deadline_at
        }).to_string(),
    )]))
}

async fn call_continue_cancel(
    &self,
    agent_name: &str,
    context: &crate::progress::ToolCallContext,
    args: &serde_json::Value,
) -> Result<CallToolResult, anyhow::Error> {
    use right_agent::async_runs as ar;
    let params: ContinueCancelParams = match serde_json::from_value(args.clone()) {
        Ok(p) => p,
        Err(e) => return Ok(tool_error("invalid_argument", format!("invalid continue_cancel params: {e:#}"), None)),
    };
    let Some(invocation_id) = context.invocation_id.as_deref() else {
        return Ok(conversation_scope_unavailable());
    };
    let scope = match self.progress.conversation_scope(invocation_id).await {
        Ok(s) => s,
        Err(_) => return Ok(conversation_scope_unavailable()),
    };
    let conn_arc = self.get_conn(agent_name).await?;
    let conn = conn_arc.lock().await;
    let eff_thread = (scope.thread_id != 0).then_some(scope.thread_id);
    let n = ar::cancel_continuation(&conn, &params.continuation_id, scope.chat_id, eff_thread,
        &chrono::Utc::now().to_rfc3339()).await?;
    if n == 0 {
        return Ok(tool_error("not_found", "no scheduled continuation with that id in this chat", None));
    }
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::json!({"status":"cancelled","continuation_id":params.continuation_id}).to_string(),
    )]))
}

async fn call_continue_list(
    &self,
    agent_name: &str,
    context: &crate::progress::ToolCallContext,
) -> Result<CallToolResult, anyhow::Error> {
    use right_agent::async_runs as ar;
    let Some(invocation_id) = context.invocation_id.as_deref() else {
        return Ok(conversation_scope_unavailable());
    };
    let scope = match self.progress.conversation_scope(invocation_id).await {
        Ok(s) => s,
        Err(_) => return Ok(conversation_scope_unavailable()),
    };
    let conn_arc = self.get_conn(agent_name).await?;
    let conn = conn_arc.lock().await;
    let eff_thread = (scope.thread_id != 0).then_some(scope.thread_id);
    let rows = ar::list_continuations(&conn, scope.chat_id, eff_thread).await?;
    let items: Vec<_> = rows.iter().map(|r| serde_json::json!({
        "continuation_id": r.id,
        "instruction": r.instruction,
        "give_up_at": r.deadline_at,
        "check_in_secs": r.check_in_secs,
        "hops_done": r.hop_count
    })).collect();
    Ok(CallToolResult::success(vec![Content::text(serde_json::json!({"continuations": items}).to_string())]))
}
```

Add to `tools_list()` (after the `send_message` `Tool::new` entry):

```rust
Tool::new(
    right_mcp::internal_client::CONTINUE_LATER_TOOL,
    "Schedule yourself to resume THIS conversation later to finish/collect out-of-turn async work (e.g. a browser-use cloud session, a long external job). Use this instead of ending your turn 'waiting' — a finished turn is NOT auto-resumed. When resumed you see the full conversation and decide: report results, wait again, or stay silent if already handled. Returns continuation_id. Foreground-only.",
    schema_for_type::<ContinueLaterParams>(),
),
Tool::new(
    right_mcp::internal_client::CONTINUE_CANCEL_TOOL,
    "Cancel a scheduled continuation by continuation_id (e.g. the task finished out of band or the user changed direction). Foreground-only; scope-enforced.",
    schema_for_type::<ContinueCancelParams>(),
),
Tool::new(
    right_mcp::internal_client::CONTINUE_LIST_TOOL,
    "List your scheduled continuations for the current chat. Foreground-only; scope-enforced.",
    schema_for_type::<ContinueListParams>(),
),
```

Add to `tools_call()` dispatch:

```rust
right_mcp::internal_client::CONTINUE_LATER_TOOL => {
    self.call_continue_later(agent_name, agent_dir, &context, &args).await
}
right_mcp::internal_client::CONTINUE_CANCEL_TOOL => {
    self.call_continue_cancel(agent_name, &context, &args).await
}
right_mcp::internal_client::CONTINUE_LIST_TOOL => {
    self.call_continue_list(agent_name, &context).await
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p right continue_later`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-mcp/src/internal_client.rs crates/right/src/right_backend.rs
git commit -m "feat(right): continue_later/cancel/list MCP tools"
```

---

## Task 8: Mark continue_* foreground-only in the disallow chain

**Files:**
- Modify: `crates/bot/src/cc/invocation.rs`
- Test: `crates/bot/src/cc/invocation.rs` `#[cfg(test)]`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn disallow_chain_includes_continue_tools() {
    let d = disallow_foreground_only_tools(baseline_disallowed_tools());
    assert!(d.iter().any(|t| t == right_mcp::internal_client::CONTINUE_LATER_MCP_TOOL));
    assert!(d.iter().any(|t| t == right_mcp::internal_client::CONTINUE_CANCEL_MCP_TOOL));
    assert!(d.iter().any(|t| t == right_mcp::internal_client::CONTINUE_LIST_MCP_TOOL));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p bot disallow_chain_includes_continue_tools`
Expected: FAIL.

- [ ] **Step 3: Implement**

Add a disallow function and wire it into `disallow_foreground_only_tools_keep_learning` (so all non-foreground callsites — background, cron, reflection, delivery, AND continuation hops — exclude them):

```rust
pub(crate) fn disallow_continue_tools(mut tools: Vec<String>) -> Vec<String> {
    for tool_name in [
        right_mcp::internal_client::CONTINUE_LATER_MCP_TOOL,
        right_mcp::internal_client::CONTINUE_CANCEL_MCP_TOOL,
        right_mcp::internal_client::CONTINUE_LIST_MCP_TOOL,
    ] {
        if !tools.iter().any(|tool| tool == tool_name) {
            tools.push(tool_name.to_owned());
        }
    }
    tools
}
```

Update the composed chain:

```rust
pub(crate) fn disallow_foreground_only_tools_keep_learning(tools: Vec<String>) -> Vec<String> {
    disallow_continue_tools(disallow_thread_focus_set(disallow_forum_topic_tools(
        disallow_conversation_search(disallow_send_message(disallow_send_progress(tools))),
    )))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p bot disallow_chain_includes_continue_tools`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cc/invocation.rs
git commit -m "feat(bot): continue_* are foreground-only"
```

---

## Task 9: Delivery branches for kind='continuation'

**Files:**
- Modify: `crates/bot/src/async_delivery.rs`
- Test: `crates/bot/src/async_delivery.rs` `#[cfg(test)]` (the `format_async_yaml` instruction selection is pure-ish — test the match).

A continuation report already satisfies `fetch_pending_batch` (delivery_required=1, delivery_status='pending', status IN success/failed, delivery_json NOT NULL). No dedup at delivery (re-arm keeps one row). We only add correct wording + outbox subdir.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn continuation_uses_its_own_delivery_instruction() {
    assert_eq!(
        select_delivery_instruction("continuation", "success"),
        CONTINUATION_DELIVERY_INSTRUCTION_SUCCESS
    );
    assert_eq!(
        select_delivery_instruction("continuation", "failed"),
        CONTINUATION_DELIVERY_INSTRUCTION_FAILURE
    );
}
```

(Refactor the inline `match (kind, status)` in `format_async_yaml` into a `select_delivery_instruction(kind, status) -> &'static str` so it's unit-testable; call it from `format_async_yaml`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p bot continuation_uses_its_own_delivery_instruction`
Expected: FAIL.

- [ ] **Step 3: Implement**

Add constants near the other delivery instructions:

```rust
const CONTINUATION_DELIVERY_INSTRUCTION_SUCCESS: &str = "\
You are delivering the result of a follow-up you scheduled earlier (a continuation).
The `content` field below is the FINAL user-facing message — send it VERBATIM in your response.
Do NOT summarize, rephrase, or omit any part of the content.
You MAY prepend a short contextual intro (1 sentence max) so the message feels natural after the gap.
Re-emit any attachments in your reply's `attachments` array. `content` and an attachment `caption` are delivered as SEPARATE messages — never repeat the content text in a caption.

Here is the YAML report of the continuation:
";

const CONTINUATION_DELIVERY_INSTRUCTION_FAILURE: &str = "\
The follow-up you scheduled (a continuation) did not complete successfully. The
`content` field contains a platform-generated summary. Relay it to the user in
natural prose — you MAY rephrase lightly for flow, but keep all factual claims
intact. Do not invent details. Ignore the attachments field.

Here is the YAML report of the continuation:
";

fn select_delivery_instruction(kind: &str, status: &str) -> &'static str {
    match (kind, status) {
        ("continuation", "failed") => CONTINUATION_DELIVERY_INSTRUCTION_FAILURE,
        ("continuation", _) => CONTINUATION_DELIVERY_INSTRUCTION_SUCCESS,
        ("background", "failed") => BACKGROUND_DELIVERY_INSTRUCTION_FAILURE,
        ("background", _) => BACKGROUND_DELIVERY_INSTRUCTION_SUCCESS,
        (_, "failed") => DELIVERY_INSTRUCTION_FAILURE,
        _ => CRON_DELIVERY_INSTRUCTION_SUCCESS,
    }
}
```

Replace the inline match in `format_async_yaml` with `let instruction = select_delivery_instruction(pending.kind.as_str(), pending.status.as_str());`. In the YAML-structure branch, add a `continuation_result:` label arm (mirror the `background_result:` arm — `label` = `"continuation"`). In the outbox-cleanup `match to_deliver.kind.as_str()` add `"continuation" => "continuation",` (and ensure `continuation` outbox subdir exists or reuse `"background"` — pick reuse `"background"` to avoid a new dir; document the choice). `select_delivery_candidate` (the dedup) stays cron-only — continuation falls through (returns the single row), which is correct.

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo nextest run -p bot continuation_uses_its_own_delivery_instruction`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/async_delivery.rs
git commit -m "feat(bot): deliver continuation results with continuation wording"
```

---

## Task 10: Prompt + instructions sync

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
- Modify: `crates/right/src/memory_server.rs` (`with_instructions`)
- Modify: `crates/right/src/aggregator.rs` (`with_instructions`)
- Modify: `PROMPT_SYSTEM.md`
- Test: none (docs/prompt). Verify by `rg` and a codegen build.

- [ ] **Step 1: Update OPERATING_INSTRUCTIONS.md**

In the "## Cron Management" section, the "Self-scheduled follow-up" bullet currently says cron is "your only deferred-action mechanism." Add a tight continuation note distinguishing the two (prompt-tier brevity — declarative, no examples beyond the rule):

```markdown
## Continuations

A finished turn is NOT auto-resumed. If you start work that completes out of
turn (browser-use cloud sessions, a long external job), arm a continuation with
`mcp__right__continue_later` (instruction + check_in/give_up) rather than ending
the turn "waiting" — that just stops. A continuation resumes THIS conversation
with full context; when resumed you report results, wait again, or stay silent if
it was already handled. Cancel a stale one with `mcp__right__continue_cancel`;
list yours with `mcp__right__continue_list`. Use a continuation (not a cron) when
the follow-up needs the current conversation's context; cron is for isolated
scheduled jobs.
```

- [ ] **Step 2: Update both `with_instructions` blocks**

In `crates/right/src/memory_server.rs` and `crates/right/src/aggregator.rs`, add a "## Continuation" section to the instructions string (mirror the existing "## Cron"/"## Progress" entries):

```
## Continuation\n\
- mcp__right__continue_later: Resume THIS conversation later to finish out-of-turn async work; returns continuation_id. Foreground-only.\n\
- mcp__right__continue_cancel: Cancel a scheduled continuation by continuation_id. Foreground-only.\n\
- mcp__right__continue_list: List scheduled continuations for the current chat. Foreground-only.\n\n\
```

- [ ] **Step 3: Sync PROMPT_SYSTEM.md**

Add a short subsection documenting the continuation tools and the schema-driven hop decision (report/wait/abandon), and that hops run scope-less (no foreground tools), mirroring how background/cron are documented. Keep it operator-facing (longer narration is fine here, unlike the prompt).

- [ ] **Step 4: Verify**

Run: `rg -n "continue_later" crates/right/src/aggregator.rs crates/right/src/memory_server.rs crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md PROMPT_SYSTEM.md`
Expected: matches in all four. Then `devenv shell -- cargo build -p right -p right-codegen`.

- [ ] **Step 5: Commit**

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md crates/right/src/memory_server.rs crates/right/src/aggregator.rs PROMPT_SYSTEM.md
git commit -m "docs(prompt): continuation tools + operating instructions"
```

---

## Task 11: Live end-to-end test + final verification

**Files:**
- Create: `crates/bot/tests/continuation_lifecycle.rs` (or extend an existing bot integration test). This exercises arm → reconciler fire → wait/re-arm → report → delivery against a temp agent DB, with CC stubbed. If CC cannot be stubbed without a live sandbox, gate the live portions with the `ci_*` ignore convention (`AGENTS.rust.md` §5) — but keep the pure DB-lifecycle assertions un-ignored.

- [ ] **Step 1: Write the lifecycle test (no live CC)**

Drive the DB API + mapping directly to prove the state machine, since the CC subprocess is the only un-stubable piece:

```rust
#[tokio::test]
async fn continuation_db_lifecycle_report() {
    let dir = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(dir.path(), true).await.unwrap();
    // arm
    right_agent::async_runs::insert_continuation(&conn, right_agent::async_runs::NewContinuation {
        id: "c1", source_session_id: "s1", target_chat_id: 7, target_thread_id: None,
        instruction: "check", scheduled_at: "2000-01-01T00:00:00+00:00",
        deadline_at: "2999-01-01T00:00:00+00:00", check_in_secs: 90, budget_usd: 5.0,
        created_at: "2000-01-01T00:00:00+00:00",
    }).await.unwrap();
    // fire
    assert_eq!(right_agent::async_runs::mark_continuation_running(&conn, "c1", "fork1", "t", "/l").await.unwrap(), 1);
    // hop 1: wait → re-arm
    right_agent::async_runs::rearm_continuation(&conn, "c1", "2000-01-01T00:01:30+00:00", 90, 0.02).await.unwrap();
    let due = right_agent::async_runs::fetch_due_continuations(&conn, "2999-01-01T00:00:00+00:00").await.unwrap();
    assert_eq!(due[0].hop_count, 1);
    // fire hop 2 → report
    right_agent::async_runs::mark_continuation_running(&conn, "c1", "fork2", "t2", "/l2").await.unwrap();
    right_agent::async_runs::finalize_continuation_report(&conn, "c1",
        r#"{"content":"results","attachments":null}"#, "done", 0.03, "t3").await.unwrap();
    // now deliverable, and no longer 'due'
    let due2 = right_agent::async_runs::fetch_due_continuations(&conn, "2999-01-01T00:00:00+00:00").await.unwrap();
    assert!(due2.is_empty());
    let (status, dr): (String, i64) = conn.query_one(
        "SELECT status, delivery_required FROM async_runs WHERE id='c1'",
        right_db::params![], |r| Ok((r.get::<String>(0)?, r.get::<i64>(1)?))).await.unwrap();
    assert_eq!((status.as_str(), dr), ("success", 1));
}
```

- [ ] **Step 2: Run it**

Run: `devenv shell -- cargo nextest run -p bot continuation_db_lifecycle_report`
Expected: PASS.

- [ ] **Step 3: Full workspace verification (mandatory)**

Run:
```
devenv shell -- cargo nextest run --workspace
devenv shell -- cargo test --doc --workspace
```
Expected: PASS. Note any pre-existing flakes (see memory: cc/invocation pid race + dashboard warn-count flake under parallel load) and re-run those isolated before attributing failures to this work.

- [ ] **Step 4: Clippy + build**

Run:
```
devenv shell -- cargo clippy --workspace --all-targets
devenv shell -- cargo build --workspace
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/tests/continuation_lifecycle.rs
git commit -m "test: continuation lifecycle state machine"
```

---

## ARCHITECTURE.md / docs upkeep (cite-on-touch)

- [ ] Update `ARCHITECTURE.md` **MCP Aggregator** section: add `continue_later`/`continue_cancel`/`continue_list` as foreground-only, scope-server-resolved tools (one or two sentences; respect the 40k budget — if tight, put detail in `docs/architecture/sessions.md` and link).
- [ ] Update `docs/architecture/sessions.md`: add the continuation lifecycle (arm → due-scan fire → fork hop → report/wait/abandon → delivery), the `kind='continuation'` async_runs columns, bounding ceilings, and that hops run scope-less. This is the descriptive home per the docs split.
- [ ] Commit: `docs(architecture): continuation primitive`.

---

## Self-Review (completed during authoring)

**Spec coverage:** general primitive (Tasks 2,5,7) ✓; resumes foreground session (Task 5 forks `source_session_id`, resolved Task 7) ✓; idempotency by agent judgment (Task 4 prompt + Task 3 abandon variant) ✓; cancellation by stable id (Tasks 2,7) ✓; bounded with forced give-up notify (Task 5 `is_final` + `resolve_hop_outcome` coercion) ✓; reuse async_runs/delivery (Tasks 1,2,9) ✓; prompt fix (Task 10) ✓; duplicate-collapse — handled by re-arm updating the same row (no separate collapse path needed; documented in Task 9 that delivery dedup stays cron-only) ✓.

**Deviation from spec (intentional, documented above):** re-arm is via the hop's output schema, not a tool call, because background-style runs have no MCP scope; `continue_*` are foreground-only. Per-agent ceiling *config* is deferred (constants in this version; agent sets per-call intent within hard constant ceilings) — noted as a follow-up, not a gap.

**Placeholder scan:** none — every code step shows real code; `run_forked_cc_collect` is explicitly defined as "reuse/lift the background.rs spawn+reader block" with the exact source named.

**Type consistency:** `ContinuationRow`, `NewContinuation`, `ContinuationDecision`/`ContinuationOutput`, `HopOutcome`, `CONTINUE_*` consts, and the async_runs function names are used identically across Tasks 2/5/6/7. `CronNotify` reused for `delivery_json` so the existing delivery YAML/relay path consumes it unchanged.

## Open items to confirm during execution (verify against real code, don't assume)

1. Exact `right-db` read idiom in `async_runs.rs` (`prepare`+`query_map` vs `query_one`); match the file.
2. The per-agent SYSTEM_NOTICE token accessor used by cron/worker (Task 6 needs it for `wrap_system_notice`).
3. `reconcile_jobs` already receives all params `fire_due_continuations` needs (thread any missing through from `run_cron_task` + the `lib.rs:1009` spawn site).
4. Whether `crate::background` exposes a reusable spawn/reader or it must be lifted to a shared `pub(crate)` helper (Task 5).
5. Confirm `49` is the next free migration version at implementation time.
