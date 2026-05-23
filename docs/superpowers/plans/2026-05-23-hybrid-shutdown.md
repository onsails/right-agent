# Hybrid Shutdown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `SIGINT`/`SIGTERM` graceful from Telegram's point of view by backgrounding active foreground turns, bounded-draining cron jobs, and flushing ready deliveries.

**Architecture:** Reuse existing foreground background-continuation machinery instead of adding a parallel shutdown path. Add a typed foreground background-request reason, route process shutdown through that reason, add explicit cron interruption persistence for owned running jobs, and extract a shutdown-only delivery flush that bypasses idle politeness but keeps existing target/dedup/send rules.

**Tech Stack:** Rust 2024, Tokio, Teloxide, DashMap, rusqlite, existing `right_process::ProcessGroupChild`, existing `async_runs` persistence.

---

## Preconditions

- The execution session must load `rust-dev:rust-dev` before writing Rust. If that skill is still unavailable, state that explicitly and proceed only if the user accepts the fallback.
- Work in an isolated worktree under `.worktrees/` if using subagents or a long implementation session.
- Run one baseline targeted suite before changes:

```bash
devenv shell -- cargo test -p right-bot background_continuation_tests
devenv shell -- cargo test -p right-bot target_snapshot_tests
devenv shell -- cargo test -p right-bot async_delivery
```

Expected: all pass or record pre-existing failures before editing.

## File Structure

- Modify `crates/bot/src/telegram/mod.rs`: shared foreground-control types, `BgRequests` value type, shutdown handoff helpers.
- Modify `crates/bot/src/telegram/handler.rs`: Background button writes the new typed request.
- Modify `crates/bot/src/telegram/dispatch.rs`: on signal/config shutdown, request backgrounding for active foreground runs and wait for handoff gates before returning.
- Modify `crates/bot/src/telegram/worker.rs`: add `BgReason::Shutdown`, consume typed background requests, render shutdown banner, update prompt reason tests.
- Modify `crates/bot/src/cron.rs`: add explicit cron shutdown interruption persistence and fix shutdown timeout handling so timed-out owned jobs are abortable and marked failed.
- Modify `crates/bot/src/async_delivery.rs`: extract one delivery tick, add bounded shutdown flush that skips idle-delay checks but preserves target validation, dedup, and send outcome handling.
- Modify `crates/bot/src/lib.rs`: call delivery shutdown flush after cron joins and before the delivery loop handle is awaited.
- Modify `docs/architecture/sessions.md`: document shutdown foreground handoff, cron interruption, and delivery flush semantics.
- Modify `docs/architecture/lifecycle.md`: document the `right bot` shutdown flow.
- Modify `ARCHITECTURE.md` only if implementation creates a new prescriptive contract or module ownership boundary.

### Task 1: Typed Foreground Background Requests

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/telegram/mod.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Test: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write failing tests for shutdown reason text and typed request consumption**

Add tests in `crates/bot/src/telegram/worker.rs` inside `background_continuation_tests` and `bg_request_race_tests`:

```rust
#[test]
fn continuation_prompt_mentions_shutdown_reason() {
    let p = build_continuation_prompt(BgReason::Shutdown);
    assert!(p.contains("the bot process is shutting down"));
    assert!(p.contains("MOST RECENT MESSAGE"));
}

#[test]
fn shutdown_bg_request_consumes_matching_turn_id() {
    let bg: super::super::BgRequests = Arc::new(DashMap::new());
    bg.insert(
        (1, 0),
        super::super::BgRequest {
            turn_id: 42,
            reason: BgReason::Shutdown,
        },
    );

    let consumed = consume_bg_request(&bg, (1, 0), 42);
    assert_eq!(consumed, Some(BgReason::Shutdown));
    assert!(bg.get(&(1, 0)).is_none());
}

#[test]
fn stale_shutdown_bg_request_is_removed_and_ignored() {
    let bg: super::super::BgRequests = Arc::new(DashMap::new());
    bg.insert(
        (1, 0),
        super::super::BgRequest {
            turn_id: 999,
            reason: BgReason::Shutdown,
        },
    );

    let consumed = consume_bg_request(&bg, (1, 0), 1);
    assert_eq!(consumed, None);
    assert!(bg.get(&(1, 0)).is_none());
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
devenv shell -- cargo test -p right-bot background_continuation_tests::continuation_prompt_mentions_shutdown_reason
devenv shell -- cargo test -p right-bot bg_request_race_tests::shutdown_bg_request_consumes_matching_turn_id
```

