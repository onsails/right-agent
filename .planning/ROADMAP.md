# Roadmap: RightClaw

## Milestones

- ✅ **v1.0 Core Runtime** - Phases 1-4 (shipped 2026-03-23)
- ✅ **v2.0 Native Sandbox** - Phases 5-7 (shipped 2026-03-24)
- ✅ **v2.1 Headless Agent Isolation** - Phases 8-10 (shipped 2026-03-25)
- ✅ **v2.2 Skills Registry** - Phases 11-15 (shipped 2026-03-26)
- ✅ **v2.3 Memory System** - Phases 16-19 (shipped 2026-03-27)
- ✅ **v2.4 Sandbox Telegram Fix** - Phase 20 (shipped 2026-03-28)
- ✅ **v2.5 RightCron Reliability** - Phase 21 (shipped 2026-03-31)
- ✅ **v3.0 Teloxide Bot Runtime** - Phases 22-28.2 (shipped 2026-04-01)
- ✅ **v3.1 Sandbox Fix & Verification** - Phases 29-31 (shipped 2026-04-03)
- 🚧 **v3.2 MCP OAuth** - Phases 32-35 (in progress)

## Phases

<details>
<summary>✅ v1.0 Core Runtime (Phases 1-4) - SHIPPED 2026-03-23</summary>

See [milestones/v1.0-ROADMAP.md](milestones/v1.0-ROADMAP.md)

</details>

<details>
<summary>✅ v2.0 Native Sandbox (Phases 5-7) - SHIPPED 2026-03-24</summary>

See [milestones/v2.0-ROADMAP.md](milestones/v2.0-ROADMAP.md)

</details>

<details>
<summary>✅ v2.1 Headless Agent Isolation (Phases 8-10) - SHIPPED 2026-03-25</summary>

See [milestones/v2.1-ROADMAP.md](milestones/v2.1-ROADMAP.md)

</details>

<details>
<summary>✅ v2.2 Skills Registry (Phases 11-15) - SHIPPED 2026-03-26</summary>

See [milestones/v2.2-ROADMAP.md](milestones/v2.2-ROADMAP.md)

</details>

<details>
<summary>✅ v2.3 Memory System (Phases 16-19) — SHIPPED 2026-03-27</summary>

See [milestones/v2.3-ROADMAP.md](milestones/v2.3-ROADMAP.md)

</details>

<details>
<summary>✅ v2.4 Sandbox Telegram Fix (Phase 20) — SHIPPED 2026-03-28</summary>

See [milestones/v2.4-ROADMAP.md](milestones/v2.4-ROADMAP.md)

</details>

<details>
<summary>✅ v2.5 RightCron Reliability (Phase 21) — SHIPPED 2026-03-31</summary>

See [milestones/v2.5-ROADMAP.md](milestones/v2.5-ROADMAP.md)

</details>

<details>
<summary>✅ v3.0 Teloxide Bot Runtime (Phases 22-28.2) — SHIPPED 2026-04-01</summary>

See [milestones/v3.0-ROADMAP.md](milestones/v3.0-ROADMAP.md)

</details>

<details>
<summary>✅ v3.1 Sandbox Fix & Verification (Phases 29-31) — SHIPPED 2026-04-03</summary>

See [milestones/v3.1-ROADMAP.md](milestones/v3.1-ROADMAP.md)

</details>

### 🚧 v3.2 MCP OAuth (In Progress)

**Milestone Goal:** Automate MCP OAuth authentication for agents — detect unauthenticated servers, complete the full OAuth 2.1 + PKCE flow, write tokens to CC's credential store, refresh on expiry, and expose the full workflow via Telegram bot commands.

- [x] **Phase 32: Credential Foundation** - Correct key formula + atomic credential writes (completed 2026-04-03)
- [x] **Phase 33: Auth Detection** - Per-agent MCP auth status surface and pre-flight warning (completed 2026-04-03)
- [x] **Phase 34: Core OAuth Flow + Bot MCP Commands** - Full OAuth 2.1 + PKCE with cloudflared named tunnel + bot commands (merged with former Phase 36) (completed 2026-04-03)
- [ ] **Phase 35: Token Refresh** - Automatic bot-owned refresh scheduler + doctor integration

## Phase Details

### Phase 32: Credential Foundation
**Goal**: Operators can trust that MCP OAuth tokens are written to `~/.claude/.credentials.json` under the exact key CC expects, atomically, without corrupting unrelated keys
**Depends on**: Phase 31 (v3.1 complete)
**Requirements**: CRED-01, CRED-02
**Success Criteria** (what must be TRUE):
  1. Unit test `mcp_oauth_key("notion", "http", "https://mcp.notion.com/mcp") == "notion|eac663db915250e7"` passes — key formula is verified against live CC credential data
  2. Concurrent `rightclaw` invocations writing to `.credentials.json` do not corrupt the file — atomic tmp+rename write with backup before modification
  3. Writing an MCP token never removes or modifies `claudeAiOauth` or other unrelated keys already in the file
**Plans**: 1 plan
Plans:
- [x] 32-01-PLAN.md — mcp/credentials module: key derivation + atomic write with backup rotation

### Phase 33: Auth Detection
**Goal**: Operators can see which MCP servers need OAuth and get warned before launching agents with unauthenticated servers
**Depends on**: Phase 32
**Requirements**: DETECT-01, DETECT-02
**Success Criteria** (what must be TRUE):
  1. `rightclaw mcp status` prints a table of MCP servers per agent showing auth state (present / missing / expired) for each server
  2. `rightclaw mcp status --agent <name>` filters the table to a single named agent
  3. `rightclaw up` prints a non-fatal Warn (does not abort launch) when any agent has MCP servers with missing or expired OAuth tokens
