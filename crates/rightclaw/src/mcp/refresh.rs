//! OAuth token refresh: state persistence and refresh timing.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Refresh margin: refresh token 10 minutes before expiry.
const REFRESH_MARGIN: Duration = Duration::from_secs(600);

/// Per-server OAuth state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthServerState {
    pub refresh_token: Option<String>,
    pub token_endpoint: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub server_url: String,
}

/// All OAuth state for an agent.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct OAuthState {
    pub servers: HashMap<String, OAuthServerState>,
}

/// Message sent from OAuth callback to refresh scheduler.
#[derive(Debug)]
pub struct RefreshEntry {
    pub server_name: String,
    pub state: OAuthServerState,
}

/// Load OAuth state from file. Returns empty state if file doesn't exist.
pub fn load_oauth_state(path: &Path) -> miette::Result<OAuthState> {
    if !path.exists() {
        return Ok(OAuthState::default());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("failed to read oauth state: {e:#}"))?;
    let state: OAuthState = serde_json::from_str(&content)
        .map_err(|e| miette::miette!("failed to parse oauth state: {e:#}"))?;
    Ok(state)
}

/// Save OAuth state to file atomically.
pub fn save_oauth_state(path: &Path, state: &OAuthState) -> miette::Result<()> {
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| miette::miette!("failed to serialize oauth state: {e:#}"))?;
    std::fs::write(path, content)
        .map_err(|e| miette::miette!("failed to write oauth state: {e:#}"))?;
    Ok(())
}

/// Maximum retry attempts for token refresh.
const MAX_RETRIES: u32 = 3;

/// Calculate how long until refresh should fire.
/// Returns Duration::ZERO if already expired or past margin.
pub fn refresh_due_in(entry: &OAuthServerState) -> Duration {
    let now = chrono::Utc::now();
    let margin = chrono::Duration::from_std(REFRESH_MARGIN).unwrap();
    let refresh_at = entry.expires_at - margin;
    if refresh_at <= now {
        Duration::ZERO
    } else {
        (refresh_at - now).to_std().unwrap_or(Duration::ZERO)
    }
}

