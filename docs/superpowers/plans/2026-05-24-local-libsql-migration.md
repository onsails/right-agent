# Local libSQL Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move per-agent `data.db` from raw `rusqlite` ownership to a local-only `libsql` driver boundary owned by `right-db`.

**Architecture:** `right-db` becomes the only crate that depends on or exposes database driver details. Other crates move from `rusqlite::Connection`, `rusqlite::Row`, `rusqlite::Transaction`, and `rusqlite::Error` to project-owned `right_db` connection, row, transaction, and error contracts. This milestone stays local-only and preserves the existing `<agent>/data.db` file, schema, FTS5 behavior, and migration model.

**Tech Stack:** Rust 2024, `libsql`, `tokio`, local SQLite-compatible `data.db`, `right-db` migrations, GitHub issues #74-#78 for future Turso/cloud work.

**Spec:** `docs/superpowers/specs/2026-05-24-local-libsql-migration-design.md`.

---

## Already Created Future GitHub Issues

The approved design required future Turso/cloud work to be left in GitHub issues. These were created before this plan:

- #74 `Design Turso cloud configuration model`
- #75 `Add embedded replica and sync mode`
- #76 `Design cloud backup and restore UX`
- #77 `Add sync health checks and doctor output`
- #78 `Plan migration from local data.db to cloud-backed storage`

These issues are future work and must not be pulled into the local `libsql` migration.

## File Structure

- Modify `Cargo.toml`
- Modify `Cargo.lock`
- Modify `crates/right-db/Cargo.toml`
- Modify `crates/right-db/src/lib.rs`
- Modify `crates/right-db/src/error.rs`
- Create `crates/right-db/src/connection.rs`
- Create `crates/right-db/src/row.rs`
- Create `crates/right-db/src/params.rs`
- Create `crates/right-db/src/transaction.rs`
- Create `crates/right-db/src/test_support.rs`
- Modify `crates/right-db/src/migrations.rs`
- Modify `crates/right-db/src/conversation.rs`
- Modify `crates/right-db/tests/smoke.rs`
- Modify crate manifests that currently depend directly on `rusqlite`: `crates/right-memory/Cargo.toml`, `crates/right-mcp/Cargo.toml`, `crates/right-agent/Cargo.toml`, `crates/right/Cargo.toml`, `crates/bot/Cargo.toml`, `crates/right-dashboard/Cargo.toml`, `crates/right-lifecycle/Cargo.toml`
- Modify DB helper modules currently taking raw `rusqlite::Connection`, starting with:
  - `crates/right-memory/src/retain_queue.rs`
  - `crates/right-memory/src/error.rs`
  - `crates/right-mcp/src/credentials.rs`
  - `crates/right-mcp/src/refresh.rs`
  - `crates/right-mcp/src/reconnect.rs`
  - `crates/right-agent/src/async_runs.rs`
  - `crates/right-agent/src/cron_spec.rs`
  - `crates/right-agent/src/learned_skills.rs`
  - `crates/right-agent/src/usage/insert.rs`
  - `crates/right-agent/src/usage/aggregate.rs`
  - `crates/right-agent/src/usage/turn_baseline.rs`
  - `crates/right-agent/src/usage/error.rs`
  - `crates/right-lifecycle/src/lib.rs`
  - `crates/right-dashboard/src/read_model.rs`
  - `crates/right-dashboard/src/read_model/activity.rs`
  - `crates/right-dashboard/src/read_model/dashboard_overview.rs`
  - `crates/right-dashboard/src/read_model/learning.rs`
  - `crates/right-dashboard/src/read_model/learning_episodes.rs`
  - `crates/right-dashboard/src/read_model/usage.rs`
  - `crates/bot/src/telegram/session.rs`
  - `crates/bot/src/telegram/archive.rs`
  - `crates/bot/src/telegram/alerts.rs`
  - `crates/bot/src/telegram/dashboard.rs`
  - `crates/bot/src/telegram/dashboard/skills.rs`
  - `crates/bot/src/telegram/worker.rs`
  - `crates/bot/src/async_delivery.rs`
  - `crates/bot/src/background.rs`
  - `crates/bot/src/cron.rs`
  - `crates/bot/src/execution_events.rs`
  - `crates/bot/src/learning_curator.rs`
  - `crates/bot/src/learning_episode.rs`
  - `crates/bot/src/learning_prefilter.rs`
  - `crates/right/src/right_backend.rs`
  - `crates/right/src/memory_server.rs`
  - `crates/right/src/restore.rs`
  - `crates/right/src/main.rs`
