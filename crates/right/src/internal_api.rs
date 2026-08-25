//! Internal REST API served on a Unix domain socket for bot→aggregator IPC.
//!
//! Exposes endpoints for MCP server management and foreground progress plumbing
//! that are accessible only to the Telegram bot process, not to agents.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use right_mcp::credentials::{self, CredentialError};
use right_mcp::proxy::{AuthMethod, BackendStatus, ProxyBackend};
use serde::{Deserialize, Serialize};

use crate::aggregator::{
    AgentInfo, AgentTokenMap, ReconnectManagers, RefreshSenders, ToolDispatcher,
};
use right_mcp::internal_client::{
    DbReadyRequest, DbReadyResponse, DbReadyState, HttpHeaderInput, ProgressInvocationKindDto,
    ProgressRegisterRequest, ProgressRegisterResponse, ProgressUnregisterRequest,
    ProgressUnregisterResponse,
};
use right_mcp::refresh::{OAuthServerState, RefreshMessage};

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct McpAddRequest {
    pub agent: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub auth_type: Option<String>,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default)]
    pub headers: Vec<HttpHeaderInput>,
}

#[derive(Serialize)]
pub(crate) struct McpAddResponse {
    pub tools_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct McpRemoveRequest {
    pub agent: String,
    pub name: String,
}

#[derive(Serialize)]
pub(crate) struct McpRemoveResponse {
    pub removed: bool,
}

#[derive(Deserialize)]
pub(crate) struct McpSetHeadersRequest {
    pub agent: String,
    pub name: String,
    #[serde(default)]
    pub headers: Vec<HttpHeaderInput>,
}

#[derive(Serialize)]
pub(crate) struct McpSetHeadersResponse {
    pub ok: bool,
}

#[derive(Deserialize)]
pub(crate) struct SetTokenRequest {
    pub agent: String,
    pub server: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub resource: String,
}

#[derive(Serialize)]
pub(crate) struct SetTokenResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct McpListRequest {
    pub agent: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct McpListResponse {
    pub servers: Vec<McpServerStatus>,
}

#[derive(Debug, Serialize)]
pub(crate) struct McpServerStatus {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub status: String,
    pub tool_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub header_names: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connect_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct McpInstructionsRequest {
    pub agent: String,
}

#[derive(Serialize)]
pub(crate) struct McpInstructionsResponse {
    pub instructions: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct InternalRouterDeps {
    pub(crate) dispatcher: Arc<ToolDispatcher>,
    pub(crate) refresh_senders: RefreshSenders,
    pub(crate) reconnect_managers: ReconnectManagers,
    pub(crate) token_map: AgentTokenMap,
    pub(crate) db_owners: crate::db_owner::DbOwnerRegistry,
    pub(crate) token_map_path: PathBuf,
    pub(crate) agents_dir: PathBuf,
    pub(crate) providers: std::sync::Arc<right_providers::ProviderStore>,
}

#[derive(Clone)]
pub(crate) struct InternalState {
    dispatcher: Arc<ToolDispatcher>,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
    token_map: AgentTokenMap,
    pub(crate) db_owners: crate::db_owner::DbOwnerRegistry,
    token_map_path: PathBuf,
    pub(crate) agents_dir: PathBuf,
    /// Right's provider credential store — the single authority for provider
    /// records and credentials, replacing the retired OpenShell provider
    /// gateway. Never exposes a credential value on a read path.
    pub(crate) providers: std::sync::Arc<right_providers::ProviderStore>,
    reload_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    // Per-agent provider mutation serialization lives in ProviderStore. Its
    // advisory file lock is authoritative across processes; the in-process
    // mutex is only the cheap first queue for same-process callers.
}

impl InternalState {
    pub(crate) fn new(deps: InternalRouterDeps) -> Self {
        Self {
            dispatcher: deps.dispatcher,
            refresh_senders: deps.refresh_senders,
            reconnect_managers: deps.reconnect_managers,
            db_owners: deps.db_owners,
            token_map: deps.token_map,
            token_map_path: deps.token_map_path,
            agents_dir: deps.agents_dir,
            providers: deps.providers,
            reload_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

/// Open (creating if absent) the provider credential store at
/// `<home>/providers.db`. The store is the single authority for provider
/// state, so serving the internal API without it would answer
/// `/provider-list` with lies — the error propagates instead of degrading.
/// FAIL FAST per AGENTS.rust.md §2.
pub(crate) async fn open_provider_store(
    home: &std::path::Path,
) -> miette::Result<std::sync::Arc<right_providers::ProviderStore>> {
    let store = right_providers::ProviderStore::open(home)
        .await
        .map_err(|e| miette::miette!("cannot open providers.db under {}: {e:#}", home.display()))?;
    Ok(std::sync::Arc::new(store))
}

async fn cancel_mcp_refresh(
    state: &InternalState,
    agent: &str,
    server: &str,
) -> Result<(), axum::response::Response> {
    let Some(sender) = state.refresh_senders.get(agent).map(|entry| entry.clone()) else {
        return Ok(());
    };
    let (completion, completed) = tokio::sync::oneshot::channel();
    sender
        .send(RefreshMessage::RemoveServerAndWait {
            server_name: server.to_owned(),
            completion,
        })
        .await
        .map_err(|error| {
            internal_error(format!("cancel MCP refresh for '{server}': {error:#}")).into_response()
        })?;
    completed.await.map_err(|error| {
        internal_error(format!(
            "await MCP refresh cancellation for '{server}': {error:#}"
        ))
        .into_response()
    })
}

pub(crate) fn internal_router(deps: InternalRouterDeps) -> Router {
    let state = InternalState::new(deps);
    Router::new()
        .route("/mcp-add", post(handle_mcp_add))
        .route("/mcp-remove", post(handle_mcp_remove))
        .route("/mcp-set-headers", post(handle_mcp_set_headers))
        .route("/db/ready", post(handle_db_ready))
        .route("/set-token", post(handle_set_token))
        .route("/mcp-list", post(handle_mcp_list))
        .route("/mcp-instructions", post(handle_mcp_instructions))
        .route("/reload", post(handle_reload))
        .route("/progress/register", post(handle_progress_register))
        .route("/progress/unregister", post(handle_progress_unregister))
        .route(
            "/provider-list",
            post(crate::internal_api_providers::handle_provider_list),
        )
        .route(
            "/provider-types",
            post(crate::internal_api_providers::handle_provider_types),
        )
        .route(
            "/provider-create",
            post(crate::internal_api_providers::handle_provider_create),
        )
        .route(
            "/provider-rotate",
            post(crate::internal_api_providers::handle_provider_rotate),
        )
        .route(
            "/provider-config-update",
            post(crate::internal_api_providers::handle_provider_config_update),
        )
        .route(
            "/provider-remove",
            post(crate::internal_api_providers::handle_provider_remove),
        )
        .route(
            "/provider-peers",
            post(crate::internal_api_providers::handle_provider_peers),
        )
        .route(
            "/provider-share",
            post(crate::internal_api_providers::handle_provider_share),
        )
        .route(
            "/provider-unshare",
            post(crate::internal_api_providers::handle_provider_unshare),
        )
        .merge(crate::internal_api_db::router())
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Error helpers
// ---------------------------------------------------------------------------

fn error_response(
    status: StatusCode,
    error: impl Into<String>,
    detail: Option<String>,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: error.into(),
            detail,
        }),
    )
}

fn validation_error(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::BAD_REQUEST, msg, None)
}

fn not_found(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::NOT_FOUND, msg, None)
}

fn internal_error(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, msg, None)
}

fn plain_http_warning(url: &str) -> Option<String> {
    match reqwest::Url::parse(url) {
        Ok(parsed) if parsed.scheme() == "http" => {
            Some("Plain HTTP: trusted/encrypted networks only.".to_string())
        }
        _ => None,
    }
}

fn header_inputs_to_secrets(
    headers: Vec<HttpHeaderInput>,
) -> Result<Vec<credentials::HttpHeaderSecret>, CredentialError> {
    headers
        .into_iter()
        .map(|header| credentials::HttpHeaderSecret::new(header.name, header.value))
        .collect()
}

fn display_header_name(name: String) -> String {
    if name == "authorization" {
        "Authorization".to_string()
    } else {
        name
    }
}

const OAUTH_RECONNECT_MAX_ATTEMPTS: usize = 3;

fn oauth_reconnect_retry_delay(attempt: usize) -> std::time::Duration {
    #[cfg(test)]
    {
        let _ = attempt;
        std::time::Duration::from_millis(10)
    }

    #[cfg(not(test))]
    {
        std::time::Duration::from_secs(match attempt {
            1 => 1,
            2 => 2,
            _ => 4,
        })
    }
}

fn oauth_reconnect_http_client() -> reqwest::Client {
    // PublicOnly (not AllowPrivate): this reconnect runs only after a completed
    // OAuth flow, and OAuth is unsupported for local servers — a private base
    // URL's discovered token_endpoint is rejected at discovery, so this path
    // never serves a private/LAN server.
    right_mcp::ssrf::hardened_client_builder(right_mcp::ssrf::NetworkPolicy::PublicOnly)
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        // SSRF-hardened builder uses only static config (no proxy, no redirect,
        // static DNS resolver). The build step is infallible in practice — a
        // failure here would mean a broken rustls/TLS setup, not bad runtime
        // input, so propagating up the OAuth reconnect path adds no value.
        .unwrap_or_else(|e| panic!("reqwest builder failed: {e:#}"))
}

async fn reconnect_after_oauth_update(
    server_name: &str,
    handle: Arc<ProxyBackend>,
) -> Result<Option<String>, right_mcp::proxy::ProxyError> {
    let mut last_error = None;

    for attempt in 1..=OAUTH_RECONNECT_MAX_ATTEMPTS {
        let client = oauth_reconnect_http_client();
        match handle.connect_staged(client).await {
            Ok(instructions) => {
                tracing::info!(
                    server = %server_name,
                    attempt,
                    "reconnected after OAuth token update",
                );
                return Ok(instructions);
            }
            Err(e) => {
                let detail = format!("{e:#}");
                tracing::warn!(
                    server = %server_name,
                    attempt,
                    max_attempts = OAUTH_RECONNECT_MAX_ATTEMPTS,
                    err = %detail,
                    "reconnect after OAuth failed",
                );
                if right_mcp::proxy::is_upstream_auth_error(&detail) {
                    return Err(e);
                }
                last_error = Some(e);
            }
        }

        if attempt < OAUTH_RECONNECT_MAX_ATTEMPTS {
            tokio::time::sleep(oauth_reconnect_retry_delay(attempt)).await;
        }
    }

    Err(last_error.expect("at least one OAuth reconnect attempt should run"))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_db_ready(
    State(state): State<InternalState>,
    Json(request): Json<DbReadyRequest>,
) -> Json<DbReadyResponse> {
    let state = match state.db_owners.state(&request.agent).await {
        Some(crate::db_owner::DbOwnerState::Starting) => DbReadyState::Starting,
        Some(crate::db_owner::DbOwnerState::Ready) => DbReadyState::Ready,
        Some(crate::db_owner::DbOwnerState::Draining) => DbReadyState::Draining,
        Some(crate::db_owner::DbOwnerState::Failed) => DbReadyState::Failed,
        None => DbReadyState::Unavailable,
    };
    Json(DbReadyResponse {
        agent: request.agent,
        ready: state == DbReadyState::Ready,
        state,
    })
}

async fn database_owner(
    state: &InternalState,
    agent: &str,
) -> Result<Arc<crate::db_owner::AgentDbOwner>, axum::response::Response> {
    state.db_owners.get(agent).await.map_err(|error| {
        tracing::error!(agent, error = %format!("{error:#}"), "database owner unavailable");
        internal_error("database owner unavailable").into_response()
    })
}

async fn handle_mcp_add(
    State(state): State<InternalState>,
    Json(req): Json<McpAddRequest>,
) -> axum::response::Response {
    let owner = match state.db_owners.get(&req.agent).await {
        Ok(owner) => owner,
        Err(crate::db_owner::DbOwnerError::NotFound { .. }) => {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        }
        Err(error) => {
            return internal_error(format!("database owner unavailable: {error:#}"))
                .into_response();
        }
    };
    let persistence: Arc<dyn right_mcp::persistence::McpPersistence> = Arc::new(
        crate::mcp_persistence::OwnerMcpPersistence::new(Arc::clone(&owner)),
    );
    let dispatcher = &state.dispatcher;
    // Validate name
    if let Err(e) = credentials::validate_server_name(&req.name) {
        return validation_error(format!("{e}")).into_response();
    }

    // Validate URL
    if let Err(e) = credentials::validate_server_url(&req.url) {
        return validation_error(format!("{e}")).into_response();
    }

    let header_secrets = match header_inputs_to_secrets(req.headers.clone()) {
        Ok(headers) => headers,
        Err(e) => return validation_error(format!("{e}")).into_response(),
    };
    if req.auth_type.as_deref() == Some("headers") && header_secrets.is_empty() {
        return validation_error("headers auth requires at least one header").into_response();
    }

    let auth_method = AuthMethod::from_db_with_headers(
        req.auth_type.as_deref(),
        req.auth_header.as_deref(),
        header_secrets.clone(),
    );
    let auth_token = if req.auth_type.as_deref() == Some("headers") {
        None
    } else {
        req.auth_token.clone()
    };
    if let Err(response) = cancel_mcp_refresh(&state, &req.agent, &req.name).await {
        return response;
    }
    let persisted_auth = if req.auth_type.as_deref() == Some("headers") {
        credentials::McpServerAuth::Headers(header_secrets)
    } else if let Some(auth_type) = req.auth_type.clone() {
        credentials::McpServerAuth::Legacy {
            auth_type,
            auth_header: req.auth_header.clone(),
            auth_token: req.auth_token.clone(),
        }
    } else {
        credentials::McpServerAuth::None
    };
    let _mutation_guard = owner.lock_mcp_mutation(&req.name).await;
    let http_warning = plain_http_warning(&req.url);

    // Get agent directory and proxies, then drop the registry guard before DB await.
    let proxies_lock = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        Arc::clone(&registry.proxies)
    };
    if let Some(manager) = state.reconnect_managers.get(&req.agent) {
        manager.lock().await.cancel(&req.name);
    }
    let previous = match owner
        .replace_mcp_server(req.name.clone(), req.url.clone(), persisted_auth)
        .await
    {
        Ok(previous) => previous,
        Err(error) => {
            return internal_error(format!("MCP persistence failed: {error:#}")).into_response();
        }
    };

    // Create ProxyBackend with the resolved auth method and optional token
    let token = Arc::new(tokio::sync::RwLock::new(auth_token.clone()));
    let backend = ProxyBackend::new(
        req.name.clone(),
        persistence,
        req.url.clone(),
        token,
        auth_method,
    );
    let handle = Arc::new(backend);

    // Skip connection for OAuth servers without a token — they need dashboard OAuth first.
    let skip_connect = req.auth_type.as_deref() == Some("oauth") && req.auth_token.is_none();

    if skip_connect {
        tracing::info!(server = %req.name, "mcp-add: OAuth server registered (skipping connect — no token yet)");
        {
            let mut proxies = proxies_lock.write().await;
            proxies.insert(req.name.clone(), Arc::clone(&handle));
        }
        return (
            StatusCode::OK,
            Json(McpAddResponse {
                tools_count: 0,
                excluded: Vec::new(),
                warning: http_warning,
            }),
        )
            .into_response();
    }

    // Attempt connection (with timeout to prevent hanging on slow upstreams)
    tracing::info!(server = %req.name, url = %req.url, "mcp-add: connecting to upstream MCP server");
    let connect_client = match right_mcp::ssrf::hardened_client_builder(
        right_mcp::ssrf::NetworkPolicy::AllowPrivate,
    )
    .connect_timeout(std::time::Duration::from_secs(10))
    .timeout(std::time::Duration::from_secs(30))
    .build()
    {
        Ok(client) => client,
        Err(error) => {
            let original = format!("reqwest client build: {error:#}");
            return rollback_mcp_add(
                &owner,
                &req.name,
                previous,
                original,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await;
        }
    };
    match handle.connect_staged(connect_client).await {
        Ok(instructions) => {
            if let Err(error) = owner
                .update_mcp_instructions(req.name.clone(), instructions)
                .await
            {
                return rollback_mcp_add(
                    &owner,
                    &req.name,
                    previous,
                    format!("persist replacement instructions: {error:#}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
                .await;
            }
            tracing::info!(server = %req.name, "mcp-add: upstream connection successful");
            let tools_count = handle.try_tools().map(|tools| tools.len()).unwrap_or(0);
            {
                let mut proxies = proxies_lock.write().await;
                proxies.insert(req.name.clone(), Arc::clone(&handle));
            }
            (
                StatusCode::OK,
                Json(McpAddResponse {
                    tools_count,
                    excluded: Vec::new(),
                    warning: http_warning,
                }),
            )
                .into_response()
        }
        Err(error) => {
            let safe_error = right_mcp::proxy::redact_query_strings(&format!("{error:#}"));
            tracing::warn!(
                server = %req.name,
                err = %safe_error,
                "mcp-add: upstream connection failed"
            );
            rollback_mcp_add(
                &owner,
                &req.name,
                previous,
                format!("connection failed: {safe_error}"),
                StatusCode::BAD_GATEWAY,
            )
            .await
        }
    }
}

async fn rollback_mcp_add(
    owner: &Arc<crate::db_owner::AgentDbOwner>,
    server_name: &str,
    previous: Option<credentials::McpServerSnapshot>,
    original_error: String,
    original_status: StatusCode,
) -> axum::response::Response {
    let original_error = right_mcp::proxy::redact_query_strings(&original_error);
    match owner
        .rollback_mcp_server_replacement(server_name.to_owned(), previous)
        .await
    {
        Ok(()) => error_response(original_status, original_error, None).into_response(),
        Err(rollback_error) => {
            let rollback_detail = format!("{rollback_error:#}");
            tracing::error!(
                server = %server_name,
                original_error = %original_error,
                rollback_error = %rollback_detail,
                "mcp-add failed and durable rollback failed"
            );
            internal_error(format!(
                "{original_error}; rollback failed: {rollback_detail}"
            ))
            .into_response()
        }
    }
}

async fn handle_mcp_remove(
    State(state): State<InternalState>,
    Json(req): Json<McpRemoveRequest>,
) -> axum::response::Response {
    let dispatcher = &state.dispatcher;
    if right_mcp::is_protected_server_name(&req.name) {
        return validation_error(format!(
            "'{}' is a protected server and cannot be removed",
            req.name
        ))
        .into_response();
    }

    // Clone proxies Arc and backend, then drop the DashMap guard before DB await.
    let proxies_lock = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        Arc::clone(&registry.proxies)
    };
    let owner = match database_owner(&state, &req.agent).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };

    if let Some(manager) = state.reconnect_managers.get(&req.agent) {
        manager.lock().await.cancel(&req.name);
    }
    if let Err(response) = cancel_mcp_refresh(&state, &req.agent, &req.name).await {
        return response;
    }
    let _mutation_guard = owner.lock_mcp_mutation(&req.name).await;
    // Remove from proxies (in-memory).
    let removed_from_proxies = {
        let mut proxies = proxies_lock.write().await;
        proxies.remove(&req.name).is_some()
    };

    // Remove from SQLite regardless of in-memory presence. DB rows can
    // outlive the in-memory map (e.g. after an aggregator restart where the
    // proxy failed to reconnect), and leaving them orphans the dashboard.
    let server_name = req.name.clone();
    let removed_from_db = match owner
        .local_operation(move |conn| {
            Box::pin(async move {
                match credentials::db_remove_server(conn, &server_name).await {
                    Ok(()) => Ok(true),
                    Err(CredentialError::ServerNotFound(_)) => Ok(false),
                    Err(error) => Err(error.into()),
                }
            })
        })
        .await
    {
        Ok(removed) => removed,
        Err(error) => {
            return internal_error(format!("db_remove_server: {error:#}")).into_response();
        }
    };

    if !removed_from_proxies && !removed_from_db {
        return not_found(format!(
            "server '{}' not found for agent '{}'",
            req.name, req.agent
        ))
        .into_response();
    }

    (StatusCode::OK, Json(McpRemoveResponse { removed: true })).into_response()
}

async fn handle_mcp_set_headers(
    State(state): State<InternalState>,
    Json(req): Json<McpSetHeadersRequest>,
) -> axum::response::Response {
    if right_mcp::is_protected_server_name(&req.name) {
        return validation_error("protected MCP server cannot be modified").into_response();
    }

    let header_secrets = match header_inputs_to_secrets(req.headers) {
        Ok(headers) => headers,
        Err(e) => return validation_error(format!("{e}")).into_response(),
    };
    if header_secrets.is_empty() {
        return validation_error("headers auth requires at least one header").into_response();
    }

    let proxies_lock = {
        let Some(registry) = state.dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        Arc::clone(&registry.proxies)
    };
    let owner = match database_owner(&state, &req.agent).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    if let Some(manager) = state.reconnect_managers.get(&req.agent) {
        manager.lock().await.cancel(&req.name);
    }
    if let Err(response) = cancel_mcp_refresh(&state, &req.agent, &req.name).await {
        return response;
    }
    let _mutation_guard = owner.lock_mcp_mutation(&req.name).await;

    let existing = {
        let proxies = proxies_lock.read().await;
        let Some(existing) = proxies.get(&req.name) else {
            return not_found(format!("server '{}' not found", req.name)).into_response();
        };
        Arc::clone(existing)
    };

    // Build the background-connect client before any mutation: it needs no
    // inputs from persistence, and a build failure must return 500 with zero
    // state changes — never after the credential is already persisted.
    let connect_client = match right_mcp::ssrf::hardened_client_builder(
        right_mcp::ssrf::NetworkPolicy::AllowPrivate,
    )
    .connect_timeout(std::time::Duration::from_secs(5))
    .timeout(std::time::Duration::from_secs(10))
    .build()
    {
        Ok(client) => client,
        Err(e) => {
            return internal_error(format!("reqwest client build: {e:#}")).into_response();
        }
    };

    // Persist next — a credential write does not depend on upstream reachability.
    let server_name = req.name.clone();
    let persisted_headers = header_secrets.clone();
    if let Err(error) = owner
        .local_operation(move |conn| {
            Box::pin(async move {
                credentials::db_set_http_headers(conn, &server_name, &persisted_headers)
                    .await
                    .map_err(Into::into)
            })
        })
        .await
    {
        return internal_error(format!("db_set_http_headers: {error:#}")).into_response();
    }

    // Swap in a fresh backend carrying the new headers. It starts Unreachable;
    // the reconciler re-probes it (with the new headers) until it connects.
    let replacement = Arc::new(ProxyBackend::new(
        req.name.clone(),
        existing.persistence(),
        existing.url().to_string(),
        Arc::new(tokio::sync::RwLock::new(None)),
        AuthMethod::Headers(header_secrets),
    ));
    {
        let mut proxies = proxies_lock.write().await;
        proxies.insert(req.name.clone(), Arc::clone(&replacement));
    }

    // Best-effort connect in the background: connect() self-logs and records the
    // outcome, so the live status reflects reality without blocking this request.
    tokio::spawn(async move {
        let _ = replacement.connect(connect_client).await;
    });

    (StatusCode::OK, Json(McpSetHeadersResponse { ok: true })).into_response()
}

async fn handle_progress_register(
    State(state): State<InternalState>,
    Json(req): Json<ProgressRegisterRequest>,
) -> axum::response::Response {
    let (progress, bot_socket_path) = {
        let Some(registry) = state.dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        (
            registry.right.progress_registry(),
            registry.agent_dir.join("bot.sock"),
        )
    };

    let kind = match req.kind {
        ProgressInvocationKindDto::Foreground => {
            crate::progress::ProgressInvocationKind::Foreground
        }
        ProgressInvocationKindDto::BackgroundReview => {
            crate::progress::ProgressInvocationKind::BackgroundReview
        }
        ProgressInvocationKindDto::ProbeWriter => {
            crate::progress::ProgressInvocationKind::ProbeWriter
        }
        ProgressInvocationKindDto::Curator => crate::progress::ProgressInvocationKind::Curator,
        ProgressInvocationKindDto::Cron => crate::progress::ProgressInvocationKind::Cron,
    };
    let conversation_scope = match (req.chat_id, req.thread_id) {
        (Some(chat_id), Some(thread_id)) => {
            Some(crate::progress::ConversationScope { chat_id, thread_id })
        }
        _ => None,
    };
    progress
        .register(crate::progress::ProgressRegistration {
            invocation_id: req.invocation_id,
            kind,
            bot_socket_path,
            bot_send_token: req.bot_send_token,
            conversation_scope,
        })
        .await;

    (StatusCode::OK, Json(ProgressRegisterResponse { ok: true })).into_response()
}

async fn handle_progress_unregister(
    State(state): State<InternalState>,
    Json(req): Json<ProgressUnregisterRequest>,
) -> axum::response::Response {
    let progress = {
        let Some(registry) = state.dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        registry.right.progress_registry()
    };

    progress.unregister(&req.invocation_id).await;

    (
        StatusCode::OK,
        Json(ProgressUnregisterResponse { ok: true }),
    )
        .into_response()
}

async fn handle_set_token(
    State(state): State<InternalState>,
    Json(req): Json<SetTokenRequest>,
) -> axum::response::Response {
    let dispatcher = &state.dispatcher;
    // Extract what we need from DashMap guard (scope guard before await)
    let proxies_lock = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        Arc::clone(&registry.proxies)
    };
    let owner = match database_owner(&state, &req.agent).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };

