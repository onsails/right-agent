---
phase: 23-bot-skeleton
plan: 02
subsystem: bot
tags: [teloxide, telegram, bot, signal-handling, dispatcher]

# Dependency graph
requires:
  - AgentConfig.allowed_chat_ids (from 23-01)
  - rightclaw::memory::open_connection
  - rightclaw::agent::discovery::parse_agent_config
  - rightclaw::config::resolve_home
provides:
  - rightclaw_bot::run(BotArgs) entry point
  - rightclaw_bot::BotArgs struct
  - rightclaw_bot::BotError enum
  - telegram::resolve_token 4-step priority chain
  - telegram::run_telegram with full graceful shutdown
  - CacheMe<Throttle<Bot>> adaptor ordering confirmed
affects: [23-03, 25-bot-dispatch, 26-bot-process-compose]

# Tech tracking
tech-stack:
  added:
    - teloxide 0.17 (default-features=false, features: macros, throttle, cache-me)
    - vecrem 0.1 (transitive via teloxide throttle)
  patterns:
    - CacheMe<Throttle<Bot>> adaptor ordering (Throttle inner, CacheMe outer)
    - dptree filter_map for chat_id allow-list silencing
    - ShutdownToken extracted before dispatch(), passed to signal handler task
    - tokio::select! on SIGTERM + SIGINT for unified shutdown path

key-files:
  created:
    - crates/bot/Cargo.toml
    - crates/bot/src/lib.rs
    - crates/bot/src/error.rs
    - crates/bot/src/telegram/mod.rs
    - crates/bot/src/telegram/bot.rs
    - crates/bot/src/telegram/filter.rs
    - crates/bot/src/telegram/dispatch.rs
  modified:
    - Cargo.toml (workspace members + teloxide workspace dep)
    - Cargo.lock

key-decisions:
  - "teloxide workspace dep uses features=[macros, throttle, cache-me] — default-features=false to avoid ctrlc_handler (anti-pattern #2)"
  - "cache-me feature name uses hyphen not underscore (cargo rejects cache_me)"
  - "AgentConfig::default() unavailable — fallback uses explicit struct literal in unwrap_or_else"

patterns-established:
  - "Teloxide adaptors: .throttle(Limits::default()).cache_me() — method chain gives inside-out ordering automatically"
  - "ShutdownToken.shutdown() Err case handled as IdleShutdownError (already stopped), not panicked"
  - "Arc<Mutex<Vec<tokio::process::Child>>> wired empty in Phase 23, ready for Phase 25 population"

requirements-completed: [BOT-01, BOT-03, BOT-04, BOT-05]

# Metrics
duration: 4min
completed: 2026-03-31
---

# Phase 23 Plan 02: rightclaw-bot crate skeleton Summary

**`rightclaw-bot` crate with full teloxide long-polling skeleton: CacheMe<Throttle<Bot>> adaptor ordering, chat_id allow-list filtering, SIGTERM/SIGINT graceful shutdown with ShutdownToken, and Arc<Mutex<Vec<Child>>> subprocess tracking wired for Phase 25**

## Performance

- **Duration:** ~4 min
- **Started:** 2026-03-31T21:13:53Z
- **Completed:** 2026-03-31T21:17:47Z
- **Tasks:** 2
- **Files created/modified:** 9

## Accomplishments

- New `crates/bot` workspace crate (`rightclaw-bot`) compiles clean
- `BotArgs { agent, home }` and `BotError` (thiserror) publicly exported
- `telegram::resolve_token` implements 4-step priority chain: RC_TELEGRAM_TOKEN env → RC_TELEGRAM_TOKEN_FILE env → agent.yaml telegram_token_file → agent.yaml telegram_token
- `CacheMe<Throttle<Bot>>` adaptor ordering confirmed in `bot.rs`
- `make_chat_id_filter` in `filter.rs` — empty HashSet blocks all (D-05), no log on silenced messages (D-06)
- `run_telegram` in `dispatch.rs` — ShutdownToken extracted before `dispatch().await`, SIGTERM+SIGINT unified shutdown via `tokio::select!`, `Arc<Mutex<Vec<Child>>>` wired empty
- `ShutdownToken::shutdown()` Err case handled as debug log (Pitfall 4 avoided)
- `cargo build --workspace` and `cargo test -p rightclaw-bot` both exit 0

## Task Commits

1. **Task 1+2: Scaffold crates/bot + telegram/ submodule** - `55f2b2c` (feat)
2. **Cargo.lock update** - `874874c` (chore)

## Files Created/Modified

- `Cargo.toml` — Added `"crates/bot"` to members; teloxide 0.17 with throttle + cache-me features
- `Cargo.lock` — Added teloxide 0.17, vecrem 0.1 lockfile entries
- `crates/bot/Cargo.toml` — rightclaw-bot package manifest
- `crates/bot/src/lib.rs` — `BotArgs`, `run()` entry point, `run_async()` full setup chain
- `crates/bot/src/error.rs` — `BotError` with AgentNotFound, NoToken, DbError, ConfigError, SignalError
- `crates/bot/src/telegram/mod.rs` — `resolve_token` 4-step chain, re-exports
- `crates/bot/src/telegram/bot.rs` — `build_bot()` returning `CacheMe<Throttle<Bot>>`
- `crates/bot/src/telegram/filter.rs` — `make_chat_id_filter()` with HashSet<i64>
- `crates/bot/src/telegram/dispatch.rs` — `run_telegram()` with ShutdownToken, signal handling, subprocess Arc

## Decisions Made

- teloxide workspace dep uses `features = ["macros", "throttle", "cache-me"]` — `default-features = false` to avoid pulling in `ctrlc_handler` feature (per anti-pattern #2 in research)
- Feature name is `cache-me` (hyphen), not `cache_me` — caught by cargo at resolve time (deviation)
- `AgentConfig` does not derive `Default` — `parse_agent_config` None case uses explicit struct literal in `unwrap_or_else` per plan fallback guidance

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Missing teloxide feature flags**
- **Found during:** Task 1 build attempt
- **Issue:** `teloxide = { version = "0.17", default-features = false, features = ["macros"] }` in workspace Cargo.toml did not include `throttle` and `cache-me` features. The `bot.rs` imports (`CacheMe`, `Throttle`, `Limits`) failed to resolve.
- **Fix:** Added `"throttle"` and `"cache-me"` to teloxide workspace features. Discovered `cache_me` (underscore) is rejected by cargo — correct name is `cache-me` (hyphen).
- **Files modified:** `Cargo.toml`
- **Commit:** `55f2b2c` (same task commit)

---

**Total deviations:** 1 auto-fixed (Rule 3 - missing feature flags blocked compilation)
**Impact on plan:** One extra iteration; added 2 features to workspace dep. No scope change.

## Issues Encountered

None beyond the feature flag deviation above.

## Next Phase Readiness

- `rightclaw_bot::run(BotArgs)` is publicly callable from `rightclaw-cli` (Plan 03)
- `telegram::run_telegram` accepts `Vec<i64>` allowed_chat_ids — Plan 03 CLI wiring can pass this directly
- `Arc<Mutex<Vec<Child>>>` wired in dispatch.rs — Phase 25 populates it without restructuring
- ShutdownToken and signal handler task structure are final — Phase 25 only adds message dispatch handlers

## Known Stubs

- `dispatch.rs` schema uses no-op `.endpoint(|_msg: Message| async { respond(()) })` — intentional Phase 23 stub. Phase 25 replaces with real message dispatch. Does not prevent plan goal (bot skeleton compiles and runs).

## Self-Check: PASSED
