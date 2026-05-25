//! OAuth token refresh: state persistence and refresh timing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use right_db::Connection;
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

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
    pub resource: String,
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
pub async fn load_oauth_entries_from_db(
    conn: &Connection,
) -> miette::Result<Vec<(String, OAuthServerState)>> {
    let servers = crate::credentials::db_list_oauth_servers(conn)
        .await
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
                resource: s
                    .oauth_resource
                    .as_deref()
                    .filter(|r| !r.trim().is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| {
                        crate::oauth::canonical_resource_uri(&s.url)
                            .unwrap_or_else(|_| s.url.clone())
                    }),
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
    let margin = std::cmp::min(REFRESH_MARGIN_MAX, remaining / 2);
    remaining.saturating_sub(margin)
}

/// Exponential-style backoff schedule for transient refresh failures (seconds).
///
/// Entry `i` is the delay before retry attempt `i+1` (1-indexed externally).
/// The last entry equals [`TRANSIENT_BACKOFF_CAP_SECS`] so that exhausting the
/// schedule plateaus instead of regressing on the boundary.
const TRANSIENT_BACKOFF_SECS: &[u64] = &[60, 120, 300, 600, 1200, 1800];

/// Plateau delay used once [`TRANSIENT_BACKOFF_SECS`] is exhausted. MUST equal
/// the last entry of `TRANSIENT_BACKOFF_SECS` (asserted by a unit test).
const TRANSIENT_BACKOFF_CAP_SECS: u64 = 1800;

/// Compute the delay before the next transient-retry attempt.
///
/// `attempt` is 1-indexed (1 = first retry after initial failure).
/// Sequence: 60, 120, 300, 600, 1200, 1800, 1800, ... (cap at 30 min).
pub(crate) fn transient_backoff_secs(attempt: u32) -> u64 {
    TRANSIENT_BACKOFF_SECS
        .get((attempt.saturating_sub(1)) as usize)
        .copied()
        .unwrap_or(TRANSIENT_BACKOFF_CAP_SECS)
}

fn transient_backoff_delay(attempt: u32) -> Duration {
    #[cfg(test)]
    {
        let _ = attempt;
        Duration::from_millis(10)
    }

    #[cfg(not(test))]
    {
        Duration::from_secs(transient_backoff_secs(attempt))
    }
}

/// Result of a single in-flight refresh task: the server name, a per-server
/// generation counter (bumped on every `NewEntry`), and the classified
/// [`do_refresh_cancellable`] outcome.
///
/// The generation lets the scheduler discard results whose source entry has
/// been superseded by a fresh `NewEntry` (typically from `/mcp auth`). Without
/// it, an HTTP response that races with cancellation could overwrite freshly-
/// rotated credentials with the outcome of a refresh against the old
/// refresh_token — masking the rotation until the next expiry.
type RefreshTaskOutput = (
    String,
    u64,
    Result<(OAuthServerState, String), crate::reconnect::ReconnectError>,
);

