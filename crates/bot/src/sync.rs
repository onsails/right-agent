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
pub async fn initial_sync(
    agent_dir: &Path,
    sbox: &rightclaw::sandbox_exec::SandboxExec,
) -> miette::Result<()> {
    tracing::info!(sandbox = sbox.sandbox_name(), "sync: initial cycle (blocking)");
    sync_cycle(agent_dir, sbox).await?;

    // Ensure /sandbox/.local/bin is in PATH for agent-installed CLI tools.
    ensure_local_bin_in_path(sbox).await?;

    Ok(())
}

/// Run the periodic sync loop (spawned as background task after initial_sync).
pub async fn run_sync_task(
    agent_dir: PathBuf,
    sbox: rightclaw::sandbox_exec::SandboxExec,
    shutdown: CancellationToken,
) {
    let mut tick = interval(SYNC_INTERVAL);
    tick.tick().await; // consume immediate tick

    loop {
        tokio::select! {
            _ = tick.tick() => {
                tracing::debug!(sandbox = %sbox.sandbox_name(), "sync: starting cycle");

                if let Err(e) = sync_cycle(&agent_dir, &sbox).await {
                    tracing::error!(sandbox = %sbox.sandbox_name(), "sync cycle failed: {e:#}");
                }
            }
            _ = shutdown.cancelled() => {
                tracing::info!(sandbox = %sbox.sandbox_name(), "sync task shutting down");
                break;
            }
        }
    }
}

async fn sync_cycle(
    agent_dir: &Path,
    sbox: &rightclaw::sandbox_exec::SandboxExec,
) -> miette::Result<()> {
    // Build manifest of platform-managed files
    let manifest = rightclaw::platform_store::build_manifest(agent_dir)?;

    // Deploy to /platform/ with content-addressed names + symlinks
    rightclaw::platform_store::deploy_manifest(sbox, &manifest).await?;

    // Verify .claude.json (separate flow — not content-addressed)
    verify_claude_json(agent_dir, sbox.sandbox_name()).await?;

    tracing::debug!("sync: cycle complete");
    Ok(())
}

