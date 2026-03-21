---
phase: 01-foundation-and-agent-discovery
plan: 02
subsystem: agent-discovery
tags: [rust, filesystem, cli, clap, tempfile, serde-saphyr, include_str]

requires:
  - phase: 01-foundation-and-agent-discovery-01
    provides: AgentDef, AgentConfig, AgentError types, resolve_home, CLI skeleton
provides:
  - Agent discovery logic (discover_agents, validate_agent_name, parse_agent_config)
  - Init command with embedded default Right agent templates
  - List command displaying discovered agents
  - Integration tests for all CLI subcommands
affects: [02-sandbox-and-process-compose, 03-default-agent-and-channels]

tech-stack:
  added: [tempfile]
  patterns: [include_str for embedded templates, tempdir-based filesystem tests, assert_cmd integration tests]

key-files:
  created:
    - crates/rightclaw/src/agent/discovery.rs
    - crates/rightclaw/src/agent/discovery_tests.rs
    - crates/rightclaw/src/init.rs
    - crates/rightclaw-cli/tests/cli_integration.rs
    - templates/right/IDENTITY.md
    - templates/right/SOUL.md
    - templates/right/AGENTS.md
    - templates/right/policy.yaml
  modified:
    - crates/rightclaw/src/agent/mod.rs
    - crates/rightclaw/src/lib.rs
    - crates/rightclaw/Cargo.toml
    - crates/rightclaw-cli/src/main.rs

key-decisions:
  - "Tests extracted to discovery_tests.rs to keep discovery.rs focused on implementation"
  - "Optional file detection uses simple exists() check rather than walkdir for flat structure"
  - "Agents sorted by name for deterministic output ordering"

patterns-established:
  - "TDD with tests in separate _tests.rs files using #[path] attribute"
  - "Embedded templates via include_str! from templates/ directory at repo root"
  - "Integration tests using assert_cmd + tempdir for full CLI verification"

requirements-completed: [WORK-03, WORK-04, WORK-05, PROJ-02]

duration: 5min
completed: 2026-03-21
---

# Phase 01 Plan 02: Agent Discovery and Init Summary

**Agent discovery scanning directories with strict validation, init command embedding Right agent templates, and CLI integration tests covering all subcommands**

## Performance

- **Duration:** 5 min
- **Started:** 2026-03-21T23:12:17Z
- **Completed:** 2026-03-21T23:17:00Z
- **Tasks:** 2
- **Files modified:** 12

## Accomplishments
- Agent discovery logic: scans directories, validates names, checks required/optional files, parses agent.yaml with deny_unknown_fields
- Init command creates default Right agent from compile-time embedded templates
- List command discovers and displays agents with config/mcp status
- 41 total tests (35 unit + 6 integration), all passing, clippy clean

## Task Commits

Each task was committed atomically:

1. **Task 1: Agent discovery, config parsing, and init with templates** - `4e0a449` (feat)
2. **Task 2: Wire CLI commands and integration tests** - `ee57fe2` (feat)

## Files Created/Modified
- `crates/rightclaw/src/agent/discovery.rs` - discover_agents, validate_agent_name, parse_agent_config
- `crates/rightclaw/src/agent/discovery_tests.rs` - 20 unit tests for discovery logic
- `crates/rightclaw/src/init.rs` - init_rightclaw_home with embedded templates
- `crates/rightclaw-cli/src/main.rs` - Wired init and list command handlers
- `crates/rightclaw-cli/tests/cli_integration.rs` - 6 end-to-end CLI tests
- `templates/right/IDENTITY.md` - Default Right agent identity
- `templates/right/SOUL.md` - Default Right agent personality
- `templates/right/AGENTS.md` - Default Right agent capabilities (blank for user customization)
- `templates/right/policy.yaml` - Placeholder OpenShell policy
- `crates/rightclaw/src/agent/mod.rs` - Added discovery module exports
- `crates/rightclaw/src/lib.rs` - Added init module
- `crates/rightclaw/Cargo.toml` - Added tempfile dev-dependency

## Decisions Made
- Tests extracted to separate `discovery_tests.rs` file to keep `discovery.rs` under 900 LoC (per project conventions)
- Used simple `path.exists()` checks for optional file detection rather than walkdir -- agent directories are flat
- Sorted agents by name for deterministic CLI output

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Agent discovery and CLI foundation complete
- Ready for Phase 2: sandbox policy parsing and process-compose generation
- `rightclaw init` + `rightclaw list` are functional end-to-end

## Self-Check: PASSED

All 8 created files verified on disk. Both task commits (4e0a449, ee57fe2) found in git log.

---
*Phase: 01-foundation-and-agent-discovery*
*Completed: 2026-03-21*
