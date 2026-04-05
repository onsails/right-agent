---
phase: 10-doctor-managed-settings
plan: 01
subsystem: cli
tags: [miette, serde_json, doctor, managed-settings, config, clap]

requires:
  - phase: 09-agent-environment-setup
    provides: doctor.rs DoctorCheck/CheckStatus patterns, cmd_* function shape in main.rs

provides:
  - "rightclaw config strict-sandbox command writes /etc/claude-code/managed-settings.json"
  - "check_managed_settings() in doctor.rs with silent-skip/strict-warn/generic-warn branches"
  - "Commands::Config subcommand with ConfigCommands::StrictSandbox variant"
  - "write_managed_settings() helper extracted for testability"

affects: [v2.1-milestone-complete, future-config-subcommands]

tech-stack:
  added: []
  patterns:
    - "Nested clap subcommand: Commands::Config { command: ConfigCommands } with own enum"
    - "Extract write helper for testability: cmd_X() calls write_X(dir, path) so tests pass temp paths"
    - "check_managed_settings(path: &str) -> Option<DoctorCheck>: None = absent, Some(Warn) = present"
    - "serde_json::Value path for JSON check with fallback to generic warn for all error cases"

key-files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw/src/doctor.rs

key-decisions:
  - "write_managed_settings(dir, path) extracted from cmd_config_strict_sandbox for testability — lets tests pass temp paths without touching /etc"
  - "check_managed_settings takes &str path (not hardcoded) — run_doctor passes constant, tests pass temp files"
  - "Silent skip (None) when managed-settings.json absent — avoids polluting doctor output for majority of users"
  - "use super::{write_managed_settings, ConfigCommands} import in test module — avoids use super::*"

patterns-established:
  - "Config subcommand pattern: top-level Commands::Config { command: ConfigCommands } with nested enum"
  - "Testable fs helper: fn write_X(dir: &str, path: &str) extracted from cmd_X for parameterized path injection"

requirements-completed: [TOOL-01, TOOL-02]

duration: 3min
completed: 2026-03-25
---

# Phase 10 Plan 01: Doctor Managed Settings Summary

**`rightclaw config strict-sandbox` writes /etc/claude-code/managed-settings.json with `allowManagedDomainsOnly:true`; doctor warns when file exists with rich or generic detail depending on content**

## Performance

- **Duration:** 3 min
- **Started:** 2026-03-25T12:34:17Z
- **Completed:** 2026-03-25T12:37:xx Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- Added `rightclaw config strict-sandbox` nested subcommand via `Commands::Config` + `ConfigCommands::StrictSandbox`
- `write_managed_settings(dir, path)` helper extracted for testability — creates dir, writes exact JSON, returns miette error with sudo hint on permission denied
- `check_managed_settings(path)` in doctor.rs: returns None when absent (D-08), Warn with `allowManagedDomainsOnly:true` message (D-06), Warn with generic "content may affect" message for all other cases (D-07)
- 10 new tests across both crates (4 config, 6 doctor), zero regressions

## Task Commits

Each task was committed atomically:

1. **Task 1: Add config strict-sandbox command to CLI** - `0c27c3a` (feat)
2. **Task 2: Add managed settings check to doctor** - `9debef3` (feat)

**Plan metadata:** (docs commit follows)

_Note: Both tasks used TDD — tests written first (RED), implementation added (GREEN), all tests pass_

## Files Created/Modified
- `crates/rightclaw-cli/src/main.rs` - Added ConfigCommands enum, Commands::Config variant, write_managed_settings(), cmd_config_strict_sandbox(), 4 tests
- `crates/rightclaw/src/doctor.rs` - Added MANAGED_SETTINGS_PATH constant, check_managed_settings() function, wired into run_doctor(), 6 tests

## Decisions Made
- Extracted `write_managed_settings(dir, path)` helper so tests can inject temp paths without needing root. The public `cmd_config_strict_sandbox()` calls it with the real `/etc/claude-code` constants.
- `check_managed_settings` takes `path: &str` (not hardcoded path) — same testability pattern as the CLI helper. `run_doctor()` passes the `MANAGED_SETTINGS_PATH` constant.
- Test module uses `use super::{write_managed_settings, ConfigCommands}` (explicit imports) rather than `use super::*` to keep test dependencies visible.

## Deviations from Plan

None — plan executed exactly as written.

## Issues Encountered
- Test module initially missing imports (`write_managed_settings` not in scope). Fixed by adding `use super::{write_managed_settings, ConfigCommands}` at top of test module. Not a deviation — normal Rust module scoping.

## User Setup Required
None — no external service configuration required.

## Next Phase Readiness
- v2.1 milestone complete. `rightclaw config strict-sandbox` and doctor conflict detection shipped.
- No blockers for milestone closure.
- Pre-existing `test_status_no_running_instance` failure is unchanged (known issue from Phase 9).

## Self-Check: PASSED

- SUMMARY.md: FOUND
- Commit 0c27c3a (Task 1): FOUND
- Commit 9debef3 (Task 2): FOUND

---
*Phase: 10-doctor-managed-settings*
*Completed: 2026-03-25*