Expected: fail because `BgReason::Shutdown`, `BgRequest`, and the new `consume_bg_request` return type do not exist yet.

- [ ] **Step 3: Implement typed background requests**

In `crates/bot/src/telegram/worker.rs`, extend `BgReason`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BgReason {
    /// The foreground safety timeout fired.
    AutoTimeout,
    /// The user pressed the "Background" inline button during the thinking phase.
    UserRequested,
    /// The bot process is shutting down and moved the turn out of foreground.
    Shutdown,
}
```

Update `continuation_reason_text` in `crates/bot/src/telegram/worker.rs`:

```rust
fn continuation_reason_text(reason: BgReason) -> &'static str {
    match reason {
        BgReason::AutoTimeout => {
            "the foreground turn hit the 10-minute safety limit and was terminated"
        }
        BgReason::UserRequested => "the user moved this work to background execution",
        BgReason::Shutdown => {
            "the bot process is shutting down and moved this foreground turn to background execution"
        }
    }
}
```

In `crates/bot/src/telegram/mod.rs`, replace `BgRequests` with a typed request:

```rust
/// A foreground turn request to convert the active Claude invocation into a
/// background continuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BgRequest {
    pub(crate) turn_id: u64,
    pub(crate) reason: worker::BgReason,
}

/// Per-(chat, thread) flag set by Background button or shutdown.
pub(crate) type BgRequests = Arc<DashMap<(i64, i64), BgRequest>>;
```

In `crates/bot/src/telegram/handler.rs`, update the Background callback insert:

```rust
worker_ctl.bg_requests.insert(
    key,
    super::BgRequest {
        turn_id: *turn_id,
        reason: super::worker::BgReason::UserRequested,
    },
);
```

In `crates/bot/src/telegram/worker.rs`, update `consume_bg_request` to return a reason:

```rust
fn consume_bg_request(
    bg_requests: &super::BgRequests,
    key: SessionKey,
    turn_id: u64,
) -> Option<BgReason> {
    let Some((_, request)) = bg_requests.remove(&key) else {
        return None;
    };
    (request.turn_id == turn_id).then_some(request.reason)
}
```

Update callsites in `invoke_cc`:

```rust
let bg_reason = consume_bg_request(&ctx.bg_requests, (chat_id, eff_thread_id), turn_id);
let was_bg_request = bg_reason.is_some();
```

Then return:

```rust
if let Some(reason) = bg_reason {
    return Err(InvokeCcFailure::Backgrounded {
        reason,
        main_session_id: session_uuid.clone(),
        thinking_msg_id,
        session_guard,
    });
}
```

- [ ] **Step 4: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot background_continuation_tests
devenv shell -- cargo test -p right-bot bg_request_race_tests
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
devenv shell -- git add crates/bot/src/telegram/mod.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs
devenv shell -- git commit -m "feat(bot): type foreground background requests"
```

### Task 2: Request Foreground Backgrounding During Shutdown

**Files:**
- Modify: `crates/bot/src/telegram/mod.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Test: `crates/bot/src/telegram/mod.rs` or `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Write failing tests for shutdown request helper**

Add tests in `crates/bot/src/telegram/mod.rs` under a new `#[cfg(test)] mod shutdown_request_tests`:

```rust
#[test]
fn request_shutdown_backgrounding_sets_gate_and_cancels_tokens() {
    let stop_tokens: StopTokens = Arc::new(DashMap::new());
    let bg_requests: BgRequests = Arc::new(DashMap::new());
    let gates: BgHandoffGates = Arc::new(DashMap::new());
    let token = CancellationToken::new();

    stop_tokens.insert((10, 0), (7, token.clone()));

    let requested = request_shutdown_backgrounding(&stop_tokens, &bg_requests, &gates);

    assert_eq!(requested, 1);
    assert!(token.is_cancelled());
    assert!(gates.get(&(10, 0)).is_some());
    let request = bg_requests.get(&(10, 0)).unwrap();
    assert_eq!(request.turn_id, 7);
    assert_eq!(request.reason, worker::BgReason::Shutdown);
}

#[tokio::test]
async fn wait_for_handoff_gates_empty_returns_after_release() {
    let gates: BgHandoffGates = Arc::new(DashMap::new());
    set_bg_handoff_gate(&gates, (10, 0));
    let release = Arc::clone(&gates);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        release_bg_handoff_gate(&release, (10, 0));
    });

    let done = wait_for_handoff_gates_empty(&gates, std::time::Duration::from_secs(1)).await;
    assert!(done);
}
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
devenv shell -- cargo test -p right-bot shutdown_request_tests
```

Expected: fail because helper functions do not exist.

- [ ] **Step 3: Implement shutdown request helpers**

In `crates/bot/src/telegram/mod.rs`, add:

```rust
pub(crate) fn request_shutdown_backgrounding(
    stop_tokens: &StopTokens,
    bg_requests: &BgRequests,
    gates: &BgHandoffGates,
) -> usize {
    let mut requested = 0usize;
    for entry in stop_tokens.iter() {
        let key = *entry.key();
        let (turn_id, token) = entry.value();
        set_bg_handoff_gate(gates, key);
        bg_requests.insert(
            key,
            BgRequest {
                turn_id: *turn_id,
                reason: worker::BgReason::Shutdown,
            },
        );
        token.cancel();
        requested += 1;
    }
    requested
}

pub(crate) async fn wait_for_handoff_gates_empty(
    gates: &BgHandoffGates,
    timeout: std::time::Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if gates.is_empty() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
```

In `crates/bot/src/telegram/dispatch.rs`, capture the maps into the shutdown driver task:

```rust
let stop_tokens_for_shutdown = Arc::clone(&stop_tokens);
let bg_requests_for_shutdown = Arc::clone(&bg_requests);
let bg_handoff_gates_for_shutdown = Arc::clone(&bg_handoff_gates);
```

Inside the spawned shutdown driver, before `worker_shutdown_task.cancel();`, request foreground handoff:

```rust
let requested = super::request_shutdown_backgrounding(
    &stop_tokens_for_shutdown,
    &bg_requests_for_shutdown,
    &bg_handoff_gates_for_shutdown,
);
tracing::info!(
    active_foreground = requested,
    "shutdown: requested foreground background handoff"
);
```

After the `dispatcher.dispatch_with_listener` await and before `worker_shutdown.cancel();`, wait for gates:

```rust
let handoffs_done = super::wait_for_handoff_gates_empty(
    &bg_handoff_gates,
    std::time::Duration::from_secs(30),
)
.await;
if handoffs_done {
    tracing::info!("shutdown: foreground handoff gates drained");
} else {
    tracing::warn!("shutdown: timed out waiting for foreground handoff gates");
}
```

- [ ] **Step 4: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot shutdown_request_tests
devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
devenv shell -- git add crates/bot/src/telegram/mod.rs crates/bot/src/telegram/dispatch.rs
devenv shell -- git commit -m "feat(bot): background foreground turns on shutdown"
```

### Task 3: Shutdown Banner For Foreground Handoffs

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Test: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write failing banner test**

In `crates/bot/src/telegram/worker.rs` inside `background_continuation_tests`, add:

```rust
#[test]
fn background_banner_distinguishes_shutdown() {
    assert_eq!(
        background_banner(BgReason::Shutdown),
        "Shutting down - continuing in background. Will reply when ready"
    );
}
```

- [ ] **Step 2: Run failing test**

Run:

```bash
devenv shell -- cargo test -p right-bot background_continuation_tests::background_banner_distinguishes_shutdown
```

Expected: fail because `background_banner` does not exist.

- [ ] **Step 3: Extract banner helper and use it**

In `crates/bot/src/telegram/worker.rs`, add near `build_continuation_prompt`:

```rust
fn background_banner(reason: BgReason) -> &'static str {
    match reason {
        BgReason::AutoTimeout => {
            "Foreground hit 10-min limit - continuing in background. Will reply when ready"
        }
        BgReason::UserRequested => "Working in background. Will reply when ready",
        BgReason::Shutdown => "Shutting down - continuing in background. Will reply when ready",
    }
}
```

Replace the inline banner match in the `HandoffStatus::Spawned` arm with:

```rust
let banner = background_banner(reason);
let _ = ctx
    .bot
    .edit_message_text(tg_chat_id, msg_id, banner)
    .reply_markup(teloxide::types::InlineKeyboardMarkup::default())
    .await;
