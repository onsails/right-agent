# OpenShell Test-Process Leaks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate orphaned `openshell sandbox create`, `openshell sandbox upload`, `ssh`, and `openshell ssh-proxy` processes left behind by tests (happy path + panic) and by the production bot (happy path).

**Architecture:** Introduce a `ProcessGroupChild` wrapper (tokio `Child` + `process_group(0)` + `killpg(SIGKILL)` on Drop) to replace all `tokio::process::Command` spawns that have multi-generation descendants (ssh→ssh-proxy, openshell→ssh→ssh-proxy). Because the workspace sets `panic = "abort"` (deciders skip unwinding), add a global registry + `std::panic::set_hook` fallback so test sandboxes are still deleted on panic. Add a narrow `pkill_test_orphans` safety net for SIGKILL/external-abort scenarios where neither Drop nor hook fires.

**Tech Stack:** Rust 2024, tokio 1.50, `nix = "0.31"` (new — for safe `killpg`), `std::panic::set_hook`, `std::process::Command` (sync, for panic-safe cleanup).

**Design doc:** `docs/superpowers/specs/2026-04-17-openshell-test-leaks-design.md`
**Diagnosis doc:** `docs/superpowers/specs/2026-04-17-openshell-test-leaks.md`

---

## Ground rules

- Each task ends with `cargo build --workspace` passing. Clippy (`cargo clippy --workspace --all-targets -- -D warnings`) runs at the end of each major task.
- Never remove `kill_on_drop(true)` on a Command without also switching to `ProcessGroupChild::spawn` — the group kill on Drop subsumes per-PID kill_on_drop.
- Never touch processes that aren't ours. `pkill_test_orphans` takes a *specific* sandbox name; it never matches broad patterns like `openshell` alone.
- Commit after each task. Prefer a few commits over one giant.

---

### Task 1: Add `nix` dependency and `ProcessGroupChild` wrapper

**Files:**
- Modify: `Cargo.toml` (workspace deps)
- Modify: `crates/rightclaw/Cargo.toml`
- Create: `crates/rightclaw/src/process_group.rs`
- Modify: `crates/rightclaw/src/lib.rs` (expose module)

- [ ] **Step 1: Add `nix` to workspace dependencies**

Edit `Cargo.toml` (workspace root). Under `[workspace.dependencies]`, add:

```toml
nix = { version = "0.31", default-features = false, features = ["signal"] }
```

Place it near `fs4 = "0.13"` alphabetically.

- [ ] **Step 2: Add `nix` to rightclaw crate dependencies**

Edit `crates/rightclaw/Cargo.toml`. Under `[dependencies]`, add:

```toml
nix = { workspace = true }
```

Place it near `fs4 = { workspace = true }` alphabetically.

- [ ] **Step 3: Create the wrapper module**

Create `crates/rightclaw/src/process_group.rs` with this exact content:

```rust
//! `ProcessGroupChild` — a newtype around `tokio::process::Child` that
//! spawns the child in a new process group and kills the entire group on
//! Drop via `killpg(SIGKILL)`.
//!
//! Rationale: tokio's `kill_on_drop(true)` only SIGKILLs the direct child.
//! When the child is `ssh` (which spawns `ProxyCommand`) or `openshell
//! sandbox upload` (which spawns `ssh` which spawns `ssh-proxy`), those
//! grandchildren are reparented to launchd/init and survive indefinitely.
//! Putting the child into its own process group lets us atomically reap
//! the whole tree with one `killpg` syscall.

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::process::{Child, Command};

/// A child process handle that kills its entire process group on Drop.
pub struct ProcessGroupChild {
    inner: Child,
    pgid: Option<i32>,
}

impl ProcessGroupChild {
    /// Spawn `cmd` as the leader of a new process group. The returned
    /// handle kills the entire group on Drop via `killpg(SIGKILL)`.
    pub fn spawn(mut cmd: Command) -> std::io::Result<Self> {
        cmd.process_group(0);
        let inner = cmd.spawn()?;
        let pgid = inner.id().map(|p| p as i32);
        Ok(Self { inner, pgid })
    }

    pub fn id(&self) -> Option<u32> {
        self.inner.id()
    }

    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.inner.wait().await
    }

    pub async fn wait_with_output(self) -> std::io::Result<std::process::Output> {
        self.inner.wait_with_output().await
    }

    pub async fn kill(&mut self) -> std::io::Result<()> {
        self.inner.kill().await
    }

    pub fn stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.inner.stdout.take()
    }

    pub fn stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        self.inner.stderr.take()
    }

    pub fn stdin(&mut self) -> Option<tokio::process::ChildStdin> {
        self.inner.stdin.take()
    }
}

