# SSH ControlMaster Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate per-message SSH handshake latency by enabling OpenSSH ControlMaster connection multiplexing in the SSH config the bot writes for each agent's sandbox.

**Architecture:** Append three `ControlMaster` directives to the SSH config file emitted by `generate_ssh_config`. The first `ssh -F <config>` call after bot startup (the existing `mkdir /sandbox/inbox /sandbox/outbox`) implicitly establishes the master. Subsequent calls — worker, cron, cron_delivery, reflection, keepalive, attachments — multiplex over it. Stale sockets from previous bot processes are cleaned up at startup; the master is torn down on graceful shutdown and during sandbox migration.

**Tech Stack:** Rust (edition 2024), tokio::process::Command, OpenSSH client (already a project dependency). No new crates.

**Spec:** `docs/superpowers/specs/2026-05-05-ssh-controlmaster-design.md`

---

## File Structure

**Create:**
- `crates/right-agent/tests/control_master.rs` — integration test using `TestSandbox`.

**Modify:**
- `crates/right-agent/src/openshell.rs` — extend `generate_ssh_config` to append ControlMaster directives; add helpers `control_master_socket_path`, `check_control_master`, `clean_stale_control_master`, `tear_down_control_master`.
- `crates/right-agent/src/openshell_tests.rs` — unit tests for the new helpers.
- `crates/bot/src/lib.rs` — call `clean_stale_control_master` before first `ssh_exec`; call `tear_down_control_master` in graceful-shutdown path.
- `crates/right/src/main.rs` — call `tear_down_control_master` in `perform_migration` after restoring data, before deleting the old SSH config file.

**No changes:** worker, cron, cron_delivery, reflection, keepalive, attachments, prompt, invocation. They use `ssh -F <config>` and inherit multiplexing for free.

---

## Task 1: Add `control_master_socket_path` helper

**Files:**
- Modify: `crates/right-agent/src/openshell.rs` (after the existing `ssh_host_for_sandbox` function around line 51).
- Modify: `crates/right-agent/src/openshell_tests.rs` (add tests after `ssh_host_for_sandbox_formats_correctly` around line 45).

- [ ] **Step 1: Write failing unit tests**

Append to `crates/right-agent/src/openshell_tests.rs`:

```rust
#[test]
fn control_master_socket_path_uses_sandbox_name() {
    use std::path::Path;
    let dir = Path::new("/tmp/foo/run/ssh");
    assert_eq!(
        control_master_socket_path(dir, "rightclaw-brain-20260415-1430"),
        Path::new("/tmp/foo/run/ssh/rightclaw-brain-20260415-1430.cm"),
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p right-agent --lib control_master_socket_path_uses_sandbox_name`
Expected: FAIL with "cannot find function `control_master_socket_path` in this scope".

- [ ] **Step 3: Add the helper**

In `crates/right-agent/src/openshell.rs`, after `ssh_host_for_sandbox`:

```rust
/// Filesystem path to the OpenSSH ControlMaster socket for a sandbox.
///
/// Keyed on sandbox name so each sandbox gets its own master. Sandbox
/// migration produces a new sandbox name → new ControlPath → no stale-master
/// reuse risk. See spec for rationale (OpenSSH matches multiplex candidates
/// on `(user, host, port)` and the host alias is per-sandbox).
pub fn control_master_socket_path(ssh_config_dir: &Path, sandbox_name: &str) -> PathBuf {
    ssh_config_dir.join(format!("{sandbox_name}.cm"))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p right-agent --lib control_master_socket_path_uses_sandbox_name`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/openshell.rs crates/right-agent/src/openshell_tests.rs
