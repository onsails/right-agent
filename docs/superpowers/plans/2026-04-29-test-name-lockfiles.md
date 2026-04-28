# Cross-Worktree Test Lockfiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent silent test corruption when parallel Claude Code sessions in different git worktrees concurrently run sandbox-creating or fixed-port tests against the same machine.

**Architecture:** A new `acquire_test_name_lock(name)` helper takes an exclusive `$TMPDIR` file lock keyed on a logical test name. `TestSandbox::create` holds it for the lifetime of the sandbox so concurrent workers serialize on identical sandbox names instead of clobbering each other's running sandboxes. Same helper replaces `#[serial]` on the `right_up` tunnel tests, extending serialization across processes/binaries/worktrees and dropping the `serial_test` dependency.

**Tech Stack:** Rust 2024 stdlib `File::try_lock` / `TryLockError` (already used by `acquire_sandbox_slot`), `tempfile`, `assert_cmd`, `predicates`. No new dependencies.

---

## Background

`TestSandbox::create("policy_apply")` (`crates/right-agent/src/test_support.rs:23`) produces a sandbox named `right-test-policy-apply` deterministically from the test name. Every git worktree sees the same name. The constructor then `pkill_test_orphans(&name)` and `delete_sandbox(&name)` if a sandbox with that name exists — so worktree B starting `policy_apply` while worktree A is mid-test will *delete* A's sandbox. This is silent corruption, not a flake.

The 30-slot sandbox cap (`acquire_sandbox_slot`, `crates/right-agent/src/openshell.rs:333`) already coordinates across worktrees via `$TMPDIR` file locks — that part is correct. We extend the same pattern to per-test-name and to fixed-port tests.

The two `right_up` tunnel tests (`crates/right/tests/right_up_requires_tunnel.rs`) bind a fixed TCP port (`MCP_HTTP_PORT`) inside `cmd_up`'s probe and are currently `#[serial]` — intra-binary only. Two worktrees running them simultaneously race on the port.

## File Structure

- `crates/right-agent/src/openshell.rs` — add `TestNameLock` struct + `acquire_test_name_lock()` next to existing `SandboxTestSlot` / `acquire_sandbox_slot`. Co-located because both are `$TMPDIR`-file-lock test helpers.
- `crates/right-agent/src/openshell_tests.rs` — unit tests for the new helper (acquire/release, contention, drop releases).
- `crates/right-agent/src/test_support.rs` — `TestSandbox::create` acquires the name lock before any cleanup work; struct gains a `_name_lock` field.
- `crates/right/tests/right_up_requires_tunnel.rs` — drop `#[serial]`, acquire the name lock instead.
- `crates/right/Cargo.toml` — drop `serial_test = "3.2"` dev-dep.

No new files. All changes are additive within existing modules.

---

## Task 1: Add `acquire_test_name_lock` helper

**Files:**
- Modify: `crates/right-agent/src/openshell.rs` (add helper after line 353, next to `acquire_sandbox_slot`)
- Modify: `crates/right-agent/src/openshell_tests.rs` (add unit tests at end of file)

- [ ] **Step 1: Write the failing unit tests**

Append at the end of `crates/right-agent/src/openshell_tests.rs`:

```rust
#[test]
fn test_name_lock_acquire_and_release() {
    let lock = super::acquire_test_name_lock("unit-test-acquire-release");
    drop(lock);
    // Re-acquiring after drop must succeed — same process, lock was released.
    let _lock2 = super::acquire_test_name_lock("unit-test-acquire-release");
}

#[test]
fn test_name_lock_blocks_when_held() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    let name = "unit-test-blocks-when-held";
    let held = super::acquire_test_name_lock(name);

    let acquired = Arc::new(AtomicBool::new(false));
    let acquired_clone = Arc::clone(&acquired);
    let handle = thread::spawn(move || {
        let _lock = super::acquire_test_name_lock("unit-test-blocks-when-held");
        acquired_clone.store(true, Ordering::SeqCst);
    });

    // Give the thread time to attempt acquisition; it must still be blocked.
    thread::sleep(Duration::from_millis(500));
    assert!(
        !acquired.load(Ordering::SeqCst),
        "second acquire returned while first lock still held"
    );

    drop(held);
    handle.join().unwrap();
    assert!(
        acquired.load(Ordering::SeqCst),
        "second acquire never completed after first lock released"
    );
}

#[test]
fn test_name_lock_sanitizes_name() {
    // Names with path separators or other unsafe chars must not crash and
    // must round-trip — two distinct unsafe names are still distinct locks.
    let _a = super::acquire_test_name_lock("foo/bar:baz");
    let _b = super::acquire_test_name_lock("foo_bar_baz_other");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p right-agent --lib openshell_tests::test_name_lock`
