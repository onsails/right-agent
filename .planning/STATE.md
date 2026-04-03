---
gsd_state_version: 1.0
milestone: v3.2
milestone_name: MCP OAuth
status: executing
stopped_at: Completed 34-02-PLAN.md
last_updated: "2026-04-03T22:49:32.054Z"
last_activity: 2026-04-03
progress:
  total_phases: 4
  completed_phases: 2
  total_plans: 6
  completed_plans: 4
  percent: 0
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-04-03 after v3.1 milestone)

**Core value:** Run multiple autonomous Claude Code agents safely -- each sandboxed by native OS-level isolation, orchestrated by a single CLI command.
**Current focus:** Phase 34 — core-oauth-flow

## Current Position

Phase: 34 (core-oauth-flow) — EXECUTING
Plan: 3 of 4
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

### Pending Todos

None.

### Blockers/Concerns

- `discoveryState` internal field schema not fully documented — inspect live .credentials.json during Phase 34 planning
- process-compose single-agent restart endpoint — verify exact endpoint/payload during Phase 34 planning (pattern exists in codebase)
- macOS: MCP tokens may use Keychain (CC issue #19456) — v3.2 scoped to Linux only

## Session Continuity

Last session: 2026-04-03T22:49:32.051Z
Stopped at: Completed 34-02-PLAN.md
Resume file: None
