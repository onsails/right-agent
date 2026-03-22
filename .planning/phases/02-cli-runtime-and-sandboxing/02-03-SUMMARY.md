---
phase: 02-cli-runtime-and-sandboxing
plan: 03
subsystem: cli
tags: [clap, tokio, process-compose, unix-socket, openshell]

requires:
  - phase: 02-cli-runtime-and-sandboxing
    plan: 01
    provides: "generate_wrapper and generate_process_compose codegen functions"
  - phase: 02-cli-runtime-and-sandboxing
    plan: 02
    provides: "PcClient, verify_dependencies, destroy_sandboxes, RuntimeState"
  - phase: 01-project-setup-and-agent-structure
    provides: "discover_agents, resolve_home, Clap CLI with Init/List"
provides:
  - "Complete CLI with all lifecycle commands: up, down, status, restart, attach"
  - "End-to-end agent launch pipeline: discover -> codegen -> spawn process-compose"
  - "Stale socket detection and cleanup"
  - "RuntimeState tracking of no_sandbox for down cleanup"
affects: [phase-03, default-agent, install-script]

tech-stack:
  added: []
  patterns:
    - "Async main with tokio::main for PcClient async methods"
    - "CommandExt for attach -- replaces current process"
    - "Per-function command handlers (cmd_up, cmd_down, etc.) for readability"

key-files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw-cli/Cargo.toml
    - crates/rightclaw-cli/tests/cli_integration.rs
    - crates/rightclaw/src/runtime/sandbox.rs
    - crates/rightclaw/src/runtime/sandbox_tests.rs

key-decisions:
  - "SystemTime for started_at timestamp instead of chrono -- avoids adding chrono dependency"
  - "Per-function command handlers instead of inline match arms for cleaner main.rs"

patterns-established:
  - "cmd_* function pattern: each subcommand gets its own async/sync function"
  - "Socket existence check as proxy for running instance detection"

requirements-completed: [CLI-01, CLI-02, CLI-03, CLI-04, CLI-07]

duration: 4min
completed: 2026-03-22
---

# Phase 2 Plan 3: CLI Subcommand Wiring Summary

**All five lifecycle CLI subcommands (up/down/status/restart/attach) wired end-to-end with codegen, runtime, and process-compose integration**

## Performance

- **Duration:** 4 min
- **Started:** 2026-03-22T17:01:35Z
- **Completed:** 2026-03-22T17:05:35Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments
- All five CLI subcommands wired: up (--agents, -d, --no-sandbox), down, status, restart, attach
- Up command: verify deps, discover agents, generate wrappers + process-compose.yaml, write state, spawn process-compose
- Integration tests for help text, error paths (no socket, no state file)

## Task Commits

Each task was committed atomically:

1. **Task 1: Wire all CLI subcommands into main.rs** - `e89d17a` (feat)
2. **Task 2: Integration tests for CLI subcommands** - `71fbe08` (test)

## Files Created/Modified
- `crates/rightclaw-cli/src/main.rs` - All subcommand implementations with async main
- `crates/rightclaw-cli/Cargo.toml` - Added tracing dependency
- `crates/rightclaw-cli/tests/cli_integration.rs` - 8 new integration tests
- `crates/rightclaw/src/runtime/sandbox.rs` - Added no_sandbox field to RuntimeState
- `crates/rightclaw/src/runtime/sandbox_tests.rs` - Fixed tests for new field

## Decisions Made
- Used `std::time::SystemTime` for timestamps instead of adding chrono dependency
- Extracted each subcommand to its own function (cmd_up, cmd_down, etc.) for readability
- Clippy-clean: all function params use `&Path` not `&PathBuf`

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added tracing dependency to CLI crate**
- **Found during:** Task 1
- **Issue:** main.rs uses tracing::debug/warn but tracing wasn't in CLI crate dependencies
- **Fix:** Added `tracing = { workspace = true }` to rightclaw-cli/Cargo.toml
- **Files modified:** crates/rightclaw-cli/Cargo.toml
- **Committed in:** e89d17a (Task 1 commit)

**2. [Rule 1 - Bug] Fixed RuntimeState construction in sandbox_tests.rs**
- **Found during:** Task 1
- **Issue:** Adding no_sandbox field to RuntimeState broke existing test struct literals
- **Fix:** Added `no_sandbox: false` to test struct construction
- **Files modified:** crates/rightclaw/src/runtime/sandbox_tests.rs
- **Committed in:** e89d17a (Task 1 commit)

---

**Total deviations:** 2 auto-fixed (1 blocking, 1 bug)
**Impact on plan:** Both necessary for compilation. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Phase 2 complete: all CLI lifecycle commands implemented and tested
- Ready for Phase 3: default agent content, install script, etc.
- External tools (process-compose, openshell, claude) required at runtime but not for build/test

---
*Phase: 02-cli-runtime-and-sandboxing*
*Completed: 2026-03-22*
