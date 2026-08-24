//! Background sync task: periodically uploads config files to sandbox.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::time::{Duration, interval};
use tokio_util::sync::CancellationToken;

use crate::sandbox::{Sandbox, exec_argv, exec_argv_as_guest, upload_into_dir};
use crate::sandbox_runtime::SandboxRuntimeHandle;

/// Interval between sync cycles.
const SYNC_INTERVAL: Duration = Duration::from_secs(300);

/// Ask the supervisor to verify the backend after a sync-cycle failure.
///
/// No-ops without a handle; the sync task reports suspicion and does not decide
/// sandbox health.
pub(crate) fn report_sync_failure(handle: Option<&SandboxRuntimeHandle>) {
    if let Some(h) = handle {
        h.report_suspected_failure();
    }
}

/// Run one sync cycle. Called synchronously at startup before the Telegram bot starts,
/// ensuring sandbox has correct config before any `claude -p` invocations.
pub(crate) async fn initial_sync(agent_dir: &Path, sbox: &Sandbox) -> miette::Result<()> {
    tracing::info!(sandbox = sbox.name(), "sync: initial cycle (blocking)");

    // The unprivileged guest user must exist before any `claude -p` runs: the
    // turn executes as `GUEST_USER`, and Claude Code refuses root. Provisioning
    // owns this (the stage-1 probes did it inline), but the stage-4 rewire
    // dropped the step, so a migrated agent booted with no `sandbox` user and
    // every turn hung. Idempotent, and must land before deploy_manifest because
    // the platform tree is handed to the agent.
    ensure_sandbox_user(sbox).await?;

    // Re-assert the user-local environment on every bring-up. This provisions
    // its directories through the guest control plane only; it never requires
    // public guest egress, so existing restrictive sandboxes upgrade in place.
    ensure_sandbox_user_local_env(agent_dir, sbox).await?;
    sync_cycle(agent_dir, sbox).await?;
    Ok(())
}

/// Run the periodic sync loop (spawned as background task after initial_sync).
pub(crate) async fn run_sync_task(
    agent_dir: PathBuf,
    sbox: Sandbox,
    sandbox_runtime: Option<Arc<SandboxRuntimeHandle>>,
    shutdown: CancellationToken,
) {
    let mut tick = interval(SYNC_INTERVAL);
    tick.tick().await; // consume immediate tick

    loop {
        tokio::select! {
            _ = tick.tick() => {
                tracing::debug!(sandbox = %sbox.name(), "sync: starting cycle");

                if let Err(e) = sync_cycle(&agent_dir, &sbox).await {
                    tracing::error!(sandbox = %sbox.name(), "sync cycle failed: {e:#}");
                    report_sync_failure(sandbox_runtime.as_deref());
                }
            }
            _ = shutdown.cancelled() => {
                tracing::info!(sandbox = %sbox.name(), "sync task shutting down");
                break;
            }
        }
    }
}

pub(crate) async fn sync_cycle(agent_dir: &Path, sbox: &Sandbox) -> miette::Result<()> {
    // Build manifest of platform-managed files
    let manifest =
        right_platform_store::build_manifest(agent_dir, right_codegen::BUILTIN_SKILL_NAMES)?;

    // Deploy to /platform/ with content-addressed names + symlinks
    right_platform_store::deploy_manifest(sbox, &manifest).await?;

    // Verify .claude.json (separate flow — not content-addressed)
    verify_claude_json(agent_dir, sbox).await?;

    tracing::debug!("sync: cycle complete");
    Ok(())
}

fn obsolete_builtin_skill_paths() -> Vec<String> {
    right_codegen::BUILTIN_SKILL_LEGACY_NAMES
        .iter()
        .map(|legacy_name| format!("/sandbox/.claude/skills/{legacy_name}"))
        .collect()
}

