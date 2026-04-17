# Share TestSandbox Across Crates — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Promote the ephemeral `TestSandbox` helper from `pub(crate)` inside `rightclaw`'s test module to a feature-gated `pub` module, so `rightclaw-bot`'s integration tests can use it. Replace the failing `sandbox_upgrade.rs` fixture-dependent tests with a single self-sufficient test. Document the convention in ARCHITECTURE.md.

**Architecture:** New module `crates/rightclaw/src/test_support.rs`, gated on `cfg(all(unix, any(test, feature = "test-support")))`. `rightclaw` gains a `test-support` feature (empty — only toggles module compilation). `crates/bot` adds `rightclaw` as a `[dev-dependencies]` entry with that feature enabled. The old `openshell_tests.rs` stops owning the helper and imports it. The combined integration test runs all four upgrade assertions against one ephemeral sandbox.

**Tech Stack:** Rust 2024, Cargo workspace, `tokio`, `tempfile`, existing `crate::openshell::*` + `crate::test_cleanup::*` helpers.

Spec: `docs/superpowers/specs/2026-04-17-testsandbox-sharing-design.md`.

---

## File Structure

**Create:**
- `crates/rightclaw/src/test_support.rs` — home of `TestSandbox`.

**Modify:**
- `crates/rightclaw/src/lib.rs` — declare the new module under cfg gate.
- `crates/rightclaw/src/openshell_tests.rs` — delete the `TestSandbox` definition + `Drop` impl (lines 176–273), replace with `use crate::test_support::TestSandbox;`.
- `crates/rightclaw/Cargo.toml` — add `[features] test-support = []`.
- `crates/bot/Cargo.toml` — add `[dev-dependencies] rightclaw = { path = "../rightclaw", features = ["test-support"] }`.
- `crates/bot/tests/sandbox_upgrade.rs` — full rewrite: one combined `#[tokio::test]` using `TestSandbox`.
- `ARCHITECTURE.md` — new `## Integration Tests Using Live Sandboxes` section between `## SQLite Rules` and `## Security Model`.

---

## Task 1: Extract `TestSandbox` into `src/test_support.rs`

**Files:**
- Create: `crates/rightclaw/src/test_support.rs`
- Modify: `crates/rightclaw/src/lib.rs`
- Modify: `crates/rightclaw/src/openshell_tests.rs` (delete lines 176–273, add import)

This is a pure move — behavior is unchanged, the existing `#[tokio::test]` functions in `openshell_tests.rs` (which all call `TestSandbox::create` and `.exec` / `.name`) serve as regression tests. We run them at the end of the task to confirm no breakage.

- [ ] **Step 1: Create the new module file**

Create `crates/rightclaw/src/test_support.rs`:

```rust
//! Test-only helpers for consumers that need a live OpenShell sandbox.
//!
//! Gated behind `cfg(all(unix, any(test, feature = "test-support")))`.
//! Consumers outside `rightclaw`'s own test binary depend on the
//! `test-support` feature.

use std::path::PathBuf;

use crate::openshell;
use crate::test_cleanup;

/// Ephemeral test sandbox. Created per test, destroyed on `Drop`. Panic-hook
/// cleanup in `test_cleanup` handles `panic = "abort"` cases.
pub struct TestSandbox {
    name: String,
    mtls_dir: PathBuf,
    _tmp: tempfile::TempDir, // keeps policy file alive
}

impl TestSandbox {
    /// Create an ephemeral sandbox for testing. Cleans up any leftover from
    /// previous runs. The sandbox name is `rightclaw-test-<test_name>`.
    pub async fn create(test_name: &str) -> Self {
        let name = format!("rightclaw-test-{test_name}");

        // Belt-and-suspenders cleanup of any orphan processes from a
        // previous SIGKILLed test run that Drop/hook could not handle.
        test_cleanup::pkill_test_orphans(&name);

        // Register in the panic-hook registry so abort-on-panic still
        // triggers sandbox cleanup.
        test_cleanup::register_test_sandbox(&name);

        let mtls_dir = match openshell::preflight_check() {
            openshell::OpenShellStatus::Ready(dir) => dir,
            other => panic!("OpenShell not ready: {other:?}"),
        };

        // Clean up leftover from a previous failed run.
        let mut client = openshell::connect_grpc(&mtls_dir).await.unwrap();
        if openshell::sandbox_exists(&mut client, &name).await.unwrap() {
            openshell::delete_sandbox(&name).await;
            openshell::wait_for_deleted(&mut client, &name, 60, 2)
                .await
                .expect("cleanup of leftover sandbox failed");
        }

        // Minimal policy — fast startup, permissive network (wildcard 443).
        let tmp = tempfile::tempdir().unwrap();
        let policy_path = tmp.path().join("policy.yaml");
        let policy = "\
version: 1
filesystem_policy:
  include_workdir: true
  read_write:
    - /tmp
    - /sandbox
process:
  run_as_user: sandbox
  run_as_group: sandbox
network_policies:
  outbound:
    endpoints:
      - host: \"**.*\"
        port: 443
        protocol: rest
        access: full
        tls: terminate
    binaries:
      - path: \"**\"
";
        std::fs::write(&policy_path, policy).unwrap();

        let mut child = openshell::spawn_sandbox(&name, &policy_path, None)
            .expect("failed to spawn sandbox");
        openshell::wait_for_ready(&mut client, &name, 120, 2)
            .await
            .expect("sandbox did not become READY");

        // Kill the create process — it doesn't exit on its own after READY.
        let _ = child.kill().await;

        Self { name, mtls_dir, _tmp: tmp }
    }

    /// Sandbox name (already prefixed with `rightclaw-test-`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Execute a command inside the sandbox via gRPC. Returns `(stdout, exit_code)`.
    pub async fn exec(&self, cmd: &[&str]) -> (String, i32) {
        let mut client = openshell::connect_grpc(&self.mtls_dir).await.unwrap();
        let id = openshell::resolve_sandbox_id(&mut client, &self.name)
            .await
            .unwrap();
        openshell::exec_in_sandbox(&mut client, &id, cmd)
            .await
            .unwrap()
    }
}

impl Drop for TestSandbox {
    fn drop(&mut self) {
        test_cleanup::unregister_test_sandbox(&self.name);
        test_cleanup::delete_sandbox_sync(&self.name);
    }
}
```