- Modify tests that directly open `rusqlite::Connection` after each owning module moves to `right_db::test_support`.
- Modify `ARCHITECTURE.md`
- Modify `docs/architecture/memory.md`
- Modify `docs/architecture/modules.md`

## Baseline

- [ ] **Step 0.1: Confirm Rust skill availability**

Run:

```bash
devenv shell -- rg -n "rust-dev" AGENTS.md AGENTS.rust.md
```

Expected: project instructions mention `rust-dev:rust-dev`. If the skill remains unavailable in the session skill list, record that in implementation notes and proceed with direct Rust edits.

- [ ] **Step 0.2: Re-read approved design and architecture docs**

Run:

```bash
devenv shell -- sed -n '1,240p' docs/superpowers/specs/2026-05-24-local-libsql-migration-design.md
devenv shell -- sed -n '460,490p' ARCHITECTURE.md
devenv shell -- sed -n '1,220p' docs/architecture/memory.md
devenv shell -- sed -n '1,220p' docs/architecture/modules.md
```

Expected: design is local-only; architecture docs describe current `rusqlite` SQLite rules and memory queue details.

- [ ] **Step 0.3: Run targeted baseline**

Run:

```bash
devenv shell -- cargo test -p right-db
devenv shell -- cargo test -p right-memory
devenv shell -- cargo test -p right-mcp
devenv shell -- cargo test -p right-agent
devenv shell -- cargo test -p right-dashboard
```

Expected: PASS or record pre-existing failures before edits. Do not run the full workspace suite at this stage.

- [ ] **Step 0.4: Capture raw `rusqlite` surface**

Run:

```bash
devenv shell -- rg -n "rusqlite::|use rusqlite|rusqlite =|rusqlite_migration" Cargo.toml crates > /tmp/right-agent-rusqlite-before.txt
devenv shell -- sed -n '1,220p' /tmp/right-agent-rusqlite-before.txt
```

Expected: output lists all direct `rusqlite` usage. Keep `/tmp/right-agent-rusqlite-before.txt` for comparison during the migration.

## Task 1: Add `libsql` Dependency And Driver Contract Tests

**Files:**
- Modify `Cargo.toml`
- Modify `Cargo.lock`
- Modify `crates/right-db/Cargo.toml`
- Modify `crates/right-db/tests/smoke.rs`

- [ ] **Step 1.1: Confirm the latest `libsql` crate version**

Run:

```bash
devenv shell -- cargo search libsql --limit 1
```

Expected: output includes `libsql = "0.9.30"` or a newer version. This plan was written after checking docs.rs on 2026-05-24, where `0.9.30` was current. If `cargo search` returns a newer version, use the newer version and record it in the implementation notes.

- [ ] **Step 1.2: Add `libsql` to the workspace**

Run with `0.9.30`, unless Step 1.1 returned a newer version:

```bash
devenv shell -- cargo add libsql@0.9.30 --workspace
```

Then edit `crates/right-db/Cargo.toml` so `right-db` depends on `libsql` through the workspace:

```toml
[dependencies]
libsql = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
```

Expected: `Cargo.toml`, `Cargo.lock`, and `crates/right-db/Cargo.toml` mention `libsql`; `right-db` still temporarily has `rusqlite` until later tasks remove it.

- [ ] **Step 1.3: Write failing local-open contract tests**

Add these tests to `crates/right-db/tests/smoke.rs`:

```rust
#[test]
fn libsql_open_connection_creates_file_and_preserves_local_path() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).unwrap();

    assert!(
        dir.path().join("data.db").exists(),
        "local libsql open should create data.db",
    );

    conn.execute_batch("CREATE TABLE local_probe (id INTEGER PRIMARY KEY)").unwrap();
    assert_eq!(query_table_count(&conn, "local_probe"), 1);
}

#[test]
fn libsql_open_connection_readonly_requires_existing_db() {
    let dir = tempdir().unwrap();

    let err = open_connection_readonly(dir.path()).expect_err("missing db should not open");

    assert!(
        !dir.path().join("data.db").exists(),
        "readonly open must not create data.db",
    );
    assert!(err.is_open_error(), "expected readonly open failure, got {err:?}");
}
```

Run:

```bash
devenv shell -- cargo test -p right-db libsql_open_connection
```