impl Drop for ProcessGroupChild {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid {
            // Best-effort. ESRCH (group already gone) is fine to ignore.
            let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
        }
        // tokio's `Child::Drop` schedules a non-blocking `waitpid` via its
        // internal reaper; the leader zombie is reaped asynchronously.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Given a bash parent that spawns a `sleep 600` grandchild, dropping
    /// the `ProcessGroupChild` must kill both within ~200ms.
    #[tokio::test(flavor = "multi_thread")]
    async fn drop_kills_grandchild() {
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(
            "sleep 600 & \
             echo $! > /tmp/rightclaw_pgtest_grandchild.pid; \
             wait",
        );
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        let child = ProcessGroupChild::spawn(cmd).expect("spawn");
        let parent_pid = child.id().expect("pid");

        // Give bash time to spawn the sleep and write the pid file.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let grandchild_pid: i32 = std::fs::read_to_string("/tmp/rightclaw_pgtest_grandchild.pid")
            .expect("grandchild pid file")
            .trim()
            .parse()
            .expect("parse pid");
        std::fs::remove_file("/tmp/rightclaw_pgtest_grandchild.pid").ok();

        // Both alive before drop.
        assert!(is_alive(parent_pid as i32), "parent should be alive before drop");
        assert!(is_alive(grandchild_pid), "grandchild should be alive before drop");

        drop(child);

        // Give the signal time to propagate.
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!is_alive(parent_pid as i32), "parent must be dead after drop");
        assert!(!is_alive(grandchild_pid), "grandchild must be dead after drop");
    }

    /// Sanity check: without `process_group(0)`, a plain `Child` drop with
    /// `kill_on_drop(true)` kills the direct child but leaves the grandchild
    /// alive. This is the bug ProcessGroupChild exists to fix.
    #[tokio::test(flavor = "multi_thread")]
    async fn control_without_group_leaks_grandchild() {
        let mut cmd = Command::new("bash");
        cmd.kill_on_drop(true);
        cmd.arg("-c").arg(
            "sleep 600 & \
             echo $! > /tmp/rightclaw_pgtest_control_grandchild.pid; \
             wait",
        );
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        let child = cmd.spawn().expect("spawn");
        let parent_pid = child.id().expect("pid") as i32;

        tokio::time::sleep(Duration::from_millis(200)).await;
        let grandchild_pid: i32 =
            std::fs::read_to_string("/tmp/rightclaw_pgtest_control_grandchild.pid")
                .expect("grandchild pid file")
                .trim()
                .parse()
                .expect("parse pid");
        std::fs::remove_file("/tmp/rightclaw_pgtest_control_grandchild.pid").ok();

        assert!(is_alive(parent_pid), "parent alive before drop");
        assert!(is_alive(grandchild_pid), "grandchild alive before drop");

        drop(child);
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!is_alive(parent_pid), "parent killed by kill_on_drop");
        assert!(
            is_alive(grandchild_pid),
            "control: grandchild must survive without process_group(0)"
        );

        // Cleanup the leaked grandchild so the test doesn't itself leak.
        unsafe {
            libc::kill(grandchild_pid, libc::SIGKILL);
        }
    }

    fn is_alive(pid: i32) -> bool {
        // `kill(pid, 0)` returns 0 if the process exists, -1 with ESRCH if not.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0
    }
}
```

**Note on the control test's `unsafe { libc::kill }`:** this is only in the test to reap the intentionally-leaked grandchild. The production `ProcessGroupChild::Drop` uses `nix::killpg` (safe).

- [ ] **Step 4: Add `libc` dev-dep for the control test**

The control test uses `libc::kill`. `libc` is already transitively present via tokio, but make it explicit as a dev-dep.

Edit `crates/rightclaw/Cargo.toml`. Under `[dev-dependencies]`, add:

```toml
libc = "0.2"
```

- [ ] **Step 5: Expose the module**

Edit `crates/rightclaw/src/lib.rs`. Find the list of `pub mod` declarations. Add:

```rust
pub mod process_group;
```

Alphabetical order among `pub mod` lines.

- [ ] **Step 6: Run the wrapper tests**

Run: `cargo test -p rightclaw --lib process_group::tests`
Expected: both tests pass. If `control_without_group_leaks_grandchild` fails (says grandchild died without group), then the host's shell/job control is doing something unexpected — investigate before continuing.

- [ ] **Step 7: Build workspace + clippy**

Run: `cargo build --workspace`
Expected: no errors.

Run: `cargo clippy -p rightclaw --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/rightclaw/Cargo.toml crates/rightclaw/src/process_group.rs crates/rightclaw/src/lib.rs
git commit -m "feat(openshell): add ProcessGroupChild wrapper for multi-gen process cleanup

Spawns children with process_group(0), kills the whole group via nix::killpg
on Drop. Solves the ssh→ssh-proxy and openshell→ssh→ssh-proxy orphan tree
on panic/cancellation. Unit test verifies grandchild dies; control test
confirms the bug exists without the wrapper."
```

---

### Task 2: Convert `spawn_sandbox` to return `ProcessGroupChild`

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs`
- Modify: `crates/rightclaw-cli/src/main.rs`
- Modify: `crates/rightclaw-cli/tests/cli_integration.rs`
- Modify: `crates/rightclaw/src/openshell_tests.rs`
- Modify: `crates/bot/src/sync.rs`
- Modify: `crates/rightclaw-cli/src/right_backend_tests.rs`

- [ ] **Step 1: Change `spawn_sandbox` return type**

Edit `crates/rightclaw/src/openshell.rs`. Find `pub fn spawn_sandbox` at line ~260.

Replace the entire function (currently lines 256-285) with:

```rust
/// Spawn an OpenShell sandbox. Returns a [`ProcessGroupChild`] handle.
///
/// On Drop, the openshell CLI process and all its descendants (ssh,
/// ssh-proxy, internal k3s spawns) are killed via `killpg(SIGKILL)`.
/// Callers that need the sandbox to outlive the Rust process must
/// `std::mem::forget` the returned handle (we currently have no such
/// callers — the CLI process does nothing useful after READY, and the
/// sandbox itself lives in k3s state, not in the CLI process).
pub fn spawn_sandbox(
    name: &str,
    policy_path: &Path,
    upload_dir: Option<&Path>,
) -> miette::Result<crate::process_group::ProcessGroupChild> {
    let mut cmd = Command::new("openshell");
    cmd.args(["sandbox", "create", "--name", name, "--policy"]);
    cmd.arg(policy_path);
    cmd.arg("--no-tty");

    if let Some(dir) = upload_dir {
        cmd.arg("--upload");
        cmd.arg(dir);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let child = crate::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| miette::miette!("failed to spawn openshell sandbox create: {e:#}"))?;

    tracing::info!(sandbox = name, "spawned sandbox create process");
    Ok(child)
}
```

