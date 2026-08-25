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
    _guard: &crate::runtime::RuntimeExclusionGuard,
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

/// Destroy an agent: stop process, optionally backup, delete sandbox, remove directory, reload PC.
///
/// Non-fatal steps (stop, sandbox delete, PC reload) warn and continue.
/// Fatal steps (backup if requested, directory removal) propagate errors.
pub async fn destroy_agent(home: &Path, options: &DestroyOptions) -> miette::Result<DestroyResult> {
    let _quiescence_guard = crate::runtime::require_runtime_quiesced(home).await?;
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
        let backup_path = run_backup(
            home,
            &options.agent_name,
            &agent_dir,
            &config,
            &_quiescence_guard,
        )
        .await?;
        result.backup_path = Some(backup_path);
    }

    {
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
    }

    // Cascade-clean the agent's provider records from the credential store.
    //
    // The store is the authority: a record the destroyed agent owned would
    // otherwise outlive every agent that could rotate it. `remove` re-homes an
    // owned record to a surviving borrower rather than deleting a credential
    // someone still declares, and a borrowed reference is only unshared — so
    // no other agent loses access. Best-effort: every failure logs and
    // continues so the agent-directory removal below still runs.
    match right_providers::ProviderStore::open(home).await {
        Ok(store) => {
            let held = match store.list(&options.agent_name).await {
                Ok(held) => held,
                Err(e) => {
                    tracing::warn!(
                        agent = %options.agent_name,
                        error = %format!("{e:#}"),
                        "could not list provider records during destroy; continuing"
                    );
                    Vec::new()
                }
            };
            for record in held {
                let outcome = if record.is_borrowed() {
                    store.unshare(&options.agent_name, &record.name).await
                } else {
                    store.remove(&options.agent_name, &record.name).await
                };
                if let Err(e) = outcome {
                    tracing::warn!(
                        agent = %options.agent_name,
                        provider = %record.name,
                        error = %format!("{e:#}"),
                        "failed to clean up provider record during destroy; continuing"
                    );
                }
            }
        }
        Err(e) => tracing::warn!(
            error = %format!("{e:#}"),
            "could not open the provider store for destroy cleanup; continuing"
        ),
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