Expected: FAIL with "cannot find function `acquire_test_name_lock` in module `super`" (or similar — function does not exist yet).

- [ ] **Step 3: Implement the helper**

Insert in `crates/right-agent/src/openshell.rs` immediately after the closing `}` of `acquire_sandbox_slot` (currently at line 353):

```rust

/// Test-only mutex for any test resource keyed by a logical name.
///
/// Held across process boundaries (including different git worktrees and
/// different `cargo test` binaries) via an advisory file lock at
/// `$TMPDIR/right-test-name-<sanitized>.lock`. Drop releases the lock; the
/// kernel also releases on process death.
///
/// Acquire via [`acquire_test_name_lock`].
pub struct TestNameLock {
    _file: std::fs::File,
}

/// Acquire an exclusive cross-process lock on `name`.
///
/// Two callers — in the same process or different processes, including
/// different worktrees running the same test — that pass the same `name`
/// will serialize. Different names do not contend with each other.
///
/// Blocks (polling every 100ms) until the lock is free. Drop the returned
/// [`TestNameLock`] to release.
///
/// Use this to wrap any test that owns a non-discriminated shared resource:
/// a sandbox name, a fixed TCP port, etc. The 30-slot cap from
/// [`acquire_sandbox_slot`] is orthogonal and still applies for sandbox
/// tests.
pub fn acquire_test_name_lock(name: &str) -> TestNameLock {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = std::env::temp_dir().join(format!("right-test-name-{safe}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("open test-name lock {}: {e:#}", path.display()));
    loop {
        match file.try_lock() {
            Ok(()) => return TestNameLock { _file: file },
            Err(std::fs::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                panic!("lock test-name file {}: {e:#}", path.display())
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p right-agent --lib openshell_tests::test_name_lock`
Expected: PASS — three tests pass.

- [ ] **Step 5: Run clippy to ensure no warnings on the new helper**

Run: `cargo clippy -p right-agent --lib --no-deps -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/openshell.rs crates/right-agent/src/openshell_tests.rs
git commit -m "test: add acquire_test_name_lock for cross-worktree resource locks

Adds a $TMPDIR-file-lock-based mutex keyed on an arbitrary logical
test name. Mirrors the existing acquire_sandbox_slot pattern but for
exclusive ownership of named resources (sandbox names, fixed ports)
across processes, test binaries, and git worktrees."
```

---

## Task 2: Hold the name lock inside `TestSandbox::create`

**Files:**
- Modify: `crates/right-agent/src/test_support.rs` (struct + `create` body)

- [ ] **Step 1: Write the failing test**

Append at the end of `crates/right-agent/src/openshell_tests.rs` (the integration helpers live in `right-agent` so this stays in the same crate):

```rust
#[tokio::test]
async fn test_sandbox_holds_name_lock() {
    use right_agent::test_support::TestSandbox;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    let _slot = super::acquire_sandbox_slot();
    let sandbox = TestSandbox::create("name-lock-holds").await;

    // While `sandbox` is alive, acquire_test_name_lock with the same logical
    // name MUST block.
    let acquired = Arc::new(AtomicBool::new(false));
    let acquired_clone = Arc::clone(&acquired);
    let handle = std::thread::spawn(move || {
        let _lock =
            super::acquire_test_name_lock("right-test-name-lock-holds");
        acquired_clone.store(true, Ordering::SeqCst);
    });

    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !acquired.load(Ordering::SeqCst),
        "name lock not held by live TestSandbox"
    );

    drop(sandbox);
    handle.join().unwrap();
    assert!(acquired.load(Ordering::SeqCst));
}
```

