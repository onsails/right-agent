//! Owns the sandbox lifecycle: first bring-up, degrade, recovery.
//!
//! [`bring_up_sandbox`] performs the bot-startup sequence (hard errors
//! propagate as `miette::Err`; operator-fixable availability problems return
//! `Ok(Err(SandboxDiagnosis))` instead of crashing). [`spawn_supervisor`] then
//! owns the long-lived monitor/recovery loop and the sandbox sync task: on a
//! verified failure it degrades the shared [`SandboxRuntimeHandle`] and aborts
//! the sync task; on recovery it re-runs bring-up with backoff, respawns the
//! sync task, and notifies affected chats.
//!
//! The supervisor is the sole writer of [`SandboxRuntimeHandle`]: bring-up's
//! outcome is seeded into the handle at construction
//! ([`SandboxRuntimeHandle::new`]), and every later publication —
//! `set_ready` on recovery, `set_unavailable` on a verified failure — happens
//! here. Consumers only read, and read per unit of work, because recovery
//! publishes a *new* handle addressing a newly created VM.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use right_agent::agent::types::AgentConfig;
use right_providers::{ProviderStatus, ProviderStore};
use right_sandbox::{
    DEFAULT_READY_TIMEOUT, SandboxCause, SandboxDiagnosis, SandboxError, SandboxHandle,
    SandboxPhase, SandboxSpec, agent_sandbox_spec,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::sandbox::{SANDBOX_INBOX, SANDBOX_OUTBOX, Sandbox};
use crate::sandbox_runtime::SandboxRuntimeHandle;
use crate::sync;

/// Borrowed inputs the sandbox bring-up sequence reads. All fields are
/// read-only references into `run_async`'s locals.
pub(crate) struct BringUpCtx<'a> {
    /// Agent name (logging, operator-facing help text, provider ownership).
    pub agent: &'a str,
    /// Per-agent directory (sync source).
    pub agent_dir: &'a Path,
    /// Deterministic sandbox name for this agent.
    pub sandbox_name: &'a str,
    /// Full parsed agent config (egress mode, declared providers).
    pub config: &'a AgentConfig,
    /// Provider credential store; the only reader of stored credentials.
    pub providers: &'a ProviderStore,
}

/// Successful bring-up. `initial_sync` + `reverse_sync_md` have already
/// completed, so the sandbox is fully Ready.
pub(crate) struct SandboxBringUp {
    /// The live sandbox handle every consumer shares.
    pub sandbox: Sandbox,
}

/// Classify a [`SandboxError`] into the operator-facing diagnosis.
///
/// Errors that say nothing about backend health (invalid spec, guest command
/// failure) fall back to `Unreachable`, matching the old gateway taxonomy's
/// "inconclusive" bucket.
fn diagnose(error: &SandboxError) -> SandboxDiagnosis {
    error
        .cause()
        .unwrap_or(SandboxCause::Unreachable)
        .diagnose()
}

/// Resolve every declared provider into a source-ref secret binding.
///
/// Reading a binding publishes the credential into this process's environment
/// under the binding's source variable; the value itself never enters the
/// binding, the sandbox config, or a log line.
///
/// The two ways a declared provider can fail to bind are deliberately *not*
/// the same thing:
///
/// - [`ProviderStatus::NeedsValue`] — the record exists and carries no
///   credential yet, which is exactly the state
///   `right agent migrate-sandbox` leaves a migrated provider in (OpenShell
///   redacts credentials, so they are re-entered once from the dashboard).
///   The binding is skipped and the operator is warned by name. Nothing
///   pretends the provider is authenticated: with no binding there is no
///   placeholder to substitute, so the agent's calls fail as the
///   unauthenticated calls they are, and `/providers` shows the record
///   awaiting its value.
/// - the record is absent entirely — `agent.yaml` and `providers.db`
///   disagree about what this agent holds. That is a config/store
///   inconsistency no operator action is pending on, so it stays a hard
///   error.
async fn secret_bindings(
    agent: &str,
    config: &AgentConfig,
    providers: &ProviderStore,
) -> miette::Result<Vec<right_sandbox::SecretBinding>> {
    let mut bindings = Vec::new();
    for entry in config.providers() {
        // Read the record first: only its status distinguishes "awaiting a
        // credential" from "not there at all", and `source_ref_binding`
        // rejects both.
        let record = providers.get(agent, &entry.name).await.map_err(|e| {
            miette::miette!(
                "provider '{}' declared by agent {agent} cannot be bound: {e:#}",
                entry.name
            )
        })?;
        if record.status == ProviderStatus::NeedsValue {
            tracing::warn!(
                agent = %agent,
                provider = %entry.name,
                env_var = %record.env_var,
                "provider holds no credential yet, so the sandbox starts without it; \
                 add its credential from the dashboard: /providers"
            );
            continue;
        }
        let binding = providers
            .source_ref_binding(agent, &entry.name)
            .await
            .map_err(|e| {
                miette::miette!(
                    "provider '{}' declared by agent {agent} cannot be bound: {e:#}",
                    entry.name
                )
            })?;
        bindings.push(binding);
    }
    Ok(bindings)
}