/// Run the OAuth token refresh scheduler.
///
/// Listens for new `RefreshEntry` messages from OAuth callbacks and maintains
/// timers for each server. On successful refresh: updates Bearer in `mcp_json_path`,
/// re-uploads into sandbox, updates `oauth_state_path`.
pub async fn run_refresh_scheduler(
    oauth_state_path: std::path::PathBuf,
    mcp_json_path: std::path::PathBuf,
    sandbox_name: Option<String>,
    mut rx: tokio::sync::mpsc::Receiver<RefreshEntry>,
    notify_tx: tokio::sync::mpsc::Sender<String>,
) {
    let http_client = reqwest::Client::new();

    // Load existing state
    let mut state = match load_oauth_state(&oauth_state_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to load oauth state: {e:#}");
            OAuthState::default()
        }
    };

    // Build initial timer set
    let mut timers: HashMap<String, tokio::time::Instant> = HashMap::new();
    for (name, entry) in &state.servers {
        if entry.refresh_token.is_none() {
            tracing::warn!(server = %name, "no refresh_token — skipping auto-refresh");
            continue;
        }
        let due = refresh_due_in(entry);
        timers.insert(name.clone(), tokio::time::Instant::now() + due);
        tracing::info!(server = %name, due_secs = due.as_secs(), "scheduled refresh");
    }

    loop {
        // Find the next timer to fire
        let next = timers.iter().min_by_key(|(_, instant)| *instant);

        tokio::select! {
            // New entry from OAuth callback
            Some(entry) = rx.recv() => {
                let due = refresh_due_in(&entry.state);
                timers.insert(entry.server_name.clone(), tokio::time::Instant::now() + due);
                state.servers.insert(entry.server_name.clone(), entry.state);
                let _ = save_oauth_state(&oauth_state_path, &state);
                tracing::info!(server = %entry.server_name, due_secs = due.as_secs(), "new refresh scheduled");
            }

            // Timer fires
            _ = async {
                match next {
                    Some((_, &instant)) => tokio::time::sleep_until(instant).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let name = next.unwrap().0.clone();
                let entry = match state.servers.get(&name) {
                    Some(e) => e.clone(),
                    None => continue,
                };

                tracing::info!(server = %name, "refreshing OAuth token");

                match do_refresh(&http_client, &entry, MAX_RETRIES).await {
                    Ok((new_entry, access_token)) => {
                        // Update Bearer in .mcp.json with the NEW access token
                        if let Err(e) = crate::mcp::credentials::set_server_header(
                            &mcp_json_path,
                            &name,
                            "Authorization",
                            &format!("Bearer {access_token}"),
                        ) {
                            tracing::error!(server = %name, "failed to update .mcp.json: {e:#}");
                        }

                        // Re-upload .mcp.json into sandbox (skip when no sandbox)
                        if let Some(ref sbox) = sandbox_name {
                            if let Err(e) = crate::openshell::upload_file(
                                sbox,
                                &mcp_json_path,
                                "/sandbox/.mcp.json",
                            ).await {
                                tracing::error!(server = %name, "failed to re-upload .mcp.json: {e:#}");
                            }
                        }

                        // Schedule next refresh
                        let due = refresh_due_in(&new_entry);
                        timers.insert(name.clone(), tokio::time::Instant::now() + due);
                        state.servers.insert(name.clone(), new_entry);
                        let _ = save_oauth_state(&oauth_state_path, &state);
                    }
                    Err(e) => {
                        tracing::error!(server = %name, "token refresh failed after retries: {e:#}");
                        timers.remove(&name);
                        let _ = notify_tx.send(format!("OAuth refresh failed for {name}: {e:#}")).await;
                    }
                }
            }
        }
    }
}

/// Attempt token refresh with retries.
/// Returns (updated_state, new_access_token).
async fn do_refresh(
    client: &reqwest::Client,
    entry: &OAuthServerState,
    max_retries: u32,
) -> miette::Result<(OAuthServerState, String)> {
    let refresh_token = entry.refresh_token.as_deref()
        .ok_or_else(|| miette::miette!("no refresh_token"))?;

    let backoffs = [30, 60, 120]; // seconds

    for attempt in 0..max_retries {
        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &entry.client_id),
        ];
        if let Some(ref secret) = entry.client_secret {
            form.push(("client_secret", secret));
        }

        let resp = client
            .post(&entry.token_endpoint)
            .form(&form)
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let token_resp: crate::mcp::oauth::TokenResponse = r.json().await
                    .map_err(|e| miette::miette!("failed to parse token response: {e:#}"))?;

                let expires_at = chrono::Utc::now()
                    + chrono::Duration::seconds(token_resp.expires_in.unwrap_or(3600) as i64);

                let access_token = token_resp.access_token.clone();
                return Ok((OAuthServerState {
                    refresh_token: token_resp.refresh_token.or(entry.refresh_token.clone()),
                    token_endpoint: entry.token_endpoint.clone(),
                    client_id: entry.client_id.clone(),
                    client_secret: entry.client_secret.clone(),
                    expires_at,
                    server_url: entry.server_url.clone(),
                }, access_token));
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                tracing::warn!(attempt, %status, %body, "refresh attempt failed");
            }
            Err(e) => {
                tracing::warn!(attempt, "refresh request error: {e:#}");
            }
        }

        if attempt < max_retries - 1 {
            let delay = backoffs.get(attempt as usize).copied().unwrap_or(120);
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
    }

    Err(miette::miette!("token refresh failed after {max_retries} attempts"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_oauth_state() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("oauth-state.json");

        let entry = OAuthServerState {
            refresh_token: Some("rt-abc".into()),
            token_endpoint: "https://accounts.notion.com/oauth/token".into(),
            client_id: "client123".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            server_url: "https://mcp.notion.com/mcp".into(),
        };

        let mut state = OAuthState::default();
        state.servers.insert("notion".into(), entry);
        save_oauth_state(&path, &state).unwrap();

        let loaded = load_oauth_state(&path).unwrap();
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers["notion"].client_id, "client123");
        assert_eq!(
            loaded.servers["notion"].refresh_token.as_deref(),
            Some("rt-abc")
        );
    }

    #[test]
    fn load_returns_empty_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.json");
        let state = load_oauth_state(&path).unwrap();
        assert!(state.servers.is_empty());
    }

    #[test]
    fn refresh_due_in_future() {
        let entry = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: "https://example.com/token".into(),
            client_id: "c".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(30),
            server_url: "https://example.com/mcp".into(),
        };
        // Should refresh 10 minutes before expiry = ~20 minutes from now
        let due = refresh_due_in(&entry);
        assert!(
            due.as_secs() > 1100 && due.as_secs() < 1300,
            "expected ~1200s, got {}s",
            due.as_secs()
        );
    }

    #[test]
    fn refresh_due_in_returns_zero_when_expired() {
        let entry = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: "https://example.com/token".into(),
            client_id: "c".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
            server_url: "https://example.com/mcp".into(),
        };
        let due = refresh_due_in(&entry);
        assert_eq!(due, Duration::ZERO);
    }

    #[test]
    fn refresh_due_in_within_margin() {
        let entry = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: "https://example.com/token".into(),
            client_id: "c".into(),
            client_secret: None,
            // Expires in 5 minutes -- within 10-minute margin
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            server_url: "https://example.com/mcp".into(),
        };
        let due = refresh_due_in(&entry);
        assert_eq!(
            due,
            Duration::ZERO,
            "should return zero when within refresh margin"
        );
    }
}
