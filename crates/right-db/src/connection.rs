use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{DbError, migrations};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Connection {
    db_path: PathBuf,
    _database: libsql::Database,
    inner: libsql::Connection,
    runtime: tokio::runtime::Runtime,
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
        // Temporary while rusqlite remains linked for migrations/tests: rusqlite
        // can initialize SQLite before libSQL's safety configuration runs.
        // Task 3 removes this bridge with rusqlite_migration.
        let builder = unsafe { builder.skip_safety_assert(true) };
        let database = runtime
            .block_on(builder.build())
            .map_err(|source| DbError::Open {
                path: db_path.clone(),
                source,
            })?;
        let inner = database.connect().map_err(|source| DbError::Open {
            path: db_path.clone(),
            source,
        })?;

        Ok(Self {
            db_path,
            _database: database,
            inner,
            runtime,
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

    pub(crate) fn block_on_libsql<T>(
        &self,
        future: impl Future<Output = libsql::Result<T>>,
    ) -> Result<T, DbError> {
        self.runtime.block_on(future).map_err(Into::into)
    }
}
