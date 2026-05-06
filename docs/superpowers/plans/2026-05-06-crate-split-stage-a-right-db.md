# Crate Split Stage A — Extract `right-db` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carve the per-agent SQLite plumbing (`open_connection`, `open_db`, all 17+ migrations, `sql/v*.sql` files) out of `right-agent::memory` into a brand-new `right-db` crate. Move three misplaced auth-token helpers from `memory::store` to `mcp::credentials` (still inside `right-agent` for now). After this stage `right-agent::memory` is purely Hindsight + retain queue.

**Architecture:** Create `crates/right-db/` as a thin foundation crate (deps: `rusqlite`, `rusqlite_migration`, `thiserror`, `tempfile` for tests). It exports `DbError`, `open_db`, `open_connection`, and re-exports `migrations::MIGRATIONS` for tests that need to drive migrations on `Connection::open_in_memory()`. `right-agent::memory::MemoryError` becomes a slim wrapper that re-exposes `DbError` via `#[from]`, so existing code paths keep compiling without per-callsite error-type rewrites. `right-agent`, `right-bot`, and `right` each add `right-db` as a direct dependency; ~50 callsites of `right_agent::memory::open_connection` / `open_db` / `memory::store::*_auth_token` get rewritten in batched per-file commits.

**Tech Stack:** Rust 2024, Cargo workspace, `rusqlite` (bundled feature), `rusqlite_migration` 2.5, `thiserror` 2.0, `tempfile`, `include_str!` for SQL files. Spec at `docs/superpowers/specs/2026-05-06-crate-split-design.md` (commit `16429d54`). All commands run via `devenv shell -- <cmd>` because the project's CLAUDE.md mandates it when `devenv.nix` exists at repo root.

**Pre-existing context:**
- `crates/right-agent/src/memory/mod.rs:35-71` defines `open_db` / `open_connection` and a small `tests` module that exercises them.
- `crates/right-agent/src/memory/migrations.rs` is 1000 LoC and references `sql/v*.sql` files via `include_str!("sql/v1_schema.sql")` etc. The relative path stays correct as long as both files move together.
- `crates/right-agent/src/memory/error.rs` defines `MemoryError` with 9 variants. Two are db-only (`Sqlite`, `Migration`); the other seven are Hindsight/content/lookup-related and stay with memory.
- `crates/right-agent/src/memory/store.rs` is just three thin auth-token functions plus a `#[path = "store_tests.rs"] mod tests;` block.
- `crates/right-agent/src/memory/store_tests.rs` runs against `Connection::open_in_memory()` and drives migrations via `crate::memory::migrations::MIGRATIONS`. After the move it'll drive migrations via `right_db::MIGRATIONS`.
- `right-agent/Cargo.toml` already lists `rusqlite` and `rusqlite_migration` as deps. After Stage A, those entries can stay — `memory::retain_queue` still uses them.
- `right-agent` exposes `pub mod memory;` from `lib.rs:5`. After Stage A `memory::open_db` / `open_connection` re-exports stay temporarily for back-compat re-exports (until final cleanup at Stage F of the umbrella spec). However, this plan rewrites every callsite in the workspace, so the re-export is only a safety net during PR review — we remove it in the same PR's last task.
- The `[workspace]` `Cargo.toml` lives at repo root, currently lists three members. We add a fourth.
- `rusqlite` workspace dep version (`0.39`, with `bundled` feature) — reuse via `{ workspace = true }`.

**Verification commands** (run from repo root):
- Build: `devenv shell -- cargo build --workspace`
- Test: `devenv shell -- cargo test --workspace`
- Lint: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
- Single-package check: `devenv shell -- cargo check -p <name>`

---

## Task 1: Create the `right-db` crate skeleton

**Files:**
- Create: `crates/right-db/Cargo.toml`
- Create: `crates/right-db/src/lib.rs`

- [ ] **Step 1: Create the directory and `Cargo.toml`**

Create `crates/right-db/Cargo.toml`:

```toml
[package]
name = "right-db"
version.workspace = true
edition.workspace = true

[dependencies]
rusqlite = { workspace = true }
rusqlite_migration = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 2: Create a stub `lib.rs`**

Create `crates/right-db/src/lib.rs` with the contents:

```rust
//! Per-agent SQLite plumbing for `right`.
//!
//! Owns `data.db` open/migrate logic and the central migration registry.
//! Domain crates (`right-mcp`, `right-memory`, `right-codegen`, the slim
//! `right-agent`, `right-bot`) call `open_connection` here; new tables
//! are added by editing the central `migrations::MIGRATIONS` array.

