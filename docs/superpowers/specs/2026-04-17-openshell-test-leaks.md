# OpenShell test-process leaks — fork exhaustion on dev box

**Status:** design resolved, implementation pending
**Date:** 2026-04-17
**Branch observed on:** `groups` (but problem is pre-existing, independent of branch work)

## Symptom

Running `cargo test --workspace` (or even `-p rightclaw`) on a macOS dev box eventually
pins `ulimit -u` (5333 on this host, `kern.maxproc = 8000`). Subsequent forks —
`rustc`, `cargo`, `openshell`, even plain `echo` — fail with `EAGAIN` / `fork failed:
resource temporarily unavailable`. Recovery requires manual `pkill` of orphans or a
reboot.

## Evidence

Snapshot taken after a few aborted `cargo test` runs:

| Pattern | Count |
|---|---:|
| `ssh -o ProxyCommand=… openshell ssh-proxy …` | ~2000 |
| `openshell ssh-proxy --gateway …` (spawned by ssh) | ~2000 |
| Orphan `openshell sandbox create --name rightclaw-test-*` | 439 |
| Orphan `openshell sandbox upload rightclaw-test-*` | 51 |
| **Total PIDs to kill** | **4613** |

Broken down by test name:

```
 211 rightclaw-test-lifecycle             (openshell_tests.rs::exec_immediately_after_sandbox_create_reproduces_init_flow)
  86 rightclaw-test-sync-upload           (bot/src/sync.rs::tests::initial_sync_does_not_upload_agent_md_files)
  52 rightclaw-test-bootstrap-present     (right_backend_tests.rs)
  52 rightclaw-test-bootstrap-missing     (right_backend_tests.rs)
  32 rightclaw-right                      (CLI integration)
   2 test-openshell-host / test-host-policy
```

Two leaked **cargo test harness binaries** were found still running from earlier
sessions:

```
PID  2967  target/debug/deps/rightclaw-8ec86e5128c85f64  --test-threads=1
PID 39542  target/debug/deps/rightclaw-c753295ac624b0a8  --test-threads 1
```

These were the GGP ancestors holding most of the orphan tree alive.

## Process tree for a single live-sandbox test

```
cargo test (test harness binary)
└── openshell sandbox create --name rightclaw-test-FOO --policy … --no-tty    ← kill_on_drop(false)
    └── (internally spawns k3s/containerd/kubelet/runc)                        ← grandchildren
└── openshell sandbox upload rightclaw-test-FOO path /sandbox/                 ← `Command::output().await`
    └── ssh -o ProxyCommand=… sandbox mkdir -p … && tar …                     ← spawned by the `openshell` Go binary
        └── openshell ssh-proxy --gateway … --sandbox-id … --token …          ← spawned by ssh as its ProxyCommand
```

Each live-sandbox test thus has **≥ 4 generations** of processes — and both
`spawn_sandbox` and `openshell sandbox upload` internally do NOT propagate their
child deaths upward.

## Root causes

### 1. `kill_on_drop(false)` on `spawn_sandbox`

`crates/rightclaw/src/openshell.rs:277`:

```rust
cmd.kill_on_drop(false);
```

When a test panics (or is killed mid-flight), the `Child` drops but the openshell
CLI process keeps going. Its grandchildren (ssh, ssh-proxy, k3s bits) are not in
our process group → they survive indefinitely.

This was originally *claimed* to be justified in prod ("so the sandbox
survives if the parent exits"), but that claim was wrong — see the
**Additional finding during design review** section below.

### 2. No `Drop` cleanup on `TestSandbox`

`openshell_tests.rs::TestSandbox` relies on explicit `.destroy().await` at the end
of each test. On panic, `destroy` never runs → sandbox remains in OpenShell →
next test with the same name hits
`"× sandbox 'rightclaw-test-FOO' was not deleted within 60s"` in pre-test cleanup.

### 3. `openshell sandbox upload` spawns SSH+ssh-proxy that detach

Even when the parent test harness exits cleanly, the `openshell sandbox upload`
CLI — written in Go — starts `ssh` as a subprocess. `ssh` uses
`ProxyCommand=openshell ssh-proxy …`, which spawns **another** process.
When the Go binary exits, ssh/ssh-proxy don't always get cleaned up (especially
on SIGKILL of the harness — their PPID becomes 1 and they keep polling the
gateway).

### 4. Cargo does not kill the previous test binary

