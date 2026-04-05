---
phase: 14-rightskills-rename
plan: 01
subsystem: skills
tags: [rename, skill-manager, rightskills, codegen]

requires:
  - phase: 13-policy-gate-rework
    provides: skills/skills/SKILL.md policy-gate rewrite, SKILL_SKILLS constant in codegen

provides:
  - skills/rightskills/SKILL.md with name: rightskills and /rightskills heading
  - SKILL_RIGHTSKILLS constant with updated include_str! path
  - install_builtin_skills() installs to .claude/skills/rightskills/ (not skills/)
  - All test assertions updated to rightskills path

affects: [phase-15, skill-installation, rightclaw-up, rightclaw-init]

tech-stack:
  added: []
  patterns:
    - "git mv preserves history for source directory renames"
    - "TDD: update test assertions first (RED), then production code (GREEN)"

key-files:
  created: []
  modified:
    - skills/rightskills/SKILL.md
    - crates/rightclaw/src/codegen/skills.rs
    - crates/rightclaw/src/init.rs
    - crates/rightclaw-cli/src/main.rs

key-decisions:
  - "Source dir: skills/skills -> skills/rightskills via git mv (history preserved)"
  - "Constant: SKILL_SKILLS -> SKILL_RIGHTSKILLS; include_str! path updated accordingly"
  - "Install path tuple: skills/SKILL.md -> rightskills/SKILL.md (agent-side dir changes)"
  - "Slash command: /skills -> /rightskills in SKILL.md name field and H1 heading"
  - "skills.sh domain references and npx skills CLI commands left unchanged — not slash commands"
  - "No stale .claude/skills/skills/ cleanup in cmd_up — project not in production (D-10)"

patterns-established: []

requirements-completed: [RS-01, RS-02, RS-03, RS-04]

duration: 2min
completed: 2026-03-26
---

# Phase 14 Plan 01: rightskills Rename Summary

**Renamed /skills built-in skill to /rightskills across source dir, SKILL.md frontmatter/heading, Rust constant, include path, install tuple, and all test assertions — workspace builds clean, 19/20 tests pass (pre-existing failure excluded)**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-26T11:28:33Z
- **Completed:** 2026-03-26T11:30:39Z
- **Tasks:** 2
- **Files modified:** 4

## Accomplishments
- Renamed `skills/skills/` to `skills/rightskills/` via `git mv` (history preserved)
- Updated SKILL.md: `name: rightskills`, heading `# /rightskills -- Agent Skills Manager (skills.sh)`
- Renamed `SKILL_SKILLS` constant to `SKILL_RIGHTSKILLS`, updated `include_str!` path, install path tuple, and doc comment in `skills.rs`
- Updated `println!` and test assertion in `init.rs` to reference `rightskills/SKILL.md`
- Fixed stale test assertion in `main.rs` (`skills_install_creates_builtin_skill_dirs`) — auto-fix Rule 1

## Task Commits

1. **Task 1: Rename skill directory and update SKILL.md content** - `bded971` (feat)
2. **Task 2: Update Rust constant, include path, install path, and test assertions** - `2529efc` (feat)

## Files Created/Modified
- `skills/rightskills/SKILL.md` - Renamed from skills/skills/; frontmatter name + H1 heading updated
- `crates/rightclaw/src/codegen/skills.rs` - SKILL_RIGHTSKILLS constant, updated include_str!, install tuple, doc comment, test assertions
- `crates/rightclaw/src/init.rs` - println and test assertion updated to rightskills path
- `crates/rightclaw-cli/src/main.rs` - Auto-fixed stale test assertion in skills_install_creates_builtin_skill_dirs

## Decisions Made
- `skills.sh` domain and `npx skills` CLI references in SKILL.md left unchanged — these are domain names / npm package names, not slash command invocations
- No stale `.claude/skills/skills/` cleanup added to `cmd_up` — project not in production (D-10)
- TDD order: updated test assertions first (RED = compile error on include_str!), then production code (GREEN)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed stale test assertion in rightclaw-cli/src/main.rs**
- **Found during:** Task 2 (running cargo test --workspace after production code update)
- **Issue:** `tests::skills_install_creates_builtin_skill_dirs` in `main.rs` still asserted `.join("skills").join("SKILL.md")` — missed in plan's file list
- **Fix:** Updated path to `.join("rightskills").join("SKILL.md")` and message to `"rightskills/SKILL.md should be installed"`
- **Files modified:** `crates/rightclaw-cli/src/main.rs`
- **Verification:** `cargo test --workspace` passes (19/20, pre-existing failure excluded)
- **Committed in:** `2529efc` (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (Rule 1 - bug)
**Impact on plan:** Auto-fix necessary for correctness — plan's file list missed a stale reference in main.rs. No scope creep.

## Issues Encountered
- Pre-existing `test_status_no_running_instance` integration test failure (HTTP error instead of "No running instance" message) — documented in project memory, unrelated to this plan.

## Known Stubs
None.

## Next Phase Readiness
- `rightskills` rename complete end-to-end; agents will be installed to `.claude/skills/rightskills/` on next `rightclaw up` or `rightclaw init`
- Phase 14 complete — no further plans in this phase

---
*Phase: 14-rightskills-rename*
*Completed: 2026-03-26*

## Self-Check: PASSED

- FOUND: skills/rightskills/SKILL.md
- FOUND: crates/rightclaw/src/codegen/skills.rs
- FOUND: crates/rightclaw/src/init.rs
- FOUND: crates/rightclaw-cli/src/main.rs
- FOUND: .planning/phases/14-rightskills-rename/14-01-SUMMARY.md
- FOUND commit: bded971 (Task 1)
- FOUND commit: 2529efc (Task 2)