pub mod error;
pub mod migrations;

pub use error::DbError;
pub use migrations::MIGRATIONS;

use std::path::Path;

/// Open the per-agent SQLite database, applying migrations if requested.
///
/// Idempotent. WAL journal mode + 5s busy_timeout. The connection is
/// returned for callers that need it; use [`open_db`] when you only
/// want to ensure the file exists.
pub fn open_connection(
    agent_path: &Path,
    migrate: bool,
) -> Result<rusqlite::Connection, DbError> {
    let db_path = agent_path.join("data.db");
    let mut conn = rusqlite::Connection::open(&db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    if migrate {
        migrations::MIGRATIONS.to_latest(&mut conn)?;
    }
    Ok(conn)
}

/// Open the per-agent SQLite database, dropping the connection.
/// Used when the caller only needs the file created and migrated.
pub fn open_db(agent_path: &Path, migrate: bool) -> Result<(), DbError> {
    open_connection(agent_path, migrate).map(drop)
}
```

- [ ] **Step 3: Create the placeholder `error.rs` and `migrations.rs`**

Create `crates/right-db/src/error.rs`:

```rust
/// Errors from per-agent SQLite operations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),
}
```

Create `crates/right-db/src/migrations.rs` with a placeholder body (it'll be replaced in Task 4):

```rust
use rusqlite_migration::Migrations;

/// Per-agent SQLite migration registry. Single source of truth for
/// every table the `right` platform writes to `data.db`.
pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| Migrations::new(Vec::new()));
```

- [ ] **Step 4: Verify the new crate compiles in isolation**

Run: `devenv shell -- cargo check -p right-db --manifest-path crates/right-db/Cargo.toml`

Expected: this fails because the workspace doesn't yet know about `right-db`. That's fine — Task 2 wires it in.

- [ ] **Step 5: Commit**

```bash
git add crates/right-db/
git commit -m "feat(right-db): scaffold new crate for SQLite plumbing"
```

---

## Task 2: Add `right-db` to the workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add `right-db` to workspace members**

In repo-root `Cargo.toml`, replace:

```toml
[workspace]
members = ["crates/right-agent", "crates/right", "crates/bot"]
resolver = "3"
```

with:

```toml
[workspace]
members = ["crates/right-agent", "crates/right-db", "crates/right", "crates/bot"]
resolver = "3"
```

- [ ] **Step 2: Verify the workspace builds the empty crate**

Run: `devenv shell -- cargo build -p right-db`
Expected: succeeds.

- [ ] **Step 3: Verify the workspace as a whole still builds**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds (we haven't broken existing code yet).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat(workspace): register right-db crate"
```

---

## Task 3: Move SQL files into `right-db`

**Files:**
- Move: `crates/right-agent/src/memory/sql/v*.sql` → `crates/right-db/src/sql/v*.sql`

- [ ] **Step 1: Move the SQL directory**

Run:

```bash
mkdir -p crates/right-db/src/sql
git mv crates/right-agent/src/memory/sql/*.sql crates/right-db/src/sql/
rmdir crates/right-agent/src/memory/sql
```

- [ ] **Step 2: Verify the file count matches**

