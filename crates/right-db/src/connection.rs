use std::fmt;
use std::future::Future;
use std::panic;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::DbError;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Connection {
    db_path: PathBuf,
    inner: libsql::Connection,
    runtime: LibsqlRuntime,
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
        let runtime = LibsqlRuntime::new();
        let flags = if create {
            libsql::OpenFlags::default()
        } else {
            libsql::OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let builder = libsql::Builder::new_local(&db_path).flags(flags);
        // SAFETY: right-db is only using local libSQL through synchronous
        // wrappers that drive each async operation to completion. During the
        // staged libSQL migration this crate still links and uses rusqlite in
        // compatibility tests, so rusqlite can initialize SQLite
        // before libSQL's one-time serialized-mode assertion runs. The project
        // still uses serialized, mutex-protected handles; this skips only that
        // temporary global init assertion until the remaining right-db rusqlite
        // surfaces are removed.
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

    pub fn query_all<T>(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
        mut map: impl FnMut(&crate::row::Row<'_>) -> Result<T, DbError>,
    ) -> Result<Vec<T>, DbError> {
        let params = params.into_params()?.into_libsql();
        let mut rows = self.block_on_libsql(self.inner.query(sql, params))?;
        let mut values = Vec::new();
        while let Some(row) = self.block_on_libsql(rows.next())? {
            values.push(map(&crate::row::Row::new(&row))?);
        }
        Ok(values)
    }

    pub fn transaction(&self) -> Result<crate::transaction::Transaction<'_>, DbError> {
        self.transaction_with_behavior(libsql::TransactionBehavior::Deferred)
    }

    pub fn with_immediate_transaction<T>(
        &self,
        f: impl FnOnce(&crate::transaction::Transaction<'_>) -> Result<T, DbError>,
    ) -> Result<T, DbError> {
        let tx = self.transaction_with_behavior(libsql::TransactionBehavior::Immediate)?;
        match f(&tx) {
            Ok(value) => {
                tx.commit()?;
                Ok(value)
            }
            Err(err) => {
                let rollback = tx.rollback();
                Err(preserve_transaction_error(err, rollback))
            }
        }
    }

    fn transaction_with_behavior(
        &self,
        behavior: libsql::TransactionBehavior,
    ) -> Result<crate::transaction::Transaction<'_>, DbError> {
        let inner = self.block_on_libsql(self.inner.transaction_with_behavior(behavior))?;
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

    pub(crate) fn block_on_libsql<T: Send>(
        &self,
        future: impl Future<Output = libsql::Result<T>> + Send,
    ) -> Result<T, DbError> {
        self.runtime.block_on(future).map_err(Into::into)
    }
}

fn preserve_transaction_error(original: DbError, rollback: Result<(), DbError>) -> DbError {
    match rollback {
        Ok(()) | Err(_) => original,
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

struct LibsqlRuntime {
    runtime: Option<tokio::runtime::Runtime>,
}

impl LibsqlRuntime {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("right-db libsql runtime should initialize");

        Self {
            runtime: Some(runtime),
        }
    }

    fn block_on<F, T>(&self, future: F) -> T
    where
        F: Future<Output = T> + Send,
        T: Send,
    {
        let runtime = self
            .runtime
            .as_ref()
            .expect("right-db libsql runtime should live until guard drop");
        block_on_runtime_safe(runtime, future)
    }
}

impl Drop for LibsqlRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_error_result_prefers_operation_error_when_rollback_fails() {
        let operation = DbError::InvalidParameter("operation failed".into());
        let rollback = DbError::InvalidParameter("rollback failed".into());

        let err = preserve_transaction_error(operation, Err(rollback));

        assert_eq!(
            err.to_string(),
            "invalid database parameter: operation failed",
        );
    }
}
