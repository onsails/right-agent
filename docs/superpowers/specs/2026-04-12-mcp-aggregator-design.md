# MCP Aggregator Design Spec

## Problem

External MCP servers (Notion, GitHub, Linear, etc.) currently run inside the agent's OpenShell sandbox. Claude connects to each independently. This means:

1. **Credentials in sandbox** — OAuth tokens live in `.mcp.json` inside the sandbox, accessible to the agent
2. **Multiple connections** — each `claude -p` invocation must establish separate MCP handshakes to every server, slow and fragile
3. **Token refresh in sandbox** — refresh logic uploads updated `.mcp.json` into the sandbox

## Goal

Proxy all MCP traffic through a single rightclaw HTTP endpoint. The agent sees one MCP server (`right`) that aggregates tools from all registered MCP backends. Credentials never enter the sandbox.

## Architecture: Shared Process, Backend Abstraction

The Aggregator runs as a **shared process** (current `rightclaw memory-server-http` in process-compose), serving all agents from one HTTP endpoint on port 8100. This is the same hosting model as the current `HttpMemoryServer` — no architectural change in process topology.

```
Claude (sandbox) ──HTTP──▶ Aggregator (:8100/mcp)
                              ├── RightBackend (in-process: memory, cron, bootstrap)
                              ├── ProxyBackend("notion", upstream HTTP MCP)
                              └── ProxyBackend("github", upstream HTTP MCP)

Telegram Bot ──HTTP──▶ Aggregator (:8100/mcp)
  (for /mcp add, /mcp remove — calls rightmeta__* tools via MCP protocol)
```

Claude (from sandbox) interacts with the Aggregator through the MCP HTTP endpoint using Bearer tokens. The Telegram bot (from host) interacts with the Aggregator through an **internal REST API** on the same HTTP server — separate from the MCP endpoint, bound to localhost only.

```
Claude (sandbox) ──HTTP──▶ :8100/mcp      (MCP protocol, Bearer auth)
Telegram Bot     ──HTTP──▶ :8100/internal (REST API, localhost only)
```

No file-based IPC, no direct state mutation from the bot process.

## Core Abstraction: Enum Dispatch

No `dyn` trait objects — all backend variants known at compile time. Avoids async trait boxing overhead.

```rust
enum Backend {
    Right(RightBackend),
    Proxy(ProxyBackend),
    // Future: Sandbox(SandboxBackend) for stdio MCP in OpenShell
}

impl Backend {
    fn name(&self) -> &str { /* match dispatch */ }
    fn instructions(&self) -> Option<String> { /* match dispatch */ }
    async fn tools_list(&self) -> Result<Vec<Tool>> { /* match dispatch */ }
    async fn tools_call(&self, name: &str, args: Value) -> Result<CallToolResult> { /* match dispatch */ }
}
```

`RightBackend` and `ProxyBackend` both implement a `McpBackend` trait as a contract, but the Aggregator stores `Backend` (enum), not `Box<dyn McpBackend>`.

## Aggregator: Decomposed Structure

The Aggregator is split into three layers to avoid a God object:

```rust
/// HTTP handler + auth. Implements rmcp::ServerHandler.
/// rmcp's StreamableHttpService takes a factory closure that creates
/// a new Aggregator instance per MCP session. Shared state is captured
/// in the closure via Arc — the Aggregator struct itself does not need Clone.
struct Aggregator {
    /// Routes tool calls to backends by prefix
    dispatcher: Arc<ToolDispatcher>,

    /// Bearer token → agent name (unchanged from current architecture)
    token_map: Arc<RwLock<HashMap<String, AgentInfo>>>,
}

/// Prefix parsing + routing. Stateless logic.
struct ToolDispatcher {
    /// Per-agent backend registries
    agents: DashMap<String, BackendRegistry>,
}

/// Per-agent backend management + notifications.
struct BackendRegistry {
    /// In-process rightclaw tools (own SQLite DB, own agent dir)
    right: RightBackend,

    /// External MCP proxies — keyed by name for O(1) lookup
    proxies: HashMap<String, Arc<ProxyBackend>>,

    /// Peer handles for notify_tool_list_changed — one per active MCP session.
    /// Multiple concurrent sessions possible (cron + interactive).
    /// Keyed by session ID. Stale peers removed on notification failure.
    peers: Arc<RwLock<HashMap<String, Peer<RoleServer>>>>,

    /// Shared HTTP client for creating new ProxyBackends
    http_client: reqwest::Client,

    /// OAuth refresh scheduler channel
    refresh_tx: mpsc::Sender<RefreshMessage>,
}
```