Running `cargo test` again does not SIGKILL a stuck previous `target/debug/deps/*`
process; cargo only blocks on rebuilding that binary. A hung harness from run N-1
continues holding its orphan tree while run N starts new forks → compound
explosion.

### 5. No pre-test cleanup of leaked sandboxes or processes

`TestSandbox::create` does call
`delete_sandbox(&name); wait_for_deleted(...)` before creating, but only when
OpenShell *reports* the sandbox as still present. If the sandbox itself was
deleted but orphan `openshell ssh-proxy` / `openshell sandbox upload` processes
still exist on the host (from a previous test session), they're never cleaned up
— and they still hold slot in `ulimit -u`.

## My branch's contribution

Minor. This branch (`groups`) added ~17 pure-data unit tests (no sandbox) and
3 CLI integration tests (assert_cmd-style, no sandbox). It also added the 3-slot
file-lock concurrency limiter (`acquire_sandbox_slot`) which *mitigates* the
problem by capping concurrent sandbox tests at 3 within a single workspace run
— but does nothing about leaks from previously-aborted runs.

The fork-exhaust appeared "after my changes" mostly because the branch's scope
made me run `cargo test --workspace` more frequently than usual, accumulating
leaked trees faster.

## Fix options (ordered cheapest → most thorough)

| # | Fix | Scope | Side effects |
|---|---|---|---|
| A | `kill_on_drop(true)` on `spawn_sandbox` **in tests only** (new test helper or `#[cfg(test)]` branch) | `openshell.rs` | None — prod code unchanged |
| B | `impl Drop for TestSandbox` — synchronous `delete_sandbox` + short wait. Call even on panic. | `openshell_tests.rs` (+ refactor `create_test_sandbox`) | May slow tests by a few seconds in happy path |
| C | Pre-test cleanup of orphan processes: `TestSandbox::create` does `pkill -9 -f "rightclaw-test-"` (and ssh-proxy matching) before spawning | `openshell_tests.rs` | Aggressive; kills unrelated in-flight bot `sandbox upload` on the same host |
| D | Kill ancestor test-harness binaries in `TestSandbox::create` — `pkill -9 -f "target/debug/deps/rightclaw-.*-test-threads"` | test helper | Nukes any other cargo test session user is running in parallel — **dangerous** |
| E | Add a session-level cleanup shell script at `scripts/cleanup-test-leaks.sh`, document "run this between test sessions" | docs + script | Requires human discipline, not self-healing |
| F | Switch to `cargo-nextest` which kills test processes per group cleanly | tooling | New dep, but proper fix — nextest has `--final-status-level` and proper process isolation |

### Superseded by Design (resolved) below

The initial recommendation was `A + B + C-lite` (test-only `kill_on_drop(true)`,
`Drop for TestSandbox`, narrow pre-test pkill). During design review we
discovered (a) prod code also leaks on the happy path, and (b) there is a
second leak vector via `ssh` process groups that affects bot operation, not
only tests. The revised solution — **see "Design (resolved)" below** —
replaces "test-only `kill_on_drop`" with a prod-wide `ProcessGroupChild`
wrapper that handles both vectors.

Option D (killing cargo harness binaries) and F (cargo-nextest) remain
rejected for the same reasons (dangerous / out of scope).

## Additional finding during design review

Inspecting callers of `spawn_sandbox` reveals that **prod code also leaks on the
happy path**, not only on panic:

- `crates/rightclaw/src/openshell.rs:825-831` — `create_sandbox` does
  `spawn_sandbox → wait_for_ready → drop(child)`. Combined with
  `kill_on_drop(false)`, that `drop(child)` is a no-op: the openshell CLI
  process does not self-exit after READY (that's why `TestSandbox::create`
  explicitly does `child.kill().await`). Every successful bot sandbox-create
  leaks one `openshell sandbox create` process plus its k3s/ssh descendants.
- `crates/bot/src/sync.rs:376` and `crates/rightclaw-cli/src/right_backend_tests.rs:180`
  use `let _child = spawn_sandbox(...)` with no subsequent kill — leaks on
  every success, not just panic.

