# Pitfalls Research

**Domain:** MCP OAuth automation for headless multi-agent runtime (RightClaw v3.2)
**Researched:** 2026-04-03
**Confidence:** HIGH — verified against CC v2.1.89 source (cli.js) and live credential files

---

## Critical Pitfalls

### Pitfall 1: Wrong Credential Storage Key Format

**What goes wrong:**
RightClaw writes OAuth tokens into `.credentials.json` under a key that CC ignores. Agents start, MCP tools are registered, but all calls return 401. No visible error — CC's token lookup returns `undefined` and it sends requests without Authorization headers.

**Why it happens:**
Developers assume the key is `serverName` or `serverName|sha256(url)`. The actual formula (verified from CC v2.1.89 `fM()` function in cli.js):

```
key = serverName + "|" + sha256(JSON.stringify({type, url, headers:{}}), no_spaces)[0:16]
```

The `JSON.stringify` call uses no whitespace (`separators=(",",":")` equivalent). The `headers` field must be present as `{}` even when empty — omitting it changes the hash.

Verified with live credentials:
- Server `notion`, URL `https://mcp.notion.com/mcp`, type `http`
- Input string: `{"type":"http","url":"https://mcp.notion.com/mcp","headers":{}}`
- `sha256(input).hex[:16]` = `eac663db915250e7`
- Stored key: `notion|eac663db915250e7` — matches live `~/.claude/.credentials.json`

**How to avoid:**
Implement `fn mcp_oauth_key(server_name: &str, server_type: &str, url: &str) -> String` using SHA-256 of the canonical JSON with no whitespace. Add a unit test against a known live entry before integrating with agent startup. Server type is always the `.mcp.json` type field (`"http"` for HTTP MCP servers, `"sse"` for SSE).

**Warning signs:**
- Agent starts but MCP tools return auth errors
- `mcp-needs-auth-cache.json` gets written in agent dir shortly after startup
- Token entry exists in `.credentials.json` but server still fails

**Phase to address:**
Credential storage implementation — unit test the key formula against a known entry before writing token storage code.

---

### Pitfall 2: mcp-needs-auth-cache.json Is Per-Agent, Not Shared With Host

**What goes wrong:**
RightClaw's auth detection reads the host `~/.claude/mcp-needs-auth-cache.json`. Under HOME isolation (`HOME=$AGENT_DIR`), CC reads and writes `$AGENT_DIR/.claude/mcp-needs-auth-cache.json`. These are separate files. Auth detection misses agent-specific needs-auth state. After OAuth completes, the agent-local cache still shows the server as needing auth until TTL expires (15 minutes).

**Why it happens:**
`.credentials.json` is symlinked to host (RightClaw sets this up). `mcp-needs-auth-cache.json` is NOT symlinked and NOT created by rightclaw — CC creates it lazily when it encounters an unauthenticated server. The host file only reflects what the host CC session encountered, not what agents encounter.

Verified: CC source `RF1()` = `r1() + "mcp-needs-auth-cache.json"` where `r1() = CLAUDE_CONFIG_DIR ?? HOME/.claude`. Under HOME isolation this is `$AGENT_DIR/.claude/mcp-needs-auth-cache.json`. Agent `right`'s `.claude/` directory has no `mcp-needs-auth-cache.json` because it has not started yet and encountered unauthenticated servers.

**How to avoid:**
- Auth detection must scan `$AGENT_DIR/.mcp.json` + check `.credentials.json` for missing/expired tokens — not rely on the needs-auth cache at all
- The cache is a passive artifact CC writes; use it for diagnostic output in `rightclaw oauth status`, not for detection logic
- After OAuth completes, optionally delete the entry from `$AGENT_DIR/.claude/mcp-needs-auth-cache.json` to avoid 15-minute stale state
- Cache TTL is 900,000ms (15 min) — entries expire automatically

**Warning signs:**
- `rightclaw oauth status` shows servers authenticated but agent still fails
- Auth detection reports "needs auth" after token was written

**Phase to address:**
Auth detection phase — base detection on token presence/expiry in `.credentials.json`, not on the needs-auth cache.

