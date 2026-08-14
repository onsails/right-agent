//! Owns the sandbox-backend lifecycle: first bring-up, degrade, recovery.
//!
//! [`bring_up_sandbox`] performs the bot-startup sandbox bring-up sequence
//! (hard errors propagate as `miette::Err`; operator-fixable
//! backend-availability problems return `Ok(Err(GatewayDiagnosis))` instead of
//! crashing). [`spawn_supervisor`] then owns the long-lived monitor/recovery
//! loop and the sandbox sync task: on a verified failure it degrades the shared
//! [`SandboxRuntimeHandle`] and aborts the sync task; on recovery it re-runs
//! bring-up with backoff, respawns the sync task, and notifies affected chats.

use right_agent::agent::types::AgentConfig;
use right_openshell::diagnosis::{GatewayCause, GatewayDiagnosis, diagnose_gateway};
use right_openshell::openshell::SandboxPhaseStatus;
use right_openshell::preflight::PreflightError;
use right_openshell::sandbox_exec::SandboxExec;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::sandbox_runtime::SandboxRuntimeHandle;
use crate::{RESOLVE_HOST_IPS_BACKOFFS_MS, run_with_backoff, sync};

/// Borrowed inputs the sandbox bring-up sequence reads. All fields are
/// read-only references into `run_async`'s locals.
pub(crate) struct BringUpCtx<'a> {
    /// Agent name (used for logging and operator-facing help text).
    pub agent: &'a str,
    /// Resolved `~/.right` home dir (used to derive the ssh config dir).
    pub home: &'a Path,
    /// Per-agent directory (policy path resolution + sync source).
    pub agent_dir: &'a Path,
    /// Resolved sandbox name (always set when sandbox mode is active).
    pub resolved_sandbox: &'a str,
    /// Full parsed agent config (policy path, network policy, providers).
    pub config: &'a AgentConfig,
}

/// Successful bring-up result. `initial_sync` + `reverse_sync_md` have already
/// completed before this is returned, so the sandbox is fully Ready.
pub(crate) struct SandboxBringUp {
    /// Long-lived gRPC-backed exec handle for the sandbox.
    pub sandbox: SandboxExec,
    /// Path to the generated SSH config (consumed downstream for ssh calls and
    /// shutdown teardown).
    pub ssh_config_path: PathBuf,
}

/// Bring-up cannot continue in this process because agent.yaml was migrated
/// to a new sandbox name mid-flight: every downstream handle (ssh config
/// path, resolved sandbox, provider reconcile) is keyed on the old name.
/// The bot caller converts this into a graceful restart; the restarted
/// process re-resolves the fitted name and brings the sandbox up cleanly.
#[derive(Debug)]
pub(crate) struct SandboxNameMigrated {
    pub(crate) old: String,
    pub(crate) new: String,
}

impl std::fmt::Display for SandboxNameMigrated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "sandbox.name migrated '{}' -> '{}' in agent.yaml; restart required",
            self.old, self.new
        )
    }
}

impl std::error::Error for SandboxNameMigrated {}

/// Map an `openshell_preflight` failure to a [`GatewayDiagnosis`].
///
/// The version-too-old variants carry `found`/`required` cleanly, so they map
/// to `VersionTooOld`. Every other preflight failure (CLI missing, gateway
/// unreachable, unparseable version) is a generic backend-availability problem
/// and maps to `Unreachable`.
fn diagnose_preflight(err: PreflightError) -> GatewayDiagnosis {
    match err {
        PreflightError::CliTooOld { found, required }
        | PreflightError::GatewayTooOld { found, required } => GatewayCause::VersionTooOld {
            found: found.to_string(),
            min: required.to_string(),
        }
        .diagnose(),
        _ => GatewayCause::Unreachable.diagnose(),
    }
}

fn bring_up_phase_diagnosis(status: SandboxPhaseStatus, sandbox: &str) -> Option<GatewayDiagnosis> {
    match status {
        SandboxPhaseStatus::Ready => None,
        SandboxPhaseStatus::Error { .. } => Some(
            GatewayCause::SandboxError {
                sandbox: sandbox.to_owned(),
            }
            .diagnose(),
        ),
        SandboxPhaseStatus::NotFound => Some(
            GatewayCause::SandboxNotFound {
                sandbox: sandbox.to_owned(),
            }
            .diagnose(),
        ),
        // Non-terminal phases (PROVISIONING etc.): the gateway is reachable and
        // the sandbox exists, it just is not READY yet. Report a recoverable
        // "still starting up" diagnosis rather than the misleading Unreachable
        // ("can't reach the backend / check Docker / restart the gateway").
        SandboxPhaseStatus::Other { .. } => Some(
            GatewayCause::SandboxNotReady {
                sandbox: sandbox.to_owned(),
            }
            .diagnose(),
        ),
    }
}

fn provider_reconcile_diagnosis(sandbox: &str, detail: String) -> GatewayDiagnosis {
    GatewayCause::ProviderComposition {
        sandbox: sandbox.to_owned(),
        detail,
    }
    .diagnose()
}

fn generic_provider_profiles_for_config(
    agent_name: &str,
    config: &AgentConfig,
) -> miette::Result<Vec<right_openshell::managed_profiles::ManagedProfile>> {
    if !config.is_sandboxed() {
        return Ok(Vec::new());
    }

    let mut providers = Vec::new();
    for entry in config.providers() {
        // Borrowed (shared) records reference a profile owned & imported by the
        // owner agent; never import/own it here.
        if entry.is_borrowed() {
            continue;
        }
        match &entry.type_ {
            right_agent_config::ProviderType::Generic => {
                let generic = entry.generic.as_ref().ok_or_else(|| {
                    miette::miette!(
                        "agent {agent_name} generic provider {} is missing generic config",
                        entry.name
                    )
                })?;
                providers.push(
                    right_openshell::managed_profiles::GenericProviderProfileInput {
                        name: &entry.name,
                        upstream_hosts: &generic.upstream_hosts,
                        upstream_path_prefix: generic.upstream_path_prefix.as_deref(),
                        env_var: &generic.env_var,
                    },
                );
            }
            right_agent_config::ProviderType::BuiltIn(_) => {}
        }
    }

    Ok(right_openshell::managed_profiles::generic_provider_profiles(providers))
}