/// Build an agent's create-time sandbox specification.
///
/// Resolving the agent's declared providers is the only part that needs the
/// bot's store; every other field comes from the shared
/// [`right_sandbox::agent_sandbox_spec`]. The bot's bring-up, the CLI's
/// `agent restore`, and `right agent migrate-sandbox` are the only sandbox
/// creators and all three go through here, so a restored or migrated sandbox
/// is identical to a bot-created one — egress and secret structure are
/// create-time only, so drift there is unrecoverable without a recreate.
pub async fn agent_sandbox_spec_for(
    agent: &str,
    sandbox_name: &str,
    config: &AgentConfig,
    providers: &ProviderStore,
) -> miette::Result<SandboxSpec> {
    let secrets = secret_bindings(agent, config, providers).await?;
    agent_sandbox_spec(sandbox_name, config.network_policy, secrets)
        .map_err(|e| miette::miette!("invalid sandbox spec for agent {agent}: {e:#}"))
}

/// The bring-up sequence's view of [`agent_sandbox_spec_for`].
async fn sandbox_spec(ctx: &BringUpCtx<'_>) -> miette::Result<SandboxSpec> {
    agent_sandbox_spec_for(ctx.agent, ctx.sandbox_name, ctx.config, ctx.providers).await
}

/// Bring the Agent Sandbox up.
///
/// Returns:
/// - `Ok(Ok(SandboxBringUp))` — Ready; initial + reverse-mirror sync done.
/// - `Ok(Err(diagnosis))` — operator-fixable availability problem. The caller
///   MUST degrade to this rather than a hard `Err` for anything self-healing:
///   a hard `Err` lands on `recovery_step`'s `Err => Break` arm and
///   permanently stops auto-recovery.
/// - `Err(_)` — genuine, non-self-healing error (unbindable provider, invalid
///   spec, sync failure).
pub(crate) async fn bring_up_sandbox(
    ctx: &BringUpCtx<'_>,
) -> miette::Result<Result<SandboxBringUp, SandboxDiagnosis>> {
    // The runtime installs itself on first use: no PATH dependency and no
    // user-visible install step.
    if let Err(e) = right_sandbox::ensure_runtime_installed().await {
        tracing::warn!(agent = %ctx.agent, "sandbox runtime install failed: {e:#}");
        return Ok(Err(diagnose(&e)));
    }
    if let Err(e) = right_sandbox::diagnose_host() {
        tracing::error!(agent = %ctx.agent, "host cannot run microVMs: {e:#}");
        return Ok(Err(diagnose(&e)));
    }

    let spec = sandbox_spec(ctx).await?;
    let sandbox = match SandboxHandle::create_or_attach(&spec).await {
        Ok(sandbox) => Arc::new(sandbox),
        Err(e) => {
            tracing::warn!(agent = %ctx.agent, sandbox = %ctx.sandbox_name, "sandbox bring-up failed: {e:#}");
            return Ok(Err(diagnose(&e)));
        }
    };
    if let Err(e) = sandbox.wait_ready(DEFAULT_READY_TIMEOUT).await {
        tracing::warn!(agent = %ctx.agent, sandbox = %ctx.sandbox_name, "sandbox did not become ready: {e:#}");
        return Ok(Err(diagnose(&e)));
    }
    tracing::info!(agent = %ctx.agent, sandbox = %ctx.sandbox_name, "Agent Sandbox ready");

    // Attachment transfer directories. Created every bring-up because a
    // recreated sandbox starts from the stock image.
    for dir in [SANDBOX_INBOX, SANDBOX_OUTBOX] {
        if let Err(e) = sandbox.fs_mkdir(dir).await {
            tracing::warn!(agent = %ctx.agent, dir, "creating attachment dir failed: {e:#}");
            return Ok(Err(diagnose(&e)));
        }
    }

    // Sync config files before considering the sandbox Ready: the guest must
    // have its `.claude.json`, settings, and platform tree before any turn.
    sync::initial_sync(ctx.agent_dir, &sandbox).await?;
    if let Err(e) = sync::reverse_sync_md(ctx.agent_dir, &sandbox).await {
        tracing::warn!(
            agent = %ctx.agent,
            sandbox = %ctx.sandbox_name,
            "startup identity mirror sync failed: {e:#}"
        );
    }

    Ok(Ok(SandboxBringUp { sandbox }))
}