**Layer responsibilities:**
- `Aggregator`: HTTP auth middleware, `rmcp::ServerHandler` impl, delegates everything to `ToolDispatcher`
- `ToolDispatcher`: parses `__` prefix, finds agent's `BackendRegistry`, dispatches
- `BackendRegistry`: owns backends, handles management tools (`mcp_add`, `mcp_remove`), sends notifications

### Multi-agent isolation

Each agent has its own `BackendRegistry`. Bearer token resolves agent identity (unchanged). Agent A's ProxyBackends are invisible to Agent B. One HTTP endpoint, one port, per-agent tool sets via `DashMap`.

### Request routing

**`tools/list`:**
1. RightBackend: `right.tools_list().await` — returned **unprefixed** (current tool names preserved)
2. For each ProxyBackend: `proxy.tools_list().await` — each tool prefixed as `{proxy.name()}__{tool.name}`
3. Append management tools with prefix `rightmeta__`
4. On backend error: include diagnostic tool entry `{backend}__ERROR` with error message (not silent drop — FAIL FAST)
5. Return merged list

**`tools/call`:**
```rust
if let Some((prefix, tool)) = split_prefix(tool_name) {
    // Has "__" → external backend or management
    match prefix {
        "rightmeta" => registry.handle_management_tool(tool, args).await,
        other => registry.dispatch_to_proxy(other, tool, args).await,
    }
} else {
    // No "__" → RightBackend (unprefixed)
    registry.right.tools_call(tool_name, args).await
}
```

**`initialize`:**
- Merge instructions from all backends + management section
- Replace peer handle in `BackendRegistry` (see Peer Lifecycle below)
- Report `capabilities: {tools: {listChanged: true}}`
- **Instructions size limit:** max 4000 chars per backend, 16000 chars total. Truncate with note: "[truncated — see server documentation]"

Merged instructions example:
```
RightClaw MCP Aggregator.

## Management (rightmeta)
- rightmeta__mcp_add: Add an external HTTP MCP server
- rightmeta__mcp_remove: Remove an MCP server
- rightmeta__mcp_list: List all configured MCP servers
- rightmeta__mcp_auth: Discover OAuth endpoint for a server

## Built-in tools (unprefixed)
- store_record: Store tagged records for persistent memory
- query_records: ...
...

## notion
<instructions from Notion MCP server>

## github
<instructions from GitHub MCP server>
```

## Tool Namespacing

External backend tools are prefixed with `{backend_name}__`. RightBackend tools are **unprefixed** to avoid double-stacking with Claude's MCP namespacing.

| Prefix | Source | Examples | Claude sees (with MCP namespace) |
|--------|--------|----------|----------------------------------|
| *(none)* | RightBackend (in-process) | `store_record`, `cron_create` | `mcp__right__store_record` |
| `rightmeta__` | Aggregator management | `rightmeta__mcp_add` | `mcp__right__rightmeta__mcp_add` |
| `notion__` | ProxyBackend | `notion__search` | `mcp__right__notion__search` |
| `github__` | ProxyBackend | `github__create_issue` | `mcp__right__github__create_issue` |

RightBackend tools keep their current names — no breaking change for existing agents. Only external backends and management tools get prefixed.

### Name validation

