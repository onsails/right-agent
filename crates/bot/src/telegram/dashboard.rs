use std::{collections::BTreeSet, future::Future, path::PathBuf};

use axum::Json;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use hmac::{Hmac, KeyInit as _, Mac as _};
use right_dashboard::api_types::{
    ApiErrorBody, BootstrapResponse, DashboardFeatures, ForegroundActivity, PinSkillRequest,
};
use right_dashboard::auth::{
    AuthError, DashboardUser, InitDataValidation, authorize_user, validate_init_data,
};
use right_dashboard::read_model::{
    ReadModelError,
    activity::{ActivityOverviewInput, activity_overview, activity_run_detail},
    dashboard_overview::{DashboardOverviewInput, dashboard_overview},
    learning::{LearningOverviewInput, learning_overview, skill_lifecycle_overview},
    usage::{UsageOverviewInput, usage_overview},
};
use serde::Deserialize;
use sha2::Sha256;
use subtle::ConstantTimeEq;

mod focus;
#[cfg(test)]
pub(crate) use focus::FocusNotification;
pub(crate) use focus::FocusNotifier;
mod health;
mod identity;
mod mcp;
mod providers;
mod skills;

const REFRESH_INTERVAL_SECS: u64 = 5;
pub(super) const DASHBOARD_SANDBOX_TIMEOUT_SECS: u64 = 4;
/// Skills are listed by exec-ing a shell script inside the sandbox, which on
/// a cold sandbox (SSH/gRPC warm-up) routinely exceeds the 4s probe budget
/// the lightweight health/identity reads use. A generous bound here avoids
/// the misleading "0 learned skills" empty state — we now surface a timeout
/// error instead of silently falling back to the host filesystem (where no
/// `rightx-*` learned skills exist for sandboxed agents). Scoped to the cold
/// list scan only; interactive single-skill reads (detail/pin) run on an
/// already-warm sandbox and keep the short `DASHBOARD_SANDBOX_TIMEOUT_SECS`.
pub(super) const DASHBOARD_SANDBOX_SKILLS_TIMEOUT_SECS: u64 = 20;
const INIT_DATA_MAX_AGE_SECS: i64 = 86_400;
const FOCUS_SCOPE_TOKEN_TTL_SECS: i64 = 600;
const MAX_LOG_LINES: usize = 80;
#[cfg(test)]
const TEST_USAGE_GENERATED_AT: &str = "2026-06-04T12:00:00Z";

