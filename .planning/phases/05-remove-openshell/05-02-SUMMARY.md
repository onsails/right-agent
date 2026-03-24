---
phase: 05-remove-openshell
plan: 02
subsystem: testing
tags: [rust, tests, serde, backward-compat]

requires:
  - phase: 05-01
    provides: "Simplified RuntimeState/AgentState types, OpenShell-free shell wrapper, policy_path-free AgentDef"
provides:
  - "Full test coverage for OpenShell-free types and APIs"
  - "v1 backward compatibility test for state deserialization"
  - "Verified zero openshell/policy.yaml/sandbox references in production code and tests"
affects: [06-sandbox-config]

tech-stack:
  added: []
  patterns: ["v1 backward compat tests using serde's ignore-unknown-fields behavior"]

key-files:
  created: []
  modified:
    - "crates/rightclaw/src/runtime/state_tests.rs"

key-decisions:
  - "v1 state.json files with extra fields (sandbox_name, no_sandbox) still deserialize via serde's default ignore-unknown-fields"
  - "Pre-existing test failures in init.rs (empty ~/.claude/settings.json) documented as deferred, not fixed -- out of scope"

patterns-established:
  - "Backward compat test pattern: feed old-format JSON through new structs, assert serde ignores unknown fields"

requirements-completed: [SBMG-01, SBMG-02, SBMG-03, SBMG-04, SBMG-05]

duration: 2min
completed: 2026-03-24
---

# Phase 05 Plan 02: Test Updates Summary

**v1 backward compatibility test added, all 48 relevant tests pass with zero openshell/sandbox references in codebase**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-24T13:32:55Z
- **Completed:** 2026-03-24T13:35:46Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments
- Added `read_state_ignores_extra_fields_from_v1` test proving old v1.0 state.json files (with sandbox_name, no_sandbox) still deserialize under new simplified RuntimeState/AgentState structs
- Verified all 48 tests across state, deps, doctor, discovery, and shell_wrapper modules pass
- Confirmed zero openshell/policy.yaml/sandbox_name references remain in production code or test struct construction
- Confirmed `no_sandbox` only exists as kept CLI flag for Phase 6 and in v1 compat test data

## Task Commits

Each task was committed atomically:

1. **Task 1: Create state_tests.rs and update shell_wrapper_tests.rs and discovery_tests.rs** - `d1393c7` (test)
2. **Task 2: Update inline tests in doctor.rs, init.rs, deps.rs and run full test suite** - No changes needed (all updates applied in Plan 05-01)

## Files Created/Modified
- `crates/rightclaw/src/runtime/state_tests.rs` - Added v1 backward compatibility test

## Decisions Made
- v1 state.json backward compatibility verified via serde's default `deny_unknown_fields` NOT being set on RuntimeState/AgentState, so unknown fields are silently ignored
- Pre-existing test failures (init tests failing due to empty `~/.claude/settings.json`, CLI integration test error message mismatch) logged as deferred items, not fixed

## Deviations from Plan

### Observation: Most Task 1 and all Task 2 changes already applied

Plan 05-01 already applied most of the changes specified in this plan (shell_wrapper_tests.rs cleanup, discovery_tests.rs cleanup, doctor.rs test updates, init.rs test updates, deps.rs test updates). The only missing piece was the `read_state_ignores_extra_fields_from_v1` test in state_tests.rs.

**Impact on plan:** Reduced to single file change. All acceptance criteria met regardless.

## Issues Encountered

Pre-existing test failures discovered (not caused by our changes):
1. `init::tests::init_with_telegram_creates_settings_json` -- `pre_trust_directory()` fails parsing empty `~/.claude/settings.json`
2. `init::tests::init_errors_if_already_initialized` -- same root cause
3. `cli_integration::test_status_no_running_instance` -- error message format mismatch

These are documented in `deferred-items.md` and not addressed in this plan.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Phase 05 (remove-openshell) is fully complete -- zero OpenShell references remain
- Ready for Phase 06 (sandbox-config): add CC native sandbox configuration with per-agent settings.json
- The kept `--no-sandbox` CLI flag is ready for Phase 6 to repurpose for CC native sandbox bypass

---
*Phase: 05-remove-openshell*
*Completed: 2026-03-24*