Note: the `kill_on_drop(false)` line is removed entirely. Drop-on-scope-exit (including the `drop(child)` in `ensure_sandbox`) now kills the group.

- [ ] **Step 2: Update `ensure_sandbox` (the prod caller)**

Same file, around line 825. The existing code is:

```rust
    let mut child = spawn_sandbox(&sandbox, policy_path, staging_dir)?;

    tokio::select! {
        result = wait_for_ready(&mut grpc_client, &sandbox, 120, 2) => {
            result?;
            drop(child);
        }
        status = child.wait() => {
            ...
        }
    }
```

No change required — `child.wait()` and `drop(child)` still work on `ProcessGroupChild`. Confirm by re-reading the block. If the type inference chokes because `tokio::select!` requires `Unpin` on the `wait()` future, adjust by pinning (should not be needed).

- [ ] **Step 3: Update CLI main.rs callers**

Edit `crates/rightclaw-cli/src/main.rs`. Two callsites:
- Line ~2154 (migration flow)
- Line ~3573 (restore flow)

Both follow the same pattern:

```rust
    let mut child = rightclaw::openshell::spawn_sandbox(...)?;

    tokio::select! {
        result = ... => {
            result?;
            drop(child);
        }
        status = child.wait() => { ... }
    }
```

No change required — same methods are forwarded. Re-read both to confirm they compile against the new signature.

- [ ] **Step 4: Update `crates/rightclaw-cli/tests/cli_integration.rs`**

Around line 616:

```rust
    let mut child = rightclaw::openshell::spawn_sandbox(sandbox_name, &policy_path, None)
        .expect("failed to spawn sandbox");
    let ready = rightclaw::openshell::wait_for_ready(&mut client, sandbox_name, 120, 2).await;
    let _ = child.kill().await;
```

`child.kill().await` forwards to inner — no change. Leave as-is.

- [ ] **Step 5: Update `crates/rightclaw/src/openshell_tests.rs::TestSandbox::create`**

Around line 232:

```rust
    let mut child = super::spawn_sandbox(&name, &policy_path, None)
        .expect("failed to spawn sandbox");
    super::wait_for_ready(&mut client, &name, 120, 2)
        .await
        .expect("sandbox did not become READY");

    // Kill the create process — it doesn't exit on its own after READY.
    let _ = child.kill().await;
```

No change required. The explicit `child.kill().await` was there to paper over the previous `kill_on_drop(false)` leak; it still works with the new wrapper and is belt-and-suspenders now (the Drop would kill the group anyway).

- [ ] **Step 6: Fix `crates/bot/src/sync.rs:376` (test that never killed the child)**

Current code:

```rust
        let _child = rightclaw::openshell::spawn_sandbox(sandbox_name, &policy_path, None)
            .expect("failed to spawn sandbox");
        rightclaw::openshell::wait_for_ready(&mut grpc_client, sandbox_name, 120, 2)
            .await
            .expect("sandbox did not become READY");
```

Replace with:

```rust
        let mut child = rightclaw::openshell::spawn_sandbox(sandbox_name, &policy_path, None)
            .expect("failed to spawn sandbox");
        rightclaw::openshell::wait_for_ready(&mut grpc_client, sandbox_name, 120, 2)
            .await
            .expect("sandbox did not become READY");
        let _ = child.kill().await;
```

The explicit kill is clearer than relying on group-kill at scope exit — matches the `TestSandbox::create` pattern and signals intent to readers.

- [ ] **Step 7: Fix `crates/rightclaw-cli/src/right_backend_tests.rs:180` (same bug)**

Same rewrite. Current:

```rust
    let _child = rightclaw::openshell::spawn_sandbox(sandbox_name, &policy_path, None)
        .expect("failed to spawn sandbox");
    rightclaw::openshell::wait_for_ready(&mut grpc_client, sandbox_name, 120, 2)
        .await
        .expect("sandbox did not become READY");
```

Replace:

```rust
    let mut child = rightclaw::openshell::spawn_sandbox(sandbox_name, &policy_path, None)
        .expect("failed to spawn sandbox");
    rightclaw::openshell::wait_for_ready(&mut grpc_client, sandbox_name, 120, 2)
        .await
        .expect("sandbox did not become READY");
    let _ = child.kill().await;
```

- [ ] **Step 8: Build + clippy**

Run: `cargo build --workspace`
Expected: no errors.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/rightclaw/src/openshell.rs \
        crates/rightclaw-cli/src/main.rs \
        crates/rightclaw-cli/tests/cli_integration.rs \
        crates/rightclaw/src/openshell_tests.rs \
        crates/bot/src/sync.rs \
        crates/rightclaw-cli/src/right_backend_tests.rs
git commit -m "fix(openshell): spawn_sandbox now uses ProcessGroupChild

Eliminates orphan 'openshell sandbox create' processes left by every bot
startup (drop(child) in ensure_sandbox) and by tests that forgot to kill
the child after wait_for_ready (sync.rs:376, right_backend_tests.rs:180).
Also fixes those two tests to call child.kill().await explicitly for
readability."
```

---

### Task 3: Convert `upload_single_file`, `delete_sandbox`, and `keepalive::ping_claude`

These are the `.output()/.status().await` callsites where a panic/cancellation of the future currently leaks `openshell → ssh → ssh-proxy`.

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs`
- Modify: `crates/bot/src/keepalive.rs`

- [ ] **Step 1: Convert `upload_single_file`**

Edit `crates/rightclaw/src/openshell.rs`. Current function (line ~568):

