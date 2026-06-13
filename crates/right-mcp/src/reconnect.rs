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

use crate::proxy::{BackendStatus, ProxyBackend};
use crate::refresh::{OAuthServerState, RefreshMessage};

/// Maximum retry attempts for a cancellable refresh.
const MAX_RETRIES: u32 = 3;

/// Backoff delays between retry attempts, in seconds.
///
/// The last entry equals [`BACKOFF_FALLBACK_SECS`] so that exhausting the
/// schedule plateaus instead of regressing on the boundary.
const BACKOFFS: [u64; 3] = [30, 60, 120];

/// Fallback delay used when `BACKOFFS` is indexed past its end. MUST equal the
/// last entry of `BACKOFFS` (asserted by a unit test).
const BACKOFF_FALLBACK_SECS: u64 = 120;

fn refresh_retry_delay(attempt: u32) -> Duration {
    #[cfg(test)]
    {
        let _ = attempt;
        Duration::from_millis(10)
    }

    #[cfg(not(test))]
    {
        let delay = BACKOFFS
            .get(attempt as usize)
            .copied()
            .unwrap_or(BACKOFF_FALLBACK_SECS);
        Duration::from_secs(delay)
    }
}

/// Classification of a token endpoint refresh failure.
#[derive(Debug, thiserror::Error)]
pub enum RefreshFailure {
    /// Transient — network error, 5xx, 408, 429, or a Cloudflare challenge.
    /// Retry later.
    #[error("transient refresh failure: {0}")]
    Transient(String),

    /// Permanent — token endpoint returned a non-recoverable 4xx (typically
    /// `invalid_grant` / `invalid_client`). Refresh token is dead; user must
    /// re-authenticate from the dashboard MCP view.
    #[error("permanent refresh failure: {0}")]
    Permanent(String),
}

impl RefreshFailure {
    pub(crate) fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent(_))
    }
}

fn is_cloudflare_challenge_response(status: http::StatusCode, body: &str) -> bool {
    status == http::StatusCode::FORBIDDEN
        && (body.contains("__cf_chl_")
            || body.contains("/cdn-cgi/challenge-platform/")
            || body.contains("cf_chl_opt")
            || body.contains("Just a moment"))
}

fn refresh_http_error_detail(status: http::StatusCode, body: &str) -> String {
    if is_cloudflare_challenge_response(status, body) {
        format!("HTTP {status}: Cloudflare challenge")
    } else {
        format!("HTTP {status}: {body}")
    }
}

/// Errors returned by [`do_refresh_cancellable`] and [`reconnect_task`].
#[derive(Debug, thiserror::Error)]
pub enum ReconnectError {
    /// The operation was cancelled via the [`CancellationToken`].
    #[error("refresh cancelled")]
    Cancelled,

