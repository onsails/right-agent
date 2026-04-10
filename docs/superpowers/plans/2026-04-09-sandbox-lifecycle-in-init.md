# Sandbox Lifecycle in Init — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move sandbox creation from bot startup to `rightclaw init` / `rightclaw agent init`, so stale sandboxes are detected at init time and fresh sandboxes are created with correct files.

**Architecture:** Extract sandbox creation logic from bot lib.rs into a shared `ensure_sandbox` function in openshell.rs. Wire it into init flows in main.rs. Bot startup fails if sandbox is missing. Doctor checks sandbox existence.

**Tech Stack:** Rust, tokio, tonic (gRPC), rightclaw::openshell

---

### Task 1: Extract `ensure_sandbox` into openshell.rs

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs`

- [ ] **Step 1: Add `ensure_sandbox` function**

At the end of `crates/rightclaw/src/openshell.rs` (before the `#[cfg(test)]` block at line 394), add:

```rust
/// Outcome of `ensure_sandbox` — tells the caller what happened.
#[derive(Debug, PartialEq)]
pub enum SandboxOutcome {
    /// A new sandbox was created.
    Created,
    /// An existing sandbox was deleted and a new one was created.
    Recreated,
}

/// Create a sandbox, handling the case where one already exists.
///
/// - If no sandbox exists: create it, wait for READY, return `Created`.
/// - If sandbox exists and `force_recreate` is true: delete + create, return `Recreated`.
/// - If sandbox exists and `force_recreate` is false: return error.
///
/// `staging_dir`: optional directory to upload into the sandbox at creation time.
pub async fn ensure_sandbox(
    agent_name: &str,
    policy_path: &Path,
    staging_dir: Option<&Path>,
    force_recreate: bool,
) -> miette::Result<SandboxOutcome> {
    let sandbox = sandbox_name(agent_name);

    // Preflight: check OpenShell availability.
    let mtls_dir = match preflight_check() {
        OpenShellStatus::Ready(dir) => dir,
        OpenShellStatus::NotInstalled => {
            return Err(miette::miette!(
                help = "Install OpenShell and run `openshell auth login`,\n  \
                        or use `--sandbox-mode none` to run without a sandbox",
                "OpenShell is required for sandbox mode 'openshell'"
            ));
        }
        OpenShellStatus::NoGateway(_) => {
            return Err(miette::miette!(
                help = "Run `openshell gateway start`,\n  \
                        or use `--sandbox-mode none`",
                "OpenShell gateway is not running"
            ));
        }
        OpenShellStatus::BrokenGateway(dir) => {
            return Err(miette::miette!(
                help = "Try `openshell gateway destroy && openshell gateway start`,\n  \
                        or use `--sandbox-mode none`",
                "OpenShell gateway exists but mTLS certificates are missing at {}",
                dir.display()
            ));
        }
    };

    let mut grpc_client = connect_grpc(&mtls_dir).await?;
    let exists = is_sandbox_ready(&mut grpc_client, &sandbox).await?;

    if exists && !force_recreate {
        return Err(miette::miette!(
            help = "Use --force to recreate the sandbox,\n  \
                    or `rightclaw agent config` to update an existing agent",
            "Sandbox '{sandbox}' already exists"
        ));
    }

    if exists {
        tracing::info!(sandbox = %sandbox, "deleting existing sandbox for recreate");
        delete_sandbox(&sandbox).await;
    }

    tracing::info!(sandbox = %sandbox, "creating sandbox");
    let mut child = spawn_sandbox(&sandbox, policy_path, staging_dir)?;

    tokio::select! {
        result = wait_for_ready(&mut grpc_client, &sandbox, 120, 2) => {
            result?;
            drop(child);
        }
        status = child.wait() => {
            let status = status.map_err(|e| miette::miette!("sandbox create child wait failed: {e:#}"))?;
            if !status.success() {
                return Err(miette::miette!(
                    "openshell sandbox create for '{}' exited with {status} before reaching READY",
                    agent_name
                ));
            }
        }
    }

    let outcome = if exists { SandboxOutcome::Recreated } else { SandboxOutcome::Created };
    tracing::info!(sandbox = %sandbox, ?outcome, "sandbox ready");
    Ok(outcome)
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p rightclaw`
Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/openshell.rs
git commit -m "feat: extract ensure_sandbox into openshell module

