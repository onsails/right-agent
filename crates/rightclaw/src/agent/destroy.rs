use std::path::{Path, PathBuf};

use crate::agent::types::AgentConfig;

/// Options for destroying an agent (resolved by caller — no TTY interaction).
pub struct DestroyOptions {
    pub agent_name: String,
    pub backup: bool,
    pub pc_port: u16,
}

/// Result of a destroy operation — booleans reflect what actually happened.
pub struct DestroyResult {
    /// Whether the agent process was stopped via process-compose.
    pub agent_stopped: bool,
    /// Whether an OpenShell sandbox was deleted.
    pub sandbox_deleted: bool,
    /// Path to backup if one was created.
    pub backup_path: Option<PathBuf>,
    /// Whether the agent directory was removed.
    pub dir_removed: bool,
    /// Whether process-compose was reloaded.
    pub pc_reloaded: bool,
}

/// Run a pre-destroy backup. Returns the backup directory path.
///
/// For non-sandboxed agents: tars the agent directory (excluding data.db).
/// For sandboxed agents: attempts SSH tar of sandbox, falls back to config-only backup.
/// Always copies agent.yaml, policy.yaml, and VACUUM-copies data.db.
async fn run_backup(
    home: &Path,
    agent_name: &str,
    agent_dir: &Path,
    config: &Option<AgentConfig>,
    is_sandboxed: bool,
) -> miette::Result<PathBuf> {
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let backup_dir = crate::config::backups_dir(home, agent_name).join(&timestamp);
    std::fs::create_dir_all(&backup_dir).map_err(|e| {
        miette::miette!("failed to create backup dir {}: {e:#}", backup_dir.display())
    })?;

    tracing::info!(agent = agent_name, backup_dir = %backup_dir.display(), "starting pre-destroy backup");

    if is_sandboxed {
        // Try SSH tar download from sandbox; skip if sandbox not ready
        let sandbox_backed_up = try_sandbox_backup(home, agent_name, config, &backup_dir).await;
        if !sandbox_backed_up {
            tracing::warn!(agent = agent_name, "sandbox not available for backup — backing up config files only");
        }
    } else {
        // Non-sandboxed: tar the agent dir (excluding data.db — backed up separately)
        let dest_tar = backup_dir.join("sandbox.tar.gz");
        let parent = agent_dir.parent().ok_or_else(|| miette::miette!("agent_dir has no parent"))?;
        let status = tokio::process::Command::new("tar")
            .args([
                "czpf",
                dest_tar.to_str().ok_or_else(|| miette::miette!("non-UTF-8 backup path"))?,
                "--exclude=data.db",
                "-C",
                parent.to_str().ok_or_else(|| miette::miette!("non-UTF-8 agents_dir"))?,
                agent_name,
            ])
            .status()
            .await
            .map_err(|e| miette::miette!("failed to spawn tar: {e:#}"))?;
        if !status.success() {
            return Err(miette::miette!("tar exited with status {status}"));
        }
    }

    // Copy config files
    for filename in &["agent.yaml", "policy.yaml"] {
        let src = agent_dir.join(filename);
        if src.exists() {
            std::fs::copy(&src, backup_dir.join(filename)).map_err(|e| {
                miette::miette!("failed to copy {filename}: {e:#}")
            })?;
        }
    }

    // VACUUM data.db if it exists
    let db_path = agent_dir.join("data.db");
    if db_path.exists() {
        let backup_db = backup_dir.join("data.db");
        let db_display = db_path.display().to_string();
        let backup_display = backup_db.display().to_string().replace('\'', "''");
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| {
            miette::miette!("failed to open {}: {e:#}", db_display)
        })?;
        conn.execute(&format!("VACUUM INTO '{backup_display}'"), []).map_err(|e| {
            miette::miette!("VACUUM INTO failed: {e:#}")
        })?;
    }

    tracing::info!(backup_dir = %backup_dir.display(), "pre-destroy backup complete");
    Ok(backup_dir)
}

/// Attempt to SSH-tar the sandbox contents. Returns true if successful.
async fn try_sandbox_backup(
    home: &Path,
    agent_name: &str,
    config: &Option<AgentConfig>,
    backup_dir: &Path,
) -> bool {
    let sb_name = config
        .as_ref()
        .map(|c| crate::openshell::resolve_sandbox_name(agent_name, c))
        .unwrap_or_else(|| crate::openshell::sandbox_name(agent_name));

    // Check OpenShell availability
    let mtls_dir = match crate::openshell::preflight_check() {
        crate::openshell::OpenShellStatus::Ready(dir) => dir,
        _ => return false,
    };

    // Check sandbox readiness
    let mut grpc = match crate::openshell::connect_grpc(&mtls_dir).await {
        Ok(g) => g,
        Err(_) => return false,
    };
    let ready = match crate::openshell::is_sandbox_ready(&mut grpc, &sb_name).await {
        Ok(r) => r,
        Err(_) => return false,
    };
    if !ready {
        return false;
    }

    let ssh_config = home.join("run").join("ssh").join(format!("{sb_name}.ssh-config"));
    if !ssh_config.exists() {
        return false;
    }

    let ssh_host = crate::openshell::ssh_host_for_sandbox(&sb_name);
    let dest_tar = backup_dir.join("sandbox.tar.gz");

    crate::openshell::ssh_tar_download(&ssh_config, &ssh_host, "sandbox", &dest_tar, 300)
        .await
        .is_ok()
}

/// Destroy an agent: stop process, optionally backup, delete sandbox, remove directory, reload PC.
///
/// Non-fatal steps (stop, sandbox delete, PC reload) warn and continue.
/// Fatal steps (backup if requested, directory removal) propagate errors.
pub async fn destroy_agent(home: &Path, options: &DestroyOptions) -> miette::Result<DestroyResult> {
    todo!()
}
