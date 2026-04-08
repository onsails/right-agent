# Per-Agent Sandbox Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the global `--no-sandbox` flag with per-agent `sandbox.mode` in `agent.yaml`, allowing mixed sandboxed/unsandboxed agents in the same `rightclaw up` invocation.

**Architecture:** Replace `SandboxOverrides` with `SandboxConfig { mode, policy_file }` in agent types. Remove `--no-sandbox` from CLI. Each agent declares its own sandbox mode. The bot reads its mode from `agent.yaml` instead of a CLI flag. Process-compose template and codegen derive sandbox behavior per-agent from `AgentConfig`.

**Tech Stack:** Rust (edition 2024), serde-saphyr, clap, minijinja, miette

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `crates/rightclaw/src/agent/types.rs` | Replace `SandboxOverrides` → `SandboxConfig` with `SandboxMode` enum |
| Modify | `crates/rightclaw/src/codegen/process_compose.rs` | Remove global `no_sandbox`, derive per-agent from `AgentConfig` |
| Modify | `crates/rightclaw/src/codegen/process_compose_tests.rs` | Update test helpers and sandbox tests for per-agent config |
| Modify | `templates/process-compose.yaml.j2` | Remove `--no-sandbox` flag, keep per-agent env vars (already per-agent) |
| Modify | `crates/rightclaw-cli/src/main.rs` | Remove `--no-sandbox` from Up/Bot, read sandbox mode from agent config in `cmd_up` |
| Modify | `crates/bot/src/lib.rs` | Remove `BotArgs.no_sandbox`, read sandbox mode from agent.yaml |
| Modify | `crates/rightclaw/src/init.rs` | Add sandbox mode + policy generation to init, extract `init_agent()` |
| Modify | `crates/rightclaw/src/codegen/policy.rs` | No structural changes — called from init wizard instead of cmd_up |
| Modify | `templates/right/agent.yaml` | Add `sandbox:` section to template |

---

### Task 1: Replace `SandboxOverrides` with `SandboxConfig` in agent types

**Files:**
- Modify: `crates/rightclaw/src/agent/types.rs:58-103`

- [ ] **Step 1: Write failing tests for new SandboxConfig deserialization**

Add these tests at the end of the `#[cfg(test)] mod tests` block in `crates/rightclaw/src/agent/types.rs`:

```rust
#[test]
fn sandbox_config_mode_openshell_with_policy() {
    let yaml = r#"
sandbox:
  mode: openshell
  policy_file: policy.yaml
"#;
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let sandbox = config.sandbox.unwrap();
    assert_eq!(sandbox.mode, SandboxMode::Openshell);
    assert_eq!(sandbox.policy_file.as_deref(), Some(std::path::Path::new("policy.yaml")));
}

#[test]
fn sandbox_config_mode_none() {
    let yaml = r#"
sandbox:
  mode: none
"#;
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let sandbox = config.sandbox.unwrap();
    assert_eq!(sandbox.mode, SandboxMode::None);
    assert!(sandbox.policy_file.is_none());
}

#[test]
fn sandbox_config_defaults_to_openshell() {
    let yaml = "sandbox: {}";
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let sandbox = config.sandbox.unwrap();
    assert_eq!(sandbox.mode, SandboxMode::Openshell);
}

#[test]
fn sandbox_config_rejects_unknown_mode() {
    let yaml = r#"
sandbox:
  mode: docker
"#;
    let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
    assert!(result.is_err());
}

#[test]
fn sandbox_config_rejects_old_allow_write_field() {
    let yaml = r#"
sandbox:
  allow_write:
    - "/tmp"
"#;
    let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
    assert!(result.is_err(), "old SandboxOverrides fields must be rejected");
}

#[test]
fn agent_config_without_sandbox_defaults_mode_openshell() {
    let yaml = "{}";
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    // sandbox is None — effective mode should be openshell (tested via helper)
    assert!(config.sandbox.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib agent::types::tests`
Expected: compilation errors — `SandboxMode` doesn't exist yet, `SandboxOverrides` has different fields.

- [ ] **Step 3: Replace SandboxOverrides with SandboxConfig**

In `crates/rightclaw/src/agent/types.rs`, replace the `SandboxOverrides` struct (lines 58-79) with:

```rust
/// Sandbox execution mode for an agent.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    /// Run inside OpenShell container (default — secure).
    #[default]
    Openshell,
    /// Run directly on host (needed for computer-use, Chrome, etc.).
    None,
}

/// Per-agent sandbox configuration in agent.yaml.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxConfig {
    /// Execution mode: openshell (sandboxed) or none (direct host).
    #[serde(default)]
    pub mode: SandboxMode,
    /// Path to OpenShell policy file, relative to agent directory.
    /// Required when mode is openshell.
    pub policy_file: Option<std::path::PathBuf>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Openshell,
            policy_file: Option::None,
        }
    }
}
```

Update the `AgentConfig` field (line 102-103) from:

```rust
    pub sandbox: Option<SandboxOverrides>,
```

to:

```rust
    pub sandbox: Option<SandboxConfig>,
```

Add a helper method on `AgentConfig` to resolve effective sandbox mode:

```rust
impl AgentConfig {
    /// Effective sandbox mode — defaults to Openshell when `sandbox` section is absent.
    pub fn sandbox_mode(&self) -> &SandboxMode {
        self.sandbox
            .as_ref()
            .map(|s| &s.mode)
            .unwrap_or(&SandboxMode::Openshell)
    }

    /// Resolved policy file path (absolute), or None if mode is None.
    /// Returns Err if mode is Openshell but policy_file is missing.
    pub fn resolve_policy_path(&self, agent_dir: &std::path::Path) -> miette::Result<Option<std::path::PathBuf>> {
        match self.sandbox_mode() {
            SandboxMode::None => Ok(Option::None),
            SandboxMode::Openshell => {
                let rel = self.sandbox
                    .as_ref()
                    .and_then(|s| s.policy_file.as_ref())
                    .ok_or_else(|| miette::miette!(
                        help = "Add `sandbox:\\n  policy_file: policy.yaml` to agent.yaml, or set `sandbox:\\n  mode: none`",
                        "agent.yaml has sandbox mode 'openshell' but no policy_file specified"
                    ))?;
                let abs = agent_dir.join(rel);
                if !abs.exists() {
                    return Err(miette::miette!(
                        help = "Run `rightclaw agent init <name>` to generate a default policy, or create the file manually",
                        "policy file not found: {}",
                        abs.display()
                    ));
                }
                Ok(Some(abs))
            }
        }
    }
}
```

- [ ] **Step 4: Remove old sandbox override tests, update remaining tests**

Delete these tests from `crates/rightclaw/src/agent/types.rs` (they test the old `SandboxOverrides` struct):
- `agent_config_with_sandbox_overrides` (line 253)
- `sandbox_overrides_deserializes_allow_read` (line 275)
- `sandbox_overrides_allow_read_defaults_empty` (line 288)
- `sandbox_overrides_empty_section` (line 303)
- `sandbox_overrides_rejects_unknown_fields` (line 314)

Keep `agent_config_without_sandbox_section` (line 296) — it still tests that `sandbox: None` works.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib agent::types::tests`
Expected: all tests pass including the new `SandboxConfig` tests.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/agent/types.rs
git commit -m "refactor: replace SandboxOverrides with SandboxConfig (mode + policy_file)"
```

---

### Task 2: Update process-compose codegen for per-agent sandbox

**Files:**
- Modify: `crates/rightclaw/src/codegen/process_compose.rs:12-86`
- Modify: `crates/rightclaw/src/codegen/process_compose_tests.rs:10-50,396-456`
- Modify: `templates/process-compose.yaml.j2:21,29-36`

- [ ] **Step 1: Write failing tests for per-agent sandbox in process-compose**

In `crates/rightclaw/src/codegen/process_compose_tests.rs`, add a helper and new tests. First, update imports at the top to include the new types:

```rust
use crate::agent::{AgentConfig, AgentDef, RestartPolicy};
use crate::agent::types::{SandboxConfig, SandboxMode};
```

Add a new helper function after `make_agent_with_restart`:

```rust
fn make_agent_with_sandbox(name: &str, token: &str, mode: SandboxMode, policy_file: Option<&str>) -> AgentDef {
    let config = Some(AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        network_policy: Default::default(),
        model: None,
        sandbox: Some(SandboxConfig {
            mode,
            policy_file: policy_file.map(std::path::PathBuf::from),
        }),
        telegram_token: Some(token.to_string()),
        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
        secret: None,
        attachments: Default::default(),
    });
    AgentDef {
        name: name.to_owned(),
        path: PathBuf::from(format!("/home/user/.rightclaw/agents/{name}")),
        identity_path: PathBuf::from(format!("/home/user/.rightclaw/agents/{name}/IDENTITY.md")),
        config,
        soul_path: None,
        user_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }
}
```

Add new tests:

```rust
#[test]
fn per_agent_sandbox_openshell_emits_openshell_mode() {
    let agents = vec![make_agent_with_sandbox(
        "sandboxed", "123:tok", SandboxMode::Openshell, Some("policy.yaml"),
    )];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, &default_config()).unwrap();
    assert!(
        output.contains("RC_SANDBOX_MODE=openshell"),
        "expected RC_SANDBOX_MODE=openshell for openshell agent:\n{output}"
    );
    assert!(
        output.contains("RC_SANDBOX_POLICY=/home/user/.rightclaw/agents/sandboxed/policy.yaml"),
        "expected policy path from agent config:\n{output}"
    );
    assert!(
        !output.contains("--no-sandbox"),
        "--no-sandbox must not appear for sandboxed agent:\n{output}"
    );
}

#[test]
fn per_agent_sandbox_none_emits_none_mode() {
    let agents = vec![make_agent_with_sandbox(
        "unsandboxed", "123:tok", SandboxMode::None, None,
    )];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, &default_config()).unwrap();
    assert!(
        output.contains("RC_SANDBOX_MODE=none"),
        "expected RC_SANDBOX_MODE=none for unsandboxed agent:\n{output}"
    );
    assert!(
        !output.contains("RC_SANDBOX_POLICY"),
        "RC_SANDBOX_POLICY must be absent for unsandboxed agent:\n{output}"
    );
}

#[test]
fn mixed_sandbox_modes_in_same_config() {
    let agents = vec![
        make_agent_with_sandbox("sandboxed", "123:tok", SandboxMode::Openshell, Some("policy.yaml")),
        make_agent_with_sandbox("direct", "456:tok", SandboxMode::None, None),
    ];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, &default_config()).unwrap();
    // Sandboxed agent has openshell mode
    assert!(output.contains("sandboxed-bot:"));
    assert!(output.contains("direct-bot:"));
    // Both modes present in output
    assert!(output.contains("RC_SANDBOX_MODE=openshell"));
    assert!(output.contains("RC_SANDBOX_MODE=none"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib codegen::tests`
Expected: compilation errors — `ProcessComposeConfig` still has `no_sandbox` field, `SandboxConfig`/`SandboxMode` not imported.

- [ ] **Step 3: Update ProcessComposeConfig and BotProcessAgent**

In `crates/rightclaw/src/codegen/process_compose.rs`:

Remove the `no_sandbox` field from `ProcessComposeConfig` (line 67):

