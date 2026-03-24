---
phase: 06-sandbox-configuration
plan: 01
subsystem: codegen
tags: [sandbox, settings-json, serde, security, claude-code]

# Dependency graph
requires:
  - phase: 05-remove-openshell
    provides: "Clean AgentDef without OpenShell fields, state.rs replacing sandbox.rs"
provides:
  - "generate_settings() function producing per-agent .claude/settings.json as serde_json::Value"
  - "SandboxOverrides struct for user overrides in agent.yaml sandbox: section"
  - "Public re-export of generate_settings from codegen module"
affects: [06-02-PLAN, init.rs refactoring, cmd_up integration]

# Tech tracking
tech-stack:
  added: []
  patterns: ["codegen function: &AgentDef + config args -> miette::Result<serde_json::Value>", "merge-not-replace for user overrides via Vec::extend()"]

key-files:
  created:
    - crates/rightclaw/src/codegen/settings.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
  modified:
    - crates/rightclaw/src/agent/types.rs
    - crates/rightclaw/src/agent/mod.rs
    - crates/rightclaw/src/codegen/mod.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - crates/rightclaw/src/codegen/system_prompt_tests.rs

key-decisions:
  - "denyRead defaults include ~/.ssh, ~/.aws, ~/.gnupg (security-first stance)"
  - "excludedCommands omitted from JSON when empty (cleaner output)"
  - "Use ~/ prefix for denyRead paths (portable, resolves correctly until HOME override in v2.1)"

patterns-established:
  - "Settings codegen follows same &AgentDef -> Result pattern as shell_wrapper and process_compose"
  - "User-facing YAML structs always use deny_unknown_fields with serde(default) on all fields"

requirements-completed: [SBCF-01, SBCF-02, SBCF-03, SBCF-04, SBCF-05, SBCF-06]

# Metrics
duration: 4min
completed: 2026-03-24
---

# Phase 06 Plan 01: Settings Generation Summary

**generate_settings() producing per-agent sandbox JSON with filesystem/network restrictions, security denyRead defaults, and user override merging via SandboxOverrides**

## Performance

- **Duration:** 4 min
- **Started:** 2026-03-24T14:31:51Z
- **Completed:** 2026-03-24T14:36:40Z
- **Tasks:** 2
- **Files modified:** 8

## Accomplishments

- SandboxOverrides struct with deny_unknown_fields for strict agent.yaml validation (allow_write, allowed_domains, excluded_commands)
- generate_settings() produces complete .claude/settings.json with sandbox.enabled, filesystem restrictions, network allowedDomains, denyRead security defaults
- User overrides merge with (not replace) generated defaults via Vec::extend()
- no_sandbox=true only toggles sandbox.enabled=false, all other settings preserved
- Telegram plugin conditionally included based on mcp_config_path presence
- 13 new tests (4 for SandboxOverrides deserialization, 9 for generate_settings scenarios)

## Task Commits

Each task was committed atomically:

1. **Task 1: Add SandboxOverrides struct and update AgentConfig** - `b638bdf` (feat)
2. **Task 2 RED: Failing tests for generate_settings()** - `8ade907` (test)
3. **Task 2 GREEN: Implement generate_settings()** - `782b632` (feat)

_TDD tasks have test and implementation commits separately._

## Files Created/Modified

- `crates/rightclaw/src/codegen/settings.rs` - Core generate_settings() function with sandbox config generation
- `crates/rightclaw/src/codegen/settings_tests.rs` - 9 unit tests covering all settings generation scenarios
- `crates/rightclaw/src/agent/types.rs` - SandboxOverrides struct, sandbox field on AgentConfig
- `crates/rightclaw/src/agent/mod.rs` - Re-export SandboxOverrides
- `crates/rightclaw/src/codegen/mod.rs` - Register settings module, re-export generate_settings
- `crates/rightclaw/src/codegen/process_compose_tests.rs` - Add sandbox: None to test helpers
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` - Add sandbox: None to test helpers
- `crates/rightclaw/src/codegen/system_prompt_tests.rs` - Add sandbox: None to test helpers

## Decisions Made

- **denyRead security defaults:** Added `~/.ssh`, `~/.aws`, `~/.gnupg` to sandbox.filesystem.denyRead by default. Research recommended this as security best practice and CONTEXT.md left it to Claude's discretion.
- **Omit empty excludedCommands:** When no excluded_commands overrides exist, the key is omitted from JSON entirely for cleaner output (CC defaults to [] when omitted).
- **~/ prefix for denyRead paths:** Using `~/` prefix (home-relative) for denyRead entries rather than absolute paths. Portable and correct for v2.0; will need revisiting in v2.1 when HOME isolation is added.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Updated existing test helpers for new AgentConfig field**
- **Found during:** Task 1
- **Issue:** Adding `sandbox: Option<SandboxOverrides>` to AgentConfig caused compilation failures in process_compose_tests.rs, shell_wrapper_tests.rs, and system_prompt_tests.rs -- all construct AgentConfig literals missing the new field
- **Fix:** Added `sandbox: None` to all three test helper functions
- **Files modified:** process_compose_tests.rs, shell_wrapper_tests.rs, system_prompt_tests.rs
- **Verification:** All 95 existing tests pass
- **Committed in:** b638bdf (Task 1 commit)

**2. [Rule 3 - Blocking] Export SandboxOverrides from agent module**
- **Found during:** Task 2 RED phase
- **Issue:** settings_tests.rs imports `SandboxOverrides` via `crate::agent::SandboxOverrides` but the type was not re-exported from agent/mod.rs
- **Fix:** Added `SandboxOverrides` to the pub use statement in agent/mod.rs
- **Files modified:** crates/rightclaw/src/agent/mod.rs
- **Verification:** Tests compile and run
- **Committed in:** 8ade907 (Task 2 RED commit)

---

**Total deviations:** 2 auto-fixed (2 blocking)
**Impact on plan:** Both fixes were necessary for compilation. No scope creep.

## Issues Encountered

- Pre-existing flaky init tests: When running with default parallel threads, some init tests sporadically fail due to race conditions reading ~/.claude/settings.json from the real filesystem. All tests pass with --test-threads=1. This is a pre-existing issue, not caused by our changes.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- generate_settings() is ready to be wired into cmd_up() and init.rs (Plan 06-02)
- SandboxOverrides is ready for agent.yaml parsing (already integrated into AgentConfig)
- All 104 library tests pass (single-threaded), workspace builds clean

## Self-Check: PASSED

- All 5 key files verified present on disk
- All 3 commit hashes verified in git log

---
*Phase: 06-sandbox-configuration*
*Completed: 2026-03-24*
