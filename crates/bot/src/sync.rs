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
pub(crate) async fn initial_sync(
    agent_dir: &Path,
    sbox: &right_openshell::sandbox_exec::SandboxExec,
) -> miette::Result<()> {
    tracing::info!(
        sandbox = sbox.sandbox_name(),
        "sync: initial cycle (blocking)"
    );
    sync_cycle(agent_dir, sbox).await?;

    // One-shot migration: clear legacy built-in skill paths from the sandbox.
    // Kept out of `sync_cycle` so it does not re-exec every 5 minutes forever.
    cleanup_obsolete_builtin_skill_paths(sbox).await?;

    // Ensure user-local CLI/npm environment is configured before any claude invocation.
    ensure_sandbox_user_local_env(sbox).await?;

    Ok(())
}

/// Run the periodic sync loop (spawned as background task after initial_sync).
pub(crate) async fn run_sync_task(
    agent_dir: PathBuf,
    sbox: right_openshell::sandbox_exec::SandboxExec,
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

pub(crate) async fn sync_cycle(
    agent_dir: &Path,
    sbox: &right_openshell::sandbox_exec::SandboxExec,
) -> miette::Result<()> {
    // Build manifest of platform-managed files
    let manifest =
        right_platform_store::build_manifest(agent_dir, right_codegen::BUILTIN_SKILL_NAMES)?;

    // Deploy to /platform/ with content-addressed names + symlinks
    right_platform_store::deploy_manifest(sbox, &manifest).await?;

    // Verify .claude.json (separate flow — not content-addressed)
    verify_claude_json(agent_dir, sbox.sandbox_name()).await?;

    tracing::debug!("sync: cycle complete");
    Ok(())
}

fn obsolete_builtin_skill_paths() -> Vec<String> {
    right_codegen::BUILTIN_SKILL_LEGACY_NAMES
        .iter()
        .map(|legacy_name| format!("/sandbox/.claude/skills/{legacy_name}"))
        .collect()
}

async fn cleanup_obsolete_builtin_skill_paths(
    sbox: &right_openshell::sandbox_exec::SandboxExec,
) -> miette::Result<()> {
    let paths = obsolete_builtin_skill_paths();
    let mut args = vec!["rm", "-rf"];
    args.extend(paths.iter().map(String::as_str));

    let (output, code) = sbox.exec(&args).await?;
    if code != 0 {
        return Err(obsolete_builtin_skill_cleanup_error(
            sbox.sandbox_name(),
            &paths,
            &output,
            code,
        ));
    }

    tracing::debug!(
        sandbox = %sbox.sandbox_name(),
        count = paths.len(),
        "sync: removed obsolete builtin skill paths"
    );
    Ok(())
}

fn obsolete_builtin_skill_cleanup_error(
    sandbox_name: &str,
    paths: &[String],
    output: &str,
    code: i32,
) -> miette::Report {
    let output = if output.trim().is_empty() {
        "<empty>"
    } else {
        output.trim()
    };
    miette::miette!(
        "sync: failed to remove obsolete builtin skill paths in sandbox {sandbox_name}: \
         rm exited with {code}; paths={paths:?}; output={output}"
    )
}

/// Files that CC creates/modifies inside the sandbox and must be mirrored to host.
///
/// This intentionally excludes TOOLS.md. Sandboxed prompt assembly reads
/// `/sandbox/TOOLS.md` directly, and current host-side consumers require only
/// the identity mirror files.
fn reverse_sync_files() -> &'static [&'static str] {
    &right_agent::identity_mirror::IDENTITY_MIRROR_FILES
}

