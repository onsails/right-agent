# Backup Rebuildable Excludes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `right agent backup` omit rebuildable sandbox bulk by default while adding `--include-rebuildable` for forensic full-sandbox archives.

**Architecture:** Keep the archive model broad: archive all of `/sandbox` except a small rebuildable exclude set by default. Add an explicit include flag at the CLI layer and pass the archive mode into the OpenShell tar helper; keep sandbox migration and pre-destroy backup call sites on forensic mode to preserve existing semantics for migration/destructive flows.

**Tech Stack:** Rust 2024, clap, tokio, miette, GNU tar over SSH, assert_cmd integration tests, tempfile, `devenv shell -- cargo`.

---

## File Structure

- Modify `crates/right-openshell/src/openshell.rs`
  - Owns OpenShell SSH tar command construction.
  - Exposes the default rebuildable exclude list for the CLI no-sandbox tar path.
  - Adds an `include_rebuildable` parameter to `ssh_tar_download`.
  - Keeps stderr propagation for failed remote tar commands.
- Modify `crates/right-openshell/src/openshell_tests.rs`
  - Unit-tests tar command arguments for default excludes and forensic mode.
- Modify `crates/right/src/main.rs`
  - Adds `--include-rebuildable` to `right agent backup`.
  - Passes the flag to sandboxed and no-sandbox backup paths.
  - Preserves forensic mode for sandbox migration.
- Modify `crates/right-agent/src/agent/destroy.rs`
  - Updates the helper call signature and preserves current forensic pre-destroy behavior.
- Modify `crates/right/tests/cli_integration.rs`
  - Adds no-sandbox integration coverage for default excludes and `--include-rebuildable`.
- Modify `docs/architecture/lifecycle.md`
  - Documents default backup excludes and the new forensic flag.
- Modify `ARCHITECTURE.md`
  - Updates the runtime backup invariant summary if needed after `docs/architecture/lifecycle.md` is updated.

Verification cadence:

- First run a narrow failing test before implementation for each behavior slice.
- Use targeted package/test commands while iterating.
- Final verification must run both:
  - `devenv shell -- cargo test --workspace`
  - `devenv shell -- cargo build --workspace`

## Task 1: OpenShell Tar Args Support Rebuildable Excludes

**Files:**
- Modify: `crates/right-openshell/src/openshell.rs`
- Modify: `crates/right-openshell/src/openshell_tests.rs`

- [ ] **Step 1: Write failing unit tests for default excludes and forensic mode**

In `crates/right-openshell/src/openshell_tests.rs`, replace the existing `sandbox_tar_download_args_reads_sandbox_dir_and_preserves_archive_root` test with these tests:

```rust
#[test]
fn sandbox_tar_download_args_excludes_rebuildable_dirs_by_default() {
    let args = sandbox_tar_download_args("sandbox", false).unwrap();

    assert_eq!(&args[0..5], ["tar", "czpf", "-", "-C", "/sandbox"]);
    assert!(args.contains(&"--transform=s,^\\.$,sandbox,".to_string()));
    assert!(args.contains(&"--transform=s,^\\./,sandbox/,".to_string()));
    assert_eq!(args.last().unwrap(), ".");

    for path in DEFAULT_REBUILDABLE_BACKUP_EXCLUDES {
        assert!(
            args.contains(&format!("--exclude=./{path}")),
            "missing directory exclude for {path}: {args:?}"
        );
        assert!(
            args.contains(&format!("--exclude=./{path}/*")),
            "missing child exclude for {path}: {args:?}"
        );
    }
}

#[test]
fn sandbox_tar_download_args_include_rebuildable_has_no_rebuildable_excludes() {
    let args = sandbox_tar_download_args("sandbox", true).unwrap();

    assert_eq!(&args[0..5], ["tar", "czpf", "-", "-C", "/sandbox"]);
    assert!(args.contains(&"--transform=s,^\\.$,sandbox,".to_string()));
    assert!(args.contains(&"--transform=s,^\\./,sandbox/,".to_string()));
    assert_eq!(args.last().unwrap(), ".");

    for path in DEFAULT_REBUILDABLE_BACKUP_EXCLUDES {
        assert!(
            !args.contains(&format!("--exclude=./{path}")),
            "forensic mode should not exclude {path}: {args:?}"
        );
        assert!(
            !args.contains(&format!("--exclude=./{path}/*")),
            "forensic mode should not exclude children of {path}: {args:?}"
        );
    }
}
```

