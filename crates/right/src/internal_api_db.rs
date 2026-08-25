//! Finite typed database router served exclusively on `internal.sock`.
//!
//! The outer internal API listener owns the Unix socket. This router is only
//! nested there; it is never merged into the TCP MCP service. Every route has
//! one concrete request DTO and one concrete response DTO. Unknown routes are
//! absent (404), malformed JSON is rejected (400), and no SQL-like generic
//! operation exists.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{OriginalUri, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use axum::routing::post;
use axum::{Json, Router};
use right_mcp::internal_db as wire;
use secrecy::ExposeSecret as _;
use serde::de::DeserializeOwned;
use subtle::ConstantTimeEq as _;

use crate::db_owner::{
    DbOwnerError, DbOwnerState, InteractionStateOp, LearningMemoryOp, OwnerResponse, RunLedgerOp,
    SecretsRegistryOp,
};
use crate::internal_api::InternalState;

const PROVIDER_ERROR_MESSAGE: &str = "provider binding resolution failed";

pub(crate) fn router() -> Router<InternalState> {
    let mut router = Router::new();
    for path in ROUTES {
        router = router.route(path, post(dispatch));
    }
    router
}

const ROUTES: &[&str] = &[
    wire::ROUTE_ARCHIVE_MESSAGE,
    wire::ROUTE_MARK_MESSAGE_ROUTED,
    wire::ROUTE_GET_ACTIVE_SESSION,
    wire::ROUTE_CREATE_SESSION,
    wire::ROUTE_DEACTIVATE_CURRENT_SESSION,
    wire::ROUTE_ACTIVATE_SESSION,
    wire::ROUTE_TOUCH_SESSION,
    wire::ROUTE_LIST_SESSIONS,
    wire::ROUTE_FIND_SESSIONS_BY_UUID,
    wire::ROUTE_FIND_SESSION_BY_ROOT,
    wire::ROUTE_LATEST_ASSISTANT_IS_UNIQUE_EXACT,
    wire::ROUTE_IS_RECENT_ROUTED_TARGET,
    wire::ROUTE_FETCH_MESSAGES_BY_IDS,
    wire::ROUTE_CONVERSATION_LATEST_TURN_ID,
    wire::ROUTE_THREAD_FOCUS_GET,
    wire::ROUTE_THREAD_FOCUS_SET_OPERATOR,
    wire::ROUTE_ERROR_DETAIL_INSERT,
    wire::ROUTE_ERROR_DETAIL_GET,
    wire::ROUTE_LIFECYCLE_BUMP_USE_MANY,
    wire::ROUTE_BOOTSTRAP_OWNER,
    wire::ROUTE_BOOTSTRAP_CLAIM_OWNER,
    wire::ROUTE_BOOTSTRAP_MISSING_STAGES,
    wire::ROUTE_BOOTSTRAP_FIRST_MISSING_STAGE,
    wire::ROUTE_BOOTSTRAP_ISSUED_QUESTION_STAGE,
    wire::ROUTE_BOOTSTRAP_RECORD_QUESTION_ISSUE,
    wire::ROUTE_BOOTSTRAP_RECORD_CURRENT_ANSWER,
    wire::ROUTE_BOOTSTRAP_RECORD_ANSWER,
    wire::ROUTE_BOOTSTRAP_RECORDED_ANSWERS,
    wire::ROUTE_BOOTSTRAP_CLEAR,
    wire::ROUTE_CRON_SPECS_LIST,
    wire::ROUTE_CRON_SPEC_DETAIL,
    wire::ROUTE_CRON_RECENT_RUNS,
    wire::ROUTE_CRON_DELETE_SPEC,
    wire::ROUTE_CRON_CLEAR_TRIGGERED,
    wire::ROUTE_ENQUEUE_BACKGROUND_RUN,
    wire::ROUTE_CRON_INSERT_RUNNING_RUN,
    wire::ROUTE_MARK_BACKGROUND_SPAWNED,
    wire::ROUTE_PERSIST_RUN_OUTPUT,
    wire::ROUTE_FINISH_RUN,
    wire::ROUTE_MARK_HANDOFF_FAILED,
    wire::ROUTE_RECOVER_INTERRUPTED_HANDOFFS,
    wire::ROUTE_CRON_MARK_INTERRUPTED_BY_SHUTDOWN,
    wire::ROUTE_DELIVERY_FETCH_PENDING,
    wire::ROUTE_DELIVERY_MARK_OUTCOME,
    wire::ROUTE_DELIVERY_DEDUPLICATE_JOB,
    wire::ROUTE_AUTH_STATUS,
    wire::ROUTE_AUTH_TOKEN_GET,
    wire::ROUTE_AUTH_TOKEN_SAVE,
    wire::ROUTE_AUTH_TOKEN_DELETE,
    wire::ROUTE_NOTICE_TOKEN_GET_OR_CREATE,
    wire::ROUTE_MCP_OAUTH_STATE_SET,
    wire::ROUTE_USAGE_INSERT_EVENT,
    wire::ROUTE_LEARNING_EVENT_INSERT,
    wire::ROUTE_LEARNING_TODAY_SPEND,
    wire::ROUTE_LEARNING_RECORD_BUDGET_SKIP,
    wire::ROUTE_LEARNING_AUTHORED_SKILL_THIS_TURN,
    wire::ROUTE_LEARNING_LINK_CRON_AUTHORED,
    wire::ROUTE_LEARNING_LATEST_INTERACTIVE_CONTEXT_TOKENS,
    wire::ROUTE_LEARNING_TURN_BASELINES,
    wire::ROUTE_LEARNING_PROBE_COST_SPIKE,
    wire::ROUTE_CURATOR_LOAD_STATE,
    wire::ROUTE_CURATOR_SAVE_STATE,
    wire::ROUTE_CURATOR_INSERT_RUN,
    wire::ROUTE_CURATOR_LATEST_CHAT_ACTIVITY,
    wire::ROUTE_CURATOR_CHANGE_COUNT,
    wire::ROUTE_CURATOR_ARCHIVED_SNAPSHOT,
    wire::ROUTE_CURATOR_APPLY_TRANSITIONS,
    wire::ROUTE_CURATOR_FINALIZE,
    wire::ROUTE_LIFECYCLE_ARCHIVED_SINCE,
    wire::ROUTE_SKILL_LIFECYCLE_GET,
    wire::ROUTE_SKILL_LIFECYCLE_LIST,
    wire::ROUTE_SKILL_PIN,
    wire::ROUTE_SKILL_SPEND_RECORD,
    wire::ROUTE_SKILL_SPEND_BY_SKILL,
    wire::ROUTE_ALERT_CHECK_AND_RECORD,
    wire::ROUTE_ALERT_RECORD,
    wire::ROUTE_ALERT_CLEAR,
    wire::ROUTE_RETAIN_ENQUEUE,
    wire::ROUTE_RETAIN_CLAIM_BATCH,
    wire::ROUTE_RETAIN_ACK,
    wire::ROUTE_RETAIN_NACK,
    wire::ROUTE_RETAIN_QUEUE_STATS,
    wire::ROUTE_DASHBOARD_ACTIVITY,
    wire::ROUTE_DASHBOARD_RUN_DETAIL,
    wire::ROUTE_DASHBOARD_OVERVIEW,
    wire::ROUTE_DASHBOARD_USAGE,
    wire::ROUTE_DASHBOARD_LEARNING,
    wire::ROUTE_DASHBOARD_SKILL_LIFECYCLE,
    wire::ROUTE_DASHBOARD_SKILL_SPEND,
    wire::ROUTE_PROVIDER_BINDINGS_RESOLVE,
    wire::ROUTE_PROVIDER_BINDINGS_RESOLVE_NAMED,
];

fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, Box<Response>> {
    serde_json::from_slice(body).map_err(|error| {
        tracing::warn!(error = %error, "rejecting malformed internal DB request");
        Box::new(error_response(wire::DbErrorCategory::Invalid).into_response())
    })
}

async fn owner(
    state: &InternalState,
    agent: &str,
) -> Result<Arc<crate::db_owner::AgentDbOwner>, Response> {
    state
        .db_owners
        .get(agent)
        .await
        .map_err(owner_error_response)
}

fn success(result: Result<OwnerResponse, DbOwnerError>) -> Response {
    match result {
        Ok(
            OwnerResponse::Interaction(value)
            | OwnerResponse::Runs(value)
            | OwnerResponse::Secrets(value)
            | OwnerResponse::Learning(value),
        ) => Json(value).into_response(),
        Err(error) => owner_error_response(error),
    }
}

fn owner_error_response(error: DbOwnerError) -> Response {
    let category = match &error {
        DbOwnerError::Unavailable {
            state: DbOwnerState::Starting,
            ..
        }
        | DbOwnerError::NotOpened { .. } => wire::DbErrorCategory::NotReady,
        DbOwnerError::Unavailable { .. }
        | DbOwnerError::Open { .. }
        | DbOwnerError::DrainTimeout { .. } => wire::DbErrorCategory::Unavailable,
        DbOwnerError::NotFound { .. } | DbOwnerError::Database(right_db::DbError::NotFound) => {
            wire::DbErrorCategory::NotFound
        }
        DbOwnerError::Conflict(_) => wire::DbErrorCategory::Conflict,
        DbOwnerError::Invalid(_)
        | DbOwnerError::Database(
            right_db::DbError::InvalidParameter(_) | right_db::DbError::Constraint(_),
        ) => wire::DbErrorCategory::Invalid,
        DbOwnerError::Database(error)
            if format!("{error:#}").to_ascii_lowercase().contains("busy") =>
        {
            wire::DbErrorCategory::Transient
        }
        _ => wire::DbErrorCategory::Internal,
    };
    tracing::error!(?category, error = %format!("{error:#}"), "internal DB owner operation failed");
    error_response(category).into_response()
}

