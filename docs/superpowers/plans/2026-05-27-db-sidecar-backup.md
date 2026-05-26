# DB Sidecar Backup Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make database sidecars (`data.db-*`) disposable runtime files that are never archived and are removed during restore.

**Architecture:** Keep the canonical durable database backup as `VACUUM INTO backup/data.db`. Add one small helper in `crates/right/src/main.rs` for deleting sidecars in an agent directory, use it during restore, and make the no-sandbox tar path exclude `data.db-*`. Cover behavior with one unit-test slice and two CLI integration regressions.

**Tech Stack:** Rust 2024, existing `right` CLI, std filesystem APIs, system `tar`, existing `assert_cmd` integration tests.

---

## File Structure

- Modify `crates/right/src/main.rs`
  - Add a private `remove_database_sidecars(&Path) -> miette::Result<usize>` helper near restore helpers.
  - Add a private `push_no_sandbox_database_tar_excludes(&mut Vec<String>, &str)` helper near backup tar construction.
  - Call sidecar cleanup after restore config copy and again after no-sandbox tar extraction.
  - Extend existing unit-test imports and add helper tests in the existing `#[cfg(test)] mod tests`.
- Modify `crates/right/tests/cli_integration.rs`
  - Extend no-sandbox backup/restore coverage with `data.db-*` sidecars.
  - Add a legacy backup restore regression that restores from a tar containing stale sidecars.
- Modify `ARCHITECTURE.md`
  - Clarify that `data.db-*` files are runtime sidecars and not backup state.
- Modify `docs/architecture/lifecycle.md`
  - Clarify full backup and restore sidecar behavior.

## Task 1: Add Sidecar Cleanup Helper

**Files:**
- Modify: `crates/right/src/main.rs`

- [ ] **Step 1: Write failing unit tests**

In `crates/right/src/main.rs`, update the test module import list so it includes `remove_database_sidecars`:

```rust
    use super::{
        ConfigCommands, MemoryCommands, build_agent_ssh_command, cleanup_failed_restore_agent_dir,
        copy_agent_backup_config_files, copy_agent_restore_config_files, remove_database_sidecars,
        resolve_agent_db, resolve_restored_policy_path, restored_mcp_auth_method,
        truncate_content, write_bootstrap_right_mcp_policy, write_managed_settings,
    };
```

Add these tests near `cleanup_failed_restore_agent_dir_removes_partial_agent_state`:

```rust
    #[test]
    fn remove_database_sidecars_deletes_runtime_files_only() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("right-drill");
        fs::create_dir_all(agent_dir.join("data.db-dir")).unwrap();
        fs::write(agent_dir.join("data.db"), "canonical").unwrap();
        fs::write(agent_dir.join("data.db-wal"), "wal").unwrap();
        fs::write(agent_dir.join("data.db-shm"), "shm").unwrap();
        fs::write(agent_dir.join("data.db-tshm"), "tshm").unwrap();
        fs::write(agent_dir.join("data.db-future"), "future").unwrap();
        fs::write(agent_dir.join("notes.txt"), "notes").unwrap();

        let removed = remove_database_sidecars(&agent_dir).unwrap();

        assert_eq!(removed, 4);
        assert!(agent_dir.join("data.db").exists());
        assert!(agent_dir.join("data.db-dir").exists());
        assert!(agent_dir.join("notes.txt").exists());
        assert!(!agent_dir.join("data.db-wal").exists());
        assert!(!agent_dir.join("data.db-shm").exists());
        assert!(!agent_dir.join("data.db-tshm").exists());
        assert!(!agent_dir.join("data.db-future").exists());
    }

    #[cfg(unix)]
    #[test]
    fn remove_database_sidecars_unlinks_symlink_without_touching_target() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("right-drill");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("target.txt"), "target").unwrap();
        std::os::unix::fs::symlink(agent_dir.join("target.txt"), agent_dir.join("data.db-link"))
            .unwrap();

        let removed = remove_database_sidecars(&agent_dir).unwrap();

        assert_eq!(removed, 1);
        assert!(agent_dir.join("target.txt").exists());
        assert!(!agent_dir.join("data.db-link").exists());
    }
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
devenv shell -- cargo test -p right --lib remove_database_sidecars
```