Run: `ls crates/right-db/src/sql | wc -l`
Expected: `17` (v1 through v19, with v12, v18 missing — they're Rust-hook migrations, not SQL files).

Run: `ls crates/right-agent/src/memory/sql 2>/dev/null || echo gone`
Expected: `gone`.

- [ ] **Step 3: Confirm `right-agent` no longer compiles** (sanity check)

Run: `devenv shell -- cargo check -p right-agent`
Expected: fails — `crates/right-agent/src/memory/migrations.rs` still has `include_str!("sql/v1_schema.sql")` calls but the files are gone. We fix this in Task 4.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/src/memory/sql crates/right-db/src/sql
git commit -m "refactor(right-db): move SQL migration files from right-agent"
```

---

## Task 4: Move `migrations.rs` into `right-db`

**Files:**
- Move: `crates/right-agent/src/memory/migrations.rs` → `crates/right-db/src/migrations.rs`
- Delete: the placeholder `crates/right-db/src/migrations.rs` from Task 1

- [ ] **Step 1: Replace the placeholder with the real file**

Run:

```bash
git rm crates/right-db/src/migrations.rs
git mv crates/right-agent/src/memory/migrations.rs crates/right-db/src/migrations.rs
```

- [ ] **Step 2: Verify `include_str!` paths still resolve**

Open `crates/right-db/src/migrations.rs` and confirm the first 30 lines look like:

```rust
const V1_SCHEMA: &str = include_str!("sql/v1_schema.sql");
```

The `sql/` directory is now `crates/right-db/src/sql/`, sibling to `migrations.rs`, so the relative path is correct.

- [ ] **Step 3: Make `migrations` a public module of `right-db`**

In `crates/right-db/src/migrations.rs`, the existing module body uses `pub static MIGRATIONS`. Verify the top of the file already has `pub` on the relevant items. If not, change `static MIGRATIONS` to `pub static MIGRATIONS`. Open the file and check the line that defines `MIGRATIONS` — it should read:

```rust
pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
```

If it's `static MIGRATIONS` (no `pub`), prefix it with `pub`.

- [ ] **Step 4: Build `right-db`**

Run: `devenv shell -- cargo build -p right-db`
Expected: succeeds. The migration registry compiles inside the new crate.

- [ ] **Step 5: Commit**

```bash
git add crates/right-db/src/migrations.rs crates/right-agent/src/memory/migrations.rs
git commit -m "refactor(right-db): move migrations.rs from right-agent::memory"
```

---

## Task 5: Wire `right-agent` to depend on `right-db`

**Files:**
- Modify: `crates/right-agent/Cargo.toml`

- [ ] **Step 1: Add the path dep**

In `crates/right-agent/Cargo.toml`, in the `[dependencies]` section, add:

```toml
right-db = { path = "../right-db" }
```

Place it alphabetically — between `rand` (above) and `reqwest` (below) — to match the existing alphabetic ordering in that file.

- [ ] **Step 2: Verify cargo resolves the dependency**

Run: `devenv shell -- cargo check -p right-agent --no-default-features 2>&1 | head -30`
Expected: still fails — `right-agent::memory::mod.rs` now refers to a `migrations` submodule that no longer exists. Task 6 fixes this.

- [ ] **Step 3: Commit**

```bash
git add crates/right-agent/Cargo.toml Cargo.lock
git commit -m "deps(right-agent): add right-db path dep"
```

---

## Task 6: Reshape `right-agent::memory::mod.rs` to delegate to `right-db`

**Files:**
- Modify: `crates/right-agent/src/memory/mod.rs`

- [ ] **Step 1: Remove the moved declarations**

Open `crates/right-agent/src/memory/mod.rs`. Remove these two lines:

```rust
pub(crate) mod migrations;
```

(The migrations module is gone — moved to `right-db`.)

- [ ] **Step 2: Replace `open_db` and `open_connection` with re-exports**

Replace the entire body of `open_db` and `open_connection` (lines roughly 35-71 — see the current file for exact extents) with re-exports from `right-db`. After your edit the file should contain, near the top:

```rust
pub use right_db::{open_connection, open_db};
```

Delete:
- the local `pub fn open_db(...) -> Result<(), MemoryError>` definition
- the local `pub fn open_connection(...) -> Result<rusqlite::Connection, MemoryError>` definition
- the inline `mod tests` block (the smoke tests for `open_db` / `open_connection` move to `right-db` in Task 8)

Keep the doc comments at the top of the file plus `alert_types`, `pub mod` declarations for `circuit`, `classify`, `error`, `guard`, `hindsight`, `prefetch`, `resilient`, `retain_queue`, `status`, `store` and the `pub use` re-exports for `ErrorKind`, `MemoryError`, `ResilientError`, `ResilientHindsight`, `MemoryStatus`.

- [ ] **Step 3: Update `MemoryError` to delegate sqlite and migration variants to `DbError`**

Open `crates/right-agent/src/memory/error.rs` and replace its contents with:

```rust
//! Errors raised by the Hindsight resilience layer + retain queue.
//!
//! Pure SQLite plumbing errors come from `right_db::DbError`; we
//! wrap them via `#[from]` so existing call sites that match on
//! `MemoryError::Sqlite(_)` or `MemoryError::Migration(_)` keep
//! compiling.

pub use right_db::DbError;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("db error: {0}")]
    Db(#[from] DbError),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),

    #[error("content rejected: possible prompt injection detected")]
    InjectionDetected,

    #[error("memory not found: id {0}")]
    NotFound(i64),

    #[error("hindsight API error (HTTP {status}): {body}")]
    Hindsight { status: u16, body: String },

    #[error("hindsight request timed out")]
    HindsightTimeout,

    #[error("hindsight connection error: {0}")]
    HindsightConnect(String),

    #[error("hindsight response parse error: {0}")]
    HindsightParse(String),

    #[error("hindsight request error: {0}")]
    HindsightOther(String),
}

