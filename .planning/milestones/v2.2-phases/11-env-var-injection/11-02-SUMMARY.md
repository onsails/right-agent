---
phase: 11-env-var-injection
plan: "02"
subsystem: codegen
tags: [skills, agent-config, yaml-template, tdd]

# Dependency graph
requires:
  - phase: 09-agent-environment-setup
    provides: create-if-absent pattern for settings.local.json (reused for installed.json)
provides:
  - create-if-absent installed.json — user skill state survives rightclaw up restarts
  - commented env: example block in generated agent.yaml with plaintext warning
affects: [11-01, agent-init, skill-install]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "create-if-absent: check !path.exists() before fs::write — preserves user data across restarts"

key-files:
  created: []
  modified:
    - crates/rightclaw/src/codegen/skills.rs
    - templates/right/agent.yaml

key-decisions:
  - "installed.json uses create-if-absent (not unconditional overwrite) — same pattern as settings.local.json from Phase 9"
  - "env: example in agent.yaml is fully commented out — zero impact on existing agents, purely documentary"

patterns-established:
  - "create-if-absent: check !path.exists() before fs::write when preserving user-managed state"

requirements-completed: [ENV-04, ENV-05]

# Metrics
duration: 8min
completed: 2026-03-25
---

# Phase 11 Plan 02: Env Var Injection (Part 2) Summary

**Create-if-absent installed.json fix and commented env: example in agent.yaml template — user skill state no longer clobbered on restart**

## Performance

- **Duration:** 8 min
- **Started:** 2026-03-25T00:00:00Z
- **Completed:** 2026-03-25T00:08:00Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments

- Fixed data-loss bug: `install_builtin_skills()` no longer overwrites user-modified `installed.json` on subsequent calls
- Added TDD regression tests: `installed_json_preserves_existing_content` and `installed_json_created_on_first_call`
- Added commented `env:` example block to `templates/right/agent.yaml` with explicit plaintext warning and file-reference guidance

## Task Commits

Each task was committed atomically:

1. **Task 1: Fix installed.json create-if-absent (TDD)** - `242f59c` (feat)
2. **Task 2: Add env: commented example to agent.yaml template** - `56c7de9` (feat)

**Plan metadata:** TBD (docs: complete plan)

_Note: Task 1 followed TDD — test written first (RED confirmed), implementation fixed (GREEN confirmed)_

## Files Created/Modified

- `crates/rightclaw/src/codegen/skills.rs` — Changed unconditional `fs::write` to create-if-absent; added 2 new regression tests
- `templates/right/agent.yaml` — Added commented `env:` example block with plaintext warning after `backoff_seconds`

## Decisions Made

- `installed.json` create-if-absent follows the Phase 9 `settings.local.json` pattern exactly — consistent approach across user-managed state files
- `env:` example in agent.yaml is fully commented (not active YAML) — zero parsing impact on existing agents without `env:` field

## Deviations from Plan

None — plan executed exactly as written.

## Issues Encountered

None. The pre-existing `test_status_no_running_instance` failure (documented in MEMORY.md) appeared in the workspace test run but is out of scope for this plan.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- Phase 11 Plan 02 complete: installed.json data-safety and env: template documentation done
- Phase 11 Plan 01 (AgentConfig.env field + shell wrapper injection) is parallel/independent work
- Once both plans complete, agents can declare per-agent env vars that survive restarts without losing skill state

---
*Phase: 11-env-var-injection*
*Completed: 2026-03-25*
