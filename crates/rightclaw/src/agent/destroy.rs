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

    for filename in &["agent.yaml", "policy.yaml"] {
        let src = agent_dir.join(filename);
        if src.exists() {
            std::fs::copy(&src, backup_dir.join(filename)).map_err(|e| {
                miette::miette!("failed to copy {filename}: {e:#}")
            })?;
        }
    }

    let db_path = agent_dir.join("data.db");
    if db_path.exists() {
        let backup_db = backup_dir.join("data.db");
        let db_display = db_path.display().to_string();
        let backup_path_sql = backup_db.display().to_string().replace('\'', "''");
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| {
            miette::miette!("failed to open {}: {e:#}", db_display)
        })?;
        conn.execute(&format!("VACUUM INTO '{backup_path_sql}'"), []).map_err(|e| {
            miette::miette!("VACUUM INTO failed: {e:#}")
        })?;
    }

    tracing::info!(backup_dir = %backup_dir.display(), "pre-destroy backup complete");
    Ok(backup_dir)
}

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
    let agents_dir = crate::config::agents_dir(home);
    let agent_dir = agents_dir.join(&options.agent_name);

    if !agent_dir.exists() {
        return Err(miette::miette!(
            "Agent '{}' not found at {}",
            options.agent_name,
            agent_dir.display(),
        ));
    }

    let config = crate::agent::parse_agent_config(&agent_dir)?;
    let is_sandboxed = config
        .as_ref()
        .map(|c| c.is_sandboxed())
        .unwrap_or(true);

    let mut result = DestroyResult {
        agent_stopped: false,
        sandbox_deleted: false,
        backup_path: None,
        dir_removed: false,
        pc_reloaded: false,
    };

    let pc_client = crate::runtime::PcClient::new(options.pc_port)?;
    let pc_running = pc_client.health_check().await.is_ok();

    if pc_running {
        let process_name = format!("{}-bot", options.agent_name);
        match pc_client.stop_process(&process_name).await {
            Ok(()) => {
                tracing::info!(agent = %options.agent_name, "stopped agent process");
                result.agent_stopped = true;
            }
            Err(e) => {
                tracing::warn!(agent = %options.agent_name, error = format!("{e:#}"), "failed to stop agent process (may already be stopped)");
            }
        }
    }

    if options.backup {
        let backup_path = run_backup(home, &options.agent_name, &agent_dir, &config, is_sandboxed).await?;
        result.backup_path = Some(backup_path);
    }

    if is_sandboxed {
        let sb_name = config
            .as_ref()
            .map(|c| crate::openshell::resolve_sandbox_name(&options.agent_name, c))
            .unwrap_or_else(|| crate::openshell::sandbox_name(&options.agent_name));
        crate::openshell::delete_sandbox(&sb_name).await;
        result.sandbox_deleted = true;
    }

    std::fs::remove_dir_all(&agent_dir).map_err(|e| {
        miette::miette!(
            "failed to remove agent directory {}: {e:#}",
            agent_dir.display(),
        )
    })?;
    result.dir_removed = true;
    tracing::info!(agent = %options.agent_name, dir = %agent_dir.display(), "removed agent directory");

    if pc_running {
        let all_agents = crate::agent::discover_agents(&agents_dir)?;
        let self_exe = std::env::current_exe().map_err(|e| {
            miette::miette!("failed to resolve current executable path: {e:#}")
        })?;
        crate::codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

        match pc_client.reload_configuration().await {
            Ok(()) => {
                tracing::info!("reloaded process-compose configuration");
                result.pc_reloaded = true;
            }
            Err(e) => {
                tracing::warn!(error = format!("{e:#}"), "failed to reload process-compose (non-fatal)");
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn destroy_nonsandboxed_agent_removes_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        let agents_dir = home.join("agents").join("test-agent");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("agent.yaml"),
            "sandbox:\n  mode: none\n",
        )
        .unwrap();

        let options = DestroyOptions {
            agent_name: "test-agent".into(),
            backup: false,
            pc_port: 19999,
        };

        let result = destroy_agent(home, &options).await.unwrap();

        assert!(!result.agent_stopped, "PC not running, should not have stopped");
        assert!(!result.sandbox_deleted, "non-sandboxed agent, no sandbox to delete");
        assert!(result.backup_path.is_none());
        assert!(result.dir_removed);
        assert!(!result.pc_reloaded, "PC not running, should not have reloaded");
        assert!(!agents_dir.exists(), "agent dir should be deleted");
    }

    #[tokio::test]
    async fn destroy_nonexistent_agent_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("agents")).unwrap();

        let options = DestroyOptions {
            agent_name: "nonexistent".into(),
            backup: false,
            pc_port: 19999,
        };

        let result = destroy_agent(home, &options).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn destroy_with_backup_creates_backup_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        let agents_dir = home.join("agents").join("backup-test");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("agent.yaml"),
            "sandbox:\n  mode: none\n",
        )
        .unwrap();
        std::fs::write(agents_dir.join("AGENTS.md"), "# Test agent").unwrap();

        let options = DestroyOptions {
            agent_name: "backup-test".into(),
            backup: true,
            pc_port: 19999,
        };

        let result = destroy_agent(home, &options).await.unwrap();

        assert!(result.backup_path.is_some(), "backup should have been created");
        let backup_path = result.backup_path.unwrap();
        assert!(backup_path.exists(), "backup dir should exist");
        assert!(backup_path.join("sandbox.tar.gz").exists(), "sandbox.tar.gz should exist");
        assert!(result.dir_removed, "agent dir should be removed after backup");
    }
}
