---
phase: 20-diagnosis
plan: 01
subsystem: diagnosis
tags: [telegram, cc-event-loop, sandbox, socat, SubagentStop, M6, grammy]

# Dependency graph
requires: []
provides:
  - DIAGNOSIS.md naming CC event loop stall (post-SubagentStop M6 gap) as root cause
  - Elimination of Hypothesis B (socat TCP timeout) with process topology evidence
  - Two confirmation tests for Phase 21 regression guard
  - Fix proposal: Option A (persistent background agent keeps step cycle active)
affects: [21-fix]

# Tech tracking
tech-stack:
  added: []
  patterns: []

key-files:
  created:
    - .planning/phases/20-diagnosis/DIAGNOSIS.md
  modified: []

key-decisions:
  - "Hypothesis B (socat TCP timeout) is structurally impossible — Telegram plugin runs outside bwrap, shares host network namespace (inode 4026531840), direct TCP to api.telegram.org, no proxy"
  - "Hypothesis A (CC event loop stall) confirmed via cli.js bundle analysis — iv6 callback does not call M6() when Z===null (idle); channel messages queue in hz with no drain mechanism"
  - "The 1-hour delay is not a real diagnostic signal — failure begins immediately after SubagentStop; 1 hour was when user happened to send the next message"
  - "Works-without-sandbox observation is confounded — test session likely had no rightcron startup_prompt, so SubagentStop never fired"
  - "Recommended fix for Phase 21: Option A (persistent background agent / rightcron watch mode) — only strategy that guarantees M6() is called without patching CC"

patterns-established: []

requirements-completed: [DIAG-01, DIAG-02, DIAG-03]

# Metrics
duration: 2min
completed: 2026-03-28
---

# Phase 20 Plan 01: Diagnosis Summary

**CC event loop stall diagnosed: iv6() callback never calls M6() when Z===null after SubagentStop, leaving Telegram channel messages queued in hz indefinitely**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-28T21:51:48Z
- **Completed:** 2026-03-28T21:53:41Z
- **Tasks:** 1 of 1
- **Files modified:** 1

## Accomplishments

- Eliminated socat TCP timeout hypothesis with process topology evidence (plugin outside bwrap, direct TCP to api.telegram.org, network namespace inode confirmed)
- Confirmed CC event loop stall as root cause via cli.js bundle analysis — iv6() subscriber never calls M6() when idle, channel messages sit in hz queue
- Identified SubagentStop as the exact trigger — messages work during rightcron run because M6() is called after each step; fails immediately after SubagentStop
- Produced DIAGNOSIS.md with two confirmation tests and three-option fix proposal for Phase 21

## Task Commits

1. **Task 1: Write DIAGNOSIS.md** - `1c616e2` (docs)

## Files Created/Modified

- `.planning/phases/20-diagnosis/DIAGNOSIS.md` - Root cause analysis: evidence summary, hypothesis evaluation, confirmation tests, Phase 21 fix proposal

## Decisions Made

- Hypothesis B (socat) eliminated — structurally impossible given plugin process topology
- Hypothesis A (CC event loop) confirmed — no M6() call path from iv6() when idle
- 1-hour delay is not real — failure is immediate post-SubagentStop
- "Works without sandbox" is confounded by test conditions (no rightcron in that session)
- Phase 21 recommended approach: Option A (persistent background agent)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- DIAGNOSIS.md ready as Phase 21 primary input
- Confirmation Test A (send message 30s after SubagentStop) should be first Phase 21 test
- Phase 21 should evaluate rightcron watch mode vs separate heartbeat agent strategy
- CC bug report (Option C) should be filed in parallel regardless of rightclaw fix chosen

---
*Phase: 20-diagnosis*
*Completed: 2026-03-28*
