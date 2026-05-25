# Turso Local Foundation And FTS Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace runtime `right-db` `libsql` usage with `turso`, enable the `sync` feature for future Turso Cloud work, and migrate local search from SQLite FTS5 virtual tables to Turso FTS indexes without adding cloud behavior.

**Architecture:** `right-db` remains the only database-driver boundary. Every local Turso open enables `experimental_index_method(true)`. Fresh schemas create Turso FTS indexes over base tables. Writable `open_connection` opens first run a private bundled-`rusqlite` scrubber to remove real legacy SQLite FTS5 virtual tables/triggers, then migrated opens run v34 to create equivalent Turso indexes. Read-only helpers do not scrub or mutate. No `push()`, `pull()`, credential, config, UI, CLI, bot command, scheduler, or restore behavior is included.

**Tech Stack:** Rust 2024, `right-db`, `turso` crate with `sync` feature, Turso index-method FTS, bundled `rusqlite` only for legacy FTS5 pre-scrub, Tokio runtime bridge, per-agent `data.db`, `devenv shell -- cargo`.

**Spec:** `docs/superpowers/specs/2026-05-25-turso-local-foundation-design.md`.

---

## File Structure

- Modify `Cargo.toml`
  - Add `turso = { version = "...", features = ["sync"] }`.
  - Add `rusqlite = { version = "...", features = ["bundled"] }` for the
    private legacy FTS5 scrubber only.
  - Remove workspace `libsql` after runtime and temporary gates no longer use it.
- Modify `crates/right-db/Cargo.toml`
  - Add workspace `turso`.
  - Add workspace `rusqlite`.
  - Remove workspace `libsql` after the transition gates are removed.
- Create then remove `crates/right-db/tests/turso_compat.rs`
  - Direct Turso compatibility probes. Direct driver usage is allowed here because this is a temporary boundary test.
- Modify `crates/right-db/src/connection.rs`
  - Wrap `turso::Database` and `turso::Connection`.
  - Enable `experimental_index_method(true)` for every local builder.
- Modify `crates/right-db/src/transaction.rs`
  - Wrap `turso::Transaction`.
- Modify `crates/right-db/src/error.rs`
  - Normalize `turso::Error` into project `DbError`.
- Modify `crates/right-db/src/lib.rs`
  - Run the legacy FTS5 scrubber before writable Turso `open_connection` opens.
- Modify `crates/right-db/src/params.rs`
  - Convert project params to `turso::Params` and `turso::Value`.
- Modify `crates/right-db/src/row.rs`
  - Convert `turso::Row` values into project row conversions.
- Modify `crates/right-db/src/sql/v1_schema.sql`
  - Replace `memories_fts` SQLite FTS5 virtual table and sync triggers with `idx_memories_turso_fts`.
- Modify `crates/right-db/src/sql/v21_conversation_messages.sql`
  - Replace `conversation_messages_fts` SQLite FTS5 virtual table and sync triggers with `idx_conversation_messages_turso_fts`.
- Create `crates/right-db/src/sql/v34_turso_fts_indexes.sql`
  - Drop any remaining legacy FTS5 sync triggers and virtual tables.
- Modify `crates/right-db/src/migrations.rs`
  - Add v34 SQL and hook, bump `LATEST_SCHEMA_VERSION`, update FTS tests.
- Modify `crates/right-db/src/conversation.rs`
  - Query `conversation_messages` with `content MATCH ?`.
  - Generate deterministic bounded snippets in Rust.
- Modify `crates/right-db/tests/smoke.rs`
  - Rename `libsql_*` tests to `turso_*` or driver-neutral names.
  - Assert Turso FTS index behavior instead of SQLite FTS5 virtual-table behavior.
- Modify `crates/right/src/main.rs`
  - Update memory search SQL from `memories_fts` joins to base-table Turso FTS.
- Inspect and update:
  - `ARCHITECTURE.md`
  - `docs/architecture/modules.md`
  - `docs/architecture/memory.md`
- No planned changes:
  - `PROMPT_SYSTEM.md`
  - cloud/Turso config, credentials, bot, CLI, dashboard, scheduler, or restore behavior.

## Task 0: Baseline And Dependency Probe

**Files:**
- Read: `docs/superpowers/specs/2026-05-25-turso-local-foundation-design.md`
- Read: `ARCHITECTURE.md`
- Read: `docs/architecture/modules.md`
- Read: `docs/architecture/memory.md`

- [ ] **Step 0.1: Record Rust skill availability**

