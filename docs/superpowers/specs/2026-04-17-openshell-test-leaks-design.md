# OpenShell test-process leaks — fix design

**Status:** design approved, plan pending
**Date:** 2026-04-17
**Companion to:** `2026-04-17-openshell-test-leaks.md` (diagnosis)

## Goal

Eliminate orphaned `openshell sandbox create`, `openshell sandbox upload`, `ssh`,
and `openshell ssh-proxy` processes left behind by:

1. Test panics / aborts / SIGKILL of cargo test harness.
2. Tests that successfully complete but never `kill()` their `openshell sandbox
   create` child (it does not self-exit after READY).
3. Production bot startup: `create_sandbox` drops the `openshell` CLI handle
   without killing it (`kill_on_drop(false)`), accumulating one orphan per bot
   start.
4. Bot SSH invocations that SIGKILL the direct `ssh` child but leave the
   `ProxyCommand` (`openshell ssh-proxy`) child orphaned (PPID→1).

The fork-bomb on the dev box (`ulimit -u` exhaustion) is the loudest symptom.
Same root causes leak in production at lower volume.

## Out of scope

- **Q2 (diagnosis doc):** auto-cleaning sandboxes when bot crashes mid-create.
  Current behavior — sandbox remains in OpenShell — preserved for debugging.
- **Q3:** periodic reap-orphans task at bot startup. The fixes below remove the
  source of orphans; reactive reaping is unnecessary if the source is plugged.
- **Q5:** replacing `openshell sandbox upload` with a direct tokio SSH client.
  Process-group cleanup makes the multi-generation CLI tree manageable; the
  rewrite has no remaining motivation.
- Switching to `cargo-nextest`. Larger tooling change, separate decision.

## Core mechanism: `ProcessGroupChild`

A newtype wrapper around `tokio::process::Child` that:

1. Forces the spawned process into a new process group (it becomes group leader,
   `pgid == pid`).
2. On `Drop`, sends `SIGKILL` to the negative PID via `nix::sys::signal::killpg`
   — kills the leader **and** every descendant in the group atomically.
3. Forwards `wait`, `wait_with_output`, `id`, `kill`, `stdout`, `stderr`,
   `stdin` to the inner `Child` so callers don't change.

**Why a wrapper, not just `kill_on_drop(true)`:** tokio's `kill_on_drop` is
per-PID — it sends `SIGKILL` to the direct child only. Grandchildren (here:
`ssh-proxy` spawned by `ssh`, or `ssh` spawned by `openshell sandbox upload`)
survive. Empirically verified on macOS (Darwin 25.3.0): `process_group(0)` +
`killpg(SIGKILL)` reaps the entire tree atomically; the per-PID kill leaves the
grandchild reparented to launchd.

### New file

`crates/rightclaw/src/process_group.rs`

```rust
use std::process::Stdio;
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use tokio::process::{Child, Command};

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

    pub fn id(&self) -> Option<u32> { self.inner.id() }
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.inner.wait().await
    }
    pub async fn wait_with_output(self) -> std::io::Result<std::process::Output> {
        self.inner.wait_with_output().await
    }
    pub async fn kill(&mut self) -> std::io::Result<()> {
        self.inner.kill().await
    }
    pub fn stdout(&mut self) -> Option<&mut tokio::process::ChildStdout> {
        self.inner.stdout.as_mut()
    }
    pub fn stderr(&mut self) -> Option<&mut tokio::process::ChildStderr> {
        self.inner.stderr.as_mut()
    }
    pub fn stdin(&mut self) -> Option<&mut tokio::process::ChildStdin> {
        self.inner.stdin.as_mut()
    }
}

impl Drop for ProcessGroupChild {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid {
            // Best-effort. ESRCH (group already gone) is fine.
            let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
        }
        // tokio's Child::Drop schedules a non-blocking waitpid via its
        // internal reaper, so the leader zombie is collected automatically.
    }
}
```

`Stdio` import is for caller convenience when configuring stdout/stderr before
calling `ProcessGroupChild::spawn`.

