use std::path::PathBuf;

/// Errors from per-agent database operations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Database(#[from] turso::Error),

    #[error("database row not found")]
    NotFound,

    #[error("invalid database parameter: {0}")]
    InvalidParameter(String),

    #[error("database constraint violation: {0}")]
    Constraint(String),

    #[error("open database {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: turso::Error,
    },

    #[error("remove WAL sidecar {path}: {source}")]
    SidecarRemove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("database migration lock {path}: {source}")]
    MigrationLock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("migration {version} on {path}: {source}")]
    Migration {
        path: PathBuf,
        version: u32,
        #[source]
        source: Box<DbError>,
    },

    #[error("migration version {version} on {path}: {message}")]
    MigrationVersion {
        path: PathBuf,
        version: u32,
        message: String,
    },
}

impl DbError {
    pub fn is_open_error(&self) -> bool {
        matches!(self, Self::Open { .. })
    }

    pub fn is_constraint_violation(&self) -> bool {
        match self {
            Self::Constraint(_) => true,
            Self::Database(error) => is_turso_constraint(error),
            Self::Migration { source, .. } => source.is_constraint_violation(),
            _ => false,
        }
    }

    /// True if the error is a retryable lock contention (SQLite `BUSY` /
    /// `BUSY_SNAPSHOT`). Callers retrying their own write loop should use
    /// this predicate instead of stringly classifying the `Display` form.
    pub fn is_transient(&self) -> bool {
        self.transient_kind().is_some()
    }

    pub(crate) fn transient_kind(&self) -> Option<DbTransientKind> {
        match self {
            Self::Database(error) => turso_transient_kind(error),
            Self::Open { source, .. } => turso_transient_kind(source),
            Self::Migration { source, .. } => source.transient_kind(),
            _ => None,
        }
    }

    /// True if the error is a recoverable WAL-sidecar desync. Turso's
    /// experimental multiprocess WAL (tursodatabase/turso#769) can leave a stale
    /// `-tshm` authority that claims frames the `-wal` no longer holds, so every
    /// open fails with "short read on WAL frame". Recoverable by resetting the
    /// `-tshm`/`-shm` sidecars. Deliberately narrow: never matches main-database
    /// corruption, which is NOT sidecar-recoverable.
    pub fn is_wal_corruption(&self) -> bool {
        match self {
            Self::Database(error) => is_turso_wal_corruption(error),
            Self::Open { source, .. } => is_turso_wal_corruption(source),
            Self::Migration { source, .. } => source.is_wal_corruption(),
            _ => false,
        }
    }
}

// The `Turso` prefix is intentional: these are Turso-driver-specific transient
// conditions, and each maps to a stable `turso_*` string id in `as_str()` used
// for telemetry/logging. Keeping the variant names aligned with those ids is
// worth the redundant prefix.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbTransientKind {
    TursoBusy,
    TursoBusySnapshot,
    TursoFileLock,
}

impl DbTransientKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::TursoBusy => "turso_busy",
            Self::TursoBusySnapshot => "turso_busy_snapshot",
            Self::TursoFileLock => "turso_file_lock",
        }
    }
}

fn is_turso_constraint(error: &turso::Error) -> bool {
    matches!(error, turso::Error::Constraint(_))
}

// WORKAROUND (tracked in onsails/right-agent#127): we detect the WAL-sidecar
// desync by substring-matching Turso's stringly-typed `Error::Error` message
// because the driver exposes no typed variant for it. This is brittle to Turso
// message changes. When tursodatabase/turso#769 is fixed upstream — either a
// typed short-read/corruption error, or the authority-rebuild no longer leaving
// a stale `-tshm` against an empty `-wal` — revisit: prefer matching the typed
// error here, and the sidecar-reset recovery in `lib.rs` may become unnecessary.
fn is_turso_wal_corruption(error: &turso::Error) -> bool {
    matches!(
        error,
        turso::Error::Error(message)
            if message.contains("short read on WAL") || message.contains("WAL short read")
    )
}