- [ ] **Step 2: Declare the module in `lib.rs`**

In `crates/rightclaw/src/lib.rs`, add after the existing `#[cfg(unix)] pub mod test_cleanup;` line (around line 17):

```rust
#[cfg(all(unix, any(test, feature = "test-support")))]
pub mod test_support;
```

The resulting module declarations section should look like:

```rust
pub mod runtime;
pub mod sandbox_exec;
#[cfg(unix)]
pub mod test_cleanup;
#[cfg(all(unix, any(test, feature = "test-support")))]
pub mod test_support;
pub mod tunnel;
```

- [ ] **Step 3: Remove the old definition from `openshell_tests.rs`**

Open `crates/rightclaw/src/openshell_tests.rs`. Delete lines 176 through 273 (from the comment block starting `// ---` above `TestSandbox` through the end of `impl Drop for TestSandbox { ... }`).

Replace the deleted block with a single import line near the top of the file (add it right after `use std::sync::Arc;` at line 49):

```rust
use crate::test_support::TestSandbox;
```

- [ ] **Step 4: Build to confirm nothing regressed**

Run: `devenv shell -- cargo build -p rightclaw`
Expected: clean compile, no warnings about `TestSandbox`.

- [ ] **Step 5: Run `rightclaw`'s non-sandbox unit tests**

Run: `devenv shell -- cargo test -p rightclaw --lib -- --skip openshell::tests::`
Expected: all pass. (Skipping `openshell::tests::` avoids live-sandbox tests, which are covered in the next step.)

- [ ] **Step 6: Run one live-sandbox test to confirm the helper still works end-to-end**

Run: `devenv shell -- cargo test -p rightclaw --lib openshell::tests::exec_in_sandbox_runs_command -- --exact --nocapture`
Expected: PASS. (This test calls `TestSandbox::create("exec-run")` → `.exec(&["echo", "hello-from-test"])`. If the helper move broke anything, this surfaces it.)

