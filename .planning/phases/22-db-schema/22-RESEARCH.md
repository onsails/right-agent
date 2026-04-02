# Phase 22: DB Schema - Research

**Researched:** 2026-03-31
**Domain:** SQLite schema migration via rusqlite_migration
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** `chat_id INT NOT NULL` — not nullable.
- **D-02:** `thread_id INT NOT NULL DEFAULT 0` — guards against thread_id=1 normalization bug.
- **D-03:** `root_session_id TEXT NOT NULL` — stores first-call session UUID; never updated on resume.
- **D-04:** `created_at TEXT NOT NULL DEFAULT (datetime('now'))` — same pattern as V1 schema.
- **D-05:** `last_used_at TEXT` — nullable. NULL on creation; updated each time session is resumed.
- **D-06:** `UNIQUE(chat_id, thread_id)` — composite key preventing duplicate session rows per thread.
- **D-07:** `telegram_sessions` stays Telegram-specific. Future channels each get their own table.
- **D-08:** Extend `memory/migrations.rs` — add `v2_telegram_sessions.sql` to `memory/sql/` and append `M::up(V2_SCHEMA)` to the migration vec. No new module for Phase 22.

### Claude's Discretion
- None specified.

### Deferred Ideas (OUT OF SCOPE)
- Multi-channel generic session table — rejected; per-channel tables only.
- Slack/Discord/webhook session tables — future phases when those channels are implemented.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SES-01 | `telegram_sessions` V2 migration added to memory.db: `(chat_id INT, thread_id INT NOT NULL DEFAULT 0, root_session_id TEXT NOT NULL, created_at, last_used_at, UNIQUE(chat_id, thread_id))` | rusqlite_migration positional vec pattern; V1 SQL conventions fully documented below |
</phase_requirements>

---

## Summary

Phase 22 is a pure schema migration. The entire work is: one SQL file (`v2_telegram_sessions.sql`) + one line change in `migrations.rs`. No new Rust modules, no CRUD, no business logic.

The existing migration infrastructure in `crates/rightclaw/src/memory/` is already the right pattern. `MIGRATIONS` is a `LazyLock<Migrations<'static>>` backed by a positional `vec![M::up(V1_SCHEMA)]`. Adding V2 means appending `M::up(V2_SCHEMA)` as the second element — rusqlite_migration tracks progress via `PRAGMA user_version` and applies only missing steps. Existing DBs get the migration applied automatically on next `open_db()` call.

The SQL conventions are fully established by V1: `INT` for integers, `TEXT` for strings and timestamps, `INTEGER PRIMARY KEY AUTOINCREMENT` for PKs, `CREATE TABLE IF NOT EXISTS` guard, `TEXT NOT NULL DEFAULT (datetime('now'))` for auto-timestamp columns, bare `TEXT` for nullable fields. V2 must follow these exactly.

**Primary recommendation:** Write `v2_telegram_sessions.sql` following V1 conventions, append to `MIGRATIONS` vec, add two tests (`user_version == 2` and `telegram_sessions` table exists).

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| rusqlite_migration | existing | Schema migration runner | Already in use (V1). Positional vec approach — order is the migration number. |
| rusqlite | existing | SQLite bindings | Already in use for all DB access. |

No new dependencies. This phase adds no crates.

## Architecture Patterns

### Migration Registration Pattern (from existing code)

`migrations.rs` holds a single `LazyLock<Migrations<'static>>`. Each migration is `M::up(SQL_CONST)` where `SQL_CONST` is loaded via `include_str!("sql/filename.sql")`. Position in the vec = migration version number. rusqlite_migration uses `PRAGMA user_version` to track which migrations have been applied.

```rust
// crates/rightclaw/src/memory/migrations.rs — current state
const V1_SCHEMA: &str = include_str!("sql/v1_schema.sql");

pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| Migrations::new(vec![M::up(V1_SCHEMA)]));
```

V2 addition — minimal diff:

```rust
// After change
const V1_SCHEMA: &str = include_str!("sql/v1_schema.sql");
const V2_SCHEMA: &str = include_str!("sql/v2_telegram_sessions.sql");

pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| Migrations::new(vec![M::up(V1_SCHEMA), M::up(V2_SCHEMA)]));
```

