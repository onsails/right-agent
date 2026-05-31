# Idle `/compact` for opus[1m] sessions — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Before writing Rust, load the `rust-dev:rust-dev` skill; follow FAIL FAST and edition-2024 standards in `AGENTS.rust.md`.

**Goal:** After 2h of inactivity, silently run Claude Code's native `/compact` on an opus[1m] chat session whose context is ≥40% of the 1M window, so the user's next (cold-cache) turn re-reads a small summary instead of the full history.

**Architecture:** A per-`(chat_id, thread_id)` in-memory debounce. Every Normal foreground turn cancels any pending compaction at turn start and (if the session is opus[1m] and ≥400k tokens full) re-arms a 2h `CancellationToken`-gated timer at turn end. On fire, a specialized maintenance `claude -p --resume <id> "/compact <recency instruction>"` runs under the existing per-session mutex, re-checking eligibility first. No polling, no persistence, no config.

**Tech Stack:** Rust 2024, tokio, `tokio_util::sync::CancellationToken`, `dashmap`, `arc_swap`, `right-db` (turso), the existing `ClaudeInvocation` builder.

**Spec:** `docs/superpowers/specs/2026-05-31-idle-compaction-design.md`

**Verification cadence:** Targeted package tests during the loop (`-p right-agent`, `-p bot idle_compaction`); a `cargo build -p bot` after each plumbing/wiring task; **one mandatory full `cargo test --workspace` + clippy at the end** (Task 9). All commands run via `devenv shell -- …`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/right-agent/src/usage/insert.rs` | **Modify.** Add `insert_idle_compaction` (source `'idle_compaction'`). |
| `crates/bot/src/idle_compaction.rs` | **Create.** Constants, gate predicates, fullness query, `CompactTimers` lifecycle (`cancel`/`arm`), `run_compaction`, `on_turn_end`, `build_compact_invocation`, unit tests. |
| `crates/bot/src/lib.rs` | **Modify.** `mod idle_compaction;`; construct `compact_timers`; add to `WorkerControlDeps`. |
| `crates/bot/src/telegram/mod.rs` | **Modify.** `CompactTimers` type alias; `WorkerControlDeps.compact_timers` field. |
| `crates/bot/src/telegram/handler.rs` | **Modify.** Pass `compact_timers` into `WorkerContext`. |
| `crates/bot/src/telegram/worker.rs` | **Modify.** `WorkerContext.compact_timers` field; turn-start `cancel` hook; turn-end `on_turn_end` hook. |
| `ARCHITECTURE.md` | **Modify.** Claude Invocation Contract: list idle-compaction as a specialized maintenance callsite. |
| `docs/architecture/sessions.md` | **Modify.** Narrate the debounce lifecycle (cite-on-touch). |
| `docs/superpowers/specs/2026-05-31-idle-compaction-design.md` | **Modify.** Reconcile timer type to `CancellationToken`. |

---

## Task 1: `insert_idle_compaction` usage writer

**Files:**
- Modify: `crates/right-agent/src/usage/insert.rs` (add fn after `insert_learning_curator`, ~line 96)
- Test: same file's `#[cfg(test)]` module (or a new test fn alongside existing insert tests)

- [ ] **Step 1: Write the failing test**

Add to the test module in `crates/right-agent/src/usage/insert.rs`:

```rust
#[tokio::test]
async fn insert_idle_compaction_writes_row_with_source() {
    let dir = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(dir.path(), true).await.unwrap();
    let b = super::super::UsageBreakdown {
        session_uuid: "sess-1".into(),
        total_cost_usd: 1.23,
        num_turns: 1,
        input_tokens: 10,
        output_tokens: 20,
        cache_creation_tokens: 30,
        cache_read_tokens: 40,
        web_search_requests: 0,
        web_fetch_requests: 0,
        model_usage_json: "{}".into(),
        api_key_source: "none".into(),
        wall_elapsed_ms: Some(50),
    };
    super::insert_idle_compaction(&conn, &b, -100, 7).await.unwrap();
    let (source, chat_id, thread_id): (String, i64, i64) = conn
        .query_row(
            "SELECT source, chat_id, thread_id FROM usage_events LIMIT 1",
            right_db::params![],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .await
        .unwrap();
    assert_eq!(source, "idle_compaction");
    assert_eq!(chat_id, -100);
    assert_eq!(thread_id, 7);
}

#[test]
fn idle_compaction_is_not_a_learning_source() {
    assert!(!crate::usage::LEARNING_SOURCES.contains(&"idle_compaction"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-agent insert_idle_compaction`