#[derive(Debug, Deserialize)]
struct UsageOverviewQuery {
    timezone: Option<String>,
    range: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DashboardUrlError {
    #[error("invalid dashboard hostname: {0}")]
    Parse(#[from] url::ParseError),
    #[error("dashboard hostname must not contain a path, query, or fragment (got {0:?})")]
    HostnameNotBare(String),
}

pub(crate) fn dashboard_url(
    hostname: &str,
    agent_name: &str,
) -> Result<url::Url, DashboardUrlError> {
    let stripped = hostname
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let base = url::Url::parse(&format!("https://{stripped}/"))?;
    if base.path() != "/"
        || base.query().is_some()
        || base.fragment().is_some()
        || !base.username().is_empty()
        || base.password().is_some()
    {
        return Err(DashboardUrlError::HostnameNotBare(hostname.to_string()));
    }
    Ok(base.join(&format!("/dashboard/{agent_name}/"))?)
}

pub(crate) fn generate_focus_scope_token(
    bot_token: &str,
    agent_name: &str,
    chat_id: i64,
    thread_id: i64,
) -> String {
    let expires_unix = chrono::Utc::now().timestamp() + FOCUS_SCOPE_TOKEN_TTL_SECS;
    focus_scope_token_for_expires(bot_token, agent_name, chat_id, thread_id, expires_unix)
}

fn focus_scope_token_for_expires(
    bot_token: &str,
    agent_name: &str,
    chat_id: i64,
    thread_id: i64,
    expires_unix: i64,
) -> String {
    let mac = focus_scope_mac(bot_token, agent_name, chat_id, thread_id, expires_unix);
    format!("{expires_unix}.{mac}")
}

fn focus_scope_mac(
    bot_token: &str,
    agent_name: &str,
    chat_id: i64,
    thread_id: i64,
    expires_unix: i64,
) -> String {
    let payload = format!("focus-scope-v1\n{agent_name}\n{chat_id}\n{thread_id}\n{expires_unix}");
    Hmac::<Sha256>::new_from_slice(bot_token.as_bytes())
        .expect("HMAC accepts any key length")
        .chain_update(payload.as_bytes())
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn focus_scope_token_valid(
    bot_token: &str,
    agent_name: &str,
    chat_id: i64,
    thread_id: i64,
    token: &str,
) -> bool {
    let Some((raw_expires, mac)) = token.split_once('.') else {
        return false;
    };
    let Ok(expires_unix) = raw_expires.parse::<i64>() else {
        return false;
    };
    if expires_unix <= chrono::Utc::now().timestamp() {
        return false;
    }

    let expected = focus_scope_mac(bot_token, agent_name, chat_id, thread_id, expires_unix);
    bool::from(expected.as_bytes().ct_eq(mac.as_bytes()))
}

#[derive(Clone)]
pub(crate) struct DashboardState {
    pub agent_name: String,
    pub bot_token: String,
    pub focus_notifier: FocusNotifier,
    pub home: PathBuf,
    pub agent_dir: PathBuf,
    /// Deterministic sandbox name for this agent. Always known, even while the
    /// sandbox itself is unavailable.
    pub sandbox_name: String,
    /// Live sandbox-backend state, published by the supervisor. The sandbox
    /// handle itself is read per request via [`DashboardState::sandbox`]: the
    /// supervisor installs a *new* handle after every recovery, so a snapshot
    /// taken at startup would keep addressing a deleted VM.
    pub sandbox_runtime: std::sync::Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    pub allowlist: right_agent::agent::allowlist::AllowlistHandle,
    pub foreground: super::StopTokens,
    pub internal_client: std::sync::Arc<right_mcp::internal_client::InternalClient>,
    /// Provider store used to resolve source-ref bindings after dashboard
    /// Authenticated provider-binding resolver for immediate sandbox apply.
    pub provider_bindings:
        Option<std::sync::Arc<crate::provider_bindings::ProviderBindingResolver>>,
    /// Shared with config-watcher reconcile to serialize this bot process's
    /// provider config publication and recovery. ProviderStore's per-agent
    /// advisory file lock is authoritative for sandbox mutation across bot and
    /// aggregator processes.
    pub provider_mutation: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// Latest config whose provider YAML mutation was accepted. Dashboard
    /// writes and the watcher publish under `provider_mutation`; recovery
    /// snapshots the same cell under that lock.
    pub provider_config: std::sync::Arc<arc_swap::ArcSwap<right_agent::agent::types::AgentConfig>>,
    pub pending_auth: super::oauth_callback::PendingAuthMap,
    pub oauth_status: super::oauth_status::OAuthFlowStatusStore,
    #[cfg(test)]
    pub mcp_oauth_allow_private_urls: bool,
    #[cfg(test)]
    pub doctor_checks: Option<Vec<right_agent::doctor::DoctorCheck>>,
}

impl DashboardState {
    /// The sandbox handle published right now, or `None` when the backend is
    /// unavailable (bring-up failed or a recovery is in flight). Every route
    /// that touches the guest resolves through here so it never addresses a
    /// retired handle.
    pub(crate) fn sandbox(&self) -> Option<crate::sandbox::Sandbox> {
        self.sandbox_runtime.current_sandbox()
    }
}

pub(crate) fn build_dashboard_router(state: DashboardState) -> axum::Router {
    axum::Router::new()
        .route("/dashboard/{agent}/", get(handle_static_index))
        .route("/dashboard/{agent}/api/v1/bootstrap", get(handle_bootstrap))
        .route(
            "/dashboard/{agent}/api/v1/activity/overview",
            get(handle_activity_overview),
        )
        .route(
            "/dashboard/{agent}/api/v1/activity/runs/{run_id}",
            get(handle_activity_run_detail),
        )
        .route(
            "/dashboard/{agent}/api/v1/usage",
            get(handle_usage_overview),
        )
        .route("/dashboard/{agent}/api/v1/overview", get(handle_overview))
        .route(
            "/dashboard/{agent}/api/v1/runs/{run_id}",
            get(handle_activity_run_detail),
        )
        .route(
            "/dashboard/{agent}/api/v1/learning/overview",
            get(handle_learning_overview),
        )
        .route(
            "/dashboard/{agent}/api/v1/knowledge/learning/overview",
            get(handle_learning_overview),
        )
        .route(
            "/dashboard/{agent}/api/v1/learning/skill_lifecycle",
            get(handle_learning_skill_lifecycle),
        )
        .route(
            "/dashboard/{agent}/api/v1/knowledge/skills",
            get(handle_skills_overview),
        )
        .route(
            "/dashboard/{agent}/api/v1/knowledge/skills/{skill_name}",
            get(handle_skill_detail),
        )
        .route(
            "/dashboard/{agent}/api/v1/knowledge/skills/{skill_name}/pin",
            patch(handle_pin_skill),
        )
        .route(
            "/dashboard/{agent}/api/v1/identity",
            get(handle_identity_files),
        )
        .route(
            "/dashboard/{agent}/api/v1/identity/{file_name}",
            get(handle_identity_file_detail),
        )
        .route(
            "/dashboard/{agent}/api/v1/health/doctor",
            get(handle_health_doctor),
        )
        .route(
            "/dashboard/{agent}/api/v1/health/sandbox",
            get(handle_health_sandbox),
        )
        .route(
            "/dashboard/{agent}/api/v1/mcp/servers",
            get(mcp::handle_mcp_servers).post(mcp::handle_mcp_add),
        )
        .route(
            "/dashboard/{agent}/api/v1/mcp/detect",
            post(mcp::handle_mcp_detect),
        )
        .route(
            "/dashboard/{agent}/api/v1/mcp/servers/{server_name}/headers",
            patch(mcp::handle_mcp_headers),
        )
        .route(
            "/dashboard/{agent}/api/v1/mcp/servers/{server_name}/oauth/start",
            post(mcp::handle_mcp_oauth_start),
        )
        .route(
            "/dashboard/{agent}/api/v1/mcp/oauth/{flow_id}/status",
            get(mcp::handle_mcp_oauth_status),
        )
        .route(
            "/dashboard/{agent}/api/v1/mcp/servers/{server_name}",
            delete(mcp::handle_mcp_remove),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers",
            get(providers::handle_list).post(providers::handle_create),
        )
        .route(
            "/dashboard/{agent}/api/v1/focus",
            get(focus::handle_get).patch(focus::handle_update),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers/types",
            get(providers::handle_types),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers/peers",
            get(providers::handle_peers),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers/share",
            post(providers::handle_share),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers/unshare",
            post(providers::handle_unshare),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers/borrow",
            post(providers::handle_borrow),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers/{provider_name}",
            delete(providers::handle_remove),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers/{provider_name}/rotate",
            post(providers::handle_rotate),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers/{provider_name}/config",
            patch(providers::handle_config_update),
        )
        .route(
            "/dashboard/{agent}/api/v1/crons/{job_name}",
            delete(handle_delete_cron),
        )
        .route("/dashboard/{agent}/{*asset}", get(handle_static_asset))
        .with_state(state)
}

async fn handle_static_index(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
) -> Response {
    serve_asset(&state, &agent, "")
}

async fn handle_static_asset(
    AxumPath((agent, asset)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
) -> Response {
    serve_asset(&state, &agent, &asset)
}

async fn handle_bootstrap(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    let user = match authenticate_api(&state, &agent, &headers) {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };

    Json(BootstrapResponse {
        agent: state.agent_name,
        api_version: "v1".to_string(),
        refresh_interval_secs: REFRESH_INTERVAL_SECS,
        user_id: user.id,
        features: DashboardFeatures {
            readonly: true,
            commands_enabled: false,
            learning_metrics: true,
            learning_evidence_snippets: false,
            learning_commands: false,
            activity: true,
            knowledge_learning: true,
            knowledge_skills: true,
            usage: true,
            identity: true,
            doctor: true,
            sandbox_stats: true,
        },
    })
    .into_response()
}

async fn handle_activity_overview(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let input = ActivityOverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        refresh_interval_secs: REFRESH_INTERVAL_SECS,
        foreground: foreground_activity(&state),
    };

    let request = right_mcp::internal_db::DashboardActivityRequest {
        agent: input.agent,
        generated_at: input.generated_at,
        refresh_interval_secs: input.refresh_interval_secs,
        foreground: input.foreground,
    };
    match state.internal_client.dashboard_activity(&request).await {
        Ok(response) => Json(response.overview).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard activity owner query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "overview_failed",
                Some("failed to read dashboard overview"),
            )
        }
    }
}

async fn handle_overview(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let input = DashboardOverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        foreground_active_count: foreground_activity(&state).len() as i64,
        sandbox: overview_sandbox_status(&state),
    };

