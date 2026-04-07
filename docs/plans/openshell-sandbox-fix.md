# Fix OpenShell Integration: Move Sandbox Lifecycle to Per-Agent Bot Process

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Problem:** OpenShell sandbox lifecycle was incorrectly placed in `cmd_up`/`cmd_down` (the CLI orchestrator), making it a hard prerequisite that blocks process-compose + cloudflared tunnel from starting. OpenShell should only wrap `claude -p` calls.

**Goal:** Move sandbox lifecycle to each per-agent bot process. Two modes: default = OpenShell sandbox, `--no-sandbox` = direct `claude -p` calls. CC native sandbox (bubblewrap/Seatbelt) is removed entirely — always `--dangerously-skip-permissions`.

**Architecture after fix:**
```
rightclaw up [--no-sandbox]
  -> discover agents, generate configs (+ policy.yaml if sandbox)
  -> generate process-compose.yaml (passes RC_SANDBOX_MODE + RC_SANDBOX_POLICY)
  -> start process-compose
      -> cloudflared tunnel
      -> rightclaw bot --agent right [--no-sandbox]  <- per-agent process
          -> IF sandbox mode:
              -> connect gRPC, create sandbox, wait_for_ready, generate SSH config
          -> teloxide long-polling
          -> on message: invoke_cc()
              -> IF sandbox: ssh -F config host -- claude -p ...
              -> IF no-sandbox: claude -p ... (direct)
          -> on shutdown:
              -> IF sandbox: delete_sandbox (best-effort)
```

---

### Task 1: Remove OpenShell lifecycle from cmd_up and cmd_down

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`

**What to do:**

- [ ] **Step 1: Remove the OpenShell sandbox lifecycle block from cmd_up**

Delete the entire block (currently around lines 882-934) that does:
- `connect_grpc(&mtls_dir)`
- `create_dir_all(&ssh_config_dir)`
- `create_dir_all(&policy_dir)`
- Per-agent loop: `delete_sandbox`, `generate_policy`, `spawn_sandbox`, `wait_for_ready`, `generate_ssh_config`

Keep everything before (per-agent config generation) and after (cloudflared + process-compose).

- [ ] **Step 2: Remove RIGHTMEMORY_PORT constant**

Delete `const RIGHTMEMORY_PORT: u16 = 8100;` and its `#[allow(dead_code)]`. This will move to bot process later.

- [ ] **Step 3: Remove sandbox deletion from cmd_down**

In `cmd_down`, remove the loop that iterates `state.agents` and calls `openshell::delete_sandbox`. Revert `state` back to `_state` if nothing else uses it.

- [ ] **Step 4: Add policy generation for sandbox mode**

In `cmd_up`, after the per-agent config loop but before process-compose generation, add conditional policy generation:

```rust
// Generate OpenShell policies when sandbox mode is active.
if !no_sandbox {
    let policy_dir = run_dir.join("policies");
    std::fs::create_dir_all(&policy_dir)
        .map_err(|e| miette::miette!("failed to create policy dir: {e:#}"))?;

    for agent in &agents {
        let policy_yaml = rightclaw::codegen::policy::generate_policy(8100, &[]);
        let policy_path = policy_dir.join(format!("{}.yaml", agent.name));
        std::fs::write(&policy_path, &policy_yaml)
            .map_err(|e| miette::miette!("failed to write policy for '{}': {e:#}", agent.name))?;
    }
}
```

- [ ] **Step 5: Pass `no_sandbox` to process-compose generation**

Add `no_sandbox: bool` parameter to `generate_process_compose()` and thread it to the template context via `BotProcessAgent`. The template will use it to emit `RC_SANDBOX_MODE` env var and `--no-sandbox` CLI flag.

- [ ] **Step 6: Verify compilation**

Run: `cargo check --workspace`

- [ ] **Step 7: Commit**

Message: "refactor: remove OpenShell sandbox lifecycle from cmd_up/cmd_down"

---

### Task 2: Update process-compose to pass sandbox mode

**Files:**
- Modify: `crates/rightclaw/src/codegen/process_compose.rs`
- Modify: `templates/process-compose.yaml.j2`

- [ ] **Step 1: Add sandbox fields to BotProcessAgent and generate_process_compose**

Add to `BotProcessAgent`:
```rust
/// When true, sandbox is disabled (direct claude -p calls).
no_sandbox: bool,
/// Absolute path to the generated OpenShell policy.yaml for this agent.
/// None when no_sandbox is true.
sandbox_policy_path: Option<String>,
```

