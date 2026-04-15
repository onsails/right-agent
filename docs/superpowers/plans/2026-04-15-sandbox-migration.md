# Sandbox Migration, Backup & Restore Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable filesystem policy changes on existing agents via sandbox migration, and provide backup/restore primitives for disaster recovery.

**Architecture:** Add `sandbox.name` to agent.yaml for explicit sandbox naming. Introduce `rightclaw agent backup` and `rightclaw agent restore` (via `agent init --from-backup`) as CLI primitives. Migration composes backup + restore when `rightclaw agent config` detects filesystem policy changes via gRPC policy comparison. Bot changes are minimal — only sandbox name resolution.

**Tech Stack:** Rust, clap (CLI), tokio (async SSH/tar), rusqlite (VACUUM INTO), tonic (gRPC), inquire (interactive prompts)

---

### Task 1: Add `name` field to SandboxConfig

**Files:**
- Modify: `crates/rightclaw/src/agent/types.rs`

- [ ] **Step 1: Write failing test for sandbox name deserialization**

In `crates/rightclaw/src/agent/types.rs`, add to the existing `#[cfg(test)]` module:

```rust
#[test]
fn sandbox_config_with_name() {
    let yaml = r#"
sandbox:
  mode: openshell
  policy_file: policy.yaml
  name: "rightclaw-brain-20260415-1430"
"#;
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let sb = config.sandbox.unwrap();
    assert_eq!(sb.name.as_deref(), Some("rightclaw-brain-20260415-1430"));
}

#[test]
fn sandbox_config_without_name_is_none() {
    let yaml = r#"
sandbox:
  mode: openshell
  policy_file: policy.yaml
"#;
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let sb = config.sandbox.unwrap();
    assert!(sb.name.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw sandbox_config_with_name sandbox_config_without_name_is_none`
Expected: `sandbox_config_with_name` fails (unknown field `name`)

- [ ] **Step 3: Add `name` field to SandboxConfig**

In `crates/rightclaw/src/agent/types.rs`, add to `SandboxConfig`:

```rust
pub struct SandboxConfig {
    pub mode: SandboxMode,
    pub policy_file: Option<std::path::PathBuf>,
    /// Explicit sandbox name. When set, overrides the deterministic
    /// `rightclaw-{agent_name}` default. Written by migration/restore flows.
    #[serde(default)]
    pub name: Option<String>,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw sandbox_config_with_name sandbox_config_without_name_is_none`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/agent/types.rs
git commit -m "feat: add optional sandbox.name field to SandboxConfig"
```

---

### Task 2: Add `resolve_sandbox_name()` and replace `sandbox_name()` calls

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs`
- Modify: `crates/rightclaw/src/openshell_tests.rs`
- Modify: `crates/bot/src/lib.rs` (line 235, 306, 319)
- Modify: `crates/bot/src/cron.rs` (lines 120, 316)
- Modify: `crates/bot/src/cron_delivery.rs` (line 418)
- Modify: `crates/bot/src/telegram/handler.rs` (line 957)
- Modify: `crates/bot/src/telegram/worker.rs` (line 761)
- Modify: `crates/bot/src/telegram/attachments.rs` (line 618)
- Modify: `crates/rightclaw-cli/src/main.rs` (line 1649)

- [ ] **Step 1: Write failing test for resolve_sandbox_name**

In `crates/rightclaw/src/openshell_tests.rs`, add:

```rust
#[test]
fn resolve_sandbox_name_with_explicit_name() {
    use crate::agent::types::{AgentConfig, SandboxConfig, SandboxMode};

    let config = AgentConfig {
        sandbox: Some(SandboxConfig {
            mode: SandboxMode::Openshell,
            policy_file: None,
            name: Some("rightclaw-brain-20260415-1430".to_owned()),
        }),
        ..Default::default()
    };
    assert_eq!(
        resolve_sandbox_name("brain", &config),
        "rightclaw-brain-20260415-1430"
    );
}

#[test]
fn resolve_sandbox_name_falls_back_to_deterministic() {
    use crate::agent::types::AgentConfig;

    let config = AgentConfig::default();
    assert_eq!(resolve_sandbox_name("brain", &config), "rightclaw-brain");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw resolve_sandbox_name`
Expected: FAIL — `resolve_sandbox_name` not found

- [ ] **Step 3: Implement `resolve_sandbox_name`**

In `crates/rightclaw/src/openshell.rs`, add next to the existing `sandbox_name()`:

```rust
/// Resolve sandbox name: explicit from config, or deterministic fallback.
pub fn resolve_sandbox_name(agent_name: &str, config: &crate::agent::types::AgentConfig) -> String {
    config
        .sandbox
        .as_ref()
        .and_then(|s| s.name.clone())
        .unwrap_or_else(|| sandbox_name(agent_name))
}
```

Keep the existing `sandbox_name()` — it's still used as the fallback and for generating new names.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw resolve_sandbox_name`
Expected: PASS

- [ ] **Step 5: Replace `sandbox_name()` calls in bot**

In `crates/bot/src/lib.rs`, the bot startup reads `config` from agent.yaml already. Replace line 235:

```rust
// Before:
let sandbox = rightclaw::openshell::sandbox_name(&args.agent);
// After:
let sandbox = rightclaw::openshell::resolve_sandbox_name(&args.agent, &config);
```

Replace line 306 (`ssh_host` call — this derives from sandbox name, not agent name):
```rust
// Before:
let ssh_host = rightclaw::openshell::ssh_host(&args.agent);
// After — ssh_host still uses agent name for the SSH alias, which is fine:
// The SSH config file is generated from sandbox name, ssh_host is the alias.
// No change needed here — ssh_host() generates the SSH alias which is stable.
```

Actually, `ssh_host()` generates the SSH config host alias. This is derived from `sandbox_name`, not `agent_name` — check `generate_ssh_config()` which takes the sandbox name. The SSH alias in the config file is `openshell-{sandbox_name}`. So `ssh_host()` must match the sandbox name used in `generate_ssh_config()`.

Replace all `ssh_host(&args.agent)` calls with `ssh_host_for_sandbox(&sandbox)` pattern. Add a new helper:

