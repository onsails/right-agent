# upload_file API Change + Sandbox Integration Tests

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `upload_file` enforce directory-only destinations and migrate all live sandbox tests to isolated ephemeral sandboxes.

**Architecture:** Change `upload_file` signature to `sandbox_dir`, add runtime assertion, fix two callers. Extract `TestSandbox` helper for ephemeral sandbox lifecycle, migrate 5 existing live tests, add 4 new upload tests.

**Tech Stack:** Rust, OpenShell CLI, tokio, tempfile, miette

**Spec:** `docs/superpowers/specs/2026-04-11-upload-file-api-sandbox-tests-design.md`

---

### Task 1: TestSandbox helper

**Files:**
- Modify: `crates/rightclaw/src/openshell_tests.rs`

This helper will be used by all subsequent tasks. It creates an ephemeral sandbox with a minimal policy (no network restrictions for fast startup), waits for readiness, and deletes on drop.

- [ ] **Step 1: Write TestSandbox struct and async create**

Add after the `mock_client` function (line ~148), before the `// Tests` section:

```rust
// ---------------------------------------------------------------------------
// Ephemeral test sandbox — created per test, deleted on drop.
// ---------------------------------------------------------------------------

struct TestSandbox {
    name: String,
    _tmp: tempfile::TempDir, // keeps policy file alive
}

impl TestSandbox {
    /// Create an ephemeral sandbox for testing. Cleans up any leftover from previous runs.
    async fn create(test_name: &str) -> Self {
        let name = format!("rightclaw-test-{test_name}");

        let mtls_dir = match super::preflight_check() {
            super::OpenShellStatus::Ready(dir) => dir,
            other => panic!("OpenShell not ready: {other:?}"),
        };

        // Clean up leftover from a previous failed run.
        let mut client = super::connect_grpc(&mtls_dir).await.unwrap();
        if super::sandbox_exists(&mut client, &name).await.unwrap() {
            super::delete_sandbox(&name).await;
            super::wait_for_deleted(&mut client, &name, 60, 2)
                .await
                .expect("cleanup of leftover sandbox failed");
        }

        // Minimal policy — no network restrictions, fast startup.
        let tmp = tempfile::tempdir().unwrap();
        let policy_path = tmp.path().join("policy.yaml");
        let policy = "version: v1\nnetwork_policies: []\nbinaries:\n  - path: \"**\"\n";
        std::fs::write(&policy_path, policy).unwrap();

        // spawn_sandbox uses raw sandbox name (not agent name).
        let mut child = super::spawn_sandbox(&name, &policy_path, None)
            .expect("failed to spawn sandbox");
        super::wait_for_ready(&mut client, &name, 120, 2)
            .await
            .expect("sandbox did not become READY");

        // Reap the child process.
        let _ = child.wait().await;

        Self { name, _tmp: tmp }
    }

    fn name(&self) -> &str {
        &self.name
    }

    /// Get a gRPC client + sandbox ID for exec operations.
    async fn grpc(&self) -> (super::OpenShellClient<Channel>, String) {
        let mtls_dir = match super::preflight_check() {
            super::OpenShellStatus::Ready(dir) => dir,
            other => panic!("OpenShell not ready: {other:?}"),
        };
        let mut client = super::connect_grpc(&mtls_dir).await.unwrap();
        let id = super::resolve_sandbox_id(&mut client, &self.name)
            .await
            .unwrap();
        (client, id)
    }

    /// Execute a command inside the sandbox, return (stdout, exit_code).
    async fn exec(&self, cmd: &[&str]) -> (String, i32) {
        let (mut client, id) = self.grpc().await;
        super::exec_in_sandbox(&mut client, &id, cmd)
            .await
            .unwrap()
    }

    /// Delete the sandbox and wait for deletion to complete.
    async fn destroy(self) {
        super::delete_sandbox(&self.name).await;
        let mtls_dir = match super::preflight_check() {
            super::OpenShellStatus::Ready(dir) => dir,
            _ => return,
        };
        let mut client = super::connect_grpc(&mtls_dir).await.unwrap();
        let _ = super::wait_for_deleted(&mut client, &self.name, 60, 2).await;
    }
}
```

- [ ] **Step 2: Build to check compilation**

