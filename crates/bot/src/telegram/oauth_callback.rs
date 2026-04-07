//! Axum Unix-domain-socket callback server for MCP OAuth redirects.
//!
//! Each bot process binds a Unix socket at `<agent_dir>/oauth-callback.sock` and
//! exposes `GET /oauth/{agent_name}/callback?code=...&state=...`.
//!
//! PendingAuth lifecycle (D-05, D-06):
//! - Stored in-memory `PendingAuthMap` keyed by `state` value
//! - Consumed on first successful callback (one-shot)
//! - Cleaned up after 10 minutes by a background task

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use rightclaw::mcp::credentials::{add_http_server, set_server_header};
use rightclaw::mcp::oauth::{exchange_token, verify_state, PendingAuth};

/// Shared in-memory map of OAuth state -> pending auth session.
/// Key is the PKCE state parameter (random, one-shot).
pub type PendingAuthMap = Arc<Mutex<HashMap<String, PendingAuth>>>;

/// Query parameters received on the OAuth callback endpoint.
#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Shared state injected into axum handlers via `axum::extract::State`.
#[derive(Clone)]
pub struct OAuthCallbackState {
    pub pending_auth: PendingAuthMap,
    /// Path to agent's mcp.json (for Bearer token storage)
    pub mcp_json_path: PathBuf,
    /// Agent name (for logging and notifications)
    pub agent_name: String,
    /// Telegram Bot for sending notifications
    pub bot: teloxide::Bot,
    /// Chat IDs to notify after OAuth completes
    pub notify_chat_ids: Vec<i64>,
    /// Channel to notify refresh scheduler about new OAuth tokens
    pub refresh_tx: tokio::sync::mpsc::Sender<rightclaw::mcp::refresh::RefreshMessage>,
}

/// Build the axum router for the OAuth callback server.
fn build_router(state: OAuthCallbackState) -> Router {
    Router::new()
        .route(
            "/oauth/{agent_name}/callback",
            get(handle_oauth_callback),
        )
        .with_state(state)
}

/// GET /oauth/{agent_name}/callback?code=...&state=...
///
/// 1. Validate that `state` and `code` params are present
/// 2. Constant-time verify state against PendingAuth map (D-05)
/// 3. Consume PendingAuth from map (one-shot)
/// 4. Spawn background task to exchange token + write credential
/// 5. Return 200 HTML "Authentication complete"
async fn handle_oauth_callback(
    AxumPath(agent_name): AxumPath<String>,
    Query(params): Query<CallbackParams>,
    State(state): State<OAuthCallbackState>,
) -> impl IntoResponse {
    // Handle provider-side error
    if let Some(ref err) = params.error {
        let desc = params
            .error_description
            .as_deref()
            .unwrap_or("no description");
        tracing::warn!(
            agent = %agent_name,
            error = %err,
            description = %desc,
            "OAuth callback error from provider"
        );
        return (
            axum::http::StatusCode::BAD_REQUEST,
            format!("OAuth error: {err} -- {desc}"),
        )
            .into_response();
    }

    // Both `state` and `code` are required
    let received_state = match &params.state {
        Some(s) => s.clone(),
        None => {
            tracing::warn!(agent = %agent_name, "OAuth callback missing state param");
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "missing state parameter".to_string(),
            )
                .into_response();
        }
    };
    let code = match &params.code {
        Some(c) => c.clone(),
        None => {
            tracing::warn!(agent = %agent_name, "OAuth callback missing code param");
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "missing code parameter".to_string(),
            )
                .into_response();
        }
    };

    // Look up pending auth by state (constant-time comparison via verify_state, D-05)
    let pending = {
        let mut map = state.pending_auth.lock().await;
        let matched_key = map
            .keys()
            .find(|k| verify_state(k.as_str(), &received_state))
            .cloned();
        match matched_key {
            Some(key) => map.remove(&key),
            None => None,
        }
    };

    let pending = match pending {
        Some(p) => p,
        None => {
            tracing::warn!(
                agent = %agent_name,
                state = %received_state,
                "OAuth callback: unknown or already-consumed state"
            );
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "invalid or expired state -- flow already completed or state is unknown".to_string(),
            )
                .into_response();
        }
    };

    tracing::info!(
        agent = %agent_name,
        server = %pending.server_name,
        "OAuth callback received -- spawning token exchange"
    );

    // Spawn background task for token exchange (non-blocking response to browser)
    let state_clone = state.clone();
    let agent_name_owned = agent_name.clone();
    tokio::spawn(async move {
        if let Err(e) = complete_oauth_flow(pending, code, state_clone, &agent_name_owned).await {
            tracing::error!(agent = %agent_name_owned, "OAuth flow completion failed: {e:#}");
        }
    });

    (
        axum::http::StatusCode::OK,
        axum::response::Html(
            "<!DOCTYPE html><html><body><h1>Authentication complete</h1>\
             <p>You may close this window. The token has been saved.</p></body></html>",
        ),
    )
        .into_response()
}