```rust
/// SSH host alias for a sandbox (matches generate_ssh_config output).
pub fn ssh_host_for_sandbox(sandbox_name: &str) -> String {
    format!("openshell-{sandbox_name}")
}
```

Then in bot/src/lib.rs, after resolving `sandbox`:
```rust
let ssh_alias = rightclaw::openshell::ssh_host_for_sandbox(&sandbox);
```

Pass this `ssh_alias` (or the resolved `sandbox` name) through to all callsites that currently call `ssh_host(&agent_name)`.

The key callsites to update:
- `crates/bot/src/lib.rs:306` — `ssh_host(&args.agent)` → use resolved sandbox
- `crates/bot/src/lib.rs:319` — `sandbox_name(&args.agent)` → use resolved sandbox (for SandboxExec)
- `crates/bot/src/cron.rs:120,316` — these receive `agent_name`, need sandbox name threaded through
- `crates/bot/src/cron_delivery.rs:418` — same
- `crates/bot/src/telegram/handler.rs:957` — same
- `crates/bot/src/telegram/worker.rs:761` — same
- `crates/bot/src/telegram/attachments.rs:618` — same

The cleanest approach: store the resolved sandbox name in the shared context that these callsites already have access to. Check what shared state they use — likely `WorkerContext` or similar.

Read `WorkerContext` and the relevant structs to determine how to thread the sandbox name. The sandbox name should be resolved once at bot startup and stored alongside `ssh_config_path`.

- [ ] **Step 6: Replace in CLI**

In `crates/rightclaw-cli/src/main.rs:1649`, the `ssh` subcommand. Here we need to load agent config first:

```rust
// Before:
let ssh_host = rightclaw::openshell::ssh_host(agent_name);
// After:
let agent_dir = rightclaw::config::agents_dir(home).join(agent_name);
let agent_config = rightclaw::agent::discovery::parse_agent_config(&agent_dir)?;
let sandbox = rightclaw::openshell::resolve_sandbox_name(agent_name, &agent_config);
let ssh_host = rightclaw::openshell::ssh_host_for_sandbox(&sandbox);
```

- [ ] **Step 7: Build workspace to verify everything compiles**

Run: `cargo build --workspace`
Expected: Successful build