/// Run the OAuth token refresh scheduler.
///
/// Listens for `RefreshMessage` messages (new tokens or removals) and maintains
/// timers for each server. On successful refresh: writes new token to ProxyBackend's
/// shared `Arc<RwLock>` in-memory, and persists state to SQLite.
pub async fn run_refresh_scheduler(
    agent_dir: std::path::PathBuf,
    mut rx: tokio::sync::mpsc::Receiver<RefreshMessage>,
) {
    crate::ensure_crypto_provider();

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
    // In-flight refresh tasks. Each task returns its server name plus the
    // refresh result; we look up `entries`/`backend_handles` on completion
    // and apply the result. Spawning into a JoinSet keeps the scheduler
    // responsive to `rx.recv` — without this, a long `do_refresh_cancellable`
    // (~210s exhausting-backoff path) starves `RemoveServer` / `NewEntry`
    // for minutes.
    let mut in_flight: JoinSet<RefreshTaskOutput> = JoinSet::new();
    // Per-server cancellation handles for in-flight refreshes. `RemoveServer`
    // and a superseding `NewEntry` both cancel via this map so stale results
    // can't pollute newly-rebuilt state.
    let mut cancel_tokens: HashMap<String, CancellationToken> = HashMap::new();
    // Per-server generation counter. Bumped every `NewEntry` (i.e. every time
    // the credential chain is replaced); the in-flight task carries the
    // generation it was spawned with, and the join_next handler drops results
    // whose generation doesn't match the current one. Defends against the
    // race where a successful HTTP response is returned just AFTER a
    // superseding `NewEntry` cancels the task — the cancel can't abort an
    // already-completed HTTP call inside `do_refresh_cancellable`, so the
    // task may still return `Ok` with the now-stale refresh_token.
    let mut generations: HashMap<String, u64> = HashMap::new();

    loop {
        // Find the next timer to fire. Clone keys/instants up front so we
        // don't hold a borrow of `timers` across the select arms (the timer
        // arm needs to remove from `timers` before spawning).
        let next = timers
            .iter()
            .min_by_key(|(_, instant)| *instant)
            .map(|(name, instant)| (name.clone(), *instant));
        let next_instant = next.as_ref().map(|(_, instant)| *instant);
        let next_name = next.map(|(name, _)| name);

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
                        match right_db::open_connection(&agent_dir, false).await {
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
                                    &entry_state.resource,
                                )
                                .await
                                {
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
                        // Reset retry counter: a fresh NewEntry (e.g. after
                        // /mcp auth) supersedes any in-flight retry cycle. Without
                        // this, a stale counter from prior transient failures
                        // would push the next backoff well past the 60s first step.
                        retry_attempts.remove(&server_name);
                        // Bump the generation: any in-flight refresh tagged
                        // with an older generation will be discarded when it
                        // completes, even if it managed to return Ok before
                        // the cancel propagated.
                        *generations.entry(server_name.clone()).or_insert(0) += 1;
                        // Cancel any in-flight refresh for this server: the
                        // new entry supersedes whatever the in-flight task is
                        // computing. The join_next arm will see Cancelled (or
                        // whatever the task returned) and discard on
                        // generation mismatch.
                        if let Some(token) = cancel_tokens.remove(&server_name) {
                            token.cancel();
                            tracing::debug!(
                                server = %server_name,
                                "cancelled in-flight refresh — superseded by NewEntry",
                            );
                        }
                    }
                    RefreshMessage::RemoveServer { server_name } => {
                        timers.remove(&server_name);
                        entries.remove(&server_name);
                        token_handles.remove(&server_name);
                        backend_handles.remove(&server_name);
                        retry_attempts.remove(&server_name);
                        // Don't drop the generation: a re-registered server
                        // would otherwise reuse generation 0, potentially
                        // matching an in-flight task that was tagged with 0
                        // before this removal. Keeping the counter monotonic
                        // preserves the entries-contains-key short-circuit
                        // as the load-bearing defense for removed servers.
                        // Cancel any in-flight refresh for this server.
                        // do_refresh_cancellable checks the token before each
                        // attempt and races backoff sleeps against it, so the
                        // task aborts cleanly with Err(Cancelled) rather than
                        // running for minutes against a removed server.
                        if let Some(token) = cancel_tokens.remove(&server_name) {
                            token.cancel();
                        }
                        tracing::info!(server = %server_name, "refresh cancelled — server removed");
                    }
                }
            }

            // An in-flight refresh completed. Apply its result against the
            // CURRENT scheduler state (which may have shifted while the task
            // was running). If the server was removed or superseded, short-
            // circuit.
            Some(joined) = in_flight.join_next() => {
                match joined {
                    Err(e) if e.is_cancelled() => {
                        // Task aborted via JoinSet abort. We don't abort
                        // tasks ourselves (we cancel via CancellationToken
                        // and let the task return Err(Cancelled) cleanly),
                        // so this branch is purely defensive.
                        tracing::debug!("in-flight refresh aborted (JoinSet)");
                    }
                    Err(e) => {
                        tracing::error!("in-flight refresh task panicked: {e}");
                    }
                    Ok((name, task_generation, result)) => {
                        // Drop the cancel token first so a concurrent
                        // RemoveServer/NewEntry doesn't try to cancel an
                        // already-completed task. Only remove if it still
                        // belongs to the current task (a superseding
                        // NewEntry may have installed its own cancel token).
                        if let Some(current) = generations.get(&name).copied()
                            && current == task_generation
                        {
                            cancel_tokens.remove(&name);
                        }

                        // If the server was removed (or replaced) while the
                        // refresh was in flight, discard the result. Without
                        // this, a stale Ok response would re-insert a timer
                        // for a server the operator just removed.
                        if !entries.contains_key(&name) {
                            tracing::debug!(
                                server = %name,
                                "discarding refresh result — server no longer tracked"
                            );
                            continue;
                        }

                        // Discard results from superseded generations. A
                        // NewEntry that arrives while a refresh is in flight
                        // bumps the generation; if the in-flight task's HTTP
                        // call had already succeeded by then, it will return
                        // Ok with credentials derived from the OLD refresh
                        // chain — overwriting the freshly-rotated state.
                        let current_generation = generations.get(&name).copied().unwrap_or(0);
                        if task_generation != current_generation {
                            tracing::debug!(
                                server = %name,
                                task_generation,
                                current_generation,
                                "discarding refresh result — entry superseded mid-flight"
                            );
                            continue;
                        }

                        match result {
                            Ok((new_entry, access_token)) => {
                                retry_attempts.remove(&name);

                                if let Some(token_arc) = token_handles.get(&name) {
                                    *token_arc.write().await = Some(access_token.clone());
                                    tracing::info!(server = %name, "token refreshed in-memory");
                                }

                                let due = refresh_due_in(&new_entry);
                                timers.insert(name.clone(), tokio::time::Instant::now() + due);

                                match right_db::open_connection(&agent_dir, false).await {
                                    Ok(conn) => {
                                        let expires_at = new_entry.expires_at.to_rfc3339();
                                        if let Err(e) = crate::credentials::db_update_oauth_token(
                                            &conn,
                                            &name,
                                            &access_token,
                                            new_entry.refresh_token.as_deref(),
                                            &expires_at,
                                        )
                                        .await
                                        {
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

                                // Re-read status AFTER the token write so a concurrent
                                // tool_call 401 that flipped the backend to NeedsAuth
                                // during persistence is still observed here. Snapshotting
                                // before the write would create a TOCTOU window where a
                                // post-snapshot 401 leaves the backend stuck in NeedsAuth
                                // despite a fresh token.
                                let needs_reconnect =
                                    if let Some(backend) = backend_handles.get(&name) {
                                        backend.status().await == crate::proxy::BackendStatus::NeedsAuth
                                    } else {
                                        false
                                    };

                                // If backend is NeedsAuth (set by a 401 at tool-call time or by a
                                // previous permanent failure that has since cleared), the rmcp
                                // session is probably dead. Spawn a background reconnect.
                                if needs_reconnect
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
                                    let delay = transient_backoff_delay(*attempt);
                                    tracing::info!(
                                        server = %name,
                                        attempt = *attempt,
                                        delay_secs = delay.as_secs(),
                                        "scheduling transient retry"
                                    );
                                    timers.insert(name.clone(), tokio::time::Instant::now() + delay);
                                }
                            }
                            Err(crate::reconnect::ReconnectError::Cancelled) => {
                                // Cancellation triggered by RemoveServer/NewEntry.
                                // The entries-contains check above already
                                // handled RemoveServer; for NewEntry, the
                                // new timer is already in `timers` and we
                                // must not overwrite it. Drop silently.
                                tracing::debug!(
                                    server = %name,
                                    "in-flight refresh cancelled"
                                );
                            }
                            Err(other) => {
                                // do_refresh_cancellable spawned with a fresh
                                // CancellationToken cannot return Connect or
                                // PersistFailed. Treat as a contract violation:
                                // log loudly, drop the server from the scheduler
                                // so the bug is visible in mcp_list (Unreachable)
                                // rather than masked by an infinite retry loop.
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

            // Timer fires — spawn the refresh into the JoinSet so the
            // scheduler stays responsive to inbound messages while it runs.
            _ = async {
                match next_instant {
                    Some(instant) => tokio::time::sleep_until(instant).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let name = next_name.expect("next_instant is Some implies next_name is Some");
                // Clear the timer so the same wake-up doesn't re-fire on the
                // next loop iteration before the spawned task completes.
                timers.remove(&name);

                let Some(entry) = entries.get(&name).cloned() else {
                    // Server was removed between min_by_key and the sleep
                    // completing.
                    continue;
                };

                tracing::info!(server = %name, "refreshing OAuth token");

                let cancel = CancellationToken::new();
                cancel_tokens.insert(name.clone(), cancel.clone());
                let task_generation = generations.get(&name).copied().unwrap_or(0);
                let client = http_client.clone();
                let name_for_task = name;

                in_flight.spawn(async move {
                    let result = crate::reconnect::do_refresh_cancellable(
                        &client, &entry, &cancel,
                    ).await;
                    (name_for_task, task_generation, result)
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn request_count(server: &wiremock::MockServer) -> usize {
        server.received_requests().await.unwrap().len()
    }

    async fn wait_for_scheduler_entry(agent_dir: &std::path::Path, server_name: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let found = match right_db::open_connection(agent_dir, false).await {
                Ok(conn) => load_oauth_entries_from_db(&conn)
                    .await
                    .map(|entries| entries.iter().any(|(name, _)| name == server_name))
                    .unwrap_or(false),
                Err(_) => false,
            };
            if found {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!("scheduler did not persist OAuth state for {server_name}");
            }
            tokio::task::yield_now().await;
        }
    }

    async fn wait_for_token(
        token: &Arc<tokio::sync::RwLock<Option<String>>>,
        expected: &str,
        server: &wiremock::MockServer,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if token.read().await.as_deref() == Some(expected) {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!(
                    "token did not update to {expected}; received_requests={}",
                    request_count(server).await
                );
            }
            tokio::task::yield_now().await;
        }
    }

    async fn wait_for_backend_status(
        backend: &crate::proxy::ProxyBackend,
        expected: crate::proxy::BackendStatus,
        server: &wiremock::MockServer,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if backend.status().await == expected {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!(
                    "backend status did not update to {expected:?}; current={:?}; received_requests={}",
                    backend.status().await,
                    request_count(server).await
                );
            }
            tokio::task::yield_now().await;
        }
    }

    async fn wait_for_response_count(count: &AtomicUsize, expected: usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if count.load(Ordering::SeqCst) >= expected {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!(
                    "mock only produced {} responses, expected at least {expected}",
                    count.load(Ordering::SeqCst)
                );
            }
            tokio::task::yield_now().await;
        }
    }

    async fn advance_until_request_count(server: &wiremock::MockServer, expected: usize) {
        for _ in 0..200 {
            if request_count(server).await >= expected {
                return;
            }
            tokio::time::advance(Duration::from_secs(5)).await;
            for _ in 0..5 {
                tokio::task::yield_now().await;
                if request_count(server).await >= expected {
                    return;
                }
            }
        }
        panic!(
            "server received {} requests, expected at least {expected}",
            request_count(server).await
        );
    }

    async fn advance_until_token(
        token: &Arc<tokio::sync::RwLock<Option<String>>>,
        expected: &str,
        server: &wiremock::MockServer,
    ) {
        for _ in 0..200 {
            if token.read().await.as_deref() == Some(expected) {
                return;
            }
            tokio::time::advance(Duration::from_secs(5)).await;
            for _ in 0..5 {
                tokio::task::yield_now().await;
                if token.read().await.as_deref() == Some(expected) {
                    return;
                }
            }
        }
        panic!(
            "token did not update to {expected}; received_requests={}",
            request_count(server).await
        );
    }

    #[test]
    fn backoff_cap_matches_last_step() {
        assert_eq!(
            *TRANSIENT_BACKOFF_SECS.last().expect("steps not empty"),
            TRANSIENT_BACKOFF_CAP_SECS,
            "cap must equal last step or backoff regresses on the boundary"
        );
        assert_eq!(transient_backoff_secs(999), TRANSIENT_BACKOFF_CAP_SECS);
    }

    #[tokio::test]
    async fn load_oauth_entries_from_db_test() {
        let (_dir, conn) = right_db::test_support::migrated_connection().await;
        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type, auth_token, refresh_token, \
             token_endpoint, client_id, expires_at) \
             VALUES ('notion', 'https://mcp.notion.com/mcp', 'oauth', 'tok', 'rt', \
             'https://ex.com/token', 'cid', '2026-04-13T12:00:00+00:00')",
            [],
        )
        .await
        .unwrap();
        let entries = load_oauth_entries_from_db(&conn).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "notion");
        assert_eq!(entries[0].1.client_id, "cid");
        assert_eq!(entries[0].1.resource, "https://mcp.notion.com/mcp");
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
            resource: "https://example.com/mcp".into(),
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
            resource: "https://example.com/mcp".into(),
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
            resource: "https://example.com/mcp".into(),
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
            resource: "https://example.com/mcp".into(),
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
        // 200. Use one stateful responder instead of overlapping mocks so
        // response order is explicit.
        let responses = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with({
                let responses_for_mock = responses.clone();
                move |_: &wiremock::Request| {
                    let index = responses_for_mock.fetch_add(1, Ordering::SeqCst);
                    if index < 3 {
                        ResponseTemplate::new(503).set_body_string("temporarily down")
                    } else {
                        ResponseTemplate::new(200).set_body_json(serde_json::json!({
                            "access_token": "new-tok",
                            "refresh_token": "new-rt",
                            "expires_in": 3600,
                            "token_type": "Bearer"
                        }))
                    }
                }
            })
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('s', 'https://x/mcp', 'oauth')",
            [],
        )
        .await
        .unwrap();
        drop(conn);

        let entry_state = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: format!("{}/token", server.uri()),
            client_id: "c".into(),
            client_secret: None,
            // Already expired -> fires immediately.
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
            server_url: "https://x/mcp".into(),
            resource: "https://x/mcp".into(),
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

        wait_for_scheduler_entry(tmp.path(), "s").await;

        // First scheduler fire calls do_refresh_cancellable, which retries
        // internally 3 times. In test builds those backoffs are milliseconds.
        // All three initial attempts return 503 -> Transient. The scheduler's
        // next fire hits the 200 mock -> success.
        wait_for_response_count(&responses, 4).await;
        wait_for_token(&token_arc, "new-tok", &server).await;

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
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('s', 'https://x/mcp', 'oauth')",
            [],
        )
        .await
        .unwrap();
        drop(conn);

        let entry_state = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: format!("{}/token", server.uri()),
            client_id: "c".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
            server_url: "https://x/mcp".into(),
            resource: "https://x/mcp".into(),
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

        wait_for_scheduler_entry(tmp.path(), "s").await;

        // Already-expired tokens fire immediately. This test uses real time:
        // real reqwest/wiremock I/O is a poor fit for paused Tokio time.
        wait_for_backend_status(&backend, crate::proxy::BackendStatus::NeedsAuth, &server).await;

        scheduler.abort();
    }

    /// Verify that the scheduler stays responsive to `rx.recv` while a
    /// refresh is in flight. Specifically: `RemoveServer` MUST cancel an
    /// in-flight refresh quickly, and a subsequent `NewEntry` for a
    /// different server MUST be processed without waiting for the
    /// cancelled refresh's full backoff (~210s).
    ///
    /// Before Fix #1, the scheduler `.await`ed `do_refresh_cancellable`
    /// directly inside the timer arm, starving the `rx.recv` arm for the
    /// entire duration of the (potentially exhausting) retry loop. After
    /// the fix, refreshes are spawned into a JoinSet and `RemoveServer`
    /// cancels them via the per-server CancellationToken.
    #[tokio::test]
    async fn remove_server_cancels_in_flight_refresh() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Server "a" never finishes — every request to /a/token delays
        // ~600s before returning 503. Without cancellation, the refresh
        // task would block the scheduler for at least ~10 minutes
        // (3 attempts × 600s per attempt). With cancellation, the task
        // returns Err(Cancelled) as soon as the per-attempt request is
        // dropped or, more importantly, before subsequent attempts run.
        //
        // Server "b" responds immediately with a successful token —
        // proving the scheduler processed a fresh NewEntry while the
        // "a" refresh was still in flight (now cancelled).
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/a/token"))
            .respond_with(
                ResponseTemplate::new(503)
                    .set_delay(std::time::Duration::from_secs(600))
                    .set_body_string("slow"),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/b/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "b-tok",
                "refresh_token": "b-rt",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type) VALUES \
             ('a', 'https://x/a/mcp', 'oauth'), \
             ('b', 'https://x/b/mcp', 'oauth')",
            [],
        )
        .await
        .unwrap();
        drop(conn);

        let entry_a = OAuthServerState {
            refresh_token: Some("rt-a".into()),
            token_endpoint: format!("{}/a/token", server.uri()),
            client_id: "c".into(),
            client_secret: None,
            // Already expired -> fires immediately.
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
            server_url: "https://x/a/mcp".into(),
            resource: "https://x/a/mcp".into(),
        };
        let entry_b = OAuthServerState {
            refresh_token: Some("rt-b".into()),
            token_endpoint: format!("{}/b/token", server.uri()),
            client_id: "c".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
            server_url: "https://x/b/mcp".into(),
            resource: "https://x/b/mcp".into(),
        };
        let token_a: Arc<tokio::sync::RwLock<Option<String>>> =
            Arc::new(tokio::sync::RwLock::new(Some("old-a".into())));
        let token_b: Arc<tokio::sync::RwLock<Option<String>>> =
            Arc::new(tokio::sync::RwLock::new(Some("old-b".into())));
        let backend_a = Arc::new(crate::proxy::ProxyBackend::new(
            "a".into(),
            tmp.path().to_path_buf(),
            "https://x/a/mcp".into(),
            token_a.clone(),
            crate::proxy::AuthMethod::Bearer,
        ));
        let backend_b = Arc::new(crate::proxy::ProxyBackend::new(
            "b".into(),
            tmp.path().to_path_buf(),
            "https://x/b/mcp".into(),
            token_b.clone(),
            crate::proxy::AuthMethod::Bearer,
        ));

        tokio::time::pause();

        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let scheduler = tokio::spawn(run_refresh_scheduler(tmp.path().to_path_buf(), rx));

        // Register server "a" — its refresh will be spawned and stuck on
        // the slow 600s response.
        tx.send(RefreshMessage::NewEntry {
            server_name: "a".into(),
            state: entry_a,
            token: token_a.clone(),
            backend: backend_a.clone(),
        })
        .await
        .unwrap();

        wait_for_scheduler_entry(tmp.path(), "a").await;
        advance_until_request_count(&server, 1).await;

        // Remove server "a" — must cancel the in-flight refresh. Then
        // immediately register server "b". If rx.recv were starved, this
        // second NewEntry would be queued behind the (~10-minute)
        // in-flight refresh and we'd never see the token update.
        tx.send(RefreshMessage::RemoveServer {
            server_name: "a".into(),
        })
        .await
        .unwrap();
        tx.send(RefreshMessage::NewEntry {
            server_name: "b".into(),
            state: entry_b,
            token: token_b.clone(),
            backend: backend_b.clone(),
        })
        .await
        .unwrap();

        wait_for_scheduler_entry(tmp.path(), "b").await;
        advance_until_token(&token_b, "b-tok", &server).await;

        // Token "a" must NOT have been refreshed (the slow mock never
        // returns a real token; cancellation should drop the task).
        assert_eq!(
            *token_a.read().await,
            Some("old-a".to_string()),
            "cancelled refresh must not update token 'a'"
        );

        scheduler.abort();
    }
}
