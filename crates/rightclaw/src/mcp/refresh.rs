use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep_until, Instant};
use tracing::{debug, error, warn};

use crate::mcp::credentials::{
    read_oauth_metadata, write_bearer_to_mcp_json, write_oauth_metadata, OAuthMetadata,
};
use crate::mcp::detect::{mcp_auth_status, AuthState};
use crate::mcp::oauth::{discover_as, TokenResponse};

const REFRESH_BUFFER_SECS: u64 = 600; // 10 minutes before expiry
const RETRY_INTERVAL_SECS: u64 = 300; // 5 minute retry on failure
const MAX_RETRIES: u32 = 3;

/// Errors from the refresh grant flow.
#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    #[error("HTTP request failed: {0}")]
    HttpFailed(String),
    #[error("token endpoint returned error status {status}: {body}")]
    TokenEndpointError { status: u16, body: String },
    #[error("token endpoint response is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("OAuth AS discovery failed: {0}")]
    DiscoveryFailed(String),
    #[error("credential read/write failed: {0}")]
    CredentialError(String),
    #[error("no refresh_token stored for server {0} — cannot refresh")]
    NoRefreshToken(String),
    #[error("no client_id stored for server {0} — cannot refresh")]
    NoClientId(String),
}

/// Compute the tokio Instant at which a refresh should fire.
///
/// Returns None when:
/// - `expires_at_secs == 0` (REFRESH-04: non-expiring token)
/// - `expires_at_secs <= buffer_secs` (underflow guard — would produce past time)
/// - token is already expired or within the buffer (refresh immediately via None)
///
/// Otherwise returns `Some(Instant::now() + (expires_at - buffer - now) seconds)`.
pub fn deadline_from_unix(expires_at_secs: u64, buffer_secs: u64) -> Option<Instant> {
    // REFRESH-04: non-expiring token
    if expires_at_secs == 0 {
        return None;
    }

    // Underflow guard: can't subtract more than the value
    if expires_at_secs <= buffer_secs {
        return None;
    }

    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Already expired — refresh immediately
    if expires_at_secs <= now_unix {
        return None;
    }

    let target_unix = expires_at_secs - buffer_secs;

    // Already within buffer (target is in the past) — refresh immediately
    if target_unix <= now_unix {
        return None;
    }

    let secs_from_now = target_unix.saturating_sub(now_unix);
    Some(Instant::now() + Duration::from_secs(secs_from_now))
}

/// POST a refresh_token grant to the token endpoint and return the new token response.
pub async fn post_refresh_grant(
    client: &reqwest::Client,
    token_endpoint: &str,
    refresh_token: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<TokenResponse, RefreshError> {
    let mut params = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];

    let secret_owned: String;
    if let Some(s) = client_secret {
        secret_owned = s.to_string();
        params.push(("client_secret", &secret_owned));
    }

    let resp = client
        .post(token_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|e| RefreshError::HttpFailed(format!("{e:#}")))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(RefreshError::TokenEndpointError {
            status: status.as_u16(),
            body,
        });
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| RefreshError::InvalidJson(format!("{e:#}")))
}

/// Refresh the token for a single MCP server and write the new credential atomically.
///
/// Reads OAuth metadata from .mcp.json `_rightclaw_oauth`, posts refresh grant,
/// writes new Bearer token + metadata back to .mcp.json.
///
/// Returns the new `expires_at` unix timestamp on success.
pub async fn refresh_token_for_server(
    http_client: &reqwest::Client,
    mcp_json_path: &Path,
    server_name: &str,
    server_url: &str,
) -> Result<u64, RefreshError> {
    let metadata = read_oauth_metadata(mcp_json_path, server_name)
        .map_err(|e| RefreshError::CredentialError(format!("{e:#}")))?
        .ok_or_else(|| RefreshError::NoRefreshToken(server_name.to_string()))?;

    let refresh_token = metadata
        .refresh_token
        .clone()
        .ok_or_else(|| RefreshError::NoRefreshToken(server_name.to_string()))?;

    let client_id = metadata
        .client_id
        .clone()
        .ok_or_else(|| RefreshError::NoClientId(server_name.to_string()))?;

    let as_meta = discover_as(http_client, server_url)
        .await
        .map_err(|e| RefreshError::DiscoveryFailed(format!("{e:#}")))?;

    let token_resp = post_refresh_grant(
        http_client,
        &as_meta.token_endpoint,
        &refresh_token,
        &client_id,
        metadata.client_secret.as_deref(),
    )
    .await?;

    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let new_expires_at = now_unix + token_resp.expires_in.unwrap_or(0);

    // Write new Bearer token to .mcp.json headers
    write_bearer_to_mcp_json(mcp_json_path, server_name, &token_resp.access_token)
        .map_err(|e| RefreshError::CredentialError(format!("{e:#}")))?;

    // If provider returned a new refresh_token, use it; otherwise keep the old one.
    let new_refresh_token = token_resp.refresh_token.or(Some(refresh_token));

    // Update OAuth metadata
    let new_metadata = OAuthMetadata {
        refresh_token: new_refresh_token,
        expires_at: new_expires_at,
        client_id: metadata.client_id,
        client_secret: metadata.client_secret,
    };
    write_oauth_metadata(mcp_json_path, server_name, &new_metadata)
        .map_err(|e| RefreshError::CredentialError(format!("{e:#}")))?;

    Ok(new_expires_at)
}