git commit -m "feat(openshell): add control_master_socket_path helper"
```

---

## Task 2: Append ControlMaster directives in `generate_ssh_config`

**Files:**
- Modify: `crates/right-agent/src/openshell.rs` lines 411-442 (`generate_ssh_config`).
- Modify: `crates/right-agent/src/openshell_tests.rs`.

- [ ] **Step 1: Write failing unit test**

Append to `crates/right-agent/src/openshell_tests.rs`:

```rust
#[test]
fn generate_ssh_config_appends_control_master_directives() {
    // Pure-content check: verify the appended block has the expected
    // shape without spawning the openshell CLI. Calls the helper that
    // builds the appended snippet.
    let dir = std::path::Path::new("/var/lib/right/run/ssh");
    let block = control_master_directives(dir, "rightclaw-brain-20260415");
    assert!(block.contains("\nControlMaster auto\n"), "missing ControlMaster auto: {block}");
    assert!(
        block.contains(
            "\nControlPath /var/lib/right/run/ssh/rightclaw-brain-20260415.cm\n"
        ),
        "missing ControlPath: {block}",
    );
    assert!(block.contains("\nControlPersist yes\n"), "missing ControlPersist: {block}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p right-agent --lib generate_ssh_config_appends_control_master_directives`
Expected: FAIL — `control_master_directives` is undefined.

- [ ] **Step 3: Add the directive-builder helper, then call it from `generate_ssh_config`**

In `crates/right-agent/src/openshell.rs`, add (right after `control_master_socket_path` from Task 1):

```rust
/// Build the ControlMaster directive block to append to an SSH config file.
///
/// Uses the resolved absolute socket path (no `~` expansion ambiguity) so the
/// directive behaves identically regardless of the bot's `HOME` env var.
pub fn control_master_directives(ssh_config_dir: &Path, sandbox_name: &str) -> String {
    let socket = control_master_socket_path(ssh_config_dir, sandbox_name);
    format!(
        "\n# Connection multiplexing — see docs/superpowers/specs/2026-05-05-ssh-controlmaster-design.md\nControlMaster auto\nControlPath {}\nControlPersist yes\n",
        socket.display()
    )
}
```

Then modify `generate_ssh_config` (lines 411-442). Replace the block starting at line 435 (`let dest = config_dir.join(...)` through the `tokio::fs::write` call) with:

```rust
    let dest = config_dir.join(format!("{name}.ssh-config"));
    let mut content = output.stdout;
    let directives = control_master_directives(config_dir, name);
    content.extend_from_slice(directives.as_bytes());
    tokio::fs::write(&dest, &content)
        .await
        .map_err(|e| miette::miette!("failed to write ssh-config to {}: {e:#}", dest.display()))?;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p right-agent --lib generate_ssh_config_appends_control_master_directives`
Expected: PASS.

- [ ] **Step 5: Run full openshell tests to confirm no regression**

Run: `cargo test -p right-agent --lib openshell`
Expected: All existing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/openshell.rs crates/right-agent/src/openshell_tests.rs
git commit -m "feat(openshell): append ControlMaster directives to generated SSH config"
```

---

## Task 3: Add `check_control_master` helper

**Files:**
- Modify: `crates/right-agent/src/openshell.rs`.

This is a thin wrapper around `ssh -F <config> -O check <host>`. Returns `bool` — true if a master is alive at the configured ControlPath, false otherwise. Used to decide whether to clean up a stale socket at startup. No unit test (it shells out to `ssh`); covered by the integration test in Task 8.

- [ ] **Step 1: Add the helper**

In `crates/right-agent/src/openshell.rs`, after the `ssh_exec` function (which ends around line 510), add:

```rust
/// Check whether an OpenSSH ControlMaster is alive at the ControlPath
/// specified in the given SSH config.
///
/// Returns `true` iff `ssh -F <config> -O check <host>` exits 0. Returns
/// `false` for any other exit, spawn failure, or timeout. This is a probe,
/// not an error path — the caller decides what to do with the answer.
pub async fn check_control_master(config_path: &Path, host: &str) -> bool {
    let mut command = Command::new("ssh");
    command.arg("-F").arg(config_path);
    command.args(["-O", "check"]);
    command.arg(host);
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());

    let child = match crate::process_group::ProcessGroupChild::spawn(command) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(host, error = %e, "ssh -O check spawn failed");
            return false;
        }
    };

    let output = match tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            tracing::debug!(host, error = %e, "ssh -O check wait failed");
            return false;
        }
        Err(_) => {
            tracing::debug!(host, "ssh -O check timed out after 5s");
            return false;
        }
    };

    output.status.success()
}
```

- [ ] **Step 2: Confirm it compiles**

Run: `cargo build -p right-agent`
Expected: Builds without warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/right-agent/src/openshell.rs
git commit -m "feat(openshell): add check_control_master helper"
```