Add `no_sandbox: bool` and `run_dir: &Path` parameters to `generate_process_compose()`. For each agent, compute policy path:
```rust
no_sandbox,
sandbox_policy_path: if no_sandbox {
    None
} else {
    Some(run_dir.join("policies").join(format!("{}.yaml", agent.name)).display().to_string())
},
```

- [ ] **Step 2: Update template to emit sandbox env vars and CLI flag**

In `templates/process-compose.yaml.j2`, update the command line and environment:

```jinja
    command: "{{ agent.exe_path }} bot --agent {{ agent.agent_name }}{% if agent.no_sandbox %} --no-sandbox{% endif %}{% if agent.debug %} --debug{% endif %}"
    ...
    environment:
      ...
{% if not agent.no_sandbox %}
      - RC_SANDBOX_MODE=openshell
{% if agent.sandbox_policy_path %}
      - RC_SANDBOX_POLICY={{ agent.sandbox_policy_path }}
{% endif %}
{% else %}
      - RC_SANDBOX_MODE=none
{% endif %}
```

- [ ] **Step 3: Update all callers of generate_process_compose**

Update the call in `cmd_up` and any test code to pass the new parameters.

- [ ] **Step 4: Update tests**

Update process_compose_tests.rs to cover the new fields.

- [ ] **Step 5: Verify and commit**

Run: `cargo test -p rightclaw -- process_compose`
Message: "feat: pass sandbox mode + policy path through process-compose to bot"

---

### Task 3: Add --no-sandbox flag to BotArgs and bot CLI