**Plans**: 1 plan
Plans:
- [x] 33-01-PLAN.md — mcp::detect module + CLI wiring for mcp status + cmd_up warn

### Phase 34: Core OAuth Flow + Bot MCP Commands
**Goal**: Operators can complete a full OAuth 2.1 + PKCE flow for any named MCP server via Telegram bot `/mcp auth`, with tokens written to CC's credential store and the agent restarted automatically. All MCP management commands (/mcp list, /mcp auth, /mcp add, /mcp remove, /doctor) are available in Telegram. cloudflared named tunnel provides stable callback URL.
**Depends on**: Phase 33
**Requirements**: OAUTH-01, OAUTH-02, OAUTH-03, OAUTH-04, OAUTH-05, OAUTH-06, OAUTH-07, BOT-01, BOT-02, BOT-03, BOT-04, BOT-05, TUNL-01
**Note**: Phase 36 (former Telegram Bot MCP Commands) merged into this phase per D-10
**Success Criteria** (what must be TRUE):
  1. `/mcp auth <server>` via Telegram completes the full flow end-to-end: AS discovery -> DCR (or static client fallback) -> PKCE auth URL -> cloudflared tunnel -> callback -> token write -> agent restart
  2. AS discovery tries RFC 9728 (resource metadata) first, then RFC 8414 (AS metadata), then OIDC well-known — visible in debug output and confirmed by unit test
  3. If the MCP server has no `registration_endpoint`, the flow falls back to the static `clientId` from `.mcp.json` without error
  4. If `cloudflared` binary is absent, the bot replies with a clear error before any OAuth state is created
  5. Tunnel healthcheck fails before presenting auth URL if the cloudflared URL is not reachable — bot replies with error
  6. PKCE verifier and state are stored in-process; axum callback server on a Unix socket receives the redirect through the tunnel
  7. After successful token exchange, the token appears in `~/.claude/.credentials.json` under the correct key and the agent process is restarted via process-compose REST API
**Plans**: 4 plans
Plans:
- [x] 34-01-PLAN.md — Foundation: workspace deps, OAuth types, GlobalConfig, init --tunnel-token/--tunnel-url
- [x] 34-02-PLAN.md — OAuth engine: AS discovery, DCR, PKCE, token exchange
- [x] 34-03-PLAN.md — cloudflared integration: config generation, process-compose entry, doctor checks
- [x] 34-04-PLAN.md — Bot integration: axum UDS callback server, /mcp and /doctor commands

### Phase 35: Token Refresh
**Goal**: The bot automatically refreshes MCP OAuth tokens before expiry; `rightclaw doctor` surfaces missing/expired tokens per agent
**Depends on**: Phase 34
**Requirements**: REFRESH-01, REFRESH-02, REFRESH-03, REFRESH-04
**Note**: REFRESH-01 (CLI command) and REFRESH-02 (rightclaw up refresh) superseded by D-01/D-02 — bot scheduler is the only refresh mechanism
**Success Criteria** (what must be TRUE):
  1. Bot startup spawns a background task that refreshes tokens before expiry (10-minute buffer) and retries up to 3 times on failure without crashing
  2. `rightclaw doctor` includes an `mcp-tokens` check: Warn when any agent has missing/expired tokens, Pass otherwise
  3. Tokens with `expiresAt=0` are skipped by the scheduler and counted as ok in doctor (non-expiring, Linear and similar providers)
  4. `CredentialToken` stores `client_id`/`client_secret` so the scheduler can POST the refresh grant without re-running DCR
**Plans**: 3 plans
Plans:
- [ ] 35-01-PLAN.md — Extend CredentialToken with client_id/client_secret; backfill OAuth callback
- [ ] 35-02-PLAN.md — mcp::refresh module: scheduler, per-token loop, refresh grant POST
- [ ] 35-03-PLAN.md — Bot integration: spawn scheduler; doctor check_mcp_tokens

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1-4. Core Runtime | v1.0 | ✓ | Complete | 2026-03-23 |
| 5-7. Native Sandbox | v2.0 | ✓ | Complete | 2026-03-24 |
| 8-10. Headless Agent Isolation | v2.1 | ✓ | Complete | 2026-03-25 |
| 11-15. Skills Registry | v2.2 | ✓ | Complete | 2026-03-26 |
| 16-19. Memory System | v2.3 | ✓ | Complete | 2026-03-27 |
| 20. Sandbox Telegram Fix | v2.4 | ✓ | Complete | 2026-03-28 |
| 21. RightCron Reliability | v2.5 | ✓ | Complete | 2026-03-31 |
| 22-28.2. Teloxide Bot Runtime | v3.0 | ✓ | Complete | 2026-04-01 |
| 29. Sandbox Dependency Fix | v3.1 | 1/1 | Complete | 2026-04-02 |
| 30. Doctor Diagnostics | v3.1 | 1/1 | Complete | 2026-04-02 |
| 31. E2E Verification | v3.1 | 1/1 | Complete | 2026-04-03 |
| 32. Credential Foundation | v3.2 | 1/1 | Complete    | 2026-04-03 |
| 33. Auth Detection | v3.2 | 1/1 | Complete    | 2026-04-03 |
| 34. Core OAuth Flow + Bot | v3.2 | 4/4 | Complete    | 2026-04-03 |
| 35. Token Refresh | v3.2 | 0/3 | Not started | - |