Expected: FAIL because `right_db::Connection` still returns `rusqlite::Connection`, lacks `execute_batch`, and `DbError::is_open_error` does not exist.

## Task 2: Introduce `right-db` Driver Abstractions

**Files:**
- Create `crates/right-db/src/connection.rs`
- Create `crates/right-db/src/row.rs`
- Create `crates/right-db/src/params.rs`
- Create `crates/right-db/src/transaction.rs`
- Modify `crates/right-db/src/lib.rs`
- Modify `crates/right-db/src/error.rs`
- Modify `crates/right-db/tests/smoke.rs`

- [ ] **Step 2.1: Add `DbError` variants before implementation**

Modify `crates/right-db/src/error.rs` to expose project errors:

```rust
use std::path::PathBuf;

/// Errors from per-agent database operations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Database(#[from] libsql::Error),

    #[error("database row not found")]
    NotFound,

    #[error("invalid database parameter: {0}")]
    InvalidParameter(String),

    #[error("database constraint violation: {0}")]
    Constraint(String),

    #[error("open database {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: libsql::Error,
    },

    #[error("migration {version} on {path}: {source}")]
    Migration {
        path: PathBuf,
        version: u32,
        #[source]
        source: libsql::Error,
    },
}

impl DbError {
    pub fn is_open_error(&self) -> bool {
        matches!(self, Self::Open { .. })
    }

    pub fn not_found() -> Self {
        Self::NotFound
    }
}
```

Run:

```bash
devenv shell -- cargo test -p right-db libsql_open_connection
```

Expected: still FAIL because `Connection` is not implemented.

- [ ] **Step 2.2: Add minimal connection wrapper**

Create `crates/right-db/src/connection.rs`:

```rust
use crate::DbError;
use libsql::{Builder, Database, OpenFlags};
use std::path::{Path, PathBuf};

pub struct Connection {
    db_path: PathBuf,
    database: Database,
    inner: libsql::Connection,
    runtime: tokio::runtime::Runtime,
}

impl Connection {
    pub(crate) fn open_local(db_path: PathBuf, create: bool) -> Result<Self, DbError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("right-db local runtime must build");
        let flags = if create {
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let database = runtime
            .block_on(async {
                Builder::new_local(&db_path)
                    .flags(flags)
                    .build()
                    .await
            })
            .map_err(|source| DbError::Open {
                path: db_path.clone(),
                source,
            })?;
        let inner = database.connect().map_err(|source| DbError::Open {
            path: db_path.clone(),
            source,
        })?;
        Ok(Self {
            db_path,
            database,
            inner,
            runtime,
        })
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    pub fn execute_batch(&self, sql: &str) -> Result<(), DbError> {
        self.runtime.block_on(async {
            self.inner.execute_batch(sql).await?;
            Ok(())
        })
    }

    pub fn execute(&self, sql: &str, params: impl crate::params::IntoParams) -> Result<usize, DbError> {
        let params = params.into_params();
        self.runtime
            .block_on(async { self.inner.execute(sql, params).await })
            .map(|rows| rows as usize)
            .map_err(DbError::from)
    }
}
```

Modify `crates/right-db/src/lib.rs` to export the type and use it:

```rust
pub mod connection;
pub mod error;
pub mod migrations;
pub mod params;
pub mod row;
pub mod transaction;

pub use connection::Connection;
pub use error::DbError;
pub use migrations::MIGRATIONS;

use std::path::Path;

pub fn open_connection(agent_path: &Path, migrate: bool) -> Result<Connection, DbError> {
    let db_path = agent_path.join("data.db");
    let conn = Connection::open_local(db_path, true)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;")?;
    if migrate {
        migrations::MIGRATIONS.to_latest(&conn)?;
    }
    Ok(conn)
}

pub fn open_db(agent_path: &Path, migrate: bool) -> Result<(), DbError> {
    open_connection(agent_path, migrate).map(drop)
}

pub fn open_connection_readonly(agent_dir: impl AsRef<Path>) -> Result<Connection, DbError> {
    let db_path = agent_dir.as_ref().join("data.db");
    let conn = Connection::open_local(db_path, false)?;
    conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
    Ok(conn)
}
```

Run:

```bash
devenv shell -- cargo test -p right-db libsql_open_connection
```

Expected: FAIL on missing params and migration compatibility.

- [ ] **Step 2.3: Add params and row adapters**

Create `crates/right-db/src/params.rs`:

