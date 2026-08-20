# MCP Agent-Facing Self-Heal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add automatic repair for stale Claude Code `right` MCP `needs-auth` cache using the existing Haiku keepalive path, without retrying current user turns or replacing Claude sessions.

**Architecture:** Reuse the current `keepalive` background loop as a Claude health service that runs a Haiku `claude -p` probe with strict MCP config and parses `system/init`. Share one repair handle with Telegram workers; health probes and user-turn observation both trigger the same bounded repair path, while only successful repair sets a one-shot next-turn system notification.

**Tech Stack:** Rust 2024, Tokio, Claude Code stream-json, OpenShell `SandboxExec`, existing `ClaudeInvocation` builder, existing platform-store sync.

---

## Scope And Verification Cadence

- The approved design is `docs/superpowers/specs/2026-05-18-mcp-agent-facing-self-heal-design.md`.
- This touches Rust. Before editing Rust, load `rust-dev:rust-dev` if that skill is available in the implementation session. If it is unavailable, record that explicitly and continue with the repo Rust conventions.
- Start with one targeted baseline test: `devenv shell -- cargo test -p right-bot keepalive`.
- Prefer targeted package tests while iterating.
- Final verification is mandatory:
  - `devenv shell -- cargo test --workspace`
  - `devenv shell -- cargo build --workspace`
- Do not modify or revert unrelated local changes. At plan-writing time, `crates/right-openshell/src/openshell_tests.rs` had an unrelated unstaged diff.

## File Structure

- Modify: `crates/bot/src/cc/stream.rs`
  - Owns parsing Claude Code stream-json. Add `RightMcpInitStatus` and parser tests.
- Modify or rename: `crates/bot/src/keepalive.rs`
  - Keep the file name for smaller patch surface. Expand responsibility from token keepalive to Claude health probing and MCP repair.
- Modify: `crates/bot/src/sync.rs`
  - Expose `sync_cycle` as `pub(crate)` so the health repair path can redeploy the platform manifest.
- Modify: `crates/bot/src/lib.rs`
  - Construct `SandboxExec` once, pass it to sync and health, spawn immediate/periodic health probe, pass health handle to Telegram.
- Modify: `crates/bot/src/telegram/handler.rs`
  - Carry the health handle through `AgentSettings` into `WorkerContext`.
- Modify: `crates/bot/src/telegram/dispatch.rs`
  - Add health handle to `run_telegram` parameters and settings.
- Modify: `crates/bot/src/telegram/worker.rs`
  - Observe user-turn `system/init`, trigger async repair without interrupting the turn, and inject one-shot repair notification into the next prompt.
- Modify: `docs/architecture/mcp.md`
  - Document Aggregator status vs agent-facing Claude Code MCP init and health repair.
- Modify: `docs/architecture/lifecycle.md`
  - Document startup/periodic Claude health probe and user-turn observation.
- Optional modify: `docs/architecture/modules.md`
  - If `keepalive.rs` responsibility wording becomes stale, update the `right-bot` module summary.

---

### Task 1: Baseline And Stream Init Parser

**Files:**
- Modify: `crates/bot/src/cc/stream.rs`

- [ ] **Step 1: Run targeted baseline**

Run:

```bash
devenv shell -- cargo test -p right-bot keepalive
```

Expected: existing keepalive tests pass. If they fail before edits, record the failure in the implementation notes and continue only if unrelated.

- [ ] **Step 2: Write failing parser tests**

Add these tests inside `#[cfg(test)] mod tests` in `crates/bot/src/cc/stream.rs`. If the module has existing tests, append these there; otherwise create the module at the bottom of the file.

```rust
#[test]
fn parse_right_mcp_init_status_connected() {
    let line = r#"{
        "type":"system",
        "subtype":"init",
        "mcp_servers":[
            {"name":"right","status":"connected"},
            {"name":"composio","status":"connected"}
        ]
    }"#;

    assert_eq!(
        parse_right_mcp_init_status(line),
        Some(RightMcpInitStatus::Connected)
    );
}

#[test]
fn parse_right_mcp_init_status_needs_auth() {
    let line = r#"{
        "type":"system",
        "subtype":"init",
        "mcp_servers":[{"name":"right","status":"needs-auth"}]
    }"#;

    assert_eq!(
        parse_right_mcp_init_status(line),
        Some(RightMcpInitStatus::Unhealthy {
            status: Some("needs-auth".to_owned())
        })
    );
}

#[test]
fn parse_right_mcp_init_status_missing_right_is_unhealthy() {
    let line = r#"{
        "type":"system",
        "subtype":"init",
        "mcp_servers":[{"name":"composio","status":"connected"}]
    }"#;

    assert_eq!(
        parse_right_mcp_init_status(line),
        Some(RightMcpInitStatus::Unhealthy { status: None })
    );
}

#[test]
fn parse_right_mcp_init_status_ignores_non_init_lines() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;

    assert_eq!(parse_right_mcp_init_status(line), None);
}
```