impl MemoryError {
    /// Convert a reqwest::Error from send/recv into the appropriate classified variant.
    pub fn from_reqwest(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            MemoryError::HindsightTimeout
        } else if e.is_connect() || e.is_request() {
            MemoryError::HindsightConnect(format!("{e:#}"))
        } else if e.is_decode() || e.is_body() {
            MemoryError::HindsightParse(format!("{e:#}"))
        } else {
            MemoryError::HindsightOther(format!("{e:#}"))
        }
    }

    /// Convert a JSON deserialization error.
    pub fn from_parse(e: impl std::fmt::Display) -> Self {
        MemoryError::HindsightParse(format!("{e:#}"))
    }
}
```

Why both `Db(DbError)` AND the original `Sqlite`/`Migration` variants? The latter two stay so existing matchers (e.g. `MemoryError::Sqlite(rusqlite::Error::QueryReturnedNoRows) => …`) keep compiling. `From<rusqlite::Error> for MemoryError` still exists via the `#[from]` on `Sqlite`. Future cleanup can collapse this in Stage B/C.

- [ ] **Step 4: Verify the crate type-checks**

Run: `devenv shell -- cargo check -p right-agent`
Expected: succeeds. (Other consumers may still fail because they import `crate::memory::migrations::MIGRATIONS`; we fix those in Tasks 9-11.)

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/memory/mod.rs crates/right-agent/src/memory/error.rs
git commit -m "refactor(right-agent): delegate db plumbing to right-db"
```

---

## Task 7: Move `*_auth_token` helpers from `memory::store` to `mcp::credentials`

**Files:**
- Modify: `crates/right-agent/src/mcp/credentials.rs`
- Delete: `crates/right-agent/src/memory/store.rs`
- Delete: `crates/right-agent/src/memory/store_tests.rs`
- Modify: `crates/right-agent/src/memory/mod.rs` (remove `pub mod store;`)

- [ ] **Step 1: Append the three functions to `mcp/credentials.rs`**

At the bottom of `crates/right-agent/src/mcp/credentials.rs` (above any existing `#[cfg(test)] mod tests` block, if there is one — or at the very bottom if not), add:

```rust
/// Save an auth token, replacing any existing one.
pub fn save_auth_token(conn: &rusqlite::Connection, token: &str) -> Result<(), rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM auth_tokens", [])?;
    tx.execute(
        "INSERT INTO auth_tokens (token) VALUES (?1)",
        rusqlite::params![token],
    )?;
    tx.commit()?;
    Ok(())
}

/// Get the stored auth token, if any.
pub fn get_auth_token(conn: &rusqlite::Connection) -> Result<Option<String>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT token FROM auth_tokens LIMIT 1")?;
    let mut rows = stmt.query([])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// Delete the stored auth token.
pub fn delete_auth_token(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    conn.execute("DELETE FROM auth_tokens", [])?;
    Ok(())
}
```

- [ ] **Step 2: Move the tests to a sibling file**

Move the entire body of `crates/right-agent/src/memory/store_tests.rs` into a new file `crates/right-agent/src/mcp/credentials_auth_token_tests.rs` (we keep the existing `mcp/credentials_tests.rs` if it exists separate from these tests — append a uniquely-named module for the auth-token tests). Adjust the imports at the top of the moved test file:

Replace the old import line:
```rust
use crate::memory::store::{delete_auth_token, get_auth_token, save_auth_token};
```

with:
```rust
use crate::mcp::credentials::{delete_auth_token, get_auth_token, save_auth_token};
```

Replace every reference to `crate::memory::migrations::MIGRATIONS` (there are several) with `right_db::MIGRATIONS`.

Then add an attached-tests declaration at the bottom of `crates/right-agent/src/mcp/credentials.rs`:

```rust
#[cfg(test)]
#[path = "credentials_auth_token_tests.rs"]
mod auth_token_tests;
```

If `mcp/credentials.rs` already has a `#[cfg(test)] mod tests;` block for its own tests, leave that block alone — we're appending a new sibling test module under a distinct name (`auth_token_tests`).

- [ ] **Step 3: Delete the old store files**

```bash
git rm crates/right-agent/src/memory/store.rs crates/right-agent/src/memory/store_tests.rs
```

