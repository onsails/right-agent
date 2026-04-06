---
phase: 01-mcp-management-tools-in-rightmemory-server
plan: 02
subsystem: memory-server
tags: [mcp, memory-server, mcp-tools, credentials, oauth, tdd]
dependency_graph:
  requires: [MemoryServer::new-4-arg, RC_RIGHTCLAW_HOME-in-mcp-json, memory_server_tests.rs]
  provides: [mcp_add-tool, mcp_remove-tool, mcp_list-tool, mcp_auth-tool]
  affects: [rightclaw-cli/memory_server.rs, rightclaw-cli/memory_server_tests.rs, rightclaw-cli/Cargo.toml]
tech_stack:
  added: [reqwest-in-rightclaw-cli]
  patterns: [tool-router-method, credentials-api-delegation, detect-api-delegation, oauth-discovery-only]
key_files:
  created: []
  modified:
    - crates/rightclaw-cli/src/memory_server.rs
    - crates/rightclaw-cli/src/memory_server_tests.rs
    - crates/rightclaw-cli/Cargo.toml
decisions:
  - "mcp_auth performs AS discovery only — no PKCE/DCR — code_verifier cannot be stored in bot PendingAuthMap from separate process"
  - "https:// URL validation added to mcp_add per T-02-01 threat mitigation (plan threat model)"
  - "reqwest added directly to rightclaw-cli rather than proxied through rightclaw crate — needed for mcp_auth HTTP client construction"
  - "setup_server_with_dir() is an alias for setup_server() — both use same 4-arg MemoryServer::new()"
metrics:
  duration: 4min
  completed_date: "2026-04-05"
  tasks: 2
  files_modified: 3
---

# Phase 01 Plan 02: MCP Management Tools — SUMMARY

Four MCP tools (mcp_add, mcp_remove, mcp_list, mcp_auth) added to MemoryServer wrapping existing credentials.rs, detect.rs, and oauth.rs infrastructure. Agents can now self-manage HTTP MCP server connections without Telegram.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add mcp_add, mcp_remove, mcp_list, mcp_auth tool methods | d9c3191 | memory_server.rs, Cargo.toml |
| 2 | Add tests for all four new MCP tools | d85ad8a | memory_server_tests.rs |

## What Was Built

**Task 1:** Four `#[tool]` methods added inside the existing `#[tool_router] impl MemoryServer` block. Four param structs added before the block (`McpAddParams`, `McpRemoveParams`, `McpListParams`, `McpAuthParams`).

- `mcp_add`: validates `https://` prefix (T-02-01 threat mitigation), then calls `add_http_server_to_claude_json()` using `self.agent_dir` for both the claude.json path and agent_path_key derivation.
- `mcp_remove`: guards `rightclaw::mcp::PROTECTED_MCP_SERVER` ("rightmemory") before calling `remove_http_server_from_claude_json()`. Returns `McpError::invalid_params` for both the protected server and `ServerNotFound` cases.
- `mcp_list`: calls `mcp_auth_status(&self.agent_dir)`, maps results to JSON with name/url/auth/source/kind fields only — no token fields (MCP-NF-01 satisfied by library design).
- `mcp_auth`: looks up server URL from `.claude.json` via `list_http_servers_from_claude_json()`, calls `discover_as()` only, returns `authorization_endpoint` URL plus Telegram bot instruction. No PKCE/DCR (code_verifier cannot cross process boundary to bot's PendingAuthMap).

`get_info()` instructions updated to list all 10 tools. `reqwest` added to rightclaw-cli dependencies for mcp_auth HTTP client.

**Task 2:** 9 new test functions added to `memory_server_tests.rs`. `setup_server_with_dir()` helper aliasing `setup_server()`. All tests pass.

## Deviations from Plan

**1. [Rule 2 - Security] Added https:// URL validation to mcp_add**

- **Found during:** Task 1 implementation
- **Issue:** Plan threat model T-02-01 requires URL validation to reject non-https URLs
- **Fix:** Added `if !params.url.starts_with("https://")` guard returning `McpError::invalid_params`
- **Files modified:** crates/rightclaw-cli/src/memory_server.rs
- **Commit:** d9c3191

**2. [Rule 3 - Blocking] Added reqwest to rightclaw-cli Cargo.toml**

- **Found during:** Task 1 implementation
- **Issue:** mcp_auth needs `reqwest::Client::new()` but reqwest was not in rightclaw-cli deps
- **Fix:** Added `reqwest = { workspace = true }` to rightclaw-cli/Cargo.toml
- **Files modified:** crates/rightclaw-cli/Cargo.toml
- **Commit:** d9c3191

## Known Stubs

None — all four tools are fully wired to existing infrastructure. `rightclaw_home` field is stored on the struct but not used by any of the four tools (mcp_auth uses an ad-hoc reqwest client rather than reading tunnel config).

## Threat Flags

None — no new network endpoints or auth paths beyond what the plan's threat model already covers. mcp_auth makes outbound HTTPS calls to external OAuth AS, which is within the T-02-06 acceptance boundary.

## Self-Check: PASSED

- `/home/wb/dev/rightclaw/.claude/worktrees/agent-a87f927b/crates/rightclaw-cli/src/memory_server.rs` — FOUND
- `/home/wb/dev/rightclaw/.claude/worktrees/agent-a87f927b/crates/rightclaw-cli/src/memory_server_tests.rs` — FOUND
- Commit d9c3191 — FOUND
- Commit d85ad8a — FOUND
- `cargo build --workspace` exits 0
- `cargo test -p rightclaw-cli --bin rightclaw`: 49 passed, 0 failed (pre-existing test_status_no_running_instance excluded from unit test run)
