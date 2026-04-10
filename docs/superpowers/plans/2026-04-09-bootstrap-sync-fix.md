# Bootstrap Sync Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix bootstrap by syncing content `.md` files to the OpenShell sandbox so CC's `@` references resolve correctly.

**Architecture:** Forward-sync content files on `initial_sync` only (not periodic). Extend reverse sync to include `AGENTS.md` and `TOOLS.md`. Add startup logging and a real-sandbox integration test.

**Tech Stack:** Rust, tokio, rightclaw::openshell (CLI wrappers for `openshell sandbox upload/download`)

---

### Task 1: Add AGENTS.md and TOOLS.md to reverse sync

**Files:**
- Modify: `crates/bot/src/sync.rs:104-111`

- [ ] **Step 1: Add files to REVERSE_SYNC_FILES**

In `crates/bot/src/sync.rs`, change the `REVERSE_SYNC_FILES` const from:

```rust
const REVERSE_SYNC_FILES: &[&str] = &[
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
    "BOOTSTRAP.md",
];
```

to:

```rust
const REVERSE_SYNC_FILES: &[&str] = &[
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
    "BOOTSTRAP.md",
    "AGENTS.md",
    "TOOLS.md",
];
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/sync.rs
git commit -m "fix: add AGENTS.md and TOOLS.md to reverse sync

Agent can edit these files inside the sandbox. Without reverse sync,
edits are lost on sandbox recreate."
```

---

### Task 2: Forward-sync content `.md` files on initial_sync

**Files:**
- Modify: `crates/bot/src/sync.rs:10-17` (add const)
- Modify: `crates/bot/src/sync.rs:14-17` (change `initial_sync` function)

- [ ] **Step 1: Add CONTENT_MD_FILES const**

In `crates/bot/src/sync.rs`, after the existing `SYNC_INTERVAL` const (line 10), add:

```rust
/// Content `.md` files that agent definitions reference via `@./FILE.md`.
/// Uploaded to sandbox root (`/sandbox/`) during initial_sync only — NOT during
/// periodic sync_cycle, because the agent may edit these inside the sandbox.
const CONTENT_MD_FILES: &[&str] = &[
    "BOOTSTRAP.md",
    "AGENTS.md",
    "TOOLS.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
];
```

- [ ] **Step 2: Update initial_sync to upload content files**

Replace the `initial_sync` function (currently lines 14-17):

```rust
pub async fn initial_sync(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()> {
    tracing::info!(sandbox = sandbox_name, "sync: initial cycle (blocking)");
    sync_cycle(agent_dir, sandbox_name).await?;

    // Upload content .md files that CC agent definitions reference via @./FILE.md.
    // Only on startup — sandbox is source of truth after this point.
    for &filename in CONTENT_MD_FILES {
        let host_path = agent_dir.join(filename);
        if host_path.exists() {
            rightclaw::openshell::upload_file(sandbox_name, &host_path, "/sandbox/")
                .await
                .map_err(|e| miette::miette!("sync {filename}: {e:#}"))?;
            tracing::debug!(file = filename, "sync: uploaded content file");
        }
    }

    Ok(())
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles without errors.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/sync.rs
git commit -m "fix: forward-sync content .md files to sandbox on startup

BOOTSTRAP.md, AGENTS.md, TOOLS.md and identity files are uploaded to
/sandbox/ during initial_sync so CC's @ references resolve correctly.
Only on startup — sandbox is source of truth after first sync."
```

---

### Task 3: Add bot startup logging

**Files:**
- Modify: `crates/bot/src/lib.rs:88-93`

- [ ] **Step 1: Add INFO log after config parse**

In `crates/bot/src/lib.rs`, after line 88 (`let is_sandboxed = ...`) and before line 90 (`// Open memory.db`), add:

```rust
    let bootstrap_pending = agent_dir.join("BOOTSTRAP.md").exists();
    tracing::info!(
        agent = %args.agent,
        sandbox_mode = ?config.sandbox_mode(),
        model = config.model.as_deref().unwrap_or("inherit"),
        restart = ?config.restart,
        network_policy = %config.network_policy,
        bootstrap_pending,
        "bot starting"
    );
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "feat: log agent config params at bot startup

Logs sandbox_mode, model, restart, network_policy, bootstrap_pending
at INFO level. Makes debugging easier — previously only memory.db
open was logged."
```

---

### Task 4: Integration test — real sandbox sync

**Files:**
- Modify: `crates/bot/src/sync.rs` (add test module at bottom)