On `mcp_add`, the server name is validated:
- **Reserved names rejected:** `right`, `rightmeta` — prevents shadowing built-in backends
- **`__` in name rejected:** prevents ambiguous prefix parsing (e.g., name `no__tion` would break routing)
- **Upstream tool names containing `__`:** excluded from aggregated list at registration time. Prevents mangled routing (e.g., `notion__my__tool` would parse as prefix=`notion`, tool=`my__tool`). Excluded tools reported in `mcp_add` response: "Added server 'notion' with 15 tools. 2 tools excluded (names contain '__'): tool_a, tool_b."

## ProxyBackend

```rust
struct ProxyBackend {
    /// Server name as registered via mcp_add (e.g. "notion")
    server_name: String,

    /// Upstream MCP endpoint URL
    url: String,

    /// Cached tool list from upstream (excludes tools with __ in name)
    cached_tools: RwLock<Vec<Tool>>,

    /// Cached instructions from upstream initialize response
    cached_instructions: RwLock<Option<String>>,

    /// MCP client session to upstream
    /// Type: rmcp RunningService or equivalent handle
    client_session: RwLock<Option<ClientSession>>,

    /// Dynamic auth token — shared with DynamicAuthClient
    token: Arc<tokio::sync::RwLock<Option<String>>>,

    /// Last successful tool list refresh timestamp
    last_refresh: RwLock<Instant>,
}
```

### Credential injection: DynamicAuthClient

Wrapper around `reqwest::Client` that reads Bearer token from shared mutable state. Implements `rmcp::StreamableHttpClient` by delegating to the inner reqwest client with dynamic auth header substitution.

**Verified:** `rmcp` 1.3 implements `StreamableHttpClient` directly for `reqwest::Client` (no wrapper type). Source: `rmcp/src/transport/common/reqwest/streamable_http_client.rs`, line 46: `impl StreamableHttpClient for reqwest::Client`.

```rust
#[derive(Clone)]
struct DynamicAuthClient {
    inner: reqwest::Client,
    token: Arc<tokio::sync::RwLock<Option<String>>>,
}

impl StreamableHttpClient for DynamicAuthClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_token: Option<String>,  // ignored — dynamic token used instead
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let token = self.token.read().await.clone();
        let auth = token.map(|t| format!("Bearer {t}"));
        self.inner.post_message(uri, message, session_id, auth, custom_headers).await
    }

    // delete_session, get_stream — same pattern: read token, delegate to inner
}
```

**Why this works:** `rmcp` passes `config.auth_header.clone()` as a parameter to `StreamableHttpClient::post_message()` on every request. No additional auth logic in the transport layer. We intercept at this level — before the request goes to the network.

**Why not reconnect on refresh:** Reconnect = close session + initialize + tools/list. If a tool call is in-flight during reconnect, it fails. Dynamic auth = zero downtime token rotation.

**Monitoring ignored param:** If `_auth_token` is `Some`, emit `tracing::debug!` — detects if rmcp ever starts passing meaningful auth through this parameter.

### 401 retry logic

On upstream 401 during `tools_call`:
```
1. Re-read token from Arc<RwLock> (may have been refreshed since request was built)
2. If token changed → retry once with fresh token
3. If still 401 or token unchanged → return error:
   "Authentication required for {server_name}. Use /mcp auth {server_name} in Telegram."
```

### Required rmcp features

Add to workspace `Cargo.toml`:
```toml
rmcp = { version = "1.3", features = [
    "server",
    "transport-io",
    "transport-streamable-http-server",
    "transport-streamable-http-client",        # new
    "transport-streamable-http-client-reqwest", # new
    "macros",
] }
```

### MCP session lifecycle

MCP HTTP transport is stateful — requires `initialize` → `initialized` → then `tools/list`/`tools/call`. ProxyBackend uses `rmcp`'s `StreamableHttpClientTransport` which handles:

- Session initialization (handshake)
- `MCP-Session-Id` header management
- SSE streaming for server notifications
- Automatic session cleanup

### Upstream error handling

Error taxonomy for ProxyBackend upstream communication:

