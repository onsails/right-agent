//! Cause-specific diagnosis for sandbox-backend (OpenShell gateway) failures.
//!
//! The connect error itself is opaque (`transport error`); the cause is found
//! by probing `openshell doctor check` + `openshell status`. Classification is
//! a pure function over captured CLI text so it is unit-testable.

use std::path::PathBuf;

/// Why the sandbox backend is unusable. Every variant is operator-fixable
/// without recreating the sandbox or restarting the bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayCause {
    NotInstalled,
    DockerDown,
    /// Docker is up but the gateway is not listening.
    GatewayNotStarted,
    BrokenCerts(PathBuf),
    VersionTooOld {
        found: String,
        min: String,
    },
    SandboxNotFound {
        sandbox: String,
    },
    /// Connect failed but probes are inconclusive (race/transient).
    Unreachable,
}

/// A human-facing, cause-specific diagnosis: one consequence-first summary
/// plus ordered, actionable fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayDiagnosis {
    pub cause: GatewayCause,
    pub summary: String,
    pub fixes: Vec<String>,
}

impl GatewayCause {
    /// Build the operator-facing diagnosis for this cause.
    pub fn diagnose(self) -> GatewayDiagnosis {
        let (summary, fixes): (&str, Vec<&str>) = match &self {
            GatewayCause::NotInstalled => (
                "the sandbox backend isn't installed",
                vec!["Install OpenShell from https://github.com/NVIDIA/OpenShell"],
            ),
            GatewayCause::DockerDown => (
                "Docker isn't running",
                vec!["Start Docker, then I'll reconnect automatically"],
            ),
            GatewayCause::GatewayNotStarted => (
                "the sandbox gateway isn't running",
                vec!["Run: openshell gateway start"],
            ),
            GatewayCause::BrokenCerts(_) => (
                "the sandbox gateway is missing its security certificates",
                vec!["Run: openshell gateway destroy && openshell gateway start"],
            ),
            GatewayCause::VersionTooOld { .. } => (
                "the installed OpenShell is too old",
                vec!["Upgrade OpenShell, then restart the gateway"],
            ),
            GatewayCause::SandboxNotFound { .. } => (
                "my sandbox doesn't exist yet",
                vec!["Create it on the host with: right init"],
            ),
            GatewayCause::Unreachable => (
                "I can't reach the sandbox backend",
                vec![
                    "Check Docker is running",
                    "Run: openshell gateway start",
                    "Check OpenShell version (openshell --version)",
                ],
            ),
        };
        // Enrich messages that carry data.
        let summary = match &self {
            GatewayCause::VersionTooOld { found, min } => {
                format!("the installed OpenShell ({found}) is older than the required {min}")
            }
            GatewayCause::SandboxNotFound { sandbox } => {
                format!("my sandbox '{sandbox}' doesn't exist yet")
            }
            _ => summary.to_owned(),
        };
        GatewayDiagnosis {
            cause: self,
            summary,
            fixes: fixes.into_iter().map(str::to_owned).collect(),
        }
    }
}

/// Classify a connect failure from `openshell doctor check` + `openshell status`
/// output. Pure: no I/O. Brittle CLI-wording lives here and nowhere else.
pub fn classify_doctor_output(doctor: &str, status: &str) -> GatewayCause {
    let doctor_l = doctor.to_lowercase();
    if doctor_l.contains("docker") && doctor_l.contains("failed") {
        return GatewayCause::DockerDown;
    }
    // Docker OK but the gateway server refused the connection.
    let status_l = status.to_lowercase();
    if status_l.contains("connection refused") || status_l.contains("connect)") {
        return GatewayCause::GatewayNotStarted;
    }
    GatewayCause::Unreachable
}

#[cfg(test)]
#[path = "diagnosis_tests.rs"]
mod tests;
