//! Background sync task: periodically uploads config files to sandbox.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use tokio::time::{Duration, interval};
use tokio_util::sync::CancellationToken;

/// Interval between sync cycles.
const SYNC_INTERVAL: Duration = Duration::from_secs(300);

/// Run one sync cycle. Called synchronously at startup before teloxide starts,
/// ensuring sandbox has correct config before any `claude -p` invocations.
pub async fn initial_sync(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()> {
    tracing::info!(sandbox = sandbox_name, "sync: initial cycle (blocking)");
    sync_cycle(agent_dir, sandbox_name).await
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

    // 3. Upload mcp.json (right MCP server + external MCP servers with Bearer tokens)
    let mcp_json = agent_dir.join("mcp.json");
    if mcp_json.exists() {
        rightclaw::openshell::upload_file(sandbox, &mcp_json, "/sandbox/")
            .await
            .map_err(|e| miette::miette!("sync mcp.json: {e:#}"))?;
        tracing::debug!("sync: uploaded mcp.json");
    }

    // 4. Upload rightclaw builtin skills
    for skill_name in &["rightskills", "cronsync"] {
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
        let sandbox_path = format!("/sandbox/{filename}");
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
