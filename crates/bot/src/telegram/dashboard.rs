use std::collections::BTreeSet;
use std::path::PathBuf;

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
use right_dashboard::read_model::{OverviewInput, overview, run_detail};

const REFRESH_INTERVAL_SECS: u64 = 5;
const INIT_DATA_MAX_AGE_SECS: i64 = 86_400;
const MAX_LOG_LINES: usize = 80;

pub(crate) fn dashboard_url(hostname: &str, agent_name: &str) -> Result<url::Url, url::ParseError> {
    url::Url::parse(&format!(
        "https://{}/dashboard/{}/",
        hostname
            .trim_end_matches('/')
            .trim_start_matches("https://")
            .trim_start_matches("http://"),
        agent_name
    ))
}

#[derive(Clone)]
pub(crate) struct DashboardState {
    pub agent_name: String,
    pub bot_token: String,
    pub agent_dir: PathBuf,
    pub allowlist: right_agent::agent::allowlist::AllowlistHandle,
    pub foreground: super::StopTokens,
}

pub(crate) fn build_dashboard_router(state: DashboardState) -> axum::Router {
    axum::Router::new()
        .route("/dashboard/{agent}/", get(handle_static_index))
        .route("/dashboard/{agent}/api/v1/bootstrap", get(handle_bootstrap))
        .route("/dashboard/{agent}/api/v1/overview", get(handle_overview))
        .route(
            "/dashboard/{agent}/api/v1/runs/{run_id}",
            get(handle_run_detail),
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
        },
    })
    .into_response()
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
    let input = OverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        refresh_interval_secs: REFRESH_INTERVAL_SECS,
        foreground: foreground_activity(&state),
    };

    match overview(&conn, input) {
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

async fn handle_run_detail(
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

    match run_detail(&conn, &run_id, MAX_LOG_LINES) {
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
    let db_path = state.agent_dir.join("data.db");
    if !db_path.exists() {
        tracing::error!(
            agent = %state.agent_name,
            path = %db_path.display(),
            "dashboard database missing"
        );
        return Err(DashboardRouteError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_open_failed",
            Some("dashboard database does not exist"),
        ));
    }

    right_db::open_connection(&state.agent_dir, false).map_err(|error| {
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
            agent_dir,
            allowlist,
            foreground: Arc::new(DashMap::new()),
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

    #[tokio::test]
    async fn static_index_loads_without_auth() {
        let temp = tempfile::tempdir().expect("tempdir");

        let status = get("/dashboard/alpha/", None, temp.path().to_path_buf()).await;

        assert_eq!(status, StatusCode::OK);
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

        let status = get(
            "/dashboard/alpha/api/v1/overview",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
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
}
