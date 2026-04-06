# Phase 1: MCP management tools in rightmemory server - Context

**Gathered:** 2026-04-05 (assumptions mode)
**Status:** Ready for planning

<domain>
## Phase Boundary

Add mcp_add, mcp_remove, mcp_list, mcp_auth tools to the rightmemory MCP server so agents can self-manage their MCP connections. All tools exposed via the existing rightmemory stdio MCP server. OAuth callback handling (token exchange via cloudflared) is out of scope — mcp_auth only constructs and returns the auth URL.

</domain>

<decisions>
## Implementation Decisions

### MCP Tool Infrastructure
- **D-01:** Add all four MCP tools (`mcp_add`, `mcp_remove`, `mcp_list`, `mcp_auth`) directly to the existing `MemoryServer` struct in `crates/rightclaw-cli/src/memory_server.rs` using the same `#[tool]` macro pattern as existing tools (`store`, `recall`, `search`, `forget`, `cron_list_runs`, `cron_show_run`).
- **D-02:** No new binary, no new `.mcp.json` entry, no separate MCP server — MCP-TOOL-05 is satisfied by adding to the existing server.

### Agent Directory and .claude.json Path
- **D-03:** Derive agent directory from `$HOME` env var (already read in `run_memory_server()` for `memory.db`). `.claude.json` path = `$HOME/.claude.json`.
- **D-04:** Use `agent_dir.canonicalize().unwrap_or_else(|_| agent_dir.clone()).display().to_string()` as the `agent_path_key` for `.claude.json` project lookups — matches the pattern used by `rightclaw up` and bot handlers (`handler.rs:483-487`).

### mcp_auth OAuth Scope
- **D-05:** `mcp_auth` tool implements only Phase 1: AS discovery + auth URL construction. Returns the auth URL string to the agent. Does NOT start an HTTP listener or block waiting for callback — the token exchange lives in the bot process via cloudflared tunnel (`oauth_callback.rs`), which is already implemented.
- **D-06:** `mcp_auth` is headless-compatible (MCP-NF-02) because it just returns a URL. The user clicks it; the existing bot callback infrastructure completes the flow.

### Secret Hygiene
- **D-07:** `mcp_list` uses the existing `mcp_auth_status()` from `crates/rightclaw/src/mcp/detect.rs` which returns `ServerStatus` with only `name`, `url`, `state` (`AuthState::Present/Missing`), `source`, and `kind` — no token field. Bearer tokens are never read into the output value.
- **D-08:** `mcp_remove` must reject attempts to remove `rightmemory` (MCP-TOOL-02). Check server name before modifying `.claude.json`.

### Claude's Discretion
- Exact error message wording for protected server removal attempts
- `.claude.json` write strategy (read-modify-write vs in-place patch)
- Whether to support both `.claude.json` and `.mcp.json` sources in `mcp_remove` or only `.claude.json`

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Existing MCP server (where tools are added)
- `crates/rightclaw-cli/src/memory_server.rs` — MemoryServer struct, existing tool registration pattern, run_memory_server() with $HOME env setup

### MCP detection and credential infrastructure
- `crates/rightclaw/src/mcp/detect.rs` — mcp_auth_status(), ServerStatus, AuthState — reuse for mcp_list
- `crates/rightclaw/src/mcp/credentials.rs` — credential/token storage patterns
- `crates/rightclaw/src/mcp/oauth.rs` — existing OAuth AS discovery logic (reuse for mcp_auth URL construction)
- `crates/rightclaw/src/mcp/mod.rs` — MCP module public interface

### .claude.json structure (for mcp_add, mcp_remove, mcp_list)
- `crates/rightclaw/src/codegen/claude_json.rs` — .claude.json read/write; mcpServers structure; project key derivation
- `crates/rightclaw/src/codegen/mcp_config.rs` — MCP config generation patterns

### Bot handlers (reference implementation for same operations via Telegram)
- `crates/bot/src/telegram/handler.rs` — /mcp add, /mcp remove, /mcp list, /mcp auth handlers (lines 239-530) — canonical patterns to replicate as MCP tools

### Requirements
- `.planning/REQUIREMENTS.md` — MCP-TOOL-01..05, MCP-NF-01..02

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `mcp_auth_status()` in `detect.rs` — returns `Vec<ServerStatus>` with safe metadata only (no tokens); direct reuse for `mcp_list` output
- `mcp/oauth.rs` — AS discovery logic for constructing auth URLs; reuse in `mcp_auth` tool
- `mcp/credentials.rs` — token storage; reuse in `mcp_auth` for writing Bearer token after callback (though callback itself lives in bot)
- `#[tool]` / `#[tool_router]` macros from `rmcp` — already used for all 6 existing tools in `memory_server.rs`

### Established Patterns
- Tool registration: `#[tool(description = "...")]` fn on `MemoryServer`, router via `#[tool_router]` — match exactly for new tools
- Agent dir from `$HOME`: `std::env::var("HOME")` → `PathBuf` — already in `run_memory_server()`
- Project key for `.claude.json`: `canonicalize().unwrap_or(dir).display().to_string()` — from `handler.rs:483-487` and `claude_json.rs`
- Bot handlers (`handler.rs` lines 239-530) contain the full logic for all 4 operations — primary reference for what the MCP tools should do

### Integration Points
- `MemoryServer` struct → add 4 new tool methods
- `.claude.json` mcpServers field → read/write for mcp_add, mcp_remove, mcp_list
- `mcp/oauth.rs` AS discovery → call from `mcp_auth` to get authorization_endpoint
- `mcp/detect.rs` mcp_auth_status() → call from `mcp_list`

</code_context>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches within the patterns above.

</specifics>

<deferred>
## Deferred Ideas

None — analysis stayed within phase scope.

</deferred>

---

*Phase: 01-mcp-management-tools-in-rightmemory-server*
*Context gathered: 2026-04-05*
