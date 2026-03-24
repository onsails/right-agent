---
gsd_state_version: 1.0
milestone: v2.0
milestone_name: Native Sandbox & Agent Isolation
status: Defining requirements
stopped_at: v2.1 requirements blocked — awaiting manual dontAsk mode testing
last_updated: "2026-03-24T19:29:45.319Z"
last_activity: 2026-03-24 — Milestone v2.1 started
progress:
  total_phases: 3
  completed_phases: 3
  total_plans: 6
  completed_plans: 6
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-03-24)

**Core value:** Run multiple autonomous Claude Code agents safely -- each sandboxed by native OS-level isolation, orchestrated by a single CLI command.
**Current focus:** Defining requirements for v2.1

## Current Position

Phase: Not started (defining requirements)
Plan: —
Status: Defining requirements
Last activity: 2026-03-24 — Milestone v2.1 started

## Performance Metrics

**Velocity:**

- Total plans completed: 0 (v2.0)
- Average duration: -
- Total execution time: 0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| - | - | - | - |

**Recent Trend:**

- Last 5 plans: -
- Trend: -

*Updated after each plan completion*
| Phase 05 P01 | 9min | 2 tasks | 20 files |
| Phase 05 P02 | 2min | 2 tasks | 1 files |
| Phase 06 P01 | 4min | 2 tasks | 8 files |
| Phase 06 P02 | 2min | 2 tasks | 3 files |
| Phase 07 P02 | 1min | 1 tasks | 1 files |
| Phase 07 P01 | 2min | 1 tasks | 1 files |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- [v2.0 roadmap]: CC native sandbox (bubblewrap/Seatbelt) replaces OpenShell -- no API key, simpler stack
- [v2.0 roadmap]: HOME override deferred to v2.1 -- edge cases with trust files, git/SSH, Telegram, credentials
- [v2.0 roadmap]: Coarse granularity -- 3 phases (remove OpenShell, add sandbox config, update tooling)
- [Phase 05]: Kept --no-sandbox CLI flag as no-op for Phase 6 sandbox config reuse
- [Phase 05]: state.rs replaces sandbox.rs with sandbox-agnostic structs for clean Phase 6 foundation
- [Phase 05]: v1 state.json backward compat verified -- serde ignores unknown fields (sandbox_name, no_sandbox) in simplified structs
- [Phase 06]: denyRead defaults include ~/.ssh, ~/.aws, ~/.gnupg for security-first sandbox config
- [Phase 06]: excludedCommands omitted from settings JSON when empty for cleaner output
- [Phase 06]: no_sandbox hardcoded to false for init (sandbox always enabled for fresh agents)
- [Phase 06]: Synthetic AgentDef in init.rs reuses codegen path -- single source of truth for settings.json
- [Phase 07]: Selective sandbox dep install: only missing packages installed (bwrap/socat checked individually)
- [Phase 07]: bwrap smoke test uses --unshare-net to match CC sandbox-runtime code path

### Pending Todos

None yet.

### Blockers/Concerns

- Ubuntu 24.04+ AppArmor blocks unprivileged bubblewrap -- doctor must detect and guide fix (Phase 7)
- Write/Edit tools bypass bwrap sandbox in bypassPermissions mode -- CC limitation, accepted constraint

## Session Continuity

Last session: 2026-03-24T19:29:45.316Z
Stopped at: v2.1 requirements blocked — awaiting manual dontAsk mode testing
Resume file: .planning/research/SUMMARY.md