/// Hot-apply a `sandbox.providers` change to a live sandbox.
///
/// Only credential *values* can change on a running sandbox: the secret
/// structure (which bindings exist, and their allowed hosts) is fixed at
/// create, so a newly declared or removed provider needs a sandbox recreate.
/// This rotates every declared provider's value and reports the ones that need
/// a recreate instead of silently leaving the agent unauthenticated.
pub(crate) async fn hot_reconcile_providers(
    agent: &str,
    config: &AgentConfig,
    providers: &ProviderStore,
    sandbox: &SandboxHandle,
) -> miette::Result<()> {
    let mut needs_recreate: Vec<String> = Vec::new();
    for binding in secret_bindings(agent, config, providers).await? {
        match sandbox.rotate_secret(&binding).await {
            Ok(rotation) => {
                tracing::info!(
                    agent = %agent,
                    env_var = %binding.env_var,
                    disposition = ?rotation.disposition,
                    warnings = ?rotation.warnings,
                    "provider credential rotated into the sandbox"
                );
            }
            Err(SandboxError::RotationTargetMissing { .. }) => {
                needs_recreate.push(binding.env_var.clone());
                tracing::warn!(
                    agent = %agent,
                    env_var = %binding.env_var,
                    "provider is declared but the sandbox has no binding for it; \
                     recreate the sandbox to pick it up"
                );
            }
            Err(e) => {
                return Err(miette::miette!(
                    "rotating provider secret {} failed: {e:#}",
                    binding.env_var
                ));
            }
        }
    }
    if !needs_recreate.is_empty() {
        tracing::warn!(
            agent = %agent,
            unbound = ?needs_recreate,
            "declared providers are missing from the live sandbox; recreate it to bind them"
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
    /// Per-agent directory (sync source).
    pub agent_dir: PathBuf,
    /// Deterministic sandbox name for this agent.
    pub sandbox_name: String,
    /// Full parsed agent config.
    pub config: AgentConfig,
    /// Provider credential store.
    pub providers: Arc<ProviderStore>,
    /// Shutdown token shared with the rest of the bot.
    pub shutdown: CancellationToken,
}

impl SupervisorDeps {
    /// Construct deps from owned values.
    pub(crate) fn new(
        agent: String,
        agent_dir: PathBuf,
        sandbox_name: String,
        config: AgentConfig,
        providers: Arc<ProviderStore>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            agent,
            agent_dir,
            sandbox_name,
            config,
            providers,
            shutdown,
        }
    }

    /// Borrow self's fields into a fresh `BringUpCtx` for one recovery attempt.
    fn bring_up_ctx(&self) -> BringUpCtx<'_> {
        BringUpCtx {
            agent: &self.agent,
            agent_dir: &self.agent_dir,
            sandbox_name: &self.sandbox_name,
            config: &self.config,
            providers: &self.providers,
        }
    }
}

