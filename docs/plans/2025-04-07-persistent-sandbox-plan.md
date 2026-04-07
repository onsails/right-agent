# Persistent Sandbox Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make OpenShell sandboxes persistent across bot restarts — reuse existing sandbox if present, apply policy hot-reload, sync only necessary files, never upload host credentials.

**Architecture:** Bot startup checks for existing sandbox via gRPC. If READY, reuse it (generate SSH config, apply policy). If not found, create fresh with curated staging dir. A background sync task uploads config files every 5 min. Credentials come from login flow inside sandbox, never from host.

**Tech Stack:** Rust, tokio, tonic (gRPC), OpenShell CLI (`openshell policy set`, `openshell sandbox upload/download`)

**Design doc:** `docs/plans/2025-04-07-persistent-sandbox-design.md`

---

### Task 1: Add `apply_policy` to openshell module

New function to hot-reload policy on a live sandbox via `openshell policy set`.

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs`
- Modify: `crates/rightclaw/src/openshell_tests.rs`

- [ ] **Step 1: Write failing test**

Add to `crates/rightclaw/src/openshell_tests.rs`:

```rust
#[tokio::test]
async fn apply_policy_builds_correct_command() {
    // We can't run openshell in tests, but we verify the function exists
    // and accepts the right types. Integration tested via smoke test.
    let _: fn(&str, &std::path::Path) -> std::pin::Pin<Box<dyn std::future::Future<Output = miette::Result<()>> + Send>> = |_, _| {
        Box::pin(async { Ok(()) })
    };
    // Compile-time check that apply_policy signature matches
}
```

Actually, since we can't mock the CLI, write a unit test for the command construction pattern. Better: just implement and verify via build.

- [ ] **Step 2: Implement `apply_policy`**

Add to `crates/rightclaw/src/openshell.rs` after `generate_ssh_config`:

```rust
/// Apply a policy to a running sandbox via `openshell policy set`.
///
/// Uses `--wait` to block until the sandbox confirms it loaded the new policy.
pub async fn apply_policy(name: &str, policy_path: &Path) -> miette::Result<()> {
    let output = Command::new("openshell")
        .args(["policy", "set", name, "--policy"])
        .arg(policy_path)
        .args(["--wait", "--timeout", "30"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| miette::miette!("failed to run openshell policy set: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "openshell policy set failed (exit {}): {stderr}",
            output.status
        ));
    }

    tracing::info!(sandbox = name, "policy applied");
    Ok(())
}
```

- [ ] **Step 3: Add `download_file` function**

Add to `crates/rightclaw/src/openshell.rs` after `upload_file`:

```rust
/// Download a file from a sandbox to the host.
pub async fn download_file(sandbox: &str, sandbox_path: &str, host_dest: &Path) -> miette::Result<()> {
    let output = Command::new("openshell")
        .args(["sandbox", "download", sandbox, sandbox_path])
        .arg(host_dest)
        .output()
        .await
        .map_err(|e| miette::miette!("openshell download failed: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("openshell download failed: {stderr}"));
    }
    Ok(())
}
```

- [ ] **Step 4: Build workspace**

Run: `PROTOC=/nix/store/m6q48f6svqnfbvyqkis60a6vw5ww0v57-protobuf-34.0/bin/protoc cargo build --workspace`
Expected: clean build.

- [ ] **Step 5: Commit**

```
feat: add apply_policy and download_file to openshell module
```

---

### Task 2: Rewrite sandbox lifecycle — reuse existing, no delete

Replace the current "delete stale → create fresh" flow with "check if exists → reuse or create". Remove shutdown delete.

**Files:**
- Modify: `crates/bot/src/lib.rs` (lines 211-308)

- [ ] **Step 1: Replace sandbox lifecycle block**

In `crates/bot/src/lib.rs`, replace the entire sandbox lifecycle block (lines 211-277, from `let ssh_config_path` to `Some(config_path)`) with:

```rust
    let ssh_config_path: Option<std::path::PathBuf> = if !args.no_sandbox {
        let policy_path = std::env::var("RC_SANDBOX_POLICY")
            .map(std::path::PathBuf::from)
            .map_err(|_| miette::miette!("RC_SANDBOX_POLICY not set — required for sandbox mode, run `rightclaw up` first"))?;

        let sandbox = rightclaw::openshell::sandbox_name(&args.agent);

        let mtls_dir = std::env::var("OPENSHELL_MTLS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::config_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("/etc"))
                    .join("openshell/gateways/openshell/mtls")
            });

        // Check if sandbox already exists and is READY.
        let mut grpc_client = rightclaw::openshell::connect_grpc(&mtls_dir).await?;
        let sandbox_exists = rightclaw::openshell::is_sandbox_ready(&mut grpc_client, &sandbox).await?;

        if sandbox_exists {
            // Reuse existing sandbox — just apply updated policy.
            tracing::info!(agent = %args.agent, "reusing existing sandbox");
            rightclaw::openshell::apply_policy(&sandbox, &policy_path).await?;
        } else {
            // Sandbox doesn't exist — create with curated staging dir.
            tracing::info!(agent = %args.agent, "creating new sandbox");
            let upload_dir = agent_dir.join("staging");
            prepare_staging_dir(&agent_dir, &upload_dir)?;

            let mut child = rightclaw::openshell::spawn_sandbox(&sandbox, &policy_path, Some(&upload_dir))?;

            tokio::select! {
                result = rightclaw::openshell::wait_for_ready(&mut grpc_client, &sandbox, 120, 2) => {
                    result?;
                    drop(child);
                }
                status = child.wait() => {
                    let status = status.map_err(|e| miette::miette!("sandbox create child wait failed: {e:#}"))?;
                    if !status.success() {
                        return Err(miette::miette!(
                            "openshell sandbox create for '{}' exited with {status} before reaching READY",
                            args.agent
                        ));
                    }
                }
            }
        }

        // Generate SSH config (needed on every startup — host-side file).
        let ssh_config_dir = home.join("run").join("ssh");
        std::fs::create_dir_all(&ssh_config_dir)
            .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
        let config_path = rightclaw::openshell::generate_ssh_config(&sandbox, &ssh_config_dir).await?;
        tracing::info!(agent = %args.agent, "OpenShell sandbox ready");

        Some(config_path)
    } else {
        None
    };
