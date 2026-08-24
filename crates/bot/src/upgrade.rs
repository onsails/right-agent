//! Background task that periodically upgrades Claude Code inside a sandbox.
//!
//! Runs `claude upgrade` in the guest every 8 hours. The upgraded binary is
//! installed to `/sandbox/.local/bin/claude` and takes precedence over the
//! image-baked `/usr/local/bin/claude` via PATH ordering (set up by `sync.rs`).

use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use right_sandbox::{ExecRequest, GUEST_HOME, GUEST_USER};

use crate::sandbox::Sandbox;

/// Default interval between upgrade checks (8 hours).
const UPGRADE_INTERVAL: Duration = Duration::from_secs(8 * 3600);

/// Timeout for the in-guest `claude upgrade` (2 minutes).
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(120);
const CLAUDE_UPGRADE_SCRIPT: &str = "exec claude upgrade 2>&1";
const BUN_INSTALL: &str = "/sandbox/.bun";
const UPGRADE_PATH: &str = "/sandbox/.local/bin:/sandbox/.bun/bin:/usr/local/bin:/usr/bin:/bin";

// The shell text is platform-owned and sources no guest files. PATH lookup is
// intentional: prefer the guest-owned upgraded binary, then fall back to the
// image-baked binary, while the explicit user keeps either execution unprivileged.
fn claude_upgrade_request() -> ExecRequest {
    ExecRequest {
        cmd: "/bin/sh".to_owned(),
        args: vec!["-c".to_owned(), CLAUDE_UPGRADE_SCRIPT.to_owned()],
        cwd: Some(GUEST_HOME.to_owned()),
        user: Some(GUEST_USER.to_owned()),
        env: vec![
            ("HOME".to_owned(), GUEST_HOME.to_owned()),
            ("BUN_INSTALL".to_owned(), BUN_INSTALL.to_owned()),
            ("PATH".to_owned(), UPGRADE_PATH.to_owned()),
        ],
        timeout: Some(UPGRADE_TIMEOUT),
        ..ExecRequest::default()
    }
}

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

    match sandbox.exec(&claude_upgrade_request()).await {
        Ok(outcome) => {
            let mut output = String::from_utf8_lossy(&outcome.stdout).into_owned();
            if !outcome.stderr.is_empty() {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str(&String::from_utf8_lossy(&outcome.stderr));
            }
            let exit_code = outcome.code;
            let last_line = last_meaningful_line(&output);
            match classify_claude_upgrade(exit_code, &output) {
                ClaudeUpgradeResult::Updated => {
                    tracing::info!(agent = %agent_name, output = %last_line, "claude upgraded");
                }
                ClaudeUpgradeResult::UpToDate => {
                    tracing::info!(agent = %agent_name, output = %last_line, "claude is up to date");
                }
                ClaudeUpgradeResult::ConfigRepaired => {
                    tracing::info!(agent = %agent_name, output = %last_line, "claude upgrade configuration repaired");
                }
                ClaudeUpgradeResult::Completed => {
                    tracing::info!(agent = %agent_name, output = %last_line, "claude upgrade completed");
                }
                ClaudeUpgradeResult::Failed => {
                    tracing::error!(
                        agent = %agent_name,
                        exit_code,
                        output = %last_line,
                        "claude upgrade exited non-zero"
                    );
                }
            }
        }
        Err(e) => {
            tracing::error!(agent = %agent_name, "claude upgrade failed: {e:#}");
        }
    }
}

const CLAUDE_UP_TO_DATE_LINE: &str = "Claude Code is up to date";
const CLAUDE_CONFIG_REPAIRED_LINE: &str = "Installation method set to: native";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeUpgradeResult {
    Updated,
    UpToDate,
    ConfigRepaired,
    Completed,
    Failed,
}

fn classify_claude_upgrade(exit_code: i32, stdout: &str) -> ClaudeUpgradeResult {
    let last_line = last_meaningful_line(stdout);
    if exit_code == 0 {
        if stdout.contains("Successfully updated") {
            ClaudeUpgradeResult::Updated
        } else if last_line == CLAUDE_UP_TO_DATE_LINE {
            ClaudeUpgradeResult::UpToDate
        } else if last_line == CLAUDE_CONFIG_REPAIRED_LINE {
            ClaudeUpgradeResult::ConfigRepaired
        } else {
            ClaudeUpgradeResult::Completed
        }
    } else if exit_code == 1
        && (!stdout.contains("Updating to") || stdout.contains("Successfully updated"))
        && last_line == CLAUDE_UP_TO_DATE_LINE
    {
        ClaudeUpgradeResult::UpToDate
    } else if exit_code == 1
        && (!stdout.contains("Updating to") || stdout.contains("Successfully updated"))
        && last_line == CLAUDE_CONFIG_REPAIRED_LINE
    {
        ClaudeUpgradeResult::ConfigRepaired
    } else {
        ClaudeUpgradeResult::Failed
    }
}