```

- [ ] **Step 4: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot background_continuation_tests
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
devenv shell -- git add crates/bot/src/telegram/worker.rs
devenv shell -- git commit -m "feat(bot): show shutdown background banner"
```

### Task 4: Persist Cron Shutdown Interruptions

**Files:**
- Modify: `crates/bot/src/cron.rs`
- Test: `crates/bot/src/cron.rs`

- [ ] **Step 1: Write failing tests for cron interruption persistence**

In `crates/bot/src/cron.rs`, add tests inside the existing `target_snapshot_tests` module so they can reuse `migrated_conn()`:

```rust
#[test]
fn mark_cron_interrupted_by_shutdown_sets_failed_pending_delivery_for_target() {
    let (_dir, conn) = target_snapshot_tests::migrated_conn();
    let spec = right_agent::cron_spec::CronSpec {
        schedule_kind: right_agent::cron_spec::ScheduleKind::Recurring("*/5 * * * *".into()),
        prompt: "p".into(),
        lock_ttl: None,
        max_budget_usd: 1.0,
        triggered_at: None,
        target_chat_id: Some(-777),
        target_thread_id: Some(13),
    };
    insert_running_run(
        &conn,
        "run-1",
        "job-x",
        "2026-05-05T12:00:00Z",
        "/sandbox/crons/logs/job-x-run-1.ndjson",
        &spec,
    )
    .unwrap();

    let updated = mark_cron_interrupted_by_shutdown(&conn, "job-x", "shutdown timeout").unwrap();

    assert_eq!(updated, 1);
    let row: (String, String, String, String) = conn
        .query_row(
            "SELECT status, delivery_status, COALESCE(run_note, ''), COALESCE(error_json, '') \
             FROM async_runs WHERE id = 'run-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row.0, "failed");
    assert_eq!(row.1, "pending");
    assert!(row.2.contains("job-x"));
    assert!(row.3.contains("shutdown timeout"));
}

#[test]
fn mark_cron_interrupted_by_shutdown_uses_none_delivery_for_targetless() {
    let (_dir, conn) = target_snapshot_tests::migrated_conn();
    let spec = right_agent::cron_spec::CronSpec {
        schedule_kind: right_agent::cron_spec::ScheduleKind::Recurring("*/5 * * * *".into()),
        prompt: "p".into(),
        lock_ttl: None,
        max_budget_usd: 1.0,
        triggered_at: None,
        target_chat_id: None,
        target_thread_id: None,
    };
    insert_running_run(&conn, "run-2", "job-y", "2026-05-05T12:00:00Z", "/log/path", &spec)
        .unwrap();

    let updated = mark_cron_interrupted_by_shutdown(&conn, "job-y", "shutdown timeout").unwrap();

    assert_eq!(updated, 1);
    let row: (String, i64, String) = conn
        .query_row(
            "SELECT status, delivery_required, delivery_status FROM async_runs WHERE id = 'run-2'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, ("failed".to_string(), 0, "none".to_string()));
}
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
devenv shell -- cargo test -p right-bot mark_cron_interrupted_by_shutdown
```

Expected: fail because `mark_cron_interrupted_by_shutdown` does not exist.

- [ ] **Step 3: Implement cron interruption helper**

In `crates/bot/src/cron.rs`, add:

```rust
fn cron_shutdown_failure_payload(
    run_id: &str,
    job_name: &str,
    reason: &str,
) -> Result<(String, String, String), rusqlite::Error> {
    let content = format!(
        "Cron job `{job_name}` was interrupted because the bot is shutting down. Run `{run_id}` did not finish."
    );
    let delivery_json = notify_delivery_json(&content, None)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let run_note = format!("Cron job `{job_name}` interrupted by shutdown");
    let error_json = serde_json::json!({
        "kind": "cron_shutdown_interrupted",
        "run_id": run_id,
        "job_name": job_name,
        "reason": reason,
    })
    .to_string();
    Ok((run_note, delivery_json, error_json))
}

fn mark_cron_interrupted_by_shutdown(
    conn: &rusqlite::Connection,
    job_name: &str,
    reason: &str,
) -> Result<usize, rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    let rows = {
        let mut stmt = tx.prepare(
            "SELECT id, target_chat_id
             FROM async_runs
             WHERE kind = 'cron'
               AND producer_ref = ?1
               AND status = 'running'",
        )?;
        stmt.query_map([job_name], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut updated = 0usize;
    let now = chrono::Utc::now().to_rfc3339();
    for (run_id, target_chat_id) in rows {
        let delivery_required = target_chat_id != 0;
        let (run_note, delivery_json, error_json) =
            cron_shutdown_failure_payload(&run_id, job_name, reason)?;
        let changed = tx.execute(
            "UPDATE async_runs
             SET run_note = ?2,
                 delivery_json = ?3,
                 error_json = ?4,
                 delivery_required = ?5,
                 delivery_status = ?6,
                 finished_at = ?7,
                 exit_code = NULL,
                 status = 'failed',
                 updated_at = ?7
             WHERE id = ?1
               AND kind = 'cron'
               AND status = 'running'",
            rusqlite::params![
                run_id,
                run_note,
                if delivery_required { Some(delivery_json) } else { None },
                error_json,
                delivery_required,
                if delivery_required { "pending" } else { "none" },
                now,
            ],
        )?;
        updated += changed;
    }
    tx.commit()?;
    Ok(updated)
}
```

- [ ] **Step 4: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot mark_cron_interrupted_by_shutdown
devenv shell -- cargo test -p right-bot target_snapshot_tests
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
devenv shell -- git add crates/bot/src/cron.rs
devenv shell -- git commit -m "feat(cron): persist shutdown interruptions"
```

### Task 5: Abort Timed-Out Owned Cron Jobs Safely

**Files:**
- Modify: `crates/bot/src/cron.rs`
- Test: `crates/bot/src/cron.rs`

- [ ] **Step 1: Write failing test for timeout branch ownership**

Add a unit test around a small helper rather than spawning real Claude:

```rust
#[test]
fn pending_execute_handle_retains_job_name_for_shutdown_marking() {
    let handle = PendingExecuteHandle::new_for_test("job-a");
    assert_eq!(handle.job_name(), "job-a");
}
```

- [ ] **Step 2: Run failing test**

Run:

```bash
devenv shell -- cargo test -p right-bot pending_execute_handle_retains_job_name_for_shutdown_marking
```

Expected: fail because `PendingExecuteHandle` does not exist.

- [ ] **Step 3: Replace tuple handle storage with a named struct**

In `crates/bot/src/cron.rs`, replace:

```rust
type ExecuteHandles = Arc<std::sync::Mutex<Vec<(String, JoinHandle<()>)>>>;
```

with:

```rust
struct PendingExecuteHandle {
    job_name: String,
    handle: JoinHandle<()>,
}

impl PendingExecuteHandle {
    #[cfg(test)]
    fn new_for_test(job_name: &str) -> Self {
        Self {
            job_name: job_name.to_owned(),
            handle: tokio::spawn(async {}),
        }
    }

    fn job_name(&self) -> &str {
        &self.job_name
    }
}

