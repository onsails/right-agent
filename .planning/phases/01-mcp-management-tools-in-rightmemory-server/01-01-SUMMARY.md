---
phase: 01-mcp-management-tools-in-rightmemory-server
plan: 01
subsystem: memory-server
tags: [mcp, memory-server, rightclaw-home, env-injection, test-extraction]
dependency_graph:
  requires: []
  provides: [MemoryServer::new-4-arg, RC_RIGHTCLAW_HOME-in-mcp-json, memory_server_tests.rs]
  affects: [rightclaw-cli/memory_server.rs, rightclaw/codegen/mcp_config.rs, rightclaw-cli/main.rs]
tech_stack:
  added: []
  patterns: [env-var-read-with-warn-fallback, path-redirect-test-extraction]
key_files:
  created:
    - crates/rightclaw-cli/src/memory_server_tests.rs
  modified:
    - crates/rightclaw-cli/src/memory_server.rs
    - crates/rightclaw/src/codegen/mcp_config.rs
    - crates/rightclaw-cli/src/main.rs
decisions:
  - "agent_dir initialized from existing home var (reuse, no redundant env read)"
  - "rightclaw_home fallback to PathBuf::from('.') with warn — non-fatal for Plan 01 (Plan 02 tools will fail gracefully at call time)"
  - "generate_mcp_config receives rightclaw_home: &Path, not String — consistent with existing Path params"
metrics:
  duration: 19min
  completed_date: "2026-04-05"
  tasks: 2
  files_modified: 4
---

# Phase 01 Plan 01: MemoryServer Foundation — SUMMARY

Extend MemoryServer struct with agent_dir and rightclaw_home fields, inject RC_RIGHTCLAW_HOME into rightmemory .mcp.json env, and extract memory_server tests to a separate file to make room for Plan 02's mcp_add/mcp_remove/mcp_list/mcp_auth tools.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add agent_dir+rightclaw_home to MemoryServer, inject RC_RIGHTCLAW_HOME | d740d62 | memory_server.rs, mcp_config.rs, main.rs |
| 2 | Extract memory_server.rs tests to memory_server_tests.rs | 620c59e | memory_server.rs, memory_server_tests.rs |

## What Was Built

**Task 1:** `MemoryServer` gains two new `PathBuf` fields — `agent_dir` (agent's HOME dir, where `.claude.json` lives) and `rightclaw_home` (`~/.rightclaw/`, config root). The 4-arg `new()` signature sets up Plan 02's tool implementations to have direct access to both paths without env reads at tool call time.

`generate_mcp_config()` now accepts `rightclaw_home: &Path` as a 4th param and injects it as `RC_RIGHTCLAW_HOME` in the rightmemory env section of `.mcp.json`. The single caller in `cmd_up` passes `home` (already the rightclaw home path from `resolve_home()`).

`run_memory_server()` reads `RC_RIGHTCLAW_HOME` env var with a warn-on-missing fallback (`PathBuf::from(".")`) — non-fatal because Plan 02 tools will fail with a clear error when they try to use the path.

**Task 2:** The `#[cfg(test)] mod tests { ... }` block (170 lines) moved to `memory_server_tests.rs` with a `#[path = "memory_server_tests.rs"]` redirect in memory_server.rs. All 7 tests continue to pass.

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None — no stub values introduced. `agent_dir` and `rightclaw_home` are stored on the struct but not yet used by any tool (Plan 02 adds the tools that will use them).

## Threat Flags

None — RC_RIGHTCLAW_HOME is written into agent-owned `.mcp.json` from a trusted caller (cmd_up). Path used only to construct PathBuf; no code execution risk.

## Self-Check: PASSED

- `/home/wb/dev/rightclaw/crates/rightclaw-cli/src/memory_server.rs` — FOUND
- `/home/wb/dev/rightclaw/crates/rightclaw-cli/src/memory_server_tests.rs` — FOUND
- `/home/wb/dev/rightclaw/crates/rightclaw/src/codegen/mcp_config.rs` — FOUND
- Commit d740d62 — FOUND
- Commit 620c59e — FOUND
- `cargo test --workspace`: 431 passed (excluding pre-existing `test_status_no_running_instance` failure)
