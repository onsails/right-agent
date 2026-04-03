# Requirements: RightClaw v3.2 MCP OAuth

**Defined:** 2026-04-03
**Core Value:** Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, each with its own sandbox configuration and identity, orchestrated by a single CLI command.

## v3.2 Requirements

### CRED — Credential Foundation

- [x] **CRED-01**: Operator can trust that MCP OAuth tokens are written under the exact key CC expects — `serverName|sha256({"type":"...","url":"...","headers":{}}, no whitespace)[:16]` — verified by unit test against live Notion entry (`notion|eac663db915250e7`)
- [x] **CRED-02**: Operator can trust concurrent `rightclaw` invocations never corrupt `.credentials.json` — write is atomic (tmp + POSIX rename) with backup before modification; never clobbers unrelated keys (`claudeAiOauth`, etc.)

### DETECT — Auth Detection

- [x] **DETECT-01**: Operator can run `rightclaw mcp status [--agent <name>]` and see a table of MCP servers with auth state per agent (present / missing / expired)
- [x] **DETECT-02**: Operator sees a non-fatal Warn during `rightclaw up` when any agent has MCP servers with missing or expired OAuth tokens

### OAUTH — Core OAuth Flow

- [ ] **OAUTH-01**: Operator can send `/mcp auth <server>` via Telegram bot to complete a full OAuth 2.1 + PKCE flow for a named MCP server (per D-01: no CLI command — bot is the only entrypoint)
- [ ] **OAUTH-02**: OAuth flow performs AS discovery in priority order: RFC 9728 (resource metadata) → RFC 8414 (AS metadata) → OIDC `.well-known/openid-configuration` fallback
- [ ] **OAUTH-03**: OAuth flow performs Dynamic Client Registration (RFC 7591) with automatic fallback to static `clientId` from `.mcp.json` when server lacks a `registration_endpoint`
- [ ] **OAUTH-04**: OAuth flow requires cloudflared named tunnel as redirect URI — if `cloudflared` binary is absent, bot replies with a clear error before the flow starts (no partial state left behind)
- [ ] **OAUTH-05**: OAuth flow verifies tunnel is reachable via explicit HTTP request before presenting auth URL to operator — aborts with error if tunnel healthcheck fails (named tunnel, not quick tunnel)
- [ ] **OAUTH-06**: OAuth flow persists PKCE state to file before opening browser; axum callback server on random loopback port receives the redirect through the tunnel
- [ ] **OAUTH-07**: OAuth flow writes completed token to `~/.claude/.credentials.json` via atomic CRED write; agent is restarted via process-compose REST API after successful token storage

### REFRESH — Token Refresh

- [ ] **REFRESH-01**: Operator can run `rightclaw mcp refresh [<server>] [--agent <name>]` to on-demand refresh an MCP OAuth token without user interaction (uses `refresh_token`, no browser)
- [ ] **REFRESH-02**: `rightclaw up` proactively refreshes tokens with expired `expiresAt` before launching agents (non-fatal — logs Warn if refresh fails, continues launch)
- [ ] **REFRESH-03**: `rightclaw doctor` reports missing/expired MCP OAuth tokens per agent (Warn severity) and checks that `cloudflared` binary is available in PATH (Warn severity)
- [ ] **REFRESH-04**: Tokens with `expiresAt=0` are skipped by the refresh loop and treated as non-expiring (handles Linear and similar providers)

### BOT — Telegram Bot MCP Commands

- [ ] **BOT-01**: User can send `/mcp` in Telegram to receive a list of MCP servers configured for the agent with their auth status (present / missing / expired)
- [ ] **BOT-02**: User can send `/mcp auth <server>` in Telegram to trigger the OAuth flow — bot replies with the auth URL; after user completes auth, bot confirms success or reports tunnel/auth error
- [ ] **BOT-03**: User can send `/mcp add <config>` in Telegram to add a new MCP server to the agent's `.mcp.json` (syntax mirrors `claude mcp add`)
- [ ] **BOT-04**: User can send `/mcp remove <server>` in Telegram to remove an MCP server from the agent's `.mcp.json`
- [ ] **BOT-05**: User can send `/doctor` in Telegram to run `rightclaw doctor` and receive the results in chat (including tunnel availability and MCP auth status per server)

### TUNL — Tunnel Integration

- [ ] **TUNL-01**: Operator can configure cloudflared named tunnel via `rightclaw init --tunnel-token <TOKEN> --tunnel-url <URL>` — config stored in `~/.rightclaw/config.yaml`; `rightclaw up` spawns cloudflared as a persistent process-compose entry; `rightclaw doctor` checks cloudflared binary and tunnel config (Warn severity). Stable URL across restarts — required for bot-initiated OAuth (TUNL-02 merged here).

## Future Requirements

### Tunnel

- **TUNL-ALT**: Operator can configure ngrok as alternative tunnel provider (requires authtoken in config) — deferred past Phase 34

### Per-Agent OAuth

- **PERAG-01**: Each agent can hold its own MCP OAuth tokens (isolated from other agents' credentials) — deferred pending per-agent HOME isolation v2 work (SEED-004 territory)

## Out of Scope

| Feature | Reason |
|---------|--------|
| Device Flow (RFC 8628) | Not in MCP spec; no production MCP server supports it |
| Storing tokens outside `~/.claude/.credentials.json` | CC reads only its own credential file; separate store would be ignored |
| Relying on CC's internal headless token refresh | Confirmed broken (CC issues #28256, #29718, #35092); rightclaw owns refresh |
| macOS Keychain for MCP tokens | macOS may use Keychain; v3.2 scoped to Linux only |
| Automated background refresh daemon | No long-lived rightclaw process to host it; on-demand refresh is sufficient |
| Tunnel as default for CLI OAuth flow | CLI-initiated flow (`rightclaw mcp auth`) can print URL to terminal; tunnel only mandatory for bot-initiated flow |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| CRED-01 | Phase 32 | Complete |
| CRED-02 | Phase 32 | Complete |
| DETECT-01 | Phase 33 | Complete |
| DETECT-02 | Phase 33 | Complete |
| OAUTH-01 | Phase 34 | Pending |
| OAUTH-02 | Phase 34 | Pending |
| OAUTH-03 | Phase 34 | Pending |
| OAUTH-04 | Phase 34 | Pending |
| OAUTH-05 | Phase 34 | Pending |
| OAUTH-06 | Phase 34 | Pending |
| OAUTH-07 | Phase 34 | Pending |
| REFRESH-01 | Phase 35 | Pending |
| REFRESH-02 | Phase 35 | Pending |
| REFRESH-03 | Phase 35 | Pending |
| REFRESH-04 | Phase 35 | Pending |
| BOT-01 | Phase 34 | Pending |
| BOT-02 | Phase 34 | Pending |
| BOT-03 | Phase 34 | Pending |
| BOT-04 | Phase 34 | Pending |
| BOT-05 | Phase 34 | Pending |
| TUNL-01 | Phase 34 | Pending |

**Coverage:**
- v3.2 requirements: 21 total (CRED×2 + DETECT×2 + OAUTH×7 + REFRESH×4 + BOT×5 + TUNL×1)
- Mapped to phases: 21
- Unmapped: 0 ✓

---
*Requirements defined: 2026-04-03*
*Last updated: 2026-04-03 — D-10: OAUTH-01/04/05 restated per bot-only scope; BOT-01..05 moved to Phase 34; TUNL-01 (named tunnel) moved from Future into Phase 34; coverage count updated to 21*