Shared function for sandbox create/recreate with preflight checks,
gRPC readiness polling, and force-recreate support. Will be called
by init and agent-init commands."
```

---

### Task 2: Move `prepare_staging_dir` from bot to rightclaw core

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs`
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Move `prepare_staging_dir` and `copy_dir_resolve_symlinks` to openshell.rs**

Copy the two functions from `crates/bot/src/lib.rs` (lines 400-493) into `crates/rightclaw/src/openshell.rs` before the `ensure_sandbox` function. Make them `pub`:

```rust
/// Prepare a staging directory for sandbox creation.
///
/// Copies curated files from the agent directory into `upload_dir`:
/// - `.claude/settings.json`, `.claude/reply-schema.json`, `.claude/agents/`
/// - `.claude/skills/{rightskills,cronsync}/` (builtin only)
/// - `.claude.json`, `mcp.json`
/// Excludes: credentials, plugins, shell-snapshots, user-installed skills.
pub fn prepare_staging_dir(agent_dir: &Path, upload_dir: &Path) -> miette::Result<()> {
    // ... exact copy of the function body from bot/src/lib.rs lines 400-466
}

/// Recursively copy a directory, resolving symlinks to regular files.
pub fn copy_dir_resolve_symlinks(src: &Path, dst: &Path) -> std::io::Result<()> {
    // ... exact copy from bot/src/lib.rs lines 470-493
}
```

- [ ] **Step 2: Update bot lib.rs to use the moved functions**

In `crates/bot/src/lib.rs`, replace the local `prepare_staging_dir` call (line 293) with:

```rust
rightclaw::openshell::prepare_staging_dir(&agent_dir, &upload_dir)?;
```

Delete the local `prepare_staging_dir` and `copy_dir_resolve_symlinks` functions (lines 400-493).

- [ ] **Step 3: Verify compilation**

Run: `cargo check --workspace`
Expected: Compiles without errors.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/openshell.rs crates/bot/src/lib.rs
git commit -m "refactor: move prepare_staging_dir to rightclaw core

Needed by both bot (existing) and init (new sandbox creation)."
```

---

### Task 3: Wire sandbox creation into `rightclaw agent init`

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs:525-697` (`cmd_agent_init`)

- [ ] **Step 1: Add sandbox creation after `init_agent`**

In `cmd_agent_init` (line 683), after `init_agent` returns and before the println, add sandbox creation for openshell agents:

```rust
    let agent_dir = rightclaw::init::init_agent(&agents_parent, name, Some(&overrides))?;

    // Create sandbox for openshell agents.
    if matches!(overrides.sandbox_mode, rightclaw::agent::types::SandboxMode::Openshell) {
        // Run codegen first so staging dir has agent defs, settings, etc.
        let self_exe = std::env::current_exe()
            .unwrap_or_else(|_| std::path::PathBuf::from("rightclaw"));
        let agent_def = rightclaw::agent::AgentDef {
            name: name.to_string(),
            path: agent_dir.clone(),
            identity_path: agent_dir.join("IDENTITY.md"),
            config: rightclaw::agent::discovery::parse_agent_config(&agent_dir)?,
            soul_path: None,
            user_path: None,
            agents_path: Some(agent_dir.join("AGENTS.md")),
            tools_path: None,
            bootstrap_path: Some(agent_dir.join("BOOTSTRAP.md")),
            heartbeat_path: None,
        };
        rightclaw::codegen::run_agent_codegen(
            home, &[agent_def.clone()], &[agent_def], &self_exe, false,
        )?;

        // Prepare staging dir and create sandbox.
        let staging = agent_dir.join("staging");
        rightclaw::openshell::prepare_staging_dir(&agent_dir, &staging)?;

        let policy_path = agent_dir.join("policy.yaml");
        println!("Creating OpenShell sandbox...");
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                rightclaw::openshell::ensure_sandbox(
                    name,
                    &policy_path,
                    Some(&staging),
                    force, // --force flag controls sandbox recreate
                )
                .await
            })
        })?;
        println!("  Sandbox '{}' ready", rightclaw::openshell::sandbox_name(name));

        // Generate SSH config.
        let ssh_config_dir = home.join("run").join("ssh");
        std::fs::create_dir_all(&ssh_config_dir)
            .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                rightclaw::openshell::generate_ssh_config(
                    &rightclaw::openshell::sandbox_name(name),
                    &ssh_config_dir,
                )
                .await
            })
        })?;
    }
```