- [ ] **Step 2: Run the unit test and verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-openshell sandbox_tar_download_args
```

Expected: FAIL because `sandbox_tar_download_args` does not yet accept `include_rebuildable`, and `DEFAULT_REBUILDABLE_BACKUP_EXCLUDES` is not public.

- [ ] **Step 3: Implement tar argument construction**

In `crates/right-openshell/src/openshell.rs`, define the public exclude list near the existing SSH/tar helpers:

```rust
pub const DEFAULT_REBUILDABLE_BACKUP_EXCLUDES: &[&str] = &[".cache", ".venv", ".npm", ".uv"];
```

Replace the existing `sandbox_tar_download_args` helper with:

```rust
fn sandbox_tar_download_args(
    sandbox_path: &str,
    include_rebuildable: bool,
) -> miette::Result<Vec<String>> {
    let archive_root = sandbox_path.trim_matches('/');
    if archive_root.is_empty() {
        miette::bail!("sandbox path must not be empty");
    }

    let mut args = vec![
        "tar".to_string(),
        "czpf".to_string(),
        "-".to_string(),
        "-C".to_string(),
        format!("/{archive_root}"),
        format!("--transform=s,^\\.$,{archive_root},"),
        format!("--transform=s,^\\./,{archive_root}/,"),
    ];

    if !include_rebuildable {
        for path in DEFAULT_REBUILDABLE_BACKUP_EXCLUDES {
            // GNU tar evaluates excludes before transforms, so match names as
            // seen under `-C /sandbox .`, not final `sandbox/...` archive names.
            args.push(format!("--exclude=./{path}"));
            args.push(format!("--exclude=./{path}/*"));
        }
    }

    args.push(".".to_string());
    Ok(args)
}
```

Update the `ssh_tar_download` signature in the same file:

```rust
pub async fn ssh_tar_download(
    config_path: &Path,
    ssh_host: &str,
    sandbox_path: &str,
    dest_path: &Path,
    include_rebuildable: bool,
    timeout_secs: u64,
) -> miette::Result<()> {
```

Update command construction inside `ssh_tar_download`:

```rust
command.args(sandbox_tar_download_args(
    sandbox_path,
    include_rebuildable,
)?);
```

Keep the existing stderr capture/error propagation if it is already present in the worktree:

```rust
if !child_result.success() {
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return Err(miette::miette!(
            "ssh tar download failed (exit {child_result}): {stderr}"
        ));
    }
    return Err(miette::miette!(
        "ssh tar download failed (exit {child_result})"
    ));
}
```

- [ ] **Step 4: Update non-CLI call sites to preserve existing forensic behavior**

Update `crates/right/src/main.rs` sandbox migration call around `maybe_migrate_sandbox`:

```rust
right_openshell::openshell::ssh_tar_download(
    &old_ssh_config,
    &old_ssh_host,
    "sandbox",
    &backup_tar,
    true,
    600,
)
.await?;
```

Update `crates/right-agent/src/agent/destroy.rs` pre-destroy backup call:

```rust
right_openshell::openshell::ssh_tar_download(
    &ssh_config,
    &ssh_host,
    "sandbox",
    &dest_tar,
    true,
    300,
)
.await
```

Rationale: sandbox migration and pre-destroy backup currently preserve all sandbox bytes. Do not change those semantics in this feature.

- [ ] **Step 5: Run the unit test and verify it passes**

Run:

```bash
devenv shell -- cargo test -p right-openshell sandbox_tar_download_args
```

Expected: PASS with both `sandbox_tar_download_args_*` tests passing.

## Task 2: CLI Flag And No-Sandbox Backup Excludes

**Files:**
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/tests/cli_integration.rs`

- [ ] **Step 1: Write failing integration helpers and default exclude test**

At the top of `crates/right/tests/cli_integration.rs`, change imports from:

```rust
use std::fs;
```

to:

```rust
use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
```

Add this helper near `minimal_config_yaml`:

```rust
fn tar_entries(path: &Path) -> Vec<String> {
    let output = StdCommand::new("tar")
        .args(["-tzf", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tar -tzf failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}
```

After `test_agent_backup_sandbox_only`, add:

