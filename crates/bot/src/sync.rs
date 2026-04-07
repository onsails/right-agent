//! Background sync task: periodically uploads config files to sandbox.

use std::path::{Path, PathBuf};
use tokio::time::{Duration, interval};

/// Interval between sync cycles.
const SYNC_INTERVAL: Duration = Duration::from_secs(300);

/// Run the periodic sync loop. Uploads config files from host to sandbox.
///
/// Files synced:
/// - settings.json -- CC behavioral flags
/// - reply-schema.json -- structured output schema
/// - rightclaw builtin skills
/// - .claude.json -- verified and fixed if CC overwrote rightclaw keys
pub async fn run_sync_task(agent_dir: PathBuf, sandbox_name: String) {
    let mut tick = interval(SYNC_INTERVAL);
    tick.tick().await; // first tick is immediate -- skip it

    loop {
        tick.tick().await;
        tracing::debug!(sandbox = %sandbox_name, "sync: starting cycle");

        if let Err(e) = sync_cycle(&agent_dir, &sandbox_name).await {
            tracing::error!(sandbox = %sandbox_name, "sync cycle failed: {e:#}");
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
            rightclaw::openshell::upload_file(sandbox, &skill_dir, "/sandbox/.claude/skills/")
                .await
                .map_err(|e| miette::miette!("sync skill {skill_name}: {e:#}"))?;
        }
    }

    // 4. Verify .claude.json -- download, check rightclaw keys, fix if needed
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
        let fixed_path = tmp_dir.path().join(".claude.json.fixed");
        std::fs::write(&fixed_path, &fixed)
            .map_err(|e| miette::miette!("failed to write fixed .claude.json: {e:#}"))?;
        rightclaw::openshell::upload_file(sandbox, &fixed_path, "/sandbox/")
            .await
            .map_err(|e| miette::miette!("sync: re-upload fixed .claude.json: {e:#}"))?;
        tracing::info!("sync: fixed and re-uploaded .claude.json (rightclaw keys were modified)");
    }

    Ok(())
}