### Cargo dependency

Add to `crates/rightclaw/Cargo.toml`:

```toml
nix = { version = "0.30", features = ["signal"] }   # latest at impl time
```

Confirm latest stable on crates.io when implementing.

## Application matrix

| Callsite | Current state | Change |
|---|---|---|
| `crates/rightclaw/src/openshell.rs::spawn_sandbox` (line ~260) | `kill_on_drop(false)`. Prod `create_sandbox` drops child after READY → orphan. Tests sometimes do `child.kill().await`, sometimes not. | Return `ProcessGroupChild`. Remove `kill_on_drop(false)`. All callers' `let _child = …` or `drop(child)` now also kill the group. |
| `crates/rightclaw/src/openshell.rs::upload_single_file` (line ~569) | `Command::new(...).output().await`. No group, no `kill_on_drop`. Future drop on panic = orphan tree (openshell → ssh → ssh-proxy). | Switch to `ProcessGroupChild::spawn(cmd)` then `wait_with_output().await`. Group kill on drop. |
| `crates/rightclaw/src/openshell.rs::upload_directory` (parallel uploads) | Calls `upload_single_file` | Inherits the fix automatically. |
| ssh in `crates/bot/src/telegram/worker.rs:1017` | `kill_on_drop(true)` | Replace with `ProcessGroupChild::spawn`. Drop the per-PID `kill_on_drop`. |
| ssh in `crates/bot/src/telegram/handler.rs:1114` | `kill_on_drop(true)` | Same. |
| ssh in `crates/bot/src/cron.rs:381` | `kill_on_drop(true)` | Same. |
| ssh in `crates/bot/src/cron_delivery.rs:470` | `kill_on_drop(true)` | Same. |
| ssh in `crates/bot/src/keepalive.rs:72` | `kill_on_drop` not set | Same. |
| ssh in `crates/rightclaw/src/openshell.rs:403,445,501` (`wait_for_ssh`, etc.) | `kill_on_drop` not set | Same. |
| ssh in `crates/bot/src/cron.rs:127` (`tar` log retention) | `Command::output().await`, no group | Same wrapping pattern as `upload_single_file`. |

**Audit step during implementation:** grep `Command::new("ssh")` and
`Command::new("openshell")` across the workspace one more time to catch any
callsite missed by this matrix. Any new ssh/openshell callsite must use
`ProcessGroupChild`.

## Test changes

### `TestSandbox` Drop

`crates/rightclaw/src/openshell_tests.rs::TestSandbox`:

- Add `impl Drop` calling `std::process::Command::new("openshell")` synchronously
  with `["sandbox", "delete", "--name", &self.name, "--no-tty"]`,
  `Stdio::null()` for stdout/stderr, `.status()`. Best-effort, ignore errors.
- Remove `TestSandbox::destroy()` entirely and remove all `.destroy().await` call
  sites. Drop-on-scope-exit handles success and panic uniformly.
- Do **not** add `wait_for_deleted` in Drop — slow, and the next test's
  pre-create check already handles "still deleting" state.

Rationale for sync `std::process::Command`:
- Works regardless of tokio runtime flavor (current_thread vs multi_thread).
- Works during stack unwinding from panic.
- `openshell sandbox delete` returns quickly enough that bounded blocking is
  acceptable in test teardown.

### Pre-test narrow cleanup (C-lite from diagnosis doc)

Add to `TestSandbox::create`, before `sandbox_exists` check:

```rust
fn pkill_test_orphans(test_prefix: &str) {
    // Kill *only* processes whose argv contains "rightclaw-test-<prefix>"
    // or matches our orphan ssh-proxy pattern for this test sandbox.
    // Implemented via pgrep -f → kill -9 (no unsafe needed).
    for pattern in [
        &format!("openshell sandbox create --name rightclaw-test-{test_prefix}"),
        &format!("openshell sandbox upload rightclaw-test-{test_prefix}"),
        &format!("openshell ssh-proxy.*sandbox-id.*rightclaw-test-{test_prefix}"),
    ] {
        let _ = std::process::Command::new("pkill")
            .args(["-9", "-f", pattern])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
```