fn error_response(category: wire::DbErrorCategory) -> (StatusCode, Json<wire::DbErrorResponse>) {
    let (status, message) = match category {
        wire::DbErrorCategory::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "database owner unavailable",
        ),
        wire::DbErrorCategory::NotReady => {
            (StatusCode::SERVICE_UNAVAILABLE, "database owner not ready")
        }
        wire::DbErrorCategory::NotFound => {
            (StatusCode::NOT_FOUND, "requested database object not found")
        }
        wire::DbErrorCategory::Conflict => (StatusCode::CONFLICT, "database operation conflict"),
        wire::DbErrorCategory::Transient => (
            StatusCode::SERVICE_UNAVAILABLE,
            "transient database failure",
        ),
        wire::DbErrorCategory::Invalid => (StatusCode::BAD_REQUEST, "invalid database request"),
        wire::DbErrorCategory::Internal => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal database failure",
        ),
    };
    (
        status,
        Json(wire::DbErrorResponse {
            category,
            message: message.to_owned(),
        }),
    )
}

macro_rules! dispatch_owner {
    ($state:expr, $body:expr, $ty:ty, $variant:path, $method:ident) => {{
        let request: $ty = match parse($body) {
            Ok(value) => value,
            Err(response) => return *response,
        };
        let db_owner = match owner($state, &request.agent).await {
            Ok(value) => value,
            Err(response) => return response,
        };
        success(db_owner.$method($variant(request)).await)
    }};
}

