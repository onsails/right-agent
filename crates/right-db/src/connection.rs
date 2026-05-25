use std::fmt;
use std::future::Future;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use crate::DbError;

pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Number of worker threads in the process-wide shared runtime.
///
/// Local Turso wraps blocking SQLite, so each DB op is effectively
/// synchronous. We still want at least two workers so independent
/// connections (different agents, dashboard, aggregator) do not serialize
/// behind one another at the scheduler level.
const SHARED_RUNTIME_WORKER_THREADS: usize = 2;

/// Process-wide tokio runtime shared by every [`Connection`].
///
/// Local Turso wraps blocking SQLite, so there is no genuine async work to
/// overlap inside a single op. However, this runtime is shared across every
/// agent DB, the dashboard, and the aggregator in the same process; a
/// `current_thread` runtime would serialise all of them through one
/// scheduler. Use a small multi-thread runtime so independent connections
/// can make progress concurrently. The runtime is initialised lazily on the
/// first connection and lives for the lifetime of the process; we
/// deliberately never shut it down because that would race with concurrent
/// `Connection` operations across the workspace.
fn shared_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(SHARED_RUNTIME_WORKER_THREADS)
            .enable_all()
            .build()
            .expect("right-db Turso runtime should initialize")
    })
}

pub struct Connection {
    db_path: PathBuf,
    _database: turso::Database,
    inner: turso::Connection,
    readonly: bool,
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
        Self::build(PathBuf::from(":memory:"), true, false)
    }

    pub(crate) fn open_local(db_path: PathBuf, create: bool) -> Result<Self, DbError> {
        if !create && !db_path.exists() {
            return Err(DbError::Open {
                path: db_path.clone(),
                source: turso::Error::Readonly(format!(
                    "database file does not exist: {}",
                    db_path.display()
                )),
            });
        }
        Self::build(db_path, create, !create)
    }

    fn build(db_path: PathBuf, create: bool, readonly: bool) -> Result<Self, DbError> {
        let runtime = shared_runtime();
        let path = db_path.to_str().ok_or_else(|| {
            DbError::InvalidParameter(format!(
                "database path is not valid UTF-8: {}",
                db_path.display()
            ))
        })?;
        let open_err = |source| DbError::Open {
            path: db_path.clone(),
            source,
        };
        let database = block_on_runtime_safe(
            runtime,
            turso::Builder::new_local(&path)
                .experimental_index_method(true)
                .build(),
        )
        .map_err(open_err)?;
        let inner = database.connect().map_err(open_err)?;
        let conn = Self {
            db_path,
            _database: database,
            inner,
            readonly,
        };
        if !create {
            conn.block_on_turso(conn.inner.pragma_update("query_only", 1))
                .map(drop)?;
        }
        Ok(conn)
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    pub fn execute_batch(&self, sql: &str) -> Result<(), DbError> {
        self.ensure_writable()?;
        self.block_on_turso(self.inner.execute_batch(sql)).map(drop)
    }

    pub fn execute(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
    ) -> Result<usize, DbError> {
        self.ensure_writable()?;
        let params = params.into_params()?.into_turso();
        let changed = self.block_on_turso(self.inner.execute(sql, params))?;
        usize::try_from(changed)
            .map_err(|_| DbError::InvalidParameter("changed row count exceeds usize".into()))
    }

    /// Issue a read-only query and decode the first row.
    ///
    /// On a readonly [`Connection`] the Rust-side gate accepts the following
    /// SQL forms (after stripping leading whitespace, `--` line comments, and
    /// `/* ... */` block comments — comment stripping only affects what the
    /// gate sees, not the SQL sent to Turso):
    ///
    /// - `SELECT ...`
    /// - `WITH cte AS (...) SELECT ...` — CTE-prefixed read; rejected if the
    ///   remaining SQL contains `INSERT`, `UPDATE`, or `DELETE`.
    /// - `EXPLAIN ...` / `EXPLAIN QUERY PLAN ...` — never executes the wrapped
    ///   statement, always safe.
    /// - `PRAGMA name` — bare read form only. Both `PRAGMA name(arg)` and
    ///   `PRAGMA name = value` are rejected because Turso/SQLite treats them
    ///   as writes; in particular `PRAGMA query_only(OFF)` would disable the
    ///   readonly flag.
    ///
    /// Writable connections accept any SQL. The gate is a defense-in-depth
    /// layer on top of Turso's `PRAGMA query_only=1`.
    pub fn query_one<T>(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
        map: impl FnOnce(&crate::row::Row<'_>) -> Result<T, DbError>,
    ) -> Result<T, DbError> {
        self.ensure_query_allowed(sql)?;
        let params = params.into_params()?.into_turso();
        let mut rows = self.block_on_turso(self.inner.query(sql, params))?;
        let Some(row) = self.block_on_turso(rows.next())? else {
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

    /// Issue a read-only query and decode every row.
    ///
    /// See [`Connection::query_one`] for the readonly SQL grammar accepted by
    /// the Rust-side gate.
    pub fn query_all<T>(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
        mut map: impl FnMut(&crate::row::Row<'_>) -> Result<T, DbError>,
    ) -> Result<Vec<T>, DbError> {
        self.ensure_query_allowed(sql)?;
        let params = params.into_params()?.into_turso();
        let mut rows = self.block_on_turso(self.inner.query(sql, params))?;
        let mut values = Vec::new();
        while let Some(row) = self.block_on_turso(rows.next())? {
            values.push(map(&crate::row::Row::new(&row))?);
        }
        Ok(values)
    }

    /// Capture `sql` for later execution via [`Statement::query_map`] or
    /// [`Statement::query_row`].
    ///
    /// Despite the name, this does NOT prepare or cache a Turso statement.
    /// Each subsequent `query_map`/`query_row` re-issues
    /// [`turso::Connection::query`] with the same SQL, which re-parses on
    /// every call. The owned-`String` storage is only there so callers can
    /// hold the [`Statement`] across `query_map` iterations.
    ///
    /// For one-shot queries prefer [`Connection::query_all`] or
    /// [`Connection::query_row`] directly.
    ///
    /// On a readonly [`Connection`] the same SQL grammar described on
    /// [`Connection::query_one`] applies.
    pub fn prepare<'conn>(&'conn self, sql: &str) -> Result<Statement<'conn>, DbError> {
        self.ensure_query_allowed(sql)?;
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
        self.ensure_writable()?;
        self.transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
    }

    pub fn with_immediate_transaction<T>(
        &self,
        f: impl FnOnce(&crate::transaction::Transaction<'_>) -> Result<T, DbError>,
    ) -> Result<T, DbError> {
        self.ensure_writable()?;
        let tx =
            self.transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)?;
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
        behavior: turso::transaction::TransactionBehavior,
    ) -> Result<crate::transaction::Transaction<'_>, DbError> {
        self.ensure_writable()?;
        let inner = self.block_on_turso(turso::transaction::Transaction::new_unchecked(
            &self.inner,
            behavior,
        ))?;
        Ok(crate::transaction::Transaction::new(self, inner))
    }

    pub(crate) fn apply_connection_pragmas(&self) -> Result<(), DbError> {
        self.block_on_turso(self.inner.pragma_update("journal_mode", "WAL"))
            .map(drop)?;
        self.block_on_turso(
            self.inner
                .pragma_update("busy_timeout", BUSY_TIMEOUT.as_millis()),
        )
        .map(drop)?;
        Ok(())
    }

    pub(crate) fn apply_readonly_pragmas(&self) -> Result<(), DbError> {
        self.block_on_turso(
            self.inner
                .pragma_update("busy_timeout", BUSY_TIMEOUT.as_millis()),
        )
        .map(drop)?;
        Ok(())
    }

    pub(crate) fn block_on_turso<T: Send>(
        &self,
        future: impl Future<Output = turso::Result<T>> + Send,
    ) -> Result<T, DbError> {
        block_on_runtime_safe(shared_runtime(), future).map_err(Into::into)
    }

    fn ensure_writable(&self) -> Result<(), DbError> {
        if self.readonly {
            return Err(readonly_error());
        }
        Ok(())
    }

    fn ensure_query_allowed(&self, sql: &str) -> Result<(), DbError> {
        if self.readonly && !is_readonly_query_sql(sql) {
            return Err(readonly_error());
        }
        Ok(())
    }
}

fn readonly_error() -> DbError {
    DbError::Database(turso::Error::Readonly("readonly database".into()))
}

fn is_readonly_query_sql(sql: &str) -> bool {
    let sql = strip_leading_whitespace_and_comments(sql);
    let Some(keyword) = leading_keyword(sql) else {
        return false;
    };

    if keyword.eq_ignore_ascii_case("SELECT") {
        return true;
    }
    if keyword.eq_ignore_ascii_case("EXPLAIN") {
        // EXPLAIN [QUERY PLAN] never executes the wrapped statement; it only
        // dumps the plan. Safe regardless of what follows.
        return true;
    }
    if keyword.eq_ignore_ascii_case("WITH") {
        let rest = &sql[keyword.len()..];
        return is_readonly_cte(rest);
    }
    if keyword.eq_ignore_ascii_case("PRAGMA") {
        let rest = sql[keyword.len()..].trim_start();
        let Some(pragma_name) = leading_pragma_name(rest) else {
            return false;
        };
        if pragma_name.is_empty() {
            return false;
        }
        // Only bare `PRAGMA name` is accepted as a read. The parenthesized
        // form `PRAGMA name(arg)` and the assignment form
        // `PRAGMA name = value` both mutate connection state in Turso/SQLite
        // (verified: `PRAGMA query_only(OFF)` toggles `query_only` off), so
        // both are rejected by the Rust-side gate.
        let tail = rest[pragma_name.len()..].trim_start();
        return tail.is_empty();
    }
    false
}

/// Return true if a CTE prefix is followed only by read statements. We do not
/// parse the CTE grammar; instead we reject any `INSERT`, `UPDATE`, or
/// `DELETE` keyword in the remainder. False negatives (e.g. those words
/// appearing inside a string literal) only over-restrict reads, never permit
/// writes.
fn is_readonly_cte(rest: &str) -> bool {
    !contains_keyword_ascii_ci(rest, "INSERT")
        && !contains_keyword_ascii_ci(rest, "UPDATE")
        && !contains_keyword_ascii_ci(rest, "DELETE")
}

fn contains_keyword_ascii_ci(haystack: &str, keyword: &str) -> bool {
    let bytes = haystack.as_bytes();
    let kw = keyword.as_bytes();
    if kw.is_empty() || bytes.len() < kw.len() {
        return false;
    }
    let is_word_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    for start in 0..=bytes.len() - kw.len() {
        let end = start + kw.len();
        if !bytes[start..end].eq_ignore_ascii_case(kw) {
            continue;
        }
        let prev_is_word = start > 0 && is_word_byte(bytes[start - 1]);
        let next_is_word = end < bytes.len() && is_word_byte(bytes[end]);
        if !prev_is_word && !next_is_word {
            return true;
        }
    }
    false
}

/// Skip leading ASCII whitespace and SQL comments (`--` line comments and
/// `/* ... */` block comments). Used only by the Rust-side readonly gate to
/// decide which form of statement is being issued; the original SQL is still
/// sent to Turso untouched.
fn strip_leading_whitespace_and_comments(sql: &str) -> &str {
    let mut s = sql;
    loop {
        let trimmed = s.trim_start();
        if let Some(after) = trimmed.strip_prefix("--") {
            // Line comment: skip up to and including the next newline.
            match after.find('\n') {
                Some(nl) => s = &after[nl + 1..],
                None => return "",
            }
            continue;
        }
        if let Some(after) = trimmed.strip_prefix("/*") {
            // Block comment: skip up to and including the next `*/`.
            match after.find("*/") {
                Some(end) => s = &after[end + 2..],
                None => return "",
            }
            continue;
        }
        return trimmed;
    }
}

fn leading_keyword(sql: &str) -> Option<&str> {
    let len = sql
        .char_indices()
        .find_map(|(idx, ch)| (!ch.is_ascii_alphabetic()).then_some(idx))
        .unwrap_or(sql.len());
    (len > 0).then_some(&sql[..len])
}

fn leading_pragma_name(sql: &str) -> Option<&str> {
    let len = sql
        .char_indices()
        .find_map(|(idx, ch)| {
            (!(ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')).then_some(idx)
        })
        .unwrap_or(sql.len());
    (len > 0).then_some(&sql[..len])
}

/// Owns an SQL string for repeated execution via `query_map`/`query_row`.
/// No Turso-level prepared-statement caching is performed; each call
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
        let params = params.into_params()?.into_turso();
        let mut query_rows = self
            .conn
            .block_on_turso(self.conn.inner.query(&self.sql, params))?;
        let mut rows = Vec::new();
        while let Some(row) = self.conn.block_on_turso(query_rows.next())? {
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
    fn dropped_transaction_is_rolled_back() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE dropped_tx_probe (value TEXT NOT NULL)")
            .unwrap();

        {
            let tx = conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO dropped_tx_probe (value) VALUES (?1)",
                crate::params!["inside-tx"],
            )
            .unwrap();
        }

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM dropped_tx_probe", (), |row| {
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

    #[test]
    fn readonly_connection_cannot_disable_query_only_or_write() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data.db");

        let writable = Connection::open_local(db_path.clone(), true).unwrap();
        writable
            .execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
            .unwrap();
        writable
            .execute("INSERT INTO probe (value) VALUES (?1)", [7_i64])
            .unwrap();
        drop(writable);

        let readonly = Connection::open_local(db_path, false).unwrap();

        for result in [
            readonly.execute_batch("PRAGMA query_only = OFF"),
            readonly.execute_batch("CREATE TABLE forbidden (value INTEGER NOT NULL)"),
            readonly
                .execute("INSERT INTO probe (value) VALUES (?1)", [8_i64])
                .map(drop),
            readonly
                .query_row(
                    "INSERT INTO probe (value) VALUES (9) RETURNING value",
                    (),
                    |row| row.get::<_, i64>(0),
                )
                .map(drop),
            readonly
                .query_row("PRAGMA query_only(OFF)", (), |row| row.get::<_, i64>(0))
                .map(drop),
            readonly
                .prepare("PRAGMA query_only(OFF)")
                .and_then(|mut stmt| stmt.query_row((), |row| row.get::<_, i64>(0)))
                .map(drop),
            readonly.transaction().map(drop),
        ] {
            let err = result.expect_err("readonly connection must reject writes");
            assert!(
                err.to_string().contains("readonly database"),
                "expected readonly database error, got {err:#}",
            );
        }

        let count: i64 = readonly
            .query_row("SELECT COUNT(*) FROM probe", (), |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn readonly_query_gate_accepts_documented_read_forms() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data.db");

        let writable = Connection::open_local(db_path.clone(), true).unwrap();
        writable
            .execute_batch(
                "CREATE TABLE probe (value INTEGER NOT NULL); \
                 INSERT INTO probe (value) VALUES (1); \
                 INSERT INTO probe (value) VALUES (2);",
            )
            .unwrap();
        drop(writable);

        let readonly = Connection::open_local(db_path, false).unwrap();

        // Plain SELECT.
        let v: i64 = readonly
            .query_row("SELECT COUNT(*) FROM probe", (), |row| row.get(0))
            .unwrap();
        assert_eq!(v, 2);

        // SELECT with leading whitespace and a line comment.
        let v: i64 = readonly
            .query_row(
                "-- this is a comment\nSELECT COUNT(*) FROM probe",
                (),
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v, 2);

        // SELECT with leading block comment.
        let v: i64 = readonly
            .query_row("/* preamble */ SELECT COUNT(*) FROM probe", (), |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(v, 2);

        // SELECT with leading whitespace + block + line comments interleaved.
        let v: i64 = readonly
            .query_row(
                "  /* one */\n-- two\n/* three */ SELECT COUNT(*) FROM probe",
                (),
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v, 2);

        // WITH-prefixed read CTE.
        let v: i64 = readonly
            .query_row(
                "WITH doubled AS (SELECT value * 2 AS v FROM probe) \
                 SELECT COUNT(*) FROM doubled",
                (),
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v, 2);

        // EXPLAIN gate-passes (we don't care about the row shape, only that
        // it isn't rejected as a write).
        let _ = readonly.query_all("EXPLAIN SELECT 1", (), |row| row.get::<_, i64>(0));
        let _ = readonly.query_all("EXPLAIN QUERY PLAN SELECT 1", (), |row| {
            row.get::<_, i64>(0)
        });

        // Bare PRAGMA read.
        let _: i64 = readonly
            .query_row("PRAGMA query_only", (), |row| row.get(0))
            .unwrap();

        // prepare() should also accept these forms.
        let mut stmt = readonly
            .prepare("WITH doubled AS (SELECT value * 2 AS v FROM probe) SELECT v FROM doubled")
            .unwrap();
        let _ = stmt.query_map((), |row| row.get::<_, i64>(0)).unwrap();
    }

    #[test]
    fn readonly_query_gate_rejects_write_forms() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data.db");

        let writable = Connection::open_local(db_path.clone(), true).unwrap();
        writable
            .execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
            .unwrap();
        drop(writable);

        let readonly = Connection::open_local(db_path, false).unwrap();

        for sql in [
            // CTE that contains a write keyword is rejected.
            "WITH x AS (SELECT 1) INSERT INTO probe (value) VALUES (1)",
            "WITH x AS (SELECT 1) UPDATE probe SET value = 2",
            "WITH x AS (SELECT 1) DELETE FROM probe",
            // PRAGMA write forms — both parenthesized and assignment.
            "PRAGMA query_only(OFF)",
            "PRAGMA query_only = OFF",
            // Bare writes.
            "INSERT INTO probe (value) VALUES (1)",
            "UPDATE probe SET value = 1",
            "DELETE FROM probe",
            // Leading comments do not rescue a write.
            "-- safe?\nINSERT INTO probe (value) VALUES (1)",
            "/* safe? */ UPDATE probe SET value = 1",
        ] {
            let err = readonly
                .query_row(sql, (), |row| row.get::<_, i64>(0))
                .expect_err(&format!("expected gate to reject {sql:?}"));
            assert!(
                err.to_string().contains("readonly database"),
                "expected readonly database error for {sql:?}, got {err:#}",
            );
        }
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
