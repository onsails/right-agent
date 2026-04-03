---
gsd_state_version: 1.0
milestone: v3.2
milestone_name: MCP OAuth
status: executing
stopped_at: Completed 35-01-PLAN.md
last_updated: "2026-04-03T23:51:27.162Z"
last_activity: 2026-04-03
progress:
  total_phases: 4
  completed_phases: 3
  total_plans: 9
  completed_plans: 7
  percent: 0
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-04-03 after v3.1 milestone)

**Core value:** Run multiple autonomous Claude Code agents safely -- each sandboxed by native OS-level isolation, orchestrated by a single CLI command.
**Current focus:** Phase 35 — token-refresh

## Current Position

Phase: 35 (token-refresh) — EXECUTING
Plan: 2 of 3
Status: Ready to execute
Last activity: 2026-04-03

Progress: [░░░░░░░░░░] 0%

## Performance Metrics

*Carried from v3.1 for reference — no v3.2 plans complete yet.*

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| Phase 29-01 | 1 | — | — |
| Phase 30-01 | 1 | — | — |
| Phase 31-01 | 1 | — | — |
| Phase 32-credential-foundation P01 | 5 | 2 tasks | 7 files |
| Phase 33-auth-detection P01 | 8 | 2 tasks | 3 files |
| Phase 34 P01 | 7m35s | 3 tasks | 8 files |
| Phase 34-core-oauth-flow P02 | 4m | 2 tasks | 3 files |
| Phase 34 P03 | 6m | 2 tasks | 9 files |
| Phase 34 P04 | 12min | 4 tasks | 6 files |
| Phase 35 P01 | 4m | 2 tasks | 3 files |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions relevant to v3.2:

- [v3.2 research]: Wrong credential key is invisible at runtime — unit test against live Notion entry (`notion|eac663db915250e7`) is mandatory before integration work
- [v3.2 research]: CC token refresh broken in headless mode (issues #28262, #29718) — rightclaw owns all refresh logic
- [v3.2 research]: `mcp-needs-auth-cache.json` is per-agent (under agent HOME), not host — detect.rs reads .mcp.json + .credentials.json directly
- [v3.2 research]: Tunnel is mandatory for Telegram-initiated OAuth (users cannot access localhost URLs); CLI-initiated flow can print URL to terminal
- [v3.2 research]: cloudflared quick tunnel (no account required) chosen over ngrok (authtoken required for stable URLs)
- [v3.2 research]: axum 0.8 + oauth2 5.0 + open 5.3 + sha2 0.10 added as new workspace deps
- [v3.2 research]: expiresAt=0 means non-expiring (Linear); must skip in refresh loop
- [v3.2 research]: Agent must be restarted after OAuth — CC MCP client does not reconnect in-process (issue #10250)
- [Phase 32-credential-foundation]: serde_json::json! sorts keys alphabetically — build compact JSON manually to guarantee type->url->headers field order for CC credential key formula
- [Phase 32-credential-foundation]: Backup rotation slot shift must iterate ascending to avoid overwriting slots; concurrent ENOENT on rename is benign (TOCTOU)
- [Phase 33-auth-detection]: expires_at=0 treated as Present (non-expiring), not Expired — Linear case
- [Phase 33-auth-detection]: Stdio servers (no url field) silently skipped — url presence is the OAuth candidate boundary
- [Phase 34]: rand 0.10 uses RngExt trait for fill() on ThreadRng (not Rng or RngCore)
- [Phase 34]: GlobalConfig YAML write is manual (serde-saphyr deserialize-only); schema is 2 fields so manual formatting is sufficient
- [Phase 34-core-oauth-flow]: reqwest form feature added to workspace — exchange_token requires application/x-www-form-urlencoded POST
- [Phase 34-core-oauth-flow]: discovery_urls helper extracts URL construction for pure unit tests without HTTP
- [Phase 34]: cloudflared process in PC config is conditional on tunnel_token — bots without tunnel work unchanged
- [Phase 34]: catch-all http_status:404 is always emitted regardless of agent count — cloudflared rejects configs without it
- [Phase 34]: write_credential takes (path, server_name, server_url, token) and derives key internally; plan interface mismatch auto-corrected
- [Phase 34]: exchange_token arg order: (client, endpoint, code, redirect_uri, client_id, secret, verifier) — not as documented in plan
- [Phase 35]: client_id stored as Some(String) always — PendingAuth.client_id is non-optional (DCR or static fallback guarantees it); client_secret is Option to handle public/confidential clients

### Pending Todos

None.

### Blockers/Concerns

- `discoveryState` internal field schema not fully documented — inspect live .credentials.json during Phase 34 planning
- process-compose single-agent restart endpoint — verify exact endpoint/payload during Phase 34 planning (pattern exists in codebase)
- macOS: MCP tokens may use Keychain (CC issue #19456) — v3.2 scoped to Linux only

## Session Continuity

Last session: 2026-04-03T23:51:27.158Z
Stopped at: Completed 35-01-PLAN.md
Resume file: None
