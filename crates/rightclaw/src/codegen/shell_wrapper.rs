use crate::agent::AgentDef;
use minijinja::{Environment, context};

const WRAPPER_TEMPLATE: &str = include_str!("../../../../templates/agent-wrapper.sh.j2");

/// Generate a shell wrapper script for an agent.
///
/// `combined_prompt_path` points to a single file containing the merged
/// identity + start prompt + optional RightCron bootstrap content.
/// Claude Code only allows one `--append-system-prompt-file`, so all
/// system prompt content must be combined into a single file.
pub fn generate_wrapper(
    agent: &AgentDef,
    no_sandbox: bool,
    combined_prompt_path: &str,
    debug_log_path: Option<&str>,
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

    let model = agent.config.as_ref().and_then(|c| c.model.as_deref());

    // Startup prompt — executed as first message (positional arg, stays interactive).
    let startup_prompt = "Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user.";

    tmpl.render(context! {
        agent_name => agent.name,
        policy_path => agent.policy_path.display().to_string(),
        working_dir => agent.path.display().to_string(),
        combined_prompt_path => combined_prompt_path,
        no_sandbox => no_sandbox,
        channels => channels,
        model => model,
        startup_prompt => startup_prompt,
        debug => debug_log_path.is_some(),
        debug_log_path => debug_log_path.unwrap_or_default(),
    })
    .map_err(|e| miette::miette!("template render error: {e:#}"))
}

#[cfg(test)]
#[path = "shell_wrapper_tests.rs"]
mod tests;