/// Files that CC creates/modifies inside the sandbox and should be synced back to host.
/// Excludes codegen-only files (BOOTSTRAP.md) — those are uploaded by
/// forward sync and never modified by CC.
const REVERSE_SYNC_FILES: &[&str] = &[
    "AGENTS.md",
    "TOOLS.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
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

/// Ensure `/sandbox/.local/bin` is in PATH via `.bashrc`.
///
/// Agents install CLI tools (gh extensions, etc.) to `$HOME/.local/bin` which maps
/// to `/sandbox/.local/bin`. This is already writable, but not in PATH by default.
async fn ensure_local_bin_in_path(
    sbox: &rightclaw::sandbox_exec::SandboxExec,
) -> miette::Result<()> {
    let (bashrc, code) = sbox.exec(&["cat", "/sandbox/.bashrc"]).await?;

    if code != 0 || !bashrc.contains("/sandbox/.local/bin") {
        // Prepend to PATH in .bashrc
        sbox.exec(&[
            "sed",
            "-i",
            r#"s|export PATH="/sandbox/.venv/bin:|export PATH="/sandbox/.local/bin:/sandbox/.venv/bin:|"#,
            "/sandbox/.bashrc",
        ])
        .await?;
        tracing::info!("sync: added /sandbox/.local/bin to PATH in .bashrc");
    }

    // Ensure the directory exists
    sbox.exec(&["mkdir", "-p", "/sandbox/.local/bin"]).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that initial_sync does NOT upload agent-managed .md files to sandbox.
    ///
    /// Creates an ephemeral sandbox, writes .md files on host, runs initial_sync,
    /// then verifies those files are absent in sandbox.
    ///
    /// Requires: running OpenShell gateway.
    #[tokio::test]
    async fn initial_sync_does_not_upload_agent_md_files() {
        let _slot = rightclaw::openshell::acquire_sandbox_slot();
        let sandbox_name = "rightclaw-test-sync-upload";

        rightclaw::test_cleanup::pkill_test_orphans(sandbox_name);
        rightclaw::test_cleanup::register_test_sandbox(sandbox_name);

        let mtls_dir = match rightclaw::openshell::preflight_check() {
            rightclaw::openshell::OpenShellStatus::Ready(dir) => dir,
            other => panic!("OpenShell not ready: {other:?}"),
        };

        // Clean up leftover from a previous failed run.
        let mut grpc_client = rightclaw::openshell::connect_grpc(&mtls_dir)
            .await
            .expect("gRPC connect");
        if rightclaw::openshell::sandbox_exists(&mut grpc_client, sandbox_name)
            .await
            .unwrap()
        {
            rightclaw::openshell::delete_sandbox(sandbox_name).await;
            rightclaw::openshell::wait_for_deleted(&mut grpc_client, sandbox_name, 60, 2)
                .await
                .expect("cleanup of leftover sandbox failed");
        }

        // Create a fresh sandbox with minimal policy.
        let policy_dir = tempfile::tempdir().unwrap();
        let policy_path = policy_dir.path().join("policy.yaml");
        std::fs::write(
            &policy_path,
            "\
version: 1
filesystem_policy:
  include_workdir: true
  read_write:
    - /tmp
    - /sandbox
    - /platform
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
",
        )
        .unwrap();

        let mut child = rightclaw::openshell::spawn_sandbox(sandbox_name, &policy_path, None)
            .expect("failed to spawn sandbox");
        rightclaw::openshell::wait_for_ready(&mut grpc_client, sandbox_name, 120, 2)
            .await
            .expect("sandbox did not become READY");
        let _ = child.kill().await;

        let sandbox_id =
            rightclaw::openshell::resolve_sandbox_id(&mut grpc_client, sandbox_name)
                .await
                .expect("resolve sandbox_id");

        // Poll exec until it succeeds — OpenShell reports READY before exec transport is available.
        let sbox = rightclaw::sandbox_exec::SandboxExec::new(
            mtls_dir,
            sandbox_name.to_owned(),
            sandbox_id,
        );
        for attempt in 1..=20 {
            match sbox.exec(&["echo", "ready"]).await {
                Ok((out, 0)) if out.trim() == "ready" => break,
                _ if attempt == 20 => panic!("exec not ready after 20 attempts"),
                _ => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
            }
        }

        // Build a fake agent dir with known content.
        let agent_dir = tempfile::tempdir().unwrap();
        let root = agent_dir.path();

        // Write agent-managed .md files on host.
        let test_files: &[&str] = &["IDENTITY.md", "AGENTS.md", "TOOLS.md"];
        for &name in test_files {
            std::fs::write(root.join(name), format!("# test {name}\n")).unwrap();
        }

        // Minimal .claude/ infrastructure so initial_sync doesn't fail on missing files.
        let claude_dir = root.join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("settings.json"), "{}").unwrap();
        std::fs::write(claude_dir.join("reply-schema.json"), "{}").unwrap();
        std::fs::write(claude_dir.join("cron-schema.json"), "{}").unwrap();
        std::fs::write(claude_dir.join("bootstrap-schema.json"), "{}").unwrap();
        std::fs::write(
            claude_dir.join("system-prompt.md"),
            "# test system prompt\n",
        )
        .unwrap();
        std::fs::write(root.join("mcp.json"), "{}").unwrap();

        // Run initial_sync.
        initial_sync(root, &sbox)
            .await
            .expect("initial_sync should succeed");

        // Verify agent-managed .md files are NOT in sandbox.
        for &name in test_files {
            let sandbox_path = format!("/sandbox/{name}");
            let (_stdout, exit_code) = sbox
                .exec(&["test", "-f", &sandbox_path])
                .await
                .unwrap_or_else(|e| panic!("exec test -f {sandbox_path} failed: {e:#}"));
            assert_ne!(
                exit_code, 0,
                "{name} should NOT exist in sandbox after initial_sync"
            );
        }

        // Clean up.
        rightclaw::openshell::delete_sandbox(sandbox_name).await;
        rightclaw::test_cleanup::unregister_test_sandbox(sandbox_name);
    }
}
