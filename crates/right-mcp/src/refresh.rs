//! OAuth token refresh: state persistence and refresh timing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Maximum refresh margin: refresh tokens up to 1 hour before expiry so
/// transient network outages (which can last minutes on laptops) have time
/// to resolve via exponential-backoff retries before the token actually dies.
///
/// Used as an upper bound — actual margin is `min(MAX, remaining_lifetime / 2)`
/// to avoid busy-looping the scheduler on short-lived tokens.
const REFRESH_MARGIN_MAX: Duration = Duration::from_secs(3600);

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
pub enum RefreshMessage {
    /// New or updated OAuth token — schedule refresh timer.
    NewEntry {
        server_name: String,
        state: OAuthServerState,
        /// Shared token handle — scheduler writes new tokens here.
        token: Arc<tokio::sync::RwLock<Option<String>>>,
        /// Backend handle — scheduler updates status on permanent failure
        /// and triggers reconnect after recovery.
        backend: Arc<crate::proxy::ProxyBackend>,
    },
    /// Server removed — cancel timer and clean up state.
    RemoveServer { server_name: String },
}

impl std::fmt::Debug for RefreshMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewEntry {
                server_name, state, ..
            } => f
                .debug_struct("NewEntry")
                .field("server_name", server_name)
                .field("state", state)
                .finish_non_exhaustive(),
            Self::RemoveServer { server_name } => f
                .debug_struct("RemoveServer")
                .field("server_name", server_name)
                .finish(),
        }
    }
}

