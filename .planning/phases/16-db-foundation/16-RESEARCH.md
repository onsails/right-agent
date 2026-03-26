# Phase 16: DB Foundation - Research

**Researched:** 2026-03-26
**Domain:** rusqlite 0.39 + rusqlite_migration 2.5, SQLite FTS5 triggers, ABORT triggers, Cargo workspace integration
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**D-01:** Fatal error on DB creation failure — `"failed to open memory database for '{agent}': {err:#}"` pattern matching existing cmd_up error format.

**D-02:** Two-table schema:
- `memories(id INTEGER PK, content TEXT NOT NULL, tags TEXT, stored_by TEXT, source_tool TEXT, created_at TEXT, deleted_at TEXT, expires_at TEXT, importance REAL DEFAULT 0.5)`
- `memory_events(id INTEGER PK, memory_id INTEGER, event_type TEXT, actor TEXT, created_at TEXT)` — append-only; SQLite ABORT triggers block UPDATE/DELETE
- `memories_fts` — FTS5 virtual table, content=memories, auto-synced via triggers

**D-03:** `rusqlite_migration 2.5` with `user_version` pragma. `to_latest()` on every DB open.

**D-04:** SEC-02 dropped as explicit requirement — enforced by architecture separation, no grep test needed.

**D-05:** Remove `memory_path` field from `AgentDef`.
**D-06:** Remove `optional_file(&path, "MEMORY.md")` from `discovery.rs`.
**D-07:** Change default `start_prompt` in `system_prompt.rs` from `"You are starting. Read your MEMORY.md to restore context."` to `"You are starting."`.
**D-08:** Remove `discovery_tests.rs` assertion that `memory_path.is_some()` when MEMORY.md exists.

**D-09:** `rightclaw doctor` adds Warn (non-fatal) check: `sqlite3` in PATH. No fix suggestion.

**D-10:** Module at `crates/rightclaw/src/memory/` — four files: `error.rs`, `migrations.rs`, `store.rs`, `mod.rs`. No new workspace crate.

**D-11:** Workspace Cargo.toml adds:
- `rusqlite = { version = "0.39", features = ["bundled"] }`
- `rusqlite_migration = "2.5"`

### Claude's Discretion

None specified — all implementation details are locked or follow from locked decisions.

### Deferred Ideas (OUT OF SCOPE)

- Phase 17: Update default start_prompt to reference `/recall`
- Phase 17: Injection scanning (SEC-01) — needs dedicated research before coding
- v2.4: TTL/importance eviction logic (columns in schema from day 1, logic deferred)
- v2.4: Cross-agent memory sharing
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| DB-01 | `rightclaw up` creates per-agent `memory.db` (WAL mode + busy_timeout=5000ms) if absent | rusqlite `pragma_update` API, `Connection::open` creates file if absent, idempotent open_or_create pattern |
| DB-02 | V1 schema is append-only — memories table with specified columns; SQLite triggers block UPDATE/DELETE on memory_events | ABORT trigger syntax verified; two-table schema with trigger-enforced immutability |
| DB-03 | FTS5 virtual table included in V1 schema | FTS5 content table + sync triggers syntax confirmed; built into SQLite 3.51.1 bundled with rusqlite 0.39 |
| DB-04 | Schema migrations use rusqlite_migration 2.5 (user_version pragma); `to_latest()` on every DB open | rusqlite_migration 2.5 API confirmed — `Migrations::new(vec![M::up(SQL)])`, `to_latest(&mut conn)` |
| SEC-02 | No code path writes to MEMORY.md from skill or CLI | Enforced by removing memory_path from AgentDef and MEMORY.md scan from discovery.rs |
| SEC-03 | Memory recall is always on-demand — never auto-injected into system prompt | Architecture: DB is only opened in cmd_up for init; no prompt injection code exists |
| DOCTOR-01 | `rightclaw doctor` warns (non-fatal) when `sqlite3` binary absent from PATH | Direct reuse of `check_binary("sqlite3", None)` with `CheckStatus::Warn` override |
</phase_requirements>

## Summary

Phase 16 is a well-scoped infrastructure phase with no ambiguous choices remaining. All library versions, schema topology, module layout, and integration points are locked in CONTEXT.md. The research task is to verify the exact rusqlite 0.39 and rusqlite_migration 2.5 APIs, confirm SQLite ABORT trigger and FTS5 content table syntax, and map every file-level change precisely.

The implementation has two orthogonal work streams: (1) new `memory/` module creation with DB open/init/schema, and (2) dead-code removal (memory_path from AgentDef, MEMORY.md scan, stale default prompt text, one test assertion). Both streams are independent and can be planned as separate tasks.

