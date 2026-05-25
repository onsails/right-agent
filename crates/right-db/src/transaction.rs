use std::fmt;

use crate::DbError;
use crate::connection::Connection;

pub struct Transaction<'conn> {
    conn: &'conn Connection,
    inner: Option<libsql::Transaction>,
}

impl<'conn> Transaction<'conn> {
    pub(crate) fn new(conn: &'conn Connection, inner: libsql::Transaction) -> Self {
        Self {
            conn,
            inner: Some(inner),
        }
    }

    pub fn execute(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
    ) -> Result<usize, DbError> {
        let params = params.into_params()?.into_libsql();
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        let changed = self.conn.block_on_libsql(inner.execute(sql, params))?;
        usize::try_from(changed)
            .map_err(|_| DbError::InvalidParameter("changed row count exceeds usize".into()))
    }

    pub fn execute_batch(&self, sql: &str) -> Result<(), DbError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        self.conn
            .block_on_libsql(inner.execute_batch(sql))
            .map(drop)
    }

    pub fn query_one<T>(
        &self,
        sql: &str,
        params: impl crate::params::IntoParams,
        map: impl FnOnce(&crate::row::Row<'_>) -> Result<T, DbError>,
    ) -> Result<T, DbError> {
        let params = params.into_params()?.into_libsql();
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        let mut rows = self.conn.block_on_libsql(inner.query(sql, params))?;
        let Some(row) = self.conn.block_on_libsql(rows.next())? else {
            return Err(DbError::not_found());
        };
        map(&crate::row::Row::new(&row))
    }

    pub fn connection(&self) -> &Connection {
        self.conn
    }

    pub fn commit(mut self) -> Result<(), DbError> {
        let inner = self
            .inner
            .take()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        self.conn.block_on_libsql(inner.commit())?;
        Ok(())
    }

    pub fn rollback(mut self) -> Result<(), DbError> {
        let inner = self
            .inner
            .take()
            .ok_or_else(|| DbError::InvalidParameter("transaction already closed".into()))?;
        self.conn.block_on_libsql(inner.rollback())?;
        Ok(())
    }
}

impl fmt::Debug for Transaction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transaction").finish_non_exhaustive()
    }
}
