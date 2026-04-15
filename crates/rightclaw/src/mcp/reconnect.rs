//! Cancellable OAuth token refresh for reconnect scenarios.
//!
//! When a fresh OAuth token arrives while a stale retry loop is in progress,
//! the loop must be cancelled so it does not overwrite the fresh token.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::mcp::proxy::{BackendStatus, ProxyBackend};
use crate::mcp::refresh::{OAuthServerState, RefreshMessage};

/// Maximum retry attempts for a cancellable refresh.
const MAX_RETRIES: u32 = 3;

/// Backoff delays between retry attempts, in seconds.
const BACKOFFS: [u64; 3] = [30, 60, 120];

/// Errors returned by [`do_refresh_cancellable`].
#[derive(Debug, thiserror::Error)]
pub enum ReconnectError {
    /// The operation was cancelled via the [`CancellationToken`].
    #[error("refresh cancelled")]
    Cancelled,

    /// The token endpoint returned errors on all attempts.
    #[error("token refresh failed after {0} attempts")]
    RefreshFailed(u32),

    /// Could not connect to the token endpoint (network error on all attempts).
    #[error("token endpoint unreachable: {0}")]
    ConnectFailed(String),

    /// Refresh succeeded but the result could not be persisted.
    #[error("failed to persist refreshed token: {0}")]
    PersistFailed(String),
}

/// Attempt token refresh with retries, checking `cancel` between backoff sleeps.
///
/// Returns `(updated_state, new_access_token)` on success.
///
/// Differences from [`crate::mcp::refresh::do_refresh`]:
/// - Accepts a [`CancellationToken`] and checks it before each attempt.
/// - During backoff sleeps, races the sleep against `cancel.cancelled()` so
///   cancellation wakes up immediately rather than waiting the full delay.
/// - Returns typed [`ReconnectError`] instead of `miette::Result`.
pub async fn do_refresh_cancellable(
    client: &reqwest::Client,
    entry: &OAuthServerState,
    cancel: &CancellationToken,
) -> Result<(OAuthServerState, String), ReconnectError> {
    let refresh_token = entry
        .refresh_token
        .as_deref()
        .ok_or_else(|| ReconnectError::RefreshFailed(0))?;

    let mut last_connect_error: Option<String> = None;

    for attempt in 0..MAX_RETRIES {
        // Check cancellation before each attempt.
        if cancel.is_cancelled() {
            return Err(ReconnectError::Cancelled);
        }

        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", entry.client_id.as_str()),
        ];
        if let Some(ref secret) = entry.client_secret {
            form.push(("client_secret", secret.as_str()));
        }

        let resp = client
            .post(&entry.token_endpoint)
            .form(&form)
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let token_resp: crate::mcp::oauth::TokenResponse =
                    r.json().await.map_err(|e| {
                        tracing::warn!(attempt, "failed to parse token response: {e:#}");
                        ReconnectError::RefreshFailed(attempt + 1)
                    })?;

                let expires_in = token_resp.expires_in.unwrap_or(3600);
                let has_new_refresh = token_resp.refresh_token.is_some();
                let expires_at =
                    chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);

                tracing::info!(
                    attempt,
                    expires_in,
                    has_new_refresh,
                    %expires_at,
                    "cancellable refresh succeeded",
                );

                let access_token = token_resp.access_token.clone();
                return Ok((
                    OAuthServerState {
                        refresh_token: token_resp
                            .refresh_token
                            .or_else(|| entry.refresh_token.clone()),
                        token_endpoint: entry.token_endpoint.clone(),
                        client_id: entry.client_id.clone(),
                        client_secret: entry.client_secret.clone(),
                        expires_at,
                        server_url: entry.server_url.clone(),
                    },
                    access_token,
                ));
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                tracing::warn!(attempt, %status, %body, "cancellable refresh attempt failed");
                last_connect_error = None; // HTTP-level failure, not network
            }
            Err(e) => {
                let msg = format!("{e:#}");
                tracing::warn!(attempt, "cancellable refresh request error: {msg}");
                last_connect_error = Some(msg);
            }
        }

        // Backoff before next attempt — unless this was the last one.
        if attempt < MAX_RETRIES - 1 {
            let delay = BACKOFFS.get(attempt as usize).copied().unwrap_or(120);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(delay)) => {}
                _ = cancel.cancelled() => {
                    return Err(ReconnectError::Cancelled);
                }
            }
        }
    }

    if let Some(err) = last_connect_error {
        Err(ReconnectError::ConnectFailed(err))
    } else {
        Err(ReconnectError::RefreshFailed(MAX_RETRIES))
    }
}