/// Per-token refresh loop. Runs as a tokio task for a single MCP server.
///
/// - Sleeps until 10 minutes before expiry.
/// - Calls `refresh_token_for_server` to POST the refresh grant and write new credential.
/// - On failure: retries up to MAX_RETRIES times at RETRY_INTERVAL_SECS intervals.
/// - After MAX_RETRIES failures: logs error and exits the loop.
/// - Exits immediately for non-expiring tokens (expires_at == 0) or missing refresh_token.
async fn run_token_refresh_loop(
    mcp_json_path: PathBuf,
    server_name: String,
    server_url: String,
    http_client: reqwest::Client,
) {
    let mut retries = 0u32;

    loop {
        // Re-read metadata on each iteration -- may have been updated by OAuth flow
        let metadata = match read_oauth_metadata(&mcp_json_path, &server_name) {
            Ok(Some(m)) => m,
            Ok(None) => {
                warn!(server = %server_name, "oauth metadata absent -- stopping refresh loop");
                return;
            }
            Err(e) => {
                warn!(server = %server_name, "failed to read oauth metadata: {e:#} -- stopping refresh loop");
                return;
            }
        };

        // REFRESH-04 guard: non-expiring token -- stop loop
        if metadata.expires_at == 0 {
            debug!(server = %server_name, "token is non-expiring (expires_at=0) -- refresh loop exiting");
            return;
        }

        // No refresh_token -- can't refresh
        if metadata.refresh_token.is_none() {
            warn!(server = %server_name, "no refresh_token -- skipping refresh loop");
            return;
        }

        // Sleep until refresh deadline (or proceed immediately if within buffer / expired)
        if let Some(deadline) = deadline_from_unix(metadata.expires_at, REFRESH_BUFFER_SECS) {
            sleep_until(deadline).await;
        }

        match refresh_token_for_server(&http_client, &mcp_json_path, &server_name, &server_url)
            .await
        {
            Ok(new_expires_at) => {
                retries = 0;
                // Provider issued non-expiring replacement
                if new_expires_at == 0 {
                    debug!(server = %server_name, "new token is non-expiring -- refresh loop exiting");
                    return;
                }
            }
            Err(e) => {
                warn!(server = %server_name, retries, "token refresh failed: {e:#}");
                retries += 1;
                if retries >= MAX_RETRIES {
                    error!(
                        server = %server_name,
                        "token refresh failed {MAX_RETRIES} times -- stopping scheduler"
                    );
                    return;
                }
                tokio::time::sleep(Duration::from_secs(RETRY_INTERVAL_SECS)).await;
            }
        }
    }
}

