use crate::agent::AgentDef;
use minijinja::{Environment, context};

const WRAPPER_TEMPLATE: &str = include_str!("../../../../templates/agent-wrapper.sh.j2");

/// Generate a shell wrapper script for an agent.
///
/// When `no_sandbox` is false, the wrapper invokes `openshell sandbox create`
/// with the agent's policy. When true, it runs `claude` directly.
pub fn generate_wrapper(agent: &AgentDef, no_sandbox: bool) -> miette::Result<String> {
    let mut env = Environment::new();
    env.add_template("wrapper", WRAPPER_TEMPLATE)
        .map_err(|e| miette::miette!("template parse error: {e:#}"))?;
    let tmpl = env.get_template("wrapper").expect("template just added");

    let start_prompt = agent
        .config
        .as_ref()
        .and_then(|c| c.start_prompt.as_deref())
        .unwrap_or("You are starting. Read your MEMORY.md to restore context.");

    // Detect Telegram channel configuration.
    // If agent has .mcp.json, set channels to the Telegram plugin identifier.
    // V1 simplification: .mcp.json presence implies Telegram. Future versions
    // could parse .mcp.json contents for more granular channel detection.
    let channels: Option<&str> = if agent.mcp_config_path.is_some() {
        Some("plugin:telegram@claude-plugins-official")
    } else {
        None
    };

    tmpl.render(context! {
        agent_name => agent.name,
        identity_path => agent.identity_path.display().to_string(),
        policy_path => agent.policy_path.display().to_string(),
        no_sandbox => no_sandbox,
        start_prompt => start_prompt,
        channels => channels,
    })
    .map_err(|e| miette::miette!("template render error: {e:#}"))
}

#[cfg(test)]
#[path = "shell_wrapper_tests.rs"]
mod tests;