**Files:**
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/rightclaw-cli/src/main.rs` (CLI definition + dispatch)

- [ ] **Step 1: Add no_sandbox to BotArgs**

In `crates/bot/src/lib.rs`, add field to `BotArgs`:
```rust
pub struct BotArgs {
    pub agent: String,
    pub home: Option<String>,
    pub debug: bool,
    /// Disable OpenShell sandbox — invoke claude -p directly.
    pub no_sandbox: bool,
}
```

- [ ] **Step 2: Add --no-sandbox to Bot CLI variant**

In `crates/rightclaw-cli/src/main.rs`, Commands::Bot:
```rust
Bot {
    #[arg(long)]
    agent: String,
    #[arg(long)]
    debug: bool,
    /// Disable OpenShell sandbox (direct claude -p calls)
    #[arg(long)]
    no_sandbox: bool,
},
```

Update the dispatch:
```rust
Commands::Bot { agent, debug, no_sandbox } => {
    rightclaw_bot::run(rightclaw_bot::BotArgs {
        agent,
        home: cli.home,
        debug,
        no_sandbox,
    })
    .await
}
```

- [ ] **Step 3: Verify and commit**

Run: `cargo check --workspace`
Message: "feat: add --no-sandbox flag to rightclaw bot CLI"

---

### Task 4: Remove CC native sandbox from settings.json

**Files:**
- Modify: `crates/rightclaw/src/codegen/settings.rs`
- Modify: `crates/rightclaw/src/codegen/settings_tests.rs`
- Modify: `crates/rightclaw-cli/src/main.rs` (caller)

**Rationale:** CC native sandbox (bubblewrap/Seatbelt) is no longer used. We always run with `--dangerously-skip-permissions`. OpenShell is the security layer when sandboxed.

- [ ] **Step 1: Simplify generate_settings**

Remove the `no_sandbox` parameter. Remove the entire `sandbox` section from the generated JSON. Keep only behavioral flags:
```rust
pub fn generate_settings(
    agent: &AgentDef,
    chrome_config: Option<&ChromeConfig>,
) -> miette::Result<serde_json::Value> {
    let mut settings = serde_json::json!({
        "skipDangerousModePermissionPrompt": true,
        "spinnerTipsEnabled": false,
        "prefersReducedMotion": true,
        "autoMemoryEnabled": false,
    });

    // Chrome profile path still needed for Chrome MCP integration.
    // (if Chrome is configured, CC needs to know the profile dir)
    // ... keep chrome-related settings if any exist ...

    Ok(settings)
}
```

Remove `host_home` and `rg_path` parameters — they were only used for sandbox config.

- [ ] **Step 2: Remove generate_settings_minimal**

Delete `generate_settings_minimal()` — it's now identical to the simplified `generate_settings`.

- [ ] **Step 3: Update caller in cmd_up**

Change:
```rust
let settings = rightclaw::codegen::generate_settings(agent, no_sandbox, &host_home, rg_path.clone(), chrome_cfg)?;
```
To:
```rust
let settings = rightclaw::codegen::generate_settings(agent, chrome_cfg)?;
```

Remove `rg_path` resolution and `host_home` computation if no longer needed elsewhere.

- [ ] **Step 4: Update tests**

Update settings_tests.rs — remove tests about sandbox.enabled, filesystem deny/allow, network domains, ripgrep injection. Add/keep tests for behavioral flags and chrome settings.

- [ ] **Step 5: Verify and commit**

Run: `cargo test -p rightclaw -- settings`
Message: "refactor: remove CC native sandbox from settings.json — OpenShell is the security layer"

---

### Task 5: Wire OpenShell sandbox lifecycle into bot startup/shutdown

**Files:**
- Modify: `crates/bot/src/lib.rs`

This is the core change. The bot process now manages its own sandbox.

- [ ] **Step 1: Remove SSH config hard fail**

Delete the block in `run_async` that computes `ssh_config_path` from `home.join("run/ssh/...")` and returns error if file doesn't exist. The bot will create its own SSH config.

- [ ] **Step 2: Add sandbox initialization on startup**

In `run_async`, after resolving agent config but before starting teloxide, add conditional sandbox setup:

```rust
let ssh_config_path: Option<PathBuf> = if !args.no_sandbox {
    // Read policy path from env (generated by cmd_up).
    let policy_path = std::env::var("RC_SANDBOX_POLICY")
        .map(PathBuf::from)
        .map_err(|_| miette::miette!("RC_SANDBOX_POLICY not set — run rightclaw up first"))?;

    let sandbox = rightclaw::openshell::sandbox_name(&args.agent);

    // mTLS certs dir (default or env override).
    let mtls_dir = std::env::var("OPENSHELL_MTLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("/etc"))
                .join("openshell/gateways/openshell/mtls")
        });

    // Delete stale sandbox (best-effort).
    rightclaw::openshell::delete_sandbox(&sandbox).await;

    // Spawn sandbox.
    let upload_dir = agent_dir.join("staging");
    let mut child = rightclaw::openshell::spawn_sandbox(&sandbox, &policy_path, Some(&upload_dir))?;

    // Connect gRPC and wait for ready, racing against child exit.
    let mut grpc_client = rightclaw::openshell::connect_grpc(&mtls_dir).await?;
    tokio::select! {
        result = rightclaw::openshell::wait_for_ready(&mut grpc_client, &sandbox, 120, 2) => {
            result?;
        }
        status = child.wait() => {
            let status = status.map_err(|e| miette::miette!("sandbox create child wait failed: {e:#}"))?;
            if !status.success() {
                return Err(miette::miette!(
                    "openshell sandbox create for '{}' exited with {status} before reaching READY",
                    args.agent
                ));
            }
        }
    }

    // Generate SSH config.
    let ssh_config_dir = home.join("run").join("ssh");
    std::fs::create_dir_all(&ssh_config_dir)
        .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
    let config_path = rightclaw::openshell::generate_ssh_config(&sandbox, &ssh_config_dir).await?;
    tracing::info!(agent = %args.agent, "OpenShell sandbox ready");

    Some(config_path)
} else {
    None
};
```

- [ ] **Step 3: Add sandbox cleanup on shutdown**

Wrap the `tokio::select!` block (teloxide + axum) with a cleanup guard. After the select returns, delete sandbox:

```rust
let result = tokio::select! {
    result = telegram::run_telegram(...) => result,
    result = axum_handle => result.map_err(|e| miette::miette!("axum task panicked: {e:#}"))?,
};

// Cleanup sandbox on shutdown (best-effort).
if !args.no_sandbox {
    let sandbox = rightclaw::openshell::sandbox_name(&args.agent);
    rightclaw::openshell::delete_sandbox(&sandbox).await;
}

