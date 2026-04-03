# Stack Research: v3.2 MCP OAuth Automation

**Domain:** MCP OAuth 2.1 automation in Rust CLI with tokio async runtime
**Researched:** 2026-04-03
**Confidence:** HIGH — credentials format confirmed from live ~/.claude/ inspection; crate choices verified against current crates.io; MCP spec read from modelcontextprotocol.io

## Scope

Delta-research for the v3.2 milestone. Covers ONLY the new crates and patterns needed
for MCP OAuth automation. Existing stack (tokio, reqwest, serde, serde_json, rusqlite,
thiserror, miette, etc.) is NOT re-evaluated.

---

## Question 1: Local HTTP Server for OAuth Callback

**Recommendation: axum 0.8**

| Option | Verdict | Reason |
|--------|---------|--------|
| axum | **USE THIS** | tokio-native async, already in RightClaw's ecosystem (tokio workspace dep), macro-free API, Tower middleware, one-shot route pattern is trivial |
| tiny_http | Avoid | Blocking/synchronous — would require `spawn_blocking` or a dedicated thread to not block tokio. Callback needs to accept exactly one request then shut down; axum's `oneshot` channel pattern does this cleanly. |
| warp | Avoid | Heavier macro API, less maintained than axum, no advantage for single-route server |
| hyper direct | Overkill | axum is the ergonomic layer on top of hyper; use axum |

axum is not yet in the workspace dependencies but tokio already is, so adding axum adds only one new direct dep (it pulls in hyper/tower transitively which are already present via reqwest).

The OAuth callback server needs to:
1. Bind to `127.0.0.1:0` (OS-assigned port) to get a random free port
2. Accept exactly one GET request with `?code=...&state=...`
3. Return a "you can close this tab" HTML response
4. Send the code through a `tokio::sync::oneshot` channel to the main OAuth flow
5. Shut down

axum handles this with a `Router`, a `oneshot::Sender` in State, and `axum::Server::bind`.

**Version:** 0.8.x (current is 0.8.6 as of early 2026). Use `axum = "0.8"` in workspace.

---

## Question 2: Tunnel Integration

**Recommendation: shell out to `cloudflared` binary (quick tunnel mode)**

There is no Cloudflare Tunnel Rust SDK. Cloudflare provides only the `cloudflared` CLI binary (Go), and no official Rust crate for tunnel management. The `cloudflared` crate on lib.rs is a thin third-party wrapper with minimal maintenance.

ngrok has an official Rust SDK (`ngrok` crate, v0.16.x), but it **requires a registered ngrok account and an authtoken**. This is a hard dependency on user infrastructure that is unsuitable as a default. The crate embeds the ngrok agent — it is not a thin HTTP client.

**Approach: shell out to `cloudflared`**

Cloudflare Quick Tunnels (`cloudflared tunnel --url http://localhost:<port>`) require:
- The `cloudflared` binary installed (via `rightclaw doctor` check)
- No account, no login, no token — completely free
- Tunnel URL is printed to stderr as `https://<random>.trycloudflare.com`

Integration pattern:
```rust
// Spawn cloudflared, parse tunnel URL from stderr
let mut child = tokio::process::Command::new("cloudflared")
    .args(["tunnel", "--url", &format!("http://127.0.0.1:{port}")])
    .stderr(Stdio::piped())
    .spawn()?;

// Parse URL from stderr lines:
// "INF +----------------------------+"
// "INF | Your quick Tunnel has been created! Visit it at (it may take a few seconds to start): |"
// "INF | https://<hash>.trycloudflare.com |"
```

Parse with a regex: `https://[a-z0-9-]+\.trycloudflare\.com` from stderr lines.

**ngrok as opt-in alternative:** If the user has an ngrok authtoken set as `NGROK_AUTHTOKEN` env var, support `ngrok` crate as a secondary option (provides stable subdomain if user has a paid plan). Gate behind a feature flag or CLI option `--tunnel ngrok|cloudflare`. Default: cloudflare.