async fn ensure_generic_provider_profiles_for_config(
    client: &mut right_openshell::managed_profiles::OpenShellGrpcClient,
    agent_name: &str,
    config: &AgentConfig,
) -> miette::Result<Vec<right_openshell::managed_profiles::EnsureOutcome>> {
    let profiles = generic_provider_profiles_for_config(agent_name, config)?;
    if profiles.is_empty() {
        return Ok(Vec::new());
    }

    let outcomes = right_openshell::managed_profiles::ensure_profiles(client, &profiles)
        .await
        .map_err(|e| miette::miette!("ensure generic provider profiles failed: {e:#}"))?;
    tracing::info!(agent = %agent_name, outcomes = ?outcomes, "generic provider profiles ensured");
    Ok(outcomes)
}

/// Heal any generic profile that `ensure_*` reported as drifted, using the
/// detach-dance primitive. Built-in profiles are not healed here (handled at
/// `right up`); only generic providers declare drift in this path.
async fn heal_drifted_generic_profiles(
    client: &mut right_openshell::managed_profiles::OpenShellGrpcClient,
    sandbox_name: &str,
    agent_name: &str,
    config: &AgentConfig,
    outcomes: &[right_openshell::managed_profiles::EnsureOutcome],
) -> miette::Result<()> {
    use right_openshell::managed_profiles::EnsureOutcome;
    for entry in config.providers() {
        if entry.is_borrowed() {
            continue;
        }
        let right_agent_config::ProviderType::Generic = entry.type_ else {
            continue;
        };
        let id = right_openshell::managed_profiles::generic_provider_profile_id(&entry.name);
        let drifted = outcomes
            .iter()
            .any(|o| matches!(o, EnsureOutcome::DriftedSkipped(d) if *d == id));
        if !drifted {
            continue;
        }
        let generic = entry.generic.as_ref().ok_or_else(|| {
            miette::miette!(
                "agent {agent_name} generic provider {} missing generic config",
                entry.name
            )
        })?;
        let desired = right_openshell::managed_profiles::author_generic_profile(
            &id,
            &generic.upstream_hosts,
            generic.upstream_path_prefix.as_deref(),
            &generic.env_var,
        );
        let atts = vec![right_openshell::providers::ProfileAttachment {
            sandbox_name: sandbox_name.to_string(),
            provider_name: entry.name.clone(),
        }];
        right_openshell::providers::update_referenced_profile(client, &atts, desired)
            .await
            .map_err(|e| {
                miette::miette!("heal drifted generic profile {} failed: {e:#}", entry.name)
            })?;
        tracing::info!(agent = %agent_name, provider = %entry.name, "healed drifted generic provider profile");
    }
    Ok(())
}

enum ProviderCompositionExpectation<'a> {
    RuleOnly,
    Endpoints(Vec<(&'a str, &'a str)>),
}

fn provider_composition_expectation<'a>(
    agent_name: &str,
    entry: &'a right_agent_config::ProviderEntry,
) -> miette::Result<ProviderCompositionExpectation<'a>> {
    match &entry.type_ {
        right_agent_config::ProviderType::BuiltIn(_) => {
            Ok(ProviderCompositionExpectation::RuleOnly)
        }
        right_agent_config::ProviderType::Generic => {
            let generic = entry.generic.as_ref().ok_or_else(|| {
                miette::miette!(
                    "agent {agent_name} generic provider {} is missing generic config",
                    entry.name
                )
            })?;
            let path = generic.upstream_path_prefix.as_deref().unwrap_or("");
            Ok(ProviderCompositionExpectation::Endpoints(
                generic
                    .upstream_hosts
                    .iter()
                    .map(|host| (host.as_str(), path))
                    .collect(),
            ))
        }
    }
}

async fn wait_for_provider_entry_composed(
    client: &mut right_openshell::managed_profiles::OpenShellGrpcClient,
    sandbox_name: &str,
    agent_name: &str,
    entry: &right_agent_config::ProviderEntry,
) -> miette::Result<()> {
    match provider_composition_expectation(agent_name, entry)? {
        ProviderCompositionExpectation::RuleOnly => {
            right_openshell::openshell::wait_for_provider_composed(
                client,
                sandbox_name,
                &entry.name,
            )
            .await
        }
        ProviderCompositionExpectation::Endpoints(endpoints) => {
            let expected = endpoints
                .into_iter()
                .map(|(host, path)| (host.to_string(), path.to_string()))
                .collect();
            right_openshell::openshell::wait_for_provider_composed_with_exact_endpoints(
                client,
                sandbox_name,
                &entry.name,
                expected,
            )
            .await
        }
    }
}

fn provider_policy_reload_needed(
    declared: &[String],
    report: &right_openshell::providers::ReconcileReport,
    profile_outcomes: &[right_openshell::managed_profiles::EnsureOutcome],
) -> bool {
    // One reload refreshes composition for the whole sandbox. A providers-only
    // re-save should still retry the loaded signal even when attach state is
    // already converged, so any declared provider triggers a reload.
    !declared.is_empty()
        || !report.detached.is_empty()
        || profile_outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                right_openshell::managed_profiles::EnsureOutcome::Imported(_)
            )
        })
}

/// Reconcile attached providers against `agent.yaml` and confirm composition.
///
/// Every step here is gateway-RPC interaction — generic-profile ensure, gateway
/// attach/detach, the `policy set --wait` reload, and active-policy composition
/// polling. The caller MUST degrade any error to a recoverable
/// `Ok(Err(diagnosis))`, never a hard `Err`: a transient gateway blip or slow
/// composition on a cold gateway is self-healing, and a hard `Err` from
/// `bring_up_sandbox` lands on `recovery_step`'s `Err => Break` arm and
/// permanently stops auto-recovery (see the contract on [`bring_up_sandbox`]).
/// Split a sandbox's declared providers into `(all names, borrowed names)` for
/// `reconcile_for_sandbox`: the full declared list drives attach/detach, while
/// the borrowed set marks records the owner — not this agent — must migrate, so
/// the reconciler attaches them but never recreates/deletes them.
fn declared_and_borrowed(
    providers: &[right_agent_config::ProviderEntry],
) -> (Vec<String>, std::collections::HashSet<String>) {
    let declared = providers.iter().map(|p| p.name.clone()).collect();
    let borrowed = providers
        .iter()
        .filter(|p| p.is_borrowed())
        .map(|p| p.name.clone())
        .collect();
    (declared, borrowed)
}

