---
phase: 03-default-agent-and-installation
plan: 03
subsystem: cli
tags: [doctor, init, telegram, validation, which, bootstrap, policy-variant]

requires:
  - phase: 03-default-agent-and-installation
    plan: 01
    provides: "BOOTSTRAP.md, policy.yaml, policy-telegram.yaml templates"
  - phase: 01-project-skeleton
    provides: "init.rs with init_rightclaw_home(), lib.rs module structure, which crate"
provides:
  - "doctor.rs module with run_doctor(), DoctorCheck, CheckStatus for dependency/agent validation"
  - "Extended init_rightclaw_home() with telegram_token, BOOTSTRAP.md, policy variant selection"
  - "validate_telegram_token() format checker"
  - "prompt_telegram_token() interactive input"
affects: [03-04]

tech-stack:
  added: []
  patterns:
    - "Doctor runs all checks without short-circuiting (unlike verify_dependencies)"
    - "telegram_env_dir parameter for testable token storage path"

key-files:
  created:
    - crates/rightclaw/src/doctor.rs
  modified:
    - crates/rightclaw/src/init.rs
    - crates/rightclaw/src/lib.rs
    - crates/rightclaw-cli/src/main.rs

key-decisions:
  - "telegram_env_dir parameter on init_rightclaw_home for testability instead of always writing to ~/.claude"
  - "Doctor module uses check_binary + check_agent_structure separation for clarity"
  - "BOOTSTRAP.md presence reported as Warn (not Fail) since it's expected on fresh installs"

patterns-established:
  - "Doctor check pattern: collect all results, report at end, never short-circuit"
  - "Optional path override for testability of filesystem writes"

requirements-completed: [DFLT-01, INST-02, INST-03, CHAN-01, CHAN-02]

duration: 3min
completed: 2026-03-22
---

# Phase 3 Plan 3: Doctor Module and Init Extension Summary

**Doctor command checking 4 binaries + agent structure, init extended with telegram_token/BOOTSTRAP.md/policy variant selection, validate_telegram_token format checker**

## Performance

- **Duration:** 3 min
- **Started:** 2026-03-22T18:37:06Z
- **Completed:** 2026-03-22T18:40:54Z
- **Tasks:** 2
- **Files modified:** 4

## Accomplishments
- doctor.rs module with run_doctor() checking rightclaw, process-compose, openshell, claude in PATH plus agent directory structure validation
- init_rightclaw_home extended with telegram_token parameter, BOOTSTRAP.md creation, policy-telegram.yaml variant selection, and token .env file writing
- validate_telegram_token with numeric:alphanumeric format checking and prompt_telegram_token for interactive input
- 86 total library tests passing, clippy clean

## Task Commits

Each task was committed atomically:

1. **Task 1: Create doctor.rs module with run_doctor()** - `18d0e8b` (feat)
2. **Task 2: Extend init.rs with telegram_token, BOOTSTRAP.md, policy variant** - `bfc289a` (feat)

## Files Created/Modified
- `crates/rightclaw/src/doctor.rs` - Doctor module: DoctorCheck/CheckStatus types, run_doctor(), check_binary(), check_agent_structure(), 12 tests
- `crates/rightclaw/src/init.rs` - Extended init with telegram_token, BOOTSTRAP.md, policy variant, validate/prompt functions, 14 tests
- `crates/rightclaw/src/lib.rs` - Added pub mod doctor
- `crates/rightclaw-cli/src/main.rs` - Updated cmd_init to pass new parameters

## Decisions Made
- Added `telegram_env_dir: Option<&Path>` parameter for testability -- tests use tempdir instead of writing to real `~/.claude/channels/telegram/`
- BOOTSTRAP.md presence in agent dir reported as CheckStatus::Warn ("first-run onboarding pending") rather than Fail, since it's expected before first launch
- Doctor binary checks include fix hints (install URLs/commands) on failure

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed BOOTSTRAP.md content assertion case sensitivity**
- **Found during:** Task 2 (init tests)
- **Issue:** Test asserted `bootstrap.contains("first-run onboarding")` but BOOTSTRAP.md frontmatter has `"First-run onboarding"` (capital F)
- **Fix:** Updated assertion to match actual content
- **Files modified:** crates/rightclaw/src/init.rs
- **Committed in:** bfc289a (Task 2 commit)

**2. [Rule 1 - Bug] Fixed clippy vec_init_then_push warning in doctor.rs**
- **Found during:** Final verification
- **Issue:** Clippy warned about creating empty Vec then pushing 4 items
- **Fix:** Changed to vec![] macro initialization
- **Files modified:** crates/rightclaw/src/doctor.rs
- **Committed in:** bfc289a (Task 2 commit)

---

**Total deviations:** 2 auto-fixed (2 bugs)
**Impact on plan:** Minor corrections, no scope change.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Doctor module ready for CLI subcommand wiring (Plan 03-04)
- init with telegram_token ready for --telegram-token CLI flag (Plan 03-04)
- prompt_telegram_token ready for interactive init flow

## Self-Check: PASSED

All 4 files verified present. Both commit hashes (18d0e8b, bfc289a) found in git log.

---
*Phase: 03-default-agent-and-installation*
*Completed: 2026-03-22*