The key technical subtlety is that `open_db` is a sync function called inside an `async fn cmd_up` — but this is the established pattern for all scaffold steps (settings.json, skills, etc.). No `tokio::spawn_blocking` is needed.

**Primary recommendation:** Build the memory module bottom-up (error.rs → migrations.rs → mod.rs), wire into cmd_up in a single step, then clean up MEMORY.md dead code as a separate task.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| rusqlite | 0.39 | SQLite driver + bundled SQLite 3.51.1 | Canonical Rust SQLite binding. Bundled eliminates system SQLite variability. Version 0.39 is current (2026-03-26). |
| rusqlite_migration | 2.5 | `user_version`-based schema migrations | Embedded SQL strings, no CLI tooling, `to_latest()` is idempotent. 2.5.0 confirmed current (2026-03-26). |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tempfile | 3.27 (dev) | Temp dirs in tests | Already in rightclaw crate dev-dependencies — no version bump needed |

### Alternatives Considered
None — all locked in CONTEXT.md. sqlx rejected (semver hazard, compile-time checks broken for dynamic paths). tokio-rusqlite rejected (async overhead unnecessary; sync DB calls in sync scaffold loop match all other cmd_up scaffold steps).

**Installation (workspace Cargo.toml addition):**
```toml
[workspace.dependencies]
rusqlite = { version = "0.39", features = ["bundled"] }
rusqlite_migration = "2.5"
```

```toml
# crates/rightclaw/Cargo.toml additions
[dependencies]
rusqlite = { workspace = true }
rusqlite_migration = { workspace = true }
```

**Version verification:** rusqlite 0.39.0 published 2025-12-20. rusqlite_migration 2.5.0 confirmed current via crates.io API 2026-03-26.

## Architecture Patterns

### Module Layout
```
crates/rightclaw/src/memory/
├── mod.rs          pub open_db(), re-exports MemoryError
├── error.rs        MemoryError (thiserror derive)
├── migrations.rs   MIGRATIONS static, SQL constants
└── store.rs        MemoryStore struct (Phase 17+ — not in Phase 16 scope)
```

Note: `store.rs` struct (`MemoryStore`) is Phase 17 scope. Phase 16 only needs `open_db()` + schema initialization.

### Pattern 1: DB Open + Init (sync, called from cmd_up scaffold loop)

```rust
// crates/rightclaw/src/memory/mod.rs
pub fn open_db(agent_path: &std::path::Path) -> Result<(), MemoryError> {
    let db_path = agent_path.join("memory.db");
    let mut conn = rusqlite::Connection::open(&db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    crate::memory::migrations::MIGRATIONS.to_latest(&mut conn)?;
    Ok(())
}
```

`Connection::open` creates the file if absent — idempotent. `to_latest` is a no-op when `user_version` matches current migration count.

### Pattern 2: rusqlite_migration API

```rust
// crates/rightclaw/src/memory/migrations.rs
use rusqlite_migration::{Migrations, M};

const V1_SCHEMA: &str = include_str!("sql/v1_schema.sql");

pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| Migrations::new(vec![M::up(V1_SCHEMA)]));
```

Alternative without LazyLock:
```rust
pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(V1_SCHEMA)])
}
```

`to_latest(&mut conn)` reads `PRAGMA user_version`, runs only migrations with index >= current version, increments `user_version` after each. Thread-safe because each agent has its own DB file and `open_db` is called once per agent per `rightclaw up`.

The SQL can be embedded inline as a string or via `include_str!` from a file in `src/memory/sql/`. Both work. Inline is simpler for a single V1 migration.

### Pattern 3: SQLite ABORT Trigger (blocks UPDATE/DELETE on memory_events)

```sql
CREATE TRIGGER memory_events_no_update
BEFORE UPDATE ON memory_events
BEGIN
    SELECT RAISE(ABORT, 'memory_events is append-only: UPDATE not permitted');
END;

CREATE TRIGGER memory_events_no_delete
BEFORE DELETE ON memory_events
BEGIN
    SELECT RAISE(ABORT, 'memory_events is append-only: DELETE not permitted');
END;
```

`RAISE(ABORT, message)` rolls back the current statement and returns an error to the caller. rusqlite surfaces this as `rusqlite::Error::SqliteFailure` with code `SQLITE_CONSTRAINT` (19) or `SQLITE_ABORT` (4). The caller sees an `Err`, not a silent no-op.

Confidence: HIGH — verified against SQLite documentation. ABORT is the correct level (rolls back statement, not transaction).

