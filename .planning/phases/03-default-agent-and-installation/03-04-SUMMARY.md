---
phase: 03-default-agent-and-installation
plan: 04
subsystem: cli
tags: [clap, doctor, telegram, channels, shell-wrapper, minijinja]

# Dependency graph
requires:
  - phase: 03-default-agent-and-installation/03-03
    provides: "doctor.rs run_doctor(), init.rs with telegram_token support, validate_telegram_token, prompt_telegram_token"
provides:
  - "rightclaw doctor subcommand wired to CLI"
  - "rightclaw init --telegram-token flag with interactive fallback"
  - "Shell wrapper conditional --channels flag for Telegram agents"
  - "Integration tests for doctor and init --telegram-token"
affects: [04-clawhub-and-cronsync]

# Tech tracking
tech-stack:
  added: []
  patterns: ["conditional template rendering for --channels flag based on mcp_config_path"]

key-files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
    - templates/agent-wrapper.sh.j2
    - crates/rightclaw/src/codegen/shell_wrapper.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - crates/rightclaw-cli/tests/cli_integration.rs

key-decisions:
  - "mcp_config_path.is_some() used as Telegram signal for --channels (v1 simplification)"
  - "cmd_doctor reuses DoctorCheck Display impl for output formatting"

patterns-established:
  - "Conditional template variables: generate_wrapper passes Option<&str> for channels"

requirements-completed: [DFLT-01, INST-02, INST-03, CHAN-01, CHAN-02]

# Metrics
duration: 2min
completed: 2026-03-22
---

# Phase 3 Plan 04: CLI Wiring and Shell Wrapper Channels Summary

**Doctor subcommand, Init --telegram-token flag, and conditional --channels in shell wrappers wired to CLI with 10 new tests**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-22T18:43:30Z
- **Completed:** 2026-03-22T18:46:23Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments
- `rightclaw doctor` subcommand wired to CLI, calls `run_doctor()` and displays pass/fail/warn per check with summary
- `rightclaw init --telegram-token <token>` validates token upfront; without flag, prompts interactively via `prompt_telegram_token()`
- Shell wrapper template conditionally includes `--channels plugin:telegram@claude-plugins-official` when agent has `.mcp.json`
- 10 new tests: 4 shell wrapper channels tests + 6 CLI integration tests (doctor, init --telegram-token, invalid token)

## Task Commits

Each task was committed atomically:

1. **Task 1: Wire Doctor subcommand and Init --telegram-token to CLI** - `9038f8a` (feat)
2. **Task 2: Update shell wrapper template for --channels flag and add integration tests** - `c83e81a` (feat)

## Files Created/Modified
- `crates/rightclaw-cli/src/main.rs` - Added Doctor variant, Init telegram_token field, cmd_doctor, updated cmd_init
- `templates/agent-wrapper.sh.j2` - Conditional `--channels` flag in both sandbox and no-sandbox paths
- `crates/rightclaw/src/codegen/shell_wrapper.rs` - generate_wrapper passes channels to template based on mcp_config_path
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` - 4 new tests for channels inclusion/exclusion
- `crates/rightclaw-cli/tests/cli_integration.rs` - 6 new tests for doctor and init --telegram-token

## Decisions Made
- Used `mcp_config_path.is_some()` as the signal to enable `--channels` flag -- v1 simplification since Telegram is the only channel. Future versions can parse `.mcp.json` contents.
- Reused `DoctorCheck`'s `Display` impl in `cmd_doctor` instead of duplicating formatting logic.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Phase 3 (default-agent-and-installation) is complete: all 4 plans executed
- `rightclaw init`, `list`, `doctor`, `up`, `down`, `status`, `restart`, `attach` all wired
- Default "Right" agent with BOOTSTRAP.md, OpenShell policy, and optional Telegram channel ready
- Ready for Phase 4: ClawHub skill management and CronSync

## Self-Check: PASSED

All 5 modified files verified present. Both task commits (9038f8a, c83e81a) verified in git log.