```rust
async fn upload_single_file(sandbox: &str, host_path: &Path, sandbox_dir: &str) -> miette::Result<()> {
    let output = Command::new("openshell")
        .args(["sandbox", "upload", sandbox])
        .arg(host_path)
        .arg(sandbox_dir)
        .output()
        .await
        .map_err(|e| miette::miette!("openshell upload failed: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("openshell upload failed: {stderr}"));
    }
    Ok(())
}
```

Replace with:

```rust
async fn upload_single_file(sandbox: &str, host_path: &Path, sandbox_dir: &str) -> miette::Result<()> {
    let mut cmd = Command::new("openshell");
    cmd.args(["sandbox", "upload", sandbox])
        .arg(host_path)
        .arg(sandbox_dir);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let child = crate::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| miette::miette!("failed to spawn openshell upload: {e:#}"))?;

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| miette::miette!("openshell upload failed: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("openshell upload failed: {stderr}"));
    }
    Ok(())
}
```

- [ ] **Step 2: Convert `delete_sandbox`**

Same file, function `delete_sandbox` at line ~651. Current:

```rust
pub async fn delete_sandbox(name: &str) {
    let result = Command::new("openshell")
        .args(["sandbox", "delete", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match result { ... }
}
```

Replace with:

```rust
pub async fn delete_sandbox(name: &str) {
    let mut cmd = Command::new("openshell");
    cmd.args(["sandbox", "delete", name]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let child = match crate::process_group::ProcessGroupChild::spawn(cmd) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(sandbox = name, "failed to spawn openshell delete: {e:#}");
            return;
        }
    };

    let result = child.wait_with_output().await;

    match result {
        Ok(output) if output.status.success() => {
            tracing::info!(sandbox = name, "deleted sandbox");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(
                sandbox = name,
                exit = %output.status,
                %stderr,
                "failed to delete sandbox (best-effort)"
            );
        }
        Err(e) => {
            tracing::warn!(
                sandbox = name,
                "failed to wait for openshell delete: {e:#}"
            );
        }
    }
}
```

- [ ] **Step 3: Convert `ping_claude` in keepalive.rs**

Edit `crates/bot/src/keepalive.rs`. Replace the final `cmd.status().await` block (lines ~100-103) with the spawn+wait pattern.

The function builds `cmd` as either `ssh` or `bash`. After the `cmd.stdin/stdout/stderr` lines (~96-98), replace:

```rust
    let status = cmd
        .status()
        .await
        .map_err(|e| format!("spawn failed: {e}"))?;
```

with:

```rust
    let mut child = rightclaw::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| format!("spawn failed: {e}"))?;
    let status = child
        .wait()
        .await
        .map_err(|e| format!("wait failed: {e}"))?;
```

Note: `rightclaw::process_group::ProcessGroupChild` — the bot crate accesses it through the `rightclaw` crate's public module.

- [ ] **Step 4: Build + clippy**

Run: `cargo build --workspace`
Expected: no errors.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/openshell.rs crates/bot/src/keepalive.rs
git commit -m "fix(openshell): wrap upload/delete/keepalive in ProcessGroupChild

These three async callsites previously used Command::output()/status().await
with no kill_on_drop and no process group. Future drop on panic leaked the
full openshell→ssh→ssh-proxy tree. Group-kill on Drop now reaps atomically."
```

---

### Task 4: Convert SSH helpers in `openshell.rs`

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs`

The three functions `ssh_exec`, `ssh_tar_download`, `ssh_tar_upload` (lines ~397, 438, 495) all spawn `ssh` and can orphan `ssh-proxy` on cancellation.

- [ ] **Step 1: Convert `ssh_exec`**

Current (line ~397-432):

```rust
pub async fn ssh_exec(
    config_path: &Path,
    host: &str,
    cmd: &[&str],
    timeout_secs: u64,
) -> miette::Result<String> {
    let mut command = Command::new("ssh");
    command.arg("-F").arg(config_path);
    command.arg(host);
    command.arg("--");
    command.args(cmd);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let child = command
        .spawn()
        .map_err(|e| miette::miette!("failed to spawn ssh: {e:#}"))?;

    let timeout_dur = Duration::from_secs(timeout_secs);
    let output = tokio::time::timeout(timeout_dur, child.wait_with_output())
        .await
        .map_err(|_| miette::miette!("ssh exec timed out after {timeout_secs}s"))?
        .map_err(|e| miette::miette!("ssh exec failed: {e:#}"))?;
    ...
}
```

Change only the spawn line. Replace:

```rust
    let child = command
        .spawn()
        .map_err(|e| miette::miette!("failed to spawn ssh: {e:#}"))?;
```

with:

```rust
    let child = crate::process_group::ProcessGroupChild::spawn(command)
        .map_err(|e| miette::miette!("failed to spawn ssh: {e:#}"))?;
```

The rest (`child.wait_with_output()`) works unchanged via the forwarded method.

- [ ] **Step 2: Convert `ssh_tar_download`**

Function at line ~438. The spawn line is:

```rust
    let mut child = command
        .spawn()
        .map_err(|e| miette::miette!("failed to spawn ssh for tar download: {e:#}"))?;
```

Replace with:

```rust
    let mut child = crate::process_group::ProcessGroupChild::spawn(command)
        .map_err(|e| miette::miette!("failed to spawn ssh for tar download: {e:#}"))?;
```

The function then does `child.stdout.take()`. That field is now accessed via the `stdout()` method on `ProcessGroupChild`. Replace:

```rust
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| miette::miette!("no stdout handle from ssh tar download"))?;
```

with:

```rust
    let mut stdout = child
        .stdout()
        .ok_or_else(|| miette::miette!("no stdout handle from ssh tar download"))?;
```

