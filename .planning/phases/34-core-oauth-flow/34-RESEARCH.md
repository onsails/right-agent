# Phase 34: Core OAuth Flow + Bot MCP Commands — Research

**Researched:** 2026-04-03
**Domain:** OAuth 2.1/PKCE, axum Unix socket, cloudflared named tunnel, teloxide bot commands
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**D-01:** No CLI mcp auth — Bot is the only entrypoint. `rightclaw mcp auth <server>` does NOT exist. Only `/mcp auth <server>` via Telegram.

**D-02:** Bot commands scope — all in this phase:
- `/mcp` or `/mcp list` — list MCP servers with auth status per agent
- `/mcp auth <server>` — trigger full OAuth flow, reply with auth URL
- `/mcp add <config>` — add MCP server to agent's .mcp.json
- `/mcp remove <server>` — remove MCP server from agent's .mcp.json
- `/doctor` — run rightclaw doctor and reply with results

**D-03:** cloudflared — Named Tunnel (stable URL, NOT quick tunnel). Operator configures via `rightclaw init --tunnel-token <TOKEN> --tunnel-url <URL>`. Stored in `~/.rightclaw/config.yaml`. `rightclaw up` spawns cloudflared as a persistent process-compose entry using the token. `rightclaw doctor` checks cloudflared binary and tunnel config presence (Warn severity).

**D-04:** OAuth architecture — Option B (per-agent axum, cloudflared path routing). Each bot-process embeds its own axum callback server on a Unix socket. cloudflared routes by path prefix to the correct agent's socket. `redirect_uri = https://<tunnel-url>/oauth/<agent-name>/callback`. `rightclaw up` generates `~/.rightclaw/cloudflared-config.yml`.

**D-05:** Security model — state = 128-bit cryptographically random token. Stored in HashMap (state → PendingAuth). Constant-time comparison via `subtle`. PendingAuth consumed on first successful use (one-shot). PKCE code_verifier stored server-side only.

**D-06:** OAuth flow sequence — AS discovery (RFC 9728 → RFC 8414 → OIDC) → DCR or static clientId → PKCE → axum callback → credential write → agent restart.

**D-07:** AS discovery fallback — RFC 9728: 404 → try next, 5xx → abort. RFC 8414: 404 → try OIDC, 5xx → abort. OIDC not found → abort.

**D-08:** DCR fallback — no `registration_endpoint` → use static `clientId` from `.mcp.json`. No clientId either → abort with clear error.

**D-09:** cloudflared config generation — `rightclaw up` generates `~/.rightclaw/cloudflared-config.yml`. Format: `unix:<agent_dir>/oauth-callback.sock`.

**D-10:** REQUIREMENTS.md updates needed — OAUTH-01 superseded, OAUTH-04/05 updated, BOT-01..05 moved to Phase 34, TUNL-01 now in scope.

### Claude's Discretion
- axum port/socket binding strategy within bot process (tokio select on bot + axum)
- PendingAuth timeout (how long to wait before cleaning up)
- Exact response HTML/text returned to browser after successful callback
- `rightclaw-config.yaml` schema (tunnel_token, tunnel_url fields)
- cloudflared process name in process-compose YAML

