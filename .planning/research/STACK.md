# Stack Research: v2.3 Memory System

**Domain:** Per-agent SQLite-backed persistent memory in Rust async CLI
**Researched:** 2026-03-26
**Confidence:** HIGH

## Scope

Delta-research for the v2.3 Memory System milestone. Covers ONLY the new SQLite memory store capability. Validated v2.2 stack (tokio, serde, reqwest, minijinja, etc.) is not re-evaluated.

---

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| rusqlite | 0.38 | SQLite driver | The canonical Rust SQLite binding. Thin wrapper over libsqlite3. Zero-overhead, battle-tested, 10M+ downloads. For a single-agent embedded store this is the right primitive — no ORM overhead, raw SQL, full control. SQLx adds async complexity with no SQLite benefit (SQLite is not actually concurrent). |
| tokio-rusqlite | 0.7 | Async bridge for rusqlite | Runs blocking rusqlite calls on a dedicated background thread via crossbeam channel, exposing async `.call()` / `.call_unwrap()`. Required because the rightclaw CLI is tokio-native; blocking on SQLite in an async task would starve the executor. 100% safe Rust (`#![forbid(unsafe_code)]`). |
| rusqlite_migration | 2.4 | Schema migrations | Simple, embedded, no CLI required. Uses SQLite `user_version` pragma (single integer at fixed offset) instead of a migration table — faster than any table-based approach. Migrations are SQL strings in Rust code. `to_latest()` is idempotent, safe to call on every startup. Plays well with tokio-rusqlite via `call_unwrap(|conn| MIGRATIONS.to_latest(conn))`. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| rusqlite (features = ["bundled"]) | 0.38 | Bundle SQLite 3.51.1 | Use `bundled` feature to embed SQLite into the binary. Eliminates system SQLite version variability across Linux distros and macOS. RightClaw ships as a single binary — bundled is the right call. Adds ~1 MB to binary. |

---

## Rejected Alternative: sqlx

**Do not use sqlx for this milestone.**

sqlx 0.8.6 is the current version. It provides async SQLite via `SqlitePool`. The reasons to reject it here are concrete:

1. **Conflict hazard.** sqlx and rusqlite both link `libsqlite3-sys`. Using them in the same workspace is a semver hazard requiring pinned versions on both and lockstep upgrades. The workspace already uses rusqlite (transitively through other crates or directly — confirm before adding sqlx).

2. **Compile-time query checking requires a live database or offline `.sqlx/` cache checked into git.** For a per-agent database at `~/.rightclaw/agents/<name>/memory.db`, the path is runtime-dynamic. The `query!()` macro's safety benefit evaporates — you'd use `query()` (dynamic) anyway.

3. **SQLite is not concurrently accessed.** sqlx's primary benefit is async connection pooling for PostgreSQL where many queries run truly in parallel. Per-agent SQLite databases are single-writer-per-agent; the blocking-on-background-thread model of tokio-rusqlite is sufficient and carries none of sqlx's complexity.

4. **Migration embedding.** sqlx `migrate!()` requires timestamped `.sql` files in `./migrations/` and `sqlx-cli` for dev workflow. rusqlite_migration embeds SQL as string literals directly in Rust code — fewer moving parts, no external tool dependency.

sqlx is the right choice for web servers with PostgreSQL or MySQL. It is wrong for this use case.

---

## What NOT to Add

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `sqlx` | Conflict hazard with libsqlite3-sys, compile-time checks broken for dynamic paths, connection pool overhead irrelevant for SQLite | `rusqlite` + `tokio-rusqlite` |
| `diesel` | ORM overhead, code generation, migration CLI, massive compile time — all wrong for a simple key-value/FTS memory store | Raw SQL via rusqlite |
| `sea-orm` | Same problems as diesel, async but built on sqlx | rusqlite + tokio-rusqlite |
| `sqlite` crate | Low adoption, thin wrapper with poor error handling, not actively maintained | rusqlite |
| `refinery` | Migration CLI dependency, heavier than rusqlite_migration, SQL files on disk | rusqlite_migration |
| FTS5 extension | Built into SQLite already — no extra crate needed. Enable via `PRAGMA` and use `CREATE VIRTUAL TABLE ... USING fts5(...)` | SQLite built-in FTS5 |

---

## Architecture Fit

### Per-agent database location

```
~/.rightclaw/agents/<name>/memory.db
```

One database per agent. Path constructed from agent HOME dir (already known at `rightclaw up` time). No shared state between agents — matches the isolation model.

### tokio-rusqlite async pattern