- [ ] **Step 3: Run parser tests and verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-bot parse_right_mcp_init_status
```

Expected: compile failure because `parse_right_mcp_init_status` and `RightMcpInitStatus` do not exist.

- [ ] **Step 4: Implement parser**

Add this code near `parse_api_key_source` in `crates/bot/src/cc/stream.rs`:

```rust
/// Agent-facing status of the built-in `right` MCP server from Claude Code's
/// `system/init` stream event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RightMcpInitStatus {
    Connected,
    /// `status = None` means the init event did not list `right` at all.
    Unhealthy { status: Option<String> },
}

/// Parse the built-in `right` MCP server status from a Claude Code
/// `system/init` NDJSON line.
///
/// Returns `None` for non-init lines and malformed JSON. Returns
/// `Unhealthy { status: None }` when the line is an init event but `right`
/// is absent, because the agent-facing MCP registry is missing the platform
/// server.
pub(crate) fn parse_right_mcp_init_status(init_json: &str) -> Option<RightMcpInitStatus> {
    let v: serde_json::Value = serde_json::from_str(init_json).ok()?;
    if v.get("type")?.as_str()? != "system" {
        return None;
    }
    if v.get("subtype")?.as_str()? != "init" {
        return None;
    }

    let servers = v.get("mcp_servers").and_then(|s| s.as_array())?;
    for server in servers {
        if server.get("name").and_then(|n| n.as_str()) == Some("right") {
            let status = server
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown");
            return Some(if status == "connected" {
                RightMcpInitStatus::Connected
            } else {
                RightMcpInitStatus::Unhealthy {
                    status: Some(status.to_owned()),
                }
            });
        }
    }

    Some(RightMcpInitStatus::Unhealthy { status: None })
}
```

- [ ] **Step 5: Run parser tests and verify they pass**

Run:

```bash
devenv shell -- cargo test -p right-bot parse_right_mcp_init_status
```

Expected: all four parser tests pass.

- [ ] **Step 6: Commit parser**

```bash
git add crates/bot/src/cc/stream.rs
git commit -m "feat(bot): parse right mcp init status"
```

---

### Task 2: Health Probe Command And Pure Decisions

**Files:**
- Modify: `crates/bot/src/keepalive.rs`

- [ ] **Step 1: Write failing tests for health command args**

Replace the existing single keepalive test module in `crates/bot/src/keepalive.rs` with tests that preserve the one-hour interval and lock down the probe invocation.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_interval_is_one_hour() {
        assert_eq!(DEFAULT_INTERVAL, Duration::from_secs(3600));
    }

    #[test]
    fn health_probe_invocation_uses_haiku_stream_json_and_strict_mcp() {
        let args = health_probe_invocation("/sandbox/mcp.json").into_args();

        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"haiku".to_string()));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        assert!(args.contains(&"--mcp-config".to_string()));
        assert!(args.contains(&"/sandbox/mcp.json".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--max-turns".to_string()));
        assert!(args.contains(&"1".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
    }

    #[test]
    fn init_status_decision_maps_only_connected_to_healthy() {
        assert_eq!(
            classify_init_status(crate::cc::stream::RightMcpInitStatus::Connected),
            ProbeInitDecision::Healthy
        );
        assert_eq!(
            classify_init_status(crate::cc::stream::RightMcpInitStatus::Unhealthy {
                status: Some("needs-auth".to_owned())
            }),
            ProbeInitDecision::Repair
        );
        assert_eq!(
            classify_init_status(crate::cc::stream::RightMcpInitStatus::Unhealthy {
                status: None
            }),
            ProbeInitDecision::Repair
        );
    }
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-bot health_probe_invocation
devenv shell -- cargo test -p right-bot init_status_decision
```

Expected: compile failure for missing `health_probe_invocation`, `ProbeInitDecision`, and `classify_init_status`.