### Deferred Ideas (OUT OF SCOPE)
- `/mcp refresh` command via bot — Phase 35
- Proactive `rightclaw up` token refresh — Phase 35
- `rightclaw doctor` MCP token warnings — Phase 35
- CLI `rightclaw mcp auth` entrypoint — eliminated, not deferred
- Anonymous quick tunnel (ephemeral URL) — eliminated in favor of named tunnel
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| OAUTH-01 | Superseded by D-01 — bot is the only entrypoint (no CLI command) | N/A — eliminated |
| OAUTH-02 | AS discovery: RFC 9728 → RFC 8414 → OIDC well-known | Discovery URL patterns documented below |
| OAUTH-03 | DCR with fallback to static clientId | RFC 7591 request format documented below |
| OAUTH-04 | cloudflared named tunnel as redirect URI; abort if absent | cloudflared 2026.3.0 available; doctor check pattern |
| OAUTH-05 | Verify tunnel reachable before presenting URL | healthcheck via reqwest GET to tunnel URL |
| OAUTH-06 | PKCE state persisted in-process; axum callback on Unix socket receives redirect | axum UDS pattern documented; PKCE generation using sha2+base64 |
| OAUTH-07 | Token written via `write_credential`; agent restarted via PC REST API | Both already exist in codebase |
| BOT-01 | `/mcp` — list MCP servers with auth status | `mcp_auth_status` already exists in `mcp::detect` |
| BOT-02 | `/mcp auth <server>` — trigger OAuth, reply with URL; confirm after | Full flow is the core of this phase |
| BOT-03 | `/mcp add <config>` — add server to `.mcp.json` | `mcp_config.rs` pattern already exists |
| BOT-04 | `/mcp remove <server>` — remove from `.mcp.json` | JSON merge pattern already established |
| BOT-05 | `/doctor` — run doctor and reply | `cmd_doctor` exists in CLI |
| TUNL-01 | Stable tunnel URL (named tunnel) | cloudflared named tunnel config documented |
</phase_requirements>

---

## Summary

Phase 34 builds on a solid foundation: credentials module (Phase 32), auth detection (Phase 33), teloxide bot (Phases 23-26), and process-compose codegen patterns are all in place. The new work is: (1) per-agent axum Unix socket callback server running alongside the teloxide dispatcher, (2) the full OAuth 2.1 discovery + DCR + PKCE engine, (3) cloudflared named tunnel config generation in `rightclaw up`, and (4) six new Telegram bot commands.

The MCP Authorization specification (modelcontextprotocol.io) defines the exact discovery order: RFC 9728 resource metadata → RFC 8414 AS metadata → OIDC discovery. The spec is authoritative and recent (draft as of 2025). Discovery URL construction has clear rules for path vs. no-path issuers. PKCE S256 is mandatory per MCP spec. DCR (RFC 7591) is the fallback when Client ID Metadata Documents are not supported.

The axum Unix socket pattern is well-established: `tokio::net::UnixListener::bind(path)` + `axum::serve(uds, app)`. The bot and axum server run concurrently via `tokio::join!` or `tokio::select!` inside the existing `run_async` function. The entire callback server shuts down after handling one OAuth callback (or on SIGTERM) using axum's `with_graceful_shutdown`.

**Primary recommendation:** Build the OAuth engine as a new module `crates/rightclaw/src/mcp/oauth.rs`, add bot commands to a new `crates/bot/src/telegram/commands/` sub-module, add cloudflared config generation to `crates/rightclaw/src/codegen/cloudflared.rs`.

---

## Standard Stack

### Core (already in workspace)
| Library | Version | Purpose | Status |
|---------|---------|---------|--------|
| tokio | 1.50 | Async runtime, UnixListener, select! | Already in workspace |
| reqwest | 0.13 | AS discovery HTTP calls, token exchange POST | Already in workspace |
| sha2 | 0.10 | PKCE S256 code_challenge computation | Already in workspace |
| hex | 0.4 | Not needed for PKCE — use base64 instead | Already in workspace |
| uuid | 1 (v4) | Available but rand preferred for PKCE state | Already in workspace |
| serde_json | 1.0 | DCR request/response, AS metadata parsing | Already in workspace |
| minijinja | 2.18 | cloudflared config template generation | Already in workspace |
| teloxide | 0.17 | Bot commands | Already in workspace |
| miette | 7.6 | User-facing errors | Already in workspace |
| thiserror | 2.0 | Structured OAuth error types | Already in workspace |

### New Deps Needed
| Library | Version | Purpose | Why |
|---------|---------|---------|-----|
| axum | 0.8 | Per-agent HTTP callback server on Unix socket | Official axum 0.8.8 on crates.io; `axum::serve` + `UnixListener` |
| subtle | 2.6 | Constant-time state token comparison | Defeats timing attacks on public callback endpoint |
| rand | 0.10 | PKCE state token generation (128-bit random) | `rand::random::<[u8; 16]>()` for state, `rand::random::<[u8; 32]>()` for code_verifier |
| base64 | 0.22 | PKCE code_challenge BASE64URL encoding | `base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)` |

