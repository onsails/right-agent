use std::collections::HashMap;
use std::path::Path;

use right_agent_config::{AgentDef, MemoryProvider};
use right_runtime_state::{
    AgentState, MCP_HTTP_PORT, PC_PORT, RuntimeState, generate_pc_api_token, read_state,
    write_state,
};

use crate::cloudflared::CloudflaredCredentials;
use crate::contract::{
    write_agent_owned, write_merged_rmw, write_regenerated, write_regenerated_detect_change,
};

/// Inject a secret into agent.yaml if not already present.
/// Returns the existing or newly generated secret.
fn ensure_agent_secret(
    agent_path: &Path,
    agent_name: &str,
    existing: Option<&str>,
) -> miette::Result<String> {
    if let Some(secret) = existing {
        return Ok(secret.to_owned());
    }

    let new_secret = right_mcp::generate_agent_secret();
    let yaml_path = agent_path.join("agent.yaml");

    write_merged_rmw(&yaml_path, |existing| {
        let content =
            existing.ok_or_else(|| miette::miette!("agent.yaml missing for '{agent_name}'"))?;
        let mut doc: serde_json::Map<String, serde_json::Value> = serde_saphyr::from_str(content)
            .map_err(|e| {
            miette::miette!("failed to parse agent.yaml for '{agent_name}': {e:#}")
        })?;
        doc.insert(
            "secret".to_owned(),
            serde_json::Value::String(new_secret.clone()),
        );
        serde_saphyr::to_string(&doc).map_err(|e| {
            miette::miette!("failed to serialize agent.yaml for '{agent_name}': {e:#}")
        })
    })?;
    tracing::info!(agent = %agent_name, "generated new agent secret");
    Ok(new_secret)
}

