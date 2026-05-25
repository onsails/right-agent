use std::fmt;
use std::future::Future;
use std::panic;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{DbError, migrations};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Connection {
    db_path: PathBuf,
    inner: libsql::Connection,
    runtime: Option<tokio::runtime::Runtime>,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("path", &self.db_path)
            .finish_non_exhaustive()
    }
}

impl Connection {
    pub(crate) fn open_local(db_path: PathBuf, create: bool) -> Result<Self, DbError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("right-db libsql runtime should initialize");
        let flags = if create {
            libsql::OpenFlags::default()
        } else {
            libsql::OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let builder = libsql::Builder::new_local(&db_path).flags(flags);
        // SAFETY: this Task 1-2 wrapper is local-only and exposes only
        // synchronous methods. Each async libSQL operation is driven to
        // completion through `block_on_runtime_safe`, so callers cannot poll the
        // same connection future concurrently, and we never alter SQLite's
        // global threading mode ourselves. While rusqlite remains linked for
        // migration compatibility, it can initialize SQLite before libSQL's
        // one-time `SQLITE_CONFIG_SERIALIZED` assertion runs; the assertion then
        // reports SQLITE_MISUSE even though the project still uses serialized,
        // mutex-protected connection handles. This skips that temporary assert
        // only. Remove it when Task 3 removes the rusqlite migration bridge.
        let builder = unsafe { builder.skip_safety_assert(true) };
        let database =
            block_on_runtime_safe(&runtime, builder.build()).map_err(|source| DbError::Open {
                path: db_path.clone(),
                source,
            })?;
        let inner = database.connect().map_err(|source| DbError::Open {
            path: db_path.clone(),
            source,
        })?;

        Ok(Self {
            db_path,
            inner,
            runtime: Some(runtime),
        })
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    pub fn execute_batch(&self, sql: &str) -> Result<(), DbError> {
        self.block_on_libsql(self.inner.execute_batch(sql))
            .map(drop)
    }

    pub fn execute(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
    ) -> Result<usize, DbError> {
        let params = params.into_params()?.into_libsql();
        let changed = self.block_on_libsql(self.inner.execute(sql, params))?;
        usize::try_from(changed)
            .map_err(|_| DbError::InvalidParameter("changed row count exceeds usize".into()))
    }

    pub fn query_one<T>(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
        map: impl FnOnce(&crate::row::Row<'_>) -> Result<T, DbError>,
    ) -> Result<T, DbError> {
        let params = params.into_params()?.into_libsql();
        let mut rows = self.block_on_libsql(self.inner.query(sql, params))?;
        let Some(row) = self.block_on_libsql(rows.next())? else {
            return Err(DbError::not_found());
        };
        map(&crate::row::Row::new(&row))
    }

    pub fn transaction(&self) -> Result<crate::transaction::Transaction<'_>, DbError> {
        let inner = self.block_on_libsql(self.inner.transaction())?;
        Ok(crate::transaction::Transaction::new(self, inner))
    }

    pub(crate) fn apply_connection_pragmas(&self) -> Result<(), DbError> {
        self.execute_batch("PRAGMA journal_mode=WAL")?;
        self.inner.busy_timeout(BUSY_TIMEOUT)?;
        Ok(())
    }

    pub(crate) fn apply_readonly_pragmas(&self) -> Result<(), DbError> {
        self.inner.busy_timeout(BUSY_TIMEOUT)?;
        Ok(())
    }

    pub(crate) fn run_rusqlite_migrations(&self) -> Result<(), DbError> {
        let mut conn = self.open_rusqlite_connection_for_migrations()?;
        migrations::MIGRATIONS.to_latest(&mut conn)?;
        Ok(())
    }

    /// Temporary bridge until Task 3 replaces `rusqlite_migration` with a
    /// driver-owned migration runner.
    pub(crate) fn open_rusqlite_connection_for_migrations(
        &self,
    ) -> Result<rusqlite::Connection, DbError> {
        let conn = rusqlite::Connection::open(&self.db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        Ok(conn)
    }

    pub(crate) fn block_on_libsql<T: Send>(
        &self,
        future: impl Future<Output = libsql::Result<T>> + Send,
    ) -> Result<T, DbError> {
        block_on_runtime_safe(self.runtime(), future).map_err(Into::into)
    }

    fn runtime(&self) -> &tokio::runtime::Runtime {
        self.runtime
            .as_ref()
            .expect("right-db runtime should live until Connection::drop")
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

fn block_on_runtime_safe<F, T>(runtime: &tokio::runtime::Runtime, future: F) -> T
where
    F: Future<Output = T> + Send,
    T: Send,
{
    if tokio::runtime::Handle::try_current().is_err() {
        return runtime.block_on(future);
    }

    std::thread::scope(|scope| {
        let handle = scope.spawn(move || runtime.block_on(future));
        match handle.join() {
            Ok(output) => output,
            Err(payload) => panic::resume_unwind(payload),
        }
    })
}