Expected: FAIL to compile — `insert_idle_compaction` not found.

- [ ] **Step 3: Implement the writer**

Insert after `insert_learning_curator` (~line 96) in `crates/right-agent/src/usage/insert.rs`:

```rust
/// Insert a row for an idle-compaction maintenance invocation (CC `/compact`
/// driven after a session goes idle). Not a learning source.
///
/// `chat_id` and `thread_id` carry the session the compaction targeted so the
/// dashboard can group compaction spend by chat.
pub async fn insert_idle_compaction(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(
        conn,
        b,
        "idle_compaction",
        Some(chat_id),
        Some(thread_id),
        None,
    )
    .await
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-agent insert_idle_compaction && devenv shell -- cargo test -p right-agent idle_compaction_is_not_a_learning_source`
Expected: PASS (both).

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/usage/insert.rs
git commit -m "feat(usage): add insert_idle_compaction writer (source idle_compaction)"
```

---

## Task 2: `idle_compaction` module scaffold — constants, gate predicates, invocation builder

**Files:**
- Create: `crates/bot/src/idle_compaction.rs`
- Modify: `crates/bot/src/lib.rs` (add `pub(crate) mod idle_compaction;` next to the other module decls, ~line 10)
- Modify: `crates/bot/src/telegram/mod.rs` (add `CompactTimers` alias next to `SessionLocks`, ~line 76)

- [ ] **Step 1: Add the `CompactTimers` type alias**

In `crates/bot/src/telegram/mod.rs`, immediately after the `SessionLocks` alias (~line 76):

```rust
/// Per-(chat_id, thread_id) idle-compaction debounce timers. Cancelling the
/// token aborts a *pending* (still-sleeping) compaction; once the 2h sleep
/// wins, the compaction runs to completion regardless. In-memory only —
/// lost on restart, re-armed on the next turn.
pub(crate) type CompactTimers =
    Arc<DashMap<(i64, i64), tokio_util::sync::CancellationToken>>;
```

- [ ] **Step 2: Register the module**

In `crates/bot/src/lib.rs`, next to the other `pub(crate) mod …;` lines (~line 10):

```rust
pub(crate) mod idle_compaction;
```

- [ ] **Step 3: Write the failing tests**

Create `crates/bot/src/idle_compaction.rs` with only the tests first (and a `use` so it compiles after Step 4):

```rust
//! Idle-compaction debounce: after 2h of inactivity, run CC's native
//! `/compact` on an opus[1m] session that is >=40% full. See
//! docs/superpowers/specs/2026-05-31-idle-compaction-design.md

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_1m_variants_match() {
        assert!(is_opus_1m(Some("claude-opus-4-8[1m]")));
        assert!(is_opus_1m(Some("claude-opus-4-9[1m]"))); // future bump
    }

    #[test]
    fn non_opus_1m_rejected() {
        assert!(!is_opus_1m(Some("claude-sonnet-4-6[1m]")));
        assert!(!is_opus_1m(Some("claude-opus-4-8"))); // not 1m
        assert!(!is_opus_1m(Some("claude-haiku-4-5")));
        assert!(!is_opus_1m(None));
    }

    #[test]
    fn should_compact_boundary() {
        assert!(should_compact(Some("claude-opus-4-8[1m]"), 400_000));
        assert!(!should_compact(Some("claude-opus-4-8[1m]"), 399_999));
        assert!(!should_compact(Some("claude-sonnet-4-6[1m]"), 1_000_000));
        assert!(!should_compact(None, 1_000_000));
    }

    #[test]
    fn compact_invocation_argv_is_maintenance_shaped() {
        let debug = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let args = build_compact_invocation("root-uuid", debug).into_args();
        let joined = args.join(" ");
        // resumes the real session
        let pos = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[pos + 1], "root-uuid");
        // prompt is the /compact command with the recency instruction
        let dash = args.iter().position(|a| a == "--").unwrap();
        assert!(args[dash + 1].starts_with("/compact "));
        assert!(args[dash + 1].contains("most recently discussed"));
        // maintenance contract: no schema, no MCP
        assert!(!joined.contains("--json-schema"));
        assert!(!joined.contains("--mcp-config"));
    }
}
```

- [ ] **Step 4: Implement constants, predicates, and the invocation builder**

Prepend (above the test module) in `crates/bot/src/idle_compaction.rs`:

```rust
use std::time::Duration;

