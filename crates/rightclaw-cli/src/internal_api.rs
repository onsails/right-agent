//! Internal REST API served on a Unix domain socket for bot→aggregator IPC.
//!
//! Exposes endpoints for MCP server management (add/remove/set-token) that are
//! accessible only to the Telegram bot process, not to agents.

use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use rightclaw::mcp::credentials::{self, CredentialError};
use rightclaw::mcp::proxy::{AuthMethod, ProxyBackend};
use serde::{Deserialize, Serialize};

use crate::aggregator::{ReconnectManagers, RefreshSenders, ToolDispatcher};
use rightclaw::mcp::refresh::{OAuthServerState, RefreshMessage};

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
}

pub(crate) fn internal_router(
    dispatcher: Arc<ToolDispatcher>,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
) -> Router {
    let state = InternalState { dispatcher, refresh_senders, reconnect_managers };
    Router::new()
        .route("/mcp-add", post(handle_mcp_add))
        .route("/mcp-remove", post(handle_mcp_remove))
        .route("/set-token", post(handle_set_token))
        .route("/mcp-list", post(handle_mcp_list))
        .route("/mcp-instructions", post(handle_mcp_instructions))
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

    // Determine AuthMethod from request fields
    let auth_method = AuthMethod::from_db(req.auth_type.as_deref(), req.auth_header.as_deref());

    // Get DB connection, agent_dir, and proxies from DashMap (single lookup, scope guard before await)
    let (conn_arc, agent_dir, proxies_lock) = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        let conn = match registry.right.get_conn(&req.agent) {
            Ok(c) => c,
            Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
        };
        (conn, registry.agent_dir.clone(), Arc::clone(&registry.proxies))
    };

    {
        let conn = match conn_arc.lock() {
            Ok(c) => c,
            Err(e) => return internal_error(format!("mutex poisoned: {e}")).into_response(),
        };
        if let Err(e) = credentials::db_add_server(&conn, &req.name, &req.url) {
            return internal_error(format!("db_add_server: {e:#}")).into_response();
        }
        // Persist auth fields if provided
        if let Some(ref auth_type_str) = req.auth_type {
            if let Err(e) = credentials::db_set_auth(
                &conn,
                &req.name,
                auth_type_str,
                req.auth_header.as_deref(),
                req.auth_token.as_deref(),
            ) {
                return internal_error(format!("db_set_auth: {e:#}")).into_response();
            }
        }
    }

    // Create ProxyBackend with the resolved auth method and optional token
    let token = Arc::new(tokio::sync::RwLock::new(req.auth_token.clone()));
    let backend = ProxyBackend::new(
        req.name.clone(),
        agent_dir,
        req.url.clone(),
        token,
        auth_method,
    );
    let handle = Arc::new(backend);

    // Skip connection for OAuth servers without a token — they need /mcp auth first.
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
                warning: Some("OAuth server registered. Run /mcp auth to authenticate.".into()),
            }),
        )
            .into_response();
    }

    // Attempt connection (with timeout to prevent hanging on slow upstreams)
    tracing::info!(server = %req.name, url = %req.url, "mcp-add: connecting to upstream MCP server");
    let connect_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
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
                    warning: None,
                }),
            )
                .into_response()
        }
        Err(e) => {
            // Remove from SQLite on connection failure (reuse conn_arc from initial lookup)
            {
                let conn = match conn_arc.lock() {
                    Ok(c) => c,
                    Err(poison) => {
                        return internal_error(format!("mutex poisoned: {poison}")).into_response()
                    }
                };
                // Best-effort rollback — ignore ServerNotFound
                match credentials::db_remove_server(&conn, &req.name) {
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
    // Reject protected names
    if req.name == rightclaw::mcp::PROTECTED_MCP_SERVER || req.name == "rightmeta" {
        return validation_error(format!(
            "'{}' is a protected server and cannot be removed",
            req.name
        ))
        .into_response();
    }

    // Clone proxies Arc and conn (scope DashMap guard before await)
    let (proxies_lock, conn_arc) = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        let conn = match registry.right.get_conn(&req.agent) {
            Ok(c) => c,
            Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
        };
        (Arc::clone(&registry.proxies), conn)
    };

    // Remove from proxies
    let removed = {
        let mut proxies = proxies_lock.write().await;
        proxies.remove(&req.name).is_some()
    };

    if !removed {
        return not_found(format!(
            "server '{}' not found for agent '{}'",
            req.name, req.agent
        ))
        .into_response();
    }

    // Remove from SQLite
    {
        let conn = match conn_arc.lock() {
            Ok(c) => c,
            Err(e) => return internal_error(format!("mutex poisoned: {e}")).into_response(),
        };
        // Ignore ServerNotFound — we already removed from the in-memory map.
        match credentials::db_remove_server(&conn, &req.name) {
            Ok(()) => {}
            Err(CredentialError::ServerNotFound(_)) => {}
            Err(e) => return internal_error(format!("db_remove_server: {e:#}")).into_response(),
        }
    }

    (StatusCode::OK, Json(McpRemoveResponse { removed: true })).into_response()
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

    // Update the token in the shared Arc<RwLock<Option<String>>>
    {
        let mut token_guard = handle.token().write().await;
        *token_guard = Some(req.access_token.clone());
    }

    // Persist OAuth state to SQLite
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(req.expires_in as i64);
    let expires_at_str = expires_at.to_rfc3339();
    {
        let conn_arc = {
            let Some(registry) = dispatcher.agents.get(&req.agent) else {
                return not_found("agent_not_found").into_response();
            };
            match registry.right.get_conn(&req.agent) {
                Ok(c) => c,
                Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
            }
        };
        let conn = match conn_arc.lock() {
            Ok(c) => c,
            Err(e) => return internal_error(format!("mutex poisoned: {e}")).into_response(),
        };
        if let Err(e) = rightclaw::mcp::credentials::db_set_oauth_state(
            &conn,
            &req.server,
            &req.access_token,
            Some(&req.refresh_token),
            &req.token_endpoint,
            &req.client_id,
            req.client_secret.as_deref(),
            &expires_at_str,
        ) {
            return internal_error(format!("db_set_oauth_state: {e:#}")).into_response();
        }
    }

    // Cancel stale reconnect if one is running for this server.
    if let Some(mgr) = state.reconnect_managers.get(&req.agent) {
        mgr.lock().await.cancel(&req.server);
    }

    // Reconnect in background with the new token.
    let server_name = req.server.clone();
    let handle_clone = Arc::clone(&handle);
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        match handle_clone.connect(client).await {
            Ok(_) => tracing::info!(server = %server_name, "reconnected after OAuth token update"),
            Err(e) => tracing::warn!(server = %server_name, err = %format!("{e:#}"), "reconnect after OAuth failed"),
        }
    });

    // Notify refresh scheduler so it schedules future token refreshes
    if let Some(tx) = state.refresh_senders.get(&req.agent) {
        let entry = OAuthServerState {
            refresh_token: Some(req.refresh_token.clone()),
            token_endpoint: req.token_endpoint.clone(),
            client_id: req.client_id.clone(),
            client_secret: req.client_secret.clone(),
            expires_at,
            server_url: handle.url().to_string(),
        };
        if let Err(e) = tx
            .send(RefreshMessage::NewEntry {
                server_name: req.server.clone(),
                state: entry,
                token: handle.token().clone(),
            })
            .await
        {
            tracing::warn!(agent = req.agent.as_str(), server = req.server.as_str(), "failed to notify refresh scheduler: {e:#}");
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
    let Some(registry) = dispatcher.agents.get(&req.agent) else {
        return not_found(format!("agent '{}' not found", req.agent)).into_response();
    };

    let mut servers = Vec::new();

    // Right backend (always connected)
    servers.push(McpServerStatus {
        name: "right".into(),
        url: None,
        status: "connected".into(),
        tool_count: registry.right.tools_list().len(),
        auth_type: None,
    });

    // Read auth_type from SQLite (preserves "oauth" — AuthMethod enum has no OAuth variant)
    let db_auth_types: std::collections::HashMap<String, Option<String>> = {
        match registry.right.get_conn(&req.agent) {
            Ok(conn_arc) => {
                let conn = conn_arc.lock().unwrap_or_else(|e| e.into_inner());
                credentials::db_list_servers(&conn)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| (s.name, s.auth_type))
                    .collect()
            }
            Err(_) => std::collections::HashMap::new(),
        }
    };

    // External proxy backends
    let proxies = registry.proxies.read().await;
    for (name, proxy) in proxies.iter() {
        let status = proxy.status().await;
        let tool_count = proxy.try_tools().map(|t| t.len()).unwrap_or(0);
        let auth_type = db_auth_types
            .get(name)
            .cloned()
            .flatten()
            .or_else(|| Some(proxy.auth_method().to_string()));
        servers.push(McpServerStatus {
            name: name.clone(),
            url: Some(proxy.url().to_string()),
            status: status.to_string(),
            tool_count,
            auth_type,
        });
    }

    Json(McpListResponse { servers }).into_response()
}