---

### Pitfall 3: Token Expiry Silently Breaks MCP Tools — CC Headless Refresh Is Unreliable

**What goes wrong:**
An agent's MCP token expires mid-session. MCP tool calls start returning 401-equivalent errors that appear to be network or server failures. The agent continues running with broken MCP access. In headless mode, there is no browser prompt and no user-visible error.

**Why it happens:**
CC's `tokens()` method logic (verified from v2.1.89 source):
1. If `expiresAt - now < 300s` AND `refreshToken` present → attempts proactive refresh
2. If expired AND no `refreshToken` → logs "Token expired without refresh token", **returns `undefined`** (silent tool failure)
3. If refresh fails → logs "Token refresh failed, returning current tokens" (expired token sent — still 401)
4. If `expiresAt = 0` (no expiry) AND no `refreshToken` → returns token unconditionally (correct for non-expiring tokens like Linear)

Critical bugs confirmed in CC issue tracker:
- CC does not reliably auto-refresh in unattended/headless mode even with valid `refreshToken` (issue #28262)
- Tokens from servers that omit `expires_in` get stored as `expiresAt = now + 3600*1000` as fallback — expire after 1 hour with no refresh path (issue #26281)
- After expiry, `/mcp` menu may not surface the server as needing re-auth (issue #30272)

**How to avoid:**
- RightClaw must own token refresh — do not rely on CC's internal refresh in headless mode
- Background task: scan `.credentials.json` every 5 minutes, find tokens expiring within 10 minutes, refresh using `POST {authorizationServerUrl}/token` with `grant_type=refresh_token`
- Use `discoveryState.authorizationServerUrl` stored in the credential entry as the token endpoint base
- Write refreshed tokens atomically (see Pitfall 7)
- For tokens with no `refreshToken`: log warning at storage time; schedule operator notification near `expiresAt`
- For `expiresAt = 0`: treat as non-expiring; skip from refresh loop

**Warning signs:**
- MCP tool failures starting ~1 hour after agent startup
- `expiresAt` in `.credentials.json` is past
- Agent logs show tool errors without explicit "auth" or "401" strings

**Phase to address:**
Token refresh phase — implement before shipping OAuth flow; even a polling refresh loop is sufficient.

---

### Pitfall 4: Tunnel URL Changes on Restart Break Provider-Registered Redirect URIs

**What goes wrong:**
RightClaw's OAuth flow registers a redirect URI with the provider (via RFC 7591 DCR or static registration). The redirect URI contains the ngrok URL. On next `rightclaw up`, ngrok assigns a new random URL. The `redirect_uri` in the authorization request no longer matches the registered URI. OAuth fails with `redirect_uri_mismatch`.

**Why it happens:**
ngrok free tier assigns random ephemeral URLs per session. As of early 2026, free sessions cap at 2 hours and URLs change on every restart. Dynamic client registrations often embed redirect URIs in the `client_id` metadata — changing the URI requires re-registration.

**How to avoid:**
Option 1 (preferred): Use RFC 8252 loopback redirect — `http://127.0.0.1:<port>/callback`. Most providers (Google, GitHub, Notion, Linear) exempt loopback URIs from exact URL validation and accept any port. No tunnel needed. Bind callback server to `127.0.0.1` only.

Option 2: Use Cloudflare Tunnel (`cloudflared`) with a named tunnel — provides stable URL on all tiers including free. URL survives restarts.

Option 3: Use ngrok with a free static domain (one per free account) — tunnel always gets the same hostname.

Loopback is strongly preferred for a local CLI tool. Tunnel is only needed if the OAuth provider refuses loopback URIs (rare for modern providers).

**Warning signs:**
- `error=redirect_uri_mismatch` in callback
- Flow works on first run, fails on second run after ngrok restart
- Provider's registered app shows different redirect URI than current tunnel URL

**Phase to address:**
Tunnel integration design phase — decide loopback vs. tunnel before building callback server. The callback server API should support both.

---

### Pitfall 5: PKCE State/Verifier Lost if Callback Handler Process Restarts

**What goes wrong:**
The operator opens the browser OAuth URL. During the ~2-minute callback window, rightclaw is restarted (or the callback server is on a different process/instance). The callback arrives with `code` and `state`, but `code_verifier` is not found. Token exchange fails.

**Why it happens:**
PKCE requires the `code_verifier` to be retained from auth request through callback. If stored only in memory, it's lost on any restart. In multi-agent setups where a central callback server handles callbacks for all agents, the state must survive process restarts.

The `state` parameter (CSRF token) has the same problem — if the validation side is in-memory only, a new process can't validate it.

**How to avoid:**
- Before opening the browser, write `(state, code_verifier, server_name, started_at)` to `~/.rightclaw/oauth-pending/<server_name>.json`
- Callback server reads this file on arrival, validates `state`, retrieves `code_verifier`
- Delete the file after exchange (success or error) and after 120s timeout
- Single callback server process per `rightclaw up` session — not per-agent
- In-memory storage is acceptable for MVP if restart-during-flow is an edge case; file-based is better long-term

**Warning signs:**
- `code_verifier invalid` or `invalid_grant` error from token endpoint
- `state mismatch` in callback handler

**Phase to address:**
OAuth callback server phase — decision on memory vs. file storage should be made upfront; file is safer and the complexity is trivial.

---

### Pitfall 6: Dynamic Client Registration Absent — No Static Client Fallback

**What goes wrong:**
`rightclaw oauth` attempts DCR on a server that doesn't support RFC 7591. Gets "does not support dynamic client registration". Flow aborts. User cannot authenticate even though the server has a working OAuth flow via pre-configured `clientId`.

**Why it happens:**
RFC 7591 is "SHOULD" in the MCP spec, not "MUST". Major production providers don't implement it: Slack (confirmed), AWS Cognito, Microsoft Entra ID, Okta in some configurations. They require out-of-band app registration. The operator already has a `clientId` (and sometimes `clientSecret`) from the provider's developer console.

Confirmed: Slack's official MCP plugin has `clientId` pre-configured in `.mcp.json`. CC's own Slack plugin issue #17 hit this exact error when the plugin didn't pass the pre-configured `clientId` through to the OAuth layer.

**How to avoid:**
- Before attempting DCR, check if `.mcp.json` server config has `oauth.clientId` set
- If `clientId` is present, skip DCR entirely — proceed directly to PKCE auth code flow using that `clientId`
- DCR fallback detection: fetch RFC 8414 server metadata; if `registration_endpoint` is absent, DCR unavailable; fall through to static client mode
- In static client mode, prompt operator for `clientId` if not already in config, optionally `clientSecret`
- Never require DCR exclusively

**Warning signs:**
- "Incompatible auth server: does not support dynamic client registration" (exact CC error message)
- Server config has `oauth.clientId` in `.mcp.json` but rightclaw ignores it and attempts DCR

**Phase to address:**
OAuth flow implementation — design the auth code path to branch on `clientId` presence before DCR attempt.

---

### Pitfall 7: Concurrent Credential Writes Corrupt .credentials.json

**What goes wrong:**
Two OAuth flows complete at the same time (e.g., Notion and Slack authenticated back-to-back). Both processes read `.credentials.json`, merge their token entry, write back. The second write overwrites the first. One token entry is lost.

**Why it happens:**
`.credentials.json` is a read-modify-write operation on a shared symlinked file. No locking = last write wins. Under multi-agent HOME isolation, all agents share the same file via symlink — so any agent writing credentials risks clobbering concurrent writes.

**How to avoid:**
- Atomic write pattern: write to `~/.claude/.credentials.json.tmp`, then `rename()` — POSIX `rename` is atomic within same filesystem
- Advisory file lock via `fcntl` / `flock` before read-modify-write cycle
- Simpler: serialize all credential writes through the single `rightclaw up` process (agents don't write credentials themselves — only the operator-facing OAuth flow writes them, and the CLI is single-instance)
- Write a backup to `~/.claude/.credentials.json.bak` before any modification

**Warning signs:**
- Intermittent token loss for one server when multiple OAuth flows run simultaneously
- `.credentials.json` has truncated JSON or missing `mcpOAuth` entries

**Phase to address:**
Credential storage implementation — atomic write from the start; add backup creation.

---

### Pitfall 8: HOME Isolation Scopes .mcp.json — Global Server Config Is Invisible

**What goes wrong:**
The operator adds OAuth-protected servers to `~/.claude/.mcp.json` (global CC config) expecting all agents to pick them up. Under HOME isolation, CC resolves MCP config relative to `r1()` = `$AGENT_DIR/.claude/`. The global `~/.claude/.mcp.json` on the host is not read. Auth detection and OAuth flows on rightclaw's side also don't see these servers.

**Why it happens:**
RightClaw correctly generates per-agent `.mcp.json` at `$AGENT_DIR/.mcp.json`. But operators who are familiar with CC's global config may add servers there and be confused when agents don't pick them up.

**How to avoid:**
- Auth detection scans only `$AGENT_DIR/.mcp.json` (project-level MCP config for that agent)
- Document clearly: under HOME isolation, `~/.claude/.mcp.json` is not visible to agents. Add servers via `agent.yaml` → regenerated into `.mcp.json` on `rightclaw up`
- `rightclaw doctor` or `rightclaw oauth status` should note if host global `.mcp.json` has servers not present in any agent config

**Warning signs:**
- Auth detection reports 0 servers when operator has OAuth servers in global config
- Agent lacks tools that exist in `~/.claude/.mcp.json`

**Phase to address:**
Auth detection phase — explicitly document and enforce per-agent `.mcp.json` scope.

---

### Pitfall 9: MCP Reconnection Failure After OAuth — Agent Must Restart

**What goes wrong:**
OAuth completes successfully. Token is stored in `.credentials.json`. The MCP server is still not connected in the running agent session — tools remain unavailable. Confirmed in CC issue #10250: "OAuth Authentication Succeeds but MCP Reconnection Fails - Requires Restart."

**Why it happens:**
CC's MCP client initializes connections at startup. After tokens are written post-auth, CC does not automatically reconnect the MCP server for an already-running session. The new token is available on disk but the in-process MCP client still has no active connection.

This means rightclaw cannot complete OAuth and immediately resume an agent session seamlessly — the agent process must restart after OAuth.

**How to avoid:**
- Design the OAuth flow as a stop-the-agent → authenticate → restart-agent cycle, not a live connection injection
- `rightclaw oauth <agent> <server>` should: pause agent → run OAuth → write token → restart agent via process-compose
- Alternatively: run OAuth before initial agent startup so the token is present when CC starts (during `rightclaw up`)
- Do NOT promise seamless live OAuth injection — it doesn't work in current CC versions

**Warning signs:**
- OAuth completes, rightclaw reports success, but agent still lacks MCP tools
- "Authentication successful, but server reconnection failed" message from CC

**Phase to address:**
OAuth flow design phase — restart-after-auth must be part of the UX design, not an afterthought.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Rely on CC's internal token refresh | No refresh code to write | Silent failures in headless mode (confirmed bug #28262) | Never for headless agents |
| Use ngrok free tier as default | Easy to set up | URL instability, 2h limit, redirect URI breaks on restart | Never for production; dev-only with static domain |
| Store PKCE state in memory only | No file I/O during OAuth | Lost on process restart during 2-min flow window | Acceptable for MVP; migrate to file storage before v1 |
| Skip file locking on credential write | Simpler implementation | Token corruption under concurrent OAuth | Only if OAuth flows are strictly sequential |
| Write entire .credentials.json on update | Simple serialization | Overwrites entries written by CC between read and write | Never — always merge at key level |
| Require DCR exclusively | Simpler implementation | Breaks Slack, GitHub Copilot, and most enterprise providers | Never — static clientId fallback required |

---

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| CC `.credentials.json` key | `serverName` or `sha256(url)` | `serverName\|sha256({"type":"http","url":"...","headers":{}}, no_spaces)[:16]` |
| CC `.credentials.json` value | Flat `{accessToken, refreshToken}` | Must include: `serverName`, `serverUrl`, `accessToken`, `expiresAt`, `discoveryState.authorizationServerUrl`; optionally `refreshToken`, `scope`, `clientId`, `clientSecret` |
| `mcp-needs-auth-cache.json` | Read from `~/.claude/` (host) | Read from `$AGENT_DIR/.claude/` (agent-local); use for diagnostics only, not detection logic |
| Token refresh | Rely on CC headless refresh | Rightclaw-owned background refresh; POST to `discoveryState.authorizationServerUrl` with `grant_type=refresh_token` |
| ngrok callback | Assume URL stable across restarts | Use loopback `127.0.0.1` (RFC 8252) or Cloudflare named tunnel |
| DCR endpoint | Assume all MCP servers support RFC 7591 | Check `registration_endpoint` in server metadata; fall back to `oauth.clientId` from `.mcp.json` |
| Linear OAuth `expiresAt` | Treat `expiresAt=0` as expired | `expiresAt=0` means no expiry — skip from refresh loop |
| Post-OAuth agent state | Expect live connection injection | Must restart agent process after token write — live injection doesn't work in CC |

---

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Logging token values | Token in terminal history / log files | Log only first 8 chars or `[redacted]`; never log `access_token`, `refresh_token`, `client_secret` |
| Callback server bound to `0.0.0.0` | Any process on the machine can intercept OAuth callback | Bind only to `127.0.0.1`; RFC 8252 loopback exemption applies only to loopback |
| PKCE state file not deleted on error | State file with `code_verifier` persists | Always delete state file in callback handler — both success and error paths; TTL cleanup on startup |
| `client_secret` in URL params | Secret in server logs, browser history, proxy logs | Always POST `client_secret` in request body |
| Writing tokens before verifying `state` | CSRF — attacker substitutes their authorization code | Validate `state` before calling token endpoint |
| File permissions on `.credentials.json` | Other users on system can read OAuth tokens | Ensure `chmod 600` after creation; rightclaw writes should not weaken permissions |
| `clientSecret` in plaintext `.credentials.json` | Token theft if file exfiltrated | CC's design — rightclaw matches CC's format; document as "by design, protect the file" |

---

## "Looks Done But Isn't" Checklist

- [ ] **Credential key formula verified:** Unit test `mcp_oauth_key("notion", "http", "https://mcp.notion.com/mcp")` == `"notion|eac663db915250e7"`
- [ ] **Per-agent cache path:** Auth detection reads `$AGENT_DIR/.claude/mcp-needs-auth-cache.json`, not `~/.claude/`
- [ ] **Atomic credential write:** Uses `rename()` atomic swap; file permissions 0600 preserved after write
- [ ] **Backup before write:** `~/.claude/.credentials.json.bak` created before modification
- [ ] **PKCE state persistence:** State file created before browser opens; deleted after callback (both success and error paths)
- [ ] **Token refresh task:** Does not rely solely on CC's internal refresh; rightclaw-owned proactive refresh runs
- [ ] **DCR fallback:** Checks for `oauth.clientId` in `.mcp.json` before attempting RFC 7591 registration
- [ ] **Loopback binding:** Callback server binds to `127.0.0.1` not `0.0.0.0`
- [ ] **No token logging:** `access_token` and `refresh_token` values never appear in stdout/stderr/log output
- [ ] **expiresAt=0 handled:** Tokens with `expiresAt=0` (Linear) not treated as expired; excluded from refresh loop
- [ ] **`discoveryState` written:** `authorizationServerUrl` included in credential entry (required for refresh)
- [ ] **Restart-after-auth:** Flow includes agent restart step; not "write token and hope CC reconnects"
- [ ] **`.mcp.json` scope documented:** `rightclaw oauth status` warns if host global `.mcp.json` has servers absent from agent configs

---

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Wrong credential key | LOW | Delete wrong key from `.credentials.json`, re-run OAuth flow |
| mcp-needs-auth-cache stale | LOW | Delete `$AGENT_DIR/.claude/mcp-needs-auth-cache.json`; CC re-evaluates on next tool call |
| Token expired, no refresh token | LOW | `rightclaw oauth <agent> <server>` — re-initiates browser flow |
| Tunnel URL changed | LOW | Re-run OAuth with new redirect URI; may require new DCR if provider validates exact URI |
| PKCE state file lost | LOW | Delete any pending state files in `~/.rightclaw/oauth-pending/`; retry flow |
| Credentials JSON corrupted | MEDIUM | Restore from `~/.claude/.credentials.json.bak`; re-run any affected OAuth flows |
| All agent tokens expired simultaneously | HIGH | Operator completes OAuth flow for each server; mitigated by proactive refresh |
| No reconnection after OAuth | LOW | Restart agent via `rightclaw restart <agent>`; design this as the expected flow |

---

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| Wrong credential key format | Credential storage implementation | Unit test against live credential entry |
| Per-agent mcp-needs-auth-cache | Auth detection phase | Path assertion: uses `$AGENT_DIR/.claude/`, not `~/.claude/` |
| Token expiry silent failure | Token refresh phase | Integration test: write expired token, verify refresh fires |
| Tunnel URL instability | Tunnel design phase | Default to loopback; explicit test with both loopback and tunnel modes |
| PKCE state loss | OAuth callback server phase | Test: interrupt and restart callback server mid-flow |
| No DCR fallback | OAuth flow implementation | Test with pre-configured `clientId` in `.mcp.json` (no DCR) |
| Concurrent credential write | Credential storage implementation | Stress test: simultaneous OAuth completions, both tokens present |
| HOME isolation scope | Auth detection phase | Verify detection reads only `$AGENT_DIR/.mcp.json` |
| MCP reconnection failure | OAuth flow design | Acceptance test: OAuth → token written → agent restarted → tools available |

---

## Sources

- CC v2.1.89 source `cli.js` — `fM()` key function, `tokens()` refresh logic, `sD8()` credential path, `RF1()` auth cache path, `r1()` config dir resolution (HIGH — direct source inspection)
- Live `~/.claude/.credentials.json` — verified `mcpOAuth` structure with Notion (`notion|eac663db915250e7`) and Linear (`plugin:linear:linear|638130d5ab3558f4`) entries (HIGH — live data)
- [CC issue #38813: OAuth tokens expire silently in headless automation](https://github.com/anthropics/claude-code/issues/38813) (MEDIUM)
- [CC issue #28262: MCP OAuth tokens not auto-refreshing despite valid refresh tokens](https://github.com/anthropics/claude-code/issues/28262) (MEDIUM)
- [CC issue #10250: OAuth succeeds but MCP reconnection fails, requires restart](https://github.com/anthropics/claude-code/issues/10250) (HIGH — matches source analysis)
- [CC issue #26281: MCP OAuth tokens without expires_in and refresh_token silently expire](https://github.com/anthropics/claude-code/issues/26281) (MEDIUM)
- [CC issue #30272: /mcp menu doesn't surface revoked server as needing re-auth](https://github.com/anthropics/claude-code/issues/30272) (MEDIUM)
- [claude-plugins-official issue #17: Slack MCP fails — does not support dynamic client registration](https://github.com/anthropics/claude-plugins-official/issues/17) (HIGH — official plugin repo)
- [CC issue #38102: MCP OAuth "does not support dynamic client registration" despite clientId configured](https://github.com/anthropics/claude-code/issues/38102) (MEDIUM)
- [ngrok free plan limits documentation](https://ngrok.com/docs/pricing-limits/free-plan-limits) — 2h session cap, random URLs (HIGH — official docs)
- [RFC 8252: OAuth 2.0 for Native Apps](https://datatracker.ietf.org/doc/html/rfc8252) — loopback redirect URI exemption (HIGH — RFC)
- [MCP Authorization spec 2025-06-18](https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization) — SHOULD support DCR, fallback to static client (HIGH — official spec)

---
*Pitfalls research for: MCP OAuth automation in headless multi-agent runtime (RightClaw v3.2)*
*Researched: 2026-04-03*
