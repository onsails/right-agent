use std::path::{Path, PathBuf};

use right_sandbox::{SandboxError, SandboxHandle};

use crate::agent::types::AgentConfig;
use crate::sandbox_backup::archive_guest_home;

/// Options for destroying an agent (resolved by caller — no TTY interaction).
pub struct DestroyOptions {
    pub agent_name: String,
    pub backup: bool,
}

/// Result of a destroy operation — booleans reflect what actually happened.
#[derive(Debug)]
pub struct DestroyResult {
    /// Whether the agent process was stopped via process-compose.
    pub agent_stopped: bool,
    /// Whether the agent's Agent Sandbox was actually deleted.
    ///
    /// False when no sandbox existed under the agent's name, and never set
    /// optimistically: a `true` here is a microVM that is provably gone.
    pub sandbox_deleted: bool,
    /// Path to backup if one was created.
    pub backup_path: Option<PathBuf>,
    /// Whether the agent directory was removed.
    pub dir_removed: bool,
    /// Whether process-compose was reloaded.
    pub pc_reloaded: bool,
}

/// Run a pre-destroy backup. Returns the backup directory path.
///
/// Archives the Agent Sandbox to `sandbox.tar.gz`, then copies agent.yaml,
/// policy.yaml, allowlist.yaml and VACUUM-copies data.db. Every step is
/// fatal: `destroy_agent` calls this *before* it deletes anything, and a
/// half-written backup that still lets the destroy proceed is how agent data
/// disappears.
async fn run_backup(
    home: &Path,
    agent_name: &str,
    agent_dir: &Path,
    config: &Option<AgentConfig>,
) -> miette::Result<PathBuf> {
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let backup_dir = right_config::backups_dir(home, agent_name).join(&timestamp);
    std::fs::create_dir_all(&backup_dir).map_err(|e| {
        miette::miette!(
            "failed to create backup dir {}: {e:#}",
            backup_dir.display()
        )
    })?;

    tracing::info!(agent = agent_name, backup_dir = %backup_dir.display(), "starting pre-destroy backup");

    if back_up_sandbox(agent_name, config, &backup_dir).await? == SandboxBackup::Absent {
        tracing::warn!(
            agent = agent_name,
            "no Agent Sandbox exists — backing up config files only"
        );
    }

    for filename in &["agent.yaml", "policy.yaml", "allowlist.yaml"] {
        let src = agent_dir.join(filename);
        if src.exists() {
            std::fs::copy(&src, backup_dir.join(filename))
                .map_err(|e| miette::miette!("failed to copy {filename}: {e:#}"))?;
        }
    }

    let db_path = agent_dir.join("data.db");
    if db_path.exists() {
        let backup_db = backup_dir.join("data.db");
        let db_display = db_path.display().to_string();
        let backup_path_sql = backup_db.display().to_string().replace('\'', "''");
        // `open_connection(.., migrate=false)` returns a writable handle.
        // Turso's `VACUUM INTO` needs writability on the source DB even though
        // we never mutate user rows here.
        let conn = right_db::open_connection(agent_dir, false)
            .await
            .map_err(|e| miette::miette!("failed to open {}: {e:#}", db_display))?;
        conn.execute(&format!("VACUUM INTO '{backup_path_sql}'"), ())
            .await
            .map_err(|e| miette::miette!("VACUUM INTO failed: {e:#}"))?;
    }

    tracing::info!(backup_dir = %backup_dir.display(), "pre-destroy backup complete");
    Ok(backup_dir)
}

/// Whether the sandbox half of a pre-destroy backup produced an archive.
#[derive(Debug, PartialEq, Eq)]
enum SandboxBackup {
    /// `sandbox.tar.gz` was written from the live guest.
    Archived,
    /// No sandbox exists under the agent's name, so there was nothing to
    /// archive. Distinct from a failure: there is no data at risk.
    Absent,
}