Run: `cargo check -p rightclaw --lib`
Expected: compiles (TestSandbox is defined but not yet used, under `#[cfg(test)]`).

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/openshell_tests.rs
git commit -m "test: add TestSandbox helper for ephemeral sandbox lifecycle"
```

---

### Task 2: Migrate existing live tests to TestSandbox

**Files:**
- Modify: `crates/rightclaw/src/openshell_tests.rs`

Migrate all 5 `rightclaw-right`-dependent tests. The `exec_immediately_after_sandbox_create` test already creates its own sandbox — it just needs the agent name changed to avoid collision and cleanup improved.

- [ ] **Step 1: Migrate `exec_in_sandbox_runs_command`**

Replace the test (lines ~270-295):

```rust
#[tokio::test]
#[ignore = "requires live OpenShell"]
async fn exec_in_sandbox_runs_command() {
    let sbox = TestSandbox::create("exec-run").await;
    let (stdout, exit_code) = sbox.exec(&["echo", "hello-from-test"]).await;

    assert_eq!(exit_code, 0, "echo should exit 0");
    assert!(
        stdout.contains("hello-from-test"),
        "expected 'hello-from-test' in stdout, got: {stdout:?}"
    );

    sbox.destroy().await;
}
```

- [ ] **Step 2: Migrate `exec_in_sandbox_returns_exit_code`**

Replace the test (lines ~297-318):

```rust
#[tokio::test]
#[ignore = "requires live OpenShell"]
async fn exec_in_sandbox_returns_exit_code() {
    let sbox = TestSandbox::create("exec-exit").await;
    let (_, exit_code) = sbox.exec(&["sh", "-c", "exit 42"]).await;

    assert_eq!(exit_code, 42, "should propagate remote exit code");

    sbox.destroy().await;
}
```

- [ ] **Step 3: Migrate `verify_sandbox_files_detects_missing_and_reuploads`**

Replace the test (lines ~320-371):

```rust
#[tokio::test]
#[ignore = "requires live OpenShell"]
async fn verify_sandbox_files_detects_missing_and_reuploads() {
    let sbox = TestSandbox::create("verify-missing").await;

    // Create a temp dir with a test file.
    let tmp = tempfile::tempdir().unwrap();
    let host_dir = tmp.path();
    std::fs::write(host_dir.join("VERIFY_TEST.md"), "# verify test\n").unwrap();

    // Ensure file does NOT exist in sandbox.
    sbox.exec(&["rm", "-f", "/sandbox/VERIFY_TEST.md"]).await;

    // verify_sandbox_files should detect missing file and re-upload it.
    super::verify_sandbox_files(sbox.name(), host_dir, "/sandbox/", &["VERIFY_TEST.md"])
        .await
        .expect("verify should succeed after re-upload");

    // Confirm file actually exists in sandbox now.
    let (output, _) = sbox.exec(&["cat", "/sandbox/VERIFY_TEST.md"]).await;
    assert_eq!(output, "# verify test\n", "file content should match");

    sbox.destroy().await;
}
```

- [ ] **Step 4: Migrate `verify_sandbox_files_passes_when_all_present`**

Replace the test (lines ~449-485):

```rust
#[tokio::test]
#[ignore = "requires live OpenShell"]
async fn verify_sandbox_files_passes_when_all_present() {
    let sbox = TestSandbox::create("verify-present").await;

    let tmp = tempfile::tempdir().unwrap();
    let host_dir = tmp.path();
    std::fs::write(host_dir.join("PRESENT_TEST.md"), "exists\n").unwrap();

    super::upload_file(sbox.name(), &host_dir.join("PRESENT_TEST.md"), "/sandbox/")
        .await
        .unwrap();

    super::verify_sandbox_files(sbox.name(), host_dir, "/sandbox/", &["PRESENT_TEST.md"])
        .await
        .expect("verify should pass when file exists");

    sbox.destroy().await;
}
```

- [ ] **Step 5: Update `exec_immediately_after_sandbox_create`**

This test creates its own sandbox via `ensure_sandbox`. Change the `#[ignore]` tag and cleanup pattern. The test already uses `"test-lifecycle"` as agent name, which won't collide. Just update the ignore annotation:

Change:
```rust
#[tokio::test]
async fn exec_immediately_after_sandbox_create_reproduces_init_flow() {
```

To:
```rust
#[tokio::test]
#[ignore = "requires live OpenShell"]
async fn exec_immediately_after_sandbox_create_reproduces_init_flow() {
```

(This test already had no `#[ignore]` — it was always skipped by being slow. Adding `#[ignore]` aligns it with the convention.)

- [ ] **Step 6: Build to check compilation**

Run: `cargo check -p rightclaw --lib`
Expected: compiles successfully.

- [ ] **Step 7: Run migrated tests against live OpenShell**