---

## Task 4: Add `clean_stale_control_master` and `tear_down_control_master`

**Files:**
- Modify: `crates/right-agent/src/openshell.rs`.

Two helpers with overlapping mechanics but different intent:

- `clean_stale_control_master`: called at bot startup. If a socket file exists, run `ssh -O check`; if check fails, send `ssh -O exit` (best-effort, in case the master is unreachable but still alive elsewhere) and `rm -f` the socket. If check succeeds, leave the alive master alone. Logs decisions.

- `tear_down_control_master`: called on shutdown / migration. Unconditional: `ssh -O exit` (best-effort) then `rm -f` the socket. Always logs at info.

- [ ] **Step 1: Add the two helpers**

In `crates/right-agent/src/openshell.rs`, after `check_control_master`:

```rust
/// At bot startup, clean up a stale ControlMaster socket left behind by
/// a previous SIGKILL'd bot process.
///
/// If the socket file does not exist: no-op.
/// If the socket exists and a master is alive (`ssh -O check` succeeds):
///   leave it alone — the new bot will reuse the existing master.
/// If the socket exists but no master is alive:
///   best-effort `ssh -O exit` (in case the master is reachable from
///   somewhere we don't see), then `rm -f` the socket so the next ssh
///   call can establish a fresh master without `bind: Address already in use`.
pub async fn clean_stale_control_master(
    config_path: &Path,
    host: &str,
    socket_path: &Path,
) -> miette::Result<()> {
    if !socket_path.exists() {
        tracing::debug!(socket = %socket_path.display(), "no stale control-master socket");
        return Ok(());
    }

    if check_control_master(config_path, host).await {
        tracing::info!(
            socket = %socket_path.display(),
            "control-master from previous bot is alive, will reuse",
        );
        return Ok(());
    }

    tracing::info!(
        socket = %socket_path.display(),
        "stale control-master socket found, cleaning up",
    );

    // Best-effort exit; ignore failure (master is dead anyway).
    let mut exit_cmd = Command::new("ssh");
    exit_cmd.arg("-F").arg(config_path);
    exit_cmd.args(["-O", "exit"]);
    exit_cmd.arg(host);
    exit_cmd.stdout(Stdio::null());
    exit_cmd.stderr(Stdio::null());
    if let Ok(child) = crate::process_group::ProcessGroupChild::spawn(exit_cmd) {
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await;
    }

    if let Err(e) = std::fs::remove_file(socket_path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(miette::miette!(
                "failed to remove stale control-master socket {}: {e:#}",
                socket_path.display(),
            ));
        }
    }

    Ok(())
}

/// Tear down a ControlMaster: send `ssh -O exit` and remove the socket file.
///
/// Best-effort and non-fatal — used during graceful shutdown and sandbox
/// migration. Logs at info; never returns an error.
pub async fn tear_down_control_master(config_path: &Path, host: &str, socket_path: &Path) {
    let mut exit_cmd = Command::new("ssh");
    exit_cmd.arg("-F").arg(config_path);
    exit_cmd.args(["-O", "exit"]);
    exit_cmd.arg(host);
    exit_cmd.stdout(Stdio::null());
    exit_cmd.stderr(Stdio::null());

    match crate::process_group::ProcessGroupChild::spawn(exit_cmd) {
        Ok(child) => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await;
        }
        Err(e) => {
            tracing::warn!(host, error = %e, "ssh -O exit spawn failed during teardown");
        }
    }

    match std::fs::remove_file(socket_path) {
        Ok(_) => {
            tracing::info!(socket = %socket_path.display(), "control-master socket removed");
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(socket = %socket_path.display(), "control-master socket already gone");
        }
        Err(e) => {
            tracing::warn!(
                socket = %socket_path.display(),
                error = %e,
                "failed to remove control-master socket",
            );
        }
    }
}
```