```rust
pub trait IntoParams {
    fn into_params(self) -> libsql::Params;
}

impl IntoParams for () {
    fn into_params(self) -> libsql::Params {
        libsql::params![]
    }
}

impl<const N: usize> IntoParams for [&str; N] {
    fn into_params(self) -> libsql::Params {
        libsql::Params::Positional(
            self.into_iter()
                .map(|value| libsql::Value::Text(value.to_owned()))
                .collect(),
        )
    }
}

impl<const N: usize> IntoParams for [i64; N] {
    fn into_params(self) -> libsql::Params {
        libsql::Params::Positional(
            self.into_iter().map(libsql::Value::Integer).collect(),
        )
    }
}
```

Create `crates/right-db/src/row.rs`:

```rust
use crate::DbError;

pub struct Row {
    inner: libsql::Row,
}

impl Row {
    pub(crate) fn new(inner: libsql::Row) -> Self {
        Self { inner }
    }

    pub fn get<T: libsql::FromValue>(&self, index: usize) -> Result<T, DbError> {
        self.inner.get(index as i32).map_err(DbError::from)
    }
}
```

Run:

```bash
devenv shell -- cargo test -p right-db libsql_open_connection
```

Expected: compile errors, if any, are limited to `libsql` parameter conversion details in `params.rs`. Keep the public `right_db` method names stable while correcting those conversion implementations against the installed crate source.

- [ ] **Step 2.4: Commit the wrapper scaffold**

Run:

```bash
devenv shell -- cargo test -p right-db libsql_open_connection
devenv shell -- git add Cargo.toml Cargo.lock crates/right-db
devenv shell -- git commit -m "refactor(db): add local libsql connection wrapper"
```

Expected: targeted tests pass and commit succeeds.

## Task 3: Replace Migration Runner

**Files:**
- Modify `crates/right-db/src/migrations.rs`
- Modify `crates/right-db/tests/smoke.rs`

- [ ] **Step 3.1: Write migration compatibility tests**

Add these tests to `crates/right-db/tests/smoke.rs`:

```rust
#[test]
fn libsql_migrations_set_latest_user_version() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();

    assert_eq!(
        query_user_version(&conn),
        right_db::migrations::LATEST_SCHEMA_VERSION as i64,
    );
}

#[test]
fn libsql_migrations_are_idempotent_on_existing_data_db() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();
    open_db(dir.path(), true).unwrap();

    let conn = open_connection(dir.path(), false).unwrap();
    assert_eq!(
        query_user_version(&conn),
        right_db::migrations::LATEST_SCHEMA_VERSION as i64,
    );
}
```

Update helper signatures in the same file:

```rust
fn query_user_version(conn: &right_db::Connection) -> i64 {
    conn.query_one("PRAGMA user_version", (), |row| row.get(0))
        .unwrap()
}

fn query_table_count(conn: &right_db::Connection, table_name: &str) -> i64 {
    conn.query_one(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
        [table_name],
        |row| row.get(0),
    )
    .unwrap()
}
```

Run:

```bash
devenv shell -- cargo test -p right-db libsql_migrations
```

Expected: FAIL until `MIGRATIONS` no longer depends on `rusqlite_migration`.

- [ ] **Step 3.2: Add project migration types**

In `crates/right-db/src/migrations.rs`, replace the exported `rusqlite_migration::Migrations` with project-owned types:

```rust
pub const LATEST_SCHEMA_VERSION: u32 = 32;

pub struct Migration {
    pub version: u32,
    pub sql: &'static str,
    pub hook: Option<fn(&crate::Connection) -> Result<(), crate::DbError>>,
}

pub struct Migrations {
    migrations: &'static [Migration],
}

pub static MIGRATIONS: Migrations = Migrations {
    migrations: &[
        Migration { version: 1, sql: V1_SCHEMA, hook: None },
        // Preserve every existing version in order.
        Migration { version: 32, sql: V32_SCHEMA, hook: None },
    ],
};

impl Migrations {
    pub fn to_latest(&self, conn: &crate::Connection) -> Result<(), crate::DbError> {
        let current: i64 = conn.query_one("PRAGMA user_version", (), |row| row.get(0))?;
        for migration in self.migrations {
            if migration.version as i64 <= current {
                continue;
            }
            conn.with_immediate_transaction(|tx| {
                tx.execute_batch(migration.sql)?;
                if let Some(hook) = migration.hook {
                    hook(tx.connection())?;
                }
                tx.execute_batch(&format!("PRAGMA user_version = {}", migration.version))?;
                Ok(())
            })
            .map_err(|source| crate::DbError::Migration {
                path: conn.path().to_path_buf(),
                version: migration.version,
                source: source.into_libsql_error(),
            })?;
        }
        Ok(())
    }
}
```

