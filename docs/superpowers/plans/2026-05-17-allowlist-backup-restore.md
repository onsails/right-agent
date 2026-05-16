# Allowlist Backup Restore Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Full agent backup, restore, and pre-destroy backup must preserve `allowlist.yaml` so restored cron targets keep their Telegram delivery permissions.

**Architecture:** `allowlist.yaml` is host-side bot-managed control-plane state under `agents/<name>/`, alongside `agent.yaml`, `policy.yaml`, and `data.db`. Treat it as optional for backward compatibility, but include it whenever present in full backups, restore it before bot startup/codegen, and include it in `right agent destroy --backup` safety backups. `--sandbox-only` remains unchanged and must not copy host control-plane files.

**Tech Stack:** Rust 2024, `cargo test`, `devenv shell --`, `tempfile`, `rusqlite`, existing Right Agent backup/restore helpers.

---

## File Structure

- Modify `crates/right/src/main.rs`
  - Extend `copy_agent_backup_config_files()` to copy `allowlist.yaml` when present.
  - Extend `copy_agent_restore_config_files()` to restore `allowlist.yaml` when present.
  - Update `agent backup --sandbox-only` help text so it names `allowlist.yaml`.
  - Add focused unit tests near existing backup/restore helper tests.

- Modify `crates/right/tests/cli_integration.rs`
  - Extend the existing no-sandbox backup/restore integration test to create, back up, and verify `allowlist.yaml`.

- Modify `crates/right-agent/src/agent/destroy.rs`
  - Extend pre-destroy backup to copy `allowlist.yaml` when present.
  - Add a focused unit test for `destroy_agent(..., backup: true)`.

- Modify `ARCHITECTURE.md`
  - Correct the chat ID allowlist path from legacy `agent.yaml` wording to `allowlist.yaml`.
  - List `allowlist.yaml` in key per-agent state and full backup contents.

- Modify `docs/architecture/lifecycle.md`
  - Update backup/restore lifecycle notes to include `allowlist.yaml`.

---

### Task 1: Unit Coverage for Full Backup/Restore Config Files

**Files:**
- Modify: `crates/right/src/main.rs`

- [ ] **Step 1: Add failing unit tests**

In `crates/right/src/main.rs`, inside the existing `#[cfg(test)] mod tests`, add these tests after `backup_config_files_include_custom_sandbox_policy_file()`:

```rust
    #[test]
    fn backup_config_files_include_allowlist_yaml() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("allowlisted-agent");
        let backup_dir = tmp.path().join("backups").join("allowlisted-agent");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: openshell\n").unwrap();
        let allowlist = "\
version: 1
users:
  - id: 111
    label: alice
    added_by: null
    added_at: 2026-05-16T12:00:00Z
groups:
  - id: -222
    label: ops
    opened_by: null
    opened_at: 2026-05-16T12:00:00Z
";
        fs::write(agent_dir.join("allowlist.yaml"), allowlist).unwrap();

        let config = right_agent::agent::discovery::parse_agent_config(&agent_dir)
            .unwrap()
            .unwrap();
        copy_agent_backup_config_files(&agent_dir, &backup_dir, Some(&config)).unwrap();

        assert_eq!(
            fs::read_to_string(backup_dir.join("allowlist.yaml")).unwrap(),
            allowlist,
            "full backup must include bot-managed allowlist.yaml"
        );
    }
```

In the same module, add this test after `restore_config_files_copy_custom_sandbox_policy_before_codegen()`:

```rust
    #[test]
    fn restore_config_files_copy_allowlist_yaml() {
        let tmp = TempDir::new().unwrap();
        let backup_dir = tmp.path().join("backup");
        let agent_dir = tmp.path().join("agents").join("restored-agent");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(backup_dir.join("agent.yaml"), "sandbox:\n  mode: openshell\n").unwrap();
        fs::write(backup_dir.join("data.db"), "db").unwrap();
        let allowlist = "\
version: 1
users:
  - id: 333
    label: bob
    added_by: null
    added_at: 2026-05-16T12:00:00Z
groups:
  - id: -444
    label: product
    opened_by: null
    opened_at: 2026-05-16T12:00:00Z
";
        fs::write(backup_dir.join("allowlist.yaml"), allowlist).unwrap();

        let config = right_agent::agent::discovery::parse_agent_config(&backup_dir)
            .unwrap()
            .unwrap();
        copy_agent_restore_config_files(&backup_dir, &agent_dir, &config).unwrap();

        assert_eq!(
            fs::read_to_string(agent_dir.join("allowlist.yaml")).unwrap(),
            allowlist,
            "restore must materialize allowlist.yaml before bot startup"
        );
    }
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
devenv shell -- cargo test -p right allowlist_yaml
```

