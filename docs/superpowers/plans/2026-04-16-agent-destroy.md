# Agent Destroy Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `rightclaw agent destroy <name>` command that cleanly tears down an agent — stops its process, optionally backs up, deletes sandbox (if sandboxed), removes agent directory, and reloads process-compose.

**Architecture:** Core destroy logic in `crates/rightclaw/src/agent/destroy.rs` (library crate, no TTY). Interactive TUI in CLI crate's `cmd_agent_destroy()`. The core function takes resolved options and returns a result struct. The CLI handles prompts via `inquire` with red-styled final confirmation.

**Tech Stack:** Rust, inquire (0.9 — `RenderConfig` for red prompt), miette, tokio, existing `PcClient`, `delete_sandbox`, `discover_agents`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `crates/rightclaw/src/agent/destroy.rs` | Core `destroy_agent()` — stop, backup, sandbox delete, dir remove, PC reload |
| Modify | `crates/rightclaw/src/agent/mod.rs` | Add `pub mod destroy;` and re-export |
| Modify | `crates/rightclaw-cli/src/main.rs` | Add `Destroy` variant to `AgentCommands`, `cmd_agent_destroy()` handler with interactive TUI |
| Modify | `crates/rightclaw-cli/tests/cli_integration.rs` | Integration tests for destroy command |

---

### Task 1: Core destroy types and module skeleton

**Files:**
- Create: `crates/rightclaw/src/agent/destroy.rs`
- Modify: `crates/rightclaw/src/agent/mod.rs`

- [ ] **Step 1: Create `destroy.rs` with types and empty function**

```rust
// crates/rightclaw/src/agent/destroy.rs
use std::path::{Path, PathBuf};

/// Options for destroying an agent (resolved by caller — no TTY interaction).
pub struct DestroyOptions {
    pub agent_name: String,
    pub backup: bool,
    pub pc_port: u16,
}

/// Result of a destroy operation — booleans reflect what actually happened.
pub struct DestroyResult {
    /// Whether the agent process was stopped via process-compose.
    pub agent_stopped: bool,
    /// Whether an OpenShell sandbox was deleted.
    pub sandbox_deleted: bool,
    /// Path to backup if one was created.
    pub backup_path: Option<PathBuf>,
    /// Whether the agent directory was removed.
    pub dir_removed: bool,
    /// Whether process-compose was reloaded.
    pub pc_reloaded: bool,
}

/// Destroy an agent: stop process, optionally backup, delete sandbox, remove directory, reload PC.
///
/// Non-fatal steps (stop, sandbox delete, PC reload) warn and continue.
/// Fatal steps (backup if requested, directory removal) propagate errors.
pub async fn destroy_agent(home: &Path, options: &DestroyOptions) -> miette::Result<DestroyResult> {
    todo!()
}
```

- [ ] **Step 2: Register module in `agent/mod.rs`**

Add to `crates/rightclaw/src/agent/mod.rs`:

```rust
pub mod destroy;
pub mod discovery;
pub mod types;

pub use destroy::{DestroyOptions, DestroyResult, destroy_agent};
pub use discovery::{discover_agents, discover_single_agent, parse_agent_config, validate_agent_name};
pub use types::{AgentConfig, AgentDef, RestartPolicy, SandboxConfig, SandboxMode};
```

- [ ] **Step 3: Verify it compiles**

Run: `cd /Users/user/dev/rightclaw && cargo check --workspace`
Expected: compiles (todo! is fine — not called yet)

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/agent/destroy.rs crates/rightclaw/src/agent/mod.rs
git commit -m "feat(agent): add destroy module skeleton with types"
```

---

### Task 2: Implement backup helper in destroy module

**Files:**
- Modify: `crates/rightclaw/src/agent/destroy.rs`

The backup logic in `cmd_agent_backup` is CLI-only. We need a subset in the core crate. For sandboxed agents, sandbox backup requires a running sandbox (SSH tar) — if the sandbox isn't ready, skip sandbox tar and only backup config files. For non-sandboxed agents, tar the agent directory.

- [ ] **Step 1: Implement `run_backup()` and `try_sandbox_backup()`**

Add these functions above `destroy_agent` in `destroy.rs`:

```rust
use crate::agent::types::AgentConfig;