async fn reconcile_and_confirm_providers(
    client: &mut right_openshell::managed_profiles::OpenShellGrpcClient,
    agent: &str,
    sandbox: &str,
    policy_path: &std::path::Path,
    config: &AgentConfig,
    sandbox_cfg: &right_agent_config::SandboxConfig,
) -> miette::Result<()> {
    let profile_outcomes =
        ensure_generic_provider_profiles_for_config(client, agent, config).await?;
    heal_drifted_generic_profiles(client, sandbox, agent, config, &profile_outcomes).await?;
    let (declared, borrowed) = declared_and_borrowed(&sandbox_cfg.providers);
    let report = right_openshell::providers::reconcile_for_sandbox(
        client, sandbox, agent, &declared, &borrowed,
    )
    .await
    .map_err(|e| miette::miette!("provider reconcile failed: {e:#}"))?;
    tracing::info!(
        agent = %agent,
        attached = ?report.attached,
        detached = ?report.detached,
        repaired = ?report.repaired,
        missing = ?report.missing,
        "provider reconcile complete"
    );
    if !report.errors.is_empty() {
        return Err(miette::miette!(
            "provider reconcile had per-provider errors: {:?}",
            report.errors
        ));
    }
    if provider_policy_reload_needed(&declared, &report, &profile_outcomes) {
        right_openshell::openshell::ensure_provider_policy_loaded(sandbox, policy_path)
            .await
            .map_err(|e| {
                miette::miette!(
                    "provider-profile composition reload failed during startup reconcile: {e:#}"
                )
            })?;
        for entry in sandbox_cfg
            .providers
            .iter()
            .filter(|entry| !report.missing.contains(&entry.name))
        {
            wait_for_provider_entry_composed(client, sandbox, agent, entry)
                .await
                .map_err(|e| {
                    miette::miette!(
                        "provider composition not confirmed during startup reconcile: {e:#}"
                    )
                })?;
        }
        tracing::info!(
            agent = %agent,
            sandbox = %sandbox,
            "provider-profile composition loaded"
        );
    }
    Ok(())
}

