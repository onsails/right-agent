# Turso Local Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the local `right-db` driver foundation from `libsql` to `turso` with the `sync` feature available for future Turso Cloud work, without adding cloud behavior.

**Architecture:** `right-db` remains the only database-driver boundary. The implementation first adds direct compatibility probes while the current `libsql` backend still exists, then migrates `right-db` internals to `turso` and preserves the existing project-owned `Connection`, `Transaction`, `Row`, `DbError`, params, migration, and read-only APIs. No `push()`, `pull()`, credential, config, UI, CLI, bot command, or scheduler work is included.

**Tech Stack:** Rust 2024, `right-db`, `turso` crate with `sync` feature, Tokio runtime bridge, per-agent SQLite-compatible `data.db`, `devenv shell -- cargo`.

**Spec:** `docs/superpowers/specs/2026-05-25-turso-local-foundation-design.md`.

---

## File Structure

- Modify `Cargo.toml`
  - Replace workspace `libsql` dependency with `turso` after compatibility probes pass.
- Modify `crates/right-db/Cargo.toml`
  - Move `right-db` from workspace `libsql` to workspace `turso`.
- Create then delete after the gate passes: `crates/right-db/tests/turso_compat.rs`
  - Direct `turso` compatibility probes. Direct driver usage is allowed here because this is a boundary test.
- Modify `crates/right-db/src/error.rs`
  - Normalize `turso::Error` into project `DbError`.
- Modify `crates/right-db/src/params.rs`
  - Convert project params to `turso::Params` and `turso::Value`.
- Modify `crates/right-db/src/row.rs`
  - Convert `turso::Row` values into project row conversions.
- Modify `crates/right-db/src/connection.rs`
  - Wrap `turso::Database` and `turso::Connection`.
- Modify `crates/right-db/src/transaction.rs`
  - Wrap `turso::Transaction`.
- Modify `crates/right-db/tests/smoke.rs`
  - Rename `libsql_*` tests to `turso_*` or driver-neutral names and keep local contract coverage.
- Inspect and likely modify:
  - `ARCHITECTURE.md`
  - `docs/architecture/modules.md`
  - `docs/architecture/memory.md`
- No planned changes:
  - `PROMPT_SYSTEM.md`
  - cloud/Turso config, credentials, bot, CLI, dashboard, scheduler, or restore behavior.

## Baseline And Dependency Probe

- [ ] **Step 0.1: Confirm Rust skill availability**

This repository asks implementers to load `rust-dev:rust-dev` before writing Rust. If that skill is available in the implementation session, load it before Task 1. If it is not available, record this line in implementation notes before editing Rust:

```text
rust-dev:rust-dev unavailable in this Codex session; proceeding with direct Rust edits under project Rust conventions.
```

- [ ] **Step 0.2: Re-read the spec and current local DB rules**

Run:

```bash
devenv shell -- sed -n '1,320p' docs/superpowers/specs/2026-05-25-turso-local-foundation-design.md
devenv shell -- sed -n '481,506p' ARCHITECTURE.md
devenv shell -- sed -n '64,72p' docs/architecture/modules.md
devenv shell -- sed -n '54,62p' docs/architecture/memory.md
```

Expected:

- Spec says migrate local foundation to `turso`.
- Spec says no cloud sync behavior in this stage.
- Current docs still mention local `libsql`; update later only after migration lands.

- [ ] **Step 0.3: Query latest `turso` crate version**

Run:

```bash
devenv shell -- cargo search turso --limit 1
devenv shell -- cargo info turso
```

Expected at plan-writing time:

```text
turso = "0.7.0-pre.3"
features:
 +default = [mimalloc, fts]
  sync = [dep:hyper, dep:tokio, dep:hyper-tls, dep:hyper-util, dep:http-body-util, dep:bytes]
```

If the latest version changed, use the latest registry version and update any version text in this plan while implementing. Do not pin an older version without user approval.

- [ ] **Step 0.4: Run targeted baseline**

Run:

```bash
devenv shell -- cargo test -p right-db
```

Expected: PASS. If this fails, stop and fix the pre-existing `right-db` failure before adding `turso`.

