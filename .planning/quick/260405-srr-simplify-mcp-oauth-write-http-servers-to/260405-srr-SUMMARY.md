---
phase: quick
plan: 260405-srr
subsystem: mcp
tags: [oauth, claude-json, mcp, cleanup]

provides:
  - ".claude.json MCP server management (add/remove/list HTTP servers)"
  - "Simplified detect.rs combining .claude.json + .mcp.json sources"
  - "CC-native OAuth via type:http entries in .claude.json"
affects: [mcp, bot, cli]

tech-stack:
  added: []
  removed: [sha2, hex, axum, subtle, rand, base64]
  patterns: [".claude.json for HTTP MCP servers, .mcp.json for stdio servers"]

key-files:
  created: []
  modified:
    - crates/rightclaw/src/mcp/credentials.rs
    - crates/rightclaw/src/mcp/detect.rs
    - crates/rightclaw/src/mcp/mod.rs
    - crates/bot/src/telegram/handler.rs
    - crates/bot/src/telegram/dispatch.rs
    - crates/bot/src/telegram/mod.rs
    - crates/bot/src/lib.rs
    - crates/rightclaw/src/doctor.rs
    - crates/rightclaw/src/doctor_tests.rs
    - crates/rightclaw-cli/src/main.rs
  deleted:
    - crates/rightclaw/src/mcp/oauth.rs
    - crates/rightclaw/src/mcp/refresh.rs
    - crates/bot/src/telegram/oauth_callback.rs

key-decisions:
  - "AuthState::Expired removed -- CC manages token lifecycle natively"
  - "mcp_auth_status now takes agent_dir instead of mcp_path, combines both file sources"
  - "ServerSource enum distinguishes .claude.json vs .mcp.json origin"
  - "read_mcp_json removed as dead code after detect.rs rewrite"

duration: 9min
completed: 2026-04-05
---

# Quick Task 260405-srr: Simplify MCP OAuth Summary

**Deleted 1500+ lines of custom OAuth flow (PKCE, token exchange, refresh, callback server) and switched to CC-native OAuth via type:http entries in .claude.json with 6 workspace deps removed**

## Performance

- **Duration:** 9 min
- **Started:** 2026-04-05T20:46:58Z
- **Completed:** 2026-04-05T20:56:09Z
- **Tasks:** 2
- **Files modified:** 13 (3 deleted, 10 modified)
- **Lines removed:** ~3000 (oauth.rs, refresh.rs, oauth_callback.rs, old credentials.rs code)

## Accomplishments
- Deleted entire custom OAuth flow: oauth.rs (PKCE/AS-discovery/DCR/token-exchange), refresh.rs (background refresh scheduler), oauth_callback.rs (axum UDS server + PendingAuth lifecycle)
- Rewrote credentials.rs with .claude.json MCP helpers: add/remove/list HTTP servers under projects.<path>.mcpServers
- Simplified detect.rs to combine HTTP servers from .claude.json and URL servers from .mcp.json with ServerSource enum
- Removed 6 workspace deps: sha2, hex, axum, subtle, rand, base64
- Bot lib.rs gutted: no more axum callback server, refresh scheduler, PendingAuthMap, or tokio::select!
- /mcp auth now returns guidance message instead of 190-line OAuth flow

## Task Commits

1. **Task 1: Delete OAuth files, gut credentials.rs, add .claude.json MCP helpers** - `8c3e1b0` (feat)
2. **Task 2: Rewire bot, delete oauth_callback.rs, simplify handler/dispatch/lib.rs, clean deps** - `b971bbe` (feat)

## Files Created/Modified

### Deleted
- `crates/rightclaw/src/mcp/oauth.rs` -- Custom PKCE + AS discovery + DCR + token exchange
- `crates/rightclaw/src/mcp/refresh.rs` -- Background token refresh scheduler
- `crates/bot/src/telegram/oauth_callback.rs` -- Axum UDS OAuth callback server + PendingAuth map

### Modified
- `crates/rightclaw/src/mcp/credentials.rs` -- Gutted: new .claude.json add/remove/list helpers
- `crates/rightclaw/src/mcp/detect.rs` -- Takes agent_dir, combines .claude.json + .mcp.json, ServerSource enum
- `crates/rightclaw/src/mcp/mod.rs` -- Removed oauth and refresh module declarations
- `crates/bot/src/telegram/handler.rs` -- /mcp add|remove use .claude.json, /mcp auth returns guidance
- `crates/bot/src/telegram/dispatch.rs` -- Removed pending_auth from params and dptree deps
- `crates/bot/src/telegram/mod.rs` -- Removed oauth_callback module
- `crates/bot/src/lib.rs` -- Removed axum server, refresh scheduler, PendingAuthMap; direct run_telegram call
- `crates/rightclaw/src/doctor.rs` -- Removed check_mcp_tokens expired checks, updated mcp_auth_status calls
- `crates/rightclaw/src/doctor_tests.rs` -- Updated for new API, removed expired token tests
- `crates/rightclaw-cli/src/main.rs` -- cmd_mcp_status uses new detect.rs API
- `Cargo.toml` -- Removed 6 workspace deps
- `crates/rightclaw/Cargo.toml` -- Removed sha2, subtle, rand, base64
- `crates/bot/Cargo.toml` -- Removed axum, reqwest, rand, base64, subtle

## Decisions Made
- AuthState::Expired removed entirely -- CC manages token lifecycle natively for HTTP MCP servers, no way to peek into CC's credential store
- mcp_auth_status signature changed from `&Path` (to .mcp.json) to `&Path` (to agent_dir) -- function now reads both .claude.json and .mcp.json internally
- ServerSource enum (ClaudeJson | McpJson) added to distinguish origin of each server entry
- read_mcp_json helper removed as dead code after detect.rs reads files directly
- MCP_ISSUES_PREFIX changed from "missing/expired: " to "missing: " to match simplified auth states

## Deviations from Plan

None -- plan executed exactly as written.

## Issues Encountered
- Pre-existing `test_status_no_running_instance` CLI integration test failure (hits running process-compose instance) -- unrelated to changes, documented in MEMORY.md

## Self-Check: PASSED