```rust
#[test]
fn test_agent_backup_excludes_rebuildable_dirs_by_default_no_sandbox() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    let agent_dir = home.path().join("agents").join("test-agent");
    fs::create_dir_all(agent_dir.join(".claude")).unwrap();
    fs::create_dir_all(agent_dir.join(".cache")).unwrap();
    fs::create_dir_all(agent_dir.join(".venv")).unwrap();
    fs::create_dir_all(agent_dir.join(".npm")).unwrap();
    fs::create_dir_all(agent_dir.join(".uv")).unwrap();
    fs::create_dir_all(agent_dir.join("custom-dir")).unwrap();
    fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
    fs::write(agent_dir.join(".claude/session.json"), "{}\n").unwrap();
    fs::write(agent_dir.join(".cache/cache.txt"), "cache\n").unwrap();
    fs::write(agent_dir.join(".venv/python.txt"), "venv\n").unwrap();
    fs::write(agent_dir.join(".npm/npm.txt"), "npm\n").unwrap();
    fs::write(agent_dir.join(".uv/uv.txt"), "uv\n").unwrap();
    fs::write(agent_dir.join("custom-dir/state.txt"), "state\n").unwrap();

    right()
        .args(["--home", home_str, "agent", "backup", "test-agent"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Backup complete:"));

    let backups_dir = home.path().join("backups").join("test-agent");
    let entries: Vec<_> = fs::read_dir(&backups_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "should have exactly one backup");
    let backup_dir = entries[0].path();
    let tar_entries = tar_entries(&backup_dir.join("sandbox.tar.gz"));

    assert!(tar_entries.contains(&"test-agent/.claude/session.json".to_string()));
    assert!(tar_entries.contains(&"test-agent/custom-dir/state.txt".to_string()));
    assert!(!tar_entries.iter().any(|entry| entry.starts_with("test-agent/.cache/")));
    assert!(!tar_entries.iter().any(|entry| entry.starts_with("test-agent/.venv/")));
    assert!(!tar_entries.iter().any(|entry| entry.starts_with("test-agent/.npm/")));
    assert!(!tar_entries.iter().any(|entry| entry.starts_with("test-agent/.uv/")));
}
```

- [ ] **Step 2: Run the default exclude integration test and verify it fails**

Run:

```bash
devenv shell -- cargo test -p right test_agent_backup_excludes_rebuildable_dirs_by_default_no_sandbox
```

Expected: FAIL because the tarball still includes `.cache`, `.venv`, `.npm`, and `.uv`.

- [ ] **Step 3: Add the CLI flag and no-sandbox tar excludes**

In `crates/right/src/main.rs`, update `AgentCommands::Backup`:

```rust
Backup {
    /// Agent name
    name: String,
    /// Only back up sandbox files (skip agent.yaml, data.db, policy.yaml)
    #[arg(long)]
    sandbox_only: bool,
    /// Include rebuildable sandbox dependency/cache directories (.cache, .venv, .npm, .uv)
    #[arg(long)]
    include_rebuildable: bool,
},
```

Update the command match arm:

```rust
AgentCommands::Backup {
    name,
    sandbox_only,
    include_rebuildable,
} => cmd_agent_backup(&home, &name, sandbox_only, include_rebuildable).await,
```

Update the backup function signature:

```rust
async fn cmd_agent_backup(
    home: &Path,
    agent_name: &str,
    sandbox_only: bool,
    include_rebuildable: bool,
) -> miette::Result<()> {
```

Update the sandboxed call:

```rust
right_openshell::openshell::ssh_tar_download(
    &ssh_config,
    &ssh_host,
    "sandbox",
    &dest_tar,
    include_rebuildable,
    300,
)
.await?;
```

In the no-sandbox branch, replace the fixed `.args([...])` call with a mutable argument vector:

```rust
let mut tar_args = vec![
    "czpf".to_string(),
    dest_tar
        .to_str()
        .ok_or_else(|| miette::miette!("non-UTF-8 backup path"))?
        .to_string(),
    "--exclude=data.db".to_string(),
];

if !include_rebuildable {
    for path in right_openshell::openshell::DEFAULT_REBUILDABLE_BACKUP_EXCLUDES {
        tar_args.push(format!("--exclude={agent_name}/{path}"));
        tar_args.push(format!("--exclude={agent_name}/{path}/*"));
    }
}

tar_args.push("-C".to_string());
tar_args.push(
    agent_dir
        .parent()
        .ok_or_else(|| miette::miette!("agent_dir has no parent"))?
        .to_str()
        .ok_or_else(|| miette::miette!("non-UTF-8 agents_dir"))?
        .to_string(),
);
tar_args.push(agent_name.to_string());

let status = std::process::Command::new("tar")
    .args(&tar_args)
    .status()
    .into_diagnostic()
    .map_err(|e| miette::miette!("failed to spawn tar: {e:#}"))?;
```