async fn cleanup_obsolete_builtin_skill_paths(sbox: &Sandbox) -> miette::Result<()> {
    let paths = obsolete_builtin_skill_paths();
    let mut args = vec!["rm", "-rf"];
    args.extend(paths.iter().map(String::as_str));

    let (output, code) = exec_argv(sbox, &args).await?;
    if code != 0 {
        return Err(obsolete_builtin_skill_cleanup_error(
            sbox.name(),
            &paths,
            &output,
            code,
        ));
    }

    tracing::debug!(
        sandbox = %sbox.name(),
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
pub(crate) async fn reverse_sync_md(agent_dir: &Path, sbox: &Sandbox) -> miette::Result<()> {
    let tmp_dir = tempfile::tempdir()
        .map_err(|e| miette::miette!("reverse sync: failed to create temp dir: {e:#}"))?;

    // Download all files concurrently; each job gets a unique dest path.
    let mut join_set = tokio::task::JoinSet::new();
    for &filename in reverse_sync_files() {
        let sandbox = Arc::clone(sbox);
        let dl_path = tmp_dir.path().join(format!("dl-{filename}"));
        let sandbox_path = format!("/sandbox/{filename}");
        join_set.spawn(async move {
            let result = sandbox.fs_copy_to_host(&sandbox_path, &dl_path).await;
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
async fn verify_claude_json(agent_dir: &Path, sandbox: &Sandbox) -> miette::Result<()> {
    let tmp_dir =
        tempfile::tempdir().map_err(|e| miette::miette!("failed to create temp dir: {e:#}"))?;
    let downloaded = tmp_dir.path().join(".claude.json");

    if let Err(e) = sandbox
        .fs_copy_to_host("/sandbox/.claude.json", &downloaded)
        .await
    {
        tracing::warn!("sync: failed to download .claude.json (may not exist yet): {e:#}");
        // Upload host version as baseline
        let host_claude_json = agent_dir.join(".claude.json");
        if host_claude_json.exists() {
            upload_into_dir(sandbox, &host_claude_json, "/sandbox")
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
        upload_into_dir(sandbox, &fixed_path, "/sandbox")
            .await
            .map_err(|e| miette::miette!("sync: re-upload fixed .claude.json: {e:#}"))?;
        tracing::info!("sync: fixed and re-uploaded .claude.json (right-agent keys were modified)");
    }

    Ok(())
}

use crate::cc::sandbox_env::{
    MANAGED_ENV_END_MARKER, MANAGED_ENV_START_MARKER, SANDBOX_BASHRC_PATH, SANDBOX_ENV_DIR,
    SANDBOX_ENV_PATH, SANDBOX_LOCAL_BIN, SANDBOX_LOCAL_PREFIX, SANDBOX_NPM_CACHE,
    bashrc_source_block, env_file_content,
};

fn ensure_bashrc_sources_managed_env(existing: &str) -> String {
    let mut out = String::with_capacity(existing.len() + bashrc_source_block().len());
    let mut cursor = 0;
    let mut inserted = false;
    let mut removed_any = false;

    loop {
        let rest = &existing[cursor..];
        let next_start = rest
            .find(MANAGED_ENV_START_MARKER)
            .map(|offset| cursor + offset);
        let next_end = rest
            .find(MANAGED_ENV_END_MARKER)
            .map(|offset| cursor + offset);

        let Some(marker_start) = earliest_marker(next_start, next_end) else {
            break;
        };

        out.push_str(&existing[cursor..marker_start]);
        if !inserted {
            out.push_str(bashrc_source_block());
            inserted = true;
        }
        removed_any = true;

        if Some(marker_start) == next_start {
            if let Some(end_relative) = existing[marker_start..].find(MANAGED_ENV_END_MARKER) {
                let end_marker_end = marker_start + end_relative + MANAGED_ENV_END_MARKER.len();
                cursor = line_end_after(existing, end_marker_end);
            } else {
                cursor = line_end_after(existing, marker_start + MANAGED_ENV_START_MARKER.len());
            }
        } else {
            cursor = line_end_after(existing, marker_start + MANAGED_ENV_END_MARKER.len());
        }
    }

    if removed_any {
        out.push_str(&existing[cursor..]);
        return out;
    }

    let mut out = existing.trim_end_matches('\n').to_owned();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(bashrc_source_block());
    out
}

fn earliest_marker(start: Option<usize>, end: Option<usize>) -> Option<usize> {
    match (start, end) {
        (Some(start), Some(end)) => Some(start.min(end)),
        (Some(start), None) => Some(start),
        (None, Some(end)) => Some(end),
        (None, None) => None,
    }
}

fn line_end_after(content: &str, index: usize) -> usize {
    if content[index..].starts_with("\r\n") {
        index + 2
    } else if content[index..].starts_with('\n') {
        index + 1
    } else {
        content[index..]
            .find('\n')
            .map_or(content.len(), |line_end| index + line_end + 1)
    }
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

fn shell_arg(content: &str) -> String {
    shlex::try_quote(content)
        .expect("shlex::try_quote cannot fail for valid UTF-8")
        .into_owned()
}

fn bashrc_read_script(path: &str) -> String {
    let path_arg = shell_arg(path);
    let message = shell_arg(&format!("not a regular file: {path}"));
    format!(
        "if [ -L {path_arg} ]; then \
           printf '%s\\n' {message} >&2; \
           exit 1; \
         elif [ -f {path_arg} ]; then \
           cat {path_arg}; \
         elif [ -e {path_arg} ]; then \
           printf '%s\\n' {message} >&2; \
           exit 1; \
         else \
           :; \
         fi"
    )
}

/// Ensure the sandbox has Right's managed user-local CLI/npm environment.
///
/// `/sandbox/.local/bin` is the single canonical location for user-installed
/// executables. npm global installs use `/sandbox/.local` as prefix and place
/// bins in that directory. The managed env file is sourced by `.bashrc` for
/// user SSH shells and provides the shared contract for non-login Claude setup.
async fn ensure_sandbox_user_local_env(agent_dir: &Path, sbox: &Sandbox) -> miette::Result<()> {
    // Read `agent.yaml::env` so the generated env.sh includes per-agent
    // operator-declared exports alongside the managed PATH/NPM block.
    // Treat a missing/unparseable agent.yaml as "no extra env" — the
    // managed block is still useful on its own, and sync should not
    // fail closed for what is, semantically, optional config. A parse
    // failure is still logged: silently dropping the operator's env
    // (e.g. ANTHROPIC_BASE_URL) would route the agent elsewhere with no
    // trace of why.
    let agent_env = match right_agent::agent::discovery::parse_agent_config(agent_dir) {
        Ok(cfg) => cfg.map(|c| c.env).unwrap_or_default(),
        Err(e) => {
            tracing::warn!(
                agent_dir = %agent_dir.display(),
                error = format!("{e:#}"),
                "sync: failed to parse agent.yaml for env injection; \
                 proceeding with managed env only"
            );
            Default::default()
        }
    };
    let env_content = shell_printf_b_arg(&env_file_content(&agent_env));
    let write_script = sandbox_env_setup_script(&env_content);
    let (output, code) = exec_argv_as_guest(sbox, &["bash", "-lc", &write_script]).await?;
    if code != 0 {
        return Err(miette::miette!(
            "sync: failed to write {SANDBOX_ENV_PATH} in {}: bash exited with {code}: {output}",
            sbox.name()
        ));
    }

    // Read as the guest user and reject every symlink. The home persists and
    // is agent-controlled between boots, so root must never follow these paths.
    let read_script = bashrc_read_script(SANDBOX_BASHRC_PATH);
    let (existing, code) = exec_argv_as_guest(sbox, &["bash", "-lc", &read_script]).await?;
    if code != 0 {
        return Err(miette::miette!(
            "sync: failed to read {SANDBOX_BASHRC_PATH} in {}: bash exited with {code} (output: {existing})",
            sbox.name()
        ));
    }
    let desired = ensure_bashrc_sources_managed_env(&existing);
    if desired != existing {
        let bashrc_content = shell_printf_b_arg(&desired);
        let script = bashrc_write_script(&bashrc_content);
        let (output, code) = exec_argv_as_guest(sbox, &["bash", "-lc", &script]).await?;
        if code != 0 {
            return Err(miette::miette!(
                "sync: failed to update /sandbox/.bashrc in {}: bash exited with {code}: {output}",
                sbox.name()
            ));
        }
        tracing::info!("sync: added managed env source block to /sandbox/.bashrc");
    }

    tracing::info!("sync: ensured sandbox user-local CLI/npm environment");
    Ok(())
}
fn sandbox_env_setup_script(env_content: &str) -> String {
    format!(
        "for path in {SANDBOX_ENV_DIR} {SANDBOX_LOCAL_PREFIX} {SANDBOX_LOCAL_BIN} {SANDBOX_NPM_CACHE}; do \
           if [ -L \"$path\" ] || {{ [ -e \"$path\" ] && [ ! -d \"$path\" ]; }}; then \
             printf 'managed path is not a real directory: %s\\n' \"$path\" >&2; exit 1; \
           fi; \
         done; \
         mkdir -p {SANDBOX_ENV_DIR} {SANDBOX_LOCAL_BIN} {SANDBOX_NPM_CACHE} && \
         set -C && \
         rm -f {SANDBOX_ENV_PATH}.tmp && \
         printf '%b' {env_content} > {SANDBOX_ENV_PATH}.tmp && \
         chmod 0644 {SANDBOX_ENV_PATH}.tmp && \
         mv -f {SANDBOX_ENV_PATH}.tmp {SANDBOX_ENV_PATH}"
    )
}

fn bashrc_write_script(content: &str) -> String {
    format!(
        "if [ -L {SANDBOX_BASHRC_PATH} ] || \
            {{ [ -e {SANDBOX_BASHRC_PATH} ] && [ ! -f {SANDBOX_BASHRC_PATH} ]; }} || \
            [ -L {SANDBOX_BASHRC_PATH}.right-tmp ]; then \
           printf '%s\\n' 'managed bashrc path is not a regular file' >&2; exit 1; \
         fi; \
         rm -f {SANDBOX_BASHRC_PATH}.right-tmp && \
         set -C && \
         printf '%b' {content} > {SANDBOX_BASHRC_PATH}.right-tmp && \
         chmod 0644 {SANDBOX_BASHRC_PATH}.right-tmp && \
         mv -f {SANDBOX_BASHRC_PATH}.right-tmp {SANDBOX_BASHRC_PATH}"
    )
}

/// Create the unprivileged guest user if it does not exist.
///
/// Idempotent: a sandbox that already has the user is a no-op. The script
/// tries the Debian-flavoured `useradd` first and the Alpine `adduser` second,
/// matching the images the agent spec supports (`node:22-slim` is Debian, the
/// probe images are Alpine). The user's home is `/sandbox`, which provisioning
/// has already created by the time this runs.
async fn ensure_sandbox_user(sbox: &Sandbox) -> miette::Result<()> {
    let user = right_sandbox::GUEST_USER;
    // Create the user if missing (Debian `useradd`, Alpine `adduser`), then —
    // whether it was just created or already existed — hand the carried home
    // to it, excluding `.platform` which must stay root-owned. The chown has
    // to be recursive and must run every time, not just on first boot: the
    // migration archives a home as root and defers the handover to the moment
    // the user exists, so a re-provision (or a migrated agent whose user was
    // created after migration) would otherwise leave `.claude` root-owned and
    // `claude` unable to write its session state — the symptom is a turn that
    // never finishes.
    let script = format!(
        "if ! id {user} >/dev/null 2>&1; then \
           if command -v useradd >/dev/null 2>&1; then \
             useradd -m -d /sandbox -s /bin/bash {user}; \
           elif command -v adduser >/dev/null 2>&1; then \
             adduser -D -h /sandbox {user}; \
           else \
             echo 'no useradd or adduser on PATH' >&2; exit 1; \
           fi; \
         fi; \
         chown {user}:{user} /sandbox; \
         find /sandbox -mindepth 1 -maxdepth 1 ! -name .platform -exec chown -R {user}:{user} {{}} +"
    );
    let (output, code) = exec_argv(sbox, &["bash", "-lc", &script]).await?;
    if code != 0 {
        return Err(miette::miette!(
            "sync: failed to provision the guest user in {}: bash exited with {code}: {output}",
            sbox.name()
        ));
    }
    tracing::info!(
        sandbox = sbox.name(),
        user = right_sandbox::GUEST_USER,
        "sync: ensured guest user and ownership"
    );
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
        let content = env_file_content(&std::collections::HashMap::new());

        assert!(content.contains("RIGHT_AGENT_MANAGED_ENV=1"));
        assert!(content.contains("mkdir -p /sandbox/.local/bin /sandbox/.npm"));
        assert!(content.contains("case \":$PATH:\" in"));
        assert!(content.contains("*:/sandbox/.local/bin:*)"));
        assert!(content.contains("export PATH=\"/sandbox/.local/bin:$PATH\""));
        assert!(content.contains("export NPM_CONFIG_PREFIX=\"/sandbox/.local\""));
        assert!(content.contains("export NPM_CONFIG_CACHE=\"/sandbox/.npm\""));
        assert!(!content.contains("/sandbox/bin"));
        assert!(!content.contains("~/bin"));
    }
    #[test]
    fn sandbox_env_setup_creates_user_local_dirs_without_network_or_root_mutation() {
        let script = sandbox_env_setup_script("env body");

        assert!(
            script.contains("mkdir -p /sandbox/.right /sandbox/.local/bin /sandbox/.npm"),
            "fresh sandboxes need the user-local directories before any shell sources env.sh: {script}"
        );
        assert!(script.contains("[ -L \"$path\" ]"));
        assert!(script.contains("[ ! -d \"$path\" ]"));
        assert!(
            script.contains("set -C"),
            "temp creation must reject symlinks: {script}"
        );
        assert!(
            !script.contains("chown") && !script.contains("apt-get") && !script.contains("curl"),
            "guest setup must neither mutate ownership nor require public egress: {script}"
        );
    }
    #[cfg(unix)]
    #[test]
    fn sandbox_env_setup_rejects_managed_directory_symlinks_without_touching_targets() {
        use std::os::unix::fs::symlink;
        use std::process::Command;

        for managed in [".right", ".local"] {
            let temp = tempfile::tempdir().expect("tempdir");
            let sandbox = temp.path().join("sandbox");
            let sentinel = temp.path().join("sentinel");
            std::fs::create_dir(&sandbox).expect("sandbox dir");
            std::fs::create_dir(&sentinel).expect("sentinel dir");
            std::fs::write(sentinel.join("canary"), "unchanged").expect("sentinel canary");
            symlink(&sentinel, sandbox.join(managed)).expect("managed symlink");

            let script = sandbox_env_setup_script("'managed env'")
                .replace("/sandbox", sandbox.to_str().expect("UTF-8 temp path"));
            let output = Command::new("bash")
                .args(["-c", &script])
                .output()
                .expect("run setup script");

            assert!(
                !output.status.success(),
                "{managed} symlink must fail closed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                std::fs::read_to_string(sentinel.join("canary")).expect("read canary"),
                "unchanged"
            );
            assert_eq!(
                std::fs::read_dir(&sentinel).expect("list sentinel").count(),
                1,
                "setup created content through {managed} symlink"
            );
        }
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

    #[test]
    fn ensure_bashrc_sources_managed_env_replaces_malformed_managed_block() {
        let original = "\
export PATH=\"/usr/bin:/bin\"
# >>> RIGHT_AGENT managed env >>>
echo stale
# <<< RIGHT_AGENT managed env <<<
alias ll='ls -la'
";
        let expected = format!(
            "export PATH=\"/usr/bin:/bin\"\n{}alias ll='ls -la'\n",
            bashrc_source_block()
        );

        assert_eq!(ensure_bashrc_sources_managed_env(original), expected);
    }

    #[test]
    fn ensure_bashrc_sources_managed_env_repairs_start_only_marker_idempotently() {
        let original = "\
export PATH=\"/usr/bin:/bin\"
# >>> RIGHT_AGENT managed env >>>
echo user-kept-this
";
        let updated = ensure_bashrc_sources_managed_env(original);

        assert_eq!(
            updated,
            format!(
                "export PATH=\"/usr/bin:/bin\"\n{}echo user-kept-this\n",
                bashrc_source_block()
            )
        );
        assert_eq!(ensure_bashrc_sources_managed_env(&updated), updated);
        assert_eq!(
            updated.matches(">>> RIGHT_AGENT managed env >>>").count(),
            1
        );
        assert_eq!(
            updated.matches("<<< RIGHT_AGENT managed env <<<").count(),
            1
        );
    }

    #[test]
    fn ensure_bashrc_sources_managed_env_repairs_end_only_marker_idempotently() {
        let original = "\
export PATH=\"/usr/bin:/bin\"
echo before
# <<< RIGHT_AGENT managed env <<<
alias ll='ls -la'
";
        let updated = ensure_bashrc_sources_managed_env(original);

        assert_eq!(
            updated,
            format!(
                "export PATH=\"/usr/bin:/bin\"\necho before\n{}alias ll='ls -la'\n",
                bashrc_source_block()
            )
        );
        assert_eq!(ensure_bashrc_sources_managed_env(&updated), updated);
        assert_eq!(
            updated.matches(">>> RIGHT_AGENT managed env >>>").count(),
            1
        );
        assert_eq!(
            updated.matches("<<< RIGHT_AGENT managed env <<<").count(),
            1
        );
    }

    #[test]
    fn ensure_bashrc_sources_managed_env_leaves_exact_block_byte_for_byte() {
        let existing = format!(
            "export PATH=\"/usr/bin:/bin\"\n{}alias ll='ls -la'\n",
            bashrc_source_block()
        );

        assert_eq!(ensure_bashrc_sources_managed_env(&existing), existing);
    }

    #[test]
    fn ensure_bashrc_sources_managed_env_collapses_duplicate_complete_blocks() {
        let original = format!(
            "export PATH=\"/usr/bin:/bin\"\n{}echo between\n{}alias ll='ls -la'\n",
            bashrc_source_block(),
            "# >>> RIGHT_AGENT managed env >>>\necho stale\n# <<< RIGHT_AGENT managed env <<<\n"
        );
        let expected = format!(
            "export PATH=\"/usr/bin:/bin\"\n{}echo between\nalias ll='ls -la'\n",
            bashrc_source_block()
        );
        let updated = ensure_bashrc_sources_managed_env(&original);

        assert_eq!(updated, expected);
        assert_eq!(ensure_bashrc_sources_managed_env(&updated), updated);
        assert_eq!(
            updated.matches(">>> RIGHT_AGENT managed env >>>").count(),
            1
        );
        assert_eq!(
            updated.matches("<<< RIGHT_AGENT managed env <<<").count(),
            1
        );
    }

    #[test]
    fn ensure_bashrc_sources_managed_env_normalizes_mixed_orphans_and_complete_blocks() {
        let original = "\
export PATH=\"/usr/bin:/bin\"
# <<< RIGHT_AGENT managed env <<<
echo before
# >>> RIGHT_AGENT managed env >>>
echo stale
# <<< RIGHT_AGENT managed env <<<
echo after
# >>> RIGHT_AGENT managed env >>>
echo orphan-start-tail
";
        let expected = format!(
            "export PATH=\"/usr/bin:/bin\"\n{}echo before\necho after\necho orphan-start-tail\n",
            bashrc_source_block()
        );
        let updated = ensure_bashrc_sources_managed_env(original);

        assert_eq!(updated, expected);
        assert_eq!(ensure_bashrc_sources_managed_env(&updated), updated);
        assert_eq!(
            updated.matches(">>> RIGHT_AGENT managed env >>>").count(),
            1
        );
        assert_eq!(
            updated.matches("<<< RIGHT_AGENT managed env <<<").count(),
            1
        );
    }

    #[test]
    fn shell_printf_b_arg_round_trips_shellish_content_through_bash() {
        let content = "single ' quote\n\
command $(printf exploited) and `printf exploited`\n\
percent % and backslash \\ and carriage\r and tab\t done\n";
        let arg = shell_printf_b_arg(content);
        let output = std::process::Command::new("bash")
            .arg("-lc")
            .arg(format!("printf '%b' {arg}"))
            .output()
            .expect("bash should run printf roundtrip");

        assert!(
            output.status.success(),
            "bash failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, content.as_bytes());
    }

    #[test]
    fn bashrc_read_script_errors_on_existing_non_regular_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bashrc_path = tmp.path().join(".bashrc");
        std::fs::create_dir(&bashrc_path).expect("create .bashrc dir");

        let output = std::process::Command::new("bash")
            .arg("-lc")
            .arg(bashrc_read_script(
                bashrc_path.to_str().expect("utf-8 temp path"),
            ))
            .output()
            .expect("bash should run read script");

        assert!(
            !output.status.success(),
            "directory .bashrc should be rejected, stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("not a regular file"),
            "stderr should explain non-regular path, got {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn bashrc_read_script_reads_regular_file_and_allows_missing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bashrc_path = tmp.path().join(".bashrc");
        std::fs::write(&bashrc_path, "export TEST=1\n").expect("write .bashrc");

        let output = std::process::Command::new("bash")
            .arg("-lc")
            .arg(bashrc_read_script(
                bashrc_path.to_str().expect("utf-8 temp path"),
            ))
            .output()
            .expect("bash should run read script");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"export TEST=1\n");

        std::fs::remove_file(&bashrc_path).expect("remove .bashrc");
        let output = std::process::Command::new("bash")
            .arg("-lc")
            .arg(bashrc_read_script(
                bashrc_path.to_str().expect("utf-8 temp path"),
            ))
            .output()
            .expect("bash should run read script");
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn bashrc_read_script_rejects_symlink_without_reading_target() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("sentinel");
        let bashrc_path = tmp.path().join(".bashrc");
        std::fs::write(&target, "secret sentinel").expect("write sentinel");
        std::os::unix::fs::symlink(&target, &bashrc_path).expect("create symlink");

        let output = std::process::Command::new("bash")
            .arg("-lc")
            .arg(bashrc_read_script(
                bashrc_path.to_str().expect("utf-8 temp path"),
            ))
            .output()
            .expect("bash should run read script");

        assert!(!output.status.success(), "bashrc symlink must fail closed");
        assert!(output.stdout.is_empty(), "target content must not be read");
        assert_eq!(
            std::fs::read_to_string(target).expect("read target"),
            "secret sentinel"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bashrc_write_script_rejects_target_and_temp_symlinks_without_touching_sentinel() {
        for suffix in ["", ".right-tmp"] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let sentinel = tmp.path().join("sentinel");
            let bashrc = tmp.path().join(".bashrc");
            std::fs::write(&sentinel, "unchanged").expect("write sentinel");
            std::os::unix::fs::symlink(&sentinel, format!("{}{}", bashrc.display(), suffix))
                .expect("create managed symlink");

            let script = bashrc_write_script("'managed bashrc'").replace(
                SANDBOX_BASHRC_PATH,
                bashrc.to_str().expect("utf-8 temp path"),
            );
            let output = std::process::Command::new("bash")
                .args(["-c", &script])
                .output()
                .expect("run bashrc write script");

            assert!(
                !output.status.success(),
                "{suffix:?} symlink must fail closed"
            );
            assert_eq!(
                std::fs::read_to_string(&sentinel).expect("read sentinel"),
                "unchanged"
            );
        }
    }
}
