# Bootstrap Onboarding & Reverse Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make BOOTSTRAP.md visible to CC agents so onboarding works, and sync .md file changes from sandbox back to host after every `claude -p` invocation.

**Architecture:** Add `bootstrap_path` to the optional sections in `generate_agent_definition()`. Add a `reverse_sync_md()` function to `sync.rs` that downloads a fixed list of .md files from sandbox and atomically writes changes to host. Wire it into `invoke_cc()` in worker.rs. Cron runs CC directly on host (not via sandbox), so it does not need reverse sync.

**Tech Stack:** Rust, tokio, tempfile (already a dependency), openshell CLI (download_file)

---

### Task 1: Bootstrap in Agent Definition (TDD)

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs:77` (optional array)
- Modify: `crates/rightclaw/src/codegen/agent_def_tests.rs` (new tests + update `make_agent_at`)

- [ ] **Step 1: Add `with_bootstrap` flag to `make_agent_at` test helper**

In `crates/rightclaw/src/codegen/agent_def_tests.rs`, update the helper signature and body:

```rust
fn make_agent_at(
    path: PathBuf,
    model: Option<String>,
    with_soul: bool,
    with_user: bool,
    with_agents: bool,
    with_bootstrap: bool,
) -> AgentDef {
    let config = model.map(|m| AgentConfig {
        model: Some(m),
        restart: Default::default(),
        max_restarts: 3,
        backoff_seconds: 5,
        sandbox: None,

        telegram_token: None,
        allowed_chat_ids: vec![],
        env: Default::default(),
        secret: None,
        attachments: Default::default(),
    });
    AgentDef {
        name: "testbot".to_owned(),
        path: path.clone(),
        identity_path: path.join("IDENTITY.md"),
        config,
        soul_path: if with_soul { Some(path.join("SOUL.md")) } else { None },
        user_path: if with_user { Some(path.join("USER.md")) } else { None },
        agents_path: if with_agents { Some(path.join("AGENTS.md")) } else { None },
        tools_path: None,
        bootstrap_path: if with_bootstrap { Some(path.join("BOOTSTRAP.md")) } else { None },
        heartbeat_path: None,
    }
}
```

Update ALL existing call sites to pass `false` as the new last argument:

- `all_files_produces_frontmatter_and_body_sections`: `make_agent_at(..., true, true, true, false)`
- `model_none_produces_inherit`: `make_agent_at(..., false, false, false, false)`
- `identity_only_produces_frontmatter_plus_identity`: `make_agent_at(..., false, false, false, false)`
- `missing_identity_returns_err`: `make_agent_at(..., false, false, false, false)`
- `soul_present_user_absent_skips_user`: `make_agent_at(..., true, false, false, false)`
- `agent_definition_includes_attachment_format_docs`: `make_agent_at(..., false, false, false, false)`
- `no_tools_field_in_frontmatter`: `make_agent_at(..., false, false, false, false)`

- [ ] **Step 2: Write failing test — bootstrap included between identity and soul**

Add to `crates/rightclaw/src/codegen/agent_def_tests.rs`:

```rust
#[test]
fn bootstrap_present_appears_between_identity_and_soul() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");
    write_file(tmp.path(), "BOOTSTRAP.md", "bootstrap-text");
    write_file(tmp.path(), "SOUL.md", "soul-text");

    let agent = make_agent_at(
        tmp.path().to_path_buf(),
        None,
        true,
        false,
        false,
        true,
    );
    let result = generate_agent_definition(&agent).unwrap();

    assert!(result.contains("bootstrap-text"), "must contain bootstrap section");
    let identity_pos = result.find("identity-text").unwrap();
    let bootstrap_pos = result.find("bootstrap-text").unwrap();
    let soul_pos = result.find("soul-text").unwrap();
    assert!(
        bootstrap_pos > identity_pos,
        "bootstrap must come after identity"
    );
    assert!(
        bootstrap_pos < soul_pos,
        "bootstrap must come before soul"
    );
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p rightclaw bootstrap_present_appears_between_identity_and_soul`

Expected: FAIL — `bootstrap-text` not found in output (bootstrap_path not used yet).

- [ ] **Step 4: Write failing test — bootstrap absent keeps output unchanged**

Add to `crates/rightclaw/src/codegen/agent_def_tests.rs`:

```rust
#[test]
fn bootstrap_none_excluded_from_output() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");
    write_file(tmp.path(), "SOUL.md", "soul-text");

    let agent = make_agent_at(
        tmp.path().to_path_buf(),
        None,
        true,
        false,
        false,
        false,
    );
    let result = generate_agent_definition(&agent).unwrap();

    assert!(!result.contains("bootstrap"), "must not contain bootstrap when path is None");
    assert!(result.contains("identity-text"), "must still contain identity");
    assert!(result.contains("soul-text"), "must still contain soul");
}
```

- [ ] **Step 5: Run both tests to verify the new one passes and the first still fails**

Run: `cargo test -p rightclaw bootstrap_`

Expected: `bootstrap_none_excluded_from_output` PASSES, `bootstrap_present_appears_between_identity_and_soul` FAILS.

- [ ] **Step 6: Implement — add bootstrap_path to optional array**

In `crates/rightclaw/src/codegen/agent_def.rs`, change the optional array (line 77) from:

```rust
    let optional: [Option<&std::path::PathBuf>; 3] = [
        agent.soul_path.as_ref(),
        agent.user_path.as_ref(),
        agent.agents_path.as_ref(),
    ];