use crate::cc::invocation::{ClaudeInvocation, OutputFormat};

/// Idle window before compaction fires. A turn resets this debounce.
const IDLE_AFTER: Duration = Duration::from_secs(2 * 60 * 60);
/// Compact only when the last turn's context footprint reached this many
/// tokens (40% of the opus[1m] 1,000,000-token window).
const MIN_USED_TOKENS: u64 = 400_000;
/// Wall-clock cap on a single `/compact` call. Bounds how long a returning
/// user waits on the session lock if they arrive mid-compaction.
const COMPACT_TIMEOUT: Duration = Duration::from_secs(120);
/// Steers CC's summary toward the active discussion. Static — CC already has
/// the full conversation at compaction time.
const RECENCY_INSTRUCTION: &str = "Prioritize the most recently discussed \
topics and any open or unresolved threads. Preserve concrete details from \
recent exchanges — names, file paths, decisions, values, and the user's \
current goal — over older, settled context.";

/// True for an Opus model running the 1M-context (`[1m]`) window. Matches the
/// suffix rather than a pinned id so an opus version bump keeps working while
/// `sonnet[1m]` and non-1M opus stay excluded.
pub(crate) fn is_opus_1m(model: Option<&str>) -> bool {
    matches!(model, Some(m) if m.starts_with("claude-opus") && m.ends_with("[1m]"))
}

/// The full gate: opus[1m] AND context footprint at/above the threshold.
pub(crate) fn should_compact(model: Option<&str>, used_tokens: u64) -> bool {
    is_opus_1m(model) && used_tokens >= MIN_USED_TOKENS
}

