---
phase: 16-db-foundation
plan: "02"
subsystem: agent
tags: [rust, agentdef, dead-code, cleanup, sec-02, sec-03]

requires:
  - phase: 16-01
    provides: memory module scaffolding (AgentDef still had memory_path before this plan)

provides:
  - AgentDef struct without memory_path field
  - No code path scans for MEMORY.md in discovery or prompt generation
  - Default start_prompt is "You are starting." with no MEMORY.md reference

affects: [16-03, 17-memory-skill]

tech-stack:
  added: []
  patterns:
    - "SEC-02 enforced by architecture: memory module disconnected from agent discovery at struct level"

key-files:
  created: []
  modified:
    - crates/rightclaw/src/agent/types.rs
    - crates/rightclaw/src/agent/discovery.rs
    - crates/rightclaw/src/agent/discovery_tests.rs
    - crates/rightclaw/src/init.rs
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw/src/codegen/system_prompt_tests.rs
    - crates/rightclaw/src/codegen/telegram.rs
    - crates/rightclaw/src/codegen/claude_json.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs

key-decisions:
  - "SEC-02 enforced by removing the struct field entirely — not by policy gate or runtime check"
  - "Task 2 (system_prompt default) was pre-completed by plan 16-01 (commit e11f9ff)"

patterns-established:
  - "Dead code removal: remove field from struct, fix all struct literal sites, remove associated test assertions"

requirements-completed:
  - SEC-02
  - SEC-03

duration: 5min
completed: "2026-03-26"
---

# Phase 16 Plan 02: Dead Code Removal — memory_path and MEMORY.md References Summary

**Removed memory_path from AgentDef and all struct literal sites (11 files), and confirmed default start_prompt is already "You are starting." — SEC-02 enforced architecturally**

## Performance

- **Duration:** ~5 min
- **Started:** 2026-03-26T21:20:00Z
- **Completed:** 2026-03-26T21:23:14Z
- **Tasks:** 2 (Task 2 was pre-completed by plan 16-01)
- **Files modified:** 11

## Accomplishments

- Removed `memory_path: Option<PathBuf>` field from `AgentDef` struct and all 11 struct literal sites across the codebase
- Removed `optional_file(&path, "MEMORY.md")` scan from `discovery.rs`
- Removed MEMORY.md test setup and `assert!(a.memory_path.is_some())` from `discover_detects_optional_files`
- Confirmed default `start_prompt` is `"You are starting."` (pre-applied by plan 16-01)
- All 162 non-memory tests pass; cargo build --workspace exits 0

## Task Commits

1. **Task 1: Remove memory_path from AgentDef and all struct literal sites** - `93641d4` (feat)
2. **Task 2: Update default start_prompt and its test** - `e11f9ff` (pre-completed by 16-01, no new commit needed)

## Files Created/Modified

- `crates/rightclaw/src/agent/types.rs` — Removed memory_path field + doc comment from AgentDef
- `crates/rightclaw/src/agent/discovery.rs` — Removed optional_file(&path, "MEMORY.md") struct initializer field
- `crates/rightclaw/src/agent/discovery_tests.rs` — Removed MEMORY.md file creation + memory_path assertion
- `crates/rightclaw/src/init.rs` — Removed memory_path: None from two AgentDef literal sites
- `crates/rightclaw-cli/src/main.rs` — Removed memory_path: None from AgentDef literal
- `crates/rightclaw/src/codegen/system_prompt_tests.rs` — Removed memory_path: None from make_agent_at
- `crates/rightclaw/src/codegen/telegram.rs` — Removed memory_path: None from make_agent
- `crates/rightclaw/src/codegen/claude_json.rs` — Removed memory_path: None from two AgentDef literals
- `crates/rightclaw/src/codegen/settings_tests.rs` — Removed memory_path: None from make_test_agent
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` — Removed memory_path: None from three AgentDef literals
- `crates/rightclaw/src/codegen/process_compose_tests.rs` — Removed memory_path: None from two AgentDef literals

## Decisions Made

- SEC-02 enforced by architecture: the `memory_path` field is gone from `AgentDef`, so the memory module has no connection to MEMORY.md at the type level — no runtime guard needed
- Task 2 (system_prompt default change) was already applied in plan 16-01 commit `e11f9ff`; no duplicate commit made

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Additional struct literal sites in codegen files not listed in plan interfaces**
- **Found during:** Task 1 (after initial edits, rg check revealed 9 more memory_path references)
- **Issue:** Plan listed 5 struct literal sites; actual codebase had 9 more across telegram.rs, claude_json.rs, settings_tests.rs, shell_wrapper_tests.rs, process_compose_tests.rs
- **Fix:** Removed memory_path: None from all remaining sites via replace_all edits
- **Files modified:** 5 additional codegen files
- **Verification:** `rg "memory_path" crates/` returns zero matches; `cargo build --workspace` exits 0
- **Committed in:** 93641d4 (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (Rule 3 - Blocking: extra struct literal sites)
**Impact on plan:** Required for compile correctness. No scope creep.

## Issues Encountered

None — build passed cleanly after finding and fixing all struct literal sites.

## Next Phase Readiness

- `AgentDef` is clean — no MEMORY.md references anywhere in agent discovery or codegen
- Plan 16-03 can safely add `memory_db_path: Option<PathBuf>` to `AgentDef` and wire `open_db()` without MEMORY.md confusion
- Pre-existing memory stub failures (9 tests, `not yet implemented`) are from plan 16-01 and unrelated to this plan

---
*Phase: 16-db-foundation*
*Completed: 2026-03-26*