- [ ] **Step 8: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor: replace sandbox_name() with resolve_sandbox_name() for configurable sandbox names"
```

---

### Task 3: Add `backups_dir` helper and `ssh_tar_download` primitive

**Files:**
- Modify: `crates/rightclaw/src/config/mod.rs`
- Modify: `crates/rightclaw/src/openshell.rs`

- [ ] **Step 1: Add `backups_dir` helper**

In `crates/rightclaw/src/config/mod.rs`, add:

```rust
/// Path to the backups directory for a specific agent.
///
/// Layout: `<home>/backups/<agent_name>/<timestamp>/`
pub fn backups_dir(home: &Path, agent_name: &str) -> PathBuf {
    home.join("backups").join(agent_name)
}
```

- [ ] **Step 2: Add `ssh_tar_download` — tar sandbox to local file via SSH**

In `crates/rightclaw/src/openshell.rs`, add a new function that pipes SSH tar stdout to a file:

```rust
/// Stream `tar czpf -` from inside the sandbox to a local file via SSH.
///
/// Uses the SSH config generated by `generate_ssh_config()`. The `-p` flag
/// preserves file permissions. Stdout is piped directly to `dest_path` —
/// no intermediate buffering in memory.
pub async fn ssh_tar_download(
    config_path: &Path,
    ssh_host: &str,
    sandbox_path: &str,
    dest_path: &Path,
    timeout_secs: u64,
) -> miette::Result<()> {
    let mut child = tokio::process::Command::new("ssh")
        .arg("-F").arg(config_path)
        .arg(ssh_host)
        .arg("--")
        .args(["tar", "czpf", "-", "-C", "/", sandbox_path.trim_start_matches('/')])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| miette::miette!("failed to spawn ssh tar: {e:#}"))?;

    let stdout = child.stdout.take()
        .ok_or_else(|| miette::miette!("ssh tar: no stdout"))?;

    let dest = dest_path.to_owned();
    let copy_task = tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut file = tokio::fs::File::create(&dest).await
            .map_err(|e| miette::miette!("failed to create {}: {e:#}", dest.display()))?;
        tokio::io::copy(&mut reader, &mut file).await
            .map_err(|e| miette::miette!("failed to write tar to {}: {e:#}", dest.display()))?;
        Ok::<_, miette::Report>(())
    });

    let timeout_dur = std::time::Duration::from_secs(timeout_secs);
    let output = tokio::time::timeout(timeout_dur, child.wait())
        .await
        .map_err(|_| miette::miette!("ssh tar download timed out after {timeout_secs}s"))?
        .map_err(|e| miette::miette!("ssh tar wait failed: {e:#}"))?;

    copy_task.await
        .map_err(|e| miette::miette!("tar copy task panicked: {e:#}"))??;

    if !output.success() {
        return Err(miette::miette!("ssh tar exited with {output}"));
    }

    Ok(())
}
```

- [ ] **Step 3: Add `ssh_tar_upload` — restore tar into sandbox via SSH**

```rust
/// Stream a local tar.gz file into the sandbox via SSH `tar xzpf -`.
pub async fn ssh_tar_upload(
    config_path: &Path,
    ssh_host: &str,
    src_path: &Path,
    timeout_secs: u64,
) -> miette::Result<()> {
    let mut child = tokio::process::Command::new("ssh")
        .arg("-F").arg(config_path)
        .arg(ssh_host)
        .arg("--")
        .args(["tar", "xzpf", "-", "-C", "/"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| miette::miette!("failed to spawn ssh tar restore: {e:#}"))?;

    let stdin = child.stdin.take()
        .ok_or_else(|| miette::miette!("ssh tar restore: no stdin"))?;

    let src = src_path.to_owned();
    let feed_task = tokio::spawn(async move {
        let file = tokio::fs::File::open(&src).await
            .map_err(|e| miette::miette!("failed to open {}: {e:#}", src.display()))?;
        let mut reader = tokio::io::BufReader::new(file);
        let mut writer = stdin;
        tokio::io::copy(&mut reader, &mut writer).await
            .map_err(|e| miette::miette!("failed to pipe tar to ssh: {e:#}"))?;
        Ok::<_, miette::Report>(())
    });

    let timeout_dur = std::time::Duration::from_secs(timeout_secs);
    let output = tokio::time::timeout(timeout_dur, child.wait())
        .await
        .map_err(|_| miette::miette!("ssh tar upload timed out after {timeout_secs}s"))?
        .map_err(|e| miette::miette!("ssh tar wait failed: {e:#}"))?;

    feed_task.await
        .map_err(|e| miette::miette!("tar feed task panicked: {e:#}"))??;

    if !output.success() {
        return Err(miette::miette!("ssh tar restore exited with {output}"));
    }

    Ok(())
}
```

- [ ] **Step 4: Build to verify compilation**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/config/mod.rs crates/rightclaw/src/openshell.rs
git commit -m "feat: add SSH tar download/upload primitives and backups_dir helper"
```

---

### Task 4: Implement `rightclaw agent backup`

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs` (add Backup subcommand + handler)

- [ ] **Step 1: Add Backup to AgentCommands enum**

In `crates/rightclaw-cli/src/main.rs`, add to the `AgentCommands` enum:

```rust
/// Back up an agent's sandbox and configuration
Backup {
    /// Agent name
    name: String,
    /// Only back up sandbox files (skip agent.yaml, data.db, policy.yaml)
    #[arg(long)]
    sandbox_only: bool,
},
```

Add the dispatch in the `Agent` match arm:

```rust
AgentCommands::Backup { name, sandbox_only } => {
    cmd_agent_backup(&home, &name, sandbox_only).await
}
```

- [ ] **Step 2: Implement `cmd_agent_backup`**

Add the handler function. The core logic:

```rust
async fn cmd_agent_backup(home: &Path, agent_name: &str, sandbox_only: bool) -> miette::Result<()> {
    let agents_dir = rightclaw::config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);
    if !agent_dir.exists() {
        return Err(miette::miette!("Agent '{agent_name}' not found"));
    }

    let config = rightclaw::agent::discovery::parse_agent_config(&agent_dir)?;
    let is_sandboxed = config.sandbox.as_ref()
        .map(|s| s.mode == rightclaw::agent::types::SandboxMode::Openshell)
        .unwrap_or(true); // default is openshell

    // Generate timestamp-based backup directory
    let now = chrono::Local::now();
    let timestamp = now.format("%Y%m%d-%H%M").to_string();
    let backup_dir = rightclaw::config::backups_dir(home, agent_name).join(&timestamp);
    std::fs::create_dir_all(&backup_dir)
        .map_err(|e| miette::miette!("failed to create backup dir: {e:#}"))?;

    let tar_path = backup_dir.join("sandbox.tar.gz");

    if is_sandboxed {
        // Verify sandbox is ready
        let sandbox = rightclaw::openshell::resolve_sandbox_name(agent_name, &config);
        let mtls_dir = match rightclaw::openshell::preflight_check() {
            rightclaw::openshell::OpenShellStatus::Ready(dir) => dir,
            _ => return Err(miette::miette!("OpenShell is not available")),
        };
        let mut grpc = rightclaw::openshell::connect_grpc(&mtls_dir).await?;
        if !rightclaw::openshell::is_sandbox_ready(&mut grpc, &sandbox).await? {
            return Err(miette::miette!("Sandbox '{sandbox}' is not ready"));
        }

        // SSH tar download
        let ssh_config_dir = home.join("run").join("ssh");
        let ssh_config_path = ssh_config_dir.join(format!("{sandbox}.ssh-config"));
        let ssh_alias = rightclaw::openshell::ssh_host_for_sandbox(&sandbox);

        println!("Backing up sandbox '{sandbox}'...");
        rightclaw::openshell::ssh_tar_download(
            &ssh_config_path,
            &ssh_alias,
            "sandbox",
            &tar_path,
            600, // 10 min timeout for large sandboxes
        ).await?;
    } else {
        // No-sandbox: tar the agent dir excluding data.db
        println!("Backing up agent directory...");
        let status = tokio::process::Command::new("tar")
            .args(["czpf"])
            .arg(&tar_path)
            .arg("--exclude=data.db")
            .arg("-C")
            .arg(&agent_dir)
            .arg(".")
            .status()
            .await
            .map_err(|e| miette::miette!("failed to run tar: {e:#}"))?;
        if !status.success() {
            return Err(miette::miette!("tar failed with {status}"));
        }
    }

    println!("  sandbox.tar.gz written");

    // Full backup: copy config files + VACUUM INTO for data.db
    if !sandbox_only {
        let agent_yaml = agent_dir.join("agent.yaml");
        if agent_yaml.exists() {
            std::fs::copy(&agent_yaml, backup_dir.join("agent.yaml"))
                .map_err(|e| miette::miette!("failed to copy agent.yaml: {e:#}"))?;
            println!("  agent.yaml copied");
        }

        let policy_yaml = agent_dir.join("policy.yaml");
        if policy_yaml.exists() {
            std::fs::copy(&policy_yaml, backup_dir.join("policy.yaml"))
                .map_err(|e| miette::miette!("failed to copy policy.yaml: {e:#}"))?;
            println!("  policy.yaml copied");
        }

        let db_path = agent_dir.join("data.db");
        if db_path.exists() {
            let backup_db = backup_dir.join("data.db");
            let conn = rusqlite::Connection::open(&db_path)
                .map_err(|e| miette::miette!("failed to open data.db: {e:#}"))?;
            conn.execute(
                &format!("VACUUM INTO '{}'", backup_db.display()),
                [],
            ).map_err(|e| miette::miette!("VACUUM INTO failed: {e:#}"))?;
            println!("  data.db snapshot created");
        }
    }

    println!("\nBackup saved to: {}", backup_dir.display());
    Ok(())
}
```

Note: this uses `chrono::Local::now()`. Check if chrono is already a dependency; if not, add it. Alternatively use `std::time::SystemTime` + manual formatting.

- [ ] **Step 3: Check if chrono is available or use alternative**

Run: `grep -r "chrono" crates/rightclaw-cli/Cargo.toml crates/rightclaw/Cargo.toml`

If not present, check if the bot crate has it: `grep -r "chrono" crates/bot/Cargo.toml`

If chrono is not available, use this alternative for timestamp:

```rust
fn backup_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    // Use time crate or manual UTC formatting
    // Simplest: just use unix timestamp
    // But spec says YYYYMMDD-HHMM, so we need proper formatting.
    // Add chrono to rightclaw-cli/Cargo.toml if needed.
}
```

Add `chrono = "0.4"` to `crates/rightclaw-cli/Cargo.toml` if not already present.

- [ ] **Step 4: Add rusqlite dependency to rightclaw-cli if needed**

Run: `grep rusqlite crates/rightclaw-cli/Cargo.toml`

If not present, add `rusqlite = { version = "0.34", features = ["bundled"] }` (match the version used in other crates).

- [ ] **Step 5: Build and verify**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 6: Manual test**

Run: `cargo run -p rightclaw-cli -- agent backup right --sandbox-only`
Verify: backup dir created at `~/.rightclaw/backups/right/<timestamp>/sandbox.tar.gz`

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw-cli/
git commit -m "feat: implement rightclaw agent backup command"
```