Expected adjustment: if `DbError` cannot expose `into_libsql_error`, store `Box<DbError>` in the migration variant instead of `libsql::Error`. Keep the public error message carrying path and version.

- [ ] **Step 3.3: Port conditional migration hooks**

For each existing hook in `crates/right-db/src/migrations.rs`, rewrite it from `rusqlite::Transaction` to `right_db::Connection` operations. Use this helper style:

```rust
fn column_exists(conn: &crate::Connection, table: &str, column: &str) -> Result<bool, crate::DbError> {
    let sql = "SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?";
    let count: i64 = conn.query_one(sql, [table, column], |row| row.get(0))?;
    Ok(count > 0)
}
```

Run:

```bash
devenv shell -- cargo test -p right-db libsql_migrations
devenv shell -- cargo test -p right-db migrations
```

Expected: PASS.

- [ ] **Step 3.4: Commit migration runner**

Run:

```bash
devenv shell -- git add crates/right-db
devenv shell -- git commit -m "refactor(db): run migrations through right-db"
```

Expected: commit succeeds.

## Task 4: Prove Local SQLite Feature Compatibility

**Files:**
- Modify `crates/right-db/tests/smoke.rs`
- Modify `crates/right-db/src/transaction.rs`
- Modify `crates/right-db/src/connection.rs`

- [ ] **Step 4.1: Add FTS5 and trigger tests**

Add tests in `crates/right-db/tests/smoke.rs`:

```rust
#[test]
fn libsql_supports_conversation_fts_triggers() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();

    conn.execute(
        "INSERT INTO conversation_messages (chat_id, thread_id, role, content)
         VALUES (1, 0, 'user', ?)",
        ["needle phrase"],
    )
    .unwrap();

    let count: i64 = conn
        .query_one(
            "SELECT COUNT(*) FROM conversation_messages_fts WHERE conversation_messages_fts MATCH ?",
            ["needle"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(count, 1);
}
```

Run:

```bash
devenv shell -- cargo test -p right-db libsql_supports_conversation_fts_triggers
```

Expected: PASS before moving callsites.

- [ ] **Step 4.2: Add `RETURNING` test**

Add:

```rust
#[test]
fn libsql_supports_returning_clause() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();

    let id: i64 = conn
        .query_one(
            "INSERT INTO conversation_messages (chat_id, thread_id, role, content)
             VALUES (1, 0, 'assistant', 'returning probe')
             RETURNING id",
            (),
            |row| row.get(0),
        )
        .unwrap();

    assert!(id > 0);
}
```

Run:

```bash
devenv shell -- cargo test -p right-db libsql_supports_returning_clause
```

Expected: PASS.

- [ ] **Step 4.3: Add transaction rollback test**

Create `crates/right-db/src/transaction.rs` with the public API:

```rust
pub struct Transaction<'conn> {
    conn: &'conn crate::Connection,
}

impl<'conn> Transaction<'conn> {
    pub fn connection(&self) -> &'conn crate::Connection {
        self.conn
    }

    pub fn execute_batch(&self, sql: &str) -> Result<(), crate::DbError> {
        self.conn.execute_batch(sql)
    }
}
```

Add this test:

```rust
#[test]
fn libsql_transaction_rolls_back_on_error() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    conn.execute_batch("CREATE TABLE rollback_probe (id INTEGER PRIMARY KEY, value TEXT UNIQUE)")
        .unwrap();

    let result = conn.with_immediate_transaction(|tx| {
        tx.execute_batch("INSERT INTO rollback_probe (value) VALUES ('same')")?;
        tx.execute_batch("INSERT INTO rollback_probe (value) VALUES ('same')")?;
        Ok(())
    });

    assert!(result.is_err());
    let count: i64 = conn
        .query_one("SELECT COUNT(*) FROM rollback_probe", (), |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
```

Run:

```bash
devenv shell -- cargo test -p right-db libsql_transaction_rolls_back_on_error
```

Expected: PASS after `with_immediate_transaction` uses `BEGIN IMMEDIATE`, `COMMIT`, and `ROLLBACK` through `libsql`.