/// Run a pre-destroy backup. Returns the backup directory path.
///
/// For non-sandboxed agents: tars the agent directory (excluding data.db).
/// For sandboxed agents: attempts SSH tar of sandbox, falls back to config-only backup.
/// Always copies agent.yaml, policy.yaml, and VACUUM-copies data.db.
async fn run_backup(
    home: &Path,
    agent_name: &str,
    agent_dir: &Path,
    config: &Option<AgentConfig>,
    is_sandboxed: bool,
) -> miette::Result<PathBuf> {
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let backup_dir = crate::config::backups_dir(home, agent_name).join(&timestamp);
    std::fs::create_dir_all(&backup_dir).map_err(|e| {
        miette::miette!("failed to create backup dir {}: {e:#}", backup_dir.display())
    })?;

    tracing::info!(agent = agent_name, backup_dir = %backup_dir.display(), "starting pre-destroy backup");

    if is_sandboxed {
        // Try SSH tar download from sandbox; skip if sandbox not ready
        let sandbox_backed_up = try_sandbox_backup(home, agent_name, config, &backup_dir).await;
        if !sandbox_backed_up {
            tracing::warn!(agent = agent_name, "sandbox not available for backup — backing up config files only");
        }
    } else {
        // Non-sandboxed: tar the agent dir (excluding data.db — backed up separately)
        let dest_tar = backup_dir.join("sandbox.tar.gz");
        let parent = agent_dir.parent().ok_or_else(|| miette::miette!("agent_dir has no parent"))?;
        let status = tokio::process::Command::new("tar")
            .args([
                "czpf",
                dest_tar.to_str().ok_or_else(|| miette::miette!("non-UTF-8 backup path"))?,
                "--exclude=data.db",
                "-C",
                parent.to_str().ok_or_else(|| miette::miette!("non-UTF-8 agents_dir"))?,
                agent_name,
            ])
            .status()
            .await
            .map_err(|e| miette::miette!("failed to spawn tar: {e:#}"))?;
        if !status.success() {
            return Err(miette::miette!("tar exited with status {status}"));
        }
    }

    // Copy config files
    for filename in &["agent.yaml", "policy.yaml"] {
        let src = agent_dir.join(filename);
        if src.exists() {
            std::fs::copy(&src, backup_dir.join(filename)).map_err(|e| {
                miette::miette!("failed to copy {filename}: {e:#}")
            })?;
        }
    }

    // VACUUM data.db if it exists
    let db_path = agent_dir.join("data.db");
    if db_path.exists() {
        let backup_db = backup_dir.join("data.db");
        let db_display = db_path.display().to_string();
        let backup_display = backup_db.display().to_string().replace('\'', "''");
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| {
            miette::miette!("failed to open {}: {e:#}", db_display)
        })?;
        conn.execute(&format!("VACUUM INTO '{backup_display}'"), []).map_err(|e| {
            miette::miette!("VACUUM INTO failed: {e:#}")
        })?;
    }

    tracing::info!(backup_dir = %backup_dir.display(), "pre-destroy backup complete");
    Ok(backup_dir)
}

