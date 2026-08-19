//! Background task that periodically upgrades Claude Code inside a sandbox.
//!
//! Runs `claude upgrade` in the guest every 8 hours. The upgraded binary is
//! installed to `/sandbox/.local/bin/claude` and takes precedence over the
//! image-baked `/usr/local/bin/claude` via PATH ordering (set up by `sync.rs`).

use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::sandbox::{Sandbox, exec_argv_with_timeout};

/// Default interval between upgrade checks (8 hours).
const UPGRADE_INTERVAL: Duration = Duration::from_secs(8 * 3600);

/// Timeout for the in-guest `claude upgrade` (2 minutes).
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(120);
const CLAUDE_UPGRADE_SCRIPT: &str = r#"output="$(claude upgrade 2>&1)"
status=$?
printf '%s\n' "$output"
if [ "$status" -eq 1 ]; then
  case "$output" in
    *"Current version"*) exit 0 ;;
  esac
fi
exit "$status"
"#;
const CLAUDE_UPGRADE_CMD: [&str; 3] = ["bash", "-lc", CLAUDE_UPGRADE_SCRIPT];

/// Run a single upgrade attempt at startup (blocking).
/// Called before cron/telegram tasks exist — no lock needed.
pub(crate) async fn run_startup_upgrade(sandbox: &Sandbox, agent_name: &str) {
    run_upgrade(sandbox, agent_name).await;
}

/// Spawn a background task that periodically runs `claude upgrade` in the sandbox.
///
/// Runs every 8 hours (first tick consumed since startup upgrade already ran).
/// Errors are logged but never propagated — the task keeps running.
///
/// Returns the `JoinHandle` so the caller can await it during shutdown,
/// preventing a tokio runtime panic from in-flight `Interval::tick()` futures.
pub(crate) fn spawn_upgrade_task(
    sandbox_runtime: Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    agent_name: String,
    shutdown: CancellationToken,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_upgrade_loop(&sandbox_runtime, &agent_name, shutdown, &upgrade_lock).await;
    })
}

async fn run_upgrade_loop(
    sandbox_runtime: &crate::sandbox_runtime::SandboxRuntimeHandle,
    agent_name: &str,
    shutdown: CancellationToken,
    upgrade_lock: &tokio::sync::RwLock<()>,
) {
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

        // Resolved per attempt: a recovery between ticks retires the previous
        // handle, and there is no host fallback when none is published.
        let Some(sandbox) = sandbox_runtime.current_sandbox() else {
            tracing::info!(agent = %agent_name, "skipping upgrade — sandbox unavailable");
            continue;
        };
        run_upgrade(&sandbox, agent_name).await;
        // _guard dropped here — CC sessions unblock
    }
}

async fn run_upgrade(sandbox: &Sandbox, agent_name: &str) {
    tracing::info!(agent = %agent_name, "checking for claude upgrade");

    match exec_argv_with_timeout(sandbox, &CLAUDE_UPGRADE_CMD, UPGRADE_TIMEOUT).await {
        Ok((stdout, exit_code)) => {
            let stdout = stdout.trim();
            if exit_code != 0 {
                tracing::error!(
                    agent = %agent_name,
                    exit_code,
                    output = %stdout,
                    "claude upgrade exited non-zero"
                );
            } else if stdout.contains("Successfully updated") {
                tracing::info!(agent = %agent_name, output = %stdout, "claude upgraded");
            } else if claude_upgrade_up_to_date(stdout) || stdout.contains("already") {
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
fn claude_upgrade_success(exit: i32, stdout: &str) -> bool {
    exit == 0 || (exit == 1 && claude_upgrade_up_to_date(stdout))
}

fn claude_upgrade_up_to_date(stdout: &str) -> bool {
    stdout.contains("Current version")
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
    async fn upgrade_does_not_run_claude_install_before_upgrade() {
        let src = include_str!("upgrade.rs");
        let bad_pattern = ["[\"", "claude", "\", \"", "install", "\"]"].concat();

        assert!(
            !src.contains(&bad_pattern),
            "upgrade.rs must not run `claude install` before `claude upgrade`; \
             current Claude Code treats install as a slow native-build install, \
             while upgrade already repairs install metadata when needed."
        );
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

    #[tokio::test]
    async fn claude_upgrade_accepts_current_version_exit_one() {
        let stdout = "Current version: 2.1.143\n\nClaude Code is up to date\n";
        assert!(super::claude_upgrade_success(1, stdout));
    }

    #[tokio::test]
    async fn claude_upgrade_accepts_current_version_config_repair_exit_one() {
        let stdout = "Current version: 2.1.143\n\
            Checking for updates to latest version...\n\
            Warning: Running native installation but config install method is 'not set'\n\
            Updating configuration to track installation method...\n\
            Installation method set to: native\n";
        assert!(super::claude_upgrade_success(1, stdout));
    }

    #[tokio::test]
    async fn claude_upgrade_rejects_unrelated_exit_one() {
        assert!(!super::claude_upgrade_success(1, "network failed"));
    }
}
