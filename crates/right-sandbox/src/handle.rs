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

use microsandbox::{MicrosandboxError, PlannedChange, Sandbox};

use crate::error::SandboxError;
use crate::exec::{ExecOutcome, ExecRequest, ExecStream, apply_request};
use crate::phase::SandboxPhase;
use crate::secrets::{
    SecretApply, SecretApplyDisposition, SecretBinding, SecretRemove, SecretRemoveDisposition,
    addition_supported, classify_apply_change, host_rotation_stages,
};
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

/// Never-routable exact host used as a crash-window sentinel while rotating a
/// secret's allow-list down to empty. The SDK treats an empty host list as
/// "unchanged", so a shrink that removes every old host must name a single
/// impossible destination until rotation succeeds; the winning rotation then
/// replaces this placeholder outright. `.invalid.` is reserved by RFC 2606
/// and can never resolve, so no substituted credential can reach it even if a
/// crash leaves the sandbox configured with this host.
const INVALID_HOST_SENTINEL: &str = "invalid.";

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
        let _resolver = microsandbox::sandbox::install_secret_resolvers(
            spec.secrets.iter().map(SecretBinding::resolver_value),
        );
        match Self::attach(&spec.name).await {
            Ok(handle) => {
                tracing::debug!(sandbox = %spec.name, "attached to existing sandbox");
                Ok(handle)
            }
            Err(SandboxError::NotFound { .. }) => {
                let builder = spec.to_builder()?;
                let sandbox =
                    builder
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

    /// Apply one store-backed provider binding to this sandbox.
    ///
    /// Existing bindings first replace their complete host allow-list through
    /// the SDK's live `HostsUpdated` path, then rotate source material live.
    /// Omitting injection fields preserves the existing query-injection policy.
    /// A missing binding is added with the full non-secret structure through
    /// restart-backed apply, preserving its writable filesystem.
    ///
    /// microsandbox 0.6.10 cannot set query-parameter injection through
    /// `modify().secret()`. Missing bindings that require it therefore fail
    /// explicitly instead of being added with weaker semantics.
    pub async fn apply_secret(&self, binding: &SecretBinding) -> Result<SecretApply, SandboxError> {
        binding.validate()?;
        let _resolver = microsandbox::sandbox::install_secret_resolvers([binding.resolver_value()]);

        // Plan with the complete non-secret shape. A missing binding otherwise
        // produces a false "needs at least one allowed host" conflict before
        // Right can classify it as a restart-backed addition. For an existing
        // binding, material still classifies the patch as a rotation; matching
        // placeholder/hosts do not change that live disposition.
        let fresh = Sandbox::get(&self.name)
            .await
            .map_err(|source| get_error(&self.name, source))?;
        let plan = fresh
            .modify()
            .secret(|s| {
                let mut secret = s
                    .env(&binding.env_var)
                    .source(microsandbox::SecretSource::Env {
                        var: binding.source_env_var.clone(),
                    })
                    .placeholder(&binding.placeholder);
                for host in &binding.allowed_hosts {
                    secret = secret.allow_host(host);
                }
                secret
            })
            .dry_run()
            .await
            .map_err(|source| operation_error(&self.name, "plan secret apply", source))?;

        let secret_change = plan.changes.iter().find_map(|change| match change {
            PlannedChange::Secret(secret) if secret.name == binding.env_var => {
                Some((secret.change, secret.disposition))
            }
            _ => None,
        });
        let secret_change_kind = secret_change.as_ref().map(|(change, _)| *change);
        let secret_disposition = secret_change.as_ref().map(|(_, disposition)| *disposition);
        match secret_change_kind.and_then(classify_apply_change) {
            Some(SecretApplyDisposition::RotatedLive) => {
                if secret_disposition != Some(microsandbox::ModificationDisposition::Live) {
                    return Err(SandboxError::SecretApplyUnsupported {
                        name: self.name.clone(),
                        env_var: binding.env_var.clone(),
                    });
                }
                if !plan.conflicts.is_empty() {
                    return Err(secret_conflict(&self.name, binding, &plan.conflicts));
                }

                // Failure-safe host ordering:
                // 1. remove obsolete hosts while the old credential is live;
                // 2. rotate the credential;
                // 3. add new hosts only after rotation succeeds.
                // A rotation failure can therefore only leave a narrower
                // policy and can never expose the old credential to a new host.
                let current_hosts = self.secret_allowed_hosts(&binding.env_var).await?;
                let (shrink_hosts, widen_hosts) =
                    host_rotation_stages(&current_hosts, &binding.allowed_hosts);
                let mut warnings = modification_warnings(&plan.warnings);
                if let Some(hosts) = shrink_hosts {
                    let fresh = Sandbox::get(&self.name)
                        .await
                        .map_err(|source| get_error(&self.name, source))?;
                    let mut hosts_modify = fresh.modify().secret(|s| {
                        let mut secret = s.env(&binding.env_var);
                        for host in &hosts {
                            secret = secret.allow_host(host);
                        }
                        secret
                    });
                    // The SDK treats an empty host list as "unchanged". When
                    // every old host is removed, use one never-routable exact
                    // sentinel until rotation succeeds, then replace it.
                    if hosts.is_empty() {
                        hosts_modify = fresh
                            .modify()
                            .secret(|s| s.env(&binding.env_var).allow_host(INVALID_HOST_SENTINEL));
                    }
                    let applied = hosts_modify.apply().await.map_err(|source| {
                        operation_error(&self.name, "shrink secret hosts", source)
                    })?;
                    warnings.extend(modification_warnings(&applied.warnings));
                }

                let fresh = Sandbox::get(&self.name)
                    .await
                    .map_err(|source| get_error(&self.name, source))?;
                let applied = fresh
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
                warnings.extend(modification_warnings(&applied.warnings));

                if let Some(hosts) = widen_hosts {
                    let fresh = Sandbox::get(&self.name)
                        .await
                        .map_err(|source| get_error(&self.name, source))?;
                    let hosts_modify = fresh.modify().secret(|s| {
                        let mut secret = s.env(&binding.env_var);
                        for host in &hosts {
                            secret = secret.allow_host(host);
                        }
                        secret
                    });
                    let applied = hosts_modify.apply().await.map_err(|source| {
                        operation_error(&self.name, "widen secret hosts", source)
                    })?;
                    warnings.extend(modification_warnings(&applied.warnings));
                }
                Ok(SecretApply {
                    disposition: SecretApplyDisposition::RotatedLive,
                    warnings,
                })
            }
            Some(SecretApplyDisposition::AddedWithRestart) => {
                // Full validation already ran before planning.
                if secret_disposition
                    != Some(microsandbox::ModificationDisposition::RequiresRestart)
                {
                    return Err(SandboxError::SecretApplyUnsupported {
                        name: self.name.clone(),
                        env_var: binding.env_var.clone(),
                    });
                }
                if !addition_supported(binding) {
                    return Err(SandboxError::SecretAdditionUnsupported {
                        name: self.name.clone(),
                        env_var: binding.env_var.clone(),
                        reason: "the SDK modify API cannot preserve query-parameter injection for a new binding",
                    });
                }
                if !plan.conflicts.is_empty() {
                    return Err(secret_conflict(&self.name, binding, &plan.conflicts));
                }
                let fresh = Sandbox::get(&self.name)
                    .await
                    .map_err(|source| get_error(&self.name, source))?;
                let mut modify = fresh.modify().secret(|s| {
                    let mut secret = s
                        .env(&binding.env_var)
                        .source(microsandbox::SecretSource::Env {
                            var: binding.source_env_var.clone(),
                        })
                        .placeholder(&binding.placeholder);
                    for host in &binding.allowed_hosts {
                        secret = secret.allow_host(host);
                    }
                    secret
                });
                modify = modify.restart();
                let full_plan = modify.clone().dry_run().await.map_err(|source| {
                    operation_error(&self.name, "plan secret addition", source)
                })?;
                if !full_plan.conflicts.is_empty() {
                    return Err(secret_conflict(&self.name, binding, &full_plan.conflicts));
                }
                let applied = modify
                    .apply()
                    .await
                    .map_err(|source| operation_error(&self.name, "add secret", source))?;
                Ok(SecretApply {
                    disposition: SecretApplyDisposition::AddedWithRestart,
                    warnings: modification_warnings(&applied.warnings),
                })
            }
            _ => Err(SandboxError::SecretApplyUnsupported {
                name: self.name.clone(),
                env_var: binding.env_var.clone(),
            }),
        }
    }

    /// Revoke one guest-visible secret binding from the running sandbox and
    /// its durable configuration.
    ///
    /// Removal is explicit and idempotent. A present binding must plan as a
    /// live removal; Right refuses restart-only or unsupported dispositions so
    /// callers never report success while the old credential is still usable.
    /// Removing the final binding disables TLS interception in desired config;
    /// the running VM keeps TLS interception enabled until next start because
    /// microsandbox 0.6.10 has no live TLS toggle.
    pub async fn remove_secret(&self, env_var: &str) -> Result<SecretRemove, SandboxError> {
        validate_secret_env_var(env_var)?;
        let fresh = Sandbox::get(&self.name)
            .await
            .map_err(|source| get_error(&self.name, source))?;
        let modify = fresh.modify().remove_secret(env_var);
        let plan = modify
            .clone()
            .dry_run()
            .await
            .map_err(|source| operation_error(&self.name, "plan secret removal", source))?;
        if !plan.conflicts.is_empty() {
            return Err(SandboxError::SecretApplyConflict {
                name: self.name.clone(),
                env_var: env_var.to_owned(),
                details: plan
                    .conflicts
                    .iter()
                    .map(|conflict| format!("{}: {}", conflict.field, conflict.message))
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
        let change = plan.changes.iter().find_map(|change| match change {
            PlannedChange::Secret(secret) if secret.name == env_var => Some(secret),
            _ => None,
        });
        let Some(change) = change else {
            return Ok(SecretRemove {
                disposition: SecretRemoveDisposition::AlreadyAbsent,
                warnings: modification_warnings(&plan.warnings),
            });
        };
        if change.change != microsandbox::SecretChangeKind::Removed
            || change.disposition != microsandbox::ModificationDisposition::Live
        {
            return Err(SandboxError::SecretApplyUnsupported {
                name: self.name.clone(),
                env_var: env_var.to_owned(),
            });
        }

        let applied = modify
            .apply()
            .await
            .map_err(|source| operation_error(&self.name, "remove secret", source))?;
        Ok(SecretRemove {
            disposition: SecretRemoveDisposition::RemovedLive,
            warnings: modification_warnings(&applied.warnings),
        })
    }

    /// Guest environment variables currently backed by sandbox secret
    /// substitution entries. This exposes identities only; material and source
    /// references never leave the SDK configuration.
    pub async fn secret_env_vars(&self) -> Result<Vec<String>, SandboxError> {
        let fresh = Sandbox::get(&self.name)
            .await
            .map_err(|source| get_error(&self.name, source))?;
        let config = fresh
            .config()
            .map_err(|source| operation_error(&self.name, "read secret configuration", source))?;
        Ok(config
            .spec
            .network
            .secrets
            .map(|secrets| {
                secrets
                    .secrets
                    .into_iter()
                    .map(|entry| entry.env_var)
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn secret_allowed_hosts(&self, env_var: &str) -> Result<Vec<String>, SandboxError> {
        let fresh = Sandbox::get(&self.name)
            .await
            .map_err(|source| get_error(&self.name, source))?;
        let config = fresh
            .config()
            .map_err(|source| operation_error(&self.name, "read secret configuration", source))?;
        Ok(config
            .spec
            .network
            .secrets
            .and_then(|secrets| {
                secrets
                    .secrets
                    .into_iter()
                    .find(|entry| entry.env_var == env_var)
            })
            .map(|entry| {
                entry
                    .allowed_hosts
                    .into_iter()
                    .map(|host| match host {
                        microsandbox::HostPattern::Exact(host)
                        | microsandbox::HostPattern::Wildcard(host) => host,
                        microsandbox::HostPattern::Any => "*".to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Current source-reference secret identities as `(guest env var, host
    /// source env var)`. Raw values and inline-value secrets are never exposed.
    pub async fn secret_source_refs(&self) -> Result<Vec<(String, String)>, SandboxError> {
        let fresh = Sandbox::get(&self.name)
            .await
            .map_err(|source| get_error(&self.name, source))?;
        let config = fresh
            .config()
            .map_err(|source| operation_error(&self.name, "read secret configuration", source))?;
        Ok(config
            .spec
            .network
            .secrets
            .map(|secrets| {
                secrets
                    .secrets
                    .into_iter()
                    .filter_map(|entry| match entry.source {
                        Some(microsandbox::SecretSource::Env { var }) => Some((entry.env_var, var)),
                        Some(microsandbox::SecretSource::Store { .. }) | None => None,
                    })
                    .collect()
            })
            .unwrap_or_default())
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

#[cfg(test)]
mod secret_removal_tests {
    use super::*;

    #[test]
    fn removal_env_var_validation_matches_binding_identity_rules() {
        assert!(validate_secret_env_var("RIGHT_PROVIDER_KEY").is_ok());
        assert!(validate_secret_env_var("").is_err());
        assert!(validate_secret_env_var("A=B").is_err());
        assert!(validate_secret_env_var("A\0B").is_err());
    }
}

fn validate_secret_env_var(env_var: &str) -> Result<(), SandboxError> {
    if env_var.is_empty() || env_var.contains(['=', '\0']) {
        return Err(SandboxError::InvalidSpec {
            field: "secrets.env_var",
            reason: "must be non-empty and contain no '=' or NUL".to_owned(),
        });
    }
    Ok(())
}

/// Convert an SDK plan conflict into Right's redacted error taxonomy.
fn secret_conflict(
    name: &str,
    binding: &SecretBinding,
    conflicts: &[microsandbox::ModificationConflict],
) -> SandboxError {
    SandboxError::SecretApplyConflict {
        name: name.to_owned(),
        env_var: binding.env_var.clone(),
        details: conflicts
            .iter()
            .map(|conflict| format!("{}: {}", conflict.field, conflict.message))
            .collect::<Vec<_>>()
            .join("; "),
    }
}

fn modification_warnings(warnings: &[microsandbox::ModificationWarning]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| format!("{}: {}", warning.field, warning.message))
        .collect()
}

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