    // Find proxy handle
    let handle = {
        let proxies = proxies_lock.read().await;
        proxies.get(&req.server).cloned()
    };

    let Some(handle) = handle else {
        return not_found(format!(
            "server '{}' not found for agent '{}'",
            req.server, req.agent
        ))
        .into_response();
    };
    let oauth_resource = if req.resource.trim().is_empty() {
        right_mcp::oauth::canonical_resource_uri(handle.url())
            .unwrap_or_else(|_| handle.url().to_string())
    } else {
        req.resource.clone()
    };
    if !right_mcp::ssrf::is_public_http_url(&req.token_endpoint) {
        return validation_error("OAuth token endpoint must be a public HTTP(S) URL")
            .into_response();
    }
    if let Err(response) = cancel_mcp_refresh(&state, &req.agent, &req.server).await {
        return response;
    }
    let _mutation_guard = owner.lock_mcp_mutation(&req.server).await;

    // Update the token in the shared Arc<RwLock<Option<String>>>
    {
        let mut token_guard = handle.token().write().await;
        *token_guard = Some(req.access_token.clone());
    }

    // Persist OAuth state to SQLite
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(req.expires_in as i64);
    let expires_at_str = expires_at.to_rfc3339();
    {
        if !dispatcher.agents.contains_key(&req.agent) {
            return not_found("agent_not_found").into_response();
        }
        let server = req.server.clone();
        let access = req.access_token.clone();
        let refresh = req.refresh_token.clone();
        let endpoint = req.token_endpoint.clone();
        let client_id = req.client_id.clone();
        let client_secret = req.client_secret.clone();
        let expires = expires_at_str.clone();
        let resource = oauth_resource.clone();
        if let Err(error) = owner
            .local_operation(move |conn| {
                Box::pin(async move {
                    right_mcp::credentials::db_set_oauth_state(
                        conn,
                        &server,
                        &access,
                        Some(&refresh),
                        &endpoint,
                        &client_id,
                        client_secret.as_deref(),
                        &expires,
                        &resource,
                    )
                    .await
                    .map_err(Into::into)
                })
            })
            .await
        {
            return internal_error(format!("db_set_oauth_state: {error:#}")).into_response();
        }
    }