Expected: compile failure because `remove_database_sidecars` does not exist.

- [ ] **Step 3: Implement cleanup helper**

In `crates/right/src/main.rs`, add this helper after `cleanup_failed_restore_agent_dir`:

```rust
fn remove_database_sidecars(agent_dir: &Path) -> miette::Result<usize> {
    use miette::IntoDiagnostic;

    if !agent_dir.exists() {
        return Ok(0);
    }

    let entries = std::fs::read_dir(agent_dir)
        .into_diagnostic()
        .map_err(|e| {
            miette::miette!(
                "failed to read agent dir {} for database sidecar cleanup: {e:#}",
                agent_dir.display()
            )
        })?;

    let mut removed = 0usize;
    for entry in entries {
        let entry = entry.into_diagnostic().map_err(|e| {
            miette::miette!(
                "failed to inspect agent dir {} during database sidecar cleanup: {e:#}",
                agent_dir.display()
            )
        })?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if !file_name.starts_with("data.db-") {
            continue;
        }

        let file_type = entry.file_type().into_diagnostic().map_err(|e| {
            miette::miette!(
                "failed to read file type for {} during database sidecar cleanup: {e:#}",
                entry.path().display()
            )
        })?;
        if file_type.is_dir() {
            continue;
        }

        std::fs::remove_file(entry.path())
            .into_diagnostic()
            .map_err(|e| {
                miette::miette!(
                    "failed to remove database sidecar {}: {e:#}",
                    entry.path().display()
                )
            })?;
        removed += 1;
    }

    Ok(removed)
}
```

- [ ] **Step 4: Run tests to verify pass**

Run:

```bash
devenv shell -- cargo test -p right --lib remove_database_sidecars
```

Expected: tests matching `remove_database_sidecars` pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "fix(backup): add database sidecar cleanup helper"
```

## Task 2: Exclude Sidecars From No-Sandbox Backup Tar

**Files:**
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/tests/cli_integration.rs`

- [ ] **Step 1: Write failing integration coverage**

In `crates/right/tests/cli_integration.rs`, inside `test_agent_backup_and_restore_no_sandbox`, immediately after `drop(conn);`, add:

```rust
    for sidecar in [
        "data.db-wal",
        "data.db-shm",
        "data.db-tshm",
        "data.db-future",
    ] {
        fs::write(agent_dir.join(sidecar), format!("{sidecar}\n")).unwrap();
    }
```

In the same test, immediately after `assert!(backup_dir.join("data.db").exists(), "should have data.db");`, add:

```rust
    let tar_entries = tar_entries(&backup_dir.join("sandbox.tar.gz"));
    for sidecar in [
        "data.db-wal",
        "data.db-shm",
        "data.db-tshm",
        "data.db-future",
    ] {
        assert!(
            !tar_entries.contains(&format!("test-agent/{sidecar}")),
            "no-sandbox backup tar must not contain database sidecar {sidecar}"
        );
    }
```

- [ ] **Step 2: Run test to verify failure**

Run:

```bash
devenv shell -- cargo test -p right --test cli_integration test_agent_backup_and_restore_no_sandbox -- --exact --nocapture
```

Expected: FAIL because `sandbox.tar.gz` still contains at least one `test-agent/data.db-*` entry.

- [ ] **Step 3: Implement tar exclude helper**

In `crates/right/src/main.rs`, add this helper near `copy_agent_backup_config_files`:

