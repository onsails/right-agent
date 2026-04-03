# Feature Research: MCP OAuth Automation (v3.2)

**Domain:** MCP OAuth authentication automation for headless Claude Code agents
**Researched:** 2026-04-03
**Confidence:** HIGH (spec verified from modelcontextprotocol.io; CC credential structure verified from live ~/.credentials.json; CC bugs verified from GitHub issues)

---

## Context: What This Milestone Solves

RightClaw agents run headless. When an MCP server requires OAuth, CC normally:
1. Detects it needs auth (stores server name + timestamp in `~/.claude/mcp-needs-auth-cache.json`)
2. Triggers `/mcp` interactive menu inside the session
3. Opens a browser for the user

None of this works headless. There is also a live CC bug (issues #11585, #36307, opened 2025,
still open April 2026): HTTP MCP servers requiring OAuth **never trigger the browser flow** even in
interactive mode. CC shows "Needs authentication" permanently without any path to complete it.

RightClaw needs to own the full OAuth flow externally — detect, authorize, write credentials, then
let CC pick them up without any interactive prompting.

---

## MCP OAuth Specification — Verified Facts

**Source: modelcontextprotocol.io/specification/draft/basic/authorization — HIGH confidence**

### What Flow Is Used

- **OAuth 2.1 Authorization Code Flow + PKCE (S256)** — mandatory, not optional
- **NOT** RFC 8628 Device Flow — the spec does not reference device flow at all
- Redirect URI must be `localhost` or HTTPS (spec requirement)
- `resource` parameter (RFC 8707) required in both auth request and token request
- No implicit grant (OAuth 2.1 removes it)

### How Auth Need Is Detected

MCP server returns `HTTP 401 Unauthorized` with `WWW-Authenticate` header:
```
WWW-Authenticate: Bearer resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource",
                         scope="files:read"
```

CC writes the server name + timestamp to `~/.claude/mcp-needs-auth-cache.json`:
```json
{"notion":{"timestamp":1775217851548},"claude.ai Google Calendar":{"timestamp":1775217851861}}
```
This cache records which servers need auth at the time CC last tried connecting. Rightclaw reads
this to determine what needs authorization.

### Discovery Sequence

1. Client hits MCP server → gets 401 + `resource_metadata` URL
2. Client fetches Protected Resource Metadata (RFC 9728) to find authorization server URL
3. Client fetches Authorization Server Metadata (RFC 8414 or OIDC discovery) from AS
4. Client registers itself (Client ID Metadata Document, Dynamic Client Registration, or pre-registered)
5. Client opens browser to auth URL (with PKCE code_challenge, resource parameter)
6. User authenticates → AS redirects to `redirect_uri` with authorization code
7. Client exchanges code for tokens (with code_verifier)
8. Client uses access token on subsequent MCP requests

### Client Registration — Priority Order

1. Pre-registered client_id (if known relationship with AS)
2. Client ID Metadata Document — client hosts JSON at HTTPS URL used as client_id
3. Dynamic Client Registration (RFC 7591) — POST to `/register`, get client_id back
4. Manual user input (last resort)

Most popular MCP servers (Notion, Linear, Slack, Google Workspace) support DCR or have
pre-known client registration endpoints.

### Token Storage — Verified from Live ~/.credentials.json

CC stores all OAuth credentials in `~/.claude/.credentials.json` (Linux), macOS Keychain on macOS.

**Actual structure (values redacted, keys confirmed):**
```json
{
  "claudeAiOauth": { ... },
  "mcpOAuth": {
    "<serverName>|<hash>": {
      "serverName": "notion",
      "serverUrl": "https://api.notion.com/mcp",
      "accessToken": "<token>",
      "expiresAt": <unix_ms>,
      "discoveryState": {
        "authorizationServerUrl": "https://api.notion.com",
        "resourceMetadataUrl": "https://api.notion.com/.well-known/..."
      },
      "clientId": "<id>",
      "refreshToken": "<token>",
      "scope": ""
    }
  }
}
```

