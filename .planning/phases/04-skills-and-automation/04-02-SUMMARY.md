---
phase: 04-skills-and-automation
plan: 02
subsystem: codegen
tags: [minijinja, system-prompt, cronsync, shell-wrapper, codegen]

# Dependency graph
requires:
  - phase: 02-runtime-orchestration
    provides: "Shell wrapper codegen (generate_wrapper) and template (agent-wrapper.sh.j2)"
provides:
  - "codegen::system_prompt module with generate_system_prompt -> Option<String>"
  - "Shell wrapper template conditional second --append-system-prompt-file for system prompt"
  - "cmd_up generates system prompt file at run/<agent>-system.md for crons-enabled agents"
affects: [04-skills-and-automation]

# Tech tracking
tech-stack:
  added: []
  patterns: ["Conditional system prompt generation based on agent crons/ directory presence"]

key-files:
  created:
    - crates/rightclaw/src/codegen/system_prompt.rs
    - crates/rightclaw/src/codegen/system_prompt_tests.rs
  modified:
    - crates/rightclaw/src/codegen/mod.rs
    - crates/rightclaw/src/codegen/shell_wrapper.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - templates/agent-wrapper.sh.j2
    - crates/rightclaw-cli/src/main.rs

key-decisions:
  - "generate_system_prompt returns Option<String> (None when no crons/ dir) instead of empty string"
  - "System prompt file placed at run/<agent>-system.md, regenerated on every rightclaw up"

patterns-established:
  - "Conditional template variable pattern: pass Option to template context for optional CLI flags"

requirements-completed: [CRON-01]

# Metrics
duration: 3min
completed: 2026-03-22
---

# Phase 04 Plan 02: System Prompt Codegen Summary

**System prompt codegen module generating CronSync bootstrap instructions for crons-enabled agents, wired into shell wrapper via conditional --append-system-prompt-file**

## Performance

- **Duration:** 3 min
- **Started:** 2026-03-22T20:14:34Z
- **Completed:** 2026-03-22T20:17:39Z
- **Tasks:** 2
- **Files modified:** 7

## Accomplishments
- Created system_prompt.rs module that generates CronSync bootstrap content for agents with crons/ directory
- Updated shell wrapper template with conditional second --append-system-prompt-file in both sandbox and no-sandbox modes
- Extended generate_wrapper signature with system_prompt_path parameter, all existing tests updated
- Wired system prompt generation into cmd_up loop -- writes to run/<agent>-system.md and passes path to wrapper

## Task Commits

Each task was committed atomically:

1. **Task 1: System prompt generation module and shell wrapper template update** - `c02c440` (feat)
2. **Task 2: Wire system prompt generation into cmd_up** - `7c6aef9` (feat)

## Files Created/Modified
- `crates/rightclaw/src/codegen/system_prompt.rs` - System prompt generation with crons/ directory detection
- `crates/rightclaw/src/codegen/system_prompt_tests.rs` - 5 tests: crons exists/absent/file-not-dir, content checks
- `crates/rightclaw/src/codegen/mod.rs` - Module re-export of system_prompt
- `crates/rightclaw/src/codegen/shell_wrapper.rs` - Extended signature with system_prompt_path: Option<&str>
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` - Updated all calls for new signature + 3 new tests
- `templates/agent-wrapper.sh.j2` - Conditional second --append-system-prompt-file in both branches
- `crates/rightclaw-cli/src/main.rs` - cmd_up generates system prompt file and passes path to wrapper

## Decisions Made
- Used `Option<String>` return type for generate_system_prompt instead of empty string -- cleaner API, None clearly signals "no system prompt needed"
- System prompt file at `run/<agent>-system.md` is regenerated on every `rightclaw up` invocation (per D-17)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- System prompt codegen complete, ready for CronSync skill (Plan 04-01) to produce the instructions that trigger /cronsync
- Shell wrapper template now supports optional system prompt file alongside identity file

## Self-Check: PASSED

All 8 files verified present. Both commits (c02c440, 7c6aef9) verified in git log.

---
*Phase: 04-skills-and-automation*
*Completed: 2026-03-22*