- [ ] **Step 4: Run the default exclude integration test and verify it passes**

Run:

```bash
devenv shell -- cargo test -p right test_agent_backup_excludes_rebuildable_dirs_by_default_no_sandbox
```

Expected: PASS.

- [ ] **Step 5: Write failing integration test for `--include-rebuildable`**

Add this test after `test_agent_backup_excludes_rebuildable_dirs_by_default_no_sandbox`:

```rust
#[test]
fn test_agent_backup_include_rebuildable_keeps_rebuildable_dirs_no_sandbox() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    let agent_dir = home.path().join("agents").join("test-agent");
    fs::create_dir_all(agent_dir.join(".cache")).unwrap();
    fs::create_dir_all(agent_dir.join(".venv")).unwrap();
    fs::create_dir_all(agent_dir.join(".npm")).unwrap();
    fs::create_dir_all(agent_dir.join(".uv")).unwrap();
    fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
    fs::write(agent_dir.join(".cache/cache.txt"), "cache\n").unwrap();
    fs::write(agent_dir.join(".venv/python.txt"), "venv\n").unwrap();
    fs::write(agent_dir.join(".npm/npm.txt"), "npm\n").unwrap();
    fs::write(agent_dir.join(".uv/uv.txt"), "uv\n").unwrap();

    right()
        .args([
            "--home",
            home_str,
            "agent",
            "backup",
            "test-agent",
            "--include-rebuildable",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Backup complete:"));

    let backups_dir = home.path().join("backups").join("test-agent");
    let entries: Vec<_> = fs::read_dir(&backups_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "should have exactly one backup");
    let backup_dir = entries[0].path();
    let tar_entries = tar_entries(&backup_dir.join("sandbox.tar.gz"));

    assert!(tar_entries.contains(&"test-agent/.cache/cache.txt".to_string()));
    assert!(tar_entries.contains(&"test-agent/.venv/python.txt".to_string()));
    assert!(tar_entries.contains(&"test-agent/.npm/npm.txt".to_string()));
    assert!(tar_entries.contains(&"test-agent/.uv/uv.txt".to_string()));
}
```

- [ ] **Step 6: Run the include-rebuildable integration test and verify it passes**

Run:

```bash
devenv shell -- cargo test -p right test_agent_backup_include_rebuildable_keeps_rebuildable_dirs_no_sandbox
```

Expected: PASS if Step 3 was implemented correctly. If it fails because clap rejects the flag, re-check the `AgentCommands::Backup` field and match arm.

## Task 3: Preserve Restore And Existing Backup Tests

**Files:**
- Modify: `crates/right/tests/cli_integration.rs` if needed
- Modify: `crates/right/src/main.rs` if failures reveal missing no-sandbox restore handling

- [ ] **Step 1: Run existing backup/restore tests**

Run:

```bash
devenv shell -- cargo test -p right test_agent_backup_and_restore_no_sandbox
devenv shell -- cargo test -p right test_agent_backup_sandbox_only
devenv shell -- cargo test -p right test_agent_restore_fails_if_agent_exists
```

Expected: PASS. The existing restore test should still pass because it does not depend on excluded rebuildable dirs.

- [ ] **Step 2: If the existing restore test fails, inspect the tar entries before changing code**

Run only if Step 1 fails:

```bash
devenv shell -- cargo test -p right test_agent_backup_and_restore_no_sandbox -- --nocapture
```

Expected: failure output identifies whether archive shape changed. Do not change restore extraction unless the archive root changed unexpectedly; the intended no-sandbox archive root remains `test-agent/...`.

- [ ] **Step 3: Run focused right-openshell tests again**

Run:

```bash
devenv shell -- cargo test -p right-openshell sandbox_tar_download_args
```

Expected: PASS. This confirms OpenShell tar args still match the approved design after CLI integration work.

## Task 4: Documentation Updates

**Files:**
- Modify: `docs/architecture/lifecycle.md`
- Modify: `ARCHITECTURE.md` if the prescriptive backup invariant is stale

- [ ] **Step 1: Update lifecycle backup flow**

