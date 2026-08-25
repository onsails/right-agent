//! Typed domain IPC between bot processes and the Aggregator-owned database.
//!
//! Single-owner invariant: the Aggregator (`right mcp-server`) is the only
//! live process holding a per-agent `data.db` connection. Bots never open the
//! database directly at runtime; they call the typed methods below, which POST
//! JSON to finite routes on `internal.sock` (0600). The server side lives in
//! `crates/right/src/internal_api_db.rs`.
//!
//! Contract rules enforced here:
//!
//! - No SQL, table names, raw rows, arbitrary parameters, or generic key/value
//!   operations cross the wire. Every request/response is a named struct with
//!   typed fields (verified by tests in `internal_db_tests.rs`).
//! - Every retryable mutation carries a `request_id`; the owner records it and
//!   treats a replay as a no-op success.
//! - Failures map to typed [`DbErrorCategory`] values; secret material never
//!   appears in error bodies, `Debug` output, or logs.
//! - Secret-bearing DTOs serialize the inner value only at the UDS body
//!   boundary (`serialize_with` + `ExposeSecret`), redact `Debug`, and zeroize
//!   on drop through [`SecretString`].

use std::collections::HashMap;

use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};

use crate::internal_client::{
    ErrorBodyMode, InternalClient, InternalClientError, RedactedServerCategory,
};

// ---------------------------------------------------------------------------
// Routes (single source of truth shared with the server)
// ---------------------------------------------------------------------------

pub const ROUTE_ARCHIVE_MESSAGE: &str = "/db/interaction/archive-message";
pub const ROUTE_MARK_MESSAGE_ROUTED: &str = "/db/interaction/mark-message-routed";
pub const ROUTE_GET_ACTIVE_SESSION: &str = "/db/interaction/get-active-session";
pub const ROUTE_CREATE_SESSION: &str = "/db/interaction/create-session";
pub const ROUTE_DEACTIVATE_CURRENT_SESSION: &str = "/db/interaction/deactivate-current-session";
pub const ROUTE_ACTIVATE_SESSION: &str = "/db/interaction/activate-session";
pub const ROUTE_TOUCH_SESSION: &str = "/db/interaction/touch-session";
pub const ROUTE_LIST_SESSIONS: &str = "/db/interaction/list-sessions";
pub const ROUTE_FIND_SESSIONS_BY_UUID: &str = "/db/interaction/find-sessions-by-uuid";
pub const ROUTE_FIND_SESSION_BY_ROOT: &str = "/db/interaction/find-session-by-root";
pub const ROUTE_LATEST_ASSISTANT_IS_UNIQUE_EXACT: &str =
    "/db/interaction/latest-assistant-is-unique-exact";
pub const ROUTE_IS_RECENT_ROUTED_TARGET: &str = "/db/interaction/is-recent-routed-target";
pub const ROUTE_FETCH_MESSAGES_BY_IDS: &str = "/db/interaction/fetch-messages-by-ids";
pub const ROUTE_CONVERSATION_LATEST_TURN_ID: &str = "/db/interaction/conversation-latest-turn-id";
pub const ROUTE_THREAD_FOCUS_GET: &str = "/db/interaction/thread-focus-get";
pub const ROUTE_THREAD_FOCUS_SET_OPERATOR: &str = "/db/interaction/thread-focus-set-operator";
pub const ROUTE_ERROR_DETAIL_INSERT: &str = "/db/interaction/error-detail-insert";
pub const ROUTE_ERROR_DETAIL_GET: &str = "/db/interaction/error-detail-get";
pub const ROUTE_LIFECYCLE_BUMP_USE_MANY: &str = "/db/interaction/lifecycle-bump-use-many";
pub const ROUTE_BOOTSTRAP_OWNER: &str = "/db/interaction/bootstrap-owner";
pub const ROUTE_BOOTSTRAP_CLAIM_OWNER: &str = "/db/interaction/bootstrap-claim-owner";
pub const ROUTE_BOOTSTRAP_MISSING_STAGES: &str = "/db/interaction/bootstrap-missing-stages";
pub const ROUTE_BOOTSTRAP_FIRST_MISSING_STAGE: &str =
    "/db/interaction/bootstrap-first-missing-stage";
pub const ROUTE_BOOTSTRAP_ISSUED_QUESTION_STAGE: &str =
    "/db/interaction/bootstrap-issued-question-stage";
pub const ROUTE_BOOTSTRAP_RECORD_QUESTION_ISSUE: &str =
    "/db/interaction/bootstrap-record-question-issue";
pub const ROUTE_BOOTSTRAP_RECORD_CURRENT_ANSWER: &str =
    "/db/interaction/bootstrap-record-current-answer";
pub const ROUTE_BOOTSTRAP_RECORD_ANSWER: &str = "/db/interaction/bootstrap-record-answer";
pub const ROUTE_BOOTSTRAP_RECORDED_ANSWERS: &str = "/db/interaction/bootstrap-recorded-answers";
pub const ROUTE_BOOTSTRAP_CLEAR: &str = "/db/interaction/bootstrap-clear";

pub const ROUTE_CRON_SPECS_LIST: &str = "/db/runs/cron-specs-list";
pub const ROUTE_CRON_SPEC_DETAIL: &str = "/db/runs/cron-spec-detail";
pub const ROUTE_CRON_RECENT_RUNS: &str = "/db/runs/cron-recent-runs";
pub const ROUTE_CRON_DELETE_SPEC: &str = "/db/runs/cron-delete-spec";
pub const ROUTE_CRON_CLEAR_TRIGGERED: &str = "/db/runs/cron-clear-triggered";
pub const ROUTE_ENQUEUE_BACKGROUND_RUN: &str = "/db/runs/enqueue-background-run";
pub const ROUTE_CRON_INSERT_RUNNING_RUN: &str = "/db/runs/cron-insert-running-run";
pub const ROUTE_MARK_BACKGROUND_SPAWNED: &str = "/db/runs/mark-background-spawned";
pub const ROUTE_PERSIST_RUN_OUTPUT: &str = "/db/runs/persist-run-output";
pub const ROUTE_FINISH_RUN: &str = "/db/runs/finish-run";
pub const ROUTE_MARK_HANDOFF_FAILED: &str = "/db/runs/mark-handoff-failed";
pub const ROUTE_RECOVER_INTERRUPTED_HANDOFFS: &str = "/db/runs/recover-interrupted-handoffs";
pub const ROUTE_CRON_MARK_INTERRUPTED_BY_SHUTDOWN: &str =
    "/db/runs/cron-mark-interrupted-by-shutdown";
pub const ROUTE_DELIVERY_FETCH_PENDING: &str = "/db/runs/delivery-fetch-pending";
pub const ROUTE_DELIVERY_MARK_OUTCOME: &str = "/db/runs/delivery-mark-outcome";
pub const ROUTE_DELIVERY_DEDUPLICATE_JOB: &str = "/db/runs/delivery-deduplicate-job";

pub const ROUTE_AUTH_STATUS: &str = "/db/secrets/auth-status";
pub const ROUTE_AUTH_TOKEN_GET: &str = "/db/secrets/auth-token-get";
pub const ROUTE_AUTH_TOKEN_SAVE: &str = "/db/secrets/auth-token-save";
pub const ROUTE_AUTH_TOKEN_DELETE: &str = "/db/secrets/auth-token-delete";
pub const ROUTE_NOTICE_TOKEN_GET_OR_CREATE: &str = "/db/secrets/notice-token-get-or-create";
pub const ROUTE_MCP_OAUTH_STATE_SET: &str = "/db/secrets/mcp-oauth-state-set";

pub const ROUTE_USAGE_INSERT_EVENT: &str = "/db/learning/usage-insert-event";
pub const ROUTE_LEARNING_EVENT_INSERT: &str = "/db/learning/learning-event-insert";
pub const ROUTE_LEARNING_TODAY_SPEND: &str = "/db/learning/learning-today-spend";
pub const ROUTE_LEARNING_RECORD_BUDGET_SKIP: &str = "/db/learning/learning-record-budget-skip";
pub const ROUTE_LEARNING_AUTHORED_SKILL_THIS_TURN: &str =
    "/db/learning/learning-authored-skill-this-turn";
pub const ROUTE_LEARNING_LINK_CRON_AUTHORED: &str = "/db/learning/learning-link-cron-authored";
pub const ROUTE_LEARNING_LATEST_INTERACTIVE_CONTEXT_TOKENS: &str =
    "/db/learning/learning-latest-interactive-context-tokens";