- [ ] **Step 4.4: Commit compatibility tests**

Run:

```bash
devenv shell -- cargo test -p right-db
devenv shell -- git add crates/right-db
devenv shell -- git commit -m "test(db): prove local libsql sqlite features"
```

Expected: `right-db` tests pass and commit succeeds.

## Task 5: Migrate `right-db` Conversation API

**Files:**
- Modify `crates/right-db/src/conversation.rs`
- Modify `crates/right-db/tests/smoke.rs`

- [ ] **Step 5.1: Rewrite `conversation.rs` imports and signatures**

Change the top of `crates/right-db/src/conversation.rs` from:

```rust
use rusqlite::{Connection, Result, named_params};
```

to:

```rust
use crate::{Connection, DbError};

type Result<T> = std::result::Result<T, DbError>;
```

Change public function signatures so they accept `&crate::Connection` and return `DbError`:

```rust
pub fn archive_message(conn: &Connection, message: ConversationMessage<'_>) -> Result<i64>
pub fn mark_routed(...) -> Result<usize>
pub fn search_thread(...) -> Result<Vec<ConversationSearchResult>>
pub fn search_chat(...) -> Result<Vec<ConversationSearchResult>>
```

Run:

```bash
devenv shell -- cargo test -p right-db conversation
```

Expected: FAIL on named params and row APIs.

- [ ] **Step 5.2: Replace named params with positional params**

Rewrite the `archive_message` assistant/no-message insert in `crates/right-db/src/conversation.rs` to use positional parameters:

```rust
return conn.query_one(
    "INSERT INTO conversation_messages (
        platform, chat_id, thread_id, message_id, sender_user_id, sender_name,
        addressed_to_bot, routed_to_agent, root_session_id, turn_id, role, content
     ) VALUES (
        ?, ?, ?, NULL, ?, ?,
        ?, ?, ?, ?, ?, ?
     )
     RETURNING id",
        crate::params![
        message.platform,
        message.chat_id,
        message.thread_id,
        message.sender_user_id,
        message.sender_name,
        addressed_to_bot,
        routed_to_agent,
        message.root_session_id,
        turn_id,
        role,
        content.as_str(),
    ],
    |row| row.get(0),
);
```

Add a `right_db::params!` macro if the installed `libsql` parameter API needs heterogeneous values. Keep the macro inside `crates/right-db/src/params.rs` and export it from `crates/right-db/src/lib.rs`.

Run:

```bash
devenv shell -- cargo test -p right-db conversation
```

Expected: PASS for conversation module tests.

- [ ] **Step 5.3: Commit conversation migration**

Run:

```bash
devenv shell -- git add crates/right-db
devenv shell -- git commit -m "refactor(db): move conversation queries to libsql wrapper"
```

Expected: commit succeeds.

## Task 6: Migrate Shared Storage Crates Off Raw Driver Types

**Files:**
- Modify `crates/right-memory/src/retain_queue.rs`
- Modify `crates/right-memory/src/error.rs`
- Modify `crates/right-mcp/src/credentials.rs`
- Modify `crates/right-mcp/src/refresh.rs`
- Modify `crates/right-mcp/src/reconnect.rs`
- Modify `crates/right-lifecycle/src/lib.rs`
- Modify corresponding tests

- [ ] **Step 6.1: Move `right-memory` queue to `right_db::Connection`**

In `crates/right-memory/src/retain_queue.rs`, replace:

```rust
use rusqlite::{Connection, params};
```

with:

```rust
use right_db::{Connection, DbError};
```

Change public function errors from `rusqlite::Error` to `MemoryError` wrapping `right_db::DbError`. Convert helper queries to `conn.execute`, `conn.query_one`, and `conn.query_all`.

Run:

```bash
devenv shell -- cargo test -p right-memory retain_queue
```

Expected: PASS.

- [ ] **Step 6.2: Move MCP credentials storage to `right_db::Connection`**

In `crates/right-mcp/src/credentials.rs`, replace every public `&rusqlite::Connection` parameter with `&right_db::Connection`. Replace `rusqlite::Statement` helper functions with typed helper methods that return `Vec<McpServerEntry>`.

Run:

```bash
devenv shell -- cargo test -p right-mcp credentials
devenv shell -- cargo test -p right-mcp credentials_auth_token
```

Expected: PASS.

- [ ] **Step 6.3: Move reconnect and refresh tests to `right_db::test_support`**