> **Note for the implementer:** the `right-agent` crate's own tests already use `right_agent::test_support` paths (the crate refers to itself by name in its `[dev-dependencies]` `test-support` feature gate). If the import path differs in this codebase, use `crate::test_support::TestSandbox`. Check existing tests in the file before editing.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p right-agent --lib openshell_tests::test_sandbox_holds_name_lock`
Expected: FAIL — the assertion at `!acquired.load(...)` fires (name lock not held; second acquire returns immediately).

- [ ] **Step 3: Add `_name_lock` field to `TestSandbox`**

In `crates/right-agent/src/test_support.rs`, change the struct definition (currently lines 14–18) to:

```rust
pub struct TestSandbox {
    name: String,
    mtls_dir: PathBuf,
    _tmp: tempfile::TempDir, // keeps policy file alive
    _name_lock: openshell::TestNameLock,
}
```

The new field is declared LAST. Rust drops fields in declaration order AFTER `Drop::drop` returns; declaring it last means the lock release is the very last thing to happen, after `test_cleanup::delete_sandbox_sync(&self.name)` in the existing `Drop` impl (lines 115–120) has already destroyed the sandbox. This guarantees the next acquirer of the same name sees no leftover sandbox.

- [ ] **Step 4: Acquire the lock at the top of `create`**

In `crates/right-agent/src/test_support.rs`, modify `TestSandbox::create` (currently starts at line 23). Replace the body up to and including the `register_test_sandbox` call with:

```rust
    pub async fn create(test_name: &str) -> Self {
        let name = format!("right-test-{test_name}");

        // Acquire the per-name lock FIRST. Blocks until any other process
        // (including a different worktree's test binary) holding the same
        // name has finished and released. Held for the lifetime of `Self`
        // — released only after Drop completes, by which point the sandbox
        // is already destroyed.
        let name_lock = openshell::acquire_test_name_lock(&name);

        // Belt-and-suspenders cleanup of any orphan processes from a
        // previous SIGKILLed test run that Drop/hook could not handle.
        // Safe under the name lock: we are the unique owner of this name.
        test_cleanup::pkill_test_orphans(&name);

        // Register in the panic-hook registry so abort-on-panic still
        // triggers sandbox cleanup.
        test_cleanup::register_test_sandbox(&name);
```

The rest of `create` (preflight check, leftover-sandbox delete, policy write, spawn, wait_for_ready, kill child) stays unchanged. Update the final `Self { ... }` literal (currently lines 82–86) to:

```rust
        Self {
            name,
            mtls_dir,
            _tmp: tmp,
            _name_lock: name_lock,
        }
```

- [ ] **Step 5: Run the new test to verify it passes**

Run: `cargo test -p right-agent --lib openshell_tests::test_sandbox_holds_name_lock`
Expected: PASS.

- [ ] **Step 6: Run the full sandbox-test suite to verify no regressions**

Run: `cargo test -p right-agent --lib openshell_tests`
Expected: all sandbox tests still pass. The 30-slot cap is unchanged.

Run: `cargo test -p right-agent --test policy_apply`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/right-agent/src/test_support.rs crates/right-agent/src/openshell_tests.rs
git commit -m "test: TestSandbox holds per-name lock across worktrees

Acquires acquire_test_name_lock(name) at the top of TestSandbox::create
and stores the guard as the last struct field, so it releases only
after the sandbox is destroyed in Drop. Two parallel cargo test runs
(typically from different git worktrees) that hit the same test name
now serialize on that name instead of clobbering each other's running
sandbox."
```

---

## Task 3: Replace `#[serial]` on tunnel tests with the name lock

**Files:**
- Modify: `crates/right/tests/right_up_requires_tunnel.rs` (replace serial annotation)
- Modify: `crates/right/Cargo.toml` (drop `serial_test`)

- [ ] **Step 1: Replace the test file body**

Overwrite `crates/right/tests/right_up_requires_tunnel.rs` with:

