# Agent Re-initialization (`agent init --force`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `--force` and `--fresh` flags to `rightclaw agent init` so users can wipe and re-create agents in one command.

**Architecture:** Sequential delete-then-init. `--force` reads existing `agent.yaml`, confirms with user, deletes sandbox + agent dir, then runs `init_agent()` with saved config. `--fresh` skips config preservation and re-runs the wizard.

**Tech Stack:** Rust (edition 2024), clap, serde-saphyr, miette, tokio (for async `delete_sandbox`)

---

### Task 1: Add `InitOverrides` struct and update `init_agent()` signature

**Files:**
- Modify: `crates/rightclaw/src/init.rs:18-26`

- [ ] **Step 1: Write failing test — init_agent with overrides skips wizard values**

Add to the `#[cfg(test)] mod tests` in `crates/rightclaw/src/init.rs`:

```rust
#[test]
fn init_agent_with_overrides_applies_saved_config() {
    use crate::agent::types::{NetworkPolicy, SandboxMode};
    let dir = tempdir().unwrap();
    let overrides = InitOverrides {
        sandbox_mode: SandboxMode::None,
        network_policy: NetworkPolicy::Permissive,
        telegram_token: Some("999888:XYZtoken".to_string()),
        allowed_chat_ids: vec![111, 222],
        model: Some("opus".to_string()),
        env: [("FOO".to_string(), "bar".to_string())].into(),
    };
    let agent_dir = init_agent(
        &dir.path().join("agents"),
        "override-test",
        Some(&overrides),
    )
    .unwrap();

    let yaml = std::fs::read_to_string(agent_dir.join("agent.yaml")).unwrap();
    assert!(yaml.contains("mode: none"), "sandbox mode from overrides, got:\n{yaml}");
    assert!(yaml.contains("network_policy: permissive"), "network policy from overrides, got:\n{yaml}");
    assert!(yaml.contains("telegram_token: \"999888:XYZtoken\""), "telegram token from overrides, got:\n{yaml}");
    assert!(yaml.contains("  - 111"), "chat id 111 from overrides, got:\n{yaml}");
    assert!(yaml.contains("  - 222"), "chat id 222 from overrides, got:\n{yaml}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw init_agent_with_overrides_applies_saved_config`
Expected: FAIL — `InitOverrides` doesn't exist, `init_agent` signature is wrong.

- [ ] **Step 3: Define `InitOverrides` and refactor `init_agent()` signature**

In `crates/rightclaw/src/init.rs`, add the struct before `init_agent`:

```rust
use std::collections::HashMap;

/// Preserved config from a previous agent, used during `--force` re-init.
/// When provided, `init_agent` skips the wizard and applies these values.
pub struct InitOverrides {
    pub sandbox_mode: SandboxMode,
    pub network_policy: NetworkPolicy,
    pub telegram_token: Option<String>,
    pub allowed_chat_ids: Vec<i64>,
    pub model: Option<String>,
    pub env: HashMap<String, String>,
}
```

Change `init_agent` signature from:

```rust
pub fn init_agent(
    agents_parent_dir: &Path,
    name: &str,
    telegram_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    network_policy: &NetworkPolicy,
    sandbox_mode: &SandboxMode,
) -> miette::Result<PathBuf> {
```

To:

```rust
pub fn init_agent(
    agents_parent_dir: &Path,
    name: &str,
    overrides: Option<&InitOverrides>,
) -> miette::Result<PathBuf> {
```

Inside `init_agent`, extract values from overrides with defaults:

```rust
let sandbox_mode = overrides.map(|o| &o.sandbox_mode).cloned().unwrap_or(SandboxMode::Openshell);
let network_policy = overrides.map(|o| &o.network_policy).cloned().unwrap_or(NetworkPolicy::Permissive);
let telegram_token = overrides.and_then(|o| o.telegram_token.as_deref());
let telegram_allowed_chat_ids = overrides.map(|o| o.allowed_chat_ids.as_slice()).unwrap_or(&[]);
```

Then use these local variables in the existing logic (lines 80-125) replacing the old parameters.

Also add model and env to the agent.yaml append block:

```rust
if let Some(overrides) = overrides {
    if let Some(ref model) = overrides.model {
        yaml.push_str(&format!("\nmodel: \"{model}\"\n"));
    }
    if !overrides.env.is_empty() {
        yaml.push_str("\nenv:\n");
        for (k, v) in &overrides.env {
            yaml.push_str(&format!("  {k}: \"{v}\"\n"));
        }
    }
}
```

- [ ] **Step 4: Update all callers of `init_agent` to use new signature**

