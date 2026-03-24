---
phase: 07-platform-compatibility
plan: 02
subsystem: infra
tags: [install, bubblewrap, socat, sandbox, bash, platform-detection]

# Dependency graph
requires:
  - phase: 05-remove-openshell
    provides: OpenShell removal from codebase (install.sh still had openshell function)
provides:
  - install.sh with Linux sandbox deps (bubblewrap + socat) via apt/dnf/pacman
  - install.sh with no OpenShell references
  - macOS early-return with Seatbelt message
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns: [selective-package-install, platform-conditional-install]

key-files:
  created: []
  modified: [install.sh]

key-decisions:
  - "Only install missing packages (check bwrap/socat individually before installing)"
  - "Use die() for unsupported package manager with manual install instructions"

patterns-established:
  - "install_sandbox_deps: platform-aware function with macOS early return and Linux package manager detection"

requirements-completed: [PLAT-04, PLAT-05]

# Metrics
duration: 1min
completed: 2026-03-24
---

# Phase 7 Plan 2: Install Script Sandbox Deps Summary

**Replace OpenShell installation with bubblewrap + socat Linux deps and macOS Seatbelt early-return in install.sh**

## Performance

- **Duration:** 1 min
- **Started:** 2026-03-24T15:10:27Z
- **Completed:** 2026-03-24T15:11:26Z
- **Tasks:** 1
- **Files modified:** 1

## Accomplishments
- Removed `install_openshell()` function and all OpenShell references from install.sh
- Added `install_sandbox_deps()` with selective package install (only missing deps) via apt-get, dnf, and pacman
- macOS returns early with Seatbelt message (no sandbox deps needed)

## Task Commits

Each task was committed atomically:

1. **Task 1: Replace OpenShell install with sandbox deps in install.sh** - `53761aa` (feat)

## Files Created/Modified
- `install.sh` - Replaced install_openshell() with install_sandbox_deps(), updated header comment

## Decisions Made
- Only install missing packages: checks bwrap and socat individually via `command -v`, builds package list of only what's needed
- die() with manual install instructions when no supported package manager found (apt-get, dnf, pacman)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- install.sh is fully updated for the native sandbox dependency chain
- Doctor check updates (plan 07-01) handle runtime verification of these same dependencies

## Self-Check: PASSED

- install.sh: FOUND
- 07-02-SUMMARY.md: FOUND
- Commit 53761aa: FOUND

---
*Phase: 07-platform-compatibility*
*Completed: 2026-03-24*