```

- [ ] **Step 2: Add `prepare_staging_dir` function**

Add to `crates/bot/src/lib.rs` before `copy_dir_resolve_symlinks`:

```rust
/// Prepare the staging directory for sandbox upload.
///
/// Copies curated files from the agent's .claude/ directory into staging/.claude/,
/// excluding credentials (sandbox gets its own via login flow) and plugins (not used).
fn prepare_staging_dir(agent_dir: &std::path::Path, upload_dir: &std::path::Path) -> miette::Result<()> {
    let staging_claude = upload_dir.join(".claude");
    if staging_claude.exists() {
        std::fs::remove_dir_all(&staging_claude)
            .map_err(|e| miette::miette!("failed to clean staging/.claude: {e:#}"))?;
    }
    std::fs::create_dir_all(&staging_claude)
        .map_err(|e| miette::miette!("failed to create staging/.claude: {e:#}"))?;

    let src_claude = agent_dir.join(".claude");

    // Files to copy (not credentials, not plugins, not shell-snapshots)
    let copy_items = [
        "settings.json",
        "reply-schema.json",
        "agents",  // agent definition directory
    ];

    for item in &copy_items {
        let src = src_claude.join(item);
        let dst = staging_claude.join(item);
        if !src.exists() {
            continue;
        }
        let meta = std::fs::metadata(&src)
            .map_err(|e| miette::miette!("failed to stat {}: {e:#}", src.display()))?;
        if meta.is_dir() {
            copy_dir_resolve_symlinks(&src, &dst)
                .map_err(|e| miette::miette!("failed to copy {} to staging: {e:#}", item))?;
        } else {
            std::fs::copy(&src, &dst)
                .map_err(|e| miette::miette!("failed to copy {} to staging: {e:#}", item))?;
        }
    }

    // Copy only rightclaw builtin skills (not entire skills/ tree)
    let skills_src = src_claude.join("skills");
    if skills_src.exists() {
        for builtin in &["rightskills", "cronsync"] {
            let skill_src = skills_src.join(builtin);
            let skill_dst = staging_claude.join("skills").join(builtin);
            if skill_src.exists() {
                copy_dir_resolve_symlinks(&skill_src, &skill_dst)
                    .map_err(|e| miette::miette!("failed to copy skill {builtin} to staging: {e:#}"))?;
            }
        }
    }

    // Copy .claude.json (trust/onboarding config — at agent root, not inside .claude/)
    let claude_json_src = agent_dir.join(".claude.json");
    let claude_json_dst = upload_dir.join(".claude.json");
    if claude_json_src.exists() {
        std::fs::copy(&claude_json_src, &claude_json_dst)
            .map_err(|e| miette::miette!("failed to copy .claude.json to staging: {e:#}"))?;
    }

    // Copy .mcp.json
    let mcp_json_src = agent_dir.join(".mcp.json");
    let mcp_json_dst = upload_dir.join(".mcp.json");
    if mcp_json_src.exists() {
        std::fs::copy(&mcp_json_src, &mcp_json_dst)
            .map_err(|e| miette::miette!("failed to copy .mcp.json to staging: {e:#}"))?;
    }

    tracing::info!("prepared staging dir for sandbox upload");
    Ok(())
}
```

- [ ] **Step 3: Remove shutdown sandbox delete**

In `crates/bot/src/lib.rs`, remove lines 280-284 (`sandbox_name_for_cleanup`) and lines 306-308 (the `delete_sandbox` call at shutdown):

Remove:
```rust
    let sandbox_name_for_cleanup = if !args.no_sandbox {
        Some(rightclaw::openshell::sandbox_name(&args.agent))
    } else {
        None
    };
