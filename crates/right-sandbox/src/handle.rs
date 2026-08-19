//! The unified Agent Sandbox handle.
//!
//! One [`SandboxHandle`] replaces the OpenShell-era `resolved_sandbox` +
//! `ssh_config_path` pair: construction creates the sandbox detached or
//! re-attaches by name, and every operation (exec, fs, secrets, health, logs)
//! goes through the SDK — no SSH anywhere.
//!
//! Readiness semantics (the brief's lifecycle contract): the SDK's create and
//! `start_detached` already block until the guest agent is serving and fail
//! with a boot error if the VM dies first, so a freshly created/started
//! sandbox needs no extra readiness wait. The poll exists for the *attach
//! race* — a sandbox left in `Created`/`Starting` by another process — and it
//! treats a terminal phase as an immediate error and preserves the last
//! observed phase in a timeout.

use std::time::{Duration, Instant};

use microsandbox::{MicrosandboxError, ModificationDisposition, PlannedChange, Sandbox};

use crate::error::SandboxError;
use crate::exec::{ExecOutcome, ExecRequest, ExecStream, apply_request};
use crate::phase::SandboxPhase;
use crate::secrets::{RotationDisposition, SecretBinding, SecretRotation};
use crate::spec::SandboxSpec;

/// How long readiness polling waits for a mid-boot sandbox to come up.
///
/// Distinct from the SDK's own 180s agent-relay deadline inside create/start:
/// this covers only the attach-race phases (`Created`/`Starting`), which the
/// local backend passes through in well under a second. Stage-1 measured boot
/// at ~10s with an explicit 16 GiB writable layer, so the default leaves
/// comfortable headroom for slow first boots.
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(120);

/// Interval between readiness polls.
const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Grace period given to a sandbox to stop during destroy before it is
/// killed (matches the stage-1 probe cleanup timeout).
const DESTROY_KILL_TIMEOUT: Duration = Duration::from_secs(15);

/// An owned connection to an Agent Sandbox.
///
/// The wrapped SDK sandbox runs detached: it survives this process exiting,
/// and dropping the handle does not stop the sandbox. Lifecycle is explicit —
/// [`stop`](Self::stop)/[`destroy`](Self::destroy).
pub struct SandboxHandle {
    name: String,
    sandbox: Sandbox,
}

impl SandboxHandle {
    /// Create the sandbox from `spec` detached, or attach to the existing
    /// sandbox of the same name.
    ///
    /// Attach wins over create: an existing sandbox is used as-is (the
    /// supervisor owns config-drift reconciliation). A sandbox found in a
    /// non-running phase is started from its persisted config; one found
    /// mid-boot is awaited per [`DEFAULT_READY_TIMEOUT`].
    pub async fn create_or_attach(spec: &SandboxSpec) -> Result<Self, SandboxError> {
        spec.validate()?;
        match Self::attach(&spec.name).await {
            Ok(handle) => {
                tracing::debug!(sandbox = %spec.name, "attached to existing sandbox");
                Ok(handle)
            }
            Err(SandboxError::NotFound { .. }) => {
                tracing::info!(sandbox = %spec.name, "creating sandbox");
                let sandbox = spec
                    .to_builder()?
                    .create_detached()
                    .await
                    .map_err(|source| SandboxError::Operation {
                        name: spec.name.clone(),
                        operation: "create",
                        source: Box::new(crate::error::SdkError(source)),
                    })?;
                Ok(Self::from_sandbox(spec.name.clone(), sandbox))
            }
            Err(err) => Err(err),
        }
    }

    /// Attach to an existing sandbox by name, starting it when stopped.
    ///
    /// This is the bot's restart path: sandboxes do not auto-start on host
    /// reboot, so the supervisor re-attaches by name and this starts the
    /// stopped sandbox from its persisted config.
    pub async fn attach(name: &str) -> Result<Self, SandboxError> {
        let handle = Sandbox::get(name).await.map_err(|e| get_error(name, e))?;
        let phase = SandboxPhase::from(handle.status_snapshot());
        let sandbox = match phase {
            SandboxPhase::Running | SandboxPhase::Draining => handle
                .connect()
                .await
                .map_err(|source| operation_error(name, "connect", source))?,
            SandboxPhase::Stopped | SandboxPhase::Crashed => {
                tracing::info!(sandbox = %name, phase = %phase, "starting stopped sandbox");
                handle
                    .start_detached()
                    .await
                    .map_err(|source| operation_error(name, "start", source))?
            }
            SandboxPhase::Created | SandboxPhase::Starting => {
                wait_ready_by_name(name, DEFAULT_READY_TIMEOUT).await?;
                let fresh = Sandbox::get(name).await.map_err(|e| get_error(name, e))?;
                fresh
                    .connect()
                    .await
                    .map_err(|source| operation_error(name, "connect", source))?
            }
            SandboxPhase::Paused => {
                return Err(SandboxError::NotRunning {
                    name: name.to_owned(),
                    phase,
                });
            }
        };
        Ok(Self::from_sandbox(name.to_owned(), sandbox))
    }

