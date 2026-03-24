---
phase: 09-agent-environment-setup
plan: 01
subsystem: codegen
tags: [telegram, skills, codegen, agent-config, serde]

# Dependency graph
requires:
  - phase: 08-home-override
    provides: AgentDef struct with all fields, AgentConfig with sandbox
provides:
  - AgentConfig with telegram_token_file, telegram_token, telegram_user_id fields
  - codegen::generate_telegram_channel_config for per-agent Telegram setup
  - codegen::install_builtin_skills extracted from init.rs for reuse in cmd_up
affects:
  - 09-02 (wires these into cmd_up)

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Token file precedence: telegram_token_file > telegram_token > None (resolved relative to agent.path)"
    - "Idempotent overwrite for .env and access.json; create-if-absent for .mcp.json"
    - "Shared codegen functions testable in isolation before cmd_up integration"

key-files:
  created:
    - crates/rightclaw/src/codegen/telegram.rs
    - crates/rightclaw/src/codegen/skills.rs
  modified:
    - crates/rightclaw/src/agent/types.rs
    - crates/rightclaw/src/codegen/mod.rs
    - crates/rightclaw/src/init.rs
    - crates/rightclaw/src/codegen/settings.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - crates/rightclaw/src/codegen/system_prompt_tests.rs
    - crates/rightclaw/src/codegen/claude_json.rs

key-decisions:
  - "telegram_token_file path resolved relative to agent.path, not cwd"
  - ".mcp.json is create-if-absent to preserve user customizations"
  - "init.rs Telegram write block kept as-is (different call convention, per plan revision)"
  - "install_builtin_skills extracted from init.rs inline loop to enable reuse in cmd_up"

patterns-established:
  - "Token resolution helper: resolve_telegram_token() isolates file-vs-inline precedence logic"
  - "Codegen functions take &AgentDef, return miette::Result<()>"

requirements-completed:
  - AENV-02
  - AENV-03

# Metrics
duration: 5min
completed: 2026-03-24
---

# Phase 9 Plan 01: Agent Environment Setup — Type Definitions and Codegen Functions Summary

**Extended AgentConfig with three Telegram Option fields and extracted two codegen functions (generate_telegram_channel_config, install_builtin_skills) with 14 new tests covering all behaviors**

## Performance

- **Duration:** 5 min
- **Started:** 2026-03-24T23:26:01Z
- **Completed:** 2026-03-24T23:31:06Z
- **Tasks:** 2
- **Files modified:** 9 (3 new, 6 updated)

## Accomplishments

- AgentConfig gains `telegram_token_file`, `telegram_token`, `telegram_user_id` with `#[serde(default)]` — deny_unknown_fields preserved
- `codegen::generate_telegram_channel_config` handles token file vs inline precedence, writes .env and conditional access.json, creates .mcp.json only if absent
- `codegen::install_builtin_skills` extracted from init.rs inline loop, now reusable for cmd_up in plan 02

## Task Commits

1. **Task 1: Extend AgentConfig with Telegram fields** - `35b0e7b` (feat)
2. **Task 2: Create codegen/telegram.rs, codegen/skills.rs, update mod.rs** - `2754a43` (feat)
3. **Fix: clippy warnings** - `019a4ed` (fix)

## Files Created/Modified

- `crates/rightclaw/src/codegen/telegram.rs` - generate_telegram_channel_config + 9 tests
- `crates/rightclaw/src/codegen/skills.rs` - install_builtin_skills (extracted from init.rs) + 5 tests
- `crates/rightclaw/src/agent/types.rs` - three new telegram Option fields + 5 new tests
- `crates/rightclaw/src/codegen/mod.rs` - re-exports for install_builtin_skills and generate_telegram_channel_config
- `crates/rightclaw/src/init.rs` - inline skills loop replaced by install_builtin_skills call
- `crates/rightclaw/src/codegen/settings.rs` - clippy collapsible-if fix (pre-existing lint)
- `crates/rightclaw/src/codegen/{claude_json,process_compose,settings,shell_wrapper,system_prompt}_tests.rs` - AgentConfig struct initializers updated with new fields

## Decisions Made

- Token file precedence: `telegram_token_file` beats `telegram_token` when both set — avoids accidentally using inline token when file is present
- `.mcp.json` is create-if-absent: user may have existing custom `.mcp.json`; telegram marker doesn't need to overwrite it
- init.rs Telegram write block NOT refactored to use generate_telegram_channel_config — different call convention (token passed directly vs via AgentConfig), refactoring would break existing init tests with no benefit in plan 01 scope

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed struct initializer compile errors across existing test files**
- **Found during:** Task 1 (GREEN phase)
- **Issue:** Adding fields to AgentConfig without Default trait caused all explicit struct initializers in test files to fail compilation
- **Fix:** Added `telegram_token_file: None, telegram_token: None, telegram_user_id: None` to 5 test helper functions
- **Files modified:** claude_json.rs, process_compose_tests.rs, settings_tests.rs, shell_wrapper_tests.rs, system_prompt_tests.rs
- **Verification:** cargo test -p rightclaw --lib passes 147 tests
- **Committed in:** 35b0e7b (Task 1 commit)

**2. [Rule 1 - Bug] Fixed clippy: unused import Path in telegram.rs, collapsible-if in settings.rs**
- **Found during:** Post-task-2 clippy run
- **Issue:** telegram.rs had top-level `use std::path::Path` used only in test module; settings.rs had nested if-let that clippy::collapsible_if flagged (pre-existing lint exposed by new module adding to compile unit)
- **Fix:** Moved Path import into `#[cfg(test)]` mod; collapsed nested if-let in settings.rs
- **Files modified:** codegen/telegram.rs, codegen/settings.rs
- **Verification:** cargo clippy --workspace -- -D warnings exits 0
- **Committed in:** 019a4ed (fix commit)

---

**Total deviations:** 2 auto-fixed (2x Rule 1 bugs)
**Impact on plan:** Both fixes necessary for compilation and clippy compliance. No scope creep.

## Issues Encountered

None beyond the auto-fixed struct initializer issue above.

## Next Phase Readiness

- Plan 02 can now call `codegen::generate_telegram_channel_config(agent)` and `codegen::install_builtin_skills(&agent.path)` from cmd_up
- All contracts compile and are tested in isolation
- 147 lib tests pass; pre-existing `test_status_no_running_instance` integration test failure is unrelated

---
*Phase: 09-agent-environment-setup*
*Completed: 2026-03-24*

## Self-Check: PASSED

- FOUND: crates/rightclaw/src/codegen/telegram.rs
- FOUND: crates/rightclaw/src/codegen/skills.rs
- FOUND: .planning/phases/09-agent-environment-setup/09-01-SUMMARY.md
- FOUND: commit 35b0e7b (feat: extend AgentConfig)
- FOUND: commit 2754a43 (feat: add telegram.rs and skills.rs)
- FOUND: commit 019a4ed (fix: clippy warnings)
