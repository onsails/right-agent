---
gsd_state_version: 1.0
milestone: v3.2
milestone_name: MCP OAuth
status: verifying
stopped_at: Completed 32-01-PLAN.md
last_updated: "2026-04-03T13:17:18.622Z"
last_activity: 2026-04-03
progress:
  total_phases: 5
  completed_phases: 1
  total_plans: 1
  completed_plans: 1
  percent: 0
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-04-03 after v3.1 milestone)

**Core value:** Run multiple autonomous Claude Code agents safely -- each sandboxed by native OS-level isolation, orchestrated by a single CLI command.
**Current focus:** Phase 32 — credential-foundation

## Current Position

Phase: 32 (credential-foundation) — EXECUTING
Plan: 1 of 1
Status: Phase complete — ready for verification
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

### Pending Todos

None.

### Blockers/Concerns

- `discoveryState` internal field schema not fully documented — inspect live .credentials.json during Phase 34 planning
- process-compose single-agent restart endpoint — verify exact endpoint/payload during Phase 34 planning (pattern exists in codebase)
- macOS: MCP tokens may use Keychain (CC issue #19456) — v3.2 scoped to Linux only

## Session Continuity

Last session: 2026-04-03T13:17:18.619Z
Stopped at: Completed 32-01-PLAN.md
Resume file: None