/// Bring the OpenShell sandbox backend up for an agent.
///
/// Returns:
/// - `Ok(Ok(SandboxBringUp))` — backend Ready; initial + reverse-mirror sync done.
/// - `Ok(Err(diagnosis))` — operator-fixable backend-availability problem.
/// - `Err(_)` — genuine, non-self-healing hard error (policy drift, fs/ssh
///   failures, sync failure).
///
/// This function does NOT spawn `run_sync_task` — the caller derives that from
/// the returned `SandboxExec`, preserving prior behavior until Tasks 7/8 move
/// the spawn into the supervisor.
pub(crate) async fn bring_up_sandbox(
    ctx: &BringUpCtx<'_>,
) -> miette::Result<Result<SandboxBringUp, GatewayDiagnosis>> {
    let agent = ctx.agent;
    let home = ctx.home;
    let agent_dir = ctx.agent_dir;
    let sandbox = ctx.resolved_sandbox.to_owned();
    let config = ctx.config;

    // Resolve policy path from agent.yaml sandbox config.
    let policy_path = config.resolve_policy_path(agent_dir)?
        .ok_or_else(|| miette::miette!(
            "sandbox mode is openshell but no policy path resolved — check sandbox.policy_file in agent.yaml"
        ))?;

    // Verify OpenShell is ready before attempting gRPC connection.
    let mtls_dir = match right_openshell::openshell::preflight_check() {
        right_openshell::openshell::OpenShellStatus::Ready(dir) => dir,
        right_openshell::openshell::OpenShellStatus::NotInstalled => {
            return Ok(Err(GatewayCause::NotInstalled.diagnose()));
        }
        right_openshell::openshell::OpenShellStatus::NoGateway(_) => {
            return Ok(Err(GatewayCause::GatewayNotStarted.diagnose()));
        }
        right_openshell::openshell::OpenShellStatus::BrokenGateway(dir) => {
            return Ok(Err(GatewayCause::BrokenCerts(dir).diagnose()));
        }
    };

    // Check if sandbox already exists and is READY.
    let mut grpc_client = match right_openshell::openshell::connect_grpc(&mtls_dir).await {
        Ok(client) => client,
        Err(_) => return Ok(Err(diagnose_gateway().await)),
    };

    // OpenShell version preflight — refuse to start on too-old CLI or
    // gateway before any further interaction. Both must be
    // >= MIN_OPENSHELL_VERSION.
    if let Err(e) = right_openshell::preflight::openshell_preflight(&mut grpc_client).await {
        tracing::error!(error = %e, "OpenShell version preflight failed");
        return Ok(Err(diagnose_preflight(e)));
    }
    tracing::info!("OpenShell version preflight passed");

    // A phase-query RPC error after a successful connect is transient/
    // inconclusive (gateway blip, status-shape skew). Degrade recoverably like
    // the connect_grpc/preflight failures above — never propagate a hard `Err`
    // here: during recovery that reaches `recovery_step`'s `Err => Break` arm
    // and permanently stops auto-recovery; at startup it crashes the bot
    // instead of starting degraded-and-recovering.
    let phase_status =
        match right_openshell::openshell::sandbox_phase_status(&mut grpc_client, &sandbox).await {
            Ok(status) => status,
            Err(e) => {
                tracing::warn!(agent = %agent, "sandbox phase query failed: {e:#}");
                return Ok(Err(diagnose_gateway().await));
            }
        };
    let phase_status = match phase_status {
        SandboxPhaseStatus::NotFound
            if let Some(right_openshell::openshell::OversizedNameAction::Migrate(fitted)) =
                right_openshell::openshell::oversized_name_action(&sandbox, false) =>
        {
            // Pre-cap name (e.g. `rightclaw-{agent}` fallback or a long
            // explicit `sandbox.name`) with no live sandbox: rewrite
            // agent.yaml to the fitted name and retry the phase query once.
            // The existing-sandbox case (KeepExisting) never reaches here —
            // reads of a live long-named sandbox are not length-validated
            // upstream, so its phase query succeeds.
            tracing::info!(
                agent = %agent,
                old = %sandbox,
                new = %fitted,
                "migrating over-long sandbox name in agent.yaml"
            );
            let yaml_path = agent_dir.join("agent.yaml");
            right_codegen::contract::write_merged_rmw(&yaml_path, |existing| {
                let existing = existing.ok_or_else(|| {
                    miette::miette!("agent.yaml missing at {}", yaml_path.display())
                })?;
                rewrite_sandbox_name_line(existing, &sandbox, &fitted).ok_or_else(|| {
                    miette::miette!(
                        "sandbox.name migration failed: no `name: {sandbox}` line in {}",
                        yaml_path.display()
                    )
                })
            })?;
            // The migration rewrote agent.yaml, but this process still holds
            // the old name everywhere downstream (ssh config path formula,
            // resolved_sandbox, provider reconcile). Continuing would trip
            // the ssh-config-path debug_assert and mis-target policy/SSH;
            // ask the caller for a graceful restart instead — the watcher
            // will also see the write, but an explicit signal is not racy.
            return Err(miette::miette!(SandboxNameMigrated {
                old: sandbox.clone(),
                new: fitted,
            }));
        }
        status => status,
    };
    if let Some(diag) = bring_up_phase_diagnosis(phase_status, &sandbox) {
        return Ok(Err(diag));
    }

    // Resolve host IPs from inside sandbox for policy allowed_ips.
    // Retry transient failures (DNS hiccup, NSS warmup race, OpenShell
    // alias rename) — startup is non-interactive and must self-heal.
    let sandbox_id =
        right_openshell::openshell::resolve_sandbox_id(&mut grpc_client, &sandbox).await?;
    let host_ips = run_with_backoff(
        "resolve_host_ips",
        agent,
        RESOLVE_HOST_IPS_BACKOFFS_MS,
        async || right_openshell::openshell::resolve_host_ips(&mut grpc_client, &sandbox_id).await,
    )
    .await?;

    // Regenerate policy with resolved host IPs and apply.
    let network_policy = config.network_policy;
    let policy_content = right_codegen::policy::generate_policy(
        right_runtime_state::MCP_HTTP_PORT,
        &network_policy,
        right_codegen::policy::HostMcpAccess::Resolved(host_ips.clone()),
    );
    // Drift check BEFORE write+apply: `openshell policy set --wait` rejects
    // landlock changes on a live sandbox with InvalidArgument, so applying
    // a drifted filesystem policy cannot safely repair the sandbox. Fail
    // startup instead of running with stale network policy and hidden MCP
    // reachability failures.
    let desired_filesystem = match right_openshell::openshell::parse_policy_yaml_filesystem(
        &policy_content,
    ) {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::warn!(agent = %agent, "could not parse generated policy.yaml for drift check: {e:#}");
            None
        }
    };
    let active_filesystem = match right_openshell::openshell::get_active_policy(
        &mut grpc_client,
        &sandbox,
    )
    .await
    {
        Ok(Some(a)) => Some(a),
        Ok(None) => {
            tracing::warn!(agent = %agent, "active policy has no payload; skipping drift check");
            None
        }
        Err(e) => {
            tracing::warn!(agent = %agent, "could not fetch active policy for drift check: {e:#}");
            None
        }
    };
    let drifted = match (active_filesystem, desired_filesystem) {
        (Some(active), Some(desired)) => {
            right_openshell::openshell::filesystem_policy_changed(&active, &desired)
        }
        _ => true,
    };

    if drifted {
        // Still write so a later `right agent config`-triggered
        // migration sees the fresh policy, then fail startup. Running with
        // stale network policy would make MCP availability nondeterministic.
        right_codegen::contract::write_regenerated(&policy_path, &policy_content)?;
        return Err(miette::miette!(
            help = format!(
                "Run `right agent config {}` (accept defaults) to trigger sandbox migration, or `right agent backup {} --sandbox-only` first if you want a recovery point.",
                agent, agent
            ),
            "Filesystem policy drift detected for '{}'. Refusing to start with stale OpenShell policy.",
            agent,
        ));
    } else {
        tracing::info!(agent = %agent, ?host_ips, "reusing existing sandbox, applying policy with resolved host IPs");
        right_codegen::contract::write_and_apply_sandbox_policy(
            &sandbox,
            &policy_path,
            &policy_content,
        )
        .await?;
    }

    // Generate SSH config.
    let ssh_config_dir = home.join("run").join("ssh");
    std::fs::create_dir_all(&ssh_config_dir)
        .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
    let config_path =
        right_openshell::openshell::generate_ssh_config(&sandbox, &ssh_config_dir).await?;

    // Clean up stale ControlMaster socket from a SIGKILL'd previous bot.
    // The next ssh call (inbox/outbox mkdir below) implicitly establishes
    // a fresh master via ControlMaster=auto in the config we just wrote.
    let cm_socket =
        right_openshell::openshell::control_master_socket_path(&ssh_config_dir, &sandbox);
    let cm_host = right_openshell::openshell::ssh_host_for_sandbox(&sandbox);
    right_openshell::openshell::clean_stale_control_master(&config_path, &cm_host, &cm_socket)
        .await?;

    tracing::info!(agent = %agent, "OpenShell sandbox ready");

    // Reconcile attached providers with the `sandbox.providers` list in agent.yaml.
    // Attaches declared providers that exist on the gateway but are not yet attached,
    // and detaches stale `<agent>-*` entries that were removed from the config.
    if let Some(sandbox_cfg) = config.sandbox.as_ref()
        && let Err(e) = reconcile_and_confirm_providers(
            &mut grpc_client,
            agent,
            &sandbox,
            &policy_path,
            config,
            sandbox_cfg,
        )
        .await
    {
        // Provider reconcile + composition is all gateway-RPC work: a transient
        // failure or slow composition on a cold gateway is recoverable, not a
        // hard config error. Degrade like the phase/drift checks above so
        // `recovery_step` retries with backoff instead of hitting its
        // `Err => Break` arm and permanently stopping auto-recovery.
        tracing::warn!(
            agent = %agent,
            "provider reconcile/composition not confirmed; staying degraded and retrying: {e:#}"
        );
        return Ok(Err(provider_reconcile_diagnosis(
            &sandbox,
            format!("{e:#}"),
        )));
    }

    // Create inbox/outbox inside sandbox for attachment handling.
    // This is also the first ssh -F <config> call, which establishes the
    // ControlMaster (see clean_stale_control_master above + SSH config
    // appended directives in generate_ssh_config).
    let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(&sandbox);
    right_openshell::openshell::ssh_exec(
        &config_path,
        &ssh_host,
        &["mkdir", "-p", "/sandbox/inbox", "/sandbox/outbox"],
        10,
    )
    .await
    .map_err(|e| miette::miette!("failed to create sandbox attachment dirs: {e:#}"))?;

    // Sync config files to sandbox before considering it Ready. Blocks until
    // first sync completes — ensures sandbox has correct .claude.json,
    // settings.json, etc. before any claude -p invocations.
    let sbox = SandboxExec::new(mtls_dir.clone(), sandbox.clone(), sandbox_id.clone());
    sync::initial_sync(agent_dir, &sbox).await?;
    if let Err(e) = sync::reverse_sync_md(agent_dir, sbox.sandbox_name()).await {
        tracing::warn!(
            agent = %agent,
            sandbox = %sbox.sandbox_name(),
            "startup identity mirror sync failed: {e:#}"
        );
    }

    Ok(Ok(SandboxBringUp {
        sandbox: sbox,
        ssh_config_path: config_path,
    }))
}