This repository asks implementers to load `rust-dev:rust-dev` before writing Rust. If that skill is available in the implementation session, load it before Task 1. If it is not available, record this implementation note before editing Rust:

```text
rust-dev:rust-dev unavailable in this Codex session; proceeding with direct Rust edits under project Rust conventions.
```

- [ ] **Step 0.2: Re-read the revised spec and current local DB docs**

Run:

```bash
devenv shell -- sed -n '1,360p' docs/superpowers/specs/2026-05-25-turso-local-foundation-design.md
devenv shell -- sed -n '400,455p' ARCHITECTURE.md
devenv shell -- sed -n '64,72p' docs/architecture/modules.md
devenv shell -- sed -n '54,72p' docs/architecture/memory.md
```

Expected:

- Spec says migrate runtime `right-db` to `turso`.
- Spec says migrate search from SQLite FTS5 virtual tables to Turso FTS indexes.
- Spec says no cloud sync behavior in this stage.

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

Use the latest registry version returned by the command. If it differs from `0.7.0-pre.3`, update the version in every dependency edit in this plan before committing.

- [ ] **Step 0.4: Run targeted baseline**

Run:

```bash
devenv shell -- cargo test -p right-db
```

Expected: PASS. If this fails, stop and fix the pre-existing `right-db` failure before changing dependencies or code.

- [ ] **Step 0.5: Confirm current direct driver surface**

Run:

```bash
devenv shell -- rg -n "libsql|rusqlite|memories_fts|conversation_messages_fts|USING fts5|snippet\\(|bm25\\(" Cargo.toml crates/right-db crates/right docs ARCHITECTURE.md --glob '*.rs' --glob '*.sql' --glob '*.md' --glob '*.toml'
```

Expected:

- Runtime `libsql` usage is inside `right-db`.
- SQLite FTS5 references are in schema SQL, `right-db` search, `right/src/main.rs` memory search, tests, docs, and older specs/plans.
- No `rusqlite` dependency remains in active code.

## Task 1: Add Turso And Prove Local FTS Surface

**Files:**
- Modify `Cargo.toml`
- Modify `crates/right-db/Cargo.toml`
- Create `crates/right-db/tests/turso_compat.rs`

- [ ] **Step 1.1: Add `turso` while keeping `libsql` temporarily**

In workspace `Cargo.toml`, add the current registry version near the current `libsql` entry:

```toml
libsql = "0.9.30"
turso = { version = "0.7.0-pre.3", features = ["sync"] }
```

In `crates/right-db/Cargo.toml`, add `turso` while keeping `libsql`:

```toml
[dependencies]
libsql = { workspace = true }
tempfile = { workspace = true, optional = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
turso = { workspace = true }
```

- [ ] **Step 1.2: Write the Turso compatibility gate**

Create or replace `crates/right-db/tests/turso_compat.rs` with:

```rust
use right_db::{open_db, open_database_path_readonly};
use tempfile::tempdir;

async fn open_turso(path: &std::path::Path) -> turso::Result<turso::Connection> {
    let path = path
        .to_str()
        .expect("temp database path should be valid UTF-8");
    let db = turso::Builder::new_local(path)
        .experimental_index_method(true)
        .build()
        .await?;
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
         CREATE TABLE docs_audit (
             doc_id INTEGER NOT NULL,
             content TEXT NOT NULL
         );
         CREATE INDEX docs_turso_fts ON docs USING fts(content);
         CREATE TRIGGER docs_ai AFTER INSERT ON docs BEGIN
             INSERT INTO docs_audit(doc_id, content) VALUES (new.id, new.content);
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
        .query("SELECT COUNT(*) FROM docs WHERE content MATCH ?", ["needle"])
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("fts count row");
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 1, "Turso FTS index should find inserted content");

    let mut rows = conn
        .query("SELECT COUNT(*) FROM docs_audit WHERE content = ?", ["needle phrase"])
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("trigger count row");
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 1, "trigger should copy inserted content");

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

Expected: PASS. If it fails, stop and report the exact unsupported Turso capability.

- [ ] **Step 1.4: Commit the compatibility gate**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock crates/right-db/Cargo.toml crates/right-db/tests/turso_compat.rs
devenv shell -- git commit -m "test(db): add turso fts compatibility gate"
```

## Task 2: Port `right-db` Core Types To Turso