- [ ] **Step 0.5: Confirm current direct `libsql` surface**

Run:

```bash
devenv shell -- rg -n "libsql|rusqlite" Cargo.toml crates/right-db crates --glob '*.rs'
```

Expected:

- `libsql` appears only in workspace deps, `right-db`, right-db smoke test names/messages, and docs/comments.
- No `rusqlite` usage in `Cargo.toml` or `crates/`.

## Task 1: Add `turso` Beside `libsql` And Run Compatibility Gate

**Files:**
- Modify `Cargo.toml`
- Modify `crates/right-db/Cargo.toml`
- Create `crates/right-db/tests/turso_compat.rs`

- [ ] **Step 1.1: Add `turso` while keeping `libsql` for the probe**

In workspace [Cargo.toml](/Users/developer/dev/rightclaw/Cargo.toml), add the latest registry version from Step 0.3 near the current `libsql` entry:

```toml
libsql = "0.9.30"
turso = { version = "0.7.0-pre.3", features = ["sync"] }
```

In [crates/right-db/Cargo.toml](/Users/developer/dev/rightclaw/crates/right-db/Cargo.toml), add `turso` while keeping `libsql`:

```toml
[dependencies]
libsql = { workspace = true }
turso = { workspace = true }
tempfile = { workspace = true, optional = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
```

- [ ] **Step 1.2: Write direct Turso compatibility probes**

Create [turso_compat.rs](/Users/developer/dev/rightclaw/crates/right-db/tests/turso_compat.rs):

```rust
use right_db::{open_db, open_database_path_readonly};
use tempfile::tempdir;

async fn open_turso(path: &std::path::Path) -> turso::Result<turso::Connection> {
    let path = path
        .to_str()
        .expect("temp database path should be valid UTF-8");
    let db = turso::Builder::new_local(path).build().await?;
    db.connect()
}

#[tokio::test(flavor = "current_thread")]
async fn turso_opens_current_right_db_file_before_backend_swap() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).expect("current right-db should create migrated data.db");
    let db_path = dir.path().join("data.db");

    let conn = open_turso(&db_path)
        .await
        .expect("turso should open data.db created by current right-db");

    let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
    let row = rows.next().await.unwrap().expect("user_version row");
    let version: i64 = row.get(0).unwrap();
    assert_eq!(version, right_db::migrations::LATEST_SCHEMA_VERSION as i64);

    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("sessions count row");
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn turso_direct_supports_required_local_sql_features() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("turso-direct.db");
    let mut conn = open_turso(&db_path).await.unwrap();

    conn.execute_batch(
        "CREATE TABLE docs (
             id INTEGER PRIMARY KEY,
             content TEXT NOT NULL
         );
         CREATE VIRTUAL TABLE docs_fts USING fts5(content);
         CREATE TRIGGER docs_ai AFTER INSERT ON docs BEGIN
             INSERT INTO docs_fts(rowid, content) VALUES (new.id, new.content);
         END;",
    )
    .await
    .unwrap();

    let id: i64 = {
        let mut rows = conn
            .query(
                "INSERT INTO docs (content) VALUES ('needle phrase') RETURNING id",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("RETURNING row");
        row.get(0).unwrap()
    };
    assert!(id > 0);

    let mut rows = conn
        .query("SELECT COUNT(*) FROM docs_fts WHERE docs_fts MATCH ?", ["needle"])
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("fts count row");
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 1, "FTS trigger should index inserted content");

    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .unwrap();
    tx.execute("INSERT INTO docs (content) VALUES (?1)", ["rolled back"])
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    let mut rows = conn
        .query("SELECT COUNT(*) FROM docs WHERE content = 'rolled back'", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("rollback count row");
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn current_readonly_contract_still_rejects_writes_before_backend_swap() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();
    let db_path = dir.path().join("data.db");

    let conn = open_database_path_readonly(&db_path).unwrap();
    let err = conn
        .execute("CREATE TABLE readonly_probe (id INTEGER)", ())
        .expect_err("readonly connection should reject writes");

    assert!(err.to_string().contains("readonly database"));
}
```