/// Hot-apply a `sandbox.providers` change to a live sandbox without a restart.
///
/// Ensures generic provider profiles, reconciles gateway attach/detach, then
/// reloads OpenShell's provider-profile composition with `openshell policy set
/// --wait`. Used by the config-watcher providers hot path; on failure the
/// lib.rs consumer retries with backoff. There is no periodic provider
/// reconcile, so persistent failure leaves live sandbox attachment/composition
/// state stale until the next bot restart.
pub(crate) async fn hot_reconcile_providers(
    agent: &str,
    agent_dir: &std::path::Path,
    resolved_sandbox: &str,
    config: &AgentConfig,
) -> miette::Result<()> {
    // `resolve_policy_path` yields None only for `mode: none`, which the lib.rs
    // consumer already pre-filters; this guard covers any direct caller.
    let policy_path = config
        .resolve_policy_path(agent_dir)?
        .ok_or_else(|| miette::miette!(
            "sandbox mode is openshell but no policy path resolved — check sandbox.policy_file in agent.yaml"
        ))?;

    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir).await?;

    let providers = config.providers();
    let profile_outcomes =
        ensure_generic_provider_profiles_for_config(&mut client, agent, config).await?;
    heal_drifted_generic_profiles(
        &mut client,
        resolved_sandbox,
        agent,
        config,
        &profile_outcomes,
    )
    .await?;

    let (declared, borrowed) = declared_and_borrowed(providers);
    let report = right_openshell::providers::reconcile_for_sandbox(
        &mut client,
        resolved_sandbox,
        agent,
        &declared,
        &borrowed,
    )
    .await
    .map_err(|e| miette::miette!("provider reconcile failed: {e:#}"))?;
    tracing::info!(
        agent = %agent,
        attached = ?report.attached,
        detached = ?report.detached,
        repaired = ?report.repaired,
        missing = ?report.missing,
        profile_outcomes = ?profile_outcomes,
        "providers hot-reconcile complete"
    );
    // Mirror `bring_up_sandbox`: per-provider attach/detach errors are reported
    // in `report.errors` (the call itself returned Ok). Surface them — the
    // reconcile is "complete" but some providers may not match declared state.
    if !report.errors.is_empty() {
        return Err(miette::miette!(
            "providers hot-reconcile had per-provider errors: {:?}",
            report.errors
        ));
    }
    if provider_policy_reload_needed(&declared, &report, &profile_outcomes) {
        right_openshell::openshell::ensure_provider_policy_loaded(resolved_sandbox, &policy_path)
            .await
            .map_err(|e| miette::miette!("provider-profile composition reload failed: {e:#}"))?;
        for entry in providers
            .iter()
            .filter(|entry| !report.missing.contains(&entry.name))
        {
            wait_for_provider_entry_composed(&mut client, resolved_sandbox, agent, entry)
                .await
                .map_err(|e| miette::miette!("provider composition not confirmed: {e:#}"))?;
        }
    }
    Ok(())
}

/// Recovery backoff schedule, in seconds. Consecutive failed bring-up attempts
/// advance through this table; the last value repeats indefinitely. A success
/// resets the counter to 0.
const RECOVERY_BACKOFF: &[u64] = &[5, 10, 15, 15, 30];

/// Owned inputs the supervisor needs to rebuild a [`BringUpCtx`] on every
/// recovery attempt, plus the sync inputs and the shutdown token.
///
/// `BringUpCtx` borrows from `run_async`'s locals; the supervisor outlives that
/// scope, so it owns the data here and hands out fresh borrows via
/// [`SupervisorDeps::bring_up_ctx`].
pub(crate) struct SupervisorDeps {
    /// Agent name (logging + operator-facing help text).
    pub agent: String,
    /// Resolved `~/.right` home dir (ssh config dir derivation).
    pub home: PathBuf,
    /// Per-agent directory (policy path resolution + sync source).
    pub agent_dir: PathBuf,
    /// Resolved sandbox name.
    pub resolved_sandbox: String,
    /// Full parsed agent config.
    pub config: AgentConfig,
    /// Shutdown token shared with the rest of the bot.
    pub shutdown: CancellationToken,
}

impl SupervisorDeps {
    /// Construct deps from owned values.
    pub(crate) fn new(
        agent: String,
        home: PathBuf,
        agent_dir: PathBuf,
        resolved_sandbox: String,
        config: AgentConfig,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            agent,
            home,
            agent_dir,
            resolved_sandbox,
            config,
            shutdown,
        }
    }

    /// Borrow self's fields into a fresh `BringUpCtx` for one recovery attempt.
    fn bring_up_ctx(&self) -> BringUpCtx<'_> {
        BringUpCtx {
            agent: &self.agent,
            home: &self.home,
            agent_dir: &self.agent_dir,
            resolved_sandbox: &self.resolved_sandbox,
            config: &self.config,
        }
    }
}

