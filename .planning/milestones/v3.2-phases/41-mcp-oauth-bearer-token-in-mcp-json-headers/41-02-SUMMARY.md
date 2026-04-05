---
phase: 41-mcp-oauth-bearer-token-in-mcp-json-headers
plan: 02
subsystem: auth
tags: [oauth, mcp, bearer-token, credentials, migration]

requires:
  - phase: 41
    plan: 01
    provides: write_bearer_to_mcp_json, read_oauth_metadata, mcp_auth_status single-arg
provides:
  - Complete removal of .credentials.json from OAuth token flow
  - OAuthCallbackState without credentials_path or pc_port
  - run_refresh_scheduler with (agent_dir, http_client) signature
  - check_mcp_tokens_impl without credentials_path parameter
affects: [bot-startup, oauth-callback, refresh-scheduler, doctor]

tech-stack:
  added: []
  patterns:
    - "All OAuth token storage reads/writes go through .mcp.json — zero .credentials.json references in OAuth flow"
    - "Agent restart removed from OAuth callback — CC uses claude -p per message, no persistent session"

key-files:
  created: []
  modified:
    - crates/rightclaw/src/mcp/refresh.rs
    - crates/bot/src/telegram/oauth_callback.rs
    - crates/bot/src/lib.rs
    - crates/rightclaw/src/doctor.rs
    - crates/rightclaw/src/doctor_tests.rs

key-decisions:
  - "Agent restart via PcClient removed from OAuth callback — CC uses claude -p per message, no persistent session to restart (T-41-05 accepted)"
  - "check_mcp_tokens_with_creds renamed to check_mcp_tokens_impl and credentials_path param removed"
  - "Remaining .credentials.json references in codegen/claude_json.rs and home_isolation.rs are CC native credential symlinks, not OAuth — out of scope"

patterns-established:
  - "OAuth callback notifies 'Token written to .mcp.json' instead of 'Restarting agent'"

requirements-completed: [OAUTH-HEADER-04, OAUTH-HEADER-05, OAUTH-HEADER-06]

duration: 4min
completed: 2026-04-05
---

# Phase 41 Plan 02: Wire .mcp.json Header-Based Credentials Into All Consumers Summary

**Completed OAuth token migration from .credentials.json to .mcp.json headers — removed credentials_path from all structs/functions, eliminated PcClient agent restart from callback**

## Performance

- **Duration:** 4 min
- **Started:** 2026-04-05T20:14:46Z
- **Completed:** 2026-04-05T20:19:10Z
- **Tasks:** 2/2
- **Files modified:** 5

## Accomplishments

- Removed `_credentials_path: PathBuf` parameter from `run_refresh_scheduler` (refresh.rs)
- Removed `credentials_path` and `pc_port` fields from `OAuthCallbackState` struct
- Removed PcClient agent restart logic and "Restarting agent" notification from OAuth callback
- Removed `credentials_path` variable and `refresh_credentials_path` clone from bot `lib.rs`
- Renamed `check_mcp_tokens_with_creds` to `check_mcp_tokens_impl`, removed `_credentials_path` param
- Eliminated `dirs::home_dir().join(".credentials.json")` from `check_mcp_tokens` wrapper
- Updated all doctor_tests.rs to call `check_mcp_tokens_impl` without creds_path

## Task Commits

1. **Task 1: Remove credentials_path from run_refresh_scheduler** - `3151d8d` (feat)
2. **Task 2: Complete .credentials.json migration in bot + doctor** - `36779af` (feat)

## Files Created/Modified

- `crates/rightclaw/src/mcp/refresh.rs` - Removed `_credentials_path` from `run_refresh_scheduler`, updated test
- `crates/bot/src/telegram/oauth_callback.rs` - Removed `credentials_path`, `pc_port`, PcClient restart, updated tests
- `crates/bot/src/lib.rs` - Removed credentials_path wiring, pc_port derivation, simplified OAuthCallbackState construction
- `crates/rightclaw/src/doctor.rs` - Renamed function, removed credentials_path param and construction
- `crates/rightclaw/src/doctor_tests.rs` - Updated all check_mcp_tokens tests to use new single-arg API

## Decisions Made

- Agent restart via PcClient removed from OAuth callback (T-41-05: CC uses `claude -p` per message, no persistent session)
- `check_mcp_tokens_with_creds` renamed to `check_mcp_tokens_impl` — cleaner than inlining
- Remaining `.credentials.json` references in `codegen/claude_json.rs` and `home_isolation.rs` tests are about CC's native credential file symlink (HOME isolation feature), not OAuth tokens — intentionally out of scope

## Deviations from Plan

None - plan executed exactly as written. Plan 01's Rule 3 deviation had already updated all callers to compile; this plan removed the remaining vestigial parameters and dead code.

## Verification Results

```
cargo test --workspace: 344 passed, 1 failed (pre-existing test_status_no_running_instance)
rg '.credentials.json' crates/ --type rust: 0 matches in OAuth flow code
  (remaining matches are CC native credential symlink in codegen/claude_json.rs — not OAuth)
rg 'write_credential|read_credential|mcp_oauth_key' crates/ --type rust: 0 function references
  (only comments mentioning old function names)
```

## Known Stubs

None.

## Self-Check: PASSED

- crates/rightclaw/src/mcp/refresh.rs: FOUND
- crates/bot/src/telegram/oauth_callback.rs: FOUND
- crates/bot/src/lib.rs: FOUND
- crates/rightclaw/src/doctor.rs: FOUND
- crates/rightclaw/src/doctor_tests.rs: FOUND
- Commit 3151d8d: FOUND
- Commit 36779af: FOUND
- No credentials_path in OAuthCallbackState: VERIFIED
- No _credentials_path in run_refresh_scheduler: VERIFIED
- No PcClient in oauth_callback.rs: VERIFIED

---
*Phase: 41-mcp-oauth-bearer-token-in-mcp-json-headers*
*Completed: 2026-04-05*