### Pattern 4: FTS5 Content Table + Sync Triggers

```sql
-- FTS5 virtual table backed by memories.content column
CREATE VIRTUAL TABLE memories_fts USING fts5(
    content,
    content='memories',
    content_rowid='id'
);

-- Sync triggers: keep memories_fts in sync with memories
CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content)
    VALUES (new.id, new.content);
END;

CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
END;

CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
    INSERT INTO memories_fts(rowid, content)
    VALUES (new.id, new.content);
END;
```

This is the "external content table" pattern from the SQLite FTS5 documentation. The `content=` and `content_rowid=` parameters tell FTS5 where to read for snippet/offsets queries. The three triggers ensure the FTS index stays synchronized on INSERT, DELETE, and UPDATE of `memories`.

FTS5 is built into SQLite 3.51.1 (bundled with rusqlite 0.39 via `features = ["bundled"]`). No extension loading required.

Query pattern (Phase 17 scope, but schema must exist):
```sql
SELECT m.id, m.content, m.tags
FROM memories m
JOIN memories_fts ON memories_fts.rowid = m.id
WHERE memories_fts MATCH ?
ORDER BY bm25(memories_fts)
LIMIT 20;
```

### Pattern 5: Doctor Warn Check for sqlite3

```rust
// In run_doctor(), after existing checks vec construction:
let sqlite3_check = check_binary("sqlite3", None);
checks.push(DoctorCheck {
    name: sqlite3_check.name,
    status: if sqlite3_check.status == CheckStatus::Pass {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn  // non-fatal — unlike bwrap/socat
    },
    detail: sqlite3_check.detail,
    fix: None,  // available on all standard macOS/Linux installs
});
```

`check_binary` returns `Fail` for missing binaries. This pattern overrides `Fail` to `Warn` since sqlite3 is informational (agents can still run without it; the Rust CLI uses bundled SQLite via rusqlite). A simpler approach: write a `check_binary_warn` helper or inline a warn-only version.

### Pattern 6: cmd_up Integration (step 10)

Insert after step 9 (settings.local.json) inside the per-agent loop at line 405 of main.rs:

```rust
// 10. Initialize per-agent memory database (Phase 16, DB-01).
rightclaw::memory::open_db(&agent.path)
    .map_err(|e| miette::miette!("failed to open memory database for '{}': {e:#}", agent.name))?;
tracing::debug!(agent = %agent.name, "memory.db initialized");
```

### Anti-Patterns to Avoid