/// Build the specialized maintenance invocation: `claude -p --resume <id>
/// "/compact <recency instruction>"`, no schema, no MCP, tools disabled.
/// Deliberate exception to the standard session-bearing contract (see
/// ARCHITECTURE.md → Claude Invocation Contract).
pub(crate) fn build_compact_invocation(
    root_session_id: &str,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> ClaudeInvocation {
    ClaudeInvocation {
        mcp_config_path: None,
        json_schema: None,
        output_format: OutputFormat::Json,
        model: None, // inherit the session's model (opus[1m])
        max_budget_usd: None,
        max_turns: None,
        resume_session_id: Some(root_session_id.to_owned()),
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: crate::cc::invocation::disable_all_tools_args(),
        prompt: Some(format!("/compact {RECENCY_INSTRUCTION}")),
        debug_flag: Some(debug),
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p bot idle_compaction`
Expected: PASS (4 tests). Warnings about unused `IDLE_AFTER`/`COMPACT_TIMEOUT` are expected at this stage — they are consumed in Tasks 4–5.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/idle_compaction.rs crates/bot/src/lib.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(idle-compaction): module scaffold, gate predicates, /compact invocation builder"
```

---

## Task 3: Context-fullness query

**Files:**
- Modify: `crates/bot/src/idle_compaction.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/bot/src/idle_compaction.rs`:

```rust
#[tokio::test]
async fn fullness_reads_latest_interactive_sum() {
    let dir = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(dir.path(), true).await.unwrap();

    let mk = |input: u64, cache_read: u64, cache_create: u64| right_agent::usage::UsageBreakdown {
        session_uuid: "s".into(),
        total_cost_usd: 0.0,
        num_turns: 1,
        input_tokens: input,
        output_tokens: 0,
        cache_creation_tokens: cache_create,
        cache_read_tokens: cache_read,
        web_search_requests: 0,
        web_fetch_requests: 0,
        model_usage_json: "{}".into(),
        api_key_source: "none".into(),
        wall_elapsed_ms: None,
    };

    // Older smaller turn, then a newer larger turn, for the same (chat, thread).
    right_agent::usage::insert::insert_interactive(&conn, &mk(1, 1, 1), 42, 0).await.unwrap();
    right_agent::usage::insert::insert_interactive(&conn, &mk(100, 200, 50), 42, 0).await.unwrap();
    // A different source must be ignored.
    right_agent::usage::insert::insert_learning_prefilter(&conn, &mk(9_999, 0, 0), 42, 0).await.unwrap();

    let used = latest_interactive_context_tokens(&conn, 42, 0).await.unwrap();
    assert_eq!(used, Some(350)); // 100 + 200 + 50

    let absent = latest_interactive_context_tokens(&conn, 999, 0).await.unwrap();
    assert_eq!(absent, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p bot fullness_reads_latest_interactive_sum`
Expected: FAIL to compile — `latest_interactive_context_tokens` not found.

- [ ] **Step 3: Implement the query**

Add (above the test module) in `crates/bot/src/idle_compaction.rs`:

```rust
/// Context footprint of the most recent interactive turn for this session:
/// `input + cache_read + cache_creation` tokens. `None` when no turn exists.
/// This is the prompt size going into the last API call — i.e. how full the
/// context is, regardless of how much was cache-served.
pub(crate) async fn latest_interactive_context_tokens(
    conn: &right_db::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<u64>, right_db::DbError> {
    use right_db::OptionalExtension as _;
    conn.query_row(
        "SELECT input_tokens + cache_read_tokens + cache_creation_tokens \
         FROM usage_events \
         WHERE chat_id = ?1 AND thread_id = ?2 AND source = 'interactive' \
         ORDER BY ts DESC LIMIT 1",
        right_db::params![chat_id, thread_id],
        |r| r.get::<_, i64>(0),
    )
    .await
    .optional()
    .map(|opt| opt.map(|v| v.max(0) as u64))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p bot fullness_reads_latest_interactive_sum`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/idle_compaction.rs
git commit -m "feat(idle-compaction): latest-interactive context-fullness query"
```

---

## Task 4: Context bundle + `run_compaction`

**Files:**
- Modify: `crates/bot/src/idle_compaction.rs`

This task adds the orchestration that fires once a timer elapses. It has no default-path unit test (it spawns `claude` in a sandbox); correctness of its inputs is covered by Tasks 2–3 and the argv test. It must compile and pass clippy.

- [ ] **Step 1: Implement the context struct and `run_compaction`**

Add (above the test module) in `crates/bot/src/idle_compaction.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Everything a fire task needs. Cloned from `WorkerContext` at the turn-end
/// hook (Task 7).
#[derive(Clone)]
pub(crate) struct IdleCompactionCtx {
    pub compact_timers: crate::telegram::CompactTimers,
    pub model: Arc<arc_swap::ArcSwap<Option<String>>>,
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub session_locks: crate::telegram::SessionLocks,
    pub debug: Arc<AtomicBool>,
    pub chat_id: i64,
    pub thread_id: i64,
}

/// Fire path. Re-checks eligibility, resolves the active session, takes the
/// per-session mutex, runs `/compact`, records usage. Best-effort: every
/// failure logs and returns (the next idle cycle retries). Never aborted
/// mid-flight (see `arm`), so the session lock is always released cleanly.
async fn run_compaction(ctx: IdleCompactionCtx) {
    let conn = match right_db::open_connection(&ctx.agent_db_dir, false).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: open_connection failed: {e:#}");
            return;
        }
    };

    // Fire-time re-checks: model (hot-reloadable via /model) and fullness.
    let model = crate::snapshot_model(&ctx.model);
    let used = match latest_interactive_context_tokens(&conn, ctx.chat_id, ctx.thread_id).await {
        Ok(v) => v.unwrap_or(0),
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: fullness query failed: {e:#}");
            return;
        }
    };
    if !should_compact(model.as_deref(), used) {
        tracing::debug!(agent = %ctx.agent_name, "idle-compaction: no longer eligible at fire time, skipping");
        return;
    }

    let root_session_id = match crate::telegram::session::get_active_session(
        &conn, ctx.chat_id, ctx.thread_id,
    )
    .await
    {
        Ok(Some(s)) => s.root_session_id,
        Ok(None) => {
            tracing::debug!(agent = %ctx.agent_name, "idle-compaction: no active session, skipping");
            return;
        }
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: get_active_session failed: {e:#}");
            return;
        }
    };

    // Serialize against live worker/delivery turns on the same session.
    let _guard: tokio::sync::OwnedMutexGuard<()> = {
        let entry = ctx
            .session_locks
            .entry(root_session_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        entry.lock_owned().await
    };

    let args = build_compact_invocation(&root_session_id, Arc::clone(&ctx.debug)).into_args();
    let mut cmd = crate::cc::invocation::build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    )
    .await;
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = match tokio::time::timeout(COMPACT_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: spawn failed: {e:#}");
            return;
        }
        Err(_) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "idle-compaction: timed out after {}s",
                COMPACT_TIMEOUT.as_secs()
            );
            return;
        }
    };

    if !output.status.success() {
        // `/compact` returns an empty `result`; success is exit status only.
        tracing::warn!(
            agent = %ctx.agent_name,
            status = ?output.status,
            stderr_bytes = output.stderr.len(),
            "idle-compaction: /compact non-zero exit"
        );
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(b) = crate::cc::stream::parse_usage_full(&stdout)
        && let Err(e) = right_agent::usage::insert::insert_idle_compaction(
            &conn, &b, ctx.chat_id, ctx.thread_id,
        )
        .await
    {
        tracing::warn!(agent = %ctx.agent_name, "idle-compaction: usage insert failed: {e:#}");
    }

    tracing::info!(
        agent = %ctx.agent_name,
        chat_id = ctx.chat_id,
        thread_id = ctx.thread_id,
        used_tokens = used,
        "idle-compaction complete"
    );
}
```

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo build -p bot`
Expected: builds. `run_compaction` is currently unused (called in Task 5) — allow the dead-code warning for now, or proceed directly to Task 5 in the same review batch.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/idle_compaction.rs
git commit -m "feat(idle-compaction): run_compaction fire path under the session mutex"
```

---

## Task 5: Timer lifecycle (`cancel`, `arm`) and `on_turn_end`

**Files:**
- Modify: `crates/bot/src/idle_compaction.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/bot/src/idle_compaction.rs`:

```rust
fn dummy_ctx(timers: crate::telegram::CompactTimers, chat: i64, thread: i64) -> IdleCompactionCtx {
    IdleCompactionCtx {
        compact_timers: timers,
        model: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(Some(
            "claude-opus-4-8[1m]".to_string(),
        ))),
        agent_dir: std::path::PathBuf::from("/nonexistent"),
        agent_db_dir: std::path::PathBuf::from("/nonexistent"),
        agent_name: "test".into(),
        ssh_config_path: None,
        resolved_sandbox: None,
        session_locks: std::sync::Arc::new(dashmap::DashMap::new()),
        debug: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        chat_id: chat,
        thread_id: thread,
    }
}