pub const ROUTE_LEARNING_TURN_BASELINES: &str = "/db/learning/learning-turn-baselines";
pub const ROUTE_LEARNING_PROBE_COST_SPIKE: &str = "/db/learning/learning-probe-cost-spike";
pub const ROUTE_CURATOR_LOAD_STATE: &str = "/db/learning/curator-load-state";
pub const ROUTE_CURATOR_SAVE_STATE: &str = "/db/learning/curator-save-state";
pub const ROUTE_CURATOR_INSERT_RUN: &str = "/db/learning/curator-insert-run";
pub const ROUTE_CURATOR_LATEST_CHAT_ACTIVITY: &str = "/db/learning/curator-latest-chat-activity";
pub const ROUTE_CURATOR_CHANGE_COUNT: &str = "/db/learning/curator-change-count";
pub const ROUTE_CURATOR_ARCHIVED_SNAPSHOT: &str = "/db/learning/curator-archived-snapshot";
pub const ROUTE_CURATOR_APPLY_TRANSITIONS: &str = "/db/learning/curator-apply-transitions";
pub const ROUTE_CURATOR_FINALIZE: &str = "/db/learning/curator-finalize";
pub const ROUTE_LIFECYCLE_ARCHIVED_SINCE: &str = "/db/learning/lifecycle-archived-since";
pub const ROUTE_SKILL_LIFECYCLE_GET: &str = "/db/learning/skill-lifecycle-get";
pub const ROUTE_SKILL_LIFECYCLE_LIST: &str = "/db/learning/skill-lifecycle-list";
pub const ROUTE_SKILL_PIN: &str = "/db/learning/skill-pin";
pub const ROUTE_SKILL_SPEND_RECORD: &str = "/db/learning/skill-spend-record";
pub const ROUTE_SKILL_SPEND_BY_SKILL: &str = "/db/learning/skill-spend-by-skill";
pub const ROUTE_ALERT_CHECK_AND_RECORD: &str = "/db/learning/alert-check-and-record";
pub const ROUTE_ALERT_RECORD: &str = "/db/learning/alert-record";
pub const ROUTE_ALERT_CLEAR: &str = "/db/learning/alert-clear";
pub const ROUTE_RETAIN_ENQUEUE: &str = "/db/learning/retain-enqueue";
pub const ROUTE_RETAIN_CLAIM_BATCH: &str = "/db/learning/retain-claim-batch";
pub const ROUTE_RETAIN_ACK: &str = "/db/learning/retain-ack";
pub const ROUTE_RETAIN_NACK: &str = "/db/learning/retain-nack";
pub const ROUTE_RETAIN_QUEUE_STATS: &str = "/db/learning/retain-queue-stats";

pub const ROUTE_DASHBOARD_ACTIVITY: &str = "/db/dashboard/activity";
pub const ROUTE_DASHBOARD_RUN_DETAIL: &str = "/db/dashboard/run-detail";
pub const ROUTE_DASHBOARD_OVERVIEW: &str = "/db/dashboard/overview";
pub const ROUTE_DASHBOARD_USAGE: &str = "/db/dashboard/usage";
pub const ROUTE_DASHBOARD_LEARNING: &str = "/db/dashboard/learning";
pub const ROUTE_DASHBOARD_SKILL_LIFECYCLE: &str = "/db/dashboard/skill-lifecycle";
pub const ROUTE_DASHBOARD_SKILL_SPEND: &str = "/db/dashboard/skill-spend";

pub const ROUTE_PROVIDER_BINDINGS_RESOLVE: &str = "/db/provider-bindings/resolve";
pub const ROUTE_PROVIDER_BINDINGS_RESOLVE_NAMED: &str = "/db/provider-bindings/resolve-named";

// ---------------------------------------------------------------------------
// Error taxonomy
// ---------------------------------------------------------------------------

/// Typed failure categories for `/db/*` routes. The server classifies every
/// failure into exactly one of these; the client surfaces them typed so bot
/// code can distinguish "retry later" from "caller bug" from "gone for good".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DbErrorCategory {
    /// Owner exists but is draining/failed — do not retry against a
    /// direct-open fallback (there is none); surface the outage.
    Unavailable,
    /// Owner is still starting (migrations running). Retry within the
    /// caller's startup deadline.
    NotReady,
    /// Agent, run, or row does not exist.
    NotFound,
    /// Idempotency/lease/conflict violation (e.g. stale retain claim token).
    Conflict,
    /// Transient database contention. Safe to retry.
    Transient,
    /// Request failed validation (bad stage name, empty content, ...).
    Invalid,
    /// Everything else. The complete error chain is logged server-side; the
    /// client only ever sees a sanitized message.
    Internal,
}

/// Wire body for failed `/db/*` calls. `message` is operator-safe: the server
/// logs the full chain and returns only a non-secret summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DbErrorResponse {
    pub category: DbErrorCategory,
    pub message: String,
}

/// Error type returned by every typed method in this module.
#[derive(Debug, thiserror::Error)]
pub enum InternalDbError {
    /// Transport failure: socket unreachable, HTTP/JSON framing broken. The
    /// bot must treat this as "unknown outcome" — mutations carry
    /// `request_id` precisely so a retry here cannot duplicate.
    #[error("internal DB transport: {0}")]
    Transport(#[from] InternalClientError),

    /// The owner classified the failure.
    #[error("internal DB {category:?} (HTTP {status}): {message}")]
    Server {
        category: DbErrorCategory,
        status: u16,
        message: String,
    },
}

/// Truncate a server error body before it is embedded in an error value.
/// Secret-bearing routes (provider bindings) must never let a response body —
/// even an error one — flow into logs at full length.
pub(crate) const ERROR_BODY_MAX_CHARS: usize = 1024;

fn sanitize_error_body(body: &str) -> String {
    body.chars().take(ERROR_BODY_MAX_CHARS).collect()
}

/// Convert the generic transport error into the typed taxonomy, parsing the
/// server's [`DbErrorResponse`] body when present.
pub fn classify_transport_error(error: InternalClientError) -> InternalDbError {
    match error {
        InternalClientError::Server { status, body } => {
            match serde_json::from_str::<DbErrorResponse>(&body) {
                Ok(parsed) => InternalDbError::Server {
                    category: parsed.category,
                    status,
                    message: parsed.message,
                },
                // Fail closed: an unparseable error body is Internal, never
                // echoed back verbatim at full length.
                Err(_) => InternalDbError::Server {
                    category: DbErrorCategory::Internal,
                    status,
                    message: sanitize_error_body(&body),
                },
            }
        }
        InternalClientError::RedactedServer { status, category } => InternalDbError::Server {
            category: category
                .map(Into::into)
                .unwrap_or(DbErrorCategory::Internal),
            status,
            message: "secret-route request failed".to_owned(),
        },
        other => InternalDbError::Transport(other),
    }
}

impl From<RedactedServerCategory> for DbErrorCategory {
    fn from(category: RedactedServerCategory) -> Self {
        match category {
            RedactedServerCategory::Unavailable => Self::Unavailable,
            RedactedServerCategory::NotReady => Self::NotReady,
            RedactedServerCategory::NotFound => Self::NotFound,
            RedactedServerCategory::Conflict => Self::Conflict,
            RedactedServerCategory::Transient => Self::Transient,
            RedactedServerCategory::Invalid => Self::Invalid,
            RedactedServerCategory::Internal => Self::Internal,
        }
    }
}

// ---------------------------------------------------------------------------
// Secret serialization helpers
// ---------------------------------------------------------------------------

/// Serialize a secret by exposing it — the ONLY sanctioned exposure point,
/// used exclusively while encoding a UDS body. `Debug` and logs never reach
/// this.
fn serialize_secret_exposed<S: serde::Serializer>(
    value: &SecretString,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(value.expose_secret())
}

fn serialize_secret_opt<S: serde::Serializer>(
    value: &Option<SecretString>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(v) => serializer.serialize_some(v.expose_secret()),
        None => serializer.serialize_none(),
    }
}

fn deserialize_secret_opt<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SecretString>, D::Error> {
    let value: Option<String> = Option::deserialize(deserializer)?;
    Ok(value.map(SecretString::from))
}

// ---------------------------------------------------------------------------
// InteractionState DTOs
// ---------------------------------------------------------------------------