/// Spawn the long-lived sync task for a freshly-Ready sandbox.
fn spawn_sync_task(
    deps: &SupervisorDeps,
    handle: &Arc<SandboxRuntimeHandle>,
    sandbox: Sandbox,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(sync::run_sync_task(
        deps.agent_dir.clone(),
        sandbox,
        Some(Arc::clone(handle)),
        deps.shutdown.clone(),
    ))
}

/// Decide whether a failed health probe means "degrade".
///
/// Pure, so the phase policy is testable without a microVM. A sandbox that is
/// still coming up (`Created`, `Starting`) is not a failure: degrading on it
/// would take the agent offline for the seconds a normal boot takes, and the
/// recovery loop would then "recover" a sandbox that was never broken. Every
/// other error — unreachable runtime, crashed or stopped guest, metrics
/// failure — degrades.
fn degrade_decision(error: &SandboxError) -> Option<SandboxDiagnosis> {
    if let SandboxError::NotRunning {
        phase: SandboxPhase::Created | SandboxPhase::Starting,
        ..
    } = error
    {
        return None;
    }
    Some(diagnose(error))
}

/// Verify a suspected failure against the live sandbox.
///
/// `Some(diagnosis)` means degrade. Health deliberately reports memory and
/// writable-layer figures only — CPU is never a health signal (stage-1
/// correction 6) — so this degrades on reachability and phase, never on load.
async fn probe_health(sandbox: &SandboxHandle) -> Option<SandboxDiagnosis> {
    match sandbox.health().await {
        Ok(report) => {
            tracing::debug!(
                sandbox = %sandbox.name(),
                phase = %report.phase,
                memory_used_bytes = report.memory_used_bytes,
                "sandbox health probe passed"
            );
            None
        }
        Err(e) => match degrade_decision(&e) {
            Some(diagnosis) => {
                tracing::warn!(sandbox = %sandbox.name(), "sandbox health probe failed: {e:#}");
                Some(diagnosis)
            }
            None => {
                tracing::debug!(
                    sandbox = %sandbox.name(),
                    "sandbox health probe found a still-booting sandbox: {e:#}"
                );
                None
            }
        },
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
            // Ready implies a live handle; its absence is itself a failure.
            let diagnosis = match handle.current_sandbox() {
                Some(sandbox) => probe_health(&sandbox).await,
                None => Some(
                    SandboxCause::SandboxNotRunning { sandbox: deps.sandbox_name.clone() }
                        .diagnose(),
                ),
            };
            let Some(diagnosis) = diagnosis else {
                return LoopStep::Continue;
            };
            tracing::error!(agent = %deps.agent, cause = ?diagnosis.cause, "{}", diagnosis.summary);
            handle.set_unavailable(Arc::new(diagnosis));
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
            handle.set_ready(Arc::clone(&bring_up.sandbox));
            *sync_task = Some(spawn_sync_task(deps, handle, bring_up.sandbox));
            notify_back_online(handle, bot).await;
            *attempt = 0;
            tracing::info!(agent = %deps.agent, "sandbox recovered");
            // Skip the backoff sleep: re-enter the monitor branch immediately
            // now that the handle is Ready.
            return LoopStep::Continue;
        }
        Ok(Err(diagnosis)) => {
            // Surface the diagnosis on every recovery failure; a silent retry
            // loop hides a persistent bring-up failure completely.
            tracing::warn!(
                agent = %deps.agent,
                cause = ?diagnosis.cause,
                attempt = *attempt,
                "sandbox bring-up failed; staying degraded and retrying: {}",
                diagnosis.summary
            );
            handle.set_unavailable(Arc::new(diagnosis));
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

/// Drive the supervisor loop to completion.
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
pub(crate) fn spawn_supervisor(
    handle: Arc<SandboxRuntimeHandle>,
    failure_rx: mpsc::Receiver<()>,
    bot: crate::telegram::BotType,
    deps: SupervisorDeps,
    initial_sync_task: Option<tokio::task::JoinHandle<()>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_supervisor(
        handle,
        failure_rx,
        bot,
        deps,
        initial_sync_task,
    ))
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
#[path = "sandbox_supervisor_tests.rs"]
mod tests;