```rust
fn push_no_sandbox_database_tar_excludes(tar_args: &mut Vec<String>, agent_name: &str) {
    tar_args.push("--exclude=data.db".to_string());
    tar_args.push("--exclude=data.db-*".to_string());
    tar_args.push(format!("--exclude={agent_name}/data.db"));
    tar_args.push(format!("--exclude={agent_name}/data.db-*"));
}
```

In `cmd_agent_backup`, replace the current no-sandbox `tar_args` initialization:

```rust
        let mut tar_args = vec![
            "czpf".to_string(),
            dest_tar
                .to_str()
                .ok_or_else(|| miette::miette!("non-UTF-8 backup path"))?
                .to_string(),
            "--exclude=data.db".to_string(),
        ];
```

with:

```rust
        let mut tar_args = vec![
            "czpf".to_string(),
            dest_tar
                .to_str()
                .ok_or_else(|| miette::miette!("non-UTF-8 backup path"))?
                .to_string(),
        ];
        push_no_sandbox_database_tar_excludes(&mut tar_args, agent_name);
```

- [ ] **Step 4: Run targeted test to verify pass**

Run:

```bash
devenv shell -- cargo test -p right --test cli_integration test_agent_backup_and_restore_no_sandbox -- --exact --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/main.rs crates/right/tests/cli_integration.rs
git commit -m "fix(backup): exclude database sidecars from tar"
```

## Task 3: Remove Sidecars During Restore