### SQL File Conventions (from v1_schema.sql)

```sql
-- V2 schema: telegram_sessions
-- Source: Phase 22 decision D-01..D-06

CREATE TABLE IF NOT EXISTS telegram_sessions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id         INT     NOT NULL,
    thread_id       INT     NOT NULL DEFAULT 0,
    root_session_id TEXT    NOT NULL,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    last_used_at    TEXT,
    UNIQUE(chat_id, thread_id)
);
```

Key conventions matched to V1:
- `INTEGER PRIMARY KEY AUTOINCREMENT` for PK (not `INT`)
- `INT` for application integers (chat_id, thread_id)
- `TEXT` for strings and timestamps
- `CREATE TABLE IF NOT EXISTS` guard
- `datetime('now')` inside parens for default expression
- No trailing comma after last column/constraint
- `UNIQUE(...)` as table-level constraint, not inline

### Anti-Patterns to Avoid
- **Adding an `id` column as `INT PRIMARY KEY`:** SQLite requires `INTEGER` (not `INT`) for the rowid alias — use `INTEGER PRIMARY KEY AUTOINCREMENT` exactly as V1 does.
- **Inline UNIQUE on a single column:** The constraint is composite `(chat_id, thread_id)` — must be table-level.
- **`NOT NULL DEFAULT NULL`:** Contradiction. Nullable columns (`last_used_at`) use bare `TEXT` with no DEFAULT clause (NULL is implicit default).
- **Skipping `IF NOT EXISTS`:** Without this guard, re-running the migration on a fresh DB via any tooling fails.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Migration version tracking | Custom PRAGMA user_version logic | rusqlite_migration | Already handles versioning, ordering, error recovery |
| Schema idempotency | Manual `DROP TABLE IF EXISTS` / existence checks | `CREATE TABLE IF NOT EXISTS` guard | Standard SQLite pattern; rusqlite_migration won't re-run applied steps |

## Common Pitfalls

### Pitfall 1: Migration Order Sensitivity
**What goes wrong:** Inserting V2 before V1 in the vec, or using a non-positional key, corrupts the version counter.
**Why it happens:** rusqlite_migration's version = 1-based index into the vec. Reordering changes what "version 2" means.
**How to avoid:** Always append new migrations to the END of the vec. Never insert in the middle.
**Warning signs:** Tests that assert `user_version == N` fail with off-by-one.

### Pitfall 2: PRAGMA user_version vs migration count mismatch
**What goes wrong:** Existing DB has `user_version = 1` (V1 applied). If V2 constant is added but NOT appended to the vec, `to_latest()` is a no-op and the table never gets created.
**Why it happens:** The vec length is the authoritative "latest" version. SQL file on disk without vec registration does nothing.
**How to avoid:** The SQL file addition and the vec append must be done together. Tests catch this.
**Warning signs:** `telegram_sessions` table missing even though SQL file exists.

### Pitfall 3: NULL semantics for last_used_at
**What goes wrong:** Adding `DEFAULT NULL` or `NOT NULL DEFAULT ''` to `last_used_at`, breaking the semantic contract (NULL = created but never resumed).
**Why it happens:** Developer instinct to be explicit; SQLite accepts both but they mean different things to application code.
**How to avoid:** Use bare `TEXT` with no DEFAULT clause. D-05 is explicit: NULL on creation, only updated by CRUD in Phase 25.
**Warning signs:** Phase 25 CRUD code cannot distinguish "fresh session" from "active session."

### Pitfall 4: thread_id normalization is application-layer only
**What goes wrong:** Adding a CHECK constraint `CHECK(thread_id != 1)` to enforce the normalization rule at DB level.
**Why it happens:** Temptation to enforce invariants in schema. But the normalization (Some(1) → 0) happens in `effective_thread_id()` helper (SES-04, Phase 25). The schema stores the already-normalized value.
**How to avoid:** No CHECK constraint on thread_id. Schema stores what application provides; application is responsible for normalization.
**Warning signs:** Valid test data (`thread_id=1`) inserted by Phase 25 tests gets rejected.

## Code Examples

### Test Pattern for V2 (following V1 test conventions in memory/mod.rs)