**Files:**
- Modify `crates/right-db/src/params.rs`
- Modify `crates/right-db/src/row.rs`
- Modify `crates/right-db/src/error.rs`
- Modify `crates/right-db/src/connection.rs`
- Modify `crates/right-db/src/transaction.rs`

- [ ] **Step 2.1: Port params, rows, and errors**

In `params.rs`:

- Replace `libsql::params::Params` with `turso::Params`.
- Replace `libsql::Value` with `turso::Value`.
- Rename `into_libsql` to `into_turso`.
- Keep tuple, array, `Vec<T>`, `ParamsBuilder`, and `Option<T>` behavior unchanged.

Key resulting definitions:

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
```

In `row.rs`:

```rust
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
```

In `error.rs`, replace the driver error variants with:

```rust
#[error("database error: {0}")]
Database(#[from] turso::Error),

#[error("open database {path}: {source}")]
Open {
    path: PathBuf,
    #[source]
    source: turso::Error,
},
```

and classify constraints with:

```rust
fn is_turso_constraint(error: &turso::Error) -> bool {
    matches!(error, turso::Error::Constraint(_))
}
```

- [ ] **Step 2.2: Port connection storage and local opens**

In `connection.rs`, change the connection struct to:

```rust
pub struct Connection {
    db_path: PathBuf,
    _database: turso::Database,
    inner: turso::Connection,
}
```

Replace `open_in_memory`, `open_local`, and `build` with Turso opens that enable index-method FTS:

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
    let database = block_on_runtime_safe(
        runtime,
        turso::Builder::new_local(&path)
            .experimental_index_method(true)
            .build(),
    )
    .map_err(open_err)?;
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

Replace `block_on_libsql` with:

```rust
pub(crate) fn block_on_turso<T: Send>(
    &self,
    future: impl Future<Output = turso::Result<T>> + Send,
) -> Result<T, DbError> {
    block_on_runtime_safe(shared_runtime(), future).map_err(Into::into)
}
```

Replace every `.into_libsql()` call with `.into_turso()`.

- [ ] **Step 2.3: Port transactions**

In `transaction.rs`:

- Replace `libsql::Transaction` with `turso::Transaction`.
- Replace `.into_libsql()` with `.into_turso()`.
- Replace `block_on_libsql` with `block_on_turso`.
- Keep `Deref<Target = Connection>` only if `transaction_deref_helper_write_is_rolled_back` passes after this task.
- Update the struct-level docs to refer to Turso, not libSQL, after the test passes.

In `connection.rs`, change transaction behavior calls to:

```rust
pub fn transaction(&self) -> Result<crate::transaction::Transaction<'_>, DbError> {
    self.transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
}

fn transaction_with_behavior(
    &self,
    behavior: turso::transaction::TransactionBehavior,
) -> Result<crate::transaction::Transaction<'_>, DbError> {
    let inner = self.block_on_turso(self.inner.transaction_with_behavior(behavior))?;
    Ok(crate::transaction::Transaction::new(self, inner))
}
```

- [ ] **Step 2.4: Run the narrow compile/test check**

Run:

```bash
devenv shell -- cargo test -p right-db transaction_deref_helper_write_is_rolled_back error::tests::is_constraint_violation_detects_unique_violation
```

Expected after Task 2 only: compile may still fail because schema SQL and search queries still use SQLite FTS5. Fix only driver-wrapper compile errors in this task; schema/search changes are Task 3.

## Task 3: Replace Fresh Schema And Search With Turso FTS

**Files:**
- Modify `crates/right-db/src/sql/v1_schema.sql`
- Modify `crates/right-db/src/sql/v21_conversation_messages.sql`
- Modify `crates/right-db/src/conversation.rs`
- Modify `crates/right-db/src/migrations.rs`
- Modify `crates/right-db/tests/smoke.rs`
- Modify `crates/right/src/main.rs`

- [ ] **Step 3.1: Replace fresh memory FTS schema**

In `crates/right-db/src/sql/v1_schema.sql`, delete the `CREATE VIRTUAL TABLE memories_fts ...` block and the `memories_ai`, `memories_ad`, and `memories_au` triggers. Add:

```sql
CREATE INDEX IF NOT EXISTS idx_memories_turso_fts
ON memories USING fts(content);
```

- [ ] **Step 3.2: Replace fresh conversation FTS schema**

In `crates/right-db/src/sql/v21_conversation_messages.sql`, delete the `CREATE VIRTUAL TABLE conversation_messages_fts ...` block and the `conversation_messages_ai`, `conversation_messages_ad`, and `conversation_messages_au` triggers. Add:

```sql
CREATE INDEX IF NOT EXISTS idx_conversation_messages_turso_fts
ON conversation_messages USING fts(content);
```

- [ ] **Step 3.3: Rewrite conversation search queries**

In `crates/right-db/src/conversation.rs`, change `search_thread` and `search_chat` to select `m.content` from `conversation_messages` directly:

```sql
SELECT
    m.id,
    m.role,
    m.content,
    m.sender_user_id,
    m.sender_name,
    m.created_at,
    m.thread_id,
    m.message_id,
    m.root_session_id
 FROM conversation_messages m
 WHERE m.content MATCH ?
   AND m.platform = 'telegram'
   AND m.chat_id = ?
   AND m.thread_id = ?
 ORDER BY m.created_at DESC, m.id DESC
 LIMIT ?