- **Deleting memory.db on `rightclaw up`:** settings.json is regenerated (safe); memory.db must survive restarts (it's accumulated knowledge). Never delete.
- **Async MemoryStore in Phase 16:** No async needed. `open_db` is sync, cmd_up scaffold loop is effectively sync before the process-compose spawn. Adding `spawn_blocking` adds complexity for zero benefit.
- **Putting memory.db inside `.claude/`:** `.claude/settings.json` is regenerated on every up. Future scaffold changes could interfere. Flat in agent root is the established pattern (same as `crons/`).
- **`user_version` manual management:** Let rusqlite_migration own it. Never write `PRAGMA user_version` manually — it breaks the migration system.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Migration tracking | Custom table (`schema_version`) | `rusqlite_migration` user_version | user_version is atomic at fixed offset in SQLite header; table approach needs DDL on every open |
| FTS index | Custom search with LIKE | SQLite FTS5 built-in | LIKE on unindexed columns is O(n); FTS5 BM25 ranking is O(log n) and already in bundled SQLite |
| ABORT enforcement | Application-layer guard in Rust | SQLite TRIGGER RAISE(ABORT) | DB-level enforcement is the only way to truly protect against all code paths including future bugs |

**Key insight:** SQLite's trigger and pragma systems handle the hardest invariants (append-only audit log, FTS sync). Trust them over application-layer guards.

## Common Pitfalls

### Pitfall 1: FTS5 Not Available in Non-Bundled SQLite
**What goes wrong:** `CREATE VIRTUAL TABLE ... USING fts5` returns "no such module: fts5" on some Linux distros that ship SQLite without FTS5 compiled in.
**Why it happens:** FTS5 is a compile-time option. Some distro packages omit it.
**How to avoid:** Use `features = ["bundled"]` in rusqlite — SQLite 3.51.1 bundled by rusqlite 0.39 always includes FTS5.
**Warning signs:** Test against a non-bundled SQLite; CI/CD on Ubuntu 22.04 ships SQLite without FTS5.

### Pitfall 2: ABORT vs FAIL in Triggers
**What goes wrong:** Using `RAISE(FAIL, ...)` instead of `RAISE(ABORT, ...)` — FAIL rolls back only the statement but continues the transaction; ABORT rolls back both statement and transaction.
**Why it happens:** SQLite has IGNORE, FAIL, ABORT, ROLLBACK — confusing semantics.
**How to avoid:** Use `RAISE(ABORT, ...)` — it's the correct level for data integrity enforcement in single-statement operations.
**Warning signs:** Test that a rolled-back UPDATE on memory_events doesn't leave partial state.

### Pitfall 3: `to_latest` Called with Immutable Connection
**What goes wrong:** `rusqlite_migration::Migrations::to_latest` requires `&mut Connection`. If you pass `&Connection` the compile fails.
**Why it happens:** Migrations need to run DDL and update `user_version` — mutating operations.
**How to avoid:** Always pass `&mut conn`. In the `open_db` function: `let mut conn = Connection::open(...)?; MIGRATIONS.to_latest(&mut conn)?;`

### Pitfall 4: WAL Mode Pragma Must Be Set Before Migrations
**What goes wrong:** Running migrations before setting WAL mode creates the DB in journal mode (DELETE), requiring a checkpoint to switch. Harmless but adds overhead.
**Why it happens:** WAL mode is a per-file setting that persists. First open determines mode.
**How to avoid:** Set `journal_mode=WAL` and `busy_timeout` before calling `to_latest`. Order: open → pragmas → migrations → return.

### Pitfall 5: FTS Content Table Sync Trigger for Updates
**What goes wrong:** Omitting the UPDATE trigger on memories — FTS index goes stale when memories.content is edited (soft-delete sets deleted_at but content unchanged; still fine). Phase 17 `/forget` sets `deleted_at` not editing content, so this is low-risk, but include the trigger anyway.
**Why it happens:** Developers write INSERT and DELETE triggers but forget UPDATE.
**How to avoid:** Include all three sync triggers (AFTER INSERT, AFTER DELETE, AFTER UPDATE) in V1 schema.

### Pitfall 6: AgentDef Struct Literal Sites for memory_path Removal
**What goes wrong:** Removing `memory_path` from `AgentDef` struct fails to compile at all 4 struct literal sites:
- `crates/rightclaw/src/init.rs:80` — `memory_path: None`
- `crates/rightclaw/src/init.rs:155` — `memory_path: None`
- `crates/rightclaw-cli/src/main.rs:731` — `memory_path: None`
- `crates/rightclaw/src/agent/discovery.rs:123` — `memory_path: optional_file(&path, "MEMORY.md")`

**How to avoid:** Remove field from struct definition in types.rs first, then let the compiler locate all struct literal errors.

## Code Examples

### Complete V1 Schema SQL

```sql
-- V1 schema for per-agent memory database
-- Source: Phase 16 decision D-02

-- Main memories table
CREATE TABLE IF NOT EXISTS memories (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    content     TEXT    NOT NULL,
    tags        TEXT,
    stored_by   TEXT,
    source_tool TEXT,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    deleted_at  TEXT,
    expires_at  TEXT,
    importance  REAL    NOT NULL DEFAULT 0.5
);

-- Append-only audit log
CREATE TABLE IF NOT EXISTS memory_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id   INTEGER,
    event_type  TEXT    NOT NULL,
    actor       TEXT,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- Block UPDATE and DELETE on memory_events (append-only invariant)
CREATE TRIGGER IF NOT EXISTS memory_events_no_update
BEFORE UPDATE ON memory_events
BEGIN
    SELECT RAISE(ABORT, 'memory_events is append-only: UPDATE not permitted');
END;

CREATE TRIGGER IF NOT EXISTS memory_events_no_delete
BEFORE DELETE ON memory_events
BEGIN
    SELECT RAISE(ABORT, 'memory_events is append-only: DELETE not permitted');
END;

-- FTS5 virtual table (external content = memories.content)
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content,
    content='memories',
    content_rowid='id'
);

-- Sync triggers: keep memories_fts current
CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content)
    VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
    INSERT INTO memories_fts(rowid, content)
    VALUES (new.id, new.content);
END;
```

Note: `IF NOT EXISTS` on triggers requires SQLite 3.35.0+. Bundled SQLite 3.51.1 supports it. Protects against re-running migrations that include trigger DDL.

### MemoryError (thiserror)

```rust
// crates/rightclaw/src/memory/error.rs
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),
}
```

### open_db (complete)

```rust
// crates/rightclaw/src/memory/mod.rs
pub fn open_db(agent_path: &std::path::Path) -> Result<(), MemoryError> {
    let db_path = agent_path.join("memory.db");
    let mut conn = rusqlite::Connection::open(&db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    migrations::MIGRATIONS.to_latest(&mut conn)?;
    Ok(())
}
```

### migrations.rs (static Migrations instance)

```rust
// crates/rightclaw/src/memory/migrations.rs
use rusqlite_migration::{Migrations, M};

const V1_SCHEMA: &str = include_str!("sql/v1_schema.sql");

pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| Migrations::new(vec![M::up(V1_SCHEMA)]));
```

Or if LazyLock feels heavy for one migration:
```rust
pub fn get() -> Migrations<'static> {
    Migrations::new(vec![M::up(V1_SCHEMA)])
}
// Usage: migrations::get().to_latest(&mut conn)?;
```

Either works. `LazyLock` is in std since Rust 1.80 (edition 2024 uses it freely).

### discovery_tests.rs change

The test at line 196-197 that asserts `a.memory_path.is_some()` when MEMORY.md exists must be removed. The whole `discover_detects_optional_files` test also writes MEMORY.md at line 186 — remove that line and the `a.memory_path.is_some()` assertion.

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| serde_yaml | serde-saphyr | March 2024 | serde_yaml archived; project already uses serde-saphyr |
| tokio-rusqlite for async bridge | Sync rusqlite in sync scaffold loop | Phase 16 CONTEXT decision | No async bridge needed; all scaffold steps are sync before process-compose spawn |

**Deprecated/outdated:**
- STACK.md (`.planning/research/STACK.md`) recommends tokio-rusqlite — superseded by CONTEXT.md D-10 which confirms sync-only approach.
- STACK.md recommends rusqlite 0.38 — superseded by D-11 which locks 0.39. Version gap: `rusqlite_migration` 2.5 requires rusqlite 0.39 (compatibility matrix from STACK.md confirms 0.38 + 2.4.x; 0.39 needs 2.5).

## Open Questions

1. **SQL embedded inline vs. `include_str!` from file**
   - What we know: Both are valid rusqlite_migration patterns. Inline is simpler for a single migration. `include_str!` from `src/memory/sql/v1_schema.sql` is more readable for long SQL.
   - What's unclear: Project has no established convention (this is the first SQL in the codebase).
   - Recommendation: Use `include_str!("sql/v1_schema.sql")` — the schema is ~40 lines of SQL, cleaner as its own file. Easier to audit.

2. **`IF NOT EXISTS` on triggers in migration SQL**
   - What we know: Requires SQLite 3.35.0+. Bundled SQLite 3.51.1 supports it.
   - What's unclear: Whether to use it for defensive correctness or omit since to_latest is already idempotent.
   - Recommendation: Include `IF NOT EXISTS` on all DDL — more defensive, no downside with bundled SQLite.

3. **Doctor check: `check_binary` returns Fail for missing binary, but DOCTOR-01 requires Warn**
   - What we know: `check_binary` in doctor.rs returns `CheckStatus::Fail` when binary not found. DOCTOR-01 says sqlite3 check is non-fatal (Warn).
   - What's unclear: Whether to add a `check_binary_warn` helper or inline the override.
   - Recommendation: Inline the override in `run_doctor` — single use, no abstraction needed. Or add a `check_binary_with_severity(name, fix, severity)` helper if cleaner.

## Sources

### Primary (HIGH confidence)
- Direct codebase inspection: `/home/wb/dev/rightclaw/crates/` — exact line numbers for all modification sites confirmed
- rusqlite_migration 2.5.0 version confirmed via crates.io API 2026-03-26
- SQLite FTS5 external content table pattern — from SQLite official documentation (content= and content_rowid= parameters)
- SQLite RAISE(ABORT) trigger semantics — SQLite documentation (triggers.html)

### Secondary (MEDIUM confidence)
- ARCHITECTURE.md (`.planning/research/ARCHITECTURE.md`) — sync MemoryStore pattern, module layout, data flow diagrams
- STACK.md (`.planning/research/STACK.md`) — rusqlite version rationale, sqlx rejection; note: version superseded by CONTEXT.md D-11

### Tertiary (LOW confidence)
- None — all claims verified against code or official docs.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — versions confirmed via crates.io API, not training data
- Architecture: HIGH — based on direct codebase inspection; all struct literal sites located
- Pitfalls: HIGH — FTS5 availability, ABORT vs FAIL semantics verified against SQLite docs

**Research date:** 2026-03-26
**Valid until:** 2026-04-26 (stable library versions; rusqlite releases infrequently)
