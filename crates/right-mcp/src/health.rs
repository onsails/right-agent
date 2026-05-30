//! Periodic health reconciler: keeps external MCP `BackendStatus` honest on the
//! Connected↔Unreachable axis (and demotes to NeedsAuth when a probe reveals
//! auth death). See docs/superpowers/specs/2026-05-31-mcp-health-reconciler-design.md.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::proxy::{BackendStatus, ProbeOutcome, ProxyBackend};

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
                            if !matches!(outcome, ProbeOutcome::Alive) {
                                tracing::debug!(server = %name, strikes = s, max = MAX_STRIKES, "health: dead probe");
                            }
                            BackendStatus::Connected
                        }
                        ConnectedDecision::Unreachable => {
                            strikes.remove(&name);
                            backend.set_status(BackendStatus::Unreachable).await;
                            tracing::warn!(server = %name, "health: connected → unreachable (strike {MAX_STRIKES}/{MAX_STRIKES})");
                            BackendStatus::Unreachable
                        }
                        ConnectedDecision::NeedsAuth => {
                            strikes.remove(&name);
                            backend.set_status(BackendStatus::NeedsAuth).await;
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
        let wake = next_due
            .values()
            .min()
            .copied()
            .unwrap_or_else(|| tokio::time::Instant::now() + UNREACHABLE_CADENCE);
        tokio::time::sleep_until(wake).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::AuthMethod;

    type Proxies = Arc<RwLock<HashMap<String, Arc<ProxyBackend>>>>;

    async fn set_status(b: &Arc<ProxyBackend>, s: BackendStatus) {
        b.set_status(s).await;
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
        set_status(&backend, BackendStatus::Unreachable).await;
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

    #[tokio::test]
    async fn connected_flips_unreachable_after_three_dead_probes() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            tmp.path().to_path_buf(),
            "http://127.0.0.1:1/mcp".into(),
            Arc::new(RwLock::new(None)),
            AuthMethod::default(),
        ));
        set_status(&backend, BackendStatus::Connected).await;
        let proxies: Proxies = Arc::new(RwLock::new(HashMap::from([(
            "composio".into(),
            backend.clone(),
        )])));

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
        let tmp = tempfile::tempdir().unwrap();
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            tmp.path().to_path_buf(),
            "http://127.0.0.1:1/mcp".into(),
            Arc::new(RwLock::new(None)),
            AuthMethod::default(),
        ));
        set_status(&backend, BackendStatus::NeedsAuth).await;
        let proxies: Proxies = Arc::new(RwLock::new(HashMap::from([(
            "composio".into(),
            backend.clone(),
        )])));

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
