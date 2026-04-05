---
phase: 06-sandbox-configuration
plan: 02
subsystem: sandbox
tags: [settings-json, codegen, sandbox, claude-code]

requires:
  - phase: 06-01
    provides: "generate_settings() codegen function and SandboxOverrides struct"
provides:
  - "cmd_up() generates .claude/settings.json per agent with sandbox config"
  - "init.rs delegates settings generation to shared codegen::generate_settings()"
  - "--no-sandbox flag wired through from CLI to generate_settings()"
affects: [07-tooling-updates]

tech-stack:
  added: [serde_json (direct dep in rightclaw-cli)]
  patterns: [synthetic AgentDef construction for init path]

key-files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw-cli/Cargo.toml
    - crates/rightclaw/src/init.rs

key-decisions:
  - "no_sandbox hardcoded to false for init (sandbox always enabled for fresh agents)"
  - "Synthetic AgentDef constructed in init.rs to reuse codegen path"

patterns-established:
  - "Settings generation via codegen::generate_settings() for both init and up paths"

requirements-completed: [SBCF-01, SBCF-05]

duration: 2min
completed: 2026-03-24
---

# Phase 06 Plan 02: Settings Integration Summary

**Wired generate_settings() into cmd_up() per-agent loop and refactored init.rs to delegate to shared codegen -- single source of truth for .claude/settings.json**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-24T14:39:56Z
- **Completed:** 2026-03-24T14:42:24Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- cmd_up() now generates .claude/settings.json with sandbox config for every discovered agent
- init.rs no longer has inline settings JSON -- delegates to codegen::generate_settings()
- --no-sandbox CLI flag properly wired through from cmd_up() to generate_settings()
- 105 total tests pass (1 new test added)

## Task Commits

Each task was committed atomically:

1. **Task 1: Hook generate_settings() into cmd_up() agent loop** - `751dd68` (feat)
2. **Task 2: Refactor init.rs to delegate settings generation to codegen** - `e6dd741` (refactor)

## Files Created/Modified
- `crates/rightclaw-cli/src/main.rs` - Added generate_settings() call in agent loop, removed no_sandbox suppression
- `crates/rightclaw-cli/Cargo.toml` - Added serde_json as direct dependency
- `crates/rightclaw/src/init.rs` - Replaced inline settings JSON with codegen::generate_settings() call, added sandbox config test

## Decisions Made
- no_sandbox hardcoded to `false` for `init` path (fresh agents always get sandbox enabled per D-01)
- Synthetic AgentDef constructed in init.rs to match codegen::generate_settings() signature -- mcp_config_path controlled by telegram_token.is_some()

## Deviations from Plan

None -- plan executed exactly as written.

## Issues Encountered
- Pre-existing test flakiness in init tests when run with multiple threads (parallel writes to ~/.claude.json and ~/.claude/settings.json from pre_trust_directory()). Not caused by this plan's changes. Tests pass reliably with --test-threads=1.

## User Setup Required

None -- no external service configuration required.

## Next Phase Readiness
- Phase 06 (sandbox-configuration) complete -- both plans delivered
- Ready for Phase 07 (tooling-updates): doctor checks, install.sh, shell wrapper updates

---
*Phase: 06-sandbox-configuration*
*Completed: 2026-03-24*