- [ ] **Step 2: Handle stale sandbox in non-force case**

When `ensure_sandbox` returns the "already exists" error and `--force` is not set, the error message already tells the user what to do. No additional handling needed — the `?` propagation surfaces the message.

But for `cmd_agent_init` WITHOUT `--force`, the agent directory doesn't exist yet (no `--force` means the agent is new). In this case, if a stale sandbox exists from a previous `rightclaw init` that was deleted, `ensure_sandbox(name, ..., false)` will error.

Fix: pass `force_recreate: true` when the agent dir didn't exist (fresh init always creates sandbox):

```rust
        let force_sandbox = force || !agent_dir_existed_before;
```

Add a boolean before the force wipe block to track this:

At line 536 (after `let agent_dir = agents_parent.join(name);`), add:

```rust
    let agent_existed = agent_dir.exists();
```

Then in the sandbox creation block, use:

```rust
                    force || !agent_existed, // fresh agent init always creates sandbox
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p rightclaw-cli`
Expected: Compiles without errors.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat: create sandbox during agent init

For openshell agents, runs codegen then creates the sandbox with
staging dir upload. Stale sandboxes are recreated automatically
for fresh agents or with --force."
```

---

### Task 4: Wire sandbox creation into `rightclaw init`

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs:430-523` (init command handler)

- [ ] **Step 1: Add sandbox creation after `init_rightclaw_home`**

In the init handler (around line 493), after `init_rightclaw_home` and the success printlns, add sandbox creation:

```rust
    rightclaw::init::init_rightclaw_home(home, token.as_deref(), &chat_ids, &network_policy, &sandbox)?;

    // Create sandbox for the default "right" agent if openshell mode.
    if matches!(sandbox, rightclaw::agent::types::SandboxMode::Openshell) {
        let agent_dir = home.join("agents/right");
        let self_exe = std::env::current_exe()
            .unwrap_or_else(|_| std::path::PathBuf::from("rightclaw"));
        let agent_def = rightclaw::agent::AgentDef {
            name: "right".to_string(),
            path: agent_dir.clone(),
            identity_path: agent_dir.join("IDENTITY.md"),
            config: rightclaw::agent::discovery::parse_agent_config(&agent_dir)?,
            soul_path: None,
            user_path: None,
            agents_path: Some(agent_dir.join("AGENTS.md")),
            tools_path: None,
            bootstrap_path: Some(agent_dir.join("BOOTSTRAP.md")),
            heartbeat_path: None,
        };
        rightclaw::codegen::run_agent_codegen(
            home, &[agent_def.clone()], &[agent_def], &self_exe, false,
        )?;

        let staging = agent_dir.join("staging");
        rightclaw::openshell::prepare_staging_dir(&agent_dir, &staging)?;

        let policy_path = agent_dir.join("policy.yaml");
        println!("Creating OpenShell sandbox...");
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                rightclaw::openshell::ensure_sandbox(
                    "right",
                    &policy_path,
                    Some(&staging),
                    true, // rightclaw init always recreates if stale sandbox exists
                )
                .await
            })
        })?;
        println!("  Sandbox '{}' ready", rightclaw::openshell::sandbox_name("right"));

        let ssh_config_dir = home.join("run").join("ssh");
        std::fs::create_dir_all(&ssh_config_dir)
            .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                rightclaw::openshell::generate_ssh_config(
                    &rightclaw::openshell::sandbox_name("right"),
                    &ssh_config_dir,
                )
                .await
            })
        })?;
    }
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p rightclaw-cli`
Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat: create sandbox during rightclaw init

