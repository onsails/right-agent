---
phase: 18-cli-inspection
plan: "01"
subsystem: database
tags: [sqlite, rusqlite, fts5, serde, memory-store]

# Dependency graph
requires:
  - phase: 17-memory-skill
    provides: store.rs with store_memory/recall_memories/search_memories/forget_memory; MemoryEntry struct; memory DB schema with FTS5
provides:
  - list_memories(conn, limit, offset) — non-deleted rows ordered by created_at DESC, id DESC
  - search_memories_paged(conn, query, limit, offset) — FTS5 with explicit pagination (no LIMIT 50 cap)
  - hard_delete_memory(conn, id) — removes memories row; NotFound on absent id; succeeds on soft-deleted rows
  - MemoryEntry derives serde::Serialize — JSON output for CLI
  - mod.rs re-exports all store functions and MemoryEntry at crate::memory level
affects: [18-cli-inspection plan 02 — CLI commands depend on these store functions]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Secondary sort by id DESC for deterministic pagination when created_at timestamps collide"
    - "hard_delete uses SELECT 1 without deleted_at filter — operators can remove any existing row"
    - "All store functions re-exported from memory mod.rs — CLI crate calls rightclaw::memory::*"

key-files:
  created: []
  modified:
    - crates/rightclaw/src/memory/store.rs
    - crates/rightclaw/src/memory/store_tests.rs
    - crates/rightclaw/src/memory/mod.rs

key-decisions:
  - "list_memories uses ORDER BY created_at DESC, id DESC — secondary sort ensures deterministic order when timestamps tie in tests"
  - "hard_delete_memory checks existence without deleted_at filter — operators can hard-delete soft-deleted rows (differs from forget_memory)"
  - "MemoryEntry derives serde::Serialize only — no Deserialize needed (DB is the source, not user input)"
  - "search_memories unchanged (LIMIT 50 hardcoded) — MCP skill must not regress; paged variant is separate function"

patterns-established:
  - "TDD cycle: RED (compile error), fix with minimal implementation, GREEN (all pass)"

requirements-completed: [CLI-01, CLI-02, CLI-03, CLI-04]

# Metrics
duration: ~4min
completed: 2026-03-26
---

# Phase 18 Plan 01: Memory Store Data Layer Summary

**Three CLI-facing store functions (list_memories, search_memories_paged, hard_delete_memory) + serde::Serialize on MemoryEntry + full mod.rs re-exports — data layer ready for CLI inspection commands**

## Performance

- **Duration:** ~4 min
- **Started:** 2026-03-26T23:11:52Z
- **Completed:** 2026-03-26T23:14:47Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments

- `list_memories(conn, limit, offset)` lists non-deleted memories ordered by `created_at DESC, id DESC` — deterministic pagination
- `search_memories_paged(conn, query, limit, offset)` provides FTS5 search with operator-controlled pagination (unlike `search_memories` which is hardcoded at LIMIT 50 for MCP)
- `hard_delete_memory(conn, id)` removes the memories row entirely; works on soft-deleted rows; returns `NotFound` when id is absent
- `MemoryEntry` now derives `serde::Serialize` — enables JSON output for CLI `--json` flag
- All seven store functions + MemoryEntry re-exported from `crate::memory` — CLI crate can import cleanly
- 12 new tests (7 for list/search_paged in Task 1, 5 for hard_delete/serialize in Task 2); total test count: 221 → 226

## Task Commits

1. **Task 1: list_memories and search_memories_paged** - `abc98d3` (feat)
2. **Task 2: hard_delete_memory, Serialize derive, mod.rs re-exports** - `d5d50fc` (feat)

## Files Created/Modified

- `crates/rightclaw/src/memory/store.rs` — Added `list_memories`, `search_memories_paged`, `hard_delete_memory`; added `serde::Serialize` to `MemoryEntry` derive
- `crates/rightclaw/src/memory/store_tests.rs` — 12 new tests covering all three functions plus Serialize behavior
- `crates/rightclaw/src/memory/mod.rs` — Re-exports all store functions and `MemoryEntry` at module level

## Decisions Made

- `list_memories` uses `ORDER BY created_at DESC, id DESC`: secondary sort is essential for deterministic behavior when multiple entries share the same second-granularity timestamp (common in fast tests)
- `hard_delete_memory` does NOT filter by `deleted_at IS NULL` in the existence check — this is intentional: operators can hard-delete rows that are already soft-deleted
- `serde::Serialize` only on `MemoryEntry` (not `Deserialize`) — entries come from the DB, not user input
- `search_memories` left unchanged — CLI uses the new `search_memories_paged` variant; MCP skill must not regress

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Added secondary `id DESC` sort to `list_memories`**
- **Found during:** Task 1 (GREEN phase — test execution)
- **Issue:** `list_memories_returns_all_non_deleted_ordered_desc` and `list_memories_respects_offset` failed because all test inserts happen within the same second — SQLite's `created_at` has second granularity, so `ORDER BY created_at DESC` alone produces non-deterministic order among ties
- **Fix:** Changed query to `ORDER BY created_at DESC, id DESC` — `id` is autoincrement so higher = newer, breaking ties correctly
- **Files modified:** `crates/rightclaw/src/memory/store.rs`
- **Verification:** Both tests pass; all 226 tests pass
- **Committed in:** `abc98d3` (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (Rule 1 — bug in sort order)
**Impact on plan:** Fix is strictly better than the plan spec — secondary sort is always correct, not just for tests.

## Issues Encountered

None beyond the sort order deviation above.

## Next Phase Readiness

- All three store functions ready for consumption by `rightclaw memory list/search/delete` CLI commands (Plan 02)
- `MemoryEntry` is JSON-serializable — CLI `--json` flag can use `serde_json::to_string`
- `crate::memory::*` re-exports complete — Plan 02 CLI crate imports work without `store::` prefix

---
*Phase: 18-cli-inspection*
*Completed: 2026-03-26*