Expected: FAIL. `backup_config_files_include_allowlist_yaml` fails because `allowlist.yaml` is not copied into the backup dir, and `restore_config_files_copy_allowlist_yaml` fails because `allowlist.yaml` is not restored into the agent dir.

- [ ] **Step 3: Implement the minimal backup/restore helper change**

In `crates/right/src/main.rs`, replace `copy_agent_backup_config_files()` with:

```rust
fn copy_agent_backup_config_files(
    agent_dir: &Path,
    backup_dir: &Path,
    config: Option<&right_agent::agent::types::AgentConfig>,
) -> miette::Result<()> {
    for filename in ["agent.yaml", "policy.yaml", "allowlist.yaml"] {
        let rel = Path::new(filename);
        if copy_agent_file_if_exists(agent_dir, backup_dir, rel)? {
            println!("{filename} copied");
        }
    }

    if let Some(policy_file) = custom_sandbox_policy_file(config)? {
        copy_required_agent_file(agent_dir, backup_dir, &policy_file)?;
        println!("{} copied", policy_file.display());
    }

    Ok(())
}
```

In `crates/right/src/main.rs`, replace `copy_agent_restore_config_files()` with:

```rust
fn copy_agent_restore_config_files(
    backup_dir: &Path,
    agent_dir: &Path,
    config: &right_agent::agent::types::AgentConfig,
) -> miette::Result<()> {
    for filename in ["agent.yaml", "policy.yaml", "allowlist.yaml", "data.db"] {
        let rel = Path::new(filename);
        if copy_agent_file_if_exists(backup_dir, agent_dir, rel)? {
            println!("{filename} restored");
        }
    }

    if let Some(policy_file) = custom_sandbox_policy_file(Some(config))? {
        copy_required_agent_file(backup_dir, agent_dir, &policy_file)?;
        println!("{} restored", policy_file.display());
    }

    Ok(())
}
```

- [ ] **Step 4: Run tests and verify they pass**

Run:

```bash
devenv shell -- cargo test -p right allowlist_yaml
```

Expected: PASS, including both new allowlist helper tests.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "fix(backup): preserve allowlist in backup restore"
```

---

### Task 2: CLI Integration Coverage for No-Sandbox Backup/Restore

**Files:**
- Modify: `crates/right/tests/cli_integration.rs`

- [ ] **Step 1: Extend the existing integration test**

In `crates/right/tests/cli_integration.rs`, inside `test_agent_backup_and_restore_no_sandbox()`, add this block after the existing `policy.yaml` write:

```rust
    let allowlist = "\
version: 1
users:
  - id: 111
    label: alice
    added_by: null
    added_at: 2026-05-16T12:00:00Z
groups:
  - id: -222
    label: ops
    opened_by: null
    opened_at: 2026-05-16T12:00:00Z
";
    fs::write(agent_dir.join("allowlist.yaml"), allowlist).unwrap();
```

In the backup command assertion in the same test, add an allowlist stdout assertion:

```rust
        .stdout(predicate::str::contains("allowlist.yaml"));
