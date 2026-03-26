---
phase: 17-memory-skill
plan: "01"
subsystem: database
tags: [rusqlite, sqlite, fts5, injection-guard, memory, owasp]

# Dependency graph
requires:
  - phase: 16-db-foundation
    provides: SQLite schema (memories, memory_events, memories_fts), open_db(), WAL migrations
provides:
  - has_injection() scanning 15 OWASP-derived injection patterns via substring match
  - open_connection() returning live rusqlite::Connection with WAL + migrations applied
  - store_memory() — injection-guarded INSERT with audit event
  - recall_memories() — LIKE search on tags/content, excludes soft-deleted
  - search_memories() — FTS5 BM25-ranked search, excludes soft-deleted
  - forget_memory() — soft-delete with audit trail, NotFound on missing ID
  - MemoryEntry struct
  - InjectionDetected and NotFound(i64) error variants
affects: [17-02-memory-mcp-server]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Injection guard: lowercase once then any(|pat| lower.contains(pat)) — O(n*k) for small k"
    - "TDD in Rust: test behavior block first, then implementation to pass"
    - "External test file via #[path = store_tests.rs] mod tests — keeps store.rs under 900 LoC"
    - "rusqlite::OptionalExtension needed for .optional() on query_row results"

key-files:
  created:
    - crates/rightclaw/src/memory/guard.rs
    - crates/rightclaw/src/memory/store.rs
    - crates/rightclaw/src/memory/store_tests.rs
  modified:
    - crates/rightclaw/src/memory/mod.rs
    - crates/rightclaw/src/memory/error.rs

key-decisions:
  - "Use str::contains() on lowercased input over 15-pattern list — no regex crate needed"
  - "open_connection() returns live Connection (vs open_db() which drops it) for use by store ops"
  - "store_memory() calls guard before any DB write — injection check is first line of function"
  - "forget_memory() soft-deletes (sets deleted_at) — never hard-deletes; audit trail preserved"

patterns-established:
  - "Memory CRUD layer: pure Rust fns taking &Connection, no async, no global state"
  - "Injection guard is the first call in store_memory — cannot be bypassed by refactoring"

requirements-completed: [SKILL-01, SKILL-02, SKILL-03, SKILL-04, SEC-01]

# Metrics
duration: 4min
completed: 2026-03-26
---

# Phase 17 Plan 01: Memory Library Layer Summary

**Injection-guarded SQLite CRUD layer (store/recall/search/forget) with FTS5 BM25 search, soft-delete audit trail, and 44 passing tests against real SQLite**

## Performance

- **Duration:** ~4 min
- **Started:** 2026-03-26T22:18:11Z
- **Completed:** 2026-03-26T22:21:30Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments

- `guard.rs`: `has_injection()` checks 15 OWASP LLM01:2025-derived patterns case-insensitively; zero false positives on the test set
- `store.rs`: 4 CRUD functions backed by the Phase 16 schema — all use `&Connection` directly (sync, no async)
- `store_tests.rs`: 17 tests covering all CRUD paths + injection rejection + soft-delete exclusion
- `mod.rs`: `open_connection()` added alongside existing `open_db()` — same WAL+migrations logic, returns live `Connection`
- `error.rs`: `InjectionDetected` and `NotFound(i64)` variants added

## Task Commits

Each task was committed atomically:

1. **Task 1: Injection guard + open_connection helper** - `5ce9a18` (feat)
2. **Task 2: Memory CRUD store operations** - `2da732a` (feat)

## Files Created/Modified

- `crates/rightclaw/src/memory/guard.rs` — `has_injection()` + `INJECTION_PATTERNS` (15 entries) + 15 tests
- `crates/rightclaw/src/memory/store.rs` — `store_memory`, `recall_memories`, `search_memories`, `forget_memory`, `MemoryEntry`
- `crates/rightclaw/src/memory/store_tests.rs` — 17 integration tests using real tempdir SQLite
- `crates/rightclaw/src/memory/mod.rs` — added `pub mod guard`, `pub mod store`, `open_connection()`, 3 new tests
- `crates/rightclaw/src/memory/error.rs` — added `InjectionDetected` and `NotFound(i64)` variants

## Decisions Made

- `str::contains()` over regex: 15 fixed literals with to_lowercase() once — faster, zero compile overhead, matches SEC-01 research recommendation
- `open_connection()` as separate function from `open_db()` to preserve existing cmd_up callers while enabling store operations
- Injection guard is the **first line** of `store_memory()` — structural guarantee it cannot be bypassed by refactoring paths

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added `rusqlite::OptionalExtension` import**
- **Found during:** Task 2 (forget_memory implementation)
- **Issue:** `.optional()` on `query_row` result required `OptionalExtension` trait in scope; compiler error
- **Fix:** Added `use rusqlite::OptionalExtension;` to `store.rs`
- **Files modified:** `crates/rightclaw/src/memory/store.rs`
- **Verification:** Compiled and all 17 tests passed
- **Committed in:** `2da732a` (Task 2 commit)

**2. [Rule 1 - Bug] Fixed empty-line-after-doc-comment clippy warning in guard.rs**
- **Found during:** Task 2 (clippy run)
- **Issue:** Empty line between two doc comment blocks triggered `empty_line_after_doc_comments` lint
- **Fix:** Merged doc comments into a single block with no empty line
- **Files modified:** `crates/rightclaw/src/memory/guard.rs`
- **Committed in:** `2da732a` (Task 2 commit)

---

**Total deviations:** 2 auto-fixed (1 blocking import, 1 clippy lint)
**Impact on plan:** Both minimal — one missing trait import, one doc comment formatting. No scope change.

## Issues Encountered

None beyond the two auto-fixed deviations above.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- All 4 CRUD functions are tested against real SQLite and ready for Plan 02 (MCP server)
- `open_connection()` is the entry point Plan 02 will call from the MCP server binary
- `store_memory()` injection guard is in place — Plan 02 tool handlers call these functions directly
- `MemoryError::InjectionDetected` maps to `McpError::invalid_params` in the MCP layer

---
*Phase: 17-memory-skill*
*Completed: 2026-03-26*
