# Architecture Research

**Domain:** MCP OAuth automation for headless multi-agent runtime
**Researched:** 2026-04-03
**Confidence:** HIGH (MCP spec + CC credential storage confirmed, protocol flow verified)

## Standard Architecture

### System Overview

```
Operator runs: rightclaw mcp auth <server> [--agent <name>]
    │
    ├── 1. MCP Auth Detection
    │      Read agent .mcp.json → find remote MCP servers
    │      Probe each: GET /.well-known/oauth-authorization-server
    │      If 401 → auth required; check ~/.rightclaw/mcp-oauth/<server>.json for token
    │      Report which servers need auth
    │
    ├── 2. OAuth Flow (for unauthenticated servers)
    │      Dynamic client registration (RFC 7591): POST /register
    │      PKCE: generate code_verifier + code_challenge (SHA-256)
    │      Spawn local callback HTTP server (axum, random port)
    │      If server requires external URL: spawn ngrok tunnel
    │      Open browser with authorization URL + code_challenge + redirect_uri
    │      Wait for callback → receive auth code
    │      Exchange code for tokens (access + refresh) at /token
    │
    ├── 3. Credential Storage
    │      Write to ~/.claude/.credentials.json  ← CC's internal credential store
    │        {"mcpOAuth": {"<server>|*": {accessToken, refreshToken, expiresAt}}}
    │      This is the HOST ~/.claude path — not per-agent
    │      Agents inherit via credential symlink (already in place for v2.1+)
    │
    └── 4. Token Refresh (rightclaw mcp refresh)
           Read ~/.rightclaw/mcp-oauth/<server>.json for refresh token + expiry
           If expired: use refresh_token → POST /token → update credentials
           On-demand only — no background daemon

rightclaw up (existing cmd_up, modified)
    │
    ├── Existing: generate .mcp.json, settings.json, process-compose config
    │
    └── NEW: Pre-flight MCP auth check
           For each remote MCP server in .mcp.json:
               Check ~/.claude/.credentials.json for valid, non-expired token
               If missing or expired: print warning (non-fatal)
               If --require-auth flag: return Err (fatal)
```

### Component Responsibilities

| Component | Responsibility | Location |
|-----------|----------------|----------|
| `mcp/detect.rs` | Parse .mcp.json, probe servers for 401, classify as local/remote/authed | `crates/rightclaw/src/mcp/` |
| `mcp/oauth.rs` | Full OAuth 2.1 + PKCE flow: discovery, registration, code exchange | `crates/rightclaw/src/mcp/` |
| `mcp/callback.rs` | Local axum HTTP server for OAuth redirect capture | `crates/rightclaw/src/mcp/` |
| `mcp/tunnel.rs` | Spawn ngrok (CLI subprocess) and extract public URL | `crates/rightclaw/src/mcp/` |
| `mcp/credentials.rs` | Read/write ~/.claude/.credentials.json under mcpOAuth key | `crates/rightclaw/src/mcp/` |
| `mcp/refresh.rs` | Token expiry check, refresh_token exchange | `crates/rightclaw/src/mcp/` |
| `cmd_mcp_auth()` | CLI entry point: orchestrates detect → oauth → store | `crates/rightclaw-cli/src/main.rs` |
| `cmd_up()` (modified) | Pre-flight: warn if remote MCPs have missing/expired tokens | `crates/rightclaw-cli/src/main.rs` |
| `doctor.rs` (modified) | New DoctorCheck: validates MCP OAuth credential presence | `crates/rightclaw/src/doctor.rs` |

## Recommended Project Structure

