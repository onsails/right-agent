---
phase: 27-cron-runtime
plan: 02
subsystem: mcp
tags: [rmcp, rusqlite, mcp-tools, cron, sqlite]

# Dependency graph
requires:
  - phase: 27-cron-runtime-01
    provides: cron_runs V3 migration in memory.db + log capture infra

provides:
  - MCP server renamed to 'rightclaw' via Implementation::new() in get_info()
  - cron_list_runs tool: filters by job_name, limit, ordered by started_at DESC
  - cron_show_run tool: returns full row or 'not found' message for unknown run_id
  - 7 unit tests covering all new behaviors

affects: [27-cron-runtime, cronsync-skill, agent-observability]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "rmcp server rename via Implementation::new(name, version) passed to with_server_info()"
    - "cron_run_to_json() helper mirrors entry_to_json() pattern for structured row output"
    - "Optional filter via SQL NULL check: WHERE (?1 IS NULL OR job_name = ?1)"

key-files:
  created: []
  modified:
    - crates/rightclaw-cli/src/memory_server.rs

key-decisions:
  - "Implementation::new('rightclaw', CARGO_PKG_VERSION) — cleaner than struct update syntax"
  - "server_info is non-Optional in rmcp 1.3 InitializeResult — access as info.server_info.name directly"
  - "cron_show_run returns Ok with 'not found' text for missing IDs — consistent with D-05 spec"

patterns-established:
  - "Insert test data via locked conn before tool calls — WAL mode allows concurrent reads in tests"

requirements-completed: []

# Metrics
duration: 3min
completed: 2026-04-01
---

# Phase 27 Plan 02: Cron Runtime MCP Tools Summary

**MCP server renamed to 'rightclaw' with Implementation::new() + two cron observability tools backed by cron_runs SQLite table**

## Performance

- **Duration:** ~3 min
- **Started:** 2026-04-01T19:06:01Z
- **Completed:** 2026-04-01T19:08:21Z
- **Tasks:** 1
- **Files modified:** 1

## Accomplishments
- Renamed MCP server display name from Cargo package default to "rightclaw" via `with_server_info(Implementation::new(...))`
- Added `cron_list_runs` tool: optional `job_name` filter + `limit`, ordered by `started_at DESC`, returns `log_path` field
- Added `cron_show_run` tool: retrieves single row by `run_id`, returns graceful "not found" message instead of error
- Added helper `cron_run_to_json()` for consistent row serialization
- Added `CronListRunsParams` and `CronShowRunParams` parameter structs
- 7 unit tests: all pass — get_info name, list empty, list 2 rows, filter by job_name, limit, show found, show not found
- Updated `get_info()` instructions to enumerate all 6 tools

## Task Commits

1. **Task 1: Rename MCP server and add cron tools** - `22097e0` (feat)

**Plan metadata:** (docs commit — see below)

## Files Created/Modified
- `crates/rightclaw-cli/src/memory_server.rs` - Added Implementation import, CronListRunsParams/CronShowRunParams, cron_run_to_json(), cron_list_runs/cron_show_run tools, updated get_info(), 7 tests

## Decisions Made
- Used `Implementation::new("rightclaw", env!("CARGO_PKG_VERSION"))` — cleaner than struct update syntax with `..Default::default()`
- `server_info` field in rmcp 1.3 `InitializeResult` is a non-Optional `Implementation` — test accesses `info.server_info.name` directly (no `.as_ref().unwrap()`)
- `cron_show_run` returns `Ok(CallToolResult::success(...))` with "not found" text for missing IDs — consistent with D-05 spec (not an MCP error)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed test accessing server_info.as_ref() on non-Optional field**
- **Found during:** Task 1 (writing tests)
- **Issue:** Plan's test spec used `.as_ref().expect(...)` pattern assuming `server_info` was `Option<Implementation>`, but rmcp 1.3 has it as a plain `Implementation` field
- **Fix:** Changed `info.server_info.as_ref().expect("server_info present")` to `info.server_info.name` direct access
- **Files modified:** crates/rightclaw-cli/src/memory_server.rs
- **Verification:** Compile error resolved, test passes
- **Committed in:** 22097e0 (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (Rule 1 - bug)
**Impact on plan:** Minor API shape correction. No scope change.

## Issues Encountered
- rmcp 1.3 `server_info` is a required `Implementation` field (not `Option`) — test pattern from plan spec adjusted accordingly

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- D-05 complete: agents can now call `cron_list_runs` and `cron_show_run` via the rightclaw MCP server
- Phase 28 (cronsync skill rewrite) can reference these tools in SKILL.md
- MCP server identity is now "rightclaw" matching product name

---
*Phase: 27-cron-runtime*
*Completed: 2026-04-01*

## Self-Check: PASSED

- FOUND: crates/rightclaw-cli/src/memory_server.rs
- FOUND: .planning/phases/27-cron-runtime/27-02-SUMMARY.md
- FOUND: commit 22097e0 (feat task)
- FOUND: commit ffb7676 (docs metadata)