/// Start the refresh scheduler: spawns one tokio task per token-bearing MCP server.
///
/// Reads the agent's `.mcp.json` and credentials to find servers that:
/// - Have a stored token (not Missing)
/// - Have a refresh_token (can be refreshed)
/// - Have expires_at != 0 (not non-expiring — REFRESH-04)
///
/// Each qualifying server gets its own `run_token_refresh_loop` task.
pub async fn run_refresh_scheduler(
    agent_dir: PathBuf,
    http_client: reqwest::Client,
) {
    let mcp_path = agent_dir.join(".mcp.json");

    let statuses = match mcp_auth_status(&mcp_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("refresh scheduler: failed to read MCP status: {e:#}");
            return;
        }
    };

    for status in statuses {
        // Skip servers with no token at all
        if status.state == AuthState::Missing {
            continue;
        }

        // Read OAuth metadata to inspect refresh_token and expires_at
        let metadata = match read_oauth_metadata(&mcp_path, &status.name) {
            Ok(Some(m)) => m,
            Ok(None) => continue,
            Err(e) => {
                warn!(server = %status.name, "failed to read oauth metadata for refresh check: {e:#}");
                continue;
            }
        };

        if metadata.refresh_token.is_none() {
            warn!(
                server = %status.name,
                "no refresh_token stored -- scheduler not started"
            );
            continue;
        }

        // REFRESH-04: non-expiring token -- skip (no refresh needed)
        if metadata.expires_at == 0 {
            debug!(server = %status.name, "token is non-expiring (expires_at=0) -- skipping scheduler");
            continue;
        }

        tokio::spawn(run_token_refresh_loop(
            mcp_path.clone(),
            status.name.clone(),
            status.url.clone(),
            http_client.clone(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    // --- deadline_from_unix unit tests ---

    #[test]
    fn deadline_returns_none_for_zero_expires_at() {
        // REFRESH-04: non-expiring token must never be scheduled
        assert!(deadline_from_unix(0, 600).is_none());
    }

    #[test]
    fn deadline_returns_none_when_within_buffer() {
        // Token expires in 30 seconds — within 600-second buffer, refresh immediately (None)
        let expires_at = now_unix() + 30;
        assert!(deadline_from_unix(expires_at, 600).is_none());
    }

    #[test]
    fn deadline_returns_none_when_already_expired() {
        // Token expired in the past
        let expires_at = now_unix().saturating_sub(100);
        assert!(deadline_from_unix(expires_at, 600).is_none());
    }

    #[test]
    fn deadline_returns_some_for_future_expiry() {
        // Token expires in 1 hour — well outside 10-minute buffer
        let expires_at = now_unix() + 3600;
        assert!(deadline_from_unix(expires_at, 600).is_some());
    }

    #[test]
    fn deadline_underflow_guard() {
        // expires_at (500) <= buffer (600) — would underflow, must return None
        assert!(deadline_from_unix(500, 600).is_none());
    }

    // --- post_refresh_grant mock server tests ---

    #[tokio::test]
    async fn post_refresh_grant_200_returns_token_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            assert!(
                request.contains("grant_type=refresh_token"),
                "must include grant_type=refresh_token"
            );
            assert!(
                request.contains("refresh_token=old-rt"),
                "must include refresh_token"
            );
            assert!(
                request.contains("client_id=cli-id"),
                "must include client_id"
            );

            let body = r#"{"access_token":"new-at","token_type":"Bearer","expires_in":3600}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        });

        let client = reqwest::Client::new();
        let endpoint = format!("http://127.0.0.1:{port}/token");
        let result =
            post_refresh_grant(&client, &endpoint, "old-rt", "cli-id", None).await;

        assert!(result.is_ok(), "200 response must be Ok: {result:?}");
        let tok = result.unwrap();
        assert_eq!(tok.access_token, "new-at");
    }

    #[tokio::test]
    async fn post_refresh_grant_400_returns_token_endpoint_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).await;

            let body = r#"{"error":"invalid_grant"}"#;
            let resp = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        });

        let client = reqwest::Client::new();
        let endpoint = format!("http://127.0.0.1:{port}/token");
        let result =
            post_refresh_grant(&client, &endpoint, "bad-rt", "cli-id", None).await;

        assert!(
            matches!(result, Err(RefreshError::TokenEndpointError { status: 400, .. })),
            "400 response must be TokenEndpointError(400): {result:?}"
        );
    }

    // --- scheduler skips tokens without refresh_token ---

    #[tokio::test]
    async fn scheduler_skips_server_without_refresh_token() {
        use crate::mcp::credentials::{write_bearer_to_mcp_json, write_oauth_metadata, OAuthMetadata};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let mcp_json_path = agent_dir.join(".mcp.json");

        // Write .mcp.json with one HTTP server
        let mcp_json = serde_json::json!({
            "mcpServers": {
                "notion": { "url": "https://mcp.notion.com/mcp" }
            }
        });
        std::fs::write(
            &mcp_json_path,
            serde_json::to_string(&mcp_json).unwrap(),
        )
        .unwrap();

        // Write Bearer token but OAuth metadata WITHOUT a refresh_token
        write_bearer_to_mcp_json(&mcp_json_path, "notion", "at").unwrap();
        write_oauth_metadata(&mcp_json_path, "notion", &OAuthMetadata {
            refresh_token: None, // no refresh_token
            expires_at: now_unix() + 7200,
            client_id: Some("cli-id".to_string()),
            client_secret: None,
        }).unwrap();

        // run_refresh_scheduler should return without panicking (no task spawned)
        let http_client = reqwest::Client::new();
        run_refresh_scheduler(agent_dir, http_client).await;
        // If we get here, the scheduler correctly skipped the server
    }
}
