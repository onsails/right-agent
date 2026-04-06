# Phase 1: MCP management tools in rightmemory server - Research

**Researched:** 2026-04-05
**Domain:** Rust MCP server tool extension — `rmcp` macros, `.claude.json` read-modify-write, OAuth AS discovery
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** Add all four MCP tools (`mcp_add`, `mcp_remove`, `mcp_list`, `mcp_auth`) directly to the existing `MemoryServer` struct in `crates/rightclaw-cli/src/memory_server.rs` using the same `#[tool]` macro pattern as existing tools.
- **D-02:** No new binary, no new `.mcp.json` entry, no separate MCP server — MCP-TOOL-05 is satisfied by adding to the existing server.
- **D-03:** Derive agent directory from `$HOME` env var (already read in `run_memory_server()` for `memory.db`). `.claude.json` path = `$HOME/.claude.json`.
- **D-04:** Use `agent_dir.canonicalize().unwrap_or_else(|_| agent_dir.clone()).display().to_string()` as the `agent_path_key` for `.claude.json` project lookups.
- **D-05:** `mcp_auth` implements only Phase 1: AS discovery + auth URL construction. Returns the auth URL string. Does NOT start an HTTP listener or block waiting for callback.
- **D-06:** `mcp_auth` is headless-compatible (MCP-NF-02) because it just returns a URL. Existing bot callback infrastructure completes the flow.
- **D-07:** `mcp_list` uses `mcp_auth_status()` from `detect.rs` — no token field in output.
- **D-08:** `mcp_remove` must reject attempts to remove `rightmemory`. Check server name against `rightclaw::mcp::PROTECTED_MCP_SERVER` constant before modifying `.claude.json`.

### Claude's Discretion

- Exact error message wording for protected server removal attempts
- `.claude.json` write strategy (read-modify-write vs in-place patch)
- Whether to support both `.claude.json` and `.mcp.json` sources in `mcp_remove` or only `.claude.json`

### Deferred Ideas (OUT OF SCOPE)

None — analysis stayed within phase scope.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| MCP-TOOL-01 | Agent can call `mcp_add(name, url)` to add HTTP MCP server to `.claude.json` with `type: http` | `add_http_server_to_claude_json()` in `credentials.rs` is the exact implementation — wrap it in a `#[tool]` method |
| MCP-TOOL-02 | Agent can call `mcp_remove(name)` to remove server (rightmemory protected) | `remove_http_server_from_claude_json()` exists; `PROTECTED_MCP_SERVER` constant exists in `mcp/mod.rs` |
| MCP-TOOL-03 | Agent can call `mcp_list()` to see all MCP servers with source and auth state | `mcp_auth_status()` returns `Vec<ServerStatus>` with name, url, state, source, kind — direct reuse |
| MCP-TOOL-04 | Agent can call `mcp_auth(server_name)` to initiate OAuth — returns auth URL | `discover_as()` + `register_client_or_fallback()` + `build_auth_url()` in `oauth.rs` — compose them |
| MCP-TOOL-05 | All tools exposed via existing rightmemory MCP server | D-01/D-02 locked: add to `MemoryServer` in `memory_server.rs` |
| MCP-NF-01 | Tools must not expose secrets in return values | `mcp_list` via `mcp_auth_status()` returns `AuthState::Present/Missing` only (no token) — verify `mcp_auth` only returns URL |
| MCP-NF-02 | `mcp_auth` works headless — returns URL | D-05/D-06: returns URL string, no blocking listener |
</phase_requirements>

## Summary

This phase adds four MCP tools to the existing `MemoryServer` struct. All the hard work is already done: `credentials.rs` has `add_http_server_to_claude_json`, `remove_http_server_from_claude_json`, and `list_http_servers_from_claude_json`. `detect.rs` has `mcp_auth_status()`. `oauth.rs` has `discover_as`, `register_client_or_fallback`, and `build_auth_url`. The bot's `handler.rs` has working implementations of all four operations (lines 239–530) that are the canonical pattern to replicate.