`child.wait().await` below works unchanged.

- [ ] **Step 3: Convert `ssh_tar_upload`**

Function at line ~495. Same two changes:

- Spawn → `ProcessGroupChild::spawn(command)`
- `child.stdin.take()` → `child.stdin()`

Replace:

```rust
    let mut child = command
        .spawn()
        .map_err(|e| miette::miette!("failed to spawn ssh for tar upload: {e:#}"))?;
```

with:

```rust
    let mut child = crate::process_group::ProcessGroupChild::spawn(command)
        .map_err(|e| miette::miette!("failed to spawn ssh for tar upload: {e:#}"))?;
```

Replace:

```rust
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| miette::miette!("no stdin handle from ssh tar upload"))?;
```

with:

```rust
    let mut stdin = child
        .stdin()
        .ok_or_else(|| miette::miette!("no stdin handle from ssh tar upload"))?;
```

`child.wait_with_output().await` at the end works unchanged.

- [ ] **Step 4: Build + clippy**

Run: `cargo build --workspace`
Expected: no errors.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/openshell.rs
git commit -m "fix(openshell): wrap ssh_exec/ssh_tar_download/ssh_tar_upload in ProcessGroupChild

Prevents orphan openshell ssh-proxy from surviving when the ssh future is
cancelled by timeout or parent task cancellation."
```

---

### Task 5: Convert SSH / claude callsites in `bot` crate

These spawn either `ssh <host>` (which fan-outs to `openshell ssh-proxy`) or `bash -c <claude ...>` (which fan-outs to claude and its child processes). Both patterns benefit from group cleanup.

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/cron.rs`
- Modify: `crates/bot/src/cron_delivery.rs`

- [ ] **Step 1: Convert `worker.rs` claude invocation**

Edit `crates/bot/src/telegram/worker.rs`. Around line 1014-1030:

```rust
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true); // BOT-04: killed on SIGTERM
    ...
    let mut child = cmd
        .spawn()
        .map_err(|e| format_error_reply(-1, &format!("spawn failed: {:#}", e)))?;
```

Replace the `cmd.kill_on_drop(true);` line with a comment (remove it — group kill on Drop subsumes it):

```rust
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Group kill on Drop (via ProcessGroupChild) subsumes kill_on_drop(true)
    // and also reaps ssh-proxy / bash-spawned claude grandchildren.
```

Replace the spawn with:

```rust
    let mut child = rightclaw::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| format_error_reply(-1, &format!("spawn failed: {:#}", e)))?;
```

The subsequent code does `child.stdin.take()` and `child.stderr.take()` via field access. With `ProcessGroupChild`, these become method calls. Two specific replacements in `worker.rs`:

- Line ~1033: `if let Some(mut stdin) = child.stdin.take() {` → `if let Some(mut stdin) = child.stdin() {`
- Line ~1200: `let stderr_str = if let Some(mut stderr) = child.stderr.take() {` → `let stderr_str = if let Some(mut stderr) = child.stderr() {`

Other methods (`wait`, `wait_with_output`, `kill`) are already methods, unchanged.

- [ ] **Step 2: Convert `handler.rs` haiku invocation**

Edit `crates/bot/src/telegram/handler.rs`. Around line 1111-1118:

```rust
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());
    cmd.kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn haiku failed: {e:#}"))?;
```

Replace with:

```rust
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());

    let mut child = rightclaw::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| format!("spawn haiku failed: {e:#}"))?;
```

One subsequent replacement at line ~1120:

- `if let Some(mut stdin) = child.stdin.take() {` → `if let Some(mut stdin) = child.stdin() {`

`child.wait_with_output()` is unchanged.

- [ ] **Step 3: Convert `cron.rs` log cleanup**

Edit `crates/bot/src/cron.rs`. Around line 127-133:

```rust
        let output = tokio::process::Command::new("ssh")
            .arg("-F").arg(ssh_config)
            .arg(&ssh_host)
            .arg("--")
            .arg(&cleanup_cmd)
            .output()
            .await;
```

Replace with:

```rust
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config)
            .arg(&ssh_host)
            .arg("--")
            .arg(&cleanup_cmd);
        c.stdout(std::process::Stdio::piped());
        c.stderr(std::process::Stdio::piped());
        let output = match rightclaw::process_group::ProcessGroupChild::spawn(c) {
            Ok(child) => child.wait_with_output().await,
            Err(e) => Err(e),
        };
```

The subsequent match on `output` is unchanged.

- [ ] **Step 4: Convert `cron.rs` main invocation**

Same file, around line 378-393. Current:

```rust
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    tracing::info!(job = %job_name, run_id = %run_id, "executing cron job");

    let mut child = match cmd.spawn() {
        Err(e) => {
            tracing::error!(job = %job_name, "spawn failed: {e:#}");
            update_run_record(&conn, &run_id, None, "failed");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
        Ok(c) => c,
    };
```

Replace with:

```rust
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    tracing::info!(job = %job_name, run_id = %run_id, "executing cron job");

    let mut child = match rightclaw::process_group::ProcessGroupChild::spawn(cmd) {
        Err(e) => {
            tracing::error!(job = %job_name, "spawn failed: {e:#}");
            update_run_record(&conn, &run_id, None, "failed");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
        Ok(c) => c,
    };
```

Two subsequent replacements:

- Line ~396: `let stdout = child.stdout.take().expect("stdout piped");` → `let stdout = child.stdout().expect("stdout piped");`
- Line ~415: `let stderr_bytes = if let Some(mut stderr) = child.stderr.take() {` → `let stderr_bytes = if let Some(mut stderr) = child.stderr() {`

- [ ] **Step 5: Convert `cron_delivery.rs`**

