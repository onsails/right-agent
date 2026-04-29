# `right agent init` Auto-Reload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When `right agent init` finishes and process-compose (PC) is already running, auto-add the new bot to PC instead of telling the user to `right up`. Update the recap's tail line to reflect the actual outcome.

**Architecture:** New library helper `right_agent::agent::register::register_with_running_pc` mirrors `agent::destroy` — probes PC via `PcClient::from_home`, runs cross-agent codegen, calls `pc_client.reload_configuration()`, optionally calls `restart_process` for the recreate path. CLI calls it from `cmd_agent_init` after the wizard finishes and renders one of three recap tails based on the outcome. Renames `agent init --force` → `--force-recreate` (no alias). Spec: `docs/superpowers/specs/2026-04-29-init-pc-reload-design.md`.

**Tech Stack:** Rust 2024, `miette`, `tokio`, `tracing`. CLI tested with `assert_cmd` + `predicates`. Library tests use `tempfile` + the existing `read_state`/`PcClient::from_home` patterns.

---

## File Structure

**Create:**
- `crates/right-agent/src/agent/register.rs` — `RegisterOptions`, `RegisterResult`, `register_with_running_pc` (~100 LoC including the inline `#[cfg(test)] mod tests`).

**Modify:**
- `crates/right-agent/src/agent/mod.rs` — `pub mod register;` + re-exports, mirror destroy's surface.
- `crates/right/src/main.rs` — rename `force` → `force_recreate` for `AgentCommands::Init` and `cmd_agent_init`; wire `register_with_running_pc` into `cmd_agent_init`; update help text and shared prompt copy.
- `crates/right/tests/cli_integration.rs` — flip `--force` to `--force-recreate` for `agent init` callsites; add a negative test asserting bare `--force` is rejected on `agent init` and the suggestion text mentions `--force-recreate`.
- `crates/right-agent/src/ui/recap_tests.rs` — three new string-comparison tests for the rendered recap on the three end states (PC absent / PC reload OK / PC reload failed). Pure UI tests — no PC involved.

**Untouched:** `cmd_init` (the bootstrap `right` agent) keeps `--force`. `agent destroy --force` keeps `--force` (different semantics: skip prompts).

---

## Task 1: Library — `register` module skeleton with no-PC path (TDD)

**Files:**
- Create: `crates/right-agent/src/agent/register.rs`
- Modify: `crates/right-agent/src/agent/mod.rs`

The first cycle proves the runtime-isolation guard works. With no `state.json` in the home, the helper must return `pc_running: false` and produce zero side effects — no codegen, no network calls.

- [ ] **Step 1: Write the failing test**

Add `crates/right-agent/src/agent/register.rs` containing only:

```rust
//! Register a newly-created agent with a running process-compose.
//!
//! Mirrors [`crate::agent::destroy`]: probes PC via
//! [`crate::runtime::PcClient::from_home`] (which enforces `--home` isolation),
//! regenerates cross-agent codegen, and reloads PC's configuration so the new
//! agent's bot process appears live.

use std::path::Path;

/// Inputs for [`register_with_running_pc`].
pub struct RegisterOptions {
    pub agent_name: String,
    /// True when init wiped a pre-existing agent dir (`--force-recreate` on an
    /// existing agent). Drives the post-reload `restart_process` call.
    pub recreated: bool,
}

/// Outcome of [`register_with_running_pc`].
#[derive(Debug, PartialEq, Eq)]
pub struct RegisterResult {
    /// True if PC was alive and the reload succeeded.
    /// False if PC was not running (no `state.json`, stale port, health-check fail).
    pub pc_running: bool,
}

/// Register a newly-init'd agent with a running PC instance.
///
/// Returns `Ok(RegisterResult { pc_running: false })` if PC isn't running —
/// caller should print `next: right up`. Returns `Err` only if PC was alive but
/// the config reload failed; caller renders a warn row.
pub async fn register_with_running_pc(
    _home: &Path,
    _options: RegisterOptions,
) -> miette::Result<RegisterResult> {
    miette::bail!("not implemented")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_pc_running_false_when_state_json_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = register_with_running_pc(
            dir.path(),
            RegisterOptions {
                agent_name: "test".to_string(),
                recreated: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(result, RegisterResult { pc_running: false });
    }
}
```

