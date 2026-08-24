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

fn retryable_sync_diagnosis(error: &miette::Report) -> Option<SandboxDiagnosis> {
    error
        .downcast_ref::<crate::claude_runtime::ClaudeRuntimeError>()
        .filter(|runtime| runtime.is_retryable())
        .map(|_| SandboxCause::Unreachable.diagnose())
}

/// Resolve one declared provider into a source-ref secret binding.
///
/// The returned binding privately carries a redacted credential for
/// right-sandbox's scoped SDK resolver. `NeedsValue` is returned as `None`
/// only for create-time bulk resolution, where a migrated agent must be
/// allowed to start before the dashboard receives its value.
async fn secret_binding(
    agent: &str,
    provider_name: &str,
    providers: &ProviderStore,
) -> miette::Result<Option<right_sandbox::SecretBinding>> {
    let record = providers.get(agent, provider_name).await.map_err(|e| {
        miette::miette!(
            "provider '{provider_name}' declared by agent {agent} cannot be bound: {e:#}"
        )
    })?;
    if record.status == ProviderStatus::NeedsValue {
        tracing::warn!(
            agent = %agent,
            provider = %provider_name,
            env_var = %record.env_var,
            "provider holds no credential yet, so the sandbox starts without it; \
             add its credential from the dashboard: /providers"
        );
        return Ok(None);
    }
    providers
        .source_ref_binding(agent, provider_name)
        .await
        .map(Some)
        .map_err(|e| {
            miette::miette!(
                "provider '{provider_name}' declared by agent {agent} cannot be bound: {e:#}"
            )
        })
}

async fn secret_bindings(
    agent: &str,
    config: &AgentConfig,
    providers: &ProviderStore,
) -> miette::Result<Vec<right_sandbox::SecretBinding>> {
    secret_bindings_with(agent, config, providers, false).await
}

