//! Periodic health reconciler: keeps external MCP `BackendStatus` honest on the
//! Connected↔Unreachable axis (and demotes to NeedsAuth when a probe reveals
//! auth death). See docs/superpowers/specs/2026-05-31-mcp-health-reconciler-design.md.

use std::time::Duration;

use crate::proxy::{BackendStatus, ProbeOutcome};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
