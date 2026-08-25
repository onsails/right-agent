//! Aggregator-side retain queue adapter over the agent's [`AgentDbOwner`].
//!
//! Implements the injected `right-memory` queue traits against the owner's
//! scoped SQL primitives. This is the only production code outside
//! `db_owner.rs` that executes retain-queue SQL, and it does so inside the
//! owner's serialized connection — no `Connection` crosses this boundary.

use std::sync::Arc;
use std::time::Duration;

use right_memory::MemoryError;
use right_memory::retain_queue as sql;
use right_memory::retain_sink::{
    BoxFuture, NewPendingRetain, PendingRetainSink, RetainClaim, RetainLeaseQueue, RetainQueueStats,
};

use crate::db_owner::{AgentDbOwner, DbOperationFuture, DbOwnerError};

/// Queue outcome routed through the owner's `DbOwnerError` channel.
///
/// `right-memory`'s SQL primitives report `MemoryError`; the owner reports
/// `DbOwnerError`. Non-DB `MemoryError` variants (e.g. a lease conflict) are
/// flattened into `DbError::InvalidParameter` with the full chain in the
/// message so no context is lost at the boundary.
fn into_owner_error(result: Result<(), MemoryError>) -> Result<(), DbOwnerError> {
    match result {
        Ok(()) => Ok(()),
        Err(MemoryError::Db(source)) => Err(DbOwnerError::Database(source)),
        Err(other) => Err(DbOwnerError::Database(right_db::DbError::InvalidParameter(
            format!("retain queue operation failed: {other:#}"),
        ))),
    }
}

fn from_owner_error(error: DbOwnerError) -> MemoryError {
    MemoryError::HindsightOther(format!("{error:#}"))
}

/// Aggregator-local [`PendingRetainSink`] + [`RetainLeaseQueue`] over the
/// per-agent owner connection.
#[derive(Debug, Clone)]
pub(crate) struct OwnerRetainQueue {
    owner: Arc<AgentDbOwner>,
}

impl OwnerRetainQueue {
    pub(crate) fn new(owner: Arc<AgentDbOwner>) -> Self {
        Self { owner }
    }
}

impl PendingRetainSink for OwnerRetainQueue {
    fn enqueue(&self, item: NewPendingRetain) -> BoxFuture<'static, Result<(), MemoryError>> {
        let owner = Arc::clone(&self.owner);
        Box::pin(async move {
            owner
                .with_connection(move |connection| -> DbOperationFuture<'_, ()> {
                    Box::pin(async move {
                        into_owner_error(
                            sql::enqueue(
                                connection,
                                &item.source,
                                &item.content,
                                item.context.as_deref(),
                                item.document_id.as_deref(),
                                item.update_mode.as_deref(),
                                item.tags.as_deref(),
                            )
                            .await,
                        )
                    })
                })
                .await
                .map_err(from_owner_error)
        })
    }
}

impl RetainLeaseQueue for OwnerRetainQueue {
    fn claim_batch(
        &self,
        limit: usize,
        lease_ttl: Duration,
    ) -> BoxFuture<'static, Result<RetainClaim, MemoryError>> {
        let owner = Arc::clone(&self.owner);
        Box::pin(async move {
            owner
                .with_connection(move |connection| -> DbOperationFuture<'_, RetainClaim> {
                    Box::pin(async move {
                        sql::claim_batch(connection, limit, lease_ttl)
                            .await
                            .map_err(|error| match error {
                                MemoryError::Db(source) => DbOwnerError::Database(source),
                                other => {
                                    DbOwnerError::Database(right_db::DbError::InvalidParameter(
                                        format!("retain claim failed: {other:#}"),
                                    ))
                                }
                            })
                    })
                })
                .await
                .map_err(from_owner_error)
        })
    }

    fn ack(&self, claim_token: &str, id: i64) -> BoxFuture<'static, Result<(), MemoryError>> {
        let owner = Arc::clone(&self.owner);
        let claim_token = claim_token.to_owned();
        Box::pin(async move {
            owner
                .with_connection(move |connection| -> DbOperationFuture<'_, ()> {
                    Box::pin(async move {
                        into_owner_error(sql::ack(connection, &claim_token, id).await)
                    })
                })
                .await
                .map_err(from_owner_error)
        })
    }

    fn nack(
        &self,
        claim_token: &str,
        id: i64,
        retry: bool,
        error: &str,
    ) -> BoxFuture<'static, Result<(), MemoryError>> {
        let owner = Arc::clone(&self.owner);
        let claim_token = claim_token.to_owned();
        let error = error.to_owned();
        Box::pin(async move {
            owner
                .with_connection(move |connection| -> DbOperationFuture<'_, ()> {
                    Box::pin(async move {
                        into_owner_error(
                            sql::nack(connection, &claim_token, id, retry, &error).await,
                        )
                    })
                })
                .await
                .map_err(from_owner_error)
        })
    }

    fn stats(&self) -> BoxFuture<'static, Result<RetainQueueStats, MemoryError>> {
        let owner = Arc::clone(&self.owner);
        Box::pin(async move {
            owner
                .with_connection(
                    move |connection| -> DbOperationFuture<'_, RetainQueueStats> {
                        Box::pin(async move {
                            let count =
                                sql::count(connection).await.map_err(|error| match error {
                                    MemoryError::Db(source) => DbOwnerError::Database(source),
                                    other => {
                                        DbOwnerError::Database(right_db::DbError::InvalidParameter(
                                            format!("retain stats failed: {other:#}"),
                                        ))
                                    }
                                })?;
                            let oldest_age =
                                sql::oldest_age(connection)
                                    .await
                                    .map_err(|error| match error {
                                        MemoryError::Db(source) => DbOwnerError::Database(source),
                                        other => DbOwnerError::Database(
                                            right_db::DbError::InvalidParameter(format!(
                                                "retain stats failed: {other:#}"
                                            )),
                                        ),
                                    })?;
                            Ok(RetainQueueStats { count, oldest_age })
                        })
                    },
                )
                .await
                .map_err(from_owner_error)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn owner_queue_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Arc::new(AgentDbOwner::starting("alpha", dir.path().to_path_buf()));
        owner.open_and_migrate().await.unwrap();
        let queue = OwnerRetainQueue::new(owner);

        queue
            .enqueue(NewPendingRetain {
                source: "aggregator".to_owned(),
                content: "c".to_owned(),
                context: None,
                document_id: None,
                update_mode: None,
                tags: None,
            })
            .await
            .unwrap();
        assert_eq!(queue.stats().await.unwrap().count, 1);

        let claim = queue
            .claim_batch(10, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(claim.items.len(), 1);
        queue
            .ack(&claim.claim_token, claim.items[0].id)
            .await
            .unwrap();
        assert_eq!(queue.stats().await.unwrap().count, 0);
    }
}
