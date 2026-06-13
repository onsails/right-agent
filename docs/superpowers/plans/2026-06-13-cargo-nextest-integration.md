# cargo-nextest Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Adopt `cargo-nextest` as the recommended test runner (faster CI + local loop) while preserving every concurrency guarantee and rewriting the one guarantee nextest's process-per-test model breaks (the shared live sandbox).

**Architecture:** Add nextest to devenv and a `.config/nextest.toml` (profiles + per-prefix test-groups + slow-timeout backstops + retries). Keep the cross-process file locks (`acquire_sandbox_slot`, `acquire_test_name_lock`) as the host-global backstop; layer nextest test-groups for precise per-prefix caps. Replace the `OnceCell` process-local shared sandbox with a non-owning, cross-process create-once `SharedSandboxRef` keyed on a per-run id (`RIGHT_TEST_RUN_ID` or `getppid`). Restructure CI; `cargo test` stays valid but docs recommend nextest.

**Tech Stack:** Rust 2024, cargo-nextest, OpenShell gRPC test helpers (`right-openshell`), devenv/Nix, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-06-13-cargo-nextest-integration-design.md`

---

## File Structure

- `devenv.nix` — add `cargo-nextest` package; switch `enterTest` to nextest.
- `.config/nextest.toml` — **new**; profiles `default`/`ci`/`ci-ignored`, test-groups, overrides, slow-timeouts, retries.
- `crates/right-openshell/src/test_support.rs` — hoist `MINIMAL_POLICY` to a module const; add `exec_in_named_sandbox` helper; add `test_run_id`, `SharedSandboxRef`, `shared_sandbox`.
- `crates/right-openshell/src/openshell_tests.rs` — delete `shared_test_sandbox` (+ OnceCell); repoint 5 consumers; add one reuse regression test.
- `.github/workflows/tests.yml` — workspace job → nextest + doctest guard; ignored job → STT step (nextest) + merged claude/openshell step (nextest, test-groups).
- `AGENTS.rust.md`, `AGENTS.md` — verification-cadence wording → nextest; add shared-sandbox prune note.

---

## Task 1: Add cargo-nextest to devenv

**Files:**
- Modify: `devenv.nix:4-27` (packages list)

- [ ] **Step 1: Add the package**

In `devenv.nix`, add `cargo-nextest` to the `packages` list. Change:

```nix
    git-lfs
    curl             # libcurl required to link release-plz (installed in enterShell)
  ] ++ lib.optionals pkgs.stdenv.isLinux [
```

to:

```nix
    git-lfs
    curl             # libcurl required to link release-plz (installed in enterShell)
    cargo-nextest    # recommended test runner (process-per-test, faster CI/local loop)
  ] ++ lib.optionals pkgs.stdenv.isLinux [
```

- [ ] **Step 2: Verify nextest is available**

Run: `devenv shell -- cargo nextest --version`
Expected: prints a `cargo-nextest 0.9.x` version line (no "command not found").

- [ ] **Step 3: Baseline — nextest runs the existing suite**

Run: `devenv shell -- cargo nextest run -p right-openshell`
Expected: PASS (non-ignored tests). Record any pre-existing failures; they are not introduced by this task.

- [ ] **Step 4: Commit**

```bash
git add devenv.nix
git commit -m "build(devenv): add cargo-nextest"
```

---

## Task 2: Add `.config/nextest.toml`

**Files:**
- Create: `.config/nextest.toml`

- [ ] **Step 1: Create the config**

Create `.config/nextest.toml` with exactly:

```toml
# cargo-nextest configuration. https://nexte.st/docs/configuration
#
# Concurrency model: test-groups bound concurrency WITHIN one `cargo nextest
# run`. The host-global, cross-worktree cap is still enforced separately by
# acquire_sandbox_slot() via RIGHT_MAX_CONCURRENT_SANDBOX_TESTS. Both apply.

[profile.default]
# Local dev loop: fail fast, no retries.
fail-fast = true
retries = 0

[profile.ci]
# Workspace job: run everything, absorb the two known parallel-load flakes
# (cc/invocation pid race, dashboard warn-count) with one retry.
fail-fast = false
retries = 1

[profile.ci-ignored]
# Live-sandbox job: run everything; never auto-mask a flaky sandbox test.
fail-fast = false
retries = 0

[test-groups]
sandbox-claude = { max-threads = 1 }
sandbox-openshell = { max-threads = 2 }

# ci_claude_* — heaviest (a full Claude Code turn inside the sandbox): 1 at a time.
[[profile.ci-ignored.overrides]]
filter = 'test(/ci_claude_/)'
test-group = 'sandbox-claude'
slow-timeout = { period = '120s', terminate-after = 6 }

# ci_openshell_* — sandbox I/O suite: 2 at a time.
[[profile.ci-ignored.overrides]]
filter = 'test(/ci_openshell_/)'
test-group = 'sandbox-openshell'
slow-timeout = { period = '120s', terminate-after = 6 }
```

- [ ] **Step 2: Verify the config parses and filters resolve**

Run: `devenv shell -- cargo nextest list --profile ci-ignored --run-ignored=only -E 'test(/ci_openshell_/)'`
Expected: nextest builds, prints a list of `ci_openshell_*` test names, exits 0. A TOML error or "unknown profile" means the file is malformed — fix before continuing.

- [ ] **Step 3: Verify the union filter resolves both prefixes**

Run: `devenv shell -- cargo nextest list --profile ci-ignored --run-ignored=only -E 'test(/ci_claude_/) | test(/ci_openshell_/)'`
Expected: lists both `ci_claude_*` and `ci_openshell_*` tests, exits 0.

- [ ] **Step 4: Commit**

```bash
git add .config/nextest.toml
git commit -m "test(nextest): profiles, sandbox test-groups, slow-timeouts, retries"
```

---

## Task 3: Refactor test_support — hoist policy const + extract exec helper (no behavior change)

**Files:**
- Modify: `crates/right-openshell/src/test_support.rs`

- [ ] **Step 1: Hoist `MINIMAL_POLICY` to a module-level const**

In `crates/right-openshell/src/test_support.rs`, the `MINIMAL_POLICY` const currently lives **inside** `create`. Move it out so both `create` and the new `shared_sandbox` can use it. Add this module-level const just below the `SANDBOX_SSH_TIMEOUT_ENV` line (around line 51):

```rust
/// Minimal fast-startup policy: public `allowed_ips` endpoint on 443, all
/// binaries allowed. Shared by [`TestSandbox::create`] and [`shared_sandbox`].
pub(crate) const MINIMAL_POLICY: &str = "\
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
      - port: 443
        allowed_ips:
          - \"1.1.1.1/32\"
        protocol: rest
        access: full
    binaries:
      - path: \"**\"
";
```

Then in `TestSandbox::create`, delete the inline `const MINIMAL_POLICY: &str = "...";` block and keep the call `Self::create_with_policy(test_name, MINIMAL_POLICY).await` — it now resolves to the module const.

- [ ] **Step 2: Extract the exec helper**

Add this free helper at the bottom of the file, before the `#[cfg(test)]` module:

```rust
/// Execute a command inside a named sandbox via gRPC. Shared by
/// [`TestSandbox`] and [`SharedSandboxRef`].
pub(crate) async fn exec_in_named_sandbox(
    mtls_dir: &Path,
    name: &str,
    cmd: &[&str],
    timeout_seconds: u32,
) -> (String, i32) {
    let mut client = openshell::connect_grpc(mtls_dir).await.unwrap();
    let id = openshell::resolve_sandbox_id(&mut client, name).await.unwrap();
    openshell::exec_in_sandbox(&mut client, &id, cmd, timeout_seconds)
        .await
        .unwrap()
}
```

- [ ] **Step 3: Route `TestSandbox::exec_with_timeout` through the helper**

Replace the body of `TestSandbox::exec_with_timeout` with a delegating call:

```rust
    pub async fn exec_with_timeout(&self, cmd: &[&str], timeout_seconds: u32) -> (String, i32) {
        exec_in_named_sandbox(&self.mtls_dir, &self.name, cmd, timeout_seconds).await
    }
```

- [ ] **Step 4: Build + clippy (no behavior change)**

Run: `devenv shell -- cargo clippy -p right-openshell --features test-support --all-targets -- -D warnings`
Expected: clean build, no warnings. (These are compile-only checks; the exec paths are exercised by ignored live tests later.)

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/test_support.rs
git commit -m "refactor(test-support): hoist MINIMAL_POLICY, extract exec_in_named_sandbox"
```

---

## Task 4: Implement `SharedSandboxRef` + `shared_sandbox` (cross-process create-once)

**Files:**
- Modify: `crates/right-openshell/src/test_support.rs`
- Test: `crates/right-openshell/src/openshell_tests.rs` (new ignored live test)

- [ ] **Step 1: Write the failing regression test**

In `crates/right-openshell/src/openshell_tests.rs`, add this test in the `// Tests` section (placement is not load-bearing):

```rust
#[tokio::test]
#[ignore = "ci-openshell: boots a live shared sandbox"]
async fn ci_openshell_shared_sandbox_reuses_within_run() {
    // Two calls with the same label in the same process (= same run id) must
    // resolve to ONE sandbox: the first boots it, the second attaches.
    let a = crate::test_support::shared_sandbox("reuse").await;
    let b = crate::test_support::shared_sandbox("reuse").await;
    assert_eq!(
        a.name(),
        b.name(),
        "same run + label must reuse a single shared sandbox"
    );
    let (out, code) = a.exec(&["echo", "shared-ok"]).await;
    assert_eq!(code, 0, "exec in attached shared sandbox should succeed");
    assert_eq!(out.trim(), "shared-ok");
}
```

- [ ] **Step 2: Run it to verify it fails (does not compile yet)**

Run: `devenv shell -- cargo nextest run -p right-openshell --run-ignored=only -E 'test(ci_openshell_shared_sandbox_reuses_within_run)'`
Expected: FAIL — compile error `cannot find function shared_sandbox in module crate::test_support`.

- [ ] **Step 3: Add the run-id helper, `SharedSandboxRef`, and `shared_sandbox`**

In `crates/right-openshell/src/test_support.rs`, add:

```rust
/// Per-run identifier shared by every test process of one runner invocation.
/// Under nextest each test is its own process, but they all share one parent
/// (the nextest runner), so `parent_id()` is identical across the run and
/// distinct across invocations. `RIGHT_TEST_RUN_ID` overrides it (CI pins it
/// to the GitHub run id).
fn test_run_id() -> String {
    std::env::var("RIGHT_TEST_RUN_ID")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::os::unix::process::parent_id().to_string())
}

/// Non-owning handle to a long-lived, cross-process shared sandbox.
///
/// Unlike [`TestSandbox`] it does NOT delete the sandbox on drop and does NOT
/// hold the lifetime name lock — so many test processes can attach
/// concurrently (capped only by the nextest test-group). The sandbox persists
/// past the process; a later run with a different run id recreates a fresh one.
pub struct SharedSandboxRef {
    name: String,
    mtls_dir: PathBuf,
}

impl SharedSandboxRef {
    /// Sandbox name (already prefixed with `right-test-shared-`).
    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn exec(&self, cmd: &[&str]) -> (String, i32) {
        self.exec_with_timeout(cmd, openshell::DEFAULT_EXEC_TIMEOUT_SECS)
            .await
    }

    pub async fn exec_with_timeout(&self, cmd: &[&str], timeout_seconds: u32) -> (String, i32) {
        exec_in_named_sandbox(&self.mtls_dir, &self.name, cmd, timeout_seconds).await
    }
}

/// Boot-once-per-run, reuse-across-processes shared sandbox for tests that need
/// a generic working sandbox and don't care about its initial state. Each
/// caller MUST use a distinct sandbox-side path to avoid stepping on peers.
///
/// Safety: the coordination lock is advisory and held ONLY across this
/// create-or-attach block (kernel releases it on process death). The sandbox
/// name is run-scoped (`right-test-shared-<label>-<runid>`), so concurrent
/// runs/worktrees never block on each other's lock or delete each other's live
/// sandbox. Attach is gated on liveness (`exists && ready`), never on mere
/// existence. The sandbox slot is held only during boot.
pub async fn shared_sandbox(label: &str) -> SharedSandboxRef {
    let runid = test_run_id();
    let name = format!("right-test-shared-{label}-{runid}");

    // Coordination lock — released when `_create_lock` drops on return.
    let _create_lock = openshell::acquire_test_name_lock(&format!("shared-create-{name}"));

    let mtls_dir = match openshell::preflight_check() {
        openshell::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };
    let mut client = openshell::connect_grpc(&mtls_dir).await.unwrap();

    // Attach to a live, healthy shared sandbox booted earlier in this run.
    if openshell::sandbox_exists(&mut client, &name).await.unwrap()
        && openshell::is_sandbox_ready(&mut client, &name).await.unwrap()
    {
        let id = openshell::resolve_sandbox_id(&mut client, &name).await.unwrap();
        openshell::wait_for_ssh(&mut client, &id, sandbox_ssh_timeout_secs(60), 2)
            .await
            .expect("shared sandbox SSH not ready");
        return SharedSandboxRef { name, mtls_dir };
    }

    // Stale / half-booted leftover (crashed boot, or a prior run that reused
    // this run id): delete before recreating.
    if openshell::sandbox_exists(&mut client, &name).await.unwrap() {
        openshell::delete_sandbox(&name).await;
        openshell::wait_for_deleted(&mut client, &name, 60, 2)
            .await
            .expect("cleanup of stale shared sandbox failed");
    }

    // Boot once. Hold a sandbox slot ONLY during creation so the long-lived
    // idle shared sandbox doesn't permanently consume a concurrency slot.
    let slot = openshell::acquire_sandbox_slot();
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("policy.yaml");
    std::fs::write(&policy_path, MINIMAL_POLICY).unwrap();

    let mut child = openshell::spawn_sandbox(&name, &policy_path, None, &[])
        .expect("failed to spawn shared sandbox");
    openshell::wait_for_ready(&mut client, &name, sandbox_ready_timeout_secs(120), 2)
        .await
        .expect("shared sandbox did not become READY");
    let id = openshell::resolve_sandbox_id(&mut client, &name).await.unwrap();
    openshell::wait_for_ssh(&mut client, &id, sandbox_ssh_timeout_secs(60), 2)
        .await
        .expect("shared sandbox SSH did not become ready");
    let _ = child.kill().await;
    drop(slot);

    SharedSandboxRef { name, mtls_dir }
}
```

- [ ] **Step 4: Run the regression test to verify it passes**

Run: `devenv shell -- cargo nextest run -p right-openshell --run-ignored=only -E 'test(ci_openshell_shared_sandbox_reuses_within_run)'`
Expected: PASS. The log shows exactly one `openshell sandbox create` for `right-test-shared-reuse-<runid>` (the second call attaches). Requires a live OpenShell gateway (present on dev machines).

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/test_support.rs crates/right-openshell/src/openshell_tests.rs
git commit -m "feat(test-support): cross-process create-once SharedSandboxRef"
```

---

## Task 5: Repoint consumers + delete the OnceCell `shared_test_sandbox`

**Files:**
- Modify: `crates/right-openshell/src/openshell_tests.rs`

- [ ] **Step 1: Repoint the 5 consumers**

In `crates/right-openshell/src/openshell_tests.rs`, change each of these lines from
`let sbox = shared_test_sandbox().await;` to
`let sbox = crate::test_support::shared_sandbox("io").await;`

Occurrences (line numbers approximate — match by the surrounding test fn):
- in `ci_openshell_verify_sandbox_files_detects_missing_and_reuploads` (~878)
- in `ci_openshell_verify_sandbox_files_passes_when_all_present` (~1044)
- in `ci_openshell_upload_file_to_directory` (~1066)
- in `ci_openshell_upload_directory_preserves_files_and_overwrites` (~1113)
- in `ci_openshell_download_file_writes_to_exact_dest_path` (~1162)

All five share the single label `"io"`, so they reuse one shared sandbox per run. Each already targets a distinct sandbox-side path (`/sandbox/VERIFY_TEST.md`, `/sandbox/hello.txt`, `/sandbox/.claude/skills/...`, `/sandbox/download_test.txt`), so 2-wide concurrency is safe. `sbox.name()` and `sbox.exec(...)` resolve on `SharedSandboxRef` unchanged.

- [ ] **Step 2: Delete the OnceCell fixture**

Delete the `shared_test_sandbox` function and its doc comment (the block from the `/// Shared sandbox for upload / download / verify tests ...` doc comment through the closing `}` of `async fn shared_test_sandbox`, ~lines 629-651). Do NOT touch the `use crate::test_support::TestSandbox;` import (line 625) — it is still used by `TestSandbox::create("name-lock-holds")` (~1360) and `TestSandbox::create("readiness-compat")` (~1418).

- [ ] **Step 3: Build to confirm no unused-import / dead-code warnings**

Run: `devenv shell -- cargo clippy -p right-openshell --features test-support --all-targets -- -D warnings`
Expected: clean. If clippy flags an unused import that ONLY your deletion made unused, remove that specific import; otherwise leave imports untouched.

- [ ] **Step 4: Verify the I/O suite, live, at 2-wide concurrency**

Run: `devenv shell -- cargo nextest run -p right-openshell --features test-support --profile ci-ignored --run-ignored=only -E 'test(/ci_openshell_/)'`
Expected: PASS. The log shows a single `openshell sandbox create` for `right-test-shared-io-<runid>`; the five consumers attach and run (≤2 concurrently). Re-run the same command immediately: the second run uses a different `<runid>` (different parent pid), recreates a fresh shared sandbox without thrashing the first.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/openshell_tests.rs
git commit -m "test(openshell): repoint I/O suite to shared_sandbox, drop OnceCell"
```

---

## Task 6: Restructure CI (`.github/workflows/tests.yml`)

**Files:**
- Modify: `.github/workflows/tests.yml`

- [ ] **Step 1: Workspace job — switch test step to nextest + add doctest guard**

Replace the `Test workspace` step:

```yaml
      - name: Test workspace
        run: |
          devenv shell -- bash -lc "export PATH=\"\$RUNNER_TEMP/bin:\$PATH\"; cargo test --workspace --lib --bins --tests --no-fail-fast --locked"
```

with:

```yaml
      - name: Test workspace
        run: |
          devenv shell -- bash -lc "export PATH=\"\$RUNNER_TEMP/bin:\$PATH\"; cargo nextest run --profile ci --workspace --locked"
      - name: Doctests (guard)
        run: devenv shell -- cargo test --doc --workspace --locked
```

- [ ] **Step 2: Workspace job — dashboard freshness via nextest**

Replace:

```yaml
      - name: Dashboard bundle freshness
        run: devenv shell -- cargo test -p right-dashboard dashboard_bundle_contains_providers_view
```

with:

```yaml
      - name: Dashboard bundle freshness
        run: devenv shell -- cargo nextest run -p right-dashboard -E 'test(dashboard_bundle_contains_providers_view)'
```

- [ ] **Step 3: Ignored job — STT step via nextest**

Replace the `Run STT ignored tests` step's `run:` body:

```yaml
        run: |
          devenv shell -- cargo test --workspace --lib --bins --tests --no-fail-fast --locked ci_stt -- --ignored
```

with:

```yaml
        run: |
          devenv shell -- cargo nextest run --profile ci-ignored --workspace --locked --run-ignored=only -E 'test(/ci_stt_/)'
```

(Keep the step's `id: stt_tests` and `continue-on-error: true`.)

- [ ] **Step 4: Ignored job — merge claude + openshell into one nextest step**

Delete the two steps `Run Claude/OpenShell ignored tests` (`id: claude_tests`) and `Run OpenShell ignored tests` (`id: openshell_tests`). Replace them with a single step:

```yaml
      - name: Run Claude + OpenShell ignored tests
        id: sandbox_tests
        continue-on-error: true
        env:
          RIGHT_MAX_CONCURRENT_SANDBOX_TESTS: "2"
          RIGHT_TEST_RUN_ID: "${{ github.run_id }}-${{ github.run_attempt }}"
        run: |
          devenv shell -- bash -lc "export PATH=\"\$PATH:/usr/local/bin:/usr/bin\"; cargo nextest run --profile ci-ignored --workspace --features right-openshell/test-support --locked --run-ignored=only -E 'test(/ci_claude_/) | test(/ci_openshell_/)'"
```

Per-prefix concurrency (claude=1, openshell=2) comes from the test-groups; the file-lock cap stays at 2 via `RIGHT_MAX_CONCURRENT_SANDBOX_TESTS`.

- [ ] **Step 5: Update the diagnostics + final aggregate to the new step ids**

In `Dump OpenShell diagnostics`, replace the condition referencing the old ids:

```yaml
        if: ${{ failure() || steps.claude_tests.outcome == 'failure' || steps.openshell_tests.outcome == 'failure' }}
```

with:

```yaml
        if: ${{ failure() || steps.sandbox_tests.outcome == 'failure' }}
```

In `Check ignored test results`, replace the two old checks:

```yaml
          if test "${{ steps.claude_tests.outcome }}" != "success"; then
            echo "::error::Claude/OpenShell ignored tests failed"
            failed=1
          fi
          if test "${{ steps.openshell_tests.outcome }}" != "success"; then
            echo "::error::OpenShell ignored tests failed"
            failed=1
          fi
```

with:

```yaml
          if test "${{ steps.sandbox_tests.outcome }}" != "success"; then
            echo "::error::Claude/OpenShell ignored tests failed"
            failed=1
          fi
```

(The `stt_tests` check stays as-is.)

- [ ] **Step 6: Lint the workflow**

Run: `devenv shell -- actionlint .github/workflows/tests.yml`
Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add .github/workflows/tests.yml
git commit -m "ci(tests): run via nextest; merge claude+openshell ignored steps"
```

---

## Task 7: Docs — recommend nextest, note the shared-sandbox prune

**Files:**
- Modify: `devenv.nix` (`enterTest`)
- Modify: `AGENTS.rust.md`
- Modify: `AGENTS.md`

- [ ] **Step 1: `enterTest` → nextest + doctest guard**

In `devenv.nix`, replace the `enterTest` block:

```nix
  enterTest = ''
    cargo test --workspace
    cargo clippy --workspace -- -D warnings
  '';
```

with:

```nix
  enterTest = ''
    cargo nextest run --workspace
    cargo test --doc --workspace
    cargo clippy --workspace -- -D warnings
  '';
```

- [ ] **Step 2: `AGENTS.rust.md` — Targeting + Final verification → nextest**

In `AGENTS.rust.md`, replace the `**Targeting**` bullet (line 68):

```
- **Targeting**: Prefer `devenv shell -- cargo test -p <crate> <filter>` or `devenv shell -- cargo test -p <crate>` during development. Use workspace-wide tests midstream only for broad cross-crate changes or when targeted results cannot prove the behavior.
```

with:

```
- **Targeting**: Prefer `devenv shell -- cargo nextest run -p <crate> <filter>` or `devenv shell -- cargo nextest run -p <crate>` during development (`cargo test` still works but nextest is the recommended runner; doctests run only under `cargo test --doc`). Use workspace-wide tests midstream only for broad cross-crate changes or when targeted results cannot prove the behavior.
```

Replace the `**Final verification**` bullet (line 70):

```
- **Final verification**: Before declaring code work complete, run `devenv shell -- cargo test --workspace`. This is mandatory even when all targeted tests passed.
```

with:

```
- **Final verification**: Before declaring code work complete, run `devenv shell -- cargo nextest run --workspace` plus `devenv shell -- cargo test --doc --workspace`. This is mandatory even when all targeted tests passed.
```

- [ ] **Step 3: `AGENTS.rust.md` — add the shared-sandbox prune note**

In `AGENTS.rust.md`, under the `### 5. Testing` section (after the `**Worktrees**` bullet), add:

```
- **Shared test sandbox**: live OpenShell I/O tests reuse one cross-process sandbox per runner invocation, named `right-test-shared-<label>-<runid>` (`runid` = `RIGHT_TEST_RUN_ID` or the runner's pid). It is not deleted on test exit. CI runners are ephemeral so nothing accumulates there; locally, prune leftovers with `openshell sandbox list` then `openshell sandbox delete right-test-shared-...`. Never delete one mid-run — a different `runid` may be live.
```

- [ ] **Step 4: `AGENTS.md` — verification-cadence wording → nextest**

In `AGENTS.md`, replace line 31:

```
- During implementation, prefer the narrowest useful command (`devenv shell -- cargo test -p <crate> <filter>`, package-level tests, or a targeted build/check) after a TDD red/green loop or a coherent feature slice.
```

with:

```
- During implementation, prefer the narrowest useful command (`devenv shell -- cargo nextest run -p <crate> <filter>`, package-level tests, or a targeted build/check) after a TDD red/green loop or a coherent feature slice. `cargo nextest run` is the recommended runner; doctests run only under `cargo test --doc`.
```

Replace line 32:

```
- At the end of all code work, including work done inside a worktree, `devenv shell -- cargo test --workspace` is mandatory. Targeted tests do not replace the final full workspace test.
```

with:

```
- At the end of all code work, including work done inside a worktree, `devenv shell -- cargo nextest run --workspace` plus `devenv shell -- cargo test --doc --workspace` is mandatory. Targeted tests do not replace the final full workspace test.
```

- [ ] **Step 5: Commit**

```bash
git add devenv.nix AGENTS.rust.md AGENTS.md
git commit -m "docs: recommend cargo nextest; document shared test sandbox prune"
```

---

## Task 8: Final full-workspace verification (mandatory)

**Files:** none (verification only)

- [ ] **Step 1: Full nextest workspace run**

Run: `devenv shell -- cargo nextest run --workspace`
Expected: PASS (ignored tests excluded by default). Record any pre-existing failures unrelated to this change.

- [ ] **Step 2: Doctests**

Run: `devenv shell -- cargo test --doc --workspace`
Expected: PASS (0 doctests today → "0 tests run", exits 0).

- [ ] **Step 3: Clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Live shared-sandbox suite (recommended, requires OpenShell)**

Run: `devenv shell -- cargo nextest run -p right-openshell --features test-support --profile ci-ignored --run-ignored=only -E 'test(/ci_openshell_/)'`
Expected: PASS; one shared-sandbox boot; consumers attach.

- [ ] **Step 5: Commit any fixups**

```bash
git add -A
git commit -m "test: final workspace verification fixups" || echo "nothing to commit"
```

---

## Self-Review notes

- **Spec coverage:** A (devenv+config) → Tasks 1-2; B (SharedSandboxRef) → Tasks 3-5; C (serialization mapping) → realized by Task 2 config + kept locks (no code change needed); D (CI) → Task 6; E (docs) → Task 7; verification cadence → intermediate checks in each task + Task 8 final.
- **Slow-timeout invariant:** `terminate-after = 6 × period 120s = 720s` > worst-case in-test setup (READY 360s + SSH 120s) + work — backstop, never preempts.
- **Locks untouched:** `acquire_sandbox_slot`, `acquire_test_name_lock`, `ARCHIVE_TEST_MUTEX` keep working; nextest self-heals the archive mutex via process isolation.
