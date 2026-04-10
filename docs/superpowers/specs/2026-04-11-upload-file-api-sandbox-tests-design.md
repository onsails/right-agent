# upload_file API Change + Sandbox Integration Tests

**Date:** 2026-04-11
**Status:** Approved

## Problem

`mcp/refresh.rs` passes `SANDBOX_MCP_JSON_PATH` (`"/sandbox/mcp.json"`) as the destination
to `upload_file()`. On some OpenShell versions (observed on < 0.0.24), this causes:

```
mkdir: cannot create directory '/sandbox/mcp.json': File exists
```

The `upload_file` API accepts a bare `&str` for the destination with no semantic distinction
between file path and directory path. This allowed the bug to compile and ship.

Additionally, all existing live sandbox tests are hardcoded to `rightclaw-right` — the
production sandbox — risking conflicts with running bot sessions.

## Changes

### 1. `upload_file` API change

**Current signature:**
```rust
pub async fn upload_file(sandbox: &str, host_path: &Path, sandbox_path: &str) -> miette::Result<()>
```

**New signature:**
```rust
pub async fn upload_file(sandbox: &str, host_path: &Path, sandbox_dir: &str) -> miette::Result<()>
```

- Rename `sandbox_path` → `sandbox_dir`
- Add runtime assertion: bail if `sandbox_dir` does not end with `/`
- File lands in `sandbox_dir` with its original name from `host_path.file_name()`
- `openshell sandbox upload` invocation unchanged — it already expects a directory

### 2. Caller fixes

| File | Current | Fixed |
|------|---------|-------|
| `mcp/refresh.rs:182` | `SANDBOX_MCP_JSON_PATH` (`"/sandbox/mcp.json"`) | `"/sandbox/"` |
| `bot/telegram/attachments.rs:356` | `format!("{SANDBOX_INBOX}/{file_name}")` | `format!("{SANDBOX_INBOX}/")` or const with trailing `/` |

All other callers already pass directory paths ending with `/`.

### 3. Regression test update

The existing unit test `refresh_mcp_upload_dest_must_be_directory` in `refresh.rs` currently
asserts against `SANDBOX_MCP_JSON_PATH`. After the fix, update it to verify the actual
destination used in the refresh flow is a directory path.

### 4. Test sandbox infrastructure

New helper in `openshell_tests.rs`:

```rust
struct TestSandbox { name: String }

impl TestSandbox {
    async fn create(test_name: &str) -> Self {
        // name: rightclaw-test-{test_name}
        // minimal policy (no network — fast startup)
        // wait_for_ready
    }
}

impl Drop for TestSandbox {
    fn drop(&mut self) {
        // delete_sandbox + best-effort wait_for_deleted
    }
}
```

- Ephemeral sandbox per test, no dependency on `rightclaw-right`
- `#[ignore]` tag changes from `"requires live OpenShell sandbox 'rightclaw-right'"`
  to `"requires live OpenShell"` — still skipped by default `cargo test`

### 5. Migrate existing live tests

All 5 tests currently hardcoded to `rightclaw-right` migrate to `TestSandbox`:

- `exec_in_sandbox_runs_command`
- `exec_in_sandbox_returns_exit_code`
- `verify_sandbox_files_detects_missing_and_reuploads`
- `exec_immediately_after_sandbox_create_reproduces_init_flow` (already creates own sandbox)
- `verify_sandbox_files_passes_when_all_present`

### 6. New integration tests

| Test | Verifies |
|------|----------|
| `upload_file_to_directory` | Upload file to dir dest, verify content via `exec cat` |
| `upload_file_rejects_non_directory_dest` | `upload_file(s, path, "/sandbox/mcp.json")` → bail |
| `upload_file_overwrites_existing` | Upload twice → second content wins |
| `upload_file_to_nested_dir` | Upload to `/sandbox/inbox/` — intermediate dirs created |

All new tests use `TestSandbox` and are marked `#[ignore = "requires live OpenShell"]`.