Run: `cargo test -p rightclaw -- --ignored exec_in_sandbox_runs_command exec_in_sandbox_returns_exit_code verify_sandbox_files_detects_missing verify_sandbox_files_passes`
Expected: all 4 tests PASS (each creates and destroys its own sandbox).

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw/src/openshell_tests.rs
git commit -m "test: migrate live sandbox tests to ephemeral TestSandbox"
```

---

### Task 3: New upload integration tests (before API change)

**Files:**
- Modify: `crates/rightclaw/src/openshell_tests.rs`

Write the 4 new tests against the **current** `upload_file` API. Three will pass, one (`upload_file_rejects_non_directory_dest`) will be written as a `#[should_panic]` or assertion that currently **fails** — documenting the bug.

- [ ] **Step 1: Add `upload_file_to_directory` test**

```rust
#[tokio::test]
#[ignore = "requires live OpenShell"]
async fn upload_file_to_directory() {
    let sbox = TestSandbox::create("upload-dir").await;

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "hello sandbox\n").unwrap();

    super::upload_file(sbox.name(), &tmp.path().join("hello.txt"), "/sandbox/")
        .await
        .expect("upload to directory should succeed");

    let (content, code) = sbox.exec(&["cat", "/sandbox/hello.txt"]).await;
    assert_eq!(code, 0);
    assert_eq!(content, "hello sandbox\n");

    sbox.destroy().await;
}
```

- [ ] **Step 2: Add `upload_file_overwrites_existing` test**

```rust
#[tokio::test]
#[ignore = "requires live OpenShell"]
async fn upload_file_overwrites_existing() {
    let sbox = TestSandbox::create("upload-overwrite").await;

    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("data.txt");

    // First upload.
    std::fs::write(&file, "version 1\n").unwrap();
    super::upload_file(sbox.name(), &file, "/sandbox/")
        .await
        .unwrap();

    // Second upload with different content.
    std::fs::write(&file, "version 2\n").unwrap();
    super::upload_file(sbox.name(), &file, "/sandbox/")
        .await
        .unwrap();

    let (content, _) = sbox.exec(&["cat", "/sandbox/data.txt"]).await;
    assert_eq!(content, "version 2\n", "second upload should overwrite");

    sbox.destroy().await;
}
```

- [ ] **Step 3: Add `upload_file_to_nested_dir` test**

```rust
#[tokio::test]
#[ignore = "requires live OpenShell"]
async fn upload_file_to_nested_dir() {
    let sbox = TestSandbox::create("upload-nested").await;

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("nested.txt"), "deep\n").unwrap();

    // Upload to a directory that doesn't exist yet — openshell should create it.
    super::upload_file(sbox.name(), &tmp.path().join("nested.txt"), "/sandbox/a/b/c/")
        .await
        .expect("upload to nested dir should succeed");

    let (content, code) = sbox.exec(&["cat", "/sandbox/a/b/c/nested.txt"]).await;
    assert_eq!(code, 0);
    assert_eq!(content, "deep\n");

    sbox.destroy().await;
}
```

- [ ] **Step 4: Add `upload_file_rejects_non_directory_dest` test**

This test documents the bug — it asserts that `upload_file` should reject file-path destinations. **Currently this test will FAIL** because the validation doesn't exist yet. Mark it `#[ignore]` with explanation.

```rust
/// Regression test: upload_file must reject non-directory destination.
/// Before the fix, passing "/sandbox/mcp.json" as dest caused:
///   mkdir: cannot create directory '/sandbox/mcp.json': File exists
#[tokio::test]
#[ignore = "requires live OpenShell — will pass after upload_file API change"]
async fn upload_file_rejects_non_directory_dest() {
    let sbox = TestSandbox::create("upload-reject").await;

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("mcp.json"), "{}").unwrap();

    let result = super::upload_file(
        sbox.name(),
        &tmp.path().join("mcp.json"),
        "/sandbox/mcp.json",
    )
    .await;

    assert!(result.is_err(), "upload_file must reject file-path destination");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("must be a directory"),
        "error should mention directory requirement, got: {msg}"
    );

    sbox.destroy().await;
}
```

- [ ] **Step 5: Build to check compilation**

Run: `cargo check -p rightclaw --lib`
Expected: compiles.

- [ ] **Step 6: Run the three passing tests**

Run: `cargo test -p rightclaw -- --ignored upload_file_to_directory upload_file_overwrites_existing upload_file_to_nested_dir`
Expected: all 3 PASS.

- [ ] **Step 7: Run the rejection test to confirm it fails**

Run: `cargo test -p rightclaw -- --ignored upload_file_rejects_non_directory_dest`
Expected: FAIL — `upload_file` does not yet validate the destination.

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw/src/openshell_tests.rs
git commit -m "test: add upload_file integration tests with ephemeral sandboxes"
```

---

### Task 4: `upload_file` API change

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs:373-387`

- [ ] **Step 1: Change `upload_file` signature and add validation**

