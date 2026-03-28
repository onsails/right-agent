---
gsd_state_version: 1.0
milestone: v2.5
milestone_name: RightCron Reliability
status: planning
stopped_at: ""
last_updated: "2026-03-28T00:00:00.000Z"
last_activity: 2026-03-28
progress:
  total_phases: 2
  completed_phases: 0
  total_plans: 0
  completed_plans: 0
  percent: 0
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-03-26)

**Core value:** Run multiple autonomous Claude Code agents safely -- each sandboxed by native OS-level isolation, orchestrated by a single CLI command.
**Current focus:** v2.5 — Roadmap ready, Phase 21 next

## Current Position

Phase: Not started (roadmap complete)
Plan: —
Status: Ready to plan Phase 21
Last activity: 2026-03-28 — Roadmap created (Phases 21-22)

Progress: [░░░░░░░░░░] 0%

## Performance Metrics

*Carried from v2.2 for reference:*

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| Phase 11 | 2 | ~18min | ~9min |
| Phase 12 | 1 | — | — |
| Phase 13 | 1 | — | — |
| Phase 14 | 1 | — | — |
| Phase 15 | 1 | — | — |
| Phase 16-db-foundation P02 | 5 | 2 tasks | 11 files |
| Phase 16-db-foundation P01 | 3 | 2 tasks | 9 files |
| Phase 16 P03 | 90 | 2 tasks | 2 files |
| Phase 17 P01 | 4 | 2 tasks | 5 files |
| Phase 17 P02 | 455 | 2 tasks | 8 files |
| Phase 18-cli-inspection P01 | 4 | 2 tasks | 3 files |
| Phase 18-cli-inspection P02 | 3 | 2 tasks | 1 files |
| Phase 19-home-isolation-hardening P01 | 7 | 2 tasks | 15 files |
| Phase 20-diagnosis P01 | 2 | 1 tasks | 1 files |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions relevant to v2.3:

- [v2.3 research]: Use rusqlite 0.39 + rusqlite_migration 2.5 (sync-only; tokio-rusqlite rejected)
- [v2.3 research]: FTS5 virtual table in V1 schema even if skill uses LIKE in v2.3 — avoids costly retrofit
- [v2.3 research]: memory.db lives in agent root (not .claude/), never referenced by MEMORY.md
- [v2.3 research]: Injection scanning deferred to Phase 17 with dedicated research before implementation
- [Phase 16-02]: SEC-02 enforced by removing memory_path from AgentDef struct entirely — no MEMORY.md connection at type level
- [Phase 16-02]: Task 2 system_prompt default was pre-completed by plan 16-01 (commit e11f9ff)
- [Phase 16-db-foundation]: rusqlite 0.39 bundled + rusqlite_migration 2.5 for per-agent SQLite memory; WAL mode + FTS5 + ABORT triggers in V1 schema
- [Phase 16]: sqlite3 check uses inline Warn override pattern — matches RESEARCH.md Pattern 5
- [Phase 17]: Use str::contains() on lowercased input over 15-pattern list — no regex crate, matches SEC-01 research
- [Phase 17]: open_connection() returns live Connection for store ops; open_db() retained for cmd_up callers
- [Phase 17]: Injection guard is first line of store_memory() — structural guarantee cannot be bypassed
- [Phase 17]: Use ServerInfo::new().with_instructions() — InitializeResult is #[non_exhaustive] in rmcp 1.3
- [Phase 17]: run_memory_server() returns miette::Result — no anyhow in CLI crate, miette is project standard
- [Phase 17]: cargo update required before build — rmcp-macros 1.3.0 not in stale local crates.io index
- [Phase 18-cli-inspection]: list_memories uses ORDER BY created_at DESC, id DESC for deterministic pagination when timestamps tie
- [Phase 18-cli-inspection]: hard_delete_memory checks existence without deleted_at filter — operators can hard-delete soft-deleted rows
- [Phase 18-cli-inspection]: search_memories unchanged (LIMIT 50); search_memories_paged is separate function for CLI pagination
- [Phase 18-cli-inspection]: cmd_memory_delete fetches entry preview via direct SQL including soft-deleted rows — operators see what they are hard-deleting
- [Phase 18-cli-inspection]: resolve_agent_db centralizes agent-dir and memory.db validation for all cmd_memory_* functions
- [Phase 19-home-isolation-hardening]: Telegram detection reads agent.config.telegram_token/telegram_token_file; mcp_config_path removed as unreliable proxy
- [Phase 19-home-isolation-hardening]: generate_mcp_config gains agent_name param; RC_AGENT_NAME injected into rightmemory env section for memory provenance
- [Phase 20-diagnosis]: Hypothesis B (socat TCP timeout) eliminated — plugin runs outside bwrap, direct TCP to api.telegram.org, socat cannot affect it
- [Phase 20-diagnosis]: Root cause confirmed: CC iv6() callback does not call M6() when Z===null (idle) — channel messages queue in hz with no drain mechanism after SubagentStop
- [Phase 20-diagnosis]: Phase 21 fix: Option A (persistent background agent / rightcron watch mode) — keeps M6() step cycle running, guarantees channel message drain

### Roadmap Evolution

- Phase 19 added: HOME Isolation Hardening — plugin sharing, shell snapshot cleanup, fresh-init UAT
- v2.4 phases 20-21 added: Sandbox Telegram Fix (diagnosis + fix/verification)
- v2.5 phases 21-22 added: RightCron Reliability (bootstrap fix + reconciler redesign + verification)

### Pending Todos

None yet.

### Blockers/Concerns

- Phase 17 (injection scanning): Practical Rust implementation patterns sparse — needs research pass before coding SEC-01
- OAuth broken under HOME override on Linux -- ANTHROPIC_API_KEY required for headless (carry-over from v2.2)

### Quick Tasks Completed

| # | Description | Date | Commit | Directory |
|---|-------------|------|--------|-----------|
| 260326-us1 | Replace is_tty with is_interactive in process-compose template | 2026-03-26 | 427f5e1 | [260326-us1-replace-is-tty-with-is-interactive-in-pr](./quick/260326-us1-replace-is-tty-with-is-interactive-in-pr/) |
| 260327-04d | Fix rightmemory MCP binary path — use absolute path from current_exe() | 2026-03-27 | fb5972e | [260327-04d-fix-rightmemory-mcp-binary-path-use-abso](./quick/260327-04d-fix-rightmemory-mcp-binary-path-use-abso/) |

## Session Continuity

Last session: 2026-03-28T21:54:31.189Z
Stopped at: Completed 20-diagnosis 20-01-PLAN.md
Resume file: None