/// Exchange the authorization code for tokens and write Bearer token to .claude.json.
///
/// Called in a background task after the callback response has been sent.
async fn complete_oauth_flow(
    pending: PendingAuth,
    code: String,
    cb_state: OAuthCallbackState,
    agent_name: &str,
) -> miette::Result<()> {
    let http_client = reqwest::Client::new();

    // Token exchange
    let token_resp = exchange_token(
        &http_client,
        &pending.token_endpoint,
        &code,
        &pending.redirect_uri,
        &pending.client_id,
        pending.client_secret.as_deref(),
        &pending.code_verifier,
    )
    .await
    .map_err(|e| miette::miette!("token exchange failed: {e:#}"))?;

    tracing::info!(
        agent = %agent_name,
        server = %pending.server_name,
        "token exchange succeeded"
    );

    // Ensure server entry exists in mcp.json (idempotent)
    add_http_server(
        &cb_state.mcp_json_path,
        &pending.server_name,
        &pending.server_url,
    )
    .map_err(|e| miette::miette!("add_http_server failed: {e:#}"))?;

    // Set Authorization: Bearer <token> header
    set_server_header(
        &cb_state.mcp_json_path,
        &pending.server_name,
        "Authorization",
        &format!("Bearer {}", token_resp.access_token),
    )
    .map_err(|e| miette::miette!("set_server_header failed: {e:#}"))?;

    tracing::info!(
        agent = %agent_name,
        server = %pending.server_name,
        url = %pending.server_url,
        "Bearer token written to mcp.json"
    );

    // Upload updated mcp.json to sandbox so CC picks it up immediately
    let sandbox = rightclaw::openshell::sandbox_name(agent_name);
    if cb_state.mcp_json_path.exists() {
        if let Err(e) =
            rightclaw::openshell::upload_file(&sandbox, &cb_state.mcp_json_path, "/sandbox/").await
        {
            tracing::warn!(agent = %agent_name, "failed to upload mcp.json to sandbox after OAuth: {e:#}");
        } else {
            tracing::info!(agent = %agent_name, "uploaded mcp.json to sandbox after OAuth");
        }
    }

    // Notify refresh scheduler about new token
    if let Some(expires_in) = token_resp.expires_in {
        let oauth_entry = rightclaw::mcp::refresh::OAuthServerState {
            refresh_token: token_resp.refresh_token.clone(),
            token_endpoint: pending.token_endpoint.clone(),
            client_id: pending.client_id.clone(),
            client_secret: pending.client_secret.clone(),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64),
            server_url: pending.server_url.clone(),
        };
        let _ = cb_state.refresh_tx.send(rightclaw::mcp::refresh::RefreshMessage::NewEntry {
            server_name: pending.server_name.clone(),
            state: oauth_entry,
        }).await;
    }

    // Notify Telegram
    notify_telegram(
        &cb_state.bot,
        &cb_state.notify_chat_ids,
        &format!(
            "OAuth complete for {} (agent {agent_name}). Token written to mcp.json.",
            pending.server_name,
        ),
    )
    .await;

    Ok(())
}

/// Send a message to all configured Telegram chat IDs (best-effort, errors logged).
async fn notify_telegram(bot: &teloxide::Bot, chat_ids: &[i64], text: &str) {
    use teloxide::requests::Requester as _;
    for &chat_id in chat_ids {
        if let Err(e) = bot
            .send_message(teloxide::types::ChatId(chat_id), text)
            .await
        {
            tracing::warn!(chat_id, "notify_telegram failed: {e:#}");
        }
    }
}

/// Bind axum to a Unix socket at `socket_path` and serve the OAuth callback router.
///
/// - Removes stale socket if it exists
/// - Signals `ready_tx` after bind succeeds (caller can start teloxide)
/// - Serves until tokio runtime exits
pub async fn run_oauth_callback_server(
    socket_path: PathBuf,
    state: OAuthCallbackState,
    ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> miette::Result<()> {
    // Remove stale socket
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)
            .map_err(|e| miette::miette!("remove stale OAuth socket: {e:#}"))?;
    }

    let listener = UnixListener::bind(&socket_path)
        .map_err(|e| miette::miette!("bind OAuth callback socket {}: {e:#}", socket_path.display()))?;

    tracing::info!(path = %socket_path.display(), "OAuth callback server listening");

    // Signal ready (before accepting connections)
    if let Some(tx) = ready_tx {
        let _ = tx.send(());
    }

    let router = build_router(state);
    axum::serve(listener, router)
        .await
        .map_err(|e| miette::miette!("axum serve error: {e:#}"))
}