    /// The token endpoint refresh step failed (classified).
    #[error("refresh failed: {0}")]
    Refresh(#[from] RefreshFailure),

    /// Post-refresh `backend.connect()` (to the MCP server) failed.
    #[error("backend connect failed: {0}")]
    Connect(String),

    /// Refresh succeeded but the result could not be persisted.
    #[error("failed to persist refreshed token: {0}")]
    PersistFailed(String),
}

/// Attempt token refresh with retries, checking `cancel` between backoff sleeps.
///
/// Returns `(updated_state, new_access_token)` on success.
///
/// Cancellable refresh:
/// - Accepts a [`CancellationToken`] and checks it before each attempt.
/// - Races each in-flight token request against `cancel.cancelled()` so
///   server removal does not wait for a slow token endpoint or request timeout.
/// - During backoff sleeps, races the sleep against `cancel.cancelled()` so
///   cancellation wakes up immediately rather than waiting the full delay.
/// - Returns typed [`ReconnectError`] instead of `miette::Result`.
pub async fn do_refresh_cancellable(
    client: &reqwest::Client,
    entry: &OAuthServerState,
    cancel: &CancellationToken,
) -> Result<(OAuthServerState, String), ReconnectError> {
    let refresh_token = entry.refresh_token.as_deref().ok_or_else(|| {
        ReconnectError::Refresh(RefreshFailure::Permanent(
            "no refresh_token available".into(),
        ))
    })?;

    let mut last_error: Option<String> = None;

    for attempt in 0..MAX_RETRIES {
        // Check cancellation before each attempt.
        if cancel.is_cancelled() {
            return Err(ReconnectError::Cancelled);
        }

        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", entry.client_id.as_str()),
            ("resource", entry.resource.as_str()),
        ];
        if let Some(ref secret) = entry.client_secret {
            form.push(("client_secret", secret.as_str()));
        }

        let request = client.post(&entry.token_endpoint).form(&form).send();
        let resp = tokio::select! {
            resp = request => resp,
            _ = cancel.cancelled() => {
                return Err(ReconnectError::Cancelled);
            }
        };

        match resp {
            Ok(r) if r.status().is_success() => {
                let token_resp: crate::oauth::TokenResponse = r.json().await.map_err(|e| {
                    tracing::warn!(attempt, "failed to parse token response: {e:#}");
                    ReconnectError::Refresh(RefreshFailure::Transient(format!(
                        "malformed token response: {e:#}"
                    )))
                })?;

                let expires_in = token_resp.expires_in.unwrap_or(3600);
                let has_new_refresh = token_resp.refresh_token.is_some();
                let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);

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
                        resource: entry.resource.clone(),
                    },
                    access_token,
                ));
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                let is_transient_http = status == http::StatusCode::REQUEST_TIMEOUT
                    || status == http::StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()
                    || is_cloudflare_challenge_response(status, &body);
                let detail = refresh_http_error_detail(status, &body);
                if is_transient_http {
                    tracing::warn!(attempt, %status, detail = %detail, "cancellable refresh attempt failed (transient http)");
                    last_error = Some(detail);
                    // fall through to backoff
                } else {
                    tracing::warn!(attempt, %status, detail = %detail, "cancellable refresh attempt failed (permanent http)");
                    return Err(ReconnectError::Refresh(RefreshFailure::Permanent(detail)));
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                tracing::warn!(attempt, "cancellable refresh request error: {msg}");
                last_error = Some(msg);
            }
        }

        // Backoff before next attempt — unless this was the last one.
        if attempt < MAX_RETRIES - 1 {
            tokio::select! {
                _ = tokio::time::sleep(refresh_retry_delay(attempt)) => {}
                _ = cancel.cancelled() => {
                    return Err(ReconnectError::Cancelled);
                }
            }
        }
    }

    let detail = last_error.unwrap_or_else(|| format!("exhausted {MAX_RETRIES} attempts"));
    Err(ReconnectError::Refresh(RefreshFailure::Transient(detail)))
}