The implementation is almost mechanical: wrap each existing function in a `#[tool]` method on `MemoryServer`, derive the agent dir from `$HOME`, derive `agent_path_key` using the established canonicalize pattern, and return text strings. The only non-trivial tool is `mcp_auth` which needs tunnel hostname access — the bot reads this from `global_config.tunnel.hostname` but the MCP server has no config.yaml access. The planner must decide how `mcp_auth` gets the redirect URI: either read `config.yaml` from `$HOME/../..` (global config), or accept it as a parameter, or omit redirect URI and require it as a parameter.

`MemoryServer` is `Clone` and already stores `agent_name: String`. Adding `agent_dir: PathBuf` (or deriving it at call time from `HOME` env var) is the straightforward approach. Since `$HOME` is already read in `run_memory_server()` and passed to `MemoryServer::new()`, the cleanest path is to also pass `agent_dir: PathBuf` to `MemoryServer::new()` and store it on the struct — no env var reads inside tool methods.

**Primary recommendation:** Add `agent_dir: PathBuf` field to `MemoryServer`; compute it once in `run_memory_server()` alongside the existing `HOME` read; pass it to `new()`. Tool methods call existing `credentials.rs`/`detect.rs`/`oauth.rs` functions directly. For `mcp_auth` redirect URI, read global `config.yaml` from the system config dir (not agent HOME) via `rightclaw::config::read_global_config()`.

## Standard Stack

### Core (already in workspace — no new deps needed)
| Library | Location | Purpose |
|---------|----------|---------|
| `rmcp` | workspace | `#[tool]`, `#[tool_router]`, `Parameters`, `CallToolResult`, `Content` macros — already imported in `memory_server.rs` |
| `schemars` | workspace | `JsonSchema` derive for param structs — already imported |
| `serde` | workspace | `Deserialize` derive for param structs — already imported |
| `reqwest` | workspace | HTTP client for OAuth AS discovery in `mcp_auth` — already in `rightclaw` crate |
| `rightclaw::mcp::credentials` | crate | `add_http_server_to_claude_json`, `remove_http_server_from_claude_json` |
| `rightclaw::mcp::detect` | crate | `mcp_auth_status`, `ServerStatus`, `AuthState`, `ServerKind`, `ServerSource` |
| `rightclaw::mcp::oauth` | crate | `discover_as`, `register_client_or_fallback`, `build_auth_url`, `generate_pkce`, `generate_state` |
| `rightclaw::mcp::PROTECTED_MCP_SERVER` | crate | `"rightmemory"` constant |
| `rightclaw::config::read_global_config` | crate | Read tunnel hostname for `mcp_auth` redirect URI |

[VERIFIED: codebase grep] All listed modules exist and are pub in the rightclaw crate.

**Installation:** No new dependencies required. [VERIFIED: codebase inspection]

## Architecture Patterns

### MemoryServer tool registration pattern
[VERIFIED: `crates/rightclaw-cli/src/memory_server.rs`]

```rust
// 1. Parameter struct (required for every tool):
#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpAddParams {
    #[schemars(description = "Server name (alphanumeric, hyphens)")]
    pub name: String,
    #[schemars(description = "HTTP MCP server URL (https://)")]
    pub url: String,
}

// 2. Tool method inside #[tool_router] impl MemoryServer:
#[tool(description = "Add an HTTP MCP server to agent's .claude.json.")]
async fn mcp_add(
    &self,
    Parameters(params): Parameters<McpAddParams>,
) -> Result<CallToolResult, McpError> {
    // ... call credentials::add_http_server_to_claude_json(...)
    Ok(CallToolResult::success(vec![Content::text("Added MCP server: name (url)")]))
}
```

### MemoryServer struct extension
Current struct has `tool_router`, `conn`, `agent_name`. Add `agent_dir`:

```rust
#[derive(Clone)]
pub struct MemoryServer {
    tool_router: ToolRouter<Self>,
    conn: Arc<Mutex<rusqlite::Connection>>,
    agent_name: String,
    agent_dir: std::path::PathBuf,   // NEW
}
```

`run_memory_server()` already reads `HOME` into `home: PathBuf`. Pass `home.clone()` as `agent_dir` to `MemoryServer::new()`.

