use std::fmt;
use std::ops::Deref;

use crate::DbError;
use crate::connection::Connection;

/// An open database transaction.
///
/// # `Deref<Target = Connection>` is intentional
///
/// `Transaction` deliberately implements `Deref<Target = Connection>` so that
/// helpers written against `&Connection` (e.g. `ensure_nudge_state`) can also
/// be called with `&Transaction` without a separate `_tx` overload. Callers
/// inside a transaction can pass `&tx` to a `&Connection`-taking helper and
/// the writes still participate in the open `BEGIN IMMEDIATE`.
///
/// This only preserves transactional semantics because the current local
/// Turso backend shares the underlying SQLite handle between `Connection`
/// and `turso::transaction::Transaction`. The invariant is covered by
/// `transaction_deref_helper_write_is_rolled_back` in `connection.rs`, which
/// verifies that a `&Connection`-taking write reached through `Deref` is
/// rolled back with the outer transaction.
///
/// # WARNING: backend swap
///
/// If `right-db` ever swaps to a backend where
/// `turso::transaction::Transaction` is not the same wire-level handle as
/// the parent `Connection`, this `Deref` silently turns every
/// `_tx`-less helper call into an autocommit write outside the transaction
/// with no compile-time signal. Re-audit every `&Connection`-taking helper
/// and the `transaction_deref_helper_write_is_rolled_back` test before
/// changing backends.
pub struct Transaction<'conn> {
    conn: &'conn Connection,
    inner: Option<turso::transaction::Transaction<'conn>>,
}

impl<'conn> Transaction<'conn> {
    pub(crate) fn new(
        conn: &'conn Connection,
        inner: turso::transaction::Transaction<'conn>,
    ) -> Self {
        Self {
            conn,
            inner: Some(inner),
        }
    }

    pub async fn execute(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
    ) -> Result<usize, DbError> {
        let params = params.into_params()?.into_turso();
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        let changed = inner.execute(sql, params).await?;
        usize::try_from(changed)
            .map_err(|_| DbError::InvalidParameter("changed row count exceeds usize".into()))
    }

    pub async fn execute_batch(&self, sql: &str) -> Result<(), DbError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        inner.execute_batch(sql).await.map(drop).map_err(Into::into)
    }

    pub async fn query_one<T>(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
        map: impl FnOnce(&crate::row::Row<'_>) -> Result<T, DbError>,
    ) -> Result<T, DbError> {
        let params = params.into_params()?.into_turso();
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        let mut rows = inner.query(sql, params).await?;
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

    pub async fn query_all<T>(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
        mut map: impl FnMut(&crate::row::Row<'_>) -> Result<T, DbError>,
    ) -> Result<Vec<T>, DbError> {
        let params = params.into_params()?.into_turso();
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        let mut rows = inner.query(sql, params).await?;
        let mut values = Vec::new();
        while let Some(row) = rows.next().await? {
            values.push(map(&crate::row::Row::new(&row))?);
        }
        Ok(values)
    }

    /// Capture `sql` for later execution via
    /// [`TransactionStatement::query_map`] or
    /// [`TransactionStatement::query_row`].
    ///
    /// Despite the name, this does NOT prepare or cache a Turso statement.
    /// Each subsequent `query_map`/`query_row` re-issues the query on the
    /// underlying Turso transaction with the same SQL, which re-parses on
    /// every call. The owned-`String` storage is only there so callers can
    /// hold the [`TransactionStatement`] across `query_map` iterations.
    pub fn prepare<'tx>(&'tx self, sql: &str) -> Result<TransactionStatement<'tx, 'conn>, DbError> {
        Ok(TransactionStatement {
            tx: self,
            sql: sql.to_owned(),
        })
    }

    pub fn connection(&self) -> &Connection {
        self.conn
    }

    pub async fn commit(mut self) -> Result<(), DbError> {
        let inner = self
            .inner
            .take()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        inner.commit().await?;
        Ok(())
    }

    pub async fn rollback(mut self) -> Result<(), DbError> {
        let inner = self
            .inner
            .take()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        inner.rollback().await?;
        Ok(())
    }
}

/// Owns an SQL string for repeated execution via `query_map`/`query_row`.
/// No Turso-level prepared-statement caching is performed; each call
/// re-issues the query on the underlying transaction.
pub struct TransactionStatement<'tx, 'conn> {
    tx: &'tx Transaction<'conn>,
    sql: String,
}

impl<'tx, 'conn> TransactionStatement<'tx, 'conn> {
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
        let inner = self
            .tx
            .inner
            .as_ref()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        let mut query_rows = inner.query(&self.sql, params).await?;
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
        self.tx.query_one(&self.sql, params, map).await
    }
}

impl fmt::Debug for Transaction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transaction").finish_non_exhaustive()
    }
}

/// Intentional `Deref` so `&Transaction` can be passed to helpers that take
/// `&Connection`. See the [`Transaction`] struct-level docs for the
/// transactional-semantics invariant and the backend-swap warning.
impl<'conn> Deref for Transaction<'conn> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.conn
    }
}
