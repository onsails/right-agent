# DB Bootstrap Lock Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move legacy FTS5 cleanup out of runtime DB opens and serialize startup schema bootstrap with a per-agent advisory lock.

**Architecture:** `right_db::open_connection(agent_dir, true)` becomes the only schema bootstrap path. It acquires a per-agent advisory lock, runs the pre-Turso legacy FTS5 cleanup, opens Turso, applies pragmas, and runs migrations; `open_connection(agent_dir, false)` only opens Turso and applies pragmas.

**Tech Stack:** Rust 2024, `right-db`, Turso local driver, bundled `rusqlite` for legacy cleanup, `fs4` advisory file locks, Tokio tests.

---

## Scope Check

This plan covers one subsystem: per-agent database bootstrap in `right-db`. It does not introduce a single runtime DB writer task and does not change the public `open_connection(agent_dir, migrate)` shape.

## File Structure

- Modify `crates/right-db/Cargo.toml`: add the existing workspace `fs4` dependency to `right-db`.
- Create `crates/right-db/src/bootstrap_lock.rs`: own the per-agent migration lock file, bounded polling, logging, and unlock-on-drop guard.
- Modify `crates/right-db/src/error.rs`: add a `DbError::MigrationLock` variant for lock file open/try-lock/timeout failures.
- Modify `crates/right-db/src/lib.rs`: route legacy cleanup and migrations only through `migrate = true`; keep runtime opens free of legacy probing.
- Modify `crates/right-db/src/migrations.rs`: replace the old `migrate = false` scrubber test with the opposite invariant and add concurrent legacy migration coverage.
- Modify `ARCHITECTURE.md`: update migration ownership rules to match the new bootstrap lock design.

---

### Task 1: Add Failing Regression Tests

**Files:**
- Modify: `crates/right-db/Cargo.toml`
- Modify: `crates/right-db/src/lib.rs`
- Modify: `crates/right-db/src/migrations.rs`

- [ ] **Step 1: Add `fs4` to `right-db` dependencies**

In `crates/right-db/Cargo.toml`, update `[dependencies]`:

```toml
[dependencies]
fs4 = { workspace = true }
tempfile = { workspace = true, optional = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
turso = { workspace = true }
rusqlite = { workspace = true }
```

- [ ] **Step 2: Add a red test proving `migrate = true` waits for the bootstrap lock**

In `crates/right-db/src/lib.rs`, inside the existing `#[cfg(test)] mod tests`, add this test after `open_connection_retries_transient_legacy_probe_lock`:

```rust
#[tokio::test]
async fn migration_open_waits_for_existing_bootstrap_lock() {
    use fs4::FileExt;

    let dir = tempfile::tempdir().unwrap();
    let lock_path = dir.path().join(".right-db-migrate.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    lock_file.lock().unwrap();

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(750),
        open_connection(dir.path(), true),
    )
    .await;
    assert!(
        result.is_err(),
        "migrate=true open must wait for the bootstrap lock"
    );

    FileExt::unlock(&lock_file).unwrap();
    open_connection(dir.path(), true).await.unwrap();
}
```

- [ ] **Step 3: Replace the old runtime-scrub test with the new invariant**

In `crates/right-db/src/migrations.rs`, replace the whole existing test function named `open_connection_without_migration_scrubs_legacy_fts5` with:

```rust
#[tokio::test]
async fn open_connection_without_migration_does_not_scrub_legacy_fts5() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("data.db");
    create_legacy_v33_fts5_database(&db_path);

    let result = crate::open_connection(dir.path(), false).await;
    drop(result);

    let sqlite = rusqlite::Connection::open(&db_path).unwrap();
    let version: i64 = sqlite
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 33, "runtime open must not run migrations");

    for table_name in ["memories_fts", "conversation_messages_fts"] {
        let table_count: i64 = sqlite
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table_name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            table_count, 1,
            "{table_name} must remain because migrate=false must not scrub"
        );
    }

    for trigger_name in [
        "memories_ai",
        "memories_ad",
        "memories_au",
        "conversation_messages_ai",
        "conversation_messages_ad",
        "conversation_messages_au",
    ] {
        let trigger_count: i64 = sqlite
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name=?1",
                [trigger_name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            trigger_count, 1,
            "{trigger_name} must remain because migrate=false must not scrub"
        );
    }
}
```

- [ ] **Step 4: Add a concurrent startup migration scrubber-overlap regression test**

Add minimal `#[cfg(test)]` support in `crates/right-db/src/lib.rs` at the
legacy FTS5 scrubber boundary. The test-only probe must be scoped to the exact
fixture `data.db` path, must yield while the legacy scrubber window is active,
must record whether a second caller enters that same window, and must not hold a
mutex across `.await`.

