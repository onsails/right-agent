# Cloudflared Restart On Config Change Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restart the running `cloudflared` process whenever regenerated tunnel ingress config content changes, so newly added/restored agent webhook routes work without a manual restart.

**Architecture:** Cross-agent codegen will return a small outcome object that records whether `~/.right/cloudflared-config.yml` content changed. Runtime reload paths will call process-compose reload first, then restart the managed `cloudflared` process only when that flag is true. Initial `right up` and offline init paths keep generating files but do not restart anything because there is no existing tunnel process to refresh.

**Tech Stack:** Rust 2024, `miette`, `reqwest`, `wiremock`, process-compose REST API, existing `right-codegen` and `right-agent` crates.

---

## File Structure

- Modify `crates/right-codegen/src/contract.rs`
  - Add a sanctioned writer that preserves `write_regenerated` semantics but returns whether file content changed.
- Modify `crates/right-codegen/src/pipeline.rs`
  - Add `CodegenOutcome`.
  - Make `run_agent_codegen()` return `miette::Result<CodegenOutcome>`.
  - Track `cloudflared-config.yml` content changes.
- Modify `crates/right-codegen/src/lib.rs`
  - Re-export `CodegenOutcome`.
- Modify `crates/right-agent/src/runtime/pc_client.rs`
  - Add `restart_cloudflared_if_config_changed()`.
- Modify `crates/right-agent/src/runtime/pc_client_tests.rs`
  - Add process-compose REST tests for the new restart helper.
- Modify `crates/right-agent/src/agent/register.rs`
  - Restart `cloudflared` after successful process-compose reload when codegen reports changed tunnel config.
- Modify `crates/right-agent/src/agent/destroy.rs`
  - Restart `cloudflared` after successful process-compose reload when codegen reports changed tunnel config.
- Modify `crates/right/src/main.rs`
  - Restart `cloudflared` in `right reload` after successful process-compose reload when codegen reports changed tunnel config.
- Modify `docs/architecture/lifecycle.md`
  - Document that reload/register/destroy refresh the tunnel process when ingress config changes.

---

### Task 1: Add A Change-Detecting Regenerated Writer

**Files:**
- Modify: `crates/right-codegen/src/contract.rs`

- [ ] **Step 1: Write failing tests for change detection**

Add these tests inside `#[cfg(test)] mod tests` in `crates/right-codegen/src/contract.rs`, near the existing `write_regenerated_*` tests:

```rust
#[test]
fn write_regenerated_detect_change_reports_new_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sub/file.txt");

    let changed = write_regenerated_detect_change(&path, "first").unwrap();

    assert!(changed, "new file must count as changed");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
}

#[test]
fn write_regenerated_detect_change_reports_same_content_unchanged() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sub/file.txt");

    write_regenerated_detect_change(&path, "first").unwrap();
    let changed = write_regenerated_detect_change(&path, "first").unwrap();

    assert!(!changed, "identical content must not count as changed");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
}

#[test]
fn write_regenerated_detect_change_reports_different_content_changed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sub/file.txt");

    write_regenerated_detect_change(&path, "first").unwrap();
    let changed = write_regenerated_detect_change(&path, "second").unwrap();

    assert!(changed, "different content must count as changed");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
}
```

- [ ] **Step 2: Run the narrow failing test**

Run:

```bash
devenv shell -- cargo test -p right-codegen write_regenerated_detect_change -- --nocapture
```

Expected: FAIL with an unresolved function error for `write_regenerated_detect_change`.

- [ ] **Step 3: Implement the helper**

In `crates/right-codegen/src/contract.rs`, add this function after `write_regenerated()`:

```rust
/// Unconditional write that also reports whether the file content changed.
///
/// This keeps `Regenerated` write semantics intact: callers still rewrite the
/// file every time, but can use the returned flag to decide whether a running
/// process must be restarted to read the new content.
pub fn write_regenerated_detect_change(path: &Path, content: &str) -> miette::Result<bool> {
    ensure_parent_dir(path)?;

    let changed = match std::fs::read_to_string(path) {
        Ok(existing) => existing != content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            return Err(miette::miette!(
                "failed to read existing {} before write: {e:#}",
                path.display()
            ));
        }
    };

    std::fs::write(path, content)
        .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))?;
    Ok(changed)
}
```