- [ ] **Step 2: Confirm it compiles**

Run: `cargo build -p right-agent`
Expected: Builds without warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/right-agent/src/openshell.rs
git commit -m "feat(openshell): add clean_stale_control_master and tear_down_control_master"
```

---

## Task 5: Wire stale-socket cleanup into bot startup

**Files:**
- Modify: `crates/bot/src/lib.rs` (in the sandbox-startup region around line 668-693).

After `generate_ssh_config` returns (line 673) but before the existing `ssh_exec` for inbox/outbox (lines 685-692), call `clean_stale_control_master`. The existing `ssh_exec` then implicitly establishes a fresh master because it's the first call against the (cleaned) ControlPath.

- [ ] **Step 1: Add the cleanup call**

In `crates/bot/src/lib.rs`, modify the section starting at line 668. Replace this block (current lines 668-693):

```rust
        // Generate SSH config.
        let ssh_config_dir = home.join("run").join("ssh");
        std::fs::create_dir_all(&ssh_config_dir)
            .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
        let config_path =
            right_agent::openshell::generate_ssh_config(&sandbox, &ssh_config_dir).await?;
        tracing::info!(agent = %args.agent, "OpenShell sandbox ready");

        (Some(config_path), Some((mtls_dir, sandbox_id)))
    } else {
        (None, None)
    };

    // Create inbox/outbox inside sandbox for attachment handling
    if is_sandboxed && let Some(ref cfg_path) = ssh_config_path {
        let ssh_host =
            right_agent::openshell::ssh_host_for_sandbox(resolved_sandbox.as_deref().unwrap());
        right_agent::openshell::ssh_exec(
            cfg_path,
            &ssh_host,
            &["mkdir", "-p", "/sandbox/inbox", "/sandbox/outbox"],
            10,
        )
        .await
        .map_err(|e| miette::miette!("failed to create sandbox attachment dirs: {e:#}"))?;
    }
```

with:

```rust
        // Generate SSH config.
        let ssh_config_dir = home.join("run").join("ssh");
        std::fs::create_dir_all(&ssh_config_dir)
            .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
        let config_path =
            right_agent::openshell::generate_ssh_config(&sandbox, &ssh_config_dir).await?;

        // Clean up stale ControlMaster socket from a SIGKILL'd previous bot.
        // The next ssh call (inbox/outbox mkdir below) implicitly establishes
        // a fresh master via ControlMaster=auto in the config we just wrote.
        let cm_socket =
            right_agent::openshell::control_master_socket_path(&ssh_config_dir, &sandbox);
        let cm_host = right_agent::openshell::ssh_host_for_sandbox(&sandbox);
        right_agent::openshell::clean_stale_control_master(&config_path, &cm_host, &cm_socket)
            .await?;

        tracing::info!(agent = %args.agent, "OpenShell sandbox ready");

        (Some(config_path), Some((mtls_dir, sandbox_id)))
    } else {
        (None, None)
    };

    // Create inbox/outbox inside sandbox for attachment handling.
    // This is also the first ssh -F <config> call, which establishes the
    // ControlMaster (see clean_stale_control_master above + SSH config
    // appended directives in generate_ssh_config).
    if is_sandboxed && let Some(ref cfg_path) = ssh_config_path {
        let ssh_host =
            right_agent::openshell::ssh_host_for_sandbox(resolved_sandbox.as_deref().unwrap());
        right_agent::openshell::ssh_exec(
            cfg_path,
            &ssh_host,
            &["mkdir", "-p", "/sandbox/inbox", "/sandbox/outbox"],
            10,
        )
        .await
        .map_err(|e| miette::miette!("failed to create sandbox attachment dirs: {e:#}"))?;
    }
```

- [ ] **Step 2: Confirm bot crate compiles**

Run: `cargo build -p right-bot`
Expected: Builds without warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "feat(bot): clean stale ControlMaster socket at startup"
```

---