- [ ] **Step 3: Add pure probe helpers**

Add these definitions above `spawn_keepalive` in `crates/bot/src/keepalive.rs`:

```rust
const HEALTH_PROMPT: &str = "Reply exactly OK. Do not use tools.";
const REPAIR_NOTICE: &str =
    "Right MCP stale needs-auth cache was repaired. Use current MCP tool availability, not previous disconnected status.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeInitDecision {
    Healthy,
    Repair,
}

fn classify_init_status(status: crate::cc::stream::RightMcpInitStatus) -> ProbeInitDecision {
    match status {
        crate::cc::stream::RightMcpInitStatus::Connected => ProbeInitDecision::Healthy,
        crate::cc::stream::RightMcpInitStatus::Unhealthy { .. } => ProbeInitDecision::Repair,
    }
}

fn health_probe_invocation(mcp_config_path: &str) -> crate::cc::invocation::ClaudeInvocation {
    crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: Some(mcp_config_path.to_owned()),
        json_schema: None,
        output_format: crate::cc::invocation::OutputFormat::StreamJson,
        model: Some("haiku".to_owned()),
        max_budget_usd: None,
        max_turns: Some(1),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: vec!["--no-session-persistence".to_owned()],
        prompt: Some(HEALTH_PROMPT.to_owned()),
        debug_flag: None,
    }
}
```

- [ ] **Step 4: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot health_probe_invocation
devenv shell -- cargo test -p right-bot init_status_decision
```

Expected: both pass.

- [ ] **Step 5: Commit pure health helpers**

```bash
git add crates/bot/src/keepalive.rs
git commit -m "test(bot): define claude health probe command"
```

---

### Task 3: Health Handle, Repair Lock, And Notice State

**Files:**
- Modify: `crates/bot/src/keepalive.rs`

- [ ] **Step 1: Write failing tests for one-shot notice and in-flight lock**

Add these tests to `crates/bot/src/keepalive.rs`:

```rust
#[test]
fn repair_notice_is_one_shot() {
    let health = ClaudeHealth::new(
        "him".to_owned(),
        PathBuf::from("/tmp/agent"),
        None,
        None,
        None,
    );

    assert_eq!(health.consume_repair_notice(), None);
    health.mark_repaired_for_next_turn();
    assert_eq!(health.consume_repair_notice(), Some(REPAIR_NOTICE));
    assert_eq!(health.consume_repair_notice(), None);
}

