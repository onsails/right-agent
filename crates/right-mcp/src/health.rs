//! Periodic health reconciler: keeps external MCP `BackendStatus` honest on the
//! Connected↔Unreachable axis (and demotes to NeedsAuth when a probe reveals
//! auth death). See docs/superpowers/specs/2026-05-31-mcp-health-reconciler-design.md.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::proxy::{BackendStatus, ProbeOutcome, ProxyBackend, ProxyError};

/// Probe cadence for a healthy backend (light backstop; the tool-call event
/// path catches death between ticks).
pub(crate) const CONNECTED_CADENCE: Duration = Duration::from_secs(120);
/// Probe cadence for a down backend (the only path that detects recovery).
pub(crate) const UNREACHABLE_CADENCE: Duration = Duration::from_secs(20);
/// Consecutive Dead probes required before flipping Connected → Unreachable.
pub(crate) const MAX_STRIKES: u32 = 3;
/// Per-probe timeout — a black-holed connection must not wedge a tick.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Decision for a `Connected` backend after a probe, given its prior strike count.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConnectedDecision {
    /// Stay Connected; carry this strike count forward.
    Stay { strikes: u32 },
    /// Flip to Unreachable (strike budget exhausted).
    Unreachable,
    /// Flip to NeedsAuth (probe revealed auth death; debounce-exempt).
    NeedsAuth,
}

/// Pure debounce policy for a Connected backend. `strikes` is the count BEFORE
/// this probe. A Dead probe increments; reaching `MAX_STRIKES` flips Unreachable.
/// Alive resets to 0; AuthRequired flips immediately.
pub(crate) fn decide_connected(strikes: u32, outcome: &ProbeOutcome) -> ConnectedDecision {
    match outcome {
        ProbeOutcome::Alive => ConnectedDecision::Stay { strikes: 0 },
        ProbeOutcome::AuthRequired => ConnectedDecision::NeedsAuth,
        ProbeOutcome::Dead(_) => {
            let next = strikes + 1;
            if next >= MAX_STRIKES {
                ConnectedDecision::Unreachable
            } else {
                ConnectedDecision::Stay { strikes: next }
            }
        }
    }
}

/// Cadence for the next probe of a backend in `status`. `None` = never probe
/// (NeedsAuth — owned by refresh/reconnect).
pub(crate) fn cadence_for(status: BackendStatus) -> Option<Duration> {
    match status {
        BackendStatus::Connected => Some(CONNECTED_CADENCE),
        BackendStatus::Unreachable => Some(UNREACHABLE_CADENCE),
        BackendStatus::NeedsAuth => None,
    }
}

/// Post-probe result for a single backend within one reconcile tick.
enum Probed {
    /// Backend was Connected; carries the liveness probe outcome.
    Connected(ProbeOutcome),
    /// Backend was Unreachable (reconnect attempted) or NeedsAuth (skipped);
    /// carries the post-probe status read back from the backend.
    Settled(BackendStatus),
}