```rust
// Open
let conn = tokio_rusqlite::Connection::open(&db_path).await?;

// Migrate (runs sync migration logic in background thread)
conn.call_unwrap(|conn| MIGRATIONS.to_latest(conn)).await?;

// Query
let results = conn.call(|conn| {
    let mut stmt = conn.prepare("SELECT * FROM memory WHERE ...")?;
    // ... map rows
    Ok(results)
}).await?;
```

`call_unwrap` is safe here — the connection is opened at agent startup and held for the process lifetime. Prefer `call_unwrap` for ergonomics; use `call` only where explicit `ConnectionClosed` handling is needed.

### WAL mode

Enable WAL mode on every new database open:

```rust
conn.call_unwrap(|conn| {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}).await?;
```

WAL enables concurrent readers + one writer without blocking. `synchronous = NORMAL` is safe with WAL and improves write throughput. Set before running migrations.

### Schema design

```sql
CREATE TABLE memory (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    key     TEXT NOT NULL,
    value   TEXT NOT NULL,
    source  TEXT NOT NULL DEFAULT 'agent',   -- provenance
    created_at INTEGER NOT NULL,             -- unix timestamp
    updated_at INTEGER NOT NULL
);

CREATE INDEX memory_key_idx ON memory(key);

-- FTS5 for full-text search (built-in SQLite feature, no extension)
CREATE VIRTUAL TABLE memory_fts USING fts5(
    key, value,
    content='memory', content_rowid='id'
);
```

FTS5 is part of SQLite — no extension loading, no extra crate. Query with `SELECT * FROM memory WHERE rowid IN (SELECT rowid FROM memory_fts WHERE memory_fts MATCH ?)`.

---

## IronClaw Memory Reference

IronClaw (nearai/ironclaw) is a competing Rust agent framework. Its primary memory backend is **PostgreSQL + pgvector** (hybrid FTS + vector search via RRF). A community fork (JoasASantos/ironclaw) adds a configurable SQLite backend with `encrypt_at_rest` option.

The official nearai implementation is not relevant as a direct technical reference — it targets server deployments, not local multi-agent CLI tools. The dual PostgreSQL/libSQL backend confirms that vector search is the direction for production memory systems, but that is out of scope for v2.3. v2.3 targets simple key-value + FTS search via SQLite FTS5, which is the right MVP scope.

---

## Version Compatibility Matrix

| rusqlite | tokio-rusqlite | rusqlite_migration | Notes |
|----------|---------------|-------------------|-------|
| 0.38 | 0.7.0 | 2.4.x | All three compatible. rusqlite 0.38 breaks `rusqlite_migration` 2.3.x (statement caching change). Use 2.4.1. |
| 0.37 | 0.6.1 | 2.3.x | Prior stable set — do not use, 0.38 is current. |

SQLite version bundled with rusqlite 0.38: **SQLite 3.51.1**

---

## Cargo.toml Delta

```toml
[dependencies]
rusqlite = { version = "0.38", features = ["bundled"] }
tokio-rusqlite = "0.7"
rusqlite_migration = "2.4"
```

No dev dependencies needed — rusqlite_migration has a `.validate()` method usable in tests without a separate test crate.

---

## Sources

- [rusqlite 0.38.0 on crates.io](https://crates.io/crates/rusqlite) — version confirmed 2026-03-26 (published 2025-12-20, bundles SQLite 3.51.1)
- [tokio-rusqlite 0.7.0 on docs.rs](https://docs.rs/crate/tokio-rusqlite/latest) — version confirmed 2026-03-26 (published 2025-11-16), exposes all 42 rusqlite feature flags
- [rusqlite_migration 2.4.1 on docs.rs](https://docs.rs/crate/rusqlite_migration/latest) — version confirmed 2026-03-26, updated for rusqlite 0.38 compatibility
- [sqlx 0.8.6 on crates.io](https://crates.io/crates/sqlx) — current version confirmed 2026-03-26
- [sqlx + rusqlite semver hazard — sqlx GitHub issue #3926](https://github.com/launchbadge/sqlx/issues/3926) — confirmed libsqlite3-sys conflict
- [Rust ORMs in 2026: Diesel vs SQLx vs SeaORM vs Rusqlite](https://aarambhdevhub.medium.com/rust-orms-in-2026-diesel-vs-sqlx-vs-seaorm-vs-rusqlite-which-one-should-you-actually-use-706d0fe912f3) — MEDIUM confidence (WebSearch, Feb 2026)
- [rusqlite_migration async example](https://github.com/cljoly/rusqlite_migration/blob/master/examples/async/src/main.rs) — `call_unwrap` pattern confirmed
- [nearai/ironclaw CLAUDE.md](https://github.com/nearai/ironclaw/blob/main/CLAUDE.md) — PostgreSQL + libSQL dual backend, no SQLite for production memory

---
*Stack research for: RightClaw v2.3 Memory System*
*Researched: 2026-03-26*
