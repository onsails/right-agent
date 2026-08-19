//! Lifecycle phase of an Agent Sandbox.
//!
//! Mirrors the SDK's `SandboxStatus` without leaking it into the public API:
//! the phase set is a stable contract for the supervisor, while the SDK enum
//! stays free to change under a pinned-version bump.

use std::fmt;

use microsandbox::sandbox::SandboxStatus;

/// Lifecycle phase of an Agent Sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SandboxPhase {
    /// Recorded in the runtime catalog but never booted.
    Created,

    /// Boot in progress.
    Starting,

    /// Booted and serving the agent bridge.
    Running,

    /// Graceful shutdown in progress.
    Draining,

    /// Suspended.
    Paused,

    /// Cleanly stopped. Terminal.
    Stopped,

    /// Exited unexpectedly. Terminal.
    Crashed,
}

impl SandboxPhase {
    /// Whether the sandbox is serving exec/fs traffic.
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether the phase is terminal (mirrors the SDK's `Stopped | Crashed`
    /// terminal predicate).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Crashed)
    }
}

impl From<SandboxStatus> for SandboxPhase {
    fn from(status: SandboxStatus) -> Self {
        match status {
            SandboxStatus::Created => Self::Created,
            SandboxStatus::Starting => Self::Starting,
            SandboxStatus::Running => Self::Running,
            SandboxStatus::Draining => Self::Draining,
            SandboxStatus::Paused => Self::Paused,
            SandboxStatus::Stopped => Self::Stopped,
            SandboxStatus::Crashed => Self::Crashed,
        }
    }
}

impl fmt::Display for SandboxPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Created => "created",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Draining => "draining",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
            Self::Crashed => "crashed",
        };
        f.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sdk_status_maps_to_a_phase() {
        let statuses = [
            (SandboxStatus::Created, SandboxPhase::Created),
            (SandboxStatus::Starting, SandboxPhase::Starting),
            (SandboxStatus::Running, SandboxPhase::Running),
            (SandboxStatus::Draining, SandboxPhase::Draining),
            (SandboxStatus::Paused, SandboxPhase::Paused),
            (SandboxStatus::Stopped, SandboxPhase::Stopped),
            (SandboxStatus::Crashed, SandboxPhase::Crashed),
        ];
        for (status, phase) in statuses {
            assert_eq!(SandboxPhase::from(status), phase);
        }
    }

    #[test]
    fn terminal_matches_upstream_predicate() {
        assert!(SandboxPhase::Stopped.is_terminal());
        assert!(SandboxPhase::Crashed.is_terminal());
        for phase in [
            SandboxPhase::Created,
            SandboxPhase::Starting,
            SandboxPhase::Running,
            SandboxPhase::Draining,
            SandboxPhase::Paused,
        ] {
            assert!(!phase.is_terminal(), "{phase} must not be terminal");
        }
    }
}