Key: `<serverName>|<8-char hash>` — e.g., `"notion|eac663db915250e7"`. Hash is derived from
server URL (confirmed: same server URL produces same hash across instances).

**`expiresAt` is Unix epoch in milliseconds**, not seconds.

**Access token empty string** = auth was started but never completed (seen in Linear entry with
`accessToken: ""`). This means DCR ran but browser flow was never completed.

### Token Refresh

- CC is supposed to auto-refresh before expiry using `refreshToken`
- **Known CC bug (issues #28256, #29718, #35092):** Token refresh fails silently or not triggered
  proactively. CC only refreshes on-demand when the token has already expired.
- Refresh is standard OAuth: POST to token endpoint with `grant_type=refresh_token`
- Refresh token rotation is required by OAuth 2.1 for public clients — new access + refresh token
  issued on each refresh; old refresh token is immediately invalid
- No CC env var or hook to trigger refresh externally
- **Rightclaw must own refresh** — check `expiresAt`, call token endpoint, write updated entry
  back to `.credentials.json`

---

## Feature Landscape

### Table Stakes (Users Expect These)

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Read `mcp-needs-auth-cache.json` to detect which servers need auth | The entry point. Without knowing what needs auth, nothing else works. | LOW | File is at `~/.claude/mcp-needs-auth-cache.json` (or `$CLAUDE_CONFIG_DIR`). Must handle HOME-isolated agents (each agent has its own `$AGENT_DIR/.claude/`). |
| Cross-reference with `.mcp.json` to find server URLs | Auth cache has server names; `.mcp.json` has URLs. Need both to run discovery. | LOW | Agent's `.mcp.json` is generated by rightclaw — we already own its structure. Only `http`/`https` transport servers need OAuth; `stdio` servers use env credentials per MCP spec. |
| OAuth Authorization Server discovery (RFC 9728 + RFC 8414) | Spec-required. Must handle both `WWW-Authenticate` header path and well-known URI fallback. Also OIDC discovery. | MEDIUM | Three well-known URIs to try in priority order per spec. Each popular service (Notion, Linear, Slack) has different AS URL patterns. |
| Local callback HTTP server on `localhost` | OAuth 2.1 requires redirect URI to be localhost or HTTPS. Headless = no browser on remote hosts; callback must be captured locally. | MEDIUM | Bind to random port (or configurable), register `http://localhost:<port>/callback` as redirect URI. Axum or hyper in Rust. Must handle the code parameter from redirect. |
| Open browser for user authorization | OAuth requires human approval. Even headless agents need one-time human authorization per service. Rightclaw cannot bypass this. | LOW | `open::that()` crate or `xdg-open`/`open` command. Print URL to terminal as fallback. User authorizes; AS redirects to localhost callback. |
| Exchange authorization code for tokens (with PKCE) | Core OAuth step. Must include code_verifier, resource parameter, client_id. | MEDIUM | POST to token endpoint. Handle Dynamic Client Registration first if no client_id stored. |
| Write token to `~/.claude/.credentials.json` under `mcpOAuth.<key>` | CC reads this file to get its tokens. If rightclaw doesn't write here, CC still sees "needs auth". | MEDIUM | Must merge into existing JSON (not overwrite). Key format: `<serverName>\|<hash>`. Hash derivation must match CC's algorithm or be verified empirically. |
| `rightclaw mcp-auth <agent>` subcommand | User-facing entry point. Discovers what needs auth, runs the flow, writes tokens. | LOW | Top-level UX. Lists what needs auth, prompts user to confirm, runs flow per server. |

### Differentiators

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Token refresh detection and execution | CC's refresh is buggy. Rightclaw proactively checks `expiresAt` in `.credentials.json` and refreshes tokens before they expire. Agents never hit auth failures mid-task. | MEDIUM | Run as part of `rightclaw up` pre-flight or as separate `rightclaw mcp-refresh` command. POST to token endpoint with `grant_type=refresh_token`. Handle rotation (update both access + refresh token). |
| Tunnel integration for external redirect URIs | Some OAuth providers reject `localhost` redirect URIs (Notion requires HTTPS). Rightclaw can spin up a Cloudflare/ngrok tunnel automatically, get a stable HTTPS URL, register it, complete flow. | HIGH | ngrok-rs or cloudflared-rs integration. Tunnel URL must be used as redirect_uri in DCR registration. URL changes per session unless using paid static domain. Cloudflare Tunnel with stable domain is preferable for repeatability. |
| Pre-flight auth check in `rightclaw up` | Before starting agents, `rightclaw up` checks which servers need auth and warns the operator. Prevents agents from starting in a state where MCP tools are silently unavailable. | LOW | Read auth cache + credentials, cross-check expiry. Print "Warning: Notion needs auth — run `rightclaw mcp-auth <agent>` first". |
| Dynamic Client Registration support | Many popular MCP servers (Notion, Linear, Google Workspace) use DCR. Without it, user must manually register an OAuth app. With it, rightclaw registers itself automatically per-server. | MEDIUM | POST to registration endpoint from AS metadata. Store `clientId` (and optionally `clientSecret` for confidential clients) in rightclaw's own state, not in CC's credential file. |
| `rightclaw doctor` checks MCP auth status | Doctor reports which agents have expired or missing MCP tokens. Surfaces before agent failure, not after. | LOW | Warn severity. Read `mcpOAuth` from `.credentials.json`, check `expiresAt` < now + buffer. |

### Anti-Features

| Anti-Feature | Why Requested | Why Avoid | Alternative |
|--------------|---------------|-----------|-------------|
| Device Flow (RFC 8628) | "Headless-friendly — no redirect needed" | MCP spec does not include device flow. No popular MCP server (Notion, Linear, Slack) supports it. Building device flow support would only work for custom servers that implement it. | Authorization Code + PKCE with localhost callback is spec-compliant and works with all production MCP servers. |
| Storing tokens in rightclaw's own credential store | "Don't touch CC's internal files" | If tokens aren't in `~/.claude/.credentials.json`, CC won't use them. CC reads only its own credential file. Rightclaw would own tokens but agents would still fail. | Write to CC's credential file. This is the only supported path. File format is verified from live system. |
| Automatic browser opener in headless CI | "Automate everything" | OAuth requires human consent. No browser in CI means the flow cannot complete. Automated token injection from static credentials is an OAuth violation and breaks token rotation. | In CI, pre-populate credentials via `rightclaw mcp-auth` in development, then copy credentials to CI environment as a secret. Rightclaw handles refresh from that point forward. |
| Polling for callback (no local server) | "Simpler than running HTTP server" | Polling requires public URL. Local callback server is simpler, more reliable, and spec-compliant. Redirect URI is `localhost` which all spec-compliant servers must accept. | Local callback HTTP server — one tokio task, one endpoint, shut down after single request. |
| Tunnel by default | "Always works, even behind NAT" | Most OAuth providers accept `localhost` redirect URIs. Tunnel introduces complexity, external dependency, and ephemeral URLs that break registered redirect URIs. | Only use tunnel when provider explicitly rejects localhost (detected from DCR error response or documented provider requirement). |

---

## Feature Dependencies

```
[Read mcp-needs-auth-cache.json]
    └──required by──> [Know which servers need auth]
                          └──required by──> [OAuth discovery per server]
                          └──required by──> [Pre-flight check in `rightclaw up`]
                          └──required by──> [Doctor auth status check]

[Cross-reference with .mcp.json]
    └──required by──> [Get server URL for discovery]
    └──parallel to──> [Read auth cache]

[Authorization Server Discovery]
    └──required by──> [Dynamic Client Registration]
    └──required by──> [Open browser with auth URL]
    └──required by──> [Token exchange]
    └──required by──> [Token refresh]

[Dynamic Client Registration]
    └──required by──> [Browser auth (need client_id for auth URL)]
    └──skipped if──> [client_id already stored from previous DCR]

[Local callback HTTP server]
    └──required by──> [Receive authorization code from AS redirect]
    └──parallel with──> [Browser opener]

[Open browser with auth URL]
    └──required by──> [User authorization step]
    └──depends on──> [PKCE code_challenge generated]
    └──depends on──> [client_id from DCR or pre-registration]

[Authorization code exchange]
    └──required by──> [Get access + refresh tokens]
    └──depends on──> [Authorization code from callback server]
    └──depends on──> [PKCE code_verifier stored from earlier]

[Write to ~/.credentials.json]
    └──required by──> [CC picks up tokens and uses MCP server]
    └──depends on──> [Access token from exchange]
    └──must merge──> [Existing mcpOAuth entries not overwritten]

[Token refresh]
    └──depends on──> [refreshToken in credentials.json]
    └──depends on──> [Token endpoint URL from stored discoveryState]
    └──independent of──> [Browser, callback server — no user interaction needed]
    └──writes back to──> [credentials.json same as initial write]

[Tunnel integration]
    └──depends on──> [Local callback server (tunnel forwards to it)]
    └──only needed when──> [Provider rejects localhost redirect URI]
    └──enhances──> [Authorization flow for restrictive providers]
```

### Dependency Notes

- **DCR before browser auth:** Client must have a `client_id` to construct the authorization URL. DCR must succeed first. If DCR is already done (client_id stored), skip.
- **Callback server parallel with browser:** Server must be listening before browser opens, or the redirect arrives with no one home.
- **Hash derivation is critical:** The key `<serverName>|<hash>` in `mcpOAuth` must match what CC expects. Hash must be verified empirically against a known-good credential entry (confirmed: Notion hash is `eac663db915250e7` for `https://api.notion.com/mcp`). Wrong hash = CC ignores the entry.
- **Credentials.json merge is critical:** File contains `claudeAiOauth` (CC's own login). Overwriting it logs the user out. Must read-modify-write only the `mcpOAuth` section.
- **Token refresh does not depend on user:** Refresh is safe to run unattended. Only the initial authorization requires user interaction.

---

## MVP Definition

### Launch With (v3.2)

Minimum to make MCP OAuth work for agents with the most popular servers (Notion, Linear):

- [ ] Read `mcp-needs-auth-cache.json` — detect which servers need auth per agent
- [ ] Cross-reference with agent's `.mcp.json` — get server URLs
- [ ] Authorization Server discovery — RFC 9728 + RFC 8414 + OIDC (all three endpoints)
- [ ] Dynamic Client Registration — works for Notion, Linear, Google Workspace
- [ ] PKCE code_challenge/code_verifier generation (S256)
- [ ] Local callback HTTP server (tokio/axum, single request, then shut down)
- [ ] Browser opener via `open` crate with terminal URL fallback
- [ ] Token exchange with PKCE verifier and resource parameter
- [ ] Write tokens to `~/.claude/.credentials.json` — merge into `mcpOAuth`, correct key format
- [ ] `rightclaw mcp-auth <agent>` subcommand — orchestrates full flow
- [ ] Pre-flight warning in `rightclaw up` — report servers needing auth, don't block startup

### Add After Validation (v3.2.x)

- [ ] Token refresh command — `rightclaw mcp-refresh <agent>` — run automatically from `rightclaw up`
- [ ] Doctor check for expired/missing MCP tokens — Warn severity
- [ ] `rightclaw doctor` reports per-agent MCP auth status

### Future Consideration (v3.3+)

- [ ] Tunnel integration — only for providers that reject localhost (Slack enterprise, some Google Workspace configs)
- [ ] Automated refresh on schedule — periodic background refresh before expiry via cron task or rightclaw up hook
- [ ] Multi-agent credential sharing — right now each agent has isolated HOME; shared OAuth tokens need explicit opt-in (SEED-004 territory)

---

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Auth detection (cache + .mcp.json cross-ref) | HIGH | LOW | P1 |
| AS discovery (RFC 9728 + RFC 8414) | HIGH | MEDIUM | P1 |
| Dynamic Client Registration | HIGH | MEDIUM | P1 |
| PKCE generation | HIGH | LOW | P1 |
| Local callback server | HIGH | MEDIUM | P1 |
| Browser opener | HIGH | LOW | P1 |
| Token exchange | HIGH | MEDIUM | P1 |
| Write to .credentials.json (correct key format) | HIGH | MEDIUM | P1 |
| `rightclaw mcp-auth` subcommand | HIGH | LOW | P1 |
| Pre-flight warning in `rightclaw up` | MEDIUM | LOW | P1 |
| Token refresh | HIGH | MEDIUM | P2 |
| Doctor MCP auth check | MEDIUM | LOW | P2 |
| Tunnel integration | MEDIUM | HIGH | P3 |

---

## Known CC Bugs Affecting This Feature (as of April 2026)

These are bugs in CC that rightclaw must work around, not fix:

| Bug | Issue | Impact on RightClaw |
|-----|-------|---------------------|
| HTTP MCP servers never trigger browser OAuth flow | #11585, #36307 | Core motivation for this milestone. RightClaw owns the flow entirely. |
| After token revocation, `/mcp` menu doesn't show re-auth needed | #30272 | Auth cache file (`mcp-needs-auth-cache.json`) may not be updated on revocation. RightClaw should also check `expiresAt` and empty `accessToken` in credentials as signals. |
| OAuth auth succeeds but MCP reconnection fails, requires restart | #10250 | After rightclaw writes tokens, may need to restart CC session for it to pick them up. `rightclaw mcp-auth` should note this. |
| Token refresh not triggered proactively | #28256, #29718 | Rightclaw's refresh command is the workaround. Run from `rightclaw up` pre-flight. |
| Token refresh fails due to Keychain permission errors (macOS) | #19456 | On macOS, Keychain may block programmatic access. RightClaw reads/writes `~/.claude/.credentials.json` directly on Linux; macOS path may need separate investigation. |

---

## Sources

- [MCP Authorization Specification](https://modelcontextprotocol.io/specification/draft/basic/authorization) — OAuth 2.1 flow, PKCE requirements, discovery sequence (HIGH)
- [CC issue #11585](https://github.com/anthropics/claude-code/issues/11585) — HTTP MCP servers never trigger OAuth browser flow (HIGH)
- [CC issue #36307](https://github.com/anthropics/claude-code/issues/36307) — duplicate of #11585, still open April 2026 (HIGH)
- [CC issue #30272](https://github.com/anthropics/claude-code/issues/30272) — /mcp menu doesn't show re-auth after token revocation (HIGH)
- [CC issue #10250](https://github.com/anthropics/claude-code/issues/10250) — reconnect fails after successful OAuth, requires restart (HIGH)
- [CC issue #28256 / #29718](https://github.com/anthropics/claude-code/issues/29718) — token refresh not triggered proactively (HIGH)
- [CC issue #19456](https://github.com/anthropics/claude-code/issues/19456) — macOS Keychain permission errors on token refresh (MEDIUM)
- `~/.claude/mcp-needs-auth-cache.json` — live file confirmed: `{"notion":{"timestamp":...},...}` structure (HIGH — direct inspection)
- `~/.claude/.credentials.json` — live file confirmed: `mcpOAuth.<serverName>|<hash>` key format, `accessToken`/`refreshToken`/`expiresAt`/`discoveryState`/`clientId` fields (HIGH — direct inspection, values redacted)
- [RFC 8414 — OAuth 2.0 Authorization Server Metadata](https://datatracker.ietf.org/doc/html/rfc8414) — AS discovery (HIGH)
- [RFC 9728 — OAuth 2.0 Protected Resource Metadata](https://datatracker.ietf.org/doc/html/rfc9728) — resource metadata discovery (HIGH)
- [RFC 7591 — Dynamic Client Registration](https://datatracker.ietf.org/doc/html/rfc7591) — registration endpoint (HIGH)
- [Stytch: OAuth for MCP explained](https://stytch.com/blog/oauth-for-mcp-explained-with-a-real-world-example/) — practical flow walkthrough (MEDIUM)
- [Upstash: Implementing MCP OAuth](https://upstash.com/blog/mcp-oauth-implementation) — implementation details (MEDIUM)

---
*Feature research for: RightClaw v3.2 MCP OAuth Automation*
*Researched: 2026-04-03*
