---
phase: 19-home-isolation-hardening
plan: "01"
subsystem: codegen
tags: [bug-fix, tdd, telegram, mcp, agent-isolation]
dependency_graph:
  requires: []
  provides:
    - "Correct Telegram detection via agent.config (not .mcp.json presence)"
    - "RC_AGENT_NAME env injection in per-agent .mcp.json"
    - "mcp_config_path field removed from AgentDef"
    - "RC_AGENT_NAME warning in memory server"
  affects:
    - "crates/rightclaw/src/codegen/shell_wrapper.rs"
    - "crates/rightclaw/src/codegen/settings.rs"
    - "crates/rightclaw/src/codegen/mcp_config.rs"
    - "crates/rightclaw/src/codegen/telegram.rs"
    - "crates/rightclaw/src/agent/types.rs"
    - "crates/rightclaw/src/agent/discovery.rs"
    - "crates/rightclaw/src/init.rs"
    - "crates/rightclaw-cli/src/main.rs"
    - "crates/rightclaw-cli/src/memory_server.rs"
tech_stack:
  added: []
  patterns:
    - "Telegram detection reads agent.config fields, not filesystem artifacts"
    - "MCP env injection for agent identity propagation"
key_files:
  created: []
  modified:
    - crates/rightclaw/src/codegen/shell_wrapper.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - crates/rightclaw/src/codegen/settings.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw/src/codegen/mcp_config.rs
    - crates/rightclaw/src/codegen/telegram.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - crates/rightclaw/src/codegen/claude_json.rs
    - crates/rightclaw/src/codegen/system_prompt_tests.rs
    - crates/rightclaw/src/agent/types.rs
    - crates/rightclaw/src/agent/discovery.rs
    - crates/rightclaw/src/agent/discovery_tests.rs
    - crates/rightclaw/src/init.rs
    - crates/rightclaw-cli/src/memory_server.rs
    - crates/rightclaw-cli/src/main.rs
decisions:
  - "Telegram detection reads agent.config.telegram_token/telegram_token_file; mcp_config_path was an unreliable proxy that broke when Phase 17 started generating .mcp.json for all agents"
  - "mcp_config_path removed from AgentDef entirely — filesystem check via agent.path.join('.mcp.json').exists() used at the one status display callsite"
  - "telegram.rs no longer writes {\"telegram\": true} marker to .mcp.json — that responsibility belongs to generate_mcp_config"
  - "generate_mcp_config gains agent_name param so RC_AGENT_NAME is injected into every agent's rightmemory env section"
metrics:
  duration_minutes: 7
  completed_date: "2026-03-27"
  tasks_completed: 2
  files_modified: 15
---

# Phase 19 Plan 01: Telegram false-positive + RC_AGENT_NAME fixes Summary

TDD bug fix: Telegram --channels false-positive caused by mcp_config_path check, plus RC_AGENT_NAME never injected into per-agent .mcp.json env section. Both bugs fixed with regression tests, mcp_config_path field fully removed.

## What Was Built

### Task 1 (RED): Failing regression tests
- `wrapper_without_telegram_omits_channels_when_mcp_json_exists` in `shell_wrapper_tests.rs` — demonstrates that `.mcp.json` presence without telegram config triggered `--channels` (bug)
- `mcp_config_env_contains_agent_name` in `mcp_config.rs` — demonstrates `RC_AGENT_NAME` was absent from generated env section (bug)
- Both committed failing before any production code changed

### Task 2 (GREEN): All fixes implemented

**D-01 — Telegram detection fix (shell_wrapper.rs + settings.rs):**
Both files now check `agent.config.as_ref().map(|c| c.telegram_token.is_some() || c.telegram_token_file.is_some())` instead of `agent.mcp_config_path.is_some()`. Phase 17 made `.mcp.json` universal for all agents (rightmemory), which made the old check a false positive for every agent.

**D-02 — Remove .mcp.json marker creation from telegram.rs:**
`generate_telegram_channel_config` no longer writes `{"telegram": true}` to `.mcp.json`. That pattern was the detection mechanism and is now obsolete. The function only manages `.claude/channels/telegram/` files.

**D-03 — Remove mcp_config_path from AgentDef:**
Field deleted from `types.rs`. Discovery no longer populates it. Status display in `main.rs` uses `agent.path.join(".mcp.json").exists()` directly. All 15 test helpers updated.

**D-04 — RC_AGENT_NAME injection in generate_mcp_config:**
Signature changed from `(agent_path, binary)` to `(agent_path, binary, agent_name)`. The env section now writes `{"RC_AGENT_NAME": agent_name}`. Call site in `main.rs` passes `&agent.name`. All test calls updated to 3-arg.

**D-05 — RC_AGENT_NAME warning in memory_server.rs:**
`std::env::var("RC_AGENT_NAME")` now emits a `tracing::warn!` when the var is absent or empty, flagging the misconfiguration instead of silently defaulting to "unknown".

## Test Results

- 229 library tests pass (0 failures)
- 26 integration tests pass (0 failures)
- 1 pre-existing failure: `test_status_no_running_instance` (documented in STATE.md)
- `cargo build --workspace` exits 0
- `cargo clippy --workspace` produces 2 pre-existing warnings (empty format strings, unrelated)
- Zero `mcp_config_path` references remain in codebase

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing functionality] init.rs synthetic AgentDef needed telegram config object**
- **Found during:** Task 2 — `init.rs` constructed AgentDef with `mcp_config_path` conditional on `telegram_token` to trigger telegram plugin in settings.json. After removing `mcp_config_path`, this stopped working.
- **Fix:** Built a synthetic `AgentConfig` with `telegram_token` set when the token is provided, so `generate_settings` detects telegram correctly via D-01 path.
- **Files modified:** `crates/rightclaw/src/init.rs`
- **Commit:** fb2907e

**2. [Rule 2 - Dead code] init.rs still wrote legacy `{"telegram": true}` marker**
- **Found during:** Task 2 — `init.rs` had its own copy of the `.mcp.json` marker write (same pattern as telegram.rs).
- **Fix:** Removed the `std::fs::write(agents_dir.join(".mcp.json"), ...)` block from `init_rightclaw_home`. Detection now depends on config, not marker files.
- **Files modified:** `crates/rightclaw/src/init.rs`
- **Commit:** fb2907e

## Known Stubs

None. All detection logic is wired to real agent config data.

## Commits

| Hash | Message |
|------|---------|
| 7718d20 | test(19-01): add failing regression tests for telegram false-positive and RC_AGENT_NAME |
| fb2907e | fix(19-01): telegram false-positive, RC_AGENT_NAME injection, mcp_config_path removal |

## Self-Check: PASSED
