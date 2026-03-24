---
phase: 09-agent-environment-setup
plan: 02
subsystem: runtime
tags: [git, telegram, skills, codegen, cmd_up, doctor]

# Dependency graph
requires:
  - phase: 09-agent-environment-setup-plan-01
    provides: generate_telegram_channel_config, install_builtin_skills codegen functions with tests
  - phase: 08-home-isolation-permission-model
    provides: cmd_up per-agent loop structure, host_home pre-resolved, create_credential_symlink
provides:
  - cmd_up per-agent loop with steps 6-9 (git init, telegram config, skills install, settings.local.json)
  - git Warn-severity check in verify_dependencies (doctor shows warning if git absent)
affects:
  - Phase 10 and beyond (cmd_up per-agent loop is the integration point for future env setup)

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Non-fatal process::Command in per-agent loop: match on status, warn on Err, never ?"
    - "create-if-absent pattern: if !path.exists() { fs::write(...) } preserves runtime writes"
    - "Warn-only binary check: which::which(...).is_err() -> tracing::warn!, no ? operator"

key-files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw/src/runtime/deps.rs

key-decisions:
  - "git init is non-fatal in cmd_up: uses match without ?, logs warn on any error (binary absent or nonzero exit)"
  - "settings.local.json written with {} only when absent — preserves runtime agent writes (D-11)"
  - "git check in verify_dependencies is Warn severity only — agents run fine without git"

patterns-established:
  - "Steps 6-9 ordering in cmd_up per-agent loop: git init -> telegram -> skills -> settings.local.json"

requirements-completed:
  - AENV-01
  - AENV-02
  - AENV-03
  - PERM-03

# Metrics
duration: 4min
completed: 2026-03-24
---

# Phase 9 Plan 02: Agent Environment Setup — Integration into cmd_up Summary

**Wired git init, Telegram channel config, built-in skills reinstall, and settings.local.json pre-creation into cmd_up per-agent loop, and added git Warn check to doctor**

## Performance

- **Duration:** 4 min
- **Started:** 2026-03-24T23:33:00Z
- **Completed:** 2026-03-24T23:36:48Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments

- cmd_up per-agent loop now executes steps 6-9 in D-12 order after Phase 8 steps
- git init is non-fatal (match without ?, warns on binary-not-found or nonzero exit)
- `generate_telegram_channel_config` and `install_builtin_skills` called per-agent on every `up`
- `settings.local.json` written with `{}` only when absent (preserves runtime CC writes)
- `verify_dependencies()` warns (not errors) when git not in PATH
- 7 new tests: 6 in main.rs covering all conditional behaviors, 1 in deps.rs for non-fatal git check

## Task Commits

1. **Task 1: Extend cmd_up per-agent loop with Phase 9 steps** - `dba0820` (feat)
2. **Task 2: Add git Warn check to doctor (deps.rs)** - `92d36ee` (feat)

## Files Created/Modified

- `crates/rightclaw-cli/src/main.rs` - Four new blocks (steps 6-9) added to per-agent loop; 6 unit tests added
- `crates/rightclaw/src/runtime/deps.rs` - Git Warn-only check added to verify_dependencies; 1 new test

## Decisions Made

- git init uses `match` without `?` — non-fatal by design; agents can run without git (just won't have workspace trust)
- `settings.local.json` conditional write guards on `!settings_local.exists()` — protects runtime state written by CC or agents during sessions
- git check in doctor is Warn severity (not Fail) — consistent with D-03 decision from context

## Deviations from Plan

None - plan executed exactly as written. All four blocks added verbatim per plan spec. Tests pass. Clippy clean.

## Issues Encountered

None. The codegen functions from Plan 01 compiled and behaved correctly on first integration.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- All Phase 9 requirements fully implemented (AENV-01, AENV-02, AENV-03, PERM-03)
- cmd_up per-agent loop complete through step 9
- Doctor shows git warning when git absent without blocking startup
- 154 tests pass total (148 lib + 6 unit in main.rs); pre-existing test_status_no_running_instance failure is unrelated

---
*Phase: 09-agent-environment-setup*
*Completed: 2026-03-24*

## Self-Check: PASSED

- FOUND: .planning/phases/09-agent-environment-setup/09-02-SUMMARY.md
- FOUND: crates/rightclaw-cli/src/main.rs
- FOUND: crates/rightclaw/src/runtime/deps.rs
- FOUND: commit dba0820 (feat: extend cmd_up per-agent loop)
- FOUND: commit 92d36ee (feat: add git Warn check to deps)
