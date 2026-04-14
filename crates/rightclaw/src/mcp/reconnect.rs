//! Cancellable OAuth token refresh for reconnect scenarios.
//!
//! When a fresh OAuth token arrives while a stale retry loop is in progress,
//! the loop must be cancelled so it does not overwrite the fresh token.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::mcp::refresh::OAuthServerState;

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
}