```rust
//! Integration test: `right up` must error out when the global config has no
//! tunnel block (post-mandatory-tunnel cutover).
//!
//! Both tests run `right up`, which probes a fixed TCP port (MCP_HTTP_PORT)
//! before reading config. To avoid races on that bind probe — within this
//! binary AND across parallel `cargo test` runs in different worktrees — we
//! serialize via acquire_test_name_lock on a shared logical name.

use assert_cmd::Command;
use predicates::prelude::*;
use right_agent::openshell::acquire_test_name_lock;
use tempfile::TempDir;

fn write_minimal_agent(home: &std::path::Path) {
    let agent_dir = home.join("agents").join("test");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("agent.yaml"),
        "restart: never\nnetwork_policy: permissive\n",
    )
    .unwrap();
}

#[test]
fn right_up_errors_when_global_config_missing() {
    let _lock = acquire_test_name_lock("right-up-fixed-port");
    let home = TempDir::new().unwrap();
    write_minimal_agent(home.path());

    Command::cargo_bin("right")
        .unwrap()
        .args(["--home", home.path().to_str().unwrap(), "up"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("tunnel").or(predicate::str::contains("right init")));
}

#[test]
fn right_up_errors_when_tunnel_block_missing_from_config() {
    let _lock = acquire_test_name_lock("right-up-fixed-port");
    let home = TempDir::new().unwrap();
    write_minimal_agent(home.path());
    std::fs::write(
        home.path().join("config.yaml"),
        "aggregator:\n  allowed_hosts:\n    - example.com\n",
    )
    .unwrap();

    Command::cargo_bin("right")
        .unwrap()
        .args(["--home", home.path().to_str().unwrap(), "up"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("tunnel"));
}
```

> **Note for the implementer:** verify `right_agent` is already a dev-dependency of the `right` crate before relying on the `acquire_test_name_lock` import. Check `crates/right/Cargo.toml` `[dev-dependencies]`. If `right-agent` is only a regular dependency (it is — see `crates/right/Cargo.toml`), the test still imports it under the same crate name; no Cargo.toml change for that. The `test-support` feature is NOT required because `acquire_test_name_lock` is a public unconditional helper, not gated on `cfg(test)`.

- [ ] **Step 2: Drop the `serial_test` dev-dependency**

In `crates/right/Cargo.toml`, find the line `serial_test = "3.2"` (currently line 43, in `[dev-dependencies]`). Delete that line.

- [ ] **Step 3: Run the modified tests**

Run: `cargo test -p right --test right_up_requires_tunnel`
Expected: both tests PASS, no `serial_test` references in output.

- [ ] **Step 4: Confirm `serial_test` is gone from the dependency graph for the `right` crate**

Run: `cargo tree -p right --edges=dev | grep -i serial_test || echo "OK: serial_test not in dev-deps"`
Expected: `OK: serial_test not in dev-deps`.

(If `serial_test` is still referenced by any *other* crate's dev-deps, that's out of scope — leave it. The grep above (`-p right`) is scoped.)

- [ ] **Step 5: Run the full workspace test suite to verify nothing else regressed**

Run: `cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 6: Run clippy on the modified test file**

Run: `cargo clippy -p right --tests --no-deps -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/right/tests/right_up_requires_tunnel.rs crates/right/Cargo.toml Cargo.lock
git commit -m "test(right): cross-worktree lock for right up tunnel tests

Replaces #[serial] (intra-binary only) with acquire_test_name_lock
on a shared logical name covering the MCP_HTTP_PORT bind probe in
cmd_up. Now serializes correctly across processes, test binaries,
and git worktrees. Drops the serial_test dev-dependency."
```

---

## Verification

After all three tasks land, the following invariants hold:

1. Two parallel `cargo test --workspace` runs in different git worktrees on the same machine no longer destroy each other's running test sandboxes.
2. The two `right_up_requires_tunnel.rs` tests no longer race on `MCP_HTTP_PORT` across worktrees.
3. The 30-slot sandbox cap is unchanged and still throttles total host load.
4. `serial_test` is gone from `crates/right` dev-deps.
5. No new runtime dependencies; only stdlib `File::try_lock`.

To smoke-test the worktree case manually:

```bash
git worktree add .worktrees/lock-test
( cd .worktrees/lock-test && cargo test -p right-agent --test policy_apply ) &
cargo test -p right-agent --test policy_apply
wait
git worktree remove .worktrees/lock-test
```

Both runs should succeed; if you watch `docker ps` (or `openshell sandbox list`) you should see only one `right-test-*` sandbox per logical name at any moment, and total concurrent sandboxes never exceed `MAX_CONCURRENT_SANDBOX_TESTS`.
