//! OAuth token refresh: state persistence and refresh timing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;
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

/// Message sent to refresh scheduler (new token or removal).
#[derive(Debug)]
pub enum RefreshMessage {
    /// New or updated OAuth token — schedule refresh timer.
    NewEntry {
        server_name: String,
        state: OAuthServerState,
        /// Shared token handle — scheduler writes new tokens here.
        token: Arc<tokio::sync::RwLock<Option<String>>>,
    },
    /// Server removed — cancel timer and clean up state.
    RemoveServer { server_name: String },
}

/// Load OAuth server entries from SQLite for refresh scheduling.
pub fn load_oauth_entries_from_db(
    conn: &Connection,
) -> miette::Result<Vec<(String, OAuthServerState)>> {
    let servers = crate::mcp::credentials::db_list_oauth_servers(conn)
        .map_err(|e| miette::miette!("failed to list OAuth servers: {e:#}"))?;

    let mut entries = Vec::new();
    for s in servers {
        let Some(ref token_endpoint) = s.token_endpoint else { continue };
        let Some(ref client_id) = s.client_id else { continue };
        let Some(ref expires_at_str) = s.expires_at else { continue };

        let expires_at = chrono::DateTime::parse_from_rfc3339(expires_at_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        entries.push((
            s.name.clone(),
            OAuthServerState {
                refresh_token: s.refresh_token.clone(),
                token_endpoint: token_endpoint.clone(),
                client_id: client_id.clone(),
                client_secret: s.client_secret.clone(),
                expires_at,
                server_url: s.url.clone(),
            },
        ));
    }
    Ok(entries)
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
/// Listens for `RefreshMessage` messages (new tokens or removals) and maintains
/// timers for each server. On successful refresh: writes new token to ProxyBackend's
/// shared `Arc<RwLock>` in-memory, and persists state to SQLite.
pub async fn run_refresh_scheduler(
    agent_dir: std::path::PathBuf,
    mut rx: tokio::sync::mpsc::Receiver<RefreshMessage>,
) {
    let http_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // Start with empty state — callers send NewEntry messages for all OAuth servers.
    // This avoids a race where a DB-loaded timer fires before the NewEntry arrives,
    // refreshing the token in SQLite but never updating the in-memory ProxyBackend.
    let mut entries: HashMap<String, OAuthServerState> = HashMap::new();
    let mut token_handles: HashMap<String, Arc<tokio::sync::RwLock<Option<String>>>> =
        HashMap::new();
    let mut timers: HashMap<String, tokio::time::Instant> = HashMap::new();

    loop {
        // Find the next timer to fire
        let next = timers.iter().min_by_key(|(_, instant)| *instant);

        tokio::select! {
            // Message from handler or OAuth callback
            Some(msg) = rx.recv() => {
                match msg {
                    RefreshMessage::NewEntry { server_name, state: entry_state, token } => {
                        let due = refresh_due_in(&entry_state);
                        timers.insert(server_name.clone(), tokio::time::Instant::now() + due);

                        // Read token before opening DB connection (Connection is !Send across await)
                        let current_token = token.read().await.clone().unwrap_or_default();

                        // Persist to SQLite
                        match crate::memory::open_connection(&agent_dir) {
                            Ok(conn) => {
                                let expires_at = entry_state.expires_at.to_rfc3339();
                                if let Err(e) = crate::mcp::credentials::db_set_oauth_state(
                                    &conn,
                                    &server_name,
                                    &current_token,
                                    entry_state.refresh_token.as_deref(),
                                    &entry_state.token_endpoint,
                                    &entry_state.client_id,
                                    entry_state.client_secret.as_deref(),
                                    &expires_at,
                                ) {
                                    tracing::error!("failed to persist OAuth state: {e:#}");
                                }
                            }
                            Err(e) => {
                                tracing::error!("failed to open memory DB for OAuth state persistence: {e:#}");
                            }
                        }

                        tracing::info!(
                            server = %server_name,
                            due_secs = due.as_secs(),
                            expires_at = %entry_state.expires_at,
                            has_refresh_token = entry_state.refresh_token.is_some(),
                            "new refresh scheduled",
                        );
                        entries.insert(server_name.clone(), entry_state);
                        token_handles.insert(server_name.clone(), token);
                    }
                    RefreshMessage::RemoveServer { server_name } => {
                        timers.remove(&server_name);
                        entries.remove(&server_name);
                        token_handles.remove(&server_name);
                        tracing::info!(server = %server_name, "refresh cancelled — server removed");
                    }
                }
            }

            // Timer fires
            _ = async {
                match next {
                    Some((_, &instant)) => tokio::time::sleep_until(instant).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let name = next.unwrap().0.clone();
                let entry = match entries.get(&name) {
                    Some(e) => e.clone(),
                    None => continue,
                };

                tracing::info!(server = %name, "refreshing OAuth token");

                match do_refresh(&http_client, &entry, MAX_RETRIES).await {
                    Ok((new_entry, access_token)) => {
                        // Write token directly to ProxyBackend's shared state
                        if let Some(token_arc) = token_handles.get(&name) {
                            *token_arc.write().await = Some(access_token.clone());
                            tracing::info!(server = %name, "token refreshed in-memory");
                        }

                        // Schedule next refresh
                        let due = refresh_due_in(&new_entry);
                        timers.insert(name.clone(), tokio::time::Instant::now() + due);

                        // Persist refreshed token to SQLite
                        match crate::memory::open_connection(&agent_dir) {
                            Ok(conn) => {
                                let expires_at = new_entry.expires_at.to_rfc3339();
                                if let Err(e) = crate::mcp::credentials::db_update_oauth_token(
                                    &conn,
                                    &name,
                                    &access_token,
                                    new_entry.refresh_token.as_deref(),
                                    &expires_at,
                                ) {
                                    tracing::error!("failed to persist refreshed token: {e:#}");
                                }
                            }
                            Err(e) => {
                                tracing::error!("failed to open memory DB for token refresh persistence: {e:#}");
                            }
                        }
                        entries.insert(name.clone(), new_entry);
                    }
                    Err(e) => {
                        tracing::warn!(server = %name, "token refresh failed after retries: {e:#}");
                        timers.remove(&name);
                    }
                }
            }
        }
    }
}

/// Attempt token refresh with retries.
/// Returns (updated_state, new_access_token).
///
/// Delegates to [`crate::mcp::reconnect::do_refresh_cancellable`] with a
/// never-cancelled token. See that function for the retry/backoff logic.
pub async fn do_refresh(
    client: &reqwest::Client,
    entry: &OAuthServerState,
    _max_retries: u32,
) -> miette::Result<(OAuthServerState, String)> {
    let cancel = tokio_util::sync::CancellationToken::new();
    crate::mcp::reconnect::do_refresh_cancellable(client, entry, &cancel)
        .await
        .map_err(|e| miette::miette!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_oauth_entries_from_db_test() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::migrations::MIGRATIONS
            .to_latest(&mut conn)
            .unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type, auth_token, refresh_token, \
             token_endpoint, client_id, expires_at) \
             VALUES ('notion', 'https://mcp.notion.com/mcp', 'oauth', 'tok', 'rt', \
             'https://ex.com/token', 'cid', '2026-04-13T12:00:00+00:00')",
            [],
        )
        .unwrap();
        let entries = load_oauth_entries_from_db(&conn).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "notion");
        assert_eq!(entries[0].1.client_id, "cid");
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