#[test]
fn repair_lock_rejects_concurrent_second_holder() {
    let health = ClaudeHealth::new(
        "him".to_owned(),
        PathBuf::from("/tmp/agent"),
        None,
        None,
        None,
    );

    let first = health.try_begin_repair_for_test();
    assert!(first.is_some());
    assert!(health.try_begin_repair_for_test().is_none());
    drop(first);
    assert!(health.try_begin_repair_for_test().is_some());
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-bot repair_notice repair_lock
```

Expected: compile failure for missing `ClaudeHealth` and methods.

- [ ] **Step 3: Add health handle type**

Add imports near the top of `crates/bot/src/keepalive.rs`:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
```

Add the handle below the pure helpers:

```rust
pub(crate) struct ClaudeHealth {
    agent_name: String,
    agent_dir: PathBuf,
    ssh_config_path: Option<PathBuf>,
    resolved_sandbox: Option<String>,
    sandbox_exec: Option<right_openshell::sandbox_exec::SandboxExec>,
    repair_lock: tokio::sync::Mutex<()>,
    repair_notice_pending: AtomicBool,
}

impl ClaudeHealth {
    pub(crate) fn new(
        agent_name: String,
        agent_dir: PathBuf,
        ssh_config_path: Option<PathBuf>,
        resolved_sandbox: Option<String>,
        sandbox_exec: Option<right_openshell::sandbox_exec::SandboxExec>,
    ) -> Arc<Self> {
        Arc::new(Self {
            agent_name,
            agent_dir,
            ssh_config_path,
            resolved_sandbox,
            sandbox_exec,
            repair_lock: tokio::sync::Mutex::new(()),
            repair_notice_pending: AtomicBool::new(false),
        })
    }

    pub(crate) fn consume_repair_notice(&self) -> Option<&'static str> {
        if self.repair_notice_pending.swap(false, Ordering::AcqRel) {
            Some(REPAIR_NOTICE)
        } else {
            None
        }
    }

    fn mark_repaired_for_next_turn(&self) {
        self.repair_notice_pending.store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn try_begin_repair_for_test(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.repair_lock.try_lock().ok()
    }
}
```

- [ ] **Step 4: Run health state tests**

Run:

```bash
devenv shell -- cargo test -p right-bot repair_notice
devenv shell -- cargo test -p right-bot repair_lock
```

Expected: pass.

- [ ] **Step 5: Commit health state**

```bash
git add crates/bot/src/keepalive.rs
git commit -m "test(bot): add claude health repair state"
```

---

### Task 4: Subprocess Probe Implementation

**Files:**
- Modify: `crates/bot/src/keepalive.rs`

- [ ] **Step 1: Replace token-only ping with stream-json health probe**

Replace `ping_claude` in `crates/bot/src/keepalive.rs` with this implementation shape. Keep error strings secret-free.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum HealthProbeOutcome {
    Healthy,
    NeedsRepair { status: Option<String> },
    NoInit,
}

async fn run_health_probe(health: &ClaudeHealth) -> Result<HealthProbeOutcome, String> {
    let mcp_path =
        crate::cc::invocation::mcp_config_path(health.ssh_config_path.as_deref(), &health.agent_dir);
    let args = health_probe_invocation(&mcp_path).into_args();
    let mut cmd = crate::cc::invocation::build_claude_command(
        &args,
        &health.agent_dir,
        health.ssh_config_path.as_deref(),
        health.resolved_sandbox.as_deref(),
    );

    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());

    let mut child =
        right_process::ProcessGroupChild::spawn(cmd).map_err(|e| format!("spawn failed: {e:#}"))?;
    let stdout = child
        .stdout()
        .ok_or_else(|| "health probe missing stdout".to_string())?;
    let mut lines = tokio::io::BufReader::new(stdout).lines();

    let mut init_outcome = HealthProbeOutcome::NoInit;
    let mut killed_for_repair = false;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| format!("stdout read failed: {e:#}"))?
    {
        if let Some(status) = crate::cc::stream::parse_right_mcp_init_status(&line) {
            init_outcome = match status {
                crate::cc::stream::RightMcpInitStatus::Connected => HealthProbeOutcome::Healthy,
                crate::cc::stream::RightMcpInitStatus::Unhealthy { status } => {
                    child.kill().await.ok();
                    killed_for_repair = true;
                    HealthProbeOutcome::NeedsRepair { status }
                }
            };
            break;
        }
    }

    if killed_for_repair {
        let _ = child.wait().await;
    } else {
        let status = child
            .wait()
            .await
            .map_err(|e| format!("wait failed: {e:#}"))?;
        if !status.success() {
            return Err(format!("exit code: {}", status.code().unwrap_or(-1)));
        }
    }

    Ok(init_outcome)
}
```

Also import the line reader trait:

```rust
use tokio::io::AsyncBufReadExt;
```

- [ ] **Step 2: Adjust loop to call the probe through `ClaudeHealth`**

Update `spawn_keepalive` and `run_keepalive_loop` signatures to accept `Arc<ClaudeHealth>` instead of raw paths:

```rust
pub(crate) fn spawn_keepalive(
    health: Arc<ClaudeHealth>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_keepalive_loop(health, shutdown).await;
    })
}

async fn run_keepalive_loop(health: Arc<ClaudeHealth>, shutdown: CancellationToken) {
    run_one_health_cycle(Arc::clone(&health), "startup").await;

    let mut interval = tokio::time::interval(DEFAULT_INTERVAL);
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => run_one_health_cycle(Arc::clone(&health), "periodic").await,
            _ = shutdown.cancelled() => {
                tracing::debug!("claude_health: shutdown");
                return;
            }
        }
    }
}

async fn run_one_health_cycle(health: Arc<ClaudeHealth>, reason: &'static str) {
    tracing::info!(agent = %health.agent_name, reason, "claude_health: probing");
    match run_health_probe(&health).await {
        Ok(HealthProbeOutcome::Healthy) => {
            tracing::info!(agent = %health.agent_name, reason, "claude_health: ok");
        }
        Ok(HealthProbeOutcome::NeedsRepair { status }) => {
            tracing::warn!(
                agent = %health.agent_name,
                reason,
                right_status = status.as_deref().unwrap_or("missing"),
                "claude_health: right MCP unhealthy; scheduling repair"
            );
            health.trigger_repair(reason).await;
        }
        Ok(HealthProbeOutcome::NoInit) => {
            tracing::warn!(agent = %health.agent_name, reason, "claude_health: no system/init");
        }
        Err(e) => tracing::warn!(agent = %health.agent_name, reason, "claude_health: failed: {e}"),
    }
}
```

At this point `trigger_repair` can be a temporary stub:

```rust
pub(crate) async fn trigger_repair(self: &Arc<Self>, reason: &'static str) {
    tracing::warn!(agent = %self.agent_name, reason, "claude_health: repair deferred to Task 5");
}
```

- [ ] **Step 3: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot keepalive
```