    // Cancel stale reconnect if one is running for this server.
    if let Some(mgr) = state.reconnect_managers.get(&req.agent) {
        mgr.lock().await.cancel(&req.server);
    }

    // Notify refresh scheduler before readiness probing so a token that was
    // accepted by the OAuth provider can still be refreshed after transient
    // upstream MCP failures.
    if let Some(tx) = state.refresh_senders.get(&req.agent) {
        let entry = OAuthServerState {
            refresh_token: Some(req.refresh_token.clone()),
            token_endpoint: req.token_endpoint.clone(),
            client_id: req.client_id.clone(),
            client_secret: req.client_secret.clone(),
            expires_at,
            server_url: handle.url().to_string(),
            resource: oauth_resource.clone(),
        };
        if let Err(e) = tx
            .send(RefreshMessage::NewEntry {
                server_name: req.server.clone(),
                state: entry,
                token: handle.token().clone(),
                backend: handle.clone(),
            })
            .await
        {
            tracing::warn!(
                agent = req.agent.as_str(),
                server = req.server.as_str(),
                "failed to notify refresh scheduler: {e:#}"
            );
        }
    }

    match reconnect_after_oauth_update(&req.server, Arc::clone(&handle)).await {
        Ok(instructions) => {
            if let Err(error) = owner
                .update_mcp_instructions(req.server.clone(), instructions)
                .await
            {
                return internal_error(format!("persist OAuth reconnect instructions: {error:#}"))
                    .into_response();
            }
        }
        Err(error) => {
            let detail = right_mcp::proxy::redact_query_strings(&format!("{error:#}"));
            if right_mcp::proxy::is_upstream_auth_error(&detail) {
                handle.set_status(BackendStatus::NeedsAuth).await;
                return error_response(
                    StatusCode::UNAUTHORIZED,
                    "mcp_reconnect_needs_auth",
                    Some(detail),
                )
                .into_response();
            }
            handle.set_status(BackendStatus::Unreachable).await;
            return error_response(
                StatusCode::BAD_GATEWAY,
                "mcp_reconnect_failed",
                Some(detail),
            )
            .into_response();
        }
    }

