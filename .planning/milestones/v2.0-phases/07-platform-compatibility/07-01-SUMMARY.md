---
phase: 07-platform-compatibility
plan: 01
subsystem: cli
tags: [bubblewrap, bwrap, socat, apparmor, sandbox, linux, doctor]

requires:
  - phase: 05-openshell-removal
    provides: "doctor.rs without openshell checks, check_binary pattern"
provides:
  - "Platform-conditional bwrap/socat binary checks in run_doctor()"
  - "bwrap smoke test with --unshare-net for AppArmor detection"
  - "bwrap_fix_guidance() with AppArmor profile and sysctl instructions"
affects: [07-02-install-script, doctor-output]

tech-stack:
  added: []
  patterns: ["platform-conditional checks via std::env::consts::OS", "smoke test via std::process::Command"]

key-files:
  created: []
  modified: ["crates/rightclaw/src/doctor.rs"]

key-decisions:
  - "Smoke test uses --unshare-net --dev /dev to match CC sandbox-runtime code path"
  - "AppArmor profile is primary fix, sysctl disable is secondary/temporary"
  - "No bwrap version check -- CC only checks PATH presence"

patterns-established:
  - "Platform-conditional binary checks: if std::env::consts::OS == 'linux' block after universal checks"
  - "Dependent checks: smoke test only runs if parent binary check passed"

requirements-completed: [PLAT-01, PLAT-02, PLAT-03]

duration: 2min
completed: 2026-03-24
---

# Phase 07 Plan 01: Doctor Sandbox Checks Summary

**Linux-specific bwrap/socat binary detection and bwrap smoke test with AppArmor diagnostics in rightclaw doctor**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-24T15:10:37Z
- **Completed:** 2026-03-24T15:13:18Z
- **Tasks:** 1 (TDD: RED + GREEN + REFACTOR)
- **Files modified:** 1

## Accomplishments
- `run_doctor()` checks for `bwrap` and `socat` binaries on Linux, skips on macOS
- `check_bwrap_sandbox()` runs smoke test with `--unshare-net --dev /dev` to detect AppArmor restrictions
- `bwrap_fix_guidance()` provides AppArmor profile (primary) and sysctl (temporary) fix instructions
- All 15 doctor tests pass including 4 new tests and all existing ones
- PLAT-03 confirmed: no openshell references in doctor checks (existing test validates)

## Task Commits

Each task was committed atomically:

1. **Task 1 (RED): Add failing tests for bwrap/socat checks** - `6401a06` (test)
2. **Task 1 (GREEN): Implement bwrap/socat checks and smoke test** - `d7a6e1e` (feat)

## Files Created/Modified
- `crates/rightclaw/src/doctor.rs` - Added `check_bwrap_sandbox()`, `bwrap_fix_guidance()`, platform-conditional block in `run_doctor()`, 4 new tests

## Decisions Made
- Smoke test includes `--unshare-net --dev /dev` flags (not just `--ro-bind / /`) per RESEARCH.md Pitfall 1 -- the simpler variant would miss AppArmor network namespace restrictions
- AppArmor profile with `abi <abi/4.0>` and `userns,` permission is the primary fix (targeted, secure) per RESEARCH.md Pitfall 2
- stderr parsing distinguishes AppArmor ("RTM_NEWADDR", "Operation not permitted") from Debian userns ("No permissions") for appropriate error messages

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Doctor sandbox checks complete, ready for install.sh updates (plan 07-02)
- bwrap smoke test gives actionable diagnostics for Ubuntu 24.04+ users

## Self-Check: PASSED

- crates/rightclaw/src/doctor.rs: FOUND
- Commit 6401a06 (test RED): FOUND
- Commit d7a6e1e (feat GREEN): FOUND

---
*Phase: 07-platform-compatibility*
*Completed: 2026-03-24*
