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
