# OpenShell test-process leaks — fork exhaustion on dev box

**Status:** problem identified, fix pending discussion
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

Justified in prod bot code (sandbox create is a fire-and-forget lifecycle
operation), but it is the default behavior tests inherit.

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

### Recommended combo

**A + B + C-lite**:

1. **A** — `spawn_sandbox` takes an optional `kill_on_drop: bool` flag or a separate
   test-only wrapper `spawn_sandbox_for_test` with `kill_on_drop(true)`. Test
   harness uses the new wrapper.
2. **B** — `impl Drop for TestSandbox` that does a non-blocking `delete_sandbox`
   and a bounded (10s) wait. Panics don't leak.
3. **C-lite** — narrow pre-test cleanup: only kill `openshell sandbox create` and
   `openshell sandbox upload` processes whose argv references `rightclaw-test-*`.
   Leave unrelated `openshell` and `ssh-proxy` processes alone (they may belong
   to a running bot).

This gives us self-healing tests without breaking prod bot code or killing
unrelated user processes.

### Not recommended

- **D** (killing cargo harness binaries) — too dangerous, races with other dev
  workflows.
- **F** (nextest) — bigger change, not scoped to this issue.
- `kill_on_drop(true)` **in prod** (`openshell.rs:277` unconditional) — the bot
  currently relies on `spawn_sandbox` being able to run to completion even if
  the parent Rust Future is dropped; making it kill_on_drop would change
  startup semantics.

## Open questions for discussion

1. **Is it safe to `kill_on_drop(true)` for prod `spawn_sandbox`?** I initially
   assumed no (see "Not recommended"), but the bot actually calls
   `wait_for_ready` before moving on — if the parent Rust Future drops during
   that wait, do we *want* to leak the openshell CLI?
2. **Who owns cleanup semantics for the bot?** If bot crashes during sandbox
   creation, should the sandbox auto-clean, or remain for debugging? Currently
   the sandbox remains.
3. **Should we add a periodic "reap orphans" task at bot startup?** Before
   creating a new agent sandbox, scan for orphan `openshell sandbox create`
   processes matching this agent and kill them.
4. **Is Drop-on-panic reliable enough for `TestSandbox`?** macOS signal handling
   + tokio runtime + async Drop makes this tricky. May need a
   `std::sync::Mutex<Vec<String>>` of sandboxes-to-destroy registered at create
   time, drained by a `ctor::dtor` at binary exit.
5. **Should `openshell sandbox upload` be replaced with a direct tar-over-ssh
   pipeline using the gRPC-provided SSH config?** The CLI's "fire ssh as a
   subprocess that fires ssh-proxy as another subprocess" is inherently
   leak-prone. A direct tokio SSH client would let us reap children ourselves.

## Scope of this doc

**Diagnosis only.** No code changes proposed yet — waiting on answers to the
open questions above before writing an implementation plan.

## References

- `crates/rightclaw/src/openshell.rs:260-285` — `spawn_sandbox` (sets `kill_on_drop(false)`)
- `crates/rightclaw/src/openshell_tests.rs:200-265` — `TestSandbox` impl (no `Drop`)
- `crates/rightclaw-cli/src/right_backend_tests.rs::create_test_sandbox`
- `crates/rightclaw-cli/tests/cli_integration.rs::test_policy_validates_against_openshell`
- `crates/bot/src/sync.rs::tests::initial_sync_does_not_upload_agent_md_files`

- Commit `7095f14` (this branch): 3-slot file-lock limiter — partial mitigation.