- [ ] **Step 4: Run the narrow passing test**

Run:

```bash
devenv shell -- cargo test -p right-codegen write_regenerated_detect_change -- --nocapture
```

Expected: PASS for all three `write_regenerated_detect_change_*` tests.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/right-codegen/src/contract.rs
git commit -m "feat(codegen): detect regenerated file content changes"
```

---

### Task 2: Return Cloudflared Config Change Outcome From Codegen

**Files:**
- Modify: `crates/right-codegen/src/pipeline.rs`
- Modify: `crates/right-codegen/src/lib.rs`

- [ ] **Step 1: Write failing codegen outcome tests**

In `crates/right-codegen/src/pipeline.rs`, add these tests inside `#[cfg(test)] pub(crate) mod tests`, after `run_agent_codegen_with_empty_agents`:

```rust
#[test]
fn run_agent_codegen_reports_new_cloudflared_config_changed() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();
    write_minimal_global_config(home);

    let agent_dir = home.join("agents").join("test");
    std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
    std::fs::write(
        agent_dir.join("agent.yaml"),
        "restart: never\nnetwork_policy: permissive\n",
    )
    .unwrap();

    let agent = agent_fixture(&agent_dir);
    let self_exe = std::path::PathBuf::from("/usr/bin/right");

    let outcome = run_agent_codegen(home, std::slice::from_ref(&agent), &self_exe, false).unwrap();

    assert!(
        outcome.cloudflared_config_changed,
        "first cloudflared config write must be reported as changed"
    );
}

#[test]
fn run_agent_codegen_reports_unchanged_cloudflared_config() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();
    write_minimal_global_config(home);

    let agent_dir = home.join("agents").join("test");
    std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
    std::fs::write(
        agent_dir.join("agent.yaml"),
        "restart: never\nnetwork_policy: permissive\n",
    )
    .unwrap();

    let agent = agent_fixture(&agent_dir);
    let self_exe = std::path::PathBuf::from("/usr/bin/right");

    let first = run_agent_codegen(home, std::slice::from_ref(&agent), &self_exe, false).unwrap();
    let second = run_agent_codegen(home, std::slice::from_ref(&agent), &self_exe, false).unwrap();

    assert!(first.cloudflared_config_changed);
    assert!(
        !second.cloudflared_config_changed,
        "second identical cloudflared config write must not be reported as changed"
    );
}
```

