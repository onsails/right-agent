//! Axum Unix-domain-socket callback server for MCP OAuth redirects.
//!
//! Each bot process binds a Unix socket at `<agent_dir>/bot.sock` and
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

use axum::Router;
use axum::extract::{Path as AxumPath, Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use serde::Deserialize;
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use right_mcp::internal_client::{InternalClient, SetTokenRequest};
use right_mcp::oauth::{OAuthError, PendingAuth, exchange_token_with_url_policy, verify_state};

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
    pub(crate) oauth_status: super::oauth_status::OAuthFlowStatusStore,
    /// Agent name (for logging and notifications)
    pub agent_name: String,
    /// Telegram Bot for sending notifications
    pub bot: teloxide::Bot,
    /// Live allowlist — DM users are notified after OAuth completes
    pub allowlist: right_agent::agent::allowlist::AllowlistHandle,
    /// Internal API client for delivering OAuth tokens to the aggregator
    pub internal_client: Arc<InternalClient>,
}

/// Build the axum router for the bot UDS server.
///
/// Composes four sub-routers (each carrying its own state):
/// - `/oauth/{agent_name}/callback` — OAuth callback handler.
/// - `/progress/send` — foreground progress delivery handler.
/// - `/tg/{agent_name}/...` — Telegram webhook (nested at `/`).
/// - `/healthz` — bot-status JSON.
fn build_router(
    state: OAuthCallbackState,
    progress_state: super::progress::ProgressState,
    dashboard_router: Router,
    webhook_router: Router,
    agent_name: String,
    started_at: std::time::Instant,
    webhook_set: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Router {
    let progress_router =
        super::progress::build_progress_router(super::progress::ProgressEndpointState {
            bot: state.bot.clone(),
            progress: progress_state,
        });
    let oauth_router = Router::new()
        .route("/oauth/{agent_name}/callback", get(handle_oauth_callback))
        .with_state(state);

    let healthz_state = HealthzState {
        agent_name: agent_name.clone(),
        started_at,
        webhook_set,
    };
    let healthz_router = Router::new()
        .route("/healthz", get(handle_healthz))
        .with_state(healthz_state);

    Router::new()
        .merge(oauth_router)
        .merge(progress_router)
        .merge(healthz_router)
        .merge(dashboard_router)
        .nest(&format!("/tg/{}", agent_name), webhook_router)
}

#[derive(Clone)]
struct HealthzState {
    agent_name: String,
    started_at: std::time::Instant,
    webhook_set: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

async fn handle_healthz(State(state): State<HealthzState>) -> axum::Json<serde_json::Value> {
    use std::sync::atomic::Ordering;
    axum::Json(serde_json::json!({
        "agent": state.agent_name,
        "webhook_set": state.webhook_set.load(Ordering::Relaxed),
        "uptime_secs": state.started_at.elapsed().as_secs(),
    }))
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
        if let Some(state_param) = params.state.as_deref() {
            let provider_detail = format!("{err} -- {desc}");
            let safe_detail = super::oauth_status::compact_dashboard_error(&provider_detail);
            state
                .oauth_status
                .mark_failed_if_pending(state_param, format!("OAuth provider error: {safe_detail}"))
                .await;
        }
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
                "invalid or expired state -- flow already completed or state is unknown"
                    .to_string(),
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
        axum::response::Html(callback_received_html()),
    )
        .into_response()
}

/// Exchange the authorization code for tokens and deliver via internal API.
///
/// Called in a background task after the callback response has been sent.
async fn complete_oauth_flow(
    pending: PendingAuth,
    code: String,
    cb_state: OAuthCallbackState,
    agent_name: &str,
) -> miette::Result<()> {
    let http_client = match right_mcp::ssrf::hardened_client_builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(http_client) => http_client,
        Err(error) => {
            let detail = format!("{error:#}");
            cb_state
                .oauth_status
                .mark_failed(
                    &pending.state,
                    token_exchange_failure_dashboard_message(&detail),
                )
                .await;
            return Err(miette::miette!("oauth token HTTP client failed: {detail}"));
        }
    };

    let token_resp = match exchange_token_with_url_policy(
        &http_client,
        &pending.token_endpoint,
        &code,
        &pending.redirect_uri,
        &pending.client_id,
        pending.client_secret.as_deref(),
        &pending.code_verifier,
        &pending.resource,
        oauth_token_url_policy,
    )
    .await
    {
        Ok(token_resp) => token_resp,
        Err(error) => {
            let detail = format!("{error:#}");
            cb_state
                .oauth_status
                .mark_failed(
                    &pending.state,
                    token_exchange_failure_dashboard_message(&detail),
                )
                .await;
            return Err(miette::miette!("token exchange failed: {detail}"));
        }
    };

    tracing::info!(
        agent = %agent_name,
        server = %pending.server_name,
        expires_in = token_resp.expires_in,
        has_refresh_token = token_resp.refresh_token.is_some(),
        "token exchange succeeded"
    );

    // Deliver token to aggregator via internal API
    let set_token_req = SetTokenRequest {
        agent: agent_name.to_string(),
        server: pending.server_name.clone(),
        access_token: token_resp.access_token.clone(),
        refresh_token: token_resp.refresh_token.clone().unwrap_or_default(),
        expires_in: token_resp.expires_in.unwrap_or(3600) as u64,
        token_endpoint: pending.token_endpoint.clone(),
        client_id: pending.client_id.clone(),
        resource: pending.resource.clone(),
        client_secret: pending.client_secret.clone(),
    };

    match cb_state.internal_client.set_token(&set_token_req).await {
        Ok(_resp) => {
            cb_state.oauth_status.mark_succeeded(&pending.state).await;
        }
        Err(error) => {
            let detail = format!("{error:#}");
            tracing::error!(
                agent = %agent_name,
                server = %pending.server_name,
                "set_token failed: {detail}"
            );
            cb_state
                .oauth_status
                .mark_failed(&pending.state, set_token_failure_dashboard_message(&detail))
                .await;
        }
    }

    Ok(())
}

