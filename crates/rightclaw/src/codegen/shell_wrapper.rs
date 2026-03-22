use crate::agent::AgentDef;
use minijinja::{Environment, context};

const WRAPPER_TEMPLATE: &str = include_str!("../../../../templates/agent-wrapper.sh.j2");

/// Generate a shell wrapper script for an agent.
///
/// `combined_prompt_path` points to a single file containing the merged
/// identity + start prompt + optional CronSync bootstrap content.
/// Claude Code only allows one `--append-system-prompt-file`, so all
/// system prompt content must be combined into a single file.
pub fn generate_wrapper(
    agent: &AgentDef,
    no_sandbox: bool,
    combined_prompt_path: &str,
) -> miette::Result<String> {
    let mut env = Environment::new();
    env.add_template("wrapper", WRAPPER_TEMPLATE)
        .map_err(|e| miette::miette!("template parse error: {e:#}"))?;
    let tmpl = env.get_template("wrapper").expect("template just added");

    // Detect Telegram channel configuration.
    let channels: Option<&str> = if agent.mcp_config_path.is_some() {
        Some("plugin:telegram@claude-plugins-official")
    } else {
        None
    };

    tmpl.render(context! {
        agent_name => agent.name,
        policy_path => agent.policy_path.display().to_string(),
        working_dir => agent.path.display().to_string(),
        combined_prompt_path => combined_prompt_path,
        no_sandbox => no_sandbox,
        channels => channels,
    })
    .map_err(|e| miette::miette!("template render error: {e:#}"))
}

#[cfg(test)]
#[path = "shell_wrapper_tests.rs"]
mod tests;
