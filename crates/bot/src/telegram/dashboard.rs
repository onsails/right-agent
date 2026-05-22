use std::{collections::BTreeSet, path::PathBuf};

use axum::Json;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use right_dashboard::api_types::{
    ApiErrorBody, BootstrapResponse, DashboardFeatures, ForegroundActivity,
};
use right_dashboard::auth::{
    AuthError, DashboardUser, InitDataValidation, authorize_user, validate_init_data,
};
use right_dashboard::read_model::{
    activity::{ActivityOverviewInput, activity_overview, activity_run_detail},
    dashboard_overview::{DashboardOverviewInput, dashboard_overview},
    learning::{
        LearningOverviewInput, learning_overview, learning_report_detail, skill_lifecycle_overview,
    },
    learning_episodes::{LearningEpisodesInput, learning_episode_detail, learning_episodes},
    usage::{UsageOverviewInput, usage_overview},
};

mod health;
mod identity;
mod skills;

const REFRESH_INTERVAL_SECS: u64 = 5;
pub(super) const DASHBOARD_SANDBOX_TIMEOUT_SECS: u64 = 4;
const INIT_DATA_MAX_AGE_SECS: i64 = 86_400;
const MAX_LOG_LINES: usize = 80;

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
    pub allowlist: right_agent::agent::allowlist::AllowlistHandle,
    pub foreground: super::StopTokens,
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
            "/dashboard/{agent}/api/v1/knowledge/learning/episodes",
            get(handle_learning_episodes),
        )
        .route(
            "/dashboard/{agent}/api/v1/knowledge/learning/episodes/{episode_id}",
            get(handle_learning_episode_detail),
        )
        .route(
            "/dashboard/{agent}/api/v1/learning/reports/{report_id}",
            get(handle_learning_report_detail),
        )
        .route(
            "/dashboard/{agent}/api/v1/knowledge/learning/reports/{report_id}",
            get(handle_learning_report_detail),
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
            learning_evidence_snippets: true,
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

    let conn = match open_dashboard_read_connection(&state) {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };
    let input = ActivityOverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        refresh_interval_secs: REFRESH_INTERVAL_SECS,
        foreground: foreground_activity(&state),
    };

    match activity_overview(&conn, input) {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard activity overview query failed: {error:#}");
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

    let conn = match open_dashboard_read_connection(&state) {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };
    let input = DashboardOverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        foreground_active_count: foreground_activity(&state).len() as i64,
        sandbox: overview_sandbox_status(&state),
    };

    match dashboard_overview(&conn, input) {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard overview query failed: {error:#}");
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

    let conn = match open_dashboard_read_connection(&state) {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };

    match activity_run_detail(&conn, &run_id, MAX_LOG_LINES) {
        Ok(Some(response)) => Json(response).into_response(),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "not_found", Some("run not found")),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, run_id = %run_id, "dashboard run detail query failed: {error:#}");
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
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let conn = match open_dashboard_read_connection(&state) {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };
    let input = UsageOverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
    };

    match usage_overview(&conn, input) {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard usage query failed: {error:#}");
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

    let conn = match open_dashboard_read_connection(&state) {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };
    let input = LearningOverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        refresh_interval_secs: REFRESH_INTERVAL_SECS,
    };

    match learning_overview(&conn, input) {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard learning overview query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "learning_overview_failed",
                Some("failed to read learning overview"),
            )
        }
    }
}

async fn handle_learning_episodes(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let conn = match open_dashboard_read_connection(&state) {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };
    let input = LearningEpisodesInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
    };

    match learning_episodes(&conn, input) {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard learning episodes query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "learning_episodes_failed",
                Some("failed to read learning episodes"),
            )
        }
    }
}

async fn handle_learning_episode_detail(
    AxumPath((agent, episode_id)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let episode_id = match episode_id.parse::<i64>() {
        Ok(episode_id) => episode_id,
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_episode_id",
                Some("learning episode id must be an integer"),
            );
        }
    };

    let conn = match open_dashboard_read_connection(&state) {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };

    match learning_episode_detail(&conn, &state.agent_name, episode_id) {
        Ok(Some(response)) => Json(response).into_response(),
        Ok(None) => json_error(
            StatusCode::NOT_FOUND,
            "not_found",
            Some("learning episode not found"),
        ),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, episode_id, "dashboard learning episode query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "learning_episode_failed",
                Some("failed to read learning episode"),
            )
        }
    }
}

