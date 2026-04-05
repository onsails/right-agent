---
phase: 03-default-agent-and-installation
plan: 01
subsystem: templates
tags: [openshell, policy, yaml, bootstrap, onboarding, landlock, sandbox]

requires:
  - phase: 01-project-skeleton
    provides: "templates/right/ directory with IDENTITY.md, SOUL.md, AGENTS.md, placeholder policy.yaml"
provides:
  - "BOOTSTRAP.md conversational onboarding template (4-question flow)"
  - "Production policy.yaml with hard_requirement Landlock and comprehensive HOW TO comments"
  - "policy-telegram.yaml variant with Telegram network rule uncommented"
affects: [03-02, 03-03, 03-04]

tech-stack:
  added: []
  patterns:
    - "Two policy.yaml variants (base/telegram) instead of runtime templating to preserve YAML comments"
    - "BOOTSTRAP.md as system prompt that drives conversational onboarding"

key-files:
  created:
    - templates/right/BOOTSTRAP.md
    - templates/right/policy-telegram.yaml
  modified:
    - templates/right/policy.yaml

key-decisions:
  - "Static policy files over templated generation to preserve YAML comments (D-14, D-15)"
  - "BOOTSTRAP.md uses conversational format with suggestion options, not form-like questions"

patterns-established:
  - "Policy variants: ship nearly-identical files differing only in commented/uncommented sections"
  - "Onboarding templates: system prompt style with section-based instructions"

requirements-completed: [DFLT-02, DFLT-03, DFLT-04, SAND-04, SAND-05, CHAN-03]

duration: 2min
completed: 2026-03-22
---

# Phase 3 Plan 1: Default Agent Templates Summary

**Conversational BOOTSTRAP.md onboarding (name/creature/vibe/emoji) plus self-documenting OpenShell policy.yaml with 6 HOW TO comments and Telegram-enabled variant**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-22T18:32:38Z
- **Completed:** 2026-03-22T18:35:06Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- BOOTSTRAP.md with 4-question conversational onboarding that writes IDENTITY.md, USER.md, SOUL.md then self-deletes
- Production policy.yaml with hard_requirement Landlock, filesystem/network/process restrictions, and 6 HOW TO expansion guides
- policy-telegram.yaml variant with api.telegram.org uncommented and ~/.bun + ~/.claude read_only entries active

## Task Commits

Each task was committed atomically:

1. **Task 1: Create BOOTSTRAP.md onboarding template** - `35869d9` (feat)
2. **Task 2: Create production policy.yaml and policy-telegram.yaml** - `6770ffa` (feat)

## Files Created/Modified
- `templates/right/BOOTSTRAP.md` - Conversational onboarding template: 4 questions, file-writing instructions, self-delete
- `templates/right/policy.yaml` - Full OpenShell policy with hard_requirement Landlock, commented Telegram section
- `templates/right/policy-telegram.yaml` - Telegram-enabled variant with uncommented network rules and Bun/Claude paths

## Decisions Made
- Used two static policy files instead of minijinja templating to preserve YAML comments as documentation (per D-14, D-15)
- BOOTSTRAP.md structured as conversational system prompt with suggestions per question, not a form

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Templates ready for include_str! embedding in Plan 03 (init command with Telegram token support)
- policy-telegram.yaml ready for conditional selection based on Telegram token presence
- BOOTSTRAP.md ready to be copied to agent directory during `rightclaw init`

---
*Phase: 03-default-agent-and-installation*
*Completed: 2026-03-22*
