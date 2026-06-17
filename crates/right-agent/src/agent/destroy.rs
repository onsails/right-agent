use std::path::{Path, PathBuf};

use super::backup::push_no_sandbox_database_tar_excludes;
use crate::agent::types::AgentConfig;

/// Options for destroying an agent (resolved by caller — no TTY interaction).
pub struct DestroyOptions {
    pub agent_name: String,
    pub backup: bool,
}

/// Result of a destroy operation — booleans reflect what actually happened.
pub struct DestroyResult {
    /// Whether the agent process was stopped via process-compose.
    pub agent_stopped: bool,
    /// Whether an OpenShell sandbox was deleted.
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
/// For non-sandboxed agents: tars the agent directory (excluding data.db and runtime sidecars).
/// For sandboxed agents: attempts SSH tar of sandbox, falls back to config-only backup.
/// Always copies agent.yaml, policy.yaml, allowlist.yaml, and VACUUM-copies data.db.
async fn run_backup(
    home: &Path,
    agent_name: &str,
    agent_dir: &Path,
    config: &Option<AgentConfig>,
    is_sandboxed: bool,
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

    if is_sandboxed {
        // Try SSH tar download from sandbox; skip if sandbox not ready
        let sandbox_backed_up = try_sandbox_backup(home, agent_name, config, &backup_dir).await;
        if !sandbox_backed_up {
            tracing::warn!(
                agent = agent_name,
                "sandbox not available for backup — backing up config files only"
            );
        }
    } else {
        // Non-sandboxed: tar the agent dir (excluding data.db — backed up separately)
        let dest_tar = backup_dir.join("sandbox.tar.gz");
        let parent = agent_dir
            .parent()
            .ok_or_else(|| miette::miette!("agent_dir has no parent"))?;
        let mut tar_args = vec![
            "czpf".to_string(),
            dest_tar
                .to_str()
                .ok_or_else(|| miette::miette!("non-UTF-8 backup path"))?
                .to_string(),
        ];
        push_no_sandbox_database_tar_excludes(&mut tar_args, agent_name);
        tar_args.extend([
            "-C".to_string(),
            parent
                .to_str()
                .ok_or_else(|| miette::miette!("non-UTF-8 agents_dir"))?
                .to_string(),
            agent_name.to_string(),
        ]);
        let status = tokio::process::Command::new("tar")
            .args(&tar_args)
            .status()
            .await
            .map_err(|e| miette::miette!("failed to spawn tar: {e:#}"))?;
        if !status.success() {
            return Err(miette::miette!("tar exited with status {status}"));
        }
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

async fn try_sandbox_backup(
    home: &Path,
    agent_name: &str,
    config: &Option<AgentConfig>,
    backup_dir: &Path,
) -> bool {
    let explicit_sandbox_name = config
        .as_ref()
        .and_then(|c| c.sandbox.as_ref())
        .and_then(|s| s.name.as_deref());
    let sb_name =
        right_openshell::openshell::resolve_sandbox_name(agent_name, explicit_sandbox_name);

    // Check OpenShell availability
    let mtls_dir = match right_openshell::openshell::preflight_check() {
        right_openshell::openshell::OpenShellStatus::Ready(dir) => dir,
        _ => return false,
    };

    // Check sandbox readiness
    let mut grpc = match right_openshell::openshell::connect_grpc(&mtls_dir).await {
        Ok(g) => g,
        Err(_) => return false,
    };
    let ready = match right_openshell::openshell::is_sandbox_ready(&mut grpc, &sb_name).await {
        Ok(r) => r,
        Err(_) => return false,
    };
    if !ready {
        return false;
    }

    let ssh_config = home
        .join("run")
        .join("ssh")
        .join(format!("{sb_name}.ssh-config"));
    if !ssh_config.exists() {
        return false;
    }

    let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(&sb_name);
    let dest_tar = backup_dir.join("sandbox.tar.gz");

    right_openshell::openshell::ssh_tar_download(
        &ssh_config,
        &ssh_host,
        "sandbox",
        &dest_tar,
        true,
        300,
    )
    .await
    .is_ok()
}

/// What the destroy cascade must do to provider records when `deleting` is removed.
#[derive(Debug, Default, PartialEq)]
struct DestroyProviderPlan {
    /// Records to detach from the deleting agent's sandbox (all of its own entries).
    detach: Vec<String>,
    /// Records to delete from the gateway (no surviving agent references them).
    delete: Vec<String>,
    /// record name -> surviving agent that should become the new OWNER (only when
    /// the deleting agent OWNED a record still referenced by other agents).
    rehome_owner_to: std::collections::HashMap<String, String>,
}

/// Decide the provider cascade for destroying `deleting`. `agents` is the full
/// set of (agent_name, its sandbox.providers) including the agent being deleted.
/// Pure — no gateway/fs.
fn plan_destroy_provider_cascade(
    deleting: &str,
    agents: &[(String, Vec<right_agent_config::ProviderEntry>)],
) -> DestroyProviderPlan {
    let mut plan = DestroyProviderPlan::default();
    let Some((_, deleting_providers)) = agents.iter().find(|(a, _)| a == deleting) else {
        return plan;
    };
    for entry in deleting_providers {
        plan.detach.push(entry.name.clone());
        // Other agents that still reference this record by name.
        let others: Vec<&str> = agents
            .iter()
            .filter(|(a, _)| a != deleting)
            .filter(|(_, ps)| ps.iter().any(|p| p.name == entry.name))
            .map(|(a, _)| a.as_str())
            .collect();
        if others.is_empty() {
            plan.delete.push(entry.name.clone());
        } else if entry.is_owned() {
            // Deleting agent owned a still-referenced record: re-home to a survivor.
            plan.rehome_owner_to
                .insert(entry.name.clone(), others[0].to_string());
        }
        // borrowed + others remain: the true owner is elsewhere; do nothing.
    }
    plan
}

/// Set (or clear) the `shared_from:` line for the provider named `provider` in
/// `agent`'s agent.yaml. `new_owner == None` removes the line (record becomes
/// owned); `Some(owner)` rewrites/inserts it. Comment- and field-preserving:
/// operates by line-walking the provider's block, like the rest of the platform's
/// agent.yaml editors. Best-effort caller; returns the rewritten YAML.
fn set_provider_shared_from(yaml: &str, provider: &str, new_owner: Option<&str>) -> String {
    // List items under `    providers:` are 4-space-indented `- name:` entries;
    // their fields are 6-space-indented. Match `serialize_provider_entry`.
    const FIELD_INDENT: usize = 6;

    let lines: Vec<&str> = yaml.split_inclusive('\n').collect();

    // Find the `- name: <provider>` list item (value may be single-quoted).
    let block_start = lines.iter().position(|l| {
        let Some(rest) = l.trim_end_matches(['\n', '\r']).strip_prefix("    - name:") else {
            return false;
        };
        let v = rest.trim();
        let v = v
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .unwrap_or(v);
        v == provider
    });
    let Some(block_start) = block_start else {
        // Provider not found — return unchanged (best-effort).
        return yaml.to_string();
    };

    // The block continues through more-indented lines until the next `    - `
    // list item or a dedent (line indented < FIELD_INDENT and non-blank).
    let mut block_end = lines.len();
    for (i, line) in lines.iter().enumerate().skip(block_start + 1) {
        let body = line.trim_end_matches(['\n', '\r']);
        if body.trim().is_empty() {
            // Blank line — stays part of the block (do not terminate on it).
            continue;
        }
        let indent = body.len() - body.trim_start_matches(' ').len();
        if body.starts_with("    - ") || indent < FIELD_INDENT {
            block_end = i;
            break;
        }
    }

    // Within the block, find an existing `shared_from:` field line.
    let existing = (block_start + 1..block_end).find(|&i| {
        lines[i]
            .trim_end_matches(['\n', '\r'])
            .trim_start()
            .starts_with("shared_from:")
    });

    let mut out = String::with_capacity(yaml.len() + 32);
    match (existing, new_owner) {
        (Some(idx), Some(owner)) => {
            // Replace the existing line.
            for (i, line) in lines.iter().enumerate() {
                if i == idx {
                    out.push_str(&format!(
                        "{:indent$}shared_from: '{owner}'\n",
                        "",
                        indent = FIELD_INDENT
                    ));
                } else {
                    out.push_str(line);
                }
            }
        }
        (Some(idx), None) => {
            // Drop the existing line.
            for (i, line) in lines.iter().enumerate() {
                if i != idx {
                    out.push_str(line);
                }
            }
        }
        (None, Some(owner)) => {
            // Insert at the end of the block.
            for (i, line) in lines.iter().enumerate() {
                if i == block_end {
                    out.push_str(&format!(
                        "{:indent$}shared_from: '{owner}'\n",
                        "",
                        indent = FIELD_INDENT
                    ));
                }
                out.push_str(line);
            }
            if block_end == lines.len() {
                // Block runs to EOF — append after all lines. Guard against a
                // missing trailing newline so we never glue two keys onto one
                // line (mirrors `insert_provider_entry`).
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&format!(
                    "{:indent$}shared_from: '{owner}'\n",
                    "",
                    indent = FIELD_INDENT
                ));
            }
        }
        (None, None) => {
            // Nothing to clear.
            return yaml.to_string();
        }
    }
    out
}

/// Enumerate every agent directory under `agents_dir` and load its declared
/// `sandbox.providers`. Tolerant like `build_peers`: a sibling whose directory
/// or `agent.yaml` can't be read (or has no providers) contributes an empty list
/// and is never fatal. Returns `(agent_name, providers)` pairs including the
/// agent being deleted.
fn load_agents_with_providers(
    agents_dir: &Path,
) -> Vec<(String, Vec<right_agent_config::ProviderEntry>)> {
    let entries = match std::fs::read_dir(agents_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(
                dir = %agents_dir.display(),
                error = %format!("{e:#}"),
                "could not read agents dir for provider refcount; treating as empty"
            );
            return Vec::new();
        }
    };
    let mut agents = Vec::new();
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
                Vec::new()
            }
        };
        agents.push((name, providers));
    }
    agents
}

