---
phase: 05-remove-openshell
plan: 01
subsystem: runtime
tags: [openshell, sandbox, refactor, cleanup, process-compose]

# Dependency graph
requires: []
provides:
  - "Simplified RuntimeState and AgentState structs without sandbox fields"
  - "Single-path shell wrapper template (direct claude invocation)"
  - "Agent discovery without policy.yaml requirement"
  - "verify_dependencies() without openshell check"
  - "doctor without openshell binary check or policy.yaml validation"
  - "init without policy.yaml file creation"
affects: [06-add-sandbox-config, 07-update-tooling]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Direct claude invocation via shell wrapper (no sandbox intermediary)"

key-files:
  created:
    - crates/rightclaw/src/runtime/state.rs
    - crates/rightclaw/src/runtime/state_tests.rs
  modified:
    - crates/rightclaw/src/runtime/mod.rs
    - crates/rightclaw/src/agent/types.rs
    - crates/rightclaw/src/agent/discovery.rs
    - crates/rightclaw/src/codegen/shell_wrapper.rs
    - templates/agent-wrapper.sh.j2
    - crates/rightclaw/src/runtime/deps.rs
    - crates/rightclaw/src/doctor.rs
    - crates/rightclaw/src/init.rs
    - crates/rightclaw-cli/src/main.rs

key-decisions:
  - "Kept --no-sandbox CLI flag as no-op for Phase 6 reuse"
  - "Negative test assertions for openshell kept in test code (they validate the removal)"
  - "state.rs replaces sandbox.rs with simplified structs (no rename, clean replacement)"

patterns-established:
  - "RuntimeState/AgentState are sandbox-agnostic -- future sandbox config adds new fields"

requirements-completed: [SBMG-01, SBMG-02, SBMG-03, SBMG-04, SBMG-05]

# Metrics
duration: 9min
completed: 2026-03-24
---

# Phase 05 Plan 01: Remove OpenShell Production Code Summary

**Stripped all OpenShell code paths -- sandbox.rs replaced by state.rs, policy.yaml removed from init/discovery/doctor, shell wrapper uses single direct-claude path**

## Performance

- **Duration:** 9 min
- **Started:** 2026-03-24T13:20:24Z
- **Completed:** 2026-03-24T13:29:28Z
- **Tasks:** 2
- **Files modified:** 20 (including test files)

## Accomplishments
- Deleted OpenShell policy template files (policy.yaml, policy-telegram.yaml)
- Replaced sandbox.rs with state.rs -- RuntimeState/AgentState simplified (no sandbox_name, no no_sandbox)
- Removed all openshell references from production code (12 source files updated)
- Shell wrapper template now has a single direct-claude code path (no openshell conditional)
- All 89 unit tests pass (1 pre-existing failure in init unrelated to changes)

## Task Commits

Each task was committed atomically:

1. **Task 1: Delete template files, restructure sandbox.rs to state.rs, simplify core structs** - `767d04c` (refactor)
2. **Task 2: Update all production code to compile against simplified types** - `aa56688` (feat)

## Files Created/Modified

### Created
- `crates/rightclaw/src/runtime/state.rs` - Simplified RuntimeState/AgentState structs + read/write functions
- `crates/rightclaw/src/runtime/state_tests.rs` - Tests for state serialization roundtrip

### Modified
- `crates/rightclaw/src/runtime/mod.rs` - Replaced sandbox module with state module
- `crates/rightclaw/src/agent/types.rs` - Removed policy_path field from AgentDef
- `crates/rightclaw/src/agent/discovery.rs` - Removed policy.yaml existence check
- `crates/rightclaw/src/agent/discovery_tests.rs` - Updated tests for no policy.yaml requirement
- `crates/rightclaw/src/codegen/shell_wrapper.rs` - Removed no_sandbox param and policy_path from context
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` - Rewrote tests for single code path
- `crates/rightclaw/src/codegen/process_compose_tests.rs` - Removed policy_path from test fixtures
- `crates/rightclaw/src/codegen/system_prompt_tests.rs` - Removed policy_path from test fixtures
- `templates/agent-wrapper.sh.j2` - Single direct-claude path, no openshell conditional
- `crates/rightclaw/src/runtime/deps.rs` - Removed no_sandbox param and openshell check
- `crates/rightclaw/src/doctor.rs` - Removed openshell binary check and policy.yaml agent validation
- `crates/rightclaw/src/init.rs` - Removed DEFAULT_POLICY constants and policy.yaml file creation
- `crates/rightclaw-cli/src/main.rs` - Simplified RuntimeState construction, removed destroy_sandboxes from cmd_down
- `crates/rightclaw-cli/tests/cli_integration.rs` - Updated assertions for no policy.yaml

### Deleted
- `templates/right/policy.yaml` - OpenShell policy template
- `templates/right/policy-telegram.yaml` - OpenShell Telegram policy template
- `crates/rightclaw/src/runtime/sandbox.rs` - Old sandbox module
- `crates/rightclaw/src/runtime/sandbox_tests.rs` - Old sandbox tests

## Decisions Made
- Kept `--no-sandbox` CLI flag in cmd_up as a no-op (`let _ = no_sandbox;`) -- Phase 6 will repurpose it for CC native sandbox config
- Negative test assertions that verify openshell is NOT present are intentionally kept in test files (they validate the removal was complete)
- `_state` in cmd_down keeps the read_state call to validate running instance, but result is no longer used for sandbox cleanup

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed unused variable warning in cmd_down**
- **Found during:** Task 2 (main.rs update)
- **Issue:** After removing destroy_sandboxes, the `state` variable from read_state was unused, producing a compiler warning
- **Fix:** Prefixed with underscore (`_state`) to suppress warning while keeping the read_state call (validates running instance)
- **Files modified:** crates/rightclaw-cli/src/main.rs
- **Committed in:** aa56688

**2. [Rule 3 - Blocking] Updated test files referencing removed policy_path field**
- **Found during:** Task 2 (compilation check)
- **Issue:** Test helper functions in shell_wrapper_tests.rs, process_compose_tests.rs, system_prompt_tests.rs, discovery_tests.rs, and cli_integration.rs still referenced the removed `policy_path` field
- **Fix:** Removed policy_path from all test fixtures and updated assertions
- **Files modified:** 5 test files
- **Committed in:** aa56688

---

**Total deviations:** 2 auto-fixed (1 bug, 1 blocking)
**Impact on plan:** Both fixes necessary for clean compilation. No scope creep.

## Issues Encountered
- Pre-existing test failure in `init::tests::init_with_telegram_writes_token_env_file` due to empty `~/.claude/settings.json` on host -- not caused by our changes, not in scope

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- All production code compiles cleanly with zero openshell references
- Plan 05-02 (test updates) can proceed -- state_tests.rs already created with updated tests
- Phase 06 (add CC native sandbox config) has clean foundation -- structs are sandbox-agnostic

---
*Phase: 05-remove-openshell*
*Completed: 2026-03-24*