**Files:**
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/tests/cli_integration.rs`

- [ ] **Step 1: Write failing legacy restore regression**

In `crates/right/tests/cli_integration.rs`, after `test_agent_backup_and_restore_no_sandbox`, add:

```rust
#[test]
fn test_agent_restore_no_sandbox_removes_legacy_db_sidecars() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();
    fs::write(
        home.path().join("config.yaml"),
        minimal_config_yaml(home.path()),
    )
    .unwrap();

    let backup_dir = home
        .path()
        .join("backups")
        .join("source-agent")
        .join("20260527-0100");
    fs::create_dir_all(&backup_dir).unwrap();
    fs::write(backup_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
    fs::write(backup_dir.join("data.db"), "canonical db snapshot\n").unwrap();

    let tar_root = home.path().join("legacy-tar-root");
    let tar_agent = tar_root.join("source-agent");
    fs::create_dir_all(&tar_agent).unwrap();
    fs::write(tar_agent.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
    fs::write(tar_agent.join("notes.txt"), "from tar\n").unwrap();
    for sidecar in [
        "data.db-wal",
        "data.db-shm",
        "data.db-tshm",
        "data.db-future",
    ] {
        fs::write(tar_agent.join(sidecar), format!("stale {sidecar}\n")).unwrap();
    }

    let tar_path = backup_dir.join("sandbox.tar.gz");
    let status = StdCommand::new("tar")
        .args([
            "czf",
            tar_path.to_str().unwrap(),
            "-C",
            tar_root.to_str().unwrap(),
            "source-agent",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "test tar creation must succeed");

    right()
        .args([
            "--home",
            home_str,
            "agent",
            "init",
            "restored-agent",
            "--from-backup",
            backup_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("restored"));

    let restored_dir = home.path().join("agents").join("restored-agent");
    assert_eq!(
        fs::read_to_string(restored_dir.join("data.db")).unwrap(),
        "canonical db snapshot\n"
    );
    assert_eq!(
        fs::read_to_string(restored_dir.join("notes.txt")).unwrap(),
        "from tar\n"
    );
    for sidecar in [
        "data.db-wal",
        "data.db-shm",
        "data.db-tshm",
        "data.db-future",
    ] {
        assert!(
            !restored_dir.join(sidecar).exists(),
            "restore must remove stale database sidecar {sidecar}"
        );
    }
}
```

Also extend `test_agent_backup_and_restore_no_sandbox` immediately before the
`// Verify restored database.` comment, so the assertion runs before any DB
open can legitimately recreate runtime sidecars:

```rust
    for sidecar in [
        "data.db-wal",
        "data.db-shm",
        "data.db-tshm",
        "data.db-future",
    ] {
        assert!(
            !restored_dir.join(sidecar).exists(),
            "restored agent must not contain database sidecar {sidecar}"
        );
    }
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
devenv shell -- cargo test -p right --test cli_integration test_agent_restore_no_sandbox_removes_legacy_db_sidecars -- --exact --nocapture
```

Expected: FAIL because sidecars extracted from the legacy tar remain in the restored agent directory.

- [ ] **Step 3: Call cleanup from restore flow**

In `cmd_agent_restore`, immediately after:

```rust
        copy_agent_restore_config_files(backup_path, &agent_dir, &backup_config)?;
```

add:

```rust
        remove_database_sidecars(&agent_dir)?;
```

In the no-sandbox branch, immediately after the successful tar extraction status check:

```rust
            if !status.success() {
                return Err(miette::miette!(
                    "tar extraction failed with status {status}"
                ));
            }
```

add:

```rust
            remove_database_sidecars(&agent_dir)?;
```

- [ ] **Step 4: Run targeted restore tests to verify pass**

Run:

```bash
devenv shell -- cargo test -p right --test cli_integration test_agent_restore_no_sandbox_removes_legacy_db_sidecars -- --exact --nocapture
devenv shell -- cargo test -p right --test cli_integration test_agent_backup_and_restore_no_sandbox -- --exact --nocapture
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/main.rs crates/right/tests/cli_integration.rs
git commit -m "fix(restore): remove database sidecars"
```

## Task 4: Update Architecture Docs

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/lifecycle.md`

- [ ] **Step 1: Update local database rules**

In `ARCHITECTURE.md`, in `Local Database Rules`, revise the sidecar sentence to explicitly say sidecars are not backup state:

```markdown
aggregator processes can open the same per-agent `data.db`; this may create
Turso sidecar files such as `data.db-tshm` next to the standard database/WAL
files. Files matching `data.db-*` are disposable runtime sidecars, not durable
backup state; backup and restore flows preserve only the canonical
`VACUUM INTO` snapshot at `backup/data.db`.
```

- [ ] **Step 2: Update lifecycle backup/restore docs**

In `docs/architecture/lifecycle.md`, update the backup block:

```markdown
  ├─ No-sandbox mode: tar agent dir → sandbox.tar.gz, excluding data.db and data.db-* sidecars
  ├─ Full mode: + agent.yaml, allowlist.yaml, policy.yaml, VACUUM INTO data.db
```

In the restore block, update the config restore line:

```markdown
  ├─ Restore config/control-plane files to new agent dir (agent.yaml, allowlist.yaml, policy.yaml, data.db when present)
  ├─ Remove restored data.db-* sidecars; the canonical DB snapshot is data.db only
```

- [ ] **Step 3: Run doc/diff checks**

Run:

```bash
devenv shell -- git diff --check
```

Expected: no output and exit status 0.

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md docs/architecture/lifecycle.md
git commit -m "docs(backup): document database sidecar handling"
```

## Task 5: Final Verification

**Files:**
- Verify whole workspace.

- [ ] **Step 1: Run targeted backup/restore tests**

Run:

```bash
devenv shell -- cargo test -p right --test cli_integration test_agent_backup_and_restore_no_sandbox -- --exact --nocapture
devenv shell -- cargo test -p right --test cli_integration test_agent_restore_no_sandbox_removes_legacy_db_sidecars -- --exact --nocapture
devenv shell -- cargo test -p right --lib remove_database_sidecars
```

Expected: all targeted tests pass.

- [ ] **Step 2: Run full workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: all non-ignored tests pass.

- [ ] **Step 3: Run final build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: build completes successfully.

- [ ] **Step 4: Inspect final git state**

Run:

```bash
devenv shell -- git status --short
devenv shell -- git log --oneline -5
```

Expected: worktree is clean, and the latest commits include the backup/restore sidecar implementation and docs commits.
