use std::collections::HashMap;
use std::path::Path;

use crate::agent::types::{AgentDef, SandboxMode};
use crate::codegen::cloudflared::CloudflaredCredentials;

/// Inject a secret into agent.yaml if not already present.
/// Returns the existing or newly generated secret.
fn ensure_agent_secret(agent_path: &Path, agent_name: &str, existing: Option<&str>) -> miette::Result<String> {
    if let Some(secret) = existing {
        return Ok(secret.to_owned());
    }

    let new_secret = crate::mcp::generate_agent_secret();
    let yaml_path = agent_path.join("agent.yaml");
    let yaml_content = std::fs::read_to_string(&yaml_path).map_err(|e| {
        miette::miette!("failed to read agent.yaml for '{agent_name}': {e:#}")
    })?;
    let mut doc: serde_json::Map<String, serde_json::Value> =
        serde_saphyr::from_str(&yaml_content).map_err(|e| {
            miette::miette!("failed to parse agent.yaml for '{agent_name}': {e:#}")
        })?;
    doc.insert(
        "secret".to_owned(),
        serde_json::Value::String(new_secret.clone()),
    );
    let updated_yaml = serde_saphyr::to_string(&doc).map_err(|e| {
        miette::miette!("failed to serialize agent.yaml for '{agent_name}': {e:#}")
    })?;
    std::fs::write(&yaml_path, &updated_yaml).map_err(|e| {
        miette::miette!("failed to write agent secret for '{agent_name}': {e:#}")
    })?;
    tracing::info!(agent = %agent_name, "generated new agent secret");
    Ok(new_secret)
}