Create `crates/right-db/src/test_support.rs`:

```rust
use tempfile::TempDir;

pub fn migrated_connection() -> (TempDir, crate::Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::open_connection(dir.path(), true).unwrap();
    (dir, conn)
}
```

Expose it behind `#[cfg(any(test, feature = "test-support"))]` if needed. Update test fixtures in `crates/right-mcp/src/refresh.rs` and `crates/right-mcp/src/reconnect.rs` to use it instead of `rusqlite::Connection::open_in_memory`.

Run:

```bash
devenv shell -- cargo test -p right-mcp
```

Expected: PASS.

- [ ] **Step 6.4: Move `right-lifecycle` to `right_db::Connection`**

In `crates/right-lifecycle/src/lib.rs`, replace direct `rusqlite` imports with `right_db::{Connection, DbError}`. Keep lifecycle-specific parsing errors in `LifecycleError`; wrap database errors as `LifecycleError::Database(DbError)`.

Run:

```bash
devenv shell -- cargo test -p right-lifecycle
```

Expected: PASS.

- [ ] **Step 6.5: Commit shared storage crate migration**

Run:

```bash
devenv shell -- git add crates/right-memory crates/right-mcp crates/right-lifecycle crates/right-db
devenv shell -- git commit -m "refactor(db): migrate shared storage crates to right-db"
```

Expected: commit succeeds.

## Task 7: Migrate Agent, Bot, Dashboard, And CLI Callers

**Files:**
- Modify files listed in the File Structure section under `right-agent`, `bot`, `right-dashboard`, and `right`
- Modify corresponding tests

- [ ] **Step 7.1: Migrate `right-agent` DB helpers**

Change `crates/right-agent/src/async_runs.rs`, `crates/right-agent/src/cron_spec.rs`, `crates/right-agent/src/learned_skills.rs`, and `crates/right-agent/src/usage/*` to accept `&right_db::Connection` and return domain errors wrapping `right_db::DbError`.

Run:

```bash
devenv shell -- cargo test -p right-agent async_runs
devenv shell -- cargo test -p right-agent cron_spec
devenv shell -- cargo test -p right-agent learned_skills
devenv shell -- cargo test -p right-agent usage
```

Expected: PASS.

- [ ] **Step 7.2: Migrate bot session, archive, and delivery storage**

Change bot modules to use `right_db::Connection` from `right_db::open_connection`. Replace comments that say `rusqlite::Connection is !Send` with comments naming the new `right_db::Connection` behavior once verified.

Run:

```bash
devenv shell -- cargo test -p right-bot telegram::session
devenv shell -- cargo test -p right-bot telegram::archive
devenv shell -- cargo test -p right-bot async_delivery
devenv shell -- cargo test -p right-bot background
devenv shell -- cargo test -p right-bot cron
```

Expected: PASS.

- [ ] **Step 7.3: Migrate dashboard read models**

Change `right-dashboard` read models from raw `rusqlite::Connection` to `right_db::Connection`. Keep `open_connection_readonly` for dashboard route handlers so read-only open remains structural.

Run:

```bash
devenv shell -- cargo test -p right-dashboard read_model
devenv shell -- cargo test -p right-bot dashboard
```

Expected: PASS.

- [ ] **Step 7.4: Migrate CLI backend and restore probes**

Change `crates/right/src/right_backend.rs`, `crates/right/src/memory_server.rs`, `crates/right/src/restore.rs`, and CLI tests to use `right_db::Connection`. For restore, replace direct `rusqlite::Connection::open(db_path)` with a read-only `right_db` helper that opens an explicit database path without creating it.

Run:

```bash
devenv shell -- cargo test -p right right_backend
devenv shell -- cargo test -p right memory_server
devenv shell -- cargo test -p right restore
devenv shell -- cargo test -p right --test cli_integration
```

Expected: PASS.

- [ ] **Step 7.5: Commit caller migration**

Run:

```bash
devenv shell -- git add crates/right-agent crates/bot crates/right-dashboard crates/right crates/right-db
devenv shell -- git commit -m "refactor(db): migrate callers to right-db connection"
```

Expected: commit succeeds.

## Task 8: Remove Direct `rusqlite` From Workspace Surface

**Files:**
- Modify root `Cargo.toml`
- Modify crate manifests that no longer need `rusqlite`
- Modify code and tests found by `rg`

- [ ] **Step 8.1: Search remaining direct usages**