| Option | Account needed | Rust crate | Stable URL | Verdict |
|--------|---------------|-----------|-----------|---------|
| cloudflared quick tunnel | No | None (shell out) | No (random) | **Default** |
| ngrok SDK | Yes (authtoken) | `ngrok = "0.16"` | Paid plans only | Opt-in |
| bore / rathole | Requires VPS | N/A | Self-hosted | Out of scope |

**Doctor check:** Add `cloudflared` to `rightclaw doctor` checks (Warn severity if absent — OAuth will fail without a tunnel unless callback is localhost-only). On macOS: `brew install cloudflare/cloudflare/cloudflared`. On Linux: snap or direct binary download.

---

## Question 3: OAuth 2.0 Token Management

**Recommendation: `oauth2` crate v5.0 + manual JSON persistence to ~/.claude/.credentials.json**

The `oauth2` crate (ramosbugs/oauth2-rs) is the ecosystem standard for OAuth 2.1 in Rust:
- 35M+ downloads, actively maintained
- Strongly typed (compile-time correct flows)
- PKCE built-in: `PkceCodeChallenge::new_random_sha256()`
- Async support via reqwest backend (already in workspace)
- Resource Indicators (RFC 8707) — can pass `resource` parameter manually via extra params

MCP spec mandates OAuth 2.1 + PKCE (S256 method) + Resource Indicators (RFC 8707). The `oauth2` crate covers all of this.

**Token storage: write directly to `~/.claude/.credentials.json`**

Claude Code manages OAuth tokens in `~/.claude/.credentials.json` under the `mcpOAuth` key. RightClaw must write tokens into this file so CC picks them up without requiring the user to authenticate again via `/mcp`.

Confirmed format (live inspection, 2026-04-03):
```json
{
  "mcpOAuth": {
    "<serverName>|<clientIdHash>": {
      "serverName": "notion",
      "serverUrl": "https://api.notion.com/v1/mcp",
      "accessToken": "...",
      "expiresAt": 1234567890000,
      "discoveryState": { ... },
      "clientId": "...",
      "refreshToken": "...",
      "scope": "..."
    }
  }
}
```

Notes on the format:
- Key pattern: `<serverName>|<clientIdHash>` where hash appears to be hex characters from client registration
- `expiresAt` is milliseconds since epoch (not seconds — verified: values ~1.7T range = 2024 era ms timestamps)
- `refreshToken` is optional (not all OAuth servers issue them)
- `scope` is optional
- `discoveryState` is a dict — structure TBD by inspection of a populated entry; likely caches discovery metadata to avoid re-fetching `/.well-known/oauth-protected-resource`

File merge strategy: read existing JSON, merge `mcpOAuth` key, write back. Never clobber `claudeAiOauth` or other top-level keys.

**No external token storage crate needed.** Use `serde_json::Value` for merge-safe writes. The `oauth2` crate handles in-memory token state; persistence is our responsibility.

**Token refresh:** The `oauth2` crate's `exchange_refresh_token()` method handles refresh. Check `expiresAt` before making MCP calls; if expired, refresh automatically. CC has a known bug (issue #28256) where it does NOT refresh automatically — rightclaw should own this logic proactively.

---

## Question 4: MCP Auth Detection

**No MCP client crate needed — use reqwest directly**

The MCP spec (2025-11-25 draft) defines auth detection via standard HTTP:

1. Make an unauthenticated GET or POST to the MCP server URL
2. If the server requires auth, it returns `HTTP 401 Unauthorized` with header:
   ```
   WWW-Authenticate: Bearer resource_metadata="https://..."
   ```
3. Parse `resource_metadata` URL from the header
4. GET the resource metadata document → find `authorization_servers`
5. GET `/.well-known/oauth-authorization-server` on the AS → get `authorization_endpoint`, `token_endpoint`, `code_challenge_methods_supported`