type ExecuteHandles = Arc<std::sync::Mutex<Vec<PendingExecuteHandle>>>;
```

Update push sites in `reconcile_jobs` and `run_job_loop` from:

```rust
guard.push((name.clone(), handle));
```

to:

```rust
guard.push(PendingExecuteHandle {
    job_name: name.clone(),
    handle,
});
```

Update shutdown draining:

```rust
let pending: Vec<PendingExecuteHandle> = {
    let mut guard = execute_handles
        .lock()
        .expect("execute_handles mutex poisoned");
    guard.drain(..).filter(|h| !h.handle.is_finished()).collect()
};
```

Use a mutable handle in the timeout so it can be aborted after timeout:

```rust
for mut pending_handle in pending {
    let name = pending_handle.job_name.clone();
    match tokio::time::timeout(SHUTDOWN_JOB_TIMEOUT, &mut pending_handle.handle).await {
        Ok(Ok(())) => {
            tracing::info!(job = %name, "cron shutdown: job finished cleanly");
        }
        Ok(Err(e)) => {
            tracing::warn!(job = %name, "cron shutdown: job panicked: {e}");
        }
        Err(_) => {
            tracing::warn!(
                job = %name,
                timeout_secs = SHUTDOWN_JOB_TIMEOUT.as_secs(),
                "cron shutdown: job timed out, aborting and marking interrupted"
            );
            pending_handle.handle.abort();
            match right_db::open_connection(&agent_dir, false) {
                Ok(conn) => {
                    if let Err(e) =
                        mark_cron_interrupted_by_shutdown(&conn, &name, "shutdown timeout")
                    {
                        tracing::error!(job = %name, "cron shutdown: mark interrupted failed: {e:#}");
                    }
                }
                Err(e) => tracing::error!(
                    job = %name,
                    "cron shutdown: DB open to mark interrupted failed: {e:#}"
                ),
            }
        }
    }
}
```

- [ ] **Step 4: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot pending_execute_handle_retains_job_name_for_shutdown_marking
devenv shell -- cargo test -p right-bot shutdown_completes_promptly_with_scheduled_jobs
devenv shell -- cargo test -p right-bot mark_cron_interrupted_by_shutdown
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
devenv shell -- git add crates/bot/src/cron.rs
devenv shell -- git commit -m "fix(cron): abort owned jobs after shutdown timeout"
```

### Task 6: Delivery Shutdown Flush

**Files:**
- Modify: `crates/bot/src/async_delivery.rs`
- Modify: `crates/bot/src/lib.rs`
- Test: `crates/bot/src/async_delivery.rs`

- [ ] **Step 1: Write failing selection test for shutdown flush**

In `crates/bot/src/async_delivery.rs`, add a pure helper test:

```rust
#[test]
fn delivery_mode_shutdown_flush_skips_idle_gate() {
    assert!(should_wait_for_idle(DeliveryMode::Normal, 10));
    assert!(!should_wait_for_idle(DeliveryMode::ShutdownFlush, 10));
}
```

- [ ] **Step 2: Run failing test**

Run:

```bash
devenv shell -- cargo test -p right-bot delivery_mode_shutdown_flush_skips_idle_gate
```

Expected: fail because `DeliveryMode` and `should_wait_for_idle` do not exist.

- [ ] **Step 3: Add delivery mode helpers**

In `crates/bot/src/async_delivery.rs`, add near constants:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryMode {
    Normal,
    ShutdownFlush,
}

fn should_wait_for_idle(mode: DeliveryMode, idle_for: i64) -> bool {
    mode == DeliveryMode::Normal && idle_for < IDLE_THRESHOLD_SECS
}
```

Extract the body that handles one pending result into:

```rust
struct DeliveryLoopState {
    delivered_in_memory: HashSet<String>,
    attempt_counts: std::collections::HashMap<String, u32>,
}