/// Attempt to SSH-tar the sandbox contents. Returns true if successful.
async fn try_sandbox_backup(
    home: &Path,
    agent_name: &str,
    config: &Option<AgentConfig>,
    backup_dir: &Path,
) -> bool {
    let sb_name = config
        .as_ref()
        .map(|c| crate::openshell::resolve_sandbox_name(agent_name, c))
        .unwrap_or_else(|| crate::openshell::sandbox_name(agent_name));

    // Check OpenShell availability
    let mtls_dir = match crate::openshell::preflight_check() {
        crate::openshell::OpenShellStatus::Ready(dir) => dir,
        _ => return false,
    };

    // Check sandbox readiness
    let mut grpc = match crate::openshell::connect_grpc(&mtls_dir).await {
        Ok(g) => g,
        Err(_) => return false,
    };
    let ready = match crate::openshell::is_sandbox_ready(&mut grpc, &sb_name).await {
        Ok(r) => r,
        Err(_) => return false,
    };
    if !ready {
        return false;
    }

    let ssh_config = home.join("run").join("ssh").join(format!("{sb_name}.ssh-config"));
    if !ssh_config.exists() {
        return false;
    }

    let ssh_host = crate::openshell::ssh_host_for_sandbox(&sb_name);
    let dest_tar = backup_dir.join("sandbox.tar.gz");

    crate::openshell::ssh_tar_download(&ssh_config, &ssh_host, "sandbox", &dest_tar, 300)
        .await
        .is_ok()
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /Users/user/dev/rightclaw && cargo check -p rightclaw`
Expected: compiles (functions are private, only called by `destroy_agent` which still has `todo!()`)

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/agent/destroy.rs
git commit -m "feat(agent): add backup helper for destroy flow"
```

---

### Task 3: Implement core `destroy_agent()` logic

**Files:**
- Modify: `crates/rightclaw/src/agent/destroy.rs`

- [ ] **Step 1: Write unit tests**

Add to end of `crates/rightclaw/src/agent/destroy.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn destroy_nonsandboxed_agent_removes_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        // Create a minimal agent directory with agent.yaml (mode: none)
        let agents_dir = home.join("agents").join("test-agent");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("agent.yaml"),
            "sandbox:\n  mode: none\n",
        )
        .unwrap();

        let options = DestroyOptions {
            agent_name: "test-agent".into(),
            backup: false,
            // Use a port that won't be listening — PC unreachable is a valid state
            pc_port: 19999,
        };

        let result = destroy_agent(home, &options).await.unwrap();

        assert!(!result.agent_stopped, "PC not running, should not have stopped");
        assert!(!result.sandbox_deleted, "non-sandboxed agent, no sandbox to delete");
        assert!(result.backup_path.is_none());
        assert!(result.dir_removed);
        assert!(!result.pc_reloaded, "PC not running, should not have reloaded");
        assert!(!agents_dir.exists(), "agent dir should be deleted");
    }

    #[tokio::test]
    async fn destroy_nonexistent_agent_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("agents")).unwrap();

        let options = DestroyOptions {
            agent_name: "nonexistent".into(),
            backup: false,
            pc_port: 19999,
        };

        let result = destroy_agent(home, &options).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn destroy_with_backup_creates_backup_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        let agents_dir = home.join("agents").join("backup-test");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("agent.yaml"),
            "sandbox:\n  mode: none\n",
        )
        .unwrap();
        // Create a file so tar has something to archive
        std::fs::write(agents_dir.join("AGENTS.md"), "# Test agent").unwrap();

        let options = DestroyOptions {
            agent_name: "backup-test".into(),
            backup: true,
            pc_port: 19999,
        };

        let result = destroy_agent(home, &options).await.unwrap();

        assert!(result.backup_path.is_some(), "backup should have been created");
        let backup_path = result.backup_path.unwrap();
        assert!(backup_path.exists(), "backup dir should exist");
        assert!(backup_path.join("sandbox.tar.gz").exists(), "sandbox.tar.gz should exist");
        assert!(result.dir_removed, "agent dir should be removed after backup");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/user/dev/rightclaw && cargo test -p rightclaw --lib agent::destroy::tests -- --nocapture`
Expected: FAIL (todo! panics)

- [ ] **Step 3: Implement `destroy_agent()`**

Replace the `todo!()` body in `destroy_agent` with the full implementation:

```rust
pub async fn destroy_agent(home: &Path, options: &DestroyOptions) -> miette::Result<DestroyResult> {
    let agents_dir = crate::config::agents_dir(home);
    let agent_dir = agents_dir.join(&options.agent_name);

    if !agent_dir.exists() {
        return Err(miette::miette!(
            "Agent '{}' not found at {}",
            options.agent_name,
            agent_dir.display(),
        ));
    }

    let config = crate::agent::parse_agent_config(&agent_dir)?;
    let is_sandboxed = config
        .as_ref()
        .map(|c| c.is_sandboxed())
        .unwrap_or(true);

    let mut result = DestroyResult {
        agent_stopped: false,
        sandbox_deleted: false,
        backup_path: None,
        dir_removed: false,
        pc_reloaded: false,
    };

    // Step 1: Stop agent via process-compose (non-fatal)
    let pc_client = crate::runtime::PcClient::new(options.pc_port)?;
    let pc_running = pc_client.health_check().await.is_ok();

    if pc_running {
        let process_name = format!("{}-bot", options.agent_name);
        match pc_client.stop_process(&process_name).await {
            Ok(()) => {
                tracing::info!(agent = %options.agent_name, "stopped agent process");
                result.agent_stopped = true;
            }
            Err(e) => {
                tracing::warn!(agent = %options.agent_name, error = format!("{e:#}"), "failed to stop agent process (may already be stopped)");
            }
        }
    }

    // Step 2: Backup (fatal if requested)
    if options.backup {
        let backup_path = run_backup(home, &options.agent_name, &agent_dir, &config, is_sandboxed).await?;
        result.backup_path = Some(backup_path);
    }

    // Step 3: Delete sandbox (non-fatal, sandboxed agents only)
    if is_sandboxed {
        let sb_name = config
            .as_ref()
            .map(|c| crate::openshell::resolve_sandbox_name(&options.agent_name, c))
            .unwrap_or_else(|| crate::openshell::sandbox_name(&options.agent_name));
        crate::openshell::delete_sandbox(&sb_name).await;
        result.sandbox_deleted = true;
    }

    // Step 4: Remove agent directory (fatal)
    std::fs::remove_dir_all(&agent_dir).map_err(|e| {
        miette::miette!(
            "failed to remove agent directory {}: {e:#}",
            agent_dir.display(),
        )
    })?;
    result.dir_removed = true;
    tracing::info!(agent = %options.agent_name, dir = %agent_dir.display(), "removed agent directory");

    // Step 5: Reload process-compose (non-fatal)
    if pc_running {
        // Regenerate process-compose.yaml — agent is no longer discovered
        let all_agents = crate::agent::discover_agents(&agents_dir)?;
        let self_exe = std::env::current_exe().map_err(|e| {
            miette::miette!("failed to resolve current executable path: {e:#}")
        })?;
        crate::codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

        match pc_client.reload_configuration().await {
            Ok(()) => {
                tracing::info!("reloaded process-compose configuration");
                result.pc_reloaded = true;
            }
            Err(e) => {
                tracing::warn!(error = format!("{e:#}"), "failed to reload process-compose (non-fatal)");
            }
        }
    }

    Ok(result)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Users/user/dev/rightclaw && cargo test -p rightclaw --lib agent::destroy::tests -- --nocapture`
Expected: all 3 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/agent/destroy.rs
git commit -m "feat(agent): implement core destroy_agent logic"
```

---

### Task 4: Add CLI `Destroy` subcommand with interactive TUI

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`

- [ ] **Step 1: Add `Destroy` variant to `AgentCommands` enum**

In `crates/rightclaw-cli/src/main.rs`, add after the `Backup` variant in the `AgentCommands` enum:

```rust
    /// Destroy an agent (stop, optionally backup, delete sandbox and files)
    Destroy {
        /// Agent name
        name: String,
        /// Create backup before destroying
        #[arg(long)]
        backup: bool,
        /// Skip interactive prompts
        #[arg(long)]
        force: bool,
    },
```

- [ ] **Step 2: Add dispatch in the match block**

In the `Commands::Agent { command }` match block, after the `Backup` arm (around line 425), add:

```rust
            AgentCommands::Destroy { name, backup, force } => {
                cmd_agent_destroy(&home, &name, backup, force).await
            }
```

- [ ] **Step 3: Implement `cmd_agent_destroy()`**

Add this function in `main.rs` (near the other `cmd_agent_*` functions):

```rust
async fn cmd_agent_destroy(home: &Path, agent_name: &str, backup_flag: bool, force: bool) -> miette::Result<()> {
    use inquire::ui::{Color, RenderConfig, Styled};

    // Validate agent exists
    let agents_dir = rightclaw::config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);
    if !agent_dir.exists() {
        return Err(miette::miette!("Agent '{}' not found", agent_name));
    }

    let config = rightclaw::agent::parse_agent_config(&agent_dir)?;
    let is_sandboxed = config.as_ref().map(|c| c.is_sandboxed()).unwrap_or(true);

    let do_backup = if force {
        backup_flag
    } else {
        // Show summary of what will be destroyed
        println!("Agent: {agent_name}");
        println!("  Directory: {}", agent_dir.display());
        if let Ok(size) = dir_size(&agent_dir) {
            println!("  Size: {}", format_bytes(size));
        }
        if is_sandboxed {
            let sb_name = config
                .as_ref()
                .map(|c| rightclaw::openshell::resolve_sandbox_name(agent_name, c))
                .unwrap_or_else(|| rightclaw::openshell::sandbox_name(agent_name));
            println!("  Sandbox: {sb_name}");
        } else {
            println!("  Sandbox: none");
        }
        let db_path = agent_dir.join("data.db");
        if db_path.exists() {
            if let Ok(meta) = std::fs::metadata(&db_path) {
                println!("  data.db: {}", format_bytes(meta.len()));
            }
        }

        // Check if PC is running and agent is active
        let pc_client = rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT)?;
        if pc_client.health_check().await.is_ok() {
            println!("  Process: running (will be stopped)");
        } else {
            println!("  Process: not running");
        }

        println!();

        // Backup prompt
        let do_backup = if backup_flag {
            true
        } else {
            inquire::Confirm::new("Create backup before destroying?")
                .with_default(false)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?
        };

        // Final confirmation — red styled
        let red_config = RenderConfig::default()
            .with_prompt_prefix(Styled::new("⚠").with_fg(Color::LightRed));

        let confirmed = inquire::Confirm::new(&format!(
            "Permanently destroy agent '{agent_name}'? This cannot be undone."
        ))
        .with_default(false)
        .with_render_config(red_config)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if !confirmed {
            println!("Aborted.");
            return Ok(());
        }

        do_backup
    };

    let options = rightclaw::agent::DestroyOptions {
        agent_name: agent_name.to_string(),
        backup: do_backup,
        pc_port: rightclaw::runtime::PC_PORT,
    };

    let result = rightclaw::agent::destroy_agent(home, &options).await?;

    // Print summary
    println!();
    println!("Destroyed agent '{agent_name}':");
    if result.agent_stopped {
        println!("  ✓ Stopped process");
    }
    if let Some(ref path) = result.backup_path {
        println!("  ✓ Backup saved to {}", path.display());
    }
    if result.sandbox_deleted {
        println!("  ✓ Deleted sandbox");
    }
    if result.dir_removed {
        println!("  ✓ Removed agent directory");
    }
    if result.pc_reloaded {
        println!("  ✓ Reloaded process-compose");
    }

    Ok(())
}

fn dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in walkdir::WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(total)
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cd /Users/user/dev/rightclaw && cargo check --workspace`
Expected: compiles successfully

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat(cli): add agent destroy command with interactive TUI"
```

---

### Task 5: Integration tests

**Files:**
- Modify: `crates/rightclaw-cli/tests/cli_integration.rs`

- [ ] **Step 1: Write integration test — destroy nonexistent agent**

Add to `crates/rightclaw-cli/tests/cli_integration.rs`:

```rust
#[test]
fn test_destroy_nonexistent_agent() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create home structure but no agent
    std::fs::create_dir_all(dir.path().join("agents")).unwrap();

    rightclaw()
        .args(["--home", home, "agent", "destroy", "nonexistent", "--force"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}
```

- [ ] **Step 2: Write integration test — destroy with `--force`**

```rust
#[test]
fn test_destroy_agent_force() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create an agent via init first
    rightclaw()
        .args(["--home", home, "init", "-y", "--sandbox-mode", "none", "--tunnel-hostname", "test.example.com"])
        .assert()
        .success();

    // Verify agent exists
    assert!(dir.path().join("agents/right").exists());

    // Destroy with --force (no TTY prompts)
    rightclaw()
        .args(["--home", home, "agent", "destroy", "right", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Destroyed agent"));

    // Verify agent directory is gone
    assert!(!dir.path().join("agents/right").exists());
}
```

- [ ] **Step 3: Write integration test — destroy with `--force --backup`**

```rust
#[test]
fn test_destroy_agent_force_with_backup() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "init", "-y", "--sandbox-mode", "none", "--tunnel-hostname", "test.example.com"])
        .assert()
        .success();

    rightclaw()
        .args(["--home", home, "agent", "destroy", "right", "--force", "--backup"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Backup saved to"))
        .stdout(predicate::str::contains("Destroyed agent"));

    assert!(!dir.path().join("agents/right").exists(), "agent dir should be removed");
    assert!(dir.path().join("backups/right").exists(), "backup dir should exist");
}
```

- [ ] **Step 4: Write integration test — help output lists destroy**

```rust
#[test]
fn test_help_lists_destroy() {
    rightclaw()
        .args(["agent", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("destroy"));
}
```

- [ ] **Step 5: Run all tests**

Run: `cd /Users/user/dev/rightclaw && cargo test -p rightclaw-cli --test cli_integration -- --nocapture`
Expected: all new tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/tests/cli_integration.rs
git commit -m "test(cli): add integration tests for agent destroy"
```

---

### Task 6: Final build and verify

- [ ] **Step 1: Run full workspace build**

Run: `cd /Users/user/dev/rightclaw && cargo build --workspace`
Expected: builds successfully

- [ ] **Step 2: Run full test suite**

Run: `cd /Users/user/dev/rightclaw && cargo test --workspace`
Expected: all tests pass, no regressions

- [ ] **Step 3: Run clippy**

Run: `cd /Users/user/dev/rightclaw && cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Manual smoke test**

Run: `cd /Users/user/dev/rightclaw && cargo run -- agent --help`
Expected: output includes `destroy` subcommand with description