In `crates/right-db/src/migrations.rs`, add this regression after
`v34_migrates_real_legacy_fts5_virtual_tables`. A bare `tokio::join!` without
the scrubber probe is not sufficient: the synchronous pre-open scrubber can run
through the first caller before the second future reaches the race.

```rust
#[tokio::test]
async fn concurrent_migration_opens_serialize_legacy_fts5_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("data.db");
    create_legacy_v33_fts5_database(&db_path);

    let probe = crate::arm_legacy_fts5_scrubber_overlap_probe(&db_path);

    let (first, second) = tokio::join!(
        crate::open_connection(dir.path(), true),
        crate::open_connection(dir.path(), true),
    );
    assert!(
        !probe.overlap_observed(),
        "concurrent migrate=true opens must not overlap legacy FTS5 cleanup"
    );

    let conn1 = first.expect("first migrator must succeed");
    let conn2 = second.expect("second migrator must succeed");

    for conn in [&conn1, &conn2] {
        let version: i64 = conn
            .query_row("PRAGMA user_version", (), |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(version, i64::from(LATEST_SCHEMA_VERSION));

        for table_name in ["memories_fts", "conversation_messages_fts"] {
            let table_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table_name],
                    |row| row.get(0),
                )
                .await
                .unwrap();
            assert_eq!(table_count, 0, "{table_name} must be removed");
        }
    }
}
```

- [ ] **Step 5: Run the new red tests**

Run:

```bash
devenv shell -- cargo test -p right-db migration_open_waits_for_existing_bootstrap_lock -- --nocapture
```

Expected: FAIL. The failure should say `migrate=true open must wait for the bootstrap lock`, because the current code ignores `.right-db-migrate.lock`.

Run:

```bash
devenv shell -- cargo test -p right-db open_connection_without_migration_does_not_scrub_legacy_fts5 -- --nocapture
```

Expected: FAIL. The failure should show a legacy FTS table or trigger count of `0` instead of `1`, because the current `migrate=false` path runs the scrubber.

---

### Task 2: Implement Locked Startup Bootstrap

**Files:**
- Create: `crates/right-db/src/bootstrap_lock.rs`
- Modify: `crates/right-db/src/error.rs`
- Modify: `crates/right-db/src/lib.rs`

- [ ] **Step 1: Add the lock error variant**

In `crates/right-db/src/error.rs`, add this variant after `LegacySqlite`:

```rust
#[error("database migration lock {path}: {source}")]
MigrationLock {
    path: PathBuf,
    #[source]
    source: std::io::Error,
},
```

Do not add `MigrationLock` to `DbError::transient_kind`; lock timeout and lock file I/O failures are startup bootstrap failures, not generic runtime retry signals.

- [ ] **Step 2: Create the bootstrap lock module**

Create `crates/right-db/src/bootstrap_lock.rs`:

```rust
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};

use crate::DbError;

const LOCK_FILE_NAME: &str = ".right-db-migrate.lock";
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const LOCK_WAIT_LOG_AFTER: Duration = Duration::from_secs(1);

pub(crate) struct BootstrapLockGuard {
    path: PathBuf,
    file: File,
}

pub(crate) async fn acquire(agent_path: &Path) -> Result<BootstrapLockGuard, DbError> {
    let lock_path = agent_path.join(LOCK_FILE_NAME);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| DbError::MigrationLock {
            path: lock_path.clone(),
            source,
        })?;

    let started = Instant::now();
    let mut logged_wait = false;
    loop {
        match file.try_lock() {
            Ok(()) => {
                return Ok(BootstrapLockGuard {
                    path: lock_path,
                    file,
                });
            }
            Err(TryLockError::WouldBlock) if started.elapsed() < LOCK_WAIT_TIMEOUT => {
                if !logged_wait && started.elapsed() >= LOCK_WAIT_LOG_AFTER {
                    logged_wait = true;
                    tracing::warn!(
                        path = %lock_path.display(),
                        pid = std::process::id(),
                        "waiting for database migration lock"
                    );
                }
                tokio::time::sleep(LOCK_POLL_INTERVAL).await;
            }
            Err(TryLockError::WouldBlock) => {
                return Err(DbError::MigrationLock {
                    path: lock_path,
                    source: std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "timed out after {}ms waiting for migration lock",
                            LOCK_WAIT_TIMEOUT.as_millis()
                        ),
                    ),
                });
            }
            Err(TryLockError::Error(source)) => {
                return Err(DbError::MigrationLock {
                    path: lock_path,
                    source,
                });
            }
        }
    }
}

impl Drop for BootstrapLockGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(
                path = %self.path.display(),
                "database migration lock unlock failed: {error:#}"
            );
        }
    }
}
```