impl DeliveryLoopState {
    fn new() -> Self {
        Self {
            delivered_in_memory: HashSet::new(),
            attempt_counts: std::collections::HashMap::new(),
        }
    }
}
```

Create:

```rust
async fn run_delivery_once(
    conn: &rusqlite::Connection,
    state: &mut DeliveryLoopState,
    mode: DeliveryMode,
    agent_dir: &Path,
    agent_name: &str,
    model: &Arc<arc_swap::ArcSwap<Option<String>>>,
    bot: &crate::telegram::BotType,
    allowlist: &right_agent::agent::allowlist::AllowlistHandle,
    idle_ts: &Arc<IdleTimestamp>,
    ssh_config_path: Option<&Path>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<&str>,
    upgrade_lock: &Arc<tokio::sync::RwLock<()>>,
    session_locks: &crate::telegram::SessionLocks,
    debug: &Arc<std::sync::atomic::AtomicBool>,
    learning: &right_agent::agent::types::LearningConfig,
    learning_drain_scheduler: &Arc<crate::learning_episode::DrainScheduler>,
) -> bool {
    let pending = match fetch_next_pending(conn, &state.delivered_in_memory) {
        Ok(Some(p)) => p,
        Ok(None) => return false,
        Err(e) => {
            tracing::error!("async delivery: fetch_next_pending failed: {e:#}");
            return false;
        }
    };

    let last = idle_ts.0.load(std::sync::atomic::Ordering::Relaxed);
    let now = chrono::Utc::now().timestamp();
    let idle_for = now - last;
    if should_wait_for_idle(mode, idle_for) {
        tracing::info!(
            kind = %pending.kind,
            producer_ref = ?pending.producer_ref,
            run_id = %pending.id,
            idle_secs = idle_for,
            wait_secs = IDLE_THRESHOLD_SECS - idle_for,
            "async delivery: result pending, waiting for chat idle ({IDLE_THRESHOLD_SECS}s)"
        );
        return false;
    }

    // The implementation body is the current code in run_delivery_loop from
    // `let (to_deliver, skipped) = match select_delivery_candidate(&conn, pending)`
    // through the end of the delivery match. Keep every existing branch and
    // replace local map references with state.delivered_in_memory and
    // state.attempt_counts.
    true
}
```

Refactor `run_delivery_loop` to own `let mut state = DeliveryLoopState::new();` and call `run_delivery_once` with `DeliveryMode::Normal` each poll.

Add public shutdown flush:

```rust
#[allow(clippy::too_many_arguments)]
pub(crate) async fn flush_ready_deliveries_for_shutdown(
    agent_dir: PathBuf,
    agent_name: String,
    model: Arc<arc_swap::ArcSwap<Option<String>>>,
    bot: crate::telegram::BotType,
    allowlist: right_agent::agent::allowlist::AllowlistHandle,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<String>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    session_locks: crate::telegram::SessionLocks,
    debug: Arc<std::sync::atomic::AtomicBool>,
    learning: right_agent::agent::types::LearningConfig,
    learning_drain_scheduler: Arc<crate::learning_episode::DrainScheduler>,
) {
    let conn = match right_db::open_connection(&agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("async delivery shutdown flush: DB open failed: {e:#}");
            return;
        }
    };
    let mut state = DeliveryLoopState::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("async delivery shutdown flush timed out");
            return;
        }
        let delivered = run_delivery_once(
            &conn,
            &mut state,
            DeliveryMode::ShutdownFlush,
            &agent_dir,
            &agent_name,
            &model,
            &bot,
            &allowlist,
            &idle_ts,
            ssh_config_path.as_deref(),
            &internal_client,
            resolved_sandbox.as_deref(),
            &upgrade_lock,
            &session_locks,
            &debug,
            &learning,
            &learning_drain_scheduler,
        )
        .await;
        if !delivered {
            return;
        }
    }
}
```

- [ ] **Step 4: Wire shutdown flush in `lib.rs`**

Before spawning `run_delivery_loop`, clone the exact arguments needed for the flush:

```rust
let delivery_flush_args = (
    delivery_agent_dir.clone(),
    delivery_agent_name.clone(),
    Arc::clone(&delivery_model),
    delivery_bot.clone(),
    delivery_allowlist.clone(),
    Arc::clone(&delivery_idle_ts),
    delivery_ssh_config.clone(),
    Arc::clone(&delivery_internal_client),
    delivery_sandbox.clone(),
    Arc::clone(&delivery_upgrade_lock),
    Arc::clone(&delivery_session_locks),
    Arc::clone(&delivery_debug),
    delivery_learning.clone(),
    Arc::clone(&delivery_learning_drain_scheduler),
);
```

After cron join and before awaiting the delivery loop handle, call:

```rust
tracing::info!("flushing ready async deliveries for shutdown");
let (
    flush_agent_dir,
    flush_agent_name,
    flush_model,
    flush_bot,
    flush_allowlist,
    flush_idle_ts,
    flush_ssh_config,
    flush_internal_client,
    flush_resolved_sandbox,
    flush_upgrade_lock,
    flush_session_locks,
    flush_debug,
    flush_learning,
    flush_learning_drain_scheduler,
) = delivery_flush_args;
async_delivery::flush_ready_deliveries_for_shutdown(
    flush_agent_dir,
    flush_agent_name,
    flush_model,
    flush_bot,
    flush_allowlist,
    flush_idle_ts,
    flush_ssh_config,
    flush_internal_client,
    flush_resolved_sandbox,
    flush_upgrade_lock,
    flush_session_locks,
    flush_debug,
    flush_learning,
    flush_learning_drain_scheduler,
)
.await;
```

- [ ] **Step 5: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot delivery_mode_shutdown_flush_skips_idle_gate
devenv shell -- cargo test -p right-bot async_delivery
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
devenv shell -- git add crates/bot/src/async_delivery.rs crates/bot/src/lib.rs
devenv shell -- git commit -m "feat(bot): flush ready deliveries on shutdown"
```

