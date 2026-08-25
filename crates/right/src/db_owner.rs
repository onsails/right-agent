//! Aggregator-owned per-agent database connections and lifecycle state.
//!
//! This module is the only live runtime owner of per-agent `data.db`
//! connections. Callers submit scoped operations; the connection itself never
//! crosses this boundary.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

#[path = "db_owner_ops.rs"]
mod ops;

use right_mcp::internal_db as wire;

pub(crate) type LocalDbFuture<'a, T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send + 'a>>;

/// The four deep database interfaces exposed to the internal API. Request
/// variants are server-internal and typed; no SQL, connection, or generic
/// key/value operation crosses the owner boundary.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum InteractionStateOp {
    ArchiveMessage(wire::ArchiveMessageRequest),
    MarkMessageRouted(wire::MarkMessageRoutedRequest),
    GetActiveSession(wire::GetActiveSessionRequest),
    CreateSession(wire::CreateSessionRequest),
    DeactivateCurrentSession(wire::DeactivateCurrentSessionRequest),
    ActivateSession(wire::ActivateSessionRequest),
    TouchSession(wire::TouchSessionRequest),
    ListSessions(wire::ListSessionsRequest),
    FindSessionsByUuid(wire::FindSessionsByUuidRequest),
    FindSessionByRoot(wire::FindSessionByRootRequest),
    LatestAssistantIsUniqueExact(wire::LatestAssistantIsUniqueExactRequest),
    IsRecentRoutedTarget(wire::IsRecentRoutedTargetRequest),
    FetchMessagesByIds(wire::FetchMessagesByIdsRequest),
    ConversationLatestTurnId(wire::ConversationLatestTurnIdRequest),
    ThreadFocusGet(wire::ThreadFocusGetRequest),
    ThreadFocusSetOperator(wire::ThreadFocusSetOperatorRequest),
    ErrorDetailInsert(wire::ErrorDetailInsertRequest),
    ErrorDetailGet(wire::ErrorDetailGetRequest),
    LifecycleBumpUseMany(wire::LifecycleBumpUseManyRequest),
    BootstrapOwner(wire::BootstrapOwnerRequest),
    BootstrapClaimOwner(wire::BootstrapClaimOwnerRequest),
    BootstrapMissingStages(wire::BootstrapStageScopeRequest),
    BootstrapFirstMissingStage(wire::BootstrapStageScopeRequest),
    BootstrapIssuedQuestionStage(wire::BootstrapStageScopeRequest),
    BootstrapRecordQuestionIssue(wire::BootstrapRecordQuestionIssueRequest),
    BootstrapRecordCurrentAnswer(wire::BootstrapRecordCurrentAnswerRequest),
    BootstrapRecordAnswer(wire::BootstrapRecordAnswerRequest),
    BootstrapRecordedAnswers(wire::BootstrapStageScopeRequest),
    BootstrapClear(wire::BootstrapClearRequest),
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum RunLedgerOp {
    CronSpecsList(wire::CronSpecsListRequest),
    CronSpecDetail(wire::CronSpecDetailRequest),
    CronRecentRuns(wire::CronRecentRunsRequest),
    CronDeleteSpec(wire::CronDeleteSpecRequest),
    EnqueueBackgroundRun(wire::EnqueueBackgroundRunRequest),
    CronClearTriggered(wire::CronJobRequest),
    CronInsertRunningRun(wire::CronInsertRunningRunRequest),
    MarkBackgroundSpawned(wire::MarkBackgroundSpawnedRequest),
    PersistRunOutput(wire::PersistRunOutputRequest),
    FinishRun(wire::FinishRunRequest),
    MarkHandoffFailed(wire::MarkHandoffFailedRequest),
    RecoverInterruptedHandoffs(wire::RecoverInterruptedHandoffsRequest),
    CronMarkInterruptedByShutdown(wire::CronMarkInterruptedByShutdownRequest),
    DeliveryFetchPending(wire::DeliveryFetchPendingRequest),
    DeliveryMarkOutcome(wire::DeliveryMarkOutcomeRequest),
    DeliveryDeduplicateJob(wire::DeliveryDeduplicateJobRequest),
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum SecretsRegistryOp {
    AuthStatus(wire::AuthStatusRequest),
    AuthTokenGet(wire::AuthTokenGetRequest),
    AuthTokenSave(wire::AuthTokenSaveRequest),
    AuthTokenDelete(wire::AuthTokenDeleteRequest),
    NoticeTokenGetOrCreate(wire::NoticeTokenGetOrCreateRequest),
    McpOauthStateSet(wire::McpOauthStateSetRequest),
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum LearningMemoryOp {
    UsageInsertEvent(wire::UsageInsertEventRequest),
    LearningEventInsert(wire::LearningEventInsertRequest),
    LearningTodaySpend(wire::LearningTodaySpendRequest),
    LearningRecordBudgetSkip(wire::LearningRecordBudgetSkipRequest),
    LearningAuthoredSkillThisTurn(wire::LearningAuthoredSkillThisTurnRequest),
    LearningLinkCronAuthored(wire::LearningLinkCronAuthoredRequest),
    LearningLatestInteractiveContextTokens(wire::LearningLatestInteractiveContextTokensRequest),
    LearningTurnBaselines(wire::LearningTurnBaselinesRequest),
    LearningProbeCostSpike(wire::LearningProbeCostSpikeRequest),
    CuratorLoadState(wire::CuratorLoadStateRequest),
    CuratorSaveState(wire::CuratorSaveStateRequest),
    CuratorInsertRun(wire::CuratorInsertRunRequest),
    CuratorLatestChatActivity(wire::CuratorLatestChatActivityRequest),
    CuratorChangeCount(wire::CuratorChangeCountRequest),
    CuratorArchivedSnapshot(wire::CuratorArchivedSnapshotRequest),
    CuratorApplyTransitions(wire::CuratorApplyTransitionsRequest),
    CuratorFinalize(wire::CuratorFinalizeRequest),
    LifecycleArchivedSince(wire::LifecycleArchivedSinceRequest),
    SkillLifecycleGet(wire::SkillLifecycleGetRequest),
    SkillLifecycleList(wire::SkillLifecycleListRequest),
    SkillPin(wire::SkillPinRequest),
    AlertClear(wire::AlertClearRequest),
    SkillSpendRecord(wire::SkillSpendRecordRequest),
    SkillSpendBySkill(wire::SkillSpendBySkillRequest),
    AlertCheckAndRecord(wire::AlertCheckAndRecordRequest),
    AlertRecord(wire::AlertRecordRequest),
    RetainEnqueue(wire::RetainEnqueueRequest),
    RetainClaimBatch(wire::RetainClaimBatchRequest),
    RetainAck(wire::RetainAckRequest),
    RetainNack(wire::RetainNackRequest),
    RetainQueueStats(wire::RetainQueueStatsRequest),
    DashboardActivity(wire::DashboardActivityRequest),
    DashboardRunDetail(wire::DashboardRunDetailRequest),
    DashboardOverview(wire::DashboardOverviewRequest),
    DashboardUsage(wire::DashboardUsageRequest),
    DashboardLearning(wire::DashboardLearningRequest),
    DashboardSkillLifecycle(wire::DashboardSkillLifecycleRequest),
    DashboardSkillSpend(wire::DashboardSkillSpendRequest),
}

#[derive(Debug)]
pub(crate) enum OwnerResponse {
    Interaction(serde_json::Value),
    Runs(serde_json::Value),
    Secrets(serde_json::Value),
    Learning(serde_json::Value),
}

const OWNER_STARTING: u8 = 0;
const OWNER_READY: u8 = 1;
const OWNER_DRAINING: u8 = 2;
const OWNER_FAILED: u8 = 3;

pub(crate) type DbOperationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DbOwnerError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbOwnerState {
    Starting,
    Ready,
    Draining,
    Failed,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DbOwnerError {
    #[error("database owner for agent '{agent}' is {state:?}")]
    Unavailable { agent: String, state: DbOwnerState },
    #[error("database owner for agent '{agent}' has not opened its connection")]
    NotOpened { agent: String },
    #[error("failed to open database for agent '{agent}': {source}")]
    Open {
        agent: String,
        #[source]
        source: right_db::DbError,
    },
    #[error("database operation failed: {0}")]
    Database(#[from] right_db::DbError),
    #[error("MCP credential operation failed: {0}")]
    Credentials(#[from] right_mcp::credentials::CredentialError),
    #[error("invalid owner operation: {0}")]
    Invalid(String),
    #[error("owner operation conflict: {0}")]
    Conflict(String),
    #[error("owner domain operation failed: {0}")]
    Domain(String),
    #[error("database owner for agent '{agent}' did not drain before timeout")]
    DrainTimeout { agent: String },
    #[error("database owner for agent '{agent}' already exists")]
    AlreadyRegistered { agent: String },
    #[error("database owner for agent '{agent}' was not found")]
    NotFound { agent: String },
    #[error("runtime task for database owner '{agent}' failed: {source}")]
    TaskJoin {
        agent: String,
        #[source]
        source: tokio::task::JoinError,
    },
}

pub(crate) struct AgentDbOwner {
    agent: String,
    agent_dir: PathBuf,
    state: AtomicU8,
    connection: Mutex<Option<right_db::Connection>>,
    mcp_mutation_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl std::fmt::Debug for AgentDbOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentDbOwner")
            .field("agent", &self.agent)
            .field("agent_dir", &self.agent_dir)
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl AgentDbOwner {
    pub(crate) fn starting(agent: impl Into<String>, agent_dir: PathBuf) -> Self {
        Self {
            agent: agent.into(),
            agent_dir,
            state: AtomicU8::new(OWNER_STARTING),
            connection: Mutex::new(None),
            mcp_mutation_locks: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn agent(&self) -> &str {
        &self.agent
    }

    pub(crate) fn state(&self) -> DbOwnerState {
        match self.state.load(Ordering::Acquire) {
            OWNER_STARTING => DbOwnerState::Starting,
            OWNER_READY => DbOwnerState::Ready,
            OWNER_DRAINING => DbOwnerState::Draining,
            _ => DbOwnerState::Failed,
        }
    }

    pub(crate) async fn lock_mcp_mutation(
        &self,
        server_name: &str,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self
            .mcp_mutation_locks
            .lock()
            .await
            .entry(server_name.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        lock.lock_owned().await
    }

    pub(crate) async fn open_and_migrate(&self) -> Result<(), DbOwnerError> {
        let connection = right_db::open_connection(&self.agent_dir, true)
            .await
            .map_err(|source| {
                self.state.store(OWNER_FAILED, Ordering::Release);
                DbOwnerError::Open {
                    agent: self.agent.clone(),
                    source,
                }
            })?;
        // A bot may have crashed after claiming retain items. Reclaim expired
        // leases before publishing Ready, then every claim reclaims again.
        right_memory::retain_queue::reclaim_expired(&connection)
            .await
            .map_err(|error| DbOwnerError::Domain(error.to_string()))?;
        let mut slot = self.connection.lock().await;
        *slot = Some(connection);
        // Ready is private until the completed bundle is inserted into the
        // registry; builders can restore owner-local state before publication.
        self.state.store(OWNER_READY, Ordering::Release);
        Ok(())
    }

    pub(crate) fn begin_draining(&self) {
        let _ = self.state.compare_exchange(
            OWNER_READY,
            OWNER_DRAINING,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub(crate) fn mark_failed(&self) {
        self.state.store(OWNER_FAILED, Ordering::Release);
    }

    pub(crate) async fn wait_for_idle(&self, timeout: Duration) -> Result<(), DbOwnerError> {
        tokio::time::timeout(timeout, self.connection.lock())
            .await
            .map(drop)
            .map_err(|_| DbOwnerError::DrainTimeout {
                agent: self.agent.clone(),
            })
    }

    pub(crate) async fn with_connection<T, F>(&self, operation: F) -> Result<T, DbOwnerError>
    where
        T: Send,
        F: for<'a> FnOnce(&'a right_db::Connection) -> DbOperationFuture<'a, T> + Send,
    {
        let state = self.state();
        if state != DbOwnerState::Ready {
            return Err(DbOwnerError::Unavailable {
                agent: self.agent.clone(),
                state,
            });
        }
        let slot = self.connection.lock().await;
        let connection = slot.as_ref().ok_or_else(|| DbOwnerError::NotOpened {
            agent: self.agent.clone(),
        })?;
        operation(connection).await
    }

    /// Run an Aggregator-local operation while holding the owner mutex. The
    /// connection is lifetime-bound to the closure and cannot escape.
    pub(crate) async fn local_operation<T, F>(&self, operation: F) -> anyhow::Result<T>
    where
        T: Send,
        F: for<'a> FnOnce(&'a right_db::Connection) -> LocalDbFuture<'a, T> + Send,
    {
        let state = self.state();
        if state != DbOwnerState::Ready {
            return Err(DbOwnerError::Unavailable {
                agent: self.agent.clone(),
                state,
            }
            .into());
        }
        let slot = self.connection.lock().await;
        let connection = slot.as_ref().ok_or_else(|| DbOwnerError::NotOpened {
            agent: self.agent.clone(),
        })?;
        operation(connection).await
    }

    pub(crate) async fn restore_mcp_servers(
        &self,
    ) -> Result<Vec<right_mcp::credentials::McpServerEntry>, DbOwnerError> {
        self.with_connection(|connection| {
            Box::pin(async move {
                right_mcp::credentials::db_list_servers(connection)
                    .await
                    .map_err(Into::into)
            })
        })
        .await
    }

    pub(crate) async fn restore_http_headers(
        &self,
        server_name: &str,
    ) -> Result<Vec<right_mcp::credentials::HttpHeaderSecret>, DbOwnerError> {
        let server_name = server_name.to_owned();
        self.with_connection(move |connection| {
            Box::pin(async move {
                right_mcp::credentials::db_list_http_headers(connection, &server_name)
                    .await
                    .map_err(Into::into)
            })
        })
        .await
    }

    pub(crate) async fn update_mcp_instructions(
        &self,
        name: String,
        instructions: Option<String>,
    ) -> Result<(), DbOwnerError> {
        self.with_connection(move |connection| {
            Box::pin(async move {
                right_mcp::credentials::db_update_instructions(
                    connection,
                    &name,
                    instructions.as_deref(),
                )
                .await
                .map_err(Into::into)
            })
        })
        .await
    }

    pub(crate) async fn replace_mcp_server(
        &self,
        name: String,
        url: String,
        auth: right_mcp::credentials::McpServerAuth,
    ) -> Result<Option<right_mcp::credentials::McpServerSnapshot>, DbOwnerError> {
        self.with_connection(move |connection| {
            Box::pin(async move {
                right_mcp::credentials::db_replace_server(connection, &name, &url, &auth)
                    .await
                    .map_err(Into::into)
            })
        })
        .await
    }

    pub(crate) async fn rollback_mcp_server_replacement(
        &self,
        name: String,
        previous: Option<right_mcp::credentials::McpServerSnapshot>,
    ) -> Result<(), DbOwnerError> {
        self.with_connection(move |connection| {
            Box::pin(async move {
                match previous {
                    Some(snapshot) => {
                        right_mcp::credentials::db_restore_server_snapshot(connection, &snapshot)
                            .await
                            .map_err(Into::into)
                    }
                    None => match right_mcp::credentials::db_remove_server(connection, &name).await
                    {
                        Ok(())
                        | Err(right_mcp::credentials::CredentialError::ServerNotFound(_)) => Ok(()),
                        Err(error) => Err(error.into()),
                    },
                }
            })
        })
        .await
    }
}

#[derive(Clone, Default)]
pub(crate) struct DbOwnerRegistry {
    bundles: Arc<RwLock<HashMap<String, Arc<AgentRuntimeBundle>>>>,
}

impl DbOwnerRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) async fn open_initial(
        entries: impl IntoIterator<Item = (String, PathBuf)>,
    ) -> Result<Self, DbOwnerError> {
        let registry = Self::new();
        for (agent, agent_dir) in entries {
            let owner = Arc::new(AgentDbOwner::starting(agent.clone(), agent_dir));
            registry.insert_starting(Arc::clone(&owner)).await?;
            owner.open_and_migrate().await?;
        }
        Ok(registry)
    }

    #[cfg(test)]
    pub(crate) async fn insert_starting(
        &self,
        owner: Arc<AgentDbOwner>,
    ) -> Result<(), DbOwnerError> {
        let mut bundles = self.bundles.write().await;
        if bundles.contains_key(owner.agent()) {
            return Err(DbOwnerError::AlreadyRegistered {
                agent: owner.agent().to_owned(),
            });
        }
        bundles.insert(
            owner.agent().to_owned(),
            Arc::new(AgentRuntimeBundle::new(owner)),
        );
        Ok(())
    }
    pub(crate) async fn insert_bundle(
        &self,
        bundle: Arc<AgentRuntimeBundle>,
    ) -> Result<(), DbOwnerError> {
        let agent = bundle.owner.agent().to_owned();
        let mut bundles = self.bundles.write().await;
        if bundles.contains_key(&agent) {
            return Err(DbOwnerError::AlreadyRegistered { agent });
        }
        bundles.insert(agent, bundle);
        Ok(())
    }

    pub(crate) async fn get(&self, agent: &str) -> Result<Arc<AgentDbOwner>, DbOwnerError> {
        self.bundles
            .read()
            .await
            .get(agent)
            .and_then(|bundle| bundle.published().then(|| Arc::clone(&bundle.owner)))
            .ok_or_else(|| DbOwnerError::NotFound {
                agent: agent.to_owned(),
            })
    }

    pub(crate) async fn state(&self, agent: &str) -> Option<DbOwnerState> {
        self.bundles
            .read()
            .await
            .get(agent)
            .and_then(|bundle| bundle.published().then(|| bundle.owner.state()))
    }

    pub(crate) async fn remove(&self, agent: &str) -> Option<Arc<AgentRuntimeBundle>> {
        self.bundles.write().await.remove(agent)
    }

    pub(crate) async fn bundle(&self, agent: &str) -> Option<Arc<AgentRuntimeBundle>> {
        self.bundles.read().await.get(agent).cloned()
    }

    pub(crate) async fn all_bundles(&self) -> Vec<Arc<AgentRuntimeBundle>> {
        self.bundles.read().await.values().cloned().collect()
    }
}

pub(crate) struct AgentRuntimeBundle {
    pub(crate) owner: Arc<AgentDbOwner>,
    published: std::sync::atomic::AtomicBool,
    pub(crate) cancellation: CancellationToken,
    tasks: Mutex<JoinSet<()>>,
}

impl AgentRuntimeBundle {
    pub(crate) fn new(owner: Arc<AgentDbOwner>) -> Self {
        Self {
            owner,
            published: std::sync::atomic::AtomicBool::new(false),
            cancellation: CancellationToken::new(),
            tasks: Mutex::new(JoinSet::new()),
        }
    }
    pub(crate) fn publish(&self) {
        self.published.store(true, Ordering::Release);
    }

    fn published(&self) -> bool {
        self.published.load(Ordering::Acquire)
    }

    pub(crate) async fn spawn<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tasks.lock().await.spawn(task);
    }

    pub(crate) async fn drain(&self, timeout: Duration) -> Result<(), DbOwnerError> {
        self.owner.begin_draining();
        self.cancellation.cancel();
        {
            let mut tasks = self.tasks.lock().await;
            tasks.abort_all();
            while let Some(result) = tasks.join_next().await {
                if let Err(error) = result
                    && !error.is_cancelled()
                {
                    self.owner.mark_failed();
                    return Err(DbOwnerError::TaskJoin {
                        agent: self.owner.agent().to_owned(),
                        source: error,
                    });
                }
            }
        }
        self.owner.wait_for_idle(timeout).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::{AgentDbOwner, DbOwnerRegistry, DbOwnerState};

    #[tokio::test]
    async fn db_owner_is_not_ready_until_open_finishes() {
        let dir = tempdir().unwrap();
        let owner = Arc::new(AgentDbOwner::starting("alpha", dir.path().to_path_buf()));
        assert_eq!(owner.state(), DbOwnerState::Starting);
        owner.open_and_migrate().await.unwrap();
        assert_eq!(owner.state(), DbOwnerState::Ready);
    }

    #[tokio::test]
    async fn initial_registry_fails_if_any_agent_database_is_broken() {
        let good = tempdir().unwrap();
        let bad = tempdir().unwrap();
        std::fs::write(bad.path().join("data.db"), b"not a sqlite database").unwrap();

        let result = DbOwnerRegistry::open_initial([
            ("good".to_owned(), good.path().to_path_buf()),
            ("bad".to_owned(), bad.path().to_path_buf()),
        ])
        .await;

        assert!(result.is_err());
    }

    #[test]
    fn right_db_connection_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<right_db::Connection>();
    }

    #[tokio::test]
    async fn registry_readiness_waits_for_runtime_publication() {
        let dir = tempdir().unwrap();
        let owner = Arc::new(AgentDbOwner::starting("alpha", dir.path().to_path_buf()));
        owner.open_and_migrate().await.unwrap();
        let bundle = Arc::new(super::AgentRuntimeBundle::new(owner));
        let registry = DbOwnerRegistry::new();
        registry.insert_bundle(Arc::clone(&bundle)).await.unwrap();

        assert_eq!(registry.state("alpha").await, None);
        assert!(registry.get("alpha").await.is_err());
        bundle.publish();
        assert_eq!(registry.state("alpha").await, Some(DbOwnerState::Ready));
        assert!(registry.get("alpha").await.is_ok());
    }

    #[tokio::test]
    async fn db_owner_rejects_new_work_while_draining_and_drains_accepted_work() {
        let dir = tempdir().unwrap();
        let owner = Arc::new(AgentDbOwner::starting("alpha", dir.path().to_path_buf()));
        owner.open_and_migrate().await.unwrap();

        let accepted = {
            let owner = Arc::clone(&owner);
            tokio::spawn(async move {
                owner
                    .with_connection(|_conn| {
                        Box::pin(async move {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            Ok(())
                        })
                    })
                    .await
            })
        };
        tokio::task::yield_now().await;
        owner.begin_draining();

        let rejected = owner
            .with_connection(|_conn| Box::pin(async move { Ok(()) }))
            .await;
        assert!(rejected.is_err());
        owner.wait_for_idle(Duration::from_secs(1)).await.unwrap();
        accepted.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn owner_mcp_replacement_rollback_restores_complete_snapshot() {
        let dir = tempdir().unwrap();
        let owner = Arc::new(AgentDbOwner::starting("alpha", dir.path().to_path_buf()));
        owner.open_and_migrate().await.unwrap();
        let old_headers = vec![
            right_mcp::credentials::HttpHeaderSecret::new("authorization", "old-secret").unwrap(),
            right_mcp::credentials::HttpHeaderSecret::new("connection-id", "old-id").unwrap(),
        ];
        owner
            .replace_mcp_server(
                "server".to_owned(),
                "https://old.example/mcp".to_owned(),
                right_mcp::credentials::McpServerAuth::Headers(old_headers.clone()),
            )
            .await
            .unwrap();
        owner
            .with_connection(|connection| {
                Box::pin(async move {
                    connection
                        .execute(
                            "UPDATE mcp_servers SET instructions = 'old instructions', \
                             auth_token = 'old-access', refresh_token = 'old-refresh', \
                             token_endpoint = 'https://auth.example/token', client_id = 'client', \
                             client_secret = 'client-secret', expires_at = '2027-01-02T03:04:05Z', \
                             oauth_resource = 'https://old.example/resource' WHERE name = 'server'",
                            [],
                        )
                        .await?;
                    Ok(())
                })
            })
            .await
            .unwrap();
        let expected = owner
            .with_connection(|connection| {
                Box::pin(async move {
                    right_mcp::credentials::db_get_server_snapshot(connection, "server")
                        .await
                        .map_err(Into::into)
                })
            })
            .await
            .unwrap()
            .unwrap();

        let previous = owner
            .replace_mcp_server(
                "server".to_owned(),
                "https://new.example/mcp".to_owned(),
                right_mcp::credentials::McpServerAuth::Legacy {
                    auth_type: "bearer".to_owned(),
                    auth_header: None,
                    auth_token: Some("replacement-secret".to_owned()),
                },
            )
            .await
            .unwrap();
        owner
            .rollback_mcp_server_replacement("server".to_owned(), previous)
            .await
            .unwrap();
        let restored = owner
            .with_connection(|connection| {
                Box::pin(async move {
                    right_mcp::credentials::db_get_server_snapshot(connection, "server")
                        .await
                        .map_err(Into::into)
                })
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored, expected);
        assert_eq!(restored.headers, old_headers);
    }
}