/// Run codegen for a single agent.
///
/// Generates all per-agent artifacts: settings, agent definitions, schemas,
/// .claude.json, mcp.json, TOOLS.md, skills, memory.db, policy.yaml.
/// Called by the bot at startup. Also used by `rightclaw init` and `rightclaw agent init`.
///
/// Returns the agent secret (existing or newly generated).
pub fn run_single_agent_codegen(
    home: &Path,
    agent: &AgentDef,
    self_exe: &Path,
    debug: bool,
) -> miette::Result<String> {
    let _ = (home, self_exe, debug);

    let host_home =
        dirs::home_dir().ok_or_else(|| miette::miette!("cannot determine home directory"))?;

    // Resolve sandbox mode early — used by system-prompt, mcp, tools.
    let agent_sandbox_mode = agent
        .config
        .as_ref()
        .map(|c| c.sandbox_mode().clone())
        .unwrap_or_default();

    // Generate .claude/settings.json with behavioral flags.
    let settings = crate::codegen::generate_settings()?;
    let claude_dir = agent.path.join(".claude");
    std::fs::create_dir_all(&claude_dir).map_err(|e| {
        miette::miette!("failed to create .claude dir for '{}': {e:#}", agent.name)
    })?;

    // Write reply-schema.json.
    std::fs::write(
        claude_dir.join("reply-schema.json"),
        crate::codegen::REPLY_SCHEMA_JSON,
    )
    .map_err(|e| {
        miette::miette!(
            "failed to write reply-schema.json for '{}': {e:#}",
            agent.name
        )
    })?;

    // Write cron-schema.json.
    std::fs::write(
        claude_dir.join("cron-schema.json"),
        crate::codegen::CRON_SCHEMA_JSON,
    )
    .map_err(|e| {
        miette::miette!(
            "failed to write cron-schema.json for '{}': {e:#}",
            agent.name
        )
    })?;

    // Write system-prompt.md (base identity for --system-prompt-file).
    std::fs::write(
        claude_dir.join("system-prompt.md"),
        crate::codegen::generate_system_prompt(&agent.name, &agent_sandbox_mode),
    )
    .map_err(|e| {
        miette::miette!(
            "failed to write system-prompt.md for '{}': {e:#}",
            agent.name
        )
    })?;

    // Write bootstrap-schema.json (bootstrap mode structured output).
    std::fs::write(
        claude_dir.join("bootstrap-schema.json"),
        crate::codegen::BOOTSTRAP_SCHEMA_JSON,
    )
    .map_err(|e| {
        miette::miette!(
            "failed to write bootstrap-schema.json for '{}': {e:#}",
            agent.name
        )
    })?;

    tracing::debug!(agent = %agent.name, "wrote schemas");

    // Pre-create shell-snapshots dir so CC Bash tool doesn't error on first run.
    std::fs::create_dir_all(claude_dir.join("shell-snapshots")).map_err(|e| {
        miette::miette!(
            "failed to create shell-snapshots dir for '{}': {e:#}",
            agent.name
        )
    })?;
    std::fs::write(
        claude_dir.join("settings.json"),
        serde_json::to_string_pretty(&settings).map_err(|e| {
            miette::miette!(
                "failed to serialize settings for '{}': {e:#}",
                agent.name
            )
        })?,
    )
    .map_err(|e| {
        miette::miette!(
            "failed to write settings.json for '{}': {e:#}",
            agent.name
        )
    })?;
    tracing::debug!(agent = %agent.name, "wrote settings.json");

    // Generate per-agent .claude.json with trust entries.
    crate::codegen::generate_agent_claude_json(agent)?;

    // Create credential symlink for OAuth under HOME override.
    crate::codegen::create_credential_symlink(agent, &host_home)?;

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
    crate::codegen::install_builtin_skills(&agent.path)?;

    // Write settings.local.json only if absent (CC may write runtime state here).
    let settings_local = agent.path.join(".claude").join("settings.local.json");
    if !settings_local.exists() {
        std::fs::write(&settings_local, "{}").map_err(|e| {
            miette::miette!(
                "failed to write settings.local.json for '{}': {e:#}",
                agent.name
            )
        })?;
    }

    // Initialize per-agent memory database.
    crate::memory::open_db(&agent.path).map_err(|e| {
        miette::miette!(
            "failed to open memory database for '{}': {e:#}",
            agent.name
        )
    })?;
    tracing::debug!(agent = %agent.name, "memory.db initialized");

    // Ensure agent has a persistent secret for token derivation.
    let existing_secret = agent.config.as_ref().and_then(|c| c.secret.as_deref());
    let agent_secret = ensure_agent_secret(&agent.path, &agent.name, existing_secret)?;

    // Generate policy.yaml from network_policy setting.
    let network_policy = agent
        .config
        .as_ref()
        .map(|c| c.network_policy.clone())
        .unwrap_or_default();
    let mcp_port = crate::runtime::MCP_HTTP_PORT;
    let policy_content = crate::codegen::policy::generate_policy(mcp_port, &network_policy, None);
    std::fs::write(agent.path.join("policy.yaml"), &policy_content).map_err(|e| {
        miette::miette!(
            "failed to write policy.yaml for '{}': {e:#}",
            agent.name
        )
    })?;
    tracing::debug!(agent = %agent.name, %network_policy, "wrote policy.yaml");

    // Generate mcp.json with right HTTP MCP server entry.
    let bearer_token = crate::mcp::derive_token(&agent_secret, "right-mcp")?;
    let right_mcp_url = match agent_sandbox_mode {
        SandboxMode::None => format!("http://127.0.0.1:{mcp_port}/mcp"),
        SandboxMode::Openshell => format!("http://host.docker.internal:{mcp_port}/mcp"),
    };
    crate::codegen::generate_mcp_config_http(
        &agent.path,
        &agent.name,
        &right_mcp_url,
        &bearer_token,
    )?;
    tracing::debug!(agent = %agent.name, "wrote mcp.json with right HTTP MCP entry");

    Ok(agent_secret)
}