    let request = right_mcp::internal_db::DashboardOverviewRequest {
        agent: input.agent,
        generated_at: input.generated_at,
        foreground_active_count: input.foreground_active_count,
        sandbox: input.sandbox,
    };
    match state.internal_client.dashboard_overview(&request).await {
        Ok(response) => Json(response.overview).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard overview owner query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "overview_failed",
                Some("failed to read dashboard overview"),
            )
        }
    }
}

async fn handle_activity_run_detail(
    AxumPath((agent, run_id)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let request = right_mcp::internal_db::DashboardRunDetailRequest {
        agent: state.agent_name.clone(),
        run_id: run_id.clone(),
        max_lines: MAX_LOG_LINES as u32,
    };
    match state.internal_client.dashboard_run_detail(&request).await {
        Ok(response) => match response.detail {
            Some(detail) => Json(detail).into_response(),
            None => json_error(StatusCode::NOT_FOUND, "not_found", Some("run not found")),
        },
        Err(error) => {
            tracing::error!(agent = %state.agent_name, run_id = %run_id, "dashboard run detail owner query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "run_detail_failed",
                Some("failed to read dashboard run detail"),
            )
        }
    }
}

async fn handle_usage_overview(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    Query(query): Query<UsageOverviewQuery>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let generated_at = {
        #[cfg(test)]
        {
            TEST_USAGE_GENERATED_AT.to_string()
        }
        #[cfg(not(test))]
        {
            chrono::Utc::now().to_rfc3339()
        }
    };

    let input = UsageOverviewInput {
        agent: state.agent_name.clone(),
        generated_at,
        timezone: query.timezone,
        range: query.range,
    };

    let request = right_mcp::internal_db::DashboardUsageRequest {
        agent: input.agent,
        generated_at: input.generated_at,
        timezone: input.timezone,
        range: input.range,
    };
    match state.internal_client.dashboard_usage(&request).await {
        Ok(response) => Json(response.overview).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard usage owner query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "usage_failed",
                Some("failed to read usage"),
            )
        }
    }
}

