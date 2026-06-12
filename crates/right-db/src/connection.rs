use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::DbError;

pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Connection {
    db_path: PathBuf,
    // Kept alive because Turso connections are owned by their parent database.
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
    pub async fn open_in_memory() -> Result<Self, DbError> {
        Self::build(PathBuf::from(":memory:"), true, false).await
    }

    pub(crate) async fn open_local(db_path: PathBuf, create: bool) -> Result<Self, DbError> {
        if !create && !db_path.exists() {
            return Err(DbError::Open {
                path: db_path.clone(),
                source: turso::Error::Readonly(format!(
                    "database file does not exist: {}",
                    db_path.display()
                )),
            });
        }
        Self::build(db_path, create, !create).await
    }

    async fn build(db_path: PathBuf, create: bool, readonly: bool) -> Result<Self, DbError> {
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
        let mut builder = turso::Builder::new_local(path).experimental_index_method(true);
        if path != ":memory:" {
            let io = crate::multiprocess_io::new().map_err(|source| DbError::Open {
                path: db_path.clone(),
                source: turso::Error::Error(format!("multiprocess WAL IO setup failed: {source}")),
            })?;
            builder = builder.experimental_multiprocess_wal(true).with_io_impl(io);
        }
        let database = builder.build().await.map_err(open_err)?;
        let inner = database.connect().map_err(open_err)?;
        let conn = Self {
            db_path,
            _database: database,
            inner,
            readonly,
        };
        if !create {
            conn.inner.pragma_update("query_only", 1).await.map(drop)?;
        }
        Ok(conn)
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    pub async fn execute_batch(&self, sql: &str) -> Result<(), DbError> {
        self.ensure_writable()?;
        self.inner
            .execute_batch(sql)
            .await
            .map(drop)
            .map_err(Into::into)
    }

    pub async fn execute(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
    ) -> Result<usize, DbError> {
        self.ensure_writable()?;
        let params = params.into_params()?.into_turso();
        let changed = self.inner.execute(sql, params).await?;
        usize::try_from(changed)
            .map_err(|_| DbError::InvalidParameter("changed row count exceeds usize".into()))
    }

    /// Issue a read-only query and decode the first row.
    ///
    /// On a readonly [`Connection`] the Rust-side gate accepts the following
    /// SQL forms (after stripping leading whitespace, `--` line comments, and
    /// `/* ... */` block comments - comment stripping only affects what the
    /// gate sees, not the SQL sent to Turso):
    ///
    /// - `SELECT ...`
    /// - `WITH cte AS (...) SELECT ...` - CTE-prefixed read; rejected if the
    ///   remaining SQL contains `INSERT`, `UPDATE`, `DELETE`, or `REPLACE`.
    /// - `EXPLAIN ...` / `EXPLAIN QUERY PLAN ...` - never executes the wrapped
    ///   statement, always safe.
    /// - `PRAGMA name` - bare read form only. `PRAGMA name(arg)` and
    ///   `PRAGMA name = value` are rejected as a class because some forms
    ///   mutate connection state; in particular `PRAGMA query_only(OFF)` would
    ///   disable the readonly flag.
    ///
    /// Writable connections accept any SQL. The gate is a defense-in-depth
    /// layer on top of Turso's `PRAGMA query_only=1`.
    pub async fn query_one<T>(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
        map: impl FnOnce(&crate::row::Row<'_>) -> Result<T, DbError>,
    ) -> Result<T, DbError> {
        self.ensure_query_allowed(sql)?;
        let params = params.into_params()?.into_turso();
        let mut rows = self.inner.query(sql, params).await?;
        let Some(row) = rows.next().await? else {
            return Err(DbError::NotFound);
        };
        map(&crate::row::Row::new(&row))
    }

    pub async fn query_row<T, P, F>(&self, sql: &str, params: P, map: F) -> Result<T, DbError>
    where
        P: crate::params::IntoParams,
        F: FnOnce(&crate::row::Row<'_>) -> Result<T, DbError>,
    {
        self.query_one(sql, params, map).await
    }

    /// Issue a read-only query and decode every row.
    ///
    /// See [`Connection::query_one`] for the readonly SQL grammar accepted by
    /// the Rust-side gate.
    pub async fn query_all<T>(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
        mut map: impl FnMut(&crate::row::Row<'_>) -> Result<T, DbError>,
    ) -> Result<Vec<T>, DbError> {
        self.ensure_query_allowed(sql)?;
        let params = params.into_params()?.into_turso();
        let mut rows = self.inner.query(sql, params).await?;
        let mut values = Vec::new();
        while let Some(row) = rows.next().await? {
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
    /// Callers are responsible for explicitly committing or rolling back the
    /// returned transaction.
    pub async fn transaction(&self) -> Result<crate::transaction::Transaction<'_>, DbError> {
        self.ensure_writable()?;
        self.transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
            .await
    }

    async fn transaction_with_behavior(
        &self,
        behavior: turso::transaction::TransactionBehavior,
    ) -> Result<crate::transaction::Transaction<'_>, DbError> {
        self.ensure_writable()?;
        let inner = turso::transaction::Transaction::new_unchecked(&self.inner, behavior).await?;
        Ok(crate::transaction::Transaction::new(self, inner))
    }

    pub(crate) async fn apply_connection_pragmas(&self) -> Result<(), DbError> {
        // WAL switching takes the file lock, so install SQLite's busy wait first.
        self.inner
            .pragma_update("busy_timeout", BUSY_TIMEOUT.as_millis())
            .await
            .map(drop)?;
        self.inner
            .pragma_update("journal_mode", "WAL")
            .await
            .map(drop)?;
        Ok(())
    }

    pub(crate) async fn apply_readonly_pragmas(&self) -> Result<(), DbError> {
        self.inner
            .pragma_update("busy_timeout", BUSY_TIMEOUT.as_millis())
            .await
            .map(drop)?;
        Ok(())
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
        // Only bare `PRAGMA name` is accepted as a read. Parenthesized and
        // assignment forms are rejected as a class because some mutate
        // connection state; `PRAGMA query_only(OFF)` would disable the
        // readonly flag.
        let tail = rest[pragma_name.len()..].trim_start();
        return tail.is_empty();
    }
    false
}

/// Return true if a CTE prefix is followed only by read statements. We do not
/// parse the CTE grammar; instead we reject write keywords in the remainder.
/// False negatives (e.g. those words appearing inside a string literal) only
/// over-restrict reads, never permit writes.
fn is_readonly_cte(rest: &str) -> bool {
    !contains_keyword_ascii_ci(rest, "INSERT")
        && !contains_keyword_ascii_ci(rest, "UPDATE")
        && !contains_keyword_ascii_ci(rest, "DELETE")
        && !contains_keyword_ascii_ci(rest, "REPLACE")
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
    pub async fn query_map<T, P, F>(
        &mut self,
        params: P,
        mut map: F,
    ) -> Result<std::vec::IntoIter<Result<T, DbError>>, DbError>
    where
        P: crate::params::IntoParams,
        F: FnMut(&crate::row::Row<'_>) -> Result<T, DbError>,
    {
        let params = params.into_params()?.into_turso();
        let mut query_rows = self.conn.inner.query(&self.sql, params).await?;
        let mut rows = Vec::new();
        while let Some(row) = query_rows.next().await? {
            rows.push(map(&crate::row::Row::new(&row)));
        }
        Ok(rows.into_iter())
    }

    pub async fn query_row<T, P, F>(&mut self, params: P, map: F) -> Result<T, DbError>
    where
        P: crate::params::IntoParams,
        F: FnOnce(&crate::row::Row<'_>) -> Result<T, DbError>,
    {
        self.conn.query_one(&self.sql, params, map).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn insert_via_connection(conn: &Connection, value: &str) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO transaction_deref_probe (value) VALUES (?1)",
            crate::params![value],
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn async_connection_operations_do_not_require_a_sync_bridge() {
        let conn = Connection::open_in_memory().await.unwrap();
        conn.execute_batch("CREATE TABLE async_probe (value INTEGER NOT NULL)")
            .await
            .unwrap();
        conn.execute("INSERT INTO async_probe (value) VALUES (?1)", [7_i64])
            .await
            .unwrap();

        let value: i64 = conn
            .query_row("SELECT value FROM async_probe", (), |row| row.get(0))
            .await
            .unwrap();

        assert_eq!(value, 7);
    }

    #[tokio::test]
    async fn writable_writes_wait_for_existing_write_lock() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data.db");

        let conn = Connection::open_local(db_path.clone(), true).await.unwrap();
        conn.apply_connection_pragmas().await.unwrap();
        conn.execute_batch("CREATE TABLE lock_probe (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();

        // A second connection holds the multiprocess WAL write lock; our write
        // must wait on busy_timeout and recover once the holder releases.
        let lock = crate::test_support::hold_write_lock(db_path, Duration::from_millis(500)).await;

        let result = conn
            .execute("INSERT INTO lock_probe (id) VALUES (1)", ())
            .await;
        lock.await.expect("release write lock");

        result.expect("writable write should honor busy_timeout and recover");
    }

    #[tokio::test]
    async fn transaction_deref_helper_write_is_rolled_back() {
        let conn = Connection::open_in_memory().await.unwrap();
        conn.execute_batch("CREATE TABLE transaction_deref_probe (value TEXT NOT NULL)")
            .await
            .unwrap();
        let tx = conn.transaction().await.unwrap();

        insert_via_connection(&tx, "inside-tx").await.unwrap();
        tx.rollback().await.unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM transaction_deref_probe", (), |row| {
                row.get(0)
            })
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn dropped_transaction_is_rolled_back() {
        let conn = Connection::open_in_memory().await.unwrap();
        conn.execute_batch("CREATE TABLE dropped_tx_probe (value TEXT NOT NULL)")
            .await
            .unwrap();

        {
            let tx = conn.transaction().await.unwrap();
            tx.execute(
                "INSERT INTO dropped_tx_probe (value) VALUES (?1)",
                crate::params!["inside-tx"],
            )
            .await
            .unwrap();
        }

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM dropped_tx_probe", (), |row| {
                row.get(0)
            })
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn explicit_transaction_rollback_after_operation_error_rolls_back() {
        let conn = Connection::open_in_memory().await.unwrap();
        conn.execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
            .await
            .unwrap();

        let tx = conn.transaction().await.unwrap();
        tx.execute(
            "INSERT INTO probe (value) VALUES (?1)",
            crate::params![1i64],
        )
        .await
        .unwrap();
        let err = Err::<(), _>(DbError::InvalidParameter("operation failed".into())).unwrap_err();
        tx.rollback().await.unwrap();

        assert!(
            matches!(err, DbError::InvalidParameter(ref msg) if msg == "operation failed"),
            "expected original operation error, got {err:#}",
        );

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM probe", (), |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(count, 0, "operation error must roll back the transaction");
    }

    #[tokio::test]
    async fn readonly_connection_cannot_disable_query_only_or_write() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data.db");

        let writable = Connection::open_local(db_path.clone(), true).await.unwrap();
        writable
            .execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
            .await
            .unwrap();
        writable
            .execute("INSERT INTO probe (value) VALUES (?1)", [7_i64])
            .await
            .unwrap();
        drop(writable);

        let readonly = Connection::open_local(db_path, false).await.unwrap();

        let mut results = Vec::new();
        results.push(readonly.execute_batch("PRAGMA query_only = OFF").await);
        results.push(
            readonly
                .execute_batch("CREATE TABLE forbidden (value INTEGER NOT NULL)")
                .await,
        );
        results.push(
            readonly
                .execute("INSERT INTO probe (value) VALUES (?1)", [8_i64])
                .await
                .map(drop),
        );
        results.push(
            readonly
                .query_row(
                    "INSERT INTO probe (value) VALUES (9) RETURNING value",
                    (),
                    |row| row.get::<_, i64>(0),
                )
                .await
                .map(drop),
        );
        results.push(
            readonly
                .query_row("PRAGMA query_only(OFF)", (), |row| row.get::<_, i64>(0))
                .await
                .map(drop),
        );
        results.push(match readonly.prepare("PRAGMA query_only(OFF)") {
            Ok(mut stmt) => stmt
                .query_row((), |row| row.get::<_, i64>(0))
                .await
                .map(drop),
            Err(e) => Err(e),
        });
        results.push(readonly.transaction().await.map(drop));

        for result in results {
            let err = result.expect_err("readonly connection must reject writes");
            assert!(
                err.to_string().contains("readonly database"),
                "expected readonly database error, got {err:#}",
            );
        }

        let count: i64 = readonly
            .query_row("SELECT COUNT(*) FROM probe", (), |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn readonly_query_gate_accepts_documented_read_forms() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data.db");

        let writable = Connection::open_local(db_path.clone(), true).await.unwrap();
        writable
            .execute_batch(
                "CREATE TABLE probe (value INTEGER NOT NULL); \
                 INSERT INTO probe (value) VALUES (1); \
                 INSERT INTO probe (value) VALUES (2);",
            )
            .await
            .unwrap();
        drop(writable);

        let readonly = Connection::open_local(db_path, false).await.unwrap();

        // Plain SELECT.
        let v: i64 = readonly
            .query_row("SELECT COUNT(*) FROM probe", (), |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(v, 2);

        // SELECT with leading whitespace and a line comment.
        let v: i64 = readonly
            .query_row(
                "-- this is a comment\nSELECT COUNT(*) FROM probe",
                (),
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(v, 2);

        // SELECT with leading block comment.
        let v: i64 = readonly
            .query_row("/* preamble */ SELECT COUNT(*) FROM probe", (), |row| {
                row.get(0)
            })
            .await
            .unwrap();
        assert_eq!(v, 2);

        // SELECT with leading whitespace + block + line comments interleaved.
        let v: i64 = readonly
            .query_row(
                "  /* one */\n-- two\n/* three */ SELECT COUNT(*) FROM probe",
                (),
                |row| row.get(0),
            )
            .await
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
            .await
            .unwrap();
        assert_eq!(v, 2);

        // EXPLAIN gate-passes (we don't care about the row shape, only that
        // it isn't rejected as a write).
        let _ = readonly
            .query_all("EXPLAIN SELECT 1", (), |row| row.get::<_, i64>(0))
            .await;
        let _ = readonly
            .query_all("EXPLAIN QUERY PLAN SELECT 1", (), |row| {
                row.get::<_, i64>(0)
            })
            .await;

        // Bare PRAGMA read.
        let _: i64 = readonly
            .query_row("PRAGMA query_only", (), |row| row.get(0))
            .await
            .unwrap();

        // prepare() should also accept these forms.
        let mut stmt = readonly
            .prepare("WITH doubled AS (SELECT value * 2 AS v FROM probe) SELECT v FROM doubled")
            .unwrap();
        let _ = stmt
            .query_map((), |row| row.get::<_, i64>(0))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn readonly_query_gate_rejects_write_forms() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data.db");

        let writable = Connection::open_local(db_path.clone(), true).await.unwrap();
        writable
            .execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
            .await
            .unwrap();
        drop(writable);

        let readonly = Connection::open_local(db_path, false).await.unwrap();

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
                .await
                .expect_err(&format!("expected gate to reject {sql:?}"));
            assert!(
                err.to_string().contains("readonly database"),
                "expected readonly database error for {sql:?}, got {err:#}",
            );
        }
    }

    #[test]
    fn readonly_cte_gate_rejects_replace_statement() {
        assert!(!is_readonly_query_sql(
            "WITH x AS (SELECT 1) REPLACE INTO probe (value) VALUES (1)"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_queries_work_inside_current_thread_tokio_runtime() {
        let conn = Connection::open_in_memory().await.unwrap();
        conn.execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
            .await
            .unwrap();
        conn.execute("INSERT INTO probe (value) VALUES (?1)", [7_i64])
            .await
            .unwrap();

        let value: i64 = conn
            .query_row("SELECT value FROM probe", (), |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(value, 7);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_queries_work_inside_multi_thread_tokio_runtime() {
        let conn = Connection::open_in_memory().await.unwrap();
        conn.execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
            .await
            .unwrap();
        conn.execute("INSERT INTO probe (value) VALUES (?1)", [9_i64])
            .await
            .unwrap();

        let value: i64 = conn
            .query_row("SELECT value FROM probe", (), |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(value, 9);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_queries_work_inside_local_set_on_multi_thread_runtime() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let conn = Connection::open_in_memory().await.unwrap();
                conn.execute_batch("CREATE TABLE probe (value INTEGER NOT NULL)")
                    .await
                    .unwrap();
                conn.execute("INSERT INTO probe (value) VALUES (?1)", [11_i64])
                    .await
                    .unwrap();

                let value: i64 = conn
                    .query_row("SELECT value FROM probe", (), |row| row.get(0))
                    .await
                    .unwrap();
                assert_eq!(value, 11);
            })
            .await;
    }
}
