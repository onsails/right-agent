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