async fn handle_learning_overview(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let input = LearningOverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        refresh_interval_secs: REFRESH_INTERVAL_SECS,
    };

    let request = right_mcp::internal_db::DashboardLearningRequest {
        agent: input.agent,
        generated_at: input.generated_at,
        refresh_interval_secs: input.refresh_interval_secs,
    };
    match state.internal_client.dashboard_learning(&request).await {
        Ok(response) => Json(response.overview).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard learning owner query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "learning_overview_failed",
                Some("failed to read learning overview"),
            )
        }
    }
}

async fn handle_learning_skill_lifecycle(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let request = right_mcp::internal_db::DashboardSkillLifecycleRequest {
        agent: state.agent_name.clone(),
    };
    match state
        .internal_client
        .dashboard_skill_lifecycle(&request)
        .await
    {
        Ok(response) => Json(response.overview).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard skill lifecycle owner query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skill_lifecycle_failed",
                Some("failed to read skill lifecycle"),
            )
        }
    }
}

async fn handle_skills_overview(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    match skills::skills_response(&state).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard skills query failed: {error:#}");
            let detail = format!("{error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skills_failed",
                Some(detail.as_str()),
            )
        }
    }
}

async fn handle_skill_detail(
    AxumPath((agent, skill_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    match skills::skill_detail_response(&state, &skill_name).await {
        Ok(response) => Json(response).into_response(),
        Err(skills::SkillDetailError::Inventory(
            right_dashboard::skill_inventory::SkillInventoryError::InvalidSkillName(_),
        )) => json_error(
            StatusCode::BAD_REQUEST,
            "invalid_skill_name",
            Some("skill name is invalid"),
        ),
        Err(skills::SkillDetailError::Inventory(
            right_dashboard::skill_inventory::SkillInventoryError::NotFound(_),
        )) => json_error(StatusCode::NOT_FOUND, "not_found", Some("skill not found")),
        // The sandbox is the only source of truth for a skill package, so an
        // unreachable one is reported as such rather than as a generic read
        // failure the UI could mistake for "skill has no content".
        Err(skills::SkillDetailError::Sandbox(detail)) => {
            tracing::error!(agent = %state.agent_name, skill = %skill_name, "dashboard skill detail unavailable: {detail}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skill_failed",
                Some(detail.as_str()),
            )
        }
        Err(error) => {
            tracing::error!(agent = %state.agent_name, skill = %skill_name, "dashboard skill detail query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skill_failed",
                Some("failed to read skill"),
            )
        }
    }
}

async fn handle_pin_skill(
    AxumPath((agent, skill_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let request: PinSkillRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(agent = %state.agent_name, skill = %skill_name, "dashboard skill pin rejected malformed request body: {error:#}");
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                Some("invalid pin request body"),
            );
        }
    };

    match skills::pin_skill_response(&state, &skill_name, request.pinned).await {
        Ok(response) => Json(response).into_response(),
        Err(skills::PinSkillError::Inventory(
            right_dashboard::skill_inventory::SkillInventoryError::InvalidSkillName(_),
        ))
        | Err(skills::PinSkillError::NonRightx) => json_error(
            StatusCode::BAD_REQUEST,
            "invalid_skill_name",
            Some("skill name is invalid"),
        ),
        Err(skills::PinSkillError::Inventory(
            right_dashboard::skill_inventory::SkillInventoryError::NotFound(_),
        ))
        | Err(skills::PinSkillError::LifecycleMissing) => {
            json_error(StatusCode::NOT_FOUND, "not_found", Some("skill not found"))
        }
        Err(skills::PinSkillError::NotCuratorManaged) => json_error(
            StatusCode::CONFLICT,
            "skill_not_curator_managed",
            Some("skill is not curator-managed"),
        ),
        // Never a silent pin: a pin the sandbox could not verify is refused
        // and reported, not written.
        Err(skills::PinSkillError::Sandbox(detail)) => {
            tracing::error!(agent = %state.agent_name, skill = %skill_name, "dashboard skill pin refused: {detail}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skill_pin_failed",
                Some(detail.as_str()),
            )
        }
        Err(error) => {
            tracing::error!(agent = %state.agent_name, skill = %skill_name, "dashboard skill pin failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skill_pin_failed",
                Some("failed to update skill pin state"),
            )
        }
    }
}

async fn handle_delete_cron(
    AxumPath((agent, job_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let request = right_mcp::internal_db::CronDeleteSpecRequest {
        agent: state.agent_name.clone(),
        job_name: job_name.clone(),
    };
    match state.internal_client.cron_delete_spec(&request).await {
        Ok(_) => Json(serde_json::json!({ "deleted": true, "job_name": job_name })).into_response(),
        Err(right_mcp::internal_db::InternalDbError::Server {
            category: right_mcp::internal_db::DbErrorCategory::NotFound,
            ..
        }) => json_error(
            StatusCode::NOT_FOUND,
            "not_found",
            Some("cron job not found"),
        ),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, job = %job_name, "dashboard cron delete owner operation failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cron_delete_failed",
                Some("failed to delete cron job"),
            )
        }
    }
}

async fn handle_identity_files(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    match identity::identity_response(&state).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard identity query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "identity_failed",
                Some("failed to read identity files"),
            )
        }
    }
}