    (
        StatusCode::OK,
        Json(SetTokenResponse {
            ok: true,
            warning: None,
        }),
    )
        .into_response()
}

async fn handle_mcp_list(
    State(state): State<InternalState>,
    Json(req): Json<McpListRequest>,
) -> axum::response::Response {
    let dispatcher = &state.dispatcher;
    let (proxies_lock, right_tool_count) = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        (
            Arc::clone(&registry.proxies),
            registry.right.tools_list().len(),
        )
    };

    let mut servers = Vec::new();

    // Right backend (always connected)
    servers.push(McpServerStatus {
        name: "right".into(),
        url: None,
        status: "connected".into(),
        tool_count: right_tool_count,
        auth_type: None,
        header_names: Vec::new(),
        last_connect_error: None,
        last_attempt_at: None,
        last_success_at: None,
    });

    // SQLite preserves "oauth" as auth_type; AuthMethod enum has no OAuth variant.
    let owner = match database_owner(&state, &req.agent).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let (db_auth_types, db_header_names) = match owner
        .local_operation(|conn| {
            Box::pin(async move {
                let server_entries = credentials::db_list_servers(conn).await?;
                let auth_types: std::collections::HashMap<String, Option<String>> = server_entries
                    .iter()
                    .map(|server| (server.name.clone(), server.auth_type.clone()))
                    .collect();
                let mut header_names: std::collections::HashMap<String, Vec<String>> =
                    server_entries
                        .iter()
                        .filter(|server| server.auth_type.as_deref() == Some("headers"))
                        .map(|server| (server.name.clone(), Vec::new()))
                        .collect();
                for (server_name, header_name) in
                    credentials::db_list_all_http_header_names(conn).await?
                {
                    if let Some(list) = header_names.get_mut(&server_name) {
                        list.push(display_header_name(header_name));
                    }
                }
                Ok((auth_types, header_names))
            })
        })
        .await
    {
        Ok(value) => value,
        Err(error) => {
            return internal_error(format!("MCP list persistence: {error:#}")).into_response();
        }
    };

    // External proxy backends
    let proxies = proxies_lock.read().await;
    for (name, proxy) in proxies.iter() {
        let status = proxy.status().await;
        let tool_count = proxy.try_tools().map(|t| t.len()).unwrap_or(0);
        let auth_type = match db_auth_types.get(name) {
            Some(auth_type) => auth_type.clone(),
            None => Some(proxy.auth_method().to_string()),
        };
        servers.push(McpServerStatus {
            name: name.clone(),
            url: Some(proxy.url().to_string()),
            status: status.to_string(),
            tool_count,
            auth_type,
            header_names: db_header_names.get(name).cloned().unwrap_or_default(),
            last_connect_error: proxy.last_connect_error().await,
            last_attempt_at: proxy.last_attempt_at().await.map(|t| t.to_rfc3339()),
            last_success_at: proxy.last_success_at().await.map(|t| t.to_rfc3339()),
        });
    }

    Json(McpListResponse { servers }).into_response()
}

async fn handle_mcp_instructions(
    State(state): State<InternalState>,
    Json(req): Json<McpInstructionsRequest>,
) -> axum::response::Response {
    let dispatcher = &state.dispatcher;
    if !dispatcher.agents.contains_key(&req.agent) {
        return not_found(format!("agent '{}' not found", req.agent)).into_response();
    }
    let owner = match database_owner(&state, &req.agent).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let servers = match owner
        .local_operation(|conn| {
            Box::pin(async move { credentials::db_list_servers(conn).await.map_err(Into::into) })
        })
        .await
    {
        Ok(value) => value,
        Err(error) => return internal_error(format!("db_list_servers: {error:#}")).into_response(),
    };

    let content = right_codegen::generate_mcp_instructions_md(&servers);

    Json(McpInstructionsResponse {
        instructions: content,
    })
    .into_response()
}