```

to:

```rust
    let optional: [Option<&std::path::PathBuf>; 4] = [
        agent.bootstrap_path.as_ref(),
        agent.soul_path.as_ref(),
        agent.user_path.as_ref(),
        agent.agents_path.as_ref(),
    ];
```

- [ ] **Step 7: Run all agent_def tests**

Run: `cargo test -p rightclaw --lib codegen::agent_def`

Expected: ALL pass, including both new bootstrap tests.

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs crates/rightclaw/src/codegen/agent_def_tests.rs
git commit -m "feat: include BOOTSTRAP.md in agent definition for CC onboarding"
```

---

### Task 2: Reverse Sync Function (TDD)

**Files:**
- Modify: `crates/bot/src/sync.rs` (add `reverse_sync_md`)

Note: `reverse_sync_md` calls `openshell::download_file` which shells out to the `openshell` CLI. Unit-testing the real download is not feasible without a live sandbox. Instead, we test the file-comparison and atomic-write logic in a separate pure helper, and integration-test the full function via code review.

- [ ] **Step 1: Write the `reverse_sync_md` function**

Add to the top of `crates/bot/src/sync.rs`, after the existing `use` statements:

```rust
use std::io::Write as _;
use tempfile::NamedTempFile;
```

Add at the end of `crates/bot/src/sync.rs` (before any `#[cfg(test)]` module if one existed — there is none currently):