---

### Task 5: Implement `rightclaw agent init --from-backup`

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs` (Init subcommand + handler)

- [ ] **Step 1: Add `--from-backup` flag to Init command**

In the `AgentCommands::Init` variant, add:

```rust
/// Restore agent from a backup directory
#[arg(long, conflicts_with_all = ["fresh", "network_policy", "sandbox_mode"])]
from_backup: Option<PathBuf>,
```

Thread it through to the dispatch:

```rust
AgentCommands::Init { name, yes, force, fresh, network_policy, sandbox_mode, from_backup } => {
    if let Some(backup_path) = from_backup {
        cmd_agent_restore(&home, &name, &backup_path).await
    } else {
        cmd_agent_init(&home, &name, yes, force, fresh, network_policy, sandbox_mode)
    }
}
```

- [ ] **Step 2: Implement `cmd_agent_restore`**

```rust
async fn cmd_agent_restore(home: &Path, agent_name: &str, backup_path: &Path) -> miette::Result<()> {
    // 1. Validate preconditions
    let agents_dir = rightclaw::config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);
    if agent_dir.exists() {
        return Err(miette::miette!(
            help = format!("Delete it first with `rightclaw agent delete {agent_name}`"),
            "Agent '{agent_name}' already exists"
        ));
    }

    let tar_path = backup_path.join("sandbox.tar.gz");
    let backup_agent_yaml = backup_path.join("agent.yaml");
    if !tar_path.exists() {
        return Err(miette::miette!("Backup missing sandbox.tar.gz at {}", backup_path.display()));
    }
    if !backup_agent_yaml.exists() {
        return Err(miette::miette!("Backup missing agent.yaml at {}", backup_path.display()));
    }

    // 2. Create agent directory and restore config files
    std::fs::create_dir_all(&agent_dir)
        .map_err(|e| miette::miette!("failed to create agent dir: {e:#}"))?;

    std::fs::copy(&backup_agent_yaml, agent_dir.join("agent.yaml"))
        .map_err(|e| miette::miette!("failed to restore agent.yaml: {e:#}"))?;

    let backup_policy = backup_path.join("policy.yaml");
    if backup_policy.exists() {
        std::fs::copy(&backup_policy, agent_dir.join("policy.yaml"))
            .map_err(|e| miette::miette!("failed to restore policy.yaml: {e:#}"))?;
    }

    let backup_db = backup_path.join("data.db");
    if backup_db.exists() {
        std::fs::copy(&backup_db, agent_dir.join("data.db"))
            .map_err(|e| miette::miette!("failed to restore data.db: {e:#}"))?;
    }

    // 3. Parse restored config to determine sandbox mode
    let config = rightclaw::agent::discovery::parse_agent_config(&agent_dir)?;
    let is_sandboxed = config.sandbox.as_ref()
        .map(|s| s.mode == rightclaw::agent::types::SandboxMode::Openshell)
        .unwrap_or(true);

    if is_sandboxed {
        // 4. Create new sandbox with timestamp name
        let now = chrono::Local::now();
        let timestamp = now.format("%Y%m%d-%H%M").to_string();
        let new_sandbox_name = format!("rightclaw-{agent_name}-{timestamp}");

        let policy_path = config.resolve_policy_path(&agent_dir)?
            .ok_or_else(|| miette::miette!("no policy path in restored agent.yaml"))?;

        println!("Creating sandbox '{new_sandbox_name}'...");

        // Prepare staging dir (minimal bootstrap files for sandbox creation)
        let staging = agent_dir.join("staging");
        // Run codegen first so staging files exist
        let agent_def = rightclaw::agent::discover_single_agent(&agent_dir)?;
        let self_exe = std::env::current_exe()
            .map_err(|e| miette::miette!("failed to get current exe: {e:#}"))?;
        rightclaw::codegen::run_single_agent_codegen(home, &agent_def, &self_exe, false)?;

        rightclaw::openshell::prepare_staging_dir(&agent_dir, &staging)?;

        // Create sandbox (ensure_sandbox uses sandbox_name() internally — we need a variant
        // that accepts an explicit name). For now, we can temporarily set config.sandbox.name
        // before calling, or add a new function.
        //
        // Better approach: add ensure_sandbox_with_name() that takes explicit sandbox name:
        let mtls_dir = match rightclaw::openshell::preflight_check() {
            rightclaw::openshell::OpenShellStatus::Ready(dir) => dir,
            _ => return Err(miette::miette!("OpenShell is not available")),
        };

        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                // spawn_sandbox takes name directly — good
                let mut child = rightclaw::openshell::spawn_sandbox(
                    &new_sandbox_name,
                    &policy_path,
                    Some(&staging),
                )?;

                let mut grpc = rightclaw::openshell::connect_grpc(&mtls_dir).await?;

                // Wait for READY
                tokio::select! {
                    result = rightclaw::openshell::wait_for_ready(&mut grpc, &new_sandbox_name, 120, 2) => {
                        result?;
                        drop(child);
                    }
                    status = child.wait() => {
                        let status = status.map_err(|e| miette::miette!("sandbox create failed: {e:#}"))?;
                        if !status.success() {
                            return Err(miette::miette!("sandbox create exited with {status}"));
                        }
                    }
                }

                // Wait for SSH
                let sandbox_id = rightclaw::openshell::resolve_sandbox_id(&mut grpc, &new_sandbox_name).await?;
                // wait_for_ssh is private — we'll need to make it pub or add a public wrapper
                // For now, use the pattern from ensure_sandbox

                Ok::<_, miette::Report>(())
            })
        })?;

        // 5. Generate SSH config
        let ssh_config_dir = home.join("run").join("ssh");
        std::fs::create_dir_all(&ssh_config_dir)
            .map_err(|e| miette::miette!("failed to create ssh dir: {e:#}"))?;

        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(
                rightclaw::openshell::generate_ssh_config(&new_sandbox_name, &ssh_config_dir)
            )
        })?;

        // 6. Restore sandbox files
        let ssh_config_path = ssh_config_dir.join(format!("{new_sandbox_name}.ssh-config"));
        let ssh_alias = rightclaw::openshell::ssh_host_for_sandbox(&new_sandbox_name);

        println!("Restoring sandbox files...");
        rightclaw::openshell::ssh_tar_upload(
            &ssh_config_path,
            &ssh_alias,
            &tar_path,
            600,
        ).await?;

        // 7. Write sandbox.name into agent.yaml
        crate::wizard::update_agent_yaml_sandbox_name(&agent_dir, &new_sandbox_name)?;

        println!("Sandbox '{new_sandbox_name}' ready");
    } else {
        // No-sandbox: unpack tar directly into agent dir
        println!("Restoring agent files...");
        let status = tokio::process::Command::new("tar")
            .args(["xzpf"])
            .arg(&tar_path)
            .arg("-C")
            .arg(&agent_dir)
            .status()
            .await
            .map_err(|e| miette::miette!("tar restore failed: {e:#}"))?;
        if !status.success() {
            return Err(miette::miette!("tar exited with {status}"));
        }
    }

    println!("\nAgent '{agent_name}' restored from {}", backup_path.display());
    Ok(())
}
```

- [ ] **Step 3: Make `wait_for_ssh` public or add a public wrapper**

In `crates/rightclaw/src/openshell.rs`, change `wait_for_ssh` from `async fn` to `pub async fn`:

```rust
// Before:
async fn wait_for_ssh(
// After:
pub async fn wait_for_ssh(
```

- [ ] **Step 4: Add `update_agent_yaml_sandbox_name` wizard helper**

In `crates/rightclaw-cli/src/wizard.rs`, add:

```rust
/// Write or update `sandbox.name` in agent.yaml.
pub fn update_agent_yaml_sandbox_name(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()> {
    let yaml_path = agent_dir.join("agent.yaml");
    let content = std::fs::read_to_string(&yaml_path)
        .map_err(|e| miette::miette!("failed to read agent.yaml: {e:#}"))?;

    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let mut in_sandbox_block = false;
    let mut name_line_idx = None;
    let mut sandbox_block_end = None;

    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("sandbox:") {
            in_sandbox_block = true;
            continue;
        }
        if in_sandbox_block {
            if line.starts_with("  ") || line.starts_with("\t") {
                if line.trim_start().starts_with("name:") {
                    name_line_idx = Some(i);
                }
                sandbox_block_end = Some(i);
            } else if !line.trim().is_empty() {
                in_sandbox_block = false;
            }
        }
    }

    let name_value = format!("  name: \"{}\"", sandbox_name);

    if let Some(idx) = name_line_idx {
        // Replace existing name line
        lines[idx] = name_value;
    } else if let Some(end) = sandbox_block_end {
        // Append after last sandbox field
        lines.insert(end + 1, name_value);
    } else {
        // No sandbox block — append one
        lines.push(format!("sandbox:\n  name: \"{}\"", sandbox_name));
    }

    let mut result = lines.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }

    std::fs::write(&yaml_path, &result)
        .map_err(|e| miette::miette!("failed to write agent.yaml: {e:#}"))?;

    Ok(())
}
```

- [ ] **Step 5: Build and verify**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/ crates/rightclaw/src/openshell.rs
git commit -m "feat: implement rightclaw agent init --from-backup for restore"
```

---

### Task 6: Add gRPC `get_sandbox_policy_status` and filesystem policy comparison

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs`

- [ ] **Step 1: Write test for filesystem policy comparison**

In `crates/rightclaw/src/openshell_tests.rs`, add:

```rust
#[test]
fn filesystem_policy_changed_detects_difference() {
    use crate::openshell_proto::openshell::sandbox::v1::{SandboxPolicy, FilesystemPolicy, LandlockPolicy};

    let old = SandboxPolicy {
        filesystem: Some(FilesystemPolicy {
            include_workdir: true,
            read_only: vec!["/usr".into(), "/lib".into()],
            read_write: vec!["/sandbox".into(), "/tmp".into()],
        }),
        landlock: Some(LandlockPolicy {
            compatibility: "best_effort".into(),
        }),
        ..Default::default()
    };

    let mut new = old.clone();
    new.filesystem.as_mut().unwrap().read_write.push("/data".into());

    assert!(filesystem_policy_changed(&old, &new));
}

#[test]
fn filesystem_policy_unchanged_when_only_network_differs() {
    use crate::openshell_proto::openshell::sandbox::v1::*;

    let old = SandboxPolicy {
        filesystem: Some(FilesystemPolicy {
            include_workdir: true,
            read_only: vec!["/usr".into()],
            read_write: vec!["/sandbox".into()],
        }),
        landlock: Some(LandlockPolicy {
            compatibility: "best_effort".into(),
        }),
        network_policies: Default::default(),
        ..Default::default()
    };

    let mut new = old.clone();
    new.network_policies.insert("test".into(), NetworkPolicyRule::default());

    assert!(!filesystem_policy_changed(&old, &new));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw filesystem_policy_changed`
Expected: FAIL — function not found

- [ ] **Step 3: Implement `filesystem_policy_changed` and `get_active_policy`**

In `crates/rightclaw/src/openshell.rs`:

```rust
use crate::openshell_proto::openshell::sandbox::v1::SandboxPolicy;

/// Compare filesystem_policy + landlock sections of two policies.
/// Returns true if they differ (migration required).
pub fn filesystem_policy_changed(old: &SandboxPolicy, new: &SandboxPolicy) -> bool {
    old.filesystem != new.filesystem || old.landlock != new.landlock
}

/// Fetch the currently active policy from a sandbox via gRPC.
/// Returns None if the full policy is not populated in the response.
pub async fn get_active_policy(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> miette::Result<Option<SandboxPolicy>> {
    use crate::openshell_proto::openshell::v1::GetSandboxPolicyStatusRequest;

    let resp = client
        .get_sandbox_policy_status(GetSandboxPolicyStatusRequest {
            name: name.to_owned(),
            version: 0, // latest
            global: false,
        })
        .await
        .map_err(|e| miette::miette!("GetSandboxPolicyStatus RPC failed: {e:#}"))?;

    let revision = resp.into_inner().revision
        .ok_or_else(|| miette::miette!("GetSandboxPolicyStatus returned no revision"))?;

    Ok(revision.policy)
}
```

Note: `FilesystemPolicy` and `LandlockPolicy` need `PartialEq` derives. Check if the proto-generated types have it. If not, compare field-by-field instead:

```rust
pub fn filesystem_policy_changed(old: &SandboxPolicy, new: &SandboxPolicy) -> bool {
    let fs_changed = match (&old.filesystem, &new.filesystem) {
        (Some(a), Some(b)) => {
            a.include_workdir != b.include_workdir
                || a.read_only != b.read_only
                || a.read_write != b.read_write
        }
        (None, None) => false,
        _ => true,
    };

    let ll_changed = match (&old.landlock, &new.landlock) {
        (Some(a), Some(b)) => a.compatibility != b.compatibility,
        (None, None) => false,
        _ => true,
    };

    fs_changed || ll_changed
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rightclaw filesystem_policy_changed`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/openshell.rs crates/rightclaw/src/openshell_tests.rs
git commit -m "feat: add gRPC policy status query and filesystem policy comparison"
```

---

### Task 7: Implement sandbox migration in `rightclaw agent config`

**Files:**
- Modify: `crates/rightclaw-cli/src/wizard.rs` (sandbox mode change handler)
- Modify: `crates/rightclaw-cli/src/main.rs` (config dispatch)

- [ ] **Step 1: Add migration logic to sandbox mode change in wizard**

When the user changes sandbox mode or network policy in `agent_setting_menu`, after writing the new config, detect if filesystem policy changed and trigger migration.

In `crates/rightclaw-cli/src/wizard.rs`, modify the "Sandbox mode" handler (around line 630) and add a new handler for filesystem policy changes. The key addition: after any policy-affecting change is saved, check if migration is needed.

Add a new public function:

```rust
/// Check if sandbox migration is needed and perform it.
/// Called after agent.yaml changes that might affect filesystem policy.
pub async fn maybe_migrate_sandbox(
    home: &Path,
    agent_name: &str,
) -> miette::Result<()> {
    let agent_dir = rightclaw::config::agents_dir(home).join(agent_name);
    let config = rightclaw::agent::discovery::parse_agent_config(&agent_dir)?;

    let is_sandboxed = config.sandbox.as_ref()
        .map(|s| s.mode == rightclaw::agent::types::SandboxMode::Openshell)
        .unwrap_or(true);

    if !is_sandboxed {
        return Ok(());
    }

    let sandbox = rightclaw::openshell::resolve_sandbox_name(agent_name, &config);

    let mtls_dir = match rightclaw::openshell::preflight_check() {
        rightclaw::openshell::OpenShellStatus::Ready(dir) => dir,
        _ => return Ok(()), // No OpenShell, nothing to migrate
    };

    let mut grpc = rightclaw::openshell::connect_grpc(&mtls_dir).await?;
    if !rightclaw::openshell::is_sandbox_ready(&mut grpc, &sandbox).await? {
        return Ok(()); // Sandbox not running, no migration needed
    }

    // Get active policy from sandbox
    let active_policy = match rightclaw::openshell::get_active_policy(&mut grpc, &sandbox).await? {
        Some(p) => p,
        None => {
            // gRPC doesn't return full policy — fallback: always migrate
            println!("Warning: cannot read active policy from sandbox, assuming migration needed");
            // Fall through to migration
            return perform_migration(home, agent_name, &config, &sandbox).await;
        }
    };

    // Generate new policy and parse it
    let policy_path = config.resolve_policy_path(&agent_dir)?
        .ok_or_else(|| miette::miette!("no policy path resolved"))?;
    let new_policy_yaml = std::fs::read_to_string(&policy_path)
        .map_err(|e| miette::miette!("failed to read policy.yaml: {e:#}"))?;

    // Parse new policy YAML to SandboxPolicy proto.
    // Since generate_policy() outputs YAML and gRPC gives us proto,
    // we need to compare at the proto level. Parse the YAML into the
    // equivalent proto fields.
    let new_policy = rightclaw::openshell::parse_policy_yaml_filesystem(&new_policy_yaml)?;

    if rightclaw::openshell::filesystem_policy_changed(&active_policy, &new_policy) {
        println!("Filesystem policy changed — sandbox migration required.");
        perform_migration(home, agent_name, &config, &sandbox).await?;
    } else {
        println!("Only network policy changed — hot-reload will apply on next bot restart.");
    }

    Ok(())
}
```

- [ ] **Step 2: Implement `perform_migration`**

```rust
async fn perform_migration(
    home: &Path,
    agent_name: &str,
    config: &rightclaw::agent::types::AgentConfig,
    old_sandbox: &str,
) -> miette::Result<()> {
    let agent_dir = rightclaw::config::agents_dir(home).join(agent_name);

    // 1. Backup sandbox-only
    println!("  Step 1/6: Backing up current sandbox...");
    let now = chrono::Local::now();
    let timestamp = now.format("%Y%m%d-%H%M").to_string();
    let backup_dir = rightclaw::config::backups_dir(home, agent_name).join(&timestamp);
    std::fs::create_dir_all(&backup_dir)
        .map_err(|e| miette::miette!("failed to create backup dir: {e:#}"))?;

    let tar_path = backup_dir.join("sandbox.tar.gz");
    let ssh_config_dir = home.join("run").join("ssh");
    let old_ssh_config = ssh_config_dir.join(format!("{old_sandbox}.ssh-config"));
    let old_ssh_alias = rightclaw::openshell::ssh_host_for_sandbox(old_sandbox);

    rightclaw::openshell::ssh_tar_download(
        &old_ssh_config,
        &old_ssh_alias,
        "sandbox",
        &tar_path,
        600,
    ).await?;

    // 2. Create new sandbox
    println!("  Step 2/6: Creating new sandbox...");
    let new_sandbox = format!("rightclaw-{agent_name}-{timestamp}");
    let policy_path = config.resolve_policy_path(&agent_dir)?
        .ok_or_else(|| miette::miette!("no policy path"))?;

    let staging = agent_dir.join("staging");
    rightclaw::openshell::prepare_staging_dir(&agent_dir, &staging)?;

    let mtls_dir = match rightclaw::openshell::preflight_check() {
        rightclaw::openshell::OpenShellStatus::Ready(dir) => dir,
        _ => return Err(miette::miette!("OpenShell not available")),
    };

    let mut child = rightclaw::openshell::spawn_sandbox(
        &new_sandbox,
        &policy_path,
        Some(&staging),
    )?;

    let mut grpc = rightclaw::openshell::connect_grpc(&mtls_dir).await?;

    tokio::select! {
        result = rightclaw::openshell::wait_for_ready(&mut grpc, &new_sandbox, 120, 2) => {
            result?;
            drop(child);
        }
        status = child.wait() => {
            let status = status.map_err(|e| miette::miette!("sandbox create failed: {e:#}"))?;
            if !status.success() {
                return Err(miette::miette!("sandbox create exited with {status}"));
            }
        }
    }

    // 3. Wait for SSH
    println!("  Step 3/6: Waiting for SSH...");
    let sandbox_id = rightclaw::openshell::resolve_sandbox_id(&mut grpc, &new_sandbox).await?;
    rightclaw::openshell::wait_for_ssh(&mut grpc, &sandbox_id, 60, 2).await?;

    // 4. Generate SSH config and restore files
    println!("  Step 4/6: Restoring sandbox files...");
    let new_ssh_config = rightclaw::openshell::generate_ssh_config(&new_sandbox, &ssh_config_dir).await?;
    let new_ssh_alias = rightclaw::openshell::ssh_host_for_sandbox(&new_sandbox);

    if let Err(e) = rightclaw::openshell::ssh_tar_upload(
        &new_ssh_config,
        &new_ssh_alias,
        &tar_path,
        600,
    ).await {
        // Rollback: delete new sandbox, keep old
        eprintln!("  Restore failed: {e:#}");
        eprintln!("  Rolling back — deleting new sandbox...");
        rightclaw::openshell::delete_sandbox(&new_sandbox).await;
        return Err(e);
    }

    // 5. Update agent.yaml with new sandbox name
    println!("  Step 5/6: Updating agent.yaml...");
    crate::wizard::update_agent_yaml_sandbox_name(&agent_dir, &new_sandbox)?;

    // 6. Delete old sandbox
    println!("  Step 6/6: Cleaning up old sandbox...");
    rightclaw::openshell::delete_sandbox(old_sandbox).await;
    // Best-effort: don't fail if old sandbox can't be deleted
    if let Err(e) = rightclaw::openshell::wait_for_deleted(&mut grpc, old_sandbox, 60, 2).await {
        tracing::warn!("old sandbox cleanup incomplete: {e:#}");
    }

    // Remove old SSH config
    let old_config = ssh_config_dir.join(format!("{old_sandbox}.ssh-config"));
    let _ = std::fs::remove_file(&old_config);

    println!("\n  Migration complete: {old_sandbox} → {new_sandbox}");
    println!("  Bot will pick up new sandbox on next restart.");
    Ok(())
}
```

- [ ] **Step 3: Add `parse_policy_yaml_to_proto` helper**

This function parses our generated policy.yaml YAML into the protobuf `SandboxPolicy` type so we can compare with gRPC response. In `crates/rightclaw/src/openshell.rs`:

```rust
/// Parse a policy YAML string into the filesystem-relevant fields of SandboxPolicy.
/// Only populates filesystem and landlock — enough for comparison.
pub fn parse_policy_yaml_filesystem(yaml: &str) -> miette::Result<SandboxPolicy> {
    use crate::openshell_proto::openshell::sandbox::v1::{FilesystemPolicy, LandlockPolicy};

    let doc: serde_saphyr::Value = serde_saphyr::from_str(yaml)
        .map_err(|e| miette::miette!("failed to parse policy YAML: {e:#}"))?;

    let fs_policy = doc.get("filesystem_policy").map(|fs| {
        FilesystemPolicy {
            include_workdir: fs.get("include_workdir")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            read_only: fs.get("read_only")
                .and_then(|v| v.as_sequence())
                .map(|seq| seq.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            read_write: fs.get("read_write")
                .and_then(|v| v.as_sequence())
                .map(|seq| seq.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default(),
        }
    });

    let landlock = doc.get("landlock").map(|ll| {
        LandlockPolicy {
            compatibility: ll.get("compatibility")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(),
        }
    });

    Ok(SandboxPolicy {
        filesystem: fs_policy,
        landlock,
        ..Default::default()
    })
}
```

- [ ] **Step 4: Wire `maybe_migrate_sandbox` into agent config dispatch**

In `crates/rightclaw-cli/src/main.rs`, after the `agent_setting_menu` call returns, check if migration is needed. The challenge: `agent_setting_menu` is sync and `maybe_migrate_sandbox` is async. Use `block_in_place`:

```rust
AgentCommands::Config { name, key, value } => {
    match (key, value) {
        (None, None) => {
            crate::wizard::agent_setting_menu(&home, name.as_deref())?;
            // After config changes, check if sandbox migration needed
            if let Some(agent_name) = name.as_deref() {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(
                        crate::wizard::maybe_migrate_sandbox(&home, agent_name)
                    )
                })?;
            }
        }
        // ... existing get/set handling
    }
    Ok(())
}
```

Note: when `name` is None (interactive selection), we need to capture which agent was selected. This requires a small refactor of `agent_setting_menu` to return the agent name. Alternatively, check all agents. For now, only trigger migration when agent name is explicitly provided.

- [ ] **Step 5: Build and verify**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/ crates/rightclaw/src/openshell.rs
git commit -m "feat: implement sandbox migration on filesystem policy change in agent config"
```

---

### Task 8: Add interactive restore option to agent init wizard

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs` (cmd_agent_init)

- [ ] **Step 1: Add "Restore from backup" option to interactive init**

In `cmd_agent_init` in `main.rs`, after the force/fresh logic and before building `InitOverrides`, add an interactive prompt when running fresh init:

```rust
// After line ~974 (if no saved config and interactive):
if interactive && !force {
    let options = vec!["Create fresh", "Restore from backup"];
    let choice = inquire::Select::new("How to initialize this agent?", options)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

    if choice == "Restore from backup" {
        // Prompt for backup path
        let backup_path = inquire::Text::new("Backup directory path:")
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
        let backup_path = PathBuf::from(backup_path.trim());
        return cmd_agent_restore(&home, name, &backup_path).await;
    }
    // Otherwise continue with fresh creation
}
```

- [ ] **Step 2: Build and verify**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat: add 'Restore from backup' option to interactive agent init wizard"
```

---

### Task 9: Integration test for backup and restore

**Files:**
- Modify: `crates/rightclaw-cli/tests/cli_integration.rs`

- [ ] **Step 1: Write integration test**

Add a test that exercises the backup/restore flow for a no-sandbox agent (doesn't require OpenShell):

```rust
#[test]
fn agent_backup_and_restore_no_sandbox() {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path();

    // Create a no-sandbox agent manually
    let agent_dir = home_path.join("agents").join("test-agent");
    std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
    std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# Test Agent\n").unwrap();
    std::fs::write(agent_dir.join("test-file.txt"), "hello world\n").unwrap();

    // Create a data.db with a table
    let db_path = agent_dir.join("data.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, val TEXT)", []).unwrap();
    conn.execute("INSERT INTO test (val) VALUES ('backup-test')", []).unwrap();
    drop(conn);

    // Run backup
    let output = Command::cargo_bin("rightclaw")
        .unwrap()
        .args(["--home", home_path.to_str().unwrap(), "agent", "backup", "test-agent"])
        .output()
        .unwrap();
    assert!(output.status.success(), "backup failed: {}", String::from_utf8_lossy(&output.stderr));

    // Find backup directory
    let backups_dir = home_path.join("backups").join("test-agent");
    let entries: Vec<_> = std::fs::read_dir(&backups_dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "expected one backup");
    let backup_dir = entries[0].as_ref().unwrap().path();

    assert!(backup_dir.join("sandbox.tar.gz").exists());
    assert!(backup_dir.join("agent.yaml").exists());
    assert!(backup_dir.join("data.db").exists());

    // Delete original agent
    std::fs::remove_dir_all(&agent_dir).unwrap();
    assert!(!agent_dir.exists());

    // Restore into new agent
    let output = Command::cargo_bin("rightclaw")
        .unwrap()
        .args([
            "--home", home_path.to_str().unwrap(),
            "agent", "init", "restored-agent",
            "--from-backup", backup_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "restore failed: {}", String::from_utf8_lossy(&output.stderr));

    // Verify restored files
    let restored_dir = home_path.join("agents").join("restored-agent");
    assert!(restored_dir.join("agent.yaml").exists());
    assert!(restored_dir.join("test-file.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restored_dir.join("test-file.txt")).unwrap(),
        "hello world\n"
    );

    // Verify restored database
    let restored_db = rusqlite::Connection::open(restored_dir.join("data.db")).unwrap();
    let val: String = restored_db
        .query_row("SELECT val FROM test WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(val, "backup-test");
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p rightclaw-cli agent_backup_and_restore_no_sandbox`
Expected: PASS (after implementation is complete)

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/tests/cli_integration.rs
git commit -m "test: integration test for no-sandbox agent backup and restore"
```

---

### Task 10: Update ARCHITECTURE.md and review

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update directory layout**

Add to the Runtime directory layout in ARCHITECTURE.md:

```
~/.rightclaw/
├── backups/<agent>/<YYYYMMDD-HHMM>/
│   ├── sandbox.tar.gz    # Sandbox files (tar czpf -p)
│   ├── agent.yaml        # (full backup only)
│   ├── data.db           # (full backup only, via VACUUM INTO)
│   └── policy.yaml       # (full backup only)
```

- [ ] **Step 2: Update Configuration Hierarchy**

Add `sandbox.name` to the agent.yaml row:

```
| Per-agent | `agents/<name>/agent.yaml` | Restart, model, telegram, sandbox overrides, sandbox.name, env |
```

- [ ] **Step 3: Document sandbox migration in Agent Lifecycle**

Add to the `rightclaw agent config` section:

```
Config change with filesystem policy:
  ├─ Detect filesystem_policy change via gRPC GetSandboxPolicyStatus
  ├─ Backup sandbox-only (SSH tar)
  ├─ Create new sandbox rightclaw-<agent>-<YYYYMMDD-HHMM>
  ├─ Wait for READY + SSH
  ├─ Restore files via SSH tar upload
  ├─ Write sandbox.name to agent.yaml
  ├─ Delete old sandbox (best-effort)
  └─ config_watcher restarts bot → picks up new sandbox
```

- [ ] **Step 4: Run review-rust-code subagent**

Dispatch `rust-dev:review-rust-code` subagent on the changed files.

- [ ] **Step 5: Fix any issues from review**

Address review findings as TODOs, fix one by one.

- [ ] **Step 6: Final build**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: update ARCHITECTURE.md with backup/restore and sandbox migration"
```