/// Perform a full OAuth reconnect for a single MCP server:
/// refresh the token, persist it, notify the refresh scheduler, and reconnect.
///
/// Steps:
/// 1. Call [`do_refresh_cancellable`] — cancellable retry loop.
/// 2. Write new access token to `token_arc` (shared with [`ProxyBackend`]).
/// 3. Persist refreshed OAuth state to SQLite via [`crate::mcp::credentials::db_update_oauth_token`].
/// 4. Send [`RefreshMessage::NewEntry`] to the refresh scheduler.
/// 5. Call [`ProxyBackend::connect`] to re-establish the MCP session.
///
/// On connect failure: returns [`ReconnectError::ConnectFailed`].
/// On cancellation: returns [`ReconnectError::Cancelled`] immediately.
/// On all other errors: if backend is not already `Connected`, sets status to `NeedsAuth`.
pub async fn reconnect_task(
    server_name: String,
    backend: Arc<ProxyBackend>,
    oauth_state: OAuthServerState,
    token_arc: Arc<RwLock<Option<String>>>,
    http_client: reqwest::Client,
    agent_dir: PathBuf,
    refresh_tx: mpsc::Sender<RefreshMessage>,
    cancel: CancellationToken,
) -> Result<(), ReconnectError> {
    let refresh_result = do_refresh_cancellable(&http_client, &oauth_state, &cancel).await;

    let (new_state, access_token) = match refresh_result {
        Ok(ok) => ok,
        Err(ReconnectError::Cancelled) => {
            tracing::debug!(server = %server_name, "reconnect cancelled during refresh");
            return Err(ReconnectError::Cancelled);
        }
        Err(e) => {
            tracing::warn!(server = %server_name, "reconnect refresh failed: {e:#}");
            // Defense-in-depth: only set NeedsAuth if we're not already Connected
            // (a concurrent path may have authenticated successfully).
            if backend.status().await != BackendStatus::Connected {
                backend.set_status(BackendStatus::NeedsAuth).await;
            }
            return Err(e);
        }
    };

    // Write access token to shared state so DynamicAuthClient picks it up immediately.
    *token_arc.write().await = Some(access_token.clone());

    // Persist to SQLite.
    let conn = crate::memory::open_connection(&agent_dir)
        .map_err(|e| ReconnectError::PersistFailed(format!("{e:#}")))?;
    let expires_at = new_state.expires_at.to_rfc3339();
    crate::mcp::credentials::db_update_oauth_token(
        &conn,
        &server_name,
        &access_token,
        new_state.refresh_token.as_deref(),
        &expires_at,
    )
    .map_err(|e| ReconnectError::PersistFailed(format!("{e:#}")))?;

    // Notify refresh scheduler so it schedules the next refresh.
    refresh_tx
        .send(RefreshMessage::NewEntry {
            server_name: server_name.clone(),
            state: new_state,
            token: token_arc.clone(),
        })
        .await
        .map_err(|e| {
            tracing::error!("refresh scheduler dropped: {e:#}");
            ReconnectError::PersistFailed(format!("refresh scheduler unavailable: {e:#}"))
        })?;

    // Re-establish MCP session.
    backend
        .connect(http_client)
        .await
        .map_err(|e| ReconnectError::ConnectFailed(format!("{e:#}")))?;

    Ok(())
}

/// Manages in-flight reconnect tasks, ensuring at most one reconnect per server runs
/// at a time. Starting a new reconnect for a server automatically cancels the previous one.
pub struct ReconnectManager {
    in_flight: HashMap<String, CancellationToken>,
    refresh_tx: mpsc::Sender<RefreshMessage>,
    agent_dir: PathBuf,
}

