//! Error taxonomy for the Agent Sandbox backend.
//!
//! [`SandboxError`] is the crate's only error type. Every variant that says
//! something about backend *health* maps to a [`SandboxCause`] — the small,
//! operator-facing taxonomy the bot's supervisor and Telegram UX match on
//! (replacing OpenShell's `GatewayCause`). Caller/config errors (invalid spec,
//! guest command spawn failure) map to `None`: they are not backend-health
//! signals, and the supervisor never sees them on bring-up paths.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use microsandbox::MicrosandboxError;

use crate::phase::SandboxPhase;

/// Why the sandbox backend is unusable.
///
/// The operator-facing classification of a [`SandboxError`]. Every variant is
/// either operator-fixable without recreating the sandbox or self-heals on the
/// supervisor's next bring-up pass.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxCause {
    /// The pinned `msb`/`libkrunfw` runtime could not be installed or the
    /// install is incomplete.
    #[error("the sandbox runtime failed to install")]
    RuntimeInstallFailed,

    /// The host lacks a usable hypervisor (Apple Silicon / `/dev/kvm`).
    #[error("this host cannot run microVMs")]
    HypervisorUnavailable,

    /// No sandbox with this name exists in the runtime catalog.
    #[error("sandbox '{sandbox}' does not exist")]
    SandboxNotFound { sandbox: String },

    /// The sandbox exists but is not in a running phase.
    #[error("sandbox '{sandbox}' is not running")]
    SandboxNotRunning { sandbox: String },

    /// The runtime or the in-guest agent could not be reached; the failure is
    /// inconclusive or transient.
    #[error("the sandbox runtime is unreachable")]
    Unreachable,
}

impl SandboxCause {
    /// Build the operator-facing diagnosis for this cause: one
    /// consequence-first summary plus ordered, actionable fixes.
    pub fn diagnose(&self) -> SandboxDiagnosis {
        let (summary, fixes): (String, Vec<&str>) = match self {
            Self::RuntimeInstallFailed => (
                "the sandbox runtime isn't installed correctly".to_owned(),
                vec![
                    "The runtime installs automatically on first use, so this means the download or its verification failed -- check the network connection and try again",
                    "The runtime lives in ~/.microsandbox; deleting that directory forces a clean reinstall",
                ],
            ),
            Self::HypervisorUnavailable => (
                "this machine can't run microVMs".to_owned(),
                vec![
                    "On macOS this requires Apple Silicon",
                    "On Linux, KVM must be available: check that /dev/kvm exists and is readable by your user",
                ],
            ),
            Self::SandboxNotFound { sandbox } => (
                format!("my sandbox '{sandbox}' doesn't exist"),
                vec![
                    "I'll recreate it automatically on the next start -- your agent data on the host is preserved",
                ],
            ),
            Self::SandboxNotRunning { sandbox } => (
                format!("my secure sandbox '{sandbox}' isn't running"),
                vec![
                    "I'll restart it automatically -- your data is preserved. If it stays down, recreate the agent's sandbox.",
                ],
            ),
            Self::Unreachable => (
                "I can't reach the sandbox runtime".to_owned(),
                vec![
                    "Check that no other sandbox operation is wedged, then restart the bot",
                    "If it persists, check the host diagnosis with `msb doctor`",
                ],
            ),
        };
        SandboxDiagnosis {
            cause: self.clone(),
            summary,
            fixes: fixes.into_iter().map(str::to_owned).collect(),
        }
    }
}

/// A human-facing, cause-specific diagnosis: one consequence-first summary
/// plus ordered, actionable fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxDiagnosis {
    pub cause: SandboxCause,
    pub summary: String,
    pub fixes: Vec<String>,
}

/// The SDK error behind a sandbox failure.
///
/// Opaque by design: consumers render it or chain from it but never name a
/// microsandbox type, so stage-4 crates never import the SDK.
#[derive(Debug)]
pub struct SdkError(pub(crate) MicrosandboxError);

impl std::fmt::Display for SdkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SdkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl From<MicrosandboxError> for SdkError {
    fn from(error: MicrosandboxError) -> Self {
        Self(error)
    }
}