Edit `crates/bot/src/cron_delivery.rs`. Around line 467-472:

```rust
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e:#}"))?;
```

Replace with:

```rust
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = rightclaw::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| format!("spawn failed: {e:#}"))?;
```

One subsequent replacement at line ~474:

- `if let Some(mut stdin) = child.stdin.take() {` → `if let Some(mut stdin) = child.stdin() {`

There may also be a `child.stdout.take()` later in the function — replace with `child.stdout()` if present (run `rg -n 'child\.(stdin|stdout|stderr)\.take' crates/bot/src/cron_delivery.rs` to confirm).

- [ ] **Step 6: Audit-grep**

Run: `rg -n 'kill_on_drop' crates/`
Expected: the only remaining matches are documentation lines/comments, not active `.kill_on_drop(true)` or `.kill_on_drop(false)` calls in code.

Run: `rg -n 'tokio::process::Command::new\("ssh"\)|tokio::process::Command::new\("openshell"\)|Command::new\("ssh"\)|Command::new\("openshell"\)' crates/`

For each match, confirm it uses `ProcessGroupChild::spawn` and not bare `cmd.spawn()`/`cmd.output()`/`cmd.status()`. Fix any stragglers.

- [ ] **Step 7: Build + clippy + test**

Run: `cargo build --workspace`
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Run: `cargo test -p rightclaw --lib process_group`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/telegram/worker.rs \
        crates/bot/src/telegram/handler.rs \
        crates/bot/src/cron.rs \
        crates/bot/src/cron_delivery.rs
git commit -m "fix(bot): wrap ssh/bash claude invocations in ProcessGroupChild

Replaces kill_on_drop(true) per-PID SIGKILL with whole-group SIGKILL on Drop.
Kills the ssh-proxy / bash-spawned claude children atomically when workers
or cron jobs are cancelled. No more orphan openshell ssh-proxy processes
per interrupted interaction."
```

---

### Task 6: Add `test_cleanup` module (registry + panic hook + narrow pkill)

**Files:**
- Create: `crates/rightclaw/src/test_cleanup.rs`
- Modify: `crates/rightclaw/src/lib.rs`

- [ ] **Step 1: Create the module**

Create `crates/rightclaw/src/test_cleanup.rs` with this exact content:

```rust
//! Test-only sandbox cleanup registry + panic hook.
//!
//! The workspace builds with `panic = "abort"` (see top-level Cargo.toml),
//! meaning stack unwinding is skipped on panic — `Drop` handlers do not run.
//! To still clean up OpenShell sandboxes created by tests that panic, we:
//!
//! 1. Register each created sandbox name in a global `Mutex<Vec<String>>`.
//! 2. On first registration, install a `std::panic::set_hook` that drains
//!    the registry and issues `openshell sandbox delete` for each entry
//!    before calling the default panic hook (which then aborts).
//! 3. Happy-path `Drop for TestSandbox` calls `unregister_and_delete`, which
//!    removes the entry and issues the same delete synchronously.
//!
//! Narrow `pkill_test_orphans(name)` is a separate safety net that kills
//! orphan openshell/ssh-proxy processes associated with a specific test
//! sandbox name, run at create-time to clean up leftovers from prior
//! SIGKILLed or externally-terminated runs.

use std::process::Stdio;
use std::sync::{Mutex, OnceLock};

static LIVE_TEST_SANDBOXES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static HOOK_INSTALLED: OnceLock<()> = OnceLock::new();

/// Register a test sandbox. Installs the panic hook on first call.
pub fn register_test_sandbox(name: &str) {
    LIVE_TEST_SANDBOXES
        .lock()
        .expect("registry lock poisoned")
        .push(name.to_owned());

    HOOK_INSTALLED.get_or_init(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            cleanup_all_registered();
            default(info);
        }));
    });
}

/// Unregister a sandbox (use from Drop — the caller should then invoke
/// `delete_sandbox_sync` to actually remove it).
pub fn unregister_test_sandbox(name: &str) {
    LIVE_TEST_SANDBOXES
        .lock()
        .expect("registry lock poisoned")
        .retain(|n| n != name);
}

