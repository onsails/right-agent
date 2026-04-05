---
phase: 41-mcp-oauth-bearer-token-in-mcp-json-headers
plan: 01
subsystem: auth
tags: [oauth, mcp, bearer-token, credentials, json]

requires:
  - phase: 35-mcp-oauth
    provides: OAuth token exchange and credential storage
provides:
  - write_bearer_to_mcp_json and read_bearer_from_mcp_json for .mcp.json header injection
  - write_oauth_metadata and read_oauth_metadata for _rightclaw_oauth persistence
  - OAuthMetadata struct for refresh token lifecycle
  - mcp_auth_status with single-arg signature (no credentials_path)
affects: [41-02, refresh, oauth-callback, bot-startup]

tech-stack:
  added: []
  patterns:
    - ".mcp.json as single source of truth for Bearer tokens and OAuth metadata"
    - "Atomic read-modify-write via write_json_atomic for .mcp.json"

key-files:
  created: []
  modified:
    - crates/rightclaw/src/mcp/credentials.rs
    - crates/rightclaw/src/mcp/detect.rs
    - crates/rightclaw/src/mcp/refresh.rs
    - crates/rightclaw/src/doctor.rs
    - crates/rightclaw/src/doctor_tests.rs
    - crates/rightclaw-cli/src/main.rs
    - crates/bot/src/lib.rs
    - crates/bot/src/telegram/handler.rs
    - crates/bot/src/telegram/oauth_callback.rs
    - crates/rightclaw/Cargo.toml

key-decisions:
  - "Removed hex crate entirely; sha2 retained for oauth.rs PKCE"
  - "Updated all callers in this plan (deviation Rule 3) to avoid compilation failures blocking tests"
  - "OAuthCallbackState.credentials_path kept in struct but no longer used for credential writes"

patterns-established:
  - "Bearer tokens written directly to .mcp.json headers -- CC passes them as-is"
  - "_rightclaw_oauth stores refresh metadata per server in .mcp.json"

requirements-completed: [OAUTH-HEADER-01, OAUTH-HEADER-02, OAUTH-HEADER-03]

duration: 9min
completed: 2026-04-05
---

# Phase 41 Plan 01: MCP OAuth Bearer Token in .mcp.json Headers Summary

**Replaced proprietary .credentials.json key-derivation with direct .mcp.json Authorization header injection and _rightclaw_oauth metadata storage**

## Performance

- **Duration:** 9 min
- **Started:** 2026-04-05T20:04:01Z
- **Completed:** 2026-04-05T20:12:32Z
- **Tasks:** 2
- **Files modified:** 10

## Accomplishments
- Gutted credentials.rs: removed mcp_oauth_key, write_credential, read_credential, rotate_backups and hex/sha2 imports
- Added 4 new functions (write_bearer_to_mcp_json, read_bearer_from_mcp_json, write_oauth_metadata, read_oauth_metadata) with OAuthMetadata struct
- Rewrote detect.rs mcp_auth_status to single-arg signature checking .mcp.json headers directly
- Updated all callers across workspace (refresh.rs, doctor.rs, main.rs, lib.rs, handler.rs, oauth_callback.rs) for clean compilation

## Task Commits

1. **Task 1: Rewrite credentials.rs** - `950acfd` (feat)
2. **Task 2: Rewrite detect.rs + caller updates** - `d8faae0` (feat)

## Files Created/Modified
- `crates/rightclaw/src/mcp/credentials.rs` - New .mcp.json header read/write functions, OAuthMetadata struct
- `crates/rightclaw/src/mcp/detect.rs` - Header-based auth status detection (single-arg)
- `crates/rightclaw/src/mcp/refresh.rs` - Updated to use read_oauth_metadata/write_bearer_to_mcp_json
- `crates/rightclaw/src/doctor.rs` - Updated mcp_auth_status call
- `crates/rightclaw/src/doctor_tests.rs` - Tests rewritten for header-based approach
- `crates/bot/src/telegram/oauth_callback.rs` - Writes Bearer + OAuthMetadata to .mcp.json
- `crates/bot/src/telegram/handler.rs` - Updated mcp_auth_status call
- `crates/bot/src/lib.rs` - Updated startup MCP auth check
- `crates/rightclaw-cli/src/main.rs` - Updated mcp status command
- `crates/rightclaw/Cargo.toml` - Removed hex dependency

## Decisions Made
- Removed hex crate entirely (only used by mcp_oauth_key); sha2 retained for oauth.rs PKCE code_challenge
- Updated all callers in same plan (deviation Rule 3) -- Plan 02 scope reduced since callers already compile
- OAuthCallbackState.credentials_path field kept in struct to avoid cascading changes in bot setup code

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Updated all callers to compile with new API**
- **Found during:** Task 1/2 (compilation gate)
- **Issue:** cargo test cannot run if lib crate fails to compile; refresh.rs, doctor.rs, main.rs, handler.rs, oauth_callback.rs, lib.rs all referenced removed functions
- **Fix:** Updated all callers to use new .mcp.json API (write_bearer_to_mcp_json, read_oauth_metadata, single-arg mcp_auth_status)
- **Files modified:** refresh.rs, doctor.rs, doctor_tests.rs, main.rs, lib.rs, handler.rs, oauth_callback.rs
- **Verification:** cargo build --workspace succeeds with zero warnings
- **Committed in:** d8faae0 (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Caller updates were necessary for test execution. This reduces Plan 02 scope since all callers already compile and pass tests.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- All credential/detect unit tests pass (15 + 9 = 24)
- All 52 doctor tests pass with updated header-based approach
- Workspace builds cleanly with zero warnings
- Plan 02 scope reduced: callers already updated, may only need cleanup of unused credentials_path params in struct definitions

## Self-Check: PASSED

- credentials.rs: FOUND
- detect.rs: FOUND
- Commit 950acfd: FOUND
- Commit d8faae0: FOUND
- No old functions (mcp_oauth_key, write_credential, read_credential, rotate_backups) in credentials.rs
- No hex/sha2 imports in credentials.rs
- Workspace builds with zero warnings

---
*Phase: 41-mcp-oauth-bearer-token-in-mcp-json-headers*
*Completed: 2026-04-05*