**Installation:**
```bash
# Add to workspace Cargo.toml [workspace.dependencies]:
axum = "0.8"
subtle = "2.6"
rand = "0.10"
base64 = "0.22"

# Add to bot/Cargo.toml [dependencies]:
axum = { workspace = true }

# Add to rightclaw/Cargo.toml [dependencies]:
subtle = { workspace = true }
rand = { workspace = true }
base64 = { workspace = true }
```

**Version verification (confirmed from crates.io 2026-04-03):**
- axum: 0.8.8
- subtle: 2.6.1
- rand: 0.10.0
- base64: 0.22.1

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| rand + sha2 + base64 | `oauth2` crate (0.4/5.0) | oauth2 crate adds significant complexity and its own HTTP client wiring; manual PKCE is ~30 lines and we already have sha2 |
| axum UDS | tokio-listener | tokio-listener is more flexible but heavier; axum 0.8 has native UnixListener support |
| axum UDS | tiny_http | tiny_http is sync; incompatible with our async bot loop |

---

## Architecture Patterns

### Recommended Module Structure
```
crates/rightclaw/src/mcp/
├── credentials.rs     # (existing) write_credential, read_credential
├── detect.rs          # (existing) mcp_auth_status, AuthState, ServerStatus
├── oauth.rs           # NEW: OAuthEngine — AS discovery, DCR, PKCE, token exchange
└── mod.rs             # export oauth module

crates/rightclaw/src/codegen/
├── process_compose.rs # (existing)
├── cloudflared.rs     # NEW: generate_cloudflared_config(agents, tunnel_token, tunnel_url)
└── mcp_config.rs      # (existing) — extend with add_mcp_server, remove_mcp_server

crates/rightclaw/src/
├── config.rs          # extend with GlobalConfig struct (tunnel_token, tunnel_url)
└── ...

crates/bot/src/telegram/
├── dispatch.rs        # (existing) — extend BotCommand enum with /doctor; add MCP commands
├── handler.rs         # (existing) — add handle_mcp, handle_doctor
├── commands/
│   ├── mod.rs         # NEW: module for MCP commands
│   ├── mcp.rs         # NEW: handle_mcp_list, handle_mcp_auth, handle_mcp_add, handle_mcp_remove
│   └── doctor.rs      # NEW: handle_doctor
└── oauth_callback.rs  # NEW: axum Unix socket callback server

crates/rightclaw-cli/src/main.rs
# extend Init to accept --tunnel-token, --tunnel-url
# extend Up to call generate_cloudflared_config and add cloudflared to PC config
```

### Pattern 1: axum Unix Socket Callback Server

```rust
// Source: github.com/tokio-rs/axum/blob/main/examples/unix-domain-socket/src/main.rs
// and axum graceful-shutdown example

use tokio::net::UnixListener;
use axum::{Router, extract::Query};

// Socket path convention: <agent_dir>/oauth-callback.sock
pub async fn run_oauth_callback_server(
    socket_path: &Path,
    pending_auth: Arc<Mutex<HashMap<String, PendingAuth>>>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> miette::Result<()> {
    // Remove stale socket before binding
    let _ = tokio::fs::remove_file(socket_path).await;

    let uds = UnixListener::bind(socket_path)
        .map_err(|e| miette::miette!("bind oauth socket: {e:#}"))?;

    let state = Arc::new(OAuthCallbackState { pending_auth });

    let app = Router::new()
        .route("/oauth/:agent_name/callback", get(handle_oauth_callback))
        .with_state(state);

    axum::serve(uds, app)
        .with_graceful_shutdown(async { let _ = shutdown_rx.await; })
        .await
        .map_err(|e| miette::miette!("oauth callback server: {e:#}"))
}
```

**Key constraint:** axum 0.8 `axum::serve()` accepts `tokio::net::UnixListener` directly — no wrapper needed. Confirmed by official example.