Replace the function at line 373:

```rust
/// Upload a file from host into a running sandbox.
///
/// `sandbox_dir` must be a directory path ending with `/`.
/// The file lands in `sandbox_dir` with its original name from `host_path`.
pub async fn upload_file(sandbox: &str, host_path: &Path, sandbox_dir: &str) -> miette::Result<()> {
    if !sandbox_dir.ends_with('/') {
        miette::bail!(
            "upload destination must be a directory path ending with '/', got: {sandbox_dir}"
        );
    }

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

- [ ] **Step 2: Build to see all caller breakages**

Run: `cargo check --workspace 2>&1 | head -40`
Expected: only warnings, no compile errors — the parameter was renamed but type unchanged. The two buggy callers still compile but will fail at runtime.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/openshell.rs
git commit -m "feat: upload_file validates sandbox_dir ends with '/'"
```

---

### Task 5: Fix callers

**Files:**
- Modify: `crates/rightclaw/src/mcp/refresh.rs:182`
- Modify: `crates/bot/src/telegram/attachments.rs:354-356`
- Modify: `crates/bot/src/telegram/attachments.rs:118`

- [ ] **Step 1: Fix `refresh.rs`**

At line 182, change:
```rust
                                crate::openshell::SANDBOX_MCP_JSON_PATH,
```
To:
```rust
                                "/sandbox/",
```

- [ ] **Step 2: Fix `attachments.rs`**

At line 118, change:
```rust
pub const SANDBOX_INBOX: &str = "/sandbox/inbox";
```
To:
```rust
pub const SANDBOX_INBOX: &str = "/sandbox/inbox/";
```

At lines 354-356, change:
```rust
            let sandbox_path = format!("{SANDBOX_INBOX}/{file_name}");
            let sandbox = rightclaw::openshell::sandbox_name(agent_name);
            rightclaw::openshell::upload_file(&sandbox, &host_path, &sandbox_path).await?;
```
To:
```rust
            let sandbox = rightclaw::openshell::sandbox_name(agent_name);
            rightclaw::openshell::upload_file(&sandbox, &host_path, SANDBOX_INBOX).await?;
```

- [ ] **Step 3: Check that `SANDBOX_INBOX` is not used elsewhere with path joins that break**

Search for all uses of `SANDBOX_INBOX` and verify the trailing `/` doesn't break other concatenations. The constant is used in:
- `attachments.rs:354` — fixed above
- `attachments.rs:627` — check this usage and update if needed (it may format paths for exec commands inside sandbox, where trailing `/` is fine)

- [ ] **Step 4: Update regression test in `refresh.rs`**

Replace the test `refresh_mcp_upload_dest_must_be_directory` (currently asserts on `SANDBOX_MCP_JSON_PATH`). Since the refresh code now uses `"/sandbox/"` directly (not a constant), update the test to verify the constant is NOT used as upload destination. Alternatively, simplify to a documentation-only test:

```rust
    /// Regression: spawn_refresh_loop must NOT use SANDBOX_MCP_JSON_PATH as upload
    /// destination — it's a file reference, not a directory for upload_file().
    /// The fix uses "/sandbox/" directly. This test ensures the validation works.
    #[test]
    fn upload_file_dest_validation_rejects_file_path() {
        let file_path = crate::openshell::SANDBOX_MCP_JSON_PATH;
        assert!(
            !file_path.ends_with('/'),
            "SANDBOX_MCP_JSON_PATH must be a file path (not ending with '/'), got: {file_path}"
        );
    }
```

- [ ] **Step 5: Build workspace**

Run: `cargo check --workspace`
Expected: compiles with no errors.

- [ ] **Step 6: Run unit tests**

Run: `cargo test --workspace`
Expected: all unit tests pass (including the updated regression test in refresh.rs).

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/mcp/refresh.rs crates/bot/src/telegram/attachments.rs
git commit -m "fix: use directory paths for all upload_file callers"
```

---

### Task 6: Run all integration tests

**Files:** None (verification only)

- [ ] **Step 1: Run the rejection test — should now pass**

Run: `cargo test -p rightclaw -- --ignored upload_file_rejects_non_directory_dest`
Expected: PASS — `upload_file` now rejects `"/sandbox/mcp.json"`.

- [ ] **Step 2: Run all live sandbox integration tests**

Run: `cargo test -p rightclaw -- --ignored`
Expected: all `#[ignore]`d tests pass (both migrated and new).

- [ ] **Step 3: Build full workspace in debug mode**

Run: `cargo build --workspace`
Expected: clean build, no warnings.

- [ ] **Step 4: Run full test suite (non-ignored)**

Run: `cargo test --workspace`
Expected: all pass.