### Task 7: Architecture Documentation

**Files:**
- Modify: `docs/architecture/sessions.md`
- Modify: `docs/architecture/lifecycle.md`
- Modify: `ARCHITECTURE.md` only if Task 1-6 introduced a new prescriptive rule.

- [ ] **Step 1: Update sessions doc**

In `docs/architecture/sessions.md`, add a short paragraph near the thinking/background handoff section:

```markdown
Process shutdown (`SIGINT`/`SIGTERM`) requests background handoff for active
foreground Telegram turns instead of dropping them. The worker uses the same
`async_runs kind='background'` continuation path as the Background button, but
with shutdown-specific logs and banner text. If handoff cannot be confirmed by
the continuation's `system/init`, the background row is marked failed with
pending delivery.
```

Add near cron/delivery:

```markdown
During shutdown, cron schedulers stop creating new runs. Running cron jobs get
the bounded `SHUTDOWN_JOB_TIMEOUT` drain. Jobs still running after that timeout
are aborted by the owning bot process and marked failed with a
`cron_shutdown_interrupted` error payload; targeted runs remain pending for
delivery. The shutdown delivery flush sends already-terminal pending async
results without waiting for chat-idle politeness, then exits.
```

- [ ] **Step 2: Update lifecycle doc**

In `docs/architecture/lifecycle.md`, add a shutdown branch under `right bot --agent <name>`:

```markdown
On `SIGINT`/`SIGTERM`:
  - Stop accepting Telegram updates
  - Request shutdown background handoff for active foreground turns
  - Wait briefly for foreground handoff gates to drain
  - Stop cron schedulers and bounded-drain running cron jobs
  - Mark owned timed-out cron runs as shutdown-interrupted failures
  - Flush already-ready async deliveries without idle-delay politeness
  - Tear down SSH control master and exit
```

- [ ] **Step 3: Check docs diff**

Run:

```bash
devenv shell -- git diff -- docs/architecture/sessions.md docs/architecture/lifecycle.md ARCHITECTURE.md
devenv shell -- git diff --check
```

Expected: docs describe the implemented behavior and diff check passes.

- [ ] **Step 4: Commit**

```bash
devenv shell -- git add docs/architecture/sessions.md docs/architecture/lifecycle.md ARCHITECTURE.md
devenv shell -- git commit -m "docs: document hybrid shutdown"
```

If `ARCHITECTURE.md` is unchanged, omit it from `git add`.

### Task 8: Final Verification

**Files:**
- Verify all modified files.

- [ ] **Step 1: Run targeted regression suites**

Run:

```bash
devenv shell -- cargo test -p right-bot background_continuation_tests
devenv shell -- cargo test -p right-bot bg_request_race_tests
devenv shell -- cargo test -p right-bot shutdown_request_tests
devenv shell -- cargo test -p right-bot mark_cron_interrupted_by_shutdown
devenv shell -- cargo test -p right-bot async_delivery
```

Expected: all pass.

- [ ] **Step 2: Run full workspace verification**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: pass. If failures are pre-existing, cite the recorded baseline. If failures are new, fix before completion.

- [ ] **Step 3: Check final git state**

Run:

```bash
devenv shell -- git status --short
```

Expected: clean worktree after commits, or only intentional uncommitted changes explicitly called out.
