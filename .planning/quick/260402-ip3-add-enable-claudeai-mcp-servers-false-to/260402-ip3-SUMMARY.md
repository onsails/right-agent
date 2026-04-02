---
phase: quick
plan: 260402-ip3
subsystem: codegen
tags: [process-compose, mcp, startup, env-vars, template]
dependency_graph:
  requires: []
  provides: [fast-mcp-startup]
  affects: [templates/process-compose.yaml.j2, codegen/process_compose_tests.rs]
tech_stack:
  added: []
  patterns: [TDD red-green, Jinja2 template env var injection]
key_files:
  created: []
  modified:
    - templates/process-compose.yaml.j2
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - .planning/seeds/SEED-015-mcp-nonblocking-env-var.md
decisions:
  - Both MCP env vars added unconditionally to template (no opt-out flag needed at this stage)
  - SEED-015 closed; ENABLE_CLAUDEAI_MCP_SERVERS=false added alongside MCP_CONNECTION_NONBLOCKING=1 as companion improvement
metrics:
  duration: ~5min
  completed: 2026-04-02
  tasks_completed: 1
  files_modified: 3
---

# Phase quick Plan 260402-ip3: Add MCP env vars to process-compose template Summary

**One-liner:** Added `ENABLE_CLAUDEAI_MCP_SERVERS=false` and `MCP_CONNECTION_NONBLOCKING=1` to the Jinja2 process-compose template, eliminating ~1.5s agent startup delay from cloud MCP server loading.

## What Was Done

Two CC env vars injected into `templates/process-compose.yaml.j2` environment block:

- `ENABLE_CLAUDEAI_MCP_SERVERS=false` — prevents CC from loading user's cloud/account-level MCP servers (Figma, Blockscout, etc.) on agent startup, eliminating a blocking ~1.5s delay
- `MCP_CONNECTION_NONBLOCKING=1` — CC starts immediately, connects to MCP servers in background instead of waiting (SEED-015)

Two regression tests added to `process_compose_tests.rs`:
- `env_contains_enable_claudeai_mcp_servers_false`
- `env_contains_mcp_connection_nonblocking`

SEED-015 status updated from `dormant` to `resolved`.

## Task Execution

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add MCP env vars to template and update tests | c6e3e61 | templates/process-compose.yaml.j2, process_compose_tests.rs, SEED-015 |

## Verification Results

All 242 tests pass. Both new tests confirmed RED before template change, GREEN after.

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None.

## Self-Check: PASSED

- templates/process-compose.yaml.j2: FOUND, contains both env var lines
- crates/rightclaw/src/codegen/process_compose_tests.rs: FOUND, contains both new tests
- Commit c6e3e61: FOUND
- All 242 tests passing