fn last_meaningful_line(output: &str) -> &str {
    output
        .lines()
        .rev()
        .find_map(|line| {
            let line = line.trim();
            (!line.is_empty()).then_some(line)
        })
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::RwLock;
    #[test]
    fn claude_upgrade_request_is_unprivileged_and_ignores_guest_startup_files() {
        let request = super::claude_upgrade_request();

        assert_eq!(request.user.as_deref(), Some(right_sandbox::GUEST_USER));
        assert_eq!(request.cmd, "/bin/sh");
        assert_eq!(request.args, ["-c", "exec claude upgrade 2>&1"]);
        assert_eq!(request.cwd.as_deref(), Some(right_sandbox::GUEST_HOME));
        assert_eq!(request.timeout, Some(super::UPGRADE_TIMEOUT));
        assert_eq!(request.stdin, right_sandbox::Stdin::Null);
        assert_eq!(
            request.env,
            [
                ("HOME".to_owned(), "/sandbox".to_owned()),
                ("BUN_INSTALL".to_owned(), "/sandbox/.bun".to_owned()),
                (
                    "PATH".to_owned(),
                    "/sandbox/.local/bin:/sandbox/.bun/bin:/usr/local/bin:/usr/bin:/bin".to_owned(),
                ),
            ]
        );
        assert!(
            request
                .args
                .iter()
                .all(|arg| !arg.contains("env.sh") && !arg.contains(".bashrc")),
            "upgrade must not source or execute guest-controlled startup files"
        );
        let source = include_str!("upgrade.rs");
        let root_helper = ["exec_argv", "_with_timeout"].concat();
        assert!(
            !source.contains(&root_helper),
            "upgrade must not use the root exec helper"
        );
    }

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

    #[test]
    fn claude_upgrade_accepts_explicit_up_to_date_exit_one() {
        let stdout = "Current version: 2.1.143\n\nClaude Code is up to date\n";
        assert_eq!(
            super::classify_claude_upgrade(1, stdout),
            super::ClaudeUpgradeResult::UpToDate
        );
    }

    #[test]
    fn claude_upgrade_accepts_exact_config_repair_exit_one() {
        let stdout = "Current version: 2.1.143\n\
            Checking for updates to latest version...\n\
            Warning: Running native installation but config install method is 'not set'\n\
            Updating configuration to track installation method...\n\
            Installation method set to: native\n";
        assert_eq!(
            super::classify_claude_upgrade(1, stdout),
            super::ClaudeUpgradeResult::ConfigRepaired
        );
    }

    #[test]
    fn claude_upgrade_rejects_unrelated_exit_one() {
        assert_eq!(
            super::classify_claude_upgrade(1, "network failed"),
            super::ClaudeUpgradeResult::Failed
        );
    }

    #[test]
    fn claude_upgrade_rejects_current_version_then_failed_update() {
        let stdout = "Current version: 2.1.234\n\
            Checking for updates to latest version...\n\
            Updating to 2.1.241...\n\
            network failed\n";
        assert_eq!(
            super::classify_claude_upgrade(1, stdout),
            super::ClaudeUpgradeResult::Failed
        );
    }

    #[test]
    fn claude_upgrade_rejects_current_version_then_generic_failure() {
        let stdout = "Current version: 2.1.234\nnetwork failed\n";
        assert_eq!(
            super::classify_claude_upgrade(1, stdout),
            super::ClaudeUpgradeResult::Failed
        );
    }

    #[test]
    fn claude_upgrade_rejects_benign_terminal_after_incomplete_update() {
        let stdout = "Current version: 2.1.234\n\
            Updating to 2.1.241...\n\
            Claude Code is up to date\n";
        assert_eq!(
            super::classify_claude_upgrade(1, stdout),
            super::ClaudeUpgradeResult::Failed
        );
    }

    #[test]
    fn claude_upgrade_exit_zero_succeeds() {
        assert_eq!(
            super::classify_claude_upgrade(0, "unexpected terminal text"),
            super::ClaudeUpgradeResult::Completed
        );
    }

    #[test]
    fn last_meaningful_line_ignores_trailing_blanks() {
        assert_eq!(super::last_meaningful_line("first\nlast\n \n"), "last");
    }
}