Expected: compile and tests pass. No live Claude probe is run by tests.

- [ ] **Step 4: Commit probe subprocess path**

```bash
git add crates/bot/src/keepalive.rs
git commit -m "feat(bot): probe claude mcp health with haiku"
```

---

### Task 5: Repair Implementation And Sync Exposure

**Files:**
- Modify: `crates/bot/src/sync.rs`
- Modify: `crates/bot/src/keepalive.rs`

- [ ] **Step 1: Expose platform sync cycle**

In `crates/bot/src/sync.rs`, change:

```rust
async fn sync_cycle(
    agent_dir: &Path,
    sbox: &right_openshell::sandbox_exec::SandboxExec,
) -> miette::Result<()> {
```

to:

```rust
pub(crate) async fn sync_cycle(
    agent_dir: &Path,
    sbox: &right_openshell::sandbox_exec::SandboxExec,
) -> miette::Result<()> {
```

- [ ] **Step 2: Implement cache cleanup helpers**

Add to `crates/bot/src/keepalive.rs`:

```rust
async fn remove_needs_auth_cache(health: &ClaudeHealth) -> Result<(), String> {
    if let Some(sbox) = health.sandbox_exec.as_ref() {
        let (output, code) = sbox
            .exec(&["rm", "-f", "/sandbox/.claude/mcp-needs-auth-cache.json"])
            .await
            .map_err(|e| format!("sandbox cache cleanup exec failed: {e:#}"))?;
        if code != 0 {
            return Err(format!(
                "sandbox cache cleanup exited {code}: {}",
                output.trim()
            ));
        }
        return Ok(());
    }

    let path = health
        .agent_dir
        .join(".claude")
        .join("mcp-needs-auth-cache.json");
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("local cache cleanup failed at {}: {e:#}", path.display())),
    }
}

async fn sync_after_cache_cleanup(health: &ClaudeHealth) -> Result<(), String> {
    if let Some(sbox) = health.sandbox_exec.as_ref() {
        crate::sync::sync_cycle(&health.agent_dir, sbox)
            .await
            .map_err(|e| format!("platform sync failed: {e:#}"))?;
    }
    Ok(())
}
```

- [ ] **Step 3: Replace repair stub with bounded repair**

Replace the temporary `trigger_repair` in `crates/bot/src/keepalive.rs`:

```rust
pub(crate) async fn trigger_repair(self: &Arc<Self>, reason: &'static str) {
    let Ok(_guard) = self.repair_lock.try_lock() else {
        tracing::debug!(
            agent = %self.agent_name,
            reason,
            "claude_health: repair already running"
        );
        return;
    };

    tracing::warn!(agent = %self.agent_name, reason, "claude_health: repairing MCP cache");

    if let Err(e) = remove_needs_auth_cache(self).await {
        tracing::warn!(agent = %self.agent_name, reason, "claude_health: {e}");
    }

    if let Err(e) = sync_after_cache_cleanup(self).await {
        tracing::error!(agent = %self.agent_name, reason, "claude_health: {e}");
        return;
    }

    match run_health_probe(self).await {
        Ok(HealthProbeOutcome::Healthy) => {
            self.mark_repaired_for_next_turn();
            tracing::info!(agent = %self.agent_name, reason, "claude_health: repair succeeded");
        }
        Ok(HealthProbeOutcome::NeedsRepair { status }) => {
            tracing::error!(
                agent = %self.agent_name,
                reason,
                right_status = status.as_deref().unwrap_or("missing"),
                "claude_health: repair probe still unhealthy"
            );
        }
        Ok(HealthProbeOutcome::NoInit) => {
            tracing::error!(agent = %self.agent_name, reason, "claude_health: repair probe had no init");
        }
        Err(e) => {
            tracing::error!(agent = %self.agent_name, reason, "claude_health: repair probe failed: {e}");
        }
    }
}
```