/// Perform a full OAuth reconnect for a single MCP server:
/// refresh the token, persist it, notify the refresh scheduler, and reconnect.
///
/// Steps:
/// 1. Call [`do_refresh_cancellable`] — cancellable retry loop.
/// 2. Write new access token to `token_arc` (shared with [`ProxyBackend`]).
/// 3. Persist refreshed OAuth state to SQLite via [`crate::credentials::db_update_oauth_token`].
/// 4. Send [`RefreshMessage::NewEntry`] to the refresh scheduler.
/// 5. Call [`ProxyBackend::connect`] to re-establish the MCP session.
///
/// On connect failure: returns [`ReconnectError::Connect`].
/// On cancellation: returns [`ReconnectError::Cancelled`] immediately.
/// On permanent refresh failure: if backend is not already `Connected`, sets status to `NeedsAuth`.
#[allow(clippy::too_many_arguments)]
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
        Err(ReconnectError::Refresh(failure)) => {
            tracing::warn!(server = %server_name, "reconnect refresh failed: {failure:#}");
            // Defense-in-depth: only set NeedsAuth on permanent failures, and
            // only if we're not already Connected (a concurrent path may have
            // authenticated successfully).
            if failure.is_permanent() && backend.status().await != BackendStatus::Connected {
                backend.set_status(BackendStatus::NeedsAuth).await;
            }
            return Err(ReconnectError::Refresh(failure));
        }
        Err(e) => {
            tracing::warn!(server = %server_name, "reconnect refresh errored: {e:#}");
            return Err(e);
        }
    };

    // Write access token to shared state so DynamicAuthClient picks it up immediately.
    *token_arc.write().await = Some(access_token.clone());

    // Persist to SQLite.
    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .map_err(|e| ReconnectError::PersistFailed(format!("{e:#}")))?;
    let expires_at = new_state.expires_at.to_rfc3339();
    crate::credentials::db_update_oauth_token(
        &conn,
        &server_name,
        &access_token,
        new_state.refresh_token.as_deref(),
        &expires_at,
    )
    .await
    .map_err(|e| ReconnectError::PersistFailed(format!("{e:#}")))?;

    // Notify refresh scheduler so it schedules the next refresh.
    refresh_tx
        .send(RefreshMessage::NewEntry {
            server_name: server_name.clone(),
            state: new_state,
            token: token_arc.clone(),
            backend: backend.clone(),
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
        .map_err(|e| ReconnectError::Connect(format!("{e:#}")))?;

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

    /// Install ring as the rustls process-level crypto provider. Idempotent —
    /// safe to call from multiple tests in the same binary.
    fn setup_crypto() {
        // install_default returns Err(existing provider Arc) when already
        // installed by another test in the same binary — that's not a failure.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[test]
    fn in_burst_fallback_matches_last_backoff() {
        assert_eq!(
            *BACKOFFS.last().expect("backoffs not empty"),
            BACKOFF_FALLBACK_SECS,
        );
    }

    fn make_entry(token_endpoint: String) -> OAuthServerState {
        OAuthServerState {
            refresh_token: Some("old-refresh-token".into()),
            token_endpoint,
            client_id: "test-client".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            server_url: "https://example.com/mcp".into(),
            resource: "https://example.com/mcp".into(),
        }
    }

    async fn wait_for_received_requests(server: &MockServer, expected: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            let received = server.received_requests().await.unwrap().len();
            if received >= expected {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!("mock server received {received} requests, expected at least {expected}");
            }
            tokio::task::yield_now().await;
        }
    }

    /// Verify that cancellation during a backoff sleep returns `Err(Cancelled)`
    /// without waiting the full backoff duration.
    #[tokio::test]
    async fn cancellation_aborts_refresh_during_backoff() {
        setup_crypto();
        // MockServer that always returns 503 — classified as transient, so the
        // inner loop enters its backoff sleep where cancellation can take effect.
        // (A 4xx would short-circuit as permanent and never reach the backoff.)
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream broke"))
            .expect(1) // exactly one attempt before cancellation fires
            .mount(&server)
            .await;

        let entry = make_entry(format!("{}/token", server.uri()));
        let client = reqwest::Client::new();
        let cancel = CancellationToken::new();

        tokio::time::pause();

        // Spawn the refresh in a background task so we can cancel from here.
        let cancel_clone = cancel.clone();
        let handle =
            tokio::spawn(
                async move { do_refresh_cancellable(&client, &entry, &cancel_clone).await },
            );

        // Let the first attempt complete (it hits the MockServer and gets 503),
        // then yield so the spawned task can reach the backoff select.
        wait_for_received_requests(&server, 1).await;
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

    /// Verify that cancellation also interrupts an in-flight HTTP refresh
    /// request, not only the retry backoff between requests.
    #[tokio::test]
    async fn cancellation_aborts_refresh_during_in_flight_request() {
        setup_crypto();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(503)
                    .set_delay(Duration::from_secs(5))
                    .set_body_string("slow upstream"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let entry = make_entry(format!("{}/token", server.uri()));
        let client = reqwest::Client::new();
        let cancel = CancellationToken::new();

        let cancel_clone = cancel.clone();
        let mut handle =
            tokio::spawn(
                async move { do_refresh_cancellable(&client, &entry, &cancel_clone).await },
            );

        wait_for_received_requests(&server, 1).await;

        cancel.cancel();

        let joined = match tokio::time::timeout(Duration::from_secs(1), &mut handle).await {
            Ok(joined) => joined.expect("task panicked"),
            Err(_) => {
                handle.abort();
                panic!("refresh did not return promptly after cancellation");
            }
        };
        assert!(
            matches!(joined, Err(ReconnectError::Cancelled)),
            "expected Cancelled, got {joined:?}",
        );
    }

    #[tokio::test]
    async fn refresh_posts_resource_parameter() {
        setup_crypto();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-access-token",
                "token_type": "Bearer",
                "expires_in": 3600
            })))
            .expect(1)
            .mount(&server)
            .await;

        let entry = make_entry(format!("{}/token", server.uri()));
        let client = reqwest::Client::new();
        let cancel = CancellationToken::new();

        let result = do_refresh_cancellable(&client, &entry, &cancel)
            .await
            .expect("refresh should succeed");

        assert_eq!(result.0.resource, "https://example.com/mcp");

        let requests = server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&requests[0].body);
        assert!(
            body.contains("resource=https%3A%2F%2Fexample.com%2Fmcp"),
            "refresh token request must include MCP resource indicator; body was {body}"
        );
    }

    /// When all refresh retries are exhausted, the backend status must NOT be set to
    /// `NeedsAuth` if it was already `Connected` — defense-in-depth guard.
    #[tokio::test]
    async fn exhausted_retries_do_not_overwrite_connected_status() {
        setup_crypto();
        let server = MockServer::start().await;
        // 503 → Transient. Exhausts all retries without short-circuiting on a
        // permanent 4xx, so the `is_permanent && status != Connected` guard
        // never fires and backend stays Connected.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream broke"))
            .mount(&server)
            .await;

        let entry = make_entry(format!("{}/token", server.uri()));

        let tmp = tempfile::tempdir().unwrap();

        let token_arc: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            tmp.path().to_path_buf(),
            "https://example.com/mcp".into(),
            token_arc.clone(),
            crate::proxy::AuthMethod::Bearer,
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
        assert!(
            result.is_err(),
            "expected error after exhausted retries, got Ok"
        );

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
        setup_crypto();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-access-tok",
                "refresh_token": "new-refresh-tok",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&server)
            .await;

        let entry = OAuthServerState {
            refresh_token: Some("old-refresh-tok".into()),
            token_endpoint: format!("{}/token", server.uri()),
            client_id: "test-client".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            server_url: "https://example.com/mcp".into(),
            resource: "https://example.com/mcp".into(),
        };

        let tmp = tempfile::tempdir().unwrap();
        // Initialize schema and insert the server row that db_update_oauth_token requires.
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('composio', 'https://example.com/mcp', 'oauth')",
            [],
        )
        .await
        .unwrap();
        drop(conn);

        let token_arc: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            tmp.path().to_path_buf(),
            // Fake URL — connect() will fail, which is expected.
            "https://example.com/mcp".into(),
            token_arc.clone(),
            crate::proxy::AuthMethod::Bearer,
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
            Err(ReconnectError::Connect(_)) => {} // Expected — fake URL fails to connect
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
                backend: _,
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

    #[tokio::test]
    async fn refresh_classifies_5xx_as_transient() {
        setup_crypto();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream broke"))
            .mount(&server)
            .await;

        let entry = make_entry(format!("{}/token", server.uri()));
        let client = reqwest::Client::new();
        let cancel = CancellationToken::new();

        tokio::time::pause();
        let handle =
            tokio::spawn(async move { do_refresh_cancellable(&client, &entry, &cancel).await });
        // Burn through all backoffs deterministically.
        for _ in 0..MAX_RETRIES {
            tokio::time::advance(Duration::from_secs(200)).await;
            tokio::task::yield_now().await;
        }
        let result = handle.await.expect("task panicked");
        assert!(
            matches!(
                result,
                Err(ReconnectError::Refresh(RefreshFailure::Transient(_)))
            ),
            "expected Transient for 5xx, got {result:?}"
        );
    }

    #[tokio::test]
    async fn refresh_classifies_network_error_as_transient() {
        setup_crypto();
        // Use a URL that fails DNS / TCP — port 1 on 127.0.0.1 should be closed.
        let entry = make_entry("http://127.0.0.1:1/token".into());
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(100))
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let cancel = CancellationToken::new();

        tokio::time::pause();
        let handle =
            tokio::spawn(async move { do_refresh_cancellable(&client, &entry, &cancel).await });
        for _ in 0..MAX_RETRIES {
            tokio::time::advance(Duration::from_secs(200)).await;
            tokio::task::yield_now().await;
        }
        let result = handle.await.expect("task panicked");
        assert!(
            matches!(
                result,
                Err(ReconnectError::Refresh(RefreshFailure::Transient(_)))
            ),
            "expected Transient for network error, got {result:?}"
        );
    }

    #[tokio::test]
    async fn refresh_classifies_400_as_permanent_no_retry() {
        setup_crypto();
        let server = MockServer::start().await;
        // 400 invalid_grant — refresh token revoked.
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid_grant"}"#),
            )
            .expect(1) // must NOT retry — first response is enough
            .mount(&server)
            .await;

        let entry = make_entry(format!("{}/token", server.uri()));
        let client = reqwest::Client::new();
        let cancel = CancellationToken::new();
        let result = do_refresh_cancellable(&client, &entry, &cancel).await;

        assert!(
            matches!(
                result,
                Err(ReconnectError::Refresh(RefreshFailure::Permanent(_)))
            ),
            "expected Permanent for 400 invalid_grant, got {result:?}"
        );
    }

    #[tokio::test]
    async fn refresh_classifies_cloudflare_403_challenge_as_transient() {
        setup_crypto();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(403).set_body_string(
                r#"<!DOCTYPE html><html><head><title>Just a moment...</title></head>
                <body><script src="/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1"></script>
                <form action="/api/v3/auth/dash/oauth2/token?__cf_chl_rt_tk=abc"></form></body></html>"#,
            ))
            .mount(&server)
            .await;

        let entry = make_entry(format!("{}/token", server.uri()));
        let client = reqwest::Client::new();
        let cancel = CancellationToken::new();

        tokio::time::pause();
        let handle =
            tokio::spawn(async move { do_refresh_cancellable(&client, &entry, &cancel).await });
        for _ in 0..MAX_RETRIES {
            tokio::time::advance(Duration::from_secs(200)).await;
            tokio::task::yield_now().await;
        }
        let result = handle.await.expect("task panicked");
        assert!(
            matches!(
                result,
                Err(ReconnectError::Refresh(RefreshFailure::Transient(_)))
            ),
            "expected Transient for Cloudflare 403 challenge, got {result:?}"
        );
    }

    #[tokio::test]
    async fn refresh_classifies_429_as_transient() {
        setup_crypto();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
            .mount(&server)
            .await;

        let entry = make_entry(format!("{}/token", server.uri()));
        let client = reqwest::Client::new();
        let cancel = CancellationToken::new();

        tokio::time::pause();
        let handle =
            tokio::spawn(async move { do_refresh_cancellable(&client, &entry, &cancel).await });
        for _ in 0..MAX_RETRIES {
            tokio::time::advance(Duration::from_secs(200)).await;
            tokio::task::yield_now().await;
        }
        let result = handle.await.expect("task panicked");
        assert!(
            matches!(
                result,
                Err(ReconnectError::Refresh(RefreshFailure::Transient(_)))
            ),
            "expected Transient for 429, got {result:?}"
        );
    }
}