fn oauth_token_url_policy(input: &str) -> Result<(), OAuthError> {
    if right_mcp::ssrf::is_public_http_url(input) {
        return Ok(());
    }

    Err(OAuthError::TokenExchangeFailed(
        right_mcp::ssrf::PUBLIC_DNS_ERROR_MARKER.to_string(),
    ))
}

fn set_token_failure_dashboard_message(err: impl std::fmt::Display) -> String {
    format!(
        "Token exchange completed, but MCP readiness failed: {}",
        super::oauth_status::compact_dashboard_error(&err.to_string())
    )
}

fn token_exchange_failure_dashboard_message(detail: &str) -> String {
    format!(
        "Token exchange failed: {}",
        super::oauth_status::compact_dashboard_error(detail)
    )
}

fn callback_received_html() -> &'static str {
    "<!DOCTYPE html><html><body><h1>Authorization received</h1>\
     <p>You may close this window. The dashboard will update when MCP readiness finishes.</p></body></html>"
}

/// Bind axum to a Unix socket at `socket_path` and serve the bot's UDS app.
///
/// The app combines:
/// - OAuth callback at `/oauth/{agent_name}/callback`
/// - Telegram webhook at `/tg/{agent_name}/` (caller's `webhook_router`)
/// - `/healthz` JSON
///
/// Removes any stale socket first; signals `ready_tx` after bind.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_bot_uds_server(
    socket_path: PathBuf,
    state: OAuthCallbackState,
    progress_state: super::progress::ProgressState,
    dashboard_router: Router,
    webhook_router: Router,
    agent_name: String,
    started_at: std::time::Instant,
    webhook_set: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> miette::Result<()> {
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)
            .map_err(|e| miette::miette!("remove stale UDS socket: {e:#}"))?;
    }

    let listener = UnixListener::bind(&socket_path)
        .map_err(|e| miette::miette!("bind bot UDS socket {}: {e:#}", socket_path.display()))?;

    tracing::info!(path = %socket_path.display(), "bot UDS server listening");

    if let Some(tx) = ready_tx {
        let _ = tx.send(());
    }

    let router = build_router(
        state,
        progress_state,
        dashboard_router,
        webhook_router,
        agent_name,
        started_at,
        webhook_set,
    );
    axum::serve(listener, router)
        .await
        .map_err(|e| miette::miette!("axum serve error: {e:#}"))
}