#[tokio::test]
async fn arm_then_cancel_removes_and_cancels() {
    let timers: crate::telegram::CompactTimers = std::sync::Arc::new(dashmap::DashMap::new());
    arm(dummy_ctx(timers.clone(), 1, 0));
    let token = timers.get(&(1, 0)).map(|e| e.value().clone());
    assert!(token.is_some(), "arm must register a timer");
    cancel(&timers, 1, 0);
    assert!(timers.get(&(1, 0)).is_none(), "cancel must remove the entry");
    assert!(token.unwrap().is_cancelled(), "cancel must cancel the token");
    // The spawned task takes the cancelled branch and never runs run_compaction
    // (the /nonexistent paths would otherwise error), so a short yield is safe.
    tokio::task::yield_now().await;
}

#[tokio::test]
async fn arm_twice_replaces_previous_timer() {
    let timers: crate::telegram::CompactTimers = std::sync::Arc::new(dashmap::DashMap::new());
    arm(dummy_ctx(timers.clone(), 2, 0));
    let first = timers.get(&(2, 0)).unwrap().value().clone();
    arm(dummy_ctx(timers.clone(), 2, 0));
    assert!(first.is_cancelled(), "re-arming must cancel the prior timer");
    assert!(timers.get(&(2, 0)).is_some(), "a fresh timer must be present");
    cancel(&timers, 2, 0);
    tokio::task::yield_now().await;
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p bot idle_compaction`
Expected: FAIL to compile — `arm` / `cancel` not found.

- [ ] **Step 3: Implement `cancel`, `arm`, and `on_turn_end`**

Add (above the test module) in `crates/bot/src/idle_compaction.rs`:

```rust
use tokio_util::sync::CancellationToken;

/// Cancel and remove any pending compaction for this session. Called at turn
/// start (activity) and when a turn ends ineligible. No-op if none armed.
pub(crate) fn cancel(timers: &crate::telegram::CompactTimers, chat_id: i64, thread_id: i64) {
    if let Some((_, token)) = timers.remove(&(chat_id, thread_id)) {
        token.cancel();
    }
}

/// (Re)arm the 2h debounce. Replaces any existing timer. The spawned task
/// waits on `sleep` racing the token: a cancel during the wait returns without
/// compacting; once `sleep` wins, the token is no longer awaited, so a late
/// cancel cannot tear down the in-flight compaction (which would orphan the
/// `claude` child and drop the session lock mid-write).
fn arm(ctx: IdleCompactionCtx) {
    let key = (ctx.chat_id, ctx.thread_id);
    if let Some((_, prev)) = ctx.compact_timers.remove(&key) {
        prev.cancel();
    }
    let token = CancellationToken::new();
    ctx.compact_timers.insert(key, token.clone());

    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(IDLE_AFTER) => {}
            _ = token.cancelled() => return,
        }
        // Survived the debounce. Drop our map entry first so a concurrent
        // cancel finds nothing to cancel, then run to completion uncancelled.
        ctx.compact_timers.remove(&key);
        run_compaction(ctx).await;
    });
}