```

and for chat-wide search:

```sql
SELECT
    m.id,
    m.role,
    m.content,
    m.sender_user_id,
    m.sender_name,
    m.created_at,
    m.thread_id,
    m.message_id,
    m.root_session_id
 FROM conversation_messages m
 WHERE m.content MATCH ?
   AND m.platform = 'telegram'
   AND m.chat_id = ?
 ORDER BY m.created_at DESC, m.id DESC
 LIMIT ?
```

Keep the existing normalized FTS query input. Replace SQLite `snippet(...)` output with a bounded Rust snippet:

```rust
fn bounded_search_snippet(content: &str) -> String {
    const MAX_SNIPPET_CHARS: usize = 180;
    let mut snippet = content.trim().chars().take(MAX_SNIPPET_CHARS).collect::<String>();
    if content.trim().chars().count() > MAX_SNIPPET_CHARS {
        snippet.push_str("...");
    }
    snippet
}
```

Use that helper when mapping the `m.content` column into `ConversationSearchResult.snippet`.

- [ ] **Step 3.4: Rewrite memory search in `right` CLI/backend**

In `crates/right/src/main.rs`, replace the current memory search join:

```sql
JOIN memories_fts f ON m.id = f.rowid
WHERE memories_fts MATCH ?1
ORDER BY bm25(memories_fts)
```

with base-table Turso FTS:

```sql
WHERE m.content MATCH ?1
ORDER BY m.created_at DESC, m.id DESC
```

Keep existing tag/deleted/limit filters intact around that predicate.

- [ ] **Step 3.5: Update schema/search tests**

In `crates/right-db/tests/smoke.rs`:

- Rename `libsql_supports_conversation_fts_triggers` to `turso_supports_conversation_fts_index`.
- Change its count query to:

```rust
"SELECT COUNT(*) FROM conversation_messages WHERE content MATCH ?"
```

- Rename `schema_has_memories_fts` to `schema_has_memories_turso_fts_index`.
- Assert the index exists with:

```rust
query_index_count(&conn, "idx_memories_turso_fts")
```

- In `schema_has_conversation_messages_table`, assert `conversation_messages` exists and `idx_conversation_messages_turso_fts` exists.

Add this helper beside `query_table_count`:

```rust
fn query_index_count(conn: &right_db::Connection, index_name: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?",
        [index_name],
        |row| row.get(0),
    )
    .unwrap()
}
```

In `crates/right-db/src/migrations.rs`, update `conversation_messages_schema_exists` and `conversation_messages_fts_tracks_updates` to use `conversation_messages WHERE content MATCH ...` and index names instead of `conversation_messages_fts`.

- [ ] **Step 3.6: Run focused Turso search tests**

Run:

```bash
devenv shell -- cargo test -p right-db turso_supports_conversation_fts_index schema_has_memories_turso_fts_index schema_has_conversation_messages_table conversation_messages_fts_tracks_updates
```

Expected: PASS. If Turso FTS index metadata is not represented as `sqlite_master type='index'`, update the helper to the observed metadata shape and keep the test name focused on the user-visible index contract.

## Task 4: Add Legacy FTS5 Scrubber And v34 Migration

**Files:**
- Modify `Cargo.toml`
- Modify `crates/right-db/Cargo.toml`
- Modify `crates/right-db/src/lib.rs`
- Modify `crates/right-db/src/error.rs`
- Create `crates/right-db/src/sql/v34_turso_fts_indexes.sql`
- Modify `crates/right-db/src/migrations.rs`

- [ ] **Step 4.1: Add the pre-Turso legacy FTS5 scrubber**

Use bundled `rusqlite` inside `right-db` only. Before `Connection::open_local`
in writable `open_connection(path, migrate)`, detect existing `data.db` files
with legacy FTS5 objects and drop:

- `memories_fts`
- `conversation_messages_fts`
- `memories_ai`, `memories_ad`, `memories_au`
- `conversation_messages_ai`, `conversation_messages_ad`,
  `conversation_messages_au`

This step is outside the Turso migration transaction because Turso cannot
reliably resolve real legacy FTS5 schemas before the virtual tables are removed.
It also runs for `migrate=false` writable backup paths; read-only helpers must
not run it.

- [ ] **Step 4.2: Add v34 SQL**

Create `crates/right-db/src/sql/v34_turso_fts_indexes.sql`:

```sql
DROP TRIGGER IF EXISTS memories_ai;
DROP TRIGGER IF EXISTS memories_ad;
DROP TRIGGER IF EXISTS memories_au;