Detection algorithm:
```rust
// 1. Probe the MCP server URL (stdio servers are skipped — auth only for HTTP transport)
let resp = reqwest_client.get(&mcp_url).send().await?;
if resp.status() == StatusCode::UNAUTHORIZED {
    let www_auth = resp.headers().get("WWW-Authenticate");
    // parse resource_metadata from header value
    // proceed with OAuth flow
}
// 200 or other = no auth needed (or already authenticated via token)
```

**For .mcp.json entries with `type: "stdio"`:** Skip — MCP spec explicitly states stdio transport SHOULD NOT follow OAuth spec; credentials come from env.

**For `type: "sse"` or `type: "http"`:** Probe as above.

The `rmcp` crate (already in workspace as MCP server library) is for implementing MCP servers, not clients. It does not expose a client API useful for auth probing. Use reqwest directly.

---

## Question 5: Claude Code's mcp-needs-auth-cache.json

**Format confirmed via live inspection (2026-04-03):**

```json
{
  "<serverName>": {
    "timestamp": 1775217851548
  }
}
```

Located at: `~/.claude/mcp-needs-auth-cache.json`

- Key = MCP server name (as declared in `.mcp.json`)
- `timestamp` = milliseconds since epoch when CC determined auth is needed
- CC writes this file when it probes an MCP server and gets 401
- If rightclaw detects a server in this cache, that server definitely needs OAuth
- After successful OAuth, the entry should be removed from this cache (or CC removes it on next successful connection)

**Integration:** RightClaw can read this file as a pre-check to identify servers that CC has already flagged as needing auth. This is faster than probing each server ourselves. Cross-reference with `.mcp.json` entries to build the auth-needed list.

---

## Recommended New Crates

| Crate | Version | Purpose | Workspace? |
|-------|---------|---------|-----------|
| `axum` | `0.8` | OAuth callback HTTP server | Add to workspace |
| `oauth2` | `5.0` | PKCE + OAuth 2.1 client flow | Add to workspace |
| `open` | `5.3` | Open browser for authorization URL | Add to workspace |

**No new crates needed for:**
- Tunnel integration (shell out to `cloudflared`)
- MCP auth detection (reqwest already in workspace)
- Token storage (serde_json already in workspace)
- Async runtime (tokio already in workspace)
- HTTP client for token exchange (reqwest already in workspace)

**Avoid adding:**
- `ngrok` crate — requires user account; shell-out to cloudflared is better default
- `openidconnect` crate — overkill; CC MCP OAuth does not use OIDC in practice
- Any keychain crate — CC reads from `~/.claude/.credentials.json` on Linux; no keychain integration needed for rightclaw to write tokens

---

## Cargo.toml Additions

Add to `[workspace.dependencies]`:
```toml
axum = "0.8"
oauth2 = "5.0"
open = "5.3"
```

Add to `crates/rightclaw/Cargo.toml` `[dependencies]`:
```toml
axum = { workspace = true }
oauth2 = { workspace = true }
open = { workspace = true }
```

---

## Integration Points in Existing Codebase

| Area | What Touches It | Notes |
|------|----------------|-------|
| `cmd_up` (rightclaw/src/cmd/up.rs) | Read `mcp-needs-auth-cache.json` after writing `.mcp.json` | Check which servers need auth; surface count to user |
| New: `cmd_auth.rs` | `rightclaw auth <agent>` subcommand | Runs OAuth flow per-server that needs auth |
| `doctor.rs` | Add `cloudflared` binary check | Warn severity if absent; OAuth flows require it |
| `~/.claude/.credentials.json` | Read+merge write | Never clobber existing keys |
| `~/.claude/mcp-needs-auth-cache.json` | Read-only | Use to detect pre-identified servers needing auth |

---

## Alternatives Considered

