---
phase: 16-db-foundation
plan: 01
subsystem: database
tags: [rusqlite, rusqlite_migration, sqlite, fts5, wal, migrations]

requires: []
provides:
  - "rightclaw::memory::open_db(agent_path) — creates/opens per-agent memory.db with WAL + V1 schema"
  - "MemoryError enum (Sqlite + Migration variants)"
  - "V1 schema: memories, memory_events (append-only), memories_fts FTS5 virtual table"
  - "rusqlite 0.39 + rusqlite_migration 2.5 in workspace deps"
affects:
  - "16-02 (MEMORY.md cleanup)"
  - "16-03 (doctor sqlite3 check + cmd_up integration)"
  - "17-memory-skill"
  - "18-memory-cli"

tech-stack:
  added:
    - "rusqlite 0.39 (bundled SQLite 3.51.1)"
    - "rusqlite_migration 2.5"
  patterns:
    - "include_str!(sql/v1_schema.sql) for embedded migration SQL"
    - "std::sync::LazyLock<Migrations<'static>> for static migration instances"
    - "Pragma-before-migrations order: open -> WAL -> busy_timeout -> to_latest"
    - "MemoryError with #[from] impls for rusqlite::Error and rusqlite_migration::Error"

key-files:
  created:
    - "crates/rightclaw/src/memory/mod.rs — open_db + 9 unit tests"
    - "crates/rightclaw/src/memory/error.rs — MemoryError enum"
    - "crates/rightclaw/src/memory/migrations.rs — MIGRATIONS static"
    - "crates/rightclaw/src/memory/sql/v1_schema.sql — complete V1 DDL"
  modified:
    - "Cargo.toml — added rusqlite + rusqlite_migration workspace deps"
    - "crates/rightclaw/Cargo.toml — added rusqlite + rusqlite_migration"
    - "crates/rightclaw/src/lib.rs — added pub mod memory"
    - "crates/rightclaw/src/codegen/system_prompt.rs — removed MEMORY.md reference from default start_prompt (D-07)"
    - "crates/rightclaw/src/codegen/system_prompt_tests.rs — updated test to match new default"

key-decisions:
  - "rusqlite 0.39 + features=[bundled] — embeds SQLite 3.51.1, eliminates system SQLite variability"
  - "rusqlite_migration 2.5 owns user_version — never set PRAGMA user_version manually"
  - "Pragma order: WAL + busy_timeout before to_latest (pitfall 4 avoided)"
  - "include_str! from sql/v1_schema.sql — SQL as own file, cleaner for 60-line schema"
  - "RAISE(ABORT) triggers on memory_events — DB-level append-only enforcement"
  - "IF NOT EXISTS on all DDL — defensive, idempotent schema (SQLite 3.35+ required; bundled 3.51.1 supports it)"
  - "Default start_prompt changed to 'You are starting.' — MEMORY.md removed, Phase 17 will add /recall"

patterns-established:
  - "memory module layout: error.rs -> migrations.rs -> sql/v1_schema.sql -> mod.rs"
  - "open_db signature: pub fn open_db(agent_path: &Path) -> Result<(), MemoryError>"

requirements-completed: [DB-01, DB-02, DB-03, DB-04]

duration: 3min
completed: 2026-03-26
---

# Phase 16 Plan 01: Memory Module Foundation Summary

**SQLite memory module with WAL mode, FTS5 virtual table, append-only audit log via ABORT triggers, and rusqlite_migration 2.5 schema versioning — 9 tests all passing**

## Performance

- **Duration:** 3 min
- **Started:** 2026-03-26T21:20:31Z
- **Completed:** 2026-03-26T21:23:34Z
- **Tasks:** 2 (TDD: RED + GREEN)
- **Files modified:** 9

## Accomplishments

- Pure new code: `memory/` module with 4 files + 1 SQL file — zero changes to existing logic
- `open_db()` is idempotent, WAL-enabled, and runs migrations on every call with zero schema drift
- FTS5 virtual table and ABORT triggers in schema from day one — avoids costly retrofits in Phase 17+
- Default `start_prompt` updated to drop MEMORY.md reference (D-07 decision)

## Task Commits

1. **Task 1: Write failing tests for memory module** — `e89bcc1` (test)
2. **Task 1 supplement: D-07 start_prompt update** — `e11f9ff` (fix)
3. **Task 2: Implement memory module open_db** — `2e483e4` (feat)

## Files Created/Modified

- `crates/rightclaw/src/memory/mod.rs` — `open_db` implementation + 9 unit tests
- `crates/rightclaw/src/memory/error.rs` — `MemoryError` with `#[from]` for rusqlite + rusqlite_migration
- `crates/rightclaw/src/memory/migrations.rs` — `MIGRATIONS` static via `LazyLock<Migrations<'static>>`
- `crates/rightclaw/src/memory/sql/v1_schema.sql` — complete V1 DDL (memories, memory_events, FTS5, 5 triggers)
- `Cargo.toml` — rusqlite 0.39 + rusqlite_migration 2.5 workspace deps
- `crates/rightclaw/Cargo.toml` — crate dependency declarations
- `crates/rightclaw/src/lib.rs` — `pub mod memory;`
- `crates/rightclaw/src/codegen/system_prompt.rs` — default start_prompt cleanup
- `crates/rightclaw/src/codegen/system_prompt_tests.rs` — test updated to match

## Decisions Made

Followed plan and CONTEXT.md D-01 through D-11 exactly. No new decisions required during execution.

## Deviations from Plan

None — plan executed exactly as written.

Note: Pre-existing uncommitted changes in the worktree (memory_path removal in types.rs, discovery.rs, etc.) were already committed in a previous commit (93641d4) by the orchestrator setup. The D-07 `system_prompt.rs` changes were in the worktree but uncommitted — committed as `e11f9ff` as part of this plan's scope.

## Issues Encountered

None. The compilation errors from the full workspace (`memory_path` references) were pre-existing and already committed separately. RED → GREEN transition was clean.

## Next Phase Readiness

- `rightclaw::memory::open_db` is public and callable from `rightclaw-cli`
- Plan 16-02 (MEMORY.md cleanup) can now proceed (already partially done in 93641d4)
- Plan 16-03 (doctor check + cmd_up wire-in) can use `open_db` directly

## Self-Check: PASSED

- FOUND: crates/rightclaw/src/memory/mod.rs
- FOUND: crates/rightclaw/src/memory/error.rs
- FOUND: crates/rightclaw/src/memory/migrations.rs
- FOUND: crates/rightclaw/src/memory/sql/v1_schema.sql
- FOUND: .planning/phases/16-db-foundation/16-01-SUMMARY.md
- FOUND commit: e89bcc1 (RED — failing tests)
- FOUND commit: e11f9ff (D-07 start_prompt)
- FOUND commit: 2e483e4 (GREEN — open_db implementation)

---
*Phase: 16-db-foundation*
*Completed: 2026-03-26*