/// Turn-end hook for Normal foreground turns. Model-checks first (no DB for
/// non-opus[1m] agents); on opus[1m], reads fullness and arms or cancels.
pub(crate) async fn on_turn_end(ctx: IdleCompactionCtx) {
    let model = crate::snapshot_model(&ctx.model);
    if !is_opus_1m(model.as_deref()) {
        cancel(&ctx.compact_timers, ctx.chat_id, ctx.thread_id);
        return;
    }
    let conn = match right_db::open_connection(&ctx.agent_db_dir, false).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: open_connection failed: {e:#}");
            return;
        }
    };
    let used = match latest_interactive_context_tokens(&conn, ctx.chat_id, ctx.thread_id).await {
        Ok(v) => v.unwrap_or(0),
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: fullness query failed: {e:#}");
            return;
        }
    };
    if should_compact(model.as_deref(), used) {
        arm(ctx);
    } else {
        cancel(&ctx.compact_timers, ctx.chat_id, ctx.thread_id);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p bot idle_compaction`
Expected: PASS (all idle_compaction tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/idle_compaction.rs
git commit -m "feat(idle-compaction): debounce lifecycle (cancel/arm) and on_turn_end gate"
```

---

## Task 6: Plumb `CompactTimers` through worker wiring

**Files:**
- Modify: `crates/bot/src/telegram/mod.rs:233-238` (`WorkerControlDeps`)
- Modify: `crates/bot/src/lib.rs:1044` (construct) and the `WorkerControlDeps { … }` literal (find with the command below)
- Modify: `crates/bot/src/telegram/worker.rs:269-310` (`WorkerContext` field)
- Modify: `crates/bot/src/telegram/handler.rs:340-369` (`WorkerContext { … }` literal)

No new behavior — this only carries the shared map to the worker. Verified by compilation.

- [ ] **Step 1: Add the field to `WorkerControlDeps`**

In `crates/bot/src/telegram/mod.rs`, inside `pub struct WorkerControlDeps` (after `progress`, ~line 238):

```rust
    pub(crate) compact_timers: CompactTimers,
```

- [ ] **Step 2: Add the field to `WorkerContext`**

In `crates/bot/src/telegram/worker.rs`, inside `pub struct WorkerContext` (after `session_locks`, ~line 299):

```rust
    /// Per-(chat, thread) idle-compaction debounce timers.
    pub compact_timers: super::CompactTimers,
```

- [ ] **Step 3: Construct and inject in `lib.rs`**

In `crates/bot/src/lib.rs`, next to `session_locks` (~line 1044):

```rust
let compact_timers: crate::telegram::CompactTimers = Arc::new(dashmap::DashMap::new());
```

Then find the `WorkerControlDeps { … }` literal and add `compact_timers: Arc::clone(&compact_timers),`:

Run to locate it: `rg -n "WorkerControlDeps \{" crates/bot/src/lib.rs`
Add inside that literal:

```rust
    compact_timers: Arc::clone(&compact_timers),
```

If `WorkerControlDeps` is constructed via field-init shorthand from a local binding, the `let compact_timers = …` above plus a bare `compact_timers,` entry suffices — match the surrounding style.

- [ ] **Step 4: Construct in `handler.rs`**

In `crates/bot/src/telegram/handler.rs`, inside the `WorkerContext { … }` literal (~line 355, next to `session_locks`):

```rust
                    compact_timers: Arc::clone(&worker_ctl.compact_timers),
```

(`worker_ctl` is the `WorkerControlDeps` parameter; match the exact binding name in that function — it appears as the deps argument to the worker-spawn helper.)

- [ ] **Step 5: Verify it compiles**

Run: `devenv shell -- cargo build -p bot`
Expected: builds with no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/mod.rs crates/bot/src/lib.rs crates/bot/src/telegram/worker.rs crates/bot/src/telegram/handler.rs
git commit -m "feat(idle-compaction): thread CompactTimers through worker control deps"
```

---

## Task 7: Wire the two hooks into the worker

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` — turn-start cancel (before the `invoke_cc(` call, ~line 1435) and turn-end arm (after the learning block, ~line 2162)

- [ ] **Step 1: Add the turn-start cancel hook**

In `crates/bot/src/telegram/worker.rs`, immediately before the `let ( … ) = match invoke_cc(` statement (~line 1435):

```rust
            // Idle-compaction: any foreground turn is activity — cancel a
            // pending compaction so it cannot fire during this turn.
            crate::idle_compaction::cancel(&ctx.compact_timers, chat_id, eff_thread_id);
```

- [ ] **Step 2: Add the turn-end arm hook**

In `crates/bot/src/telegram/worker.rs`, immediately after the closing `}` of the post-turn learning block (after line 2162, before the `// Auto-retain and prefetch` comment at ~line 2164):

```rust
            // Idle-compaction debounce (Normal foreground turns only).
            // Independent of the learning gate above: arm a 2h timer when the
            // session is opus[1m] and >=40% full; cancel otherwise.
            if matches!(cc_prompt_mode, Some(crate::cc::prompt::PromptMode::Normal)) {
                crate::idle_compaction::on_turn_end(crate::idle_compaction::IdleCompactionCtx {
                    compact_timers: ctx.compact_timers.clone(),
                    model: Arc::clone(&ctx.model),
                    agent_dir: ctx.agent_dir.clone(),
                    agent_db_dir: ctx.agent_db_dir.clone(),
                    agent_name: ctx.agent_name.clone(),
                    ssh_config_path: ctx.ssh_config_path.clone(),
                    resolved_sandbox: ctx.resolved_sandbox.clone(),
                    session_locks: ctx.session_locks.clone(),
                    debug: Arc::clone(&ctx.debug),
                    chat_id,
                    thread_id: eff_thread_id,
                })
                .await;
            }
```

- [ ] **Step 3: Verify it compiles**

Run: `devenv shell -- cargo build -p bot`
Expected: builds. If `IdleCompactionCtx`'s fields are `pub(crate)` and the struct is `pub(crate)`, the literal compiles from `worker.rs` (same crate).

- [ ] **Step 4: Quick logic check (no sandbox)**

Run: `devenv shell -- cargo test -p bot idle_compaction`
Expected: PASS — the unit suite still green after wiring.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(idle-compaction): arm/cancel debounce from the worker turn hooks"
```

---

## Task 8: Documentation

**Files:**
- Modify: `ARCHITECTURE.md` (Claude Invocation Contract section)
- Modify: `docs/architecture/sessions.md`
- Modify: `docs/superpowers/specs/2026-05-31-idle-compaction-design.md` (reconcile timer type)

- [ ] **Step 1: ARCHITECTURE.md — record the maintenance-callsite exception**

In `ARCHITECTURE.md`, in the **Claude Invocation Contract** section, after the sentence about learning callsites having specialized contracts, add:

```markdown
Idle compaction (`crates/bot/src/idle_compaction.rs`) is another specialized
maintenance callsite: it resumes a session to run `/compact` with **no
`--json-schema` and no `--mcp-config`**, judging success by exit status (the
`/compact` `result` is empty). It runs under the per-session `SessionLocks`
mutex and is never a normal deliverable turn.
```

Verify the file stays under budget: `wc -c ARCHITECTURE.md` (must be < 40000).

- [ ] **Step 2: docs/architecture/sessions.md — narrate the flow**

Add a subsection to `docs/architecture/sessions.md` describing: the per-`(chat,thread)` `CompactTimers` debounce; turn-start cancel + turn-end arm (Normal-mode only); the opus[1m] + ≥400k gate read from the latest `interactive` `usage_events` row; how per-turn re-evaluation neutralizes CC auto-compact and makes it self-limiting (no `compacted_at` marker); the fire-time re-check; and the `CancellationToken` commit-to-sleep semantics that keep an in-flight compaction from being torn down. Reference `crates/bot/src/idle_compaction.rs` as authoritative.

- [ ] **Step 3: Reconcile the spec's timer type**

In `docs/superpowers/specs/2026-05-31-idle-compaction-design.md`, change the `CompactTimers` type alias from `tokio::task::AbortHandle` to `tokio_util::sync::CancellationToken`, and adjust the one sentence that says "Aborting the handle cancels a queued compaction" to describe the `select!`/cancel-token semantics (cancelling aborts only a still-sleeping timer; once the sleep wins, compaction runs to completion). This keeps the spec consistent with the implementation.

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md docs/architecture/sessions.md docs/superpowers/specs/2026-05-31-idle-compaction-design.md
git commit -m "docs(idle-compaction): invocation-contract exception, sessions narration, spec reconcile"
```

---

## Task 9: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Record any pre-existing unrelated failures; nothing in `right-agent` usage or `bot` idle_compaction should fail.

- [ ] **Step 2: Clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets`
Expected: no new warnings from `idle_compaction.rs` or the modified files. In particular, confirm no `unused` warnings remain for `IDLE_AFTER`, `COMPACT_TIMEOUT`, or `run_compaction` (all consumed once Tasks 4–7 land).

- [ ] **Step 3: Debug build**

Run: `devenv shell -- cargo build --workspace`
Expected: builds clean.

- [ ] **Step 4: Optional manual smoke (requires a live opus[1m] agent)**

Not part of CI. With a sandboxed opus[1m] agent that has a ≥400k-token session, temporarily shrink `IDLE_AFTER` locally (e.g. 60s) and confirm via process-compose logs (`right-bot`) that a `/compact` invocation runs after idle, an `idle_compaction` row lands in `usage_events`, and the next user turn resumes from the compacted summary. Revert `IDLE_AFTER` before committing. Do **not** leave a shortened constant in the tree.

- [ ] **Step 5: Final commit (if Step 4 produced any reverts/cleanups)**

```bash
git status   # expect clean if no manual edits were made
```

---

## Self-Review

- **Spec coverage:** trigger/debounce → T5+T7; opus[1m]+40% gate → T2 (`should_compact`); fullness signal → T3; auto-compact/idempotency → structural (T5 per-turn re-eval, no marker — nothing to build); specialized `/compact` invocation → T2 (`build_compact_invocation`) + T4 (`run_compaction`); session-lock coupling → T4; fire-time re-check → T4; usage source → T1; in-memory timers/no-persistence → T2 (`CompactTimers`) + T5; constants → T2; docs (ARCHITECTURE + sessions.md) → T8. All spec sections map to a task.
- **Placeholders:** none — every code step shows complete code; every command shows expected output.
- **Type consistency:** `IdleCompactionCtx` fields (T4) match the construction in T7; `CompactTimers = Arc<DashMap<(i64,i64), CancellationToken>>` (T2) matches `cancel`/`arm` (T5) and the worker field (T6); `insert_idle_compaction` signature (T1) matches the call in `run_compaction` (T4); `should_compact`/`is_opus_1m`/`latest_interactive_context_tokens` names are consistent across T2/T3/T4/T5.