- [ ] **Step 4: Remove `pub mod store;` from `memory/mod.rs`**

Open `crates/right-agent/src/memory/mod.rs` and delete the line `pub mod store;`.

- [ ] **Step 5: Verify `right-agent` builds and tests pass**

Run: `devenv shell -- cargo test -p right-agent --lib mcp::credentials`
Expected: the moved auth-token tests run and pass.

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/memory/mod.rs crates/right-agent/src/memory/store.rs crates/right-agent/src/memory/store_tests.rs crates/right-agent/src/mcp/credentials.rs crates/right-agent/src/mcp/credentials_auth_token_tests.rs
git commit -m "refactor(right-agent): move auth_token helpers to mcp::credentials"
```

---

## Task 8: Add smoke tests for `right-db`

**Files:**
- Create: `crates/right-db/tests/smoke.rs`

- [ ] **Step 1: Write the smoke test file**

Create `crates/right-db/tests/smoke.rs`:

```rust
use right_db::{MIGRATIONS, open_connection, open_db};
use tempfile::tempdir;

#[test]
fn open_db_creates_file() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();
    assert!(
        dir.path().join("data.db").exists(),
        "data.db should exist after open_db",
    );
}

#[test]
fn open_connection_applies_migrations() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    // After migrations, telegram_sessions table should exist.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='telegram_sessions'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "telegram_sessions table should exist");
}

#[test]
fn migrations_idempotent() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();
    // Re-opening with migrate=true must not error.
    open_db(dir.path(), true).unwrap();
}

#[test]
fn migrations_static_runs_in_memory() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
}
```

- [ ] **Step 2: Run the smoke tests**

Run: `devenv shell -- cargo test -p right-db --tests`
Expected: all four tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/right-db/tests/smoke.rs
git commit -m "test(right-db): add open + migration smoke tests"
```

---

## Task 9: Update `right-agent` internal callsites to use `right_db::open_connection`

**Files (all under `crates/right-agent/src/`):**
- Modify: `cron_spec.rs`
- Modify: `cron_spec_tests.rs`
- Modify: `doctor.rs`
- Modify: `rebootstrap.rs`
- Modify: `agent/destroy.rs`
- Modify: `mcp/proxy.rs`
- Modify: `mcp/reconnect.rs`
- Modify: `mcp/refresh.rs`
- Modify: `mcp/credentials.rs`
- Modify: `usage/aggregate.rs`
- Modify: `usage/error.rs` (if present)
- Modify: `usage/insert.rs`
- Modify: `memory/retain_queue.rs`

- [ ] **Step 1: List all internal callsites**

Run:

```bash
devenv shell -- rg -n 'crate::memory::open_connection|crate::memory::open_db|crate::memory::migrations::MIGRATIONS' crates/right-agent/src
```

Expected: a list of files matching those above. If the list does not match, work the actual list — the targets above are the inventory taken at spec-write time; new code added since may have its own callsites.

- [ ] **Step 2: Replace `crate::memory::open_connection` → `right_db::open_connection`**

In every file listed in Step 1, replace each occurrence of `crate::memory::open_connection` with `right_db::open_connection`. Use:

```bash
devenv shell -- rg -l 'crate::memory::open_connection' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::memory::open_connection|right_db::open_connection|g'
devenv shell -- find crates/right-agent/src -name '*.bak' -delete
```

(macOS `sed -i.bak` is the portable form. Verify no `.bak` files remain.)

- [ ] **Step 3: Replace `crate::memory::open_db` → `right_db::open_db`**

```bash
devenv shell -- rg -l 'crate::memory::open_db' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::memory::open_db|right_db::open_db|g'
devenv shell -- find crates/right-agent/src -name '*.bak' -delete
```

- [ ] **Step 4: Replace `crate::memory::migrations::MIGRATIONS` → `right_db::MIGRATIONS`**

```bash
devenv shell -- rg -l 'crate::memory::migrations::MIGRATIONS' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::memory::migrations::MIGRATIONS|right_db::MIGRATIONS|g'
devenv shell -- find crates/right-agent/src -name '*.bak' -delete
```

- [ ] **Step 5: Verify the crate compiles**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds. If it fails with "could not find `migrations` in `memory`", grep again — there's a leftover. Fix it.

- [ ] **Step 6: Run the right-agent test suite**

Run: `devenv shell -- cargo test -p right-agent --lib`
Expected: passes. Integration tests come in Task 10.

- [ ] **Step 7: Commit**

