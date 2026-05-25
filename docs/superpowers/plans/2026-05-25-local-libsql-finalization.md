# Local libSQL Finalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finalize the local-only `libsql` migration by hardening `right-db` local contracts, proving no `rusqlite` surface remains, and running full workspace verification.

**Architecture:** `right-db` remains the only database-driver boundary. This plan adds focused local-contract regression coverage around read-only explicit-path opens, migration error context, row conversion, and parameter null handling, then verifies all dependent crates. No Turso cloud, embedded replica, sync, or cloud migration work is included.

**Tech Stack:** Rust 2024, local `libsql` 0.9.30, `right-db`, per-agent SQLite-compatible `data.db`, `devenv shell -- cargo`.

**Spec:** `docs/superpowers/specs/2026-05-25-local-libsql-finalization-design.md`.

---

## File Structure

- Modify `crates/right-db/tests/smoke.rs`
  - Add explicit-path read-only contract tests for `open_database_path_readonly`, which is used by restore and other non-standard DB-path readers.
- Modify `crates/right-db/src/migrations.rs`
  - Add a migration error-context regression test proving failed migrations include both migration version and database path.
- Modify `crates/right-db/src/row.rs`
  - Add row conversion tests for type mismatch and invalid boolean sentinels.
- Modify `crates/right-db/src/params.rs`
  - Add parameter/row round-trip coverage for `Option<T>` null and non-null values.
- Inspect only; edit only if stale text is found:
  - `ARCHITECTURE.md`
  - `docs/architecture/modules.md`
  - `docs/architecture/memory.md`
- No planned changes:
  - `PROMPT_SYSTEM.md`
  - any cloud/Turso configuration files or docs.

## Baseline

- [ ] **Step 0.1: Confirm Rust skill availability before code edits**

This repository asks implementers to load `rust-dev:rust-dev` before writing Rust. If that skill is available in the implementation session, load it before Task 1. If it is not available in the session skill list, record this line in the implementation notes before editing:

```text
rust-dev:rust-dev unavailable in this Codex session; proceeding with direct Rust edits under project Rust conventions.
```

- [ ] **Step 0.2: Re-read the approved spec and local DB rules**

Run:

```bash
devenv shell -- sed -n '1,260p' docs/superpowers/specs/2026-05-25-local-libsql-finalization-design.md
devenv shell -- sed -n '480,510p' ARCHITECTURE.md
devenv shell -- sed -n '70,95p' docs/architecture/modules.md
devenv shell -- sed -n '45,60p' docs/architecture/memory.md
```

Expected:

- Spec says local-only.
- `ARCHITECTURE.md` says local libSQL is hidden behind `right-db`.
- `docs/architecture/modules.md` describes `Connection`, `Transaction`, and `DbError` as the local libSQL boundary.
- `docs/architecture/memory.md` mentions legacy memory tables only as migration compatibility.

- [ ] **Step 0.3: Run the targeted baseline**

Run:

```bash
devenv shell -- cargo test -p right-db
```

Expected: PASS. If it fails, stop and fix the pre-existing `right-db` failure before adding new tests.

- [ ] **Step 0.4: Confirm no direct `rusqlite` surface remains**

Run:

```bash
devenv shell -- rg -n "rusqlite::|use rusqlite|rusqlite =|rusqlite_migration" Cargo.toml crates
```

Expected: no matches. If matches appear, remove only direct `rusqlite` surface introduced by the local migration and rerun this command before continuing.

## Task 1: Harden Explicit Read-Only Path Contracts

**Files:**
- Modify `crates/right-db/tests/smoke.rs`

- [ ] **Step 1.1: Import the explicit-path helper**

In `crates/right-db/tests/smoke.rs`, change the first line from:

```rust
use right_db::{MIGRATIONS, open_connection, open_connection_readonly, open_db};
```

to:

```rust
use right_db::{
    MIGRATIONS, open_connection, open_connection_readonly, open_database_path_readonly, open_db,
};
```

- [ ] **Step 1.2: Add missing explicit-path read-only tests**

In `crates/right-db/tests/smoke.rs`, add these tests immediately after `libsql_open_connection_readonly_requires_existing_db`:

```rust
#[test]
fn open_database_path_readonly_missing_file_does_not_create_file() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("restore-probe.db");

    let err = open_database_path_readonly(&db_path).expect_err("missing db should not open");

    assert!(!db_path.exists(), "readonly explicit path must not create a db file");
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

    assert!(err.to_string().contains("readonly database"));
    assert_eq!(
        query_user_version(&conn),
        before,
        "readonly explicit path must not change user_version",
    );
}
```

- [ ] **Step 1.3: Run the new read-only tests**

Run:

```bash
devenv shell -- cargo test -p right-db open_database_path_readonly
```

Expected: PASS with both new tests passing.

## Task 2: Add Migration Error Context Coverage

