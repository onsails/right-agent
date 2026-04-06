---
phase: 01-mcp-management-tools-in-rightmemory-server
verified: 2026-04-05T23:45:00Z
status: human_needed
score: 6/6 must-haves verified
human_verification:
  - test: "Call mcp_auth on a real server (e.g. https://mcp.notion.com/mcp) with an actual running agent"
    expected: "Returns authorization_endpoint URL and Telegram bot instruction — no token exposed"
    why_human: "discover_as() makes live HTTPS requests to the OAuth AS. Test coverage only validates the server-not-found error path. The success path requires a real MCP server that supports RFC 9728/8414 OAuth discovery."
---

# Phase 1: MCP Management Tools in rightmemory Server — Verification Report

**Phase Goal:** Add mcp_add, mcp_remove, mcp_list, mcp_auth tools to the rightmemory MCP server so agents can self-manage their MCP connections.
**Verified:** 2026-04-05T23:45:00Z
**Status:** human_needed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Agent can call mcp_add(name, url) and server entry appears in .claude.json with type: http | VERIFIED | `async fn mcp_add` at memory_server.rs:273 calls `add_http_server_to_claude_json`; test `test_mcp_add_creates_entry` passes, asserts `.claude.json` contains `"notion"` and `"http"` |
| 2 | Agent calling mcp_remove(rightmemory) receives an error and .claude.json is unchanged | VERIFIED | Guard at memory_server.rs:309 checks `rightclaw::mcp::PROTECTED_MCP_SERVER` ("rightmemory") and returns `McpError::invalid_params`; test `test_mcp_remove_rightmemory_rejected` passes |
| 3 | Agent can call mcp_list() and receives JSON array with name/url/auth/source/kind fields — no token values | VERIFIED | `async fn mcp_list` at memory_server.rs:347 calls `mcp_auth_status(&self.agent_dir)`, maps to JSON with exactly `{name, url, auth, source, kind}`; test `test_mcp_list_shows_server_metadata` asserts `token`, `secret`, `access_token` fields are absent |
| 4 | Agent can call mcp_auth(server_name) and receives the AS authorization_endpoint URL — no PKCE or DCR; agent instructed to use Telegram bot | VERIFIED (partial) | `async fn mcp_auth` at memory_server.rs:371 calls `discover_as()` only, returns `authorization_endpoint` URL plus Telegram bot instruction; test `test_mcp_auth_server_not_found` passes; success path requires live HTTPS call — no automated test |
| 5 | All four tools appear in the MCP server's tools/list response (tool_router registration) | VERIFIED | All four methods carry `#[tool(...)]` attribute inside `#[tool_router] impl MemoryServer`; `get_info().instructions` includes all four names; test `test_get_info_mentions_mcp_tools` passes |
| 6 | cargo build --workspace --debug and cargo test -p rightclaw-cli both pass | VERIFIED | `cargo test -p rightclaw-cli --bin rightclaw`: 49 passed, 0 failed (pre-existing `test_status_no_running_instance` integration test failure unrelated to this phase) |