/// Spawn the long-lived sync task for a freshly-Ready sandbox. Relocated from
/// the former inline `tokio::spawn(sync::run_sync_task(...))` in `lib.rs`.
fn spawn_sync_task(
    deps: &SupervisorDeps,
    handle: &Arc<SandboxRuntimeHandle>,
    sandbox: SandboxExec,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(sync::run_sync_task(
        deps.agent_dir.clone(),
        sandbox,
        Some(Arc::clone(handle)),
        deps.shutdown.clone(),
    ))
}

enum ProbeOutcome {
    Ready,
    Error { detail: String },
    Other { phase: String, detail: String },
    GatewayDiagnosis(GatewayDiagnosis),
}

fn degrade_decision(outcome: ProbeOutcome, sandbox: &str) -> Option<GatewayDiagnosis> {
    match outcome {
        ProbeOutcome::Ready => None,
        ProbeOutcome::Other { phase, detail } => {
            tracing::debug!(%phase, %detail, "sandbox in transient phase; not degrading");
            None
        }
        ProbeOutcome::Error { detail } => {
            tracing::warn!(%detail, "sandbox is in ERROR phase");
            Some(
                GatewayCause::SandboxError {
                    sandbox: sandbox.to_owned(),
                }
                .diagnose(),
            )
        }
        ProbeOutcome::GatewayDiagnosis(diag) => Some(diag),
    }
}

async fn probe_phase(sandbox: &str) -> ProbeOutcome {
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = match right_openshell::openshell::connect_grpc(&mtls_dir).await {
        Ok(c) => c,
        Err(_) => return ProbeOutcome::GatewayDiagnosis(diagnose_gateway().await),
    };
    match right_openshell::openshell::sandbox_phase_status(&mut client, sandbox).await {
        Ok(SandboxPhaseStatus::Ready) => ProbeOutcome::Ready,
        Ok(SandboxPhaseStatus::Error { detail }) => ProbeOutcome::Error { detail },
        Ok(SandboxPhaseStatus::Other { phase, detail }) => ProbeOutcome::Other { phase, detail },
        Ok(SandboxPhaseStatus::NotFound) => ProbeOutcome::GatewayDiagnosis(
            GatewayCause::SandboxNotFound {
                sandbox: sandbox.to_owned(),
            }
            .diagnose(),
        ),
        Err(e) => {
            tracing::warn!("sandbox phase probe failed: {e:#}");
            ProbeOutcome::GatewayDiagnosis(diagnose_gateway().await)
        }
    }
}

/// Control-flow signal returned by a single supervisor loop iteration.
enum LoopStep {
    /// Keep looping.
    Continue,
    /// Exit the supervisor (shutdown, channel closed, or hard config error).
    Break,
}

/// One iteration of the monitor branch (handle is Ready). Waits for a failure
/// report or shutdown; on a *verified* failure, degrades the handle.
async fn monitor_step(
    handle: &Arc<SandboxRuntimeHandle>,
    failure_rx: &mut mpsc::Receiver<()>,
    sync_task: &mut Option<tokio::task::JoinHandle<()>>,
    deps: &SupervisorDeps,
) -> LoopStep {
    tokio::select! {
        _ = deps.shutdown.cancelled() => LoopStep::Break,
        msg = failure_rx.recv() => {
            if msg.is_none() {
                return LoopStep::Break;
            }
            let Some(diag) =
                degrade_decision(probe_phase(&deps.resolved_sandbox).await, &deps.resolved_sandbox)
            else {
                return LoopStep::Continue;
            };
            tracing::error!(agent = %deps.agent, cause = ?diag.cause, "{}", diag.summary);
            handle.set_unavailable(Arc::new(diag));
            if let Some(t) = sync_task.take() {
                t.abort();
            }
            LoopStep::Continue
        }
    }
}

/// One iteration of the recovery branch (handle is Unavailable). Attempts
/// bring-up; on success spawns the sync task, notifies affected chats, and
/// re-enters monitor mode. On a recoverable diagnosis it sleeps with backoff;
/// on a hard error it breaks (stay degraded).
async fn recovery_step(
    handle: &Arc<SandboxRuntimeHandle>,
    bot: &crate::telegram::BotType,
    sync_task: &mut Option<tokio::task::JoinHandle<()>>,
    deps: &SupervisorDeps,
    attempt: &mut usize,
) -> LoopStep {
    let ctx = deps.bring_up_ctx();
    match bring_up_sandbox(&ctx).await {
        Ok(Ok(bring_up)) => {
            handle.set_ready(bring_up.sandbox.clone());
            *sync_task = Some(spawn_sync_task(deps, handle, bring_up.sandbox));
            notify_back_online(handle, bot).await;
            *attempt = 0;
            tracing::info!(agent = %deps.agent, "sandbox backend recovered");
            // Skip the backoff sleep: re-enter the monitor branch immediately
            // now that the handle is Ready.
            return LoopStep::Continue;
        }
        Ok(Err(diag)) => {
            // Surface the diagnosis on every recovery failure. Without this the
            // loop retries silently and a persistent bring-up failure (e.g. a
            // proto/phase mismatch that decodes a Ready sandbox as not-ready)
            // is invisible — only the per-iteration "preflight passed" INFO
            // shows, with no hint why recovery never completes.
            tracing::warn!(
                agent = %deps.agent,
                cause = ?diag.cause,
                attempt = *attempt,
                "sandbox bring-up failed; staying degraded and retrying: {}",
                diag.summary
            );
            handle.set_unavailable(Arc::new(diag));
        }
        Err(e) => {
            // A hard config error during recovery cannot self-heal: stay
            // degraded and stop retrying.
            tracing::error!(agent = %deps.agent, "unrecoverable sandbox error: {e:#}");
            return LoopStep::Break;
        }
    }
    // Current backoff index, then advance for the next consecutive failure.
    let secs = RECOVERY_BACKOFF[(*attempt).min(RECOVERY_BACKOFF.len() - 1)];
    *attempt += 1;
    tokio::select! {
        _ = deps.shutdown.cancelled() => LoopStep::Break,
        _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => LoopStep::Continue,
    }
}

