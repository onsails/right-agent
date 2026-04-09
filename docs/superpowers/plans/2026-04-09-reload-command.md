# `rightclaw reload` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `rightclaw reload` command that hot-updates a running process-compose instance after agent changes, and update `agent init` to suggest it.

**Architecture:** Extract the per-agent codegen loop from `cmd_up` into a shared `run_agent_codegen()` function in the core crate. Add `reload_configuration()` to `PcClient`. Wire both into a new `cmd_reload` in the CLI. Update `cmd_agent_init` to print a reload hint.

**Tech Stack:** Rust (edition 2024), clap, reqwest, miette, process-compose REST API

**Spec:** `docs/superpowers/specs/2026-04-09-reload-command-design.md`

---

### Task 1: Add `reload_configuration` to PcClient

**Files:**
- Modify: `crates/rightclaw/src/runtime/pc_client.rs:150-158` (before `shutdown` method)
- Modify: `crates/rightclaw/src/runtime/pc_client_tests.rs` (add test)

- [ ] **Step 1: Write the failing test**

Add to `crates/rightclaw/src/runtime/pc_client_tests.rs`:

```rust
#[test]
fn pc_client_has_reload_configuration_method() {
    // Verify the method exists and compiles — actual HTTP call tested in integration.
    let client = PcClient::new(PC_PORT).unwrap();
    // We can't call it without a running PC, but we can verify it returns a future.
    let _fut = client.reload_configuration();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib pc_client_has_reload_configuration_method`
Expected: FAIL — `reload_configuration` method does not exist.

- [ ] **Step 3: Implement `reload_configuration`**

Add to `crates/rightclaw/src/runtime/pc_client.rs`, before the `shutdown` method (line 150):

```rust
/// Tell process-compose to re-read its configuration files from disk.
///
/// Uses `POST /project/configuration` — process-compose diffs the new config
/// against running state and adds/updates/removes processes accordingly.
pub async fn reload_configuration(&self) -> miette::Result<()> {
    let resp = self
        .client
        .post(format!("{}/project/configuration", self.base_url))
        .send()
        .await
        .map_err(|e| miette::miette!("failed to reload process-compose configuration: {e:#}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(miette::miette!(
            "process-compose configuration reload failed ({status}): {body}"
        ));
    }
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rightclaw --lib pc_client_has_reload_configuration_method`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/runtime/pc_client.rs crates/rightclaw/src/runtime/pc_client_tests.rs
git commit -m "feat: add reload_configuration to PcClient"
```

---

### Task 2: Extract codegen pipeline from `cmd_up`

This is the largest task. The per-agent codegen loop in `cmd_up` (lines ~725-960 of `main.rs`) needs to move into a shared function in the core crate.

**Files:**
- Create: `crates/rightclaw/src/codegen/pipeline.rs`
- Modify: `crates/rightclaw/src/codegen/mod.rs` (add `pub mod pipeline;` and re-export)
- Modify: `crates/rightclaw-cli/src/main.rs` (replace inline codegen with function call)

- [ ] **Step 1: Write the failing test**

Create `crates/rightclaw/src/codegen/pipeline.rs` with just the test initially:

```rust
use std::path::Path;

use crate::agent::AgentDef;
use crate::config::GlobalConfig;

