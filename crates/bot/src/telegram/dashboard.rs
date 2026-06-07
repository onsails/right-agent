use std::{collections::BTreeSet, future::Future, path::PathBuf};

use axum::Json;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
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
use right_db::Connection;
use serde::Deserialize;

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
const MAX_LOG_LINES: usize = 80;

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

#[derive(Clone)]
pub(crate) struct DashboardState {
    pub agent_name: String,
    pub bot_token: String,
    pub home: PathBuf,
    pub agent_dir: PathBuf,
    pub resolved_sandbox: Option<String>,
    pub sandbox_exec: Option<right_openshell::sandbox_exec::SandboxExec>,
    /// Live sandbox-backend health. Read by `apply_sandbox_diagnosis` to
    /// override the `openshell-gateway` doctor check with the bot's
    /// cause-specific diagnosis when the backend is unavailable. The
    /// `sandbox_exec` snapshot above is a separate, startup-captured value.
    pub sandbox_runtime: std::sync::Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    pub allowlist: right_agent::agent::allowlist::AllowlistHandle,
    pub foreground: super::StopTokens,
    pub internal_client: std::sync::Arc<right_mcp::internal_client::InternalClient>,
    pub pending_auth: super::oauth_callback::PendingAuthMap,
    pub oauth_status: super::oauth_status::OAuthFlowStatusStore,
    #[cfg(test)]
    pub mcp_oauth_allow_private_urls: bool,
    #[cfg(test)]
    pub doctor_checks: Option<Vec<right_agent::doctor::DoctorCheck>>,
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
            "/dashboard/{agent}/api/v1/providers/types",
            get(providers::handle_types),
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

    let agent_name = state.agent_name.clone();
    match with_dashboard_conn(&state, move |conn| async move {
        activity_overview(&conn, input).await
    })
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(DashboardConnError::Open(error)) => error.into_response(),
        Err(DashboardConnError::Work(error)) => {
            tracing::error!(agent = %agent_name, "dashboard activity overview query failed: {error:#}");
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

    let agent_name = state.agent_name.clone();
    match with_dashboard_conn(&state, move |conn| async move {
        dashboard_overview(&conn, input).await
    })
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(DashboardConnError::Open(error)) => error.into_response(),
        Err(DashboardConnError::Work(error)) => {
            tracing::error!(agent = %agent_name, "dashboard overview query failed: {error:#}");
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

    let agent_name = state.agent_name.clone();
    let run_id_for_query = run_id.clone();
    match with_dashboard_conn(&state, move |conn| async move {
        activity_run_detail(&conn, &run_id_for_query, MAX_LOG_LINES).await
    })
    .await
    {
        Ok(Some(response)) => Json(response).into_response(),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "not_found", Some("run not found")),
        Err(DashboardConnError::Open(error)) => error.into_response(),
        Err(DashboardConnError::Work(error)) => {
            tracing::error!(agent = %agent_name, run_id = %run_id, "dashboard run detail query failed: {error:#}");
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

    let input = UsageOverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        timezone: query.timezone,
        range: query.range,
    };

    let agent_name = state.agent_name.clone();
    match with_dashboard_conn(&state, move |conn| async move {
        usage_overview(&conn, input).await
    })
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(DashboardConnError::Open(error)) => error.into_response(),
        Err(DashboardConnError::Work(error)) => {
            tracing::error!(agent = %agent_name, "dashboard usage query failed: {error:#}");
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

    let agent_name = state.agent_name.clone();
    match with_dashboard_conn(&state, move |conn| async move {
        learning_overview(&conn, input).await
    })
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(DashboardConnError::Open(error)) => error.into_response(),
        Err(DashboardConnError::Work(error)) => {
            tracing::error!(agent = %agent_name, "dashboard learning overview query failed: {error:#}");
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

    let agent_name = state.agent_name.clone();
    let agent_name_for_query = state.agent_name.clone();
    match with_dashboard_conn(&state, move |conn| async move {
        skill_lifecycle_overview(&conn, &agent_name_for_query).await
    })
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(DashboardConnError::Open(error)) => error.into_response(),
        Err(DashboardConnError::Work(error)) => {
            tracing::error!(agent = %agent_name, "dashboard skill_lifecycle query failed: {error:#}");
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

    let conn = match right_db::open_connection(&state.agent_dir, false).await {
        Ok(conn) => conn,
        Err(error) => {
            tracing::error!(agent = %state.agent_name, job = %job_name, "dashboard cron delete: open db failed: {error:#}");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_open_failed",
                Some("failed to open database"),
            );
        }
    };

    match right_agent::cron_spec::delete_spec(&conn, &job_name, &state.agent_dir).await {
        Ok(_) => Json(serde_json::json!({ "deleted": true, "job_name": job_name })).into_response(),
        // `delete_spec` is bot-owned and signals an absent row as
        // `Err("job '<name>' not found")`; match its sentinel substring to map
        // the absence to 404 rather than 500. Keep this in sync if that wording changes.
        Err(error) if error.contains("not found") => json_error(
            StatusCode::NOT_FOUND,
            "not_found",
            Some("cron job not found"),
        ),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, job = %job_name, "dashboard cron delete failed: {error:#}");
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
/// `openshell-gateway` doctor check with the bot's cause-specific diagnosis so
/// the dashboard surfaces the same actionable fix the Telegram user receives.
/// No-op when health is `Ready` (the independent gateway probe stands).
fn apply_sandbox_diagnosis(
    checks: &mut [right_agent::doctor::DoctorCheck],
    health: &crate::sandbox_runtime::SandboxHealth,
) {
    if let crate::sandbox_runtime::SandboxHealth::Unavailable { diagnosis } = health
        && let Some(check) = checks.iter_mut().find(|c| c.name == "openshell-gateway")
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

    Json(health::sandbox_stats_response(&state.agent_name, state.sandbox_exec.as_ref()).await)
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

/// Failure mode from `with_dashboard_conn`. Connection-open failures already
/// have HTTP responses associated with them; the `Work` variant carries the
/// read-model error so each route can produce its own status code, error code,
/// and log message.
enum DashboardConnError {
    Open(DashboardRouteError),
    Work(ReadModelError),
}

async fn with_dashboard_conn<F, Fut, T>(
    state: &DashboardState,
    work: F,
) -> Result<T, DashboardConnError>
where
    F: FnOnce(Connection) -> Fut,
    Fut: Future<Output = Result<T, ReadModelError>>,
{
    let agent_dir = state.agent_dir.clone();
    let agent_name = state.agent_name.clone();
    let conn = right_db::open_connection_readonly(&agent_dir)
        .await
        .map_err(|error| {
            tracing::error!(agent = %agent_name, "dashboard db open failed: {error:#}");
            DashboardConnError::Open(DashboardRouteError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_open_failed",
                Some("failed to open dashboard database"),
            ))
        })?;
    work(conn).await.map_err(DashboardConnError::Work)
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
    match state.resolved_sandbox.as_deref() {
        Some(sandbox) => right_dashboard::api_types::OverviewSandboxStatus {
            state: "configured".to_string(),
            detail: Some(sandbox.to_string()),
        },
        None => right_dashboard::api_types::OverviewSandboxStatus {
            state: "unavailable".to_string(),
            detail: Some("agent is configured without a sandbox".to_string()),
        },
    }
}

fn json_error(status: StatusCode, error: &str, detail: Option<&str>) -> Response {
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
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use dashmap::DashMap;
    use hmac::{Hmac, KeyInit as _, Mac as _};
    use right_agent::agent::allowlist::{
        AllowedUser, AllowlistFile, AllowlistHandle, AllowlistState,
    };
    use serde_json::json;
    use sha2::Sha256;
    use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
    use tower::ServiceExt as _;

    const BOT_TOKEN: &str = "123456:test-token";

    fn test_state(agent_dir: std::path::PathBuf) -> super::DashboardState {
        let added_at = "2026-05-20T12:00:00Z".parse().expect("valid UTC timestamp");
        let allowlist = AllowlistHandle::new(AllowlistState::from_file(AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 42,
                label: None,
                added_by: None,
                added_at,
            }],
            groups: Vec::new(),
        }));

        super::DashboardState {
            agent_name: "alpha".to_string(),
            bot_token: BOT_TOKEN.to_string(),
            home: agent_dir.clone(),
            agent_dir: agent_dir.clone(),
            resolved_sandbox: None,
            sandbox_exec: None,
            sandbox_runtime: {
                let (h, _rx) = crate::sandbox_runtime::SandboxRuntimeHandle::new(
                    crate::sandbox_runtime::SandboxHealth::Ready,
                );
                h
            },
            allowlist,
            foreground: Arc::new(DashMap::new()),
            internal_client: Arc::new(right_mcp::internal_client::InternalClient::new(
                agent_dir.join("missing-internal.sock"),
            )),
            pending_auth: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            oauth_status: super::super::oauth_status::OAuthFlowStatusStore::default(),
            mcp_oauth_allow_private_urls: false,
            doctor_checks: Some(vec![
                right_agent::doctor::DoctorCheck {
                    name: "right".to_string(),
                    status: right_agent::doctor::CheckStatus::Pass,
                    detail: "found".to_string(),
                    fix: None,
                },
                right_agent::doctor::DoctorCheck {
                    name: "openshell-gateway".to_string(),
                    status: right_agent::doctor::CheckStatus::Warn,
                    detail: "not running".to_string(),
                    fix: Some("start gateway".to_string()),
                },
                right_agent::doctor::DoctorCheck {
                    name: "agents/".to_string(),
                    status: right_agent::doctor::CheckStatus::Fail,
                    detail: "missing".to_string(),
                    fix: None,
                },
            ]),
        }
    }

    fn signed_init_data(user_id: i64) -> String {
        signed_init_data_at(user_id, chrono::Utc::now().timestamp())
    }

    fn signed_init_data_at(user_id: i64, auth_date: i64) -> String {
        let user = json!({
            "id": user_id,
            "username": "tester",
            "first_name": "Test",
        })
        .to_string();
        let auth_date = auth_date.to_string();
        let mut pairs = vec![
            ("auth_date", auth_date),
            ("query_id", "test-query".to_string()),
            ("user", user),
        ];
        pairs.sort_by(|(left, _), (right, _)| left.cmp(right));

        let data_check_string = pairs
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\n");
        let secret_key = Hmac::<Sha256>::new_from_slice(b"WebAppData")
            .expect("HMAC accepts any key length")
            .chain_update(BOT_TOKEN.as_bytes())
            .finalize()
            .into_bytes();
        let hash = Hmac::<Sha256>::new_from_slice(&secret_key)
            .expect("HMAC accepts any key length")
            .chain_update(data_check_string.as_bytes())
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();

        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(key, &value);
        }
        serializer.append_pair("hash", &hash);
        serializer.finish()
    }

    async fn get(path: &str, auth: Option<String>, agent_dir: std::path::PathBuf) -> StatusCode {
        let router = super::build_dashboard_router(test_state(agent_dir));
        let mut builder = Request::builder().uri(path).method("GET");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        router
            .oneshot(builder.body(Body::empty()).expect("valid request"))
            .await
            .expect("router response")
            .status()
    }

    async fn get_json(
        path: &str,
        auth: Option<String>,
        agent_dir: std::path::PathBuf,
    ) -> (StatusCode, serde_json::Value) {
        let router = super::build_dashboard_router(test_state(agent_dir));
        let mut builder = Request::builder().uri(path).method("GET");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        let response = router
            .oneshot(builder.body(Body::empty()).expect("valid request"))
            .await
            .expect("router response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body bytes");
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json response")
        };
        (status, value)
    }

