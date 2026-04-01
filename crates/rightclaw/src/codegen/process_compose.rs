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
    /// Inline Telegram token value (mutually exclusive with token_file).
    token_inline: Option<String>,
    /// Absolute path to Telegram token file (mutually exclusive with token_inline).
    token_file: Option<String>,
    restart_policy: String,
    backoff_seconds: u32,
    max_restarts: u32,
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
/// Only agents with `telegram_token` or `telegram_token_file` configured produce
/// a process entry. Agents without a Telegram token are excluded entirely.
///
/// Each entry runs `<exe_path> bot --agent <name>` with env vars for the agent
/// directory, name, and Telegram token.
pub fn generate_process_compose(agents: &[AgentDef], exe_path: &Path) -> miette::Result<String> {
    let bot_agents: Vec<BotProcessAgent> = agents
        .iter()
        .filter_map(|agent| {
            let config = agent.config.as_ref()?;

            // Token precedence: token_file > inline token
            let (token_inline, token_file) =
                if let Some(ref rel_path) = config.telegram_token_file {
                    let abs = agent.path.join(rel_path);
                    (None, Some(abs.display().to_string()))
                } else if let Some(ref token) = config.telegram_token {
                    (Some(token.clone()), None)
                } else {
                    // No telegram token configured — skip this agent
                    return None;
                };

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
                token_file,
                restart_policy: restart.to_owned(),
                backoff_seconds: backoff,
                max_restarts: max,
            })
        })
        .collect();

    let mut env = Environment::new();
    env.add_template("pc", PC_TEMPLATE)
        .map_err(|e| miette::miette!("template parse error: {e:#}"))?;
    let tmpl = env.get_template("pc").expect("template just added");

    tmpl.render(context! { agents => bot_agents })
        .map_err(|e| miette::miette!("template render error: {e:#}"))
}

#[cfg(test)]
#[path = "process_compose_tests.rs"]
mod tests;