/// Bulk provider resolution, with an optional tolerance mode.
///
/// The strict mode is create-time resolution: an unresolvable declared
/// provider must abort bring-up, because the spec is about to be built from
/// these bindings. The tolerant mode is reconcile-time resolution: a desired
/// provider that cannot be bound (unknown built-in slug, store error, or a
/// `Status::Error` record) is downgraded to a warning and skipped so that it
/// cannot block revocation of bindings that are already obsolete. Duplicate
/// guest env-var identities remain hard errors in both modes — that is a
/// config defect that must be fixed before any sandbox mutation is safe.
async fn secret_bindings_with(
    agent: &str,
    config: &AgentConfig,
    providers: &ProviderStore,
    tolerant: bool,
) -> miette::Result<Vec<right_sandbox::SecretBinding>> {
    let mut bindings = Vec::new();
    let mut env_vars = std::collections::HashMap::<String, &str>::new();
    for entry in config.providers() {
        let env_var = match provider_entry_env_var(entry) {
            Ok(env_var) => env_var,
            Err(error) if tolerant => {
                tracing::warn!(
                    agent = %agent,
                    provider = %entry.name,
                    error = %format!("{error:#}"),
                    "desired provider is unresolvable; skipping it during reconcile"
                );
                continue;
            }
            Err(error) => return Err(error),
        };
        if let Some(first) = env_vars.insert(env_var.to_owned(), &entry.name) {
            return Err(miette::miette!(
                "providers '{first}' and '{}' both bind guest env var '{env_var}'; binding identity must be unique per agent",
                entry.name
            ));
        }
        match secret_binding(agent, &entry.name, providers).await {
            Ok(Some(binding)) => bindings.push(binding),
            Ok(None) => {}
            Err(error) if tolerant => {
                tracing::warn!(
                    agent = %agent,
                    provider = %entry.name,
                    error = %format!("{error:#}"),
                    "desired provider cannot be bound; skipping it during reconcile"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(bindings)
}

/// Guest environment variables that are, or ever were, provider-managed for
/// this agent. Source identity is verified independently with
/// `is_source_identity`, avoiding cross-products while still allowing removal
/// after the owning store row has been deleted.
#[cfg(test)]
fn right_managed_secret_env_vars(
    previous_configs: &[AgentConfig],
    config: &AgentConfig,
) -> miette::Result<std::collections::HashSet<String>> {
    right_managed_secret_env_vars_with(previous_configs, config, false)
}

/// Tolerant variant of [`right_managed_secret_env_vars`]: an entry whose guest
/// env var cannot be derived (an unknown built-in slug or a generic entry
/// missing its definition) is skipped, so revocation of the other managed
/// bindings still proceeds.
fn right_managed_secret_env_vars_with(
    previous_configs: &[AgentConfig],
    config: &AgentConfig,
    tolerant: bool,
) -> miette::Result<std::collections::HashSet<String>> {
    let mut vars = std::collections::HashSet::new();
    for entry in previous_configs
        .iter()
        .chain(std::iter::once(config))
        .flat_map(AgentConfig::providers)
    {
        match provider_entry_env_var(entry) {
            Ok(env_var) => {
                vars.insert(env_var.to_owned());
            }
            Err(error) if tolerant => {
                tracing::warn!(
                    provider = %entry.name,
                    error = %format!("{error:#}"),
                    "provider env var is unresolvable; skipping it when computing managed identities"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(vars)
}

fn provider_entry_env_var(
    entry: &right_agent::agent::types::ProviderEntry,
) -> miette::Result<&str> {
    match &entry.type_ {
        right_agent::agent::types::ProviderType::BuiltIn(slug) => {
            right_providers::catalog::builtin(slug)
                .map(|provider| provider.env_var)
                .ok_or_else(|| {
                    miette::miette!(
                        "provider '{}' has unknown built-in type '{slug}'",
                        entry.name
                    )
                })
        }
        right_agent::agent::types::ProviderType::Generic => entry
            .generic
            .as_ref()
            .map(|generic| generic.env_var.as_str())
            .ok_or_else(|| {
                miette::miette!(
                    "generic provider '{}' has no generic definition",
                    entry.name
                )
            }),
    }
}

/// Resolve one named provider after a dashboard mutation.
///
/// The config supplies identity; [`ProviderStore`] returns a redacted binding
/// whose credential remains private to the sandbox application path.
pub(crate) async fn resolve_named_provider(
    agent: &str,
    provider_name: &str,
    config: &AgentConfig,
    providers: &ProviderStore,
) -> miette::Result<right_sandbox::SecretBinding> {
    if !config
        .providers()
        .iter()
        .any(|entry| entry.name == provider_name)
    {
        return Err(miette::miette!(
            "provider '{provider_name}' is not declared by agent {agent}"
        ));
    }
    secret_binding(agent, provider_name, providers)
        .await?
        .ok_or_else(|| miette::miette!("provider '{provider_name}' still has no usable credential"))
}

/// Resolve and apply one named provider after a dashboard mutation.
pub(crate) async fn apply_named_provider(
    agent: &str,
    provider_name: &str,
    config: &AgentConfig,
    providers: &ProviderStore,
    sandbox: &SandboxHandle,
) -> miette::Result<right_sandbox::SecretApply> {
    let binding = resolve_named_provider(agent, provider_name, config, providers).await?;
    sandbox
        .apply_secret(&binding)
        .await
        .map_err(|e| miette::miette!("applying provider secret {} failed: {e:#}", binding.env_var))
}

/// Build an agent's create-time sandbox specification.
///
/// Resolving the agent's declared providers is the only part that needs the
/// bot's store; every other field comes from the shared
/// [`right_sandbox::agent_sandbox_spec`]. The bot's bring-up, the CLI's
/// `agent restore`, and `right agent migrate-sandbox` all use this spec.
/// Later provider additions use [`SandboxHandle::apply_secret`] so existing
/// sandboxes are upgraded without deletion.
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
        tracing::warn!(agent = %ctx.agent, sandbox = %ctx.sandbox_name, "sandbox readiness failed: {e:#}");
        return Ok(Err(diagnose(&e)));
    }
    hot_reconcile_providers(ctx.agent, &[], ctx.config, ctx.providers, &sandbox)
        .await
        .map_err(|e| {
            miette::miette!(
                "failed to reconcile provider credentials for agent {}: {e:#}",
                ctx.agent
            )
        })?;

    // Sync config files before considering the sandbox Ready: the guest must
    // have its `.claude.json`, settings, and platform tree before any turn.
    if let Err(error) = sync::initial_sync(ctx.agent_dir, &sandbox).await {
        if let Some(diagnosis) = retryable_sync_diagnosis(&error) {
            tracing::warn!(agent = %ctx.agent, error = %format!("{error:#}"), "transient Claude runtime staging failure");
            return Ok(Err(diagnosis));
        }
        return Err(error);
    }
    if let Err(e) = sync::reverse_sync_md(ctx.agent_dir, &sandbox).await {
        tracing::warn!(
            agent = %ctx.agent,
            sandbox = %ctx.sandbox_name,
            "startup identity mirror sync failed: {e:#}"
        );
    }

    Ok(Ok(SandboxBringUp { sandbox }))
}

/// Reconcile provider declarations to a live sandbox.
///
/// Right removes only bindings that both (a) exist in the sandbox now and (b)
/// are named by this agent's current/previous accepted provider declarations,
/// but are absent from the latest desired bindings. This revokes obsolete
/// bindings without touching unrelated sandbox secrets. Desired bindings are
/// then applied, rotating live or using the SDK's restart-backed addition path.
pub(crate) async fn hot_reconcile_providers(
    agent: &str,
    previous_configs: &[AgentConfig],
    config: &AgentConfig,
    providers: &ProviderStore,
    sandbox: &SandboxHandle,
) -> miette::Result<()> {
    let bindings = secret_bindings_with(agent, config, providers, true).await?;
    let desired: std::collections::HashSet<&str> = bindings
        .iter()
        .map(|binding| binding.env_var.as_str())
        .collect();
    let current: std::collections::HashSet<String> = sandbox
        .secret_env_vars()
        .await
        .map_err(|e| miette::miette!("reading live sandbox secret identities failed: {e:#}"))?
        .into_iter()
        .collect();
    let current_source_refs: std::collections::HashMap<String, String> = sandbox
        .secret_source_refs()
        .await
        .map_err(|e| miette::miette!("reading live sandbox source refs failed: {e:#}"))?
        .into_iter()
        .collect();
    let managed_env_vars = right_managed_secret_env_vars_with(previous_configs, config, true)?;
    for (env_var, source_ref) in current_source_refs {
        let is_right_managed =
            managed_env_vars.contains(&env_var) && right_providers::is_source_identity(&source_ref);
        if current.contains(&env_var) && is_right_managed && !desired.contains(env_var.as_str()) {
            let removed = sandbox.remove_secret(&env_var).await.map_err(|e| {
                miette::miette!("removing obsolete provider secret {env_var} failed: {e:#}")
            })?;
            tracing::info!(
                agent = %agent,
                env_var = %env_var,
                disposition = ?removed.disposition,
                warnings = ?removed.warnings,
                "obsolete provider credential removed from the sandbox"
            );
        }
    }
    for binding in bindings {
        let applied = sandbox.apply_secret(&binding).await.map_err(|e| {
            miette::miette!("applying provider secret {} failed: {e:#}", binding.env_var)
        })?;
        tracing::info!(
            agent = %agent,
            env_var = %binding.env_var,
            disposition = ?applied.disposition,
            warnings = ?applied.warnings,
            "provider credential applied to the sandbox"
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
/// Provider-only reloads publish a new config through `config`. Recovery
/// snapshots it while holding `provider_mutation`, so a dashboard mutation or
/// watcher reconciliation cannot race bring-up with an obsolete provider set.
pub(crate) struct SupervisorDeps {
    /// Agent name (logging + operator-facing help text).
    pub agent: String,
    /// Per-agent directory (sync source).
    pub agent_dir: PathBuf,
    /// Deterministic sandbox name for this agent.
    pub sandbox_name: String,
    /// Latest accepted agent config. Provider-only reloads replace this value.
    pub config: Arc<arc_swap::ArcSwap<AgentConfig>>,
    /// Provider credential store.
    pub providers: Arc<ProviderStore>,
    /// Serializes provider config publication, live apply, and recovery.
    pub provider_mutation: Arc<tokio::sync::Mutex<()>>,
    /// Shutdown token shared with the rest of the bot.
    pub shutdown: CancellationToken,
}

impl SupervisorDeps {
    /// Construct deps from owned values.
    pub(crate) fn new(
        agent: String,
        agent_dir: PathBuf,
        sandbox_name: String,
        config: Arc<arc_swap::ArcSwap<AgentConfig>>,
        providers: Arc<ProviderStore>,
        provider_mutation: Arc<tokio::sync::Mutex<()>>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            agent,
            agent_dir,
            sandbox_name,
            config,
            providers,
            provider_mutation,
            shutdown,
        }
    }

    /// Snapshot the latest accepted config for one recovery attempt.
    fn config_snapshot(&self) -> Arc<AgentConfig> {
        self.config.load_full()
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
    let _mutation = deps.provider_mutation.lock().await;
    let _agent_guard = match deps.providers.agent_lock(&deps.agent).await {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(agent = %deps.agent, error = %format!("{error:#}"), "failed to lock provider recovery");
            return LoopStep::Break;
        }
    };
    let config = deps.config_snapshot();
    let ctx = BringUpCtx {
        agent: &deps.agent,
        agent_dir: &deps.agent_dir,
        sandbox_name: &deps.sandbox_name,
        config: &config,
        providers: &deps.providers,
    };
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