```bash
git add crates/right-agent/src
git commit -m "refactor(right-agent): switch internal callsites to right-db"
```

---

## Task 10: Update `right-agent` integration tests to use `right_db`

**Files (all under `crates/right-agent/tests/`):**
- Modify: `memory_failure_scenarios.rs`
- Modify: `rebootstrap_sandbox.rs`
- Modify: `common/mod.rs`

- [ ] **Step 1: Add `right-db` to right-agent dev-deps**

Open `crates/right-agent/Cargo.toml`, find the `[dev-dependencies]` section, and add (alphabetically):

```toml
right-db = { path = "../right-db" }
```

- [ ] **Step 2: Replace `right_agent::memory::open_connection` → `right_db::open_connection` in tests**

```bash
devenv shell -- rg -l 'right_agent::memory::open_connection' crates/right-agent/tests \
  | xargs sed -i.bak 's|right_agent::memory::open_connection|right_db::open_connection|g'
devenv shell -- find crates/right-agent/tests -name '*.bak' -delete
```

Repeat for `right_agent::memory::open_db`:

```bash
devenv shell -- rg -l 'right_agent::memory::open_db' crates/right-agent/tests \
  | xargs sed -i.bak 's|right_agent::memory::open_db|right_db::open_db|g'
devenv shell -- find crates/right-agent/tests -name '*.bak' -delete
```

- [ ] **Step 3: Run integration tests**

Run: `devenv shell -- cargo test -p right-agent --tests`
Expected: passes.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/Cargo.toml crates/right-agent/tests Cargo.lock
git commit -m "test(right-agent): switch integration tests to right-db"
```

---

## Task 11: Update `right-bot` callsites

**Files:**
- Modify: `crates/bot/Cargo.toml`
- Modify: `crates/bot/src/login.rs`
- Modify: any other file under `crates/bot/src/` that touches db plumbing or auth tokens.

- [ ] **Step 1: Add `right-db` to bot deps**

In `crates/bot/Cargo.toml`, in `[dependencies]`, add (alphabetically):

```toml
right-db = { path = "../right-db" }
```

- [ ] **Step 2: List bot callsites for db plumbing**

Run:

```bash
devenv shell -- rg -n 'right_agent::memory::open_connection|right_agent::memory::open_db|right_agent::memory::migrations::MIGRATIONS' crates/bot/src
```

Expected: the file list confirms what's in scope (e.g. `bot/login.rs` and possibly `bot/cron.rs`, `bot/cron_delivery.rs`, `bot/sync.rs`, `bot/lib.rs`). Treat the actual output as the source of truth.

- [ ] **Step 3: Replace the three patterns**

```bash
devenv shell -- rg -l 'right_agent::memory::open_connection' crates/bot/src \
  | xargs sed -i.bak 's|right_agent::memory::open_connection|right_db::open_connection|g'
devenv shell -- rg -l 'right_agent::memory::open_db' crates/bot/src \
  | xargs sed -i.bak 's|right_agent::memory::open_db|right_db::open_db|g'
devenv shell -- rg -l 'right_agent::memory::migrations::MIGRATIONS' crates/bot/src \
  | xargs sed -i.bak 's|right_agent::memory::migrations::MIGRATIONS|right_db::MIGRATIONS|g'
devenv shell -- find crates/bot/src -name '*.bak' -delete
```

- [ ] **Step 4: Update auth-token call paths**

```bash
devenv shell -- rg -l 'right_agent::memory::store::save_auth_token' crates/bot/src \
  | xargs sed -i.bak 's|right_agent::memory::store::save_auth_token|right_agent::mcp::credentials::save_auth_token|g'
devenv shell -- rg -l 'right_agent::memory::store::get_auth_token' crates/bot/src \
  | xargs sed -i.bak 's|right_agent::memory::store::get_auth_token|right_agent::mcp::credentials::get_auth_token|g'
devenv shell -- rg -l 'right_agent::memory::store::delete_auth_token' crates/bot/src \
  | xargs sed -i.bak 's|right_agent::memory::store::delete_auth_token|right_agent::mcp::credentials::delete_auth_token|g'
devenv shell -- find crates/bot/src -name '*.bak' -delete
```

- [ ] **Step 5: Build and test the bot crate**

Run: `devenv shell -- cargo build -p right-bot`
Expected: succeeds.

Run: `devenv shell -- cargo test -p right-bot --lib`
Expected: passes.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/Cargo.toml crates/bot/src Cargo.lock
git commit -m "refactor(bot): switch to right-db and mcp::credentials auth helpers"
```

