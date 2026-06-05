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
    let policy_content = right_codegen::policy::generate_provider_aware_policy(
        right_runtime_state::MCP_HTTP_PORT,
        &network_policy,
        right_codegen::policy::HostMcpAccess::Resolved(host_ips.clone()),
        config.providers(),
    )
    .map_err(|e| miette::miette!("provider policy fold failed: {e:#}"))?;
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

/// Hot-apply a `sandbox.providers` change to a live sandbox without a restart.
///
/// Re-renders the provider-aware policy (network-only, via
/// `openshell policy set --wait`) and reconciles gateway attach/detach. Used by
/// the config-watcher providers hot path; on failure the lib.rs consumer retries
/// with backoff. There is no periodic provider reconcile, so persistent failure
/// leaves the live sandbox policy stale until the next bot restart — the on-disk
/// policy is already correct because every full regen folds providers back in.
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
    let sandbox_id =
        right_openshell::openshell::resolve_sandbox_id(&mut client, resolved_sandbox).await?;
    let host_ips = right_openshell::openshell::resolve_host_ips(&mut client, &sandbox_id).await?;

    let providers = config.providers();
    let policy_content = right_codegen::policy::generate_provider_aware_policy(
        right_runtime_state::MCP_HTTP_PORT,
        &config.network_policy,
        right_codegen::policy::HostMcpAccess::Resolved(host_ips),
        providers,
    )
    .map_err(|e| miette::miette!("provider policy fold failed: {e:#}"))?;

    right_codegen::contract::write_and_apply_sandbox_policy(
        resolved_sandbox,
        &policy_path,
        &policy_content,
    )
    .await?;

    let declared: Vec<String> = providers.iter().map(|p| p.name.clone()).collect();
    let report = right_openshell::providers::reconcile_for_sandbox(
        &mut client,
        resolved_sandbox,
        agent,
        &declared,
    )
    .await
    .map_err(|e| miette::miette!("provider reconcile failed: {e:#}"))?;
    tracing::info!(
        agent = %agent,
        attached = ?report.attached,
        detached = ?report.detached,
        missing = ?report.missing,
        "providers hot-reconcile complete"
    );
    // Mirror `bring_up_sandbox`: per-provider attach/detach errors are reported
    // in `report.errors` (the call itself returned Ok). Surface them — the
    // reconcile is "complete" but some providers may not match declared state.
    if !report.errors.is_empty() {
        tracing::warn!(
            agent = %agent,
            errors = ?report.errors,
            "providers hot-reconcile had per-provider errors; re-edit sandbox.providers or restart to retry"
        );
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
fn spawn_sync_task(deps: &SupervisorDeps, sandbox: SandboxExec) -> tokio::task::JoinHandle<()> {
    tokio::spawn(sync::run_sync_task(
        deps.agent_dir.clone(),
        sandbox,
        deps.shutdown.clone(),
    ))
}

/// Direct reachability probe: a successful gRPC channel connect to the OpenShell
/// gateway. Used to verify a failure report before degrading — transient worker
/// errors should not flip a healthy backend to Unavailable.
async fn probe_reachable() -> bool {
    right_openshell::openshell::connect_grpc(&right_openshell::openshell::default_mtls_dir())
        .await
        .is_ok()
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
            // Verify with a single direct probe before degrading.
            if probe_reachable().await {
                return LoopStep::Continue;
            }
            let diag = diagnose_gateway().await;
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
            *sync_task = Some(spawn_sync_task(deps, bring_up.sandbox));
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