## Task 6: Wire shutdown teardown into bot graceful-shutdown path

**Files:**
- Modify: `crates/bot/src/lib.rs` (the shutdown region around lines 865-884).

After the dispatcher returns and the cron / sync / keepalive / upgrade tasks have been awaited, but before logging "graceful shutdown complete", call `tear_down_control_master` so we don't leak a master process across a clean bot restart.

- [ ] **Step 1: Add the teardown call**

In `crates/bot/src/lib.rs`, locate the shutdown block (around line 867):

```rust
    // Signal cron/sync tasks to stop. The teloxide dispatcher handles SIGTERM
    // internally but doesn't cancel this token, so we must do it here.
    shutdown.cancel();

    tracing::info!("waiting for cron to finish");
    let _ = cron_handle.await;
    tracing::info!("waiting for cron delivery to finish");
    let _ = delivery_handle.await;
    if let Some(handle) = sync_handle {
        tracing::info!("waiting for sync to finish");
        let _ = handle.await;
    }
    // Await keepalive/upgrade so their in-flight Interval::tick() futures
    // resolve before the tokio runtime is dropped. Without this, the runtime
    // drop panics: "A Tokio 1.x context was found, but it is being shutdown."
    let _ = keepalive_handle.await;
    if let Some(handle) = upgrade_handle {
        let _ = handle.await;
    }
    tracing::info!("graceful shutdown complete");
```

Insert the teardown right before `tracing::info!("graceful shutdown complete");`. The `ssh_config_path` and `resolved_sandbox` bindings are already in scope but `ssh_config_path` was moved into `telegram::run_telegram` (line 848). We need the path before that move. Capture a clone for shutdown use just after `ssh_config_path` is initialized.

First, modify the binding section (around line 678, right after the if/else that produces `ssh_config_path`). Add:

```rust
    // Snapshot for shutdown teardown — the original is moved into run_telegram below.
    let shutdown_ssh_config = ssh_config_path.clone();
    let shutdown_sandbox = resolved_sandbox.clone();
```

Then, in the shutdown block, replace:

```rust
    let _ = keepalive_handle.await;
    if let Some(handle) = upgrade_handle {
        let _ = handle.await;
    }
    tracing::info!("graceful shutdown complete");
```

with:

```rust
    let _ = keepalive_handle.await;
    if let Some(handle) = upgrade_handle {
        let _ = handle.await;
    }

    // Tear down the OpenSSH ControlMaster so we don't leak the master ssh
    // process across bot restarts. Best-effort.
    if let (Some(cfg_path), Some(sandbox_name)) = (shutdown_ssh_config, shutdown_sandbox) {
        let ssh_config_dir = home.join("run").join("ssh");
        let socket =
            right_agent::openshell::control_master_socket_path(&ssh_config_dir, &sandbox_name);
        let host = right_agent::openshell::ssh_host_for_sandbox(&sandbox_name);
        right_agent::openshell::tear_down_control_master(&cfg_path, &host, &socket).await;
    }

    tracing::info!("graceful shutdown complete");
```

- [ ] **Step 2: Confirm bot crate compiles**

Run: `cargo build -p right-bot`
Expected: Builds without warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "feat(bot): tear down ControlMaster on graceful shutdown"
```

---

## Task 7: Wire migration teardown into `perform_migration`

**Files:**
- Modify: `crates/right/src/main.rs` (`perform_migration` around lines 4709-4715).

After the new sandbox is in place and `agent.yaml` is updated, but before deleting the old SSH config file, tear down the old ControlMaster.

- [ ] **Step 1: Add teardown before old-config deletion**

In `crates/right/src/main.rs`, locate this block (around line 4709-4715):

```rust
    // Delete old sandbox (best-effort).
    println!("  Deleting old sandbox '{old_sandbox}'...");
    right_agent::openshell::delete_sandbox(old_sandbox).await;
    let _ = right_agent::openshell::wait_for_deleted(&mut grpc, old_sandbox, 60, 2).await;

    // Remove old SSH config (best-effort).
    let _ = std::fs::remove_file(&old_ssh_config);