/// Run per-agent codegen for a set of agents, then generate process-compose.yaml.
///
/// This is the shared pipeline used by both `rightclaw up` and `rightclaw reload`.
///
/// # Arguments
/// * `home` - RightClaw home directory (~/.rightclaw)
/// * `agents` - Agents to run codegen for
/// * `all_agents` - All discovered agents (used for process-compose.yaml generation — always includes full set)
/// * `self_exe` - Path to the rightclaw binary (for process-compose entries)
/// * `debug` - Enable debug logging in process-compose entries
pub fn run_agent_codegen(
    home: &Path,
    agents: &[AgentDef],
    all_agents: &[AgentDef],
    self_exe: &Path,
    debug: bool,
) -> miette::Result<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn run_agent_codegen_with_empty_agents_succeeds() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join("run")).unwrap();
        let self_exe = std::env::current_exe().unwrap();

        let result = run_agent_codegen(home, &[], &[], &self_exe, false);
        assert!(result.is_ok(), "empty agents should succeed: {result:?}");
    }
}
```

- [ ] **Step 2: Register module in `codegen/mod.rs`**

Add to `crates/rightclaw/src/codegen/mod.rs`:

```rust
pub mod pipeline;
pub use pipeline::run_agent_codegen;
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib run_agent_codegen_with_empty_agents`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 4: Implement `run_agent_codegen`**

Move the per-agent codegen logic from `crates/rightclaw-cli/src/main.rs` lines ~725-960 into `run_agent_codegen`. The function body should include:

1. Resolve `host_home` via `dirs::home_dir()`
2. Resolve `self_exe` is passed as parameter (already resolved by caller)
3. The per-agent loop (lines ~742-861 of current `main.rs`):
   - Generate settings.json
   - Generate agent_def.md + reply-schema.json
   - Create shell-snapshots dir
   - Generate .claude.json with trust entries
   - Create credential symlink
   - Git init if missing
   - Remove stale skill dirs + install built-in skills
   - Write settings.local.json if absent
   - Initialize memory DB
   - Generate/read agent secret + write mcp.json
4. Write agent-tokens.json (lines ~863-886)
5. Validate policy files for sandboxed agents (lines ~888-893)
6. Generate cloudflared config if tunnel configured (lines ~895-964)
7. Generate process-compose.yaml from `all_agents` (lines ~965-989)
8. Write runtime state.json (lines ~991-1006)

The function signature:

```rust
pub fn run_agent_codegen(
    home: &Path,
    agents: &[AgentDef],
    all_agents: &[AgentDef],
    self_exe: &Path,
    debug: bool,
) -> miette::Result<()> {
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| miette::miette!("failed to create run directory: {e:#}"))?;

    let host_home = dirs::home_dir()
        .ok_or_else(|| miette::miette!("cannot determine home directory"))?;

    let global_cfg = crate::config::read_global_config(home)?;

    // --- Per-agent codegen (for `agents` subset) ---
    for agent in agents {
        // ... (move lines 742-861 from main.rs here, replacing rightclaw:: with crate::)
    }

    // --- Token map (always from `all_agents`) ---
    // ... (move lines 863-886, iterating over `all_agents`)

    // --- Policy validation (always from `all_agents`) ---
    // ... (move lines 888-893, iterating over `all_agents`)

    // --- Cloudflared config ---
    // ... (move lines 895-964)

    // --- process-compose.yaml (always from `all_agents`) ---
    // ... (move lines 965-989)

    // --- Runtime state (always from `all_agents`) ---
    // ... (move lines 991-1006)

    Ok(())
}
```

Key detail: `agents` controls which agents get codegen run. `all_agents` controls what ends up in process-compose.yaml, token map, and policy validation. When called from `cmd_up` (no filter), both are the same slice. When called from `cmd_reload --agents x,y`, `agents` is filtered but `all_agents` is the full set.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rightclaw --lib run_agent_codegen_with_empty_agents`
Expected: PASS

- [ ] **Step 6: Update `cmd_up` to use `run_agent_codegen`**

In `crates/rightclaw-cli/src/main.rs`, replace the inline codegen block (lines ~725-1006) with:

```rust
// Clear rightcron init locks so the bootstrap hook fires on this session.
for agent in &agents {
    let lock = agent.path.join(".rightcron-init-done");
    let _ = std::fs::remove_file(&lock);
}

let self_exe = std::env::current_exe()
    .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;

rightclaw::codegen::run_agent_codegen(home, &agents, &agents, &self_exe, debug)?;
```