async fn handle_mcp_instructions(
    State(state): State<InternalState>,
    Json(req): Json<McpInstructionsRequest>,
) -> axum::response::Response {
    let dispatcher = &state.dispatcher;
    let conn_arc = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        match registry.right.get_conn(&req.agent) {
            Ok(c) => c,
            Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
        }
    };

    let servers = {
        let conn = match conn_arc.lock() {
            Ok(c) => c,
            Err(e) => return internal_error(format!("mutex poisoned: {e}")).into_response(),
        };
        match credentials::db_list_servers(&conn) {
            Ok(s) => s,
            Err(e) => return internal_error(format!("db_list_servers: {e:#}")).into_response(),
        }
    };

    let content = rightclaw::codegen::generate_mcp_instructions_md(&servers);
    Json(McpInstructionsResponse {
        instructions: content,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::Request;
    use tower::ServiceExt;

    fn make_test_dispatcher(tmp: &std::path::Path) -> Arc<ToolDispatcher> {
        use crate::aggregator::BackendRegistry;
        use crate::right_backend::RightBackend;
        use dashmap::DashMap;
        use std::collections::HashMap;

        let agents_dir = tmp.join("agents");
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir_all(&agent_dir).unwrap();

        let right = RightBackend::new(agents_dir, None);
        let registry = BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            agent_dir,
        };

        let agents = DashMap::new();
        agents.insert("test-agent".into(), registry);
        Arc::new(ToolDispatcher { agents })
    }

    fn make_test_router(tmp: &std::path::Path) -> Router {
        let dispatcher = make_test_dispatcher(tmp);
        let refresh_senders: RefreshSenders = Arc::new(std::collections::HashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(std::collections::HashMap::new());
        internal_router(dispatcher, refresh_senders, reconnect_managers)
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

    #[tokio::test]
    async fn mcp_add_validates_name_reserved() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path());

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
    async fn mcp_add_validates_name_double_underscore() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path());

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
    async fn mcp_add_validates_url_non_https() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path());

        let (status, body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "notion",
                "url": "http://mcp.notion.com/mcp"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body["error"].as_str().unwrap().contains("HTTPS"),
            "expected HTTPS error, got: {body}"
        );
    }

    #[tokio::test]
    async fn mcp_add_validates_url_private_ip() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path());

        let (status, _body) = send_json(
            app,
            "/mcp-add",
            serde_json::json!({
                "agent": "test-agent",
                "name": "notion",
                "url": "https://192.168.1.1/mcp"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn mcp_remove_protected_name_right() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path());

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
        let app = make_test_router(tmp.path());

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
        let app = make_test_router(tmp.path());

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
        let app = make_test_router(tmp.path());

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
    async fn set_token_agent_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path());

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
        let app = make_test_router(tmp.path());

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
    async fn mcp_add_agent_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path());

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
        let app = make_test_router(tmp.path());

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
        assert!(right["url"].is_null(), "right backend should not have a url");
    }

    #[tokio::test]
    async fn mcp_list_unknown_agent_returns_404() {
        let tmp = tempfile::tempdir().unwrap();
        let app = make_test_router(tmp.path());

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
        let app = make_test_router(tmp.path());

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
        let app = make_test_router(tmp.path());

        let (status, _body) = send_json(
            app,
            "/mcp-instructions",
            serde_json::json!({ "agent": "nonexistent" }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