```
crates/rightclaw/src/
├── mcp/                      # NEW module — all MCP OAuth logic
│   ├── mod.rs                # pub use, module declarations
│   ├── detect.rs             # parse .mcp.json, classify servers, probe for 401
│   ├── oauth.rs              # RFC 7591 registration + RFC 8414 discovery + PKCE flow
│   ├── callback.rs           # axum local HTTP server, one-shot receiver
│   ├── tunnel.rs             # ngrok subprocess spawn, URL extraction
│   ├── credentials.rs        # ~/.claude/.credentials.json read/write
│   └── refresh.rs            # token expiry logic, refresh_token exchange
├── doctor.rs                 # MODIFY: add check_mcp_oauth() DoctorCheck
└── lib.rs                    # MODIFY: pub mod mcp

crates/rightclaw-cli/src/
└── main.rs                   # MODIFY: add McpCommands enum, cmd_mcp_auth(),
                              #         cmd_mcp_refresh(), cmd_mcp_status()
                              #         modify cmd_up() for pre-flight check
```

### Structure Rationale

- **mcp/ as dedicated module:** OAuth is complex enough to warrant isolation. It has no circular deps on existing modules (only needs `agent::AgentDef` for path resolution). Clean boundary for testing.
- **credentials.rs separate from oauth.rs:** Credential I/O is reusable across `mcp auth`, `mcp refresh`, and `rightclaw up` pre-flight. Isolating it avoids duplication.
- **callback.rs separate from oauth.rs:** The local HTTP server is a self-contained concern. axum brings in a heavyweight dep — isolating the import keeps it contained if future refactoring moves callback to a feature flag.
- **tunnel.rs separate:** Tunnel is optional (only needed when server won't accept localhost callbacks). Separation makes it easy to skip in tests.

## Architectural Patterns

### Pattern 1: Credential Storage in ~/.claude/.credentials.json

**What:** Read and write MCP OAuth tokens directly into CC's internal credential file at the host `~/.claude` path — not the per-agent path. Use atomic file write (write to temp, rename).

**When to use:** Always. This is the only location CC itself reads for MCP OAuth tokens. Writing anywhere else (per-agent dir, rightclaw state) would require CC to be patched to read it.

**Trade-offs:**
- Pro: Agents inherit tokens via the existing `.claude/.credentials.json` credential symlink (established in v2.1 Phase 8).
- Pro: Single source of truth — no token duplication across N agent dirs.
- Pro: Token refresh by CC itself (when it eventually works) will update this same file.
- Con: All agents share the same OAuth token per MCP server. If two agents use the same MCP server as different users, this conflicts — acceptable for v3.2 scope, document as limitation.
- Con: File format is CC-internal and undocumented. Based on GitHub issue #28256 observation. Could change across CC versions.

**Credential file format** (confirmed from issue #28256):
```json
{
  "claudeAiOauth": { ... },
  "mcpOAuth": {
    "notion|*": {
      "accessToken": "...",
      "refreshToken": "...",
      "expiresAt": 1735000000000
    }
  }
}
```

Key format: `"{serverName}|*"` — literal asterisk, observed from Notion MCP.

**Implementation:**
```rust
// credentials.rs — atomic write pattern
pub fn write_mcp_token(server_name: &str, token: &McpToken) -> miette::Result<()> {
    let creds_path = dirs::home_dir()
        .ok_or_else(|| miette::miette!("cannot determine HOME"))?
        .join(".claude/.credentials.json");
    // read-modify-write under mcpOAuth key
    // write to .credentials.json.tmp, then rename (atomic on Linux/macOS)
}
```

### Pattern 2: Local axum Callback Server with One-Shot Channel

**What:** Spawn a temporary `axum` HTTP server on a random loopback port. Pass the port in the `redirect_uri`. When the OAuth callback fires, extract the `?code=` parameter, send it over a `tokio::sync::oneshot` channel, and shut down the server.

**When to use:** Every `rightclaw mcp auth` flow. The callback server lives only for the duration of the auth flow (seconds to minutes).

**Trade-offs:**
- Pro: No persistent process — clean lifecycle, no port conflicts between runs.
- Pro: axum is already a transitive dep via reqwest/tokio. Adding it explicitly is low cost.
- Pro: oneshot channel is the idiomatic Rust approach for "wait for one event then stop."
- Con: Random port means `redirect_uri` can't be pre-registered with servers that require exact URI registration. For those, the operator must configure the server to allow `http://localhost:*`. Most MCP servers with dynamic registration do allow this.

**Implementation sketch:**
```rust
// callback.rs
pub async fn run_callback_server() -> miette::Result<(u16, oneshot::Receiver<String>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = oneshot::channel::<String>();
    tokio::spawn(async move {
        // axum router: GET /callback → extract ?code, send via tx, return HTML
        // server shuts down after one successful callback (or timeout)
    });
    Ok((port, rx))
}
```

### Pattern 3: Metadata Discovery with Fallback (RFC 8414)

**What:** For each remote MCP server, attempt `GET <server_base>/.well-known/oauth-authorization-server`. If 200, extract `authorization_endpoint`, `token_endpoint`, `registration_endpoint`. If 404 or error, fall back to default paths: `/authorize`, `/token`, `/register`.

**When to use:** Always. The spec requires it. Servers like Notion use non-default paths exposed via metadata.

**Trade-offs:**
- Pro: Spec-compliant, works with all conformant MCP servers.
- Pro: Automatic — no per-server hardcoding.
- Con: One extra HTTP round-trip per auth flow. Acceptable — auth is operator-driven, not on the hot path.

### Pattern 4: ngrok as CLI Subprocess (Not Rust SDK)

**What:** When a server requires a non-localhost redirect URI (because it validates redirect URIs against a whitelist), spawn `ngrok http <port>` as a subprocess and parse the public URL from its stdout or REST API at `localhost:4040/api/tunnels`.

**When to use:** Optional — only when `--tunnel` flag is passed or when `rightclaw mcp auth` detects the server rejects localhost redirect URIs (detect via 400 response on registration with localhost URI).

**Trade-offs:**
- Pro: No ngrok API key for basic HTTP tunnels in most cases.
- Pro: ngrok CLI is already installed by many developers. Using subprocess avoids adding the `ngrok` crate as a dependency (it embeds the ngrok agent, which is large).
- Pro: Same pattern as existing use of `process-compose` as subprocess.
- Con: ngrok must be in PATH. Doctor check needed.
- Con: ngrok free tier tunnels have short lifespans and random URLs. The operator must complete auth quickly.
- Alternative if ngrok absent: print the `redirect_uri` and ask operator to run ngrok manually. This is acceptable UX since tunnel is rare (most MCP servers accept localhost).

**Subprocess pattern:**
```rust
// tunnel.rs
pub fn spawn_ngrok(port: u16) -> miette::Result<NgrokHandle> {
    // spawn: ngrok http <port> --log=stdout --log-format=json
    // parse stdout JSON lines for {"msg":"started tunnel","url":"https://..."}
    // return NgrokHandle with public_url and Child for cleanup
}
```

### Pattern 5: Non-Fatal Pre-flight in cmd_up

**What:** After MCP config generation in `cmd_up`, scan each agent's `.mcp.json` for remote servers. For each, check if a non-expired token exists in `~/.claude/.credentials.json`. If missing or expired, emit `tracing::warn!` with the server name and the fix command.

**When to use:** Always in `cmd_up`. Keeps the operator informed before agents start. Non-fatal because agents may work without OAuth for basic operations, and the operator may intentionally defer auth.

**Trade-offs:**
- Pro: Consistent with existing warning patterns (git, rg).
- Pro: Surfaces auth issues at launch time rather than when an agent silently fails to call a tool.
- Con: Warning may be noisy for setups where MCP servers are intentionally unauthenticated.

**Warning output:**
```
warn: MCP server 'notion' in agent 'right' needs OAuth authentication.
      Run: rightclaw mcp auth notion --agent right
```

## Data Flow

### rightclaw mcp auth Flow

```
rightclaw mcp auth notion --agent right
    │
    ├── detect: read ~/.rightclaw/agents/right/.mcp.json
    │     find "notion" server → url: "https://api.notion.com/mcp"
    │
    ├── probe: GET https://api.notion.com/.well-known/oauth-authorization-server
    │     → 200: {authorization_endpoint, token_endpoint, registration_endpoint}
    │
    ├── register (RFC 7591): POST https://api.notion.com/register
    │     body: {redirect_uris: ["http://127.0.0.1:<port>/callback"], client_name: "RightClaw"}
    │     → {client_id: "..."}  (no client_secret for public clients)
    │
    ├── callback server: bind 127.0.0.1:0 → port=XXXXX
    │     [if --tunnel: spawn ngrok → get public URL, use as redirect_uri instead]
    │
    ├── open browser: https://api.notion.com/authorize
    │     ?response_type=code&client_id=...&redirect_uri=http://127.0.0.1:XXXXX/callback
    │     &code_challenge=<SHA256(verifier)>&code_challenge_method=S256&state=<random>
    │
    ├── user clicks through OAuth in browser
    │
    ├── callback received: GET /callback?code=<auth_code>&state=<state>
    │     validate state (CSRF), send code over oneshot channel
    │     render "Authentication successful. Return to terminal."
    │
    ├── token exchange: POST https://api.notion.com/token
    │     body: {grant_type: authorization_code, code: <auth_code>,
    │            redirect_uri: ..., client_id: ..., code_verifier: <verifier>}
    │     → {access_token, refresh_token, expires_in}
    │
    └── write credentials: ~/.claude/.credentials.json
          mcpOAuth["notion|*"] = {accessToken, refreshToken, expiresAt}
          atomic write (tmp file + rename)
          print: "Authenticated notion. Token valid for 1h. Refresh: rightclaw mcp refresh notion"
```

### rightclaw up Pre-flight MCP Check

```
cmd_up()
    │
    ...existing steps...
    │
    ├── for each agent:
    │     generate_mcp_config() → .mcp.json  [existing]
    │     NEW: mcp::detect::check_auth_status(&agent.path)
    │           for each remote server URL in mcpServers:
    │               load ~/.claude/.credentials.json
    │               look up mcpOAuth["{server_name}|*"]
    │               if absent or expiresAt < now() + 60s:
    │                   tracing::warn!(...)
    │
    └── continue with process-compose launch [existing]
```

### Token Refresh Flow

```
rightclaw mcp refresh notion [--agent right]
    │
    ├── read ~/.claude/.credentials.json → mcpOAuth["notion|*"]
    │     if no refreshToken → error: "Re-authenticate: rightclaw mcp auth notion"
    │
    ├── probe server for token_endpoint (same discovery as auth flow)
    │
    ├── POST token_endpoint
    │     {grant_type: refresh_token, refresh_token: <stored>, client_id: ...}
    │     → {access_token, expires_in} (refresh_token may rotate)
    │
    └── write updated credentials → ~/.claude/.credentials.json
          print: "Token refreshed. Valid until <time>."
```

## Integration Points

### Credential Symlink (v2.1 Architecture)

The existing v2.1 credential symlink setup means:
```
~/.rightclaw/agents/right/.claude/.credentials.json
    → (symlink) → ~/.claude/.credentials.json
```

When `rightclaw mcp auth` writes tokens to the host `~/.claude/.credentials.json`, all agents automatically see the updated token via their symlink. **No per-agent credential writing needed.**

Confidence: HIGH — this symlink is established in v2.1 Phase 8 and is already in place.

### Per-Agent HOME Isolation Impact

Agents run with `HOME=$AGENT_DIR`. Claude Code resolves credentials from `$HOME/.claude/.credentials.json`. Since `$HOME/.claude/` is the symlinked agent `.claude/` dir, and `.credentials.json` in that dir symlinks to the host file, agents read the host OAuth tokens. This means:

- **Token sharing across agents is by design** — all agents using "notion" get the same OAuth token.
- **No agent-specific OAuth** in v3.2 — document as limitation, defer per-agent tokens to v3.3 if needed.
- **`CLAUDE_CONFIG_DIR` alternative** — CC supports this env var to redirect `.claude/` resolution. Could be used for per-agent credential isolation in future, but complicates the flow.

Confidence: HIGH — per-agent HOME isolation is documented in MEMORY.md and PROJECT.md.

### cmd_up Integration Points

| Integration | Location | Nature |
|-------------|----------|--------|
| MCP pre-flight check | `cmd_up()` after `generate_mcp_config()` | NEW: call `mcp::detect::check_auth_status()` per agent, emit warn |
| No auth-gate on launch | `cmd_up()` | Intentional: non-fatal. Use `--require-mcp-auth` flag if fatal needed |
| mcp_needs_auth_cache | NOT used in v3.2 | The `~/.claude/mcp-needs-auth-cache.json` CC file is CC-internal, tracks servers that returned 401 to CC. Not written by rightclaw. |

### Doctor Integration

New `check_mcp_oauth()` DoctorCheck for each agent's remote MCP servers:

```
  mcp-auth-notion    warn   Token missing or expired for agent 'right'
    fix: rightclaw mcp auth notion --agent right
```

Severity: `Warn` — agents still launch, but MCP tools will fail at runtime.

## New CLI Subcommand Design

```
rightclaw mcp auth <server-name> [--agent <name>] [--tunnel]
    # Complete OAuth flow for a named MCP server
    # --agent: scope to one agent (default: check all agents for this server)
    # --tunnel: spawn ngrok for non-localhost redirect_uri

rightclaw mcp refresh [<server-name>] [--agent <name>]
    # Refresh token using stored refresh_token
    # No <server-name>: refresh all expired tokens

rightclaw mcp status [--agent <name>]
    # Show auth status of all remote MCPs per agent
    # Output: table of server | status | expires_at | agent
```

**Clap enum:**
```rust
#[derive(Subcommand)]
pub enum McpCommands {
    Auth {
        server: String,
        #[arg(long)] agent: Option<String>,
        #[arg(long)] tunnel: bool,
    },
    Refresh {
        server: Option<String>,
        #[arg(long)] agent: Option<String>,
    },
    Status {
        #[arg(long)] agent: Option<String>,
    },
}
```

## New Dependencies Required

| Crate | Version | Purpose | Notes |
|-------|---------|---------|-------|
| `axum` | 0.8 | Local callback HTTP server | Already transitive via tokio ecosystem; add explicitly |
| `oauth2` | 4.x | PKCE generation, token types, type-safe flow | ramosbugs/oauth2-rs — the standard Rust OAuth2 crate |
| `open` | 5.x | Open browser cross-platform | `open::that(url)` — tiny, cross-platform browser launch |

**No ngrok crate** — spawn CLI subprocess. Avoids embedded agent binary weight.

**reqwest already present** — used for metadata discovery, registration, and token exchange HTTP calls.

## Scaling Considerations

Not applicable — local CLI tool. The auth flow runs once per server per operator session, not per agent or per request. Token sharing means N agents = 1 auth flow, not N flows.

## Anti-Patterns

### Anti-Pattern 1: Storing Tokens in Per-Agent Dir Instead of Host ~/.claude

**What people do:** Write `accessToken` to `~/.rightclaw/agents/<name>/.claude/.credentials.json` directly (the agent-local path).

**Why it's wrong:** The `.claude/` directory in the agent dir is a symlink to the host `~/.claude/`. Writing to the agent-local path writes to the host file. But if the symlink was ever not in place (e.g., before `rightclaw up` runs), you'd write to a stale or nonexistent location. Always write to the resolved host `~/.claude/.credentials.json` — the canonical path.

**Do this instead:** `dirs::home_dir().join(".claude/.credentials.json")` — use the host HOME, not the agent dir.

### Anti-Pattern 2: Background Token Refresh Daemon

**What people do:** Spawn a tokio task that polls token expiry and calls refresh on a timer.

**Why it's wrong:** (1) `rightclaw up` doesn't run as a persistent background process — it launches process-compose and exits or attaches. There's no long-lived rightclaw process to host the timer. (2) The GitHub issue #28256 shows CC itself has token refresh bugs. Adding a parallel refresh mechanism creates write races on `.credentials.json`. (3) Complexity far exceeds value for a CLI tool where the operator can run `rightclaw mcp refresh`.

**Do this instead:** On-demand refresh via `rightclaw mcp refresh`, plus a pre-flight warning in `rightclaw up` when a token is close to expiry. If CC fixes its own refresh, rightclaw's role shrinks naturally.

### Anti-Pattern 3: Launching Browser Inside the Agent Session

**What people do:** Let the CC agent handle OAuth via its `/mcp` command or by spawning a browser.

**Why it's wrong:** Agents run headless inside process-compose with `is_tty: false`. They have no terminal for interactive browser flows. CC's `/mcp auth` command is TUI-only. The entire point of `rightclaw mcp auth` is to move this operator interaction to the CLI side, before agents start.

**Do this instead:** `rightclaw mcp auth` is always run by the operator before `rightclaw up`, not inside an agent session.

### Anti-Pattern 4: Using mcp-needs-auth-cache.json as Auth State

**What people do:** Read `~/.claude/mcp-needs-auth-cache.json` to detect which servers need auth.

**Why it's wrong:** This file is written by CC *after* an agent session fails with a 401. It's a cache of past failures, not a source of truth for current auth state. In a fresh install, it doesn't exist. After a successful auth, CC doesn't always clear the cache immediately. Probing the server directly (detect.rs) is the authoritative approach.

**Do this instead:** Probe the server with a test request or metadata check in `detect.rs`. Use `.credentials.json` expiry to determine if re-auth is needed.

### Anti-Pattern 5: Hardcoding mcpOAuth Key Format

**What people do:** Hardcode the key as `"{server}|{scope}"` or `"{server}|all"` without checking the observed format.

**Why it's wrong:** The CC credential format is undocumented and observed as `"{serverName}|*"` from issue #28256. If CC changes this format in a future version, hardcoded keys won't match. Rightclaw should use `"{serverName}|*"` consistently (matching what CC expects) and document the CC version this was verified against.

**Do this instead:** Centralize the key format in `credentials.rs` as a constant `fn mcp_oauth_key(server: &str) -> String { format!("{server}|*") }`. Change in one place if CC updates the format.

## Sources

- [MCP Authorization Specification (2025-03-26)](https://modelcontextprotocol.io/specification/2025-03-26/basic/authorization) — OAuth 2.1, PKCE requirement, dynamic client registration, metadata discovery, redirect URI constraints. HIGH confidence.
- [GitHub issue #28256: MCP OAuth token refresh not persisting for Notion](https://github.com/anthropics/claude-code/issues/28256) — Confirmed `~/.claude/.credentials.json` file path and `mcpOAuth["{server}|*"]` key format. HIGH confidence.
- [Claude Code MCP docs](https://code.claude.com/docs/en/mcp) — MCP server configuration, .mcp.json format. HIGH confidence.
- [ngrok Rust SDK docs](https://ngrok.com/docs/getting-started/rust) — Confirms ngrok-rust crate exists. LOW confidence for using it (subprocess is simpler).
- [axum OAuth example](https://github.com/tokio-rs/axum/blob/main/examples/oauth/src/main.rs) — Confirms axum 0.8 pattern for OAuth callback server. HIGH confidence.
- [oauth2-rs crate](https://docs.rs/oauth2/latest/oauth2/) — PKCE support, token types, authorization code grant. HIGH confidence.
- [MCP spec: RFC 7591 dynamic registration](https://datatracker.ietf.org/doc/html/rfc7591) — Registration endpoint protocol. HIGH confidence.
- [MCP spec: RFC 8414 server metadata](https://datatracker.ietf.org/doc/html/rfc8414) — `/.well-known/oauth-authorization-server` discovery. HIGH confidence.
- RightClaw MEMORY.md — confirms credential symlink architecture (`$AGENT_DIR/.claude/.credentials.json → host ~/.claude/.credentials.json`). HIGH confidence.
- RightClaw PROJECT.md — v2.1 Phase 8 credential symlink established, per-agent HOME isolation. HIGH confidence.

---
*Architecture research for: v3.2 MCP OAuth automation*
*Researched: 2026-04-03*
