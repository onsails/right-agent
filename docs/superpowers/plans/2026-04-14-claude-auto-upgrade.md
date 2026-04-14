# Claude Code Auto-Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically upgrade Claude Code inside OpenShell sandboxes every 8 hours via a background tokio task.

**Architecture:** New `upgrade.rs` module in rightclaw-bot spawns a tokio task that runs `claude upgrade` via SSH on an interval. Policy codegen adds `storage.googleapis.com` to restrictive mode so the sandbox can reach the distribution CDN.

**Tech Stack:** tokio (interval + timeout), SSH exec via `rightclaw::openshell::ssh_exec`, tracing for logging

---

### File Map

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `crates/bot/src/upgrade.rs` | Background upgrade task |
| Modify | `crates/bot/src/lib.rs:1-7` | Add `mod upgrade;` declaration |
| Modify | `crates/bot/src/lib.rs:370-380` | Spawn upgrade task alongside cron |
| Modify | `crates/rightclaw/src/codegen/policy.rs:6-13` | Add `storage.googleapis.com` to `RESTRICTIVE_DOMAINS` |
| Modify | `crates/rightclaw/src/codegen/policy.rs:179-188` | Update restrictive policy test assertion |
| Create | `crates/bot/tests/sandbox_upgrade.rs` | Integration test (ignored, requires live sandbox) |

---

### Task 1: Add `storage.googleapis.com` to restrictive policy

**Files:**
- Modify: `crates/rightclaw/src/codegen/policy.rs:6-13` (RESTRICTIVE_DOMAINS)
- Modify: `crates/rightclaw/src/codegen/policy.rs:179-188` (test assertion)

- [ ] **Step 1: Update the test to expect the new domain**

In `crates/rightclaw/src/codegen/policy.rs`, add an assertion to `restrictive_policy_allows_only_anthropic_domains`:

```rust
#[test]
fn restrictive_policy_allows_only_anthropic_domains() {
    let policy = generate_policy(8100, &NetworkPolicy::Restrictive, None);
    assert!(policy.contains(r#"host: "*.anthropic.com""#));
    assert!(policy.contains(r#"host: "anthropic.com""#));
    assert!(policy.contains(r#"host: "*.claude.com""#));
    assert!(policy.contains(r#"host: "claude.com""#));
    assert!(policy.contains(r#"host: "*.claude.ai""#));
    assert!(policy.contains(r#"host: "claude.ai""#));
    assert!(policy.contains(r#"host: "storage.googleapis.com""#));
    assert!(!policy.contains(r#"host: "**.*""#), "restrictive must not contain wildcard");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw restrictive_policy_allows_only_anthropic_domains`
Expected: FAIL — `storage.googleapis.com` not yet in RESTRICTIVE_DOMAINS

- [ ] **Step 3: Add the domain to RESTRICTIVE_DOMAINS**

In `crates/rightclaw/src/codegen/policy.rs`, change `RESTRICTIVE_DOMAINS`:

```rust
const RESTRICTIVE_DOMAINS: &[&str] = &[
    "*.anthropic.com",
    "anthropic.com",
    "*.claude.com",
    "claude.com",
    "*.claude.ai",
    "claude.ai",
    "storage.googleapis.com",
];
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rightclaw restrictive_policy_allows_only_anthropic_domains`
Expected: PASS

- [ ] **Step 5: Run all policy tests**

Run: `cargo test -p rightclaw policy`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/policy.rs
git commit -m "feat: add storage.googleapis.com to restrictive policy for claude upgrade"
```

---

### Task 2: Create `upgrade.rs` module

**Files:**
- Create: `crates/bot/src/upgrade.rs`

- [ ] **Step 1: Create the upgrade module**

Create `crates/bot/src/upgrade.rs`:

```rust
//! Background task that periodically upgrades Claude Code inside a sandbox.
//!
//! Runs `claude upgrade` via SSH every 8 hours. The upgraded binary is installed
//! to `/sandbox/.local/bin/claude` and takes precedence over the image-baked
//! `/usr/local/bin/claude` via PATH ordering (set up by `sync.rs`).

use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Default interval between upgrade checks (8 hours).
const UPGRADE_INTERVAL: Duration = Duration::from_secs(8 * 3600);

/// Timeout for `claude upgrade` SSH command (2 minutes).
const UPGRADE_TIMEOUT_SECS: u64 = 120;

/// Spawn a background task that periodically runs `claude upgrade` in the sandbox.
///
/// First tick fires immediately on startup, then every 8 hours.
/// Errors are logged but never propagated — the task keeps running.
pub fn spawn_upgrade_task(
    ssh_config_path: PathBuf,
    agent_name: String,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        run_upgrade_loop(&ssh_config_path, &agent_name, shutdown).await;
    });
}

async fn run_upgrade_loop(ssh_config_path: &Path, agent_name: &str, shutdown: CancellationToken) {
    let mut interval = tokio::time::interval(UPGRADE_INTERVAL);
    let ssh_host = rightclaw::openshell::ssh_host(agent_name);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!(agent = %agent_name, "upgrade task shutting down");
                return;
            }
            _ = interval.tick() => {
                run_upgrade(ssh_config_path, &ssh_host, agent_name).await;
            }
        }
    }
}

