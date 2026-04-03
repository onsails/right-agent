---
phase: 33-auth-detection
plan: 01
subsystem: auth
tags: [mcp, oauth, credentials, detect, cli]

requires:
  - phase: 32-credential-foundation
    provides: "read_credential, write_credential, CredentialToken, mcp_oauth_key — the credential I/O layer detect.rs builds on"

provides:
  - "mcp::detect module with AuthState enum (Present/Missing/Expired) and mcp_auth_status function"
  - "rightclaw mcp status subcommand — shows per-agent OAuth server auth state"
  - "rightclaw mcp status --agent <name> — filters to single agent"
  - "cmd_up auth warn block — emits tracing::warn! listing all agent/server pairs with missing or expired tokens"

affects: [34-oauth-flow, 35-token-refresh]

tech-stack:
  added: []
  patterns:
    - "McpCommands enum mirrors MemoryCommands pattern — subcommand via #[command(subcommand)]"
    - "mcp_auth_status takes explicit path params — no env var reads in library code"
    - "Test-only imports gated behind #[cfg(test)] to avoid unused_import warnings"

key-files:
  created:
    - crates/rightclaw/src/mcp/detect.rs
  modified:
    - crates/rightclaw/src/mcp/mod.rs
    - crates/rightclaw-cli/src/main.rs

key-decisions:
  - "expires_at=0 treated as Present (non-expiring), not Expired — Linear case documented in tests"
  - "Stdio servers (no url field in .mcp.json) silently skipped — url presence is the OAuth boundary"
  - "Absent .mcp.json returns Ok(vec![]) — not an error, agent may have no HTTP servers"
  - "Auth warn in cmd_up emits a single line naming all agent/server pairs for operator clarity"

patterns-established:
  - "McpCommands enum: mirror MemoryCommands, start with Status subcommand, extend in Phase 34"
  - "mcp_auth_status as library fn with explicit path args — testable without env pollution"

requirements-completed: [DETECT-01, DETECT-02]

duration: 8min
completed: 2026-04-03
---

# Phase 33 Plan 01: Auth Detection Summary

**MCP OAuth auth state detection via `AuthState` enum, `mcp_auth_status` library fn, `rightclaw mcp status` subcommand, and `cmd_up` pre-launch warn listing unauthenticated agent/server pairs**

## Performance

- **Duration:** ~8 min
- **Started:** 2026-04-03T15:02:00Z
- **Completed:** 2026-04-03T15:10:13Z
- **Tasks:** 2
- **Files modified:** 3 (1 created)

## Accomplishments

- `mcp::detect` module with `AuthState` (Present/Missing/Expired), `ServerStatus`, and `mcp_auth_status` — reads .mcp.json + credentials file, skips stdio servers, sorts results deterministically
- 9 TDD behavior tests cover all specified cases including `expires_at=0` (non-expiring), far-future, expired, missing key, absent credentials, absent .mcp.json, stdio skip, url inclusion, sort order
- `rightclaw mcp status [--agent <name>]` CLI subcommand — groups servers by agent, skips agents without HTTP servers, errors on unknown agent name
- `cmd_up` auth warn block — runs after per-agent loop, before process-compose launch, collects all missing/expired pairs into a single `tracing::warn!` line

## Task Commits

1. **Task 1: mcp::detect module** - `9a280b8` (feat)
2. **Task 2: CLI wiring** - `e0db55d` (feat)
3. **Fix: test-only imports** - `f5f9000` (fix)

## Files Created/Modified

- `/home/wb/dev/rightclaw/crates/rightclaw/src/mcp/detect.rs` — AuthState enum, ServerStatus struct, mcp_auth_status fn, 9 tests
- `/home/wb/dev/rightclaw/crates/rightclaw/src/mcp/mod.rs` — added `pub mod detect;`
- `/home/wb/dev/rightclaw/crates/rightclaw-cli/src/main.rs` — McpCommands enum, Commands::Mcp variant, cmd_mcp_status fn, cmd_up auth warn block

## Decisions Made

- `expires_at=0` is Present (non-expiring Linear case) — matches Phase 33 CONTEXT.md decision
- Stdio servers filtered by url field absence — no hardcoded name blocklist
- Test-only imports (`write_credential`, `CredentialToken`) moved to `#[cfg(test)]` to eliminate compiler warning

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Unused import warning for test-only symbols**
- **Found during:** Task 1 verification build
- **Issue:** `write_credential` and `CredentialToken` imported at module level but only used in `#[cfg(test)]`; caused `unused_imports` warning in production build
- **Fix:** Moved those two imports under `#[cfg(test)]` block
- **Files modified:** `crates/rightclaw/src/mcp/detect.rs`
- **Verification:** `cargo build --workspace` produces zero warnings
- **Committed in:** `f5f9000`

---

**Total deviations:** 1 auto-fixed (Rule 1 — import scope bug)
**Impact on plan:** Minimal — cosmetic fix, no behavior change.

## Issues Encountered

- `test_status_no_running_instance` integration test fails — pre-existing issue documented in STATE.md and MEMORY.md. Not caused by this plan's changes.

## Known Stubs

None — `mcp_auth_status` reads real credentials file; no mock data wired to production paths.

## Next Phase Readiness

- Phase 34 (OAuth callback server) can use `mcp_auth_status` to discover which servers need auth
- `AuthState` enum is the shared contract between detection (Phase 33) and flow (Phase 34)
- `rightclaw mcp status` surface is ready for operators to check auth state before triggering OAuth

---
*Phase: 33-auth-detection*
*Completed: 2026-04-03*

## Self-Check: PASSED

- detect.rs: FOUND
- mcp/mod.rs: FOUND
- SUMMARY.md: FOUND
- Commit 9a280b8: FOUND
- Commit e0db55d: FOUND
- Commit f5f9000: FOUND
