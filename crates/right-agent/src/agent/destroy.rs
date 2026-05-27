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

    // Cascade-delete provider entries from the gateway (best-effort).
    // Failure is logged but non-fatal — destroy proceeds regardless.
    if let Some(sandbox) = config.as_ref().and_then(|c| c.sandbox.as_ref()) {
        if matches!(sandbox.mode, right_agent_config::SandboxMode::Openshell)
            && !sandbox.providers.is_empty()
        {
            let mtls_dir = right_openshell::openshell::default_mtls_dir();
            match right_openshell::openshell::connect_grpc(&mtls_dir).await {
                Ok(mut client) => {
                    for entry in &sandbox.providers {
                        if let Err(e) =
                            right_openshell::providers::delete_provider(&mut client, &entry.name)
                                .await
                        {
                            tracing::warn!(
                                name = %entry.name,
                                error = %format!("{e:#}"),
                                "failed to delete provider during destroy; continuing"
                            );
                        }
                    }
                }
                Err(e) => tracing::warn!(
                    error = %format!("{e:#}"),
                    "could not connect to openshell gateway for provider cleanup; continuing destroy"
                ),
            }
        }
    }

    if is_sandboxed {
        let explicit_sandbox_name = config
            .as_ref()
            .and_then(|c| c.sandbox.as_ref())
            .and_then(|s| s.name.as_deref());
        let sb_name = right_openshell::openshell::resolve_sandbox_name(
            &options.agent_name,
            explicit_sandbox_name,
        );
        right_openshell::openshell::delete_sandbox(&sb_name).await;
        result.sandbox_deleted = true;
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
mod tests {
    use super::*;

    fn tar_entries(path: &Path) -> Vec<String> {
        let output = std::process::Command::new("tar")
            .args(["-tzf", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "tar -tzf failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn destroy_nonsandboxed_agent_removes_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        let agents_dir = home.join("agents").join("test-agent");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();

        let options = DestroyOptions {
            agent_name: "test-agent".into(),
            backup: false,
        };

        let result = destroy_agent(home, &options).await.unwrap();

        assert!(
            !result.agent_stopped,
            "PC not running, should not have stopped"
        );
        assert!(
            !result.sandbox_deleted,
            "non-sandboxed agent, no sandbox to delete"
        );
        assert!(result.backup_path.is_none());
        assert!(result.dir_removed);
        assert!(
            !result.pc_reloaded,
            "PC not running, should not have reloaded"
        );
        assert!(!agents_dir.exists(), "agent dir should be deleted");
    }

    #[tokio::test]
    async fn destroy_nonexistent_agent_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("agents")).unwrap();

        let options = DestroyOptions {
            agent_name: "nonexistent".into(),
            backup: false,
        };

        let result = destroy_agent(home, &options).await;
        assert!(result.is_err());
    }

    /// Guards the `--home` isolation invariant: when `<home>/run/state.json`
    /// does not exist, `destroy_agent` must not touch process-compose at all.
    /// `PcClient::from_home` is the only public constructor, so there is no
    /// way for destroy to contact the user's live PC from an isolated home.
    #[tokio::test]
    async fn destroy_skips_pc_when_no_runtime_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        let agents_dir = home.join("agents").join("isolated");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();

        // No <home>/run/state.json exists.
        assert!(!home.join("run").join("state.json").exists());

        let options = DestroyOptions {
            agent_name: "isolated".into(),
            backup: false,
        };

        let result = destroy_agent(home, &options).await.unwrap();

        assert!(
            !result.agent_stopped,
            "no runtime state → PC skipped → agent not stopped"
        );
        assert!(
            !result.pc_reloaded,
            "no runtime state → PC skipped → not reloaded"
        );
        assert!(result.dir_removed, "agent dir should still be removed");
        assert!(!agents_dir.exists(), "agent dir should be deleted");
    }

    #[tokio::test]
    async fn destroy_with_backup_creates_backup_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        let agents_dir = home.join("agents").join("backup-test");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
        std::fs::write(agents_dir.join("IDENTITY.md"), "# Test agent").unwrap();

        let options = DestroyOptions {
            agent_name: "backup-test".into(),
            backup: true,
        };

        let result = destroy_agent(home, &options).await.unwrap();

        assert!(
            result.backup_path.is_some(),
            "backup should have been created"
        );
        let backup_path = result.backup_path.unwrap();
        assert!(backup_path.exists(), "backup dir should exist");
        assert!(
            backup_path.join("sandbox.tar.gz").exists(),
            "sandbox.tar.gz should exist"
        );
        assert!(
            result.dir_removed,
            "agent dir should be removed after backup"
        );
    }

    #[tokio::test]
    async fn destroy_with_backup_excludes_database_sidecars_from_no_sandbox_tar() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        let agents_dir = home.join("agents").join("backup-sidecars");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
        std::fs::write(agents_dir.join("notes.txt"), "keep me").unwrap();
        for sidecar in [
            "data.db-wal",
            "data.db-shm",
            "data.db-tshm",
            "data.db-future",
        ] {
            std::fs::write(agents_dir.join(sidecar), sidecar).unwrap();
        }

        let options = DestroyOptions {
            agent_name: "backup-sidecars".into(),
            backup: true,
        };

        let result = destroy_agent(home, &options).await.unwrap();
        let backup_path = result.backup_path.expect("backup path must be recorded");
        let entries = tar_entries(&backup_path.join("sandbox.tar.gz"));

        assert!(
            entries.contains(&"backup-sidecars/notes.txt".to_string()),
            "regular no-sandbox files should still be archived"
        );
        for sidecar in [
            "data.db-wal",
            "data.db-shm",
            "data.db-tshm",
            "data.db-future",
        ] {
            assert!(
                !entries.contains(&format!("backup-sidecars/{sidecar}")),
                "pre-destroy no-sandbox backup tar must not contain database sidecar {sidecar}"
            );
        }
    }

    #[tokio::test]
    async fn destroy_with_backup_vacuum_copies_data_db() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        let agents_dir = home.join("agents").join("backup-db");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
        let conn = right_db::open_connection(&agents_dir, true).await.unwrap();
        conn.execute(
            "INSERT INTO auth_tokens (token) VALUES (?1)",
            right_db::params!["token-for-backup"],
        )
        .await
        .unwrap();
        drop(conn);

        let options = DestroyOptions {
            agent_name: "backup-db".into(),
            backup: true,
        };

        let result = destroy_agent(home, &options).await.unwrap();
        let backup_path = result.backup_path.expect("backup path must be recorded");
        let backup_conn = right_db::open_database_path_readonly(backup_path.join("data.db"))
            .await
            .expect("backup database must be readable");
        let count: i64 = backup_conn
            .query_row("SELECT COUNT(*) FROM auth_tokens", (), |row| row.get(0))
            .await
            .unwrap();

        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn destroy_with_backup_copies_allowlist_yaml() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();

        let agents_dir = home.join("agents").join("backup-allowlist");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
        let allowlist = "\
version: 1
users:
  - id: 111
    label: alice
    added_by: null
    added_at: 2026-05-16T12:00:00Z
