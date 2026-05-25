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
    pub fn open_in_memory() -> Result<Self, DbError> {
        Self::build(PathBuf::from(":memory:"), libsql::OpenFlags::default())
    }

    pub(crate) fn open_local(db_path: PathBuf, create: bool) -> Result<Self, DbError> {
        let flags = if create {
            libsql::OpenFlags::default()
        } else {
            libsql::OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        Self::build(db_path, flags)
    }

    fn build(db_path: PathBuf, flags: libsql::OpenFlags) -> Result<Self, DbError> {
        let runtime = LibsqlRuntime::new();
        let builder = libsql::Builder::new_local(&db_path).flags(flags);
        // SAFETY: right-db drives local libSQL through synchronous wrappers
        // that own the runtime used for each operation, so libSQL's
        // process-wide serialized-mode assertion would be a false positive.
        // The project still uses local SQLite through libSQL handles with
        // SQLite mutex protection.
        let builder = unsafe { builder.skip_safety_assert(true) };
        let open_err = |source| DbError::Open {
            path: db_path.clone(),
            source,
        };
        let database = runtime.block_on(builder.build()).map_err(open_err)?;
        let inner = database.connect().map_err(open_err)?;
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
            return Err(DbError::NotFound);
        };
        map(&crate::row::Row::new(&row))
    }

    pub fn query_row<T, P, F>(&self, sql: &str, params: P, map: F) -> Result<T, DbError>
    where
        P: crate::params::IntoParams,
        F: FnOnce(&crate::row::Row<'_>) -> Result<T, DbError>,
    {
        self.query_one(sql, params, map)
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

    /// Capture `sql` for later execution via [`Statement::query_map`] or
    /// [`Statement::query_row`].
    ///
    /// Despite the name, this does NOT prepare or cache a libSQL statement.
    /// Each subsequent `query_map`/`query_row` re-issues
    /// [`libsql::Connection::query`] with the same SQL, which re-parses on
    /// every call. The owned-`String` storage is only there so callers can
    /// hold the [`Statement`] across `query_map` iterations.
    ///
    /// For one-shot queries prefer [`Connection::query_all`] or
    /// [`Connection::query_row`] directly.
    pub fn prepare<'conn>(&'conn self, sql: &str) -> Result<Statement<'conn>, DbError> {
        Ok(Statement {
            conn: self,
            sql: sql.to_owned(),
        })
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.inner.last_insert_rowid()
    }

    /// Start an immediate transaction.
    ///
    /// Multi-write operations should prefer [`Connection::with_immediate_transaction`]
    /// so rollback-on-error is centralized. Use this lower-level API when the
    /// transaction must be passed through helper boundaries or committed
    /// manually.
    pub fn transaction(&self) -> Result<crate::transaction::Transaction<'_>, DbError> {
        self.transaction_with_behavior(libsql::TransactionBehavior::Immediate)
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
                if let Err(rollback_err) = tx.rollback() {
                    tracing::warn!(
                        path = %self.db_path.display(),
                        operation_error = format!("{err:#}"),
                        rollback_error = format!("{rollback_err:#}"),
                        "transaction rollback failed; returning original operation error",
                    );
                }
                Err(err)
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

/// Owns an SQL string for repeated execution via `query_map`/`query_row`.
/// No libSQL-level prepared-statement caching is performed; each call
/// re-issues the query.
pub struct Statement<'conn> {
    conn: &'conn Connection,
    sql: String,
}

impl<'conn> Statement<'conn> {
    pub fn query_map<T, P, F>(
        &mut self,
        params: P,
        mut map: F,
    ) -> Result<std::vec::IntoIter<Result<T, DbError>>, DbError>
    where
        P: crate::params::IntoParams,
        F: FnMut(&crate::row::Row<'_>) -> Result<T, DbError>,
    {
        let params = params.into_params()?.into_libsql();
        let mut query_rows = self
            .conn
            .block_on_libsql(self.conn.inner.query(&self.sql, params))?;
        let mut rows = Vec::new();
        while let Some(row) = self.conn.block_on_libsql(query_rows.next())? {
            rows.push(map(&crate::row::Row::new(&row)));
        }
        Ok(rows.into_iter())
    }

    pub fn query_row<T, P, F>(&mut self, params: P, map: F) -> Result<T, DbError>
    where
        P: crate::params::IntoParams,
        F: FnOnce(&crate::row::Row<'_>) -> Result<T, DbError>,
    {
        self.conn.query_one(&self.sql, params, map)
    }
}

fn block_on_runtime_safe<F, T>(runtime: &tokio::runtime::Runtime, future: F) -> T
where
    F: Future<Output = T> + Send,
    T: Send,
{
    match tokio::runtime::Handle::try_current() {
        Err(_) => runtime.block_on(future),
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| runtime.block_on(future))
        }
        Ok(_) => std::thread::scope(|scope| {
            let handle = scope.spawn(move || runtime.block_on(future));
            match handle.join() {
                Ok(output) => output,
                Err(payload) => panic::resume_unwind(payload),
            }
        }),
    }
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

    fn insert_via_connection(conn: &Connection, value: &str) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO transaction_deref_probe (value) VALUES (?1)",
            crate::params![value],
        )?;
        Ok(())
    }

    #[test]
    fn transaction_deref_helper_write_is_rolled_back() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE transaction_deref_probe (value TEXT NOT NULL)")
            .unwrap();
        let tx = conn.transaction().unwrap();

        insert_via_connection(&tx, "inside-tx").unwrap();
        tx.rollback().unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM transaction_deref_probe", (), |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn with_immediate_transaction_returns_operation_error_and_rolls_back() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
            .unwrap();

        let err = conn
            .with_immediate_transaction(|tx| {
                tx.execute(
                    "INSERT INTO probe (value) VALUES (?1)",
                    crate::params![1i64],
                )?;
                Err::<(), _>(DbError::InvalidParameter("operation failed".into()))
            })
            .unwrap_err();

        assert!(
            matches!(err, DbError::InvalidParameter(ref msg) if msg == "operation failed"),
            "expected original operation error, got {err:#}",
        );

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM probe", (), |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "operation error must roll back the transaction");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sync_queries_work_inside_current_thread_tokio_runtime() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
            .unwrap();
        conn.execute("INSERT INTO probe (value) VALUES (?1)", [7_i64])
            .unwrap();

        let value: i64 = conn
            .query_row("SELECT value FROM probe", (), |row| row.get(0))
            .unwrap();
        assert_eq!(value, 7);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_queries_work_inside_multi_thread_tokio_runtime() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
            .unwrap();
        conn.execute("INSERT INTO probe (value) VALUES (?1)", [9_i64])
            .unwrap();

        let value: i64 = conn
            .query_row("SELECT value FROM probe", (), |row| row.get(0))
            .unwrap();
        assert_eq!(value, 9);
    }
}