fn turso_transient_kind(error: &turso::Error) -> Option<DbTransientKind> {
    match error {
        turso::Error::Busy(_) => Some(DbTransientKind::TursoBusy),
        turso::Error::BusySnapshot(_) => Some(DbTransientKind::TursoBusySnapshot),
        // Turso currently maps its lower-level file-locking error into the
        // generic Error variant, so classify only the precise lock owner case.
        turso::Error::Error(message) => {
            message.starts_with("Locking error:")
                && message.contains("File is locked by another process")
        }
        .then_some(DbTransientKind::TursoFileLock),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::Connection;
    use crate::DbError;
    use crate::params;

    #[test]
    fn is_transient_detects_busy_variants() {
        assert!(
            DbError::Database(turso::Error::Busy("locked".into())).is_transient(),
            "Busy must be transient",
        );
        assert!(
            DbError::Database(turso::Error::BusySnapshot("snapshot".into())).is_transient(),
            "BusySnapshot must be transient",
        );
        assert!(
            (DbError::Open {
                path: "data.db".into(),
                source: turso::Error::Busy("locked".into()),
            })
            .is_transient(),
            "Open(Busy) must be transient",
        );
        assert!(
            (DbError::Open {
                path: "data.db".into(),
                source: turso::Error::BusySnapshot("snapshot".into()),
            })
            .is_transient(),
            "Open(BusySnapshot) must be transient",
        );
        assert!(
            (DbError::Open {
                path: "data.db".into(),
                source: turso::Error::Error(
                    "Locking error: Failed locking file '/tmp/data.db'. File is locked by another process"
                        .into()
                ),
            })
            .is_transient(),
            "Turso file locking errors must be transient",
        );
        assert!(
            !DbError::Database(turso::Error::Constraint("unique".into())).is_transient(),
            "Constraint is not transient",
        );
        assert!(
            !DbError::NotFound.is_transient(),
            "NotFound is not transient",
        );
    }

    #[test]
    fn is_wal_corruption_matches_short_read_only() {
        let msg =
            "I/O error: short read on WAL frame at offset 2566792: expected 4096 bytes, got 0";
        assert!(
            DbError::Database(turso::Error::Error(msg.into())).is_wal_corruption(),
            "bare turso short-read must be WAL corruption",
        );
        assert!(
            (DbError::Open {
                path: "data.db".into(),
                source: turso::Error::Error(msg.into()),
            })
            .is_wal_corruption(),
            "Open(short-read) must be WAL corruption",
        );
        let wal_short = "I/O error: WAL short read at offset 4096, page 1, frame_id=1: expected 4096 bytes, got 0";
        assert!(
            DbError::Database(turso::Error::Error(wal_short.into())).is_wal_corruption(),
            "the 'WAL short read' variant must also be WAL corruption",
        );
        assert!(
            (DbError::Migration {
                path: "data.db".into(),
                version: 1,
                source: Box::new(DbError::Database(turso::Error::Error(msg.into()))),
            })
            .is_wal_corruption(),
            "Migration-wrapped short-read must be WAL corruption",
        );
        // Negatives: transient, constraint, not-found, and main-db corruption.
        assert!(!DbError::Database(turso::Error::Busy("locked".into())).is_wal_corruption());
        assert!(!DbError::Database(turso::Error::Constraint("unique".into())).is_wal_corruption());
        assert!(!DbError::NotFound.is_wal_corruption());
        assert!(
            !DbError::Database(turso::Error::Error("database header magic mismatch".into()))
                .is_wal_corruption(),
            "main-database corruption is NOT sidecar-recoverable",
        );
        assert!(
            !DbError::SidecarRemove {
                path: "data.db-tshm".into(),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            }
            .is_wal_corruption(),
            "sidecar-removal failure must not re-trigger recovery",
        );
    }

    #[tokio::test]
    async fn is_constraint_violation_detects_unique_violation() {
        let conn = Connection::open_in_memory()
            .await
            .expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE constraint_probe (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL UNIQUE
             )",
        )
        .await
        .expect("create table");

        conn.execute(
            "INSERT INTO constraint_probe (name) VALUES (?1)",
            params!["duplicate-name"],
        )
        .await
        .expect("first insert succeeds");

        let err = conn
            .execute(
                "INSERT INTO constraint_probe (name) VALUES (?1)",
                params!["duplicate-name"],
            )
            .await
            .expect_err("duplicate insert must fail");

        assert!(
            err.is_constraint_violation(),
            "expected constraint violation, got: {err:#}",
        );
    }
}
