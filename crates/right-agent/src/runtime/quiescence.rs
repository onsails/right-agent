//! Project-wide exclusion for runtime startup and offline database access.

use std::fs::{File, OpenOptions};

use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::{FileExt, TryLockError};

use super::PcClient;

const LOCK_FILE_NAME: &str = ".right-runtime.lock";
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Exclusive project-wide guard shared by runtime startup and offline database commands.
///
/// Holding this guard prevents `right up` from publishing a new runtime while an
/// offline command has a direct database handle (and vice versa).
pub struct RuntimeExclusionGuard {
    file: File,
    path: PathBuf,
}

impl Drop for RuntimeExclusionGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(path = %self.path.display(), %error, "failed to release runtime exclusion lock");
        }
    }
}

/// Acquire the startup/offline exclusion lock without making a runtime-state assertion.
///
/// `right up` and the typed db-repair shutdown flow use this primitive. Ordinary
/// offline database commands must use [`require_runtime_quiesced`] instead.
pub async fn acquire_runtime_exclusion(home: &Path) -> miette::Result<RuntimeExclusionGuard> {
    std::fs::create_dir_all(home).map_err(|error| {
        miette::miette!(
            "create Right home {} for runtime lock: {error:#}",
            home.display()
        )
    })?;
    let path = home.join(LOCK_FILE_NAME);
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| miette::miette!("open runtime lock {}: {error:#}", path.display()))?;
    let started = tokio::time::Instant::now();
    loop {
        match FileExt::try_lock(&file) {
            Ok(()) => return Ok(RuntimeExclusionGuard { file, path }),
            Err(TryLockError::WouldBlock) => {
                let waited = started.elapsed();
                if waited >= LOCK_TIMEOUT {
                    return Err(miette::miette!(
                        "timed out after {:?} waiting for runtime exclusion lock {}",
                        LOCK_TIMEOUT,
                        path.display()
                    ));
                }
                tokio::time::sleep(std::cmp::min(
                    LOCK_RETRY_DELAY,
                    LOCK_TIMEOUT.saturating_sub(waited),
                ))
                .await;
            }
            Err(TryLockError::Error(error)) => {
                return Err(miette::miette!(
                    "acquire runtime exclusion lock {}: {error:#}",
                    path.display()
                ));
            }
        }
    }
}

/// Acquire project-wide exclusion and prove that no recorded runtime is active.
///
/// Absence of `run/state.json` is accepted only after the lock is held. A
/// retained state file is fail-closed: whether its process-compose endpoint is
/// healthy or unreachable, an ordinary offline command must not open a database.
pub async fn require_runtime_quiesced(home: &Path) -> miette::Result<RuntimeExclusionGuard> {
    let guard = acquire_runtime_exclusion(home).await?;
    if let Some(client) = PcClient::from_home(home)? {
        match client.health_check().await {
            Ok(()) => {
                return Err(miette::miette!(
                    "offline database access requires the Right runtime to be quiesced; runtime is active"
                ));
            }
            Err(error) => {
                return Err(miette::miette!(
                    "runtime state exists but process-compose is unreachable; refusing offline database access (fail closed): {error:#}"
                ));
            }
        }
    }
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn absent_state_is_quiesced_while_guard_is_held() {
        let home = tempfile::tempdir().unwrap();
        let guard = require_runtime_quiesced(home.path()).await.unwrap();
        let second = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(home.path().join(LOCK_FILE_NAME))
            .unwrap();
        assert!(matches!(
            FileExt::try_lock(&second),
            Err(TryLockError::WouldBlock)
        ));
        drop(guard);
        FileExt::try_lock(&second).unwrap();
    }

    #[tokio::test]
    async fn retained_state_fails_closed() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        std::fs::create_dir(&run).unwrap();
        std::fs::write(
            run.join("state.json"),
            r#"{"agents":[],"socket_path":"","started_at":"x","pc_port":1,"pc_api_token":"x"}"#,
        )
        .unwrap();
        assert!(require_runtime_quiesced(home.path()).await.is_err());
    }
}