- [ ] **Step 1.3: Run compatibility gate**

Run:

```bash
devenv shell -- cargo test -p right-db --test turso_compat
```

Expected: PASS.

If this fails because `turso` does not support the existing local DB file, FTS5, triggers, `RETURNING`, immediate transactions, or other required semantics, stop the migration and report the exact failing capability. Do not proceed to Task 2.

- [ ] **Step 1.4: Commit the compatibility gate**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock crates/right-db/Cargo.toml crates/right-db/tests/turso_compat.rs
devenv shell -- git commit -m "test(db): add turso compatibility gate"
```

## Task 2: Port Params, Rows, And Errors To `turso`

**Files:**
- Modify `crates/right-db/src/params.rs`
- Modify `crates/right-db/src/row.rs`
- Modify `crates/right-db/src/error.rs`

- [ ] **Step 2.1: Port params to `turso::Params` and `turso::Value`**

In [params.rs](/Users/developer/dev/rightclaw/crates/right-db/src/params.rs), replace all `libsql` references with `turso`, and rename `into_libsql` to `into_turso`.

Key resulting definitions must be:

```rust
#[derive(Debug)]
#[doc(hidden)]
pub struct Params(turso::Params);

impl Params {
    pub(crate) fn into_turso(self) -> turso::Params {
        self.0
    }
}

pub trait IntoValue {
    fn into_value(self) -> Result<turso::Value, DbError>;
}

#[derive(Debug, Default)]
pub struct ParamsBuilder {
    values: Vec<turso::Value>,
    error: Option<DbError>,
}

impl IntoParams for () {
    fn into_params(self) -> Result<Params, DbError> {
        Ok(Params(turso::Params::None))
    }
}

impl IntoParams for [(); 0] {
    fn into_params(self) -> Result<Params, DbError> {
        Ok(Params(turso::Params::None))
    }
}
```

Keep existing tuple/array/`Option<T>` behavior, but construct `turso::Params::Positional(values)` and `turso::Value::{Text,Integer,Real,Null}`.

- [ ] **Step 2.2: Port rows to `turso::Row` and `turso::Value`**

In [row.rs](/Users/developer/dev/rightclaw/crates/right-db/src/row.rs), replace the file with this driver-neutral public shape:

```rust
use crate::DbError;

pub struct Row<'row> {
    inner: &'row turso::Row,
}

impl<'row> Row<'row> {
    pub(crate) fn new(inner: &'row turso::Row) -> Self {
        Self { inner }
    }

    pub fn get<I, T>(&self, idx: I) -> Result<T, DbError>
    where
        I: TryInto<usize>,
        T: FromValue,
    {
        let idx = idx
            .try_into()
            .map_err(|_| DbError::InvalidParameter("column index does not fit in usize".into()))?;
        T::from_value(self.inner.get_value(idx)?)
    }
}

pub trait FromValue: Sized {
    fn from_value(value: turso::Value) -> Result<Self, DbError>;
}

impl FromValue for turso::Value {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        Ok(value)
    }
}
```

Keep the existing `FromValue` impls for `i64`, `i32`, `u64`, `u32`, `f64`, `bool`, `String`, `Vec<u8>`, and `Option<T>`, replacing `libsql::Value` with `turso::Value`. Keep the existing error text for type mismatches except the column-index integer type now says `usize`.

- [ ] **Step 2.3: Port `DbError` to `turso::Error`**

In [error.rs](/Users/developer/dev/rightclaw/crates/right-db/src/error.rs), replace `libsql::Error` with `turso::Error` and replace constraint classification with Turso variants:

```rust
use std::path::PathBuf;

/// Errors from per-agent database operations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Database(#[from] turso::Error),

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
        source: turso::Error,
    },

    #[error("migration {version} on {path}: {source}")]
    Migration {
        path: PathBuf,
        version: u32,
        #[source]
        source: Box<DbError>,
    },

    #[error("migration version {version} on {path}: {message}")]
    MigrationVersion {
        path: PathBuf,
        version: u32,
        message: String,
    },
}

impl DbError {
    pub fn is_open_error(&self) -> bool {
        matches!(self, Self::Open { .. })
    }

