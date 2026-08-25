//! Bot-side pending-retain queue adapter over typed internal IPC.
//!
//! The bot never opens `data.db`. `ResilientHindsight` injects this adapter for
//! enqueue, and the lease drain loop injects the same adapter for
//! claim/ack/nack. The Aggregator executes every operation inside the sole
//! `AgentDbOwner` connection.

use std::sync::Arc;
use std::time::Duration;

use right_mcp::internal_db as ipc;
use right_memory::MemoryError;
use right_memory::retain_queue::PendingRetain;
use right_memory::retain_sink::{
    BoxFuture, NewPendingRetain, PendingRetainSink, RetainClaim, RetainLeaseQueue, RetainQueueStats,
};

#[derive(Clone)]
pub(crate) struct IpcRetainQueue {
    client: Arc<right_mcp::internal_client::InternalClient>,
    agent: String,
}

impl std::fmt::Debug for IpcRetainQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcRetainQueue")
            .field("agent", &self.agent)
            .finish_non_exhaustive()
    }
}

impl IpcRetainQueue {
    pub(crate) fn new(
        client: Arc<right_mcp::internal_client::InternalClient>,
        agent: impl Into<String>,
    ) -> Self {
        Self {
            client,
            agent: agent.into(),
        }
    }
}

fn ipc_error(error: ipc::InternalDbError) -> MemoryError {
    MemoryError::HindsightOther(format!("Aggregator retain queue unavailable: {error:#}"))
}

fn request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl PendingRetainSink for IpcRetainQueue {
    fn enqueue(&self, item: NewPendingRetain) -> BoxFuture<'static, Result<(), MemoryError>> {
        let client = Arc::clone(&self.client);
        let request = ipc::RetainEnqueueRequest {
            agent: self.agent.clone(),
            request_id: request_id(),
            item: ipc::RetainEnqueueItemDto {
                source: item.source,
                content: item.content,
                context: item.context,
                document_id: item.document_id,
                update_mode: item.update_mode,
                tags: item.tags.unwrap_or_default(),
            },
        };
        Box::pin(async move {
            client
                .retain_enqueue(&request)
                .await
                .map(drop)
                .map_err(ipc_error)
        })
    }
}

impl RetainLeaseQueue for IpcRetainQueue {
    fn claim_batch(
        &self,
        limit: usize,
        lease_ttl: Duration,
    ) -> BoxFuture<'static, Result<RetainClaim, MemoryError>> {
        let client = Arc::clone(&self.client);
        let request = ipc::RetainClaimBatchRequest {
            agent: self.agent.clone(),
            limit: u32::try_from(limit).unwrap_or(u32::MAX),
            lease_ttl_secs: u32::try_from(lease_ttl.as_secs()).unwrap_or(u32::MAX),
        };
        Box::pin(async move {
            let response = client
                .retain_claim_batch(&request)
                .await
                .map_err(ipc_error)?;
            Ok(RetainClaim {
                claim_token: response.claim.claim_token,
                lease_expires_at: response.claim.lease_expires_at,
                items: response
                    .claim
                    .items
                    .into_iter()
                    .map(|item| PendingRetain {
                        id: item.id,
                        content: item.content,
                        context: item.context,
                        document_id: item.document_id,
                        update_mode: item.update_mode,
                        tags: (!item.tags.is_empty()).then_some(item.tags),
                        created_at: item.created_at,
                        attempts: i64::from(item.attempts),
                    })
                    .collect(),
                // The wire protocol intentionally omits this owner-observability
                // counter; it is logged server-side. The bot report therefore
                // leaves it zero.
                dropped_age: 0,
            })
        })
    }

    fn ack(&self, claim_token: &str, id: i64) -> BoxFuture<'static, Result<(), MemoryError>> {
        let client = Arc::clone(&self.client);
        let request = ipc::RetainAckRequest {
            agent: self.agent.clone(),
            claim_token: claim_token.to_owned(),
            ids: vec![id],
        };
        Box::pin(async move {
            client
                .retain_ack(&request)
                .await
                .map(drop)
                .map_err(ipc_error)
        })
    }

    fn nack(
        &self,
        claim_token: &str,
        id: i64,
        retry: bool,
        error: &str,
    ) -> BoxFuture<'static, Result<(), MemoryError>> {
        let client = Arc::clone(&self.client);
        let request = ipc::RetainNackRequest {
            agent: self.agent.clone(),
            claim_token: claim_token.to_owned(),
            ids: vec![id],
            retry,
            error: error.to_owned(),
        };
        Box::pin(async move {
            client
                .retain_nack(&request)
                .await
                .map(drop)
                .map_err(ipc_error)
        })
    }

    fn stats(&self) -> BoxFuture<'static, Result<RetainQueueStats, MemoryError>> {
        let client = Arc::clone(&self.client);
        let request = ipc::RetainQueueStatsRequest {
            agent: self.agent.clone(),
        };
        Box::pin(async move {
            let response = client
                .retain_queue_stats(&request)
                .await
                .map_err(ipc_error)?;
            Ok(RetainQueueStats {
                count: response.count,
                oldest_age: response.oldest_age_secs.map(Duration::from_secs),
            })
        })
    }
}