async fn handle_identity_file_detail(
    AxumPath((agent, file_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    match identity::identity_file_response(&state, &file_name).await {
        Ok(response) => Json(response).into_response(),
        Err(right_dashboard::identity_files::IdentityFilesError::InvalidFileName(_)) => json_error(
            StatusCode::BAD_REQUEST,
            "invalid_identity_file",
            Some("identity file name is invalid"),
        ),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, file = %file_name, "dashboard identity file query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "identity_file_failed",
                Some("failed to read identity file"),
            )
        }
    }
}

async fn handle_health_doctor(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    #[cfg(test)]
    if let Some(checks) = state.doctor_checks.clone() {
        return Json(health::doctor_response_from_checks(
            &state.agent_name,
            checks,
        ))
        .into_response();
    }

    let mut checks = right_agent::doctor::run_doctor(&state.home).await;
    apply_sandbox_diagnosis(&mut checks, &state.sandbox_runtime.health());
    Json(health::doctor_response_from_checks(
        &state.agent_name,
        checks,
    ))
    .into_response()
}

/// When the bot's live sandbox-backend health is `Unavailable`, override the
/// `sandbox-runtime` doctor check with the bot's cause-specific diagnosis so
/// the dashboard surfaces the same actionable fix the Telegram user receives.
/// No-op when health is `Ready` (the independent host probe stands).
fn apply_sandbox_diagnosis(
    checks: &mut [right_agent::doctor::DoctorCheck],
    health: &crate::sandbox_runtime::SandboxHealth,
) {
    if let crate::sandbox_runtime::SandboxHealth::Unavailable { diagnosis } = health
        && let Some(check) = checks
            .iter_mut()
            .find(|c| c.name == right_agent::doctor::SANDBOX_RUNTIME_CHECK)
    {
        check.status = right_agent::doctor::CheckStatus::Fail;
        check.detail = diagnosis.summary.clone();
        check.fix = if diagnosis.fixes.is_empty() {
            None
        } else {
            Some(diagnosis.fixes.join("; "))
        };
    }
}