/// Load OAuth server entries from SQLite for refresh scheduling.
pub fn load_oauth_entries_from_db(
    conn: &Connection,
) -> miette::Result<Vec<(String, OAuthServerState)>> {
    let servers = crate::credentials::db_list_oauth_servers(conn)
        .map_err(|e| miette::miette!("failed to list OAuth servers: {e:#}"))?;

    let mut entries = Vec::new();
    for s in servers {
        let Some(ref token_endpoint) = s.token_endpoint else {
            continue;
        };
        let Some(ref client_id) = s.client_id else {
            continue;
        };
        let Some(ref expires_at_str) = s.expires_at else {
            continue;
        };

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

/// Calculate how long until refresh should fire.
///
/// Margin = `min(REFRESH_MARGIN_MAX, remaining_lifetime / 2)`. This gives
/// long-lived tokens (1h+) a full 1-hour buffer for retry recovery while
/// keeping short-lived tokens from busy-looping (refresh fires no sooner
/// than half-life).
///
/// Returns `Duration::ZERO` if the token is already past margin.
pub fn refresh_due_in(entry: &OAuthServerState) -> Duration {
    let now = chrono::Utc::now();
    let remaining = (entry.expires_at - now).to_std().unwrap_or(Duration::ZERO);
    if remaining.is_zero() {
        return Duration::ZERO;
    }
    let margin = std::cmp::min(REFRESH_MARGIN_MAX, remaining / 2);
    remaining.saturating_sub(margin)
}

/// Compute the delay before the next transient-retry attempt.
///
/// `attempt` is 1-indexed (1 = first retry after initial failure).
/// Sequence: 60, 120, 300, 600, 1200, 1800, 1800, ... (cap at 30 min).
pub(crate) fn transient_backoff_secs(attempt: u32) -> u64 {
    const STEPS: &[u64] = &[60, 120, 300, 600, 1200, 1800];
    STEPS
        .get((attempt.saturating_sub(1)) as usize)
        .copied()
        .unwrap_or(1800)
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
    let mut backend_handles: HashMap<String, Arc<crate::proxy::ProxyBackend>> = HashMap::new();
    let mut timers: HashMap<String, tokio::time::Instant> = HashMap::new();
    let mut retry_attempts: HashMap<String, u32> = HashMap::new();

    loop {
        // Find the next timer to fire
        let next = timers.iter().min_by_key(|(_, instant)| *instant);

        tokio::select! {
            // Message from handler or OAuth callback
            Some(msg) = rx.recv() => {
                match msg {
                    RefreshMessage::NewEntry { server_name, state: entry_state, token, backend } => {
                        let due = refresh_due_in(&entry_state);
                        timers.insert(server_name.clone(), tokio::time::Instant::now() + due);

                        // Read token before opening DB connection (Connection is !Send across await)
                        let current_token = token.read().await.clone().unwrap_or_default();

                        // Persist to SQLite
                        match right_db::open_connection(&agent_dir, false) {
                            Ok(conn) => {
                                let expires_at = entry_state.expires_at.to_rfc3339();
                                if let Err(e) = crate::credentials::db_set_oauth_state(
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
                        backend_handles.insert(server_name.clone(), backend);
                    }
                    RefreshMessage::RemoveServer { server_name } => {
                        timers.remove(&server_name);
                        entries.remove(&server_name);
                        token_handles.remove(&server_name);
                        backend_handles.remove(&server_name);
                        retry_attempts.remove(&server_name);
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

                let result = crate::reconnect::do_refresh_cancellable(
                    &http_client,
                    &entry,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await;

                match result {
                    Ok((new_entry, access_token)) => {
                        retry_attempts.remove(&name);

                        let was_needs_auth =
                            if let Some(backend) = backend_handles.get(&name) {
                                backend.status().await == crate::proxy::BackendStatus::NeedsAuth
                            } else {
                                false
                            };

                        if let Some(token_arc) = token_handles.get(&name) {
                            *token_arc.write().await = Some(access_token.clone());
                            tracing::info!(server = %name, "token refreshed in-memory");
                        }

                        let due = refresh_due_in(&new_entry);
                        timers.insert(name.clone(), tokio::time::Instant::now() + due);

                        match right_db::open_connection(&agent_dir, false) {
                            Ok(conn) => {
                                let expires_at = new_entry.expires_at.to_rfc3339();
                                if let Err(e) = crate::credentials::db_update_oauth_token(
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
                                tracing::error!(
                                    "failed to open memory DB for token refresh persistence: {e:#}"
                                );
                            }
                        }
                        entries.insert(name.clone(), new_entry);

                        // If backend was NeedsAuth (set by a 401 at tool-call time or by a
                        // previous permanent failure that has since cleared), the rmcp
                        // session is probably dead. Spawn a background reconnect.
                        if was_needs_auth
                            && let Some(backend) = backend_handles.get(&name).cloned()
                        {
                            let http = http_client.clone();
                            let name_owned = name.clone();
                            tokio::spawn(async move {
                                if let Err(e) = backend.connect(http).await {
                                    tracing::warn!(
                                        server = %name_owned,
                                        "post-refresh reconnect failed: {e:#}"
                                    );
                                }
                            });
                        }
                    }
                    Err(crate::reconnect::ReconnectError::Refresh(failure)) => {
                        let permanent = failure.is_permanent();
                        tracing::warn!(
                            server = %name,
                            %permanent,
                            "token refresh failed: {failure:#}"
                        );
                        if permanent {
                            if let Some(backend) = backend_handles.get(&name) {
                                backend
                                    .set_status(crate::proxy::BackendStatus::NeedsAuth)
                                    .await;
                                tracing::warn!(
                                    server = %name,
                                    "marked NeedsAuth after permanent refresh failure"
                                );
                            }
                            // User must re-OAuth — no reschedule.
                            timers.remove(&name);
                            retry_attempts.remove(&name);
                        } else {
                            let attempt = retry_attempts
                                .entry(name.clone())
                                .and_modify(|n| *n += 1)
                                .or_insert(1);
                            let delay = transient_backoff_secs(*attempt);
                            tracing::info!(
                                server = %name,
                                attempt = *attempt,
                                delay_secs = delay,
                                "scheduling transient retry"
                            );
                            timers.insert(
                                name.clone(),
                                tokio::time::Instant::now() + Duration::from_secs(delay),
                            );
                        }
                    }
                    Err(other) => {
                        // do_refresh_cancellable with a fresh never-cancelled token
                        // cannot return Cancelled/Connect/PersistFailed here. Treat as a
                        // contract violation: log loudly, drop the server from the
                        // scheduler so the bug is visible in mcp_list (Unreachable) rather
                        // than masked by an infinite retry loop.
                        tracing::error!(
                            server = %name,
                            "scheduler contract violation: do_refresh_cancellable returned unexpected variant: {other:#}"
                        );
                        timers.remove(&name);
                        retry_attempts.remove(&name);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_oauth_entries_from_db_test() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        right_db::MIGRATIONS
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
        // Margin = min(MAX=3600s, 1800s/2=900s) = 900s. due ≈ 1800s - 900s = 900s.
        let due = refresh_due_in(&entry);
        assert!(
            due.as_secs() > 850 && due.as_secs() < 950,
            "expected ~900s, got {}s",
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
    fn refresh_due_in_uses_half_lifetime_for_short_tokens() {
        let entry = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: "https://example.com/token".into(),
            client_id: "c".into(),
            client_secret: None,
            // 5-minute lifetime — far shorter than 1-hour MAX margin.
            // Margin must clamp to lifetime/2 = 150s; due = 300s - 150s = 150s.
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            server_url: "https://example.com/mcp".into(),
        };
        let due = refresh_due_in(&entry);
        assert!(
            due.as_secs() > 120 && due.as_secs() < 180,
            "expected ~150s (half of 5-min lifetime), got {}s",
            due.as_secs()
        );
    }

    #[test]
    fn refresh_due_in_caps_at_max_for_long_tokens() {
        let entry = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: "https://example.com/token".into(),
            client_id: "c".into(),
            client_secret: None,
            // 24-hour lifetime — half is 12 hours, but MAX caps margin at 1 hour.
            // due = 24h - 1h = 23h.
            expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
            server_url: "https://example.com/mcp".into(),
        };
        let due = refresh_due_in(&entry);
        // 23 hours = 82800s. Allow a few seconds of clock skew.
        assert!(
            due.as_secs() > 82700 && due.as_secs() < 82900,
            "expected ~82800s (24h - 1h MAX margin), got {}s",
            due.as_secs()
        );
    }

    #[tokio::test]
    async fn scheduler_retries_transient_indefinitely() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // First scheduler fire: do_refresh_cancellable internally retries 3
        // times. We want all 3 of those to see 503, then later attempts see
        // 200. up_to_n_times caps the first mock at 3 hits; subsequent hits
        // fall through to the second mock.
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(503).set_body_string("temporarily down"))
            .up_to_n_times(3)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-tok",
                "refresh_token": "new-rt",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('s', 'https://x/mcp', 'oauth')",
            [],
        )
        .unwrap();
        drop(conn);

        let entry_state = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: format!("{}/token", server.uri()),
            client_id: "c".into(),
            client_secret: None,
            // Already past margin → fires immediately
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            server_url: "https://x/mcp".into(),
        };
        let token_arc: Arc<tokio::sync::RwLock<Option<String>>> =
            Arc::new(tokio::sync::RwLock::new(Some("old".into())));
        let backend = Arc::new(crate::proxy::ProxyBackend::new(
            "s".into(),
            tmp.path().to_path_buf(),
            "https://x/mcp".into(),
            token_arc.clone(),
            crate::proxy::AuthMethod::Bearer,
        ));

        tokio::time::pause();

        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let scheduler = tokio::spawn(run_refresh_scheduler(tmp.path().to_path_buf(), rx));

        tx.send(RefreshMessage::NewEntry {
            server_name: "s".into(),
            state: entry_state,
            token: token_arc.clone(),
            backend: backend.clone(),
        })
        .await
        .unwrap();

        // First scheduler fire calls do_refresh_cancellable, which retries
        // internally 3 times with 30/60s backoff (~90s virtual). All return
        // 503 → Transient. Scheduler reschedules in 60s. Second fire's first
        // attempt hits the 200 mock → success.
        //
        // Drive virtual time forward until token_arc updates or we time out.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            tokio::time::advance(Duration::from_secs(5)).await;
            tokio::task::yield_now().await;
            if *token_arc.read().await == Some("new-tok".to_string()) {
                break;
            }
            if std::time::Instant::now() > deadline {
                panic!(
                    "scheduler did not refresh in time; received_requests={}",
                    server.received_requests().await.unwrap().len()
                );
            }
        }

        let n_requests = server.received_requests().await.unwrap().len();
        assert!(
            n_requests >= 4,
            "scheduler must keep retrying transient failures; got {n_requests} requests"
        );

        scheduler.abort();
    }

    #[tokio::test]
    async fn scheduler_marks_needs_auth_on_permanent_failure() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid_grant"}"#),
            )
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('s', 'https://x/mcp', 'oauth')",
            [],
        )
        .unwrap();
        drop(conn);

        let entry_state = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: format!("{}/token", server.uri()),
            client_id: "c".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            server_url: "https://x/mcp".into(),
        };
        let token_arc: Arc<tokio::sync::RwLock<Option<String>>> =
            Arc::new(tokio::sync::RwLock::new(Some("old".into())));
        let backend = Arc::new(crate::proxy::ProxyBackend::new(
            "s".into(),
            tmp.path().to_path_buf(),
            "https://x/mcp".into(),
            token_arc.clone(),
            crate::proxy::AuthMethod::Bearer,
        ));
        // Pre-set to Unreachable so the permanent flip is observable.
        backend
            .set_status(crate::proxy::BackendStatus::Unreachable)
            .await;

        tokio::time::pause();

        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let scheduler = tokio::spawn(run_refresh_scheduler(tmp.path().to_path_buf(), rx));

        tx.send(RefreshMessage::NewEntry {
            server_name: "s".into(),
            state: entry_state,
            token: token_arc.clone(),
            backend: backend.clone(),
        })
        .await
        .unwrap();

        // Allow the timer to fire and the permanent response to be processed.
        // Margin is min(MAX=3600s, 300s/2=150s) = 150s, so due ≈ 150s. Drive
        // virtual time forward until status flips or we time out.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            tokio::time::advance(Duration::from_secs(5)).await;
            tokio::task::yield_now().await;
            if backend.status().await == crate::proxy::BackendStatus::NeedsAuth {
                break;
            }
            if std::time::Instant::now() > deadline {
                break;
            }
        }

        assert_eq!(
            backend.status().await,
            crate::proxy::BackendStatus::NeedsAuth,
            "permanent refresh failure must flip backend to NeedsAuth"
        );

        scheduler.abort();
    }
}