impl ReconnectManager {
    pub fn new(refresh_tx: mpsc::Sender<RefreshMessage>, agent_dir: PathBuf) -> Self {
        Self {
            in_flight: HashMap::new(),
            refresh_tx,
            agent_dir,
        }
    }

    /// Start a reconnect task for `server_name`.
    ///
    /// If one is already in flight for this server, it is cancelled first.
    /// Returns the [`JoinHandle`] for the newly-spawned task.
    pub fn start_reconnect(
        &mut self,
        server_name: String,
        backend: Arc<ProxyBackend>,
        oauth_state: OAuthServerState,
        token_arc: Arc<RwLock<Option<String>>>,
        http_client: reqwest::Client,
    ) -> JoinHandle<Result<(), ReconnectError>> {
        // Cancel any existing in-flight reconnect for this server.
        if let Some(prev) = self.in_flight.remove(&server_name) {
            prev.cancel();
        }

        let cancel = CancellationToken::new();
        self.in_flight.insert(server_name.clone(), cancel.clone());

        let refresh_tx = self.refresh_tx.clone();
        let agent_dir = self.agent_dir.clone();

        tokio::spawn(async move {
            reconnect_task(
                server_name,
                backend,
                oauth_state,
                token_arc,
                http_client,
                agent_dir,
                refresh_tx,
                cancel,
            )
            .await
        })
    }

    /// Cancel any in-flight reconnect for `server_name`.
    pub fn cancel(&mut self, server_name: &str) {
        if let Some(token) = self.in_flight.remove(server_name) {
            token.cancel();
        }
    }

    /// Cancel all in-flight reconnects.
    pub fn cancel_all(&mut self) {
        for (_, token) in self.in_flight.drain() {
            token.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_entry(token_endpoint: String) -> OAuthServerState {
        OAuthServerState {
            refresh_token: Some("old-refresh-token".into()),
            token_endpoint,
            client_id: "test-client".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            server_url: "https://example.com/mcp".into(),
        }
    }

    /// Verify that cancellation during a backoff sleep returns `Err(Cancelled)`
    /// without waiting the full backoff duration.
    #[tokio::test]
    async fn cancellation_aborts_refresh_during_backoff() {
        // MockServer that always returns 401 — forces retry with backoff.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .expect(1) // exactly one attempt before cancellation fires
            .mount(&server)
            .await;

        let entry = make_entry(format!("{}/token", server.uri()));
        let client = reqwest::Client::new();
        let cancel = CancellationToken::new();

        tokio::time::pause();

        // Spawn the refresh in a background task so we can cancel from here.
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            do_refresh_cancellable(&client, &entry, &cancel_clone).await
        });

        // Let the first attempt complete (it hits the MockServer and gets 401).
        // Then advance time slightly — not enough to expire the 30s backoff,
        // just enough to confirm we are inside the backoff sleep.
        tokio::time::advance(Duration::from_secs(1)).await;
        // Yield so the spawned task can reach the tokio::select! inside backoff.
        tokio::task::yield_now().await;

        // Now cancel — the select! should wake immediately.
        cancel.cancel();

        // Advance time past the backoff just in case, to avoid test hangs.
        tokio::time::advance(Duration::from_secs(60)).await;

        let result = handle.await.expect("task panicked");
        assert!(
            matches!(result, Err(ReconnectError::Cancelled)),
            "expected Cancelled, got {result:?}",
        );

        // wiremock verifies exactly 1 POST was received (from the expect(1) above).
    }

    /// When all refresh retries are exhausted, the backend status must NOT be set to
    /// `NeedsAuth` if it was already `Connected` — defense-in-depth guard.
    #[tokio::test]
    async fn exhausted_retries_do_not_overwrite_connected_status() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let entry = make_entry(format!("{}/token", server.uri()));

        let tmp = tempfile::tempdir().unwrap();
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::migrations::MIGRATIONS
            .to_latest(&mut conn)
            .unwrap();

