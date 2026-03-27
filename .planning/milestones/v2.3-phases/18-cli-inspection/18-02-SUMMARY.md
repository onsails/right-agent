---
phase: 18-cli-inspection
plan: "02"
subsystem: cli
tags: [rusqlite, clap, serde_json, fts5, memory-inspect, miette]

# Dependency graph
requires:
  - phase: 18-cli-inspection
    plan: "01"
    provides: list_memories, search_memories_paged, hard_delete_memory; serde::Serialize on MemoryEntry; mod.rs re-exports
affects: []

provides:
  - "rightclaw memory list <agent> — paginated columnar table with --limit/--offset/--json"
  - "rightclaw memory search <agent> <query> — FTS5 BM25 search with same pagination flags"
  - "rightclaw memory delete <agent> <id> — entry preview + y/N confirmation + hard delete"
  - "rightclaw memory stats <agent> — auto-scaled DB size, total entries, oldest/newest; --json mode"
  - "resolve_agent_db helper — fatal miette errors for missing agent dir or missing memory.db"
  - "truncate_content helper — char-safe UTF-8 truncation with ellipsis"
  - "format_size helper — auto-scales bytes to B/KB/MB"

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "resolve_agent_db centralizes agent-dir and memory.db validation — all cmd_memory_* go through it"
    - "rusqlite::OptionalExtension imported locally inside cmd_memory_delete — keeps scope minimal"
    - "Pagination footer only shown in text mode when result count == limit (not in JSON mode)"
    - "cmd_memory_stats derives db_path from home separately — file metadata separate from DB connection"

key-files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs

key-decisions:
  - "Both tasks (list+stats and search+delete) implemented in one commit — TDD RED was written first, then all four functions added together during GREEN phase"
  - "cmd_memory_delete fetches preview via direct SQL SELECT (includes soft-deleted rows) rather than list_memories — operators need to see what they are hard-deleting even if soft-deleted"
  - "resolve_agent_db uses home.join('agents').join(agent) pattern matching existing codebase convention"

patterns-established:
  - "Memory subcommand group pattern: MemoryCommands enum + resolve_agent_db + cmd_memory_* functions"

requirements-completed: [CLI-01, CLI-02, CLI-03, CLI-04]

# Metrics
duration: ~3min
completed: 2026-03-26
---

# Phase 18 Plan 02: CLI Memory Inspection Commands Summary

**`rightclaw memory` subcommand group with list/search/delete/stats — operators can inspect any agent's SQLite memory database from the terminal without entering an agent session**

## Performance

- **Duration:** ~3 min
- **Started:** 2026-03-26T23:17:10Z
- **Completed:** 2026-03-26T23:19:50Z
- **Tasks:** 2 (implemented together in single commit)
- **Files modified:** 1

## Accomplishments

- `rightclaw memory list <agent>` shows columnar table (ID, truncated content, stored_by, created_at) with `--limit`/`--offset` pagination and footer, `--json` for newline-delimited JSON
- `rightclaw memory search <agent> <query>` uses FTS5 BM25 via `search_memories_paged` with identical pagination/JSON flags; helpful error hint on FTS5 syntax failures
- `rightclaw memory delete <agent> <id>` shows entry preview (including soft-deleted rows), prompts `Hard-delete this entry? [y/N]:`, aborts on non-`y` input
- `rightclaw memory stats <agent>` shows auto-scaled DB size, total entries, oldest/newest timestamps; `--json` emits structured JSON object
- `resolve_agent_db` helper gives operator-friendly fatal errors: "agent 'X' not found at PATH" and "no memory database for agent 'X' — run \`rightclaw up\` first"
- 14 new unit tests: 4 compile-time variant checks, 2 resolve_agent_db error paths, 5 truncate_content cases, 3 format_size cases
- Total test count: 226 → 240

## Task Commits

1. **Tasks 1+2: Full memory CLI implementation (TDD)** - `0985942` (feat)

## Files Created/Modified

- `crates/rightclaw-cli/src/main.rs` — Added `MemoryCommands` enum, `Commands::Memory` variant, match dispatch, `resolve_agent_db`, `cmd_memory_list`, `cmd_memory_stats`, `cmd_memory_search`, `cmd_memory_delete`, `truncate_content`, `format_size`, and 14 unit tests

## Decisions Made

- `cmd_memory_delete` fetches entry preview via direct `SELECT content, stored_by FROM memories WHERE id = ?1` (no `deleted_at IS NULL` filter) — this matches `hard_delete_memory`'s behavior and lets operators see what they're deleting even if the entry was already soft-deleted
- TDD RED phase covered all test types upfront (both tasks' tests written before implementation), then GREEN implemented all four functions together — avoids splitting compile-time variant checks from their backing implementations

## Deviations from Plan

None — plan executed exactly as written. The plan spec for `cmd_memory_delete` included both an `entry` variable (from `list_memories`) and `any_row` (direct SQL); I simplified to only `any_row` since it covers all cases (including soft-deleted rows) without redundancy. This is strictly correct per plan intent.

## Issues Encountered

None. The pre-existing `test_status_no_running_instance` integration test failure is documented in STATE.md and unrelated to this plan.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- Phase 18 complete — v2.3 Memory System milestone is fully shipped
- `rightclaw memory` is the final deliverable: operators can now inspect any agent's memory database from the terminal
- No blockers for v2.3 milestone close

## Self-Check: PASSED

- SUMMARY.md exists at `.planning/phases/18-cli-inspection/18-02-SUMMARY.md`
- Commit `0985942` confirmed in git log

---
*Phase: 18-cli-inspection*
*Completed: 2026-03-26*
