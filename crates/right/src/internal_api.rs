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
    HttpHeaderInput, ProgressInvocationKindDto, ProgressRegisterRequest, ProgressRegisterResponse,
    ProgressUnregisterRequest, ProgressUnregisterResponse,
};
use right_mcp::refresh::{OAuthServerState, RefreshMessage};

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
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
pub(crate) struct InternalState {
    dispatcher: Arc<ToolDispatcher>,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
    token_map: AgentTokenMap,
    token_map_path: PathBuf,
    pub(crate) agents_dir: PathBuf,
    /// Right's provider credential store — the single authority for provider
    /// records and credentials (stage 3; replaces the OpenShell provider
    /// gateway in these handlers). Never exposes a credential value on a read
    /// path.
    pub(crate) providers: std::sync::Arc<right_providers::ProviderStore>,
    /// Per-agent serialization for provider mutations. Keyed on agent name
    /// alone — every provider operation eventually does an RMW on the same
    /// `agents/<agent>/agent.yaml`, so a finer (agent, name) key would let
    /// two concurrent creates for distinct providers on the same agent race
    /// the file and silently drop one entry (gateway/policy mutated, but
    /// agent.yaml only retains the last writer's content). Different agents
    /// remain independent.
    pub(crate) provider_locks: std::sync::Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>,
        >,
    >,
}

#[cfg(test)]
impl InternalState {
    /// Test-only constructor: builds the same `InternalState` that
    /// `internal_router` constructs, but returns it directly so tests can
    /// invoke per-handler helpers (e.g. `provider_lock`) without going
    /// through the axum router.
    pub(crate) fn new_for_test(
        dispatcher: Arc<ToolDispatcher>,
        refresh_senders: RefreshSenders,
        reconnect_managers: ReconnectManagers,
        token_map: AgentTokenMap,
        token_map_path: PathBuf,
        agents_dir: PathBuf,
        providers: right_providers::ProviderStore,
    ) -> Self {
        Self {
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            agents_dir,
            providers: std::sync::Arc::new(providers),
            provider_locks: Default::default(),
        }
    }
}

/// Open (creating if absent) the provider credential store at
/// `<home>/providers.db`. FATAL on error: the store is the single authority
/// for provider state, so serving the internal API without it would answer
/// `/provider-list` with lies. FAIL FAST per AGENTS.rust.md §2.
pub(crate) async fn open_provider_store(home: &std::path::Path) -> right_providers::ProviderStore {
    match right_providers::ProviderStore::open(home).await {
        Ok(store) => store,
        Err(e) => panic!("cannot open providers.db under {}: {e:#}", home.display()),
    }
}

