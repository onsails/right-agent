---
phase: 25-telegram-handler-cc-dispatch
plan: "02"
subsystem: bot/telegram/worker
tags: [tdd, worker, debounce, cc-dispatch, typing-indicator, reply-tool]
dependency_graph:
  requires: [25-01]
  provides: [worker.rs with DebounceMsg/WorkerContext/spawn_worker]
  affects: [crates/bot/src/telegram/worker.rs, crates/bot/src/telegram/mod.rs]
tech_stack:
  added: [chrono 0.4 (workspace dep), serde (bot dep)]
  patterns: [TDD pure helpers, CancellationToken typing indicator, tokio::process CC subprocess]
key_files:
  created:
    - crates/bot/src/telegram/worker.rs
  modified:
    - crates/bot/src/telegram/mod.rs
    - Cargo.toml
    - crates/bot/Cargo.toml
decisions:
  - "Use 'y' not 'x' as repeated char in stderr truncation test — 'exit' contains 'x' causing +1 collision"
  - "Remove CcOutput struct — parse_reply_tool uses serde_json::Value directly, no typed struct needed"
  - "reply_to_message_id sent via ReplyParameters { message_id, ..Default::default() } per teloxide 0.13 API"
  - "ThreadId(MessageId(n)) construction requires i64→i32 cast from eff_thread_id"
metrics:
  duration: "7 minutes"
  completed: "2026-04-01"
  tasks: 2
  files: 4
---

# Phase 25 Plan 02: Worker Task (Debounce + CC Dispatch) Summary

Per-session worker task implementing: debounce loop, CC subprocess invocation, reply tool parsing, typing indicator, and text splitting. All pure helpers covered by 11 TDD unit tests.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Implement pure helpers (TDD GREEN) | 3718c80 | worker.rs (new), mod.rs, Cargo.toml x2 |
| 2 | Implement spawn_worker async function | 06f06cc | worker.rs |

## What Was Built

**`crates/bot/src/telegram/worker.rs`** — 420 LoC, exports:

- `DebounceMsg` — queued Telegram message with message_id, text, timestamp
- `WorkerContext` — per-worker context: chat_id, effective_thread_id, agent_dir, bot, db_path
- `ReplyOutput` — parsed `reply` tool call: content, reply_to_message_id, media_paths (STUB)
- `format_batch_xml(msgs)` — XML batch format per D-02; escapes `<`, `>`, `&`
- `split_message(text)` — splits at last `\n` in 200-char window; hard-cuts at 4096
- `format_error_reply(code, stderr)` — truncates stderr at 300 chars, formats `⚠️ Agent error`
- `parse_reply_tool(json)` — parses CC `--output-format json` for `reply` tool_use block; searches `result[]` and `content[]` arrays
- `spawn_worker(key, ctx, map)` — spawns tokio task: 500ms debounce, typing indicator (CancellationToken), CC invocation, reply send
- `invoke_cc(xml, chat_id, eff_thread_id, ctx)` — resolves `claude`/`claude-bun`, session lookup/create, CC subprocess with all required flags

**`crates/bot/src/telegram/mod.rs`** — added `pub mod worker` and `BotType` alias.

## Test Results

```
test result: ok. 20 passed; 0 failed; 0 ignored
```

11 worker tests (pure helpers) + 9 session tests from plan 01 — all green.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Test collision: 'x' in "exit" caused stderr truncation test to yield 301 instead of 300**
- **Found during:** Task 1 RED phase
- **Issue:** Test used `"x".repeat(500)` and counted all `'x'` chars in output. The word "e**x**it" in `format_error_reply`'s output template contributes one extra `'x'`.
- **Fix:** Changed test to use `"y".repeat(500)` — no collision with any format string chars
- **Files modified:** crates/bot/src/telegram/worker.rs (test only)
- **Commit:** 3718c80

**2. [Rule 1 - Bug] Unused `CcOutput` struct removed**
- **Found during:** Task 2 build
- **Issue:** Plan included a `CcOutput` serde struct that was never instantiated — `parse_reply_tool` uses `serde_json::Value` directly throughout
- **Fix:** Removed the unused struct
- **Files modified:** crates/bot/src/telegram/worker.rs
- **Commit:** 06f06cc

**3. [Rule 3 - Blocking] teloxide 0.13 uses `ReplyParameters` not `reply_to_message_id`**
- **Found during:** Task 2 implementation
- **Issue:** Plan used `.reply_to_message_id(ref_id)` method which doesn't exist in teloxide 0.13. The API changed to `reply_parameters(ReplyParameters { message_id: MessageId(id), ..Default::default() })`
- **Fix:** Used correct `ReplyParameters` API
- **Files modified:** crates/bot/src/telegram/worker.rs
- **Commit:** 06f06cc

## Known Stubs

- `media_paths` in `ReplyOutput`: warns and skips — file sending deferred to later phase per plan spec

## Self-Check: PASSED

- `crates/bot/src/telegram/worker.rs`: FOUND
- Commit `3718c80`: FOUND
- Commit `06f06cc`: FOUND
- `cargo test -p rightclaw-bot --lib`: 20/20 pass