**Files:**
- Modify `crates/right-db/src/migrations.rs`

- [ ] **Step 2.1: Add a path/version regression test**

In `crates/right-db/src/migrations.rs`, inside the existing `#[cfg(test)] mod tests`, add this test immediately after `migration_runner_semantics_libsql_rolls_back_all_pending_migrations_on_later_failure`:

```rust
#[test]
fn migration_error_reports_version_and_database_path() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::open_connection(dir.path(), false).unwrap();
    let db_path = dir.path().join("data.db");

    let err = FAILING_MIGRATIONS
        .to_latest(&conn)
        .expect_err("second migration should fail");
    let message = err.to_string();

    assert!(
        message.contains("migration 2"),
        "error should include failing migration version, got {message}",
    );
    assert!(
        message.contains(&db_path.display().to_string()),
        "error should include database path {}, got {message}",
        db_path.display(),
    );
}
```

- [ ] **Step 2.2: Run the migration error test**

Run:

```bash
devenv shell -- cargo test -p right-db migration_error_reports_version_and_database_path
```

Expected: PASS.

- [ ] **Step 2.3: Run the migration semantics tests**

Run:

```bash
devenv shell -- cargo test -p right-db migration_runner_semantics
devenv shell -- cargo test -p right-db cold_boot_concurrent_migrators_do_not_double_apply_v23
```

Expected: PASS.

## Task 3: Add Row Conversion Contract Tests

**Files:**
- Modify `crates/right-db/src/row.rs`

- [ ] **Step 3.1: Add row conversion tests**

At the end of `crates/right-db/src/row.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use crate::{Connection, DbError};

    #[test]
    fn row_get_reports_type_mismatch_as_invalid_parameter() {
        let conn = Connection::open_in_memory().unwrap();

        let err = conn
            .query_row("SELECT 'not-an-integer'", (), |row| row.get::<_, i64>(0))
            .expect_err("TEXT should not decode as i64");

        assert!(
            matches!(err, DbError::InvalidParameter(ref message) if message.contains("expected SQLite INTEGER")),
            "expected InvalidParameter type mismatch, got {err:#}",
        );
    }

    #[test]
    fn row_get_rejects_invalid_boolean_sentinel() {
        let conn = Connection::open_in_memory().unwrap();

        let err = conn
            .query_row("SELECT 2", (), |row| row.get::<_, bool>(0))
            .expect_err("only 0 and 1 should decode as bool");

        assert!(
            matches!(err, DbError::InvalidParameter(ref message) if message.contains("boolean sentinel")),
            "expected InvalidParameter boolean sentinel error, got {err:#}",
        );
    }
}
```

- [ ] **Step 3.2: Run the row tests**

Run:

```bash
devenv shell -- cargo test -p right-db row::tests
```

Expected: PASS.

## Task 4: Add Parameter Null Round-Trip Coverage

**Files:**
- Modify `crates/right-db/src/params.rs`

- [ ] **Step 4.1: Add an `Option<T>` parameter round-trip test**

In `crates/right-db/src/params.rs`, inside the existing `#[cfg(test)] mod tests`, add this test after `params_from_iter_defers_large_u64_error_to_execute_result_without_panic`:

```rust
#[test]
fn option_params_round_trip_null_and_non_null_values() {
    let conn = crate::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE option_probe (
             id INTEGER PRIMARY KEY,
             label TEXT NULL,
             enabled INTEGER NULL
         )",
    )
    .unwrap();

    conn.execute(
        "INSERT INTO option_probe (label, enabled) VALUES (?1, ?2)",
        crate::params![Option::<String>::None, Some(true)],
    )
    .unwrap();

    let (label, enabled): (Option<String>, Option<bool>) = conn
        .query_row("SELECT label, enabled FROM option_probe", (), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();

    assert_eq!(label, None);
    assert_eq!(enabled, Some(true));
}
```

- [ ] **Step 4.2: Run the parameter tests**

Run:

```bash
devenv shell -- cargo test -p right-db params::tests
```

Expected: PASS.

## Task 5: Run Focused Local DB Verification And Commit

**Files:**
- Modified in Tasks 1-4

- [ ] **Step 5.1: Run the full `right-db` test suite**

Run:

```bash
devenv shell -- cargo test -p right-db
```

Expected: PASS.

- [ ] **Step 5.2: Re-run the direct-driver surface search**

Run:

```bash
devenv shell -- rg -n "rusqlite::|use rusqlite|rusqlite =|rusqlite_migration" Cargo.toml crates
```

Expected: no matches.

- [ ] **Step 5.3: Review the focused diff**

Run:

```bash
devenv shell -- git diff -- crates/right-db/tests/smoke.rs crates/right-db/src/migrations.rs crates/right-db/src/row.rs crates/right-db/src/params.rs
```

Expected:

- only tests were added, plus the `open_database_path_readonly` import;
- no production behavior changed;
- no cloud, remote, replica, or sync code was introduced.