| Category | Recommended | Alternative | Why Not |
|----------|-------------|-------------|---------|
| Local HTTP | axum 0.8 | tiny_http | Synchronous — incompatible with tokio async architecture |
| Local HTTP | axum 0.8 | warp | Less maintained, heavier macros, no advantage |
| Tunnel | cloudflared shell-out | ngrok crate | ngrok requires paid account + authtoken; bad UX as default |
| Tunnel | cloudflared shell-out | rathole | Requires self-hosted VPS; out of scope |
| OAuth client | oauth2 5.0 | openidconnect | Superset of oauth2 crate, OIDC not required for MCP OAuth |
| OAuth client | oauth2 5.0 | manual reqwest calls | Reinvents PKCE, token exchange, error handling |
| Token storage | Direct JSON write | keyring crate | CC reads plaintext `~/.claude/.credentials.json` on Linux; keychain not needed |
| MCP detection | reqwest probe | rmcp client | rmcp is server-only; no client probe capability |

---

## What NOT to Add

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `ngrok` crate (as default) | Requires account + authtoken — blocks users without ngrok accounts | `cloudflared` quick tunnel (shell-out) |
| `openidconnect` crate | OIDC is optional in MCP OAuth spec; CC MCP servers use OAuth 2.1 not OIDC | `oauth2` crate directly |
| `hyper` direct | axum wraps it; no need to use hyper directly | axum |
| `keyring` crate | CC stores tokens in plaintext JSON on Linux; macOS keychain is not used for per-agent flow | Direct `~/.claude/.credentials.json` write |
| `tungstenite` / websocket crates | Not needed; MCP auth probing uses standard HTTP | reqwest |

---

## Confidence Assessment

| Area | Confidence | Evidence |
|------|-----------|---------|
| CC credentials format | HIGH | Live inspection of `~/.claude/.credentials.json` on dev machine |
| mcp-needs-auth-cache.json format | HIGH | Live file read — format confirmed |
| MCP auth detection (401 + WWW-Authenticate) | HIGH | Official MCP spec read from modelcontextprotocol.io |
| axum as callback server | HIGH | tokio-native, ecosystem standard, widely used pattern |
| cloudflared quick tunnel | HIGH | Official docs confirm no-account quick tunnels |
| oauth2 crate PKCE/OAuth 2.1 support | HIGH | Docs.rs confirmed, v5.0 current |
| ngrok requires authtoken | HIGH | Official ngrok docs confirm mandatory authtoken |
| `expiresAt` is milliseconds | MEDIUM | Values ~1.7T consistent with ms epoch; inferred from live data but not CC source confirmed |
| `discoveryState` field structure | LOW | Present in live data as dict; internal structure not inspected (token values redacted) |

---

## Sources

- Live `~/.claude/mcp-needs-auth-cache.json` inspection — format confirmed 2026-04-03
- Live `~/.claude/.credentials.json` structure inspection — key pattern and field types confirmed 2026-04-03
- [MCP Authorization Spec](https://modelcontextprotocol.io/specification/draft/basic/authorization) — 401 detection, WWW-Authenticate header, PKCE requirements, RFC 8707 resource parameter
- [oauth2 crate docs.rs](https://docs.rs/oauth2/latest/oauth2/) — v5.0, PKCE types
- [oauth2-rs GitHub](https://github.com/ramosbugs/oauth2-rs) — maintenance status, reqwest backend
- [ngrok Rust quickstart](https://ngrok.com/docs/getting-started/rust) — authtoken requirement confirmed
- [ngrok-rust GitHub](https://github.com/ngrok/ngrok-rust) — v0.16.x current
- [Cloudflare Quick Tunnels](https://try.cloudflare.com/) — no-account quick tunnel confirmed
- [axum crates.io](https://crates.io/crates/axum) — v0.8.6 current
- [CC issue #28256](https://github.com/anthropics/claude-code/issues/28256) — refresh token bug confirming token storage format

---
*Stack research for: RightClaw v3.2 MCP OAuth Automation*
*Researched: 2026-04-03*