/// Archive the agent's guest home into `<backup_dir>/sandbox.tar.gz`.
///
/// Any failure other than "no such sandbox" is an error: the guest holds the
/// agent's authoritative memory, skills and workspace, and `destroy_agent`
/// deletes the microVM moments later. Silently degrading to a config-only
/// backup — the pre-microsandbox behaviour — destroys that data.
async fn back_up_sandbox(
    agent_name: &str,
    config: &Option<AgentConfig>,
    backup_dir: &Path,
) -> miette::Result<SandboxBackup> {
    let explicit_sandbox_name = config
        .as_ref()
        .and_then(|c| c.sandbox.as_ref())
        .and_then(|s| s.name.as_deref());
    let sb_name = right_sandbox::resolve_sandbox_name(agent_name, explicit_sandbox_name);

    let sandbox = match SandboxHandle::attach(&sb_name).await {
        Ok(sandbox) => sandbox,
        Err(SandboxError::NotFound { .. }) => return Ok(SandboxBackup::Absent),
        Err(error) => {
            return Err(miette::miette!(
                "cannot back up sandbox '{sb_name}' before destroying agent '{agent_name}': {error:#}"
            ));
        }
    };

    // A pre-destroy backup is the agent's last copy — the microVM is deleted
    // moments later — so it keeps the rebuildable caches too, matching what
    // the pre-microsandbox destroy archived.
    archive_guest_home(&sandbox, &backup_dir.join("sandbox.tar.gz"), true)
        .await
        .map_err(|error| {
            miette::miette!("back up agent '{agent_name}' before destroying it: {error:#}")
        })?;

    Ok(SandboxBackup::Archived)
}

/// What the destroy cascade must do to provider records when `deleting` is removed.
#[derive(Debug, Default, PartialEq)]
struct DestroyProviderPlan {
    /// Records to detach from the deleting agent's sandbox (all of its own entries).
    detach: Vec<String>,
    /// Records to delete from the gateway (no surviving agent references them).
    delete: Vec<String>,
}

/// Decide the provider cascade for destroying `deleting`. `agents` is the full
/// set of (agent_name, its sandbox.providers) including the agent being deleted.
/// Pure — no gateway/fs.
fn plan_destroy_provider_cascade(
    deleting: &str,
    agents: &[(String, Vec<right_agent_config::ProviderEntry>)],
    all_complete: bool,
) -> DestroyProviderPlan {
    let mut plan = DestroyProviderPlan::default();
    let Some((_, deleting_providers)) = agents.iter().find(|(a, _)| a == deleting) else {
        return plan;
    };
    for entry in deleting_providers {
        plan.detach.push(entry.name.clone());
        if !all_complete {
            // Sibling enumeration was incomplete — skip the delete to avoid
            // removing a gateway record still referenced by an unread agent.
            continue;
        }
        // Delete only when no surviving agent still references this record by
        // name. Ownership itself lives in providers.db, so a survivor keeping
        // the record alive needs no agent.yaml edit here.
        let referenced_elsewhere = agents
            .iter()
            .filter(|(a, _)| a != deleting)
            .any(|(_, ps)| ps.iter().any(|p| p.name == entry.name));
        if !referenced_elsewhere {
            plan.delete.push(entry.name.clone());
        }
    }
    plan
}

/// Enumerate every agent directory under `agents_dir` and load its declared
/// `sandbox.providers`. Tolerant like `build_peers`: a sibling whose directory
/// or `agent.yaml` can't be read (or has no providers) contributes an empty list
/// and is never fatal. Returns `(agent_name, providers)` pairs including the
/// agent being deleted.
fn load_agents_with_providers(
    agents_dir: &Path,
) -> (Vec<(String, Vec<right_agent_config::ProviderEntry>)>, bool) {
    let entries = match std::fs::read_dir(agents_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(
                dir = %agents_dir.display(),
                error = %format!("{e:#}"),
                "could not read agents dir for provider refcount; treating as empty"
            );
            return (Vec::new(), false);
        }
    };
    let mut agents = Vec::new();
    let mut all_complete = true;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        if !path.join("agent.yaml").exists() {
            continue;
        }
        let providers = match crate::agent::parse_agent_config(&path) {
            Ok(Some(cfg)) => cfg.sandbox.map(|s| s.providers).unwrap_or_default(),
            Ok(None) => Vec::new(),
            Err(e) => {
                tracing::warn!(
                    agent = %name,
                    error = %format!("{e:#}"),
                    "skipping sibling with unreadable agent.yaml during provider refcount"
                );
                all_complete = false;
                Vec::new()
            }
        };
        agents.push((name, providers));
    }
    (agents, all_complete)
}