groups:
  - id: -222
    label: ops
    opened_by: null
    opened_at: 2026-05-16T12:00:00Z
";
        std::fs::write(agents_dir.join("allowlist.yaml"), allowlist).unwrap();

        let options = DestroyOptions {
            agent_name: "backup-allowlist".into(),
            backup: true,
        };

        let result = destroy_agent(home, &options).await.unwrap();
        let backup_path = result.backup_path.expect("backup path must be recorded");

        assert_eq!(
            std::fs::read_to_string(backup_path.join("allowlist.yaml")).unwrap(),
            allowlist,
            "pre-destroy backup must preserve allowlist.yaml outside sandbox.tar.gz"
        );
    }

    /// Guards that `sandbox.providers` in `agent.yaml` parses correctly for
    /// both built-in and generic entries. This is the property a backup/restore
    /// cycle depends on: the field must not be silently dropped when the YAML
    /// is written to a backup tarball and re-read on restore.
    #[test]
    fn sandbox_providers_round_trip_parse() {
        let yaml = r#"
sandbox:
  mode: none
  providers:
    - name: foo-anthropic
      type: anthropic
      label: anthropic
    - name: foo-acme
      type: generic
      label: acme
      generic:
        env_var: ACME_TOKEN
        header_name: X-Acme-Token
        upstream_host: api.acme.com
        upstream_path_prefix: /v1
"#;
        // Parse once — both entries must be present.
        let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = cfg.sandbox.as_ref().expect("sandbox must be present");
        assert_eq!(
            sandbox.providers.len(),
            2,
            "expected 2 providers after parse"
        );
        assert_eq!(sandbox.providers[0].name, "foo-anthropic");
        assert_eq!(sandbox.providers[1].name, "foo-acme");

        // Parse again from the same source — simulates reading the backed-up agent.yaml.
        // AgentConfig does not derive Serialize so we re-parse the original YAML string;
        // this is identical to what backup/restore does (copy the file, re-read it).
        let reparsed: right_agent_config::AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let reparsed_sandbox = reparsed.sandbox.expect("sandbox must survive re-parse");
        assert_eq!(
            reparsed_sandbox.providers.len(),
            2,
            "providers must survive backup/restore re-parse"
        );
        assert_eq!(reparsed_sandbox.providers[0].name, "foo-anthropic");
        assert_eq!(reparsed_sandbox.providers[1].name, "foo-acme");

        // Verify generic entry fields survived.
        let generic = reparsed_sandbox.providers[1]
            .generic
            .as_ref()
            .expect("second provider must have generic config");
        assert_eq!(generic.env_var, "ACME_TOKEN");
        assert_eq!(generic.upstream_host, "api.acme.com");
    }
}