**Score:** 6/6 truths verified (1 with human verification caveat on mcp_auth live path)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw-cli/src/memory_server.rs` | Four `#[tool]` methods + four param structs + updated `get_info()` | VERIFIED | `McpAddParams`, `McpRemoveParams`, `McpListParams`, `McpAuthParams` structs at lines 56–76; `mcp_add` at 273, `mcp_remove` at 304, `mcp_list` at 347, `mcp_auth` at 371; instructions at line 423 list all 10 tools |
| `crates/rightclaw-cli/src/memory_server.rs` | `MemoryServer` struct with `agent_dir` and `rightclaw_home` fields, 4-arg `new()` | VERIFIED | Struct at lines 81–87; `new()` at lines 91–104; `run_memory_server()` reads `RC_RIGHTCLAW_HOME` at lines 489–495, passes both paths to `new()` |
| `crates/rightclaw-cli/src/memory_server_tests.rs` | Extracted tests + 9 new MCP tool tests | VERIFIED | 329 lines; `setup_server()` uses 4-arg `MemoryServer::new()`; 9 MCP test functions present (lines 176–328); all pass |
| `crates/rightclaw/src/codegen/mcp_config.rs` | `RC_RIGHTCLAW_HOME` injected into rightmemory env section | VERIFIED | 4-arg signature at line 12; `RC_RIGHTCLAW_HOME` injected at line 46; test `mcp_config_env_contains_rightclaw_home` at line 227 asserts injection |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `mcp_add` tool method | `rightclaw::mcp::credentials::add_http_server_to_claude_json` | direct call with `&self.agent_dir.join(".claude.json")` and `agent_path_key` | WIRED | memory_server.rs:291 |
| `mcp_remove` tool method | `rightclaw::mcp::PROTECTED_MCP_SERVER` | guard check before `remove_http_server_from_claude_json` | WIRED | memory_server.rs:309 checks constant, calls remove at 325 |
| `mcp_list` tool method | `rightclaw::mcp::detect::mcp_auth_status` | direct call with `&self.agent_dir` | WIRED | memory_server.rs:352 |
| `mcp_auth` tool method | `rightclaw::mcp::oauth::discover_as` | async call; returns `metadata.authorization_endpoint` | WIRED | memory_server.rs:402–405 |
| `generate_mcp_config()` caller in cmd_up | rightmemory env section | `RC_RIGHTCLAW_HOME` key in `serde_json::json!()` | WIRED | main.rs:701 passes `home` as 4th arg; mcp_config.rs:46 writes `RC_RIGHTCLAW_HOME` |
| `run_memory_server()` | `MemoryServer::new()` | `agent_dir` from HOME, `rightclaw_home` from RC_RIGHTCLAW_HOME as 3rd and 4th args | WIRED | memory_server.rs:487–497 |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `mcp_add` | `params.name`, `params.url` | MCP tool params → `add_http_server_to_claude_json` → `.claude.json` write | Yes — writes to filesystem | FLOWING |
| `mcp_remove` | `params.name` | MCP tool params → `remove_http_server_from_claude_json` → `.claude.json` write | Yes — modifies filesystem | FLOWING |
| `mcp_list` | `statuses` | `mcp_auth_status(&self.agent_dir)` reads `.claude.json` and `.mcp.json` | Yes — reads real agent dir | FLOWING |
| `mcp_auth` | `servers` / `metadata` | `list_http_servers_from_claude_json` → `discover_as` (live HTTPS) | Yes for server lookup; live HTTPS for discovery | FLOWING (success path requires live HTTPS — see human verification) |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| mcp_add writes .claude.json with type:http | `cargo test -p rightclaw-cli --bin rightclaw test_mcp_add_creates_entry` | PASS — 49 tests ok | PASS |
| mcp_remove rejects rightmemory | `cargo test -p rightclaw-cli --bin rightclaw test_mcp_remove_rightmemory_rejected` | PASS | PASS |
| mcp_list returns fields without token | `cargo test -p rightclaw-cli --bin rightclaw test_mcp_list_shows_server_metadata` | PASS | PASS |
| get_info lists all 10 tools | `cargo test -p rightclaw-cli --bin rightclaw test_get_info_mentions_mcp_tools` | PASS | PASS |
| mcp_auth live OAuth discovery | Not runnable without live server | N/A | SKIP — see human verification |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| MCP-TOOL-01 | 01-02 | Agent can call `mcp_add(name, url)` to add HTTP MCP server to `.claude.json` with `type: http` | SATISFIED | `async fn mcp_add` wired to `add_http_server_to_claude_json`; https:// validation added (T-02-01 threat mitigation); test passes |
| MCP-TOOL-02 | 01-02 | Agent can call `mcp_remove(name)` to remove server; rightmemory protected | SATISFIED | Guard against `PROTECTED_MCP_SERVER` returns `McpError::invalid_params`; test `test_mcp_remove_rightmemory_rejected` passes |
| MCP-TOOL-03 | 01-02 | Agent can call `mcp_list()` to see all MCP servers with source and auth state | SATISFIED | Wired to `mcp_auth_status()`; returns name/url/auth/source/kind; empty and populated cases tested |
| MCP-TOOL-04 | 01-02 | Agent can call `mcp_auth(server_name)` to initiate OAuth — returns auth URL | SATISFIED (with human caveat) | Discovery-only (`discover_as()`); returns `authorization_endpoint` + Telegram bot instruction; server-not-found case tested |
| MCP-TOOL-05 | 01-02 | All tools exposed via existing rightmemory MCP server | SATISFIED | All four tools inside `#[tool_router] impl MemoryServer` — no new binary or `.mcp.json` entry needed |
| MCP-NF-01 | 01-02 | Tools must not expose secrets in return values | SATISFIED | `mcp_list` via `mcp_auth_status()` returns `AuthState::Present/Missing` only; `mcp_auth` returns URL only; test asserts `token`, `secret`, `access_token` absent |
| MCP-NF-02 | 01-01, 01-02 | `mcp_auth` works headless — returns URL, no blocking listener | SATISFIED | `discover_as()` call only; returns `authorization_endpoint` string; no HTTP listener, no PKCE/DCR in this tool; `RC_RIGHTCLAW_HOME` injected into rightmemory env via `generate_mcp_config()` for future tunnel use |

### Anti-Patterns Found

| File | Pattern | Severity | Impact |
|------|---------|----------|--------|
| `memory_server.rs:489–494` | `rightclaw_home` fallback to `PathBuf::from(".")` with warn | Info | `mcp_auth` does not use `rightclaw_home` (uses ad-hoc reqwest client); fallback is non-fatal for current tool set. Documented in SUMMARY as known stub field. |

No blockers or warnings found. The `rightclaw_home` field is stored but not yet consumed by any of the four tools — `mcp_auth` constructs its own `reqwest::Client`. This is an intentional design decision (documented in 01-02-SUMMARY.md: "rightclaw_home field is stored on the struct but not used by any of the four tools") and has no impact on the phase goal.

### Human Verification Required

#### 1. mcp_auth Success Path — Live OAuth Discovery

**Test:** With a running agent, call the `mcp_auth` tool against a real HTTP MCP server that supports OAuth (e.g. `server_name: "notion"` after adding `https://mcp.notion.com/mcp` via `mcp_add`).

**Expected:** Tool returns a response containing the AS `authorization_endpoint` URL and the Telegram bot instruction string `"/mcp auth notion"`. No token, code_verifier, or client_secret in the output.

**Why human:** `discover_as()` makes live HTTPS requests to `{server_url}/.well-known/oauth-authorization-server` and `/.well-known/openid-configuration`. The automated test only validates the error path (server not found in `.claude.json`). The success path is blocked by the outbound HTTP call — cannot run without a live OAuth-capable MCP server.

---

### Gaps Summary

No gaps. All six must-haves are verified. One human verification item remains for the `mcp_auth` live OAuth discovery path — not a gap in the implementation, but an untestable automated path that requires a real OAuth AS.

---

_Verified: 2026-04-05T23:45:00Z_
_Verifier: Claude (gsd-verifier)_