### Pattern 2: Concurrent Bot + OAuth Server

```rust
// In bot lib.rs run_async(), after resolving agent config:
let (oauth_shutdown_tx, oauth_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
let oauth_socket = agent_dir.join("oauth-callback.sock");
let pending_auth: Arc<Mutex<HashMap<String, PendingAuth>>> = Arc::new(Mutex::new(HashMap::new()));

tokio::select! {
    result = run_telegram(token, allowed_ids, agent_dir.clone(), debug, Arc::clone(&pending_auth)) => result?,
    result = run_oauth_callback_server(&oauth_socket, Arc::clone(&pending_auth), oauth_shutdown_rx) => result?,
}
// When teloxide dispatcher shuts down → select! exits → oauth server naturally stops
// (shutdown_tx dropped → receiver sees disconnect)
```

### Pattern 3: PKCE S256 Generation

```rust
// Source: RFC 7636 §4.1, MCP authorization spec
// MCP spec REQUIRES S256. plain is forbidden.

use rand::RngCore;
use sha2::{Digest, Sha256};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

fn generate_pkce() -> (String, String) {
    // code_verifier: 32 random bytes = 43 base64url chars (within 43-128 range)
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(&bytes);

    // code_challenge = BASE64URL(SHA256(ASCII(code_verifier)))
    let hash = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(&hash);

    (code_verifier, code_challenge)
}

fn generate_state() -> String {
    // 128-bit = 16 random bytes
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(&bytes)
}
```

### Pattern 4: Constant-Time State Comparison

```rust
// Source: subtle crate docs — prevents timing attacks on public callback
use subtle::ConstantTimeEq;

fn verify_state(expected: &str, received: &str) -> bool {
    expected.as_bytes().ct_eq(received.as_bytes()).into()
}
```

**Important:** `ct_eq` requires equal-length slices to be meaningful. For unequal lengths, subtle still returns false but timing is not guaranteed constant. The typical practice is to compare length first (in constant time) or just let ct_eq handle it — in this case, state tokens are always fixed-length base64url output so lengths will match when valid.

### Pattern 5: MCP AS Discovery Sequence

```
Given server URL: https://mcp.notion.com/mcp

Step 1 — RFC 9728 resource metadata:
  URL has path "/mcp" → try:
    GET https://mcp.notion.com/.well-known/oauth-protected-resource/mcp
  Parse response for "authorization_servers" array.
  404 → continue to Step 2. 5xx → abort.

Step 2 — RFC 8414 AS metadata (using AS URL from Step 1, or fall through):
  If AS URL has path (e.g. https://auth.notion.com/tenant1) → try in order:
    1. GET https://auth.notion.com/.well-known/oauth-authorization-server/tenant1
    2. GET https://auth.notion.com/.well-known/openid-configuration/tenant1
    3. GET https://auth.notion.com/tenant1/.well-known/openid-configuration
  If AS URL has NO path (e.g. https://api.notion.com) → try:
    1. GET https://api.notion.com/.well-known/oauth-authorization-server
    2. GET https://api.notion.com/.well-known/openid-configuration
  404 on all → abort. 5xx → abort.

Step 3 — Extract from AS metadata:
  authorization_endpoint (required)
  token_endpoint (required)
  registration_endpoint (optional — DCR)
  code_challenge_methods_supported (MUST include "S256" per MCP spec)
  client_id_metadata_document_supported (optional)
```

