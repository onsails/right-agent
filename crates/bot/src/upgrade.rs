//! Background task that periodically upgrades Claude Code inside a sandbox.
//!
//! Runs `claude upgrade` via SSH every 8 hours. The upgraded binary is installed
//! to `/sandbox/.local/bin/claude` and takes precedence over the image-baked
//! `/usr/local/bin/claude` via PATH ordering (set up by `sync.rs`).

use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Default interval between upgrade checks (8 hours).
const UPGRADE_INTERVAL: Duration = Duration::from_secs(8 * 3600);

/// Timeout for `claude upgrade` SSH command (2 minutes).
const UPGRADE_TIMEOUT_SECS: u64 = 120;

/// Spawn a background task that periodically runs `claude upgrade` in the sandbox.
///
/// First tick fires immediately on startup, then every 8 hours.
/// Errors are logged but never propagated — the task keeps running.
pub fn spawn_upgrade_task(
    ssh_config_path: PathBuf,
    agent_name: String,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        run_upgrade_loop(&ssh_config_path, &agent_name, shutdown).await;
    });
}

async fn run_upgrade_loop(ssh_config_path: &Path, agent_name: &str, shutdown: CancellationToken) {
    let mut interval = tokio::time::interval(UPGRADE_INTERVAL);
    let ssh_host = rightclaw::openshell::ssh_host(agent_name);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!(agent = %agent_name, "upgrade task shutting down");
                return;
            }
            _ = interval.tick() => {
                run_upgrade(ssh_config_path, &ssh_host, agent_name).await;
            }
        }
    }
}

async fn run_upgrade(ssh_config_path: &Path, ssh_host: &str, agent_name: &str) {
    tracing::info!(agent = %agent_name, "checking for claude upgrade");

    // Ensure native install metadata is registered. The sandbox image ships
    // claude via npm (/usr/local/bin/claude), but `claude upgrade` installs a
    // native build to .local/bin/. Without `claude install` first, upgrade
    // warns "config install method is 'unknown'". Idempotent — no-ops if
    // already installed.
    if let Err(e) = rightclaw::openshell::ssh_exec(
        ssh_config_path,
        ssh_host,
        &["claude", "install"],
        UPGRADE_TIMEOUT_SECS,
    )
    .await
    {
        tracing::error!(agent = %agent_name, "claude install failed: {e:#}");
        return;
    }

    let result = rightclaw::openshell::ssh_exec(
        ssh_config_path,
        ssh_host,
        &["claude", "upgrade"],
        UPGRADE_TIMEOUT_SECS,
    )
    .await;

    match result {
        Ok(stdout) => {
            let stdout = stdout.trim();
            if stdout.contains("Successfully updated") {
                tracing::info!(agent = %agent_name, output = %stdout, "claude upgraded");
            } else if stdout.contains("already") || stdout.contains("up to date") {
                tracing::info!(agent = %agent_name, "claude is up to date");
            } else {
                tracing::info!(agent = %agent_name, output = %stdout, "claude upgrade completed");
            }
        }
        Err(e) => {
            tracing::error!(agent = %agent_name, "claude upgrade failed: {e:#}");
        }
    }
}