In `crates/rightclaw/src/init.rs`, update `init_rightclaw_home` (line 168):

```rust
let overrides = InitOverrides {
    sandbox_mode: sandbox_mode.clone(),
    network_policy: network_policy.clone(),
    telegram_token: telegram_token.map(|t| t.to_string()),
    allowed_chat_ids: telegram_allowed_chat_ids.to_vec(),
    model: None,
    env: HashMap::new(),
};
let _agents_dir = init_agent(
    &agents_parent,
    "right",
    Some(&overrides),
)?;
```

Also update `init_rightclaw_home` signature — it still takes individual params since `rightclaw init` (root) doesn't use `--force`. Keep its signature unchanged, just build `InitOverrides` internally.

In `crates/rightclaw-cli/src/main.rs`, update `cmd_agent_init` (line 562):

```rust
let overrides = rightclaw::init::InitOverrides {
    sandbox_mode: sandbox,
    network_policy,
    telegram_token: token,
    allowed_chat_ids: chat_ids,
    model: None,
    env: std::collections::HashMap::new(),
};
let agent_dir = rightclaw::init::init_agent(
    &agents_parent,
    name,
    Some(&overrides),
)?;
```

- [ ] **Step 5: Update existing tests to use new signature**

All existing tests in `init.rs` that call `init_agent` directly (e.g., `init_generates_policy_yaml_for_openshell_mode`, `init_skips_policy_yaml_for_none_mode`, `init_writes_sandbox_mode_to_agent_yaml`) need to pass `Option<&InitOverrides>` instead of individual params. Build an `InitOverrides` in each test.

- [ ] **Step 6: Run all tests to verify they pass**

Run: `cargo test -p rightclaw`
Expected: ALL PASS

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/init.rs crates/rightclaw-cli/src/main.rs
git commit -m "refactor: introduce InitOverrides, unify init_agent signature"
```

---

### Task 2: Add `--force`, `--fresh`, `--yes` CLI flags and wipe logic

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs:44-59` (AgentCommands::Init)
- Modify: `crates/rightclaw-cli/src/main.rs:519-583` (cmd_agent_init)
- Modify: `crates/rightclaw/src/init.rs:28-33` (remove dir-exists check)

- [ ] **Step 1: Write failing integration test — `agent init --force -y` re-creates agent**

Add to `crates/rightclaw-cli/tests/cli_integration.rs`:

```rust
#[test]
fn test_agent_init_force_recreates_agent() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // First: create the agent
    rightclaw()
        .args(["--home", home, "agent", "init", "test-agent", "-y"])
        .assert()
        .success();

    let agent_dir = dir.path().join("agents/test-agent");
    assert!(agent_dir.exists());

    // Write a marker file to verify it gets wiped
    fs::write(agent_dir.join("MARKER.txt"), "should be deleted").unwrap();

    // Re-init with --force
    rightclaw()
        .args(["--home", home, "agent", "init", "test-agent", "--force", "-y"])
        .assert()
        .success();

    // Agent dir should exist again but marker should be gone
    assert!(agent_dir.exists());
    assert!(!agent_dir.join("MARKER.txt").exists(), "marker should be wiped by --force");
    assert!(agent_dir.join("agent.yaml").exists(), "agent.yaml should be re-created");
}
```

- [ ] **Step 2: Write failing integration test — `--fresh` without `--force` errors**

```rust
#[test]
fn test_agent_init_fresh_without_force_errors() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "agent", "init", "test-agent", "--fresh", "-y"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--fresh requires --force"));
}
```

- [ ] **Step 3: Write failing integration test — `--force` preserves config**

```rust
#[test]
fn test_agent_init_force_preserves_config() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create agent with sandbox mode none
    rightclaw()
        .args([
            "--home", home, "agent", "init", "preserve-test", "-y",
            "--sandbox-mode", "none",
            "--network-policy", "permissive",
        ])
        .assert()
        .success();

    let agent_yaml_path = dir.path().join("agents/preserve-test/agent.yaml");
    let yaml_before = fs::read_to_string(&agent_yaml_path).unwrap();
    assert!(yaml_before.contains("mode: none"));

    // Re-init with --force (no --fresh) — should preserve mode: none
    rightclaw()
        .args(["--home", home, "agent", "init", "preserve-test", "--force", "-y"])
        .assert()
        .success();

    let yaml_after = fs::read_to_string(&agent_yaml_path).unwrap();
    assert!(
        yaml_after.contains("mode: none"),
        "sandbox mode should be preserved from old config, got:\n{yaml_after}"
    );
}
```

- [ ] **Step 4: Write failing integration test — `--force` on nonexistent agent works normally**