The original `kill_on_drop(false)` rationale ("so the sandbox survives if the
parent process exits", commit `30c0a89`) rested on a wrong mental model: the
sandbox is owned by the OpenShell daemon / k3s, not by the CLI process. The CLI
is a gRPC client that polls state; killing it post-READY does not destroy the
sandbox. `kill_on_drop(true)` is safe.

## Additional leak vector: SSH process groups

`tokio::process::Child::kill()` / `kill_on_drop(true)` send **SIGKILL**, which
gives `ssh` zero time to propagate the signal to its `ProxyCommand` child
(`openshell ssh-proxy`). The proxy child is reparented to `init` and keeps
running. This applies to:

- All `Command::new("ssh")` invocations in `worker.rs`, `cron.rs`,
  `cron_delivery.rs`, `handler.rs`, `keepalive.rs` — they already have
  `kill_on_drop(true)` but still orphan the `ssh-proxy` grandchild on worker
  cancellation / bot shutdown.
- `openshell sandbox upload` (Go binary) — internally spawns the same
  `ssh` + `ssh-proxy` pair; if our parent `Command::output().await` future is
  dropped (panic), the whole tree orphans.

Bot-side SSH orphans accumulate during normal operation, not only test runs.
The ~2000 `ssh-proxy` processes in the evidence table are partially from bot
activity, not only from tests.

Empirical confirmation: a controlled experiment on macOS
(`tokio::process::Command::process_group(0)` + `libc::killpg(pgid, SIGKILL)`)
reliably reaps the entire tree in 5/5 runs; the control (no process group)
orphans the grandchild in 5/5 runs. See brainstorm transcript for details.

## Design (resolved)

### Component 1: `ProcessGroupChild` wrapper (prod + tests)

New module `crates/rightclaw/src/process_group.rs` with:

```rust
/// Wraps a tokio Child that was spawned with process_group(0). Drop SIGKILLs
/// the entire process group so grandchildren (e.g. `ssh -o ProxyCommand=...`)
/// are reaped along with the direct child.
pub struct ProcessGroupChild {
    inner: tokio::process::Child,
}

impl ProcessGroupChild {
    pub fn spawn(mut cmd: tokio::process::Command) -> std::io::Result<Self> {
        cmd.process_group(0);
        Ok(Self { inner: cmd.spawn()? })
    }
    pub fn inner_mut(&mut self) -> &mut tokio::process::Child { &mut self.inner }
    // Forwarders: id(), wait(), kill(), stdout/stderr/stdin getters.
}

impl Drop for ProcessGroupChild {
    fn drop(&mut self) {
        if let Some(pid) = self.inner.id() {
            killpg_sigkill(pid);
        }
        // tokio's internal reaper handles the zombie asynchronously.
    }
}

fn killpg_sigkill(pid: u32) {
    // SAFETY: killpg(SIGKILL) has no memory-safety implications. ESRCH
    // (process already gone) and EPERM are harmless; we ignore the return.
    unsafe { libc::killpg(pid as i32, libc::SIGKILL); }
}
```

No new external crate — uses `libc` (already transitive).

**Callsite migration** (all replaced with `ProcessGroupChild::spawn(cmd)`):
- `crates/rightclaw/src/openshell.rs::spawn_sandbox` — remove
  `kill_on_drop(false)` (it was never correct).
- `crates/rightclaw/src/openshell.rs::upload_single_file` — add
  process-group-aware spawn; the `.output().await` pattern becomes
  "wait via the wrapper".
- `crates/bot/src/telegram/worker.rs`, `handler.rs`, `cron.rs`,
  `cron_delivery.rs`, `keepalive.rs` — replace `cmd.kill_on_drop(true);
  cmd.spawn()?` with `ProcessGroupChild::spawn(cmd)`.

**`spawn_sandbox` API change:** returns `ProcessGroupChild` instead of
`tokio::process::Child`. All callers already hold `mut child` and call
either `.kill()` or `.wait()` — the wrapper forwards both. The two buggy
test callsites (`sync.rs:376`, `right_backend_tests.rs:180`) that bound
`_child` without killing will now get automatic cleanup on drop.

### Component 2: `Drop for TestSandbox`

`crates/rightclaw/src/openshell_tests.rs`:

```rust
impl Drop for TestSandbox {
    fn drop(&mut self) {
        // Fire-and-forget sync delete. std::process avoids any tokio runtime
        // dependency during panic unwind. setpgid isolates ssh grandchildren
        // from our process — they die when the spawned openshell CLI exits.
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut cmd = Command::new("openshell");
        cmd.args(["sandbox", "delete", "--name", &self.name, "--no-tty"])
           .stdout(Stdio::null())
           .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        let _ = cmd.spawn();  // fire-and-forget
    }
}
```

`destroy(self)` consumes `self`, so the happy path does not double-delete.
On panic, the Drop fires a delete request; the next test-run's pre-cleanup
(`sandbox_exists → delete_sandbox → wait_for_deleted`) handles completion.

### Component 3: Narrow pre-test pkill

`TestSandbox::create` runs a bounded `pkill` before `spawn_sandbox`:

```rust
// Kill any lingering openshell processes whose argv references this test's
// sandbox name. Narrow patterns — do NOT touch unrelated openshell/ssh-proxy.
for pattern in [
    &format!("openshell sandbox create.*{name}"),
    &format!("openshell sandbox upload.*{name}"),
    &format!("openshell sandbox delete.*{name}"),
    &format!("openshell ssh-proxy.*--sandbox-id .*{name}"),
] {
    let _ = std::process::Command::new("pkill")
        .args(["-9", "-f", pattern]).status();
}
```

Only matches patterns scoped to `rightclaw-test-<this test>`. Bot sandboxes
(`right` and user-named agents) and unrelated dev workflows are untouched.

### Component 4: Not doing (explicit non-goals)

| Option from diagnosis | Resolution |
|---|---|
| D — Kill cargo test harness binaries | Rejected. Races with parallel user dev workflows. |
| E — Manual cleanup shell script | Superseded by Components 1–3 (self-healing). |
| F — Switch to cargo-nextest | Out of scope. Separate tooling decision. |
| Q2 — Auto-clean sandbox on bot crash | No change. `kill_on_drop(true)` on `spawn_sandbox` kills the CLI but the sandbox survives in k3s state — identical to current behavior. |
| Q3 — Periodic orphan reaper on bot startup | Rejected. After Components 1–3 land, new orphans do not appear. Adding a reaper means permanent code to service a transient migration state. One manual `pkill` run after merging is enough. |
| Q5 — Replace `openshell sandbox upload` with direct tar-over-ssh | Deferred. `ProcessGroupChild` around the CLI contains all its grandchildren — the leak motivation goes away. Revisit only if the CLI's other known issues (silent file drops) escalate. |

## Implementation order

1. Add `process_group.rs` with `ProcessGroupChild` + `killpg_sigkill`.
2. Migrate `spawn_sandbox` (prod and the two leaky test callsites).
3. Migrate `upload_single_file`.
4. Migrate the five bot-side `ssh -F ...` callsites.
5. Add `Drop for TestSandbox` + narrow pre-test `pkill`.
6. One-time host cleanup: run `pkill -9 -f "rightclaw-test-"` and
   `pkill -9 -f "openshell ssh-proxy"` once on each dev box where leaks
   already accumulated. No documentation file needed — this is a one-shot
   migration, not an ongoing procedure.
7. Verify: run `cargo test --workspace` 5× in a row on a cold host; confirm
   no `openshell.*` or `ssh-proxy.*` survives between runs.

## Verification criteria

- Between any two consecutive `cargo test --workspace` runs, zero
  `openshell` / `ssh` / `ssh-proxy` processes survive (apart from a running
  bot if the user has one).
- `ps -ef | grep rightclaw-test-` is empty 5s after `cargo test` exits on
  either success or panic.
- Existing tests pass. No new `cargo test` time regression > 5% in CI.
- `cargo clippy --workspace -- -D warnings` clean (single isolated
  `unsafe` block passes).

## References

- `crates/rightclaw/src/openshell.rs:260-285` — `spawn_sandbox` (sets `kill_on_drop(false)`)
- `crates/rightclaw/src/openshell.rs:825-830` — `create_sandbox` drops child without killing
- `crates/rightclaw/src/openshell.rs:569-583` — `upload_single_file`
- `crates/rightclaw/src/openshell_tests.rs:200-265` — `TestSandbox` impl (no `Drop`)
- `crates/bot/src/telegram/worker.rs:1017`, `cron.rs:381`,
  `cron_delivery.rs:470`, `handler.rs:1114`, `keepalive.rs:72` — bot-side
  `ssh` callsites with `kill_on_drop(true)` but no process group.
- `crates/rightclaw-cli/src/right_backend_tests.rs:180`,
  `crates/bot/src/sync.rs:376` — tests that drop `spawn_sandbox` child
  without killing.
- `crates/rightclaw-cli/tests/cli_integration.rs:616` — correct pattern
  (explicit `child.kill().await` after READY).
- Commit `30c0a89` — original `kill_on_drop(false)` introduction.
- Commit `7095f14` (this branch): 3-slot file-lock limiter — partial mitigation,
  kept as defense-in-depth.
