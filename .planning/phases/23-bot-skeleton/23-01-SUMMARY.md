---
phase: 23-bot-skeleton
plan: 01
subsystem: api
tags: [serde, yaml, agent-config, telegram]

# Dependency graph
requires: []
provides:
  - AgentConfig.allowed_chat_ids field (Vec<i64>, serde default = empty vec)
  - Backward-compatible YAML deserialization for agent.yaml without the field
affects: [23-02, 23-bot-skeleton]

# Tech tracking
tech-stack:
  added: []
  patterns: []

key-files:
  created: []
  modified:
    - crates/rightclaw/src/agent/types.rs
    - crates/rightclaw/src/codegen/telegram.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - crates/rightclaw/src/codegen/system_prompt_tests.rs
    - crates/rightclaw/src/codegen/claude_json.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - crates/rightclaw/src/init.rs

key-decisions:
  - "allowed_chat_ids: Vec<i64> with #[serde(default)] — empty vec is secure default (blocks all messages)"

patterns-established:
  - "New optional list fields on AgentConfig use #[serde(default)] not Option<Vec<_>>"

requirements-completed: [BOT-05]

# Metrics
duration: 12min
completed: 2026-03-31
---

# Phase 23 Plan 01: bot-skeleton Summary

**`allowed_chat_ids: Vec<i64>` added to `AgentConfig` with backward-compatible `#[serde(default)]`, gating downstream bot chat filtering (Plan 02)**

## Performance

- **Duration:** ~12 min
- **Started:** 2026-03-31T21:10:00Z
- **Completed:** 2026-03-31T21:22:00Z
- **Tasks:** 1
- **Files modified:** 8

## Accomplishments
- New `allowed_chat_ids: Vec<i64>` field on `AgentConfig` with `#[serde(default)]`
- 4 new tests: list deserialization, default-to-empty, absent-field no-rejection, negative i64 values
- All 244 crate tests pass (10 pre-existing `agent_config_*` tests confirmed passing)

## Task Commits

1. **Task 1: Add allowed_chat_ids field + tests** - `9ea927f` (feat)

## Files Created/Modified
- `crates/rightclaw/src/agent/types.rs` - Added `allowed_chat_ids` field + 4 tests
- `crates/rightclaw/src/codegen/telegram.rs` - Updated `base_config()` struct literal
- `crates/rightclaw/src/codegen/settings_tests.rs` - Updated 3 struct literals
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` - Updated 2 struct literals
- `crates/rightclaw/src/codegen/system_prompt_tests.rs` - Updated 1 struct literal
- `crates/rightclaw/src/codegen/claude_json.rs` - Updated 1 struct literal
- `crates/rightclaw/src/codegen/process_compose_tests.rs` - Updated 1 struct literal
- `crates/rightclaw/src/init.rs` - Updated 1 struct literal

## Decisions Made
- Empty `Vec<i64>` is the secure default — bot must reject all messages unless explicitly allowed. Field uses `#[serde(default)]` not `Option<Vec<i64>>` because an absent field is semantically equivalent to "block all", not "unknown".

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Updated struct literal initializers across codebase**
- **Found during:** Task 1 (adding field)
- **Issue:** Adding a non-default field to `AgentConfig` (which uses exhaustive struct literals in tests) caused 10 compile errors across 7 files.
- **Fix:** Added `allowed_chat_ids: vec![]` to all struct literal initializers. All used `..spread` or explicit initializers — no implicit Default trait.
- **Files modified:** `codegen/telegram.rs`, `codegen/settings_tests.rs`, `codegen/shell_wrapper_tests.rs`, `codegen/system_prompt_tests.rs`, `codegen/claude_json.rs`, `codegen/process_compose_tests.rs`, `init.rs`
- **Verification:** `cargo test -p rightclaw` — 244 tests pass
- **Committed in:** `9ea927f` (same task commit)

---

**Total deviations:** 1 auto-fixed (Rule 1 - compile error from exhaustive struct literals)
**Impact on plan:** Required update, no scope creep. All changes are mechanical initializer additions.

## Issues Encountered
None.

## Next Phase Readiness
- `AgentConfig.allowed_chat_ids` available for Plan 02 (bot crate) to read at startup
- Downstream `crates/bot/src/telegram/filter.rs` can reference this field immediately

## Known Stubs
None.

## Self-Check: PASSED
- `crates/rightclaw/src/agent/types.rs` — FOUND
- `.planning/phases/23-bot-skeleton/23-01-SUMMARY.md` — FOUND
- Commit `9ea927f` — FOUND

---
*Phase: 23-bot-skeleton*
*Completed: 2026-03-31*
