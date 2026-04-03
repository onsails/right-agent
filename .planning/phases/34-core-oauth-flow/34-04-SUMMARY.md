---
phase: 34-core-oauth-flow
plan: "04"
subsystem: bot
tags: [oauth, telegram, axum, mcp, bot-commands]
dependency_graph:
  requires: [34-02, 34-03]
  provides: [full-oauth-bot-integration]
  affects: [crates/bot]
tech_stack:
  added: [axum UDS server, reqwest (bot crate), dirs (bot crate)]
  patterns: [tokio::select! concurrent bot+axum, PendingAuthMap one-shot state, dptree deps injection]
key_files:
  created:
    - crates/bot/src/telegram/oauth_callback.rs
  modified:
    - crates/bot/src/telegram/mod.rs
    - crates/bot/src/telegram/dispatch.rs
    - crates/bot/src/telegram/handler.rs
    - crates/bot/src/lib.rs
    - crates/bot/Cargo.toml
decisions:
  - "write_credential takes (path, server_name, server_url, token) and derives key internally — no pre-computed key parameter; server_type reading preserved as test-only utility"
  - "ServerStatus.name field (not server_name) used in handle_mcp_list — plan interface was slightly wrong"
  - "exchange_token arg order is (client, endpoint, code, redirect_uri, client_id, secret, verifier) — not as documented in plan"
  - "notify_bot constructed as plain teloxide::Bot (not BotType) since OAuthCallbackState.bot is teloxide::Bot"
metrics:
  duration: ~12min
  completed: "2026-04-03"
  tasks: 4
  files_modified: 6
---

# Phase 34 Plan 04: Bot OAuth Integration Summary

Wired the full MCP OAuth flow end-to-end in the bot: axum Unix socket callback server running alongside teloxide, all 5 bot commands (/mcp list/auth/add/remove, /doctor), PendingAuth one-shot state management with 10-minute cleanup, and post-auth credential write + agent restart.

## Tasks Completed

| Task | Name | Commit | Key Files |
|------|------|--------|-----------|
| 1a (TDD) | oauth_callback.rs module + unit tests | 70fbe3a | crates/bot/src/telegram/oauth_callback.rs |
| 1b | Wire oauth_callback into lib.rs + dispatch.rs | 6e395bd | crates/bot/src/lib.rs, dispatch.rs |
| 2 | Bot commands: /mcp list, /mcp auth, tunnel healthcheck | 21a921d | crates/bot/src/telegram/handler.rs |
| 3 | Bot commands: /mcp add, /mcp remove, /doctor | 21a921d | crates/bot/src/telegram/handler.rs |

## What Was Built

### oauth_callback.rs (new module)
- `PendingAuthMap` type alias: `Arc<Mutex<HashMap<String, PendingAuth>>>`
- `OAuthCallbackState` struct with all required fields (pending_auth, credentials_path, mcp_json_path, agent_name, pc_port, bot, notify_chat_ids)
- `handle_oauth_callback` axum handler: constant-time state verification via `verify_state`, one-shot PendingAuth consumption, background token exchange spawn
- `complete_oauth_flow`: exchange_token → write_credential → PcClient::restart_process → Telegram notification
- `run_oauth_callback_server`: removes stale socket (Pitfall 2), binds UnixListener, signals ready, serves axum router
- `run_pending_auth_cleanup`: every 60s, removes PendingAuth entries older than 10 minutes
- 9 unit tests: valid state consumption, unknown state rejection, replay rejection, cleanup behavior, server_type reads

### lib.rs changes
- Creates `PendingAuthMap`, derives `pc_port` from RC_PC_PORT env with PC_PORT fallback
- Builds `OAuthCallbackState` with credentials path, mcp_json path, agent_name
- Spawns cleanup task
- Spawns axum server with ready signal; awaits ready before starting teloxide
- `tokio::select!` runs teloxide + axum concurrently (RESEARCH.md Pitfall 4)

### dispatch.rs changes
- `BotCommand` enum extended with `Mcp(String)` and `Doctor`
- `run_telegram` signature extended with `pending_auth: PendingAuthMap` and `home: PathBuf`
- Both new commands wired into dptree dispatcher with `dptree::deps!`

### handler.rs additions
- `handle_mcp`: free-form arg router for list/auth/add/remove subcommands
- `handle_mcp_list`: calls `mcp_auth_status`, formats text table with OK/MISSING/EXPIRED icons
- `handle_mcp_auth`: full OAuth flow — reads .mcp.json, reads tunnel config, checks cloudflared binary (OAUTH-04), AS discovery, DCR/fallback, PKCE generation, tunnel ROOT healthcheck (OAUTH-05), PendingAuth storage, sends auth URL
- `handle_mcp_add`: parses name+url+optional clientId, writes .mcp.json
- `handle_mcp_remove`: removes server entry from .mcp.json
- `handle_doctor`: calls `run_doctor`, formats results with pass count

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] exchange_token argument order mismatch**
- **Found during:** Task 1a compilation
- **Issue:** Plan documented `(client, endpoint, client_id, client_secret, code, code_verifier, redirect_uri)` but actual function signature is `(client, endpoint, code, redirect_uri, client_id, client_secret, code_verifier)`
- **Fix:** Used compiler diagnostic hint to swap arguments to correct order
- **Files modified:** crates/bot/src/telegram/oauth_callback.rs

**2. [Rule 1 - Bug] write_credential signature mismatch**
- **Found during:** Task 1a compilation
- **Issue:** Plan said to pass pre-computed key as parameter, but actual `write_credential(path, server_name, server_url, token)` derives the key internally and has no key parameter
- **Fix:** Call `write_credential` with (path, server_name, server_url, token) directly; removed manual key derivation. `read_server_type` kept as test-only utility
- **Files modified:** crates/bot/src/telegram/oauth_callback.rs

**3. [Rule 1 - Bug] ServerStatus field name**
- **Found during:** Task 2 compilation
- **Issue:** Plan referenced `s.server_name` but actual `ServerStatus` struct has field `name`
- **Fix:** Changed to `s.name`
- **Files modified:** crates/bot/src/telegram/handler.rs

**4. [Rule 1 - Bug] OAuthCallbackState.bot type**
- **Found during:** Task 1b compilation
- **Issue:** `build_bot()` returns `BotType = CacheMe<Throttle<Bot>>` but `OAuthCallbackState.bot` is `teloxide::Bot`
- **Fix:** Used `teloxide::Bot::new(token.clone())` for the notify_bot field
- **Files modified:** crates/bot/src/lib.rs

**5. [Rule 3 - Blocking] dptree::deps! trailing comma**
- **Found during:** Task 1b compilation
- **Issue:** `dptree::deps!` macro does not accept trailing comma (unlike Rust `vec![]`)
- **Fix:** Removed trailing comma from deps list
- **Files modified:** crates/bot/src/telegram/dispatch.rs

## Known Stubs

None. All bot commands are fully implemented.

## Verification

- `cargo build --workspace` — exits 0
- `cargo test -p rightclaw-bot --lib telegram::oauth_callback` — 9/9 tests pass
- `cargo test -p rightclaw-bot --lib` — 59/59 tests pass

## Self-Check: PASSED

- FOUND: crates/bot/src/telegram/oauth_callback.rs
- FOUND: crates/bot/src/telegram/handler.rs
- FOUND commit 70fbe3a (Task 1a)
- FOUND commit 6e395bd (Task 1b)
- FOUND commit 21a921d (Tasks 2+3)