    fn from_sandbox(name: String, sandbox: Sandbox) -> Self {
        Self { name, sandbox }
    }

    /// The sandbox name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The wrapped SDK sandbox handle (crate-internal consumers only).
    pub(crate) fn sdk(&self) -> &Sandbox {
        &self.sandbox
    }

    /// The sandbox's current lifecycle phase, refreshed from the runtime
    /// catalog.
    pub async fn status(&self) -> Result<SandboxPhase, SandboxError> {
        let handle = Sandbox::get(&self.name)
            .await
            .map_err(|e| get_error(&self.name, e))?;
        Ok(SandboxPhase::from(handle.status_snapshot()))
    }

    /// Wait until the sandbox is running.
    ///
    /// A terminal phase is returned immediately as
    /// [`SandboxError::TerminalBeforeReady`]; a timeout preserves the last
    /// observed phase in [`SandboxError::ReadinessTimeout`] — never a bare
    /// bool.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<(), SandboxError> {
        wait_ready_by_name(&self.name, timeout).await
    }

    /// Stop the sandbox gracefully, escalating to a kill. Idempotent: an
    /// already-terminal sandbox is a no-op.
    pub async fn stop(&self) -> Result<(), SandboxError> {
        self.sandbox
            .stop()
            .await
            .map_err(|source| operation_error(&self.name, "stop", source))
    }

    /// Stop (killing if needed) and delete the sandbox and its state.
    ///
    /// Succeeds when the sandbox does not exist, so it is safe to call after
    /// a failed bring-up.
    pub async fn destroy(&self) -> Result<(), SandboxError> {
        Self::delete(&self.name).await.map(|_| ())
    }

    /// Stop (killing if needed) and delete the sandbox named `name`.
    ///
    /// Returns whether a sandbox was actually removed: `Ok(false)` means no
    /// sandbox existed under `name`. Callers that report deletion to a user
    /// MUST propagate that distinction rather than claim a delete they did
    /// not perform.
    ///
    /// Deletion never needs a live connection, so this deliberately does not
    /// go through [`attach`](Self::attach): attaching *starts* a stopped
    /// sandbox, and booting a microVM only to kill it wastes seconds of the
    /// user's time on the `right agent destroy` path.
    pub async fn delete(name: &str) -> Result<bool, SandboxError> {
        let handle = match Sandbox::get(name).await {
            Ok(handle) => handle,
            Err(MicrosandboxError::SandboxNotFound(_)) => return Ok(false),
            Err(source) => return Err(operation_error(name, "get", source)),
        };
        if !SandboxPhase::from(handle.status_snapshot()).is_terminal() {
            handle
                .kill_with_timeout(DESTROY_KILL_TIMEOUT)
                .await
                .map_err(|source| operation_error(name, "kill", source))?;
        }
        match Sandbox::remove(name).await {
            Ok(()) => Ok(true),
            Err(MicrosandboxError::SandboxNotFound(_)) => Ok(false),
            Err(source) => Err(operation_error(name, "remove", source)),
        }
    }

    /// Run a command to completion and collect its output.
    pub async fn exec(&self, request: &ExecRequest) -> Result<ExecOutcome, SandboxError> {
        let output = self
            .sandbox
            .exec_with(&request.cmd, |options| apply_request(options, request))
            .await
            .map_err(|source| self.exec_error("exec", &request.cmd, source))?;
        Ok(ExecOutcome {
            code: output.status().code,
            stdout: output.stdout_bytes().to_vec(),
            stderr: output.stderr_bytes().to_vec(),
        })
    }

    /// Start a streaming exec session: live stdout/stderr events plus an
    /// optional chunked stdin writer.
    pub async fn exec_stream(&self, request: &ExecRequest) -> Result<ExecStream, SandboxError> {
        let inner = self
            .sandbox
            .exec_stream_with(&request.cmd, |options| apply_request(options, request))
            .await
            .map_err(|source| self.exec_error("exec_stream", &request.cmd, source))?;
        Ok(ExecStream::new(
            self.name.clone(),
            request.cmd.clone(),
            inner,
        ))
    }

    /// Health snapshot: refreshed running phase plus memory and
    /// writable-layer figures.
    ///
    /// CPU is deliberately absent: the SDK's reported CPU read zero on every
    /// sample even under load (stage-1 correction 6), so it must never gate
    /// health. Memory/disk figures are correct.
    pub async fn health(&self) -> Result<SandboxHealthReport, SandboxError> {
        let fresh = Sandbox::get(&self.name)
            .await
            .map_err(|e| get_error(&self.name, e))?;
        let phase = SandboxPhase::from(fresh.status_snapshot());
        if !phase.is_running() {
            return Err(SandboxError::NotRunning {
                name: self.name.clone(),
                phase,
            });
        }
        let metrics = fresh
            .metrics()
            .await
            .map_err(|source| operation_error(&self.name, "metrics", source))?;
        Ok(SandboxHealthReport {
            phase,
            memory_used_bytes: metrics.memory_bytes,
            memory_limit_bytes: metrics.memory_limit_bytes,
            memory_available_bytes: metrics.memory_available_bytes,
            writable_layer_used_bytes: metrics.upper_used_bytes,
            writable_layer_free_bytes: metrics.upper_free_bytes,
            uptime: metrics.uptime,
        })
    }

    /// Rotate a source-ref secret on this sandbox, keeping the placeholder
    /// stable.
    ///
    /// The patch re-asserts only the identity (`env`) and the host-side
    /// source reference; the placeholder and allowed hosts are left
    /// untouched, so the change classifies as `Rotated` and applies live when
    /// the runtime advertises the `secrets_update` capability (stage-1 probe
    /// `ci_msb_source_ref_secret_rotates_live`). The new value is resolved
    /// from the host environment at apply time by the SDK.
    ///
    /// Egress/secret *structure* (adding a binding) is create-time only; this
    /// path exists for value rotation of a binding the sandbox booted with.
    pub async fn rotate_secret(
        &self,
        binding: &SecretBinding,
    ) -> Result<SecretRotation, SandboxError> {
        binding.validate_ref()?;

        let plan = self
            .sandbox
            .modify()
            .secret(|s| {
                s.env(&binding.env_var)
                    .source(microsandbox::SecretSource::Env {
                        var: binding.source_env_var.clone(),
                    })
            })
            .dry_run()
            .await
            .map_err(|source| operation_error(&self.name, "plan secret rotation", source))?;

        let mut disposition = RotationDisposition::Live;
        for change in &plan.changes {
            let PlannedChange::Secret(secret) = change else {
                continue;
            };
            // A rotation re-asserts only env+source, so a sandbox that has the
            // binding classifies the change as Rotated. Anything else (notably
            // Added) means the sandbox has no such secret. This must be checked
            // BEFORE the conflicts gate: the SDK also emits a "needs at least
            // one allowed host" conflict for an Added secret, and Right's
            // rotation patch never sets hosts, so the conflict would otherwise
            // mask the real "no such binding" error.
            if secret.change != microsandbox::SecretChangeKind::Rotated {
                return Err(SandboxError::RotationTargetMissing {
                    name: self.name.clone(),
                    env_var: binding.env_var.clone(),
                });
            }
            match secret.disposition {
                ModificationDisposition::Live => {}
                ModificationDisposition::NextStart => {
                    if disposition == RotationDisposition::Live {
                        disposition = RotationDisposition::NextStart;
                    }
                }
                ModificationDisposition::RequiresRestart => {
                    disposition = RotationDisposition::RequiresRestart;
                }
                _ => {
                    return Err(SandboxError::RotationUnsupported {
                        name: self.name.clone(),
                        env_var: binding.env_var.clone(),
                    });
                }
            }
        }

        if !plan.conflicts.is_empty() {
            return Err(SandboxError::RotationConflict {
                name: self.name.clone(),
                env_var: binding.env_var.clone(),
                details: plan
                    .conflicts
                    .iter()
                    .map(|conflict| format!("{}: {}", conflict.field, conflict.message))
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }

        let applied = self
            .sandbox
            .modify()
            .secret(|s| {
                s.env(&binding.env_var)
                    .source(microsandbox::SecretSource::Env {
                        var: binding.source_env_var.clone(),
                    })
            })
            .apply()
            .await
            .map_err(|source| operation_error(&self.name, "rotate secret", source))?;

        Ok(SecretRotation {
            disposition,
            warnings: applied
                .warnings
                .iter()
                .map(|warning| format!("{}: {}", warning.field, warning.message))
                .collect(),
        })
    }

    /// Wrap an fs future's error with sandbox context.
    pub(crate) async fn fs_op<T>(
        &self,
        operation: &'static str,
        future: impl Future<Output = microsandbox::MicrosandboxResult<T>>,
    ) -> Result<T, SandboxError> {
        future
            .await
            .map_err(|source| operation_error(&self.name, operation, source))
    }

    /// Map an exec-spawn/transport error: guest spawn failures are
    /// command-level ([`SandboxError::ExecSpawn`]), everything else is a
    /// backend operation error.
    fn exec_error(
        &self,
        operation: &'static str,
        cmd: &str,
        source: MicrosandboxError,
    ) -> SandboxError {
        match source {
            MicrosandboxError::ExecFailed(failed) => SandboxError::ExecSpawn {
                name: self.name.clone(),
                cmd: cmd.to_owned(),
                kind: format!("{:?}", failed.kind),
                message: crate::exec::format_exec_failed(&failed),
            },
            source => operation_error(&self.name, operation, source),
        }
    }
}

/// The SDK sandbox is opaque, so the name is the only meaningful thing to
/// print — and it is what log lines and `#[derive(Debug)]` containers want.
impl std::fmt::Debug for SandboxHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxHandle")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// A point-in-time health snapshot of a running sandbox.
///
/// See [`SandboxHandle::health`] — CPU is intentionally not reported.
#[derive(Debug, Clone)]
pub struct SandboxHealthReport {
    /// Freshly read lifecycle phase (always `Running` on success).
    pub phase: SandboxPhase,

    /// Resident memory usage in bytes.
    pub memory_used_bytes: u64,

    /// Configured guest memory limit in bytes.
    pub memory_limit_bytes: u64,

    /// Guest-available memory in bytes when the guest reports it.
    pub memory_available_bytes: Option<u64>,

    /// Guest-visible writable-layer used bytes, when available.
    pub writable_layer_used_bytes: Option<u64>,

    /// Guest-visible writable-layer free bytes, when available.
    pub writable_layer_free_bytes: Option<u64>,

    /// Sandbox uptime at sampling time.
    pub uptime: Duration,
}

/// Poll the runtime catalog until `name` is running, a terminal phase is
/// observed, or the timeout expires with the last phase preserved.
async fn wait_ready_by_name(name: &str, timeout: Duration) -> Result<(), SandboxError> {
    let deadline = Instant::now() + timeout;
    loop {
        let handle = Sandbox::get(name).await.map_err(|e| get_error(name, e))?;
        let phase = SandboxPhase::from(handle.status_snapshot());
        match phase {
            SandboxPhase::Running => return Ok(()),
            SandboxPhase::Stopped | SandboxPhase::Crashed | SandboxPhase::Draining => {
                return Err(SandboxError::TerminalBeforeReady {
                    name: name.to_owned(),
                    phase,
                });
            }
            SandboxPhase::Created | SandboxPhase::Starting | SandboxPhase::Paused => {}
        }
        if Instant::now() >= deadline {
            return Err(SandboxError::ReadinessTimeout {
                name: name.to_owned(),
                timeout,
                last_phase: phase,
            });
        }
        tokio::time::sleep(READINESS_POLL_INTERVAL).await;
    }
}

/// Map a `Sandbox::get` failure: not-found is its own variant, everything
/// else is a backend operation error.
fn get_error(name: &str, source: MicrosandboxError) -> SandboxError {
    match source {
        MicrosandboxError::SandboxNotFound(_) => SandboxError::NotFound {
            name: name.to_owned(),
        },
        source => operation_error(name, "get", source),
    }
}
fn operation_error(name: &str, operation: &'static str, source: MicrosandboxError) -> SandboxError {
    SandboxError::Operation {
        name: name.to_owned(),
        operation,
        source: Box::new(crate::error::SdkError(source)),
    }
}