async fn handle_learning_report_detail(
    AxumPath((agent, report_id)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let report_id = match report_id.parse::<i64>() {
        Ok(report_id) => report_id,
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_report_id",
                Some("learning report id must be an integer"),
            );
        }
    };

    let conn = match open_dashboard_read_connection(&state) {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };

    match learning_report_detail(&conn, &state.agent_name, report_id) {
        Ok(Some(response)) => Json(response).into_response(),
        Ok(None) => json_error(
            StatusCode::NOT_FOUND,
            "not_found",
            Some("learning report not found"),
        ),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, report_id, "dashboard learning report query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "learning_report_failed",
                Some("failed to read learning report"),
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

    // Lifecycle data is host-side .usage.json; no DB connection needed.
    let _ = agent;
    let usage_path = state.agent_dir.join(".claude/skills/.usage.json");
    match skill_lifecycle_overview(&state.agent_name, &usage_path) {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!(
                agent = %state.agent_name,
                "dashboard skill_lifecycle read failed: {error:#}",
            );
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
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "skills_failed",
                Some("failed to read skills"),
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

    let checks = tokio::task::block_in_place(|| right_agent::doctor::run_doctor(&state.home));
    Json(health::doctor_response_from_checks(
        &state.agent_name,
        checks,
    ))
    .into_response()
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

fn open_dashboard_read_connection(
    state: &DashboardState,
) -> Result<rusqlite::Connection, DashboardRouteError> {
    right_db::open_connection_readonly(&state.agent_dir).map_err(|error| {
        tracing::error!(agent = %state.agent_name, "dashboard db open failed: {error:#}");
        DashboardRouteError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_open_failed",
            Some("failed to open dashboard database"),
        )
    })
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
        AuthError::MissingInitData
        | AuthError::MalformedInitData
        | AuthError::InvalidHash
        | AuthError::Expired
        | AuthError::MissingUser => DashboardRouteError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            Some("invalid Telegram Mini App authorization"),
        ),
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
            agent_dir,
            resolved_sandbox: None,
            sandbox_exec: None,
            allowlist,
            foreground: Arc::new(DashMap::new()),
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
        let user = json!({
            "id": user_id,
            "username": "tester",
            "first_name": "Test",
        })
        .to_string();
        let auth_date = chrono::Utc::now().timestamp().to_string();
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

    #[test]
    fn dashboard_url_strips_scheme_and_trailing_slash() {
        let url = super::dashboard_url("https://right.example.com/", "alpha").unwrap();

        assert_eq!(url.as_str(), "https://right.example.com/dashboard/alpha/");
    }

    #[test]
    fn dashboard_url_uses_agent_path() {
        let url = super::dashboard_url("right.example.com", "bot-one").unwrap();

        assert_eq!(url.path(), "/dashboard/bot-one/");
    }

    #[test]
    fn dashboard_url_rejects_hostname_with_path() {
        let err = super::dashboard_url("right.example.com/some-path", "alpha")
            .expect_err("hostname with path must be rejected");

        assert!(matches!(err, super::DashboardUrlError::HostnameNotBare(_)));
    }

    #[test]
    fn dashboard_url_rejects_hostname_with_path_after_scheme() {
        let err = super::dashboard_url("https://right.example.com/extra", "alpha")
            .expect_err("hostname with path must be rejected");

        assert!(matches!(err, super::DashboardUrlError::HostnameNotBare(_)));
    }

    #[test]
    fn dashboard_url_rejects_hostname_with_query() {
        let err = super::dashboard_url("right.example.com/?token=abc", "alpha")
            .expect_err("hostname with query must be rejected");

        assert!(matches!(err, super::DashboardUrlError::HostnameNotBare(_)));
    }

    #[test]
    fn dashboard_url_rejects_hostname_with_fragment() {
        let err = super::dashboard_url("right.example.com/#frag", "alpha")
            .expect_err("hostname with fragment must be rejected");

        assert!(matches!(err, super::DashboardUrlError::HostnameNotBare(_)));
    }

    #[test]
    fn dashboard_url_rejects_hostname_with_userinfo() {
        let err = super::dashboard_url("user@right.example.com", "alpha")
            .expect_err("hostname with userinfo must be rejected");

        assert!(matches!(err, super::DashboardUrlError::HostnameNotBare(_)));
    }

    #[test]
    fn dashboard_url_accepts_scheme_prefix() {
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
        let _conn = right_db::open_connection(temp.path(), true).expect("open migrated db");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/bootstrap",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["features"]["learning_metrics"], true);
        assert_eq!(body["features"]["learning_evidence_snippets"], true);
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
    async fn overview_returns_data_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true).expect("open migrated db");

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
    }

    #[tokio::test]
    async fn activity_overview_returns_current_cron_payload() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true).expect("open migrated db");
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
        .expect("insert cron spec");

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
        assert_eq!(body["active"]["foreground"].as_array().unwrap().len(), 0);
        assert_eq!(body["active"]["background"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn usage_returns_structured_windows_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true).expect("open migrated db");
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
        .unwrap();

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/usage",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["agent"], "alpha");
        assert!(body["windows"].is_array());
        assert_eq!(body["windows"][0]["key"], "today");
    }

    #[tokio::test]
    async fn learning_overview_returns_data_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true).expect("open migrated db");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/learning/overview",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["capabilities"]["learning_metrics"], true);
    }

    #[tokio::test]
    async fn learning_episodes_returns_data_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true).expect("open migrated db");
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, selector_output_json,
                ready_after, last_evidence_at, created_at, updated_at
             ) VALUES (
                1, 'alpha', 'foreground_thread', 'learning_signal', 'inv:inv-1',
                'reviewed', '[]', '[]', '{}', '2026-05-20T10:01:30Z',
                '2026-05-20T10:01:00Z', '2026-05-20T10:00:00Z',
                '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/learning/episodes",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["agent"], "alpha");
        assert_eq!(body["episodes"][0]["id"], 1);
        assert_eq!(body["episodes"][0]["status"], "reviewed");
    }

    #[tokio::test]
    async fn learning_episode_detail_returns_data_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true).expect("open migrated db");
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, selector_model,
                selector_output_json, ready_after, last_evidence_at, created_at,
                updated_at
             ) VALUES (
                1, 'alpha', 'foreground_thread', 'learning_signal', 'inv:inv-1',
                'reviewed', '[\"msg:1\"]', '[]', 'claude-sonnet-4-6', '{}',
                '2026-05-20T10:01:30Z', '2026-05-20T10:01:00Z',
                '2026-05-20T10:00:00Z', '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/learning/episodes/1",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["episode"]["id"], 1);
        assert_eq!(body["selector"]["model"], "claude-sonnet-4-6");
        assert_eq!(body["selector"]["selected_message_refs"][0], "msg:1");
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

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/skills",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["source"], "host");
        assert_eq!(body["groups"]["core"][0]["name"], "right-cron");
        assert_eq!(
            body["groups"]["learned"][0]["name"],
            "rightx-oauth-debugging"
        );
        assert_eq!(body["groups"]["other"][0]["name"], "hub-browser");
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

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/knowledge/skills/rightx-oauth-debugging",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["skill"]["name"], "rightx-oauth-debugging");
        assert_eq!(body["skill"]["group"], "learned");
        assert!(
            body["content_preview"]
                .as_str()
                .unwrap()
                .contains("# OAuth")
        );
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
        let _conn = right_db::open_connection(temp.path(), true).expect("open migrated db");

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
        let _conn = right_db::open_connection(temp.path(), true).expect("open migrated db");

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

    #[tokio::test]
    async fn learning_report_detail_returns_not_found_for_unknown_report() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true).expect("open migrated db");

        let status = get(
            "/dashboard/alpha/api/v1/learning/reports/999",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn learning_report_detail_rejects_malformed_id_after_auth() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true).expect("open migrated db");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/learning/reports/not-a-number",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_report_id");
    }

    #[tokio::test]
    async fn learning_report_detail_rejects_missing_auth_before_malformed_id() {
        let temp = tempfile::tempdir().expect("tempdir");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/learning/reports/not-a-number",
            None,
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "unauthorized");
    }

    #[tokio::test]
    async fn learning_report_detail_returns_data_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true).expect("open migrated db");
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                1, 'alpha', 'inv-1', 'effort_threshold', 'nothing_to_learn',
                'medium', '[]',
                '{\"status\":\"nothing_to_learn\",\"confidence\":\"medium\",\"evidence_refs\":[],\"user_notice\":null}',
                '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/learning/reports/1",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["report"]["id"], 1);
        assert_eq!(body["report"]["status"], "nothing_to_learn");
    }
}
