use crate::agent::AgentDef;
use minijinja::{Environment, context};

/// Escape a value for single-quoted bash export.
/// Turns: it's alive  →  it'\''s alive
/// The full export line becomes: export KEY='it'\''s alive'
fn shell_single_quote_escape(value: &str) -> String {
    value.replace('\'', r"'\''")
}

const WRAPPER_TEMPLATE: &str = include_str!("../../../../templates/agent-wrapper.sh.j2");

/// Generate a shell wrapper script for an agent.
///
/// `combined_prompt_path` points to a single file containing the merged
/// identity + start prompt + optional RightCron bootstrap content.
/// Claude Code only allows one `--append-system-prompt-file`, so all
/// system prompt content must be combined into a single file.
pub fn generate_wrapper(
    agent: &AgentDef,
    combined_prompt_path: &str,
    debug_log_path: Option<&str>,
) -> miette::Result<String> {
    let mut env = Environment::new();
    env.add_template("wrapper", WRAPPER_TEMPLATE)
        .map_err(|e| miette::miette!("template parse error: {e:#}"))?;
    let tmpl = env.get_template("wrapper").expect("template just added");

    // Detect Telegram channel configuration via agent.config (D-01).
    let channels: Option<&str> = if agent
        .config
        .as_ref()
        .map(|c| c.telegram_token.is_some() || c.telegram_token_file.is_some())
        .unwrap_or(false)
    {
        Some("plugin:telegram@claude-plugins-official")
    } else {
        None
    };

    let model = agent.config.as_ref().and_then(|c| c.model.as_deref());

    // Build export lines for env: vars (D-03: after identity captures, before HOME).
    let env_exports: Vec<String> = agent
        .config
        .as_ref()
        .map(|c| &c.env)
        .into_iter()
        .flat_map(|env| env.iter())
        .map(|(k, v)| format!("export {}='{}'", k, shell_single_quote_escape(v)))
        .collect();

    // Startup prompt — executed as first message (positional arg, stays interactive).
    let startup_prompt = "Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user.";

    tmpl.render(context! {
        agent_name => agent.name,
        working_dir => agent.path.display().to_string(),
        combined_prompt_path => combined_prompt_path,
        channels => channels,
        model => model,
        startup_prompt => startup_prompt,
        debug => debug_log_path.is_some(),
        debug_log_path => debug_log_path.unwrap_or_default(),
        env_exports => env_exports,
    })
    .map_err(|e| miette::miette!("template render error: {e:#}"))
}

#[cfg(test)]
#[path = "shell_wrapper_tests.rs"]
mod tests;