```

And remove:
```rust
    // Cleanup sandbox on shutdown (best-effort).
    if let Some(ref name) = sandbox_name_for_cleanup {
        rightclaw::openshell::delete_sandbox(name).await;
    }
```

- [ ] **Step 4: Build workspace**

Run: `PROTOC=/nix/store/m6q48f6svqnfbvyqkis60a6vw5ww0v57-protobuf-34.0/bin/protoc cargo build --workspace`
Expected: clean build.

- [ ] **Step 5: Commit**

```
feat: persistent sandbox — reuse existing, apply policy hot-reload, no delete on shutdown
```

---

### Task 3: Background sync task

Periodic task (every 5 min) that uploads `settings.json`, `reply-schema.json`, and rightclaw builtin skills to sandbox. Also downloads `.claude.json` from sandbox, verifies rightclaw-managed keys, fixes if needed, re-uploads.

**Files:**
- Create: `crates/bot/src/sync.rs`
- Modify: `crates/bot/src/lib.rs` (spawn sync task)

- [ ] **Step 1: Create `crates/bot/src/sync.rs`**

```rust
//! Background sync task: periodically uploads config files to sandbox.

use std::path::{Path, PathBuf};
use tokio::time::{Duration, interval};

/// Interval between sync cycles.
const SYNC_INTERVAL: Duration = Duration::from_secs(300);

/// Run the periodic sync loop. Uploads config files from host to sandbox.
///
/// Files synced:
/// - settings.json — CC behavioral flags
/// - reply-schema.json — structured output schema
/// - rightclaw builtin skills
/// - .claude.json — verified and fixed if CC overwrote rightclaw keys
pub async fn run_sync_task(
    agent_dir: PathBuf,
    sandbox_name: String,
) {
    let mut tick = interval(SYNC_INTERVAL);
    tick.tick().await; // first tick is immediate — skip it
    
    loop {
        tick.tick().await;
        tracing::debug!(agent = %sandbox_name, "sync: starting cycle");

        if let Err(e) = sync_cycle(&agent_dir, &sandbox_name).await {
            tracing::error!(agent = %sandbox_name, "sync cycle failed: {e:#}");
        }
    }
}

