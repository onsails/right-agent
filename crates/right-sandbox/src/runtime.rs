//! Runtime installation and host preflight.
//!
//! Right has no user-visible install step and no PATH dependency: the pinned
//! `msb`/`libkrunfw` runtime is downloaded and verified into `~/.microsandbox`
//! by the SDK's `setup` module on first use.

use std::time::Duration;

use microsandbox::setup;
use tokio::sync::OnceCell;

use crate::error::SandboxError;

/// Install-once cell: success is cached for the process; failure is not, so
/// the next caller retries (mirrors the stage-1 probe helper).
static RUNTIME_INSTALL: OnceCell<()> = OnceCell::const_new();

/// Name of the cross-process advisory install lock under `$TMPDIR`. Stable:
/// every Right process on the host, from any worktree, must share one lock.
const INSTALL_LOCK_KEY: &str = "rt-msb-runtime-install";

/// Backoff between attempts to take the install lock.
const INSTALL_LOCK_RETRY: Duration = Duration::from_millis(200);

/// Ensure the SDK's pinned `msb`/`libkrunfw` runtime is installed.
///
/// `setup::is_installed()` is a file-presence check, so the fast path costs
/// two `stat` calls. The download itself is guarded by a cross-process
/// advisory lock: several Right processes (bots, CLI, test binaries from
/// sibling worktrees) may hit a first-run install concurrently, and the
/// install directory is host-global.
pub async fn ensure_runtime_installed() -> Result<(), SandboxError> {
    RUNTIME_INSTALL.get_or_try_init(install_once).await?;
    Ok(())
}

/// Preflight the host's ability to run microVMs.
///
/// Returns `Err(SandboxError::HypervisorUnavailable)` when the SDK's
/// diagnosis reports blocking problems (no hypervisor, `/dev/kvm` missing or
/// unreadable, unsupported platform). The bot's startup calls this after
/// [`ensure_runtime_installed`] so a hypervisor-less host gets a clear
/// message instead of an opaque boot failure.
pub fn diagnose_host() -> Result<(), SandboxError> {
    let diagnosis = setup::diagnose();
    if diagnosis.is_healthy() {
        return Ok(());
    }
    let mut summary = Vec::with_capacity(diagnosis.problems.len());
    let mut fixes = Vec::new();
    for problem in &diagnosis.problems {
        summary.push(problem.headline.clone());
        if let Some(fix) = &problem.fix {
            fixes.push(fix.description.clone());
            fixes.extend(
                fix.commands
                    .iter()
                    .map(|command| format!("{} {}", command.program, command.args.join(" "))),
            );
        }
    }
    Err(SandboxError::HypervisorUnavailable { summary: summary.join("; "), fixes })
}

/// The actual install path, run at most once per process unless it fails.
async fn install_once() -> Result<(), SandboxError> {
    // Take the cross-process lock unconditionally on this cold path. The
    // OnceCell guards the hot path; here a concurrent process may have a
    // half-extracted install (the SDK's install is not atomic), and the
    // sentinel files can be present before extraction/chmod finish — so the
    // is_installed() check must run under the lock, never before it.
    let _lock = acquire_install_lock().await?;
    if setup::is_installed() {
        return Ok(());
    }
    tracing::info!("installing pinned microsandbox runtime into ~/.microsandbox");
    setup::install().await.map_err(|source| SandboxError::RuntimeInstall {
        source: Box::new(crate::error::SdkError(source)),
    })?;
    if !setup::is_installed() {
        return Err(SandboxError::RuntimeInstallVerify);
    }
    tracing::info!("microsandbox runtime installed");
    Ok(())
}

/// Take the host-global advisory install lock, waiting until free.
///
/// The kernel releases the lock if the holder dies mid-install; the next
/// process re-checks `is_installed()` under the lock, so a partial install is
/// always completed by someone. The wait is async so a concurrent install
/// never blocks a tokio worker thread for the full download.
async fn acquire_install_lock() -> Result<InstallLock, SandboxError> {
    let path = std::env::temp_dir().join(format!("{INSTALL_LOCK_KEY}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|source| SandboxError::RuntimeInstallLock {
            path: path.clone(),
            source,
        })?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(InstallLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(INSTALL_LOCK_RETRY).await;
            }
            Err(std::fs::TryLockError::Error(source)) => {
                return Err(SandboxError::RuntimeInstallLock { path, source });
            }
        }
    }
}


/// The held advisory lock; drop releases it.
struct InstallLock {
    _file: std::fs::File,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SandboxCause;

    #[test]
    fn install_lock_key_is_stable() {
        // The lock must be host-global across worktrees and runs: changing
        // the key splits the lock set and re-admits concurrent installs.
        assert_eq!(INSTALL_LOCK_KEY, "rt-msb-runtime-install");
    }

    #[tokio::test]
    async fn install_lock_can_be_taken() {
        let lock = acquire_install_lock().await.expect("install lock");
        drop(lock);
    }

    #[test]
    fn install_errors_surface_the_runtime_install_cause() {
        let err = SandboxError::RuntimeInstallVerify;
        assert_eq!(err.cause(), Some(SandboxCause::RuntimeInstallFailed));
    }
}