/// Run pure file generation for a single agent.
///
/// Generates per-agent settings, definitions, schemas, MCP configuration, and
/// skills. Database creation and migrations belong to guarded offline
/// initialization/restore paths or Aggregator owner startup. Bot startup may
/// call this only after the Aggregator owner is ready.
///
/// Returns the agent secret (existing or newly generated).
pub async fn run_single_agent_codegen(
    home: &Path,
    agent: &AgentDef,
    self_exe: &Path,
    debug: bool,
) -> miette::Result<String> {
    let _ = (home, self_exe, debug);

    let host_home =
        dirs::home_dir().ok_or_else(|| miette::miette!("cannot determine home directory"))?;

    // Generate .claude/settings.json with behavioral flags.
    let settings = crate::generate_settings()?;
    let claude_dir = agent.path.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| miette::miette!("failed to create .claude dir for '{}': {e:#}", agent.name))?;

    // Write reply-schema.json.
    write_regenerated(
        &claude_dir.join("reply-schema.json"),
        crate::REPLY_SCHEMA_JSON,
    )?;

    // Write cron-schema.json.
    write_regenerated(
        &claude_dir.join("cron-schema.json"),
        crate::CRON_SCHEMA_JSON,
    )?;

    // Every agent runs sandboxed; the guest home is always /sandbox.
    let home_dir = "/sandbox";

    // Write system-prompt.md (base identity for --system-prompt-file).
    write_regenerated(
        &claude_dir.join("system-prompt.md"),
        &crate::generate_system_prompt(&agent.name, home_dir),
    )?;

    // Write bootstrap-schema.json (bootstrap mode structured output).
    write_regenerated(
        &claude_dir.join("bootstrap-schema.json"),
        crate::BOOTSTRAP_SCHEMA_JSON,
    )?;

    tracing::debug!(agent = %agent.name, "wrote schemas");

    // Pre-create shell-snapshots dir so CC Bash tool doesn't error on first run.
    std::fs::create_dir_all(claude_dir.join("shell-snapshots")).map_err(|e| {
        miette::miette!(
            "failed to create shell-snapshots dir for '{}': {e:#}",
            agent.name
        )
    })?;
    let settings_json = serde_json::to_string_pretty(&settings)
        .map_err(|e| miette::miette!("failed to serialize settings for '{}': {e:#}", agent.name))?;
    write_regenerated(&claude_dir.join("settings.json"), &settings_json)?;
    tracing::debug!(agent = %agent.name, "wrote settings.json");

    // Generate per-agent .claude.json with trust entries.
    crate::generate_agent_claude_json(agent)?;

    // Create credential symlink for OAuth under HOME override.
    crate::create_credential_symlink(agent, &host_home)?;

    // git init if .git/ missing. Non-fatal: log warning and continue if git binary absent.
    if !agent.path.join(".git").exists() {
        match std::process::Command::new("git")
            .arg("init")
            .current_dir(&agent.path)
            .status()
        {
            Ok(s) if s.success() => {
                tracing::debug!(agent = %agent.name, "git init done");
            }
            Ok(s) => {
                tracing::warn!(agent = %agent.name, "git init exited with status {}", s);
            }
            Err(e) => {
                tracing::warn!(agent = %agent.name, "git binary not found, skipping git init: {e}");
            }
        }
    }

    // Reinstall built-in skills (remove stale dirs, overwrite built-in, preserve user dirs).
    let _ = std::fs::remove_dir_all(agent.path.join(".claude/skills/clawhub"));
    let _ = std::fs::remove_dir_all(agent.path.join(".claude/skills/skills"));
    let memory_provider = agent
        .config
        .as_ref()
        .and_then(|c| c.memory.as_ref())
        .map(|m| &m.provider)
        .unwrap_or(&MemoryProvider::File);
    crate::install_builtin_skills(&agent.path, memory_provider)?;

    // Write settings.local.json only if absent (CC may write runtime state here).
    write_agent_owned(&claude_dir.join("settings.local.json"), "{}")?;

    // Ensure agent has a persistent secret for token derivation.
    let existing_secret = agent.config.as_ref().and_then(|c| c.secret.as_deref());
    let agent_secret = ensure_agent_secret(&agent.path, &agent.name, existing_secret)?;

    // Generate mcp.json with right HTTP MCP server entry.
    let mcp_port = MCP_HTTP_PORT;
    let bearer_token = right_mcp::derive_token(&agent_secret, "right-mcp")?;
    let right_mcp_url = format!("http://host.microsandbox.internal:{mcp_port}/mcp");
    crate::generate_mcp_config_http(&agent.path, &agent.name, &right_mcp_url, &bearer_token)?;
    tracing::debug!(agent = %agent.name, "wrote mcp.json with right HTTP MCP entry");

    Ok(agent_secret)
}

/// Observable effects from cross-agent codegen.
///
/// Callers that already have a running process-compose instance use this to
/// restart long-lived processes that do not hot-reload rewritten files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodegenOutcome {
    pub cloudflared_config_changed: bool,
}

/// Run cross-agent runtime codegen.
///
/// Generates agent-tokens.json, process-compose.yaml, cloudflared config, and
/// runtime state. Per-agent codegen is handled by the bot at startup via
/// `run_single_agent_codegen()`.
///
/// - `all_agents`: all discovered agents
/// - `self_exe`: path to the right binary (used in process-compose.yaml)
/// - `debug`: enable debug-level process-compose logging
pub fn run_agent_codegen(
    home: &Path,
    all_agents: &[AgentDef],
    self_exe: &Path,
    debug: bool,
) -> miette::Result<CodegenOutcome> {
    run_agent_codegen_inner(home, all_agents, self_exe, debug, true)
}

/// Run cross-agent configuration codegen without publishing runtime state.
///
/// Offline initialization uses this entry point because generating runtime
/// configuration does not mean process-compose has been started.
pub fn run_agent_codegen_for_init(
    home: &Path,
    all_agents: &[AgentDef],
    self_exe: &Path,
    debug: bool,
) -> miette::Result<CodegenOutcome> {
    run_agent_codegen_inner(home, all_agents, self_exe, debug, false)
}