    async fn get_json_with_state(
        path: &str,
        auth: Option<String>,
        state: super::DashboardState,
    ) -> (StatusCode, serde_json::Value) {
        let router = super::build_dashboard_router(state);
        let mut builder = Request::builder().uri(path).method("GET");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        let response = router
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, value)
    }

    async fn patch_json(
        path: &str,
        auth: Option<String>,
        agent_dir: std::path::PathBuf,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let router = super::build_dashboard_router(test_state(agent_dir));
        let mut builder = Request::builder()
            .uri(path)
            .method("PATCH")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        let response = router
            .oneshot(
                builder
                    .body(Body::from(body.to_string()))
                    .expect("valid request"),
            )
            .await
            .expect("router response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body bytes");
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json response")
        };
        (status, value)
    }

    async fn post_json(
        path: &str,
        auth: Option<String>,
        agent_dir: std::path::PathBuf,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let router = super::build_dashboard_router(test_state(agent_dir));
        let mut builder = Request::builder()
            .uri(path)
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        let response = router
            .oneshot(
                builder
                    .body(Body::from(body.to_string()))
                    .expect("valid request"),
            )
            .await
            .expect("router response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body bytes");
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json response")
        };
        (status, value)
    }

    async fn post_json_with_state(
        path: &str,
        auth: Option<String>,
        state: super::DashboardState,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let router = super::build_dashboard_router(state);
        let mut builder = Request::builder()
            .uri(path)
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        let response = router
            .oneshot(
                builder
                    .body(Body::from(body.to_string()))
                    .expect("valid request"),
            )
            .await
            .expect("router response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body bytes");
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json response")
        };
        (status, value)
    }

    async fn post_raw(
        path: &str,
        auth: Option<String>,
        agent_dir: std::path::PathBuf,
        body: &'static str,
    ) -> (StatusCode, serde_json::Value) {
        let router = super::build_dashboard_router(test_state(agent_dir));
        let mut builder = Request::builder()
            .uri(path)
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        let response = router
            .oneshot(builder.body(Body::from(body)).expect("valid request"))
            .await
            .expect("router response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body bytes");
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json response")
        };
        (status, value)
    }

    async fn patch_raw(
        path: &str,
        auth: Option<String>,
        agent_dir: std::path::PathBuf,
        body: &'static str,
    ) -> (StatusCode, serde_json::Value) {
        let router = super::build_dashboard_router(test_state(agent_dir));
        let mut builder = Request::builder()
            .uri(path)
            .method("PATCH")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        let response = router
            .oneshot(builder.body(Body::from(body)).expect("valid request"))
            .await
            .expect("router response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body bytes");
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json response")
        };
        (status, value)
    }

    fn write_skill(agent_dir: &std::path::Path, skill_name: &str) {
        let skill_dir = agent_dir.join(".claude").join("skills").join(skill_name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {skill_name}\ndescription: Test skill.\n---\n# {skill_name}\n"),
        )
        .unwrap();
    }

    fn setup_crypto() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    fn write_test_global_config(home: &std::path::Path) {
        let credentials_file = home.join("tunnel-credentials.json");
        std::fs::write(&credentials_file, "{}").expect("write tunnel credentials");
        right_config::write_global_config(
            home,
            &right_config::GlobalConfig {
                tunnel: right_config::TunnelConfig {
                    tunnel_uuid: "00000000-0000-0000-0000-000000000000".to_string(),
                    credentials_file,
                    hostname: "right.example.com".to_string(),
                },
                aggregator: right_config::AggregatorConfig::default(),
            },
        )
        .expect("write global config");
    }

    fn test_state_with_internal_socket(
        agent_dir: std::path::PathBuf,
        socket_path: std::path::PathBuf,
        pending_auth: super::super::oauth_callback::PendingAuthMap,
    ) -> super::DashboardState {
        let mut state = test_state(agent_dir);
        state.internal_client =
            Arc::new(right_mcp::internal_client::InternalClient::new(socket_path));
        state.pending_auth = pending_auth;
        state
    }

    struct InternalApiFixture {
        _dir: tempfile::TempDir,
        socket_path: std::path::PathBuf,
        handle: tokio::task::JoinHandle<()>,
    }

    fn start_internal_mcp_list_server(servers: serde_json::Value) -> InternalApiFixture {
        let dir = tempfile::tempdir().expect("internal API tempdir");
        let socket_path = dir.path().join("internal.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind internal socket");
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept internal client");
            let request = read_http_request(&mut stream).await;
            assert!(
                request.starts_with("POST /mcp-list "),
                "unexpected internal request: {request}"
            );
            assert!(
                request.contains(r#""agent":"alpha""#),
                "internal request should be agent-scoped: {request}"
            );
            write_json_response(&mut stream, "200 OK", &json!({ "servers": servers })).await;
        });

        InternalApiFixture {
            _dir: dir,
            socket_path,
            handle,
        }
    }

    struct MockOAuthServer {
        base_url: String,
        handle: tokio::task::JoinHandle<()>,
    }

    enum MockOAuthRegisterResponse {
        Success,
        Error { status: &'static str, body: String },
    }

    async fn start_mock_oauth_server() -> MockOAuthServer {
        start_mock_oauth_server_with_register_response(MockOAuthRegisterResponse::Success).await
    }

    async fn start_mock_oauth_server_with_register_response(
        register_response: MockOAuthRegisterResponse,
    ) -> MockOAuthServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock OAuth server");
        let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
        let server_base_url = base_url.clone();
        let handle = tokio::spawn(async move {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().await.expect("accept OAuth client");
                let request = read_http_request(&mut stream).await;
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .expect("request path");

                match path {
                    "/mcp" => {
                        let metadata = format!(
                            "{}/.well-known/oauth-protected-resource/mcp",
                            server_base_url
                        );
                        write_http_response(
                            &mut stream,
                            "401 Unauthorized",
                            &[(
                                "WWW-Authenticate",
                                format!(r#"Bearer resource_metadata="{metadata}""#),
                            )],
                            "",
                        )
                        .await;
                    }
                    "/.well-known/oauth-protected-resource/mcp" => {
                        write_json_response(
                            &mut stream,
                            "200 OK",
                            &json!({
                                "resource": format!("{}/mcp", server_base_url),
                                "authorization_servers": [server_base_url],
                                "scopes_supported": ["tools.read"]
                            }),
                        )
                        .await;
                    }
                    "/.well-known/oauth-authorization-server" => {
                        write_json_response(
                            &mut stream,
                            "200 OK",
                            &json!({
                                "authorization_endpoint": format!("{}/authorize", server_base_url),
                                "token_endpoint": format!("{}/token", server_base_url),
                                "registration_endpoint": format!("{}/register", server_base_url),
                                "scopes_supported": ["tools.read", "offline_access"]
                            }),
                        )
                        .await;
                    }
                    "/register" => match &register_response {
                        MockOAuthRegisterResponse::Success => {
                            write_json_response(
                                &mut stream,
                                "200 OK",
                                &json!({
                                    "client_id": "dashboard-client",
                                    "client_secret": "dashboard-secret"
                                }),
                            )
                            .await;
                        }
                        MockOAuthRegisterResponse::Error { status, body } => {
                            write_http_response(
                                &mut stream,
                                status,
                                &[("Content-Type", "application/json".to_string())],
                                body,
                            )
                            .await;
                        }
                    },
                    _ => {
                        write_http_response(&mut stream, "404 Not Found", &[], "").await;
                    }
                }
            }
        });

        MockOAuthServer { base_url, handle }
    }

    async fn read_http_request<S>(stream: &mut S) -> String
    where
        S: AsyncRead + Unpin,
    {
        let mut buf = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let n = stream.read(&mut chunk).await.expect("read request");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if http_request_complete(&buf) {
                break;
            }
        }
        String::from_utf8(buf).expect("utf8 request")
    }

    fn http_request_complete(buf: &[u8]) -> bool {
        let Some(header_end) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&buf[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        buf.len() >= header_end + 4 + content_length
    }

    async fn write_json_response<S>(stream: &mut S, status: &str, body: &serde_json::Value)
    where
        S: AsyncWrite + Unpin,
    {
        let body = body.to_string();
        write_http_response(
            stream,
            status,
            &[("Content-Type", "application/json".to_string())],
            &body,
        )
        .await;
    }

    async fn write_http_response<S>(
        stream: &mut S,
        status: &str,
        headers: &[(&str, String)],
        body: &str,
    ) where
        S: AsyncWrite + Unpin,
    {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        response.push_str(body);
        stream
            .write_all(response.as_bytes())
            .await
            .expect("write response");
    }

    async fn insert_lifecycle_row(
        conn: &right_db::Connection,
        skill_name: &str,
        created_by: &str,
        pinned: bool,
    ) {
        conn.execute(
            "INSERT INTO skill_lifecycle (
                skill_name, state, pinned, created_by, created_at
             ) VALUES (?1, 'active', ?2, ?3, '2026-04-01T00:00:00Z')",
            (skill_name, i64::from(pinned), created_by),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dashboard_url_strips_scheme_and_trailing_slash() {
        let url = super::dashboard_url("https://right.example.com/", "alpha").unwrap();

        assert_eq!(url.as_str(), "https://right.example.com/dashboard/alpha/");
    }

    #[tokio::test]
    async fn dashboard_url_uses_agent_path() {
        let url = super::dashboard_url("right.example.com", "bot-one").unwrap();

        assert_eq!(url.path(), "/dashboard/bot-one/");
    }

    #[tokio::test]
    async fn dashboard_url_rejects_hostname_with_path() {
        let err = super::dashboard_url("right.example.com/some-path", "alpha")
            .expect_err("hostname with path must be rejected");

        assert!(matches!(err, super::DashboardUrlError::HostnameNotBare(_)));
    }

    #[tokio::test]
    async fn dashboard_url_rejects_hostname_with_path_after_scheme() {
        let err = super::dashboard_url("https://right.example.com/extra", "alpha")
            .expect_err("hostname with path must be rejected");

        assert!(matches!(err, super::DashboardUrlError::HostnameNotBare(_)));
    }

    #[tokio::test]
    async fn dashboard_url_rejects_hostname_with_query() {
        let err = super::dashboard_url("right.example.com/?token=abc", "alpha")
            .expect_err("hostname with query must be rejected");

        assert!(matches!(err, super::DashboardUrlError::HostnameNotBare(_)));
    }

    #[tokio::test]
    async fn dashboard_url_rejects_hostname_with_fragment() {
        let err = super::dashboard_url("right.example.com/#frag", "alpha")
            .expect_err("hostname with fragment must be rejected");

        assert!(matches!(err, super::DashboardUrlError::HostnameNotBare(_)));
    }

    #[tokio::test]
    async fn dashboard_url_rejects_hostname_with_userinfo() {
        let err = super::dashboard_url("user@right.example.com", "alpha")
            .expect_err("hostname with userinfo must be rejected");

        assert!(matches!(err, super::DashboardUrlError::HostnameNotBare(_)));
    }

    #[tokio::test]
    async fn dashboard_url_accepts_scheme_prefix() {
        let url = super::dashboard_url("https://right.example.com", "alpha").unwrap();

        assert_eq!(url.as_str(), "https://right.example.com/dashboard/alpha/");
    }

    #[tokio::test]
    async fn static_index_loads_without_auth() {
        let temp = tempfile::tempdir().expect("tempdir");

        let status = get("/dashboard/alpha/", None, temp.path().to_path_buf()).await;

        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn bootstrap_exposes_learning_capabilities() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/bootstrap",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["features"]["learning_metrics"], true);
        assert_eq!(body["features"]["learning_evidence_snippets"], false);
        assert_eq!(body["features"]["learning_commands"], false);
        assert_eq!(body["features"]["commands_enabled"], false);
        assert_eq!(body["features"]["activity"], true);
        assert_eq!(body["features"]["knowledge_learning"], true);
        assert_eq!(body["features"]["knowledge_skills"], true);
        assert_eq!(body["features"]["usage"], true);
        assert_eq!(body["features"]["identity"], true);
        assert_eq!(body["features"]["doctor"], true);
        assert_eq!(body["features"]["sandbox_stats"], true);
    }

    #[tokio::test]
    async fn api_rejects_missing_auth() {
        let temp = tempfile::tempdir().expect("tempdir");

        let status = get(
            "/dashboard/alpha/api/v1/bootstrap",
            None,
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_reports_empty_tma_header_as_missing_auth() {
        let temp = tempfile::tempdir().expect("tempdir");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/bootstrap",
            Some(String::new()),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "unauthorized");
        assert_eq!(body["detail"], "missing Telegram Mini App authorization");
    }

    #[tokio::test]
    async fn api_reports_expired_tma_header_as_reopen_required() {
        let temp = tempfile::tempdir().expect("tempdir");
        let expired_auth_date = chrono::Utc::now().timestamp() - super::INIT_DATA_MAX_AGE_SECS - 1;

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/bootstrap",
            Some(signed_init_data_at(42, expired_auth_date)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "unauthorized");
        assert_eq!(
            body["detail"],
            "Telegram Mini App authorization expired; reopen the dashboard from Telegram"
        );
    }

    #[tokio::test]
    async fn api_rejects_missing_auth_before_agent_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");

        let status = get(
            "/dashboard/beta/api/v1/bootstrap",
            None,
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_rejects_agent_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");

        let status = get(
            "/dashboard/beta/api/v1/bootstrap",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_rejects_valid_non_allowlisted_user() {
        let temp = tempfile::tempdir().expect("tempdir");

        let status = get(
            "/dashboard/alpha/api/v1/bootstrap",
            Some(signed_init_data(7)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn dashboard_mcp_servers_requires_auth() {
        let temp = tempfile::tempdir().expect("tempdir");

        let status = get(
            "/dashboard/alpha/api/v1/mcp/servers",
            None,
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn dashboard_mcp_servers_surfaces_connect_observability_fields() {
        // Regression: the DashboardMcpServer DTO must forward the connect
        // observability fields from /mcp-list, or the dashboard's failure-cause
        // UI is silently dead even though the frontend type declares them.
        let temp = tempfile::tempdir().expect("tempdir");
        let internal = start_internal_mcp_list_server(json!([
            {
                "name": "obsidian",
                "url": "https://mcp.example.com/mcp",
                "status": "unreachable",
                "tool_count": 0,
                "auth_type": "headers",
                "header_names": ["x-api-key"],
                "last_connect_error": "connection refused",
                "last_attempt_at": "2026-06-01T12:00:18Z",
                "last_success_at": "2026-06-01T11:59:00Z"
            }
        ]));
        let pending_auth = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let state = test_state_with_internal_socket(
            temp.path().to_path_buf(),
            internal.socket_path.clone(),
            pending_auth,
        );

        let (status, body) = get_json_with_state(
            "/dashboard/alpha/api/v1/mcp/servers",
            Some(signed_init_data(42)),
            state,
        )
        .await;
        internal.handle.await.expect("internal API task");

        assert_eq!(status, StatusCode::OK);
        let server = &body["servers"][0];
        assert_eq!(server["last_connect_error"], "connection refused");
        assert_eq!(server["last_attempt_at"], "2026-06-01T12:00:18Z");
        assert_eq!(server["last_success_at"], "2026-06-01T11:59:00Z");
    }

    #[tokio::test]
    async fn dashboard_mcp_oauth_status_requires_auth() {
        let temp = tempfile::tempdir().expect("tempdir");
        let status = get(
            "/dashboard/alpha/api/v1/mcp/oauth/flow-1/status",
            None,
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn dashboard_mcp_oauth_status_returns_pending_and_unknown() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(temp.path().to_path_buf());
        let store = super::super::oauth_status::OAuthFlowStatusStore::default();
        store
            .insert_pending("flow-1".to_string(), "composio".to_string())
            .await;
        state.oauth_status = store;

        let (status, body) = get_json_with_state(
            "/dashboard/alpha/api/v1/mcp/oauth/flow-1/status",
            Some(signed_init_data(42)),
            state.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "pending");
        assert_eq!(body["server_name"], "composio");

        let (status, body) = get_json_with_state(
            "/dashboard/alpha/api/v1/mcp/oauth/missing/status",
            Some(signed_init_data(42)),
            state,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "unknown");
        assert_eq!(body["message"], "OAuth flow is no longer active.");
    }

    #[tokio::test]
    async fn dashboard_mcp_detect_rejects_bad_url() {
        let temp = tempfile::tempdir().expect("tempdir");

        let (status, body) = post_json(
            "/dashboard/alpha/api/v1/mcp/detect",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
            json!({ "url": "not a url" }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_url");
    }

    #[tokio::test]
    async fn dashboard_mcp_json_routes_authenticate_before_body_parse() {
        let temp = tempfile::tempdir().expect("tempdir");

        let (status, _) = post_raw(
            "/dashboard/alpha/api/v1/mcp/detect",
            None,
            temp.path().to_path_buf(),
            "{",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = post_raw(
            "/dashboard/alpha/api/v1/mcp/servers",
            None,
            temp.path().to_path_buf(),
            "{",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = patch_raw(
            "/dashboard/alpha/api/v1/mcp/servers/nango/headers",
            None,
            temp.path().to_path_buf(),
            "{",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn dashboard_mcp_oauth_start_unknown_server_returns_not_found() {
        let temp = tempfile::tempdir().expect("tempdir");
        let internal = start_internal_mcp_list_server(json!([
            {
                "name": "present",
                "url": "https://mcp.example.com/mcp",
                "status": "connected",
                "tool_count": 0,
                "auth_type": "oauth",
                "header_names": []
            }
        ]));
        let pending_auth = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let state = test_state_with_internal_socket(
            temp.path().to_path_buf(),
            internal.socket_path.clone(),
            pending_auth.clone(),
        );

        let (status, body) = post_json_with_state(
            "/dashboard/alpha/api/v1/mcp/servers/missing/oauth/start",
            Some(signed_init_data(42)),
            state,
            json!({}),
        )
        .await;
        internal.handle.await.expect("internal API task");

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "not_found");
        assert!(!body.to_string().contains("access_token"));
        assert!(
            pending_auth.lock().await.is_empty(),
            "unknown server must not create pending auth"
        );
    }

    #[tokio::test]
    async fn dashboard_mcp_oauth_start_existing_server_missing_url_returns_distinct_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let internal = start_internal_mcp_list_server(json!([
            {
                "name": "no-url",
                "url": null,
                "status": "needs-auth",
                "tool_count": 0,
                "auth_type": "oauth",
                "header_names": []
            }
        ]));
        let pending_auth = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let state = test_state_with_internal_socket(
            temp.path().to_path_buf(),
            internal.socket_path.clone(),
            pending_auth.clone(),
        );

        let (status, body) = post_json_with_state(
            "/dashboard/alpha/api/v1/mcp/servers/no-url/oauth/start",
            Some(signed_init_data(42)),
            state,
            json!({}),
        )
        .await;
        internal.handle.await.expect("internal API task");

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "mcp_server_missing_url");
        assert!(!body.to_string().contains("access_token"));
        assert!(!body.to_string().contains("refresh_token"));
        assert!(!body.to_string().contains("client_secret"));
        assert!(!body.to_string().contains("code_verifier"));
        assert!(
            pending_auth.lock().await.is_empty(),
            "missing URL must not create pending auth"
        );
    }

    #[tokio::test]
    async fn dashboard_mcp_oauth_start_rejects_private_server_url_without_pending_auth() {
        setup_crypto();
        let temp = tempfile::tempdir().expect("tempdir");
        write_test_global_config(temp.path());
        let internal = start_internal_mcp_list_server(json!([
            {
                "name": "local",
                "url": "http://127.0.0.1:9/mcp",
                "status": "needs-auth",
                "tool_count": 0,
                "auth_type": "oauth",
                "header_names": []
            }
        ]));
        let pending_auth = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let state = test_state_with_internal_socket(
            temp.path().to_path_buf(),
            internal.socket_path.clone(),
            pending_auth.clone(),
        );

        let (status, body) = post_json_with_state(
            "/dashboard/alpha/api/v1/mcp/servers/local/oauth/start",
            Some(signed_init_data(42)),
            state,
            json!({}),
        )
        .await;
        internal.handle.await.expect("internal API task");

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["error"], "oauth_discovery_failed");
        assert!(!body.to_string().contains("127.0.0.1"));
        assert!(
            pending_auth.lock().await.is_empty(),
            "private URL rejection must not create pending auth"
        );
    }

    #[tokio::test]
    async fn dashboard_mcp_oauth_start_success_returns_auth_url_and_stores_pending_auth() {
        setup_crypto();
        let temp = tempfile::tempdir().expect("tempdir");
        write_test_global_config(temp.path());
        let oauth = start_mock_oauth_server().await;
        let server_url = format!("{}/mcp", oauth.base_url);
        let internal = start_internal_mcp_list_server(json!([
            {
                "name": "linear",
                "url": server_url,
                "status": "needs-auth",
                "tool_count": 0,
                "auth_type": "oauth",
                "header_names": []
            }
        ]));
        let pending_auth = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let mut state = test_state_with_internal_socket(
            temp.path().to_path_buf(),
            internal.socket_path.clone(),
            pending_auth.clone(),
        );
        let oauth_status = state.oauth_status.clone();
        state.mcp_oauth_allow_private_urls = true;

        let (status, body) = post_json_with_state(
            "/dashboard/alpha/api/v1/mcp/servers/linear/oauth/start",
            Some(signed_init_data(42)),
            state,
            json!({}),
        )
        .await;
        internal.handle.await.expect("internal API task");
        oauth.handle.await.expect("OAuth server task");

        assert_eq!(status, StatusCode::OK);
        let mut keys = body
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(keys, vec!["auth_url", "flow_id"]);
        let body_text = body.to_string();
        assert!(!body_text.contains("access_token"));
        assert!(!body_text.contains("refresh_token"));
        assert!(!body_text.contains("client_secret"));
        assert!(!body_text.contains("code_verifier"));

        let auth_url = body["auth_url"].as_str().expect("auth_url string");
        let parsed_auth_url = url::Url::parse(auth_url).expect("parse auth URL");
        assert_eq!(
            parsed_auth_url.as_str().split('?').next(),
            Some(format!("{}/authorize", oauth.base_url).as_str())
        );
        let query = parsed_auth_url
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        let state_param = query.get("state").expect("state query parameter");
        assert_eq!(body["flow_id"].as_str(), Some(state_param.as_str()));
        let status = oauth_status.status(state_param).await;
        assert_eq!(status.server_name.as_deref(), Some("linear"));
        assert_eq!(
            status.status,
            super::super::oauth_status::OAuthFlowStatus::Pending
        );
        assert_eq!(
            query.get("redirect_uri").map(String::as_str),
            Some("https://right.example.com/oauth/alpha/callback")
        );
        assert_eq!(
            query.get("resource").map(String::as_str),
            Some(format!("{}/mcp", oauth.base_url).as_str())
        );
        assert_eq!(
            query.get("client_id").map(String::as_str),
            Some("dashboard-client")
        );
        assert_eq!(
            query.get("scope").map(String::as_str),
            Some("tools.read offline_access")
        );

        let pending = pending_auth.lock().await;
        assert_eq!(pending.len(), 1);
        let pending = pending.get(state_param).expect("pending auth by state");
        assert_eq!(pending.server_name, "linear");
        assert_eq!(pending.server_url, format!("{}/mcp", oauth.base_url));
        assert_eq!(pending.resource, format!("{}/mcp", oauth.base_url));
        assert_eq!(pending.token_endpoint, format!("{}/token", oauth.base_url));
        assert_eq!(pending.client_id, "dashboard-client");
        assert_eq!(pending.client_secret.as_deref(), Some("dashboard-secret"));
        assert_eq!(
            pending.redirect_uri,
            "https://right.example.com/oauth/alpha/callback"
        );
        assert_eq!(pending.state, *state_param);
        assert!(!pending.code_verifier.is_empty());
    }

    #[tokio::test]
    async fn dashboard_mcp_oauth_start_registration_error_omits_upstream_secret_body() {
        setup_crypto();
        let temp = tempfile::tempdir().expect("tempdir");
        write_test_global_config(temp.path());
        let leaked_body = json!({
            "error": "server_error",
            "client_secret": "leaked-client-secret",
            "access_token": "leaked-access-token",
            "code_verifier": "leaked-code-verifier"
        })
        .to_string();
        let oauth =
            start_mock_oauth_server_with_register_response(MockOAuthRegisterResponse::Error {
                status: "500 Internal Server Error",
                body: leaked_body,
            })
            .await;
        let server_url = format!("{}/mcp", oauth.base_url);
        let internal = start_internal_mcp_list_server(json!([
            {
                "name": "linear",
                "url": server_url,
                "status": "needs-auth",
                "tool_count": 0,
                "auth_type": "oauth",
                "header_names": []
            }
        ]));
        let pending_auth = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let mut state = test_state_with_internal_socket(
            temp.path().to_path_buf(),
            internal.socket_path.clone(),
            pending_auth.clone(),
        );
        state.mcp_oauth_allow_private_urls = true;

        let (status, body) = post_json_with_state(
            "/dashboard/alpha/api/v1/mcp/servers/linear/oauth/start",
            Some(signed_init_data(42)),
            state,
            json!({}),
        )
        .await;
        internal.handle.await.expect("internal API task");
        oauth.handle.await.expect("OAuth server task");

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["error"], "client_registration_failed");
        assert_eq!(body["detail"], "OAuth client registration failed");
        let body_text = body.to_string();
        assert!(!body_text.contains("leaked-client-secret"));
        assert!(!body_text.contains("leaked-access-token"));
        assert!(!body_text.contains("leaked-code-verifier"));
        assert!(!body_text.contains("client_secret"));
        assert!(!body_text.contains("access_token"));
        assert!(!body_text.contains("code_verifier"));
        assert!(
            pending_auth.lock().await.is_empty(),
            "failed registration must not create pending auth"
        );
    }

    #[tokio::test]
    async fn overview_returns_data_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/overview",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["agent"], "alpha");
        assert_eq!(body["active_runs"], 0);
        assert_eq!(body["recent_failures"], 0);
        assert_eq!(body["today_cost_usd"], 0.0);
        assert_eq!(body["doctor"]["state"], "not_loaded");
        assert_eq!(body["sandbox"]["state"], "unavailable");
        assert!(
            body.get("signals").is_some(),
            "overview must expose visual signals"
        );
        assert!(
            body.get("cost_learning_river").is_some(),
            "overview must expose cost_learning_river"
        );
        assert!(
            body.get("warnings").is_some(),
            "overview must expose warnings"
        );
    }

    #[tokio::test]
    async fn activity_overview_returns_current_cron_payload() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        let run_note = "Checked 5 pairs";
        conn.execute(
            "INSERT INTO cron_specs (
                job_name, schedule, prompt, max_budget_usd, created_at, updated_at,
                recurring, target_chat_id, target_thread_id
             ) VALUES (
                'daily', '0 8 * * *', 'daily prompt', 2.5,
                '2026-05-20T00:00:00Z', '2026-05-20T00:00:00Z',
                1, 123, 456
             )",
            [],
        )
        .await
        .expect("insert cron spec");
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                target_thread_id, status, started_at, finished_at, exit_code,
                run_note, delivery_json, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-1', 'cron', 'daily', 'run-1', 123, 456, 'success',
                '2026-05-20T08:00:00Z', '2026-05-20T08:01:00Z', 0,
                ?1, '{\"kind\":\"notify\",\"content\":\"daily summary\"}', 1, 'pending',
                '2026-05-20T08:00:00Z', '2026-05-20T08:01:00Z'
             )",
            [run_note],
        )
        .await
        .expect("insert completed cron run");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/activity/overview",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["agent"], "alpha");
        assert_eq!(body["refresh_interval_secs"], 5);
        assert_eq!(body["summary"]["cron_count"], 1);
        assert_eq!(body["crons"][0]["job_name"], "daily");
        assert_eq!(body["crons"][0]["schedule"], "0 8 * * *");
        let last_run = &body["crons"][0]["last_run"];
        assert_eq!(last_run["delivery_required"], true);
        assert_eq!(last_run["delivery_kind"], "notify");
        assert_eq!(last_run["run_note"], run_note);
        assert_eq!(body["active"]["foreground"].as_array().unwrap().len(), 0);
        assert_eq!(body["active"]["background"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn usage_returns_structured_windows_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, web_search_requests,
                web_fetch_requests, model_usage_json, api_key_source
             ) VALUES (
                '2026-05-20T08:00:00Z', 'interactive', 1, 0, NULL, 's1',
                0.15, 1, 10, 20, 0, 0, 0, 0,
                '{\"sonnet\":{\"costUSD\":0.15}}', 'none'
             )",
            [],
        )
        .await
        .unwrap();

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/usage",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["agent"], "alpha");
        assert_eq!(body["timezone"], "UTC");
        assert!(body["windows"].is_array());
        assert_eq!(body["selected_range"], "last_7_days");
        assert_eq!(body["selected_window"], "last_7_days");
        assert_eq!(body["window"]["key"], "last_7_days");
        assert_eq!(body["windows"].as_array().unwrap().len(), 1);
        assert_eq!(body["windows"][0]["key"], "last_7_days");
        assert!(body["windows"][0]["range_start"].is_string());
        assert!(body["windows"][0]["range_end"].is_string());
        assert!(
            body["windows"][0]["range_label"]
                .as_str()
                .unwrap()
                .contains("UTC")
        );
        assert!(
            body.get("daily_series").is_some(),
            "usage must expose daily_series"
        );
        assert!(
            body.get("source_series").is_some(),
            "usage must expose source_series"
        );
        assert!(body["cron_jobs"].is_array());
    }

    #[tokio::test]
    async fn usage_accepts_timezone_query_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, web_search_requests,
                web_fetch_requests, model_usage_json, api_key_source
             ) VALUES (
                '2026-06-03T20:00:00Z', 'interactive', 1, 0, NULL, 's1',
                0.15, 1, 10, 20, 0, 0, 0, 0,
                '{\"sonnet\":{\"costUSD\":0.15}}', 'none'
             )",
            [],
        )
        .await
        .unwrap();

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/usage?timezone=Asia%2FDubai&range=today",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["timezone"], "Asia/Dubai");
        assert_eq!(body["selected_range"], "today");
        assert!(
            body["windows"][0]["range_label"]
                .as_str()
                .unwrap()
                .starts_with("Asia/Dubai")
        );
    }

    #[tokio::test]
    async fn learning_overview_returns_data_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/learning/overview",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["capabilities"]["learning_metrics"], true);
        assert_eq!(body["capabilities"]["learning_evidence_snippets"], false);
        assert_eq!(body["capabilities"]["learning_commands"], false);
        assert!(
            body.get("flow_nodes").is_some(),
            "learning overview must expose flow_nodes"
        );
        assert!(
            body.get("flow_edges").is_some(),
            "learning overview must expose flow_edges"
        );
        assert!(
            body.get("recent_learning_signals").is_some(),
            "learning overview must expose recent_learning_signals"
        );
    }

    #[tokio::test]
    async fn legacy_learning_episode_routes_are_not_mounted() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        let auth = signed_init_data(42);

        for path in [
            "/dashboard/alpha/api/v1/knowledge/learning/episodes",
            "/dashboard/alpha/api/v1/knowledge/learning/episodes/1",
            "/dashboard/alpha/api/v1/learning/reports/1",
            "/dashboard/alpha/api/v1/knowledge/learning/reports/1",
        ] {
            let status = get(path, Some(auth.clone()), temp.path().to_path_buf()).await;

            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn skills_route_groups_host_skills_when_no_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skills_dir = temp.path().join(".claude").join("skills");
        std::fs::create_dir_all(skills_dir.join("right-cron")).unwrap();
        std::fs::write(
            skills_dir.join("right-cron").join("SKILL.md"),
            "---\nname: right-cron\ndescription: Core cron control.\n---\n# Cron\n",
        )
        .unwrap();
        std::fs::create_dir_all(skills_dir.join("rightx-oauth-debugging")).unwrap();
        std::fs::write(
            skills_dir.join("rightx-oauth-debugging").join("SKILL.md"),
            "---\nname: rightx-oauth-debugging\ndescription: Learned OAuth flow.\n---\n# OAuth\n",
        )
        .unwrap();
        std::fs::create_dir_all(skills_dir.join("hub-browser")).unwrap();
        std::fs::write(
            skills_dir.join("hub-browser").join("SKILL.md"),
            "---\nname: hub-browser\ndescription: Browser automation.\n---\n# Browser\n",
        )
        .unwrap();
        let conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        insert_lifecycle_row(&conn, "rightx-oauth-debugging", "curator", true).await;

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/skills",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["source"], "host");
        assert_eq!(body["groups"]["core"][0]["name"], "right-cron");
        assert_eq!(body["groups"]["core"][0]["state"], serde_json::Value::Null);
        assert_eq!(body["groups"]["core"][0]["pinned"], false);
        assert_eq!(
            body["groups"]["learned"][0]["name"],
            "rightx-oauth-debugging"
        );
        assert_eq!(body["groups"]["learned"][0]["state"], "active");
        assert_eq!(body["groups"]["learned"][0]["pinned"], true);
        assert_eq!(body["groups"]["learned"][0]["created_by"], "curator");
        assert_eq!(body["groups"]["other"][0]["name"], "hub-browser");
    }

    #[tokio::test]
    async fn skills_route_uses_neutral_lifecycle_when_db_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "rightx-oauth-debugging");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/skills",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["groups"]["learned"][0]["name"],
            "rightx-oauth-debugging"
        );
        assert_eq!(
            body["groups"]["learned"][0]["state"],
            serde_json::Value::Null
        );
        assert_eq!(body["groups"]["learned"][0]["pinned"], false);
        assert_eq!(
            body["groups"]["learned"][0]["created_by"],
            serde_json::Value::Null
        );
        assert_eq!(body["groups"]["learned"][0]["use_count"], 0);
        assert_eq!(body["groups"]["learned"][0]["patch_count"], 0);
    }

    #[tokio::test]
    async fn skill_detail_route_returns_host_skill_preview() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill_dir = temp
            .path()
            .join(".claude")
            .join("skills")
            .join("rightx-oauth-debugging");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: rightx-oauth-debugging\ndescription: Learned OAuth flow.\n---\n# OAuth\n",
        )
        .unwrap();
        let conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        insert_lifecycle_row(&conn, "rightx-oauth-debugging", "probe_writer", true).await;

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/skills/rightx-oauth-debugging",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["skill"]["name"], "rightx-oauth-debugging");
        assert_eq!(body["skill"]["group"], "learned");
        assert_eq!(body["skill"]["state"], "active");
        assert_eq!(body["skill"]["pinned"], true);
        assert_eq!(body["skill"]["created_by"], "probe_writer");
        assert!(
            body["content_preview"]
                .as_str()
                .unwrap()
                .contains("# OAuth")
        );
    }

    #[tokio::test]
    async fn skill_detail_uses_neutral_lifecycle_when_table_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "rightx-oauth-debugging");
        let _conn = right_db::open_connection(temp.path(), false)
            .await
            .expect("open unmigrated db");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/skills/rightx-oauth-debugging",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["skill"]["name"], "rightx-oauth-debugging");
        assert_eq!(body["skill"]["state"], serde_json::Value::Null);
        assert_eq!(body["skill"]["pinned"], false);
        assert_eq!(body["skill"]["created_by"], serde_json::Value::Null);
        assert_eq!(body["skill"]["use_count"], 0);
        assert_eq!(body["skill"]["patch_count"], 0);
    }

    #[tokio::test]
    async fn skill_detail_route_rejects_invalid_skill_name() {
        let temp = tempfile::tempdir().expect("tempdir");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/skills/..secret",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_skill_name");
    }

    #[tokio::test]
    async fn dashboard_skill_pin_requires_auth() {
        let temp = tempfile::tempdir().expect("tempdir");

        let (status, body) = patch_json(
            "/dashboard/alpha/api/v1/knowledge/skills/rightx-oauth-debugging/pin",
            None,
            temp.path().to_path_buf(),
            json!({ "pinned": true }),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "unauthorized");
    }

    #[tokio::test]
    async fn dashboard_skill_pin_rejects_missing_auth_before_malformed_json() {
        let temp = tempfile::tempdir().expect("tempdir");

        let (status, body) = patch_raw(
            "/dashboard/alpha/api/v1/knowledge/skills/rightx-oauth-debugging/pin",
            None,
            temp.path().to_path_buf(),
            "{malformed",
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "unauthorized");
    }

    #[tokio::test]
    async fn dashboard_skill_pin_sets_db_pinned_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "rightx-oauth-debugging");
        let conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        insert_lifecycle_row(&conn, "rightx-oauth-debugging", "curator", false).await;

        let (status, body) = patch_json(
            "/dashboard/alpha/api/v1/knowledge/skills/rightx-oauth-debugging/pin",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
            json!({ "pinned": true }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["skill_name"], "rightx-oauth-debugging");
        assert_eq!(body["pinned"], true);
        let row = right_lifecycle::get(&conn, "rightx-oauth-debugging")
            .await
            .unwrap()
            .unwrap();
        assert!(row.pinned);
    }

    #[tokio::test]
    async fn dashboard_skill_pin_clears_db_pinned_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "rightx-oauth-debugging");
        let conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        insert_lifecycle_row(&conn, "rightx-oauth-debugging", "probe_writer", true).await;

        let (status, body) = patch_json(
            "/dashboard/alpha/api/v1/knowledge/skills/rightx-oauth-debugging/pin",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
            json!({ "pinned": false }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["skill_name"], "rightx-oauth-debugging");
        assert_eq!(body["pinned"], false);
        let row = right_lifecycle::get(&conn, "rightx-oauth-debugging")
            .await
            .unwrap()
            .unwrap();
        assert!(!row.pinned);
    }

    #[tokio::test]
    async fn dashboard_skill_pin_returns_404_for_unknown_skill_package() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        insert_lifecycle_row(&conn, "rightx-missing", "curator", false).await;

        let (status, body) = patch_json(
            "/dashboard/alpha/api/v1/knowledge/skills/rightx-missing/pin",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
            json!({ "pinned": true }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "not_found");
    }

    #[tokio::test]
    async fn dashboard_skill_pin_bypasses_host_check_when_sandbox_present() {
        // Regression: when an agent runs sandboxed, learned-skill packages
        // live in /sandbox/.claude/skills/<name>/SKILL.md, not under the
        // host agent_dir. Pin must dispatch to the sandbox probe instead
        // of returning 404 from a host SKILL.md check that can never see
        // the sandbox file.
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        insert_lifecycle_row(&conn, "rightx-oauth-debugging", "curator", false).await;
        // Deliberately do NOT call `write_skill`: the SKILL.md is absent
        // on the host. The old code path returned 404 here; the fix must
        // bypass that host check and go to the sandbox.

        // Construct a SandboxExec pointing at a non-existent mTLS dir so
        // the sandbox probe fails predictably at connect time. A 500
        // (Sandbox error) — anything but 404 from the host-skill check —
        // proves the host code path was bypassed. Wiring a real sandbox
        // here would require OpenShell; that is the live integration
        // test's job.
        let fake_sandbox_exec = right_openshell::sandbox_exec::SandboxExec::new(
            temp.path().join("nonexistent-mtls"),
            "fake-sandbox".to_owned(),
            "fake-sandbox-id".to_owned(),
        );
        let mut state = test_state(temp.path().to_path_buf());
        state.sandbox_exec = Some(fake_sandbox_exec);
        state.resolved_sandbox = Some("fake-sandbox".to_owned());

        let router = super::build_dashboard_router(state);
        let body = json!({ "pinned": true }).to_string();
        let request = Request::builder()
            .uri("/dashboard/alpha/api/v1/knowledge/skills/rightx-oauth-debugging/pin")
            .method("PATCH")
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::AUTHORIZATION,
                format!("tma {}", signed_init_data(42)),
            )
            .body(Body::from(body))
            .expect("valid request");
        let response = router.oneshot(request).await.expect("router response");
        let status = response.status();

        // The crucial assertion: NOT 404. With the bug, the host check
        // would short-circuit to NOT_FOUND because the host SKILL.md is
        // missing. With the fix, the host check is skipped, the sandbox
        // probe is attempted, the fake mTLS connect fails, and we get a
        // sandbox/internal error instead.
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "sandbox pin path must bypass the host SKILL.md check; got 404",
        );
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        // Lifecycle row must remain unpinned — sandbox probe failed, no
        // pin should have been written.
        let row = right_lifecycle::get(&conn, "rightx-oauth-debugging")
            .await
            .unwrap()
            .unwrap();
        assert!(!row.pinned);
    }

    #[tokio::test]
    async fn dashboard_skill_pin_returns_404_for_missing_lifecycle_row() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "rightx-oauth-debugging");
        let _conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");

        let (status, body) = patch_json(
            "/dashboard/alpha/api/v1/knowledge/skills/rightx-oauth-debugging/pin",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
            json!({ "pinned": true }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "not_found");
    }

    #[tokio::test]
    async fn dashboard_skill_pin_rejects_non_rightx_skill() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "right-cron");
        let _conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");

        let (status, body) = patch_json(
            "/dashboard/alpha/api/v1/knowledge/skills/right-cron/pin",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
            json!({ "pinned": true }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_skill_name");
    }

    #[tokio::test]
    async fn dashboard_skill_pin_returns_409_for_non_curator_managed_rows() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");
        for (skill_name, created_by) in [
            ("rightx-foreground-owned", "foreground"),
            ("rightx-bundled-owned", "bundled"),
        ] {
            write_skill(temp.path(), skill_name);
            insert_lifecycle_row(&conn, skill_name, created_by, false).await;

            let (status, body) = patch_json(
                &format!("/dashboard/alpha/api/v1/knowledge/skills/{skill_name}/pin"),
                Some(signed_init_data(42)),
                temp.path().to_path_buf(),
                json!({ "pinned": true }),
            )
            .await;

            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(body["error"], "skill_not_curator_managed");
        }
    }

    #[tokio::test]
    async fn identity_route_returns_host_files_without_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("IDENTITY.md"), "# Identity\n").unwrap();
        std::fs::write(temp.path().join("SOUL.md"), "# Soul\n").unwrap();

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/identity",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["source"], "host");
        assert_eq!(body["files"][0]["name"], "IDENTITY.md");
        assert_eq!(body["files"][0]["content_preview"], "# Identity\n");
        assert_eq!(body["files"][1]["content_preview"], "# Soul\n");
        assert_eq!(body["files"][2]["exists"], false);
    }

    #[tokio::test]
    async fn identity_file_route_returns_host_file_without_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("IDENTITY.md"), "# Identity\n").unwrap();

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/identity/IDENTITY.md",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["file"]["name"], "IDENTITY.md");
        assert_eq!(body["file"]["content_preview"], "# Identity\n");
    }

    #[tokio::test]
    async fn identity_file_route_rejects_invalid_name() {
        let temp = tempfile::tempdir().expect("tempdir");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/identity/not-identity.md",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_identity_file");
    }

    #[tokio::test]
    async fn doctor_route_returns_grouped_checks_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/health/doctor",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["agent"], "alpha");
        assert!(body["pass"].is_array());
        assert!(body["warn"].is_array());
        assert!(body["fail"].is_array());
        assert!(
            body["pass_count"].as_i64().unwrap()
                + body["warn_count"].as_i64().unwrap()
                + body["fail_count"].as_i64().unwrap()
                > 0
        );
    }

    #[tokio::test]
    async fn sandbox_route_without_sandbox_returns_unavailable_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/health/sandbox",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["agent"], "alpha");
        assert_eq!(body["source"], "unavailable");
        assert!(body["disk"].is_null());
        assert!(body["memory"].is_null());
        assert_eq!(body["processes"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn learning_overview_rejects_missing_auth() {
        let temp = tempfile::tempdir().expect("tempdir");

        let status = get(
            "/dashboard/alpha/api/v1/learning/overview",
            None,
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn overview_on_missing_db_returns_500_without_creating_db() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("data.db");

        let status = get(
            "/dashboard/alpha/api/v1/overview",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!db_path.exists(), "dashboard read must not create data.db");
    }

    #[tokio::test]
    async fn run_detail_returns_not_found_for_unknown_run() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");

        let status = get(
            "/dashboard/alpha/api/v1/runs/missing-run",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn activity_run_detail_returns_not_found_for_unknown_run() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true)
            .await
            .expect("open migrated db");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/activity/runs/missing-run",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "not_found");
        assert_eq!(body["detail"], "run not found");
    }

    #[test]
    fn apply_sandbox_diagnosis_overrides_gateway_check_when_unavailable() {
        use right_agent::doctor::{CheckStatus, DoctorCheck};
        let mut checks = vec![
            DoctorCheck {
                name: "openshell-gateway".into(),
                status: CheckStatus::Pass,
                detail: "gateway healthy".into(),
                fix: None,
            },
            DoctorCheck {
                name: "tunnel".into(),
                status: CheckStatus::Pass,
                detail: "ok".into(),
                fix: None,
            },
        ];
        let health = crate::sandbox_runtime::SandboxHealth::Unavailable {
            diagnosis: std::sync::Arc::new(
                right_openshell::diagnosis::GatewayCause::DockerDown.diagnose(),
            ),
        };
        super::apply_sandbox_diagnosis(&mut checks, &health);
        let gw = checks
            .iter()
            .find(|c| c.name == "openshell-gateway")
            .unwrap();
        assert!(
            matches!(gw.status, CheckStatus::Fail),
            "expected Fail, got {:?}",
            gw.status
        );
        assert!(
            gw.detail.to_lowercase().contains("docker"),
            "detail should mention docker: {}",
            gw.detail
        );
        assert!(
            gw.fix.as_deref().unwrap().to_lowercase().contains("docker"),
            "fix should mention docker: {:?}",
            gw.fix
        );
        // Unrelated check is untouched.
        assert!(
            matches!(
                checks.iter().find(|c| c.name == "tunnel").unwrap().status,
                CheckStatus::Pass
            ),
            "tunnel check should be untouched"
        );
    }

    #[test]
    fn apply_sandbox_diagnosis_noop_when_ready() {
        use right_agent::doctor::{CheckStatus, DoctorCheck};
        let mut checks = vec![DoctorCheck {
            name: "openshell-gateway".into(),
            status: CheckStatus::Pass,
            detail: "gateway healthy".into(),
            fix: None,
        }];
        super::apply_sandbox_diagnosis(&mut checks, &crate::sandbox_runtime::SandboxHealth::Ready);
        assert!(
            matches!(checks[0].status, CheckStatus::Pass),
            "Ready health should not modify checks"
        );
    }

    async fn delete_req(
        path: &str,
        auth: Option<String>,
        agent_dir: std::path::PathBuf,
    ) -> StatusCode {
        let router = super::build_dashboard_router(test_state(agent_dir));
        let mut builder = Request::builder().uri(path).method("DELETE");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        router
            .oneshot(builder.body(Body::empty()).expect("valid request"))
            .await
            .expect("router response")
            .status()
    }

    #[tokio::test]
    async fn delete_cron_removes_spec_and_is_idempotent_404() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true)
            .await
            .expect("open db");
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('daily', '0 8 * * *', 'p', 1.0, '2026-05-20T00:00:00Z', '2026-05-20T00:00:00Z')",
            [],
        )
        .await
        .expect("insert cron spec");
        drop(conn);

        // Unauthenticated → 401 UNAUTHORIZED.
        assert_eq!(
            delete_req(
                "/dashboard/alpha/api/v1/crons/daily",
                None,
                dir.path().to_path_buf()
            )
            .await,
            StatusCode::UNAUTHORIZED
        );

        // Authenticated delete → 200.
        let auth = signed_init_data(42);
        assert_eq!(
            delete_req(
                "/dashboard/alpha/api/v1/crons/daily",
                Some(auth.clone()),
                dir.path().to_path_buf()
            )
            .await,
            StatusCode::OK
        );

        // Row is gone.
        let conn = right_db::open_connection(dir.path(), false)
            .await
            .expect("reopen db");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cron_specs WHERE job_name = 'daily'",
                [],
                |row| row.get(0),
            )
            .await
            .expect("count");
        assert_eq!(count, 0);
        drop(conn);

        // Second delete → 404.
        assert_eq!(
            delete_req(
                "/dashboard/alpha/api/v1/crons/daily",
                Some(auth),
                dir.path().to_path_buf()
            )
            .await,
            StatusCode::NOT_FOUND
        );
    }
}