- [ ] **Step 2: Run the narrow failing tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen run_agent_codegen_reports_ -- --nocapture
```

Expected: FAIL because `run_agent_codegen()` currently returns `()` and no `cloudflared_config_changed` field exists.

- [ ] **Step 3: Add `CodegenOutcome` and track cloudflared config changes**

In `crates/right-codegen/src/pipeline.rs`, change the import:

```rust
use crate::contract::{
    write_agent_owned, write_merged_rmw, write_regenerated, write_regenerated_detect_change,
};
```

Add this type above `run_agent_codegen()`:

```rust
/// Observable effects from cross-agent codegen.
///
/// Callers that already have a running process-compose instance use this to
/// restart long-lived processes that do not hot-reload rewritten files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodegenOutcome {
    pub cloudflared_config_changed: bool,
}
```

Change the `run_agent_codegen()` signature in `crates/right-codegen/src/pipeline.rs`:

```rust
pub fn run_agent_codegen(
    home: &Path,
    all_agents: &[AgentDef],
    self_exe: &Path,
    debug: bool,
) -> miette::Result<CodegenOutcome> {
```

Inside the function, after reading `global_cfg`, initialize the outcome:

```rust
let mut outcome = CodegenOutcome::default();
```

Replace the cloudflared config write:

```rust
outcome.cloudflared_config_changed =
    write_regenerated_detect_change(&cf_config_path, &cf_config)?;
```

Replace the final return:

```rust
Ok(outcome)
```

- [ ] **Step 4: Re-export `CodegenOutcome`**

In `crates/right-codegen/src/lib.rs`, replace:

```rust
pub use pipeline::run_agent_codegen;
```

with:

```rust
pub use pipeline::{CodegenOutcome, run_agent_codegen};
```

- [ ] **Step 5: Run the narrow passing tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen run_agent_codegen_reports_ -- --nocapture
```

Expected: PASS for both new codegen outcome tests.

- [ ] **Step 6: Run right-codegen tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add crates/right-codegen/src/pipeline.rs crates/right-codegen/src/lib.rs
git commit -m "feat(codegen): report cloudflared config changes"
```

---

### Task 3: Add Process-Compose Cloudflared Restart Helper

**Files:**
- Modify: `crates/right-agent/src/runtime/pc_client.rs`
- Modify: `crates/right-agent/src/runtime/pc_client_tests.rs`

- [ ] **Step 1: Write failing restart helper tests**

In `crates/right-agent/src/runtime/pc_client_tests.rs`, add these tests after `health_check_fails_when_token_missing`:

```rust
#[tokio::test]
async fn restart_cloudflared_if_config_changed_posts_restart_when_changed() {
    setup_crypto();
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process/restart/cloudflared"))
        .and(header("X-PC-Token-Key", "the-token"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let port = server.address().port();
    let client = PcClient::new(port, Some("the-token".to_string())).unwrap();

    client
        .restart_cloudflared_if_config_changed(true)
        .await
        .expect("changed cloudflared config must restart cloudflared");
}

#[tokio::test]
async fn restart_cloudflared_if_config_changed_skips_restart_when_unchanged() {
    setup_crypto();
    let server = wiremock::MockServer::start().await;
    let port = server.address().port();
    let client = PcClient::new(port, Some("the-token".to_string())).unwrap();

    client
        .restart_cloudflared_if_config_changed(false)
        .await
        .expect("unchanged cloudflared config must not call process-compose");
}
```

- [ ] **Step 2: Run the narrow failing tests**

Run:

```bash
devenv shell -- cargo test -p right-agent restart_cloudflared_if_config_changed -- --nocapture
```

Expected: FAIL with an unresolved method error for `restart_cloudflared_if_config_changed`.

- [ ] **Step 3: Implement the helper**

In `crates/right-agent/src/runtime/pc_client.rs`, add this method immediately after `restart_process()`:

```rust
/// Restart the managed cloudflared process when regenerated ingress config changed.
///
/// Cloudflared reads local ingress config at process start; process-compose
/// reload does not restart it when only the config file content changes.
pub async fn restart_cloudflared_if_config_changed(
    &self,
    cloudflared_config_changed: bool,
) -> miette::Result<()> {
    if !cloudflared_config_changed {
        return Ok(());
    }

    self.restart_process("cloudflared").await
}
```

- [ ] **Step 4: Run the narrow passing tests**

Run:

```bash
devenv shell -- cargo test -p right-agent restart_cloudflared_if_config_changed -- --nocapture
```

Expected: PASS for both restart helper tests.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/right-agent/src/runtime/pc_client.rs crates/right-agent/src/runtime/pc_client_tests.rs
git commit -m "feat(runtime): restart cloudflared after ingress changes"
```

---

### Task 4: Wire Cloudflared Restart Into Runtime Reload Paths

**Files:**
- Modify: `crates/right-agent/src/agent/register.rs`
- Modify: `crates/right-agent/src/agent/destroy.rs`
- Modify: `crates/right/src/main.rs`

- [ ] **Step 1: Update agent registration**

In `crates/right-agent/src/agent/register.rs`, replace:

```rust
right_codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

client.reload_configuration().await?;
tracing::info!(agent = %options.agent_name, "reloaded process-compose configuration");
```

with:

```rust
let codegen_outcome = right_codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

client.reload_configuration().await?;
client
    .restart_cloudflared_if_config_changed(codegen_outcome.cloudflared_config_changed)
    .await?;
tracing::info!(
    agent = %options.agent_name,
    cloudflared_config_changed = codegen_outcome.cloudflared_config_changed,
    "reloaded process-compose configuration"
);
```

- [ ] **Step 2: Update agent destroy**

In `crates/right-agent/src/agent/destroy.rs`, replace:

```rust
right_codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

match pc_client.reload_configuration().await {
    Ok(()) => {
        tracing::info!("reloaded process-compose configuration");
        result.pc_reloaded = true;
    }
```

with:

```rust
let codegen_outcome = right_codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

match pc_client.reload_configuration().await {
    Ok(()) => {
        tracing::info!(
            cloudflared_config_changed = codegen_outcome.cloudflared_config_changed,
            "reloaded process-compose configuration"
        );
        result.pc_reloaded = true;
        if let Err(e) = pc_client
            .restart_cloudflared_if_config_changed(codegen_outcome.cloudflared_config_changed)
            .await
        {
            tracing::warn!(
                error = format!("{e:#}"),
                "failed to restart cloudflared after config change"
            );
        }
    }
```

- [ ] **Step 3: Update explicit `right reload`**

In `crates/right/src/main.rs`, inside `cmd_reload`, replace:

```rust
right_codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

client.reload_configuration().await?;
```

with:

```rust
let codegen_outcome = right_codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

client.reload_configuration().await?;
client
    .restart_cloudflared_if_config_changed(codegen_outcome.cloudflared_config_changed)
    .await?;
```

- [ ] **Step 4: Run compile check for affected crates**

Run:

```bash
devenv shell -- cargo check -p right-agent -p right
```

Expected: PASS.

- [ ] **Step 5: Run targeted runtime/codegen tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen run_agent_codegen_reports_ -- --nocapture
devenv shell -- cargo test -p right-agent restart_cloudflared_if_config_changed -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/right-agent/src/agent/register.rs crates/right-agent/src/agent/destroy.rs crates/right/src/main.rs
git commit -m "fix(runtime): refresh tunnel after route changes"
```

---

### Task 5: Update Lifecycle Documentation

**Files:**
- Modify: `docs/architecture/lifecycle.md`

- [ ] **Step 1: Update the `right up` lifecycle block**

In `docs/architecture/lifecycle.md`, replace:

```markdown
right up [--agents x,y] [--detach] [--no-sandbox]
  ├─ Discover agents from agents/ directory
  ├─ Per agent: resolve secret for token map (generate if missing)
  ├─ Generate agent-tokens.json
  ├─ Generate process-compose.yaml (minijinja)
  ├─ Generate cloudflared config (if tunnel)
  └─ Launch process-compose (TUI or detached)
```

with:

```markdown
right up [--agents x,y] [--detach] [--no-sandbox]
  ├─ Discover agents from agents/ directory
  ├─ Per agent: resolve secret for token map (generate if missing)
  ├─ Generate agent-tokens.json
  ├─ Generate process-compose.yaml (minijinja)
  ├─ Generate cloudflared config and record whether content changed
  └─ Launch process-compose (TUI or detached)
```

- [ ] **Step 2: Add reload lifecycle block**

In `docs/architecture/lifecycle.md`, add this block immediately after the `right up` block:

```markdown
right reload / running agent register / running agent destroy
  ├─ Discover agents from agents/ directory
  ├─ Run cross-agent codegen and record whether cloudflared config content changed
  ├─ POST /project/configuration to process-compose
  ├─ If cloudflared config changed: restart `cloudflared` via process-compose
  └─ Notify aggregator reload path when applicable
```

- [ ] **Step 3: Commit docs**

Run:

```bash
git add docs/architecture/lifecycle.md
git commit -m "docs(runtime): document tunnel refresh on reload"
```

---

### Task 6: Final Verification

**Files:**
- No code changes.

- [ ] **Step 1: Run full workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 2: Run final workspace build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 3: Inspect git history and working tree**

Run:

```bash
git status --short
git log --oneline -5
```

Expected: only intentional files changed or committed. Existing unrelated dirty files must not be reverted.

---

## Self-Review

Spec coverage:
- Detects relevant cloudflared ingress config content changes: Task 1 and Task 2.
- Restarts `cloudflared` only after process-compose accepts the updated config: Task 4.
- Covers register, destroy, and explicit reload paths: Task 4.
- Avoids restarting on unchanged config: Task 2 and Task 3.
- Documents lifecycle behavior: Task 5.
- Runs targeted and full verification: Task 4 and Task 6.

Placeholder scan:
- No `TBD`, `TODO`, `implement later`, or vague "handle edge cases" instructions remain.

Type consistency:
- `CodegenOutcome.cloudflared_config_changed` is introduced in Task 2 and used by the exact same field name in Task 4.
- `restart_cloudflared_if_config_changed(bool)` is introduced in Task 3 and called by the exact same method name in Task 4.