fn run_agent_codegen_inner(
    home: &Path,
    all_agents: &[AgentDef],
    self_exe: &Path,
    debug: bool,
    publish_runtime_state: bool,
) -> miette::Result<CodegenOutcome> {
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| miette::miette!("failed to create run directory: {e:#}"))?;

    let global_cfg = right_config::read_global_config(home)?;
    let mut outcome = CodegenOutcome::default();

    // Resolve agent secrets for token map.
    // Per-agent codegen is now done by the bot at startup (run_single_agent_codegen).
    // On first `up` secrets may not exist yet — generate if missing.
    let mut generated_secrets: HashMap<String, String> = HashMap::new();
    for agent in all_agents {
        let existing = agent.config.as_ref().and_then(|c| c.secret.as_deref());
        let secret = ensure_agent_secret(&agent.path, &agent.name, existing)?;
        generated_secrets.insert(agent.name.clone(), secret);
    }

    // Write agent token map for the HTTP MCP server process.
    let mut token_map_entries = serde_json::Map::new();
    for agent in all_agents {
        let secret = generated_secrets.get(&agent.name).ok_or_else(|| {
            miette::miette!("agent '{}' has no secret after resolution", agent.name)
        })?;
        let token = right_mcp::derive_token(secret, "right-mcp")?;
        token_map_entries.insert(agent.name.clone(), serde_json::Value::String(token));
    }
    let token_map_path = run_dir.join("agent-tokens.json");
    let token_map_json =
        serde_json::to_string_pretty(&serde_json::Value::Object(token_map_entries))
            .map_err(|e| miette::miette!("failed to serialize token map: {e:#}"))?;
    write_regenerated(&token_map_path, &token_map_json)?;
    tracing::debug!("wrote agent-tokens.json");

    // Generate cloudflared config and wrapper script — only when the operator
    // selected `provider: cloudflared`. For `provider: external`, the operator
    // owns the public HTTPS front (e.g. their own caddy/nginx), so we skip
    // codegen and never add a cloudflared process to process-compose.
    let tunnel_cfg = &global_cfg.tunnel;
    let cloudflared_script_path: Option<std::path::PathBuf> = match &tunnel_cfg.provider {
        right_config::TunnelProvider::Cloudflared {
            tunnel_uuid,
            credentials_file,
        } => {
            which::which("cloudflared").map_err(|_| {
                miette::miette!(
                    "TunnelConfig is present but `cloudflared` is not in PATH -- install cloudflared first, or set `tunnel.provider: external` to bring your own reverse proxy"
                )
            })?;
            if !credentials_file.exists() {
                return Err(miette::miette!(
                    help = "Run `right config set` and select Tunnel -- choose \"Delete and recreate\" to generate new credentials on this machine",
                    "Tunnel credentials file not found: {}\n\n  \
                     This usually means the tunnel was created on a different machine,\n  \
                     or `right init` was re-run after the credentials file was removed.",
                    credentials_file.display()
                ));
            }

            let agent_pairs: Vec<(String, std::path::PathBuf)> = all_agents
                .iter()
                .map(|a| (a.name.clone(), a.path.clone()))
                .collect();

            let creds = CloudflaredCredentials {
                tunnel_uuid: tunnel_uuid.clone(),
                credentials_file: credentials_file.clone(),
            };

            let cf_config = crate::cloudflared::generate_cloudflared_config(
                &agent_pairs,
                &tunnel_cfg.hostname,
                &creds,
            )?;
            let cf_config_path = home.join("cloudflared-config.yml");
            outcome.cloudflared_config_changed =
                write_regenerated_detect_change(&cf_config_path, &cf_config)?;
            tracing::info!(path = %cf_config_path.display(), "cloudflared config written");

            // Write DNS routing wrapper script.
            let scripts_dir = home.join("scripts");
            std::fs::create_dir_all(&scripts_dir)
                .map_err(|e| miette::miette!("create scripts dir: {e:#}"))?;
            let hostname = &tunnel_cfg.hostname;
            let cf_config_path_str = cf_config_path.display();
            let script_content = format!(
                "#!/bin/sh\ncloudflared tunnel route dns --overwrite-dns {tunnel_uuid} {hostname} || true\nexec cloudflared tunnel --config {cf_config_path_str} run\n"
            );
            let script_path = scripts_dir.join("cloudflared-start.sh");
            write_regenerated(&script_path, &script_content)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700))
                    .map_err(|e| miette::miette!("chmod cloudflared-start.sh: {e:#}"))?;
            }
            tracing::info!(path = %script_path.display(), "cloudflared wrapper script written");
            Some(script_path)
        }
        right_config::TunnelProvider::External => {
            tracing::info!(
                hostname = %tunnel_cfg.hostname,
                "tunnel provider is `external` — skipping cloudflared codegen"
            );
            None
        }
    };

    // Generate process-compose.yaml.
    let pc_config = crate::generate_process_compose(
        all_agents,
        self_exe,
        &crate::ProcessComposeConfig {
            debug,
            home,
            cloudflared_script: cloudflared_script_path.as_deref(),
            token_map_path: Some(&token_map_path),
        },
    )?;
    let config_path = run_dir.join("process-compose.yaml");
    write_regenerated(&config_path, &pc_config)?;
    tracing::debug!("wrote process-compose config: {}", config_path.display());

    if publish_runtime_state {
        // Preserve started_at and pc_api_token on reload: both must stay
        // consistent with the running process-compose instance.
        let state_path = run_dir.join("state.json");
        let socket_path = run_dir.join("pc.sock");
        let existing = read_state(&state_path).ok();
        let started_at = existing
            .as_ref()
            .map(|s| s.started_at.clone())
            .unwrap_or_else(|| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                format!("{}Z", now.as_secs())
            });
        let pc_api_token = existing
            .and_then(|s| s.pc_api_token)
            .unwrap_or_else(generate_pc_api_token);
        let state = RuntimeState {
            agents: all_agents
                .iter()
                .map(|a| AgentState {
                    name: a.name.clone(),
                })
                .collect(),
            socket_path: socket_path.display().to_string(),
            started_at,
            pc_port: PC_PORT,
            pc_api_token: Some(pc_api_token),
        };
        write_state(&state, &state_path)?;
    }

    Ok(outcome)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use right_agent_config::AgentConfig;

    /// Write a minimal valid `config.yaml` (with tunnel block) into the given
    /// home directory. Defaults to `tunnel.provider: external` so tests do not
    /// require a `cloudflared` binary in `PATH` and exercise the no-cloudflared
    /// codegen path. Tests that specifically need the cloudflared path call
    /// [`write_minimal_global_config_cloudflared`] instead.
    pub(crate) fn write_minimal_global_config(home: &std::path::Path) {
        use std::fs;
        let yaml = "tunnel:\n  provider: \"external\"\n  hostname: \"test.example.com\"\n";
        fs::write(home.join("config.yaml"), yaml).unwrap();
    }

    /// Like [`write_minimal_global_config`], but writes a cloudflared-mode
    /// config. Tests using this helper must be skipped or no-op when
    /// `cloudflared` is not on `PATH` — otherwise codegen will error out.
    #[allow(dead_code)]
    pub(crate) fn write_minimal_global_config_cloudflared(home: &std::path::Path) {
        use std::fs;
        let creds_path = home.join("test-creds.json");
        fs::write(&creds_path, "{}").unwrap();
        let yaml = format!(
            "tunnel:\n  provider: \"cloudflared\"\n  tunnel_uuid: \"00000000-0000-0000-0000-000000000000\"\n  credentials_file: \"{}\"\n  hostname: \"test.example.com\"\n",
            creds_path.display()
        );
        fs::write(home.join("config.yaml"), yaml).unwrap();
    }

    fn agent_fixture(agent_dir: &Path) -> AgentDef {
        let name = agent_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let config = std::fs::read_to_string(agent_dir.join("agent.yaml"))
            .ok()
            .map(|yaml| serde_saphyr::from_str::<AgentConfig>(&yaml).unwrap());

        AgentDef {
            name,
            path: agent_dir.to_path_buf(),
            identity_path: agent_dir.join("IDENTITY.md"),
            config,
            soul_path: None,
            user_path: None,
            tools_path: agent_dir
                .join("TOOLS.md")
                .exists()
                .then(|| agent_dir.join("TOOLS.md")),
            bootstrap_path: None,
            heartbeat_path: None,
        }
    }

    #[tokio::test]
    async fn run_single_agent_codegen_generates_all_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        let agent_dir = home.join("agents").join("test");
        std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "restart: never\nnetwork_policy: permissive\n",
        )
        .unwrap();

        let agent = agent_fixture(&agent_dir);
        let self_exe = std::path::PathBuf::from("/usr/bin/right");

        run_single_agent_codegen(home, &agent, &self_exe, false)
            .await
            .unwrap();

        // Core files must exist
        assert!(agent_dir.join(".claude/settings.json").exists());
        assert!(agent_dir.join(".claude/system-prompt.md").exists());
        assert!(agent_dir.join(".claude/reply-schema.json").exists());
        assert!(agent_dir.join(".claude/cron-schema.json").exists());
        assert!(agent_dir.join(".claude/bootstrap-schema.json").exists());
        assert!(agent_dir.join("mcp.json").exists());
        assert!(
            !agent_dir.join("policy.yaml").exists(),
            "OpenShell policy files are retired; codegen must not write one"
        );
    }

    const OWNER_AGENT_DIR_ENV: &str = "RIGHT_CODEGEN_TEST_OWNER_AGENT_DIR";
    const OWNER_READY_ENV: &str = "RIGHT_CODEGEN_TEST_OWNER_READY";
    const OWNER_RELEASE_ENV: &str = "RIGHT_CODEGEN_TEST_OWNER_RELEASE";
    const OWNER_USABLE_ENV: &str = "RIGHT_CODEGEN_TEST_OWNER_USABLE";
    const OWNER_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
    const OWNER_WAIT_DELAY: std::time::Duration = std::time::Duration::from_millis(10);

    struct ChildGuard(Option<std::process::Child>);

    impl ChildGuard {
        fn spawn(command: &mut std::process::Command) -> Self {
            Self(Some(command.spawn().unwrap()))
        }

        fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
            self.0.as_mut().unwrap().try_wait().unwrap()
        }

        fn wait(mut self) -> std::process::ExitStatus {
            self.0.take().unwrap().wait().unwrap()
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let Some(child) = self.0.as_mut() else {
                return;
            };
            if child.try_wait().unwrap().is_none() {
                child.kill().unwrap();
                child.wait().unwrap();
            }
        }
    }

    #[tokio::test]
    async fn database_owner_child() {
        let Some(agent_dir) = std::env::var_os(OWNER_AGENT_DIR_ENV) else {
            return;
        };
        let ready = std::path::PathBuf::from(std::env::var_os(OWNER_READY_ENV).unwrap());
        let release = std::path::PathBuf::from(std::env::var_os(OWNER_RELEASE_ENV).unwrap());
        let usable = std::path::PathBuf::from(std::env::var_os(OWNER_USABLE_ENV).unwrap());

        let owner = right_db::open_connection(Path::new(&agent_dir), true)
            .await
            .unwrap();
        std::fs::write(&ready, []).unwrap();
        let deadline = tokio::time::Instant::now() + OWNER_WAIT_TIMEOUT;
        while !release.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting to release database owner"
            );
            tokio::time::sleep(OWNER_WAIT_DELAY).await;
        }

        let value: i64 = owner
            .query_one("SELECT 1", [], |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(value, 1);
        std::fs::write(usable, []).unwrap();
    }

    #[tokio::test]
    async fn run_single_agent_codegen_succeeds_with_live_database_owner() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        let agent_dir = home.join("agents").join("test");
        std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "restart: never\nnetwork_policy: permissive\n",
        )
        .unwrap();

        let ready = home.join("owner-ready");
        let release = home.join("owner-release");
        let usable = home.join("owner-usable");
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .arg("pipeline::tests::database_owner_child")
            .arg("--exact")
            .arg("--nocapture")
            .env(OWNER_AGENT_DIR_ENV, &agent_dir)
            .env(OWNER_READY_ENV, &ready)
            .env(OWNER_RELEASE_ENV, &release)
            .env(OWNER_USABLE_ENV, &usable);
        let mut owner = ChildGuard::spawn(&mut command);
        let deadline = tokio::time::Instant::now() + OWNER_WAIT_TIMEOUT;
        while !ready.exists() {
            assert!(
                owner.try_wait().is_none(),
                "database owner child exited before publishing readiness"
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for database owner"
            );
            tokio::time::sleep(OWNER_WAIT_DELAY).await;
        }

        let agent = agent_fixture(&agent_dir);
        let self_exe = std::path::PathBuf::from("/usr/bin/right");
        let codegen_result = run_single_agent_codegen(home, &agent, &self_exe, false).await;

        std::fs::write(&release, []).unwrap();
        let owner_status = owner.wait();
        assert!(owner_status.success(), "database owner child failed");
        assert!(
            usable.exists(),
            "the retained database owner became unusable"
        );
        codegen_result.unwrap();
        assert!(agent_dir.join(".claude/settings.json").exists());
        assert!(agent_dir.join("mcp.json").exists());
    }

    // The `run_single_agent_codegen_*_policy` and `*_custom_policy_file_path`
    // tests are gone with policy.yaml codegen itself: the microsandbox VM
    // carries its egress policy as typed sandbox spec, not a generated file.

    #[tokio::test]
    async fn run_agent_codegen_with_empty_agents() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        write_minimal_global_config(home);
        let self_exe = std::path::PathBuf::from("/usr/bin/right");
        let result = run_agent_codegen(home, &[], &self_exe, false);
        assert!(result.is_ok(), "empty agents should succeed: {result:?}");
        // run_dir should have been created
        assert!(home.join("run").exists());
        // process-compose.yaml should exist
        assert!(home.join("run/process-compose.yaml").exists());
        // state.json should exist
        assert!(home.join("run/state.json").exists());
    }

    #[tokio::test]
    async fn init_codegen_does_not_publish_runtime_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        write_minimal_global_config(home);
        let self_exe = std::path::PathBuf::from("/usr/bin/right");

        run_agent_codegen_for_init(home, &[], &self_exe, false).unwrap();

        assert!(home.join("run/process-compose.yaml").exists());
        assert!(!home.join("run/state.json").exists());
    }

    #[tokio::test]
    async fn run_agent_codegen_accepts_legacy_agent_without_policy() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        write_minimal_global_config(home);

        let agent_dir = home.join("agents").join("legacy");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "restart: never\nnetwork_policy: permissive\ntelegram_token: 123:test\n",
        )
        .unwrap();

        let agent = agent_fixture(&agent_dir);
        let self_exe = std::path::PathBuf::from("/usr/bin/right");
        run_agent_codegen(home, std::slice::from_ref(&agent), &self_exe, false).unwrap();

        let pc_yaml = std::fs::read_to_string(home.join("run/process-compose.yaml")).unwrap();
        assert!(
            !pc_yaml.contains("RC_SANDBOX_POLICY"),
            "policy paths are no longer threaded into process-compose: {pc_yaml}"
        );

        run_single_agent_codegen(home, &agent, &self_exe, false)
            .await
            .unwrap();
        assert!(
            !agent_dir.join("policy.yaml").exists(),
            "per-agent codegen must not resurrect a policy file"
        );
    }

    #[tokio::test]
    async fn run_agent_codegen_reports_new_cloudflared_config_changed() {
        if which::which("cloudflared").is_err() {
            eprintln!("skip: cloudflared not on PATH");
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        write_minimal_global_config_cloudflared(home);

        let agent_dir = home.join("agents").join("test");
        std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "restart: never\nnetwork_policy: permissive\n",
        )
        .unwrap();

        let agent = agent_fixture(&agent_dir);
        let self_exe = std::path::PathBuf::from("/usr/bin/right");

        let outcome =
            run_agent_codegen(home, std::slice::from_ref(&agent), &self_exe, false).unwrap();

        assert!(
            outcome.cloudflared_config_changed,
            "first cloudflared config write must be reported as changed"
        );
    }

    #[tokio::test]
    async fn run_agent_codegen_reports_unchanged_cloudflared_config() {
        if which::which("cloudflared").is_err() {
            eprintln!("skip: cloudflared not on PATH");
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        write_minimal_global_config_cloudflared(home);

        let agent_dir = home.join("agents").join("test");
        std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "restart: never\nnetwork_policy: permissive\n",
        )
        .unwrap();

        let agent = agent_fixture(&agent_dir);
        let self_exe = std::path::PathBuf::from("/usr/bin/right");

        let first =
            run_agent_codegen(home, std::slice::from_ref(&agent), &self_exe, false).unwrap();
        let second =
            run_agent_codegen(home, std::slice::from_ref(&agent), &self_exe, false).unwrap();

        assert!(first.cloudflared_config_changed);
        assert!(
            !second.cloudflared_config_changed,
            "second identical cloudflared config write must not be reported as changed"
        );
    }

    #[tokio::test]
    async fn run_agent_codegen_external_provider_skips_cloudflared_codegen() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        // External provider needs no cloudflared in PATH.
        write_minimal_global_config(home);

        let agent_dir = home.join("agents").join("test");
        std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "restart: never\nnetwork_policy: permissive\n",
        )
        .unwrap();

        let agent = agent_fixture(&agent_dir);
        let self_exe = std::path::PathBuf::from("/usr/bin/right");

        let outcome =
            run_agent_codegen(home, std::slice::from_ref(&agent), &self_exe, false).unwrap();

        assert!(
            !outcome.cloudflared_config_changed,
            "external provider must not report a cloudflared config change"
        );
        assert!(
            !home.join("cloudflared-config.yml").exists(),
            "external provider must not write cloudflared-config.yml"
        );
        assert!(
            !home.join("scripts").join("cloudflared-start.sh").exists(),
            "external provider must not write cloudflared-start.sh"
        );

        let pc_yaml = std::fs::read_to_string(home.join("run/process-compose.yaml")).unwrap();
        assert!(
            !pc_yaml.contains("cloudflared:"),
            "process-compose.yaml must not contain cloudflared process: {pc_yaml}"
        );
    }

    #[tokio::test]
    async fn tools_md_not_overwritten_if_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        let agent_dir = home.join("agents").join("test");
        std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "restart: never\nnetwork_policy: permissive\n",
        )
        .unwrap();
        // Write custom TOOLS.md before codegen
        let custom_content = "# My Custom Tools\n\nDo not overwrite me.\n";
        std::fs::write(agent_dir.join("TOOLS.md"), custom_content).unwrap();

        let agent = agent_fixture(&agent_dir);
        let self_exe = std::path::PathBuf::from("/usr/bin/right");
        run_single_agent_codegen(home, &agent, &self_exe, false)
            .await
            .unwrap();

        let after = std::fs::read_to_string(agent_dir.join("TOOLS.md")).unwrap();
        assert_eq!(after, custom_content, "TOOLS.md must not be overwritten");
    }

    #[tokio::test]
    async fn tools_md_not_created_by_codegen_if_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        let agent_dir = home.join("agents").join("test");
        std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "restart: never\nnetwork_policy: permissive\n",
        )
        .unwrap();
        // No TOOLS.md before codegen
        assert!(!agent_dir.join("TOOLS.md").exists());

        let agent = agent_fixture(&agent_dir);
        let self_exe = std::path::PathBuf::from("/usr/bin/right");
        run_single_agent_codegen(home, &agent, &self_exe, false)
            .await
            .unwrap();

        // Codegen no longer creates TOOLS.md — that's init's responsibility
        assert!(
            !agent_dir.join("TOOLS.md").exists(),
            "codegen must not create TOOLS.md"
        );
    }
}
