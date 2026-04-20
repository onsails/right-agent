//! Background task that periodically upgrades Claude Code inside a sandbox.
//!
//! Runs `claude upgrade` via SSH every 8 hours. The upgraded binary is installed
//! to `/sandbox/.local/bin/claude` and takes precedence over the image-baked
//! `/usr/local/bin/claude` via PATH ordering (set up by `sync.rs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Default interval between upgrade checks (8 hours).
const UPGRADE_INTERVAL: Duration = Duration::from_secs(8 * 3600);

/// Timeout for `claude upgrade` SSH command (2 minutes).
const UPGRADE_TIMEOUT_SECS: u64 = 120;

/// Run a single upgrade attempt at startup (blocking).
/// Called before cron/telegram tasks exist — no lock needed.
pub async fn run_startup_upgrade(ssh_config_path: &Path, agent_name: &str) {
    let ssh_host = rightclaw::openshell::ssh_host(agent_name);
    run_upgrade(ssh_config_path, &ssh_host, agent_name).await;
}

/// Spawn a background task that periodically runs `claude upgrade` in the sandbox.
///
/// Runs every 8 hours (first tick consumed since startup upgrade already ran).
/// Errors are logged but never propagated — the task keeps running.
pub fn spawn_upgrade_task(
    ssh_config_path: PathBuf,
    agent_name: String,
    shutdown: CancellationToken,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
) {
    tokio::spawn(async move {
        run_upgrade_loop(&ssh_config_path, &agent_name, shutdown, &upgrade_lock).await;
    });
}

async fn run_upgrade_loop(
    ssh_config_path: &Path,
    agent_name: &str,
    shutdown: CancellationToken,
    upgrade_lock: &tokio::sync::RwLock<()>,
) {
    let ssh_host = rightclaw::openshell::ssh_host(agent_name);
    let mut interval = tokio::time::interval(UPGRADE_INTERVAL);
    // First tick fires immediately — consume it since startup upgrade already ran.
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown.cancelled() => {
                tracing::info!(agent = %agent_name, "upgrade task shutting down");
                return;
            }
        }

        // try_write: skip if any CC session holds a read lock.
        let Ok(_guard) = upgrade_lock.try_write() else {
            tracing::info!(agent = %agent_name, "skipping upgrade — active sessions");
            continue;
        };

        run_upgrade(ssh_config_path, &ssh_host, agent_name).await;
        // _guard dropped here — CC sessions unblock
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn upgrade_skips_when_sessions_active() {
        let lock = Arc::new(RwLock::new(()));
        let _read_guard = lock.read().await;
        assert!(lock.try_write().is_err());
    }

    #[tokio::test]
    async fn upgrade_runs_when_idle() {
        let lock = Arc::new(RwLock::new(()));
        assert!(lock.try_write().is_ok());
    }

    #[tokio::test]
    async fn sessions_block_during_upgrade() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let lock = Arc::new(RwLock::new(()));
        let write_guard = lock.write().await;
        let blocked = Arc::new(AtomicBool::new(true));
        let blocked_clone = Arc::clone(&blocked);
        let lock_clone = Arc::clone(&lock);

        let handle = tokio::spawn(async move {
            let _read = lock_clone.read().await;
            blocked_clone.store(false, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(blocked.load(Ordering::SeqCst), "reader should be blocked");

        drop(write_guard);
        handle.await.unwrap();
        assert!(
            !blocked.load(Ordering::SeqCst),
            "reader should have proceeded"
        );
    }
}