/// Synchronously delete a sandbox via `openshell sandbox delete`. Safe to
/// call from `Drop` and from a panic hook (no tokio/async required).
pub fn delete_sandbox_sync(name: &str) {
    let _ = std::process::Command::new("openshell")
        .args(["sandbox", "delete", "--name", name, "--no-tty"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Called from the panic hook: drains the registry and synchronously kills
/// orphan processes + deletes each sandbox.
fn cleanup_all_registered() {
    let names: Vec<String> = LIVE_TEST_SANDBOXES
        .lock()
        .expect("registry lock poisoned")
        .drain(..)
        .collect();

    for name in names {
        pkill_test_orphans(&name);
        delete_sandbox_sync(&name);
    }
}

/// Narrow `pkill -9 -f` for a specific test sandbox. Kills only processes
/// whose argv matches one of three OpenShell patterns scoped to this
/// sandbox name. Never matches broad patterns like bare "openshell".
pub fn pkill_test_orphans(sandbox_name: &str) {
    let patterns = [
        format!("openshell sandbox create --name {sandbox_name}"),
        format!("openshell sandbox upload {sandbox_name}"),
        format!("openshell ssh-proxy.*sandbox-id.*{sandbox_name}"),
    ];

    for pattern in &patterns {
        let _ = std::process::Command::new("pkill")
            .args(["-9", "-f", pattern])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
```

- [ ] **Step 2: Expose the module**

Edit `crates/rightclaw/src/lib.rs`. Add to the `pub mod` list:

```rust
pub mod test_cleanup;
```

- [ ] **Step 3: Build + clippy**

Run: `cargo build --workspace`
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no errors, no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/test_cleanup.rs crates/rightclaw/src/lib.rs
git commit -m "feat(rightclaw): add test_cleanup module (registry + panic hook + pkill)

Workspace builds with panic=abort, so Drop does not run on test panics.
This module provides a global Vec<String> registry and a panic hook that
drains it before abort, plus a narrow pkill_test_orphans helper that only
matches argv patterns scoped to a specific sandbox name."
```

---

### Task 7: Wire `test_cleanup` into `TestSandbox`, remove `destroy()`

**Files:**
- Modify: `crates/rightclaw/src/openshell_tests.rs`

- [ ] **Step 1: Add pre-test pkill + registry registration in `TestSandbox::create`**

Edit `crates/rightclaw/src/openshell_tests.rs`. In `TestSandbox::create` (around line 189), after computing `let name = format!(...)` and before `preflight_check`, add:

```rust
        // Belt-and-suspenders cleanup of any orphan processes from a
        // previous SIGKILLed test run that Drop/hook could not handle.
        crate::test_cleanup::pkill_test_orphans(&name);

        // Register in the panic-hook registry so abort-on-panic still
        // triggers sandbox cleanup.
        crate::test_cleanup::register_test_sandbox(&name);
```

- [ ] **Step 2: Add `impl Drop` for `TestSandbox`**

Same file, after the `impl TestSandbox { ... }` block (around line 265), add:

```rust
impl Drop for TestSandbox {
    fn drop(&mut self) {
        crate::test_cleanup::unregister_test_sandbox(&self.name);
        crate::test_cleanup::delete_sandbox_sync(&self.name);
    }
}
```

- [ ] **Step 3: Remove `TestSandbox::destroy()`**

In the `impl TestSandbox` block, delete this method entirely (around line 259-264):

```rust
    /// Delete the sandbox and wait for deletion to complete.
    pub(crate) async fn destroy(self) {
        super::delete_sandbox(&self.name).await;
        let mut client = super::connect_grpc(&self.mtls_dir).await.unwrap();
        let _ = super::wait_for_deleted(&mut client, &self.name, 60, 2).await;
    }
```

- [ ] **Step 4: Remove all `.destroy().await` call sites**

In the same file, find every `sbox.destroy().await;` line (there are 8, at approximate lines 399, 410, 434, 531, 554, 580, 600, 664 — verify with `rg -n '\.destroy\(\)'` in the file).

Delete each such line. Drop-on-scope-exit handles cleanup now.

Example — current:

```rust
    let (out, code) = sbox.exec(&["echo", "hello"]).await;
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "hello");

    sbox.destroy().await;
}
```

Becomes:

```rust
    let (out, code) = sbox.exec(&["echo", "hello"]).await;
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "hello");
}
```

- [ ] **Step 5: Check for `.destroy().await` outside openshell_tests.rs**

Run: `rg -n '\.destroy\(\)' crates/`

Expected: no matches in source. If any remain, they need the same treatment (remove the call; let Drop handle it).

- [ ] **Step 6: Remove the pre-existing "cleanup leftover from previous failed run" block in `TestSandbox::create`**

With `register_test_sandbox` + panic hook + `pkill_test_orphans`, the old `sandbox_exists` check is still useful — it handles the case where a PREVIOUS test session deleted the orphan processes but the sandbox itself remained in OpenShell state. Keep this block as-is:

```rust
        if super::sandbox_exists(&mut client, &name).await.unwrap() {
            super::delete_sandbox(&name).await;
            super::wait_for_deleted(&mut client, &name, 60, 2)
                .await
                .expect("cleanup of leftover sandbox failed");
        }
```

No change.

- [ ] **Step 7: Apply the same registry-register pattern to `right_backend_tests.rs::create_test_sandbox`**

Edit `crates/rightclaw-cli/src/right_backend_tests.rs`. In `create_test_sandbox` (around line 132), before any openshell call, add:

```rust
    rightclaw::test_cleanup::pkill_test_orphans(sandbox_name);
    rightclaw::test_cleanup::register_test_sandbox(sandbox_name);
```

And at the sites where the test currently calls `rightclaw::openshell::delete_sandbox(sandbox_name).await;` (lines 255, 304 per earlier grep), leave those as-is — they're the happy-path cleanup. Also add `rightclaw::test_cleanup::unregister_test_sandbox(sandbox_name);` right after each such delete, so the registry matches reality on happy exit.

Example, current:

```rust
    rightclaw::openshell::delete_sandbox(sandbox_name).await;
```

Becomes:

```rust
    rightclaw::openshell::delete_sandbox(sandbox_name).await;
    rightclaw::test_cleanup::unregister_test_sandbox(sandbox_name);
```

- [ ] **Step 8: Apply the same pattern to `crates/bot/src/sync.rs` test**

Edit `crates/bot/src/sync.rs`. Find the test `initial_sync_does_not_upload_agent_md_files` around line 325. Before the spawn_sandbox at line 376, add:

```rust
        rightclaw::test_cleanup::pkill_test_orphans(sandbox_name);
        rightclaw::test_cleanup::register_test_sandbox(sandbox_name);
```

Find the matching `rightclaw::openshell::delete_sandbox(sandbox_name).await;` at line ~444 and add after it:

```rust
        rightclaw::test_cleanup::unregister_test_sandbox(sandbox_name);
```

- [ ] **Step 9: Apply the same pattern to `crates/rightclaw-cli/tests/cli_integration.rs`**

Find the test at line ~616 (sandbox_name `rightclaw-right` per diagnosis doc). Before `spawn_sandbox`, add:

```rust
    rightclaw::test_cleanup::pkill_test_orphans(sandbox_name);
    rightclaw::test_cleanup::register_test_sandbox(sandbox_name);
```

After `delete_sandbox(sandbox_name).await;` (line ~622), add:

```rust
    rightclaw::test_cleanup::unregister_test_sandbox(sandbox_name);
```

- [ ] **Step 10: Build + clippy**

Run: `cargo build --workspace`
Run: `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 11: Commit**

```bash
git add crates/rightclaw/src/openshell_tests.rs \
        crates/rightclaw-cli/src/right_backend_tests.rs \
        crates/bot/src/sync.rs \
        crates/rightclaw-cli/tests/cli_integration.rs
git commit -m "fix(tests): wire test_cleanup registry into all sandbox-creating tests

TestSandbox now has impl Drop (happy path) + registers in the panic-hook
registry (abort path) + runs a narrow pkill on create (SIGKILL-leftover
path). destroy()/.destroy().await is removed — scope exit handles cleanup.
All other sandbox-creating tests (right_backend_tests, sync, cli_integration)
register in the same pattern so panic in any of them also cleans up."
```

---

### Task 8: Verification

**Files:** none modified (verification only)

- [ ] **Step 1: Full workspace build + clippy**

Run: `cargo build --workspace`
Expected: no errors.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 2: Run the ProcessGroupChild unit tests**

Run: `cargo test -p rightclaw --lib process_group`
Expected: `drop_kills_grandchild` and `control_without_group_leaks_grandchild` both pass.

- [ ] **Step 3: Baseline orphan count**

Before running the integration tests, snapshot the current orphan count:

Run: `pgrep -f 'openshell|ssh-proxy' | wc -l`
Record the number as `BASELINE`. If a bot is running on the host, subtract its expected processes.

- [ ] **Step 4: Run the full test workspace**

Run: `cargo test --workspace -- --test-threads=1`
Expected: all tests pass. (Note: `--test-threads=1` isn't strictly required post-fix, but matches current practice while we verify cleanup.)

- [ ] **Step 5: Post-test orphan count**

Run: `pgrep -f 'openshell|ssh-proxy' | wc -l`

Expected: equals `BASELINE` (± any bot activity on host). If it is higher than BASELINE, investigate: run `pgrep -af 'openshell|ssh-proxy'` and identify the leaker.

- [ ] **Step 6: Panic-path verification**

Add a temporary `panic!("test-cleanup verification");` at the very end of `openshell_tests.rs::exec_immediately_after_sandbox_create_reproduces_init_flow` (or any other test that uses `TestSandbox`). The panic should fire AFTER the sandbox is created.

Run: `cargo test -p rightclaw --test openshell_tests -- exec_immediately_after_sandbox_create_reproduces_init_flow`
Expected: the test fails with the panic message AND the `openshell` hook message printed before process abort.

After abort, verify:
- `openshell sandbox list | grep rightclaw-test-lifecycle` returns nothing.
- `pgrep -af 'rightclaw-test-lifecycle'` returns nothing.

**Revert the temporary panic!()** before committing.

- [ ] **Step 7: Prod-path verification (only if a bot can be run)**

This step is only meaningful if the user has a bot set up. Skip if not.

Start a bot (e.g. `rightclaw up --agents test`). After it reaches READY:

Run: `pgrep -af 'openshell sandbox create'`
Expected: no output (the create process was reaped by the Drop of the `ProcessGroupChild` in `ensure_sandbox`).

Run: `rightclaw down`
Restart the bot a couple more times, repeat the check each time. Orphan count must stay at 0.

- [ ] **Step 8: Commit (if any temporary changes were reverted, ensure clean tree)**

Run: `git status`
Expected: clean working tree.

If clean, no commit needed. The verification itself does not introduce changes.

- [ ] **Step 9: Update the design doc with verification results**

Edit `docs/superpowers/specs/2026-04-17-openshell-test-leaks-design.md`. Change the status line at the top from:

```markdown
**Status:** design approved, plan pending
```

to:

```markdown
**Status:** implemented and verified, <YYYY-MM-DD>
```

Use the actual date. Commit:

```bash
git add docs/superpowers/specs/2026-04-17-openshell-test-leaks-design.md
git commit -m "docs: mark openshell test-leak fix as implemented"
```

---

## Post-implementation self-check

After Task 8 is complete, re-run the diagnosis-doc evidence command from scratch:

```
pgrep -af 'openshell sandbox create --name rightclaw-test-' | wc -l
pgrep -af 'openshell sandbox upload rightclaw-test-' | wc -l
pgrep -af 'ssh.*ProxyCommand=openshell ssh-proxy' | wc -l
pgrep -af 'openshell ssh-proxy --gateway' | wc -l
```

Each should report 0 (or match the baseline bot activity if any). If any number is non-zero and inexplicable, stop and investigate — a regression has been introduced.

`ulimit -u` on the dev box should not be approached by `cargo test --workspace` runs after this fix. The original 4613-PID count was achieved over several aborted runs; per-run leak should now be 0.

## Notes for implementers

- **Do not** touch `acquire_sandbox_slot` / `SandboxTestSlot` — the 3-slot file lock is a separate concurrency limiter, not cleanup. It stays.
- **Do not** introduce `kill_on_drop(true)` on any new Command — always prefer `ProcessGroupChild::spawn`. The wrapper's Drop subsumes per-PID kill.
- **Do not** widen `pkill_test_orphans` patterns. If a new callsite needs broader cleanup, add a new narrow pattern rather than expanding the existing three.
- `#[cfg(unix)]` attributes are not required — the entire project targets Unix (Linux + macOS) per CLAUDE.md. If a future Windows target is added, `process_group`, `killpg`, and `pkill` will all need conditional compilation.