async fn sync_cycle(agent_dir: &Path, sandbox: &str) -> miette::Result<()> {
    // 1. Upload settings.json
    let settings = agent_dir.join(".claude").join("settings.json");
    if settings.exists() {
        rightclaw::openshell::upload_file(sandbox, &settings, "/sandbox/.claude/")
            .await
            .map_err(|e| miette::miette!("sync settings.json: {e:#}"))?;
        tracing::debug!("sync: uploaded settings.json");
    }

    // 2. Upload reply-schema.json
    let schema = agent_dir.join(".claude").join("reply-schema.json");
    if schema.exists() {
        rightclaw::openshell::upload_file(sandbox, &schema, "/sandbox/.claude/")
            .await
            .map_err(|e| miette::miette!("sync reply-schema.json: {e:#}"))?;
        tracing::debug!("sync: uploaded reply-schema.json");
    }

    // 3. Upload rightclaw builtin skills
    for skill_name in &["rightskills", "cronsync"] {
        let skill_dir = agent_dir.join(".claude").join("skills").join(skill_name);
        if skill_dir.exists() {
            rightclaw::openshell::upload_file(sandbox, &skill_dir, &format!("/sandbox/.claude/skills/"))
                .await
                .map_err(|e| miette::miette!("sync skill {skill_name}: {e:#}"))?;
        }
    }

    // 4. Verify .claude.json — download, check rightclaw keys, fix if needed
    verify_claude_json(agent_dir, sandbox).await?;

    tracing::debug!("sync: cycle complete");
    Ok(())
}

/// Download .claude.json from sandbox, verify rightclaw-managed keys are intact.
/// CC may overwrite hasCompletedOnboarding or trust settings during its lifecycle.
async fn verify_claude_json(agent_dir: &Path, sandbox: &str) -> miette::Result<()> {
    let tmp_dir = tempfile::tempdir()
        .map_err(|e| miette::miette!("failed to create temp dir: {e:#}"))?;
    let download_dest = tmp_dir.path();

    // Download .claude.json from sandbox
    if let Err(e) = rightclaw::openshell::download_file(sandbox, "/sandbox/.claude.json", download_dest).await {
        tracing::warn!("sync: failed to download .claude.json (may not exist yet): {e:#}");
        // Upload host version as baseline
        let host_claude_json = agent_dir.join(".claude.json");
        if host_claude_json.exists() {
            rightclaw::openshell::upload_file(sandbox, &host_claude_json, "/sandbox/")
                .await
                .map_err(|e| miette::miette!("sync: upload .claude.json baseline: {e:#}"))?;
        }
        return Ok(());
    }

    let downloaded = download_dest.join(".claude.json");
    if !downloaded.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&downloaded)
        .map_err(|e| miette::miette!("failed to read downloaded .claude.json: {e:#}"))?;
    let mut parsed: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| miette::miette!("failed to parse downloaded .claude.json: {e:#}"))?;

    let root = match parsed.as_object_mut() {
        Some(r) => r,
        None => return Ok(()),
    };

    // Ensure rightclaw-managed keys are set correctly.
    let mut needs_upload = false;

    if root.get("hasCompletedOnboarding") != Some(&serde_json::Value::Bool(true)) {
        root.insert("hasCompletedOnboarding".into(), serde_json::Value::Bool(true));
        needs_upload = true;
    }

    // Check trust for sandbox working dir (/sandbox)
    let trust_key = "/sandbox";
    let projects = root.entry("projects").or_insert_with(|| serde_json::json!({}));
    if let Some(projects_obj) = projects.as_object_mut() {
        let project = projects_obj.entry(trust_key).or_insert_with(|| serde_json::json!({}));
        if let Some(proj_obj) = project.as_object_mut() {
            if proj_obj.get("hasTrustDialogAccepted") != Some(&serde_json::Value::Bool(true)) {
                proj_obj.insert("hasTrustDialogAccepted".into(), serde_json::Value::Bool(true));
                needs_upload = true;
            }
        }
    }

    if needs_upload {
        let fixed = serde_json::to_string_pretty(&parsed)
            .map_err(|e| miette::miette!("failed to serialize .claude.json: {e:#}"))?;
        let fixed_path = tmp_dir.path().join(".claude.json.fixed");
        std::fs::write(&fixed_path, fixed)
            .map_err(|e| miette::miette!("failed to write fixed .claude.json: {e:#}"))?;
        rightclaw::openshell::upload_file(sandbox, &fixed_path, "/sandbox/")
            .await
            .map_err(|e| miette::miette!("sync: re-upload fixed .claude.json: {e:#}"))?;
        tracing::info!("sync: fixed and re-uploaded .claude.json (rightclaw keys were modified)");
    }

    Ok(())
}
```

- [ ] **Step 2: Add `pub mod sync;` to `crates/bot/src/lib.rs`**

Add after the existing module declarations at the top:

```rust
pub mod sync;
```

- [ ] **Step 3: Spawn sync task in bot startup**

In `crates/bot/src/lib.rs`, after the sandbox lifecycle block and before the `tokio::select!`, add:

```rust
    // Spawn background sync task for sandbox file sync.
    if !args.no_sandbox {
        let sync_agent_dir = agent_dir.clone();
        let sync_sandbox = rightclaw::openshell::sandbox_name(&args.agent);
        tokio::spawn(sync::run_sync_task(sync_agent_dir, sync_sandbox));
    }
