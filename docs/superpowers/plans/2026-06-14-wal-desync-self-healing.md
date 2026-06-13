# WAL desync self-healing + per-op aggregator connections — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the platform auto-recover from the Turso multiprocess-WAL desync that wedged `agent-a`, and stop the aggregator from holding a long-lived `data.db` handle.

**Architecture:** Two composing changes. (1) `right_db::open_connection` detects the "short read on WAL frame" desync and self-heals by resetting the `-tshm`/`-shm` sidecars under the existing per-agent bootstrap lock, then retrying once. (2) The aggregator (`RightBackend`) opens `data.db` per operation instead of caching a long-lived connection, so recovery can delete sidecars without split-brain.

**Tech Stack:** Rust 2024, Turso 0.7.0-pre.7 (`turso`/`turso_core`), tokio, `fs4` advisory lock, `cargo nextest`, `devenv`.

**Spec:** `docs/superpowers/specs/2026-06-14-wal-desync-self-healing-design.md`

**Reference:** [tursodatabase/turso#769](https://github.com/tursodatabase/turso/issues/769) (experimental multiprocess WAL, not production ready).

---

## File structure

- `crates/right-db/src/error.rs` — add `DbError::is_wal_corruption()` + helper (mirrors existing `is_transient`/`transient_kind`).
- `crates/right-db/src/lib.rs` — add `remove_wal_sidecars` (pure file-op) + `recover_wal_sidecars` (lock + re-check + remove), wire a recovery branch into the `open_connection` retry loop.
- `crates/right-db/tests/wal_recovery.rs` — new committed fixture-gated integration test (self-skips without `RIGHT_WAL_FIXTURE`).
- `crates/right/src/right_backend.rs` — remove `ConnCache`; `get_conn` opens per-op with an explanatory comment.
- `crates/right/src/right_backend_tests.rs` — add per-op (no-cache) test.
- Delete `crates/right-db/tests/wal_desync_spike.rs` (throwaway).

`dashmap` stays in `crates/right/Cargo.toml` — it is still used by `aggregator.rs` and `internal_api*.rs`. Only the `use dashmap::DashMap;` in `right_backend.rs` is removed.

---

## Task 1: WAL-corruption detector in `right-db`

**Files:**
- Modify: `crates/right-db/src/error.rs` (add method + helper near `turso_transient_kind`, line ~105; add test in the existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/right-db/src/error.rs`:

```rust
    #[test]
    fn is_wal_corruption_matches_short_read_only() {
        let msg = "I/O error: short read on WAL frame at offset 2566792: expected 4096 bytes, got 0";
        assert!(
            DbError::Database(turso::Error::Error(msg.into())).is_wal_corruption(),
            "bare turso short-read must be WAL corruption",
        );
        assert!(
            (DbError::Open {
                path: "data.db".into(),
                source: turso::Error::Error(msg.into()),
            })
            .is_wal_corruption(),
            "Open(short-read) must be WAL corruption",
        );
        // Negatives: transient, constraint, not-found, and main-db corruption.
        assert!(!DbError::Database(turso::Error::Busy("locked".into())).is_wal_corruption());
        assert!(
            !DbError::Database(turso::Error::Constraint("unique".into())).is_wal_corruption()
        );
        assert!(!DbError::NotFound.is_wal_corruption());
        assert!(
            !DbError::Database(turso::Error::Error("database header magic mismatch".into()))
                .is_wal_corruption(),
            "main-database corruption is NOT sidecar-recoverable",
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-db --lib is_wal_corruption_matches_short_read_only`
Expected: FAIL — `no method named is_wal_corruption`.

- [ ] **Step 3: Write minimal implementation**

In `crates/right-db/src/error.rs`, add to `impl DbError` (after `transient_kind`):

```rust
    /// True if the error is a recoverable WAL-sidecar desync. Turso's
    /// experimental multiprocess WAL (tursodatabase/turso#769) can leave a stale
    /// `-tshm` authority that claims frames the `-wal` no longer holds, so every
    /// open fails with "short read on WAL frame". Recoverable by resetting the
    /// `-tshm`/`-shm` sidecars. Deliberately narrow: never matches main-database
    /// corruption, which is NOT sidecar-recoverable.
    pub fn is_wal_corruption(&self) -> bool {
        match self {
            Self::Database(error) => is_turso_wal_corruption(error),
            Self::Open { source, .. } => is_turso_wal_corruption(source),
            Self::Migration { source, .. } => source.is_wal_corruption(),
            _ => false,
        }
    }
```

And add this free function next to `turso_transient_kind`:

```rust
fn is_turso_wal_corruption(error: &turso::Error) -> bool {
    matches!(
        error,
        turso::Error::Error(message)
            if message.contains("short read on WAL") || message.contains("WAL short read")
    )
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-db --lib is_wal_corruption_matches_short_read_only`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-db/src/error.rs
git commit -m "feat(right-db): detect recoverable WAL-sidecar desync (turso#769)"
```

---

## Task 2: Recovery + open-loop wiring in `right-db`

**Files:**
- Modify: `crates/right-db/src/lib.rs` (add two fns; extend `open_connection` loop; add a unit test in `mod tests`)

- [ ] **Step 1: Write the failing test (pure file-op)**

Add to `mod tests` in `crates/right-db/src/lib.rs`:

```rust
    #[tokio::test]
    async fn remove_wal_sidecars_drops_authority_keeps_wal() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["data.db", "data.db-wal", "data.db-shm", "data.db-tshm"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }

        super::remove_wal_sidecars(dir.path()).unwrap();

        assert!(dir.path().join("data.db").exists(), "main db must remain");
        assert!(dir.path().join("data.db-wal").exists(), "-wal must remain");
        assert!(!dir.path().join("data.db-shm").exists(), "-shm must be removed");
        assert!(!dir.path().join("data.db-tshm").exists(), "-tshm must be removed");

        // Idempotent: re-running with sidecars already gone is fine.
        super::remove_wal_sidecars(dir.path()).unwrap();
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-db --lib remove_wal_sidecars_drops_authority_keeps_wal`
Expected: FAIL — `cannot find function remove_wal_sidecars`.

- [ ] **Step 3: Write minimal implementation**

In `crates/right-db/src/lib.rs`, add these two functions (place after `open_connection_once`). Note `Connection` and `bootstrap_lock` are already in scope (`pub use connection::Connection;`, `mod bootstrap_lock;`):

```rust
/// Delete the Turso WAL-coordination sidecars so the next open cold-rebuilds.
/// Removes `-tshm` (the persisted authority snapshot) and `-shm` (the
/// wal-index). Keeps `-wal`: a non-empty WAL's still-valid frame prefix is
/// salvaged on rebuild. See tursodatabase/turso#769.
fn remove_wal_sidecars(agent_path: &Path) -> Result<(), DbError> {
    for suffix in ["-tshm", "-shm"] {
        let sidecar = agent_path.join(format!("data.db{suffix}"));
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(DbError::Open {
                    path: sidecar,
                    source: turso::Error::Error(format!(
                        "failed to remove WAL sidecar during recovery: {e}"
                    )),
                });
            }
        }
    }
    Ok(())
}

/// Recover from a WAL-sidecar desync (turso#769). Serializes across the bot and
/// aggregator processes via the per-agent bootstrap lock, re-checks whether a
/// sibling already healed the database, and otherwise resets the sidecars.
async fn recover_wal_sidecars(agent_path: &Path) -> Result<(), DbError> {
    let _lock = bootstrap_lock::acquire(agent_path).await?;
    let db_path = agent_path.join("data.db");
    // The desync surfaces during open (WAL recovery). Re-check under the lock so
    // we don't reset sidecars a sibling just rebuilt.
    match Connection::open_local(db_path, true).await {
        Ok(_) => return Ok(()),
        Err(error) if error.is_wal_corruption() => {}
        Err(error) => return Err(error),
    }
    remove_wal_sidecars(agent_path)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-db --lib remove_wal_sidecars_drops_authority_keeps_wal`
Expected: PASS.

- [ ] **Step 5: Wire recovery into the open loop**

In `crates/right-db/src/lib.rs`, replace the body of `open_connection` (currently lines ~69-89) with:

```rust
pub async fn open_connection(agent_path: &Path, migrate: bool) -> Result<Connection, DbError> {
    let db_path = agent_path.join("data.db");
    let mut retries = 0;
    let mut recovered = false;
    loop {
        match open_connection_once(agent_path, migrate).await {
            Ok(conn) => return Ok(conn),
            Err(error) if error.is_transient() && retries < DB_OPEN_MAX_RETRIES => {
                retries += 1;
                log_transient_db_retry(
                    &db_path,
                    retries,
                    DB_OPEN_MAX_RETRIES,
                    &error,
                    "transient database open failed; retrying",
                );
                tokio::time::sleep(DB_OPEN_RETRY_DELAY).await;
            }
            Err(error) if error.is_wal_corruption() && !recovered => {
                recovered = true;
                tracing::warn!(
                    path = %db_path.display(),
                    error = format!("{error:#}"),
                    "WAL desync detected; resetting -tshm/-shm sidecars and retrying \
                     (tursodatabase/turso#769)",
                );
                recover_wal_sidecars(agent_path).await?;
            }
            Err(error) => return Err(error),
        }
    }
}
```

- [ ] **Step 6: Verify the crate still builds and existing tests pass**

Run: `devenv shell -- cargo test -p right-db --lib`
Expected: PASS (all existing lib tests + the two new ones).

- [ ] **Step 7: Commit**

```bash
git add crates/right-db/src/lib.rs
git commit -m "feat(right-db): self-heal WAL-sidecar desync on open (turso#769)"
```

---

## Task 3: Fixture-gated integration test

**Files:**
- Create: `crates/right-db/tests/wal_recovery.rs`

- [ ] **Step 1: Write the test**

Create `crates/right-db/tests/wal_recovery.rs`:

```rust
//! End-to-end check that `open_connection` self-heals a real WAL-sidecar desync.
//!
//! The desync cannot be synthesized deterministically (Turso rescans the WAL on
//! open and tolerates a torn tail; the failure needs a stale `-tshm` authority
//! accumulated across process generations). So this test runs only when
//! `RIGHT_WAL_FIXTURE` points at a real desync fixture (e.g. an incident
//! backup dir containing data.db + data.db-{shm,tshm,wal}); it self-skips
//! otherwise. See tursodatabase/turso#769.

use std::path::PathBuf;

#[tokio::test]
async fn open_connection_self_heals_wal_desync_fixture() {
    let Some(src) = std::env::var("RIGHT_WAL_FIXTURE").ok().map(PathBuf::from) else {
        eprintln!("RIGHT_WAL_FIXTURE unset — skipping live WAL-desync recovery test");
        return;
    };
    assert!(src.is_dir(), "RIGHT_WAL_FIXTURE must be a directory");

    let dir = tempfile::tempdir().unwrap();
    for name in ["data.db", "data.db-shm", "data.db-tshm", "data.db-wal"] {
        let from = src.join(name);
        if from.exists() {
            std::fs::copy(&from, dir.path().join(name)).unwrap();
        }
    }

    // First open must succeed because recovery resets the sidecars and retries.
    let conn = right_db::open_connection(dir.path(), false)
        .await
        .expect("open_connection must self-heal the WAL desync");

    // The database is usable afterward (any committed table reads back).
    let n: i64 = conn
        .query_row("SELECT count(*) FROM cron_specs", (), |r| r.get(0))
        .await
        .expect("a known table must be readable after recovery");
    assert!(n >= 0);

    // The authority/index sidecars were reset; the main db survives.
    assert!(dir.path().join("data.db").exists());
    assert!(!dir.path().join("data.db-tshm").exists());
}
```

- [ ] **Step 2: Run it without the fixture (must skip cleanly)**

Run: `devenv shell -- cargo test -p right-db --test wal_recovery`
Expected: PASS with "RIGHT_WAL_FIXTURE unset — skipping" printed.

- [ ] **Step 3: Run it with the incident fixture (manual validation)**

Run: `RIGHT_WAL_FIXTURE=/Users/developer/.right/agents/agent-a/_wal_incident_backup_20260614 devenv shell -- cargo test -p right-db --test wal_recovery -- --nocapture`
Expected: PASS — open succeeds, `cron_specs` count read, `-tshm` gone.
(If the live backup has been pruned, this step is informational; the self-skip path is the committed behavior.)

- [ ] **Step 4: Commit**

```bash
git add crates/right-db/tests/wal_recovery.rs
git commit -m "test(right-db): fixture-gated WAL-desync self-heal integration test"
```

---

## Task 4: Aggregator opens `data.db` per operation

**Files:**
- Modify: `crates/right/src/right_backend.rs` (remove `ConnCache`; rewrite `get_conn`; drop the `use dashmap::DashMap;`)
- Modify: `crates/right/src/right_backend_tests.rs` (add no-cache test)

- [ ] **Step 1: Write the failing test**

Add to `crates/right/src/right_backend_tests.rs` (follow the existing setup helpers in that file for `agents_dir`; create the agent db via `right_db::open_connection(&agent_dir, true)`):

```rust
    #[tokio::test]
    async fn get_conn_opens_per_operation_without_caching() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().to_path_buf();
        let agent_dir = agents_dir.join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        right_db::open_connection(&agent_dir, true).await.unwrap();

        let backend = RightBackend::new(agents_dir, None);
        let a = backend.get_conn("agent").await.unwrap();
        let b = backend.get_conn("agent").await.unwrap();

        assert!(
            !std::sync::Arc::ptr_eq(&a, &b),
            "get_conn must open per operation, not return a cached handle",
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right get_conn_opens_per_operation_without_caching`
Expected: FAIL — the cached `get_conn` returns the same `Arc` (`ptr_eq` true).

- [ ] **Step 3: Implement per-op `get_conn` and remove the cache**

In `crates/right/src/right_backend.rs`:

a. Remove the import (line ~14): delete `use dashmap::DashMap;`.

b. Remove the type alias (line ~118-119):
```rust
/// Connection cache keyed by agent name.
type ConnCache = Arc<DashMap<String, Arc<tokio::sync::Mutex<right_db::Connection>>>>;
```

c. Remove the field from `struct RightBackend` (line ~123): delete `conn_cache: ConnCache,`.

d. Remove the initializer in `RightBackend::new` (line ~132): delete `conn_cache: Arc::new(DashMap::new()),`.

e. Replace `get_conn` (lines ~359-377) with:

```rust
    /// Open the agent's `data.db` for a single operation. The caller drops the
    /// returned connection when done.
    ///
    /// The aggregator must NOT hold a long-lived `data.db` handle. Turso's
    /// experimental multiprocess WAL (tursodatabase/turso#769, not production
    /// ready) can desync the `-wal`/`-tshm` sidecars under concurrent
    /// cross-process access; self-healing recovery in `right_db::open_connection`
    /// repairs that by deleting the `-tshm`/`-shm` sidecars. A cached connection
    /// here would keep writing to the unlinked inodes while the bot rebuilds
    /// fresh ones — split brain. Opening per operation (like the bot) keeps the
    /// concurrency window small and lets recovery delete sidecars safely.
    /// Do NOT reintroduce a connection cache.
    pub(crate) async fn get_conn(
        &self,
        agent_name: &str,
    ) -> Result<Arc<tokio::sync::Mutex<right_db::Connection>>, anyhow::Error> {
        let db_dir = self.agents_dir.join(agent_name);
        let conn = right_db::open_connection(&db_dir, false)
            .await
            .with_context(|| format!("failed to open memory DB for {agent_name}"))?;
        Ok(Arc::new(tokio::sync::Mutex::new(conn)))
    }
```

(The 18 call sites — `let conn_arc = self.get_conn(...).await?; let conn = conn_arc.lock().await;` — are unchanged.)

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right get_conn_opens_per_operation_without_caching`
Expected: PASS.

- [ ] **Step 5: Verify the `right` crate builds and existing backend tests pass**

Run: `devenv shell -- cargo test -p right right_backend`
Expected: PASS (no `DashMap`/`conn_cache` unused-item or import errors).

- [ ] **Step 6: Commit**

```bash
git add crates/right/src/right_backend.rs crates/right/src/right_backend_tests.rs
git commit -m "fix(right): aggregator opens data.db per-op, no connection cache (turso#769)"
```

---

## Task 5: Remove spike, final verification

**Files:**
- Delete: `crates/right-db/tests/wal_desync_spike.rs`

- [ ] **Step 1: Delete the throwaway spike**

```bash
git rm -f crates/right-db/tests/wal_desync_spike.rs 2>/dev/null || rm -f crates/right-db/tests/wal_desync_spike.rs
```

- [ ] **Step 2: Clippy (workspace, tests, deny warnings)**

Run: `devenv shell -- cargo clippy --workspace --tests -- -D warnings`
Expected: no warnings/errors.

- [ ] **Step 3: Full workspace tests (mandatory in this worktree)**

Run: `devenv shell -- cargo nextest run --workspace`
Expected: PASS. Note any pre-existing flakes (see project memory: cc/invocation pid race, dashboard warn-count) and re-run isolated if they appear.

- [ ] **Step 4: Doctests**

Run: `devenv shell -- cargo test --doc --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore(right-db): drop throwaway WAL desync spike"
```

---

## Self-review notes (addressed)

- **Spec coverage:** Change 1 → Task 4; Change 2 (detector) → Task 1; (recovery + loop) → Task 2; testing items → Tasks 1-4; spike deletion + final verification → Task 5. Concurrent-writer audit needs no code (memory-server is legacy/unlaunched).
- **Reproduction constraint:** the spec's synthetic-desync test was dropped (Turso rescans/tolerates torn WALs — proven in spike Q5/Q6); replaced by deterministic file-op + detector unit tests plus a fixture-gated integration test. Turso's heal-after-removal is established by spike Q3 + live recovery.
- **Type consistency:** `is_wal_corruption` (Task 1) used by `recover_wal_sidecars` and the open loop (Task 2); `remove_wal_sidecars` (Task 2 Step 3) matches the test (Task 2 Step 1); `get_conn` signature unchanged so all 18 call sites compile (Task 4).
- **dashmap dependency:** kept in `crates/right/Cargo.toml` (used by `aggregator.rs`/`internal_api*.rs`); only the `right_backend.rs` import is removed.

## Out of scope / follow-ups (not in this plan)

- File the upstream turso issue (stale `-tshm` + empty `-wal` slips past authority-rebuild into a hard short-read).
- Dead-code removal of stdio `memory-server` / `HttpMemoryServer`.
- Variant D2 (drop `experimental_multiprocess_wal`) — needs a cross-process spike first.