async fn handle_health_sandbox(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    Json(health::sandbox_stats_response(&state.agent_name, state.sandbox().as_ref()).await)
        .into_response()
}

fn serve_asset(state: &DashboardState, agent: &str, asset_path: &str) -> Response {
    if agent != state.agent_name {
        return StatusCode::FORBIDDEN.into_response();
    }

    match right_dashboard::assets::asset(asset_path) {
        Some(asset) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, asset.content_type)],
            asset.bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn authenticate_api(
    state: &DashboardState,
    agent: &str,
    headers: &HeaderMap,
) -> Result<DashboardUser, DashboardRouteError> {
    let raw_init_data = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("tma "))
        .ok_or_else(|| {
            DashboardRouteError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                Some("missing Telegram Mini App authorization"),
            )
        })?;

    let validation = InitDataValidation {
        bot_token: state.bot_token.clone(),
        now: chrono::Utc::now(),
        max_age_secs: INIT_DATA_MAX_AGE_SECS,
    };
    let user = validate_init_data(raw_init_data, &validation).map_err(auth_error_response)?;
    let trusted_user_ids = {
        let allowlist = state.allowlist.0.read().map_err(|_| {
            DashboardRouteError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "allowlist_unavailable",
                Some("dashboard allowlist is unavailable"),
            )
        })?;
        allowlist
            .users()
            .iter()
            .map(|user| user.id)
            .collect::<BTreeSet<_>>()
    };

    let user = authorize_user(user, &trusted_user_ids).map_err(auth_error_response)?;
    if agent != state.agent_name {
        return Err(DashboardRouteError::new(
            StatusCode::FORBIDDEN,
            "agent_mismatch",
            Some("dashboard agent path does not match this bot"),
        ));
    }

    Ok(user)
}

#[derive(Clone, Copy)]
struct DashboardRouteError {
    status: StatusCode,
    error: &'static str,
    detail: Option<&'static str>,
}

impl DashboardRouteError {
    fn new(status: StatusCode, error: &'static str, detail: Option<&'static str>) -> Self {
        Self {
            status,
            error,
            detail,
        }
    }

    fn into_response(self) -> Response {
        json_error(self.status, self.error, self.detail)
    }
}

fn auth_error_response(error: AuthError) -> DashboardRouteError {
    match error {
        AuthError::UnauthorizedUser => DashboardRouteError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            Some("Telegram user is not trusted for this agent"),
        ),
        AuthError::MissingInitData => DashboardRouteError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            Some("missing Telegram Mini App authorization"),
        ),
        AuthError::Expired => DashboardRouteError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            Some("Telegram Mini App authorization expired; reopen the dashboard from Telegram"),
        ),
        AuthError::MalformedInitData | AuthError::InvalidHash | AuthError::MissingUser => {
            DashboardRouteError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                Some("invalid Telegram Mini App authorization"),
            )
        }
    }
}

fn foreground_activity(state: &DashboardState) -> Vec<ForegroundActivity> {
    state
        .foreground
        .iter()
        .map(|entry| {
            let (chat_id, thread_id) = *entry.key();
            let (turn_id, _) = entry.value();
            ForegroundActivity {
                chat_id,
                thread_id,
                turn_id: *turn_id,
            }
        })
        .collect()
}

fn overview_sandbox_status(
    state: &DashboardState,
) -> right_dashboard::api_types::OverviewSandboxStatus {
    match state.sandbox() {
        Some(sandbox) => right_dashboard::api_types::OverviewSandboxStatus {
            state: "configured".to_string(),
            detail: Some(sandbox.name().to_string()),
        },
        None => right_dashboard::api_types::OverviewSandboxStatus {
            state: "unavailable".to_string(),
            detail: Some(format!("sandbox '{}' is not running", state.sandbox_name)),
        },
    }
}

pub(super) fn json_error(status: StatusCode, error: &str, detail: Option<&str>) -> Response {
    (
        status,
        Json(ApiErrorBody {
            error: error.to_string(),
            detail: detail.map(str::to_string),
        }),
    )
        .into_response()
}

#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod tests;