/// Destroy an agent: stop process, optionally backup, delete sandbox, remove directory, reload PC.
///
/// Non-fatal steps (stop, sandbox delete, PC reload) warn and continue.
/// Fatal steps (backup if requested, directory removal) propagate errors.
pub async fn destroy_agent(home: &Path, options: &DestroyOptions) -> miette::Result<DestroyResult> {
    let agents_dir = right_config::agents_dir(home);
    let agent_dir = agents_dir.join(&options.agent_name);

    if !agent_dir.exists() {
        return Err(miette::miette!(
            "Agent '{}' not found at {}",
            options.agent_name,
            agent_dir.display(),
        ));
    }

    let config = crate::agent::parse_agent_config(&agent_dir)?;

    let mut result = DestroyResult {
        agent_stopped: false,
        sandbox_deleted: false,
        backup_path: None,
        dir_removed: false,
        pc_reloaded: false,
    };

    // `PcClient::from_home` enforces --home isolation: it reads the PC port
    // from `<home>/run/state.json` and returns None when that file is absent.
    // Without this guard, destroy invoked against an isolated --home would
    // health-check the user's live PC on the default port and SIGTERM a
    // same-named process there. See ARCHITECTURE.md "Runtime isolation — mandatory".
    let pc_client = crate::runtime::PcClient::from_home(home)?;
    if pc_client.is_none() {
        tracing::debug!(
            home = %home.display(),
            "no runtime state — skipping PC interaction"
        );
    }
    let pc_running = match &pc_client {
        Some(c) => c.health_check().await.is_ok(),
        None => false,
    };

    if let (Some(pc_client), true) = (&pc_client, pc_running) {
        let process_name = format!("{}-bot", options.agent_name);
        match pc_client.stop_process(&process_name).await {
            Ok(()) => {
                tracing::info!(agent = %options.agent_name, "stopped agent process");
                result.agent_stopped = true;
            }
            Err(e) => {
                tracing::warn!(agent = %options.agent_name, error = format!("{e:#}"), "failed to stop agent process (may already be stopped)");
            }
        }
    }

    // Best-effort deleteWebhook before sandbox/dir cleanup so Telegram stops
    // attempting deliveries. Failures (network, invalid token) log a warning
    // and continue — a stale webhook URL is a soft leak, not worth blocking
    // destroy on.
    if let Some(cfg) = config.as_ref()
        && let Some(token) = cfg.telegram_token.as_deref()
    {
        let url = format!("https://api.telegram.org/bot{token}/deleteWebhook");
        match reqwest::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => tracing::info!(
                agent = %options.agent_name,
                "deleted Telegram webhook"
            ),
            Ok(resp) => tracing::warn!(
                agent = %options.agent_name,
                status = %resp.status(),
                "deleteWebhook returned non-success (continuing)"
            ),
            Err(e) => tracing::warn!(
                agent = %options.agent_name,
                error = %format!("{e:#}"),
                "deleteWebhook failed (continuing)"
            ),
        }
    }

    if options.backup {
        let backup_path = run_backup(home, &options.agent_name, &agent_dir, &config).await?;
        result.backup_path = Some(backup_path);
    }

    let sandbox_name_for_cascade = {
        let explicit_sandbox_name = config
            .as_ref()
            .and_then(|c| c.sandbox.as_ref())
            .and_then(|s| s.name.as_deref());
        let sb_name =
            right_sandbox::resolve_sandbox_name(&options.agent_name, explicit_sandbox_name);
        // `sandbox_deleted` is what the CLI prints back to the user, so it
        // tracks the observed outcome: a microVM that is provably gone, never
        // "we asked". A failure here is non-fatal (the agent directory must
        // still go) but must never be reported as a deletion.
        match SandboxHandle::delete(&sb_name).await {
            Ok(deleted) => {
                result.sandbox_deleted = deleted;
                if !deleted {
                    tracing::info!(sandbox = %sb_name, "no sandbox to delete");
                }
            }
            Err(error) => tracing::warn!(
                sandbox = %sb_name,
                error = %format!("{error:#}"),
                "failed to delete Agent Sandbox; continuing destroy"
            ),
        }
        sb_name
    };

    // Cascade-clean provider records on the gateway (best-effort).
    // Deleting the sandbox above removes its attachments with it, but that
    // delete can fail (logged, non-fatal) and leave the sandbox — and
    // therefore the attachments — alive, at which point the gateway rejects
    // `DeleteProvider` with FailedPrecondition. To stay self-healing per
    // AGENTS.md, we explicitly detach each of THIS agent's records first
    // (NotFound = already detached, treat as success), mirroring
    // `handle_provider_remove`.
    //
    // A single gateway record may be referenced by several agents, so
    // `plan_destroy_provider_cascade` REFCOUNTS references and deletes a record
    // ONLY when no surviving agent references it. Ownership itself lives in
    // providers.db, not in agent.yaml. The whole block is best-effort: every
    // failure logs and continues so the agent-directory removal below still runs.
    if let Some(sandbox) = config.as_ref().and_then(|c| c.sandbox.as_ref())
        && !sandbox.providers.is_empty()
    {
        // Enumerate every sibling agent (including the one being deleted) and its
        // declared `sandbox.providers`. Tolerant like `build_peers`: a sibling
        // whose agent.yaml can't be read is skipped, not fatal.
        let (mut agents, all_complete) = load_agents_with_providers(&agents_dir);

        // The deleting agent's own providers are authoritative from the in-memory
        // config; never depend on disk enumeration for them (a read_dir blip must
        // not skip detach/delete of this agent's records).
        let own_providers = config
            .as_ref()
            .and_then(|c| c.sandbox.as_ref())
            .map(|s| s.providers.clone())
            .unwrap_or_default();
        match agents.iter_mut().find(|(a, _)| a == &options.agent_name) {
            Some((_, ps)) => *ps = own_providers,
            None => agents.push((options.agent_name.clone(), own_providers)),
        }

        let plan = plan_destroy_provider_cascade(&options.agent_name, &agents, all_complete);

        let mtls_dir = right_openshell::openshell::default_mtls_dir();
        match right_openshell::openshell::connect_grpc(&mtls_dir).await {
            Ok(mut client) => {
                // Detach all of THIS agent's records from its sandbox.
                for name in &plan.detach {
                    match right_openshell::providers::detach_from_sandbox(
                        &mut client,
                        &sandbox_name_for_cascade,
                        name,
                    )
                    .await
                    {
                        Ok(()) => {}
                        Err(right_openshell::providers::ProviderError::NotFound(_)) => {
                            // Already detached — expected when delete_sandbox succeeded.
                        }
                        Err(e) => tracing::warn!(
                            name = %name,
                            sandbox = %sandbox_name_for_cascade,
                            error = %format!("{e:#}"),
                            "failed to detach provider during destroy; continuing"
                        ),
                    }
                }
                // Delete ONLY records no surviving agent references.
                for name in &plan.delete {
                    match right_openshell::providers::delete_provider(&mut client, name).await {
                        Ok(()) => {}
                        Err(right_openshell::providers::ProviderError::NotFound(_)) => {
                            // Already gone — nothing to clean up.
                        }
                        Err(e) => tracing::warn!(
                            name = %name,
                            error = %format!("{e:#}"),
                            "failed to delete provider during destroy; continuing"
                        ),
                    }
                }
            }
            Err(e) => tracing::warn!(
                error = %format!("{e:#}"),
                "could not connect to openshell gateway for provider cleanup; continuing destroy"
            ),
        }
    }

    std::fs::remove_dir_all(&agent_dir).map_err(|e| {
        miette::miette!(
            "failed to remove agent directory {}: {e:#}",
            agent_dir.display(),
        )
    })?;
    result.dir_removed = true;
    tracing::info!(agent = %options.agent_name, dir = %agent_dir.display(), "removed agent directory");

    if let (Some(pc_client), true) = (pc_client, pc_running) {
        let all_agents = crate::agent::discover_agents(&agents_dir)?;
        let self_exe = std::env::current_exe()
            .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;
        let codegen_outcome =
            right_codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

        match pc_client.reload_configuration().await {
            Ok(()) => {
                tracing::info!(
                    cloudflared_config_changed = codegen_outcome.cloudflared_config_changed,
                    "reloaded process-compose configuration"
                );
                result.pc_reloaded = true;
                pc_client
                    .restart_cloudflared_or_warn(codegen_outcome.cloudflared_config_changed)
                    .await;
            }
            Err(e) => {
                tracing::warn!(
                    error = format!("{e:#}"),
                    "failed to reload process-compose (non-fatal)"
                );
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
#[path = "destroy_tests.rs"]
mod tests;
