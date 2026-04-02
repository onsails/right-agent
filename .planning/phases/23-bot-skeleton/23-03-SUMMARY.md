---
phase: 23-bot-skeleton
plan: 03
subsystem: bot
tags: [teloxide, telegram, bot, cli, clap, subcommand]

# Dependency graph
requires:
  - phase: 23-02
    provides: rightclaw_bot::run(BotArgs) entry point, BotArgs struct
  - phase: 23-01
    provides: allowed_chat_ids field in AgentConfig
provides:
  - Commands::Bot { agent } variant in rightclaw-cli Commands enum
  - rightclaw bot --agent <name> CLI surface
  - rightclaw-bot dependency wired in rightclaw-cli/Cargo.toml
affects: [26-bot-process-compose, 25-bot-dispatch]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "async fn run() in bot crate — no nested runtime builder, caller .await-s directly"
    - "CLI Bot variant dispatches to rightclaw_bot::run().await matching other async arms"

key-files:
  created: []
  modified:
    - crates/rightclaw-cli/Cargo.toml
    - crates/rightclaw-cli/src/main.rs
    - crates/bot/src/lib.rs

key-decisions:
  - "Convert bot::run() from sync (block_on) to pub async fn — avoids nested runtime collision with #[tokio::main] CLI main"
  - "Bot dispatch arm uses .await directly — consistent with Up/Down/Status/Restart arms"

patterns-established:
  - "Bot crate run() is async — callers .await it, no internal runtime construction"

requirements-completed: [BOT-01]

# Metrics
duration: 5min
completed: 2026-03-31
---

# Phase 23 Plan 03: Wire rightclaw bot subcommand into CLI Summary

**`rightclaw bot --agent <name>` wired end-to-end: Commands::Bot variant in CLI, rightclaw-bot dep in Cargo.toml, run() converted to async fn to avoid nested tokio runtime**

## Performance

- **Duration:** ~5 min
- **Started:** 2026-03-31T21:20:00Z
- **Completed:** 2026-03-31T21:25:00Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments

- `rightclaw bot --agent <name>` recognized by CLI binary and shows in `--help`
- `bot --help` shows `--agent <AGENT>` argument
- Agent-not-found case exits 1 with readable miette error (no panic)
- `run()` in `crates/bot/src/lib.rs` converted to `pub async fn` — removes nested tokio runtime anti-pattern
- `cargo build --workspace` exits 0

## Task Commits

1. **Task 1+2: Add Bot variant, Cargo dep, async run(), smoke test** - `ac61a06` (feat)

## Files Created/Modified

- `crates/rightclaw-cli/Cargo.toml` — Added `rightclaw-bot = { path = "../bot" }`
- `crates/rightclaw-cli/src/main.rs` — Added `Commands::Bot { agent: String }` variant and `.await` dispatch arm
- `crates/bot/src/lib.rs` — Converted `run()` from sync (internal `block_on`) to `pub async fn run()`

## Decisions Made

- Converted `bot::run()` to `pub async fn` — avoids nested tokio runtime construction (plan noted this as "Best approach"). The CLI's `#[tokio::main]` already owns the runtime; `run()` just needs to `.await`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Converted run() to async to avoid nested runtime**
- **Found during:** Task 1 (reading lib.rs)
- **Issue:** `run()` was sync and used `tokio::runtime::Builder::new_multi_thread().block_on()` internally. Calling this from an `#[tokio::main]` async context would panic with "Cannot start a runtime from within a runtime."
- **Fix:** Replaced sync `run()` with `pub async fn run() { run_async(args).await }`. The internal runtime builder is removed. CLI arm calls `.await`.
- **Files modified:** `crates/bot/src/lib.rs`
- **Verification:** `cargo build --workspace` exits 0; `rightclaw bot --agent nosuchagent` exits 1 with miette error (no panic)
- **Committed in:** `ac61a06`

---

**Total deviations:** 1 auto-fixed (Rule 1 - bug in run() sync/async mismatch)
**Impact on plan:** Fix was pre-noted in plan as "Best approach". No scope change.

## Issues Encountered

None beyond the async conversion noted above (which the plan itself flagged as the recommended approach).

## Next Phase Readiness

- `rightclaw bot --agent <name>` is a fully wired CLI entry point
- Phase 26 (process-compose) can generate `rightclaw bot --agent <name>` as the process command for each bot process
- Phase 25 (bot-dispatch) can add message handlers inside `dispatch.rs` without touching CLI wiring

## Known Stubs

None — this plan's goal (CLI surface) is fully achieved. The bot dispatch logic (Phase 25 stub in dispatch.rs) is documented in 23-02-SUMMARY.md.

## Self-Check: PASSED
