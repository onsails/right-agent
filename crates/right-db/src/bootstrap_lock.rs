use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::{FileExt, TryLockError};

use crate::DbError;

const LOCK_FILE_NAME: &str = ".right-db-migrate.lock";
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_WAIT_LOG_THRESHOLD: Duration = Duration::from_secs(1);

pub(crate) async fn acquire(agent_path: &Path) -> Result<BootstrapLockGuard, DbError> {
    let path = agent_path.join(LOCK_FILE_NAME);
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|source| migration_lock_error(&path, source))?;

    acquire_file(path, file).await
}

async fn acquire_file(path: PathBuf, file: File) -> Result<BootstrapLockGuard, DbError> {
    let started = tokio::time::Instant::now();
    let mut logged_wait = false;

    loop {
        match FileExt::try_lock(&file) {
            Ok(()) => return Ok(BootstrapLockGuard { path, file }),
            Err(TryLockError::WouldBlock) => {
                let waited = started.elapsed();
                if waited >= LOCK_TIMEOUT {
                    return Err(migration_lock_error(
                        &path,
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!(
                                "timed out after {}ms waiting for exclusive migration lock",
                                LOCK_TIMEOUT.as_millis()
                            ),
                        ),
                    ));
                }

                if !logged_wait && waited >= LOCK_WAIT_LOG_THRESHOLD {
                    logged_wait = true;
                    tracing::warn!(
                        path = %path.display(),
                        pid = std::process::id(),
                        process = %current_process_name(),
                        waited_ms = waited.as_millis(),
                        "waiting for database migration lock",
                    );
                }

                tokio::time::sleep(std::cmp::min(
                    LOCK_RETRY_DELAY,
                    LOCK_TIMEOUT.saturating_sub(waited),
                ))
                .await;
            }
            Err(TryLockError::Error(source)) => {
                return Err(migration_lock_error(&path, source));
            }
        }
    }
}

pub(crate) struct BootstrapLockGuard {
    path: PathBuf,
    file: File,
}

impl Drop for BootstrapLockGuard {
    fn drop(&mut self) {
        if let Err(source) = FileExt::unlock(&self.file) {
            tracing::warn!(
                path = %self.path.display(),
                error = %source,
                "failed to release database migration lock",
            );
        }
    }
}

fn migration_lock_error(path: &Path, source: io::Error) -> DbError {
    DbError::MigrationLock {
        path: path.to_path_buf(),
        source,
    }
}

fn current_process_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}