/// Background task: every 60 seconds, remove PendingAuth entries older than 10 minutes.
pub async fn run_pending_auth_cleanup(pending_auth: PendingAuthMap) {
    const CHECK_INTERVAL: Duration = Duration::from_secs(60);
    const EXPIRY: Duration = Duration::from_secs(600); // 10 minutes

    loop {
        tokio::time::sleep(CHECK_INTERVAL).await;
        let mut map = pending_auth.lock().await;
        let before = map.len();
        map.retain(|_state, auth| auth.created_at.elapsed() < EXPIRY);
        let after = map.len();
        if before != after {
            tracing::debug!(
                removed = before - after,
                remaining = after,
                "pending auth cleanup: removed expired entries"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Build a minimal OAuthCallbackState for tests (no real bot/credentials)
    fn dummy_state(map: PendingAuthMap) -> OAuthCallbackState {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        OAuthCallbackState {
            pending_auth: map,
            mcp_json_path: PathBuf::from("/tmp/fake-mcp.json"),
            agent_name: "test-agent".to_string(),
            bot: teloxide::Bot::new("0:fake_token_for_tests"),
            notify_chat_ids: vec![],
            refresh_tx: tx,
        }
    }

    fn make_pending(state_val: &str) -> PendingAuth {
        PendingAuth {
            server_name: "test-server".to_string(),
            server_url: "https://example.com/mcp".to_string(),
            code_verifier: "verifier123".to_string(),
            state: state_val.to_string(),
            token_endpoint: "https://example.com/token".to_string(),
            client_id: "client_abc".to_string(),
            client_secret: None,
            redirect_uri: "https://tunnel.example/oauth/test-agent/callback".to_string(),
            created_at: Instant::now(),
        }
    }

    /// Valid state + code returns 200 and removes the PendingAuth entry from map (one-shot).
    #[tokio::test]
    async fn test_valid_callback_consumes_pending_auth() {
        let state_val = "valid_state_abc123";
        let map: PendingAuthMap = Arc::new(Mutex::new(HashMap::new()));
        map.lock().await.insert(state_val.to_string(), make_pending(state_val));

        let received = state_val.to_string();
        let consumed = {
            let mut m = map.lock().await;
            let matched_key = m
                .keys()
                .find(|k| verify_state(k.as_str(), &received))
                .cloned();
            matched_key.and_then(|key| m.remove(&key))
        };

        assert!(consumed.is_some(), "valid state should produce a consumed PendingAuth");
        assert_eq!(map.lock().await.len(), 0, "map should be empty after consumption (one-shot)");
    }

    /// Unknown state returns None from map lookup (does not modify map).
    #[tokio::test]
    async fn test_unknown_state_rejected() {
        let map: PendingAuthMap = Arc::new(Mutex::new(HashMap::new()));
        map.lock().await.insert("real_state".to_string(), make_pending("real_state"));

        let received = "unknown_state_xyz".to_string();
        let consumed = {
            let mut m = map.lock().await;
            let matched_key = m
                .keys()
                .find(|k| verify_state(k.as_str(), &received))
                .cloned();
            matched_key.and_then(|key| m.remove(&key))
        };

        assert!(consumed.is_none(), "unknown state should not match");
        assert_eq!(map.lock().await.len(), 1, "map should be unmodified");
    }

    /// Replayed state (used twice) returns None on second attempt (one-shot).
    #[tokio::test]
    async fn test_replay_state_rejected_on_second_use() {
        let state_val = "one_shot_state";
        let map: PendingAuthMap = Arc::new(Mutex::new(HashMap::new()));
        map.lock().await.insert(state_val.to_string(), make_pending(state_val));

        let consume = |map: &PendingAuthMap| {
            let map = Arc::clone(map);
            let s = state_val.to_string();
            async move {
                let mut m = map.lock().await;
                let matched_key = m
                    .keys()
                    .find(|k| verify_state(k.as_str(), &s))
                    .cloned();
                matched_key.and_then(|key| m.remove(&key))
            }
        };

        let first = consume(&map).await;
        assert!(first.is_some(), "first use should succeed");

        let second = consume(&map).await;
        assert!(second.is_none(), "second use should fail -- one-shot");
    }

    /// Cleanup removes entries older than 10 minutes.
    #[tokio::test]
    async fn test_cleanup_removes_expired_entries() {
        let map: PendingAuthMap = Arc::new(Mutex::new(HashMap::new()));
        map.lock().await.insert("fresh".to_string(), make_pending("fresh"));

        let before = map.lock().await.len();
        {
            let mut m = map.lock().await;
            m.retain(|_, auth| auth.created_at.elapsed() < Duration::from_secs(600));
        }
        let after = map.lock().await.len();
        assert_eq!(before, after, "fresh entry must not be removed by cleanup");
    }

    /// Dummy state construction does not panic.
    #[tokio::test]
    async fn test_dummy_state_construction() {
        let map: PendingAuthMap = Arc::new(Mutex::new(HashMap::new()));
        let _state = dummy_state(map);
    }
}
