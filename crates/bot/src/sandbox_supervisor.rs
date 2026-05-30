//! Owns the sandbox-backend lifecycle: first bring-up, degrade, recovery.
//!
//! Task 6 extracts the bot-startup sandbox bring-up sequence (previously
//! inline in `lib.rs::run_async`) into [`bring_up_sandbox`]. The function is
//! behavior-preserving: hard errors still propagate as `miette::Err`, and the
//! only new shape is that operator-fixable backend-availability problems are
//! returned as `Ok(Err(GatewayDiagnosis))` instead of crashing. The temporary
//! `lib.rs` shim turns every diagnosis back into a hard error for now; Tasks
//! 7/8 will route diagnoses into graceful degrade and own `run_sync_task`
//! spawning.

use right_agent::agent::types::AgentConfig;
use right_openshell::diagnosis::{GatewayCause, GatewayDiagnosis, diagnose_gateway};
use right_openshell::preflight::PreflightError;
use right_openshell::sandbox_exec::SandboxExec;
use std::path::{Path, PathBuf};

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

    let sandbox_exists =
        right_openshell::openshell::is_sandbox_ready(&mut grpc_client, &sandbox).await?;

    if !sandbox_exists {
        return Ok(Err(GatewayCause::SandboxNotFound {
            sandbox: sandbox.clone(),
        }
        .diagnose()));
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
    // Non-fatal: a reconcile failure is logged but never prevents the bot from starting.
    if let Some(sandbox_cfg) = config.sandbox.as_ref() {
        let mtls_dir = right_openshell::openshell::default_mtls_dir();
        match right_openshell::openshell::connect_grpc(&mtls_dir).await {
            Ok(mut client) => {
                let declared: Vec<String> = sandbox_cfg
                    .providers
                    .iter()
                    .map(|p| p.name.clone())
                    .collect();
                match right_openshell::providers::reconcile_for_sandbox(
                    &mut client,
                    &sandbox,
                    agent,
                    &declared,
                )
                .await
                {
                    Ok(report) => {
                        tracing::info!(
                            agent = %agent,
                            attached = ?report.attached,
                            detached = ?report.detached,
                            missing = ?report.missing,
                            "provider reconcile complete"
                        );
                        if !report.errors.is_empty() {
                            tracing::warn!(
                                agent = %agent,
                                errors = ?report.errors,
                                "provider reconcile had per-provider errors; will retry on next pass"
                            );
                        }
                    }
                    Err(e) => tracing::warn!(
                        agent = %agent,
                        "provider reconcile failed: {e:#}"
                    ),
                }
            }
            Err(e) => tracing::warn!(
                agent = %agent,
                "could not connect to openshell gateway for provider reconcile: {e:#}"
            ),
        }
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