- [ ] **Step 4: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot keepalive
devenv shell -- cargo test -p right-bot parse_right_mcp_init_status
```

Expected: pass.

- [ ] **Step 5: Commit repair implementation**

```bash
git add crates/bot/src/keepalive.rs crates/bot/src/sync.rs
git commit -m "feat(bot): repair stale claude mcp auth cache"
```

---

### Task 6: Startup And Periodic Health Wiring

**Files:**
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Create shared `ClaudeHealth` after sandbox context is known**

In `crates/bot/src/lib.rs`, keep a cloned `SandboxExec` for health. Replace the current sync block shape with one that stores `sandbox_exec_for_health`:

```rust
let mut sandbox_exec_for_health: Option<right_openshell::sandbox_exec::SandboxExec> = None;

let sync_handle = if let Some((ref mtls_dir, ref sandbox_id)) = sandbox_ctx {
    let sandbox = resolved_sandbox.clone().unwrap();
    let sbox = right_openshell::sandbox_exec::SandboxExec::new(
        mtls_dir.clone(),
        sandbox,
        sandbox_id.clone(),
    );
    sandbox_exec_for_health = Some(sbox.clone());
    sync::initial_sync(&agent_dir, &sbox).await?;
    if let Err(e) = sync::reverse_sync_md(&agent_dir, sbox.sandbox_name()).await {
        tracing::warn!(
            agent = %args.agent,
            sandbox = %sbox.sandbox_name(),
            "startup identity mirror sync failed: {e:#}"
        );
    }
    let sync_agent_dir = agent_dir.clone();
    let sync_shutdown = shutdown.clone();
    Some(tokio::spawn(sync::run_sync_task(
        sync_agent_dir,
        sbox,
        sync_shutdown,
    )))
} else {
    None
};
```

- [ ] **Step 2: Build health handle and pass it to keepalive**

Replace the existing keepalive spawn:

```rust
let claude_health = keepalive::ClaudeHealth::new(
    args.agent.clone(),
    agent_dir.clone(),
    ssh_config_path.clone(),
    resolved_sandbox.clone(),
    sandbox_exec_for_health,
);

let keepalive_handle = keepalive::spawn_keepalive(
    Arc::clone(&claude_health),
    shutdown.clone(),
);
```

- [ ] **Step 3: Pass `claude_health` to Telegram**

Add `Arc::clone(&claude_health)` to the `telegram::run_telegram(...)` call after `debug_flag`:

```rust
result = telegram::run_telegram(
    token,
    allowlist,
    agent_dir,
    Arc::clone(&debug_flag),
    Arc::clone(&claude_health),
    Arc::clone(&pending_auth),
    home.clone(),
    ...
)
```

- [ ] **Step 4: Run compile-focused package test**

Run:

```bash
devenv shell -- cargo test -p right-bot keepalive
```

Expected: likely compile errors in Telegram signatures because the new argument is not wired yet. Continue to Task 7.

Do not commit this task separately if the workspace does not compile. Commit after Task 7.

---

### Task 7: Telegram Settings, Worker Observation, And Next-Turn Notice

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Add health handle to `run_telegram`**

In `crates/bot/src/telegram/dispatch.rs`, add this parameter after `debug`:

```rust
claude_health: std::sync::Arc<crate::keepalive::ClaudeHealth>,
```

Add this field to `AgentSettings` construction:

```rust
claude_health,
```

- [ ] **Step 2: Add field to `AgentSettings`**

In `crates/bot/src/telegram/handler.rs`, add:

```rust
/// Claude health and MCP self-heal handle.
pub claude_health: std::sync::Arc<crate::keepalive::ClaudeHealth>,
```

Add it to `WorkerContext` construction:

```rust
claude_health: Arc::clone(&settings.claude_health),
```

- [ ] **Step 3: Add field to `WorkerContext`**

In `crates/bot/src/telegram/worker.rs`, add:

```rust
/// Claude health and MCP self-heal handle.
pub claude_health: std::sync::Arc<crate::keepalive::ClaudeHealth>,
```

- [ ] **Step 4: Inject one-shot repair notice into base prompt**

In `invoke_cc`, replace:

```rust
let base_prompt =
    right_codegen::generate_system_prompt(&ctx.agent_name, &sandbox_mode, &home_dir);
```

with:

```rust
let mut base_prompt =
    right_codegen::generate_system_prompt(&ctx.agent_name, &sandbox_mode, &home_dir);
