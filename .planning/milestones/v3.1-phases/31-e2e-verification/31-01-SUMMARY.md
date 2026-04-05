---
phase: 31-e2e-verification
plan: 01
subsystem: testing
tags: [bash, e2e, sandbox, bwrap, claude-code, verification]

# Dependency graph
requires:
  - phase: 30-doctor-diagnostics
    provides: rightclaw doctor command with structured FAIL/ok/warn output
  - phase: 29-sandbox-fix
    provides: settings.json with failIfUnavailable:true, reply-schema.json per agent
provides:
  - tests/e2e/verify-sandbox.sh — 4-stage bash pipeline confirming sandbox engagement
  - Repeatable VER-01/VER-02/VER-03 verification for any live agent
affects: [future-phases, ci-integration]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "E2E sandbox proof via failIfUnavailable:true + CC exit code (not stderr grep)"
    - "CC invocation mirror: subshell cd + HOME override matches worker.rs current_dir pattern"
    - "Multi-stage abort-early bash pipeline with structured pass/fail counters"

key-files:
  created:
    - tests/e2e/verify-sandbox.sh
    - tests/e2e/.gitignore
  modified: []

key-decisions:
  - "Tasks 1+2 collapsed: subshell cwd written directly in initial implementation"
  - "Sandbox proof via exit code under failIfUnavailable:true (not brittle stderr parsing)"
  - "CC invocation uses subshell cd to match worker.rs current_dir behavior exactly"

patterns-established:
  - "E2E scripts live in tests/e2e/, last-run.log excluded via .gitignore"

requirements-completed: [VER-01, VER-02, VER-03]

# Metrics
duration: 2min
completed: 2026-04-02
---

# Phase 31 Plan 01: E2E Sandbox Verification Script Summary

**4-stage bash pipeline in tests/e2e/verify-sandbox.sh confirms CC sandbox engagement via doctor pre-flight, dependency PATH check, settings.json pre-flight, and CC smoke test with failIfUnavailable:true exit-code proof**

## Performance

- **Duration:** 2 min
- **Started:** 2026-04-02T22:14:41Z
- **Completed:** 2026-04-02T22:16:00Z
- **Tasks:** 2 (collapsed into 1 commit)
- **Files modified:** 2

## Accomplishments
- Created `tests/e2e/verify-sandbox.sh` — executable bash script with 4 stages: doctor pre-flight, dep PATH check, settings pre-flight, CC smoke test
- CC invocation mirrors `worker.rs:371-408` exactly: subshell `cd "$AGENT_DIR"`, `HOME` override, `USE_BUILTIN_RIPGREP=0`, all flags including `--dangerously-skip-permissions`
- Sandbox proof strategy: `failIfUnavailable:true` in settings.json means CC exit 0 guarantees sandbox engaged — no brittle stderr parsing
- Created `tests/e2e/.gitignore` to exclude `last-run.log` from VCS

## Task Commits

1. **Tasks 1+2: Create verify-sandbox.sh with subshell cwd** - `d8142cd` (feat)

**Plan metadata:** (pending)

## Files Created/Modified
- `tests/e2e/verify-sandbox.sh` - 4-stage sandbox verification pipeline, executable
- `tests/e2e/.gitignore` - Excludes last-run.log from git

## Decisions Made
- Tasks 1 and 2 collapsed: the plan explicitly permitted this if executor writes the full script with subshell at once
- Sandbox proof via exit code under `failIfUnavailable:true` (not stderr grep) — avoids brittleness across CC versions per 31-DISCUSSION-LOG.md D-09

## Deviations from Plan

None - plan executed exactly as written (Task 1+2 collapse was explicitly permitted by the plan).

## Issues Encountered
None.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- `tests/e2e/verify-sandbox.sh` is ready for manual run against any live agent
- Prerequisites: `rightclaw up <agent-name>` must have been run; `rg`, `socat`, `bwrap` must be in PATH
- Full run verification (Stage 4 with real CC) requires a live agent setup

---
*Phase: 31-e2e-verification*
*Completed: 2026-04-02*

## Self-Check: PASSED
- tests/e2e/verify-sandbox.sh: FOUND, EXECUTABLE
- tests/e2e/.gitignore: FOUND
- Commit d8142cd: FOUND