Add to `crates/right-agent/src/agent/mod.rs`:

```rust
pub mod allowlist;
pub mod destroy;
pub mod discovery;
pub mod register;
pub mod types;

pub use destroy::{DestroyOptions, DestroyResult, destroy_agent};
pub use discovery::{
    discover_agents, discover_single_agent, parse_agent_config, validate_agent_name,
};
pub use register::{RegisterOptions, RegisterResult, register_with_running_pc};
pub use types::{AgentConfig, AgentDef, RestartPolicy, SandboxConfig, SandboxMode};
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p right-agent agent::register::tests::returns_pc_running_false_when_state_json_absent`
Expected: FAIL with `not implemented` or panic from `unwrap()` on the bail.

- [ ] **Step 3: Implement the no-PC path**

Replace the body of `register_with_running_pc` in `crates/right-agent/src/agent/register.rs`:

```rust
pub async fn register_with_running_pc(
    home: &Path,
    options: RegisterOptions,
) -> miette::Result<RegisterResult> {
    // `from_home` enforces --home isolation by reading
    // `<home>/run/state.json` for the PC port + token. Absent or stale state
    // ⇒ no PC ⇒ skip everything. See ARCHITECTURE.md
    // "Runtime isolation — mandatory".
    let Some(client) = crate::runtime::PcClient::from_home(home)? else {
        tracing::debug!(
            home = %home.display(),
            agent = %options.agent_name,
            "no runtime state — PC not running, skipping reload"
        );
        return Ok(RegisterResult { pc_running: false });
    };

    if client.health_check().await.is_err() {
        tracing::debug!(
            agent = %options.agent_name,
            "state.json present but PC health-check failed — treating as not running"
        );
        return Ok(RegisterResult { pc_running: false });
    }

    // PC is alive. Implementation continues in subsequent tasks.
    miette::bail!("PC-alive path not yet implemented")
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p right-agent agent::register::tests::returns_pc_running_false_when_state_json_absent`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/agent/register.rs crates/right-agent/src/agent/mod.rs
git commit -m "feat(register): skeleton + no-PC path"
```

---

## Task 2: Library — stale and malformed `state.json` tests

**Files:**
- Modify: `crates/right-agent/src/agent/register.rs`

`PcClient::from_home` parses `state.json` and returns `Some(_)` even when PC is dead. The helper must health-check and treat health failure as "not running". A *malformed* `state.json` causes `from_home` to return `Err`, and `?` must propagate it. This task adds explicit coverage for both.

- [ ] **Step 1: Add the stale-port test**

Append to the `tests` module in `crates/right-agent/src/agent/register.rs`:

```rust
    #[tokio::test]
    async fn returns_pc_running_false_when_state_json_points_at_closed_port() {
        let dir = tempfile::TempDir::new().unwrap();
        let run = dir.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        // Write a state.json that points at a port nothing is listening on.
        // Port 1 is reserved and unbound on developer machines; if it ever
        // is bound, the test will be flaky — pick another low port.
        std::fs::write(
            run.join("state.json"),
            r#"{"agents":[],"pc_port":1,"pc_api_token":"x"}"#,
        )
        .unwrap();

        let result = register_with_running_pc(
            dir.path(),
            RegisterOptions {
                agent_name: "test".to_string(),
                recreated: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(result, RegisterResult { pc_running: false });
    }
```

- [ ] **Step 2: Add the malformed-state test**

Append immediately after the previous test:

```rust
    #[tokio::test]
    async fn propagates_error_when_state_json_is_malformed() {
        let dir = tempfile::TempDir::new().unwrap();
        let run = dir.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("state.json"), "{not valid json").unwrap();

        let err = register_with_running_pc(
            dir.path(),
            RegisterOptions {
                agent_name: "test".to_string(),
                recreated: false,
            },
        )
        .await
        .expect_err("malformed state.json must be a parse error");
        // Just confirm the error chain reaches us — exact wording is from_home's.
        let msg = format!("{err:#}");
        assert!(
            msg.to_lowercase().contains("state.json")
                || msg.to_lowercase().contains("parse")
                || msg.to_lowercase().contains("json"),
            "expected error to mention state.json/parse/json, got: {msg}"
        );
    }
```

- [ ] **Step 3: Run both tests**

Run: `cargo test -p right-agent agent::register::tests`
Expected: all three register tests PASS (the original `returns_pc_running_false_when_state_json_absent` plus the two new ones). If the malformed-state test fails because `from_home` swallows the parse error, the existing helper has a bug — but per `runtime/state.rs` it propagates errors. Re-check `from_home` if so.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/src/agent/register.rs
git commit -m "test(register): cover stale and malformed state.json"
```

---

## Task 3: Library — PC-alive happy path (codegen + reload)

**Files:**
- Modify: `crates/right-agent/src/agent/register.rs`

The PC-alive path mirrors `destroy.rs:265-283`: discover agents, run cross-agent codegen, call `reload_configuration`. No automated test — same posture as `destroy.rs` (only the no-PC path is unit-tested; the live PC path is verified manually). Implementation correctness comes from mirroring the production destroy code.

- [ ] **Step 1: Replace the `miette::bail!("PC-alive path not yet implemented")` line**

In `crates/right-agent/src/agent/register.rs`, in `register_with_running_pc`, replace the trailing `miette::bail!(...)` with:

```rust
    // PC is alive. Mirrors `crate::agent::destroy::destroy_agent` after the
    // dir-removal step: rediscover all agents, regenerate cross-agent codegen
    // (process-compose.yaml, agent-tokens.json, cloudflared config), then ask
    // PC to diff its running config against the new file via POST
    // /project/configuration. PC adds the new agent's processes live.
    let agents_dir = crate::config::agents_dir(home);
    let all_agents = crate::agent::discover_agents(&agents_dir)?;
    let self_exe = std::env::current_exe()
        .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;
    crate::codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

    client.reload_configuration().await.map_err(|e| {
        tracing::warn!(
            agent = %options.agent_name,
            error = format!("{e:#}"),
            "process-compose reload failed"
        );
        e
    })?;
    tracing::info!(agent = %options.agent_name, "reloaded process-compose configuration");

    if options.recreated {
        let process_name = format!("{}-bot", options.agent_name);
        if let Err(e) = client.restart_process(&process_name).await {
            // Non-fatal: config is correct on disk and in PC; only the live
            // process didn't bounce. Surface to the log file, not the recap.
            tracing::warn!(
                process = %process_name,
                error = format!("{e:#}"),
                "failed to restart bot process after recreate (non-fatal)"
            );
        }
    }

    Ok(RegisterResult { pc_running: true })
```

- [ ] **Step 2: Build the workspace**

Run: `cargo build --workspace`
Expected: clean build. If `tracing::warn!` fields fail to compile, ensure `format!("{e:#}")` is used (pre-existing convention — see CLAUDE.rust.md "Preserve Error Chains").

- [ ] **Step 3: Re-run register tests**

Run: `cargo test -p right-agent agent::register`
Expected: both existing tests PASS. (Adding the PC-alive path doesn't affect them — both return early.)

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/src/agent/register.rs
git commit -m "feat(register): PC-alive happy path with optional restart"
```

---

## Task 4: CLI — rename `agent init --force` → `--force-recreate`

**Files:**
- Modify: `crates/right/src/main.rs`

Hard rename in three places: the `AgentCommands::Init` clap struct, the dispatch destructuring, and the `cmd_agent_init` signature/body. The shared error/help strings that mention `--force` get updated only where they're reached from the `agent init` path. `cmd_init`'s `--force` and `agent destroy --force` are untouched.

- [ ] **Step 1: Rename the clap field**

In `crates/right/src/main.rs` `AgentCommands::Init` (around line 134-155), change:

```rust
        /// If agent exists, wipe and re-create (confirms unless -y)
        #[arg(long)]
        force: bool,
        /// With --force: re-run wizard instead of reusing existing config
        #[arg(long, requires = "force")]
        fresh: bool,
```

to:

```rust
        /// If agent exists, wipe and re-create (confirms unless -y)
        #[arg(long)]
        force_recreate: bool,
        /// With --force-recreate: re-run wizard instead of reusing existing config
        #[arg(long, requires = "force_recreate")]
        fresh: bool,
```

Clap converts `force_recreate` to `--force-recreate` automatically.

- [ ] **Step 2: Update the dispatch destructuring**

In `crates/right/src/main.rs` (around line 575), change the match arm:

```rust
            AgentCommands::Init {
                name,
                yes,
                force,
                fresh,
                network_policy,
                sandbox_mode,
                from_backup,
            } => {
                if let Some(backup_path) = from_backup {
                    cmd_agent_restore(&home, &name, &backup_path).await
                } else {
                    cmd_agent_init(
                        &home,
                        &name,
                        yes,
                        force,
                        fresh,
                        network_policy,
                        sandbox_mode,
                    )
                }
            }
```

to:

```rust
            AgentCommands::Init {
                name,
                yes,
                force_recreate,
                fresh,
                network_policy,
                sandbox_mode,
                from_backup,
            } => {
                if let Some(backup_path) = from_backup {
                    cmd_agent_restore(&home, &name, &backup_path).await
                } else {
                    cmd_agent_init(
                        &home,
                        &name,
                        yes,
                        force_recreate,
                        fresh,
                        network_policy,
                        sandbox_mode,
                    )
                }
            }
```

- [ ] **Step 3: Rename in `cmd_agent_init` signature and body**

In `crates/right/src/main.rs` (around line 1600), change:

```rust
fn cmd_agent_init(
    home: &Path,
    name: &str,
    yes: bool,
    force: bool,
    fresh: bool,
    network_policy: Option<right_agent::agent::types::NetworkPolicy>,
    sandbox_mode: Option<right_agent::agent::types::SandboxMode>,
) -> miette::Result<()> {
```

to:

```rust
fn cmd_agent_init(
    home: &Path,
    name: &str,
    yes: bool,
    force_recreate: bool,
    fresh: bool,
    network_policy: Option<right_agent::agent::types::NetworkPolicy>,
    sandbox_mode: Option<right_agent::agent::types::SandboxMode>,
) -> miette::Result<()> {
```

Then in the same function body, replace the three `force` references:

- The "exists without force" guard (around line 1614):

  ```rust
      if agent_dir.exists() && !force {
          return Err(miette::miette!(
              help =
                  "Use --force to wipe and re-create, or `right agent config` to change settings",
              "Agent directory already exists at {}",
              agent_dir.display()
          ));
      }
  ```

  becomes:

  ```rust
      if agent_dir.exists() && !force_recreate {
          return Err(miette::miette!(
              help =
                  "Use --force-recreate to wipe and re-create, or `right agent config` to change settings",
              "Agent directory already exists at {}",
              agent_dir.display()
          ));
      }
  ```

- The force-wipe block guard (around line 1625):

  ```rust
      let saved_overrides = if force && agent_dir.exists() {
  ```

  becomes:

  ```rust
      let saved_overrides = if force_recreate && agent_dir.exists() {
  ```

- The interactive confirmation guard (around line 1753):

  ```rust
          if interactive && !force {
  ```

  becomes:

  ```rust
          if interactive && !force_recreate {
  ```

- The sandbox-recreate decision (around line 1988):

  ```rust
          let force_recreate = if force || !agent_existed {
  ```

  becomes:

  ```rust
          let recreate_sandbox = if force_recreate || !agent_existed {
  ```

  And update the `force_recreate` reference passed to `ensure_sandbox` two lines below to `recreate_sandbox` (it's a local, distinct from the function param now).

  Concretely, the surrounding block becomes:

  ```rust
          let recreate_sandbox = if force_recreate || !agent_existed {
              let exists = tokio::task::block_in_place(|| {
                  tokio::runtime::Handle::current()
                      .block_on(async { check_sandbox_exists_async(&sb_name).await })
              });
              exists.unwrap_or(false)
          } else {
              prompt_sandbox_recreate_if_exists(&sb_name, interactive)?
          };
          println!("Creating OpenShell sandbox...");
          tokio::task::block_in_place(|| {
              tokio::runtime::Handle::current().block_on(async {
                  right_agent::openshell::ensure_sandbox(
                      &sb_name,
                      &policy_path,
                      Some(&staging),
                      recreate_sandbox,
                  )
                  .await
              })
          })?;
  ```

- [ ] **Step 4: Update the shared `prompt_sandbox_recreate_if_exists` help text**

The function lives around line 2086 of `crates/right/src/main.rs` and is called from both `cmd_init` (where the flag is `--force`) and `cmd_agent_init` (where it's now `--force-recreate`). The non-interactive error message at line ~2099 currently says:

```rust
        return Err(miette::miette!(
            help = "Run interactively to confirm, or use `--force`",
            "Sandbox '{sandbox_name}' already exists"
        ));
```

Change the help string to mention both flags:

```rust
        return Err(miette::miette!(
            help = "Run interactively to confirm, or pass --force-recreate (agent init) / --force (init)",
            "Sandbox '{sandbox_name}' already exists"
        ));
```

- [ ] **Step 5: Build and run unit tests**

Run: `cargo build --workspace`
Expected: clean build.

Run: `cargo test -p right-agent`
Expected: PASS (no library-level test depends on the CLI flag).

- [ ] **Step 6: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "refactor(cli): rename agent init --force to --force-recreate"
```

---

## Task 5: CLI — wire `register_with_running_pc` into `cmd_agent_init`

**Files:**
- Modify: `crates/right/src/main.rs`

Insert the register call between the existing wizard tail (sandbox ready) and the recap composition. Conditional `next:` line based on the outcome.

- [ ] **Step 1: Compute `recreated` and call the helper**

In `crates/right/src/main.rs`, find the recap composition block in `cmd_agent_init` (around line 2059, ending with `.next("right up");`). The lines immediately before it look like:

```rust
    let memory_detail = match cfg.memory.as_ref().map(|m| &m.provider) {
        Some(right_agent::agent::types::MemoryProvider::Hindsight) => "hindsight",
        _ => "file",
    };

    let recap = right_agent::ui::Recap::new("ready")
        .ok("agent", &format!("{name} created"))
        .ok("sandbox", &sandbox_with_policy)
        .ok("telegram", if cfg.telegram_token.is_some() { "configured" } else { "not configured" })
        .ok("chat ids", &chat_ids_detail)
        .ok("stt", &stt_detail)
        .ok("memory", memory_detail)
        .next("right up");
    println!("{}", recap.render(theme));
```

Replace with:

```rust
    let memory_detail = match cfg.memory.as_ref().map(|m| &m.provider) {
        Some(right_agent::agent::types::MemoryProvider::Hindsight) => "hindsight",
        _ => "file",
    };

    // If PC is already running, hot-add the new agent's bot via reload.
    // No PC ⇒ pc_running: false ⇒ recap ends with `next: right up`.
    let register_outcome = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            right_agent::agent::register_with_running_pc(
                home,
                right_agent::agent::RegisterOptions {
                    agent_name: name.to_string(),
                    recreated: agent_existed && force_recreate,
                },
            )
            .await
        })
    });

    let mut recap = right_agent::ui::Recap::new("ready")
        .ok("agent", &format!("{name} created"))
        .ok("sandbox", &sandbox_with_policy)
        .ok("telegram", if cfg.telegram_token.is_some() { "configured" } else { "not configured" })
        .ok("chat ids", &chat_ids_detail)
        .ok("stt", &stt_detail)
        .ok("memory", memory_detail);

    recap = match register_outcome {
        Ok(right_agent::agent::RegisterResult { pc_running: false }) => {
            recap.next("right up")
        }
        Ok(right_agent::agent::RegisterResult { pc_running: true }) => {
            recap.next("send /start to your bot in Telegram")
        }
        Err(e) => {
            tracing::warn!(error = format!("{e:#}"), "PC reload failed during agent init");
            recap
                .warn("reload", "failed to add to running right")
                .next("right restart")
        }
    };
    println!("{}", recap.render(theme));
```

The `agent_existed` variable already exists in `cmd_agent_init` (set near the top, line ~1612: `let agent_existed = agent_dir.exists();`). Confirm it's still in scope — it should be, since the recap is at the end of the same function.

- [ ] **Step 2: Build the workspace**

Run: `cargo build --workspace`
Expected: clean build. If you get "borrow of moved value `recap`", check the `let mut recap` + reassignment pattern is intact.

- [ ] **Step 3: Run library tests**

Run: `cargo test -p right-agent`
Expected: PASS (no test exercises this CLI path directly yet — that's Task 7).

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(cli): hot-add new agent to running process-compose"
```

---

## Task 6: UI — recap rendering tests for the three end states

**Files:**
- Modify: `crates/right-agent/src/ui/recap_tests.rs`

Pure UI tests — they don't go through `cmd_agent_init` or the register helper. They construct a `Recap` the way the CLI does for each branch and assert the rendered text. Catches accidental drift in the `next:` copy.

- [ ] **Step 1: Add the three tests**

Append to `crates/right-agent/src/ui/recap_tests.rs`:

```rust
#[test]
fn recap_init_pc_not_running() {
    let s = Recap::new("ready")
        .ok("agent", "test created")
        .next("right up")
        .render(Theme::Mono);
    assert!(s.contains("✓ agent"));
    assert!(s.contains("next: right up"));
    assert!(!s.contains("send /start"));
    assert!(!s.contains("⚠ reload"));
    assert!(!s.contains("! reload"));
}

#[test]
fn recap_init_pc_running_ok() {
    let s = Recap::new("ready")
        .ok("agent", "test created")
        .next("send /start to your bot in Telegram")
        .render(Theme::Mono);
    assert!(s.contains("✓ agent"));
    assert!(s.contains("next: send /start to your bot in Telegram"));
    assert!(!s.contains("right up"));
    assert!(!s.contains("⚠ reload"));
}

#[test]
fn recap_init_pc_reload_failed() {
    let s = Recap::new("ready")
        .ok("agent", "test created")
        .warn("reload", "failed to add to running right")
        .next("right restart")
        .render(Theme::Mono);
    assert!(s.contains("✓ agent"));
    // The warn glyph rendering is theme-dependent; check for the noun/detail pair.
    assert!(s.contains("reload"));
    assert!(s.contains("failed to add to running right"));
    assert!(s.contains("next: right restart"));
}
```

- [ ] **Step 2: Run the new tests**

Run: `cargo test -p right-agent recap_init`
Expected: three PASS.

If `recap_init_pc_reload_failed` fails because the warn glyph is `!` not `⚠`, that's already accounted for — we only assert the noun and detail strings.

- [ ] **Step 3: Commit**

```bash
git add crates/right-agent/src/ui/recap_tests.rs
git commit -m "test(ui): recap rendering for init's three end states"
```

---

## Task 7: CLI integration tests — flag rename + negative test

**Files:**
- Modify: `crates/right/tests/cli_integration.rs`

Three existing tests pass `--force` to `agent init`. Update them to `--force-recreate`. Add one new test asserting bare `--force` is rejected on `agent init` and clap's error mentions `--force-recreate` (or "unexpected"). The existing `--fresh requires --force` test (`test_agent_init_fresh_without_force_errors`) needs both an arg update and an assertion update.

- [ ] **Step 1: Update `test_agent_init_force_wipes_dir` (around line 547)**

In `crates/right/tests/cli_integration.rs`, replace `"--force"` with `"--force-recreate"` inside the second `right()` call. Update the comment and assertion message:

```rust
    // Re-init with --force-recreate.
    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "test-agent",
            "--force-recreate",
            "-y",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success();

    // Agent dir exists, MARKER.txt is gone, agent.yaml exists.
    assert!(dir.path().join("agents/test-agent").exists());
    assert!(
        !marker.exists(),
        "MARKER.txt should be wiped by --force-recreate"
    );
    assert!(dir.path().join("agents/test-agent/agent.yaml").exists());
```

- [ ] **Step 2: Update `test_agent_init_fresh_without_force_errors` (around line 569)**

Rename the test, update the args in the message, and update the assertion:

```rust
#[test]
fn test_agent_init_fresh_without_force_recreate_errors() {
    right()
        .args([
            "--home",
            "/tmp/doesnt-matter",
            "agent",
            "init",
            "test-agent",
            "--fresh",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--force-recreate"));
}
```

- [ ] **Step 3: Update `test_agent_init_force_preserves_config` (around line 587)**

Replace the two `--force` strings:

```rust
    // Re-init with --force-recreate (no --fresh) — should preserve config.
    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "preserve-test",
            "--force-recreate",
            "-y",
        ])
        .assert()
        .success();

    let yaml = fs::read_to_string(dir.path().join("agents/preserve-test/agent.yaml")).unwrap();
    assert!(
        yaml.contains("mode: none"),
        "agent.yaml should preserve sandbox mode: none after --force-recreate, got:\n{yaml}"
    );
```

(Rename the test from `test_agent_init_force_preserves_config` to `test_agent_init_force_recreate_preserves_config` for consistency.)

- [ ] **Step 4: Update `test_agent_init_force_on_nonexistent_agent` (around line 633)**

Rename to `test_agent_init_force_recreate_on_nonexistent_agent`. Replace `--force` with `--force-recreate`:

```rust
#[test]
fn test_agent_init_force_recreate_on_nonexistent_agent() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::write(dir.path().join("config.yaml"), minimal_config_yaml(dir.path())).unwrap();

    // --force-recreate on non-existent agent should just create it.
    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "new-agent",
            "--force-recreate",
            "-y",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success();

    assert!(dir.path().join("agents/new-agent/agent.yaml").exists());
}
```

- [ ] **Step 5: Add a negative test asserting bare `--force` is rejected**

Append to `crates/right/tests/cli_integration.rs`:

```rust
#[test]
fn test_agent_init_bare_force_rejected() {
    // Agent init no longer accepts --force; only --force-recreate.
    // (`agent destroy --force` and `init --force` are unchanged.)
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::write(dir.path().join("config.yaml"), minimal_config_yaml(dir.path())).unwrap();

    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "test-agent",
            "--force",
            "-y",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument"));
}
```

clap's standard error for an unknown flag is `error: unexpected argument '--force' found`. If clap's wording differs, switch to `predicate::str::contains("--force")`.

- [ ] **Step 6: Run the integration tests**

Run: `cargo test -p right --test cli_integration`
Expected: all PASS, including the four updated tests and the new negative test.

If any test that doesn't touch `--force` regresses, it's likely the recap text: `cmd_agent_init` now picks `next: right up` only when PC is not running. In `tempdir`-isolated tests there's no `state.json`, so `pc_running: false` ⇒ `next: right up`. The wizard_brand assertions still hold.

- [ ] **Step 7: Commit**

```bash
git add crates/right/tests/cli_integration.rs
git commit -m "test(cli): update agent init tests for --force-recreate rename"
```

---

## Task 8: Workspace verification

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: clean build, no warnings about unused `force` variables.

- [ ] **Step 2: Full workspace tests**

Run: `cargo test --workspace`
Expected: all PASS. Pay attention to any test that compared `next: right up` literally — if a non-tempdir-based test ran with a real `state.json`, it would now produce the PC-running branch. None known at design time, but flag any regression.

- [ ] **Step 3: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Manual verification checklist**

Document the manual tests in the commit body. The agent runs only what is automatable — leave these for the human:

```
Manual verification (not run by CI):

1. Clean state, no PC:
   cargo run --bin right -- agent init manual1
   ⇒ recap ends with "next: right up"

2. PC already running:
   cargo run --bin right -- up --detach
   cargo run --bin right -- agent init manual2
   ⇒ recap ends with "next: send /start to your bot in Telegram"
   ⇒ `right attach` shows manual2-bot in PC

3. Force-recreate while PC running:
   cargo run --bin right -- agent init --force-recreate manual2 -y
   ⇒ same recap as (2)
   ⇒ `right attach` shows manual2-bot has restarted

4. Bare --force rejected:
   cargo run --bin right -- agent init x --force
   ⇒ clap error mentions "unexpected argument"
```

---

## Spec coverage check

| Spec section | Tasks |
|---|---|
| Behavior matrix (PC absent / OK / failed) | T1 (no-PC), T3 (OK + Err), T5 (recap branches) |
| `RegisterOptions` / `RegisterResult` | T1 |
| `register_with_running_pc` semantics | T1 (skeleton + no-PC), T2 (stale + malformed), T3 (alive + restart) |
| `--force-recreate` rename | T4 |
| Wiring into `cmd_agent_init` | T5 |
| Three recap copies | T6 (rendering), T5 (composition) |
| Test posture: no-PC unit tests + malformed propagation | T1, T2 |
| Negative test for bare `--force` | T7 |
| Manual live-PC verification | T8 |
