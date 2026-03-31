---
phase: 24-system-prompt-codegen
plan: "03"
subsystem: infra
tags: [init, templates, user-md, agents-md, rightcron, communication]

requires:
  - phase: 24-system-prompt-codegen
    provides: "system prompt codegen context and decisions D-06/D-13 about moving operational content to AGENTS.md"

provides:
  - "templates/right/USER.md placeholder for learned user preferences"
  - "templates/right/AGENTS.md with Communication + Cron Management sections"
  - "rightclaw init creates USER.md in the default right agent directory"

affects: [24-system-prompt-codegen, system-prompt-compose, rightclaw-init]

tech-stack:
  added: []
  patterns:
    - "include_str! for static template files embedded at compile time"
    - "SOUL.md -> USER.md -> AGENTS.md canonical file order (D-02)"

key-files:
  created:
    - templates/right/USER.md
  modified:
    - templates/right/AGENTS.md
    - crates/rightclaw/src/init.rs

key-decisions:
  - "D-13: USER.md is a minimal placeholder — agent fills it through interaction"
  - "D-06: Communication and Cron Management sections moved from hardcoded codegen to AGENTS.md template"
  - "D-02: File order SOUL.md -> USER.md -> AGENTS.md maintained in files array"

patterns-established:
  - "Operational guidance belongs in AGENTS.md, not hardcoded in codegen"

requirements-completed: [PROMPT-01]

duration: 7min
completed: 2026-03-31
---

# Phase 24 Plan 03: System Prompt Codegen — USER.md and AGENTS.md Templates Summary

**USER.md placeholder created and AGENTS.md enriched with Communication + RightCron operational guidance, both now deployed by `rightclaw init`**

## Performance

- **Duration:** ~7 min
- **Started:** 2026-03-31T22:34:00Z
- **Completed:** 2026-03-31T22:41:00Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments

- Created `templates/right/USER.md` with minimal placeholder comment (D-13: agent fills through interaction)
- Appended Communication (daemon/reply-MCP-tool) and Cron Management (rightcron startup) sections to `templates/right/AGENTS.md` (D-06)
- Updated `init.rs` to embed USER.md and include it in the files array between SOUL.md and AGENTS.md (D-02 order)
- All 19 init tests pass; workspace builds clean

## Task Commits

1. **Task 1: Create USER.md template and update AGENTS.md with operational content** - `d38471c` (feat)
2. **Task 2: Update init.rs to include USER.md in default agent file set** - `6fb224b` (feat)

**Plan metadata:** (committed below with SUMMARY.md)

## Files Created/Modified

- `templates/right/USER.md` - Created: minimal placeholder comment for learned user preferences
- `templates/right/AGENTS.md` - Modified: appended Communication and Cron Management sections
- `crates/rightclaw/src/init.rs` - Modified: DEFAULT_USER const, USER.md in files array, println!, test assertion

## Decisions Made

None beyond plan spec — followed D-02, D-06, D-13 from RESEARCH.md exactly.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- `rightclaw init` now produces USER.md alongside other identity files
- AGENTS.md carries operational guidance agents need without relying on hardcoded codegen sections
- System prompt composition (SOUL.md + USER.md + AGENTS.md) can proceed in remaining phase 24 work

---
*Phase: 24-system-prompt-codegen*
*Completed: 2026-03-31*