/// One conversation message for archival. Mirrors
/// `right_db::conversation::ConversationMessage` with owned fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationMessageDto {
    pub platform: String,
    pub chat_id: i64,
    pub thread_id: i64,
    pub message_id: Option<i32>,
    pub sender_user_id: Option<i64>,
    pub sender_name: Option<String>,
    pub addressed_to_bot: bool,
    pub routed_to_agent: bool,
    pub root_session_id: Option<String>,
    pub turn_id: Option<u64>,
    /// `user` | `assistant` (validated server-side).
    pub role: String,
    pub content: String,
}

/// One row of the sessions store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRowDto {
    pub id: i64,
    pub chat_id: i64,
    pub thread_id: i64,
    pub root_session_id: String,
    pub label: Option<String>,
    pub is_active: bool,
    pub created_at: String,
    pub last_used_at: String,
}

/// One archived message returned by id lookup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FetchedMessageDto {
    pub message_id: Option<i32>,
    pub sender_name: Option<String>,
    pub text: String,
    pub role: String,
}

/// Thread focus row mirror (`right_db::thread_focus::ThreadFocus`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadFocusDto {
    pub operator_focus: Option<String>,
    pub agent_focus: Option<String>,
    pub updated_at: String,
}

/// Bootstrap interview ownership scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapOwnerDto {
    pub chat_id: i64,
    pub thread_id: i64,
}

/// One recorded bootstrap answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedAnswerDto {
    pub stage: String,
    pub answer: String,
}

/// Result of `bootstrap_claim_owner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimOwnerOutcomeDto {
    pub claimed: bool,
    /// Present iff `claimed == false`: the current owner.
    pub owner: Option<BootstrapOwnerDto>,
}

/// Result of `bootstrap_record_question_issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RecordQuestionIssueOutcomeDto {
    Recorded,
    OutOfOrder { expected: Option<String> },
    NotOwner { owner: Option<BootstrapOwnerDto> },
}

/// Result of `bootstrap_record_current_answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RecordCurrentAnswerOutcomeDto {
    Recorded {
        stage: String,
        next_stage: Option<String>,
    },
    NotOwner {
        owner: Option<BootstrapOwnerDto>,
    },
    QuestionNotIssued {
        stage: String,
    },
    SourceMessageNotAfterQuestion {
        stage: String,
    },
}

/// Result of `bootstrap_record_answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RecordAnswerOutcomeDto {
    Recorded,
    OutOfOrder { expected: Option<String> },
    NotOwner { owner: Option<BootstrapOwnerDto> },
    QuestionNotIssued,
    SourceMessageAlreadyUsed,
}

macro_rules! agent_request {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        pub struct $name {
            pub agent: String,
            $(pub $field: $ty,)*
        }
    };
}