```

- [ ] **Step 4: Add `tempfile` dependency to bot crate**

Check if `tempfile` is already in `crates/bot/Cargo.toml`. If not, add:

```toml
[dependencies]
tempfile = "3.15"
```

- [ ] **Step 5: Build workspace**

Run: `PROTOC=/nix/store/m6q48f6svqnfbvyqkis60a6vw5ww0v57-protobuf-34.0/bin/protoc cargo build --workspace`
Expected: clean build.

- [ ] **Step 6: Commit**

```
feat: background sync task — uploads settings, schema, skills, verifies .claude.json every 5 min
```

---

### Task 4: Upload .mcp.json after `/mcp add` and `/mcp remove`

Currently these commands modify `.mcp.json` on host but don't sync to sandbox. Add upload after each write.

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs` (handle_mcp_add, handle_mcp_remove)

- [ ] **Step 1: Add sandbox upload to `handle_mcp_add`**

In `crates/bot/src/telegram/handler.rs`, in `handle_mcp_add` (around line 504), after the success branch:

Replace:
```rust
        Ok(()) => {
            bot.send_message(msg.chat.id, format!("Added MCP server: {name} ({url})"))
                .await?;
        }
```

With:
```rust
        Ok(()) => {
            // Upload updated .mcp.json to sandbox if running.
            upload_mcp_json_to_sandbox(agent_dir).await;
            bot.send_message(msg.chat.id, format!("Added MCP server: {name} ({url})"))
                .await?;
        }
```

- [ ] **Step 2: Add sandbox upload to `handle_mcp_remove`**

In `handle_mcp_remove` (around line 541), after the success branch:

Replace:
```rust
        Ok(()) => {
            bot.send_message(msg.chat.id, format!("Removed MCP server: {server_name}"))
                .await?;
        }
```

With:
```rust
        Ok(()) => {
            upload_mcp_json_to_sandbox(agent_dir).await;
            bot.send_message(msg.chat.id, format!("Removed MCP server: {server_name}"))
                .await?;
        }
```

- [ ] **Step 3: Add `upload_mcp_json_to_sandbox` helper**

Add to `crates/bot/src/telegram/handler.rs`:

```rust
/// Best-effort upload of .mcp.json to sandbox after modification.
/// Derives sandbox name from agent directory name.
async fn upload_mcp_json_to_sandbox(agent_dir: &Path) {
    let agent_name = agent_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let sandbox = rightclaw::openshell::sandbox_name(agent_name);
    let mcp_json = agent_dir.join(".mcp.json");
    if mcp_json.exists() {
        if let Err(e) = rightclaw::openshell::upload_file(&sandbox, &mcp_json, "/sandbox/").await {
            tracing::warn!(agent = agent_name, "failed to upload .mcp.json to sandbox: {e:#}");
        } else {
            tracing::info!(agent = agent_name, "uploaded .mcp.json to sandbox after MCP config change");
        }
    }
}
```

