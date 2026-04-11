//! Background sync task: periodically uploads config files to sandbox.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use tokio::time::{Duration, interval};
use tokio_util::sync::CancellationToken;

/// Interval between sync cycles.
const SYNC_INTERVAL: Duration = Duration::from_secs(300);

use rightclaw::codegen::CONTENT_MD_FILES;

/// Run one sync cycle. Called synchronously at startup before teloxide starts,
/// ensuring sandbox has correct config before any `claude -p` invocations.
pub async fn initial_sync(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()> {
    tracing::info!(sandbox = sandbox_name, "sync: initial cycle (blocking)");
    sync_cycle(agent_dir, sandbox_name).await?;

    // Upload content .md files into .claude/agents/ inside the sandbox.
    // CC resolves @./FILE.md relative to the agent def file location (.claude/agents/).
    // Only on startup — sandbox is source of truth after this point.
    for &filename in CONTENT_MD_FILES {
        let host_path = agent_dir.join(filename);
        if host_path.exists() {
            rightclaw::openshell::upload_file(sandbox_name, &host_path, "/sandbox/.claude/agents/")
                .await
                .map_err(|e| miette::miette!("sync {filename}: {e:#}"))?;
            tracing::debug!(file = filename, "sync: uploaded content file to .claude/agents/");
        }
    }

    Ok(())
}

/// Run the periodic sync loop (spawned as background task after initial_sync).
pub async fn run_sync_task(agent_dir: PathBuf, sandbox_name: String, shutdown: CancellationToken) {
    let mut tick = interval(SYNC_INTERVAL);
    tick.tick().await; // consume immediate tick

    loop {
        tokio::select! {
            _ = tick.tick() => {
                tracing::debug!(sandbox = %sandbox_name, "sync: starting cycle");

                if let Err(e) = sync_cycle(&agent_dir, &sandbox_name).await {
                    tracing::error!(sandbox = %sandbox_name, "sync cycle failed: {e:#}");
                }
            }
            _ = shutdown.cancelled() => {
                tracing::info!(sandbox = %sandbox_name, "sync task shutting down");
                break;
            }
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

    // 2a. Upload cron-schema.json
    let cron_schema = agent_dir.join(".claude").join("cron-schema.json");
    if cron_schema.exists() {
        rightclaw::openshell::upload_file(sandbox, &cron_schema, "/sandbox/.claude/")
            .await
            .map_err(|e| miette::miette!("sync cron-schema.json: {e:#}"))?;
        tracing::debug!("sync: uploaded cron-schema.json");
    }

    // 2b. Upload system-prompt.md (base identity for --system-prompt-file)
    let sys_prompt = agent_dir.join(".claude").join("system-prompt.md");
    if sys_prompt.exists() {
        rightclaw::openshell::upload_file(sandbox, &sys_prompt, "/sandbox/.claude/")
            .await
            .map_err(|e| miette::miette!("sync system-prompt.md: {e:#}"))?;
        tracing::debug!("sync: uploaded system-prompt.md");
    }

    // 2c. Upload bootstrap-schema.json
    let bs_schema = agent_dir.join(".claude").join("bootstrap-schema.json");
    if bs_schema.exists() {
        rightclaw::openshell::upload_file(sandbox, &bs_schema, "/sandbox/.claude/")
            .await
            .map_err(|e| miette::miette!("sync bootstrap-schema.json: {e:#}"))?;
        tracing::debug!("sync: uploaded bootstrap-schema.json");
    }

    // 2d. Upload .claude/agents/ directory (agent definitions with @ references)
    let agents_dir = agent_dir.join(".claude").join("agents");
    if agents_dir.exists() {
        rightclaw::openshell::upload_file(sandbox, &agents_dir, "/sandbox/.claude/")
            .await
            .map_err(|e| miette::miette!("sync .claude/agents/: {e:#}"))?;
        tracing::debug!("sync: uploaded .claude/agents/");
    }

    // 3. Upload mcp.json (right MCP server + external MCP servers with Bearer tokens)
    let mcp_json = agent_dir.join("mcp.json");
    if mcp_json.exists() {
        rightclaw::openshell::upload_file(sandbox, &mcp_json, "/sandbox/")
            .await
            .map_err(|e| miette::miette!("sync mcp.json: {e:#}"))?;
        tracing::debug!("sync: uploaded mcp.json");
    }

    // 4. Upload rightclaw builtin skills
    for skill_name in &["rightskills", "rightcron"] {
        let skill_dir = agent_dir.join(".claude").join("skills").join(skill_name);
        if skill_dir.exists() {
            rightclaw::openshell::upload_file(sandbox, &skill_dir, "/sandbox/.claude/skills/")
                .await
                .map_err(|e| miette::miette!("sync skill {skill_name}: {e:#}"))?;
        }
    }

    // 5. Verify .claude.json -- download, check rightclaw keys, fix if needed
    verify_claude_json(agent_dir, sandbox).await?;

    tracing::debug!("sync: cycle complete");
    Ok(())
}

/// Alias for reverse sync — same file list as forward sync.
const REVERSE_SYNC_FILES: &[&str] = CONTENT_MD_FILES;

/// Sync .md files from sandbox back to host after a `claude -p` invocation.
///
/// Downloads all files concurrently. Changed files are atomically written to host.
/// Failed downloads trigger host-side deletion (only when sandbox is confirmed reachable).
pub async fn reverse_sync_md(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()> {
    let tmp_dir = tempfile::tempdir()
        .map_err(|e| miette::miette!("reverse sync: failed to create temp dir: {e:#}"))?;

    // Download all files concurrently, each into its own subdirectory to avoid
    // file collisions if openshell uses non-atomic writes internally.
    let mut join_set = tokio::task::JoinSet::new();
    for &filename in REVERSE_SYNC_FILES {
        let sandbox = sandbox_name.to_owned();
        let sub_dir = tmp_dir.path().join(format!("dl-{filename}"));
        std::fs::create_dir(&sub_dir).map_err(|e| {
            miette::miette!("reverse sync: failed to create sub dir for {filename}: {e:#}")
        })?;
        let sandbox_path = format!("/sandbox/.claude/agents/{filename}");
        join_set.spawn(async move {
            let result =
                rightclaw::openshell::download_file(&sandbox, &sandbox_path, &sub_dir).await;
            (filename, sub_dir, result)
        });
    }

    let mut errors: Vec<String> = Vec::new();
    let mut any_download_ok = false;
    let mut pending_deletes: Vec<(&str, PathBuf)> = Vec::new();

    // Process results sequentially.
    while let Some(join_result) = join_set.join_next().await {
        let (filename, sub_dir, dl_result) = join_result
            .map_err(|e| miette::miette!("reverse sync: join error: {e:#}"))?;
        let host_path = agent_dir.join(filename);

        match dl_result {
            Ok(()) => {
                any_download_ok = true;
                let downloaded = sub_dir.join(filename);
                if !downloaded.exists() {
                    continue;
                }
                let new_content = match std::fs::read(&downloaded) {
                    Ok(c) => c,
                    Err(e) => {
                        errors.push(format!("{filename}: read downloaded failed: {e:#}"));
                        continue;
                    }
                };

                if host_path.exists()
                    && let Ok(existing) = std::fs::read(&host_path)
                    && existing == new_content
                {
                    tracing::debug!(file = filename, "reverse sync: unchanged, skipping");
                    continue;
                }

                match atomic_write_bytes(&host_path, &new_content) {
                    Ok(()) => {
                        tracing::info!(file = filename, "reverse sync: updated on host");
                    }
                    Err(e) => {
                        errors.push(format!("{filename}: atomic write failed: {e:#}"));
                    }
                }
            }
            Err(e) => {
                tracing::debug!(file = filename, "reverse sync: download failed: {e:#}");
                // Defer deletion — only safe if at least one download succeeded
                // (proves sandbox is reachable, so failure = file genuinely absent).
                if host_path.exists() {
                    pending_deletes.push((filename, host_path));
                }
            }
        }
    }

    // Only apply deletions when at least one file downloaded successfully.
    // This prevents wiping host files when the sandbox is unreachable.
    if any_download_ok {
        for (filename, host_path) in pending_deletes {
            if let Err(e) = std::fs::remove_file(&host_path) {
                errors.push(format!("{filename}: host delete failed: {e:#}"));
            } else {
                tracing::info!(file = filename, "reverse sync: deleted from host (absent in sandbox)");
            }
        }
    } else if !pending_deletes.is_empty() {
        tracing::warn!(
            "reverse sync: all downloads failed — skipping {} pending deletion(s) (sandbox may be unreachable)",
            pending_deletes.len()
        );
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

/// Download .claude.json from sandbox, verify rightclaw-managed keys are intact.
/// CC may overwrite hasCompletedOnboarding or trust settings during its lifecycle.
async fn verify_claude_json(agent_dir: &Path, sandbox: &str) -> miette::Result<()> {
    let tmp_dir = tempfile::tempdir()
        .map_err(|e| miette::miette!("failed to create temp dir: {e:#}"))?;
    let download_dest = tmp_dir.path();

    // Download .claude.json from sandbox
    if let Err(e) =
        rightclaw::openshell::download_file(sandbox, "/sandbox/.claude.json", download_dest).await
    {
        tracing::warn!(
            "sync: failed to download .claude.json (may not exist yet): {e:#}"
        );
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

    let mut needs_upload = false;

    if root.get("hasCompletedOnboarding") != Some(&serde_json::Value::Bool(true)) {
        root.insert(
            "hasCompletedOnboarding".into(),
            serde_json::Value::Bool(true),
        );
        needs_upload = true;
    }

    // Check trust for sandbox working dir (/sandbox)
    let projects = root
        .entry("projects")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(projects_obj) = projects.as_object_mut() {
        let project = projects_obj
            .entry("/sandbox")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(proj_obj) = project.as_object_mut()
            && proj_obj.get("hasTrustDialogAccepted")
                != Some(&serde_json::Value::Bool(true))
        {
            proj_obj.insert(
                "hasTrustDialogAccepted".into(),
                serde_json::Value::Bool(true),
            );
            needs_upload = true;
        }
    }

    if needs_upload {
        let fixed = serde_json::to_string_pretty(&parsed)
            .map_err(|e| miette::miette!("failed to serialize .claude.json: {e:#}"))?;
        let fixed_path = tmp_dir.path().join(".claude.json");
        std::fs::write(&fixed_path, &fixed)
            .map_err(|e| miette::miette!("failed to write fixed .claude.json: {e:#}"))?;
        rightclaw::openshell::upload_file(sandbox, &fixed_path, "/sandbox/")
            .await
            .map_err(|e| miette::miette!("sync: re-upload fixed .claude.json: {e:#}"))?;
        tracing::info!("sync: fixed and re-uploaded .claude.json (rightclaw keys were modified)");
    }

    Ok(())
}

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
        std::fs::write(claude_dir.join("cron-schema.json"), "{}").unwrap();
        std::fs::write(claude_dir.join("bootstrap-schema.json"), "{}").unwrap();
        std::fs::write(claude_dir.join("system-prompt.md"), "# test system prompt\n").unwrap();
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

        // Note: test files left in sandbox — overwritten by next real initial_sync.
    }
}