```rust
#[test]
fn test_agent_init_force_on_nonexistent_agent() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // --force on a new agent should just create it normally
    rightclaw()
        .args(["--home", home, "agent", "init", "new-agent", "--force", "-y"])
        .assert()
        .success();

    assert!(dir.path().join("agents/new-agent/agent.yaml").exists());
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p rightclaw-cli -- test_agent_init_force`
Expected: FAIL — `--force` flag doesn't exist yet.

- [ ] **Step 6: Add `--force` and `--fresh` flags to clap struct**

In `crates/rightclaw-cli/src/main.rs`, update `AgentCommands::Init`:

```rust
AgentCommands::Init {
    /// Agent name (alphanumeric + hyphens)
    name: String,
    /// Non-interactive mode
    #[arg(short = 'y', long)]
    yes: bool,
    /// If agent exists, wipe and re-create (confirms unless -y)
    #[arg(long)]
    force: bool,
    /// With --force: re-run wizard instead of reusing existing config
    #[arg(long, requires = "force")]
    fresh: bool,
    /// Network policy: restrictive or permissive
    #[arg(long)]
    network_policy: Option<rightclaw::agent::types::NetworkPolicy>,
    /// Sandbox mode: openshell or none
    #[arg(long)]
    sandbox_mode: Option<rightclaw::agent::types::SandboxMode>,
},
```

Note: `requires = "force"` makes clap enforce `--fresh requires --force` automatically. The error message will be clap-generated.

- [ ] **Step 7: Update the match arm to pass new flags**

In the `Commands::Agent` match arm (~line 375):

```rust
AgentCommands::Init { name, yes, force, fresh, network_policy, sandbox_mode } => {
    cmd_agent_init(&home, &name, yes, force, fresh, network_policy, sandbox_mode)
}
```

- [ ] **Step 8: Implement wipe logic in `cmd_agent_init`**

Replace `cmd_agent_init` with:

```rust
fn cmd_agent_init(
    home: &Path,
    name: &str,
    yes: bool,
    force: bool,
    fresh: bool,
    network_policy: Option<rightclaw::agent::types::NetworkPolicy>,
    sandbox_mode: Option<rightclaw::agent::types::SandboxMode>,
) -> miette::Result<()> {
    let interactive = !yes;
    let agents_parent = home.join("agents");
    let agent_dir = agents_parent.join(name);

    // --- Force wipe logic ---
    let saved_overrides = if force && agent_dir.exists() {
        // Read existing config before deletion (unless --fresh).
        let saved = if fresh {
            None
        } else {
            let yaml_path = agent_dir.join("agent.yaml");
            let yaml_str = std::fs::read_to_string(&yaml_path).map_err(|e| {
                miette::miette!(
                    help = "Use --fresh to reconfigure from scratch",
                    "Could not read existing agent.yaml: {e:#}"
                )
            })?;
            let config: rightclaw::agent::types::AgentConfig =
                serde_saphyr::from_str(&yaml_str).map_err(|e| {
                    miette::miette!(
                        help = "Use --fresh to reconfigure from scratch",
                        "Could not parse existing agent.yaml: {e:#}"
                    )
                })?;
            Some(config)
        };

        // Check agent is not running.
        let state_path = home.join("run/runtime-state.json");
        if state_path.exists() {
            let state = rightclaw::runtime::read_state(&state_path)?;
            if state.agents.iter().any(|a| a.name == name) {
                return Err(miette::miette!(
                    help = "Run `rightclaw down` first",
                    "Agent '{name}' is currently running"
                ));
            }
        }

        // Confirm with user.
        if interactive {
            use std::io::{self, Write};
            println!("Agent \"{name}\" already exists at {}", agent_dir.display());
            println!("This will permanently delete:");
            println!("  - All agent files (identity, memory, skills, config)");
            println!(
                "  - OpenShell sandbox \"{}\" (if exists)",
                rightclaw::openshell::sandbox_name(name)
            );
            print!("Continue? [y/N] ");
            io::stdout().flush().map_err(|e| miette::miette!("stdout flush: {e}"))?;
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .map_err(|e| miette::miette!("failed to read input: {e}"))?;
            if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                return Err(miette::miette!("Aborted"));
            }
        }

        // Delete sandbox (best-effort, async).
        let sb_name = rightclaw::openshell::sandbox_name(name);
        tokio::runtime::Handle::current().block_on(async {
            rightclaw::openshell::delete_sandbox(&sb_name).await;
        });

        // Delete SSH config.
        let ssh_config = home.join(format!("run/ssh/{}.ssh-config", sb_name));
        if ssh_config.exists() {
            std::fs::remove_file(&ssh_config).ok();
        }

        // Delete agent directory.
        std::fs::remove_dir_all(&agent_dir).map_err(|e| {
            miette::miette!("Failed to delete agent directory {}: {e:#}", agent_dir.display())
        })?;

        tracing::info!(agent = name, "wiped agent directory and sandbox");

        saved
    } else {
        None
    };

    // --- Build overrides ---
    let overrides = if let Some(config) = saved_overrides {
        // Reuse saved config from old agent.yaml.
        rightclaw::init::InitOverrides {
            sandbox_mode: config.sandbox_mode().clone(),
            network_policy: config.network_policy.clone(),
            telegram_token: config.telegram_token,
            allowed_chat_ids: config.allowed_chat_ids,
            model: config.model,
            env: config.env,
        }
    } else {
        // Fresh init: run wizard or use CLI flags.
        let sandbox = match sandbox_mode {
            Some(mode) => mode,
            None if !interactive => rightclaw::agent::types::SandboxMode::Openshell,
            None => rightclaw::init::prompt_sandbox_mode()?,
        };

        let network_policy =
            if matches!(sandbox, rightclaw::agent::types::SandboxMode::Openshell) {
                match network_policy {
                    Some(p) => p,
                    None if !interactive => {
                        rightclaw::agent::types::NetworkPolicy::Restrictive
                    }
                    None => rightclaw::init::prompt_network_policy()?,
                }
            } else {
                network_policy.unwrap_or(rightclaw::agent::types::NetworkPolicy::Permissive)
            };

        let token = if interactive {
            crate::wizard::telegram_setup(None, true)?
        } else {
            None
        };

        let chat_ids: Vec<i64> = if interactive && token.is_some() {
            crate::wizard::chat_ids_setup()?
        } else {
            vec![]
        };

        rightclaw::init::InitOverrides {
            sandbox_mode: sandbox,
            network_policy,
            telegram_token: token,
            allowed_chat_ids: chat_ids,
            model: None,
            env: std::collections::HashMap::new(),
        }
    };

    let agent_dir = rightclaw::init::init_agent(&agents_parent, name, Some(&overrides))?;

    println!("Agent '{name}' created at {}", agent_dir.display());
    if overrides.telegram_token.is_some() {
        println!("Telegram channel configured.");
    }
    if !overrides.allowed_chat_ids.is_empty() {
        println!("Telegram chat ID allowlist configured.");
    }
    println!();
    println!("If rightclaw is running, apply changes with:");
    println!("  rightclaw reload");

    Ok(())
}
```

