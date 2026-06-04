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
    /// The sandbox exists, but its pod entered OpenShell Phase: Error.
    /// The gateway may still be reachable; this is distinct from Unreachable.
    SandboxError {
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
            GatewayCause::SandboxError { .. } => (
                "my secure sandbox stopped responding",
                vec![
                    "I'll reconnect automatically as soon as the sandbox is back. If it stays down, the sandbox container needs to be restarted in OpenShell -- your data is preserved.",
                ],
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
            GatewayCause::SandboxError { sandbox } => {
                format!("my secure sandbox '{sandbox}' is in an error state")
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

/// Diagnose a *connect* failure by probing the backend with
/// `openshell doctor check` and `openshell status`. Falls back to
/// `Unreachable` if the CLI cannot be run. Never returns an error — a
/// diagnosis is always producible.
///
/// # Error-swallowing exception
///
/// This function deliberately degrades CLI-spawn failures to empty output
/// (which classifies as `Unreachable`) rather than propagating an error.
/// It runs precisely when the sandbox backend is already broken, so a
/// secondary spawn failure must not prevent a diagnosis from reaching the
/// operator. This is the only sanctioned `Err(_) => String::new()` pattern
/// in the codebase.
pub async fn diagnose_gateway() -> GatewayDiagnosis {
    async fn run(args: &[&str]) -> String {
        match tokio::process::Command::new("openshell")
            .args(args)
            .env("NO_COLOR", "1")
            .output()
            .await
        {
            Ok(o) => {
                let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
                s.push('\n');
                s.push_str(&String::from_utf8_lossy(&o.stderr));
                s
            }
            Err(_) => String::new(),
        }
    }
    let doctor = run(&["doctor", "check"]).await;
    let status = run(&["status"]).await;
    classify_doctor_output(&doctor, &status).diagnose()
}

#[cfg(test)]
#[path = "diagnosis_tests.rs"]
mod tests;