pub(crate) fn internal_router(
    dispatcher: Arc<ToolDispatcher>,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
    token_map: AgentTokenMap,
    token_map_path: PathBuf,
    agents_dir: PathBuf,
    providers: right_providers::ProviderStore,
) -> Router {
    let state = InternalState {
        dispatcher,
        refresh_senders,
        reconnect_managers,
        token_map,
        token_map_path,
        agents_dir,
        providers: std::sync::Arc::new(providers),
        provider_locks: Default::default(),
    };
    Router::new()
        .route("/mcp-add", post(handle_mcp_add))
        .route("/mcp-remove", post(handle_mcp_remove))
        .route("/mcp-set-headers", post(handle_mcp_set_headers))
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
) -> Result<(), right_mcp::proxy::ProxyError> {
    let mut last_error = None;

    for attempt in 1..=OAUTH_RECONNECT_MAX_ATTEMPTS {
        let client = oauth_reconnect_http_client();
        match handle.connect(client).await {
            Ok(_) => {
                tracing::info!(
                    server = %server_name,
                    attempt,
                    "reconnected after OAuth token update",
                );
                return Ok(());
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

async fn handle_mcp_add(
    State(state): State<InternalState>,
    Json(req): Json<McpAddRequest>,
) -> axum::response::Response {
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

    // Determine AuthMethod from request fields
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
    let http_warning = plain_http_warning(&req.url);

    // Get backend, agent_dir, and proxies from DashMap, then drop the guard before DB await.
    let (right, agent_dir, proxies_lock) = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        (
            registry.right.clone(),
            registry.agent_dir.clone(),
            Arc::clone(&registry.proxies),
        )
    };
    let conn_arc = match right.get_conn(&req.agent).await {
        Ok(c) => c,
        Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
    };

    {
        let conn = conn_arc.lock().await;
        let existed_before = match credentials::db_server_exists(&conn, &req.name).await {
            Ok(exists) => exists,
            Err(e) => return internal_error(format!("db_server_exists: {e:#}")).into_response(),
        };
        if let Err(e) = credentials::db_add_server(&conn, &req.name, &req.url).await {
            return internal_error(format!("db_add_server: {e:#}")).into_response();
        }
        // Persist auth fields for the selected mode. On failure, roll back only
        // if this request created the row; an upsert over an existing server
        // must not delete the previous registration.
        let auth_result: Result<(), (String, CredentialError)> =
            if req.auth_type.as_deref() == Some("headers") {
                credentials::db_set_http_headers(&conn, &req.name, &header_secrets)
                    .await
                    .map_err(|e| ("db_set_http_headers".to_string(), e))
            } else if let Some(ref auth_type_str) = req.auth_type {
                credentials::db_set_auth(
                    &conn,
                    &req.name,
                    auth_type_str,
                    req.auth_header.as_deref(),
                    auth_token.as_deref(),
                )
                .await
                .map_err(|e| ("db_set_auth".to_string(), e))
            } else {
                credentials::db_clear_auth(&conn, &req.name)
                    .await
                    .map_err(|e| ("db_clear_auth".to_string(), e))
            };
        if let Err((label, e)) = auth_result {
            if !existed_before {
                // Best-effort rollback of the just-inserted mcp_servers row.
                match credentials::db_remove_server(&conn, &req.name).await {
                    Ok(()) | Err(CredentialError::ServerNotFound(_)) => {}
                    Err(db_err) => {
                        tracing::warn!(
                            server = %req.name,
                            "rollback db_remove_server after {label} failure failed: {db_err:#}"
                        );
                    }
                }
            }
            return internal_error(format!("{label}: {e:#}")).into_response();
        }
    }

    // Create ProxyBackend with the resolved auth method and optional token
    let token = Arc::new(tokio::sync::RwLock::new(auth_token.clone()));
    let backend = ProxyBackend::new(
        req.name.clone(),
        agent_dir,
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
        Err(e) => {
            return internal_error(format!("reqwest client build: {e:#}")).into_response();
        }
    };
    match handle.connect(connect_client).await {
        Ok(_instructions) => {
            tracing::info!(server = %req.name, "mcp-add: upstream connection successful");
            let tools_count = handle.try_tools().map(|t| t.len()).unwrap_or(0);

            // Insert into proxies map (proxies_lock extracted from initial DashMap lookup)
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
        Err(e) => {
            // Remove from SQLite on connection failure (reuse conn_arc from initial lookup)
            {
                let conn = conn_arc.lock().await;
                // Best-effort rollback — ignore ServerNotFound
                match credentials::db_remove_server(&conn, &req.name).await {
                    Ok(()) | Err(CredentialError::ServerNotFound(_)) => {}
                    Err(db_err) => {
                        tracing::warn!("rollback db_remove_server failed: {db_err:#}");
                    }
                }
            }

            tracing::warn!(server = %req.name, err = %format!("{e:#}"), "mcp-add: upstream connection failed");
            error_response(
                StatusCode::BAD_GATEWAY,
                format!("connection failed: {e:#}"),
                None,
            )
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
    let (proxies_lock, right) = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        (Arc::clone(&registry.proxies), registry.right.clone())
    };
    let conn_arc = match right.get_conn(&req.agent).await {
        Ok(c) => c,
        Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
    };

    // Remove from proxies (in-memory).
    let removed_from_proxies = {
        let mut proxies = proxies_lock.write().await;
        proxies.remove(&req.name).is_some()
    };

    // Remove from SQLite regardless of in-memory presence. DB rows can
    // outlive the in-memory map (e.g. after an aggregator restart where the
    // proxy failed to reconnect), and leaving them orphans the dashboard.
    let removed_from_db = {
        let conn = conn_arc.lock().await;
        match credentials::db_remove_server(&conn, &req.name).await {
            Ok(()) => true,
            Err(CredentialError::ServerNotFound(_)) => false,
            Err(e) => return internal_error(format!("db_remove_server: {e:#}")).into_response(),
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

    let (right, proxies_lock) = {
        let Some(registry) = state.dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        (registry.right.clone(), Arc::clone(&registry.proxies))
    };
    let conn_arc = match right.get_conn(&req.agent).await {
        Ok(c) => c,
        Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
    };

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
    {
        let conn = conn_arc.lock().await;
        if let Err(e) = credentials::db_set_http_headers(&conn, &req.name, &header_secrets).await {
            return match e {
                CredentialError::ServerNotFound(_) => {
                    not_found(format!("server '{}' not found", req.name)).into_response()
                }
                _ => internal_error(format!("db_set_http_headers: {e:#}")).into_response(),
            };
        }
    }

    // Swap in a fresh backend carrying the new headers. It starts Unreachable;
    // the reconciler re-probes it (with the new headers) until it connects.
    let replacement = Arc::new(ProxyBackend::new(
        req.name.clone(),
        existing.agent_dir().to_path_buf(),
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

    // Update the token in the shared Arc<RwLock<Option<String>>>
    {
        let mut token_guard = handle.token().write().await;
        *token_guard = Some(req.access_token.clone());
    }

    // Persist OAuth state to SQLite
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(req.expires_in as i64);
    let expires_at_str = expires_at.to_rfc3339();
    {
        let right = {
            let Some(registry) = dispatcher.agents.get(&req.agent) else {
                return not_found("agent_not_found").into_response();
            };
            registry.right.clone()
        };
        let conn_arc = match right.get_conn(&req.agent).await {
            Ok(c) => c,
            Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
        };
        let conn = conn_arc.lock().await;
        if let Err(e) = right_mcp::credentials::db_set_oauth_state(
            &conn,
            &req.server,
            &req.access_token,
            Some(&req.refresh_token),
            &req.token_endpoint,
            &req.client_id,
            req.client_secret.as_deref(),
            &expires_at_str,
            &oauth_resource,
        )
        .await
        {
            return internal_error(format!("db_set_oauth_state: {e:#}")).into_response();
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

    if let Err(e) = reconnect_after_oauth_update(&req.server, Arc::clone(&handle)).await {
        let detail = format!("{e:#}");
        let is_auth_error = right_mcp::proxy::is_upstream_auth_error(&detail);
        if is_auth_error {
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
    let (right, proxies_lock, right_tool_count) = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        (
            registry.right.clone(),
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
    let conn_arc = match right.get_conn(&req.agent).await {
        Ok(c) => c,
        Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
    };
    let (db_auth_types, db_header_names): (
        std::collections::HashMap<String, Option<String>>,
        std::collections::HashMap<String, Vec<String>>,
    ) = {
        let conn = conn_arc.lock().await;
        let server_entries = match credentials::db_list_servers(&conn).await {
            Ok(s) => s,
            Err(e) => return internal_error(format!("db_list_servers: {e:#}")).into_response(),
        };
        let auth_types = server_entries
            .iter()
            .map(|s| (s.name.clone(), s.auth_type.clone()))
            .collect();
        let mut header_names: std::collections::HashMap<String, Vec<String>> = server_entries
            .iter()
            .filter(|s| s.auth_type.as_deref() == Some("headers"))
            .map(|s| (s.name.clone(), Vec::new()))
            .collect();
        let all_headers = match credentials::db_list_all_http_header_names(&conn).await {
            Ok(rows) => rows,
            Err(e) => {
                return internal_error(format!("db_list_all_http_header_names: {e:#}"))
                    .into_response();
            }
        };
        for (server_name, header_name) in all_headers {
            if let Some(list) = header_names.get_mut(&server_name) {
                list.push(display_header_name(header_name));
            }
        }
        (auth_types, header_names)
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
    let right = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        registry.right.clone()
    };
    let conn_arc = match right.get_conn(&req.agent).await {
        Ok(c) => c,
        Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
    };

    let servers = {
        let conn = conn_arc.lock().await;
        match credentials::db_list_servers(&conn).await {
            Ok(s) => s,
            Err(e) => return internal_error(format!("db_list_servers: {e:#}")).into_response(),
        }
    };

    let content = right_codegen::generate_mcp_instructions_md(&servers);
    Json(McpInstructionsResponse {
        instructions: content,
    })
    .into_response()
}

async fn handle_reload(State(state): State<InternalState>) -> axum::response::Response {
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

        // Determine mTLS dir for sandbox agents
        let agent_config = right_agent::agent::discovery::parse_agent_config(&agent_dir)
            .ok()
            .flatten();
        let mtls_dir = match &agent_config {
            Some(config)
                if *config.sandbox_mode() == right_agent::agent::SandboxMode::Openshell =>
            {
                match right_openshell::openshell::preflight_check() {
                    right_openshell::openshell::OpenShellStatus::Ready(dir) => Some(dir),
                    _ => None,
                }
            }
            _ => None,
        };

        // Create backend registry
        let right = crate::right_backend::RightBackend::new(state.agents_dir.clone(), mtls_dir);
        let registry = crate::aggregator::BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            agent_dir: agent_dir.clone(),
            hindsight: None,
        };
        state.dispatcher.agents.insert(agent_name.clone(), registry);

        // Add to in-memory token map
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
            state.dispatcher.agents.remove(&agent_name);
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
        right_db::open_db(&agent_dir, true).await.unwrap();

        let right = RightBackend::new(agents_dir, None);
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
        let refresh_senders: RefreshSenders = Arc::new(std::collections::HashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(std::collections::HashMap::new());

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

        let router = internal_router(
            Arc::clone(&dispatcher),
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            tmp.join("agents"),
            open_provider_store(tmp).await,
        );
        (router, dispatcher)
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
        let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;

        // Pre-insert a DB row with no matching in-memory proxy. This simulates
        // an orphan left behind when the aggregator restarted but the proxy
        // failed to reconnect.
        let conn_arc = {
            let registry = dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered");
            registry
                .right
                .get_conn("test-agent")
                .await
                .expect("test db connection")
        };
        {
            let conn = conn_arc.lock().await;
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

        let conn = conn_arc.lock().await;
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

        let (agent_dir, proxies, conn_arc) = {
            let registry = dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered");
            (
                registry.agent_dir.clone(),
                Arc::clone(&registry.proxies),
                registry
                    .right
                    .get_conn("test-agent")
                    .await
                    .expect("test db connection"),
            )
        };
        {
            let conn = conn_arc.lock().await;
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
            agent_dir,
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

        let (agent_dir, proxies, conn_arc) = {
            let registry = dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered");
            (
                registry.agent_dir.clone(),
                Arc::clone(&registry.proxies),
                registry
                    .right
                    .get_conn("test-agent")
                    .await
                    .expect("test db connection"),
            )
        };
        {
            let conn = conn_arc.lock().await;
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
            agent_dir,
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
            let conn = conn_arc.lock().await;
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

        let (status, _body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "nonexistent",
                "name": "notion",
                "url": "https://mcp.notion.com/mcp"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
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
        let refresh_senders: RefreshSenders = Arc::new(std::collections::HashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(std::collections::HashMap::new());

        let app = internal_router(
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            tmp.path().join("agents"),
            open_provider_store(tmp.path()).await,
        );

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
        let refresh_senders: RefreshSenders = Arc::new(std::collections::HashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(std::collections::HashMap::new());

        let app = internal_router(
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map.clone(),
            token_map_path,
            agents_dir.clone(),
            open_provider_store(tmp.path()).await,
        );

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

        let refresh_senders: RefreshSenders = Arc::new(std::collections::HashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(std::collections::HashMap::new());

        let app = internal_router(
            dispatcher.clone(),
            refresh_senders,
            reconnect_managers,
            token_map.clone(),
            token_map_path,
            agents_dir.clone(),
            open_provider_store(tmp.path()).await,
        );

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
        let conn_arc = {
            let registry = dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered");
            registry
                .right
                .get_conn("test-agent")
                .await
                .expect("test db connection")
        };
        let dead_url = "http://127.0.0.1:1/mcp".to_string();
        {
            let conn = conn_arc.lock().await;
            credentials::db_add_server(&conn, "obsidian", &dead_url)
                .await
                .unwrap();
        }
        {
            // Clone the Arc out and drop the DashMap ref before awaiting the lock.
            let proxies = Arc::clone(&dispatcher.agents.get("test-agent").unwrap().proxies);
            let backend = Arc::new(ProxyBackend::new(
                "obsidian".into(),
                tmp.path().join("agents/test-agent"),
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
        let conn_arc = {
            let registry = dispatcher
                .agents
                .get("test-agent")
                .expect("test-agent registered");
            registry
                .right
                .get_conn("test-agent")
                .await
                .expect("test db connection")
        };
        {
            let conn = conn_arc.lock().await;
            credentials::db_add_server(&conn, "obsidian", &dead_url)
                .await
                .unwrap();
        }
        let backend = Arc::new(ProxyBackend::new(
            "obsidian".into(),
            tmp.path().join("agents/test-agent"),
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