```

Replace with:

```rust
    // Tear down the old sandbox's ControlMaster before we remove its config.
    // Best-effort — the master may already be dead if the bot exited cleanly.
    let old_socket = right_agent::openshell::control_master_socket_path(
        &home.join("run").join("ssh"),
        old_sandbox,
    );
    right_agent::openshell::tear_down_control_master(
        &old_ssh_config,
        &old_ssh_host,
        &old_socket,
    )
    .await;

    // Delete old sandbox (best-effort).
    println!("  Deleting old sandbox '{old_sandbox}'...");
    right_agent::openshell::delete_sandbox(old_sandbox).await;
    let _ = right_agent::openshell::wait_for_deleted(&mut grpc, old_sandbox, 60, 2).await;

    // Remove old SSH config (best-effort).
    let _ = std::fs::remove_file(&old_ssh_config);
```

- [ ] **Step 2: Confirm right crate compiles**

Run: `cargo build -p right`
Expected: Builds without warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(migrate): tear down old ControlMaster during sandbox migration"
```

---

## Task 8: Integration test — multiplex actually engages

**Files:**
- Create: `crates/right-agent/tests/control_master.rs`.

This test creates a real OpenShell sandbox via `TestSandbox`, invokes `generate_ssh_config` against it, runs two back-to-back `ssh_exec` calls, and verifies that after the first call the ControlMaster socket exists and `check_control_master` returns true.

- [ ] **Step 1: Write the failing test**

The `test-support` feature is already wired up
(`crates/right-agent/Cargo.toml` exports `test-support = []` and the
self dev-dep re-imports the crate with that feature). No Cargo.toml
changes needed.

Create `crates/right-agent/tests/control_master.rs`:

```rust
//! Integration test: ControlMaster directives in `generate_ssh_config`
//! actually engage multiplexing on a real sandbox.

#![cfg(unix)]

use right_agent::openshell;
use right_agent::test_support::TestSandbox;

#[tokio::test]
async fn control_master_engages_after_first_ssh_call() {
    let _slot = openshell::acquire_sandbox_slot();
    let sandbox = TestSandbox::create("controlmaster").await;

    let tmp = tempfile::tempdir().unwrap();
    let ssh_config_dir = tmp.path().to_path_buf();

    // Generate ssh-config with ControlMaster directives appended.
    let config_path = openshell::generate_ssh_config(sandbox.name(), &ssh_config_dir)
        .await
        .expect("generate_ssh_config");

    let host = openshell::ssh_host_for_sandbox(sandbox.name());
    let socket = openshell::control_master_socket_path(&ssh_config_dir, sandbox.name());

    // Pre-condition: no master yet.
    assert!(
        !socket.exists(),
        "control-master socket should not exist before any ssh call: {}",
        socket.display(),
    );
    assert!(
        !openshell::check_control_master(&config_path, &host).await,
        "check_control_master should be false before any ssh call",
    );

    // First ssh call — should establish the master.
    let out = openshell::ssh_exec(&config_path, &host, &["echo", "hello"], 10)
        .await
        .expect("first ssh_exec");
    assert!(out.contains("hello"), "unexpected stdout: {out:?}");

    // Master should now be alive.
    assert!(
        socket.exists(),
        "control-master socket should exist after first ssh call: {}",
        socket.display(),
    );
    assert!(
        openshell::check_control_master(&config_path, &host).await,
        "check_control_master should be true after first ssh call",
    );

    // Tear down — socket should be gone afterward.
    openshell::tear_down_control_master(&config_path, &host, &socket).await;
    assert!(
        !socket.exists(),
        "control-master socket should be gone after tear_down: {}",
        socket.display(),
    );
    assert!(
        !openshell::check_control_master(&config_path, &host).await,
        "check_control_master should be false after tear_down",
    );
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p right-agent --test control_master`
Expected: PASS (all of Tasks 1-4 are already merged at this point). If FAIL at the assertions, debug — the most likely cause is OpenSSH client config interference. Common fixes:
- Confirm `~/.ssh/config` does not override `ControlPath` for hosts matching the OpenShell pattern (typically `openshell-*`).
- Confirm the OpenShell-issued config sets `User`, `IdentityFile`, and `ProxyCommand` correctly (`openshell sandbox ssh-config <name>` output).