/// Sync .md files from sandbox back to host after a `claude -p` invocation.
///
/// Downloads all files concurrently. Changed files are atomically written to host.
/// Failed downloads trigger host-side deletion (only when sandbox is confirmed reachable).
pub(crate) async fn reverse_sync_md(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()> {
    let tmp_dir = tempfile::tempdir()
        .map_err(|e| miette::miette!("reverse sync: failed to create temp dir: {e:#}"))?;

    // Download all files concurrently. Each job gets a unique dest path so
    // openshell can never clash on temp files.
    let mut join_set = tokio::task::JoinSet::new();
    for &filename in reverse_sync_files() {
        let sandbox = sandbox_name.to_owned();
        let dl_path = tmp_dir.path().join(format!("dl-{filename}"));
        let sandbox_path = format!("/sandbox/{filename}");
        join_set.spawn(async move {
            let result =
                right_openshell::openshell::download_file(&sandbox, &sandbox_path, &dl_path).await;
            (filename, dl_path, result)
        });
    }

    let mut errors: Vec<String> = Vec::new();
    let mut any_download_ok = false;
    let mut pending_deletes: Vec<(&str, PathBuf)> = Vec::new();

    // Process results sequentially.
    while let Some(join_result) = join_set.join_next().await {
        let (filename, downloaded, dl_result) =
            join_result.map_err(|e| miette::miette!("reverse sync: join error: {e:#}"))?;
        let host_path = agent_dir.join(filename);

        match dl_result {
            Ok(()) => {
                any_download_ok = true;
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
                tracing::info!(
                    file = filename,
                    "reverse sync: deleted from host (absent in sandbox)"
                );
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

/// Download .claude.json from sandbox, verify right-agent-managed keys are intact.
/// CC may overwrite hasCompletedOnboarding or trust settings during its lifecycle.
async fn verify_claude_json(agent_dir: &Path, sandbox: &str) -> miette::Result<()> {
    let tmp_dir =
        tempfile::tempdir().map_err(|e| miette::miette!("failed to create temp dir: {e:#}"))?;
    let downloaded = tmp_dir.path().join(".claude.json");

    // Download .claude.json from sandbox
    if let Err(e) =
        right_openshell::openshell::download_file(sandbox, "/sandbox/.claude.json", &downloaded)
            .await
    {
        tracing::warn!("sync: failed to download .claude.json (may not exist yet): {e:#}");
        // Upload host version as baseline
        let host_claude_json = agent_dir.join(".claude.json");
        if host_claude_json.exists() {
            right_openshell::openshell::upload_file(sandbox, &host_claude_json, "/sandbox/")
                .await
                .map_err(|e| miette::miette!("sync: upload .claude.json baseline: {e:#}"))?;
        }
        return Ok(());
    }

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
            && proj_obj.get("hasTrustDialogAccepted") != Some(&serde_json::Value::Bool(true))
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
        right_openshell::openshell::upload_file(sandbox, &fixed_path, "/sandbox/")
            .await
            .map_err(|e| miette::miette!("sync: re-upload fixed .claude.json: {e:#}"))?;
        tracing::info!("sync: fixed and re-uploaded .claude.json (right-agent keys were modified)");
    }

    Ok(())
}

const SANDBOX_ENV_PATH: &str = "/sandbox/.right/env.sh";
const SANDBOX_ENV_DIR: &str = "/sandbox/.right";
const SANDBOX_LOCAL_BIN: &str = "/sandbox/.local/bin";
const SANDBOX_NPM_CACHE: &str = "/sandbox/.npm";

fn sandbox_env_file_content() -> String {
    format!(
        r#"# Generated by Right Agent. Do not edit.
export RIGHT_AGENT_MANAGED_ENV=1
mkdir -p {SANDBOX_LOCAL_BIN} {SANDBOX_NPM_CACHE}
export PATH="{SANDBOX_LOCAL_BIN}:$PATH"
export NPM_CONFIG_PREFIX="/sandbox/.local"
export NPM_CONFIG_CACHE="{SANDBOX_NPM_CACHE}"
"#
    )
}

fn managed_env_bashrc_block() -> &'static str {
    r#"# >>> RIGHT_AGENT managed env >>>
if [ -f /sandbox/.right/env.sh ]; then
  . /sandbox/.right/env.sh
fi
# <<< RIGHT_AGENT managed env <<<
"#
}

fn ensure_bashrc_sources_managed_env(existing: &str) -> String {
    if existing.contains(">>> RIGHT_AGENT managed env >>>")
        && existing.contains(". /sandbox/.right/env.sh")
    {
        return existing.to_owned();
    }

    let mut out = existing.trim_end_matches('\n').to_owned();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(managed_env_bashrc_block());
    out
}

fn shell_printf_b_arg(content: &str) -> String {
    let escaped = content
        .replace('\\', r"\\")
        .replace('\n', r"\n")
        .replace('\r', r"\r")
        .replace('\t', r"\t");
    shlex::try_quote(&escaped)
        .expect("shlex::try_quote cannot fail for valid UTF-8")
        .into_owned()
}

/// Ensure the sandbox has Right's managed user-local CLI/npm environment.
///
/// `/sandbox/.local/bin` is the single canonical location for user-installed
/// executables. npm global installs use `/sandbox/.local` as prefix and place
/// bins in that directory. The managed env file is sourced by `.bashrc` for
/// user SSH shells and by prompt assembly for non-login Claude invocations.
async fn ensure_sandbox_user_local_env(
    sbox: &right_openshell::sandbox_exec::SandboxExec,
) -> miette::Result<()> {
    let (output, code) = sbox
        .exec(&[
            "mkdir",
            "-p",
            SANDBOX_ENV_DIR,
            SANDBOX_LOCAL_BIN,
            SANDBOX_NPM_CACHE,
        ])
        .await?;
    if code != 0 {
        return Err(miette::miette!(
            "sync: failed to create sandbox env dirs in {}: mkdir exited with {code}: {output}",
            sbox.sandbox_name()
        ));
    }

    let env_content = shell_printf_b_arg(&sandbox_env_file_content());
    let script =
        format!("printf '%b' {env_content} > {SANDBOX_ENV_PATH} && chmod 0644 {SANDBOX_ENV_PATH}");
    let (output, code) = sbox.exec(&["bash", "-lc", &script]).await?;
    if code != 0 {
        return Err(miette::miette!(
            "sync: failed to write {SANDBOX_ENV_PATH} in {}: bash exited with {code}: {output}",
            sbox.sandbox_name()
        ));
    }

    let (bashrc, code) = sbox.exec(&["cat", "/sandbox/.bashrc"]).await?;
    let existing = if code == 0 { bashrc } else { String::new() };
    let desired = ensure_bashrc_sources_managed_env(&existing);
    if desired != existing {
        let bashrc_content = shell_printf_b_arg(&desired);
        let script = format!("printf '%b' {bashrc_content} > /sandbox/.bashrc");
        let (output, code) = sbox.exec(&["bash", "-lc", &script]).await?;
        if code != 0 {
            return Err(miette::miette!(
                "sync: failed to update /sandbox/.bashrc in {}: bash exited with {code}: {output}",
                sbox.sandbox_name()
            ));
        }
        tracing::info!("sync: added managed env source block to /sandbox/.bashrc");
    }

    tracing::info!("sync: ensured sandbox user-local CLI/npm environment");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsolete_builtin_skill_paths_are_exact_legacy_sandbox_paths() {
        let expected = [
            "/sandbox/.claude/skills/rightskills",
            "/sandbox/.claude/skills/rightcron",
            "/sandbox/.claude/skills/rightmcp",
            "/sandbox/.claude/skills/rightmemory",
            "/sandbox/.claude/skills/rightreflect",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();

        assert_eq!(obsolete_builtin_skill_paths(), expected);
    }

    #[test]
    fn obsolete_builtin_skill_cleanup_error_includes_context() {
        let paths = obsolete_builtin_skill_paths();
        let error = obsolete_builtin_skill_cleanup_error("right-test-sync", &paths, "denied", 13);
        let message = error.to_string();

        assert!(message.contains("right-test-sync"));
        assert!(message.contains("rightskills"));
        assert!(message.contains("denied"));
        assert!(message.contains("13"));
    }

    #[test]
    fn reverse_sync_files_match_identity_mirror_contract() {
        assert_eq!(
            reverse_sync_files(),
            right_agent::identity_mirror::IDENTITY_MIRROR_FILES
        );
        assert!(!reverse_sync_files().contains(&"TOOLS.md"));
    }

    #[test]
    fn sandbox_env_file_content_sets_user_local_npm_contract() {
        let content = sandbox_env_file_content();

        assert!(content.contains("RIGHT_AGENT_MANAGED_ENV=1"));
        assert!(content.contains("mkdir -p /sandbox/.local/bin /sandbox/.npm"));
        assert!(content.contains("export PATH=\"/sandbox/.local/bin:$PATH\""));
        assert!(content.contains("export NPM_CONFIG_PREFIX=\"/sandbox/.local\""));
        assert!(content.contains("export NPM_CONFIG_CACHE=\"/sandbox/.npm\""));
        assert!(!content.contains("/sandbox/bin"));
        assert!(!content.contains("~/bin"));
    }

    #[test]
    fn ensure_bashrc_sources_managed_env_adds_block_once() {
        let original = "export PATH=\"/sandbox/.venv/bin:/usr/local/bin:/usr/bin:/bin\"\n";
        let updated = ensure_bashrc_sources_managed_env(original);

        assert!(updated.contains("RIGHT_AGENT managed env"));
        assert!(updated.contains("[ -f /sandbox/.right/env.sh ]"));
        assert!(updated.contains(". /sandbox/.right/env.sh"));

        let updated_again = ensure_bashrc_sources_managed_env(&updated);
        assert_eq!(updated_again, updated);
    }

    #[ignore = "ci-openshell: requires live OpenShell gateway"]
    #[tokio::test]
    async fn ci_openshell_cleanup_obsolete_builtin_skill_paths_removes_legacy_paths_in_sandbox() {
        let sandbox =
            right_openshell::test_support::TestSandbox::create("obsolete-skills-cleanup").await;
        let mtls_dir = match right_openshell::openshell::preflight_check() {
            right_openshell::openshell::OpenShellStatus::Ready(dir) => dir,
            other => panic!("OpenShell not ready: {other:?}"),
        };
        let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
            .await
            .unwrap();
        let sandbox_id =
            right_openshell::openshell::resolve_sandbox_id(&mut client, sandbox.name())
                .await
                .unwrap();
        let sbox = right_openshell::sandbox_exec::SandboxExec::new(
            mtls_dir,
            sandbox.name().to_owned(),
            sandbox_id,
        );

        let paths = obsolete_builtin_skill_paths();
        for path in &paths {
            let (output, code) = sandbox.exec(&["mkdir", "-p", path]).await;
            assert_eq!(code, 0, "failed to create {path}: {output}");
            let marker = format!("{path}/SKILL.md");
            let (output, code) = sandbox.exec(&["touch", &marker]).await;
            assert_eq!(code, 0, "failed to touch {marker}: {output}");
        }

        cleanup_obsolete_builtin_skill_paths(&sbox).await.unwrap();
        cleanup_obsolete_builtin_skill_paths(&sbox).await.unwrap();

        for path in &paths {
            let (output, code) = sandbox.exec(&["test", "!", "-e", path]).await;
            assert_eq!(code, 0, "obsolete path still exists: {path}; {output}");
        }
    }

    #[ignore = "ci-openshell: requires live OpenShell gateway"]
    #[tokio::test]
    async fn ci_openshell_ensure_sandbox_user_local_env_configures_npm_and_path() {
        let sandbox = right_openshell::test_support::TestSandbox::create("user-local-env").await;
        let mtls_dir = match right_openshell::openshell::preflight_check() {
            right_openshell::openshell::OpenShellStatus::Ready(dir) => dir,
            other => panic!("OpenShell not ready: {other:?}"),
        };
        let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
            .await
            .unwrap();
        let sandbox_id =
            right_openshell::openshell::resolve_sandbox_id(&mut client, sandbox.name())
                .await
                .unwrap();
        let sbox = right_openshell::sandbox_exec::SandboxExec::new(
            mtls_dir,
            sandbox.name().to_owned(),
            sandbox_id,
        );

        ensure_sandbox_user_local_env(&sbox).await.unwrap();
        ensure_sandbox_user_local_env(&sbox).await.unwrap();

        let (output, code) = sandbox
            .exec(&[
                "bash",
                "-lc",
                ". /sandbox/.right/env.sh && \
                 test \"$NPM_CONFIG_PREFIX\" = /sandbox/.local && \
                 test \"$NPM_CONFIG_CACHE\" = /sandbox/.npm && \
                 case \":$PATH:\" in *:/sandbox/.local/bin:*) exit 0;; *) exit 1;; esac",
            ])
            .await;
        assert_eq!(code, 0, "managed env not effective: {output}");

        let (bashrc_count, code) = sandbox
            .exec(&[
                "bash",
                "-lc",
                "grep -c 'RIGHT_AGENT managed env' /sandbox/.bashrc",
            ])
            .await;
        assert_eq!(code, 0, "grep failed: {bashrc_count}");
        assert_eq!(
            bashrc_count.trim(),
            "2",
            "bashrc block duplicated or missing"
        );
    }
}
