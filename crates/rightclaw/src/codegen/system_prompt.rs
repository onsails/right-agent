use std::path::Path;

use crate::agent::AgentDef;

/// Generate a combined system prompt file content for an agent.
///
/// Concatenates: identity file content + start prompt + optional CronSync bootstrap.
/// Claude Code only allows one `--append-system-prompt-file`, so everything
/// must be merged into a single file.
pub fn generate_combined_prompt(agent: &AgentDef) -> miette::Result<String> {
    let identity_content = std::fs::read_to_string(&agent.identity_path)
        .map_err(|e| miette::miette!("Failed to read {}: {}", agent.identity_path.display(), e))?;

    let start_prompt = agent
        .config
        .as_ref()
        .and_then(|c| c.start_prompt.as_deref())
        .unwrap_or("You are starting. Read your MEMORY.md to restore context.");

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

    // Add CronSync bootstrap if agent has crons/ directory
    let crons_dir = agent.path.join("crons");
    if crons_dir.is_dir() && has_yaml_files(&crons_dir) {
        content.push_str("\n## RightClaw System Instructions\n\n");
        content.push_str(
            "On startup, check if the `crons/` directory exists in your agent directory.\n\
             If it contains `.yaml` files, run `/cronsync` to reconcile scheduled tasks.\n\n\
             This ensures all declared cron jobs are active after agent restart or session expiry.\n",
        );
    }

    Ok(content)
}

fn has_yaml_files(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.filter_map(Result::ok).any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "yaml" || ext == "yml")
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "system_prompt_tests.rs"]
mod tests;
