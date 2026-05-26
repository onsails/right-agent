use std::sync::Arc;

use turso_core::io::{Clock, Completion, File, OpenFlags};

/// IO adapter for filesystem-backed Turso databases opened in multiprocess-WAL
/// mode. Turso's shared WAL coordination owns cross-process safety here; the
/// legacy DB/WAL file lock must stay disabled or a sibling process cannot open
/// the same per-agent `data.db`.
pub(crate) fn new() -> Result<Arc<dyn turso_core::IO>, turso_core::LimboError> {
    Ok(Arc::new(MultiprocessWalIo {
        inner: turso_core::io::PlatformIO::new()?,
    }))
}

struct MultiprocessWalIo {
    inner: turso_core::io::PlatformIO,
}

impl Clock for MultiprocessWalIo {
    fn current_time_monotonic(&self) -> turso_core::io::clock::MonotonicInstant {
        self.inner.current_time_monotonic()
    }

    fn current_time_wall_clock(&self) -> turso_core::io::clock::WallClockInstant {
        self.inner.current_time_wall_clock()
    }
}

impl turso_core::IO for MultiprocessWalIo {
    fn open_file(
        &self,
        path: &str,
        flags: OpenFlags,
        direct: bool,
    ) -> Result<Arc<dyn File>, turso_core::LimboError> {
        self.inner
            .open_file(path, flags | OpenFlags::NoLock, direct)
    }

    fn remove_file(&self, path: &str) -> Result<(), turso_core::LimboError> {
        self.inner.remove_file(path)
    }

    fn supports_shared_wal_coordination(&self) -> bool {
        self.inner.supports_shared_wal_coordination()
    }

    fn step(&self) -> Result<(), turso_core::LimboError> {
        self.inner.step()
    }

    fn cancel(&self, c: &[Completion]) -> Result<(), turso_core::LimboError> {
        self.inner.cancel(c)
    }

    fn file_id(&self, path: &str) -> Result<turso_core::io::FileId, turso_core::LimboError> {
        self.inner.file_id(path)
    }
}