DROP TRIGGER IF EXISTS conversation_messages_ai;
DROP TRIGGER IF EXISTS conversation_messages_ad;
DROP TRIGGER IF EXISTS conversation_messages_au;

DROP TABLE IF EXISTS memories_fts;
DROP TABLE IF EXISTS conversation_messages_fts;
```

- [ ] **Step 4.3: Register v34**

In `crates/right-db/src/migrations.rs`, add:

```rust
const V34_SCHEMA: &str = include_str!("sql/v34_turso_fts_indexes.sql");

pub const LATEST_SCHEMA_VERSION: u32 = 34;
```

Add the migration after v33:

```rust
Migration {
    version: 34,
    sql: V34_SCHEMA,
    hook: Some(v34_turso_fts_indexes),
},
```

- [ ] **Step 4.4: Add v34 regression tests**

In `crates/right-db/src/migrations.rs`, add regression tests near the other
migration tests:

```rust
#[test]
fn v34_drops_legacy_fts_triggers_and_creates_turso_indexes() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 33).unwrap();

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS conversation_messages_fts (
             rowid INTEGER PRIMARY KEY,
             content TEXT NOT NULL
         );
         CREATE TRIGGER IF NOT EXISTS conversation_messages_ai
         AFTER INSERT ON conversation_messages
         BEGIN
             INSERT INTO conversation_messages_fts(rowid, content)
             VALUES (new.id, new.content);
         END;",
    )
    .unwrap();

    MIGRATIONS.to_latest(&mut conn).unwrap();

    let trigger_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='trigger' AND name='conversation_messages_ai'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trigger_count, 0, "legacy FTS trigger must be removed");

    let index_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='index' AND name='idx_conversation_messages_turso_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(index_count, 1, "Turso FTS index must exist");

    conn.execute(
        "INSERT INTO conversation_messages (chat_id, thread_id, role, content)
         VALUES (1, 0, 'user', 'legacy needle')",
        [],
    )
    .unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM conversation_messages WHERE content MATCH 'needle'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
```

Also add a real legacy SQLite FTS5 fixture using `rusqlite` that sets
`PRAGMA user_version = 33`, opens with `right_db::open_connection(..., true)`,
and proves:

- legacy FTS5 triggers are gone;
- `memories_fts` and `conversation_messages_fts` are gone;
- Turso FTS indexes exist;
- existing and post-migration rows are both searchable through base-table
  `MATCH`.

Add a second regression for `right_db::open_connection(..., false)` proving the
scrubber still runs for writable backup-style opens before Turso resolves the
schema.

- [ ] **Step 4.5: Run v34 tests**

Run:

```bash
devenv shell -- cargo test -p right-db v34_drops_legacy_fts_triggers_and_creates_turso_indexes v34_migrates_real_legacy_fts5_virtual_tables
```

Expected: PASS. Rename the migration-runner test from `libsql` to a driver-neutral name in the same edit if it still contains `libsql`.

## Task 5: Remove Temporary Libsql Surface

**Files:**
- Modify `Cargo.toml`
- Modify `crates/right-db/Cargo.toml`
- Delete `crates/right-db/tests/turso_compat.rs`
- Modify `crates/right-db/tests/smoke.rs`
- Modify Rust comments mentioning `libSQL` or `libsql`

- [ ] **Step 5.1: Remove direct `libsql` dependency**

Remove these dependency entries:

```toml
libsql = "0.9.30"
libsql = { workspace = true }
```

Keep:

```toml
turso = { version = "0.7.0-pre.3", features = ["sync"] }
turso = { workspace = true }
```

- [ ] **Step 5.2: Remove temporary gate test**

Delete `crates/right-db/tests/turso_compat.rs`. Its direct checks are now represented by production smoke and migration tests.

- [ ] **Step 5.3: Remove stale driver wording**

Run:

```bash
devenv shell -- rg -n "libsql|libSQL|rusqlite" Cargo.toml crates/right-db crates/right ARCHITECTURE.md docs/architecture --glob '*.rs' --glob '*.toml' --glob '*.md'
```

Update only active runtime/test/docs references touched by this migration. Do not edit historical specs or unrelated old plans.

- [ ] **Step 5.4: Run right-db suite**

Run:

```bash
devenv shell -- cargo test -p right-db
```

Expected: PASS.

- [ ] **Step 5.5: Commit runtime migration**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock crates/right-db crates/right
devenv shell -- git commit -m "refactor(db): migrate local storage to turso fts"
```