---

## Task 12: Update `right` CLI callsites

**Files:**
- Modify: `crates/right/Cargo.toml`
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/aggregator.rs`
- Modify: `crates/right/src/internal_api.rs`
- Modify: `crates/right/src/memory_server.rs`
- Modify: `crates/right/src/memory_server_mcp_tests.rs`
- Modify: `crates/right/src/right_backend.rs`
- Modify: `crates/right/src/right_backend_tests.rs`

- [ ] **Step 1: Add `right-db` to CLI deps**

In `crates/right/Cargo.toml`, in `[dependencies]`, add (alphabetically):

```toml
right-db = { path = "../right-db" }
```

- [ ] **Step 2: List CLI callsites**

Run:

```bash
devenv shell -- rg -n 'right_agent::memory::open_connection|right_agent::memory::open_db|right_agent::memory::migrations::MIGRATIONS' crates/right/src
```

Expected: ~20 lines across the files listed above.

- [ ] **Step 3: Replace the three patterns**

```bash
for pat in 'right_agent::memory::open_connection|right_db::open_connection' \
           'right_agent::memory::open_db|right_db::open_db' \
           'right_agent::memory::migrations::MIGRATIONS|right_db::MIGRATIONS'; do
  IFS='|' read -r src dst <<< "$pat"
  devenv shell -- rg -l "$src" crates/right/src \
    | xargs sed -i.bak "s|${src//::/__SCOPE__}|${dst//::/__SCOPE__}|g"
  # restore the :: separators (sed disliked the literal ::)
  devenv shell -- rg -l '__SCOPE__' crates/right/src \
    | xargs sed -i.bak "s|__SCOPE__|::|g"
done
devenv shell -- find crates/right/src -name '*.bak' -delete
```

(If the shell substitution above is awkward, run three separate `sed` invocations — the goal is a literal text replace of each path. The `::` vs `_` confusion is purely a shell-quoting concern; manual edits are equally valid.)

- [ ] **Step 4: Update CLI auth-token call paths (if any)**

Run:

```bash
devenv shell -- rg -n 'right_agent::memory::store::' crates/right/src
```

If this returns hits, repeat the auth-token rewrite from Task 11 Step 4 against `crates/right/src` instead of `crates/bot/src`. If it returns nothing, skip ahead.

- [ ] **Step 5: Build and test the CLI crate**

Run: `devenv shell -- cargo build -p right`
Expected: succeeds.

Run: `devenv shell -- cargo test -p right --lib --bins`
Expected: passes (the integration tests run later in Task 13).

- [ ] **Step 6: Commit**

```bash
git add crates/right/Cargo.toml crates/right/src Cargo.lock
git commit -m "refactor(right): switch CLI to right-db"
```

---

## Task 13: Whole-workspace build, test, lint pass

**Files:** none (verification only)

- [ ] **Step 1: Whole-workspace build (debug)**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds with zero warnings.

- [ ] **Step 2: Whole-workspace build (release)**

Run: `devenv shell -- cargo build --workspace --release`
Expected: succeeds.

- [ ] **Step 3: Whole-workspace test**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass, including `TestSandbox`-using integration tests. The dev machine has OpenShell running per CLAUDE.md.

- [ ] **Step 4: Whole-workspace clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 5: If any of the above fails, fix in-place**

Common failure modes:
- A dangling `crate::memory::migrations` import not caught by the sed runs — locate via `rg 'crate::memory::migrations' crates/`.
- An import of `MemoryError::Sqlite` matching against rusqlite errors — should still work because the variant is preserved.
- A test that expected the old test module path — adjust the `--lib mcp::credentials` selector to the actual path printed in the failure.

Commit any fixes.

```bash
git add <fixed files>
git commit -m "fix(stage-a): resolve dangling references after right-db extraction"
```

---

## Task 14: Run review-rust-code agent and resolve TODOs

**Files:** none (review only)

- [ ] **Step 1: Dispatch the review agent**

Use the `rust-dev:review-rust-code` agent with this prompt:

> Review the changes on the current branch (commits since `bd4b2a07`, i.e. since the spec was written). Focus on: (1) correctness of the `right-db` extraction — anything that should still live there but was missed, anything that shouldn't have moved; (2) the `MemoryError` wrapper — is it sound to keep both `Db(DbError)` and the original `Sqlite`/`Migration` variants, or should we collapse to one path; (3) any place where error chains might have been broken by the move (look for `e.to_string()` vs `format!("{:#}", e)` per CLAUDE.rust.md). Also flag anything else that looks off. Don't fix; report.

- [ ] **Step 2: Triage the findings**

For each finding the agent surfaces:
- If it's a clear bug, add it to a TODO file `docs/superpowers/plans/2026-05-06-stage-a-followups.md` and commit. Then fix one TODO at a time, each with its own commit.
- If it's a style nitpick that doesn't affect correctness, add it to the same TODO file but defer.
- If it's a misunderstanding of the spec, ignore.

- [ ] **Step 3: Confirm the test suite still passes after fixes**

Run: `devenv shell -- cargo test --workspace`
Expected: passes.

- [ ] **Step 4: Commit any fixes**

```bash
git add <fixed files>
git commit -m "fix(stage-a): address review-rust-code findings"
```

---

## Task 15: Update `ARCHITECTURE.md`

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Add `right-db` to the Workspace table**

Open `ARCHITECTURE.md`. In the `## Workspace` section there's a table:

```markdown
| Crate | Path | Role |
|-------|------|------|
| **right-agent** | `crates/right-agent/` | Core library — agent discovery, codegen, config, memory, runtime, MCP, OpenShell |
| **right** | `crates/right/` | CLI binary (`right`) + MCP Aggregator (HTTP) |
| **right-bot** | `crates/bot/` | Telegram bot runtime (teloxide) + cron engine + login flow |
```

Replace it with:

```markdown
| Crate | Path | Role |
|-------|------|------|
| **right-db** | `crates/right-db/` | Per-agent SQLite plumbing — `open_connection`, central migration registry, `sql/v*.sql` |
| **right-agent** | `crates/right-agent/` | Core library — agent discovery, codegen, config, memory (Hindsight + retain queue), runtime, MCP, OpenShell |
| **right** | `crates/right/` | CLI binary (`right`) + MCP Aggregator (HTTP) |
| **right-bot** | `crates/bot/` | Telegram bot runtime (teloxide) + cron engine + login flow |
```

- [ ] **Step 2: Update `## SQLite Rules → ## Migration Ownership`**

Find the section starting `### Migration Ownership`. Replace its first paragraph with:

```markdown
Both the MCP aggregator (`right-mcp-server`) and bot processes run schema migrations on per-agent `data.db` via `right_db::open_connection(path, migrate: true)`. Migrations are idempotent — concurrent callers are safe (WAL mode + busy_timeout). CLI commands and other processes open with `migrate: false`. Bot processes still declare `depends_on: right-mcp-server` for MCP readiness, but no longer depend on it for schema migrations. The migration registry (`right_db::migrations::MIGRATIONS`) is the sole place to add new tables.
```

- [ ] **Step 3: Verify the doc renders sensibly**

Skim `ARCHITECTURE.md` end-to-end. Look for any other references to `right_agent::memory::open_connection` and replace them with `right_db::open_connection`. (The Memory Schema section probably also needs a small note acknowledging `right-db` ownership.)

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs(arch): add right-db to workspace map"
```

---

## Task 16: Final verification + summary commit

**Files:** none (verification + a summary commit on top)

- [ ] **Step 1: Re-run the full check suite**

```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo build --workspace --release
devenv shell -- cargo test --workspace
devenv shell -- cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all four pass.

- [ ] **Step 2: Build-time benchmark (optional but recommended)**

Run: `devenv shell -- cargo clean && devenv shell -- cargo build --workspace --timings`
Note the wall-time reported. Save the resulting `target/cargo-timings/cargo-timing-*.html` somewhere outside the repo (e.g. `~/Desktop/`) — this is the baseline against which Stage B's improvement will be measured.

- [ ] **Step 3: Inventory check**

Run: `devenv shell -- rg -n 'right_agent::memory::open_connection|right_agent::memory::open_db|right_agent::memory::migrations::MIGRATIONS|right_agent::memory::store::' crates`
Expected: zero results. (If anything remains, it's a missed callsite — fix in-place and commit.)

- [ ] **Step 4: Optional summary commit**

If you want a checkpoint marker for `git bisect` later, add an empty commit:

```bash
git commit --allow-empty -m "chore(stage-a): right-db extraction complete"
```

- [ ] **Step 5: Open a PR (if working on a branch)**

If this work is on a feature branch, open a PR with title `Stage A: extract right-db crate` and body referencing the spec at `docs/superpowers/specs/2026-05-06-crate-split-design.md` and this plan.