/// Drive the supervisor loop to completion. `!Send` because `bring_up_sandbox`
/// holds an `AsyncFnMut` retry closure (over `&mut grpc_client`) across awaits,
/// which makes its future non-`Send`-general. Runs on a `LocalSet` (see
/// [`spawn_supervisor`]).
async fn run_supervisor(
    handle: Arc<SandboxRuntimeHandle>,
    mut failure_rx: mpsc::Receiver<()>,
    bot: crate::telegram::BotType,
    deps: SupervisorDeps,
    initial_sync_task: Option<tokio::task::JoinHandle<()>>,
) {
    // Seed with the startup sync task so the supervisor owns it: a degrade
    // aborts it (see monitor_step) and recovery replaces it, preventing a
    // duplicate sync task from running after the first recovery.
    let mut sync_task: Option<tokio::task::JoinHandle<()>> = initial_sync_task;
    // Consecutive-failure counter driving the backoff schedule. Loop-local:
    // the supervisor runs as a single task, so no synchronization is needed.
    let mut attempt: usize = 0;
    loop {
        let step = if handle.is_ready() {
            monitor_step(&handle, &mut failure_rx, &mut sync_task, &deps).await
        } else {
            recovery_step(&handle, &bot, &mut sync_task, &deps, &mut attempt).await
        };
        if let LoopStep::Break = step {
            // Abort the owned sync task on shutdown so its JoinHandle is not
            // detached-leaked when the supervisor exits.
            if let Some(t) = sync_task.take() {
                t.abort();
            }
            break;
        }
    }
}

/// Owns sandbox lifecycle. Runs for the bot's life. When `Unavailable`, retries
/// `bring_up_sandbox` with backoff. When `Ready`, sleeps until a verified
/// failure report flips it back. On every Unavailable→Ready transition, spawns
/// the sync task and notifies affected chats.
///
/// The loop future is `!Send` (see [`run_supervisor`]), so it is driven via a
/// `LocalSet` on a dedicated `spawn_blocking` thread — the same pattern
/// `run_async` uses for the `!Send` Hindsight drain loop. Async upstream calls
/// (gRPC, sync) still run on the shared runtime through the captured `Handle`.
pub(crate) fn spawn_supervisor(
    handle: Arc<SandboxRuntimeHandle>,
    failure_rx: mpsc::Receiver<()>,
    bot: crate::telegram::BotType,
    deps: SupervisorDeps,
    initial_sync_task: Option<tokio::task::JoinHandle<()>>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Handle::current();
        let local = tokio::task::LocalSet::new();
        rt.block_on(local.run_until(run_supervisor(
            handle,
            failure_rx,
            bot,
            deps,
            initial_sync_task,
        )));
    })
}

/// Send the back-online notice to every chat that hit the degraded backend.
/// Best-effort per chat: a failed send is logged but does not abort the loop.
async fn notify_back_online(handle: &Arc<SandboxRuntimeHandle>, bot: &crate::telegram::BotType) {
    let message = crate::sandbox_copy::back_online_message();
    for (chat, thread) in handle.take_affected() {
        if let Err(e) = crate::telegram::worker::send_tg(bot, chat, thread, &message).await {
            tracing::warn!(
                chat = %chat,
                "back-online notice send failed: {e:#}"
            );
        }
    }
}

#[cfg(test)]
#[path = "sandbox_supervisor_phase_tests.rs"]
mod phase_tests;