        let token_arc: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            tmp.path().to_path_buf(),
            "https://example.com/mcp".into(),
            token_arc.clone(),
            crate::mcp::proxy::AuthMethod::Bearer,
        ));
        // Pre-set status to Connected — exhausted retries must not overwrite this.
        backend.set_status(BackendStatus::Connected).await;

        let (refresh_tx, mut refresh_rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();

        tokio::time::pause();

        let handle = {
            let backend = backend.clone();
            let token_arc = token_arc.clone();
            let agent_dir = tmp.path().to_path_buf();
            let client = reqwest::Client::new();
            tokio::spawn(async move {
                reconnect_task(
                    "composio".into(),
                    backend,
                    entry,
                    token_arc,
                    client,
                    agent_dir,
                    refresh_tx,
                    cancel,
                )
                .await
            })
        };

        // Advance time through all backoffs so retries complete without hanging.
        for _ in 0..MAX_RETRIES {
            tokio::time::advance(Duration::from_secs(200)).await;
            tokio::task::yield_now().await;
        }

        let result = handle.await.expect("task panicked");
        assert!(result.is_err(), "expected error after exhausted retries, got Ok");

        // Status must still be Connected — the guard prevented the overwrite.
        assert_eq!(
            backend.status().await,
            BackendStatus::Connected,
            "exhausted retries must not overwrite Connected status"
        );

        // No NewEntry should have been sent since refresh never succeeded.
        assert!(
            refresh_rx.try_recv().is_err(),
            "no RefreshMessage::NewEntry should be sent on failure"
        );
    }

    /// When refresh succeeds, the token_arc is updated and a NewEntry is sent
    /// to the refresh scheduler.
    #[tokio::test]
    async fn successful_refresh_writes_token_and_sends_new_entry() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "access_token": "new-access-tok",
                    "refresh_token": "new-refresh-tok",
                    "expires_in": 3600,
                    "token_type": "Bearer"
                })),
            )
            .mount(&server)
            .await;

        let entry = OAuthServerState {
            refresh_token: Some("old-refresh-tok".into()),
            token_endpoint: format!("{}/token", server.uri()),
            client_id: "test-client".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            server_url: "https://example.com/mcp".into(),
        };

        let tmp = tempfile::tempdir().unwrap();
        // Initialize schema and insert the server row that db_update_oauth_token requires.
        let conn = crate::memory::open_connection(tmp.path()).unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('composio', 'https://example.com/mcp', 'oauth')",
            [],
        )
        .unwrap();
        drop(conn);

        let token_arc: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            tmp.path().to_path_buf(),
            // Fake URL — connect() will fail, which is expected.
            "https://example.com/mcp".into(),
            token_arc.clone(),
            crate::mcp::proxy::AuthMethod::Bearer,
        ));

        let (refresh_tx, mut refresh_rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let client = reqwest::Client::new();

        let result = reconnect_task(
            "composio".into(),
            backend,
            entry,
            token_arc.clone(),
            client,
            tmp.path().to_path_buf(),
            refresh_tx,
            cancel,
        )
        .await;

        // Connect to a fake URL will fail — that's expected and is non-fatal for this test.
        // We only care that token and refresh scheduler were updated before connect was attempted.
        match &result {
            Ok(()) => {} // Unexpected success — still fine for our assertions
            Err(ReconnectError::ConnectFailed(_)) => {} // Expected
            Err(other) => panic!("unexpected error: {other:?}"),
        }

        // Token must have been written to shared state.
        assert_eq!(
            *token_arc.read().await,
            Some("new-access-tok".to_string()),
            "token_arc must contain the refreshed access token"
        );

        // NewEntry must have been sent to refresh scheduler.
        let msg = refresh_rx
            .try_recv()
            .expect("expected RefreshMessage::NewEntry on refresh_rx");
        match msg {
            RefreshMessage::NewEntry {
                server_name,
                state,
                ..
            } => {
                assert_eq!(server_name, "composio");
                assert_eq!(
                    state.refresh_token.as_deref(),
                    Some("new-refresh-tok"),
                    "new refresh token must be carried in NewEntry"
                );
            }
            other => panic!("expected NewEntry, got {other:?}"),
        }
    }
}