If that test fails with `OpenShell not ready`, the gateway is down — fix it before continuing (don't skip; dev machines must have OpenShell per project rules).

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/test_support.rs crates/rightclaw/src/lib.rs crates/rightclaw/src/openshell_tests.rs
git commit -m "refactor(test_support): extract TestSandbox into its own module"
```

---

## Task 2: Add the `test-support` feature and wire `bot` to use it

**Files:**
- Modify: `crates/rightclaw/Cargo.toml`
- Modify: `crates/bot/Cargo.toml`

- [ ] **Step 1: Add the feature to `rightclaw`**

In `crates/rightclaw/Cargo.toml`, add a `[features]` section. If the file has no `[features]` section yet, add one just before `[dev-dependencies]`:

```toml
[features]
test-support = []
```

- [ ] **Step 2: Add `rightclaw` as a dev-dep in `bot` with the feature**

In `crates/bot/Cargo.toml`, under the existing `[dev-dependencies]` section, add:

```toml
rightclaw = { path = "../rightclaw", features = ["test-support"] }
```

The regular `[dependencies] rightclaw = { path = "../rightclaw" }` entry stays unchanged. Cargo unifies features across dependency kinds, so `test-support` is enabled only when building the `bot` crate's tests (or anything downstream that turns it on).

- [ ] **Step 3: Build bot's tests to confirm the feature is reachable**

Run: `devenv shell -- cargo test -p rightclaw-bot --no-run`
Expected: clean compile. No "unresolved import" errors. (Task 3's rewrite will exercise the import path — no smoke step needed here.)

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/Cargo.toml crates/bot/Cargo.toml
git commit -m "build(rightclaw): add test-support feature; bot uses it in dev-deps"
```

---

## Task 3: Rewrite `sandbox_upgrade.rs` as one combined test

**Files:**
- Modify (full rewrite): `crates/bot/tests/sandbox_upgrade.rs`

The old four tests are deleted wholesale. The replacement is one `#[tokio::test]` that creates an ephemeral sandbox and runs the whole upgrade lifecycle.

- [ ] **Step 1: Write the new test file**

Overwrite `crates/bot/tests/sandbox_upgrade.rs` with:

```rust
//! Integration test for `claude upgrade` inside an OpenShell sandbox.
//!
//! Creates an ephemeral sandbox via `rightclaw::test_support::TestSandbox`,
//! runs `claude upgrade`, and asserts the full post-upgrade state.
//! Requires a running OpenShell gateway (dev machines have it — no #[ignore]).

use rightclaw::test_support::TestSandbox;

/// Full lifecycle: upgrade runs, symlink appears, upgraded binary reports
/// a Claude Code version, and PATH precedence favours `/sandbox/.local/bin`.
#[tokio::test]
async fn claude_upgrade_lifecycle() {
    let sbox = TestSandbox::create("claude-upgrade").await;

    // 1. `claude upgrade` exits 0 and reports either a fresh install or
    //    "Current version" (idempotent re-run).
    let (stdout, exit) = sbox.exec(&["claude", "upgrade"]).await;
    assert_eq!(exit, 0, "claude upgrade failed; stdout: {stdout}");
    assert!(
        stdout.contains("Successfully updated") || stdout.contains("Current version"),
        "unexpected upgrade output: {stdout}"
    );

    // 2. The symlink `/sandbox/.local/bin/claude` now exists.
    let (_, exit) = sbox.exec(&["test", "-L", "/sandbox/.local/bin/claude"]).await;
    assert_eq!(exit, 0, "/sandbox/.local/bin/claude symlink missing");

    // 3. The upgraded binary runs and reports a Claude Code version.
    let (stdout, exit) = sbox
        .exec(&["/sandbox/.local/bin/claude", "--version"])
        .await;
    assert_eq!(exit, 0, "upgraded binary failed to run");
    assert!(
        stdout.contains("Claude Code"),
        "expected 'Claude Code' in version output, got: {stdout}"
    );

    // 4. PATH precedence: with `/sandbox/.local/bin` prepended, `which claude`
    //    resolves to the upgraded path, not the image's `/usr/local/bin/claude`.
    let (stdout, exit) = sbox
        .exec(&[
            "bash",
            "-c",
            "PATH=/sandbox/.local/bin:$PATH which claude",
        ])
        .await;
    assert_eq!(exit, 0, "`which claude` failed: {stdout}");
    assert_eq!(
        stdout.trim(),
        "/sandbox/.local/bin/claude",
        "expected /sandbox/.local/bin/claude, got: {stdout}"
    );
}
```

- [ ] **Step 2: Compile**

Run: `devenv shell -- cargo test -p rightclaw-bot --test sandbox_upgrade --no-run`
Expected: clean compile.

- [ ] **Step 3: Run the test against a live OpenShell sandbox**

Run: `devenv shell -- cargo test -p rightclaw-bot --test sandbox_upgrade -- --nocapture`
Expected: `test result: ok. 1 passed; 0 failed; 0 ignored`. Expect a runtime of roughly 1–3 minutes (sandbox create + `claude upgrade` download).

If it fails, read the failure carefully:
- `sandbox did not become READY`: OpenShell gateway is down — `openshell sandbox list` to check, fix the gateway, re-run.
- `claude upgrade failed` with 403 / DNS: network policy regression — check `TestSandbox::create`'s policy still has wildcard `"**.*"` on port 443 and `binaries: "**"`.
- `/sandbox/.local/bin/claude symlink missing`: `claude upgrade` succeeded but didn't create the symlink. Inspect the `claude upgrade` stdout printed by step 2 to see where it wrote the binary.

- [ ] **Step 4: Confirm cleanup**

After the test run, `openshell sandbox list` must NOT contain `rightclaw-test-claude-upgrade`. If it does, the `Drop` / panic-hook cleanup is broken — investigate `test_cleanup::delete_sandbox_sync` before proceeding.

Run: `openshell sandbox list | grep rightclaw-test-claude-upgrade && echo LEAK || echo clean`
Expected: `clean`.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/tests/sandbox_upgrade.rs
git commit -m "test(bot): rewrite sandbox_upgrade as self-sufficient TestSandbox-based test"
```

---

## Task 4: Document the convention in `ARCHITECTURE.md`

**Files:**
- Modify: `ARCHITECTURE.md` (insert between current `### Idempotent Migrations` section and `## Security Model`)

- [ ] **Step 1: Insert the new section**

In `ARCHITECTURE.md`, find the `### Idempotent Migrations` subsection (ends around line 450) and the subsequent `## Security Model` heading. Between them, add:

```markdown
## Integration Tests Using Live Sandboxes

Any test that needs a live OpenShell sandbox MUST create it via
`rightclaw::test_support::TestSandbox::create("<test-name>")`. The helper:

- Generates a unique `rightclaw-test-<name>` sandbox with a minimal permissive policy (wildcard `"**.*"` host on port 443, `binaries: "**"`).
- Registers the sandbox in `test_cleanup` so sandboxes are deleted even under `panic = "abort"` (the panic hook drains the registry and calls `openshell sandbox delete`).
- Cleans up leftovers from prior SIGKILLed runs via `pkill_test_orphans`.
- Exposes `.exec(&[...])` which goes through gRPC — the project bans the `openshell sandbox exec` CLI from tests.
- Exposes `.name()` for helpers like `upload_file` that take a sandbox name.

Consumers outside `rightclaw`'s own unit tests depend on the `test-support` feature:

```toml
[dev-dependencies]
rightclaw = { path = "../rightclaw", features = ["test-support"] }
```

Rules:

- Never hardcode sandbox names (no `rightclaw-foo-test-lifecycle` fixtures).
- Never invoke the `openshell` CLI from tests. Use `TestSandbox::exec` or the gRPC helpers in `crate::openshell`.
- Never add `#[ignore]` to sandbox tests. Dev machines have OpenShell.
- Parallel caps (`SandboxTestSlot` / `acquire_sandbox_slot`) are unchanged — tests that create multiple sandboxes should still acquire a slot.
```

Note: keep the triple-backtick fence for the inner `toml` block intact (markdown will render it correctly inside the outer document).

- [ ] **Step 2: Sanity-check the markdown**

Run: `devenv shell -- rg -n '^## Integration Tests Using Live Sandboxes' ARCHITECTURE.md`
Expected: a single match.

Run: `devenv shell -- rg -n '^## Security Model' ARCHITECTURE.md`
Expected: a single match, on a line number greater than the section you just added.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs(ARCHITECTURE): document TestSandbox usage convention"
```

---

## Task 5: Full workspace verification

- [ ] **Step 1: Build the whole workspace**

Run: `devenv shell -- cargo build --workspace`
Expected: clean compile.

- [ ] **Step 2: Run the whole test suite**

Run: `devenv shell -- cargo test --workspace`
Expected: 0 failures. Expect `sandbox_upgrade` (in `rightclaw-bot`) to show `1 passed`.

- [ ] **Step 3: Clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Confirm no sandbox leaks after the full run**

Run: `openshell sandbox list | grep '^rightclaw-test-' && echo LEAK || echo clean`
Expected: `clean`.

- [ ] **Step 5: If anything above failed, stop and investigate before committing**

No commit in this task — it's pure verification. Any fixes needed belong in a follow-up commit with a description of what was broken.

---

## Self-Review Notes

- Spec coverage: every bullet in the spec maps to a task above (extract helper → Task 1, feature + wiring → Task 2, test rewrite → Task 3, ARCHITECTURE.md → Task 4, risks/test plan → Task 5).
- No placeholders: all code blocks are complete, all commands are explicit.
- Type consistency: `TestSandbox::create`, `.exec`, `.name` signatures match between `test_support.rs` (Task 1) and the consumers (`openshell_tests.rs` edit in Task 1 Step 3; `sandbox_upgrade.rs` in Task 3).
- Frequent commits: five commits (one per task; Task 5 has none because it's verification-only).