```rust
/// Files that CC may modify inside the sandbox. Synced back to host after each invocation.
const REVERSE_SYNC_FILES: &[&str] = &[
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
    "BOOTSTRAP.md",
];

/// Sync .md files from sandbox back to host after a `claude -p` invocation.
///
/// For each file in `REVERSE_SYNC_FILES`:
/// - Download from sandbox. If changed vs host: atomic write (tempfile + rename).
/// - If download fails (file absent in sandbox): delete from host if it exists
///   (handles the BOOTSTRAP.md deletion case).
///
/// Per-file errors are collected; the function returns an error summarizing all failures.
/// Callers should log but not propagate — reverse sync is not on the critical path.
pub async fn reverse_sync_md(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()> {
    let tmp_dir = tempfile::tempdir()
        .map_err(|e| miette::miette!("reverse sync: failed to create temp dir: {e:#}"))?;

    let mut errors: Vec<String> = Vec::new();

    for &filename in REVERSE_SYNC_FILES {
        let sandbox_path = format!("/sandbox/{filename}");
        let host_path = agent_dir.join(filename);

        match rightclaw::openshell::download_file(sandbox_name, &sandbox_path, tmp_dir.path()).await
        {
            Ok(()) => {
                let downloaded = tmp_dir.path().join(filename);
                if !downloaded.exists() {
                    // download_file succeeded but no file materialized — skip
                    continue;
                }
                let new_content = match std::fs::read(&downloaded) {
                    Ok(c) => c,
                    Err(e) => {
                        errors.push(format!("{filename}: read downloaded failed: {e:#}"));
                        continue;
                    }
                };

                // Compare with host version — skip if identical
                if host_path.exists() {
                    if let Ok(existing) = std::fs::read(&host_path) {
                        if existing == new_content {
                            tracing::debug!(file = filename, "reverse sync: unchanged, skipping");
                            continue;
                        }
                    }
                }

                // Atomic write: tempfile in agent_dir + rename
                match atomic_write_bytes(&host_path, &new_content) {
                    Ok(()) => {
                        tracing::info!(file = filename, "reverse sync: updated on host");
                    }
                    Err(e) => {
                        errors.push(format!("{filename}: atomic write failed: {e:#}"));
                    }
                }
            }
            Err(_) => {
                // File absent in sandbox — if it exists on host, CC deleted it
                if host_path.exists() {
                    if let Err(e) = std::fs::remove_file(&host_path) {
                        errors.push(format!("{filename}: host delete failed: {e:#}"));
                    } else {
                        tracing::info!(file = filename, "reverse sync: deleted from host (absent in sandbox)");
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(miette::miette!(
            "reverse sync: {} file(s) failed: {}",
            errors.len(),
            errors.join("; ")
        ))
    }
}

/// Atomically write bytes to a path using tempfile + rename in the same directory.
fn atomic_write_bytes(path: &Path, content: &[u8]) -> miette::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| miette::miette!("path has no parent directory"))?;
    let mut tmp = NamedTempFile::new_in(dir)
        .map_err(|e| miette::miette!("failed to create temp file: {e:#}"))?;
    tmp.write_all(content)
        .map_err(|e| miette::miette!("failed to write temp file: {e:#}"))?;
    tmp.persist(path)
        .map_err(|e| miette::miette!("failed to persist temp file: {e:#}"))?;
    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rightclaw-bot`

Expected: compiles with no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/sync.rs
git commit -m "feat: add reverse_sync_md — sync .md files from sandbox to host"
```

---

### Task 3: Wire Reverse Sync into Worker

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:346` (after `invoke_cc`)

- [ ] **Step 1: Add reverse sync call after invoke_cc**

In `crates/bot/src/telegram/worker.rs`, find the block (around line 345-350):

```rust
            // Invoke claude -p (D-13, D-14)
            let reply_result = invoke_cc(&input, chat_id, eff_thread_id, &ctx).await;

            // Cancel typing indicator
            cancel_token.cancel();
```

Replace with:

```rust
            // Invoke claude -p (D-13, D-14)
            let reply_result = invoke_cc(&input, chat_id, eff_thread_id, &ctx).await;

            // Reverse sync: pull .md file changes from sandbox back to host.
            // Only in sandbox mode. Best-effort — log failures, don't block reply.
            if ctx.ssh_config_path.is_some() {
                let sandbox = rightclaw::openshell::sandbox_name(&ctx.agent_name);
                if let Err(e) = crate::sync::reverse_sync_md(&ctx.agent_dir, &sandbox).await {
                    tracing::warn!(agent = %ctx.agent_name, "reverse sync failed: {e:#}");
                }
            }

            // Cancel typing indicator
            cancel_token.cancel();
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rightclaw-bot`

Expected: compiles with no errors.

- [ ] **Step 3: Run full workspace tests**

Run: `cargo test --workspace`

Expected: ALL pass.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat: wire reverse_sync_md into worker after each claude -p call"
```

---

### Notes

**Why no cron changes:** `cron.rs` runs `claude -p` directly on the host (`cmd.current_dir(agent_dir)`), not via SSH into a sandbox. CC writes files directly to the agent directory. No reverse sync needed — changes are already on host.

**Concurrency safety:** `reverse_sync_md` uses per-file atomic writes (tempfile + rename). Two concurrent calls (e.g., worker + cron both finishing at the same time) are idempotent — both download the same content and write the same result. No mutex or coordination needed.
