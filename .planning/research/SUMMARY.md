# Project Research Summary

**Project:** RightClaw v3.2 — MCP OAuth Automation
**Domain:** OAuth 2.1 automation for headless multi-agent Claude Code runtime
**Researched:** 2026-04-03
**Confidence:** HIGH

## Executive Summary

RightClaw v3.2 solves a concrete, confirmed bug in Claude Code: HTTP MCP servers that require OAuth never trigger the browser auth flow, even in interactive mode (CC issues #11585, #36307, still open April 2026). Headless agents compound this — there is no terminal for the user, no `/mcp` TUI, and no path to authenticate. The v3.2 milestone implements a fully external OAuth 2.1 Authorization Code + PKCE flow owned by rightclaw, writing tokens directly into CC's internal credential store so agents pick them up without any CC involvement in the auth dance.

The recommended approach is: (1) detect auth-needing servers by reading agent `.mcp.json` and checking `~/.claude/.credentials.json` for missing/expired tokens, (2) run a standard OAuth 2.1 + PKCE flow using `axum` for the callback server and `oauth2` crate for PKCE/token types, (3) write the completed token into `~/.claude/.credentials.json` under the exact key CC expects — `serverName|sha256({"type":"...","url":"...","headers":{}}, no whitespace)[:16]` — and (4) restart the agent so CC picks up the new token (live connection injection does not work, confirmed from CC source + issue #10250). The credential symlink established in v2.1 means one OAuth flow covers all agents sharing a given MCP server.

Key risks are: (a) writing credentials under a wrong key format that CC silently ignores — the key hash formula was reverse-engineered from CC v2.1.89 source and verified against live data, requiring a unit test before any integration work; (b) CC's token refresh being unreliable in headless mode (confirmed bugs #28262, #29718) — rightclaw must own proactive refresh, not delegate to CC; (c) concurrent credential writes corrupting `.credentials.json` — atomic write (tmp + rename) is mandatory from day one, not a hardening step.

---

## Key Findings

### Recommended Stack

Three new crates are needed; all existing workspace deps (tokio, reqwest, serde_json, thiserror, miette) cover the rest of the flow. `axum 0.8` handles the one-shot OAuth callback HTTP server (tokio-native, single route, oneshot channel — clean lifecycle). `oauth2 5.0` provides strongly-typed PKCE generation and token exchange (35M+ downloads, reqwest backend already in workspace). `open 5.3` launches the browser cross-platform with a terminal URL fallback.

Tunnel integration (cloudflared quick tunnels, no account required) is implemented by shelling out to the `cloudflared` binary — there is no Rust SDK, and the ngrok crate requires a registered authtoken which is unsuitable as a default. Most MCP providers (Notion, Linear, Google Workspace) accept RFC 8252 loopback redirect URIs (`http://127.0.0.1:<random_port>/callback`), making tunnels optional rather than mandatory.

**Core new technologies:**
- `axum 0.8`: OAuth callback HTTP server — tokio-native, oneshot channel pattern, clean lifecycle
- `oauth2 5.0`: PKCE (S256), token exchange, refresh — typed API, reqwest backend already in workspace
- `open 5.3`: Cross-platform browser launch with fallback URL printing
- `cloudflared` (subprocess, optional): Tunnel for providers rejecting localhost — no account required, parse URL from stderr
- `reqwest` (existing): MCP server probing (401 detection), metadata discovery, DCR, token exchange HTTP calls

**Version additions to workspace Cargo.toml:**
```toml
axum = "0.8"
oauth2 = "5.0"
open = "5.3"
```

Note: `sha2` crate will also be needed for the credential key hash formula — add to workspace deps in Phase 1.

### Expected Features

**Must have (table stakes) — v3.2 launch:**
- Auth detection: cross-reference agent `.mcp.json` with `.credentials.json` for missing/expired tokens
- Authorization Server discovery: RFC 9728 (resource metadata) + RFC 8414 (AS metadata) + OIDC fallback, all three endpoints tried in priority order
- Dynamic Client Registration (RFC 7591) with static `clientId` fallback — DCR is "SHOULD" not "MUST" in spec; Slack, enterprise providers don't support it
- PKCE S256 code challenge/verifier generation
- Local callback HTTP server: axum on random loopback port, one request, then shut down
- Browser opener via `open` crate with terminal URL fallback
- Token exchange with PKCE verifier and `resource` parameter (RFC 8707)
- Write tokens to `~/.claude/.credentials.json` under correct key with merge (never clobber `claudeAiOauth` or other keys)
- `rightclaw mcp auth <server> [--agent <name>]` subcommand
- Pre-flight warning in `rightclaw up` — non-fatal warn if servers have missing/expired tokens
- Restart-after-auth: `mcp auth` must restart the agent via process-compose after writing tokens

**Should have — v3.2.x after validation:**
- `rightclaw mcp refresh [<server>]` — on-demand proactive token refresh (CC's headless refresh is buggy)
- `rightclaw mcp status` — table of server auth state per agent
- `rightclaw doctor` MCP token check — Warn severity for missing/expired tokens per agent
- PKCE state file persistence to `~/.rightclaw/oauth-pending/<server>.json` before browser opens

**Defer to v3.3+:**
- Tunnel integration (cloudflared) — only needed for providers rejecting localhost; most don't
- Per-agent OAuth tokens — all agents share tokens via credential symlink; per-agent isolation deferred (SEED-004 territory)
- Automated background refresh daemon — no long-lived rightclaw process to host it

**Anti-features to avoid:**
- Device Flow (RFC 8628) — not in MCP spec, no production server supports it
- Storing tokens outside `~/.claude/.credentials.json` — CC reads only its own file
- Tunnel as default — most providers accept loopback; tunnel adds ephemeral URL instability
- Relying on CC's internal headless refresh — confirmed broken in multiple CC issues

### Architecture Approach

The architecture is a new `mcp/` module in `crates/rightclaw/src/` with clean sub-module separation. Each component handles exactly one concern: `detect.rs` reads `.mcp.json` and checks `.credentials.json`; `oauth.rs` runs AS discovery + DCR + PKCE code exchange; `callback.rs` hosts the axum one-shot server; `credentials.rs` handles read-modify-write with atomic swap; `refresh.rs` handles expiry checks and token refresh. The CLI entry point (`cmd_mcp_auth`) orchestrates these sequentially. Integration into `cmd_up` is non-fatal pre-flight only.

Token storage writes to the host `~/.claude/.credentials.json` (not per-agent). The existing v2.1 credential symlink (`$AGENT_DIR/.claude/.credentials.json → ~/.claude/.credentials.json`) means all agents automatically see updated tokens — no per-agent writing needed.

**Major components:**
1. `mcp/detect.rs` — Parse `.mcp.json`, classify by transport type, check token presence/expiry in `.credentials.json`
2. `mcp/oauth.rs` — AS discovery (RFC 9728 + 8414 + OIDC), DCR (RFC 7591) with static client fallback, PKCE generation, code exchange
3. `mcp/callback.rs` — axum local HTTP server, oneshot channel, PKCE state file persistence
4. `mcp/credentials.rs` — atomic read-modify-write, key formula `serverName|sha256({"type":"...","url":"...","headers":{}})[:16]`
5. `mcp/refresh.rs` — expiry check, refresh_token exchange, rotation handling, expiresAt=0 skip
6. `cmd_mcp_auth()` / `McpCommands` enum — CLI orchestration, agent restart via process-compose
7. `cmd_up()` (modified) — pre-flight MCP auth status check, non-fatal warn

### Critical Pitfalls

1. **Wrong credential key hash** — CC's `fM()` function (verified from cli.js v2.1.89) computes `sha256(JSON.stringify({type, url, headers:{}}), no_spaces)[:16]`. The `headers` field must be `{}` even when empty — omitting it changes the hash. Wrong key = CC silently sends unauthenticated requests. **Mitigation:** Unit test `mcp_oauth_key("notion", "http", "https://mcp.notion.com/mcp") == "notion|eac663db915250e7"` before any integration work.

2. **mcp-needs-auth-cache.json is per-agent, not host** — Under HOME isolation, CC writes `$AGENT_DIR/.claude/mcp-needs-auth-cache.json`, not `~/.claude/`. The host file only reflects host CC session state. **Mitigation:** Auth detection reads `.mcp.json` + `.credentials.json` directly; use the needs-auth cache only for diagnostics, not detection logic.

3. **CC token refresh unreliable in headless mode** — CC's `tokens()` has confirmed bugs (#28262, #29718): proactive refresh fails, tokens from servers omitting `expires_in` get a 1h fallback with no refresh path (issue #26281). **Mitigation:** Rightclaw owns proactive refresh via `mcp refresh`; run as pre-flight in `rightclaw up`.

4. **MCP reconnection failure after OAuth** — CC's in-process MCP client does not reconnect when tokens are written post-auth (CC issue #10250). **Mitigation:** `rightclaw mcp auth` must restart the agent after writing tokens — design restart-after-auth as the primary UX, not a workaround.

5. **DCR not universal** — RFC 7591 is "SHOULD" in MCP spec. Slack, Microsoft Entra, Okta don't support it. **Mitigation:** Check `oauth.clientId` in `.mcp.json` before attempting DCR; if AS metadata has no `registration_endpoint`, fall through to static client mode.

6. **Concurrent credential writes corrupt .credentials.json** — Last-write-wins under concurrent OAuth flows. **Mitigation:** Atomic write (tmp file + POSIX rename) from day one; write backup before modification.

7. **expiresAt=0 treated as expired** — Linear and some providers issue non-expiring tokens; CC stores `expiresAt=0`. **Mitigation:** Skip `expiresAt=0` entries from the refresh loop; treat as non-expiring.

---

## Implications for Roadmap

Based on research, suggested 4-phase structure:

### Phase 1: Credential Foundation
**Rationale:** Everything else depends on correctly reading and writing `.credentials.json`. The key hash formula is the highest-risk item in the entire milestone — wrong key = silent invisible failure that passes all integration tests. Validate this first with a unit test against live data.
**Delivers:** `mcp/credentials.rs` with `mcp_oauth_key()`, atomic read-modify-write, backup before modification; unit tests for key formula against live Notion entry
**Addresses:** Token storage; credential merge safety
**Avoids:** Pitfalls 1 (wrong key), 6 (concurrent write corruption)
**Research flag:** Standard pattern (sha256 + atomic rename). No research phase needed.

### Phase 2: Auth Detection and Pre-flight
**Rationale:** Detection logic (`.mcp.json` + `.credentials.json` cross-reference) is low-complexity and enables both pre-flight warnings and the test harness for all subsequent phases. Addresses the needs-auth cache pitfall and HOME isolation scoping up front.
**Delivers:** `mcp/detect.rs`, `rightclaw mcp status` subcommand, `rightclaw up` pre-flight warning (non-fatal)
**Addresses:** Auth detection, pre-flight warning, Doctor MCP token check
**Avoids:** Pitfall 2 (per-agent cache path), Pitfall 8 (HOME isolation scope from ARCHITECTURE.md)
**Research flag:** Standard pattern. No research phase needed.

### Phase 3: Core OAuth Flow
**Rationale:** Main implementation phase. Depends on credentials.rs (Phase 1) for storage. Sub-phases must follow dependency order: AS discovery → DCR (with static client fallback) → PKCE → callback server → browser open → code exchange → token write → agent restart. DCR fallback must be in this phase, not deferred.
**Delivers:** `mcp/oauth.rs`, `mcp/callback.rs`, `rightclaw mcp auth` subcommand with full `McpCommands` enum, PKCE state file persistence, agent restart via process-compose
**Addresses:** All P1 features from FEATURES.md
**Avoids:** Pitfalls 4 (tunnel URL instability — loopback default), 5 (PKCE state loss — file before browser opens), 5 (DCR fallback), 9 (reconnection failure — restart-after-auth as primary UX)
**Research flag:** Needs validation of `discoveryState` field schema during implementation (see Gaps). AS discovery endpoint behavior for Notion and Linear should be tested against live servers early in this phase.

### Phase 4: Token Refresh and Doctor Integration
**Rationale:** Refresh is P2 priority — high value (CC's headless refresh is broken) but depends on Phase 3 infrastructure. Doctor integration follows existing DoctorCheck pattern. `expiresAt=0` handling is part of this phase.
**Delivers:** `mcp/refresh.rs`, `rightclaw mcp refresh` subcommand, Doctor `check_mcp_oauth()` DoctorCheck (Warn severity), proactive refresh call in `rightclaw up` pre-flight
**Addresses:** P2 features from FEATURES.md; token expiry silent failure
**Avoids:** Pitfall 3 (silent headless refresh failure), Pitfall 7 (expiresAt=0 Linear edge case)
**Research flag:** Standard pattern. No research phase needed.

### Phase Ordering Rationale

- Phase 1 before everything: wrong key format is invisible at runtime. Unit test against live data is the only way to confirm correctness before integration.
- Phase 2 before Phase 3: detection logic is needed as a test fixture for OAuth flow tests. Pre-flight warning is low-risk and delivers immediate operator value.
- Phase 3 is kept monolithic within its scope: OAuth protocol steps have tight sequencing dependencies (DCR must complete before PKCE auth URL is constructed; callback server must start before browser opens). Splitting risks delivering a half-functional flow that cannot be tested end-to-end.
- Phase 4 last: refresh depends on Phase 3 tokens. Doctor depends on Phase 2 detection. Both are cleanup/hardening, not blockers.

### Research Flags

Phases needing deeper research during planning:
- **Phase 3:** `discoveryState` field internal schema — known fields are `authorizationServerUrl` and `resourceMetadataUrl`; additional fields CC expects are not fully documented. Resolve by running a real OAuth flow and capturing the credential file before and after.
- **Phase 3:** Live server DCR behavior — Notion DCR is confirmed (live credential entry exists). Linear's server URL producing hash `638130d5ab3558f4` should be verified to confirm the canonical URL CC uses.

Phases with standard patterns (skip research-phase):
- **Phase 1:** SHA-256 via `sha2` crate and atomic rename are well-documented Rust patterns.
- **Phase 2:** File path resolution under HOME isolation is documented in MEMORY.md. No surprises.
- **Phase 4:** `oauth2` crate `exchange_refresh_token()` documented in crate docs. Doctor follows existing DoctorCheck pattern.

---

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Three new crates verified on crates.io. cloudflared confirmed via official docs. ngrok authtoken requirement confirmed from ngrok official docs. |
| Features | HIGH | MCP spec read from modelcontextprotocol.io. CC bugs confirmed from GitHub issue tracker with live repros. Live credential file inspected directly. Feature dependency graph complete. |
| Architecture | HIGH | Module boundaries match existing codebase conventions. Credential symlink architecture confirmed from MEMORY.md and PROJECT.md. Integration points in cmd_up are unambiguous. |
| Pitfalls | HIGH | Pitfalls 1 and 2 verified against CC v2.1.89 cli.js source. Pitfalls 3, 4, 9 confirmed from GitHub issues with live repros. Pitfall 4 confirmed from ngrok official pricing docs. |

**Overall confidence:** HIGH

### Gaps to Address

- **`discoveryState` internal schema:** Complete field set CC expects inside `discoveryState` is not documented. Known: `authorizationServerUrl`, `resourceMetadataUrl`. Resolution: inspect a live credential entry during Phase 3 by capturing `.credentials.json` before and after a real OAuth flow.
- **`sha2` crate not in STACK.md new crate list:** The credential key formula requires SHA-256. `sha2 = "0.10"` must be added to workspace deps in Phase 1. Low risk — ecosystem standard.
- **macOS `.credentials.json` vs Keychain:** All research and verification targets Linux. macOS may store MCP tokens in Keychain (CC issue #19456). v3.2 scope is Linux only; macOS path deferred.
- **Process-compose restart endpoint for single agent:** Agent restart after OAuth uses process-compose REST API. The exact endpoint and payload were not researched. Resolution: check process-compose API docs during Phase 3 planning (existing codebase uses the REST API for other operations — pattern exists to follow).

---

## Sources

### Primary (HIGH confidence)
- CC v2.1.89 source `cli.js` — `fM()` key formula, `tokens()` refresh logic, `sD8()` credential path, `RF1()` auth cache path (direct source inspection 2026-04-03)
- Live `~/.claude/.credentials.json` — `mcpOAuth` structure with Notion (`notion|eac663db915250e7`) and Linear (`plugin:linear:linear|638130d5ab3558f4`) entries confirmed 2026-04-03
- Live `~/.claude/mcp-needs-auth-cache.json` — `{"notion":{"timestamp":...},...}` format confirmed 2026-04-03
- [MCP Authorization Specification](https://modelcontextprotocol.io/specification/draft/basic/authorization) — OAuth 2.1, PKCE, discovery, DCR, redirect URI constraints
- [RFC 8252 — OAuth 2.0 for Native Apps](https://datatracker.ietf.org/doc/html/rfc8252) — loopback redirect URI exemption
- [RFC 7591 — Dynamic Client Registration](https://datatracker.ietf.org/doc/html/rfc7591)
- [RFC 8414 — OAuth 2.0 AS Metadata](https://datatracker.ietf.org/doc/html/rfc8414)
- [RFC 9728 — Protected Resource Metadata](https://datatracker.ietf.org/doc/html/rfc9728)
- [axum crates.io](https://crates.io/crates/axum) — v0.8.6 current
- [oauth2-rs docs.rs](https://docs.rs/oauth2/latest/oauth2/) — v5.0, PKCE types confirmed
- [Cloudflare Quick Tunnels](https://try.cloudflare.com/) — no-account quick tunnel confirmed

### Secondary (MEDIUM confidence)
- CC issue #11585, #36307 — HTTP MCP servers never trigger OAuth browser flow (confirmed open April 2026)
- CC issue #28256, #29718, #35092 — token refresh not triggered proactively in headless mode
- CC issue #10250 — OAuth succeeds but MCP reconnection fails, requires restart
- CC issue #30272 — /mcp menu doesn't surface revoked server as needing re-auth
- CC issue #38813 — OAuth tokens expire silently in headless automation
- CC issue #26281 — tokens without `expires_in` and `refresh_token` silently expire (1h fallback)
- claude-plugins-official issue #17 — Slack MCP fails: "does not support dynamic client registration"
- CC issue #38102 — MCP OAuth "does not support DCR" despite clientId configured
- [ngrok free plan limits](https://ngrok.com/docs/pricing-limits/free-plan-limits) — 2h session cap, random URLs
- [axum OAuth example](https://github.com/tokio-rs/axum/blob/main/examples/oauth/src/main.rs) — callback server pattern confirmed
- RightClaw MEMORY.md — credential symlink architecture confirmed

### Tertiary (LOW confidence)
- `discoveryState` internal field structure — present in live data as dict, content redacted; full schema not confirmed
- `expiresAt` milliseconds vs seconds — inferred from ~1.7T values consistent with ms epoch; not confirmed from CC source directly

---
*Research completed: 2026-04-03*
*Ready for roadmap: yes*
