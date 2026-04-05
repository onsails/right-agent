---
phase: 23-bot-skeleton
verified: 2026-03-31T21:40:00Z
status: passed
score: 10/10 must-haves verified
gaps: []
human_verification:
  - test: "rightclaw bot --agent <configured-agent> connects to Telegram and receives messages from allowed_chat_ids"
    expected: "Messages from listed chat IDs are processed; messages from unlisted IDs are silently dropped"
    why_human: "Requires live Telegram bot token, running bot process, and sending test messages — cannot verify programmatically without real credentials"
  - test: "SIGTERM from process-compose triggers clean shutdown within a reasonable time"
    expected: "Dispatcher stops cleanly, no leftover processes, exit code 0"
    why_human: "Requires a running bot instance and a coordinated signal delivery — cannot simulate in a unit test"
---

# Phase 23: bot-skeleton Verification Report

**Phase Goal:** Create the rightclaw-bot crate with full teloxide bot skeleton — allowed_chat_ids filter, CacheMe<Throttle<Bot>> adaptor ordering, SIGTERM+SIGINT shutdown, CLI subcommand `rightclaw bot --agent <name>`
**Verified:** 2026-03-31T21:40:00Z
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | `allowed_chat_ids: Vec<i64>` with `#[serde(default)]` exists on AgentConfig | VERIFIED | `crates/rightclaw/src/agent/types.rs` line 87-88 |
| 2 | Absent `allowed_chat_ids` in agent.yaml yields empty Vec (secure default) | VERIFIED | `test agent_config_allowed_chat_ids_defaults_to_empty` passes |
| 3 | Existing agent.yaml files without the field still deserialize without error | VERIFIED | `test agent_config_allowed_chat_ids_absent_does_not_reject` passes |
| 4 | rightclaw-bot crate compiles as a workspace member | VERIFIED | `cargo build --workspace` exits 0 in 0.24s (already built) |
| 5 | `run_telegram` constructs `CacheMe<Throttle<Bot>>` (not reversed) | VERIFIED | `crates/bot/src/telegram/bot.rs`: `.throttle(Limits::default()).cache_me()` — Throttle inner, CacheMe outer |
| 6 | SIGTERM and SIGINT both trigger the same graceful shutdown path | VERIFIED | `dispatch.rs` uses `tokio::select!` on `sigterm.recv()` and `tokio::signal::ctrl_c()`, both flow to same shutdown sequence |
| 7 | `ShutdownToken::shutdown()` Err case is handled (not unwrapped) | VERIFIED | `dispatch.rs` lines 71-79: `match shutdown_token.shutdown() { Ok(fut) => { ... } Err(_idle) => { tracing::debug!(...) } }` |
| 8 | `Arc<Mutex<Vec<tokio::process::Child>>>` shared state wired in dispatch.rs | VERIFIED | `dispatch.rs` line 34: `let children: Arc<Mutex<Vec<tokio::process::Child>>> = Arc::new(Mutex::new(Vec::new()))` |
| 9 | `rightclaw bot --agent <name>` subcommand recognized by CLI binary | VERIFIED | `./target/debug/rightclaw --help` shows `bot` subcommand; `bot --help` shows `--agent <AGENT>` |
| 10 | Empty `allowed_chat_ids` blocks all messages (filter_map returns None for every message) | VERIFIED | `filter.rs`: empty `HashSet` means `allowed.contains(&chat_id)` is always false → `None` returned |

**Score:** 10/10 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw/src/agent/types.rs` | AgentConfig with `allowed_chat_ids: Vec<i64>` | VERIFIED | Field at line 88 with `#[serde(default)]` at line 87 |
| `crates/bot/Cargo.toml` | rightclaw-bot crate manifest with teloxide dep | VERIFIED | `name = "rightclaw-bot"`, `teloxide = { workspace = true }` |
| `crates/bot/src/lib.rs` | `pub async fn run(args: BotArgs)` entry point | VERIFIED | Line 23: `pub async fn run(args: BotArgs) -> miette::Result<()>` |
| `crates/bot/src/error.rs` | BotError thiserror enum | VERIFIED | Contains `AgentNotFound`, `NoToken`, `DbError`, `ConfigError`, `SignalError` variants |
| `crates/bot/src/telegram/mod.rs` | `resolve_token` 4-step priority chain + re-exports | VERIFIED | Chain: RC_TELEGRAM_TOKEN env → RC_TELEGRAM_TOKEN_FILE env → agent.yaml file → agent.yaml token |
| `crates/bot/src/telegram/bot.rs` | `build_bot()` returning `CacheMe<Throttle<Bot>>` | VERIFIED | Line 10: `pub fn build_bot(token: String) -> CacheMe<Throttle<Bot>>` |
| `crates/bot/src/telegram/filter.rs` | chat_id allow-list filter via dptree filter_map | VERIFIED | `make_chat_id_filter` with `HashSet<i64>`, returns `None` for unlisted chat IDs |
| `crates/bot/src/telegram/dispatch.rs` | `run_telegram` with ShutdownToken + signal handling | VERIFIED | `SignalKind::terminate()`, `tokio::select!`, `shutdown_token`, Err handled |
| `crates/rightclaw-cli/src/main.rs` | `Commands::Bot { agent: String }` variant | VERIFIED | Lines 138-143: `Bot { agent: String }` in Commands enum |
| `crates/rightclaw-cli/Cargo.toml` | rightclaw-bot dependency | VERIFIED | Line 12: `rightclaw-bot = { path = "../bot" }` |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `crates/rightclaw-cli/src/main.rs` | `crates/bot/src/lib.rs` | `rightclaw_bot::run(BotArgs { ... }).await` | WIRED | Lines 206-212 in main.rs dispatch to `rightclaw_bot::run` with correct args |
| `crates/bot/src/telegram/dispatch.rs` | `tokio::signal::unix` | `signal(SignalKind::terminate())` | WIRED | Lines 42-44 register SIGTERM handler |
| `crates/bot/src/telegram/dispatch.rs` | `dispatcher.shutdown_token()` | `ShutdownToken` stored before `dispatch()` | WIRED | Line 30 captures token before line 83 calls `dispatcher.dispatch().await` |
| `crates/bot/src/lib.rs` | `crates/bot/src/telegram/filter.rs` | `config.allowed_chat_ids` passed to `run_telegram` | WIRED | Line 92: `telegram::run_telegram(token, config.allowed_chat_ids).await` |
| Workspace `Cargo.toml` | `crates/bot` | `members` array | WIRED | Line 2: `members = ["crates/rightclaw", "crates/rightclaw-cli", "crates/bot"]` |
| Workspace `Cargo.toml` | teloxide 0.17 | `[workspace.dependencies]` with correct features | WIRED | `teloxide = { version = "0.17", default-features = false, features = ["macros", "throttle", "cache-me"] }` |