Source: [MCP Authorization spec](https://modelcontextprotocol.io/specification/draft/basic/authorization), [RFC 9728](https://datatracker.ietf.org/doc/html/rfc9728), [RFC 8414](https://datatracker.ietf.org/doc/html/rfc8414)

### Pattern 6: DCR Request (RFC 7591)

```rust
// POST to registration_endpoint
// Content-Type: application/json
// Response: 201 Created with client_id (and optionally client_secret)

let dcr_body = serde_json::json!({
    "client_name": "RightClaw",
    "redirect_uris": [redirect_uri],
    "grant_types": ["authorization_code"],
    "response_types": ["code"],
    "token_endpoint_auth_method": "none",
    "application_type": "native"  // CLI/desktop per MCP spec
});
```

### Pattern 7: cloudflared Config Generation

```yaml
# ~/.rightclaw/cloudflared-config.yml — generated by rightclaw up
tunnel: <TUNNEL_ID_OR_NAME>
credentials-file: /path/to/credentials.json

ingress:
  - hostname: <tunnel-url>
    path: /oauth/right/callback
    service: unix:/home/wb/.rightclaw/agents/right/oauth-callback.sock
  - hostname: <tunnel-url>
    path: /oauth/scout/callback
    service: unix:/home/wb/.rightclaw/agents/scout/oauth-callback.sock
  - service: http_status:404
```

**Critical:** Catch-all rule `service: http_status:404` is REQUIRED by cloudflared — omitting it makes the ingress config invalid and cloudflared refuses to start.

**Running with token (no credentials-file needed):**
```bash
cloudflared tunnel run --token <TOKEN>
```
When using `--token`, cloudflared embeds the tunnel ID internally — no separate `credentials-file` needed. The config file is for ingress rules only.

**Process-compose entry:**
```yaml
cloudflared:
  command: "cloudflared tunnel --config ~/.rightclaw/cloudflared-config.yml run --token $RC_CLOUDFLARED_TOKEN"
  environment:
    RC_CLOUDFLARED_TOKEN: "{{ tunnel_token }}"
```

### Pattern 8: GlobalConfig schema

```yaml
# ~/.rightclaw/config.yaml (new file, written by rightclaw init --tunnel-token ... --tunnel-url ...)
tunnel:
  token: "<CLOUDFLARE_TUNNEL_TOKEN>"
  url: "rightclaw.example.com"  # operator-managed subdomain
```

```rust
// crates/rightclaw/src/config.rs
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct GlobalConfig {
    pub tunnel: Option<TunnelConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TunnelConfig {
    pub token: String,
    pub url: String,
}
```

### Pattern 9: Bot Command Routing

Extend `BotCommand` enum in `dispatch.rs`. Currently has `Start` and `Reset`. Add `Mcp` and `Doctor`:

```rust
#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum BotCommand {
    Start,
    Reset,
    Mcp { args: String },  // subcommand parsing done in handler
    Doctor,
}
```

**Important:** teloxide's `#[command]` derive does not support sub-subcommands. Parse the args string manually in `handle_mcp`:
- empty / "list" → list command
- "auth <server>" → auth command
- "add <config>" → add command
- "remove <server>" → remove command

### Anti-Patterns to Avoid

- **Do not use `axum::Server::bind` with TCP** — this phase requires Unix sockets for cloudflared routing
- **Do not store PendingAuth in a DB** — in-memory HashMap is correct; state is per-process, short-lived
- **Do not share pending_auth across agents** — each bot process has its own HashMap; no cross-agent state
- **Do not use `serde_json::json!` for DCR body field ordering** — DCR body fields have no ordering requirement (unlike CC credential key); `serde_json::json!` is fine here
- **Do not run cloudflared with `--config` AND `--token`** together — `--token` mode is self-contained; the config file is for ingress rules only, passed via `--config`

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| PKCE S256 challenge | custom SHA+base64 | sha2 + base64 (already in workspace) | One-liner; wrong encoding (URL_SAFE vs. STD) is a subtle bug |
| Random state tokens | custom PRNG | rand 0.10 `fill_bytes` | OS-entropy CSPRNG; thread_rng is sufficient for non-key material |
| Constant-time comparison | `==` on strings | subtle 2.6 `ct_eq` | `==` short-circuits on first byte mismatch → timing oracle |
| AS metadata parsing | manual JSON field extraction | serde + typed struct | Missed fields, wrong types |
| Unix socket cleanup | manual | `let _ = tokio::fs::remove_file(&path).await` before bind | Otherwise bind fails if previous process died without cleanup |

---

## Common Pitfalls

### Pitfall 1: cloudflared ingress with no catch-all rule
**What goes wrong:** cloudflared refuses to start, logs `"no rule matched"` validation error.
**Why it happens:** cloudflared requires a catch-all `- service: http_status:404` as the final ingress entry.
**How to avoid:** Always include it last in the generated template.
**Warning signs:** cloudflared process exits immediately with non-zero exit code.

### Pitfall 2: Unix socket left from previous run
**What goes wrong:** `UnixListener::bind` returns `EADDRINUSE` on restart.
**Why it happens:** Unix sockets are filesystem entries and persist after process death.
**How to avoid:** Always `tokio::fs::remove_file(&socket_path).await.ok()` before bind.
**Warning signs:** OAuth callback server fails to start on second `rightclaw up`.

### Pitfall 3: PKCE code_challenge using wrong base64 variant
**What goes wrong:** Authorization server rejects code_verifier at token exchange.
**Why it happens:** RFC 7636 mandates `BASE64URL` (no padding, `-` and `_` chars). Standard base64 (`+`, `/`, `=`) will fail.
**How to avoid:** Use `base64::engine::general_purpose::URL_SAFE_NO_PAD` explicitly.
**Warning signs:** Token exchange returns `invalid_grant` or `code_verifier mismatch`.

### Pitfall 4: Blocking the bot while axum handles OAuth callback
**What goes wrong:** Bot stops responding to Telegram messages during OAuth flow.
**Why it happens:** Awaiting axum serve inside bot task.
**How to avoid:** Run axum server and teloxide dispatcher as sibling tasks via `tokio::select!` or `tokio::join!`.

### Pitfall 5: PendingAuth state not cleaned up on timeout
**What goes wrong:** Memory leak; stale state entries accumulate.
**Why it happens:** User clicks auth URL but never completes, or OAuth error occurs.
**How to avoid:** Add a background cleanup task that sweeps PendingAuth entries older than N minutes (10 min recommended). Use `tokio::time::interval` + sweep loop, or store `created_at: Instant` in PendingAuth.

### Pitfall 6: cloudflared named tunnel requires pre-created tunnel
**What goes wrong:** `cloudflared tunnel run --token <TOKEN>` requires the tunnel to exist in Cloudflare dashboard with the token already configured.
**Why it happens:** Named tunnels are created via Cloudflare dashboard or `cloudflared tunnel create`. The token is downloaded after creation.
**How to avoid:** Document in `rightclaw init` output that the operator must create the tunnel first and download the token. `rightclaw doctor` should check that token is non-empty.

### Pitfall 7: rand 0.10 API change from 0.8/0.9
**What goes wrong:** `rand::thread_rng()` was renamed; old API code won't compile.
**Why it happens:** rand 0.10 (current) changed the primary API. `thread_rng()` → use `rand::rng()` instead.
**How to avoid:** Use `rand::rng().fill_bytes(&mut bytes)` (rand 0.10 API). Confirmed by crates.io (rand = "0.10.0").

### Pitfall 8: MCP spec discovery: RFC 9728 path-prefix discovery
**What goes wrong:** Sending GET to wrong URL path — misses the server's metadata.
**Why it happens:** For `https://mcp.notion.com/mcp`, the RFC 9728 path is `/.well-known/oauth-protected-resource/mcp` (path appended after well-known prefix), NOT `/.well-known/oauth-protected-resource`.
**How to avoid:** Strip trailing slash from path, then concatenate: `/.well-known/oauth-protected-resource` + path.
**Warning signs:** 404 from discovery when server actually supports OAuth.

### Pitfall 9: teloxide BotCommand derive with args
**What goes wrong:** `/mcp auth notion` is not parsed as `Mcp { args: "auth notion" }`.
**Why it happens:** teloxide command parsing is literal — it captures everything after `/mcp` as a single string.
**How to avoid:** Treat `args` as a free-form string and split/parse manually in the handler.

---

## Code Examples

### OAuth Module Struct Layout

```rust
// crates/rightclaw/src/mcp/oauth.rs

#[derive(Debug)]
pub struct PendingAuth {
    pub server_name: String,
    pub server_url: String,
    pub code_verifier: String,
    pub state: String,
    pub token_endpoint: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub created_at: std::time::Instant,
}

#[derive(Debug, Deserialize)]
pub struct AsMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub code_challenge_methods_supported: Option<Vec<String>>,
    pub client_id_metadata_document_supported: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ResourceMetadata {
    pub authorization_servers: Vec<String>,
    pub scopes_supported: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct DcrResponse {
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    pub expires_in: Option<u64>,
}
```

### axum OAuth Callback Handler

```rust
// Source: axum 0.8 docs, RFC 6749 §4.1.2
use axum::{extract::{Query, Path, State}, http::StatusCode, response::Html};

#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn handle_oauth_callback(
    Path(agent_name): Path<String>,
    Query(params): Query<CallbackParams>,
    State(state): State<Arc<OAuthCallbackState>>,
) -> Result<Html<String>, StatusCode> {
    let received_state = params.state.ok_or(StatusCode::BAD_REQUEST)?;
    let code = params.code.ok_or(StatusCode::BAD_REQUEST)?;

    // Constant-time state lookup + verification
    let pending = {
        let guard = state.pending_auth.lock().await;
        guard.get(&received_state)
            .filter(|p| verify_state(&p.state, &received_state))
            .cloned()
    };

    let pending = pending.ok_or(StatusCode::BAD_REQUEST)?;

    // Remove from map (one-shot)
    state.pending_auth.lock().await.remove(&received_state);

    // Exchange code for token (spawn task so handler returns quickly)
    tokio::spawn(async move { /* token exchange + write_credential + restart */ });

    Ok(Html("<html><body>Authentication successful! You can close this window.</body></html>".to_string()))
}
```

### Global Config Read/Write

```rust
// crates/rightclaw/src/config.rs — new functions
pub fn read_global_config(home: &Path) -> miette::Result<GlobalConfig> {
    let path = home.join("config.yaml");
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| miette::miette!("read config.yaml: {e:#}"))?;
    serde_saphyr::from_str(&content)
        .map_err(|e| miette::miette!("parse config.yaml: {e:#}"))
}

pub fn write_global_config(home: &Path, config: &GlobalConfig) -> miette::Result<()> {
    // Use serde_json for output since serde-saphyr doesn't serialize to YAML
    // Alternative: use serde_yaml (deprecated) or format manually
    // Recommended: use a simple manual format since schema is small
    let path = home.join("config.yaml");
    // ... write YAML
}
```

**Note:** `serde-saphyr` is deserialize-only — it cannot serialize to YAML. For writing `config.yaml`, either write a minimal manual formatter or add a new dep (`serde_yml` 0.0.12 or similar). Given the schema is tiny (2 fields), manual format string is simplest.

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `serde_yaml` | `serde-saphyr` | March 2024 (serde_yaml deprecated) | Already using serde-saphyr in workspace |
| oauth2 crate (4.x) | Manual PKCE with sha2+base64 | N/A | oauth2 5.0-alpha adds complexity without benefit for our use case |
| `rand::thread_rng()` | `rand::rng()` | rand 0.10 (2024) | API renamed; use `rand::rng().fill_bytes()` |
| Quick tunnel (ephemeral) | Named tunnel | D-03 decision | Stable URL required for Telegram-initiated OAuth |
| DCR as primary | Client ID Metadata Documents as primary (per MCP spec) | MCP spec 2025 | MCP spec prefers CIMD; we support DCR as fallback per D-08 — acceptable since most MCP servers still use DCR |

**Deprecated/outdated:**
- `serde_yaml`: archived, must not add
- `rand 0.8/0.9 thread_rng()`: use `rand::rng()` in 0.10
- Quick tunnel: eliminated by D-03

---

## Open Questions

1. **rand 0.10 exact API**
   - What we know: version 0.10.0 on crates.io; `thread_rng()` was deprecated
   - What's unclear: exact API for `fill_bytes` in 0.10 (may be `rand::rng()` or `rand::thread_rng()`)
   - Recommendation: Check Context7 or `cargo doc --open` for rand 0.10 when implementing; use `rand::rng().fill_bytes(&mut bytes)` as starting point

2. **serde-saphyr serialization**
   - What we know: serde-saphyr is deserialize-only (designed as serde_yaml replacement for reading YAML)
   - What's unclear: best approach for writing config.yaml
   - Recommendation: Write config.yaml as a simple string template (only 2-3 fields); avoid adding another YAML serialization dep

3. **cloudflared `--token` + `--config` interaction**
   - What we know: `cloudflared tunnel run --token TOKEN` does not require a credentials-file; `--config` specifies ingress rules
   - What's unclear: whether both flags can be used together in the same invocation
   - Recommendation: Test with `cloudflared tunnel --config ~/.rightclaw/cloudflared-config.yml run --token $TOKEN`; the `--config` flag applies before the `run` subcommand

4. **PendingAuth timeout duration (Claude's Discretion)**
   - Recommendation: 10 minutes; sweep interval 1 minute via `tokio::time::interval`

5. **Browser success page content (Claude's Discretion)**
   - Recommendation: Simple HTML: "Authentication successful! You can close this window."

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| cloudflared | OAUTH-04, TUNL-01, D-03 | yes | 2026.3.0 | Doctor warns Warn severity if absent |
| axum | OAUTH-06 | not yet in workspace (new dep) | 0.8.8 | none — must add |
| rand | PKCE state generation | not yet in workspace (new dep) | 0.10.0 | uuid v4 available as fallback, but rand preferred |
| base64 | PKCE challenge encoding | not yet in workspace (new dep) | 0.22.1 | could use hex (in workspace) but wrong encoding |
| subtle | State comparison | not yet in workspace (new dep) | 2.6.1 | none — timing safety required for public endpoint |

**Missing deps — all have clear installation path (workspace dep addition), no blockers.**

---

## Sources

### Primary (HIGH confidence)
- [MCP Authorization Specification](https://modelcontextprotocol.io/specification/draft/basic/authorization) — complete OAuth discovery order, PKCE requirements, DCR registration flow (fetched 2026-04-03)
- [Cloudflare Tunnel config docs](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/local-management/configuration-file/) — ingress rules, unix socket format, catch-all requirement (fetched 2026-04-03)
- [axum unix-domain-socket example](https://github.com/tokio-rs/axum/blob/main/examples/unix-domain-socket/src/main.rs) — UnixListener::bind + axum::serve pattern (fetched 2026-04-03)
- Crates.io registry — axum 0.8.8, subtle 2.6.1, rand 0.10.0, base64 0.22.1 (verified 2026-04-03)
- Existing codebase — credentials.rs, detect.rs, process_compose.rs, pc_client.rs, dispatch.rs, handler.rs (read directly 2026-04-03)

### Secondary (MEDIUM confidence)
- [RFC 9728](https://datatracker.ietf.org/doc/html/rfc9728) — OAuth Protected Resource Metadata URL construction rules
- [RFC 8414](https://datatracker.ietf.org/doc/html/rfc8414) — AS metadata discovery URL format with/without path
- [RFC 7591](https://www.rfc-editor.org/rfc/rfc7591.html) — DCR request body fields
- WebSearch results for axum graceful shutdown, cloudflared unix socket ingress (verified against official sources)

### Tertiary (LOW confidence)
- rand 0.10 API surface (thread_rng → rng() rename) — verify when implementing

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — crate versions verified from crates.io
- Architecture: HIGH — axum UDS pattern from official example; cloudflared config from official docs; MCP spec is authoritative
- Pitfalls: HIGH — cloudflared catch-all, Unix socket cleanup, PKCE encoding are well-documented; rand API change confirmed from version bump
- OAuth spec compliance: HIGH — MCP spec read directly; discovery order and PKCE requirements are explicit

**Research date:** 2026-04-03
**Valid until:** 2026-05-03 (stable specs; MCP auth spec may evolve — check for draft updates)