- [ ] **Step 5.4: Commit the local contract tests**

Run:

```bash
devenv shell -- git add crates/right-db/tests/smoke.rs crates/right-db/src/migrations.rs crates/right-db/src/row.rs crates/right-db/src/params.rs
devenv shell -- git commit -m "test(db): finalize local libsql contracts"
```

Expected: commit succeeds.

## Task 6: Documentation Drift Check

**Files:**
- Inspect `ARCHITECTURE.md`
- Inspect `docs/architecture/modules.md`
- Inspect `docs/architecture/memory.md`

- [ ] **Step 6.1: Search architecture docs for stale direct-driver guidance**

Run:

```bash
devenv shell -- rg -n "rusqlite|rusqlite_migration|unchecked_transaction|libsql::|Builder::new_remote|new_remote_replica|Turso" ARCHITECTURE.md docs/architecture
```

Expected allowed matches:

- `docs/architecture/memory.md` may mention legacy memory tables retained for migration compatibility.
- `ARCHITECTURE.md` and `docs/architecture/modules.md` may mention local libSQL through `right-db`.

No allowed match may instruct new code to use `rusqlite`, `rusqlite_migration`, raw `libsql` APIs outside `right-db`, Turso remote, or embedded replica mode.

- [ ] **Step 6.2: If stale direct-driver guidance exists, replace it with the local boundary rule**

If Step 6.1 finds stale direct-driver guidance, replace the stale sentence with this exact text in the matching architecture file:

```markdown
`right-db` owns local libSQL driver details. Other crates must use project-owned `right_db` connection, transaction, row, parameter, and error types instead of raw driver APIs.
```

Then run:

```bash
devenv shell -- rg -n "rusqlite|rusqlite_migration|unchecked_transaction|libsql::|Builder::new_remote|new_remote_replica|Turso" ARCHITECTURE.md docs/architecture
```

Expected: remaining matches are historical compatibility notes or local `right-db` boundary descriptions only.

- [ ] **Step 6.3: Commit documentation drift fixes if Step 6.2 changed files**

Run only if Step 6.2 changed files:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/modules.md docs/architecture/memory.md
devenv shell -- git commit -m "docs(db): clarify local libsql boundary"
```

Expected: commit succeeds, or this step is skipped because Step 6.2 made no changes.

## Task 7: Targeted Dependent Package Verification

**Files:**
- No planned edits

- [ ] **Step 7.1: Run dependent package tests that exercise DB callers**

Run:

```bash
devenv shell -- cargo test -p right-memory
devenv shell -- cargo test -p right-mcp
devenv shell -- cargo test -p right-agent
devenv shell -- cargo test -p right-lifecycle
devenv shell -- cargo test -p right-dashboard
devenv shell -- cargo test -p right-bot
devenv shell -- cargo test -p right
```

Expected: PASS for every command.

- [ ] **Step 7.2: If a dependent package fails with a local DB regression, add a narrow test before fixing**

Use this process for each failing package:

```text
1. Identify the failing local DB contract.
2. Add the smallest regression test in the crate that owns that behavior.
3. Run the new test and confirm it fails for the observed reason.
4. Fix the local DB bug.
5. Run the package test that failed in Step 7.1.
6. Commit with a Conventional Commit message scoped to the affected crate.
```

Expected: no dependent package DB regressions remain.

## Task 8: Final Workspace Verification

**Files:**
- No planned edits

- [ ] **Step 8.1: Run the mandatory full workspace test**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 8.2: Run the mandatory debug build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 8.3: Run final surface and scope checks**

Run:

```bash
devenv shell -- rg -n "rusqlite::|use rusqlite|rusqlite =|rusqlite_migration" Cargo.toml crates
devenv shell -- rg -n "Builder::new_remote|new_remote_replica|Turso|sync health|cloud backup|cloud-backed" Cargo.toml crates docs/architecture ARCHITECTURE.md docs/superpowers/plans/2026-05-25-local-libsql-finalization.md docs/superpowers/specs/2026-05-25-local-libsql-finalization-design.md
devenv shell -- git status --short
```

Expected:

- first command has no matches;
- second command only matches explicit deferred/out-of-scope documentation in the spec or this plan;
- worktree is clean after all commits.

## Acceptance Checklist

- [ ] `right-db` tests pass.
- [ ] Direct `rusqlite` and `rusqlite_migration` usage is absent from manifests and project code.
- [ ] Explicit-path read-only opens are covered.
- [ ] Migration error path/version context is covered.
- [ ] Row conversion errors are covered.
- [ ] `Option<T>` parameter null/non-null round trip is covered.
- [ ] Architecture docs contain no stale direct-driver guidance.
- [ ] Dependent package tests pass.
- [ ] `devenv shell -- cargo test --workspace` passes.
- [ ] `devenv shell -- cargo build --workspace` passes.