agent_request!(ArchiveMessageRequest {
    request_id: String,
    message: ConversationMessageDto
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchiveMessageResponse {
    pub id: i64,
    /// False when the natural key (platform, chat, message id, role) already
    /// existed — i.e. this call was an idempotent replay.
    pub inserted: bool,
}

agent_request!(MarkMessageRoutedRequest {
    platform: String,
    chat_id: i64,
    thread_id: i64,
    message_id: i32,
    root_session_id: String,
    turn_id: u64,
});

agent_request!(GetActiveSessionRequest {
    chat_id: i64,
    thread_id: i64
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetActiveSessionResponse {
    pub session: Option<SessionRowDto>,
}

agent_request!(CreateSessionRequest {
    chat_id: i64,
    thread_id: i64,
    session_uuid: String,
    label: Option<String>,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session_id: i64,
}

agent_request!(DeactivateCurrentSessionRequest {
    chat_id: i64,
    thread_id: i64
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeactivateCurrentSessionResponse {
    pub previous_root_session_id: Option<String>,
}

agent_request!(ActivateSessionRequest { session_id: i64 });
agent_request!(TouchSessionRequest { session_id: i64 });
agent_request!(ListSessionsRequest {
    chat_id: i64,
    thread_id: i64
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionRowDto>,
}

agent_request!(FindSessionsByUuidRequest {
    chat_id: i64,
    thread_id: i64,
    uuid_prefix: String,
});

agent_request!(FindSessionByRootRequest {
    chat_id: i64,
    thread_id: i64,
    root_session_id: String,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindSessionByRootResponse {
    pub session: Option<SessionRowDto>,
}

agent_request!(LatestAssistantIsUniqueExactRequest {
    root_session_id: String,
    target: String,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoolResultResponse {
    pub result: bool,
}

agent_request!(IsRecentRoutedTargetRequest {
    platform: String,
    chat_id: i64,
    thread_id: i64,
    message_id: i32,
    root_session_id: String,
    window_secs: i64,
    current_turn_id: i64,
});

agent_request!(FetchMessagesByIdsRequest {
    platform: String,
    chat_id: i64,
    thread_id: i64,
    message_ids: Vec<i32>,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FetchMessagesByIdsResponse {
    pub messages: Vec<FetchedMessageDto>,
}

agent_request!(ConversationLatestTurnIdRequest {
    root_session_id: String
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationLatestTurnIdResponse {
    pub turn_id: Option<u64>,
}

agent_request!(ThreadFocusGetRequest {
    chat_id: i64,
    thread_id: i64
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadFocusGetResponse {
    pub focus: Option<ThreadFocusDto>,
}

agent_request!(ThreadFocusSetOperatorRequest {
    chat_id: i64,
    thread_id: i64,
    value: Option<String>,
});

agent_request!(ErrorDetailInsertRequest {
    request_id: String,
    chat_id: i64,
    thread_id: i64,
    raw_json: String,
    created_at_unix: i64,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorDetailInsertResponse {
    pub id: i64,
}

agent_request!(ErrorDetailGetRequest {
    id: i64,
    chat_id: i64
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorDetailGetResponse {
    pub raw_json: Option<String>,
}

agent_request!(LifecycleBumpUseManyRequest {
    // Idempotency key: a replay after a lost response must not
    // double-increment the use counters.
    request_id: String,
    skill_names: Vec<String>,
    // Caller-provided RFC3339 timestamp so retries stay deterministic.
    now_utc: String,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifecycleBumpUseManyResponse {
    pub bumped: usize,
}

agent_request!(BootstrapOwnerRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapOwnerResponse {
    pub owner: Option<BootstrapOwnerDto>,
}

agent_request!(BootstrapClaimOwnerRequest {
    request_id: String,
    chat_id: i64,
    thread_id: i64,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapClaimOwnerResponse {
    pub outcome: ClaimOwnerOutcomeDto,
}

agent_request!(BootstrapStageScopeRequest {
    chat_id: i64,
    thread_id: i64
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapMissingStagesResponse {
    pub stages: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapStageResponse {
    pub stage: Option<String>,
}

agent_request!(BootstrapRecordQuestionIssueRequest {
    request_id: String,
    stage: String,
    chat_id: i64,
    thread_id: i64,
    assistant_message_id: i32,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapRecordQuestionIssueResponse {
    pub outcome: RecordQuestionIssueOutcomeDto,
}

agent_request!(BootstrapRecordCurrentAnswerRequest {
    request_id: String,
    answer: String,
    chat_id: i64,
    thread_id: i64,
    source_message_id: i32,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapRecordCurrentAnswerResponse {
    pub outcome: RecordCurrentAnswerOutcomeDto,
}

agent_request!(BootstrapRecordAnswerRequest {
    request_id: String,
    stage: String,
    answer: String,
    chat_id: i64,
    thread_id: i64,
    source_message_id: i32,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapRecordAnswerResponse {
    pub outcome: RecordAnswerOutcomeDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapRecordedAnswersResponse {
    pub answers: Vec<RecordedAnswerDto>,
}

agent_request!(BootstrapClearRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapClearResponse {
    pub cleared: usize,
}

/// Shared empty success body for mutations that return nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkResponse {}

// ---------------------------------------------------------------------------
// RunLedgerAndDelivery DTOs
// ---------------------------------------------------------------------------

/// Raw `cron_specs` row mirror. The schedule encoding columns are passed
/// through verbatim so the bot reconstructs `CronSpec` with the exact parsing
/// rules that already live in `right_agent::cron_spec`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronSpecDto {
    pub job_name: String,
    pub schedule: String,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: f64,
    pub triggered_at: Option<String>,
    pub trigger_force_notify: bool,
    pub recurring: bool,
    pub run_at: Option<String>,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
    pub model: Option<String>,
    pub trigger_extra_instruction: Option<String>,
    /// Serialized `ThenSpec` JSON, passed through uninterpreted.
    pub trigger_then_json: Option<String>,
    pub trigger_origin_chat_id: Option<i64>,
    pub trigger_origin_thread_id: Option<i64>,
}

/// Cron detail projection for the dashboard/drilldown views.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronSpecDetailDto {
    pub spec: CronSpecDto,
    pub recent_runs: Vec<CronRunRowDto>,
    /// Skill names currently linked to this job.
    pub linked_skills: Vec<String>,
}

/// One `async_runs` row as shown in cron run history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronRunRowDto {
    pub id: String,
    pub job_name: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub status: String,
    pub log_path: Option<String>,
    pub run_note: Option<String>,
    pub delivery_json: Option<String>,
    pub delivered_at: Option<String>,
    pub delivery_status: Option<String>,
}

/// One undelivered async result claimed by the delivery loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingAsyncResultDto {
    pub id: String,
    pub kind: String,
    pub producer_ref: Option<String>,
    pub delivery_json: String,
    pub run_note: String,
    pub status: String,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
    pub force_notify: bool,
}

agent_request!(CronSpecsListRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronSpecsListResponse {
    pub specs: Vec<CronSpecDto>,
}

agent_request!(CronSpecDetailRequest { job_name: String });
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronSpecDetailResponse {
    pub detail: Option<CronSpecDetailDto>,
}

agent_request!(CronRecentRunsRequest {
    job_name: String,
    limit: u32
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronRecentRunsResponse {
    pub runs: Vec<CronRunRowDto>,
}

agent_request!(CronDeleteSpecRequest { job_name: String });
agent_request!(CronJobRequest { job_name: String });

agent_request!(EnqueueBackgroundRunRequest {
    request_id: String,
    run_id: String,
    producer_ref: Option<String>,
    source_session_id: String,
    run_session_id: String,
    target_chat_id: i64,
    target_thread_id: Option<i64>,
    created_at: String,
});

agent_request!(CronInsertRunningRunRequest {
    request_id: String,
    run_id: String,
    job_name: String,
    started_at: String,
    log_path: String,
    target_chat_id: Option<i64>,
    target_thread_id: Option<i64>,
    force_notify: bool,
});

agent_request!(MarkBackgroundSpawnedRequest { run_id: String });

// Atomically persist run output (run note + delivery decision) and mark the
// run finished. One immediate transaction server-side; `delivery_status` in
// the response is owner-computed (`pending` when delivery is required,
// `none` otherwise).
agent_request!(PersistRunOutputRequest {
    request_id: String,
    run_id: String,
    run_note: Option<String>,
    delivery_json: Option<String>,
    error_json: Option<String>,
    delivery_required: bool,
    exit_code: Option<i32>,
    status: String,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistRunOutputResponse {
    pub delivery_status: String,
}

agent_request!(FinishRunRequest {
    request_id: String,
    run_id: String,
    exit_code: Option<i32>,
    status: String,
});

// Mark a background handoff failed: persist a failure delivery payload, flip
// the handoff state, and finish the run as `failed` (the current bot
// sequence, preserved server-side).
agent_request!(MarkHandoffFailedRequest {
    request_id: String,
    run_id: String,
    run_note: String,
    delivery_json: String,
    error_json: String,
});

agent_request!(RecoverInterruptedHandoffsRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveredCountResponse {
    pub recovered: usize,
}

agent_request!(CronMarkInterruptedByShutdownRequest {
    job_name: String,
    reason: String,
});

agent_request!(DeliveryFetchPendingRequest { limit: u32 });
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryFetchPendingResponse {
    pub pending: Vec<PendingAsyncResultDto>,
}

agent_request!(DeliveryMarkOutcomeRequest {
    request_id: String,
    run_id: String,
    status: String,
});

agent_request!(DeliveryDeduplicateJobRequest {
    producer_ref: String
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryDeduplicateJobResponse {
    pub candidate: Option<PendingAsyncResultDto>,
    pub superseded: u32,
}

// ---------------------------------------------------------------------------
// SecretsAndMcpRegistry DTOs
// ---------------------------------------------------------------------------

agent_request!(AuthStatusRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthStatusResponse {
    pub token_present: bool,
}

agent_request!(AuthTokenGetRequest {});
/// Secret-bearing: the token value serializes only into the UDS body; `Debug`
/// redacts it and the value zeroizes on drop.
#[derive(Clone, Serialize, Deserialize)]
pub struct AuthTokenGetResponse {
    #[serde(
        serialize_with = "serialize_secret_opt",
        deserialize_with = "deserialize_secret_opt",
        default
    )]
    pub token: Option<SecretString>,
}

impl std::fmt::Debug for AuthTokenGetResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthTokenGetResponse")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Secret-bearing request: same exposure rules as [`AuthTokenGetResponse`].
#[derive(Clone, Serialize, Deserialize)]
pub struct AuthTokenSaveRequest {
    pub agent: String,
    pub request_id: String,
    #[serde(serialize_with = "serialize_secret_exposed")]
    pub token: SecretString,
}

impl std::fmt::Debug for AuthTokenSaveRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthTokenSaveRequest")
            .field("agent", &self.agent)
            .field("request_id", &self.request_id)
            .field("token", &"<redacted>")
            .finish()
    }
}

agent_request!(AuthTokenDeleteRequest { request_id: String });

agent_request!(NoticeTokenGetOrCreateRequest { request_id: String });
/// Secret-bearing: see [`AuthTokenGetResponse`].
#[derive(Clone, Serialize, Deserialize)]
pub struct NoticeTokenGetOrCreateResponse {
    #[serde(serialize_with = "serialize_secret_exposed")]
    pub token: SecretString,
}

impl std::fmt::Debug for NoticeTokenGetOrCreateResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoticeTokenGetOrCreateResponse")
            .field("token", &"<redacted>")
            .finish()
    }
}

/// One-shot OAuth-state migration payload (oauth-state.json → DB). Secret
/// fields expose only at the UDS boundary.
#[derive(Clone, Serialize, Deserialize)]
pub struct McpOauthStateSetRequest {
    pub agent: String,
    pub request_id: String,
    pub server_name: String,
    #[serde(serialize_with = "serialize_secret_exposed")]
    pub access_token: SecretString,
    #[serde(
        serialize_with = "serialize_secret_opt",
        deserialize_with = "deserialize_secret_opt",
        default
    )]
    pub refresh_token: Option<SecretString>,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(
        serialize_with = "serialize_secret_opt",
        deserialize_with = "deserialize_secret_opt",
        default
    )]
    pub client_secret: Option<SecretString>,
    pub expires_at: String,
    pub oauth_resource: String,
}

impl std::fmt::Debug for McpOauthStateSetRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOauthStateSetRequest")
            .field("agent", &self.agent)
            .field("request_id", &self.request_id)
            .field("server_name", &self.server_name)
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("oauth_resource", &self.oauth_resource)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// LearningMemoryAndUsage DTOs
// ---------------------------------------------------------------------------

/// Per-invocation usage metrics mirror (`right_agent::usage::UsageBreakdown`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageBreakdownDto {
    pub session_uuid: String,
    pub total_cost_usd: f64,
    pub num_turns: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub model_usage_json: String,
    pub api_key_source: String,
    pub wall_elapsed_ms: Option<u64>,
}

/// Which insert path the owner runs. Variants map 1:1 onto the
/// `right_agent::usage::insert_*` functions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum UsageSourceDto {
    Interactive { chat_id: i64, thread_id: i64 },
    Cron { job_name: String },
    ReflectionWorker { chat_id: i64, thread_id: i64 },
    ReflectionCron { job_name: String },
    LearningPrefilter { chat_id: i64, thread_id: i64 },
    LearningProbeWriter { chat_id: i64, thread_id: i64 },
    LearningCurator,
    IdleCompaction { chat_id: i64, thread_id: i64 },
}

agent_request!(UsageInsertEventRequest {
    request_id: String,
    source: UsageSourceDto,
    event: UsageBreakdownDto,
});

/// Mirror of `right_agent::learned_skills::LearningEvent` (enums as their DB
/// strings; validated server-side).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningEventDto {
    pub invocation_id: String,
    /// `create` | `update`.
    pub action: String,
    pub skill_name: String,
    /// `start` | `finish`.
    pub phase: String,
    /// `created` | `updated` | `aborted` | `failed`; must be absent for
    /// `start` events.
    pub status: Option<String>,
    pub hint_outcome: Option<String>,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub summary: Option<String>,
    pub event_refs: Vec<String>,
}

agent_request!(LearningEventInsertRequest {
    request_id: String,
    event: LearningEventDto,
});

agent_request!(LearningTodaySpendRequest { now_utc: String });
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningTodaySpendResponse {
    pub usd: f64,
}

agent_request!(LearningRecordBudgetSkipRequest {
    request_id: String,
    chat_id: i64,
    thread_id: i64,
    reason: String,
    intended_kind: Option<String>,
});

agent_request!(LearningAuthoredSkillThisTurnRequest {
    invocation_id: String
});

agent_request!(LearningLinkCronAuthoredRequest {
    job_name: String,
    invocation_id: String,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningLinkCronAuthoredResponse {
    pub linked: usize,
}

agent_request!(LearningLatestInteractiveContextTokensRequest {
    chat_id: i64,
    thread_id: i64,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningLatestInteractiveContextTokensResponse {
    pub tokens: Option<u64>,
}

/// One percentile metric from the turn-baseline computation. `p50/p90/p99`
/// are `f64` on the wire; the bot casts back to the native integer types
/// (`num_turns: u32`, `wall_elapsed_ms: u64`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BaselineMetricDto {
    Insufficient { sample_size: u32 },
    Available { p50: f64, p90: f64, p99: f64 },
}

/// Mirror of `right_agent::usage::turn_baseline::TurnBaselines`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TurnBaselinesDto {
    pub sample_size: u32,
    pub elapsed_sample_size: u32,
    pub window_days: u32,
    pub cost_usd: BaselineMetricDto,
    pub num_turns: BaselineMetricDto,
    pub wall_elapsed_ms: BaselineMetricDto,
}

agent_request!(LearningTurnBaselinesRequest {
    window_days: u32,
    min_sample: u32,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningTurnBaselinesResponse {
    pub baselines: TurnBaselinesDto,
}

/// Mirror of `right_agent::usage::turn_baseline::CostSpikeEvidence`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CostSpikeEvidenceDto {
    pub today_cost_usd: f64,
    pub baseline_p50_usd: f64,
    pub k: f64,
    pub min_floor_usd: f64,
}

agent_request!(LearningProbeCostSpikeRequest {
    now_rfc3339: String,
    baseline_days: u32,
    k: f64,
    min_floor_usd: f64,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningProbeCostSpikeResponse {
    pub evidence: Option<CostSpikeEvidenceDto>,
}

/// Curator circuit-breaker state mirror (bot's `CuratorState`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CuratorStateDto {
    pub last_run_at: Option<String>,
    pub last_run_status: Option<String>,
    pub consecutive_failures: u32,
    pub circuit_open_until: Option<String>,
    pub last_spike_evidence_json: Option<String>,
}

agent_request!(CuratorLoadStateRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratorLoadStateResponse {
    pub state: CuratorStateDto,
}

agent_request!(CuratorSaveStateRequest {
    request_id: String,
    state: CuratorStateDto,
});

/// One append-only curator run-history row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratorRunRecordDto {
    pub run_at: String,
    pub trigger: String,
    pub trigger_evidence_json: Option<String>,
    pub mode: String,
    pub status: String,
    pub cost_usd: f64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub consolidations: i64,
    pub archives: i64,
    pub summary: Option<String>,
    pub actions_json: String,
    pub invocation_id: Option<String>,
}

agent_request!(CuratorInsertRunRequest {
    request_id: String,
    record: CuratorRunRecordDto,
});

agent_request!(CuratorLatestChatActivityRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratorLatestChatActivityResponse {
    /// RFC 3339 timestamp of the newest archived conversation message.
    pub at: Option<String>,
}

agent_request!(CuratorChangeCountRequest {
    since_rfc3339: String
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratorChangeCountResponse {
    pub count: u32,
}

agent_request!(CuratorArchivedSnapshotRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchivedSkillDto {
    pub skill_name: String,
    pub absorbed_into: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratorArchivedSnapshotResponse {
    pub skills: Vec<ArchivedSkillDto>,
}

agent_request!(CuratorApplyTransitionsRequest {
    now_rfc3339: String,
    stale_after_days: u32,
    archive_after_days: u32,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratorApplyTransitionsResponse {
    pub transition_changes: usize,
    pub candidates: Vec<SkillLifecycleDto>,
    pub archived_this_pass: Vec<String>,
}

agent_request!(CuratorFinalizeRequest {
    request_id: String,
    state: CuratorStateDto,
    run_record: CuratorRunRecordDto,
    maintain_spend_entries: Vec<SkillSpendDto>,
});

agent_request!(LifecycleArchivedSinceRequest {
    since_rfc3339: String
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifecycleArchivedSinceResponse {
    pub skill_names: Vec<String>,
}

/// Mirror of `right_lifecycle::SkillLifecycleRow` (timestamps as RFC 3339
/// strings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillLifecycleDto {
    pub skill_name: String,
    /// `active` | `stale` | `archived`.
    pub state: String,
    pub pinned: bool,
    /// `foreground` | `probe_writer` | `curator` | `bundled`.
    pub created_by: String,
    pub use_count: u32,
    pub patch_count: u32,
    pub created_at: Option<String>,
    pub last_used_at: Option<String>,
    pub last_patched_at: Option<String>,
    pub archived_at: Option<String>,
    pub absorbed_into: Option<String>,
}

agent_request!(SkillLifecycleGetRequest { skill_name: String });
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillLifecycleGetResponse {
    pub row: Option<SkillLifecycleDto>,
}

agent_request!(SkillLifecycleListRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillLifecycleListResponse {
    pub rows: Vec<SkillLifecycleDto>,
}

agent_request!(SkillPinRequest {
    skill_name: String,
    pinned: bool
});

/// One skill-spend insert (`create` | `patch` | `maintain` | `usage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillSpendDto {
    pub skill_name: String,
    pub kind: String,
    pub cost_usd: f64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub invocation_id: Option<String>,
}

// Insert one or more skill-spend rows in a single immediate transaction.
agent_request!(SkillSpendRecordRequest {
    request_id: String,
    entries: Vec<SkillSpendDto>,
});

agent_request!(SkillSpendBySkillRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillSpendBySkillResponse {
    pub rows: HashMap<String, right_dashboard::api_types::SkillSpendAgg>,
}

agent_request!(AlertCheckAndRecordRequest {
    request_id: String,
    alert_type: String,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertCheckAndRecordResponse {
    pub should_fire: bool,
}

agent_request!(AlertRecordRequest {
    request_id: String,
    alert_type: String,
});

agent_request!(AlertClearRequest {
    alert_type: String,
    older_than_secs: Option<u64>,
});

// --- Retain queue (lease-based) ---

/// One queued retain item. `tags` is the decoded form of the stored
/// `tags_json` (absent tags decode to an empty vec).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingRetainDto {
    pub id: i64,
    pub content: String,
    pub context: Option<String>,
    pub document_id: Option<String>,
    pub update_mode: Option<String>,
    pub tags: Vec<String>,
    pub attempts: u32,
    pub created_at: String,
}

/// New item to enqueue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetainEnqueueItemDto {
    pub source: String,
    pub content: String,
    pub context: Option<String>,
    pub document_id: Option<String>,
    pub update_mode: Option<String>,
    pub tags: Vec<String>,
}

agent_request!(RetainEnqueueRequest {
    request_id: String,
    item: RetainEnqueueItemDto,
});

agent_request!(RetainClaimBatchRequest {
    limit: u32,
    lease_ttl_secs: u32,
});

/// A lease over a batch of queue items. The `claim_token` must be presented
/// to ack/nack; after `lease_expires_at` the items become claimable by
/// another consumer (crash recovery).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetainClaimDto {
    pub claim_token: String,
    pub lease_expires_at: String,
    pub items: Vec<PendingRetainDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetainClaimBatchResponse {
    pub claim: RetainClaimDto,
}

agent_request!(RetainAckRequest {
    claim_token: String,
    ids: Vec<i64>,
});

// `retry`: true = requeue for another attempt (bumping attempts, recording
// `error`); false = drop terminally.
agent_request!(RetainNackRequest {
    claim_token: String,
    ids: Vec<i64>,
    retry: bool,
    error: String,
});

agent_request!(RetainQueueStatsRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetainQueueStatsResponse {
    pub count: usize,
    pub oldest_age_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// Dashboard projections (typed; wire types come from right-dashboard's API
// types so the bot deserializes into the exact structs it renders)
// ---------------------------------------------------------------------------

agent_request!(DashboardActivityRequest {
    generated_at: String,
    refresh_interval_secs: u64,
    foreground: Vec<right_dashboard::api_types::ForegroundActivity>,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardActivityResponse {
    pub overview: right_dashboard::api_types::OverviewResponse,
}

agent_request!(DashboardRunDetailRequest {
    run_id: String,
    max_lines: u32,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardRunDetailResponse {
    pub detail: Option<right_dashboard::api_types::RunDetailResponse>,
}

agent_request!(DashboardOverviewRequest {
    generated_at: String,
    foreground_active_count: i64,
    sandbox: right_dashboard::api_types::OverviewSandboxStatus,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardOverviewResponseWrapper {
    pub overview: right_dashboard::api_types::DashboardOverviewResponse,
}

agent_request!(DashboardUsageRequest {
    generated_at: String,
    timezone: Option<String>,
    range: Option<String>,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardUsageResponse {
    pub overview: right_dashboard::api_types::UsageOverviewResponse,
}

agent_request!(DashboardLearningRequest {
    generated_at: String,
    refresh_interval_secs: u64,
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardLearningResponse {
    pub overview: right_dashboard::api_types::LearningOverviewResponse,
}

agent_request!(DashboardSkillLifecycleRequest {});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardSkillLifecycleResponse {
    pub overview: right_dashboard::api_types::SkillLifecycleOverviewResponse,
}

agent_request!(DashboardSkillSpendRequest {});

// ---------------------------------------------------------------------------
// Provider bindings (secret-bearing; HMAC-authenticated)
// ---------------------------------------------------------------------------

/// Derivation label for the provider-binding IPC token. The bot derives
/// `derive_token(agent.yaml::secret, PROVIDER_BINDING_IPC_LABEL)`; the
/// Aggregator recomputes it from the same secret and compares in constant
/// time. The requested agent must equal the authenticated identity.
pub const PROVIDER_BINDING_IPC_LABEL: &str = "provider-binding-ipc";

/// Derive the provider-binding auth token for an agent secret
/// (base64url, 43 chars, as stored in `agent.yaml::secret`).
pub fn provider_binding_token(agent_secret_b64: &str) -> miette::Result<String> {
    crate::derive_token(agent_secret_b64, PROVIDER_BINDING_IPC_LABEL)
}

/// Hard caps for the secret-resolution routes. Requests carry only
/// identities + a token; responses carry a handful of credentials.
pub const PROVIDER_BINDING_MAX_REQUEST_BYTES: usize = 16 * 1024;
pub const PROVIDER_BINDING_MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// A resolved provider credential binding. Mirrors
/// `right_sandbox::SecretBinding`; `value` exposes only at the UDS body
/// boundary. Consumers convert into `SecretBinding` immediately and drop the
/// DTO.
#[derive(Clone, Serialize, Deserialize)]
pub struct SecretBindingDto {
    pub provider: String,
    pub env_var: String,
    pub source_env_var: String,
    pub placeholder: String,
    pub allowed_hosts: Vec<String>,
    pub inject_query: bool,
    #[serde(serialize_with = "serialize_secret_exposed")]
    pub value: SecretString,
}

impl std::fmt::Debug for SecretBindingDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretBindingDto")
            .field("provider", &self.provider)
            .field("env_var", &self.env_var)
            .field("source_env_var", &self.source_env_var)
            .field("placeholder", &self.placeholder)
            .field("allowed_hosts", &self.allowed_hosts)
            .field("inject_query", &self.inject_query)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Resolve every provider the agent declares. `auth` is the token from
/// [`provider_binding_token`].
#[derive(Clone, Serialize, Deserialize)]
pub struct ResolveProviderBindingsRequest {
    pub agent: String,
    #[serde(serialize_with = "serialize_secret_exposed")]
    pub auth: SecretString,
}

impl std::fmt::Debug for ResolveProviderBindingsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolveProviderBindingsRequest")
            .field("agent", &self.agent)
            .field("auth", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ResolveProviderBindingsResponse {
    /// Bindings for every declared provider that currently holds a usable
    /// credential (order matches the store's resolution order: owned first,
    /// then borrowed). Providers still needing a value are absent; the bot
    /// warns for those exactly as it does today.
    pub bindings: Vec<SecretBindingDto>,
}

impl std::fmt::Debug for ResolveProviderBindingsResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolveProviderBindingsResponse")
            .field("bindings", &self.bindings.len())
            .finish()
    }
}

/// Resolve one named provider (dashboard mutation apply path).
#[derive(Clone, Serialize, Deserialize)]
pub struct ResolveNamedProviderBindingRequest {
    pub agent: String,
    pub provider: String,
    #[serde(serialize_with = "serialize_secret_exposed")]
    pub auth: SecretString,
}

impl std::fmt::Debug for ResolveNamedProviderBindingRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolveNamedProviderBindingRequest")
            .field("agent", &self.agent)
            .field("provider", &self.provider)
            .field("auth", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ResolveNamedProviderBindingResponse {
    pub binding: SecretBindingDto,
}

impl std::fmt::Debug for ResolveNamedProviderBindingResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolveNamedProviderBindingResponse")
            .field("binding", &"<redacted>")
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Typed client methods
// ---------------------------------------------------------------------------

// Secret-bearing routes below use `ErrorBodyMode::Redacted`, discarding every
// raw non-success response body after extracting its typed category.

macro_rules! db_method {
    ($(#[$meta:meta])* $name:ident, $req:ty, $res:ty, $route:expr) => {
        $(#[$meta])*
        pub async fn $name(&self, request: &$req) -> Result<$res, InternalDbError> {
            self.post($route, request)
                .await
                .map_err(classify_transport_error)
        }
    };
}

impl InternalClient {
    // --- InteractionState ---

    db_method!(
        archive_message,
        ArchiveMessageRequest,
        ArchiveMessageResponse,
        ROUTE_ARCHIVE_MESSAGE
    );
    db_method!(
        mark_message_routed,
        MarkMessageRoutedRequest,
        OkResponse,
        ROUTE_MARK_MESSAGE_ROUTED
    );
    db_method!(
        get_active_session,
        GetActiveSessionRequest,
        GetActiveSessionResponse,
        ROUTE_GET_ACTIVE_SESSION
    );
    db_method!(
        create_session,
        CreateSessionRequest,
        CreateSessionResponse,
        ROUTE_CREATE_SESSION
    );
    db_method!(
        deactivate_current_session,
        DeactivateCurrentSessionRequest,
        DeactivateCurrentSessionResponse,
        ROUTE_DEACTIVATE_CURRENT_SESSION
    );
    db_method!(
        activate_session,
        ActivateSessionRequest,
        OkResponse,
        ROUTE_ACTIVATE_SESSION
    );
    db_method!(
        touch_session,
        TouchSessionRequest,
        OkResponse,
        ROUTE_TOUCH_SESSION
    );
    db_method!(
        list_sessions,
        ListSessionsRequest,
        ListSessionsResponse,
        ROUTE_LIST_SESSIONS
    );
    db_method!(
        find_sessions_by_uuid,
        FindSessionsByUuidRequest,
        ListSessionsResponse,
        ROUTE_FIND_SESSIONS_BY_UUID
    );
    db_method!(
        find_session_by_root,
        FindSessionByRootRequest,
        FindSessionByRootResponse,
        ROUTE_FIND_SESSION_BY_ROOT
    );
    db_method!(
        latest_assistant_is_unique_exact,
        LatestAssistantIsUniqueExactRequest,
        BoolResultResponse,
        ROUTE_LATEST_ASSISTANT_IS_UNIQUE_EXACT
    );
    db_method!(
        is_recent_routed_target,
        IsRecentRoutedTargetRequest,
        BoolResultResponse,
        ROUTE_IS_RECENT_ROUTED_TARGET
    );
    db_method!(
        fetch_messages_by_ids,
        FetchMessagesByIdsRequest,
        FetchMessagesByIdsResponse,
        ROUTE_FETCH_MESSAGES_BY_IDS
    );
    db_method!(
        conversation_latest_turn_id,
        ConversationLatestTurnIdRequest,
        ConversationLatestTurnIdResponse,
        ROUTE_CONVERSATION_LATEST_TURN_ID
    );
    db_method!(
        thread_focus_get,
        ThreadFocusGetRequest,
        ThreadFocusGetResponse,
        ROUTE_THREAD_FOCUS_GET
    );
    db_method!(
        thread_focus_set_operator,
        ThreadFocusSetOperatorRequest,
        OkResponse,
        ROUTE_THREAD_FOCUS_SET_OPERATOR
    );
    db_method!(
        error_detail_insert,
        ErrorDetailInsertRequest,
        ErrorDetailInsertResponse,
        ROUTE_ERROR_DETAIL_INSERT
    );
    db_method!(
        error_detail_get,
        ErrorDetailGetRequest,
        ErrorDetailGetResponse,
        ROUTE_ERROR_DETAIL_GET
    );
    db_method!(
        lifecycle_bump_use_many,
        LifecycleBumpUseManyRequest,
        LifecycleBumpUseManyResponse,
        ROUTE_LIFECYCLE_BUMP_USE_MANY
    );
    db_method!(
        bootstrap_owner,
        BootstrapOwnerRequest,
        BootstrapOwnerResponse,
        ROUTE_BOOTSTRAP_OWNER
    );
    db_method!(
        bootstrap_claim_owner,
        BootstrapClaimOwnerRequest,
        BootstrapClaimOwnerResponse,
        ROUTE_BOOTSTRAP_CLAIM_OWNER
    );
    db_method!(
        bootstrap_missing_stages,
        BootstrapStageScopeRequest,
        BootstrapMissingStagesResponse,
        ROUTE_BOOTSTRAP_MISSING_STAGES
    );
    db_method!(
        bootstrap_first_missing_stage,
        BootstrapStageScopeRequest,
        BootstrapStageResponse,
        ROUTE_BOOTSTRAP_FIRST_MISSING_STAGE
    );
    db_method!(
        bootstrap_issued_question_stage,
        BootstrapStageScopeRequest,
        BootstrapStageResponse,
        ROUTE_BOOTSTRAP_ISSUED_QUESTION_STAGE
    );
    db_method!(
        bootstrap_record_question_issue,
        BootstrapRecordQuestionIssueRequest,
        BootstrapRecordQuestionIssueResponse,
        ROUTE_BOOTSTRAP_RECORD_QUESTION_ISSUE
    );
    db_method!(
        bootstrap_record_current_answer,
        BootstrapRecordCurrentAnswerRequest,
        BootstrapRecordCurrentAnswerResponse,
        ROUTE_BOOTSTRAP_RECORD_CURRENT_ANSWER
    );
    db_method!(
        bootstrap_record_answer,
        BootstrapRecordAnswerRequest,
        BootstrapRecordAnswerResponse,
        ROUTE_BOOTSTRAP_RECORD_ANSWER
    );
    db_method!(
        bootstrap_recorded_answers,
        BootstrapStageScopeRequest,
        BootstrapRecordedAnswersResponse,
        ROUTE_BOOTSTRAP_RECORDED_ANSWERS
    );
    db_method!(
        bootstrap_clear,
        BootstrapClearRequest,
        BootstrapClearResponse,
        ROUTE_BOOTSTRAP_CLEAR
    );

    // --- RunLedgerAndDelivery ---

    db_method!(
        cron_specs_list,
        CronSpecsListRequest,
        CronSpecsListResponse,
        ROUTE_CRON_SPECS_LIST
    );
    db_method!(
        cron_spec_detail,
        CronSpecDetailRequest,
        CronSpecDetailResponse,
        ROUTE_CRON_SPEC_DETAIL
    );
    db_method!(
        cron_recent_runs,
        CronRecentRunsRequest,
        CronRecentRunsResponse,
        ROUTE_CRON_RECENT_RUNS
    );
    db_method!(
        cron_delete_spec,
        CronDeleteSpecRequest,
        OkResponse,
        ROUTE_CRON_DELETE_SPEC
    );
    db_method!(
        cron_clear_triggered,
        CronJobRequest,
        OkResponse,
        ROUTE_CRON_CLEAR_TRIGGERED
    );
    db_method!(
        enqueue_background_run,
        EnqueueBackgroundRunRequest,
        OkResponse,
        ROUTE_ENQUEUE_BACKGROUND_RUN
    );
    db_method!(
        cron_insert_running_run,
        CronInsertRunningRunRequest,
        OkResponse,
        ROUTE_CRON_INSERT_RUNNING_RUN
    );
    db_method!(
        mark_background_spawned,
        MarkBackgroundSpawnedRequest,
        OkResponse,
        ROUTE_MARK_BACKGROUND_SPAWNED
    );
    db_method!(
        persist_run_output,
        PersistRunOutputRequest,
        PersistRunOutputResponse,
        ROUTE_PERSIST_RUN_OUTPUT
    );
    db_method!(finish_run, FinishRunRequest, OkResponse, ROUTE_FINISH_RUN);
    db_method!(
        mark_handoff_failed,
        MarkHandoffFailedRequest,
        OkResponse,
        ROUTE_MARK_HANDOFF_FAILED
    );
    db_method!(
        recover_interrupted_handoffs,
        RecoverInterruptedHandoffsRequest,
        RecoveredCountResponse,
        ROUTE_RECOVER_INTERRUPTED_HANDOFFS
    );
    db_method!(
        cron_mark_interrupted_by_shutdown,
        CronMarkInterruptedByShutdownRequest,
        RecoveredCountResponse,
        ROUTE_CRON_MARK_INTERRUPTED_BY_SHUTDOWN
    );
    db_method!(
        delivery_fetch_pending,
        DeliveryFetchPendingRequest,
        DeliveryFetchPendingResponse,
        ROUTE_DELIVERY_FETCH_PENDING
    );
    db_method!(
        delivery_mark_outcome,
        DeliveryMarkOutcomeRequest,
        OkResponse,
        ROUTE_DELIVERY_MARK_OUTCOME
    );
    db_method!(
        delivery_deduplicate_job,
        DeliveryDeduplicateJobRequest,
        DeliveryDeduplicateJobResponse,
        ROUTE_DELIVERY_DEDUPLICATE_JOB
    );

    // --- SecretsAndMcpRegistry ---

    db_method!(
        auth_status,
        AuthStatusRequest,
        AuthStatusResponse,
        ROUTE_AUTH_STATUS
    );

    /// Fetch the stored auth (setup) token. Secret value crosses only inside
    /// the UDS body; the response is redacted in `Debug` and zeroized on
    /// drop. The caller needs the value (keepalive validation, login flow) —
    /// treat it as credential material end to end.
    pub async fn auth_token_get(
        &self,
        request: &AuthTokenGetRequest,
    ) -> Result<AuthTokenGetResponse, InternalDbError> {
        self.post_bounded(
            ROUTE_AUTH_TOKEN_GET,
            request,
            crate::internal_client::DEFAULT_MAX_REQUEST_BYTES,
            crate::internal_client::DEFAULT_MAX_RESPONSE_BYTES,
            ErrorBodyMode::Redacted,
        )
        .await
        .map_err(classify_transport_error)
    }

    pub async fn auth_token_save(
        &self,
        request: &AuthTokenSaveRequest,
    ) -> Result<OkResponse, InternalDbError> {
        self.post_bounded(
            ROUTE_AUTH_TOKEN_SAVE,
            request,
            crate::internal_client::DEFAULT_MAX_REQUEST_BYTES,
            crate::internal_client::DEFAULT_MAX_RESPONSE_BYTES,
            ErrorBodyMode::Redacted,
        )
        .await
        .map_err(classify_transport_error)
    }

    db_method!(
        auth_token_delete,
        AuthTokenDeleteRequest,
        OkResponse,
        ROUTE_AUTH_TOKEN_DELETE
    );

    pub async fn notice_token_get_or_create(
        &self,
        request: &NoticeTokenGetOrCreateRequest,
    ) -> Result<NoticeTokenGetOrCreateResponse, InternalDbError> {
        self.post_bounded(
            ROUTE_NOTICE_TOKEN_GET_OR_CREATE,
            request,
            crate::internal_client::DEFAULT_MAX_REQUEST_BYTES,
            crate::internal_client::DEFAULT_MAX_RESPONSE_BYTES,
            ErrorBodyMode::Redacted,
        )
        .await
        .map_err(classify_transport_error)
    }

    pub async fn mcp_oauth_state_set(
        &self,
        request: &McpOauthStateSetRequest,
    ) -> Result<OkResponse, InternalDbError> {
        self.post_bounded(
            ROUTE_MCP_OAUTH_STATE_SET,
            request,
            crate::internal_client::DEFAULT_MAX_REQUEST_BYTES,
            crate::internal_client::DEFAULT_MAX_RESPONSE_BYTES,
            ErrorBodyMode::Redacted,
        )
        .await
        .map_err(classify_transport_error)
    }

    // --- LearningMemoryAndUsage ---

    db_method!(
        usage_insert_event,
        UsageInsertEventRequest,
        OkResponse,
        ROUTE_USAGE_INSERT_EVENT
    );
    db_method!(
        learning_event_insert,
        LearningEventInsertRequest,
        OkResponse,
        ROUTE_LEARNING_EVENT_INSERT
    );
    db_method!(
        learning_today_spend,
        LearningTodaySpendRequest,
        LearningTodaySpendResponse,
        ROUTE_LEARNING_TODAY_SPEND
    );
    db_method!(
        learning_record_budget_skip,
        LearningRecordBudgetSkipRequest,
        OkResponse,
        ROUTE_LEARNING_RECORD_BUDGET_SKIP
    );
    db_method!(
        learning_authored_skill_this_turn,
        LearningAuthoredSkillThisTurnRequest,
        BoolResultResponse,
        ROUTE_LEARNING_AUTHORED_SKILL_THIS_TURN
    );
    db_method!(
        learning_link_cron_authored,
        LearningLinkCronAuthoredRequest,
        LearningLinkCronAuthoredResponse,
        ROUTE_LEARNING_LINK_CRON_AUTHORED
    );
    db_method!(
        learning_latest_interactive_context_tokens,
        LearningLatestInteractiveContextTokensRequest,
        LearningLatestInteractiveContextTokensResponse,
        ROUTE_LEARNING_LATEST_INTERACTIVE_CONTEXT_TOKENS
    );
    db_method!(
        learning_turn_baselines,
        LearningTurnBaselinesRequest,
        LearningTurnBaselinesResponse,
        ROUTE_LEARNING_TURN_BASELINES
    );
    db_method!(
        learning_probe_cost_spike,
        LearningProbeCostSpikeRequest,
        LearningProbeCostSpikeResponse,
        ROUTE_LEARNING_PROBE_COST_SPIKE
    );
    db_method!(
        curator_load_state,
        CuratorLoadStateRequest,
        CuratorLoadStateResponse,
        ROUTE_CURATOR_LOAD_STATE
    );
    db_method!(
        curator_save_state,
        CuratorSaveStateRequest,
        OkResponse,
        ROUTE_CURATOR_SAVE_STATE
    );
    db_method!(
        curator_insert_run,
        CuratorInsertRunRequest,
        OkResponse,
        ROUTE_CURATOR_INSERT_RUN
    );
    db_method!(
        curator_latest_chat_activity,
        CuratorLatestChatActivityRequest,
        CuratorLatestChatActivityResponse,
        ROUTE_CURATOR_LATEST_CHAT_ACTIVITY
    );
    db_method!(
        curator_change_count,
        CuratorChangeCountRequest,
        CuratorChangeCountResponse,
        ROUTE_CURATOR_CHANGE_COUNT
    );
    db_method!(
        curator_archived_snapshot,
        CuratorArchivedSnapshotRequest,
        CuratorArchivedSnapshotResponse,
        ROUTE_CURATOR_ARCHIVED_SNAPSHOT
    );
    db_method!(
        curator_apply_transitions,
        CuratorApplyTransitionsRequest,
        CuratorApplyTransitionsResponse,
        ROUTE_CURATOR_APPLY_TRANSITIONS
    );
    db_method!(
        curator_finalize,
        CuratorFinalizeRequest,
        OkResponse,
        ROUTE_CURATOR_FINALIZE
    );
    db_method!(
        lifecycle_archived_since,
        LifecycleArchivedSinceRequest,
        LifecycleArchivedSinceResponse,
        ROUTE_LIFECYCLE_ARCHIVED_SINCE
    );
    db_method!(
        skill_lifecycle_get,
        SkillLifecycleGetRequest,
        SkillLifecycleGetResponse,
        ROUTE_SKILL_LIFECYCLE_GET
    );
    db_method!(
        skill_lifecycle_list,
        SkillLifecycleListRequest,
        SkillLifecycleListResponse,
        ROUTE_SKILL_LIFECYCLE_LIST
    );
    db_method!(skill_pin, SkillPinRequest, OkResponse, ROUTE_SKILL_PIN);
    db_method!(
        skill_spend_record,
        SkillSpendRecordRequest,
        OkResponse,
        ROUTE_SKILL_SPEND_RECORD
    );
    db_method!(
        skill_spend_by_skill,
        SkillSpendBySkillRequest,
        SkillSpendBySkillResponse,
        ROUTE_SKILL_SPEND_BY_SKILL
    );
    db_method!(
        alert_check_and_record,
        AlertCheckAndRecordRequest,
        AlertCheckAndRecordResponse,
        ROUTE_ALERT_CHECK_AND_RECORD
    );
    db_method!(
        alert_record,
        AlertRecordRequest,
        OkResponse,
        ROUTE_ALERT_RECORD
    );
    db_method!(
        alert_clear,
        AlertClearRequest,
        OkResponse,
        ROUTE_ALERT_CLEAR
    );
    db_method!(
        retain_enqueue,
        RetainEnqueueRequest,
        OkResponse,
        ROUTE_RETAIN_ENQUEUE
    );
    db_method!(
        retain_claim_batch,
        RetainClaimBatchRequest,
        RetainClaimBatchResponse,
        ROUTE_RETAIN_CLAIM_BATCH
    );
    db_method!(retain_ack, RetainAckRequest, OkResponse, ROUTE_RETAIN_ACK);
    db_method!(
        retain_nack,
        RetainNackRequest,
        OkResponse,
        ROUTE_RETAIN_NACK
    );
    db_method!(
        retain_queue_stats,
        RetainQueueStatsRequest,
        RetainQueueStatsResponse,
        ROUTE_RETAIN_QUEUE_STATS
    );

    // --- Dashboard projections ---

    db_method!(
        dashboard_activity,
        DashboardActivityRequest,
        DashboardActivityResponse,
        ROUTE_DASHBOARD_ACTIVITY
    );
    db_method!(
        dashboard_run_detail,
        DashboardRunDetailRequest,
        DashboardRunDetailResponse,
        ROUTE_DASHBOARD_RUN_DETAIL
    );
    db_method!(
        dashboard_overview,
        DashboardOverviewRequest,
        DashboardOverviewResponseWrapper,
        ROUTE_DASHBOARD_OVERVIEW
    );
    db_method!(
        dashboard_usage,
        DashboardUsageRequest,
        DashboardUsageResponse,
        ROUTE_DASHBOARD_USAGE
    );
    db_method!(
        dashboard_learning,
        DashboardLearningRequest,
        DashboardLearningResponse,
        ROUTE_DASHBOARD_LEARNING
    );
    db_method!(
        dashboard_skill_lifecycle,
        DashboardSkillLifecycleRequest,
        DashboardSkillLifecycleResponse,
        ROUTE_DASHBOARD_SKILL_LIFECYCLE
    );
    db_method!(
        dashboard_skill_spend,
        DashboardSkillSpendRequest,
        SkillSpendBySkillResponse,
        ROUTE_DASHBOARD_SKILL_SPEND
    );

    // --- Provider bindings (secret-bearing; body-capped both ways) ---

    /// Resolve every provider credential the agent declares. The caller must
    /// pass the token from [`provider_binding_token`]; the Aggregator
    /// verifies it in constant time and rejects cross-agent requests.
    pub async fn resolve_provider_bindings(
        &self,
        request: &ResolveProviderBindingsRequest,
    ) -> Result<ResolveProviderBindingsResponse, InternalDbError> {
        self.post_bounded(
            ROUTE_PROVIDER_BINDINGS_RESOLVE,
            request,
            PROVIDER_BINDING_MAX_REQUEST_BYTES,
            PROVIDER_BINDING_MAX_RESPONSE_BYTES,
            ErrorBodyMode::Redacted,
        )
        .await
        .map_err(classify_transport_error)
    }

    /// Resolve one named provider binding (dashboard apply path).
    pub async fn resolve_named_provider_binding(
        &self,
        request: &ResolveNamedProviderBindingRequest,
    ) -> Result<ResolveNamedProviderBindingResponse, InternalDbError> {
        self.post_bounded(
            ROUTE_PROVIDER_BINDINGS_RESOLVE_NAMED,
            request,
            PROVIDER_BINDING_MAX_REQUEST_BYTES,
            PROVIDER_BINDING_MAX_RESPONSE_BYTES,
            ErrorBodyMode::Redacted,
        )
        .await
        .map_err(classify_transport_error)
    }
}