### agent_path_key derivation
[VERIFIED: `crates/rightclaw/src/mcp/detect.rs:83-87` and `crates/bot/src/telegram/handler.rs:483-487`]

```rust
let agent_path_key = self.agent_dir
    .canonicalize()
    .unwrap_or_else(|_| self.agent_dir.clone())
    .display()
    .to_string();
```

This is the established pattern used in both `detect.rs` and `handler.rs`. Use it verbatim.

### Atomic .claude.json write
[VERIFIED: `crates/rightclaw/src/mcp/credentials.rs:23-33`]

`write_json_atomic()` in `credentials.rs` uses `NamedTempFile + persist` (same-dir atomic rename). All `credentials.rs` functions call this internally — callers do not need to implement any write strategy themselves.

### mcp_list output (safe, no tokens)
[VERIFIED: `crates/rightclaw/src/mcp/detect.rs:56-64`]

`ServerStatus` has: `name`, `url`, `state` (`AuthState::Present/Missing`), `source` (`ClaudeJson/McpJson`), `kind` (`Http/Stdio`). No token field exists. MCP-NF-01 is satisfied by construction.

### mcp_auth tunnel hostname access
[VERIFIED: `crates/bot/src/telegram/handler.rs:322-386`]

Bot reads `rightclaw::config::read_global_config(home)` then accesses `global_config.tunnel.hostname`. The MCP server runs with `$HOME` = agent dir (not user's real home). Global config lives at `~/.rightclaw/config.yaml` — the user's real home, not the agent home.

Two options:
1. Read real home via `dirs::home_dir()` — this is what other callers do before HOME override. But in the MCP server, HOME is already overridden to agent dir, so `dirs::home_dir()` returns agent dir, not the real user home.
2. Read a `RC_RIGHTCLAW_HOME` env var (or similar) injected by `generate_mcp_config()` at agent launch time — this is the clean approach for headless context.

**Finding:** The current `generate_mcp_config()` in `mcp_config.rs` injects `RC_AGENT_NAME` into the env section. A `RC_RIGHTCLAW_HOME` (or `RC_CONFIG_DIR`) env var injected the same way would give `mcp_auth` access to the real config location. This is a Wave 0 gap the planner must address.

Alternatively: accept `redirect_uri` as a parameter to `mcp_auth` (agent passes it directly). This avoids env var complexity but requires the agent to know its tunnel hostname.

### Protected server check pattern
[VERIFIED: `crates/bot/src/telegram/handler.rs:516-523` and `crates/rightclaw/src/mcp/mod.rs:7`]

```rust
if server_name == rightclaw::mcp::PROTECTED_MCP_SERVER {
    return Err(McpError::invalid_params(
        format!("Cannot remove '{server_name}' — required for core functionality"),
        None,
    ));
}
```

Use `McpError::invalid_params` (not `internal_error`) since it's a user error, not a server failure.

### Anti-Patterns to Avoid

- **Reading env vars inside tool methods:** Derive agent_dir once in `run_memory_server()`, store on struct. Matches existing pattern for `agent_name` and `conn`.
- **Exposing secrets in tool output:** Never pass token values to `Content::text()`. Only confirmation strings, server names, URLs, auth state labels.
- **Blocking async in OAuth discovery:** `discover_as()` is already `async fn` — call with `.await` directly in the `async fn` tool method. No `block_in_place` needed.
- **Using `dirs::home_dir()` in tool methods:** Under HOME override, `dirs` returns agent dir. Access real home via injected env var.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead |
|---------|-------------|-------------|
| .claude.json read-modify-write | Custom JSON patch | `credentials::add_http_server_to_claude_json` / `remove_http_server_from_claude_json` |
| Atomic file write | `std::fs::write` (not atomic) | `credentials::write_json_atomic` (NamedTempFile + persist) |
| MCP server listing | Custom JSON parser | `detect::mcp_auth_status()` |
| OAuth AS discovery | Manual HTTP + parsing | `oauth::discover_as()` + `oauth::build_auth_url()` |
| PKCE generation | Custom crypto | `oauth::generate_pkce()` / `oauth::generate_state()` |
| Protected server guard | Magic string comparison | `rightclaw::mcp::PROTECTED_MCP_SERVER` constant |

## Common Pitfalls

### Pitfall 1: HOME override breaks dirs::home_dir() for config access
**What goes wrong:** `mcp_auth` calls `dirs::home_dir()` to find `~/.rightclaw/config.yaml` for the tunnel hostname, but inside the MCP server process `HOME` = agent dir, so `dirs` returns agent dir (not user's real home).
**Why it happens:** rightclaw sets `HOME` in process-compose env to the agent directory. `dirs` follows `$HOME`.
**How to avoid:** Inject `RC_RIGHTCLAW_HOME` (or `RC_CONFIG_DIR`) env var into the rightmemory env section in `generate_mcp_config()`, set to `dirs::home_dir()` value at `rightclaw up` time (before HOME override). Read it in `run_memory_server()` and pass to `MemoryServer`.
**Warning signs:** mcp_auth returns "Cannot read config.yaml" even when config exists.

### Pitfall 2: mcp_remove only targets .claude.json but server is in .mcp.json
**What goes wrong:** Agent calls `mcp_remove("notion")` but notion is in `.mcp.json` not `.claude.json`. `remove_http_server_from_claude_json` returns `ServerNotFound`. Agent is confused.
**Why it happens:** `mcp_list` shows servers from both files; `mcp_remove` only writes `.claude.json`.
**How to avoid:** The CONTEXT.md leaves this to Claude's discretion. Recommendation: `mcp_remove` targets `.claude.json` only (consistent with `mcp_add`). Return a clear error that says "server not found in .claude.json — if added via .mcp.json, edit that file directly."
**Warning signs:** `mcp_list` shows a server that `mcp_remove` cannot remove without clear error.

### Pitfall 3: tool_router macro requires all tool methods inside the same #[tool_router] impl block
**What goes wrong:** Adding tools in a separate `impl MemoryServer` block outside `#[tool_router]` means they are not registered in the router and are invisible to MCP clients.
**Why it happens:** The `#[tool_router]` macro generates the router from the methods it decorates at compile time.
**How to avoid:** All `#[tool(...)]` methods MUST be in the single `#[tool_router] impl MemoryServer` block.
**Warning signs:** Tools not appearing in `tools/list` response.

### Pitfall 4: mcp_auth returns URL but reqwest requires tokio runtime
**What goes wrong:** `discover_as()` is async and uses `reqwest::Client`. If called in a sync context or with a nested runtime, it panics.
**Why it happens:** Not applicable here — tool methods are already `async fn` inside an async rmcp handler.
**How to avoid:** Call `discover_as(...).await` directly. No special handling needed.

### Pitfall 5: MemoryServer is Clone — all new fields must be Clone
**What goes wrong:** Adding a non-Clone field to `MemoryServer` causes compile error because `#[derive(Clone)]` is on the struct.
**Why it happens:** `Clone` is derived on the struct.
**How to avoid:** `PathBuf` is `Clone`. Any additional state added must also be `Clone`.

## Code Examples

### mcp_add — full pattern
```rust
// Source: crates/bot/src/telegram/handler.rs:466-504 (adapted for MCP tool)
#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpAddParams {
    #[schemars(description = "MCP server name (identifier, e.g. 'notion')")]
    pub name: String,
    #[schemars(description = "HTTP MCP server URL (must start with https://)")]
    pub url: String,
}

#[tool(description = "Add an HTTP MCP server to this agent's .claude.json. The server becomes available after the next agent restart.")]
async fn mcp_add(
    &self,
    Parameters(params): Parameters<McpAddParams>,
) -> Result<CallToolResult, McpError> {
    let claude_json_path = self.agent_dir.join(".claude.json");
    let agent_path_key = self.agent_dir
        .canonicalize()
        .unwrap_or_else(|_| self.agent_dir.clone())
        .display()
        .to_string();

    rightclaw::mcp::credentials::add_http_server_to_claude_json(
        &claude_json_path,
        &agent_path_key,
        &params.name,
        &params.url,
    )
    .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;

    Ok(CallToolResult::success(vec![Content::text(format!(
        "Added MCP server '{}' ({}). Restart agent for it to take effect.",
        params.name, params.url
    ))]))
}
```

### mcp_remove — with protection guard
```rust
// Source: crates/bot/src/telegram/handler.rs:507-545 (adapted)
#[tool(description = "Remove an HTTP MCP server from this agent's .claude.json. The 'rightmemory' server cannot be removed.")]
async fn mcp_remove(
    &self,
    Parameters(params): Parameters<McpRemoveParams>,
) -> Result<CallToolResult, McpError> {
    if params.name == rightclaw::mcp::PROTECTED_MCP_SERVER {
        return Err(McpError::invalid_params(
            format!("Cannot remove '{}' — required for core functionality", params.name),
            None,
        ));
    }
    // ... call remove_http_server_from_claude_json
}
```

### mcp_list — serialize ServerStatus
```rust
// Source: crates/bot/src/telegram/handler.rs:239-276 (adapted for MCP tool JSON output)
let statuses = rightclaw::mcp::detect::mcp_auth_status(&self.agent_dir)
    .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;

let items: Vec<serde_json::Value> = statuses.iter().map(|s| {
    serde_json::json!({
        "name": s.name,
        "url": s.url,
        "auth": s.state.to_string(),   // "present" or "auth required"
        "source": s.source.to_string(), // ".claude.json" or ".mcp.json"
        "kind": s.kind.to_string(),     // "http" or "stdio"
    })
}).collect();
```

### mcp_auth — URL construction (Phase 1 only, no token exchange)
```rust
// Source: crates/bot/src/telegram/handler.rs:280-462 (reduced to Phase 1 scope per D-05)
// Phase 1 scope: discover AS + build URL. No DCR, no PendingAuth map.
let http_client = reqwest::Client::new();
let metadata = rightclaw::mcp::oauth::discover_as(&http_client, &server_url)
    .await
    .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
let (_, code_challenge) = rightclaw::mcp::oauth::generate_pkce();
let state = rightclaw::mcp::oauth::generate_state();
let redirect_uri = format!("https://{}/oauth/{}/callback", tunnel_hostname, agent_name);
let auth_url = rightclaw::mcp::oauth::build_auth_url(
    &metadata, &client_id, &redirect_uri, &state, &code_challenge, None,
);
Ok(CallToolResult::success(vec![Content::text(auth_url)]))
```

**Note:** D-05 says `mcp_auth` only returns the URL. The token exchange happens in the bot callback. However, without `client_id` (requires DCR or static config), `build_auth_url` cannot be called. The planner must address whether DCR (`register_client_or_fallback`) runs as part of Phase 1 or if `mcp_auth` relies on a static `clientId` param.

## Open Questions (RESOLVED)

1. **How does mcp_auth get the tunnel hostname?**
   - What we know: `HOME` is overridden to agent dir; `dirs::home_dir()` returns agent dir; global config is at `~/.rightclaw/config.yaml` (real user home).
   - What's unclear: Current `generate_mcp_config()` only injects `RC_AGENT_NAME`. No `RC_RIGHTCLAW_HOME` env var is injected.
   - Recommendation: Either (a) add `redirect_uri` as a required param to `mcp_auth` (agent passes it), or (b) inject `RC_RIGHTCLAW_HOME` env var in `generate_mcp_config()` and read it in `run_memory_server()`. Option (a) is simpler and avoids a codegen change; option (b) is cleaner for the agent UX.
   - RESOLVED: Option (b) chosen — inject `RC_RIGHTCLAW_HOME` in `generate_mcp_config()`; read in `run_memory_server()` and store as `rightclaw_home: PathBuf` field on `MemoryServer`. Plan 01 implements this in `generate_mcp_config()` and `MemoryServer::new()`.

2. **Does mcp_auth run DCR (Dynamic Client Registration) in Phase 1?**
   - What we know: `build_auth_url` requires `client_id`. Getting `client_id` requires either DCR (`register_client_or_fallback`) or a static value from `.claude.json`.
   - What's unclear: D-05 says "AS discovery + auth URL construction" — does that include DCR?
   - Recommendation: Yes, include DCR as part of Phase 1 `mcp_auth`. It is a prerequisite for building the URL. The bot does both in one operation (steps 4 and 5 at handler.rs:352-403).
   - RESOLVED: DCR is excluded. `mcp_auth` returns the AS `authorization_endpoint` URL only (from `discover_as()`). PKCE and DCR are not run because `code_verifier` cannot be shared with the bot's `PendingAuthMap` (lives in a different process). The agent returns this URL as a hint; the user triggers the full auth flow via Telegram bot which owns the PKCE state machine.

3. **Does mcp_remove target only .claude.json or also .mcp.json?**
   - CONTEXT.md leaves this to Claude's discretion.
   - Recommendation: `.claude.json` only (mirrors `mcp_add` behavior). Error message should mention ".mcp.json" as alternative if the server is not found.
   - RESOLVED: `.claude.json` only. Clear error message directs user to edit `.mcp.json` manually if needed.

## Environment Availability

Step 2.6: SKIPPED — this phase is purely code changes within the existing Rust workspace. No new external dependencies. `reqwest` (for `mcp_auth` OAuth) is already in the workspace.

## Project Constraints (from CLAUDE.md)

- Rust edition 2024
- All errors must propagate with `?` or `.map_err(...)` — no silent swallowing (`if let Err(e) = ...` without return)
- Use `format!("{:#}", e)` (alternate Display) for error chains, never `e.to_string()`
- `thiserror` for library error types; `anyhow` only in `main.rs` and tests; `miette` for `run_memory_server()` return type
- Tool methods return `Result<CallToolResult, McpError>` — use `McpError::internal_error(format!("{e:#}"), None)` for internal errors, `McpError::invalid_params(...)` for user errors
- No `Default` impl that reads environment — derive agent_dir from env in `run_memory_server()`, pass to `new()`
- `MemoryServer` is `Clone` — all struct fields must be `Clone` (`PathBuf` is `Clone`)
- All `#[tool]` methods must be in the single `#[tool_router] impl MemoryServer` block
- Tests in same file via `#[cfg(test)]` module; extract to separate file if file exceeds 800 LoC with tests >50% of content
- Workspace build: always `cargo build --workspace --debug` after changes

## Sources

### Primary (HIGH confidence)
- [VERIFIED: codebase] `crates/rightclaw-cli/src/memory_server.rs` — MemoryServer struct, tool registration pattern, run_memory_server
- [VERIFIED: codebase] `crates/rightclaw/src/mcp/credentials.rs` — add/remove/list HTTP servers in .claude.json, atomic write
- [VERIFIED: codebase] `crates/rightclaw/src/mcp/detect.rs` — mcp_auth_status(), ServerStatus, AuthState, ServerKind, ServerSource
- [VERIFIED: codebase] `crates/rightclaw/src/mcp/oauth.rs` — discover_as, build_auth_url, generate_pkce, generate_state, PendingAuth
- [VERIFIED: codebase] `crates/rightclaw/src/mcp/mod.rs` — PROTECTED_MCP_SERVER constant
- [VERIFIED: codebase] `crates/rightclaw/src/codegen/claude_json.rs` — agent_path_key derivation pattern
- [VERIFIED: codebase] `crates/bot/src/telegram/handler.rs:239-545` — canonical reference implementations for all 4 operations

### Secondary (MEDIUM confidence)
- [VERIFIED: codebase] `crates/rightclaw/src/codegen/mcp_config.rs` — env injection pattern for RC_AGENT_NAME (model for RC_RIGHTCLAW_HOME)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all libraries verified in codebase, no new deps needed
- Architecture: HIGH — direct codebase inspection of all canonical references
- Pitfalls: HIGH (pitfalls 1–3, 5) / MEDIUM (pitfall 4) — verified from source code patterns

**Research date:** 2026-04-05
**Valid until:** 2026-05-05 (stable codebase, no external API dependencies)
