use crate::agent::AgentDef;

/// Generate a combined system prompt file content for an agent.
///
/// Concatenates: identity file content + start prompt + optional RightCron bootstrap.
/// Claude Code only allows one `--append-system-prompt-file`, so everything
/// must be merged into a single file.
pub fn generate_combined_prompt(agent: &AgentDef) -> miette::Result<String> {
    let identity_content = std::fs::read_to_string(&agent.identity_path)
        .map_err(|e| miette::miette!("Failed to read {}: {}", agent.identity_path.display(), e))?;

    let start_prompt = agent
        .config
        .as_ref()
        .and_then(|c| c.start_prompt.as_deref())
        .unwrap_or("You are starting.");

    let mut content = identity_content;
    content.push_str("\n\n---\n\n## Startup Instructions\n\n");

    // Check for BOOTSTRAP.md — first-run onboarding takes priority
    let bootstrap_path = agent.path.join("BOOTSTRAP.md");
    if bootstrap_path.exists() {
        content.push_str(
            "**IMPORTANT: First-run onboarding required.**\n\n\
             Read `BOOTSTRAP.md` in your agent directory and follow its instructions \
             before doing anything else. The onboarding will ask for the user's name, \
             personality preferences, and create your identity files. After completing \
             onboarding, delete BOOTSTRAP.md as instructed.\n\n",
        );
    }

    content.push_str(start_prompt);
    content.push('\n');

    // Always communicate via remote channel (Telegram/Discord), not console.
    content.push_str(
        "\n## Communication\n\n\
         You are running as a daemon with no terminal access. \
         ALWAYS use the remote channel (reply MCP tool) to communicate with the user. \
         Never output to console — the user cannot see it.\n",
    );

    // RightCron: startup trigger + routing.
    content.push_str(
        "\n## Cron Management (RightCron)\n\n\
         **On startup:** Run `/rightcron` immediately. It will bootstrap the reconciler \
         and recover any persisted jobs. Do this before responding to the user.\n\n\
         **For user requests:** When the user wants to manage cron jobs, scheduled tasks, \
         or recurring tasks, ALWAYS use the /rightcron skill. NEVER call CronCreate \
         directly — always write a YAML spec first, then reconcile.\n",
    );

    Ok(content)
}

#[cfg(test)]
#[path = "system_prompt_tests.rs"]
mod tests;