### Data-Flow Trace (Level 4)

The bot skeleton does not render dynamic data to users in Phase 23 (dispatch endpoint is a no-op `respond(())`). Data-flow trace deferred — Phase 25 wires real message dispatch.

The filter pipeline does flow real data: `config.allowed_chat_ids` (from agent.yaml deserialization) → `Vec<i64>` → `HashSet<i64>` in `make_chat_id_filter` → `filter_map` in dptree schema. This chain is fully wired.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| `rightclaw --help` shows bot subcommand | `./target/debug/rightclaw --help \| grep -i bot` | `bot  Run the per-agent Telegram bot (long-polling, teloxide)` | PASS |
| `rightclaw bot --help` shows `--agent` flag | `./target/debug/rightclaw bot --help` | Shows `--agent <AGENT>` option | PASS |
| Agent-not-found returns miette error, exit 1 | `RIGHTCLAW_HOME=/tmp/rc-bot-test ./target/debug/rightclaw bot --agent nosuchagent 2>&1; echo "exit: $?"` | `Error: × agent directory not found: /tmp/rc-bot-test/agents/nosuchagent` + `exit: 1` | PASS |
| `cargo build --workspace` exits 0 | `cargo build --workspace 2>&1 \| tail -5` | `Finished dev profile in 0.24s` | PASS |
| All `agent_config_*` tests pass | `cargo test -p rightclaw agent_config -- --nocapture` | 14 tests ok (includes 4 new allowed_chat_ids tests + 10 pre-existing) | PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| BOT-01 | 23-02, 23-03 | `rightclaw bot --agent <name>` runs teloxide long-polling bot | SATISFIED | CLI subcommand wired, `run_telegram` starts dispatcher |
| BOT-03 | 23-02 | `CacheMe<Throttle<Bot>>` adaptor ordering | SATISFIED | `bot.rs` line 10 return type + method chain confirmed |
| BOT-04 | 23-02 | Graceful shutdown on SIGTERM, kill in-flight subprocesses | SATISFIED | `dispatch.rs` SIGTERM handler + `Arc<Mutex<Vec<Child>>>` + ShutdownToken sequence |
| BOT-05 | 23-01 | `allowed_chat_ids` in agent.yaml, unlisted IDs silently ignored | SATISFIED | Field in `AgentConfig`, `make_chat_id_filter` returns `None` for unlisted IDs |

**Note on REQUIREMENTS.md state:** BOT-05 checkbox and traceability table both show `Pending`/unchecked as of verification time. The implementation is fully present and tested. The REQUIREMENTS.md was not updated by the phase executor. This is a documentation gap only — the code satisfies the requirement. REQUIREMENTS.md should be updated to mark BOT-05 complete.

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `crates/bot/src/telegram/dispatch.rs` | 27 | No-op endpoint `respond(())` | Info | Intentional Phase 23 stub. Phase 25 replaces with real dispatch. Documented in 23-02-SUMMARY.md Known Stubs section. Does NOT block phase goal (skeleton compiles and structure is correct). |

No blocker or warning anti-patterns found. The no-op endpoint is a documented, intentional placeholder for the next phase.

### Human Verification Required

#### 1. Live Telegram message filtering

**Test:** Configure an agent with `allowed_chat_ids: [<your_chat_id>]` and a valid bot token. Run `rightclaw bot --agent <name>`. Send a message from the allowed chat. Then send from a different chat.
**Expected:** Allowed chat message is received (no-op in Phase 23, but no error); unlisted chat message is silently dropped with no bot reply.
**Why human:** Requires live Telegram bot token and actual Telegram clients. Cannot test with static code inspection.

#### 2. SIGTERM graceful shutdown sequencing

**Test:** Start `rightclaw bot --agent <name>` with a valid token. Send `kill -TERM <pid>`. Observe logs.
**Expected:** Log shows "SIGTERM received", "in-flight subprocesses terminated", "dispatcher stopped", "dispatcher exited cleanly". Exit code 0.
**Why human:** Requires running process and signal delivery. Cannot simulate without an active runtime.

### Gaps Summary

No gaps. All 10 must-have truths verified against the actual codebase. The workspace compiles clean, all signal handling is wired, the adaptor ordering is correct, the chat_id filter blocks unlisted IDs, and the CLI subcommand is fully wired end-to-end.

The only action item is a non-blocking documentation update: mark BOT-05 as complete in REQUIREMENTS.md (both the checkbox on line 15 and the traceability table on line 93).

---

_Verified: 2026-03-31T21:40:00Z_
_Verifier: Claude (gsd-verifier)_