```

The full assertion chain should become:

```rust
    right()
        .args(["--home", home_str, "agent", "backup", "test-agent"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sandbox.tar.gz"))
        .stdout(predicate::str::contains("agent.yaml"))
        .stdout(predicate::str::contains("allowlist.yaml"))
        .stdout(predicate::str::contains("data.db"));
```

In the backup contents assertions, add:

```rust
    assert!(
        backup_dir.join("allowlist.yaml").exists(),
        "should have allowlist.yaml"
    );
    assert_eq!(
        fs::read_to_string(backup_dir.join("allowlist.yaml")).unwrap(),
        allowlist,
        "backup must preserve allowlist.yaml content"
    );
```

In the restored files assertions, add:

```rust
    assert_eq!(
        fs::read_to_string(restored_dir.join("allowlist.yaml")).unwrap(),
        allowlist,
        "restore must preserve allowlist.yaml content"
    );
```

- [ ] **Step 2: Run the integration test**

Run:

```bash
devenv shell -- cargo test -p right --test cli_integration test_agent_backup_and_restore_no_sandbox -- --exact
```

Expected: PASS after Task 1. If this fails, the CLI path is bypassing `copy_agent_backup_config_files()` or `copy_agent_restore_config_files()` and must be traced before proceeding.

- [ ] **Step 3: Commit**

```bash
git add crates/right/tests/cli_integration.rs
git commit -m "test(backup): cover allowlist restore through cli"
```

---

### Task 3: Pre-Destroy Backup Coverage

**Files:**
- Modify: `crates/right-agent/src/agent/destroy.rs`

- [ ] **Step 1: Add a failing pre-destroy backup test**

In `crates/right-agent/src/agent/destroy.rs`, inside `#[cfg(test)] mod tests`, add this test after `destroy_with_backup_creates_backup_dir()`:

```rust
    #[tokio::test]
    async fn destroy_with_backup_copies_allowlist_yaml() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        let agents_dir = home.join("agents").join("backup-allowlist");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
        let allowlist = "\
version: 1
users:
  - id: 111
    label: alice
    added_by: null
    added_at: 2026-05-16T12:00:00Z
groups:
  - id: -222
    label: ops
    opened_by: null
    opened_at: 2026-05-16T12:00:00Z
";
        std::fs::write(agents_dir.join("allowlist.yaml"), allowlist).unwrap();

        let options = DestroyOptions {
            agent_name: "backup-allowlist".into(),
            backup: true,
        };

        let result = destroy_agent(home, &options).await.unwrap();
        let backup_path = result.backup_path.expect("backup path must be recorded");

        assert_eq!(
            std::fs::read_to_string(backup_path.join("allowlist.yaml")).unwrap(),
            allowlist,
            "pre-destroy backup must preserve allowlist.yaml outside sandbox.tar.gz"
        );
    }
```

- [ ] **Step 2: Run test and verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-agent destroy_with_backup_copies_allowlist_yaml --lib
```

Expected: FAIL because `backup_path/allowlist.yaml` does not exist.

- [ ] **Step 3: Implement the minimal pre-destroy backup change**

In `crates/right-agent/src/agent/destroy.rs`, update the doc comment above `run_backup()` from:

```rust
/// Always copies agent.yaml, policy.yaml, and VACUUM-copies data.db.
```

to:

```rust
/// Always copies agent.yaml, policy.yaml, allowlist.yaml, and VACUUM-copies data.db.
```

In the same file, replace this loop:

```rust
    for filename in &["agent.yaml", "policy.yaml"] {
        let src = agent_dir.join(filename);
        if src.exists() {
            std::fs::copy(&src, backup_dir.join(filename))
                .map_err(|e| miette::miette!("failed to copy {filename}: {e:#}"))?;
        }
    }
```

with:

```rust
    for filename in &["agent.yaml", "policy.yaml", "allowlist.yaml"] {
        let src = agent_dir.join(filename);
        if src.exists() {
            std::fs::copy(&src, backup_dir.join(filename))
                .map_err(|e| miette::miette!("failed to copy {filename}: {e:#}"))?;
        }
    }
```

- [ ] **Step 4: Run test and verify it passes**

Run:

```bash
devenv shell -- cargo test -p right-agent destroy_with_backup_copies_allowlist_yaml --lib
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/agent/destroy.rs
git commit -m "fix(destroy): preserve allowlist in safety backups"
```

---

### Task 4: Help Text and Architecture Docs

**Files:**
- Modify: `crates/right/src/main.rs`
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/lifecycle.md`

- [ ] **Step 1: Update CLI help text**

In `crates/right/src/main.rs`, replace the `Backup::sandbox_only` doc comment:

```rust
        /// Only back up sandbox files (skip agent.yaml, data.db, policy.yaml)
```

with:

```rust
        /// Only back up sandbox files (skip agent.yaml, data.db, policy.yaml, allowlist.yaml)
```

- [ ] **Step 2: Update `ARCHITECTURE.md` allowlist ownership**

In `ARCHITECTURE.md`, replace:

```markdown
- **Chat ID allowlist**: Empty = block all (secure default); per-agent in agent.yaml
```

with:

```markdown
- **Chat ID allowlist**: Empty = block all (secure default); per-agent in `agents/<name>/allowlist.yaml`. Legacy `agent.yaml::allowed_chat_ids` is migration input only.
```

In `ARCHITECTURE.md`, replace the `agents/<name>/` bullet under `~/.right/` paths. The existing bullet starts with:

```markdown
- `agents/<name>/` - per-agent state. Key files: `agent.yaml`, ...
```

Use this full replacement text:

```markdown
- `agents/<name>/` - per-agent state. Key files: `agent.yaml`, `allowlist.yaml`, `policy.yaml`, `data.db`, `.claude/.credentials.json` (symlink to `~/.claude/.credentials.json`, host-only - NOT uploaded to sandbox). Subdirs include `crons/`, `inbox/`, `outbox/`, and `tmp` for staging during attachment transfer. Sandbox-internal: `/sandbox/.claude/projects/-sandbox/<sid>.jsonl` (CC project history, agent-readable for self-introspection via the `/right-reflect` skill); `/sandbox/.claude/logs/<sid>.log` (CC debug output, only present when `/debug` is on).
```

In `ARCHITECTURE.md`, replace the `backups/<agent>/` bullet. The existing bullet starts with:

```markdown
- `backups/<agent>/<YYYYMMDD-HHMM>/` - `sandbox.tar.gz` plus optional `agent.yaml` + ...
```

Use this full replacement text:

```markdown
- `backups/<agent>/<YYYYMMDD-HHMM>/` - `sandbox.tar.gz` plus optional `agent.yaml` + `allowlist.yaml` + `data.db` + `policy.yaml` for full backups. `right agent backup` excludes rebuildable sandbox dirs by default (`.cache`, `.venv`, `.npm`, `.uv`); `--include-rebuildable` opts into forensic sandbox archives.
```

- [ ] **Step 3: Update lifecycle docs**

In `docs/architecture/lifecycle.md`, replace:

```markdown
  |- Full mode: + agent.yaml, policy.yaml, VACUUM INTO data.db
```

with:

```markdown
  |- Full mode: + agent.yaml, allowlist.yaml, policy.yaml, VACUUM INTO data.db
```

In the restore flow in `docs/architecture/lifecycle.md`, replace:

```markdown
  |- Restore config files to new agent dir
```

with:

```markdown
  |- Restore config/control-plane files to new agent dir (agent.yaml, allowlist.yaml, policy.yaml, data.db when present)
```

- [ ] **Step 4: Run doc-adjacent checks**

Run:

```bash
rg -n "allowlist|sandbox-only|Full mode|backups/<agent>" crates/right/src/main.rs ARCHITECTURE.md docs/architecture/lifecycle.md
```

Expected: output shows:

```text
crates/right/src/main.rs:...Only back up sandbox files (skip agent.yaml, data.db, policy.yaml, allowlist.yaml)
ARCHITECTURE.md:...per-agent in `agents/<name>/allowlist.yaml`
ARCHITECTURE.md:...`agent.yaml`, `allowlist.yaml`, `policy.yaml`, `data.db`
ARCHITECTURE.md:...`agent.yaml` + `allowlist.yaml` + `data.db` + `policy.yaml`
docs/architecture/lifecycle.md:...Full mode: + agent.yaml, allowlist.yaml, policy.yaml, VACUUM INTO data.db
docs/architecture/lifecycle.md:...agent.yaml, allowlist.yaml, policy.yaml, data.db when present
```

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/main.rs ARCHITECTURE.md docs/architecture/lifecycle.md
git commit -m "docs(backup): document allowlist backup coverage"
```

---

### Task 5: Final Verification

**Files:**
- No source edits.

- [ ] **Step 1: Run targeted helper tests**

Run:

```bash
devenv shell -- cargo test -p right allowlist_yaml
```

Expected: PASS.

- [ ] **Step 2: Run CLI integration coverage**

Run:

```bash
devenv shell -- cargo test -p right --test cli_integration test_agent_backup_and_restore_no_sandbox -- --exact
```

Expected: PASS.

- [ ] **Step 3: Run pre-destroy backup coverage**

Run:

```bash
devenv shell -- cargo test -p right-agent destroy_with_backup_copies_allowlist_yaml --lib
```

Expected: PASS.

- [ ] **Step 4: Run full workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. Ignored tests stay ignored unless explicitly enabled.

- [ ] **Step 5: Run full workspace build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 6: Run diff hygiene check**

Run:

```bash
git diff --check
```

Expected: no output and exit code 0.

- [ ] **Step 7: Commit any final fixups**

If Task 5 required changes, commit them:

```bash
git add crates/right/src/main.rs crates/right/tests/cli_integration.rs crates/right-agent/src/agent/destroy.rs ARCHITECTURE.md docs/architecture/lifecycle.md
git commit -m "fix(backup): complete allowlist backup restore coverage"
```

If Task 5 required no changes, do not create an empty commit.

---

## Self-Review

**Spec coverage:** The plan covers normal full backup, restore from backup, pre-destroy safety backup, CLI end-to-end no-sandbox restore, CLI help, and architecture docs. `--sandbox-only` remains unchanged and documented as skipping `allowlist.yaml`.

**Placeholder scan:** No steps use TBD, TODO, "similar to", or unspecified tests. Each code change includes exact snippets.

**Type consistency:** The plan uses existing functions and paths: `copy_agent_backup_config_files`, `copy_agent_restore_config_files`, `destroy_agent`, `allowlist.yaml`, `devenv shell -- cargo test`, and existing test modules.