/// The crate's only error type.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// `setup::install()` failed while downloading or verifying the pinned
    /// runtime into `~/.microsandbox`.
    #[error("failed to install the pinned microsandbox runtime: {source}")]
    RuntimeInstall {
        #[source]
        source: Box<SdkError>,
    },
    #[error("failed to lock the runtime-install lockfile {}", path.display())]
    RuntimeInstallLock { path: PathBuf, source: io::Error },

    /// `setup::install()` returned success but the runtime files are still
    /// absent — the install cannot be trusted.
    #[error("the runtime install reported success but is still not present")]
    RuntimeInstallVerify,

    /// Host preflight (`setup::diagnose()`) found blocking problems.
    #[error("this host cannot run microVMs: {summary}")]
    HypervisorUnavailable { summary: String, fixes: Vec<String> },

    /// A [`crate::SandboxSpec`] field failed validation before any SDK call.
    #[error("invalid sandbox spec: {field}: {reason}")]
    InvalidSpec { field: &'static str, reason: String },

    /// The named sandbox does not exist in the runtime catalog.
    #[error("sandbox '{name}' not found")]
    NotFound { name: String },

    /// The sandbox exists but is in a non-running phase.
    #[error("sandbox '{name}' is {phase}, not running")]
    NotRunning { name: String, phase: SandboxPhase },

    /// While waiting for readiness the sandbox entered a terminal phase, so
    /// it can never become ready without a restart.
    #[error("sandbox '{name}' reached terminal phase {phase} while waiting to become ready")]
    TerminalBeforeReady { name: String, phase: SandboxPhase },

    /// Readiness polling exhausted the timeout. The last observed phase is
    /// preserved so the caller can distinguish "still booting" from "stuck".
    #[error("sandbox '{name}' did not become ready within {timeout:?} (last phase: {last_phase})")]
    ReadinessTimeout {
        name: String,
        timeout: Duration,
        last_phase: SandboxPhase,
    },

    /// A sandbox operation failed at the SDK/runtime layer.
    #[error("sandbox '{name}': {operation} failed: {source}")]
    Operation {
        name: String,
        operation: &'static str,
        #[source]
        source: Box<SdkError>,
    },

    /// The guest command never started (binary missing, bad cwd, guest user
    /// setup failure). A command-level error: the sandbox itself is healthy.
    #[error("sandbox '{name}': command '{cmd}' failed to start in the guest ({kind}): {message}")]
    ExecSpawn {
        name: String,
        cmd: String,
        kind: String,
        message: String,
    },

    /// Writing to or closing a guest command's stdin failed. The exec session
    /// is torn down; the sandbox itself may be healthy. The message carries
    /// the full `{:#}` chain from the SDK's `ExecStdinError`.
    #[error("sandbox '{name}': stdin for '{cmd}' failed: {message}")]
    ExecStdin {
        name: String,
        cmd: String,
        message: String,
    },

    /// The exec session ended without an exit event — the agent connection
    /// dropped mid-session. A backend-health signal, not a command error.
    #[error("sandbox '{name}': exec session for '{cmd}' ended without an exit event")]
    ExecLost { name: String, cmd: String },

    /// The runtime reported the secret rotation cannot be applied through
    /// `modify()` at all (e.g. capability missing on an old runtime).
    #[error("sandbox '{name}': rotating secret '{env_var}' is not supported by the runtime")]
    RotationUnsupported { name: String, env_var: String },

    /// The rotation plan carried conflicts that must be resolved first.
    #[error("sandbox '{name}': rotating secret '{env_var}' has conflicts: {details}")]
    RotationConflict {
        name: String,
        env_var: String,
        details: String,
    },

    /// The sandbox has no secret bound to this env var to rotate. The plan
    /// classified the change as an add rather than a rotation.
    #[error("sandbox '{name}' has no secret '{env_var}' to rotate")]
    RotationTargetMissing { name: String, env_var: String },
}

impl SandboxError {
    /// The operator-facing backend-health classification, when this error says
    /// something about backend health.
    ///
    /// `None` means the error is a caller/config/command error (invalid spec,
    /// guest command spawn failure, rotation misuse) and says nothing about
    /// the backend. Callers that always need a cause fall back to
    /// [`SandboxCause::Unreachable`], matching the old `GatewayCause`
    /// "inconclusive" fallback.
    pub fn cause(&self) -> Option<SandboxCause> {
        match self {
            Self::RuntimeInstall { .. }
            | Self::RuntimeInstallLock { .. }
            | Self::RuntimeInstallVerify => Some(SandboxCause::RuntimeInstallFailed),
            Self::HypervisorUnavailable { .. } => Some(SandboxCause::HypervisorUnavailable),
            Self::NotFound { name } => Some(SandboxCause::SandboxNotFound {
                sandbox: name.clone(),
            }),
            Self::NotRunning { name, .. }
            | Self::TerminalBeforeReady { name, .. }
            | Self::ReadinessTimeout { name, .. } => Some(SandboxCause::SandboxNotRunning {
                sandbox: name.clone(),
            }),
            Self::Operation { name, source, .. } => Some(classify_sdk_error(name, &source.0)),
            // A dropped exec session is a backend-health signal (agent/connectivity).
            Self::ExecLost { .. } => Some(SandboxCause::Unreachable),
            Self::InvalidSpec { .. }
            | Self::ExecSpawn { .. }
            | Self::ExecStdin { .. }
            | Self::RotationUnsupported { .. }
            | Self::RotationTargetMissing { .. }
            | Self::RotationConflict { .. } => None,
        }
    }
}