    pub fn is_constraint_violation(&self) -> bool {
        match self {
            Self::Constraint(_) => true,
            Self::Database(error) => is_turso_constraint(error),
            Self::Migration { source, .. } => source.is_constraint_violation(),
            _ => false,
        }
    }
}

fn is_turso_constraint(error: &turso::Error) -> bool {
    matches!(error, turso::Error::Constraint(_))
}
```

Keep the existing constraint-violation unit test in the same file.

- [ ] **Step 2.4: Run params/row/error targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-db params::tests row::tests error::tests
```

Expected before connection migration: compile may fail only because connection/transaction still call `into_libsql` or expect `libsql` row/value types. That failure is acceptable at this step and should point to Task 3 changes. If errors are unrelated to the planned driver swap, fix them before continuing.

## Task 3: Port Connection And Transaction To `turso`

**Files:**
- Modify `crates/right-db/src/connection.rs`
- Modify `crates/right-db/src/transaction.rs`

- [ ] **Step 3.1: Port `Connection` storage and open path**

In [connection.rs](/Users/developer/dev/rightclaw/crates/right-db/src/connection.rs):

- Replace `libSQL`/`libsql` wording in comments with `Turso`/`turso`.
- Change the runtime initialization message to `right-db turso runtime should initialize`.
- Change the struct fields to:

```rust
pub struct Connection {
    db_path: PathBuf,
    _database: turso::Database,
    inner: turso::Connection,
}
```

- Replace `open_in_memory`, `open_local`, and `build` with:

```rust
pub fn open_in_memory() -> Result<Self, DbError> {
    Self::build(PathBuf::from(":memory:"), true)
}

pub(crate) fn open_local(db_path: PathBuf, create: bool) -> Result<Self, DbError> {
    if !create && !db_path.exists() {
        return Err(DbError::Open {
            path: db_path.clone(),
            source: turso::Error::Readonly(format!(
                "database file does not exist: {}",
                db_path.display()
            )),
        });
    }
    Self::build(db_path, create)
}

fn build(db_path: PathBuf, create: bool) -> Result<Self, DbError> {
    let runtime = shared_runtime();
    let path = db_path.to_string_lossy().into_owned();
    let open_err = |source| DbError::Open {
        path: db_path.clone(),
        source,
    };
    let database =
        block_on_runtime_safe(runtime, turso::Builder::new_local(&path).build()).map_err(open_err)?;
    let inner = database.connect().map_err(open_err)?;
    let conn = Self {
        db_path,
        _database: database,
        inner,
    };
    if !create {
        conn.execute_batch("PRAGMA query_only = ON")?;
    }
    Ok(conn)
}
```

The `PRAGMA query_only = ON` line is the read-only fallback because the public `turso::Builder` API does not expose the same `OpenFlags` surface as `libsql`. The read-only smoke tests in Task 4 decide whether this is acceptable. If those tests show file mutation or write acceptance, stop and report the read-only blocker.

- [ ] **Step 3.2: Port query/execute helpers to `block_on_turso`**

In [connection.rs](/Users/developer/dev/rightclaw/crates/right-db/src/connection.rs), replace each `into_libsql()` call with `into_turso()`, and replace `block_on_libsql` with:

```rust
pub(crate) fn block_on_turso<T: Send>(
    &self,
    future: impl Future<Output = turso::Result<T>> + Send,
) -> Result<T, DbError> {
    block_on_runtime_safe(shared_runtime(), future).map_err(Into::into)
}
```

Update caller examples:

```rust
let params = params.into_params()?.into_turso();
let changed = self.block_on_turso(self.inner.execute(sql, params))?;
```

```rust
let params = params.into_params()?.into_turso();
let mut rows = self.block_on_turso(self.inner.query(sql, params))?;
let Some(row) = self.block_on_turso(rows.next())? else {
    return Err(DbError::NotFound);
};
map(&crate::row::Row::new(&row))
```

- [ ] **Step 3.3: Port transactions to Turso immediate transactions**

In [connection.rs](/Users/developer/dev/rightclaw/crates/right-db/src/connection.rs), replace `transaction_with_behavior` with:

```rust
fn transaction_with_behavior(
    &self,
    behavior: turso::transaction::TransactionBehavior,
) -> Result<crate::transaction::Transaction<'_>, DbError> {
    let inner = self.block_on_turso(turso::Transaction::new_unchecked(&self.inner, behavior))?;
    Ok(crate::transaction::Transaction::new(self, inner))
}
```

Then update `transaction()` and `with_immediate_transaction()` to use `turso::transaction::TransactionBehavior::Immediate`.

- [ ] **Step 3.4: Port connection pragmas**

In [connection.rs](/Users/developer/dev/rightclaw/crates/right-db/src/connection.rs), keep the busy timeout, and test whether Turso accepts WAL mode:

```rust
pub(crate) fn apply_connection_pragmas(&self) -> Result<(), DbError> {
    self.execute_batch("PRAGMA journal_mode=WAL")?;
    self.inner.busy_timeout(BUSY_TIMEOUT)?;
    Ok(())
}

pub(crate) fn apply_readonly_pragmas(&self) -> Result<(), DbError> {
    self.inner.busy_timeout(BUSY_TIMEOUT)?;
    Ok(())
}
```

If `PRAGMA journal_mode=WAL` is unsupported by the Turso engine, remove only this pragma and update `open_connection_sets_sqlite_pragmas` in Task 4 to assert the Turso-supported concurrency setting instead. Do not silently ignore the failed pragma.

- [ ] **Step 3.5: Port `Transaction`**

In [transaction.rs](/Users/developer/dev/rightclaw/crates/right-db/src/transaction.rs):

- Replace `libsql::Transaction` with `turso::Transaction`.
- Replace `into_libsql()` with `into_turso()`.
- Replace `block_on_libsql` calls with `block_on_turso`.
- Update docs from `libSQL/libsql` to `Turso/turso`.

Key resulting fields and constructor:

```rust
pub struct Transaction<'conn> {
    conn: &'conn Connection,
    inner: Option<turso::Transaction<'conn>>,
}

impl<'conn> Transaction<'conn> {
    pub(crate) fn new(conn: &'conn Connection, inner: turso::Transaction<'conn>) -> Self {
        Self {
            conn,
            inner: Some(inner),
        }
    }
}
```

Keep `Deref<Target = Connection>` for now. The existing `transaction_deref_helper_write_is_rolled_back` test must pass under Turso; if it fails, stop and replace the `Deref` convenience with explicit transaction-aware helper paths in a separate design update.

- [ ] **Step 3.6: Run connection and transaction tests**

Run:

```bash
devenv shell -- cargo test -p right-db connection::tests transaction
```

Expected: PASS. If the `transaction_deref_helper_write_is_rolled_back` test fails, stop and report that the backend swap invalidates the current `Deref<Target = Connection>` transaction invariant.

## Task 4: Rename Smoke Tests And Prove Local Contracts Under Turso

**Files:**
- Modify `crates/right-db/tests/smoke.rs`
- Modify or remove `crates/right-db/tests/turso_compat.rs`

- [ ] **Step 4.1: Import explicit read-only path helper**

In [smoke.rs](/Users/developer/dev/rightclaw/crates/right-db/tests/smoke.rs), change the first import to:

```rust
use right_db::{
    MIGRATIONS, open_connection, open_connection_readonly, open_database_path_readonly, open_db,
};
use tempfile::tempdir;
```

- [ ] **Step 4.2: Rename `libsql_*` tests**

In [smoke.rs](/Users/developer/dev/rightclaw/crates/right-db/tests/smoke.rs), rename these tests and messages:

```text
libsql_open_connection_creates_file_and_preserves_local_path -> turso_open_connection_creates_file_and_preserves_local_path
libsql_open_connection_readonly_requires_existing_db -> turso_open_connection_readonly_requires_existing_db
libsql_migrations_set_latest_user_version -> turso_migrations_set_latest_user_version
libsql_migrations_are_idempotent_on_existing_data_db -> turso_migrations_are_idempotent_on_existing_data_db
libsql_migrations_static_runs_with_right_db_connection -> turso_migrations_static_runs_with_right_db_connection
libsql_supports_conversation_fts_triggers -> turso_supports_conversation_fts_triggers
libsql_supports_returning_clause -> turso_supports_returning_clause
libsql_transaction_rolls_back_on_error -> turso_transaction_rolls_back_on_error
```