/// Run cross-agent codegen pipeline.
///
/// Generates: agent-tokens.json, process-compose.yaml, cloudflared config, runtime state.
/// Per-agent codegen is handled by the bot at startup via `run_single_agent_codegen()`.
///
/// - `all_agents`: all discovered agents
/// - `self_exe`: path to the rightclaw binary (used in process-compose.yaml)
/// - `debug`: enable debug-level process-compose logging
pub fn run_agent_codegen(
    home: &Path,
    all_agents: &[AgentDef],
    self_exe: &Path,
    debug: bool,
) -> miette::Result<()> {
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| miette::miette!("failed to create run directory: {e:#}"))?;

    let global_cfg = crate::config::read_global_config(home)?;

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
        let token = crate::mcp::derive_token(secret, "right-mcp")?;
        token_map_entries.insert(agent.name.clone(), serde_json::Value::String(token));
    }
    let token_map_path = run_dir.join("agent-tokens.json");
    std::fs::write(
        &token_map_path,
        serde_json::to_string_pretty(&serde_json::Value::Object(token_map_entries))
            .map_err(|e| miette::miette!("failed to serialize token map: {e:#}"))?,
    )
    .map_err(|e| miette::miette!("failed to write agent-tokens.json: {e:#}"))?;
    tracing::debug!("wrote agent-tokens.json");

    // Validate policy files exist for all sandboxed agents.
    for agent in all_agents {
        if let Some(ref config) = agent.config {
            config.resolve_policy_path(&agent.path)?;
        }
    }

    // Generate cloudflared config and wrapper script when tunnel is configured.
    if let Some(ref tunnel_cfg) = global_cfg.tunnel {
        which::which("cloudflared").map_err(|_| {
            miette::miette!(
                "TunnelConfig is present but `cloudflared` is not in PATH -- install cloudflared first"
            )
        })?;
        if !tunnel_cfg.credentials_file.exists() {
            return Err(miette::miette!(
                help = "Run `rightclaw config set` and select Tunnel -- choose \"Delete and recreate\" to generate new credentials on this machine",
                "Tunnel credentials file not found: {}\n\n  \
                 This usually means the tunnel was created on a different machine,\n  \
                 or `rightclaw init` was re-run after the credentials file was removed.",
                tunnel_cfg.credentials_file.display()
            ));
        }
    }

    let cloudflared_script_path: Option<std::path::PathBuf> =
        if let Some(ref tunnel_cfg) = global_cfg.tunnel {
            let agent_pairs: Vec<(String, std::path::PathBuf)> = all_agents
                .iter()
                .map(|a| (a.name.clone(), a.path.clone()))
                .collect();

            let creds = CloudflaredCredentials {
                tunnel_uuid: tunnel_cfg.tunnel_uuid.clone(),
                credentials_file: tunnel_cfg.credentials_file.clone(),
            };

            let cf_config = crate::codegen::cloudflared::generate_cloudflared_config(
                &agent_pairs,
                &tunnel_cfg.hostname,
                Some(&creds),
            )?;
            let cf_config_path = home.join("cloudflared-config.yml");
            std::fs::write(&cf_config_path, &cf_config)
                .map_err(|e| miette::miette!("write cloudflared config: {e:#}"))?;
            tracing::info!(path = %cf_config_path.display(), "cloudflared config written");

            // Write DNS routing wrapper script.
            let scripts_dir = home.join("scripts");
            std::fs::create_dir_all(&scripts_dir)
                .map_err(|e| miette::miette!("create scripts dir: {e:#}"))?;
            let uuid = &tunnel_cfg.tunnel_uuid;
            let hostname = &tunnel_cfg.hostname;
            let cf_config_path_str = cf_config_path.display();
            let script_content = format!(
                "#!/bin/sh\ncloudflared tunnel route dns --overwrite-dns {uuid} {hostname} || true\nexec cloudflared tunnel --config {cf_config_path_str} run\n"
            );
            let script_path = scripts_dir.join("cloudflared-start.sh");
            std::fs::write(&script_path, &script_content)
                .map_err(|e| miette::miette!("write cloudflared-start.sh: {e:#}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(
                    &script_path,
                    std::fs::Permissions::from_mode(0o700),
                )
                .map_err(|e| miette::miette!("chmod cloudflared-start.sh: {e:#}"))?;
            }
            tracing::info!(path = %script_path.display(), "cloudflared wrapper script written");
            Some(script_path)
        } else {
            None
        };

    // Generate process-compose.yaml.
    let pc_config = crate::codegen::generate_process_compose(
        all_agents,
        self_exe,
        &crate::codegen::ProcessComposeConfig {
            debug,
            home,
            cloudflared_script: cloudflared_script_path.as_deref(),
            token_map_path: Some(&token_map_path),
        },
    )?;
    let config_path = run_dir.join("process-compose.yaml");
    std::fs::write(&config_path, &pc_config)
        .map_err(|e| miette::miette!("failed to write process-compose.yaml: {e:#}"))?;
    tracing::debug!("wrote process-compose config: {}", config_path.display());

    // Write runtime state.json, preserving started_at from existing state (reload case).
    let state_path = run_dir.join("state.json");
    let socket_path = run_dir.join("pc.sock");
    let started_at = crate::runtime::read_state(&state_path)
        .ok()
        .map(|s| s.started_at)
        .unwrap_or_else(|| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            format!("{}Z", now.as_secs())
        });
    let state = crate::runtime::RuntimeState {
        agents: all_agents
            .iter()
            .map(|a| crate::runtime::AgentState {
                name: a.name.clone(),
            })
            .collect(),
        socket_path: socket_path.display().to_string(),
        started_at,
    };
    crate::runtime::write_state(&state, &state_path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_single_agent_codegen_generates_all_files() {
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

        let agent = crate::agent::discover_single_agent(&agent_dir).unwrap();
        let self_exe = std::path::PathBuf::from("/usr/bin/rightclaw");

        run_single_agent_codegen(home, &agent, &self_exe, false).unwrap();

        // Core files must exist
        assert!(agent_dir.join(".claude/settings.json").exists());
        assert!(agent_dir.join(".claude/system-prompt.md").exists());
        assert!(agent_dir.join(".claude/reply-schema.json").exists());
        assert!(agent_dir.join(".claude/cron-schema.json").exists());
        assert!(agent_dir.join(".claude/bootstrap-schema.json").exists());
        assert!(agent_dir.join("mcp.json").exists());
        assert!(agent_dir.join("memory.db").exists());
        // Policy must be generated
        assert!(agent_dir.join("policy.yaml").exists());
        let policy = std::fs::read_to_string(agent_dir.join("policy.yaml")).unwrap();
        assert!(
            policy.contains(r#"host: "**.*""#),
            "permissive policy must allow all HTTPS"
        );
    }

    #[test]
    fn run_single_agent_codegen_restrictive_policy() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        let agent_dir = home.join("agents").join("test");
        std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "restart: never\nnetwork_policy: restrictive\n",
        )
        .unwrap();

        let agent = crate::agent::discover_single_agent(&agent_dir).unwrap();
        let self_exe = std::path::PathBuf::from("/usr/bin/rightclaw");

        run_single_agent_codegen(home, &agent, &self_exe, false).unwrap();

        let policy = std::fs::read_to_string(agent_dir.join("policy.yaml")).unwrap();
        assert!(policy.contains(r#"host: "*.anthropic.com""#));
        assert!(!policy.contains(r#"host: "**.*""#));
    }

    #[test]
    fn run_agent_codegen_with_empty_agents() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        let self_exe = std::path::PathBuf::from("/usr/bin/rightclaw");
        let result = run_agent_codegen(home, &[], &self_exe, false);
        assert!(result.is_ok(), "empty agents should succeed: {result:?}");
        // run_dir should have been created
        assert!(home.join("run").exists());
        // process-compose.yaml should exist
        assert!(home.join("run/process-compose.yaml").exists());
        // state.json should exist
        assert!(home.join("run/state.json").exists());
    }

    #[test]
    fn tools_md_not_overwritten_if_exists() {
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

        let agent = crate::agent::discover_single_agent(&agent_dir).unwrap();
        let self_exe = std::path::PathBuf::from("/usr/bin/rightclaw");
        run_single_agent_codegen(home, &agent, &self_exe, false).unwrap();

        let after = std::fs::read_to_string(agent_dir.join("TOOLS.md")).unwrap();
        assert_eq!(after, custom_content, "TOOLS.md must not be overwritten");
    }

    #[test]
    fn tools_md_not_created_by_codegen_if_missing() {
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

        let agent = crate::agent::discover_single_agent(&agent_dir).unwrap();
        let self_exe = std::path::PathBuf::from("/usr/bin/rightclaw");
        run_single_agent_codegen(home, &agent, &self_exe, false).unwrap();

        // Codegen no longer creates TOOLS.md — that's init's responsibility
        assert!(!agent_dir.join("TOOLS.md").exists(), "codegen must not create TOOLS.md");
    }

}
