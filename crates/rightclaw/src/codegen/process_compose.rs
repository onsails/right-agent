use std::path::Path;

use minijinja::{Environment, context};
use serde::Serialize;

use crate::agent::{AgentDef, RestartPolicy};

const PC_TEMPLATE: &str = include_str!("../../../../templates/process-compose.yaml.j2");

/// Template context for a single bot process entry.
#[derive(Debug, Serialize)]
struct BotProcessAgent {
    /// Agent name (template appends `-bot` suffix for the PC process key).
    name: String,
    /// Agent name again, used in `--agent <agent_name>` arg.
    agent_name: String,
    /// Absolute path to the rightclaw executable.
    exe_path: String,
    /// Agent directory path (both working_dir and RC_AGENT_DIR).
    working_dir: String,
    /// Inline Telegram token value.
    token_inline: Option<String>,
    restart_policy: String,
    backoff_seconds: u32,
    max_restarts: u32,
    /// When true, passes `--debug` to `rightclaw bot` so CC stderr is logged at debug level.
    debug: bool,
    /// When true, sandbox is disabled (direct claude -p calls).
    no_sandbox: bool,
    /// Absolute path to the generated OpenShell policy.yaml for this agent. None when no_sandbox.
    sandbox_policy_path: Option<String>,
    /// Rightclaw home directory (for deterministic SSH config path in login process).
    home_dir: String,
}

/// Template context for the shared right MCP HTTP server process.
#[derive(Debug, Serialize)]
struct RightMcpServer {
    exe_path: String,
    port: u16,
    token_map_path: String,
    home_dir: String,
}

/// Template context for the cloudflared tunnel process entry.
#[derive(Debug, Serialize)]
struct CloudflaredEntry {
    /// Absolute path to the cloudflared-start.sh wrapper script.
    command: String,
    /// Working directory for the process (rightclaw home dir).
    working_dir: String,
}

/// Map `RestartPolicy` to process-compose's expected string values.
fn restart_policy_str(policy: &RestartPolicy) -> &'static str {
    match policy {
        RestartPolicy::OnFailure => "on_failure",
        RestartPolicy::Always => "always",
        RestartPolicy::Never => "no",
    }
}

/// Generate a `process-compose.yaml` configuration for Telegram-enabled agents.
///
/// Only agents with `telegram_token` configured produce a process entry.
/// Agents without a Telegram token are excluded entirely.
///
/// Each entry runs `<exe_path> bot --agent <name>` with env vars for the agent
/// directory, name, and Telegram token.
pub fn generate_process_compose(
    agents: &[AgentDef],
    exe_path: &Path,
    debug: bool,
    no_sandbox: bool,
    run_dir: &Path,
    home: &Path,
    cloudflared_script: Option<&Path>,
    token_map_path: Option<&Path>,
) -> miette::Result<String> {
    // Build cloudflared template context when tunnel script is provided.
    // working_dir = parent of scripts/ dir (i.e., rightclaw home).
    let cf_entry: Option<CloudflaredEntry> = cloudflared_script.map(|script| {
        let working_dir = script
            .parent() // scripts/
            .and_then(|p| p.parent()) // home/
            .unwrap_or(script)
            .display()
            .to_string();
        CloudflaredEntry {
            command: script.display().to_string(),
            working_dir,
        }
    });

    let right_mcp_server: Option<RightMcpServer> = token_map_path.map(|p| RightMcpServer {
        exe_path: exe_path.display().to_string(),
        port: 8100,
        token_map_path: p.display().to_string(),
        home_dir: home.display().to_string(),
    });

    let bot_agents: Vec<BotProcessAgent> = agents
        .iter()
        .filter_map(|agent| {
            let config = agent.config.as_ref()?;

            // No telegram token configured — skip this agent.
            let token_inline = config.telegram_token.clone();
            token_inline.as_ref()?;

            let (restart, backoff, max) = (
                restart_policy_str(&config.restart),
                config.backoff_seconds,
                config.max_restarts,
            );

            Some(BotProcessAgent {
                name: agent.name.clone(),
                agent_name: agent.name.clone(),
                exe_path: exe_path.display().to_string(),
                working_dir: agent.path.display().to_string(),
                token_inline,
                restart_policy: restart.to_owned(),
                backoff_seconds: backoff,
                max_restarts: max,
                debug,
                no_sandbox,
                sandbox_policy_path: if no_sandbox {
                    None
                } else {
                    Some(
                        run_dir
                            .join("policies")
                            .join(format!("{}.yaml", agent.name))
                            .display()
                            .to_string(),
                    )
                },
                home_dir: home.display().to_string(),
            })
        })
        .collect();

    let mut env = Environment::new();
    env.add_template("pc", PC_TEMPLATE)
        .map_err(|e| miette::miette!("template parse error: {e:#}"))?;
    let tmpl = env.get_template("pc").expect("template just added");

    tmpl.render(context! {
        agents => bot_agents,
        cloudflared => cf_entry,
        pc_port => crate::runtime::pc_client::PC_PORT,
        right_mcp_server => right_mcp_server,
    })
    .map_err(|e| miette::miette!("template render error: {e:#}"))
}

#[cfg(test)]
#[path = "process_compose_tests.rs"]
mod tests;