Run:

```bash
devenv shell -- rg -n "rusqlite::|use rusqlite|rusqlite =|rusqlite_migration" Cargo.toml crates
```

Expected: remaining matches are either absent or isolated to an explicitly temporary `right-db` internal compatibility module. For this milestone, direct `rusqlite` should be absent.

- [ ] **Step 8.2: Remove manifest dependencies**

Remove `rusqlite` and `rusqlite_migration` from root `[workspace.dependencies]` and all crate manifests. Keep `libsql` in root dependencies and `right-db`.

Run:

```bash
devenv shell -- cargo check -p right-db
```

Expected: PASS.

- [ ] **Step 8.3: Commit dependency cleanup**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock crates
devenv shell -- git commit -m "build(db): remove rusqlite dependencies"
```

Expected: commit succeeds.

## Task 9: Update Architecture Documentation

**Files:**
- Modify `ARCHITECTURE.md`
- Modify `docs/architecture/memory.md`
- Modify `docs/architecture/modules.md`
- Modify `docs/superpowers/plans/2026-05-24-local-libsql-migration.md` if execution discovers a different safe runtime boundary

- [ ] **Step 9.1: Update `ARCHITECTURE.md` SQLite rules**

Replace the `rusqlite`-specific rules with `right-db` rules:

```markdown
## SQLite/libSQL Rules

`right-db` is the sole owner of the local database driver. Other crates must use
`right_db::Connection`, `right_db::Transaction`, and `right_db::DbError`; they
must not expose `libsql` or raw driver row types in public APIs.

Any operation that performs 2+ writes must use `Connection::with_immediate_transaction`.
Single-statement writes do not need a transaction. Migrations are the sole
exception because the `right-db` migration runner wraps each migration version.
```

Run:

```bash
devenv shell -- rg -n "rusqlite|rusqlite_migration|unchecked_transaction" ARCHITECTURE.md docs/architecture
```

Expected: no stale rule says new code should use `rusqlite` or `unchecked_transaction`.

- [ ] **Step 9.2: Update memory and module docs**

In `docs/architecture/memory.md`, update the pending-retains queue description to say it is local `data.db` storage through `right-db`, not `rusqlite`.

In `docs/architecture/modules.md`, update the `right-db` entry:

```markdown
### right-db

- `Connection`, `Transaction`, `DbError` - project-owned local `libsql` boundary for per-agent `data.db`.
- `migrations.rs` - ordered idempotent migration runner.
- `conversation.rs` - transcript archive and FTS search storage helpers.
- `test_support.rs` - migrated temp `data.db` fixtures for crate tests.
```

Run:

```bash
devenv shell -- rg -n "rusqlite|rusqlite_migration" docs/architecture ARCHITECTURE.md
```

Expected: remaining matches, if any, describe historical context only.

- [ ] **Step 9.3: Commit documentation updates**

Run:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/memory.md docs/architecture/modules.md
devenv shell -- git commit -m "docs(db): document local libsql boundary"
```

Expected: commit succeeds.

## Task 10: Final Verification

**Files:**
- No planned edits

- [ ] **Step 10.1: Run targeted DB surface search**

Run:

```bash
devenv shell -- rg -n "rusqlite::|use rusqlite|rusqlite =|rusqlite_migration" Cargo.toml crates
```

Expected: no direct `rusqlite` or `rusqlite_migration` usage remains.

- [ ] **Step 10.2: Run targeted package tests**

Run:

```bash
devenv shell -- cargo test -p right-db
devenv shell -- cargo test -p right-memory
devenv shell -- cargo test -p right-mcp
devenv shell -- cargo test -p right-agent
devenv shell -- cargo test -p right-lifecycle
devenv shell -- cargo test -p right-dashboard
devenv shell -- cargo test -p right-bot
devenv shell -- cargo test -p right
```

Expected: PASS.

- [ ] **Step 10.3: Run mandatory full workspace test**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. This is mandatory before claiming the implementation is complete.

- [ ] **Step 10.4: Run final debug build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 10.5: Review final diff**

Run:

```bash
devenv shell -- git status --short
devenv shell -- git diff --stat HEAD
devenv shell -- rg -n "libsql|right_db::Connection|right-db" ARCHITECTURE.md docs/architecture/memory.md docs/architecture/modules.md
```

Expected: worktree contains only intended changes since the last commit; docs mention the new local `libsql` boundary.