async fn handle_reload(State(state): State<InternalState>) -> axum::response::Response {
    let _reload_guard = state.reload_lock.lock().await;
    // 1. Read token map from disk
    let content = match tokio::fs::read_to_string(&state.token_map_path).await {
        Ok(c) => c,
        Err(e) => return internal_error(format!("read token map: {e:#}")).into_response(),
    };
    let disk_entries: std::collections::HashMap<String, String> =
        match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => return internal_error(format!("parse token map: {e:#}")).into_response(),
        };

    // 2. Find new agents (in disk but not in dispatcher) and detect token
    //    rotations for agents that already exist.
    let mut added = Vec::new();
    for (agent_name, token) in &disk_entries {
        if state.dispatcher.agents.contains_key(agent_name) {
            // Existing agent: check if the on-disk token differs from the
            // current in-memory token. If so, atomically swap the mapping so
            // the aggregator accepts the new token without a full restart.
            let mut map = state.token_map.write().await;
            let current_token = map
                .iter()
                .find(|(_, info)| info.name == *agent_name)
                .map(|(tok, _)| tok.clone());
            if let Some(current) = current_token
                && current != *token
            {
                let dir = map
                    .get(&current)
                    .map(|info| info.dir.clone())
                    .unwrap_or_else(|| state.agents_dir.join(agent_name));
                map.remove(&current);
                map.insert(
                    token.clone(),
                    AgentInfo {
                        name: agent_name.clone(),
                        dir,
                    },
                );
                tracing::info!(agent = %agent_name, "reload: rotated token");
            }
            continue;
        }

        let agent_dir = state.agents_dir.join(agent_name);
        if !agent_dir.exists() {
            tracing::warn!(
                agent = agent_name.as_str(),
                "reload: agent dir missing, skipping"
            );
            continue;
        }

        // Build the complete runtime while it is unpublished. Only after every
        // restore and task setup succeeds do all routing maps become visible.
        let http_client = match crate::runtime_builder::build_http_client() {
            Ok(client) => client,
            Err(error) => {
                return internal_error(format!("build reload HTTP client: {error:#}"))
                    .into_response();
            }
        };
        let runtime = match crate::runtime_builder::build_agent_runtime(
            agent_name,
            agent_dir.clone(),
            &state.agents_dir,
            Arc::clone(&state.providers),
            http_client,
        )
        .await
        {
            Ok(runtime) => runtime,
            Err(error) => {
                return internal_error(format!("build runtime for reload: {error:#}"))
                    .into_response();
            }
        };
        if let Err(error) = state
            .db_owners
            .insert_bundle(Arc::clone(&runtime.bundle))
            .await
        {
            state.refresh_senders.remove(agent_name);
            if let Some((_, manager)) = state.reconnect_managers.remove(agent_name) {
                manager.into_inner().cancel_all();
            }
            state.dispatcher.agents.remove(agent_name);
            state
                .token_map
                .write()
                .await
                .retain(|_, info| info.name != *agent_name);
            if let Err(drain_error) = runtime
                .bundle
                .drain(std::time::Duration::from_secs(10))
                .await
            {
                return internal_error(format!(
                    "register runtime: {error:#}; cleanup failed: {drain_error:#}"
                ))
                .into_response();
            }
            return internal_error(format!("register runtime: {error:#}")).into_response();
        }
        state
            .refresh_senders
            .insert(agent_name.clone(), runtime.refresh_sender);
        state.reconnect_managers.insert(
            agent_name.clone(),
            tokio::sync::Mutex::new(runtime.reconnect_manager),
        );
        state
            .dispatcher
            .agents
            .insert(agent_name.clone(), runtime.registry);
        {
            let mut map = state.token_map.write().await;
            map.insert(
                token.clone(),
                AgentInfo {
                    name: agent_name.clone(),
                    dir: agent_dir,
                },
            );
        }
        runtime.bundle.publish();

        added.push(agent_name.clone());
        tracing::info!(agent = agent_name.as_str(), "reload: registered new agent");
    }

    // 3. Remove agents that are in dispatcher but not on disk
    let mut removed = Vec::new();
    let current_agents: Vec<String> = state
        .dispatcher
        .agents
        .iter()
        .map(|entry| entry.key().clone())
        .collect();
    for agent_name in current_agents {
        if !disk_entries.contains_key(&agent_name) {
            let bundle = match state.db_owners.bundle(&agent_name).await {
                Some(bundle) => bundle,
                None => {
                    return internal_error(format!(
                        "remove database owner: runtime bundle for '{agent_name}' not found"
                    ))
                    .into_response();
                }
            };
            bundle.owner.begin_draining();
            if let Some((_, manager)) = state.reconnect_managers.remove(&agent_name) {
                manager.into_inner().cancel_all();
            }
            state.refresh_senders.remove(&agent_name);
            if let Err(error) = bundle.drain(std::time::Duration::from_secs(10)).await {
                bundle.owner.mark_failed();
                return internal_error(format!("database owner drain failed: {error:#}"))
                    .into_response();
            }
            state.dispatcher.agents.remove(&agent_name);
            state.db_owners.remove(&agent_name).await;
            // Remove from token_map (token→AgentInfo where AgentInfo.name matches)
            {
                let mut map = state.token_map.write().await;
                map.retain(|_token, info| info.name != agent_name);
            }
            removed.push(agent_name.clone());
            tracing::info!(
                agent = agent_name.as_str(),
                "reload: removed destroyed agent"
            );
        }
    }

    let total = state.dispatcher.agents.len();
    (
        StatusCode::OK,
        Json(right_mcp::internal_client::ReloadResponse {
            added,
            removed,
            total,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::Request;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    fn setup_crypto() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    async fn make_test_dispatcher(tmp: &std::path::Path) -> Arc<ToolDispatcher> {
        use crate::aggregator::BackendRegistry;
        use crate::right_backend::RightBackend;
        use dashmap::DashMap;
        use std::collections::HashMap;

        let agents_dir = tmp.join("agents");
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let owner = Arc::new(crate::db_owner::AgentDbOwner::starting(
            "test-agent",
            agent_dir.clone(),
        ));
        owner.open_and_migrate().await.unwrap();
        let right = RightBackend::new(agents_dir, None).with_db_owner(owner);
        let registry = BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            agent_dir,
            hindsight: None,
        };

        let agents = DashMap::new();
        agents.insert("test-agent".into(), registry);
        Arc::new(ToolDispatcher { agents })
    }

    async fn make_test_router_and_dispatcher(
        tmp: &std::path::Path,
    ) -> (Router, Arc<ToolDispatcher>) {
        let dispatcher = make_test_dispatcher(tmp).await;
        let refresh_senders: RefreshSenders = Arc::new(dashmap::DashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(dashmap::DashMap::new());

        let token_map_path = tmp.join("agent-tokens.json");
        if !token_map_path.exists() {
            std::fs::write(
                &token_map_path,
                serde_json::json!({"test-agent": "tok-test"}).to_string(),
            )
            .unwrap();
        }

        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "tok-test".into(),
                crate::aggregator::AgentInfo {
                    name: "test-agent".into(),
                    dir: tmp.join("agents/test-agent"),
                },
            );
            std::sync::Arc::new(tokio::sync::RwLock::new(map))
        };

        let owners = crate::db_owner::DbOwnerRegistry::open_initial([(
            "test-agent".to_owned(),
            tmp.join("agents/test-agent"),
        )])
        .await
        .unwrap();
        owners.bundle("test-agent").await.unwrap().publish();

        let router = internal_router(InternalRouterDeps {
            dispatcher: Arc::clone(&dispatcher),
            refresh_senders,
            reconnect_managers,
            token_map,
            db_owners: owners,
            token_map_path,
            agents_dir: tmp.join("agents"),
            providers: open_provider_store(tmp).await.unwrap(),
        });
        (router, dispatcher)
    }
    #[tokio::test]
    async fn db_ready_reports_unavailable_and_ready_states() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");
        let owner_dir = agents_dir.join("test-agent");
        std::fs::create_dir_all(&owner_dir).unwrap();

        let owners = crate::db_owner::DbOwnerRegistry::new();
        let owner = Arc::new(crate::db_owner::AgentDbOwner::starting(
            "test-agent",
            owner_dir,
        ));
        owners.insert_starting(Arc::clone(&owner)).await.unwrap();
        owners.bundle("test-agent").await.unwrap().publish();

        let dispatcher = Arc::new(ToolDispatcher {
            agents: dashmap::DashMap::new(),
        });
        let token_map = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        let app = internal_router(InternalRouterDeps {
            dispatcher,
            refresh_senders: Arc::new(dashmap::DashMap::new()),
            reconnect_managers: Arc::new(dashmap::DashMap::new()),
            token_map,
            db_owners: owners,
            token_map_path: tmp.path().join("agent-tokens.json"),
            agents_dir,
            providers: open_provider_store(tmp.path()).await.unwrap(),
        });

        let (status, body) = send_json(
            app.clone(),
            "/db/ready",
            serde_json::json!({"agent": "test-agent"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["state"], "starting");
        assert_eq!(body["ready"], false);

        owner.open_and_migrate().await.unwrap();
        let (status, body) =
            send_json(app, "/db/ready", serde_json::json!({"agent": "test-agent"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["state"], "ready");
        assert_eq!(body["ready"], true);
    }

    async fn make_test_router(tmp: &std::path::Path) -> Router {
        make_test_router_and_dispatcher(tmp).await.0
    }

    async fn send_json(
        app: Router,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        (status, json)
    }

    async fn start_failing_mcp_server(counter: Arc<AtomicUsize>) -> String {
        let app = Router::new().route(
            "/mcp",
            axum::routing::any(move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/mcp")
    }

    async fn start_blocked_failing_mcp_server(
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) -> String {
        let app = Router::new().route(
            "/mcp",
            axum::routing::any(move || {
                let started = Arc::clone(&started);
                let release = Arc::clone(&release);
                async move {
                    started.notify_one();
                    release.notified().await;
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/mcp")
    }

    async fn start_empty_mcp_server() -> String {
        #[derive(Clone)]
        struct EmptyMcpServer;

        impl rmcp::ServerHandler for EmptyMcpServer {
            fn get_info(&self) -> rmcp::model::ServerInfo {
                rmcp::model::ServerInfo::new(
                    rmcp::model::ServerCapabilities::builder()
                        .enable_tools()
                        .build(),
                )
                .with_server_info(rmcp::model::Implementation::new("test-mcp", "0.0.0"))
            }

            #[allow(clippy::manual_async_fn)]
            fn list_tools(
                &self,
                _request: Option<rmcp::model::PaginatedRequestParams>,
                _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
            ) -> impl std::future::Future<
                Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>,
            > + Send
            + '_ {
                async {
                    Ok(rmcp::model::ListToolsResult {
                        tools: Vec::new(),
                        next_cursor: None,
                        meta: None,
                    })
                }
            }
        }

        let ct = tokio_util::sync::CancellationToken::new();
        let config = rmcp::transport::streamable_http_server::StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None)
            .with_cancellation_token(ct)
            .disable_allowed_hosts();
        let session_manager = Arc::new(
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
        );
        let service = rmcp::transport::streamable_http_server::StreamableHttpService::new(
            || Ok::<_, std::io::Error>(EmptyMcpServer),
            session_manager,
            config,
        );
        let app = Router::new().nest_service("/mcp", service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/mcp")
    }

    #[tokio::test]
    async fn mcp_add_validates_name_reserved() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "right",
                "url": "https://example.com/mcp"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body["error"].as_str().unwrap().contains("reserved"),
            "expected reserved name error, got: {body}"
        );
    }

    #[tokio::test]
    async fn progress_register_adds_foreground_invocation() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;

        let (status, body) = send_json(
            app,
            "/progress/register",
            serde_json::json!({
                "agent": "test-agent",
                "invocation_id": "inv-1",
                "kind": "foreground",
                "bot_send_token": "send-token",
                "chat_id": 100,
                "thread_id": 7
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        let progress = dispatcher
            .agents
            .get("test-agent")
            .expect("test-agent registered")
            .right
            .progress_registry();
        let target = progress.get("inv-1").await.expect("invocation registered");
        assert_eq!(target.bot_send_token, "send-token");
        assert_eq!(
            target.bot_socket_path,
            tmp.path().join("agents/test-agent/bot.sock")
        );
        let scope = progress
            .conversation_scope("inv-1")
            .await
            .expect("conversation scope registered");
        assert_eq!(
            scope,
            crate::progress::ConversationScope {
                chat_id: 100,
                thread_id: 7
            }
        );
    }

    #[tokio::test]
    async fn progress_invocation_kind_register_maps_probe_writer_and_curator() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;
        let cases = [
            (
                "probe-writer-inv",
                "probe_writer",
                crate::progress::ProgressInvocationKind::ProbeWriter,
            ),
            (
                "curator-inv",
                "curator",
                crate::progress::ProgressInvocationKind::Curator,
            ),
        ];

        for (invocation_id, kind_json, expected_kind) in cases {
            let (status, body) = send_json(
                app.clone(),
                "/progress/register",
                serde_json::json!({
                    "agent": "test-agent",
                    "invocation_id": invocation_id,
                    "kind": kind_json,
                    "bot_send_token": "send-token",
                    "chat_id": 100,
                    "thread_id": 7
                }),
            )
            .await;

            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["ok"], true);
            let progress = dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered")
                .right
                .progress_registry();
            let actual_kind = progress
                .learning_invocation_kind(invocation_id)
                .await
                .expect("background learning invocation registered");
            assert_eq!(actual_kind, expected_kind);
        }
    }

    #[tokio::test]
    async fn progress_unregister_removes_invocation() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;

        let (status, _) = send_json(
            app.clone(),
            "/progress/register",
            serde_json::json!({
                "agent": "test-agent",
                "invocation_id": "inv-1",
                "kind": "foreground",
                "bot_send_token": "send-token"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send_json(
            app,
            "/progress/unregister",
            serde_json::json!({
                "agent": "test-agent",
                "invocation_id": "inv-1"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        let progress = dispatcher
            .agents
            .get("test-agent")
            .expect("test-agent registered")
            .right
            .progress_registry();
        let err = progress.get("inv-1").await.unwrap_err();
        assert_eq!(err, crate::progress::ProgressError::Unavailable);
    }

    #[tokio::test]
    async fn progress_register_rejects_unknown_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, _) = send_json(
            app,
            "/progress/register",
            serde_json::json!({
                "agent": "missing-agent",
                "invocation_id": "inv-1",
                "kind": "foreground",
                "bot_send_token": "send-token"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn mcp_add_validates_name_double_underscore() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, _body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "my__server",
                "url": "https://example.com/mcp"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn mcp_add_warns_for_plain_http_oauth_registration() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "notion",
                "url": "http://mcp.notion.com/mcp",
                "auth_type": "oauth"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "body={body}");
        assert_eq!(body["tools_count"], 0);
        assert!(
            body["warning"].as_str().unwrap().contains("Plain HTTP"),
            "expected plain HTTP warning, got: {body}"
        );
    }

    #[tokio::test]
    async fn mcp_add_rejects_cloud_metadata_url() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, _body) = send_json(
            app.clone(),
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "imds",
                "url": "http://169.254.169.254/mcp"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "imds2",
                "url": "http://100.100.100.200/latest/meta-data"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn mcp_add_allows_loopback_oauth_registration() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "local",
                "url": "http://127.0.0.1:3333/mcp",
                "auth_type": "oauth"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "body={body}");
        assert_eq!(body["tools_count"], 0);
    }

    #[tokio::test]
    async fn mcp_add_headers_auth_redacts_values_in_list() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;
        let mcp_url = start_empty_mcp_server().await;

        let (status, body) = send_json(
            app.clone(),
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nango",
                "url": mcp_url,
                "auth_type": "headers",
                "headers": [
                    { "name": "Authorization", "value": "Bearer env-secret" },
                    { "name": "connection-id", "value": "conn_123" },
                    { "name": "provider-config-key", "value": "github" }
                ]
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "body={body}");

        let (status, body) = send_json(
            app,
            "/mcp-list",
            serde_json::json!({ "agent": "test-agent" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "body={body}");
        let servers = body["servers"].as_array().unwrap();
        let nango = servers
            .iter()
            .find(|server| server["name"] == "nango")
            .expect("nango listed");
        assert_eq!(nango["auth_type"], "headers");
        assert_eq!(
            nango["header_names"],
            serde_json::json!(["Authorization", "connection-id", "provider-config-key"])
        );
        assert!(
            !body.to_string().contains("env-secret"),
            "list response must not expose header values: {body}"
        );
        assert!(
            !body.to_string().contains("conn_123"),
            "list response must not expose header values: {body}"
        );
        assert!(
            !body.to_string().contains("github"),
            "list response must not expose header values: {body}"
        );
    }

    #[tokio::test]
    async fn mcp_add_url_as_is_clears_stale_headers_auth() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;
        let mcp_url = start_empty_mcp_server().await;

        let (status, body) = send_json(
            app.clone(),
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nango",
                "url": mcp_url,
                "auth_type": "headers",
                "headers": [
                    { "name": "Authorization", "value": "Bearer old" }
                ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");

        let (status, body) = send_json(
            app.clone(),
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nango",
                "url": mcp_url
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");

        let (status, body) = send_json(
            app,
            "/mcp-list",
            serde_json::json!({ "agent": "test-agent" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "body={body}");
        let servers = body["servers"].as_array().unwrap();
        let nango = servers
            .iter()
            .find(|server| server["name"] == "nango")
            .expect("nango listed");
        assert!(nango.get("auth_type").is_none(), "body={body}");
        assert!(nango.get("header_names").is_none(), "body={body}");
        assert!(
            !body.to_string().contains("Bearer old"),
            "list response must not expose stale header values: {body}"
        );
    }

    #[tokio::test]
    async fn mcp_add_failed_replacement_restores_database_and_existing_proxy() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;
        let old_url = start_empty_mcp_server().await;

        let (status, body) = send_json(
            app.clone(),
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nango",
                "url": old_url,
                "auth_type": "headers",
                "headers": [
                    { "name": "Authorization", "value": "Bearer old-secret" },
                    { "name": "connection-id", "value": "old-connection" }
                ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");

        let proxies = Arc::clone(
            &dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered")
                .proxies,
        );
        let old_proxy = proxies
            .read()
            .await
            .get("nango")
            .cloned()
            .expect("old proxy published");
        assert_eq!(old_proxy.status().await, BackendStatus::Connected);

        let owner = dispatcher
            .agents
            .get("test-agent")
            .expect("test-agent registered")
            .right
            .mcp_persistence()
            .expect("owner persistence");
        drop(owner);
        let conn = right_db::open_connection(&tmp.path().join("agents/test-agent"), false)
            .await
            .expect("test db connection");
        conn.execute(
            "UPDATE mcp_servers SET instructions = 'old instructions', auth_header = 'legacy-header', \
             auth_token = 'old-access', refresh_token = 'old-refresh', \
             token_endpoint = 'https://auth.example/token', client_id = 'old-client', \
             client_secret = 'old-client-secret', expires_at = '2027-01-02T03:04:05Z', \
             oauth_resource = 'https://old.example/resource' WHERE name = 'nango'",
            [],
        )
        .await
        .unwrap();
        let old_record: Vec<Option<String>> = conn
            .query_one(
                "SELECT url, instructions, auth_type, auth_header, auth_token, refresh_token, \
                 token_endpoint, client_id, client_secret, expires_at, oauth_resource, created_at \
                 FROM mcp_servers WHERE name = 'nango'",
                [],
                |row| (0..12).map(|index| row.get(index)).collect(),
            )
            .await
            .unwrap();
        let old_headers: Vec<(String, String)> = conn
            .query_all(
                "SELECT header_name, header_value FROM mcp_http_headers \
                 WHERE server_name = 'nango' ORDER BY header_name",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .await
            .unwrap();

        let failing_url = start_failing_mcp_server(Arc::new(AtomicUsize::new(0))).await;
        let (status, body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nango",
                "url": failing_url,
                "auth_type": "headers",
                "headers": [{ "name": "Authorization", "value": "Bearer replacement-secret" }]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "body={body}");
        assert!(!body.to_string().contains("old-secret"));
        assert!(!body.to_string().contains("replacement-secret"));

        let restored_record: Vec<Option<String>> = conn
            .query_one(
                "SELECT url, instructions, auth_type, auth_header, auth_token, refresh_token, \
                 token_endpoint, client_id, client_secret, expires_at, oauth_resource, created_at \
                 FROM mcp_servers WHERE name = 'nango'",
                [],
                |row| (0..12).map(|index| row.get(index)).collect(),
            )
            .await
            .unwrap();
        let restored_headers: Vec<(String, String)> = conn
            .query_all(
                "SELECT header_name, header_value FROM mcp_http_headers \
                 WHERE server_name = 'nango' ORDER BY header_name",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .await
            .unwrap();
        assert_eq!(restored_record, old_record);
        assert_eq!(restored_headers, old_headers);

        let routed_proxy = proxies
            .read()
            .await
            .get("nango")
            .cloned()
            .expect("old proxy remains routed");
        assert!(Arc::ptr_eq(&routed_proxy, &old_proxy));
        assert_eq!(routed_proxy.url(), old_url);
        assert_eq!(routed_proxy.status().await, BackendStatus::Connected);
    }

    #[tokio::test]
    async fn mcp_add_serializes_overlapping_replacements() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;
        let old_url = start_empty_mcp_server().await;
        let (status, body) = send_json(
            app.clone(),
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "server",
                "url": old_url
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");

        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let failing_url =
            start_blocked_failing_mcp_server(Arc::clone(&started), Arc::clone(&release)).await;
        let first = tokio::spawn(send_json(
            app.clone(),
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "server",
                "url": failing_url
            }),
        ));
        started.notified().await;

        let final_url = start_empty_mcp_server().await;
        let second = tokio::spawn(send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "server",
                "url": final_url
            }),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !second.is_finished(),
            "second replacement must wait for the first"
        );
        release.notify_one();

        let (first_status, first_body) = first.await.unwrap();
        assert_eq!(first_status, StatusCode::BAD_GATEWAY, "body={first_body}");
        let (second_status, second_body) = second.await.unwrap();
        assert_eq!(second_status, StatusCode::OK, "body={second_body}");

        let conn = right_db::open_connection(&tmp.path().join("agents/test-agent"), false)
            .await
            .unwrap();
        let url: String = conn
            .query_one(
                "SELECT url FROM mcp_servers WHERE name = 'server'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(url, final_url);
        let proxies = Arc::clone(
            &dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered")
                .proxies,
        );
        let routed = proxies.read().await.get("server").cloned().unwrap();
        assert_eq!(routed.url(), final_url);
        assert_eq!(routed.status().await, BackendStatus::Connected);
    }

    #[tokio::test]
    async fn mcp_add_failed_new_server_leaves_no_database_row_or_proxy() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;
        let failing_url = start_failing_mcp_server(Arc::new(AtomicUsize::new(0))).await;

        let (status, body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "new-server",
                "url": failing_url,
                "auth_type": "headers",
                "headers": [{ "name": "Authorization", "value": "Bearer new-secret" }]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "body={body}");
        assert!(!body.to_string().contains("new-secret"));

        let conn = right_db::open_connection(&tmp.path().join("agents/test-agent"), false)
            .await
            .expect("test db connection");
        assert!(
            !credentials::db_server_exists(&conn, "new-server")
                .await
                .unwrap()
        );
        let proxies = Arc::clone(
            &dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered")
                .proxies,
        );
        assert!(!proxies.read().await.contains_key("new-server"));
    }

    #[tokio::test]
    async fn mcp_add_rollback_failure_reports_original_and_rollback_context() {
        let owner = Arc::new(crate::db_owner::AgentDbOwner::starting(
            "test-agent",
            tempfile::tempdir().unwrap().path().to_path_buf(),
        ));
        let response = rollback_mcp_add(
            &owner,
            "server",
            None,
            "connection failed: original failure".to_owned(),
            StatusCode::BAD_GATEWAY,
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let error = body["error"].as_str().unwrap();
        assert!(error.contains("connection failed: original failure"));
        assert!(error.contains("rollback failed"));
        assert!(error.contains("database owner"));
    }

    #[tokio::test]
    async fn mcp_set_headers_replaces_existing_header_names() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;
        let mcp_url = start_empty_mcp_server().await;

        let (status, body) = send_json(
            app.clone(),
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nango",
                "url": mcp_url,
                "auth_type": "headers",
                "headers": [
                    { "name": "Authorization", "value": "Bearer old" },
                    { "name": "connection-id", "value": "old_conn" }
                ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");

        let (status, body) = send_json(
            app.clone(),
            "/mcp-set-headers",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nango",
                "headers": [
                    { "name": "connection-id", "value": "new_conn" }
                ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");

        // The reconnect now happens in the background; poll until it lands.
        let mut nango = serde_json::Value::Null;
        for _ in 0..50 {
            let (status, body) = send_json(
                app.clone(),
                "/mcp-list",
                serde_json::json!({ "agent": "test-agent" }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "body={body}");
            nango = body["servers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|server| server["name"] == "nango")
                .cloned()
                .unwrap();
            if nango["status"] == "connected" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(
            nango["status"], "connected",
            "background reconnect should land: {nango}"
        );
        assert_eq!(nango["header_names"], serde_json::json!(["connection-id"]));

        let (status, body) = send_json(
            app,
            "/mcp-list",
            serde_json::json!({ "agent": "test-agent" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");
        assert!(
            !body.to_string().contains("new_conn"),
            "list response must not expose header values: {body}"
        );
    }

    #[tokio::test]
    async fn mcp_add_headers_auth_rejects_empty_headers() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nango",
                "url": "https://api.nango.dev/mcp",
                "auth_type": "headers",
                "headers": []
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("at least one header"),
            "expected empty headers validation error, got: {body}"
        );
    }

    #[tokio::test]
    async fn mcp_set_headers_rejects_empty_headers() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;
        let mcp_url = start_empty_mcp_server().await;

        let (status, body) = send_json(
            app.clone(),
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nango",
                "url": mcp_url,
                "auth_type": "headers",
                "headers": [
                    { "name": "Authorization", "value": "Bearer old" }
                ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");

        let (status, body) = send_json(
            app,
            "/mcp-set-headers",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nango",
                "headers": []
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("at least one header"),
            "expected empty headers validation error, got: {body}"
        );
    }

    #[tokio::test]
    async fn mcp_remove_protected_name_right() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, body) = send_json(
            app,
            "/mcp-remove",
            serde_json::json!({
                "agent": "test-agent",
                "name": "right"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body["error"].as_str().unwrap().contains("protected"),
            "expected protected error, got: {body}"
        );
    }

    #[tokio::test]
    async fn mcp_remove_protected_name_rightmeta() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, _body) = send_json(
            app,
            "/mcp-remove",
            serde_json::json!({
                "agent": "test-agent",
                "name": "rightmeta"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn mcp_remove_agent_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, _body) = send_json(
            app,
            "/mcp-remove",
            serde_json::json!({
                "agent": "nonexistent",
                "name": "notion"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn mcp_remove_server_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, _body) = send_json(
            app,
            "/mcp-remove",
            serde_json::json!({
                "agent": "test-agent",
                "name": "nonexistent"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn mcp_remove_purges_orphan_db_row_without_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, _dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;

        // Pre-insert a DB row with no matching in-memory proxy. This simulates
        // an orphan left behind when the aggregator restarted but the proxy
        // failed to reconnect.
        let conn = right_db::open_connection(&tmp.path().join("agents/test-agent"), false)
            .await
            .expect("test db connection");
        {
            credentials::db_add_server(&conn, "orphan", "https://example.com/mcp")
                .await
                .expect("seed orphan row");
            assert!(
                credentials::db_server_exists(&conn, "orphan")
                    .await
                    .expect("server exists check")
            );
        }

        let (status, body) = send_json(
            app,
            "/mcp-remove",
            serde_json::json!({
                "agent": "test-agent",
                "name": "orphan"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "body={body}");
        assert_eq!(body["removed"], true);

        assert!(
            !credentials::db_server_exists(&conn, "orphan")
                .await
                .expect("server exists check"),
            "DB row must be deleted even when proxy was missing"
        );
    }

    #[tokio::test]
    async fn set_token_agent_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, _body) = send_json(
            app,
            "/set-token",
            serde_json::json!({
                "agent": "nonexistent",
                "server": "notion",
                "access_token": "tok-abc",
                "refresh_token": "ref-abc",
                "expires_in": 3600,
                "token_endpoint": "https://auth.example.com/token",
                "client_id": "my-client"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn set_token_server_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, _body) = send_json(
            app,
            "/set-token",
            serde_json::json!({
                "agent": "test-agent",
                "server": "nonexistent",
                "access_token": "tok-abc",
                "refresh_token": "ref-abc",
                "expires_in": 3600,
                "token_endpoint": "https://auth.example.com/token",
                "client_id": "my-client"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn set_token_rejects_private_token_endpoint_before_storing_token() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;

        let (persistence, proxies, conn) = {
            let registry = dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered");
            (
                registry.right.mcp_persistence().unwrap(),
                Arc::clone(&registry.proxies),
                right_db::open_connection(&tmp.path().join("agents/test-agent"), false)
                    .await
                    .expect("test db connection"),
            )
        };
        {
            conn.execute(
                "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('composio', 'https://mcp.example.com/mcp', 'oauth')",
                [],
            )
            .await
            .unwrap();
        }

        let token = Arc::new(tokio::sync::RwLock::new(Some("old-token".to_string())));
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            persistence,
            "https://mcp.example.com/mcp".into(),
            Arc::clone(&token),
            AuthMethod::Bearer,
        ));
        proxies
            .write()
            .await
            .insert("composio".into(), Arc::clone(&backend));

        let (status, body) = send_json(
            app,
            "/set-token",
            serde_json::json!({
                "agent": "test-agent",
                "server": "composio",
                "access_token": "fresh-token",
                "refresh_token": "fresh-refresh",
                "expires_in": 3600,
                "token_endpoint": "http://127.0.0.1:9/token",
                "client_id": "my-client"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
        assert!(!body.to_string().contains("127.0.0.1"));
        assert_eq!(
            *token.read().await,
            Some("old-token".to_string()),
            "private token endpoint must be rejected before in-memory token update"
        );
    }

    #[tokio::test]
    async fn set_token_retries_500_and_reports_failure_without_success() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;
        let attempts = Arc::new(AtomicUsize::new(0));
        let mcp_url = start_failing_mcp_server(Arc::clone(&attempts)).await;

        let (persistence, proxies, conn) = {
            let registry = dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered");
            (
                registry.right.mcp_persistence().unwrap(),
                Arc::clone(&registry.proxies),
                right_db::open_connection(&tmp.path().join("agents/test-agent"), false)
                    .await
                    .expect("test db connection"),
            )
        };
        {
            conn.execute(
                "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('composio', ?1, 'oauth')",
                [&mcp_url],
            )
            .await
            .unwrap();
        }

        let token = Arc::new(tokio::sync::RwLock::new(Some("old-token".to_string())));
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            persistence,
            mcp_url.clone(),
            Arc::clone(&token),
            AuthMethod::Bearer,
        ));
        backend
            .set_status(right_mcp::proxy::BackendStatus::NeedsAuth)
            .await;
        proxies
            .write()
            .await
            .insert("composio".into(), Arc::clone(&backend));

        let (status, body) = send_json(
            app,
            "/set-token",
            serde_json::json!({
                "agent": "test-agent",
                "server": "composio",
                "access_token": "fresh-token",
                "refresh_token": "fresh-refresh",
                "expires_in": 3600,
                "token_endpoint": "https://auth.example.com/token",
                "client_id": "my-client"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_GATEWAY, "body={body}");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            3,
            "500 initialize responses should be retried a bounded number of times",
        );
        assert_eq!(
            backend.status().await,
            right_mcp::proxy::BackendStatus::Unreachable,
            "non-auth reconnect failures must not leave stale needs_auth status",
        );
        assert_eq!(
            *token.read().await,
            Some("fresh-token".to_string()),
            "fresh token should still be stored for the refresh scheduler after readiness failure",
        );
        let persisted_resource: Option<String> = {
            conn.query_row(
                "SELECT oauth_resource FROM mcp_servers WHERE name = 'composio'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap()
        };
        assert_eq!(persisted_resource.as_deref(), Some(mcp_url.as_str()));
    }

    #[tokio::test]
    async fn mcp_add_agent_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "nonexistent",
                "name": "notion",
                "url": "https://mcp.notion.com/mcp"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");
    }

    #[tokio::test]
    async fn mcp_list_returns_right_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, body) = send_json(
            app,
            "/mcp-list",
            serde_json::json!({ "agent": "test-agent" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let servers = body["servers"].as_array().unwrap();
        assert!(!servers.is_empty(), "expected at least one server");

        let right = &servers[0];
        assert_eq!(right["name"], "right");
        assert_eq!(right["status"], "connected");
        assert!(
            right["tool_count"].as_u64().unwrap() > 0,
            "right backend should have tools"
        );
        assert!(
            right["url"].is_null(),
            "right backend should not have a url"
        );
    }

    #[tokio::test]
    async fn mcp_list_unknown_agent_returns_404() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, _body) = send_json(
            app,
            "/mcp-list",
            serde_json::json!({ "agent": "nonexistent" }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn mcp_instructions_returns_header_for_no_servers() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, body) = send_json(
            app,
            "/mcp-instructions",
            serde_json::json!({ "agent": "test-agent" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let instructions = body["instructions"].as_str().unwrap();
        assert_eq!(instructions, "# MCP Server Instructions\n");
    }

    #[tokio::test]
    async fn mcp_instructions_unknown_agent_returns_404() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path()).await;

        let (status, _body) = send_json(
            app,
            "/mcp-instructions",
            serde_json::json!({ "agent": "nonexistent" }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reload_no_new_agents() {
        let tmp = tempfile::tempdir().unwrap();

        let token_map_path = tmp.path().join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"test-agent": "tok-test"}).to_string(),
        )
        .unwrap();

        let dispatcher = make_test_dispatcher(tmp.path()).await;
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "tok-test".into(),
                crate::aggregator::AgentInfo {
                    name: "test-agent".into(),
                    dir: tmp.path().join("agents/test-agent"),
                },
            );
            std::sync::Arc::new(tokio::sync::RwLock::new(map))
        };
        let refresh_senders: RefreshSenders = Arc::new(dashmap::DashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(dashmap::DashMap::new());

        let app = internal_router(InternalRouterDeps {
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            db_owners: crate::db_owner::DbOwnerRegistry::new(),
            token_map_path,
            agents_dir: tmp.path().join("agents"),
            providers: open_provider_store(tmp.path()).await.unwrap(),
        });

        let (status, body) = send_json(app, "/reload", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["added"].as_array().unwrap().is_empty());
        assert_eq!(body["total"], 1);
    }

    #[tokio::test]
    async fn reload_rotates_existing_agent_token() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        right_db::open_db(&agent_dir, true).await.unwrap();

        let token_map_path = tmp.path().join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"test-agent": "tok-new"}).to_string(),
        )
        .unwrap();

        let dispatcher = make_test_dispatcher(tmp.path()).await;
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "tok-old".into(),
                crate::aggregator::AgentInfo {
                    name: "test-agent".into(),
                    dir: agent_dir.clone(),
                },
            );
            std::sync::Arc::new(tokio::sync::RwLock::new(map))
        };
        let refresh_senders: RefreshSenders = Arc::new(dashmap::DashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(dashmap::DashMap::new());

        let app = internal_router(InternalRouterDeps {
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map: token_map.clone(),
            db_owners: crate::db_owner::DbOwnerRegistry::new(),
            token_map_path,
            agents_dir: agents_dir.clone(),
            providers: open_provider_store(tmp.path()).await.unwrap(),
        });

        let (status, body) = send_json(app, "/reload", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["added"].as_array().unwrap().is_empty());
        assert_eq!(body["total"], 1);

        let map = token_map.read().await;
        assert!(!map.contains_key("tok-old"));
        assert_eq!(map.get("tok-new").unwrap().name, "test-agent");
        assert_eq!(map.get("tok-new").unwrap().dir, agent_dir);
    }

    #[tokio::test]
    async fn reload_registers_new_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");

        let agent1_dir = agents_dir.join("test-agent");
        std::fs::create_dir_all(&agent1_dir).unwrap();
        right_db::open_db(&agent1_dir, true).await.unwrap();

        let agent2_dir = agents_dir.join("new-agent");
        std::fs::create_dir_all(&agent2_dir).unwrap();
        right_db::open_db(&agent2_dir, true).await.unwrap();

        let token_map_path = tmp.path().join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"test-agent": "tok-test", "new-agent": "tok-new"}).to_string(),
        )
        .unwrap();

        let dispatcher = make_test_dispatcher(tmp.path()).await;

        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "tok-test".into(),
                crate::aggregator::AgentInfo {
                    name: "test-agent".into(),
                    dir: agent1_dir,
                },
            );
            std::sync::Arc::new(tokio::sync::RwLock::new(map))
        };

        let refresh_senders: RefreshSenders = Arc::new(dashmap::DashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(dashmap::DashMap::new());

        let app = internal_router(InternalRouterDeps {
            dispatcher: dispatcher.clone(),
            refresh_senders,
            reconnect_managers,
            token_map: token_map.clone(),
            db_owners: crate::db_owner::DbOwnerRegistry::new(),
            token_map_path,
            agents_dir: agents_dir.clone(),
            providers: open_provider_store(tmp.path()).await.unwrap(),
        });

        let (status, body) = send_json(app, "/reload", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);

        let added = body["added"].as_array().unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0], "new-agent");
        assert_eq!(body["total"], 2);

        assert!(dispatcher.agents.contains_key("new-agent"));

        let map = token_map.read().await;
        assert!(map.contains_key("tok-new"));
    }

    #[tokio::test]
    async fn mcp_set_headers_persists_against_unreachable_server() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;

        // Seed a DB row + an unreachable proxy backend directly: /mcp-add would
        // itself fail to connect to a dead upstream, so we bypass it.
        let conn = right_db::open_connection(&tmp.path().join("agents/test-agent"), false)
            .await
            .expect("test db connection");
        let dead_url = "http://127.0.0.1:1/mcp".to_string();
        {
            credentials::db_add_server(&conn, "obsidian", &dead_url)
                .await
                .unwrap();
        }
        {
            // Clone the Arc out and drop the DashMap ref before awaiting the lock.
            let proxies = Arc::clone(&dispatcher.agents.get("test-agent").unwrap().proxies);
            let backend = Arc::new(ProxyBackend::new(
                "obsidian".into(),
                dispatcher
                    .agents
                    .get("test-agent")
                    .unwrap()
                    .right
                    .mcp_persistence()
                    .unwrap(),
                dead_url.clone(),
                Arc::new(tokio::sync::RwLock::new(None)),
                AuthMethod::default(),
            ));
            proxies.write().await.insert("obsidian".into(), backend);
        }

        // Set headers while the upstream is unreachable.
        let (status, body) = send_json(
            app.clone(),
            "/mcp-set-headers",
            serde_json::json!({
                "agent": "test-agent",
                "name": "obsidian",
                "headers": [{ "name": "X-Api-Key", "value": "secret-key" }]
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "headers must persist even when unreachable: body={body}"
        );

        // The credential was persisted and the server is still retryable.
        let (status, body) = send_json(
            app,
            "/mcp-list",
            serde_json::json!({ "agent": "test-agent" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");
        let obsidian = body["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|server| server["name"] == "obsidian")
            .unwrap();
        assert_eq!(obsidian["auth_type"], "headers");
        assert_eq!(obsidian["header_names"], serde_json::json!(["x-api-key"]));
        assert_eq!(
            obsidian["status"], "unreachable",
            "stays retryable, not parked: {obsidian}"
        );
        assert!(
            !body.to_string().contains("secret-key"),
            "header values must never be exposed: {body}"
        );
    }

    #[tokio::test]
    async fn mcp_list_exposes_last_connect_error_for_unreachable() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;

        let dead_url = "http://127.0.0.1:1/mcp".to_string();
        let conn = right_db::open_connection(&tmp.path().join("agents/test-agent"), false)
            .await
            .expect("test db connection");
        {
            credentials::db_add_server(&conn, "obsidian", &dead_url)
                .await
                .unwrap();
        }
        let backend = Arc::new(ProxyBackend::new(
            "obsidian".into(),
            dispatcher
                .agents
                .get("test-agent")
                .unwrap()
                .right
                .mcp_persistence()
                .unwrap(),
            dead_url,
            Arc::new(tokio::sync::RwLock::new(None)),
            AuthMethod::default(),
        ));
        // Run the failed connect synchronously so the recorded fields are present.
        let _ = backend.connect(reqwest::Client::new()).await;
        {
            let proxies = Arc::clone(&dispatcher.agents.get("test-agent").unwrap().proxies);
            proxies.write().await.insert("obsidian".into(), backend);
        }

        let (status, body) = send_json(
            app,
            "/mcp-list",
            serde_json::json!({ "agent": "test-agent" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");
        let obsidian = body["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|server| server["name"] == "obsidian")
            .unwrap();
        assert!(
            obsidian["last_connect_error"].is_string(),
            "expected last_connect_error to be a string: {obsidian}"
        );
        assert!(
            obsidian["last_attempt_at"].is_string(),
            "expected last_attempt_at to be a string: {obsidian}"
        );
        assert!(
            obsidian["last_success_at"].is_null(),
            "expected last_success_at to be null: {obsidian}"
        );
    }
}