- [ ] **Step 9: Remove the dir-exists check from `init_agent`**

In `crates/rightclaw/src/init.rs`, remove lines 28-33:

```rust
// DELETE THIS BLOCK:
if agents_dir.exists() {
    return Err(miette::miette!(
        "Agent directory already exists at {}. Use `rightclaw agent config` to change settings.",
        agents_dir.display()
    ));
}
```

The CLI layer now handles this check (either rejecting or wiping depending on `--force`). `init_agent` should be idempotent — `create_dir_all` already handles existing dirs.

However, we still need the CLI to reject `init` without `--force` when dir exists. Add this check at the top of `cmd_agent_init`, before the force-wipe block:

```rust
if agent_dir.exists() && !force {
    return Err(miette::miette!(
        help = "Use --force to wipe and re-create, or `rightclaw agent config` to change settings",
        "Agent directory already exists at {}",
        agent_dir.display()
    ));
}
```

- [ ] **Step 10: Update the `init_errors_if_already_initialized` test**

In `crates/rightclaw/src/init.rs`, the test `init_errors_if_already_initialized` (line 317) tests that `init_rightclaw_home` errors on double init. Since `init_rightclaw_home` still has its own check (line 161), this test should still pass. Verify.

- [ ] **Step 11: Run all tests**

Run: `cargo test --workspace`
Expected: ALL PASS

- [ ] **Step 12: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs crates/rightclaw/src/init.rs crates/rightclaw-cli/tests/cli_integration.rs
git commit -m "feat: add --force and --fresh flags to agent init for re-initialization"
```

---

### Task 3: Build and verify

**Files:**
- None (verification only)

- [ ] **Step 1: Build the full workspace**

Run: `cargo build --workspace`
Expected: BUILD SUCCESS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: NO WARNINGS

- [ ] **Step 3: Run full test suite**

Run: `cargo test --workspace`
Expected: ALL PASS

- [ ] **Step 4: Fix any issues found**

If clippy or tests fail, fix and re-run.

- [ ] **Step 5: Final commit (if any fixes)**

```bash
git add -A
git commit -m "fix: address clippy and test issues from agent reinit"
```