- [ ] **Step 1: Write the integration test**

At the bottom of `crates/bot/src/sync.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that initial_sync uploads content .md files to a real OpenShell sandbox.
    ///
    /// Requires: running OpenShell gateway + existing `rightclaw-right` sandbox.
    /// Run manually: `cargo test -p rightclaw-bot --lib sync::tests::initial_sync_uploads_content_md_files -- --ignored`
    #[tokio::test]
    #[ignore = "requires live OpenShell sandbox"]
    async fn initial_sync_uploads_content_md_files() {
        let sandbox = "rightclaw-right";

        // Build a fake agent dir with known content.
        let agent_dir = tempfile::tempdir().unwrap();
        let root = agent_dir.path();

        // Content .md files with recognizable content.
        let test_files: &[(&str, &str)] = &[
            ("BOOTSTRAP.md", "# test bootstrap content\n"),
            ("AGENTS.md", "# test agents content\n"),
            ("TOOLS.md", "# test tools content\n"),
        ];
        for &(name, content) in test_files {
            std::fs::write(root.join(name), content).unwrap();
        }

        // Minimal .claude/ infrastructure so sync_cycle doesn't fail on missing files.
        let claude_dir = root.join(".claude");
        std::fs::create_dir_all(claude_dir.join("agents")).unwrap();
        std::fs::write(claude_dir.join("settings.json"), "{}").unwrap();
        std::fs::write(claude_dir.join("reply-schema.json"), "{}").unwrap();
        std::fs::write(claude_dir.join("bootstrap-schema.json"), "{}").unwrap();
        std::fs::write(claude_dir.join("agents").join("test.md"), "---\nname: test\n---\n").unwrap();
        std::fs::write(root.join("mcp.json"), "{}").unwrap();

        // Run initial_sync.
        initial_sync(root, sandbox)
            .await
            .expect("initial_sync should succeed");

        // Download each file back and verify content.
        for &(name, expected_content) in test_files {
            let download_dir = tempfile::tempdir().unwrap();
            let sandbox_path = format!("/sandbox/{name}");

            rightclaw::openshell::download_file(sandbox, &sandbox_path, download_dir.path())
                .await
                .unwrap_or_else(|e| panic!("download {name} failed: {e:#}"));

            let downloaded = download_dir.path().join(name);
            assert!(
                downloaded.exists(),
                "{name} should have been downloaded from sandbox"
            );

            let actual = std::fs::read_to_string(&downloaded).unwrap();
            assert_eq!(
                actual, expected_content,
                "{name} content mismatch: expected {expected_content:?}, got {actual:?}"
            );
        }

        // Cleanup: remove test files from sandbox so they don't interfere with real agent.
        for &(name, _) in test_files {
            let _ = rightclaw::openshell::exec_in_sandbox(
                sandbox,
                &format!("rm -f /sandbox/{name}"),
            )
            .await;
        }
    }
}
```

- [ ] **Step 2: Check if `exec_in_sandbox` exists; if not, use SSH exec**

Look at `crates/rightclaw/src/openshell.rs` for available functions. If `exec_in_sandbox` doesn't exist, remove the cleanup block. The test files are harmless — they'll be overwritten by the next real sync. Change the cleanup to:

```rust
        // Note: test files left in sandbox — overwritten by next real initial_sync.
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p rightclaw-bot --tests`
Expected: Compiles without errors.

- [ ] **Step 4: Run the test (if OpenShell is available)**

Run: `cargo test -p rightclaw-bot --lib sync::tests::initial_sync_uploads_content_md_files -- --ignored --nocapture`
Expected: PASS (if OpenShell gateway + rightclaw-right sandbox are running).

If OpenShell is not available (mTLS certs missing per doctor output), confirm the test compiles and is properly `#[ignore]`-gated.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/sync.rs
git commit -m "test: integration test for content .md sync to sandbox

Verifies initial_sync uploads BOOTSTRAP.md, AGENTS.md, TOOLS.md to
a real OpenShell sandbox. Catches the exact bug that broke bootstrap —
content files missing from sandbox."
```

---

### Task 5: Build workspace and verify

**Files:** None (verification only)

- [ ] **Step 1: Build full workspace**

Run: `cargo build --workspace`
Expected: Clean build, no warnings.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace`
Expected: No warnings.

- [ ] **Step 3: Run all non-ignored tests**

Run: `cargo test --workspace`
Expected: All pass.