- [ ] **Step 3: Verify the openshell unit-test suite still passes**

Run: `cargo test -p right-agent`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/tests/control_master.rs
git commit -m "test(openshell): verify ControlMaster engages multiplexing on first ssh call"
```

---

## Task 9: Final workspace verification

**Files:** none (verification only).

- [ ] **Step 1: Workspace build**

Run: `cargo build --workspace`
Expected: Builds clean, no warnings.

- [ ] **Step 2: Workspace clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: Clean.

- [ ] **Step 3: Workspace tests**

Run: `cargo test --workspace`
Expected: All pass. The integration test from Task 8 will create a temporary OpenShell sandbox; this requires OpenShell running on the dev machine (per project convention; never `#[ignore]`).

- [ ] **Step 4: Manual smoke test (recorded but not gated)**

Document the following observations in the PR description:

```bash
# Start a real bot under process-compose
right up

# Once the bot is up, verify the master is alive:
ssh -F ~/.right/run/ssh/<sandbox-name>.ssh-config -O check openshell-<sandbox-name>
# Expected: "Master running (pid=NNNN)"

# Verify the socket file exists where expected:
ls ~/.right/run/ssh/<sandbox-name>.cm
# Expected: socket file present.

# Send a Telegram message to one of the agents. The CC invocation in
# worker.rs reuses the existing master — visible in the bot's strace
# (no new TCP handshake) or by tailing ~/.right/logs/<agent>.log
# (no new SSH connection lines per message after the first).

# Verify shutdown teardown:
right down
ls ~/.right/run/ssh/*.cm 2>/dev/null
# Expected: empty (no .cm files).
```

- [ ] **Step 5: Final commit (only if anything was tweaked during verification)**

```bash
git status
# If nothing to commit, skip. Otherwise:
git add -p
git commit -m "chore: address verification feedback"
```

---

## Notes for the Executor

**Why no separate eager-establish step.** The existing `ssh_exec` for `mkdir /sandbox/inbox /sandbox/outbox` (in `crates/bot/src/lib.rs` immediately after `generate_ssh_config`) is the bot's first `ssh -F <config>` call after writing the config. With `ControlMaster auto` in the config, that call automatically establishes the master. Adding a separate `ssh -- /bin/true` would be a no-op redundant call. We rely on the existing flow.

**Why `ControlPath` is keyed on sandbox name, not agent name.** OpenSSH multiplex matches `(user, host, port)`. The host alias from `ssh_host_for_sandbox` is `openshell-<sandbox-name>`, which changes whenever the sandbox is recreated by a filesystem-policy migration. Per-agent `ControlPath` would bind a master to the old host alias, then the next ssh's config (with the new alias) would refuse to reuse it. Per-sandbox naming matches the granularity OpenSSH already enforces. See spec Section "Why `ControlPath` Is Keyed on Sandbox Name".

**Why `ControlPersist yes` (no idle timeout).** The bot is the long-running owner. A numeric idle timeout creates a "first message after a quiet hour pays full handshake again" failure mode that defeats the goal.

**What is explicitly out of scope.**
- Active health-check ticker. Existing per-call retry layer in `worker.rs` handles a dead master adequately on the next message.
- Master sharing across in-place bot upgrades. Stale-socket recovery from Task 5 handles the SIGKILL'd-previous-bot case.
- `right` CLI `attach` command (interactive TTY). Leave on direct `ssh`.
- gRPC `exec_in_sandbox` (admin commands). Already a separate path; not affected.

**Forward compatibility.** If a future need outgrows multiplexing (per-channel telemetry, in-process session ownership), swapping to a russh-based session is straightforward: replace the four helpers in `openshell.rs` with russh equivalents and gate the per-site `ssh` subprocess calls behind a helper that either spawns `ssh` or opens a russh channel. Nothing in this design forecloses that path.