if let Some(notice) = ctx.claude_health.consume_repair_notice() {
    base_prompt.push_str("\n\n<system-notification>\n");
    base_prompt.push_str(notice);
    base_prompt.push_str("\n</system-notification>\n");
}
```

- [ ] **Step 5: Observe user-turn init and trigger repair without changing flow**

In the stream loop in `invoke_cc`, immediately after `parse_api_key_source`, add:

```rust
if let Some(status) = crate::cc::stream::parse_right_mcp_init_status(&line)
    && !matches!(status, crate::cc::stream::RightMcpInitStatus::Connected)
{
    let health = Arc::clone(&ctx.claude_health);
    tokio::spawn(async move {
        health.trigger_repair("user-turn-init").await;
    });
}
```

This must appear before `parse_stream_event` handling, and it must not call `child.kill()`, return early, or mutate `result_line`.

- [ ] **Step 6: Update dispatch tests or test fixtures**

Search for `run_telegram(` and `AgentSettings {` in tests:

```bash
rg -n "run_telegram\\(|AgentSettings \\{" crates/bot/src crates/bot/tests
```

For test-only settings, construct a no-sandbox health handle. Bind the tempdir before constructing the handle so the path outlives the settings:

```rust
let agent_tmp = tempfile::tempdir().unwrap();
let claude_health = crate::keepalive::ClaudeHealth::new(
    "test".to_owned(),
    agent_tmp.path().to_path_buf(),
    None,
    None,
    None,
);
```

- [ ] **Step 7: Run targeted compile/tests**

Run:

```bash
devenv shell -- cargo test -p right-bot keepalive
devenv shell -- cargo test -p right-bot parse_right_mcp_init_status
```

Expected: pass.

- [ ] **Step 8: Commit wiring**

```bash
git add crates/bot/src/lib.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs crates/bot/src/keepalive.rs
git commit -m "feat(bot): wire mcp self-heal into claude turns"
```

---

### Task 8: Focused Tests For Worker Observation Helpers

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Test: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Extract pure helper for notification wrapping**

Add near other pure helpers in `crates/bot/src/telegram/worker.rs`:

```rust
fn append_system_notification(base_prompt: &mut String, notice: &str) {
    base_prompt.push_str("\n\n<system-notification>\n");
    base_prompt.push_str(notice);
    base_prompt.push_str("\n</system-notification>\n");
}
```

Update Task 7's inline prompt code to call:

```rust
if let Some(notice) = ctx.claude_health.consume_repair_notice() {
    append_system_notification(&mut base_prompt, notice);
}
```

- [ ] **Step 2: Add pure test for notification wrapper**

Add to `crates/bot/src/telegram/worker.rs` tests:

```rust
#[test]
fn append_system_notification_wraps_notice_once() {
    let mut prompt = "base".to_owned();
    append_system_notification(&mut prompt, "repair complete");

    assert_eq!(
        prompt,
        "base\n\n<system-notification>\nrepair complete\n</system-notification>\n"
    );
}
```

- [ ] **Step 3: Extract pure helper for user-turn observation decision**

Add:

```rust
fn should_trigger_mcp_repair_from_init(line: &str) -> bool {
    matches!(
        crate::cc::stream::parse_right_mcp_init_status(line),
        Some(crate::cc::stream::RightMcpInitStatus::Unhealthy { .. })
    )
}
```

Replace the stream-loop condition with:

```rust
if should_trigger_mcp_repair_from_init(&line) {
    let health = Arc::clone(&ctx.claude_health);
    tokio::spawn(async move {
        health.trigger_repair("user-turn-init").await;
    });
}
```

- [ ] **Step 4: Add decision tests**

Add:

```rust
#[test]
fn should_trigger_mcp_repair_from_init_only_for_unhealthy_right() {
    let bad = r#"{"type":"system","subtype":"init","mcp_servers":[{"name":"right","status":"needs-auth"}]}"#;
    let good = r#"{"type":"system","subtype":"init","mcp_servers":[{"name":"right","status":"connected"}]}"#;
    let other = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;

    assert!(should_trigger_mcp_repair_from_init(bad));
    assert!(!should_trigger_mcp_repair_from_init(good));
    assert!(!should_trigger_mcp_repair_from_init(other));
}
```

- [ ] **Step 5: Run targeted worker tests**

Run:

```bash
devenv shell -- cargo test -p right-bot append_system_notification
devenv shell -- cargo test -p right-bot should_trigger_mcp_repair_from_init
```

Expected: pass.

- [ ] **Step 6: Commit worker helper tests**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "test(bot): cover mcp repair turn observation"
```

---

### Task 9: Architecture Documentation

**Files:**
- Modify: `docs/architecture/mcp.md`
- Modify: `docs/architecture/lifecycle.md`
- Modify if needed: `docs/architecture/modules.md`

- [ ] **Step 1: Update MCP architecture doc**

Add this section to `docs/architecture/mcp.md` after the Aggregator overview:

```markdown
## Agent-Facing MCP Health

`/mcp list` reports Aggregator backend status through the internal Unix-socket
API. It does not prove that a specific Claude Code process loaded the same MCP
tool registry. Agent turns are checked separately through Claude Code's
`system/init` stream-json event, which lists the MCP servers visible to that
process.

The bot runs a periodic Haiku health probe using the same strict MCP config
path as real turns (`/sandbox/mcp.json` in OpenShell mode, host `mcp.json` in
no-sandbox mode). If `system/init` reports the built-in `right` server as
`needs-auth` or omits it, the bot removes Claude Code's stale
`.claude/mcp-needs-auth-cache.json`, redeploys platform files, and probes once
more. The repair never recreates sandboxes, rewrites external MCP credentials,
or changes the user's Claude session.

Normal user turns also observe `system/init`. If a turn sees unhealthy `right`,
it schedules the same repair path asynchronously but does not kill, retry, or
replace the current turn.
```

- [ ] **Step 2: Update lifecycle doc**

In `docs/architecture/lifecycle.md`, add these bullets under `right bot --agent <name>` after initial/background sync:

```markdown
  ├─ Start Claude health loop:
  │   ├─ immediate startup Haiku probe with strict MCP config
  │   ├─ hourly Haiku probe for Claude OAuth keepalive + agent-facing MCP init
  │   └─ stale `right` MCP needs-auth cache repair when `system/init` is unhealthy
```

Under `Per message`, add after `Pipe input to claude -p via stdin`:

```markdown
  ├─ Observe Claude Code `system/init`; if `right` MCP is unhealthy, schedule
  │   cache repair asynchronously without interrupting or retrying the turn
```

- [ ] **Step 3: Update modules doc only if stale**

If `docs/architecture/modules.md` still describes `keepalive.rs` as token-only, update the `right-bot` list. Use this exact replacement if needed:

```markdown
- `keepalive.rs` — Claude health loop: Haiku OAuth keepalive, agent-facing MCP init probe, and stale MCP needs-auth cache repair.
```

If there is no token-only `keepalive.rs` entry, do not edit `modules.md`.

- [ ] **Step 4: Commit docs**

```bash
git add docs/architecture/mcp.md docs/architecture/lifecycle.md docs/architecture/modules.md
git commit -m "docs(mcp): document agent-facing health repair"
```

If `modules.md` was not changed, omit it from `git add`.

---

### Task 10: Final Verification And Review

**Files:**
- No code changes unless verification exposes a defect.

- [ ] **Step 1: Run right-bot targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot
```

Expected: pass.

- [ ] **Step 2: Run workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: pass. If there are pre-existing failures, record exact failing tests and verify they are unrelated before proceeding.

- [ ] **Step 3: Run workspace build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: pass.

- [ ] **Step 4: Run Rust review if available**

If `rust-dev:review-rust-code` is available in the implementation session, run it after tests and build. Convert findings into explicit fix items, resolve them one by one, then rerun affected targeted tests and final workspace verification.

If the review skill is unavailable, record that and continue.

- [ ] **Step 5: Inspect final diff**

Run:

```bash
git status --short
git diff --stat
git diff --check
```

Expected:

- only intended files changed;
- no whitespace errors;
- unrelated `crates/right-openshell/src/openshell_tests.rs` diff is neither staged nor modified by this work unless separately requested.

- [ ] **Step 6: Final commit if verification required fixes**

If final verification required fixes after the docs commit, commit them:

```bash
git add crates/bot/src/cc/stream.rs crates/bot/src/keepalive.rs crates/bot/src/sync.rs crates/bot/src/lib.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs docs/architecture/mcp.md docs/architecture/lifecycle.md docs/architecture/modules.md
git commit -m "fix(bot): stabilize mcp self-heal verification"
```

Omit unchanged files from `git add`.
