---
phase: 03-default-agent-and-installation
plan: 02
subsystem: infra
tags: [bash, installer, curl, process-compose, openshell]

# Dependency graph
requires:
  - phase: 01-project-init
    provides: rightclaw CLI binary structure
provides:
  - "install.sh curl-pipeable installer for rightclaw + dependencies"
  - "Platform detection for linux/darwin on x86_64/aarch64"
  - "cargo install fallback when GitHub Releases unavailable"
affects: [03-default-agent-and-installation, release-pipeline]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Bash installer with platform detection via uname"
    - "Official upstream installer scripts for dependencies"
    - "Full-path binary invocation to avoid PATH issues"

key-files:
  created:
    - install.sh
  modified: []

key-decisions:
  - "Use full path ($INSTALL_DIR/rightclaw) for post-install commands to avoid PATH refresh issues"
  - "cargo install fallback when GitHub release binary unavailable (no CI/CD pipeline yet)"
  - "Bun check as warning, not error -- optional for Telegram plugin users only"

patterns-established:
  - "Installer script structure: color helpers, platform detection, per-dependency install with skip-if-present, post-install verification"

requirements-completed: [INST-01]

# Metrics
duration: 1min
completed: 2026-03-22
---

# Phase 03 Plan 02: Install Script Summary

**Curl-pipeable install.sh with platform detection, dependency installation (process-compose + OpenShell), cargo fallback, and post-install init/doctor verification**

## Performance

- **Duration:** 1 min
- **Started:** 2026-03-22T18:32:28Z
- **Completed:** 2026-03-22T18:33:46Z
- **Tasks:** 1
- **Files modified:** 1

## Accomplishments

- Created install.sh at repo root with full platform detection (linux/darwin, x86_64/aarch64)
- Integrated official installers for process-compose and OpenShell with existing-install detection
- Added cargo install fallback for rightclaw binary (local source or crates.io)
- Post-install runs rightclaw init and rightclaw doctor via full path to avoid Pitfall 6

## Task Commits

Each task was committed atomically:

1. **Task 1: Create install.sh with platform detection and dependency installation** - `5868884` (feat)

## Files Created/Modified

- `install.sh` - Curl-pipeable installation script: platform detection, rightclaw/process-compose/OpenShell installation, init/doctor post-install steps

## Decisions Made

- Used full path (`$INSTALL_DIR/rightclaw`) for post-install commands instead of relying on PATH resolution -- avoids Pitfall 6 from research
- cargo install serves as fallback since no release CI/CD pipeline exists yet
- Bun absence generates a warning (not an error) since it's only needed for the optional Telegram channel plugin

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- install.sh is ready but depends on GitHub Releases pipeline for binary downloads (cargo fallback works in the meantime)
- Integrates with rightclaw init and rightclaw doctor commands (being built in parallel plans 03-03 and 03-04)

---
*Phase: 03-default-agent-and-installation*
*Completed: 2026-03-22*
