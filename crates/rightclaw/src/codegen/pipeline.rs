use std::collections::HashMap;
use std::path::Path;

use crate::agent::types::{AgentDef, SandboxMode};
use crate::codegen::cloudflared::CloudflaredCredentials;

/// Run the full codegen pipeline for a set of agents.
///
/// - `agents`: agents to generate per-agent artifacts for (settings.json, .claude.json, etc.)
/// - `all_agents`: all discovered agents -- used for process-compose.yaml, token map, policy
///   validation, runtime state
/// - `self_exe`: path to the rightclaw binary (written into mcp.json)
/// - `debug`: enable debug-level process-compose logging
pub fn run_agent_codegen(
    home: &Path,
    agents: &[AgentDef],
    all_agents: &[AgentDef],
    self_exe: &Path,
    debug: bool,
) -> miette::Result<()> {
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| miette::miette!("failed to create run directory: {e:#}"))?;

    let host_home =
        dirs::home_dir().ok_or_else(|| miette::miette!("cannot determine home directory"))?;

    let global_cfg = crate::config::read_global_config(home)?;

    // Track secrets generated during this run to avoid re-reading agent.yaml in the token map.
    let mut generated_secrets: HashMap<String, String> = HashMap::new();

    // Per-agent codegen loop.
    for agent in agents {
        // Generate .claude/settings.json with behavioral flags.
        let settings = crate::codegen::generate_settings()?;
        let claude_dir = agent.path.join(".claude");
        std::fs::create_dir_all(&claude_dir).map_err(|e| {
            miette::miette!("failed to create .claude dir for '{}': {e:#}", agent.name)
        })?;

        // Generate agent definition .md from present identity files.
        let agent_def_content = crate::codegen::generate_agent_definition(agent)?;
        let agents_dir = claude_dir.join("agents");
        std::fs::create_dir_all(&agents_dir).map_err(|e| {
            miette::miette!(
                "failed to create .claude/agents dir for '{}': {e:#}",
                agent.name
            )
        })?;
        std::fs::write(
            agents_dir.join(format!("{}.md", agent.name)),
            &agent_def_content,
        )
        .map_err(|e| {
            miette::miette!(
                "failed to write agent definition for '{}': {e:#}",
                agent.name
            )
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

        tracing::debug!(agent = %agent.name, "wrote agent definition + reply-schema.json");

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
        let agent_secret =
            if let Some(ref secret) = agent.config.as_ref().and_then(|c| c.secret.clone()) {
                secret.clone()
            } else {
                let new_secret = crate::mcp::generate_agent_secret();
                // Read-modify-write agent.yaml via YAML parse to avoid duplicate keys.
                let yaml_path = agent.path.join("agent.yaml");
                let yaml_content = std::fs::read_to_string(&yaml_path).map_err(|e| {
                    miette::miette!(
                        "failed to read agent.yaml for '{}': {e:#}",
                        agent.name
                    )
                })?;
                let mut doc: serde_json::Map<String, serde_json::Value> =
                    serde_saphyr::from_str(&yaml_content).map_err(|e| {
                        miette::miette!(
                            "failed to parse agent.yaml for '{}': {e:#}",
                            agent.name
                        )
                    })?;
                doc.insert(
                    "secret".to_owned(),
                    serde_json::Value::String(new_secret.clone()),
                );
                let updated_yaml = serde_saphyr::to_string(&doc).map_err(|e| {
                    miette::miette!(
                        "failed to serialize agent.yaml for '{}': {e:#}",
                        agent.name
                    )
                })?;
                std::fs::write(&yaml_path, &updated_yaml).map_err(|e| {
                    miette::miette!(
                        "failed to write agent secret for '{}': {e:#}",
                        agent.name
                    )
                })?;
                tracing::info!(agent = %agent.name, "generated new agent secret");
                generated_secrets.insert(agent.name.clone(), new_secret.clone());
                new_secret
            };

        // Generate mcp.json with right HTTP MCP server entry.
        let bearer_token = crate::mcp::derive_token(&agent_secret, "right-mcp")?;
        let mcp_port = crate::runtime::MCP_HTTP_PORT;
        let agent_sandbox_mode = agent
            .config
            .as_ref()
            .map(|c| c.sandbox_mode().clone())
            .unwrap_or_default();
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
    }

    // Write agent token map for the HTTP MCP server process.
    let mut token_map_entries = serde_json::Map::new();
    for agent in all_agents {
        let secret = agent
            .config
            .as_ref()
            .and_then(|c| c.secret.clone())
            .or_else(|| generated_secrets.get(&agent.name).cloned())
            .ok_or_else(|| {
                miette::miette!("agent '{}' has no secret after generation", agent.name)
            })?;
        let token = crate::mcp::derive_token(&secret, "right-mcp")?;
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

    // Write runtime state.json.
    let socket_path = run_dir.join("pc.sock");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| miette::miette!("system time error: {e:#}"))?;
    let state = crate::runtime::RuntimeState {
        agents: all_agents
            .iter()
            .map(|a| crate::runtime::AgentState {
                name: a.name.clone(),
            })
            .collect(),
        socket_path: socket_path.display().to_string(),
        started_at: format!("{}Z", now.as_secs()),
    };
    let state_path = run_dir.join("state.json");
    crate::runtime::write_state(&state, &state_path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_agent_codegen_with_empty_agents() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        let self_exe = std::path::PathBuf::from("/usr/bin/rightclaw");
        let result = run_agent_codegen(home, &[], &[], &self_exe, false);
        assert!(result.is_ok(), "empty agents should succeed: {result:?}");
        // run_dir should have been created
        assert!(home.join("run").exists());
        // process-compose.yaml should exist
        assert!(home.join("run/process-compose.yaml").exists());
        // state.json should exist
        assert!(home.join("run/state.json").exists());
    }
}