/// Rewrite the `sandbox.name` line in agent.yaml content from `old` to
/// `fitted`: line-anchored (no substring prefix corruption, no provider-entry
/// `name:` lines), tolerant of single/double-quoted legacy values (rewritten
/// unquoted — both parse identically). Returns `None` when no exact line
/// matches.
fn rewrite_sandbox_name_line(existing: &str, old: &str, fitted: &str) -> Option<String> {
    let mut lines: Vec<String> = existing.lines().map(str::to_owned).collect();
    let mut replaced = false;
    for line in &mut lines {
        let trimmed = line.trim_start();
        for quote in ["", "\"", "'"] {
            if trimmed == format!("name: {quote}{old}{quote}") {
                let indent = &line[..line.len() - trimmed.len()];
                *line = format!("{indent}name: {fitted}");
                replaced = true;
                break;
            }
        }
        if replaced {
            break;
        }
    }
    if !replaced {
        return None;
    }
    let mut out = lines.join("\n");
    if existing.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_profile_inputs_skip_builtins_and_author_generic_profiles() {
        let config = AgentConfig {
            sandbox: Some(right_agent_config::SandboxConfig {
                providers: vec![
                    right_agent_config::ProviderEntry {
                        name: "right-gh".into(),
                        type_: right_agent_config::ProviderType::BuiltIn("right-github".into()),
                        label: None,
                        generic: None,
                        shared_from: None,
                    },
                    right_agent_config::ProviderEntry {
                        name: "right-acme".into(),
                        type_: right_agent_config::ProviderType::Generic,
                        label: None,
                        generic: Some(right_agent_config::GenericProvider {
                            env_var: "ACME_TOKEN".into(),
                            upstream_hosts: vec!["api.acme.test".into(), "queue.acme.test".into()],
                            upstream_path_prefix: Some("/v1".into()),
                        }),
                        shared_from: None,
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        };

        let profiles = generic_provider_profiles_for_config("right", &config).unwrap();

        assert_eq!(profiles.len(), 1);
        assert_eq!(
            profiles[0].id(),
            right_openshell::managed_profiles::generic_provider_profile_id("right-acme")
        );
    }

    #[test]
    fn generic_profile_inputs_fail_on_missing_generic_block() {
        let config = AgentConfig {
            sandbox: Some(right_agent_config::SandboxConfig {
                providers: vec![right_agent_config::ProviderEntry {
                    name: "right-acme".into(),
                    type_: right_agent_config::ProviderType::Generic,
                    label: None,
                    generic: None,
                    shared_from: None,
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        let err = generic_provider_profiles_for_config("right", &config).unwrap_err();

        assert!(
            format!("{err:#}").contains("generic provider right-acme is missing generic config")
        );
    }

    #[test]
    fn provider_composition_expectation_uses_all_endpoints_for_generic() {
        let entry = right_agent_config::ProviderEntry {
            name: "right-acme".into(),
            type_: right_agent_config::ProviderType::Generic,
            label: None,
            generic: Some(right_agent_config::GenericProvider {
                env_var: "ACME_TOKEN".into(),
                upstream_hosts: vec!["api.acme.test".into(), "queue.acme.test".into()],
                upstream_path_prefix: Some("/v1".into()),
            }),
            shared_from: None,
        };

        match provider_composition_expectation("right", &entry).unwrap() {
            ProviderCompositionExpectation::Endpoints(endpoints) => {
                assert_eq!(
                    endpoints,
                    vec![("api.acme.test", "/v1"), ("queue.acme.test", "/v1")]
                );
            }
            ProviderCompositionExpectation::RuleOnly => {
                panic!("generic provider must use endpoint-aware composition")
            }
        }
    }

    #[test]
    fn provider_composition_expectation_uses_rule_only_for_builtins() {
        let entry = right_agent_config::ProviderEntry {
            name: "right-gh".into(),
            type_: right_agent_config::ProviderType::BuiltIn("right-github".into()),
            label: None,
            generic: None,
            shared_from: None,
        };

        assert!(matches!(
            provider_composition_expectation("right", &entry).unwrap(),
            ProviderCompositionExpectation::RuleOnly
        ));
    }

    #[test]
    fn provider_policy_reload_needed_when_declared_or_detached() {
        let unchanged = Vec::new();
        let empty = right_openshell::providers::ReconcileReport {
            attached: Vec::new(),
            detached: Vec::new(),
            repaired: vec![],
            missing: Vec::new(),
            errors: Vec::new(),
        };
        assert!(!provider_policy_reload_needed(&[], &empty, &unchanged));

        assert!(provider_policy_reload_needed(
            &[String::from("right-gh")],
            &empty,
            &unchanged
        ));

        let detached = right_openshell::providers::ReconcileReport {
            detached: vec![String::from("right-gh")],
            repaired: vec![],
            ..empty
        };
        assert!(provider_policy_reload_needed(&[], &detached, &unchanged));
    }

    #[test]
    fn generic_profiles_skip_borrowed_entries() {
        // owned generic provider + borrowed generic provider (shared_from set)
        // only the owned one must appear in the returned profile list.
        let config = AgentConfig {
            sandbox: Some(right_agent_config::SandboxConfig {
                providers: vec![
                    right_agent_config::ProviderEntry {
                        name: "acme-aaaaaa".into(),
                        type_: right_agent_config::ProviderType::Generic,
                        label: None,
                        generic: Some(right_agent_config::GenericProvider {
                            env_var: "ACME_KEY".into(),
                            upstream_hosts: vec!["api.acme.com".into()],
                            upstream_path_prefix: None,
                        }),
                        shared_from: None,
                    },
                    right_agent_config::ProviderEntry {
                        name: "fal-bbbbbb".into(),
                        type_: right_agent_config::ProviderType::Generic,
                        label: None,
                        generic: Some(right_agent_config::GenericProvider {
                            env_var: "FAL_KEY".into(),
                            upstream_hosts: vec!["fal.run".into()],
                            upstream_path_prefix: None,
                        }),
                        shared_from: Some("agent-a".into()),
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        };

        let profiles = generic_provider_profiles_for_config("borrower", &config).unwrap();
        let owned_id =
            right_openshell::managed_profiles::generic_provider_profile_id("acme-aaaaaa");
        let borrowed_id =
            right_openshell::managed_profiles::generic_provider_profile_id("fal-bbbbbb");

        assert!(
            profiles.iter().any(|p| p.id() == owned_id),
            "owned profile must be ensured; got: {profiles:?}"
        );
        assert!(
            !profiles.iter().any(|p| p.id() == borrowed_id),
            "borrowed profile must NOT be ensured; got: {profiles:?}"
        );
    }

    #[test]
    fn rewrite_sandbox_name_line_rewrites_plain_and_quoted() {
        let yaml = "sandbox:\n  mode: openshell\n  name: rightclaw-brain-2026\n";
        let out = rewrite_sandbox_name_line(yaml, "rightclaw-brain-2026", "right-x").unwrap();
        assert!(out.contains("  name: right-x\n"), "{out}");

        for quote in ["'", "\""] {
            let yaml = format!("sandbox:\n  name: {quote}rightclaw-brain-2026{quote}\n");
            let out = rewrite_sandbox_name_line(&yaml, "rightclaw-brain-2026", "right-x").unwrap();
            assert!(out.contains("name: right-x"), "{out}");
        }
    }

    #[test]
    fn rewrite_sandbox_name_line_rejects_prefix_collision_and_missing() {
        // `name: right-foo` must NOT match a line carrying `right-foo-bar`.
        let yaml = "sandbox:\n  name: right-foo-bar\n";
        assert!(rewrite_sandbox_name_line(yaml, "right-foo", "right-x").is_none());
        // A provider entry with the same value is a different line shape
        // (`- name:`) and must not be rewritten.
        let yaml = "providers:\n  - name: right-foo\n";
        assert!(rewrite_sandbox_name_line(yaml, "right-foo", "right-x").is_none());
        // Missing name entirely.
        assert!(
            rewrite_sandbox_name_line("sandbox:\n  mode: openshell\n", "right-foo", "right-x")
                .is_none()
        );
    }

    #[test]
    fn rewrite_sandbox_name_line_preserves_indent_and_trailing_newline() {
        let yaml = "sandbox:\n    name: right-foo-old\nmodel: opus\n";
        let out = rewrite_sandbox_name_line(yaml, "right-foo-old", "right-foo").unwrap();
        assert_eq!(out, "sandbox:\n    name: right-foo\nmodel: opus\n");
        // No trailing newline preserved as none.
        let out =
            rewrite_sandbox_name_line("name: right-foo-old", "right-foo-old", "right-foo").unwrap();
        assert_eq!(out, "name: right-foo");
    }
}