/// Re-home ownership of `record` from the deleted owner to `new_owner` by editing
/// surviving agents' `agent.yaml`. The new owner has its `shared_from` line
/// removed (becomes owned); every OTHER surviving borrower that pointed at the
/// deleted owner is repointed to `new_owner`. Best-effort: any read/write failure
/// logs a warning and the loop continues — the record stays alive regardless, so
/// a missed edit only affects rotation-ownership display, never correctness.
fn rehome_owner_in_agent_yaml(
    agents_dir: &Path,
    record: &str,
    deleted_owner: &str,
    new_owner: &str,
    agents: &[(String, Vec<right_agent_config::ProviderEntry>)],
) {
    for (agent, providers) in agents {
        if agent == deleted_owner {
            continue; // its directory is about to be removed
        }
        // Does this agent reference `record`, and if so how?
        let Some(entry) = providers.iter().find(|p| p.name == record) else {
            continue;
        };
        // The new owner becomes owned (clear shared_from). Other borrowers that
        // pointed at the deleted owner get repointed. A borrower already pointing
        // elsewhere (a different owner) is left alone.
        let new_value: Option<&str> = if agent == new_owner {
            None
        } else if entry.shared_from.as_deref() == Some(deleted_owner) {
            Some(new_owner)
        } else {
            continue;
        };

        let yaml_path = agents_dir.join(agent).join("agent.yaml");
        let original = match std::fs::read_to_string(&yaml_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    agent = %agent,
                    record = %record,
                    error = %format!("{e:#}"),
                    "re-home: could not read agent.yaml; continuing"
                );
                continue;
            }
        };
        let rewritten = set_provider_shared_from(&original, record, new_value);
        if rewritten == original {
            continue; // no change needed
        }
        if let Err(e) = std::fs::write(&yaml_path, rewritten) {
            tracing::warn!(
                agent = %agent,
                record = %record,
                error = %format!("{e:#}"),
                "re-home: could not write agent.yaml; continuing"
            );
        } else {
            tracing::info!(
                agent = %agent,
                record = %record,
                new_owner = %new_owner,
                "re-homed provider ownership during destroy"
            );
        }
    }
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
    let is_sandboxed = config.as_ref().map(|c| c.is_sandboxed()).unwrap_or(true);

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
        let backup_path =
            run_backup(home, &options.agent_name, &agent_dir, &config, is_sandboxed).await?;
        result.backup_path = Some(backup_path);
    }

    let sandbox_name_for_cascade = if is_sandboxed {
        let explicit_sandbox_name = config
            .as_ref()
            .and_then(|c| c.sandbox.as_ref())
            .and_then(|s| s.name.as_deref());
        let sb_name = right_openshell::openshell::resolve_sandbox_name(
            &options.agent_name,
            explicit_sandbox_name,
        );
        right_openshell::openshell::delete_sandbox(&sb_name).await;
        // `delete_sandbox` is best-effort and returns `()` — it only logs on
        // CLI failure. We cannot observe success here, so report
        // `sandbox_deleted = true` as a "delete attempted" signal and rely on
        // the explicit detach in the provider cascade below to guard against
        // a silent CLI failure leaving providers attached.
        result.sandbox_deleted = true;
        Some(sb_name)
    } else {
        None
    };

    // Cascade-clean provider records on the gateway (best-effort).
    // `delete_sandbox` above SHOULD have removed attachments implicitly, but
    // it is fire-and-forget (returns `()`, logs on failure). If the CLI
    // silently failed (network blip, OpenShell down, exit code != 0), the
    // sandbox — and therefore the attachments — would still exist, and the
    // gateway would reject `DeleteProvider` with FailedPrecondition. To stay
    // self-healing per AGENTS.md, we explicitly detach each of THIS agent's
    // records first (NotFound = already detached, treat as success), mirroring
    // `handle_provider_remove`.
    //
    // With cross-agent SHARING, a single gateway record may be referenced by
    // several agents (owner declares it; borrowers declare it with
    // `shared_from`). `plan_destroy_provider_cascade` REFCOUNTS references so a
    // record is deleted ONLY when no surviving agent references it, and re-homes
    // ownership to a surviving borrower when the deleting agent owned a still-
    // referenced record. The whole block is best-effort: every failure logs and
    // continues so the agent-directory removal below still runs.
    if let Some(sandbox) = config.as_ref().and_then(|c| c.sandbox.as_ref())
        && matches!(sandbox.mode, right_agent_config::SandboxMode::Openshell)
        && !sandbox.providers.is_empty()
    {
        // Enumerate every sibling agent (including the one being deleted) and its
        // declared `sandbox.providers`. Tolerant like `build_peers`: a sibling
        // whose agent.yaml can't be read is skipped, not fatal.
        let mut agents = load_agents_with_providers(&agents_dir);

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

        let plan = plan_destroy_provider_cascade(&options.agent_name, &agents);

        let mtls_dir = right_openshell::openshell::default_mtls_dir();
        match right_openshell::openshell::connect_grpc(&mtls_dir).await {
            Ok(mut client) => {
                // Detach all of THIS agent's records from its sandbox.
                for name in &plan.detach {
                    if let Some(sb_name) = sandbox_name_for_cascade.as_deref() {
                        match right_openshell::providers::detach_from_sandbox(
                            &mut client,
                            sb_name,
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
                                sandbox = %sb_name,
                                error = %format!("{e:#}"),
                                "failed to detach provider during destroy; continuing"
                            ),
                        }
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

        // Re-home ownership of any still-referenced record the deleting agent
        // owned. The record was kept alive by the delete-guard above, so a
        // re-home failure is non-fatal (borrowers keep using it; only rotation-
        // ownership display is affected). Filesystem-only — no gateway calls.
        for (record, new_owner) in &plan.rehome_owner_to {
            rehome_owner_in_agent_yaml(
                &agents_dir,
                record,
                &options.agent_name,
                new_owner,
                &agents,
            );
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