```rust
pub struct ProcessComposeConfig<'a> {
    pub debug: bool,
    pub run_dir: &'a Path,
    pub home: &'a Path,
    pub cloudflared_script: Option<&'a Path>,
    pub token_map_path: Option<&'a Path>,
}
```

Update the `BotProcessAgent` struct — replace `no_sandbox: bool` (line 30) with `sandbox_mode: String`:

```rust
    /// Sandbox mode: "openshell" or "none".
    sandbox_mode: String,
```

Update the `generate_process_compose` function. Replace the destructuring (line 86):

```rust
let &ProcessComposeConfig { debug, run_dir, home, cloudflared_script, token_map_path } = config;
```

Update the bot_agents loop (lines 124-148). Replace the `no_sandbox` and `sandbox_policy_path` assignment:

```rust
            let agent_config = agent.config.as_ref()?;

            // No telegram token configured — skip this agent.
            let token_inline = agent_config.telegram_token.clone();
            token_inline.as_ref()?;

            let (restart, backoff, max) = (
                restart_policy_str(&agent_config.restart),
                agent_config.backoff_seconds,
                agent_config.max_restarts,
            );

            let sandbox_mode = match agent_config.sandbox_mode() {
                crate::agent::types::SandboxMode::Openshell => "openshell",
                crate::agent::types::SandboxMode::None => "none",
            };

            let sandbox_policy_path = match agent_config.sandbox_mode() {
                crate::agent::types::SandboxMode::Openshell => {
                    agent_config.sandbox
                        .as_ref()
                        .and_then(|s| s.policy_file.as_ref())
                        .map(|rel| agent.path.join(rel).display().to_string())
                }
                crate::agent::types::SandboxMode::None => None,
            };

            Some(BotProcessAgent {
                name: agent.name.clone(),
                agent_name: agent.name.clone(),
                exe_path: exe_path.display().to_string(),
                working_dir: agent.path.display().to_string(),
                token_inline,
                restart_policy: restart.to_owned(),
                backoff_seconds: backoff,
                max_restarts: max,
                debug,
                sandbox_mode: sandbox_mode.to_owned(),
                sandbox_policy_path,
                home_dir: home.display().to_string(),
            })
```

- [ ] **Step 4: Update process-compose.yaml.j2 template**

In `templates/process-compose.yaml.j2`, replace line 21:

```jinja2
    command: "{{ agent.exe_path }} bot --agent {{ agent.agent_name }}{% if agent.debug %} --debug{% endif %}"
```

Replace lines 29-36 (the sandbox env var block):

```jinja2
{% if agent.sandbox_mode == "openshell" %}
      - RC_SANDBOX_MODE=openshell
{% if agent.sandbox_policy_path %}
      - RC_SANDBOX_POLICY={{ agent.sandbox_policy_path }}
{% endif %}
{% else %}
      - RC_SANDBOX_MODE=none
{% endif %}
```

- [ ] **Step 5: Update test helpers in process_compose_tests.rs**

In `crates/rightclaw/src/codegen/process_compose_tests.rs`:

Update `default_config()` — remove `no_sandbox`:

```rust
fn default_config() -> ProcessComposeConfig<'static> {
    ProcessComposeConfig {
        debug: false,
        run_dir: Path::new("/tmp/run"),
        home: Path::new("/home/user/.rightclaw"),
        cloudflared_script: None,
        token_map_path: None,
    }
}
```