Keep the OpenShell preflight check, port availability check, and PC launch logic in `cmd_up` — those are not part of codegen.

- [ ] **Step 7: Build full workspace**

Run: `cargo build --workspace`
Expected: compiles without errors.

- [ ] **Step 8: Run existing tests**

Run: `cargo test --workspace`
Expected: all existing tests still pass.

- [ ] **Step 9: Commit**

```bash
git add crates/rightclaw/src/codegen/pipeline.rs crates/rightclaw/src/codegen/mod.rs crates/rightclaw-cli/src/main.rs
git commit -m "refactor: extract codegen pipeline from cmd_up into shared function"
```

---

### Task 3: Add `Reload` command to CLI

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs` (add `Reload` variant to `Commands` enum, add dispatch, add `cmd_reload` function)

- [ ] **Step 1: Add `Reload` variant to `Commands` enum**

In `crates/rightclaw-cli/src/main.rs`, add after the `Down` variant (around line 173):

```rust
/// Re-sync agent codegen and hot-update running process-compose
Reload {
    /// Only re-run codegen for specific agents (comma-separated).
    /// process-compose.yaml always includes all agents.
    #[arg(long, value_delimiter = ',')]
    agents: Option<Vec<String>>,
},
```

- [ ] **Step 2: Add dispatch in main match**

In the `match cli.command` block (around line 330), add after the `Down` dispatch:

```rust
Commands::Reload { agents } => cmd_reload(&home, agents).await,
```

- [ ] **Step 3: Write `cmd_reload` function**

Add to `crates/rightclaw-cli/src/main.rs`:

```rust
async fn cmd_reload(home: &Path, agents_filter: Option<Vec<String>>) -> miette::Result<()> {
    // 1. Verify process-compose is running.
    let client = rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT)?;
    client.health_check().await.map_err(|_| {
        miette::miette!(
            help = "Start rightclaw first with `rightclaw up`",
            "nothing running — cannot reload"
        )
    })?;

    // 2. Discover all agents.
    let agents_dir = home.join("agents");
    let all_agents = rightclaw::agent::discover_agents(&agents_dir)?;

    if all_agents.is_empty() {
        return Err(miette::miette!(
            "no agents found. Run `rightclaw agent init <name>` to create one."
        ));
    }

    // 3. Apply --agents filter for codegen scope.
    let codegen_agents = if let Some(ref filter) = agents_filter {
        let mut filtered = Vec::new();
        for name in filter {
            let found = all_agents.iter().find(|a| a.name == *name);
            match found {
                Some(agent) => filtered.push(agent.clone()),
                None => {
                    let available: Vec<&str> = all_agents.iter().map(|a| a.name.as_str()).collect();
                    return Err(miette::miette!(
                        "agent '{}' not found. Available agents: {}",
                        name,
                        available.join(", ")
                    ));
                }
            }
        }
        filtered
    } else {
        all_agents.clone()
    };

    // 4. Run codegen (filtered agents get codegen, all agents go into PC yaml).
    let self_exe = std::env::current_exe()
        .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;

    rightclaw::codegen::run_agent_codegen(
        home,
        &codegen_agents,
        &all_agents,
        &self_exe,
        false, // debug flag not relevant for reload
    )?;

    // 5. Hot-update process-compose.
    client.reload_configuration().await?;

    // 6. Print summary.
    let has_bot = all_agents.iter().any(|a| {
        a.config.as_ref().map(|c| c.telegram_token.is_some()).unwrap_or(false)
    });
    if !has_bot {
        eprintln!("rightclaw: warning: no agents have Telegram tokens — nothing will run");
    }

    println!("Reloaded. Active agents:");
    for agent in &all_agents {
        let has_token = agent.config.as_ref().map(|c| c.telegram_token.is_some()).unwrap_or(false);
        let status = if has_token { "bot" } else { "no token (skipped)" };
        println!("  {:<20} {}", agent.name, status);
    }

    Ok(())
}
```

- [ ] **Step 4: Build workspace**

Run: `cargo build --workspace`
Expected: compiles without errors.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat: add rightclaw reload command"
```

