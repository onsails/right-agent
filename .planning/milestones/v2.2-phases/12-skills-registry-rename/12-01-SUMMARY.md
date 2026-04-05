---
phase: 12-skills-registry-rename
plan: "01"
subsystem: skills
tags: [skills, clawhub, rename, cleanup, built-in-skills]

# Dependency graph
requires:
  - phase: 11-env-var-injection
    provides: install_builtin_skills() with create-if-absent installed.json
provides:
  - skills/skills/SKILL.md as built-in skills.sh manager skill
  - install_builtin_skills() installs skills/SKILL.md (not clawhub/SKILL.md)
  - cmd_up silently removes stale .claude/skills/clawhub/ from pre-v2.2 agents
affects: [future phases that reference built-in skill paths]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Stale dir cleanup: let _ = fs::remove_dir_all() before reinstall — best-effort, never fatal"

key-files:
  created:
    - skills/skills/SKILL.md
  modified:
    - crates/rightclaw/src/codegen/skills.rs
    - crates/rightclaw/src/init.rs
    - crates/rightclaw-cli/src/main.rs

key-decisions:
  - "ClawHub removed completely — skills.sh is sole registry, no fallback (v2.2 D-01)"
  - "Stale clawhub cleanup uses let _ = (error ignored) — only acceptable error-ignoring in codebase"

patterns-established:
  - "Stale dir cleanup: best-effort remove_dir_all with let _ = before reinstall"

requirements-completed: [SKILLS-01, SKILLS-02, SKILLS-03, SKILLS-04, SKILLS-05]

# Metrics
duration: 2min
completed: 2026-03-26
---

# Phase 12 Plan 01: Skills Registry Rename Summary

**Renamed clawhub to skills throughout codebase: moved skills/clawhub/ to skills/skills/, renamed SKILL_CLAWHUB constant to SKILL_SKILLS, updated install path to skills/SKILL.md, added stale clawhub dir cleanup in cmd_up**

## Performance

- **Duration:** ~2 min
- **Started:** 2026-03-26T09:07:08Z
- **Completed:** 2026-03-26T09:09:29Z
- **Tasks:** 2
- **Files modified:** 4

## Accomplishments
- Moved skills/clawhub/ to skills/skills/ via git mv (preserves history)
- Renamed SKILL_CLAWHUB constant to SKILL_SKILLS and updated include_str! path
- Updated built_in_skills slice to install skills/SKILL.md instead of clawhub/SKILL.md
- Updated all test assertions in skills.rs, init.rs, and main.rs to reference new path
- Added stale .claude/skills/clawhub/ cleanup line in cmd_up agent loop (SKILLS-05)
- Added two new tests for stale cleanup: remove-when-exists and idempotent-when-absent

## Task Commits

Each task was committed atomically:

1. **Task 1: Rename clawhub to skills** - `3f4bd01` (feat)
2. **Task 2: Add stale clawhub cleanup in cmd_up** - `944dc37` (feat)

**Plan metadata:** (pending final commit)

## Files Created/Modified
- `skills/skills/SKILL.md` - Built-in skills.sh manager skill (renamed from skills/clawhub/SKILL.md)
- `crates/rightclaw/src/codegen/skills.rs` - SKILL_SKILLS constant, updated install path, updated test assertions
- `crates/rightclaw/src/init.rs` - Updated println! and test assertion from clawhub to skills
- `crates/rightclaw-cli/src/main.rs` - Updated integration test + added stale cleanup line + two new cleanup tests

## Decisions Made
- ClawHub removed completely (not a fallback) — skills.sh is the sole registry
- Stale cleanup uses `let _ =` pattern — only acceptable error-ignoring in the codebase (removing a stale dir is best-effort, not a data operation)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
- `test_status_no_running_instance` integration test failed — this is the pre-existing failure documented in STATE.md and PROJECT.md, not caused by this plan's changes.

## Known Stubs

None - all paths are wired to real content. skills/skills/SKILL.md is the live built-in skill installed on every `rightclaw up`.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Phase 12 complete: zero `clawhub` functional references remain in Rust source
- skills/skills/ is the only built-in skills dir; stale cleanup prevents upgrade artifacts
- Ready for any follow-on skills.sh integration work

---
*Phase: 12-skills-registry-rename*
*Completed: 2026-03-26*