```rust
// Source: existing tests in crates/rightclaw/src/memory/mod.rs — extend this #[cfg(test)] module

#[test]
fn user_version_is_2() {
    let dir = tempdir().unwrap();
    open_db(dir.path()).unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2, "user_version should be 2 after V2 migration");
}

#[test]
fn schema_has_telegram_sessions_table() {
    let dir = tempdir().unwrap();
    open_db(dir.path()).unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='telegram_sessions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "telegram_sessions table should exist after V2 migration");
}
```

Note: the existing `user_version_is_1` test will fail once V2 is added — it must be updated to assert `== 2`.

### Constraint verification test (optional but recommended)

```rust
#[test]
fn telegram_sessions_unique_chat_thread() {
    let dir = tempdir().unwrap();
    open_db(dir.path()).unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
    conn.execute(
        "INSERT INTO telegram_sessions (chat_id, thread_id, root_session_id) VALUES (42, 0, 'uuid-1')",
        [],
    ).unwrap();
    let result = conn.execute(
        "INSERT INTO telegram_sessions (chat_id, thread_id, root_session_id) VALUES (42, 0, 'uuid-2')",
        [],
    );
    assert!(result.is_err(), "UNIQUE(chat_id, thread_id) should reject duplicate");
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| serde_yaml (deprecated) | serde-saphyr | March 2024 | No impact on this phase |
| Manual migration scripts | rusqlite_migration LazyLock vec | Phase 21 | Already applied — just append V2 |

No state changes relevant to this phase. rusqlite_migration is stable; the pattern is established.

## Open Questions

1. **`user_version_is_1` test update**
   - What we know: The existing test asserts `user_version == 1`. After V2 is added, this assertion fails.
   - What's unclear: Should the test be updated in-place to assert `== 2`, or replaced with two tests (one for each version transition)?
   - Recommendation: Update `user_version_is_1` to `user_version_is_2` in place — the old assertion is now obsolete. The migration correctness is validated by the test on a fresh DB; there is no need to keep V1-specific version assertion since `to_latest()` is idempotent.

## Environment Availability

Step 2.6: SKIPPED — phase is code/config changes only. No external dependencies beyond existing Rust toolchain and rusqlite (already present).

## Project Constraints (from CLAUDE.md)

Directives that constrain this phase:

| Directive | Impact |
|-----------|--------|
| Rust edition 2024 | All new Rust code uses `edition = "2024"` in Cargo.toml (already set) |
| Dependency versions: `x.x` format | Not applicable — no new dependencies |
| Error handling: propagate with `?` | Not applicable — SQL file has no Rust error handling |
| `thiserror` for library errors | Not applicable — no new error types in this phase |
| Tests in same file via `#[cfg(test)]` | V2 tests go in existing `memory/mod.rs` `#[cfg(test)]` block |
| File > 800 LoC with >50% tests: extract | Check `memory/mod.rs` line count after adding tests; currently 212 lines, adding ~30 lines is safe |
| Always use context7 for library docs | rusqlite_migration pattern is already understood from existing code — no new API surface |
| TDD: write failing test first | Write `user_version_is_2` and `telegram_sessions` table tests first; they fail until SQL file + vec append complete |

## Sources

### Primary (HIGH confidence)
- `crates/rightclaw/src/memory/migrations.rs` — live code showing exact LazyLock + vec pattern
- `crates/rightclaw/src/memory/sql/v1_schema.sql` — SQL conventions (column types, guards, timestamp patterns)
- `crates/rightclaw/src/memory/mod.rs` — `open_db`/`open_connection` and full test suite showing assertion patterns
- `.planning/phases/22-db-schema/22-CONTEXT.md` — locked schema decisions D-01..D-08
- `.planning/REQUIREMENTS.md` §SES-01 — exact column spec

### Secondary (MEDIUM confidence)
- None required — all findings come from primary sources.

### Tertiary (LOW confidence)
- None.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all patterns verified from live production code in this repo
- Architecture: HIGH — migration pattern is already working; V2 is additive only
- Pitfalls: HIGH — sourced from SQLite docs behavior and existing test patterns
- SQL conventions: HIGH — directly read from v1_schema.sql

**Research date:** 2026-03-31
**Valid until:** Stable — rusqlite_migration API is stable; conventions from V1 are fixed