---

### Task 4: Update `agent init` to suggest reload

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs` (update `cmd_agent_init` output, around line 540)

- [ ] **Step 1: Update the success message**

In `crates/rightclaw-cli/src/main.rs`, replace lines 540-547 of `cmd_agent_init`:

```rust
    println!("Agent '{name}' created at {}", agent_dir.display());
    if token.is_some() {
        println!("Telegram channel configured.");
    }
    if !chat_ids.is_empty() {
        println!("Telegram chat ID allowlist configured.");
    }
```

With:

```rust
    println!("Agent '{name}' created at {}", agent_dir.display());
    if token.is_some() {
        println!("Telegram channel configured.");
    }
    if !chat_ids.is_empty() {
        println!("Telegram chat ID allowlist configured.");
    }
    println!();
    println!("If rightclaw is running, apply changes with:");
    println!("  rightclaw reload");
```

- [ ] **Step 2: Build workspace**

Run: `cargo build --workspace`
Expected: compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat: agent init suggests rightclaw reload"
```

---

### Task 5: Integration test for reload command

**Files:**
- Modify: `crates/rightclaw-cli/tests/cli_integration.rs` (add reload test)

- [ ] **Step 1: Read existing integration tests to match patterns**

Read `crates/rightclaw-cli/tests/cli_integration.rs` to understand the test setup conventions used in this project (assert_cmd patterns, temp dir setup, etc.).

- [ ] **Step 2: Write integration test for reload without running PC**

Add to `crates/rightclaw-cli/tests/cli_integration.rs`:

```rust
#[test]
fn reload_fails_when_not_running() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();

    // Create minimal agent structure so discovery doesn't fail first.
    let agent_dir = home.join("agents").join("test-agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# Test Agent").unwrap();

    Command::cargo_bin("rightclaw")
        .unwrap()
        .args(["--home", home.to_str().unwrap(), "reload"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("nothing running"));
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p rightclaw-cli --test cli_integration reload_fails_when_not_running`
Expected: PASS — the test verifies `reload` errors when no PC is running.

- [ ] **Step 4: Write integration test for agent init hint**

Add to `crates/rightclaw-cli/tests/cli_integration.rs`:

```rust
#[test]
fn agent_init_suggests_reload() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();

    // Create minimal home structure.
    std::fs::create_dir_all(home.join("agents")).unwrap();
    std::fs::write(home.join("config.yaml"), "{}").unwrap();

    Command::cargo_bin("rightclaw")
        .unwrap()
        .args([
            "--home", home.to_str().unwrap(),
            "agent", "init", "test-bot",
            "-y",
            "--sandbox-mode", "none",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("rightclaw reload"));
}
```

- [ ] **Step 5: Run the test**

Run: `cargo test -p rightclaw-cli --test cli_integration agent_init_suggests_reload`
Expected: PASS

- [ ] **Step 6: Run full test suite**

Run: `cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw-cli/tests/cli_integration.rs
git commit -m "test: integration tests for reload command and agent init hint"
```

---

### Task 6: Build full workspace and final verification

- [ ] **Step 1: Build workspace in debug mode**

Run: `cargo build --workspace`
Expected: clean compile, no warnings.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace`
Expected: no warnings.

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: all pass.

- [ ] **Step 4: Verify `rightclaw --help` shows reload**

Run: `cargo run -p rightclaw-cli -- --help`
Expected: `reload` appears in the subcommand list.

- [ ] **Step 5: Verify `rightclaw reload --help`**

Run: `cargo run -p rightclaw-cli -- reload --help`
Expected: shows `--agents` flag with description.

- [ ] **Step 6: Commit (if any fixes were needed)**

```bash
git add -u
git commit -m "fix: address clippy/build issues from reload feature"
```