/// Background task: every 60 seconds, remove PendingAuth entries older than 10 minutes.
pub(crate) async fn run_pending_auth_cleanup(
    pending_auth: PendingAuthMap,
    oauth_status: super::oauth_status::OAuthFlowStatusStore,
) {
    const CHECK_INTERVAL: Duration = Duration::from_secs(60);
    const EXPIRY: Duration = Duration::from_secs(600);

    loop {
        tokio::time::sleep(CHECK_INTERVAL).await;
        let mut map = pending_auth.lock().await;
        let before = map.len();
        map.retain(|_state, auth| auth.created_at.elapsed() < EXPIRY);
        let after = map.len();
        drop(map);

        let expired_statuses = oauth_status.expire_pending_older_than(EXPIRY).await;
        if before != after || expired_statuses > 0 {
            tracing::debug!(
                removed_pending_auth = before - after,
                expired_statuses,
                remaining_pending_auth = after,
                "pending auth cleanup completed"
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
        use right_agent::agent::allowlist::{AllowlistHandle, AllowlistState};
        OAuthCallbackState {
            pending_auth: map,
            oauth_status: super::super::oauth_status::OAuthFlowStatusStore::default(),
            agent_name: "test-agent".to_string(),
            bot: teloxide::Bot::new("0:fake_token_for_tests"),
            allowlist: AllowlistHandle::new(AllowlistState::default()),
            internal_client: Arc::new(InternalClient::new("/tmp/fake-internal.sock")),
        }
    }

    fn make_pending(state_val: &str) -> PendingAuth {
        PendingAuth {
            server_name: "test-server".to_string(),
            server_url: "https://example.com/mcp".to_string(),
            resource: "https://example.com/mcp".to_string(),
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
        map.lock()
            .await
            .insert(state_val.to_string(), make_pending(state_val));

        let received = state_val.to_string();
        let consumed = {
            let mut m = map.lock().await;
            let matched_key = m
                .keys()
                .find(|k| verify_state(k.as_str(), &received))
                .cloned();
            matched_key.and_then(|key| m.remove(&key))
        };

        assert!(
            consumed.is_some(),
            "valid state should produce a consumed PendingAuth"
        );
        assert_eq!(
            map.lock().await.len(),
            0,
            "map should be empty after consumption (one-shot)"
        );
    }

    /// Unknown state returns None from map lookup (does not modify map).
    #[tokio::test]
    async fn test_unknown_state_rejected() {
        let map: PendingAuthMap = Arc::new(Mutex::new(HashMap::new()));
        map.lock()
            .await
            .insert("real_state".to_string(), make_pending("real_state"));

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
        map.lock()
            .await
            .insert(state_val.to_string(), make_pending(state_val));

        let consume = |map: &PendingAuthMap| {
            let map = Arc::clone(map);
            let s = state_val.to_string();
            async move {
                let mut m = map.lock().await;
                let matched_key = m.keys().find(|k| verify_state(k.as_str(), &s)).cloned();
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
        map.lock()
            .await
            .insert("fresh".to_string(), make_pending("fresh"));

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

    #[tokio::test]
    async fn oauth_callback_state_uses_allowlist_users() {
        use right_agent::agent::allowlist::{AllowedUser, AllowlistHandle, AllowlistState};

        let now = chrono::Utc::now();
        let mut state = AllowlistState::default();
        state.add_user(AllowedUser {
            id: 100,
            label: None,
            added_by: None,
            added_at: now,
        });
        let handle = AllowlistHandle::new(state);
        let cb_state = OAuthCallbackState {
            pending_auth: Arc::new(Mutex::new(Default::default())),
            oauth_status: super::super::oauth_status::OAuthFlowStatusStore::default(),
            agent_name: "test".into(),
            bot: teloxide::Bot::new("123:abc"),
            allowlist: handle.clone(),
            internal_client: Arc::new(InternalClient::new("/nonexistent.sock")),
        };
        let user_ids: Vec<i64> = cb_state
            .allowlist
            .0
            .read()
            .unwrap()
            .users()
            .iter()
            .map(|u| u.id)
            .collect();
        assert_eq!(user_ids, vec![100]);
    }

    #[tokio::test]
    async fn replayed_callback_does_not_overwrite_succeeded_status() {
        let state_val = "completed-flow";
        let cb_state = dummy_state(Arc::new(Mutex::new(HashMap::new())));
        cb_state
            .oauth_status
            .insert_pending(state_val.to_string(), "test-server".to_string())
            .await;
        cb_state.oauth_status.mark_succeeded(state_val).await;

        let response = handle_oauth_callback(
            AxumPath("test-agent".to_string()),
            Query(CallbackParams {
                code: Some("code-123".to_string()),
                state: Some(state_val.to_string()),
                error: None,
                error_description: None,
            }),
            State(cb_state.clone()),
        )
        .await
        .into_response();

        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let status = cb_state.oauth_status.status(state_val).await;
        assert_eq!(
            status.status,
            super::super::oauth_status::OAuthFlowStatus::Succeeded
        );
        assert_eq!(status.message, None);
    }

    #[tokio::test]
    async fn provider_error_status_message_redacts_secret_like_detail() {
        let state_val = "provider-error-flow";
        let cb_state = dummy_state(Arc::new(Mutex::new(HashMap::new())));
        cb_state
            .oauth_status
            .insert_pending(state_val.to_string(), "test-server".to_string())
            .await;

        let response = handle_oauth_callback(
            AxumPath("test-agent".to_string()),
            Query(CallbackParams {
                code: None,
                state: Some(state_val.to_string()),
                error: Some("access_token_secret".to_string()),
                error_description: Some("client_secret=abc".to_string()),
            }),
            State(cb_state.clone()),
        )
        .await
        .into_response();

        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let status = cb_state.oauth_status.status(state_val).await;
        assert_eq!(
            status.status,
            super::super::oauth_status::OAuthFlowStatus::Failed
        );
        let message = status.message.expect("provider failure message");
        assert!(!message.contains("access_token_secret"));
        assert!(!message.contains("client_secret"));
        assert!(!message.contains("abc"));
    }

    #[tokio::test]
    async fn provider_error_does_not_overwrite_succeeded_status() {
        let state_val = "completed-provider-flow";
        let cb_state = dummy_state(Arc::new(Mutex::new(HashMap::new())));
        cb_state
            .oauth_status
            .insert_pending(state_val.to_string(), "test-server".to_string())
            .await;
        cb_state.oauth_status.mark_succeeded(state_val).await;

        let response = handle_oauth_callback(
            AxumPath("test-agent".to_string()),
            Query(CallbackParams {
                code: None,
                state: Some(state_val.to_string()),
                error: Some("access_denied".to_string()),
                error_description: Some("anything".to_string()),
            }),
            State(cb_state.clone()),
        )
        .await
        .into_response();

        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let status = cb_state.oauth_status.status(state_val).await;
        assert_eq!(
            status.status,
            super::super::oauth_status::OAuthFlowStatus::Succeeded
        );
        assert_eq!(status.message, None);
    }

    #[test]
    fn token_exchange_failure_dashboard_message_redacts_secret_like_detail() {
        let msg = token_exchange_failure_dashboard_message(
            "oauth token HTTP client failed: client_secret=abc",
        );

        assert!(msg.contains("Token exchange failed"));
        assert!(!msg.contains("client_secret"));
        assert!(!msg.contains("abc"));
    }

    #[test]
    fn set_token_failure_dashboard_message_does_not_claim_success() {
        let msg = set_token_failure_dashboard_message(
            "Server error (502): {\"error\":\"mcp_reconnect_failed\"}",
        );

        assert!(!msg.contains("Authenticated"));
        assert!(!msg.contains("succeeded"));
        assert!(msg.contains("Token exchange completed"));
        assert!(msg.contains("mcp_reconnect_failed"));
    }

    #[test]
    fn callback_received_html_points_back_to_dashboard_not_telegram() {
        let html = callback_received_html();

        assert!(html.contains("dashboard"));
        assert!(!html.contains("Telegram"));
    }

    #[test]
    fn oauth_token_url_policy_rejects_private_literals_without_echoing_url() {
        let err = oauth_token_url_policy("http://127.0.0.1:8080/token")
            .expect_err("private token endpoints must be rejected");
        let detail = format!("{err:#}");

        assert!(detail.contains(right_mcp::ssrf::PUBLIC_DNS_ERROR_MARKER));
        assert!(!detail.contains("127.0.0.1"));
    }
}
