---
phase: 25-telegram-handler-cc-dispatch
plan: "03"
subsystem: bot
tags: [teloxide, dispatch, handler, dashmap, botcommand]
dependency_graph:
  requires: [25-01, 25-02]
  provides: [complete-telegram-dispatch-loop]
  affects: [crates/bot/src/telegram/dispatch.rs, crates/bot/src/telegram/handler.rs, crates/bot/src/lib.rs]
tech_stack:
  added: []
  patterns: [DashMap-per-session-worker-map, dptree-dependency-injection, BotCommands-derive]
key_files:
  created:
    - crates/bot/src/telegram/handler.rs
  modified:
    - crates/bot/src/telegram/dispatch.rs
    - crates/bot/src/telegram/mod.rs
    - crates/bot/src/telegram/worker.rs
    - crates/bot/src/lib.rs
decisions:
  - "DashMap guard released before .await in handle_message — sender cloned out before send to prevent lock held across yield point"
  - "DebounceMsg derives Clone to support retry-loop in handle_message (send failure → remove + respawn)"
  - "No children registry in dispatch.rs — kill_on_drop(true) in invoke_cc is sufficient for BOT-04"
  - "set_my_commands called best-effort (.ok()) before dispatcher starts — /reset registration is non-fatal"
metrics:
  duration_seconds: 228
  completed_date: "2026-04-01"
  tasks_completed: 2
  files_changed: 5
---

# Phase 25 Plan 03: Telegram Handler and Dispatch Assembly Summary

Complete message dispatch loop assembled: `handler.rs` + rewritten `dispatch.rs` wire the building blocks from plans 01/02 into a working end-to-end bot.

## What Was Built

### handler.rs (new)
Teloxide endpoint handlers for Telegram message routing:

- `handle_message`: routes incoming text to per-session worker via DashMap. DashMap guard is released before `.await` (sender cloned out first) to avoid holding the lock across a yield point. Pitfall 7 mitigation: send failure → remove stale sender → respawn worker on next loop iteration.
- `handle_reset`: removes DashMap entry (closes channel, worker exits), deletes `telegram_sessions` row via `delete_session`. DB errors propagate via `map_err(...)? ` per CLAUDE.rust.md fail-fast rule. Sends confirmation reply to user.

### dispatch.rs (rewritten)
Full teloxide dispatcher replacing the Phase 23 no-op:

- `DashMap<SessionKey, mpsc::Sender<DebounceMsg>>` for per-session worker map
- `BotCommand` enum with `/reset` variant routed to `handle_reset`
- `run_telegram` now accepts `agent_dir: PathBuf` as third parameter
- `set_my_commands` registers `/reset` with Telegram Bot API (best-effort, `.ok()`)
- SIGTERM/SIGINT shutdown retained from Phase 23
- GOTCHA documented in module comment: queued messages in worker channel are lost on worker task panic (accepted trade-off)

### worker.rs (minor)
- `SessionKey` type alias made `pub` (was `type`, now `pub type`)
- `DebounceMsg` derives `Clone` (required for retry-loop in handler.rs)

### lib.rs
- `run_telegram` call updated to pass `agent_dir` as third argument

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| Task 1 | 732c9c5 | feat(25-03): create handler.rs with handle_message and handle_reset |
| Task 2 | cfe6333 | feat(25-03): rewrite dispatch.rs with DashMap worker map + BotCommand schema |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] RequestError::Io requires Arc<io::Error>**
- **Found during:** Task 1
- **Issue:** `teloxide-core 0.13.0` wraps `io::Error` in `Arc`. Plan template used bare `std::io::Error::other(...)` which does not match.
- **Fix:** Added `.into()` to convert `io::Error → Arc<io::Error>`.
- **Files modified:** `crates/bot/src/telegram/handler.rs`
- **Commit:** 732c9c5

**2. [Rule 1 - Bug] dptree::deps! macro rejects trailing comma**
- **Found during:** Task 2
- **Issue:** `dptree::deps![a, b,]` with trailing comma caused "unexpected end of macro invocation".
- **Fix:** Removed trailing comma.
- **Files modified:** `crates/bot/src/telegram/dispatch.rs`
- **Commit:** cfe6333

**3. [Rule 2 - Missing] DebounceMsg needs Clone for handle_message retry loop**
- **Found during:** Task 1 (compile error E0382)
- **Issue:** Plan specified Pitfall 7 retry loop that clones debounce_msg on send failure, but `DebounceMsg` did not implement `Clone`.
- **Fix:** Added `#[derive(Clone)]` to `DebounceMsg`.
- **Files modified:** `crates/bot/src/telegram/worker.rs`
- **Commit:** 732c9c5

**4. [Rule 2 - Missing] DashMap guard held across .await**
- **Found during:** Task 1 (design review)
- **Issue:** Plan template used `if let Some(sender) = worker_map.get(&key)` then `sender.send(...).await` — this holds the DashMap read guard across an await point, which would deadlock under concurrent access.
- **Fix:** Cloned the sender out of the guard before awaiting: `let maybe_tx = worker_map.get(&key).map(|entry| entry.value().clone())`.
- **Files modified:** `crates/bot/src/telegram/handler.rs`
- **Commit:** 732c9c5

## Known Stubs

None.

## Self-Check: PASSED

- handler.rs: FOUND
- dispatch.rs: FOUND
- SUMMARY.md: FOUND
- Commit 732c9c5: FOUND
- Commit cfe6333: FOUND