Narrow patterns: only kill processes belonging to **this** test's sandbox name.
Other rightclaw-test-* sandboxes (running in parallel test binaries) and any
unrelated bot processes on the host are left alone.

### Fix tests that don't kill `spawn_sandbox` child

These leak `openshell sandbox create` even on success because `--no-tty` keeps
the CLI alive after READY:

- `crates/bot/src/sync.rs:376` — bind as `let mut child = spawn_sandbox(...)`,
  after `wait_for_ready` add `let _ = child.kill().await;` matching the
  `TestSandbox::create` pattern. Explicit kill is clearer than relying on
  implicit group-kill from Drop for the reader of the test.
- `crates/rightclaw-cli/src/right_backend_tests.rs:180` — same fix.

## Verification criteria

After implementing the changes, confirm by:

1. **Unit verification:** `cargo build --workspace` clean. `cargo clippy
   --workspace --all-targets -- -D warnings` clean.
2. **Process-leak repro:** run `cargo test --workspace -- --test-threads=1`
   from a clean state. After completion: `pgrep -f 'openshell|ssh-proxy' | wc -l`
   should match the count *before* the test run (plus or minus normal bot
   activity if the user is running a bot). Baseline: 0 if no bot.
3. **Panic repro:** add a temporary `panic!()` at the end of one
   `TestSandbox`-using test, run that test, confirm `openshell sandbox list`
   shows the sandbox is gone shortly after the panic. Revert.
4. **Prod bot reduction:** start the bot, observe `pgrep -f 'openshell sandbox
   create'` returns nothing after `wait_for_ready` completes. Restart bot ×3,
   confirm orphan count stays at 0.
5. **SSH grandchild:** during a worker invocation, `pkill -9` the bot, confirm
   no `openshell ssh-proxy --sandbox-id <agent-sandbox>` process survives.

## Risks

- **`kill_on_drop(true)` semantics change for `spawn_sandbox` in prod.** The
  CLI is now killed when the `Child` is dropped instead of being detached.
  Prod's existing `drop(child)` after READY (`openshell.rs:830`) becomes a
  group-kill. The CLI doing nothing useful after READY makes this safe; this
  matches what `TestSandbox::create` does empirically. No sandbox lifecycle
  consequence — the sandbox lives in k3s state, not in the CLI process.
- **Non-Unix portability:** `nix::sys::signal::killpg` is Unix-only; same for
  `Command::process_group`. The project already targets Linux + macOS only
  (per CLAUDE.md). Add `#[cfg(unix)]` only if a Windows target is later
  introduced.
- **Drop ordering during nested `tokio::select!`:** if a future containing a
  `ProcessGroupChild` is cancelled mid-`wait`, the Drop runs synchronously.
  `killpg` is a syscall (~microseconds); negligible. The reaper handles the
  resulting zombie asynchronously.
- **macOS `pkill -f` regex differences:** BSD `pkill` matches against the full
  argv; the patterns above are designed accordingly. If we add a Linux-only
  arm later (procps `pkill -f` matches the same way), no change needed.

## Implementation order (suggested for the implementation plan)

1. Add `nix` dep, create `process_group.rs` with `ProcessGroupChild` + tests
   for the wrapper itself (spawn `bash -c 'sleep 60 & wait'`, drop, assert
   grandchild dead within 200ms — same as the empirical verification).
2. Convert `spawn_sandbox` to return `ProcessGroupChild`. Update all callers.
3. Convert `upload_single_file` to use `ProcessGroupChild`.
4. Convert ssh callsites in bot crate (worker, handler, cron, cron_delivery,
   keepalive) and openshell.rs (`wait_for_ssh` family).
5. `impl Drop for TestSandbox`, remove `destroy()` calls (or no-op the method).
6. Add `pkill_test_orphans` to `TestSandbox::create`.
7. Fix `sync.rs:376` and `right_backend_tests.rs:180` (explicit kill or rely
   on Drop).
8. Run verification criteria above.