Also replace message text like `local libsql open should create data.db` with `local turso open should create data.db`.

- [ ] **Step 4.3: Add explicit-path read-only tests**

Add these tests immediately after `turso_open_connection_readonly_requires_existing_db`:

```rust
#[test]
fn open_database_path_readonly_missing_file_does_not_create_file() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("restore-probe.db");

    let err = open_database_path_readonly(&db_path).expect_err("missing db should not open");

    assert!(
        !db_path.exists(),
        "readonly explicit path must not create a db file"
    );
    assert!(
        err.is_open_error(),
        "expected readonly open failure, got {err:?}"
    );
}

#[test]
fn open_database_path_readonly_existing_file_rejects_writes_and_preserves_version() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();
    let db_path = dir.path().join("data.db");

    let conn = open_database_path_readonly(&db_path).unwrap();
    let before = query_user_version(&conn);

    let err = conn
        .execute("CREATE TABLE explicit_readonly_write_probe (id INTEGER)", ())
        .expect_err("readonly explicit path should reject writes");

    assert!(err.to_string().contains("readonly"));
    assert_eq!(
        query_user_version(&conn),
        before,
        "readonly explicit path must not change user_version",
    );
}
```

- [ ] **Step 4.4: Remove direct Turso probe file**

After `right-db` is fully migrated, `turso_compat.rs` no longer proves `libsql`-created files because it would create files through the new backend. Remove it after the pre-migration gate has passed and the production smoke tests cover the local contract:

```bash
devenv shell -- git rm crates/right-db/tests/turso_compat.rs
```

- [ ] **Step 4.5: Run right-db smoke and migration tests**

Run:

```bash
devenv shell -- cargo test -p right-db open_database_path_readonly
devenv shell -- cargo test -p right-db turso_
devenv shell -- cargo test -p right-db migration_runner_semantics
devenv shell -- cargo test -p right-db cold_boot_concurrent_migrators_do_not_double_apply_v23
devenv shell -- cargo test -p right-db
```

Expected: PASS. If read-only tests pass only because writes fail but files are still mutated on open, stop and report the read-only limitation; do not weaken the structural read-only requirement without a design update.

- [ ] **Step 4.6: Commit right-db Turso migration**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock crates/right-db/Cargo.toml crates/right-db/src crates/right-db/tests
devenv shell -- git commit -m "refactor(db): migrate local driver to turso"
```

## Task 5: Remove `libsql` And Update Architecture Docs

**Files:**
- Modify `Cargo.toml`
- Modify `crates/right-db/Cargo.toml`
- Modify `ARCHITECTURE.md`
- Modify `docs/architecture/modules.md`
- Modify `docs/architecture/memory.md` if needed

- [ ] **Step 5.1: Remove `libsql` dependency**

In workspace [Cargo.toml](/Users/developer/dev/rightclaw/Cargo.toml), remove:

```toml
libsql = "0.9.30"
```

In [crates/right-db/Cargo.toml](/Users/developer/dev/rightclaw/crates/right-db/Cargo.toml), remove:

```toml
libsql = { workspace = true }
```

- [ ] **Step 5.2: Verify no project code references `libsql`**

Run:

```bash
devenv shell -- rg -n "libsql|libSQL|rusqlite|rusqlite_migration" Cargo.toml crates
```

Expected: no matches. If matches are only in historical docs outside `Cargo.toml` and `crates`, leave them unless they describe current behavior.

- [ ] **Step 5.3: Update architecture docs**

In [ARCHITECTURE.md](/Users/developer/dev/rightclaw/ARCHITECTURE.md), update the workspace table row:

```markdown
| **right-db** | `crates/right-db/` | Per-agent SQLite-compatible `data.db` boundary over local Turso: project DB types, migrations, `sql/v*.sql` |
```

In `ARCHITECTURE.md` Local Database Rules, replace:

```markdown
Per-agent `data.db` is a SQLite-compatible database. Local libSQL is the
current driver implementation and is hidden behind `right-db`.
```

with:

```markdown
Per-agent `data.db` is a SQLite-compatible database. Local Turso is the
current driver implementation and is hidden behind `right-db`.
```

In [docs/architecture/modules.md](/Users/developer/dev/rightclaw/docs/architecture/modules.md), update the `right-db` bullet:

```markdown
- `Connection`, `Transaction`, `DbError` — project-owned boundary over the local Turso driver for per-agent SQLite-compatible `data.db`.
```

In [docs/architecture/memory.md](/Users/developer/dev/rightclaw/docs/architecture/memory.md), replace current local-driver wording with:

```markdown
via `right-db`'s local Turso driver, not Hindsight.
```

- [ ] **Step 5.4: Run doc drift search**

Run:

```bash
devenv shell -- rg -n "local libSQL|local libsql|libsql driver|libSQL driver|rusqlite|rusqlite_migration" ARCHITECTURE.md docs/architecture Cargo.toml crates
```

Expected:

- No current-behavior references to local `libsql`.
- Historical docs under `docs/superpowers/` may still mention `libsql`; do not rewrite history.

- [ ] **Step 5.5: Commit dependency cleanup and docs**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock crates/right-db/Cargo.toml ARCHITECTURE.md docs/architecture/modules.md docs/architecture/memory.md
devenv shell -- git commit -m "docs(db): document local turso boundary"
```