Default 'right' agent gets its sandbox created at init time.
Always recreates if stale sandbox exists from previous init."
```

---

### Task 5: Remove sandbox creation from bot startup

**Files:**
- Modify: `crates/bot/src/lib.rs:246-326`

- [ ] **Step 1: Replace create-or-reuse with require-existing**

Replace the sandbox lifecycle block (lines 281-314) with:

```rust
        // Require sandbox to already exist (created by `rightclaw init` or `rightclaw agent init`).
        let mut grpc_client = rightclaw::openshell::connect_grpc(&mtls_dir).await?;
        let sandbox_exists = rightclaw::openshell::is_sandbox_ready(&mut grpc_client, &sandbox).await?;

        if !sandbox_exists {
            return Err(miette::miette!(
                help = "Run `rightclaw init` or `rightclaw agent init {}` to create the sandbox",
                "Sandbox '{}' not found",
                args.agent, sandbox
            ));
        }

        // Reuse existing sandbox — apply updated policy.
        tracing::info!(agent = %args.agent, "reusing existing sandbox");
        rightclaw::openshell::apply_policy(&sandbox, &policy_path).await?;
```

Remove the `prepare_staging_dir` call, `spawn_sandbox`, `wait_for_ready`, and the `tokio::select!` block.

- [ ] **Step 2: Keep `prepare_staging_dir` function available**

The function was already moved to `rightclaw::openshell` in Task 2. Remove the local copies from lib.rs if not already done.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles without errors.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "fix: bot requires existing sandbox, no longer creates one

Sandbox creation is now handled by rightclaw init / agent init.
Bot startup fails with helpful error if sandbox is missing."
```

---

### Task 6: Add sandbox check to doctor

**Files:**
- Modify: `crates/rightclaw/src/doctor.rs`

- [ ] **Step 1: Add sandbox existence check**

In `check_agent_structure` (or as a new helper called from `run_doctor`), for each agent with `sandbox: mode: openshell`, add a check:

```rust
fn check_sandbox_for_agent(agent_name: &str) -> Option<DoctorCheck> {
    // Only check if OpenShell is available.
    let mtls_dir = match crate::openshell::preflight_check() {
        crate::openshell::OpenShellStatus::Ready(dir) => dir,
        _ => return None, // OpenShell not ready — skip sandbox check
    };

    let sandbox = crate::openshell::sandbox_name(agent_name);

    // Use a temporary runtime for the async gRPC call.
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let mut client = crate::openshell::connect_grpc(&mtls_dir).await?;
            crate::openshell::is_sandbox_ready(&mut client, &sandbox).await
        })
    });

    match result {
        Ok(true) => Some(DoctorCheck {
            name: format!("sandbox/{agent_name}"),
            status: CheckStatus::Pass,
            detail: format!("sandbox '{sandbox}' exists and READY"),
            fix: None,
        }),
        Ok(false) => Some(DoctorCheck {
            name: format!("sandbox/{agent_name}"),
            status: CheckStatus::Fail,
            detail: format!("sandbox '{sandbox}' not found"),
            fix: Some(format!("Run `rightclaw agent init {agent_name}` to create it")),
        }),
        Err(e) => Some(DoctorCheck {
            name: format!("sandbox/{agent_name}"),
            status: CheckStatus::Warn,
            detail: format!("sandbox check failed: {e:#}"),
            fix: None,
        }),
    }
}
```

Call this from the agent structure check loop, for agents with openshell sandbox mode.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p rightclaw`
Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/doctor.rs
git commit -m "feat: doctor checks sandbox existence for openshell agents

Reports FAIL if sandbox is missing, with fix hint to run agent init."
```

---

### Task 7: Build workspace and verify

**Files:** None (verification only)

- [ ] **Step 1: Build full workspace**

Run: `cargo build --workspace`
Expected: Clean build.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace`
Expected: No new warnings.

- [ ] **Step 3: Run all non-ignored tests**

Run: `cargo test --workspace`
Expected: All pass (except known `reload_fails_when_not_running` environmental issue).
