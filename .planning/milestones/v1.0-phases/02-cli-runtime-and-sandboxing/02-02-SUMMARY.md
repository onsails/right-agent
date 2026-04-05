---
phase: 02-cli-runtime-and-sandboxing
plan: 02
subsystem: runtime
tags: [reqwest, unix-socket, process-compose, openshell, sandbox, which]

requires:
  - phase: 02-cli-runtime-and-sandboxing
    provides: "Phase 2 workspace deps (reqwest, tokio, serde_json, which) and runtime module placeholder"
provides:
  - "PcClient async REST API client for process-compose via Unix socket"
  - "ProcessInfo type for deserializing process-compose status responses"
  - "RuntimeState/AgentState types for tracking sandbox names and socket path"
  - "write_state/read_state for JSON state file persistence"
  - "destroy_sandboxes for best-effort OpenShell sandbox cleanup"
  - "verify_dependencies for checking process-compose, claude, openshell in PATH"
affects: [02-03, cli-commands, up, down, status, restart]

tech-stack:
  added: []
  patterns: [reqwest unix_socket transport, best-effort cleanup with tracing::warn, which crate for PATH lookup]

key-files:
  created:
    - crates/rightclaw/src/runtime/pc_client.rs
    - crates/rightclaw/src/runtime/pc_client_tests.rs
    - crates/rightclaw/src/runtime/sandbox.rs
    - crates/rightclaw/src/runtime/sandbox_tests.rs
    - crates/rightclaw/src/runtime/deps.rs
  modified:
    - crates/rightclaw/src/runtime/mod.rs

key-decisions:
  - "destroy_sandboxes uses best-effort cleanup (tracing::warn on failure) -- only exception to fail-fast rule per CLAUDE.rust.md"
  - "PcClient base_url hardcoded to http://localhost since host is ignored for Unix socket transport"

patterns-established:
  - "reqwest unix_socket: Client::builder().unix_socket(path).build() with http://localhost base URL"
  - "Best-effort cleanup: match on Command output, warn on failure, continue iteration"
  - "Dependency verification: which::which with miette help text containing install URLs"

requirements-completed: [CLI-05, CLI-06, SAND-03]

duration: 3min
completed: 2026-03-22
---

# Phase 02 Plan 02: Runtime Module Summary

**Async process-compose REST API client (reqwest over Unix socket) with sandbox state tracking, cleanup, and external dependency verification**

## Performance

- **Duration:** 3 min
- **Started:** 2026-03-22T16:56:35Z
- **Completed:** 2026-03-22T16:59:35Z
- **Tasks:** 2
- **Files modified:** 6

## Accomplishments
- PcClient with typed async methods for all process-compose REST API endpoints (health_check, list_processes, restart_process, stop_process, shutdown)
- Sandbox state management with JSON persistence and deterministic naming (rightclaw-{agent})
- Dependency verification with actionable error messages and install URLs
- 13 new tests across pc_client, sandbox, and deps modules

## Task Commits

Each task was committed atomically:

1. **Task 1: PcClient -- process-compose REST API client** - `aae104e` (feat)
2. **Task 2: Sandbox tracking, cleanup, and dependency verification** - `4aa7dc9` (feat)

## Files Created/Modified
- `crates/rightclaw/src/runtime/mod.rs` - Module declarations and re-exports for pc_client, sandbox, deps
- `crates/rightclaw/src/runtime/pc_client.rs` - PcClient struct with reqwest unix_socket, ProcessInfo/ProcessesResponse types
- `crates/rightclaw/src/runtime/pc_client_tests.rs` - 5 tests for client construction and JSON deserialization
- `crates/rightclaw/src/runtime/sandbox.rs` - RuntimeState/AgentState structs, write/read state, destroy_sandboxes, sandbox_name_for
- `crates/rightclaw/src/runtime/sandbox_tests.rs` - 5 tests for state roundtrip, naming convention, error cases
- `crates/rightclaw/src/runtime/deps.rs` - verify_dependencies with 3 tests for PATH lookup behavior

## Decisions Made
- PcClient uses `http://localhost` as base URL -- host is ignored for Unix socket transport per reqwest docs
- destroy_sandboxes is best-effort: logs warnings on failure instead of propagating errors, since sandboxes may already be gone during cleanup
- deps tests verify function structure rather than asserting specific binaries are missing (environment-dependent)

## Deviations from Plan

None -- plan executed exactly as written.

## Issues Encountered
None.

## Known Stubs
None -- all runtime functions are fully implemented and ready for CLI wiring.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Runtime module complete: PcClient, sandbox state, dependency verification all ready
- Plan 03 (CLI subcommands) can wire these directly into up/down/status/restart/attach commands
- All 69 workspace tests passing, clippy clean

## Self-Check: PASSED

All 7 files verified on disk. Both task commits (aae104e, 4aa7dc9) found in git log.

---
*Phase: 02-cli-runtime-and-sandboxing*
*Completed: 2026-03-22*