Update `make_bot_agent` — set sandbox to `SandboxMode::None` (so existing tests that don't care about sandbox continue working):

```rust
fn make_bot_agent(name: &str, token: &str) -> AgentDef {
    let config = Some(AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        network_policy: Default::default(),
        model: None,
        sandbox: Some(SandboxConfig {
            mode: SandboxMode::None,
            policy_file: None,
        }),
        telegram_token: Some(token.to_string()),
        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
        secret: None,
        attachments: Default::default(),
    });
    // ... rest unchanged
```

Do the same for `make_agent_no_token`, `make_agent_with_restart` — set `sandbox: Some(SandboxConfig { mode: SandboxMode::None, policy_file: None })`.

Delete the old global sandbox tests:
- `no_sandbox_true_emits_sandbox_mode_none` (line 396)
- `no_sandbox_false_emits_openshell_mode_and_policy_path` (line 412)
- `no_sandbox_false_command_lacks_no_sandbox_flag` (line 431)
- `no_sandbox_true_command_has_no_sandbox_flag` (line 446)

Update tests that referenced `--no-sandbox` in their assertions (e.g., `bot_agent_command_contains_rightclaw_bot_agent` line 144 — the command no longer contains `--no-sandbox`).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib codegen::tests`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/codegen/process_compose.rs crates/rightclaw/src/codegen/process_compose_tests.rs templates/process-compose.yaml.j2
git commit -m "refactor: per-agent sandbox mode in process-compose codegen"
```

---

### Task 3: Remove `--no-sandbox` from CLI and update `cmd_up`

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs:147-160,209-219,322,391-399,529-535,543-588,763-774,802-822,905-916`

- [ ] **Step 1: Remove `--no-sandbox` from Up command**

In `crates/rightclaw-cli/src/main.rs`, remove the `no_sandbox` field from the `Up` variant (line 154-156):

```rust
    Up {
        /// Only launch specific agents (comma-separated)
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        /// Launch in background with TUI server
        #[arg(short, long)]
        detach: bool,
        /// Enable debug logging
        #[arg(long)]
        debug: bool,
    },
```

- [ ] **Step 2: Remove `--no-sandbox` from Bot command**

Remove the `no_sandbox` field from the `Bot` variant (line 217-218):

```rust
    Bot {
        /// Agent name (resolves to $RIGHTCLAW_HOME/agents/<name>/)
        #[arg(long)]
        agent: String,
        /// Pass --verbose to CC subprocess and log CC stderr at debug level
        #[arg(long)]
        debug: bool,
    },
```

- [ ] **Step 3: Update command dispatch**

Update the `Up` match arm (around line 322) — remove `no_sandbox` from destructuring:

```rust
        Commands::Up { agents, detach, debug } => cmd_up(&home, agents, detach, debug).await,
```

Update the `Bot` match arm (around line 391) — remove `no_sandbox`:

```rust
        Commands::Bot { agent, debug } => {
            rightclaw_bot::run(rightclaw_bot::BotArgs {
                agent,
                home: cli.home,
                debug,
            })
            .await
        }
```

- [ ] **Step 4: Update `cmd_up` signature and OpenShell preflight**

Update `cmd_up` signature (line 529) — remove `no_sandbox`:

```rust
async fn cmd_up(
    home: &Path,
    agents_filter: Option<Vec<String>>,
    detach: bool,
    debug: bool,
) -> miette::Result<()> {
```

Replace the OpenShell preflight block (lines 543-588). Instead of checking a global flag, check if any agent needs sandbox:

```rust
    // Pre-flight: when any agent uses OpenShell, verify it's ready.
    let any_sandboxed = agents.iter().any(|a| {
        a.config
            .as_ref()
            .map(|c| matches!(c.sandbox_mode(), rightclaw::agent::types::SandboxMode::Openshell))
            .unwrap_or(true) // default is openshell
    });

    if any_sandboxed {
        match rightclaw::openshell::preflight_check() {
            // ... keep the existing match arms but update help text:
            // remove "or use `rightclaw up --no-sandbox`" suggestions
```

Update the help strings in the error messages to say `"Set sandbox mode to 'none' in agent.yaml"` instead of `"use rightclaw up --no-sandbox"`.

- [ ] **Step 5: Update MCP URL determination (per-agent)**

Replace the MCP URL block (lines 763-774). Instead of one global `no_sandbox` check, determine per-agent:

```rust
        // 12. Generate mcp.json with right HTTP MCP server entry.
        let bearer_token = rightclaw::mcp::derive_token(&agent_secret, "right-mcp")?;
        let mcp_port = rightclaw::runtime::MCP_HTTP_PORT;
        let agent_sandbox_mode = agent.config.as_ref()
            .map(|c| c.sandbox_mode().clone())
            .unwrap_or_default();
        let right_mcp_url = match agent_sandbox_mode {
            rightclaw::agent::types::SandboxMode::None => format!("http://127.0.0.1:{mcp_port}/mcp"),
            rightclaw::agent::types::SandboxMode::Openshell => format!("http://host.docker.internal:{mcp_port}/mcp"),
        };
```

- [ ] **Step 6: Replace global policy generation with per-agent validation**

Replace the policy generation block (lines 802-822):

```rust
    // Validate policy files exist for all sandboxed agents.
    for agent in &agents {
        if let Some(config) = agent.config.as_ref() {
            // resolve_policy_path validates existence for openshell agents
            config.resolve_policy_path(&agent.path)?;
        }
    }
```

- [ ] **Step 7: Update process-compose config construction**

Update the `ProcessComposeConfig` construction (lines 905-916) — remove `no_sandbox`:

```rust
    let pc_config = rightclaw::codegen::generate_process_compose(
        &agents,
        &self_exe,
        &rightclaw::codegen::ProcessComposeConfig {
            debug,
            run_dir: &run_dir,
            home,
            cloudflared_script: cloudflared_script_path.as_deref(),
            token_map_path: Some(&token_map_path),
        },
    )?;
```

- [ ] **Step 8: Build to verify compilation**

Run: `cargo build --workspace`
Expected: compiles successfully (bot crate will have errors — handled in Task 4).

Note: This step may fail due to `BotArgs.no_sandbox` still existing. That's expected — Task 4 fixes it. If needed, temporarily comment out the bot dispatch to verify CLI compilation.

- [ ] **Step 9: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "refactor: remove --no-sandbox from CLI, per-agent sandbox in cmd_up"
```

---

### Task 4: Update bot to read sandbox mode from agent.yaml

**Files:**
- Modify: `crates/bot/src/lib.rs:12-21,210-341`

- [ ] **Step 1: Remove `no_sandbox` from BotArgs**

In `crates/bot/src/lib.rs`, update `BotArgs` (lines 12-21):

```rust
pub struct BotArgs {
    /// Agent name (directory name under $RIGHTCLAW_HOME/agents/).
    pub agent: String,
    /// Override for RIGHTCLAW_HOME (from --home flag).
    pub home: Option<String>,
    /// Pass --verbose to CC subprocess and log CC stderr at debug level.
    pub debug: bool,
}
```

- [ ] **Step 2: Read sandbox mode from agent.yaml in run_async**

In `run_async()`, after the agent config is loaded (the bot already reads agent.yaml), determine sandbox mode from it. Find where `config` is parsed from agent.yaml and add:

```rust
    let sandbox_mode = config.sandbox_mode().clone();
    let is_sandboxed = matches!(sandbox_mode, rightclaw::agent::types::SandboxMode::Openshell);
```

- [ ] **Step 3: Replace all `!args.no_sandbox` checks with `is_sandboxed`**

Replace every occurrence of `!args.no_sandbox` in `crates/bot/src/lib.rs` with `is_sandboxed`:

- Line 210: `if !args.no_sandbox` → `if is_sandboxed`
- Line 236: `if !args.no_sandbox` → `if is_sandboxed`
- Line 317: `if !args.no_sandbox` → `if is_sandboxed`
- Line 332: `if !args.no_sandbox` → `if is_sandboxed`

- [ ] **Step 4: Replace RC_SANDBOX_POLICY env var with agent.yaml policy resolution**

Replace the policy path reading (line 238):

```rust
    let ssh_config_path: Option<std::path::PathBuf> = if is_sandboxed {
        let policy_path = config.resolve_policy_path(&agent_dir)?
            .expect("resolve_policy_path returns Some for openshell mode");
```

Remove the `RC_SANDBOX_POLICY` env var read.

- [ ] **Step 5: Update error messages**

Replace `"restart with rightclaw up --no-sandbox"` suggestions (lines 249, 255, 261) with:

```rust
help = "Set `sandbox:\\n  mode: none` in agent.yaml, or install OpenShell"
```

- [ ] **Step 6: Build workspace**

Run: `cargo build --workspace`
Expected: compiles successfully.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/lib.rs crates/rightclaw-cli/src/main.rs
git commit -m "refactor: bot reads sandbox mode from agent.yaml instead of CLI flag"
```

---

### Task 5: Update `rightclaw init` and add `rightclaw agent init`

**Files:**
- Modify: `crates/rightclaw/src/init.rs:25-156`
- Modify: `crates/rightclaw-cli/src/main.rs` (AgentCommands enum, cmd_init)
- Modify: `templates/right/agent.yaml`

- [ ] **Step 1: Write failing test for init generating policy.yaml**

In `crates/rightclaw/src/init.rs`, add to the test module:

```rust
#[test]
fn init_generates_policy_yaml_for_openshell_mode() {
    let dir = tempdir().unwrap();
    init_agent(
        &dir.path().join("agents"),
        "test-agent",
        Some("123456:ABCdef"),
        &[],
        &NetworkPolicy::Permissive,
        &rightclaw::agent::types::SandboxMode::Openshell,
    )
    .unwrap();

    let policy_path = dir.path().join("agents/test-agent/policy.yaml");
    assert!(policy_path.exists(), "policy.yaml must be generated for openshell mode");
    let content = std::fs::read_to_string(&policy_path).unwrap();
    assert!(content.contains("version: 1"), "policy must be valid OpenShell format");
}

#[test]
fn init_skips_policy_yaml_for_none_mode() {
    let dir = tempdir().unwrap();
    init_agent(
        &dir.path().join("agents"),
        "test-agent",
        Some("123456:ABCdef"),
        &[],
        &NetworkPolicy::Permissive,
        &rightclaw::agent::types::SandboxMode::None,
    )
    .unwrap();

    let policy_path = dir.path().join("agents/test-agent/policy.yaml");
    assert!(!policy_path.exists(), "policy.yaml must NOT exist for none mode");
}

#[test]
fn init_writes_sandbox_mode_to_agent_yaml() {
    let dir = tempdir().unwrap();
    init_agent(
        &dir.path().join("agents"),
        "test-agent",
        None,
        &[],
        &NetworkPolicy::Permissive,
        &rightclaw::agent::types::SandboxMode::None,
    )
    .unwrap();

    let yaml = std::fs::read_to_string(dir.path().join("agents/test-agent/agent.yaml")).unwrap();
    assert!(yaml.contains("mode: none"), "agent.yaml must contain sandbox mode: none");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib init::tests`
Expected: compilation error — `init_agent` doesn't exist yet.

- [ ] **Step 3: Extract `init_agent` from `init_rightclaw_home`**

In `crates/rightclaw/src/init.rs`, create a new `init_agent` function that contains the core agent initialization logic. Then `init_rightclaw_home` calls it for the "right" agent:

```rust
/// Initialize a single agent directory under `agents_parent_dir`.
///
/// Creates `agents_parent_dir/<name>/` with template files and configuration.
/// Generates an OpenShell policy.yaml when `sandbox_mode` is `Openshell`.
pub fn init_agent(
    agents_parent_dir: &Path,
    name: &str,
    telegram_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    network_policy: &NetworkPolicy,
    sandbox_mode: &crate::agent::types::SandboxMode,
) -> miette::Result<std::path::PathBuf> {
    let agents_dir = agents_parent_dir.join(name);

    if agents_dir.exists() {
        return Err(miette::miette!(
            "Agent '{}' already exists at {}.",
            name,
            agents_dir.display()
        ));
    }

    std::fs::create_dir_all(&agents_dir).map_err(|e| {
        miette::miette!("Failed to create directory {}: {}", agents_dir.display(), e)
    })?;

    // Create staging directory for OpenShell upload workflow
    let staging_dir = agents_dir.join("staging");
    std::fs::create_dir_all(&staging_dir)
        .map_err(|e| miette::miette!("Failed to create staging dir: {e}"))?;

    let files: &[(&str, &str)] = &[
        ("IDENTITY.md", DEFAULT_IDENTITY),
        ("SOUL.md", DEFAULT_SOUL),
        ("USER.md", DEFAULT_USER),
        ("AGENTS.md", DEFAULT_AGENTS),
        ("BOOTSTRAP.md", DEFAULT_BOOTSTRAP),
        ("agent.yaml", DEFAULT_AGENT_YAML),
    ];

    for (filename, content) in files {
        let path = agents_dir.join(filename);
        std::fs::write(&path, content)
            .map_err(|e| miette::miette!("Failed to write {}: {}", path.display(), e))?;
    }

    // Install built-in skills
    crate::codegen::install_builtin_skills(&agents_dir)?;

    let host_home = dirs::home_dir()
        .ok_or_else(|| miette::miette!("cannot determine home directory"))?;

    // Generate .claude/settings.json
    {
        let settings = crate::codegen::generate_settings()?;
        let claude_dir = agents_dir.join(".claude");
        std::fs::create_dir_all(&claude_dir).map_err(|e| {
            miette::miette!("Failed to create {}: {}", claude_dir.display(), e)
        })?;
        std::fs::write(
            claude_dir.join("settings.json"),
            serde_json::to_string_pretty(&settings)
                .map_err(|e| miette::miette!("Failed to serialize settings: {e}"))?,
        )
        .map_err(|e| miette::miette!("Failed to write settings.json: {}", e))?;
    }

    // Write sandbox config + network policy to agent.yaml
    {
        let agent_yaml_path = agents_dir.join("agent.yaml");
        let mut yaml = std::fs::read_to_string(&agent_yaml_path)
            .map_err(|e| miette::miette!("Failed to read agent.yaml: {}", e))?;

        let policy_str = match network_policy {
            NetworkPolicy::Restrictive => "restrictive",
            NetworkPolicy::Permissive => "permissive",
        };
        yaml.push_str(&format!("\nnetwork_policy: {policy_str}\n"));

        // Write sandbox configuration
        match sandbox_mode {
            crate::agent::types::SandboxMode::Openshell => {
                yaml.push_str("\nsandbox:\n  mode: openshell\n  policy_file: policy.yaml\n");
            }
            crate::agent::types::SandboxMode::None => {
                yaml.push_str("\nsandbox:\n  mode: none\n");
            }
        }

        std::fs::write(&agent_yaml_path, &yaml)
            .map_err(|e| miette::miette!("Failed to update agent.yaml: {}", e))?;
    }

    // Generate OpenShell policy.yaml when sandboxed
    if matches!(sandbox_mode, crate::agent::types::SandboxMode::Openshell) {
        let policy_yaml = crate::codegen::policy::generate_policy(
            crate::runtime::MCP_HTTP_PORT,
            network_policy,
        );
        std::fs::write(agents_dir.join("policy.yaml"), &policy_yaml)
            .map_err(|e| miette::miette!("Failed to write policy.yaml: {e}"))?;
    }

    // Write Telegram token inline into agent.yaml
    if let Some(token) = telegram_token {
        let agent_yaml_path = agents_dir.join("agent.yaml");
        let mut yaml = std::fs::read_to_string(&agent_yaml_path)
            .map_err(|e| miette::miette!("Failed to read agent.yaml: {}", e))?;
        yaml.push_str(&format!("\ntelegram_token: \"{token}\"\n"));
        if !telegram_allowed_chat_ids.is_empty() {
            yaml.push_str("\nallowed_chat_ids:\n");
            for id in telegram_allowed_chat_ids {
                yaml.push_str(&format!("  - {id}\n"));
            }
        }
        std::fs::write(&agent_yaml_path, yaml)
            .map_err(|e| miette::miette!("Failed to update agent.yaml: {}", e))?;
    }

    // Pre-trust the agent directory
    let trust_agent = crate::agent::AgentDef {
        name: name.to_owned(),
        path: agents_dir.clone(),
        identity_path: agents_dir.join("IDENTITY.md"),
        config: None,
        soul_path: None,
        user_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    };
    crate::codegen::generate_agent_claude_json(&trust_agent)?;
    crate::codegen::create_credential_symlink(&trust_agent, &host_home)?;

    Ok(agents_dir)
}
```

Then simplify `init_rightclaw_home` to call `init_agent`:

```rust
pub fn init_rightclaw_home(
    home: &Path,
    telegram_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    network_policy: &NetworkPolicy,
    sandbox_mode: &crate::agent::types::SandboxMode,
) -> miette::Result<()> {
    let agents_parent = home.join("agents");
    if agents_parent.join("right").exists() {
        return Err(miette::miette!(
            "RightClaw home already initialized at {}. Use `rightclaw config` to change settings.",
            agents_parent.join("right").display()
        ));
    }

    let agents_dir = init_agent(
        &agents_parent,
        "right",
        telegram_token,
        telegram_allowed_chat_ids,
        network_policy,
        sandbox_mode,
    )?;

    println!("Created RightClaw home at {}", home.display());
    println!("  agents/right/IDENTITY.md");
    println!("  agents/right/SOUL.md");
    println!("  agents/right/USER.md");
    println!("  agents/right/AGENTS.md");
    println!("  agents/right/BOOTSTRAP.md");
    println!("  agents/right/agent.yaml");
    println!("  agents/right/.claude/skills/rightskills/SKILL.md  (skills.sh manager)");
    println!("  agents/right/.claude/skills/rightcron/SKILL.md");
    if matches!(sandbox_mode, crate::agent::types::SandboxMode::Openshell) {
        println!("  agents/right/policy.yaml (OpenShell policy)");
    }

    if telegram_token.is_some() {
        println!("  Telegram bot token saved");
        println!("  agents/right/.claude/settings.json (Telegram plugin enabled)");
    }

    Ok(())
}
```

- [ ] **Step 4: Update existing init tests**

Update all `init_rightclaw_home` calls in tests to pass the new `sandbox_mode` parameter. Most tests should use `&SandboxMode::Openshell`:

```rust
init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell).unwrap();
```

Import `SandboxMode` at the top of the test module:

```rust
use crate::agent::types::{NetworkPolicy, SandboxMode};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rightclaw --lib init::tests`
Expected: all tests pass.

- [ ] **Step 6: Add `agent init` subcommand to CLI**

In `crates/rightclaw-cli/src/main.rs`, add `Init` to `AgentCommands`:

```rust
pub enum AgentCommands {
    /// Initialize a new agent
    Init {
        /// Agent name (alphanumeric + hyphens)
        name: String,
        /// Non-interactive mode
        #[arg(short = 'y', long)]
        yes: bool,
        /// Network policy: restrictive or permissive
        #[arg(long)]
        network_policy: Option<rightclaw::agent::types::NetworkPolicy>,
        /// Sandbox mode: openshell or none
        #[arg(long)]
        sandbox_mode: Option<String>,
    },
    /// Configure an agent interactively
    Config {
        // ... existing fields
    },
}
```

Add the match arm for the new command:

```rust
Commands::Agent {
    command: AgentCommands::Init { name, yes, network_policy, sandbox_mode },
} => {
    let interactive = !yes;

    // Sandbox mode: CLI flag > interactive prompt > openshell (default).
    let sandbox = match sandbox_mode.as_deref() {
        Some("none") => rightclaw::agent::types::SandboxMode::None,
        Some("openshell") | None if !interactive => rightclaw::agent::types::SandboxMode::Openshell,
        None => {
            // Interactive prompt
            use std::io::{self, Write};
            println!("Sandbox mode:");
            println!("  1. OpenShell — run in isolated container (recommended)");
            println!("  2. None — run directly on host (for computer-use, Chrome, etc.)");
            print!("Choose [1/2] (default: 1): ");
            io::stdout().flush().map_err(|e| miette::miette!("flush: {e}"))?;
            let mut input = String::new();
            io::stdin().read_line(&mut input).map_err(|e| miette::miette!("read: {e}"))?;
            match input.trim() {
                "" | "1" => rightclaw::agent::types::SandboxMode::Openshell,
                "2" => rightclaw::agent::types::SandboxMode::None,
                other => return Err(miette::miette!("Invalid choice: '{other}'")),
            }
        }
        Some(other) => return Err(miette::miette!("Invalid sandbox mode: '{other}'. Expected 'openshell' or 'none'.")),
    };

    // Network policy (only relevant for openshell)
    let net_policy = if matches!(sandbox, rightclaw::agent::types::SandboxMode::None) {
        rightclaw::agent::types::NetworkPolicy::Permissive // doesn't matter for none mode
    } else {
        match network_policy {
            Some(p) => p,
            None if !interactive => rightclaw::agent::types::NetworkPolicy::Restrictive,
            None => rightclaw::init::prompt_network_policy()?,
        }
    };

    // Telegram token
    let token = if !interactive { None } else { crate::wizard::telegram_setup(None, true)? };
    let chat_ids: Vec<i64> = if interactive && token.is_some() {
        crate::wizard::chat_ids_setup()?
    } else {
        vec![]
    };

    let agents_dir = home.join("agents");
    rightclaw::init::init_agent(
        &agents_dir,
        &name,
        token.as_deref(),
        &chat_ids,
        &net_policy,
        &sandbox,
    )?;

    println!("Agent '{}' created at {}/agents/{}/", name, home.display(), name);
    Ok(())
},
```

- [ ] **Step 7: Update `cmd_init` to pass sandbox mode and use `init_agent` via `init_rightclaw_home`**

Update `cmd_init` (around line 403) to add sandbox mode prompt/flag handling. Add a sandbox mode prompt before calling `init_rightclaw_home`:

```rust
    // Sandbox mode: interactive prompt > openshell (default for --yes).
    let sandbox = if !interactive {
        rightclaw::agent::types::SandboxMode::Openshell
    } else {
        use std::io::{self, Write};
        println!("Sandbox mode for the default 'right' agent:");
        println!("  1. OpenShell — run in isolated container (recommended)");
        println!("  2. None — run directly on host (for computer-use, Chrome, etc.)");
        print!("Choose [1/2] (default: 1): ");
        io::stdout().flush().map_err(|e| miette::miette!("flush: {e}"))?;
        let mut input = String::new();
        io::stdin().read_line(&mut input).map_err(|e| miette::miette!("read: {e}"))?;
        match input.trim() {
            "" | "1" => rightclaw::agent::types::SandboxMode::Openshell,
            "2" => rightclaw::agent::types::SandboxMode::None,
            other => return Err(miette::miette!("Invalid choice: '{other}'")),
        }
    };

    rightclaw::init::init_rightclaw_home(home, token.as_deref(), &chat_ids, &network_policy, &sandbox)?;
```

- [ ] **Step 8: Build workspace**

Run: `cargo build --workspace`
Expected: compiles.

- [ ] **Step 9: Commit**

```bash
git add crates/rightclaw/src/init.rs crates/rightclaw-cli/src/main.rs templates/right/agent.yaml
git commit -m "feat: add rightclaw agent init, sandbox mode in init wizard"
```

---

### Task 6: Update agent.yaml template

**Files:**
- Modify: `templates/right/agent.yaml`

- [ ] **Step 1: Add sandbox section to template**

Update `templates/right/agent.yaml` to include the sandbox section:

```yaml
# Agent configuration for the "Right" agent
# See: https://github.com/onsails/rightclaw

# Model to use (sonnet, opus, haiku)
model: sonnet

# Restart policy: on_failure, always, never
restart: on_failure

# Maximum restart attempts
max_restarts: 5

# Seconds to wait between restarts
backoff_seconds: 10

# Sandbox mode: openshell (container isolation) or none (direct host access)
# sandbox:
#   mode: openshell
#   policy_file: policy.yaml

# Network policy: restrictive = Anthropic/Claude only, permissive = all HTTPS
# network_policy: restrictive

# Per-agent environment variables injected before exec claude.
# Values are single-quoted literals — no shell expansion, no host variable forwarding.
# WARNING: values are stored in plaintext in agent.yaml. Do not store secrets here.
# env:
#   MY_VAR: "literal value"
#   ANOTHER_VAR: "also literal"

# Per-agent secret for Bearer token derivation.
# Auto-generated by rightclaw. Do not edit.
# secret: <auto-generated on first rightclaw up>
```

Note: The sandbox section is commented out because `init_agent` appends the actual values dynamically.

- [ ] **Step 2: Commit**

```bash
git add templates/right/agent.yaml
git commit -m "docs: add sandbox config section to agent.yaml template"
```

---

### Task 7: Clean up policy codegen and remove RC_SANDBOX env vars from bot

**Files:**
- Modify: `crates/bot/src/lib.rs` (remove RC_SANDBOX_POLICY env var read)
- Modify: `crates/rightclaw/src/codegen/policy.rs` (no structural change, just verify it's still called from init)

- [ ] **Step 1: Verify bot no longer reads RC_SANDBOX_POLICY**

This was done in Task 4 Step 4. Verify with grep:

Run: `rg "RC_SANDBOX_POLICY" crates/`
Expected: no matches (template still has it — that's fine, it's generated per-agent from agent config now).

Actually, the template still emits `RC_SANDBOX_POLICY` — the bot still reads it. Let's keep this env var for now as a convenient transport mechanism from process-compose to the bot process. The key change is that the *value* comes from the agent's config (per-agent) rather than a global flag. This is already handled in Task 2.

Check if bot reads it from env or from agent.yaml — after Task 4, it should read from agent.yaml.

Run: `rg "RC_SANDBOX" crates/bot/`
Expected: no matches if Task 4 was done correctly.

- [ ] **Step 2: Remove stale run/policies/ generation references**

Verify that `cmd_up` no longer writes to `run/policies/`. This was done in Task 3 Step 6.

Run: `rg "run.*policies" crates/rightclaw-cli/src/main.rs`
Expected: no matches.

- [ ] **Step 3: Full test suite**

Run: `cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 4: Commit (if any cleanup needed)**

```bash
git add -A
git commit -m "chore: remove stale sandbox env var references"
```

---

### Task 8: Integration test for mixed-mode agents

**Files:**
- Create: `crates/rightclaw/src/codegen/sandbox_integration_tests.rs` (or add to existing test file)

- [ ] **Step 1: Write integration test**

Add to `crates/rightclaw/src/codegen/process_compose_tests.rs`:

```rust
#[test]
fn mixed_mode_agents_correct_env_vars() {
    let agents = vec![
        make_agent_with_sandbox("coder", "111:tok", SandboxMode::Openshell, Some("policy.yaml")),
        make_agent_with_sandbox("browser", "222:tok", SandboxMode::None, None),
        make_agent_with_sandbox("reviewer", "333:tok", SandboxMode::Openshell, Some("custom-policy.yaml")),
    ];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, &default_config()).unwrap();

    // coder: sandboxed
    assert!(output.contains("coder-bot:"));
    assert!(output.contains("RC_SANDBOX_POLICY=/home/user/.rightclaw/agents/coder/policy.yaml"));

    // browser: unsandboxed
    assert!(output.contains("browser-bot:"));
    // browser should NOT have RC_SANDBOX_POLICY
    // Check the browser section specifically
    let browser_section = output.split("browser-bot:").nth(1).unwrap();
    let browser_section = browser_section.split("-bot:").next().unwrap_or(browser_section);
    assert!(!browser_section.contains("RC_SANDBOX_POLICY"), "browser must not have policy path");

    // reviewer: sandboxed with custom policy
    assert!(output.contains("RC_SANDBOX_POLICY=/home/user/.rightclaw/agents/reviewer/custom-policy.yaml"));
}

#[test]
fn agent_without_sandbox_config_defaults_to_openshell_in_process_compose() {
    // Agent with sandbox: None in config (field absent from yaml → None)
    let config = Some(AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        network_policy: Default::default(),
        model: None,
        sandbox: None, // absent from yaml → default openshell
        telegram_token: Some("123:tok".to_string()),
        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
        secret: None,
        attachments: Default::default(),
    });
    let agents = vec![AgentDef {
        name: "default-agent".to_owned(),
        path: PathBuf::from("/home/user/.rightclaw/agents/default-agent"),
        identity_path: PathBuf::from("/home/user/.rightclaw/agents/default-agent/IDENTITY.md"),
        config,
        soul_path: None,
        user_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, &default_config()).unwrap();
    assert!(
        output.contains("RC_SANDBOX_MODE=openshell"),
        "agent without explicit sandbox config should default to openshell:\n{output}"
    );
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rightclaw --lib codegen::tests`
Expected: all pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/process_compose_tests.rs
git commit -m "test: add mixed-mode and default sandbox integration tests"
```

---

### Task 9: Update ARCHITECTURE.md and design spec

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `docs/superpowers/specs/2026-04-09-per-agent-sandbox-design.md`

- [ ] **Step 1: Update ARCHITECTURE.md**

Update the "Configuration Hierarchy" table to reflect that sandbox config lives in agent.yaml, not as a CLI flag. Remove `RC_SANDBOX_MODE` and `RC_SANDBOX_POLICY` from the env var documentation (or note they're derived from agent.yaml). Update the "Agent Lifecycle" section for `rightclaw up` to mention per-agent sandbox validation instead of global policy generation.

Add `rightclaw agent init` to the CLI section.

Update the Module Map for `init.rs` to mention `init_agent()`.

- [ ] **Step 2: Update design spec status**

Change status from "Approved" to "Implemented" in `docs/superpowers/specs/2026-04-09-per-agent-sandbox-design.md`.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md docs/superpowers/specs/2026-04-09-per-agent-sandbox-design.md
git commit -m "docs: update architecture for per-agent sandbox"
```

---

### Task 10: Final workspace build and full test run

- [ ] **Step 1: Build full workspace**

Run: `cargo build --workspace`
Expected: compiles with no errors.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace`
Expected: no warnings.

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 4: Final commit (if any fixes needed)**

```bash
git add -A
git commit -m "chore: fix clippy warnings and test failures from per-agent sandbox refactor"
```