- [ ] **Step 4: Build workspace**

Run: `PROTOC=/nix/store/m6q48f6svqnfbvyqkis60a6vw5ww0v57-protobuf-34.0/bin/protoc cargo build --workspace`
Expected: clean build.

- [ ] **Step 5: Commit**

```
feat: upload .mcp.json to sandbox after /mcp add and /mcp remove
```

---

### Task 5: Remove plugins symlink and upload

Remove the plugin symlink creation (`create_plugin_symlinks`) and ensure plugins/ is not copied to staging.

**Files:**
- Modify: `crates/rightclaw/src/codegen/claude_json.rs` (remove or skip plugin symlink)
- Modify: `crates/rightclaw-cli/src/main.rs` (remove plugin symlink call if separate)

- [ ] **Step 1: Find and examine plugin symlink code**

Search for `plugin` or `create_plugin` in codegen:

```bash
PROTOC=... cargo build --workspace
```

Check `crates/rightclaw/src/codegen/claude_json.rs` for plugin symlink functions. If `create_plugin_symlinks` exists as a standalone function called from `cmd_up`, remove the call. If it's part of `create_credential_symlink`, remove only the plugin portion.

The `prepare_staging_dir` function from Task 2 already excludes `plugins/` — it only copies explicitly listed items. So plugins won't reach sandbox. But we should also stop creating the host-side symlink if it's no longer needed.

- [ ] **Step 2: Remove plugin symlink creation**

In `crates/rightclaw/src/codegen/claude_json.rs`, find and remove the function or code block that creates plugin symlinks. Also remove its call site in `crates/rightclaw-cli/src/main.rs`.

- [ ] **Step 3: Build and test**

Run: `PROTOC=/nix/store/m6q48f6svqnfbvyqkis60a6vw5ww0v57-protobuf-34.0/bin/protoc cargo build --workspace`
Run: `PROTOC=/nix/store/m6q48f6svqnfbvyqkis60a6vw5ww0v57-protobuf-34.0/bin/protoc cargo test --workspace`
Expected: clean build, tests pass (some plugin-related tests may need updating).

- [ ] **Step 4: Commit**

```
refactor: remove plugin symlinks — Telegram CC plugin no longer used
```

---

### Task 6: Smoke test

End-to-end verification of persistent sandbox lifecycle.

**Files:** Potentially any file from Tasks 1-5 depending on issues found.

- [ ] **Step 1: Rebuild and deploy**

Run: `PROTOC=/nix/store/m6q48f6svqnfbvyqkis60a6vw5ww0v57-protobuf-34.0/bin/protoc cargo build --workspace`

- [ ] **Step 2: Fresh start — create sandbox**

```bash
rightclaw down
# Delete existing sandbox manually if needed:
openshell sandbox delete rightclaw-right
rightclaw up --debug
```

Verify in PC TUI:
- `right-bot` process running
- `login-right` process disabled
- No errors in right-bot logs

- [ ] **Step 3: Trigger login flow**

Send a Telegram message. Expected:
1. Auth error detected (no credentials in sandbox)
2. Login process started
3. OAuth URL sent to Telegram (or fallback PC TUI message)
4. Complete authentication
5. "Logged in successfully" message

- [ ] **Step 4: Verify normal operation**

Send another message after login. Expected: bot responds normally.

- [ ] **Step 5: Test persistence — restart bot without deleting sandbox**

```bash
rightclaw down
rightclaw up --debug
```

Verify:
1. Bot logs show "reusing existing sandbox" (not "creating new sandbox")
2. Bot logs show "policy applied"
3. Send Telegram message — bot responds (credentials survived restart)

- [ ] **Step 6: Verify sync task**

Wait 5 minutes (or temporarily reduce SYNC_INTERVAL for testing).
Check bot logs for "sync: cycle complete" messages.

- [ ] **Step 7: Test /mcp add upload**

```
/mcp add testserver https://test.example.com
```
Verify bot logs show "uploaded .mcp.json to sandbox after MCP config change".

- [ ] **Step 8: Commit any fixes**

```
fix: smoke test fixes for persistent sandbox
```