In `docs/architecture/lifecycle.md`, replace:

```text
right agent backup <name> [--sandbox-only]
  - Sandbox mode: SSH tar /sandbox/ -> sandbox.tar.gz
  - No-sandbox mode: tar agent dir -> sandbox.tar.gz
  - Full mode: + agent.yaml, policy.yaml, VACUUM INTO data.db
  - Stored at ~/.right/backups/<agent>/<YYYYMMDD-HHMM>/
```

with:

```text
right agent backup <name> [--sandbox-only] [--include-rebuildable]
  - Sandbox mode: SSH tar /sandbox/ -> sandbox.tar.gz
    - Default excludes: sandbox/.cache, sandbox/.venv, sandbox/.npm, sandbox/.uv
  - --include-rebuildable: include those rebuildable dirs for forensic backup
  - No-sandbox mode: tar agent dir -> sandbox.tar.gz with the same default excludes
  - Full mode: + agent.yaml, policy.yaml, VACUUM INTO data.db
  - Stored at ~/.right/backups/<agent>/<YYYYMMDD-HHMM>/
```

- [ ] **Step 2: Update ARCHITECTURE.md if needed**

If `ARCHITECTURE.md` has a backup invariant that implies `sandbox.tar.gz` always contains every sandbox path, update it to this concise prescriptive form:

```text
- `backups/<agent>/<YYYYMMDD-HHMM>/` - `sandbox.tar.gz` plus optional `agent.yaml` + `data.db` + `policy.yaml` for full backups. `right agent backup` excludes rebuildable sandbox dirs by default (`.cache`, `.venv`, `.npm`, `.uv`); `--include-rebuildable` opts into forensic sandbox archives.
```

Do not add descriptive backup walkthroughs to `ARCHITECTURE.md`; keep detailed flow in `docs/architecture/lifecycle.md`.

- [ ] **Step 3: Verify docs mention the new flag and default excludes**

Run:

```bash
devenv shell -- rg -n "include-rebuildable|\\.cache|\\.venv|\\.npm|\\.uv" docs/architecture/lifecycle.md ARCHITECTURE.md
```

Expected: output includes the lifecycle flow and any updated architecture invariant.

## Task 5: Final Verification

**Files:**
- No edits unless verification finds a defect.

- [ ] **Step 1: Run targeted backup-related tests**

Run:

```bash
devenv shell -- cargo test -p right-openshell sandbox_tar_download_args
devenv shell -- cargo test -p right test_agent_backup_excludes_rebuildable_dirs_by_default_no_sandbox
devenv shell -- cargo test -p right test_agent_backup_include_rebuildable_keeps_rebuildable_dirs_no_sandbox
devenv shell -- cargo test -p right test_agent_backup_and_restore_no_sandbox
devenv shell -- cargo test -p right test_agent_backup_sandbox_only
devenv shell -- cargo test -p right test_agent_restore_fails_if_agent_exists
```

Expected: all commands PASS.

- [ ] **Step 2: Run final full workspace test**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. If there are pre-existing unrelated failures, capture exact failing tests and rerun the targeted backup tests to prove this feature's behavior.

- [ ] **Step 3: Run final workspace build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 4: Inspect final diff**

Run:

```bash
devenv shell -- git diff -- crates/right-openshell/src/openshell.rs crates/right-openshell/src/openshell_tests.rs crates/right/src/main.rs crates/right-agent/src/agent/destroy.rs crates/right/tests/cli_integration.rs docs/architecture/lifecycle.md ARCHITECTURE.md
```

Expected:

- `right agent backup` has `--include-rebuildable`.
- Default sandbox and no-sandbox backup excludes `.cache`, `.venv`, `.npm`, `.uv`.
- Migration and pre-destroy helper calls explicitly pass forensic mode.
- Tests cover default exclude and include flag.
- Docs match behavior.

- [ ] **Step 5: Commit implementation**

Run:

```bash
devenv shell -- git add crates/right-openshell/src/openshell.rs crates/right-openshell/src/openshell_tests.rs crates/right/src/main.rs crates/right-agent/src/agent/destroy.rs crates/right/tests/cli_integration.rs docs/architecture/lifecycle.md ARCHITECTURE.md
devenv shell -- git commit -m "feat(agent): exclude rebuildable dirs from default backups"
```

Expected: commit succeeds after hooks. Do not include unrelated `docs/research/` or unrelated working-tree edits.