async fn run_upgrade(ssh_config_path: &Path, ssh_host: &str, agent_name: &str) {
    tracing::info!(agent = %agent_name, "checking for claude upgrade");

    let result = rightclaw::openshell::ssh_exec(
        ssh_config_path,
        ssh_host,
        &["claude", "upgrade"],
        UPGRADE_TIMEOUT_SECS,
    )
    .await;

    match result {
        Ok(stdout) => {
            let stdout = stdout.trim();
            if stdout.contains("Successfully updated") {
                tracing::info!(agent = %agent_name, output = %stdout, "claude upgraded");
            } else if stdout.contains("already") || stdout.contains("up to date") {
                tracing::info!(agent = %agent_name, "claude is up to date");
            } else {
                tracing::info!(agent = %agent_name, output = %stdout, "claude upgrade completed");
            }
        }
        Err(e) => {
            tracing::error!(agent = %agent_name, "claude upgrade failed: {e:#}");
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rightclaw-bot`
Expected: warning about dead code (module not yet wired), but no errors. If it complains about module not found, proceed to Task 3 step 1 first.

---

### Task 3: Wire upgrade task into bot startup

**Files:**
- Modify: `crates/bot/src/lib.rs:1-7` (module declaration)
- Modify: `crates/bot/src/lib.rs:370-380` (spawn task)

- [ ] **Step 1: Add module declaration**

In `crates/bot/src/lib.rs`, add `mod upgrade;` after line 6 (`pub mod sync;`):

```rust
mod config_watcher;
pub mod cron;
pub mod cron_delivery;
pub mod error;
pub mod login;
pub mod sync;
pub mod telegram;
mod upgrade;
```

- [ ] **Step 2: Spawn upgrade task after cron spawn**

In `crates/bot/src/lib.rs`, after the cron task spawn block (around line 382, after `});` that closes the cron_handle spawn), add:

```rust
    // Spawn periodic claude upgrade task (sandbox-only).
    if let Some(ref cfg_path) = ssh_config_path {
        upgrade::spawn_upgrade_task(
            cfg_path.clone(),
            args.agent.clone(),
            shutdown.clone(),
        );
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rightclaw-bot`
Expected: PASS, no errors

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/upgrade.rs crates/bot/src/lib.rs
git commit -m "feat: auto-upgrade claude code in sandboxes every 8 hours"
```

---

### Task 4: Integration test for `claude upgrade` in sandbox

**Files:**
- Create: `crates/bot/tests/sandbox_upgrade.rs`

- [ ] **Step 1: Create the integration test**

Create `crates/bot/tests/sandbox_upgrade.rs`:

```rust
//! Integration tests for `claude upgrade` inside an OpenShell sandbox.
//!
//! These tests require a running OpenShell gateway and an existing sandbox
//! named `rightclaw-rightclaw-test-lifecycle` with `storage.googleapis.com`
//! in its network policy.
//!
//! Run with: cargo test -p rightclaw-bot --test sandbox_upgrade

use std::process::Command;

/// Helper: run a command inside the test sandbox via `openshell sandbox exec`.
fn sandbox_exec(args: &[&str]) -> std::process::Output {
    Command::new("openshell")
        .args(["sandbox", "exec", "-n", "rightclaw-rightclaw-test-lifecycle", "--"])
        .args(args)
        .output()
        .expect("openshell binary must be in PATH")
}

/// `claude upgrade` completes without error.
#[test]
fn claude_upgrade_succeeds() {
    let output = sandbox_exec(&["claude", "upgrade"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "claude upgrade failed:\nstdout: {stdout}\nstderr: {stderr}"
    );
    // Must contain either "Successfully updated" or version info
    assert!(
        stdout.contains("Successfully updated") || stdout.contains("Current version"),
        "unexpected output:\n{stdout}"
    );
}

/// After upgrade, the binary exists in `.local/bin`.
#[test]
fn upgraded_binary_exists_in_local_bin() {
    let output = sandbox_exec(&["test", "-L", "/sandbox/.local/bin/claude"]);
    assert!(
        output.status.success(),
        "/sandbox/.local/bin/claude symlink does not exist — run claude_upgrade_succeeds first"
    );
}

/// The upgraded binary reports a valid version.
#[test]
fn upgraded_binary_reports_version() {
    let output = sandbox_exec(&["/sandbox/.local/bin/claude", "--version"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "version check failed");
    assert!(
        stdout.contains("Claude Code"),
        "expected 'Claude Code' in version output, got: {stdout}"
    );
}

/// With `.local/bin` in PATH (via .bashrc), `which claude` resolves to the upgraded path.
#[test]
fn path_precedence_favors_local_bin() {
    let output = sandbox_exec(&["bash", "-lc", "which claude"]);
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    assert_eq!(
        stdout, "/sandbox/.local/bin/claude",
        "expected PATH to resolve to .local/bin/claude, got: {stdout}"
    );
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo test -p rightclaw-bot --test sandbox_upgrade --no-run`
Expected: Compiles successfully

- [ ] **Step 3: Commit**

```bash
git add crates/bot/tests/sandbox_upgrade.rs
git commit -m "test: integration tests for claude upgrade in sandbox"
```

---

### Task 5: Full build verification

- [ ] **Step 1: Run full workspace build**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 2: Run all tests (excluding ignored)**

Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace`
Expected: No warnings