## Task 6: Update Architecture Docs

**Files:**
- Modify `ARCHITECTURE.md`
- Modify `docs/architecture/modules.md`
- Modify `docs/architecture/memory.md`

- [ ] **Step 6.1: Update local database rules**

In `ARCHITECTURE.md`, update the local database section to state:

```markdown
- `right-db` is the only crate that owns local database-driver details.
- Runtime local storage uses the `turso` crate with `sync` enabled for future
  Turso Cloud backup work.
- Local opens must enable Turso's experimental index-method feature because
  conversation and memory search use `CREATE INDEX ... USING fts`.
- No crate outside `right-db` may expose raw `turso` connection, row,
  transaction, value, parameter, or error types.
```

- [ ] **Step 6.2: Update module and memory docs**

In `docs/architecture/modules.md`, update the `right-db` bullet to mention local Turso and project-owned wrappers.

In `docs/architecture/memory.md`, replace SQLite FTS5 wording with:

```markdown
Conversation transcript search and legacy memory search use local Turso FTS
indexes over the base tables. The schema no longer creates SQLite FTS5 virtual
tables for fresh databases; migration v34 removes old FTS5 sync triggers and
creates Turso FTS indexes for existing databases.
```

- [ ] **Step 6.3: Run doc reference scan**

Run:

```bash
devenv shell -- rg -n "local libsql|SQLite FTS5|memories_fts|conversation_messages_fts|USING fts5" ARCHITECTURE.md docs/architecture crates/right-db/src crates/right-db/tests crates/right/src --glob '*.md' --glob '*.rs' --glob '*.sql'
```

Expected:

- Historical old specs/plans may still mention the old model.
- Active architecture docs and active runtime code describe Turso FTS.

- [ ] **Step 6.4: Commit docs**

Run:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/modules.md docs/architecture/memory.md
devenv shell -- git commit -m "docs(db): document local turso fts boundary"
```

## Task 7: Dependent Package Checks

**Files:**
- No planned edits unless failures identify direct drift from this migration.

- [ ] **Step 7.1: Run targeted dependent tests**

Run:

```bash
devenv shell -- cargo test -p right
```

Expected: PASS. If memory search SQL changed in `crates/right/src/main.rs`, failures should point to missing Turso FTS query rewrites or test expectations.

- [ ] **Step 7.2: Run direct search smoke filters**

Run:

```bash
devenv shell -- cargo test -p right-db search_thread search_chat
```

Expected: PASS.

## Task 8: Final Verification

**Files:**
- No planned edits.

- [ ] **Step 8.1: Run full workspace test suite**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 8.2: Run full workspace build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 8.3: Final surface scan**

Run:

```bash
devenv shell -- rg -n "libsql|rusqlite|USING fts5|conversation_messages_fts|memories_fts|push\\(|pull\\(|with_auth_token|new_remote" Cargo.toml crates docs/architecture ARCHITECTURE.md --glob '*.rs' --glob '*.sql' --glob '*.toml' --glob '*.md'
```

Expected:

- No runtime `libsql` references; `rusqlite` appears only in `right-db` legacy
  FTS5 scrubber code/tests.
- No active schema/search dependence on SQLite FTS5 virtual tables.
- No cloud sync calls, remote URLs, auth-token setup, or sync scheduler code.

- [ ] **Step 8.4: Commit any final cleanup**

If Step 8.3 required cleanup, commit with:

```bash
devenv shell -- git add <changed-files>
devenv shell -- git commit -m "chore(db): clean turso migration surface"
```

If Step 8.3 required no cleanup, do not create an empty commit.