- [ ] **Step 3: Register the new module**

In `crates/right-db/src/lib.rs`, add the private module near the top:

```rust
mod bootstrap_lock;
pub mod connection;
pub mod conversation;
pub mod error;
pub mod migrations;
```

- [ ] **Step 4: Route only `migrate=true` through legacy cleanup and migrations**

In `crates/right-db/src/lib.rs`, replace `open_connection_once` with:

```rust
async fn open_connection_once(agent_path: &Path, migrate: bool) -> Result<Connection, DbError> {
    let db_path = agent_path.join("data.db");

    if migrate {
        let _bootstrap_lock = bootstrap_lock::acquire(agent_path).await?;
        prepare_legacy_fts5_schema_for_turso(&db_path).await?;
        let conn = Connection::open_local(db_path, true).await?;
        conn.apply_connection_pragmas().await?;
        migrations::MIGRATIONS.to_latest(&conn).await?;
        return Ok(conn);
    }

    let conn = Connection::open_local(db_path, true).await?;
    conn.apply_connection_pragmas().await?;
    Ok(conn)
}
```

Keep `prepare_legacy_fts5_schema_for_turso` and its retry loop. It now belongs to the locked startup path only.

- [ ] **Step 5: Update the existing transient legacy probe test**

In `crates/right-db/src/lib.rs`, update `open_connection_retries_transient_legacy_probe_lock` so the second open uses `migrate = true`:

```rust
let result = open_connection(dir.path(), true).await;
```

Also update the expectation message:

```rust
result.expect("migrate=true open_connection should recover from transient legacy probe lock");
```

- [ ] **Step 6: Run targeted `right-db` tests**

Run:

```bash
devenv shell -- cargo test -p right-db migration_open_waits_for_existing_bootstrap_lock -- --nocapture
```

Expected: PASS.

Run:

```bash
devenv shell -- cargo test -p right-db open_connection_without_migration_does_not_scrub_legacy_fts5 -- --nocapture
```

Expected: PASS.

Run:

```bash
devenv shell -- cargo test -p right-db concurrent_migration_opens_serialize_legacy_fts5_cleanup -- --nocapture
```

Expected: PASS.

Run:

```bash
devenv shell -- cargo test -p right-db open_connection_retries_transient_legacy_probe_lock -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit the DB implementation**

Run:

```bash
git add crates/right-db/Cargo.toml crates/right-db/src/bootstrap_lock.rs crates/right-db/src/error.rs crates/right-db/src/lib.rs crates/right-db/src/migrations.rs
git commit -m "fix(db): serialize schema bootstrap"
```

---

### Task 3: Update Architecture Documentation

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Replace the outdated migration ownership paragraph**

In `ARCHITECTURE.md`, replace the first paragraph under `### Migration Ownership` with:

```markdown
Both the MCP aggregator (`right-mcp-server`) and bot processes run schema
bootstrap on per-agent `data.db` via `right_db::open_connection(path, migrate:
true)`. This is the only path that may run legacy schema cleanup or database
migrations. `right-db` serializes that bootstrap with a per-agent advisory lock
file so concurrent startup of MCP and bot processes is safe without relying on
process-compose ordering. Under the lock, `right-db` may use bundled
`rusqlite` to drop legacy SQLite FTS5 virtual tables and sync triggers before
opening the database through Turso, because Turso cannot resolve every old FTS5
schema. Runtime opens with `migrate: false` do not run the scrubber, do not
inspect legacy FTS tables, and do not apply migrations. Read-only helpers do
not run the scrubber or mutate files. The migration registry
(`right_db::migrations::MIGRATIONS`) is the sole place to add new tables.
```

- [ ] **Step 2: Check for stale claims**

Run:

```bash
rg -n "migrate: false backup|scrubber runs before any writable|including `migrate: false`|pre-Turso legacy FTS5 scrubber described below" ARCHITECTURE.md
```

Expected: no stale claim says runtime `migrate=false` opens run the scrubber.

- [ ] **Step 3: Commit the documentation update**

Run:

```bash
git add ARCHITECTURE.md
git commit -m "docs(db): document bootstrap migration lock"
```

---

### Task 4: Final Verification

**Files:**
- No source edits unless verification exposes a failure.

- [ ] **Step 1: Run the full workspace test suite**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: all non-ignored tests pass.

- [ ] **Step 2: Run the full workspace debug build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: build finishes successfully.

- [ ] **Step 3: Inspect final git state**

Run:

```bash
git status --short --branch
git log --oneline --decorate --max-count=5
```

Expected: branch contains the docs spec commit plus the implementation/doc commits from this plan, with no unstaged source changes.