/// Per-agent health reconciler. Runs for process lifetime. Holds only a clone of
/// the shared `proxies` map (also held by the dispatcher), so it lives as long as
/// the process — matching the refresh scheduler's fire-and-forget model.
pub async fn run_health_reconciler(
    proxies: Arc<RwLock<HashMap<String, Arc<ProxyBackend>>>>,
    http_client: reqwest::Client,
) {
    let mut strikes: HashMap<String, u32> = HashMap::new();
    let mut next_due: HashMap<String, tokio::time::Instant> = HashMap::new();

    loop {
        let snapshot: Vec<(String, Arc<ProxyBackend>)> = {
            let guard = proxies.read().await;
            guard.iter().map(|(n, b)| (n.clone(), b.clone())).collect()
        };
        let now = tokio::time::Instant::now();

        // Reclaim strike/schedule state for backends removed from the map at
        // runtime, so neither HashMap grows unbounded over process lifetime.
        let live: std::collections::HashSet<&str> =
            snapshot.iter().map(|(n, _)| n.as_str()).collect();
        strikes.retain(|k, _| live.contains(k.as_str()));
        next_due.retain(|k, _| live.contains(k.as_str()));

        // Build the due set; NeedsAuth backends are pruned from schedule/strikes.
        let mut due = Vec::new();
        for (name, backend) in &snapshot {
            let status = backend.status().await;
            if cadence_for(status).is_none() {
                next_due.remove(name);
                strikes.remove(name);
                continue;
            }
            if next_due.get(name).map(|t| *t <= now).unwrap_or(true) {
                due.push((name.clone(), backend.clone(), status));
            }
        }

        // Concurrent probes. Returns the post-probe outcome/status per backend.
        let probed = futures::future::join_all(due.into_iter().map(|(name, backend, status)| {
            let http = http_client.clone();
            async move {
                let p = match status {
                    BackendStatus::Connected => {
                        let o =
                            match tokio::time::timeout(PROBE_TIMEOUT, backend.probe_live()).await {
                                Ok(o) => o,
                                Err(_) => ProbeOutcome::Dead("probe timeout".into()),
                            };
                        // A dead session on a still-reachable backend (idle-session
                        // expiry — e.g. obsidian-local-rest-api's 404 "Session not
                        // found") is restored in this same tick instead of waiting
                        // out MAX_STRIKES * CONNECTED_CADENCE. A successful reconnect
                        // keeps the backend Connected with zero Unreachable window
                        // (cron/tool calls never see `unreachable`); a failed
                        // reconnect falls through to the strike debounce below, so a
                        // genuinely-down backend still demotes after MAX_STRIKES. The
                        // tool-call path self-heals identically, so this only closes
                        // the between-calls gap the reconciler used to leave open.
                        let o = if matches!(o, ProbeOutcome::Dead(_)) {
                            match tokio::time::timeout(PROBE_TIMEOUT, backend.connect(http)).await {
                                Ok(Ok(_)) => ProbeOutcome::Alive,
                                Ok(Err(ProxyError::NeedsAuth { .. })) => ProbeOutcome::AuthRequired,
                                // Non-auth failure or timeout: keep `Dead` so the
                                // strike debounce below runs. `connect()` already
                                // logged the redacted reason and recorded
                                // `last_connect_error`; do NOT re-log the error here
                                // (its chain can carry a query-string credential).
                                _ => {
                                    tracing::debug!(server = %name, "health: in-tick reconnect failed; falling through to strike debounce");
                                    o
                                }
                            }
                        } else {
                            o
                        };
                        Probed::Connected(o)
                    }
                    BackendStatus::Unreachable => {
                        // `connect()` records status internally; a failed reconnect
                        // simply leaves the backend Unreachable for the next tick.
                        let _ = tokio::time::timeout(PROBE_TIMEOUT, backend.connect(http)).await;
                        Probed::Settled(backend.status().await)
                    }
                    BackendStatus::NeedsAuth => Probed::Settled(status),
                };
                (name, backend, p)
            }
        }))
        .await;

        // Apply decisions serially (single mutator of strikes/next_due).
        for (name, backend, p) in probed {
            let new_status = match p {
                Probed::Connected(outcome) => {
                    let prev = *strikes.get(&name).unwrap_or(&0);
                    match decide_connected(prev, &outcome) {
                        ConnectedDecision::Stay { strikes: s } => {
                            strikes.insert(name.clone(), s);
                            if let ProbeOutcome::Dead(detail) = &outcome {
                                tracing::debug!(server = %name, strikes = s, max = MAX_STRIKES, %detail, "health: dead probe");
                            }
                            BackendStatus::Connected
                        }
                        ConnectedDecision::Unreachable => {
                            strikes.remove(&name);
                            // Only flip to Unreachable if the backend is still
                            // Connected. A concurrent tool-call 401 or refresh
                            // failure may have set NeedsAuth during the probe
                            // window; auth death is debounce-exempt, so never
                            // clobber it back to Unreachable.
                            if backend
                                .compare_and_set_status(
                                    BackendStatus::Connected,
                                    BackendStatus::Unreachable,
                                )
                                .await
                            {
                                tracing::warn!(server = %name, "health: connected → unreachable (strike {MAX_STRIKES}/{MAX_STRIKES})");
                                BackendStatus::Unreachable
                            } else {
                                let actual = backend.status().await;
                                tracing::debug!(server = %name, ?actual, "health: unreachable flip skipped; status changed during probe");
                                actual
                            }
                        }
                        ConnectedDecision::NeedsAuth => {
                            strikes.remove(&name);
                            // Idempotent w.r.t. a concurrent NeedsAuth set: if the
                            // status already moved off Connected (to NeedsAuth via
                            // a racing tool-call/refresh), the end state is still
                            // NeedsAuth, which is what we want.
                            backend
                                .compare_and_set_status(
                                    BackendStatus::Connected,
                                    BackendStatus::NeedsAuth,
                                )
                                .await;
                            tracing::warn!(server = %name, "health: connected → needs_auth (auth probe)");
                            BackendStatus::NeedsAuth
                        }
                    }
                }
                Probed::Settled(s) => {
                    if s == BackendStatus::Connected {
                        strikes.remove(&name);
                        tracing::info!(server = %name, "health: unreachable → connected");
                    }
                    s
                }
            };
            if let Some(cadence) = cadence_for(new_status) {
                next_due.insert(name, now + cadence);
            } else {
                next_due.remove(&name);
            }
        }

        // Sleep until the earliest next_due; if nothing scheduled, poll on the
        // shorter cadence so newly-added backends get picked up promptly.
        // Cap the wake at UNREACHABLE_CADENCE so backends added to the map at
        // runtime (dashboard mcp_add) are picked up within one short cadence,
        // rather than waiting out a Connected backend's 120s schedule. A bare
        // re-snapshot tick does no network probe — only due backends are probed.
        let wake = next_due
            .values()
            .min()
            .copied()
            .unwrap_or(now + UNREACHABLE_CADENCE)
            .min(now + UNREACHABLE_CADENCE);
        tokio::time::sleep_until(wake).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::AuthMethod;

    type Proxies = Arc<RwLock<HashMap<String, Arc<ProxyBackend>>>>;

    /// A backend pointed at a dead loopback port (no session, so every probe is
    /// `Dead`), registered under "composio" in a fresh proxies map. The returned
    /// `TempDir` must be kept alive for the duration of the test.
    fn dead_backend() -> (tempfile::TempDir, Arc<ProxyBackend>, Proxies) {
        let tmp = tempfile::tempdir().unwrap();
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            tmp.path().to_path_buf(),
            "http://127.0.0.1:1/mcp".into(),
            Arc::new(RwLock::new(None)),
            AuthMethod::default(),
        ));
        let proxies: Proxies = Arc::new(RwLock::new(HashMap::from([(
            "composio".into(),
            backend.clone(),
        )])));
        (tmp, backend, proxies)
    }

    #[test]
    fn alive_resets_strikes() {
        assert_eq!(
            decide_connected(2, &ProbeOutcome::Alive),
            ConnectedDecision::Stay { strikes: 0 }
        );
    }

    #[test]
    fn auth_required_flips_immediately_regardless_of_strikes() {
        assert_eq!(
            decide_connected(0, &ProbeOutcome::AuthRequired),
            ConnectedDecision::NeedsAuth
        );
        assert_eq!(
            decide_connected(2, &ProbeOutcome::AuthRequired),
            ConnectedDecision::NeedsAuth
        );
    }

    #[test]
    fn dead_increments_below_threshold() {
        assert_eq!(
            decide_connected(0, &ProbeOutcome::Dead("x".into())),
            ConnectedDecision::Stay { strikes: 1 }
        );
        assert_eq!(
            decide_connected(1, &ProbeOutcome::Dead("x".into())),
            ConnectedDecision::Stay { strikes: 2 }
        );
    }

    #[test]
    fn dead_flips_on_third_strike_not_second() {
        // strikes=1 (so this is the 2nd Dead) → still Stay
        assert_eq!(
            decide_connected(1, &ProbeOutcome::Dead("x".into())),
            ConnectedDecision::Stay { strikes: 2 }
        );
        // strikes=2 (so this is the 3rd Dead) → flip
        assert_eq!(
            decide_connected(2, &ProbeOutcome::Dead("x".into())),
            ConnectedDecision::Unreachable
        );
    }

    #[test]
    fn cadence_never_for_needs_auth() {
        assert_eq!(cadence_for(BackendStatus::NeedsAuth), None);
        assert_eq!(
            cadence_for(BackendStatus::Connected),
            Some(CONNECTED_CADENCE)
        );
        assert_eq!(
            cadence_for(BackendStatus::Unreachable),
            Some(UNREACHABLE_CADENCE)
        );
    }

    // Regression: the reconciler's Unreachable flip must not clobber a
    // NeedsAuth demotion set by a concurrent tool-call/refresh during the probe
    // window (auth death is debounce-exempt). The flip arm now uses
    // `compare_and_set_status(Connected, Unreachable)`, which only swaps while
    // the backend is still Connected. We test that primitive directly for
    // determinism.
    #[tokio::test]
    async fn compare_and_set_status_only_swaps_on_match() {
        let (_tmp, backend, _proxies) = dead_backend();
        backend.set_status(BackendStatus::Connected).await;

        // Simulate a concurrent tool-call 401: a racing writer moved the backend
        // off Connected to NeedsAuth before the reconciler's flip applies. The
        // reconciler's Unreachable swap must be rejected and leave NeedsAuth.
        backend.set_status(BackendStatus::NeedsAuth).await;
        let swapped = backend
            .compare_and_set_status(BackendStatus::Connected, BackendStatus::Unreachable)
            .await;
        assert!(
            !swapped,
            "CAS must not swap when current status != expected"
        );
        assert_eq!(
            backend.status().await,
            BackendStatus::NeedsAuth,
            "NeedsAuth must survive a losing CAS to Unreachable"
        );

        // When the backend is still Connected, the swap succeeds.
        backend.set_status(BackendStatus::Connected).await;
        let swapped = backend
            .compare_and_set_status(BackendStatus::Connected, BackendStatus::Unreachable)
            .await;
        assert!(swapped, "CAS must swap when current status == expected");
        assert_eq!(backend.status().await, BackendStatus::Unreachable);
    }

    // The May-27 regression anchor: an Unreachable backend must reconnect to
    // Connected on the next probe. This test uses REAL time (no
    // `tokio::time::pause()`): the reconciler's Unreachable path calls
    // `connect()`, which performs real loopback TCP I/O. Driving that I/O while
    // also single-stepping virtual time is fragile; the behavioral assertion
    // (Unreachable → Connected) is what matters, so we bound it with a real
    // 10s timeout and a short real poll interval instead.
    #[tokio::test]
    async fn unreachable_backend_recovers_to_connected_on_probe() {
        let (_srv, url) = crate::test_server::serve_two_tool_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        crate::credentials::db_add_server(&conn, "composio", &url)
            .await
            .unwrap();
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            tmp.path().to_path_buf(),
            url,
            Arc::new(RwLock::new(None)),
            AuthMethod::default(),
        ));
        backend.set_status(BackendStatus::Unreachable).await;
        let proxies: Proxies = Arc::new(RwLock::new(HashMap::from([(
            "composio".into(),
            backend.clone(),
        )])));

        let h = tokio::spawn(run_health_reconciler(proxies, reqwest::Client::new()));
        let recovered = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if backend.status().await == BackendStatus::Connected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            recovered.is_ok(),
            "recovered backend must reconnect within timeout"
        );
        assert_eq!(backend.status().await, BackendStatus::Connected);
        h.abort();
    }

    /// A Connected backend whose upstream is reachable but whose cached session
    /// was dropped (idle-session expiry — e.g. obsidian-local-rest-api's 404
    /// "Session not found") must be reconnected on the FIRST Dead probe, not
    /// after MAX_STRIKES * CONNECTED_CADENCE. The backend must stay Connected
    /// throughout (no Unreachable window that would reject in-flight tool/cron
    /// calls), and its session must be restored (last_success_at advances).
    /// Real time + real loopback I/O, same rationale as the recovery test above.
    #[tokio::test]
    async fn connected_dead_session_reconnects_on_first_probe() {
        crate::ensure_crypto_provider();
        let (_srv, url) = crate::test_server::serve_two_tool_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        crate::credentials::db_add_server(&conn, "obsidian", &url)
            .await
            .unwrap();
        let backend = Arc::new(ProxyBackend::new(
            "obsidian".into(),
            tmp.path().to_path_buf(),
            url,
            Arc::new(RwLock::new(None)),
            AuthMethod::default(),
        ));
        // Establish a live session, then drop it while the server stays up.
        backend.connect(reqwest::Client::new()).await.unwrap();
        assert_eq!(backend.status().await, BackendStatus::Connected);
        let first_success = backend.last_success_at().await.expect("initial connect");
        backend.test_drop_session().await;

        let proxies: Proxies = Arc::new(RwLock::new(HashMap::from([(
            "obsidian".into(),
            backend.clone(),
        )])));
        let h = tokio::spawn(run_health_reconciler(proxies, reqwest::Client::new()));

        let healed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                assert_eq!(
                    backend.status().await,
                    BackendStatus::Connected,
                    "a reachable backend must not flip Unreachable on a dropped session"
                );
                if backend
                    .last_success_at()
                    .await
                    .map(|t| t > first_success)
                    .unwrap_or(false)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            healed.is_ok(),
            "dropped session must be reconnected on the first dead probe, not after demotion"
        );
        assert_eq!(backend.status().await, BackendStatus::Connected);
        h.abort();
    }

    #[tokio::test]
    async fn connected_flips_unreachable_after_three_dead_probes() {
        crate::ensure_crypto_provider();
        let (_tmp, backend, proxies) = dead_backend();
        backend.set_status(BackendStatus::Connected).await;

        // Connected backend with no session → `probe_live` returns Dead
        // immediately (no network), so paused virtual time is reliable here.
        tokio::time::pause();
        let h = tokio::spawn(run_health_reconciler(proxies, reqwest::Client::new()));
        for _ in 0..6 {
            tokio::task::yield_now().await;
            tokio::time::advance(CONNECTED_CADENCE + Duration::from_secs(1)).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(
            backend.status().await,
            BackendStatus::Unreachable,
            "3 dead probes must flip to Unreachable"
        );
        h.abort();
    }

    #[tokio::test]
    async fn needs_auth_is_never_probed() {
        crate::ensure_crypto_provider();
        let (_tmp, backend, proxies) = dead_backend();
        backend.set_status(BackendStatus::NeedsAuth).await;

        tokio::time::pause();
        let h = tokio::spawn(run_health_reconciler(proxies, reqwest::Client::new()));
        for _ in 0..6 {
            tokio::task::yield_now().await;
            tokio::time::advance(CONNECTED_CADENCE + Duration::from_secs(1)).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(
            backend.status().await,
            BackendStatus::NeedsAuth,
            "NeedsAuth must never be touched by health"
        );
        h.abort();
    }
}