## Task 6: Dependent Package Verification

**Files:**
- No planned edits unless tests expose a real compatibility issue.

- [ ] **Step 6.1: Run targeted dependent package tests**

Run:

```bash
devenv shell -- cargo test -p right-memory
devenv shell -- cargo test -p right-mcp
devenv shell -- cargo test -p right-lifecycle
devenv shell -- cargo test -p right-agent
devenv shell -- cargo test -p right-dashboard
devenv shell -- cargo test -p right-bot
devenv shell -- cargo test -p right
```

Expected: PASS.

If a failure is caused by a changed `right-db` error message or transaction behavior, add a focused regression test in the owning crate before fixing. Do not broaden public `right-db` APIs unless a caller break is unavoidable.

- [ ] **Step 6.2: Commit any dependent fixes**

If Step 6.1 required code changes, do not use a generic staging command. Run:

```bash
devenv shell -- git status --short
```

Then stage only the exact files changed for the dependent fix and commit with:

```bash
devenv shell -- git commit -m "fix(db): preserve dependent database behavior"
```

If no files changed, skip this step. If the dependent failure requires a public `right-db` API change, stop and revise the plan before committing.

## Task 7: Final Workspace Verification

**Files:**
- No planned edits.

- [ ] **Step 7.1: Final full workspace test**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 7.2: Final debug build**

Run:

```bash
devenv shell -- cargo build --workspace
```

- [ ] **Step 7.3: Final surface checks**

Run:

```bash
devenv shell -- rg -n "libsql|libSQL|rusqlite|rusqlite_migration" Cargo.toml crates ARCHITECTURE.md docs/architecture
devenv shell -- rg -n "push\\(|pull\\(|checkpoint\\(|TURSO_DATABASE_URL|TURSO_AUTH_TOKEN|turso cloud|sync scheduler" crates ARCHITECTURE.md docs/architecture
devenv shell -- git status --short
```

Expected:

- No current project code or architecture docs reference `libsql`/`rusqlite`.
- No cloud sync behavior, credentials, UI, CLI, bot command, or scheduler was introduced.
- `git status --short` is clean after final commits.

- [ ] **Step 7.4: Final commit if verification changed generated files**

If final verification changed only `Cargo.lock`, commit it:

```bash
devenv shell -- git add Cargo.lock
devenv shell -- git commit -m "chore(db): finalize turso migration"
```

If final verification changed Rust formatting, run `devenv shell -- git status --short`, stage only the Rust files listed there that were touched by this plan, and use the same commit message. Skip if the worktree is clean.