| Error | Classification | Action |
|-------|---------------|--------|
| TCP connection refused | Transient | Retry with backoff (30s, 60s, 120s). Tools temporarily removed. |
| DNS failure | Transient | Same as connection refused. |
| Timeout (30s) | Transient | Same as connection refused. |
| HTTP 401 | Auth required | 401 retry logic (see above). Mark as `needs_auth`. |
| HTTP 404 (MCP session terminated) | Session expired | Re-initialize MCP session automatically. |
| HTTP 5xx | Server error | Retry once, then treat as transient. |
| Successful response | — | Reset backoff counter. |

On transient errors during `tools_call`: return error to Claude with clear message. Do not silently swallow.

On transient errors during periodic `tools_list` refresh: keep stale cached tools (better than empty). Log warning.

### Tool list refresh

- **On mcp_add:** immediate connect + initialize + tools/list
- **Periodic:** every 30 minutes
- **On transient error:** exponential backoff (30s, 60s, 120s, max 5 min)
- **On change:** if refreshed tool list differs from cache → `peer.notify_tool_list_changed()`
- **Stale cache:** on refresh failure, keep previous cached tools rather than clearing

### Peer lifecycle (future — not implemented in v1)

`peers` field exists in `BackendRegistry` but is unused in v1 (Claude Code doesn't support SSE). When Claude Code adds persistent SSE support:

- On `initialize`: add peer to map keyed by session ID
- On `notify_tool_list_changed`: iterate all peers, notify, remove stale ones on error
- Multiple concurrent sessions supported (cron + interactive)

### Tool list updates

Claude Code CLI does **not** maintain persistent SSE connections for HTTP MCP servers. The current server runs in stateless mode (`with_stateful_mode(false)`, `with_json_response(true)`). Server-initiated notifications like `notify_tool_list_changed` cannot be delivered.

**Consequence:** after `mcp_add` or `mcp_remove`, Claude sees updated tools only on the next `claude -p` invocation (new `initialize` + `tools/list`). This is acceptable — `mcp_add` is a setup operation, not something that needs instant mid-conversation effect.

The `mcp_add` tool response should explicitly tell the agent: "Server registered. New tools will be available on your next session."

**Future:** if Claude Code adds persistent SSE support, enable `stateful_mode(true)` and `notify_tool_list_changed`. The `peers` map in `BackendRegistry` is designed for this. No architectural change needed — just a config flag.

## Internal REST API (Bot → Aggregator)

The Aggregator exposes an internal REST API for management operations from the Telegram bot process. These endpoints are **not** MCP protocol — they are plain HTTP/JSON, served on the same port (:8100) but under the `/internal` path prefix. Localhost only — OpenShell sandbox cannot reach them.

### Endpoints

**`POST /internal/mcp-add`** `{agent: str, name: str, url: str}`

Registers an external MCP server for the agent, creates ProxyBackend, connects to upstream.

| Situation | HTTP | Response |
|-----------|------|----------|
| Success + upstream reachable | 200 | `{tools_count: 15, excluded: ["a__b"]}` |
| Success + upstream unreachable | 200 | `{tools_count: 0, warning: "Server registered but currently unreachable. Tools will appear when it comes online."}` |
| Reserved name (right/rightmeta) | 400 | `{error: "reserved_name"}` |
| Name contains `__` | 400 | `{error: "invalid_name"}` |
| URL not HTTPS | 400 | `{error: "invalid_url"}` |
| URL private IP / SSRF | 400 | `{error: "ssrf_blocked"}` |
| Agent not found | 404 | `{error: "agent_not_found"}` |
| SQLite error | 500 | `{error: "internal", detail: "..."}` |

Duplicate name: upsert — disconnect old ProxyBackend, create new one with new URL.

**`POST /internal/mcp-remove`** `{agent: str, name: str}`

Removes an external MCP server for the agent.

| Situation | HTTP | Response |
|-----------|------|----------|
| Success | 200 | `{removed: true}` |
| Protected name (right/rightmeta) | 400 | `{error: "protected_server"}` |
| Server not found | 404 | `{error: "server_not_found"}` |

In-flight tool calls: `Arc<ProxyBackend>` keeps the backend alive until the call completes. Removal is safe.

**`POST /internal/set-token`** `{agent: str, server: str, access_token: str, refresh_token: str, expires_in: u64, token_endpoint: str, client_id: str, client_secret: Option<str>}`

Sets OAuth tokens for a registered MCP server. Called by the bot after OAuth callback completes.

| Situation | HTTP | Response |
|-----------|------|----------|
| Success | 200 | `{ok: true}` |
| Success but re-init fails | 200 | `{ok: true, warning: "Token set but upstream unreachable. Will retry."}` |
| Server not found | 404 | `{error: "server_not_found"}` — must `mcp-add` first |
| Agent not found | 404 | `{error: "agent_not_found"}` |

On success, the Aggregator:
1. Updates `DynamicAuthClient.token` in-memory
2. Saves full OAuth state to `oauth-state.json`
3. Starts refresh timer (expires_in - 10 min margin)
4. If ProxyBackend was in `needs_auth` state: re-initialize upstream, fetch tools

### Error propagation to Telegram

Every error from the internal API propagates to the Telegram user as a reply message. No silent failures.

```
Bot: POST /internal/mcp-add → connection refused
→ Telegram reply: "MCP aggregator is unavailable. Is the process running?"

Bot: POST /internal/mcp-add → 400 {error: "reserved_name"}
→ Telegram reply: "Cannot add 'right' — name is reserved."

Bot: POST /internal/mcp-add → 200 {tools_count: 0, warning: "..."}
→ Telegram reply: "Added 'notion'. Server currently unreachable — tools will appear when it comes online."
```

### Refresh scheduler ownership

The refresh scheduler runs **in the Aggregator process**, not the bot. The bot only passes initial tokens via `set-token`. The Aggregator:
- Owns `oauth-state.json` reads/writes
- Runs refresh timers per server
- Updates `DynamicAuthClient.token` on refresh
- On refresh failure after retries: marks server as `needs_auth`, can optionally notify bot to alert user

### Request routing (axum)

```rust
let app = axum::Router::new()
    .nest_service("/mcp", mcp_service)                    // MCP protocol (Claude)
    .route("/internal/mcp-add", post(handle_mcp_add))     // REST (bot)
    .route("/internal/mcp-remove", post(handle_mcp_remove))
    .route("/internal/set-token", post(handle_set_token))
    .layer(/* Bearer auth middleware for /mcp only */);
```

The `/internal` routes have no auth middleware — they are protected by being localhost-only. In sandbox mode, OpenShell network policy blocks agent access to localhost on the host.

## Security

### SSRF protection

On `mcp_add`, the URL is validated:

**In sandbox mode:** OpenShell network policy blocks access to internal networks. SSRF protection is defense-in-depth.

**In no-sandbox mode:** Aggregator validates URLs before connecting:
- **Block:** RFC 1918 (10.x, 172.16-31.x, 192.168.x), link-local (169.254.x), localhost (127.x), IPv6 loopback (::1)
- **Block:** non-HTTPS URLs — no exceptions. Local HTTP MCP servers are future scope (SandboxBackend).
- **Allow:** public HTTPS URLs only

### Credential isolation

- OAuth tokens live exclusively in Aggregator process memory + `oauth-state.json` on host
- `.mcp.json` in sandbox contains only `right` server entry with agent-specific Bearer token
- Agent Bearer token authenticates to Aggregator, not to upstream MCP servers
- Refresh tokens never serialized to sandbox-accessible storage
- `DynamicAuthClient` injects credentials at HTTP request time — no credential storage in MCP protocol messages

### Agent isolation

- Each agent's `BackendRegistry` is isolated by agent name in `DashMap`
- Bearer token → agent name resolution prevents cross-agent access
- ProxyBackend instances are not shared between agents
- An agent cannot enumerate or access another agent's registered MCP servers

## Persistence

### External MCP server registry: SQLite

New table in per-agent `memory.db`:

```sql
CREATE TABLE mcp_servers (
    name       TEXT PRIMARY KEY,
    url        TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

Migration added to existing `rusqlite_migration` chain.

### Edge cases

- **Duplicate `mcp_add`:** `INSERT OR REPLACE` (upsert). If name exists, update URL, disconnect old ProxyBackend, create new one, re-connect.
- **`mcp_add` with unreachable server:** persist to SQLite (registration = intent). Return success with warning: "Server registered but currently unreachable. Tools will appear when it comes online." Start periodic retry.
- **Bot restart with expired tokens:** on startup, if ProxyBackend initialize fails with 401, mark as `needs_auth`, exclude tools from aggregated list. Notify via Telegram that re-auth is needed.

### OAuth state: unchanged

`oauth-state.json` per agent on host. Contains per-server: `refresh_token`, `token_endpoint`, `client_id`, `client_secret`, `expires_at`, `server_url`.

### .mcp.json: simplified

Always contains exactly one entry — the `right` aggregator:

```json
{
  "mcpServers": {
    "right": {
      "type": "http",
      "url": "http://host.docker.internal:8100/mcp",
      "headers": { "Authorization": "Bearer <agent-token>" }
    }
  }
}
```

No-sandbox mode: `http://127.0.0.1:8100/mcp`. Same structure.

External server entries **never** appear in `.mcp.json`. Tokens **never** enter the sandbox.

### What changes in existing code

| Component | Current | New |
|-----------|---------|-----|
| `credentials.rs` | Read/write `.mcp.json` for externals | Read/write `mcp_servers` SQLite table |
| `mcp_config.rs` | Generates `.mcp.json` with right + externals | Generates `.mcp.json` with only right (simplified) |
| `refresh.rs` | In bot process, writes Bearer to `.mcp.json` + uploads to sandbox | Moves to Aggregator process. Updates `DynamicAuthClient.token` in-memory + `oauth-state.json` |
| `memory_server_http.rs` | `HttpMemoryServer` with 19 tools | Replaced by `Aggregator` (decomposed into 3 structs) |
| `memory_server.rs` (stdio) | Standalone stdio server | Deprecated — not used in production (bot always HTTP) |
| `mcp_auth` tool | Reads server URL from `.mcp.json` | Reads from `mcp_servers` SQLite table or ProxyBackend registry |
| Telegram `/mcp add` | `add_http_server()` + upload `.mcp.json` | `POST /internal/mcp-add` to Aggregator |
| Telegram `/mcp remove` | `remove_http_server()` + upload `.mcp.json` | `POST /internal/mcp-remove` to Aggregator |
| Telegram `/mcp auth` | OAuth → Bearer to `.mcp.json` + upload | OAuth on bot → `POST /internal/set-token` to Aggregator |

## Transport: Always HTTP

Stdio MCP (`memory_server.rs`, `generate_mcp_config()`) is dead code — only used in tests, never in production. The bot always uses HTTP MCP.

Both sandbox and no-sandbox modes use HTTP transport:

| Mode | URL |
|------|-----|
| Sandbox | `http://host.docker.internal:8100/mcp` |
| No-sandbox | `http://127.0.0.1:8100/mcp` |

`memory_server.rs` marked as deprecated. Not removed in this project — not blocking.

## Data Flow

### Bot startup

```
1. rightclaw memory-server-http (shared process in process-compose)
2. For each agent (from token map):
   a. Open memory.db → SELECT * FROM mcp_servers → [(notion, url), (github, url)]
   b. Read oauth-state.json → {notion: {refresh_token, ...}, github: {...}}
   c. Create BackendRegistry:
      - RightBackend(memory.db, agent_dir)
      - For each mcp_server:
        - DynamicAuthClient(reqwest::Client, Arc<RwLock<token_from_oauth_state>>)
        - ProxyBackend(name, url, dynamic_auth_client)
        - Start refresh timer if token exists
   d. For each ProxyBackend:
      - Connect → initialize → tools/list → cache
      - If upstream down → log warning, tools = [], retry later
      - If 401 → mark needs_auth, notify via Telegram
   e. Register BackendRegistry in ToolDispatcher.agents DashMap
3. Start HTTP server on :8100
```

### Bot process generates .mcp.json

```
1. rightclaw bot --agent brain
2. Generate .mcp.json (only "right" HTTP entry pointing to :8100)
3. Upload to sandbox (if sandbox mode)
4. Start teloxide dispatcher
```

### Claude connects

```
1. claude -p inside sandbox reads .mcp.json → one server "right"
2. POST /mcp: initialize (Bearer token → agent "brain")
3. Aggregator responds:
   - instructions: merged from all backends + management section
   - capabilities: {tools: {listChanged: true}}
   - Replace peer handle in BackendRegistry
4. Claude: tools/list
   - Returns: store_record, query_records, ..., rightmeta__*, notion__*, github__*
```

### Tool call (external)

```
1. Claude → tools/call "notion__search" {query: "..."}
2. ToolDispatcher: parse prefix="notion", tool="search"
3. BackendRegistry: proxies.get("notion") → Arc<ProxyBackend>
4. ProxyBackend: forward via MCP client session
   - DynamicAuthClient injects Bearer from Arc<RwLock>
5. Notion MCP responds → result to Claude
6. On 401 → retry with refreshed token → still 401 → error to Claude
```

### Tool call (rightclaw)

```
1. Claude → tools/call "store_record" {content: "..."}
2. ToolDispatcher: no "__" prefix → RightBackend
3. BackendRegistry.right.tools_call("store_record", args)
4. In-process: SQLite insert → result
5. Return to Claude
```

### Runtime mcp_add

```
1. Claude → tools/call "rightmeta__mcp_add" {name: "linear", url: "https://..."}
2. ToolDispatcher: prefix="rightmeta" → BackendRegistry.handle_management_tool("mcp_add", args)
3. BackendRegistry:
   a. Validate name: not "right"/"rightmeta", no "__"
   b. Validate URL: SSRF checks (no RFC1918, no localhost, HTTPS only in prod)
   c. INSERT OR REPLACE INTO mcp_servers (name, url)
   d. Create DynamicAuthClient + ProxyBackend
   e. Connect → initialize → tools/list → cache
      - On failure: persist anyway, return warning, start retry
   f. proxies.insert(name, Arc::new(proxy))
   g. peer.notify_tool_list_changed() (with error logging on failure)
4. Claude receives notification → tools/list → sees linear__* tools
```

### Runtime mcp_remove

```
1. Claude → tools/call "rightmeta__mcp_remove" {name: "notion"}
2. BackendRegistry:
   a. Validate: name != "right", name != "rightmeta"
   b. proxies.remove("notion") → Arc<ProxyBackend> dropped
      (if in-flight tools_call holds Arc clone, it completes before drop)
   c. DELETE FROM mcp_servers WHERE name = "notion"
   d. Send RefreshMessage::RemoveServer to refresh scheduler
   e. peer.notify_tool_list_changed()
3. Claude receives notification → tools/list → notion tools gone
```

### Telegram /mcp add

```
1. User sends "/mcp add notion https://mcp.notion.com/mcp" in Telegram
2. Bot: POST /internal/mcp-add {agent: "brain", name: "notion", url: "https://..."}
3. Aggregator:
   a. Validate name + URL
   b. INSERT OR REPLACE INTO mcp_servers
   c. Create DynamicAuthClient + ProxyBackend
   d. Connect → initialize → tools/list → cache (or fail gracefully)
   e. Return {tools_count: N} or {warning: "unreachable"}
4. Bot: reply to user with result
5. Tools available to Claude on next claude -p session
```

### Telegram /mcp auth

```
1. User sends "/mcp auth notion" in Telegram
2. Bot: reads server URL from Aggregator (or local mcp_servers DB)
3. Bot: OAuth discovery → build auth URL → send to user
4. User authorizes in browser → callback to bot's OAuth server
5. Bot: exchange code for tokens (access_token, refresh_token, expires_in)
6. Bot: POST /internal/set-token {agent: "brain", server: "notion", ...all token fields}
7. Aggregator:
   a. Update DynamicAuthClient.token
   b. Save to oauth-state.json
   c. Start refresh timer
   d. Re-initialize upstream ProxyBackend (was needs_auth → now has auth)
   e. Fetch tools/list → cache
8. Bot: reply "Authenticated with notion. N tools available."
```

### Token refresh (runs in Aggregator process)

```
1. Refresh timer fires (10 min before expiry)
2. POST refresh_token to token_endpoint → new access_token
3. proxy.token.write() = Some(new_access_token)
4. Save to oauth-state.json (atomic write)
5. Reschedule next refresh timer
6. On failure: retry with backoff [30s, 60s, 120s]
7. On all retries exhausted: mark server needs_auth, log error
   (optionally: notify bot to alert user via Telegram)
8. No session restart, no sandbox upload, no notification to Claude needed
```

## Impact on System Prompt and Skills

### PROMPT_SYSTEM.md

Update to describe:
- `right` is an aggregator, tools are prefixed (`right__`, `rightmeta__`, `<server>__`)
- `rightmeta__` tools for MCP management
- Credentials not accessible to agent
- No `.mcp.json` manipulation by agent

### TOOLS.md

Bare minimum template. Agent populates it during conversations with user. Does not contain MCP tool descriptions — those come via MCP protocol (instructions + tools/list).

### Skills (rightskills)

MCP management skill updated:
- Tool names: `rightmeta__mcp_add`, `rightmeta__mcp_auth`
- Same UX: "add server, then /mcp auth in Telegram"
- Underlying mechanism changed but user-facing flow identical

### ARCHITECTURE.md

Update module map, data flow, directory layout to reflect:
- `Aggregator` / `ToolDispatcher` / `BackendRegistry` replacing `HttpMemoryServer`
- `ProxyBackend` + `DynamicAuthClient` new types
- `mcp_servers` SQLite table
- Simplified `.mcp.json` generation
- Telegram `/mcp` commands routing through MCP protocol

## Crate placement

New types live in existing crates:

| Type | Crate | Rationale |
|------|-------|-----------|
| `Aggregator`, `ToolDispatcher` | `rightclaw-cli` | Replaces `HttpMemoryServer` in same location |
| `BackendRegistry` | `rightclaw-cli` | Per-agent state management |
| `RightBackend` | `rightclaw-cli` | Extracted from current `HttpMemoryServer` tools |
| `ProxyBackend` | `rightclaw` (core) | Reusable across CLI and bot |
| `DynamicAuthClient` | `rightclaw` (core) | Transport-level, no CLI dependency |
| `McpBackend` trait | `rightclaw` (core) | Contract for backends |
| `Backend` enum | `rightclaw-cli` | Dispatch layer, knows all variants |
| `mcp_servers` migration | `rightclaw` (core) | Alongside existing migrations |

## Scope

### In scope
- Aggregator (decomposed: Aggregator + ToolDispatcher + BackendRegistry) replacing HttpMemoryServer
- RightBackend (in-process, current 19 tools)
- ProxyBackend (HTTP proxy to remote MCP servers)
- DynamicAuthClient for credential injection
- SQLite `mcp_servers` table + migration
- Tool namespacing with `__` prefix
- Name validation (reserved names, `__` in names, `__` in upstream tool names)
- SSRF protection on `mcp_add` URLs
- 401 retry logic
- Upstream error taxonomy (transient/auth/session errors)
- Peer lifecycle management (replace on reconnect, graceful stale handling)
- Hot-reload via `notify_tool_list_changed`
- Telegram `/mcp` commands routed through MCP protocol
- System prompt / skills / ARCHITECTURE.md updates
- Deprecate stdio MCP server

### Out of scope (future)
- SandboxBackend for stdio MCP servers in OpenShell (third Backend enum variant)
- Per-server tool list refresh interval configuration
- Tool discovery meta-tools (Composio-style search)
- Stdio MCP removal
- Post-processing upstream instructions to prefix tool name references
