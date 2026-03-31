---
phase: 22-db-schema
plan: 01
subsystem: database
tags: [sqlite, rusqlite, rusqlite_migration, telegram_sessions, schema]

# Dependency graph
requires:
  - phase: 16-memory
    provides: "rusqlite_migration infrastructure, memory.db V1 schema, open_db/open_connection API"
provides:
  - "telegram_sessions table in memory.db (V2 migration)"
  - "user_version advances to 2 after migration"
  - "UNIQUE(chat_id, thread_id) composite constraint"
  - "thread_id NOT NULL DEFAULT 0 guard against Telegram General topic normalization"
  - "last_used_at bare TEXT (nullable, no DEFAULT) for resume tracking"
affects: [25-telegram-handler, 26-session-crud]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "V2 schema follows V1 SQL conventions: INTEGER PRIMARY KEY AUTOINCREMENT, INT for app integers, TEXT for strings/timestamps, CREATE TABLE IF NOT EXISTS guard"
    - "rusqlite_migration positional versioning: V2_SCHEMA must be second vec element; index determines user_version"
    - "bare TEXT (no NOT NULL, no DEFAULT) for nullable columns that start as NULL"

key-files:
  created:
    - crates/rightclaw/src/memory/sql/v2_telegram_sessions.sql
  modified:
    - crates/rightclaw/src/memory/migrations.rs
    - crates/rightclaw/src/memory/mod.rs

key-decisions:
  - "root_session_id is NOT NULL TEXT — stores first-call session UUID only; Phase 25 CRUD must never UPDATE this on resume (CC bug #8069)"
  - "thread_id INT NOT NULL DEFAULT 0 — application-layer normalization only, no CHECK constraint (RESEARCH.md pitfall 4)"
  - "last_used_at bare TEXT with no DEFAULT and no NOT NULL — NULL means created-but-never-resumed"

patterns-established:
  - "TDD RED/GREEN: failing tests committed first, implementation committed separately"
  - "Migration vec order is authoritative for user_version; V1 stays at index 0, V2 at index 1"

requirements-completed: [SES-01]

# Metrics
duration: 2min
completed: 2026-03-31
---

# Phase 22 Plan 01: DB Schema Summary

**telegram_sessions V2 migration: composite-keyed session table with UNIQUE(chat_id, thread_id), thread_id DEFAULT 0 guard, and nullable last_used_at for resume tracking**

## Performance

- **Duration:** ~2 min
- **Started:** 2026-03-31T19:23:14Z
- **Completed:** 2026-03-31T19:24:44Z
- **Tasks:** 2 (TDD RED + GREEN)
- **Files modified:** 3

## Accomplishments

- V2 migration SQL file with all D-01..D-06 constraints locked in Phase 22 context
- migrations.rs updated with V2_SCHEMA as second positional element — user_version advances to 2
- Three new tests: user_version_is_2, schema_has_telegram_sessions_table, telegram_sessions_unique_chat_thread
- All 244 lib tests pass; workspace builds clean

## Task Commits

1. **Task 1 (RED): Write failing V2 migration tests** - `aedcd23` (test)
2. **Task 2 (GREEN): Write SQL file and register V2 migration** - `f10bebc` (feat)

## Files Created/Modified

- `crates/rightclaw/src/memory/sql/v2_telegram_sessions.sql` - V2 CREATE TABLE with all constraints
- `crates/rightclaw/src/memory/migrations.rs` - V2_SCHEMA constant + second vec element
- `crates/rightclaw/src/memory/mod.rs` - Three new/updated tests

## Decisions Made

- No CHECK constraint on thread_id — normalization is application-layer only (per RESEARCH.md pitfall 4)
- INT (not INTEGER) for chat_id and thread_id — these are app integers, not rowid aliases
- last_used_at is bare TEXT with no modifiers — NULL = "created, never resumed"; Phase 25 CRUD updates it on resume

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- memory.db V2 schema is ready; Phase 25 Telegram handler can INSERT/SELECT/UPDATE telegram_sessions
- Phase 25 must never UPDATE root_session_id on resume (CC bug #8069 — resume returns new session_id)
- thread_id=0 is the canonical value for non-threaded chats (Group main topic maps to 0)

---
*Phase: 22-db-schema*
*Completed: 2026-03-31*