async fn dispatch(
    State(state): State<InternalState>,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    let path = uri.path();
    match path {
        wire::ROUTE_ARCHIVE_MESSAGE => dispatch_owner!(
            &state,
            &body,
            wire::ArchiveMessageRequest,
            InteractionStateOp::ArchiveMessage,
            interaction_state
        ),
        wire::ROUTE_MARK_MESSAGE_ROUTED => dispatch_owner!(
            &state,
            &body,
            wire::MarkMessageRoutedRequest,
            InteractionStateOp::MarkMessageRouted,
            interaction_state
        ),
        wire::ROUTE_GET_ACTIVE_SESSION => dispatch_owner!(
            &state,
            &body,
            wire::GetActiveSessionRequest,
            InteractionStateOp::GetActiveSession,
            interaction_state
        ),
        wire::ROUTE_CREATE_SESSION => dispatch_owner!(
            &state,
            &body,
            wire::CreateSessionRequest,
            InteractionStateOp::CreateSession,
            interaction_state
        ),
        wire::ROUTE_DEACTIVATE_CURRENT_SESSION => dispatch_owner!(
            &state,
            &body,
            wire::DeactivateCurrentSessionRequest,
            InteractionStateOp::DeactivateCurrentSession,
            interaction_state
        ),
        wire::ROUTE_ACTIVATE_SESSION => dispatch_owner!(
            &state,
            &body,
            wire::ActivateSessionRequest,
            InteractionStateOp::ActivateSession,
            interaction_state
        ),
        wire::ROUTE_TOUCH_SESSION => dispatch_owner!(
            &state,
            &body,
            wire::TouchSessionRequest,
            InteractionStateOp::TouchSession,
            interaction_state
        ),
        wire::ROUTE_LIST_SESSIONS => dispatch_owner!(
            &state,
            &body,
            wire::ListSessionsRequest,
            InteractionStateOp::ListSessions,
            interaction_state
        ),
        wire::ROUTE_FIND_SESSIONS_BY_UUID => dispatch_owner!(
            &state,
            &body,
            wire::FindSessionsByUuidRequest,
            InteractionStateOp::FindSessionsByUuid,
            interaction_state
        ),
        wire::ROUTE_FIND_SESSION_BY_ROOT => dispatch_owner!(
            &state,
            &body,
            wire::FindSessionByRootRequest,
            InteractionStateOp::FindSessionByRoot,
            interaction_state
        ),
        wire::ROUTE_LATEST_ASSISTANT_IS_UNIQUE_EXACT => dispatch_owner!(
            &state,
            &body,
            wire::LatestAssistantIsUniqueExactRequest,
            InteractionStateOp::LatestAssistantIsUniqueExact,
            interaction_state
        ),
        wire::ROUTE_IS_RECENT_ROUTED_TARGET => dispatch_owner!(
            &state,
            &body,
            wire::IsRecentRoutedTargetRequest,
            InteractionStateOp::IsRecentRoutedTarget,
            interaction_state
        ),
        wire::ROUTE_FETCH_MESSAGES_BY_IDS => dispatch_owner!(
            &state,
            &body,
            wire::FetchMessagesByIdsRequest,
            InteractionStateOp::FetchMessagesByIds,
            interaction_state
        ),
        wire::ROUTE_CONVERSATION_LATEST_TURN_ID => dispatch_owner!(
            &state,
            &body,
            wire::ConversationLatestTurnIdRequest,
            InteractionStateOp::ConversationLatestTurnId,
            interaction_state
        ),
        wire::ROUTE_THREAD_FOCUS_GET => dispatch_owner!(
            &state,
            &body,
            wire::ThreadFocusGetRequest,
            InteractionStateOp::ThreadFocusGet,
            interaction_state
        ),
        wire::ROUTE_THREAD_FOCUS_SET_OPERATOR => dispatch_owner!(
            &state,
            &body,
            wire::ThreadFocusSetOperatorRequest,
            InteractionStateOp::ThreadFocusSetOperator,
            interaction_state
        ),
        wire::ROUTE_ERROR_DETAIL_INSERT => dispatch_owner!(
            &state,
            &body,
            wire::ErrorDetailInsertRequest,
            InteractionStateOp::ErrorDetailInsert,
            interaction_state
        ),
        wire::ROUTE_ERROR_DETAIL_GET => dispatch_owner!(
            &state,
            &body,
            wire::ErrorDetailGetRequest,
            InteractionStateOp::ErrorDetailGet,
            interaction_state
        ),
        wire::ROUTE_LIFECYCLE_BUMP_USE_MANY => dispatch_owner!(
            &state,
            &body,
            wire::LifecycleBumpUseManyRequest,
            InteractionStateOp::LifecycleBumpUseMany,
            interaction_state
        ),
        wire::ROUTE_BOOTSTRAP_OWNER => dispatch_owner!(
            &state,
            &body,
            wire::BootstrapOwnerRequest,
            InteractionStateOp::BootstrapOwner,
            interaction_state
        ),
        wire::ROUTE_BOOTSTRAP_CLAIM_OWNER => dispatch_owner!(
            &state,
            &body,
            wire::BootstrapClaimOwnerRequest,
            InteractionStateOp::BootstrapClaimOwner,
            interaction_state
        ),
        wire::ROUTE_BOOTSTRAP_MISSING_STAGES => dispatch_owner!(
            &state,
            &body,
            wire::BootstrapStageScopeRequest,
            InteractionStateOp::BootstrapMissingStages,
            interaction_state
        ),
        wire::ROUTE_BOOTSTRAP_FIRST_MISSING_STAGE => dispatch_owner!(
            &state,
            &body,
            wire::BootstrapStageScopeRequest,
            InteractionStateOp::BootstrapFirstMissingStage,
            interaction_state
        ),
        wire::ROUTE_BOOTSTRAP_ISSUED_QUESTION_STAGE => dispatch_owner!(
            &state,
            &body,
            wire::BootstrapStageScopeRequest,
            InteractionStateOp::BootstrapIssuedQuestionStage,
            interaction_state
        ),
        wire::ROUTE_BOOTSTRAP_RECORD_QUESTION_ISSUE => dispatch_owner!(
            &state,
            &body,
            wire::BootstrapRecordQuestionIssueRequest,
            InteractionStateOp::BootstrapRecordQuestionIssue,
            interaction_state
        ),
        wire::ROUTE_BOOTSTRAP_RECORD_CURRENT_ANSWER => dispatch_owner!(
            &state,
            &body,
            wire::BootstrapRecordCurrentAnswerRequest,
            InteractionStateOp::BootstrapRecordCurrentAnswer,
            interaction_state
        ),
        wire::ROUTE_BOOTSTRAP_RECORD_ANSWER => dispatch_owner!(
            &state,
            &body,
            wire::BootstrapRecordAnswerRequest,
            InteractionStateOp::BootstrapRecordAnswer,
            interaction_state
        ),
        wire::ROUTE_BOOTSTRAP_RECORDED_ANSWERS => dispatch_owner!(
            &state,
            &body,
            wire::BootstrapStageScopeRequest,
            InteractionStateOp::BootstrapRecordedAnswers,
            interaction_state
        ),
        wire::ROUTE_BOOTSTRAP_CLEAR => dispatch_owner!(
            &state,
            &body,
            wire::BootstrapClearRequest,
            InteractionStateOp::BootstrapClear,
            interaction_state
        ),
        wire::ROUTE_CRON_SPECS_LIST => dispatch_owner!(
            &state,
            &body,
            wire::CronSpecsListRequest,
            RunLedgerOp::CronSpecsList,
            run_ledger
        ),
        wire::ROUTE_CRON_SPEC_DETAIL => dispatch_owner!(
            &state,
            &body,
            wire::CronSpecDetailRequest,
            RunLedgerOp::CronSpecDetail,
            run_ledger
        ),
        wire::ROUTE_CRON_RECENT_RUNS => dispatch_owner!(
            &state,
            &body,
            wire::CronRecentRunsRequest,
            RunLedgerOp::CronRecentRuns,
            run_ledger
        ),
        wire::ROUTE_CRON_DELETE_SPEC => dispatch_owner!(
            &state,
            &body,
            wire::CronDeleteSpecRequest,
            RunLedgerOp::CronDeleteSpec,
            run_ledger
        ),
        wire::ROUTE_CRON_CLEAR_TRIGGERED => dispatch_owner!(
            &state,
            &body,
            wire::CronJobRequest,
            RunLedgerOp::CronClearTriggered,
            run_ledger
        ),
        wire::ROUTE_ENQUEUE_BACKGROUND_RUN => dispatch_owner!(
            &state,
            &body,
            wire::EnqueueBackgroundRunRequest,
            RunLedgerOp::EnqueueBackgroundRun,
            run_ledger
        ),
        wire::ROUTE_CRON_INSERT_RUNNING_RUN => dispatch_owner!(
            &state,
            &body,
            wire::CronInsertRunningRunRequest,
            RunLedgerOp::CronInsertRunningRun,
            run_ledger
        ),
        wire::ROUTE_MARK_BACKGROUND_SPAWNED => dispatch_owner!(
            &state,
            &body,
            wire::MarkBackgroundSpawnedRequest,
            RunLedgerOp::MarkBackgroundSpawned,
            run_ledger
        ),
        wire::ROUTE_PERSIST_RUN_OUTPUT => dispatch_owner!(
            &state,
            &body,
            wire::PersistRunOutputRequest,
            RunLedgerOp::PersistRunOutput,
            run_ledger
        ),
        wire::ROUTE_FINISH_RUN => dispatch_owner!(
            &state,
            &body,
            wire::FinishRunRequest,
            RunLedgerOp::FinishRun,
            run_ledger
        ),
        wire::ROUTE_MARK_HANDOFF_FAILED => dispatch_owner!(
            &state,
            &body,
            wire::MarkHandoffFailedRequest,
            RunLedgerOp::MarkHandoffFailed,
            run_ledger
        ),
        wire::ROUTE_RECOVER_INTERRUPTED_HANDOFFS => dispatch_owner!(
            &state,
            &body,
            wire::RecoverInterruptedHandoffsRequest,
            RunLedgerOp::RecoverInterruptedHandoffs,
            run_ledger
        ),
        wire::ROUTE_CRON_MARK_INTERRUPTED_BY_SHUTDOWN => dispatch_owner!(
            &state,
            &body,
            wire::CronMarkInterruptedByShutdownRequest,
            RunLedgerOp::CronMarkInterruptedByShutdown,
            run_ledger
        ),
        wire::ROUTE_DELIVERY_FETCH_PENDING => dispatch_owner!(
            &state,
            &body,
            wire::DeliveryFetchPendingRequest,
            RunLedgerOp::DeliveryFetchPending,
            run_ledger
        ),
        wire::ROUTE_DELIVERY_MARK_OUTCOME => dispatch_owner!(
            &state,
            &body,
            wire::DeliveryMarkOutcomeRequest,
            RunLedgerOp::DeliveryMarkOutcome,
            run_ledger
        ),
        wire::ROUTE_DELIVERY_DEDUPLICATE_JOB => dispatch_owner!(
            &state,
            &body,
            wire::DeliveryDeduplicateJobRequest,
            RunLedgerOp::DeliveryDeduplicateJob,
            run_ledger
        ),
        wire::ROUTE_AUTH_STATUS => dispatch_owner!(
            &state,
            &body,
            wire::AuthStatusRequest,
            SecretsRegistryOp::AuthStatus,
            secrets_registry
        ),
        wire::ROUTE_AUTH_TOKEN_GET => dispatch_owner!(
            &state,
            &body,
            wire::AuthTokenGetRequest,
            SecretsRegistryOp::AuthTokenGet,
            secrets_registry
        ),
        wire::ROUTE_AUTH_TOKEN_SAVE => dispatch_owner!(
            &state,
            &body,
            wire::AuthTokenSaveRequest,
            SecretsRegistryOp::AuthTokenSave,
            secrets_registry
        ),
        wire::ROUTE_AUTH_TOKEN_DELETE => dispatch_owner!(
            &state,
            &body,
            wire::AuthTokenDeleteRequest,
            SecretsRegistryOp::AuthTokenDelete,
            secrets_registry
        ),
        wire::ROUTE_NOTICE_TOKEN_GET_OR_CREATE => dispatch_owner!(
            &state,
            &body,
            wire::NoticeTokenGetOrCreateRequest,
            SecretsRegistryOp::NoticeTokenGetOrCreate,
            secrets_registry
        ),
        wire::ROUTE_MCP_OAUTH_STATE_SET => dispatch_owner!(
            &state,
            &body,
            wire::McpOauthStateSetRequest,
            SecretsRegistryOp::McpOauthStateSet,
            secrets_registry
        ),
        wire::ROUTE_USAGE_INSERT_EVENT => dispatch_owner!(
            &state,
            &body,
            wire::UsageInsertEventRequest,
            LearningMemoryOp::UsageInsertEvent,
            learning_memory
        ),
        wire::ROUTE_LEARNING_EVENT_INSERT => dispatch_owner!(
            &state,
            &body,
            wire::LearningEventInsertRequest,
            LearningMemoryOp::LearningEventInsert,
            learning_memory
        ),
        wire::ROUTE_LEARNING_TODAY_SPEND => dispatch_owner!(
            &state,
            &body,
            wire::LearningTodaySpendRequest,
            LearningMemoryOp::LearningTodaySpend,
            learning_memory
        ),
        wire::ROUTE_LEARNING_RECORD_BUDGET_SKIP => dispatch_owner!(
            &state,
            &body,
            wire::LearningRecordBudgetSkipRequest,
            LearningMemoryOp::LearningRecordBudgetSkip,
            learning_memory
        ),
        wire::ROUTE_LEARNING_AUTHORED_SKILL_THIS_TURN => dispatch_owner!(
            &state,
            &body,
            wire::LearningAuthoredSkillThisTurnRequest,
            LearningMemoryOp::LearningAuthoredSkillThisTurn,
            learning_memory
        ),
        wire::ROUTE_LEARNING_LINK_CRON_AUTHORED => dispatch_owner!(
            &state,
            &body,
            wire::LearningLinkCronAuthoredRequest,
            LearningMemoryOp::LearningLinkCronAuthored,
            learning_memory
        ),
        wire::ROUTE_LEARNING_LATEST_INTERACTIVE_CONTEXT_TOKENS => dispatch_owner!(
            &state,
            &body,
            wire::LearningLatestInteractiveContextTokensRequest,
            LearningMemoryOp::LearningLatestInteractiveContextTokens,
            learning_memory
        ),
        wire::ROUTE_LEARNING_TURN_BASELINES => dispatch_owner!(
            &state,
            &body,
            wire::LearningTurnBaselinesRequest,
            LearningMemoryOp::LearningTurnBaselines,
            learning_memory
        ),
        wire::ROUTE_LEARNING_PROBE_COST_SPIKE => dispatch_owner!(
            &state,
            &body,
            wire::LearningProbeCostSpikeRequest,
            LearningMemoryOp::LearningProbeCostSpike,
            learning_memory
        ),
        wire::ROUTE_CURATOR_LOAD_STATE => dispatch_owner!(
            &state,
            &body,
            wire::CuratorLoadStateRequest,
            LearningMemoryOp::CuratorLoadState,
            learning_memory
        ),
        wire::ROUTE_CURATOR_SAVE_STATE => dispatch_owner!(
            &state,
            &body,
            wire::CuratorSaveStateRequest,
            LearningMemoryOp::CuratorSaveState,
            learning_memory
        ),
        wire::ROUTE_CURATOR_INSERT_RUN => dispatch_owner!(
            &state,
            &body,
            wire::CuratorInsertRunRequest,
            LearningMemoryOp::CuratorInsertRun,
            learning_memory
        ),
        wire::ROUTE_CURATOR_LATEST_CHAT_ACTIVITY => dispatch_owner!(
            &state,
            &body,
            wire::CuratorLatestChatActivityRequest,
            LearningMemoryOp::CuratorLatestChatActivity,
            learning_memory
        ),
        wire::ROUTE_CURATOR_CHANGE_COUNT => dispatch_owner!(
            &state,
            &body,
            wire::CuratorChangeCountRequest,
            LearningMemoryOp::CuratorChangeCount,
            learning_memory
        ),
        wire::ROUTE_CURATOR_ARCHIVED_SNAPSHOT => dispatch_owner!(
            &state,
            &body,
            wire::CuratorArchivedSnapshotRequest,
            LearningMemoryOp::CuratorArchivedSnapshot,
            learning_memory
        ),
        wire::ROUTE_CURATOR_APPLY_TRANSITIONS => dispatch_owner!(
            &state,
            &body,
            wire::CuratorApplyTransitionsRequest,
            LearningMemoryOp::CuratorApplyTransitions,
            learning_memory
        ),
        wire::ROUTE_CURATOR_FINALIZE => dispatch_owner!(
            &state,
            &body,
            wire::CuratorFinalizeRequest,
            LearningMemoryOp::CuratorFinalize,
            learning_memory
        ),
        wire::ROUTE_LIFECYCLE_ARCHIVED_SINCE => dispatch_owner!(
            &state,
            &body,
            wire::LifecycleArchivedSinceRequest,
            LearningMemoryOp::LifecycleArchivedSince,
            learning_memory
        ),
        wire::ROUTE_SKILL_LIFECYCLE_GET => dispatch_owner!(
            &state,
            &body,
            wire::SkillLifecycleGetRequest,
            LearningMemoryOp::SkillLifecycleGet,
            learning_memory
        ),
        wire::ROUTE_SKILL_LIFECYCLE_LIST => dispatch_owner!(
            &state,
            &body,
            wire::SkillLifecycleListRequest,
            LearningMemoryOp::SkillLifecycleList,
            learning_memory
        ),
        wire::ROUTE_SKILL_PIN => dispatch_owner!(
            &state,
            &body,
            wire::SkillPinRequest,
            LearningMemoryOp::SkillPin,
            learning_memory
        ),
        wire::ROUTE_SKILL_SPEND_RECORD => dispatch_owner!(
            &state,
            &body,
            wire::SkillSpendRecordRequest,
            LearningMemoryOp::SkillSpendRecord,
            learning_memory
        ),
        wire::ROUTE_SKILL_SPEND_BY_SKILL => dispatch_owner!(
            &state,
            &body,
            wire::SkillSpendBySkillRequest,
            LearningMemoryOp::SkillSpendBySkill,
            learning_memory
        ),
        wire::ROUTE_ALERT_CHECK_AND_RECORD => dispatch_owner!(
            &state,
            &body,
            wire::AlertCheckAndRecordRequest,
            LearningMemoryOp::AlertCheckAndRecord,
            learning_memory
        ),
        wire::ROUTE_ALERT_RECORD => dispatch_owner!(
            &state,
            &body,
            wire::AlertRecordRequest,
            LearningMemoryOp::AlertRecord,
            learning_memory
        ),
        wire::ROUTE_ALERT_CLEAR => dispatch_owner!(
            &state,
            &body,
            wire::AlertClearRequest,
            LearningMemoryOp::AlertClear,
            learning_memory
        ),
        wire::ROUTE_RETAIN_ENQUEUE => dispatch_owner!(
            &state,
            &body,
            wire::RetainEnqueueRequest,
            LearningMemoryOp::RetainEnqueue,
            learning_memory
        ),
        wire::ROUTE_RETAIN_CLAIM_BATCH => dispatch_owner!(
            &state,
            &body,
            wire::RetainClaimBatchRequest,
            LearningMemoryOp::RetainClaimBatch,
            learning_memory
        ),
        wire::ROUTE_RETAIN_ACK => dispatch_owner!(
            &state,
            &body,
            wire::RetainAckRequest,
            LearningMemoryOp::RetainAck,
            learning_memory
        ),
        wire::ROUTE_RETAIN_NACK => dispatch_owner!(
            &state,
            &body,
            wire::RetainNackRequest,
            LearningMemoryOp::RetainNack,
            learning_memory
        ),
        wire::ROUTE_RETAIN_QUEUE_STATS => dispatch_owner!(
            &state,
            &body,
            wire::RetainQueueStatsRequest,
            LearningMemoryOp::RetainQueueStats,
            learning_memory
        ),
        wire::ROUTE_DASHBOARD_ACTIVITY => dispatch_owner!(
            &state,
            &body,
            wire::DashboardActivityRequest,
            LearningMemoryOp::DashboardActivity,
            learning_memory
        ),
        wire::ROUTE_DASHBOARD_RUN_DETAIL => dispatch_owner!(
            &state,
            &body,
            wire::DashboardRunDetailRequest,
            LearningMemoryOp::DashboardRunDetail,
            learning_memory
        ),
        wire::ROUTE_DASHBOARD_OVERVIEW => dispatch_owner!(
            &state,
            &body,
            wire::DashboardOverviewRequest,
            LearningMemoryOp::DashboardOverview,
            learning_memory
        ),
        wire::ROUTE_DASHBOARD_USAGE => dispatch_owner!(
            &state,
            &body,
            wire::DashboardUsageRequest,
            LearningMemoryOp::DashboardUsage,
            learning_memory
        ),
        wire::ROUTE_DASHBOARD_LEARNING => dispatch_owner!(
            &state,
            &body,
            wire::DashboardLearningRequest,
            LearningMemoryOp::DashboardLearning,
            learning_memory
        ),
        wire::ROUTE_DASHBOARD_SKILL_LIFECYCLE => dispatch_owner!(
            &state,
            &body,
            wire::DashboardSkillLifecycleRequest,
            LearningMemoryOp::DashboardSkillLifecycle,
            learning_memory
        ),
        wire::ROUTE_DASHBOARD_SKILL_SPEND => dispatch_owner!(
            &state,
            &body,
            wire::DashboardSkillSpendRequest,
            LearningMemoryOp::DashboardSkillSpend,
            learning_memory
        ),
        wire::ROUTE_PROVIDER_BINDINGS_RESOLVE => resolve_provider_bindings(&state, &body).await,
        wire::ROUTE_PROVIDER_BINDINGS_RESOLVE_NAMED => {
            resolve_named_provider_binding(&state, &body).await
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn verify_provider_auth(
    state: &InternalState,
    agent: &str,
    supplied: &str,
) -> Result<(), Response> {
    // Requiring an owner makes the authenticated identity equal the requested
    // live agent; a token for any other agent cannot authorize this scope.
    owner(state, agent).await?;
    let config = right_agent::agent::discovery::parse_agent_config(&state.agents_dir.join(agent))
        .map_err(|error| {
            tracing::error!(agent, error = %format!("{error:#}"), "provider IPC agent config load failed");
            error_response(wire::DbErrorCategory::Internal).into_response()
        })?
        .ok_or_else(|| error_response(wire::DbErrorCategory::NotFound).into_response())?;
    let secret = config.secret.ok_or_else(|| {
        tracing::error!(agent, "provider IPC agent secret missing");
        error_response(wire::DbErrorCategory::Internal).into_response()
    })?;
    let expected = wire::provider_binding_token(&secret).map_err(|error| {
        tracing::error!(agent, error = %format!("{error:#}"), "provider IPC token derivation failed");
        error_response(wire::DbErrorCategory::Internal).into_response()
    })?;
    let matches = expected.len() == supplied.len()
        && bool::from(expected.as_bytes().ct_eq(supplied.as_bytes()));
    if !matches {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(wire::DbErrorResponse {
                category: wire::DbErrorCategory::Invalid,
                message: "provider binding authentication failed".to_owned(),
            }),
        )
            .into_response());
    }
    Ok(())
}

fn binding_dto(provider: String, binding: right_sandbox::SecretBinding) -> wire::SecretBindingDto {
    let (env_var, source_env_var, placeholder, allowed_hosts, inject_query, value) =
        binding.into_transport_parts();
    wire::SecretBindingDto {
        provider,
        env_var,
        source_env_var,
        placeholder,
        allowed_hosts,
        inject_query,
        value,
    }
}

async fn resolve_provider_bindings(state: &InternalState, body: &[u8]) -> Response {
    if body.len() > wire::PROVIDER_BINDING_MAX_REQUEST_BYTES {
        return error_response(wire::DbErrorCategory::Invalid).into_response();
    }
    let request: wire::ResolveProviderBindingsRequest = match parse(body) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    if let Err(response) =
        verify_provider_auth(state, &request.agent, request.auth.expose_secret()).await
    {
        return response;
    }
    let records = match state.providers.list(&request.agent).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(agent = %request.agent, error = %format!("{error:#}"), "provider binding list failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(wire::DbErrorResponse {
                    category: wire::DbErrorCategory::Internal,
                    message: PROVIDER_ERROR_MESSAGE.to_owned(),
                }),
            )
                .into_response();
        }
    };
    let mut bindings = Vec::new();
    for record in records {
        match record.status {
            right_providers::ProviderStatus::NeedsValue => continue,
            right_providers::ProviderStatus::Error => {
                tracing::error!(agent = %request.agent, provider = %record.name, "provider binding record is not resolvable");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(wire::DbErrorResponse {
                        category: wire::DbErrorCategory::Internal,
                        message: PROVIDER_ERROR_MESSAGE.to_owned(),
                    }),
                )
                    .into_response();
            }
            right_providers::ProviderStatus::Ready => {}
        }
        match state
            .providers
            .source_ref_binding(&request.agent, &record.name)
            .await
        {
            Ok(binding) => bindings.push(binding_dto(record.name, binding)),
            Err(error) => {
                tracing::error!(agent = %request.agent, provider = %record.name, error = %format!("{error:#}"), "provider binding resolution failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(wire::DbErrorResponse {
                        category: wire::DbErrorCategory::Internal,
                        message: PROVIDER_ERROR_MESSAGE.to_owned(),
                    }),
                )
                    .into_response();
            }
        }
    }
    Json(wire::ResolveProviderBindingsResponse { bindings }).into_response()
}

async fn resolve_named_provider_binding(state: &InternalState, body: &[u8]) -> Response {
    if body.len() > wire::PROVIDER_BINDING_MAX_REQUEST_BYTES {
        return error_response(wire::DbErrorCategory::Invalid).into_response();
    }
    let request: wire::ResolveNamedProviderBindingRequest = match parse(body) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    if let Err(response) =
        verify_provider_auth(state, &request.agent, request.auth.expose_secret()).await
    {
        return response;
    }
    match state
        .providers
        .source_ref_binding(&request.agent, &request.provider)
        .await
    {
        Ok(binding) => Json(wire::ResolveNamedProviderBindingResponse {
            binding: binding_dto(request.provider, binding),
        })
        .into_response(),
        Err(error) => {
            tracing::error!(agent = %request.agent, provider = %request.provider, error = %format!("{error:#}"), "named provider binding resolution failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(wire::DbErrorResponse {
                    category: wire::DbErrorCategory::Internal,
                    message: PROVIDER_ERROR_MESSAGE.to_owned(),
                }),
            )
                .into_response()
        }
    }
}