/// Map an SDK error to the backend-health taxonomy.
pub(crate) fn classify_sdk_error(name: &str, error: &MicrosandboxError) -> SandboxCause {
    match error {
        MicrosandboxError::SandboxNotFound(_) => SandboxCause::SandboxNotFound {
            sandbox: name.to_owned(),
        },
        // A boot failure leaves the sandbox stopped/crashed; the hypervisor
        // preflight (`diagnose_host`) is what distinguishes a missing
        // hypervisor from a bad image, so BootStart lands here.
        MicrosandboxError::SandboxNotRunning(_) | MicrosandboxError::BootStart { .. } => {
            SandboxCause::SandboxNotRunning {
                sandbox: name.to_owned(),
            }
        }
        // The pinned runtime tree is incomplete — the install is broken.
        MicrosandboxError::LibkrunfwNotFound(_) => SandboxCause::RuntimeInstallFailed,
        _ => SandboxCause::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_failures_classify_as_runtime_install_failed() {
        let lock = SandboxError::RuntimeInstallLock {
            path: PathBuf::from("/tmp/x.lock"),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
        };
        assert_eq!(lock.cause(), Some(SandboxCause::RuntimeInstallFailed));
        assert_eq!(
            SandboxError::RuntimeInstallVerify.cause(),
            Some(SandboxCause::RuntimeInstallFailed)
        );
    }

    #[test]
    fn lifecycle_errors_classify_with_the_sandbox_name() {
        let not_found = SandboxError::NotFound {
            name: "right-a".to_owned(),
        };
        assert_eq!(
            not_found.cause(),
            Some(SandboxCause::SandboxNotFound {
                sandbox: "right-a".to_owned()
            })
        );

        let timeout = SandboxError::ReadinessTimeout {
            name: "right-a".to_owned(),
            timeout: Duration::from_secs(1),
            last_phase: SandboxPhase::Starting,
        };
        assert_eq!(
            timeout.cause(),
            Some(SandboxCause::SandboxNotRunning {
                sandbox: "right-a".to_owned()
            })
        );

        let terminal = SandboxError::TerminalBeforeReady {
            name: "right-b".to_owned(),
            phase: SandboxPhase::Crashed,
        };
        assert_eq!(
            terminal.cause(),
            Some(SandboxCause::SandboxNotRunning {
                sandbox: "right-b".to_owned()
            })
        );
    }

    #[test]
    fn sdk_error_classification() {
        let name = "right-a";
        assert_eq!(
            classify_sdk_error(name, &MicrosandboxError::SandboxNotFound(name.to_owned())),
            SandboxCause::SandboxNotFound {
                sandbox: name.to_owned()
            }
        );
        assert_eq!(
            classify_sdk_error(name, &MicrosandboxError::SandboxNotRunning(name.to_owned())),
            SandboxCause::SandboxNotRunning {
                sandbox: name.to_owned()
            }
        );
        assert_eq!(
            classify_sdk_error(
                name,
                &MicrosandboxError::LibkrunfwNotFound("libkrunfw.efi".to_owned())
            ),
            SandboxCause::RuntimeInstallFailed
        );
        assert_eq!(
            classify_sdk_error(name, &MicrosandboxError::Custom("weird".to_owned())),
            SandboxCause::Unreachable
        );
    }

    #[test]
    fn caller_and_command_errors_have_no_backend_cause() {
        let invalid = SandboxError::InvalidSpec {
            field: "name",
            reason: "empty".to_owned(),
        };
        assert_eq!(invalid.cause(), None);

        let spawn = SandboxError::ExecSpawn {
            name: "right-a".to_owned(),
            cmd: "claude".to_owned(),
            kind: "NotFound".to_owned(),
            message: "no such file".to_owned(),
        };
        assert_eq!(spawn.cause(), None);
    }

    #[test]
    fn every_cause_produces_a_nonempty_diagnosis() {
        let causes = [
            SandboxCause::RuntimeInstallFailed,
            SandboxCause::HypervisorUnavailable,
            SandboxCause::SandboxNotFound {
                sandbox: "right-a".to_owned(),
            },
            SandboxCause::SandboxNotRunning {
                sandbox: "right-a".to_owned(),
            },
            SandboxCause::Unreachable,
        ];
        for cause in causes {
            let diagnosis = cause.diagnose();
            assert!(!diagnosis.summary.is_empty(), "{cause} summary");
            assert!(!diagnosis.fixes.is_empty(), "{cause} fixes");
        }
    }

    #[test]
    fn operation_errors_classify_through_the_sdk_source() {
        let err = SandboxError::Operation {
            name: "right-a".to_owned(),
            operation: "exec",
            source: Box::new(SdkError(MicrosandboxError::SandboxNotRunning("right-a".to_owned()))),
        };
        assert_eq!(
            err.cause(),
            Some(SandboxCause::SandboxNotRunning {
                sandbox: "right-a".to_owned()
            })
        );
    }
}