result
```

- [ ] **Step 4: Update run_telegram call**

Change the `ssh_config_path: PathBuf` parameter to `ssh_config_path: Option<PathBuf>`.

- [ ] **Step 5: Verify compilation**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

Message: "feat: bot process manages its own OpenShell sandbox lifecycle"

---

### Task 6: Make SSH-based invoke_cc conditional on sandbox mode

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Make SshConfigPath optional**

In `handler.rs`, change `SshConfigPath(pub PathBuf)` to `SshConfigPath(pub Option<PathBuf>)`.

In `dispatch.rs`:
- Change `ssh_config_path: PathBuf` param to `ssh_config_path: Option<PathBuf>`
- `Arc::new(SshConfigPath(ssh_config_path))` — works with Option directly

In `handle_message`:
- `ssh_config_path: ssh_config.0.clone()` — now `Option<PathBuf>`

- [ ] **Step 2: Make WorkerContext.ssh_config_path optional**

In `worker.rs`:
```rust
pub struct WorkerContext {
    ...
    /// Path to SSH config. Some = OpenShell sandbox, None = direct exec.
    pub ssh_config_path: Option<PathBuf>,
}
```

- [ ] **Step 3: Branch invoke_cc based on ssh_config_path**

In `invoke_cc`, after building the claude args, branch:

```rust
let mut cmd = if let Some(ref ssh_config) = ctx.ssh_config_path {
    // OpenShell sandbox: exec via SSH
    let ssh_host = rightclaw::openshell::ssh_host(&ctx.agent_name);
    let mut c = tokio::process::Command::new("ssh");
    c.arg("-F").arg(ssh_config);
    c.arg(&ssh_host);
    c.arg("--");
    c.arg(cc_bin.display().to_string());
    for arg in &claude_args {
        c.arg(arg);
    }
    c
} else {
    // Direct exec (no sandbox)
    let mut c = tokio::process::Command::new(&cc_bin);
    for arg in &claude_args {
        c.arg(arg);
    }
    c.env("HOME", &ctx.agent_dir);
    c.env("USE_BUILTIN_RIPGREP", "0");
    c.current_dir(&ctx.agent_dir);
    c
};
cmd.stdin(Stdio::null());
cmd.stdout(Stdio::piped());
cmd.stderr(Stdio::piped());
cmd.kill_on_drop(true);
```

Note: in SSH mode, HOME/env/cwd are set inside the sandbox, not on the host command. In direct mode, they're set on the host command (like master).

- [ ] **Step 4: Restore claude binary resolution for direct mode**

The current code on this branch removed `which::which("claude")`. Restore it for the direct exec path. SSH mode doesn't need it (claude is found inside the sandbox).

Actually — both paths need the cc_bin path. SSH mode passes it as an argument to SSH. Direct mode uses it as the command. Keep the `which::which` resolution.

- [ ] **Step 5: Verify and commit**

Run: `cargo check --workspace`
Message: "feat: conditional SSH vs direct invoke_cc based on sandbox mode"

---

### Task 7: Full test suite + clippy

**Files:** none (verification only)

- [ ] **Step 1: Run full test suite**

```bash
cargo test --workspace
```

Expected: all tests pass (except pre-existing `test_status_no_running_instance`).

- [ ] **Step 2: Run clippy**

```bash
cargo clippy --workspace --tests -- -D warnings
```

Fix any new warnings.

- [ ] **Step 3: Manual smoke test**

```bash
rm -rf ~/.rightclaw && cargo run --bin rightclaw -- init -y \
  --tunnel-hostname right.example.com \
  --telegram-token <TOKEN> \
  --telegram-allowed-chat-ids <ID> && \
cargo run --bin rightclaw -- up --no-sandbox --debug
```

Expected: process-compose starts with cloudflared tunnel and bot agent. Bot receives messages, invokes claude -p directly.

- [ ] **Step 4: Commit any fixes**

Message: "fix: test and clippy fixes for OpenShell sandbox refactor"

---

## Files summary

| File | Action |
|------|--------|
| `crates/rightclaw-cli/src/main.rs` | Remove sandbox lifecycle from cmd_up/cmd_down, keep policy gen, add no_sandbox to Bot variant |
| `crates/bot/src/lib.rs` | Add sandbox lifecycle on startup/shutdown, remove SSH config hard fail |
| `crates/bot/src/telegram/worker.rs` | Conditional SSH vs direct invoke_cc, ssh_config_path becomes Option |
| `crates/bot/src/telegram/handler.rs` | SshConfigPath wraps Option<PathBuf> |
| `crates/bot/src/telegram/dispatch.rs` | ssh_config_path param becomes Option<PathBuf> |
| `crates/rightclaw/src/codegen/settings.rs` | Remove sandbox section entirely, simplify params |
| `crates/rightclaw/src/codegen/settings_tests.rs` | Update for simplified settings |
| `crates/rightclaw/src/codegen/process_compose.rs` | Add no_sandbox + policy path fields |
| `crates/rightclaw/src/codegen/process_compose_tests.rs` | Update for new fields |
| `templates/process-compose.yaml.j2` | Add RC_SANDBOX_MODE env + --no-sandbox flag |

**Files NOT changed (keep as-is):**
- `crates/rightclaw/src/openshell.rs` — correct, just called from new place
- `crates/rightclaw/src/openshell_tests.rs` — correct
- `crates/rightclaw/src/codegen/policy.rs` — correct
- `crates/rightclaw/src/doctor.rs` — always checks OpenShell (per decision 7)
- `crates/rightclaw/build.rs` — needed for proto compilation
- `proto/openshell/*.proto` — needed for gRPC client
