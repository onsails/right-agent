use crate::agent::AgentDef;

/// Generate a system prompt for an agent by concatenating present identity files.
///
/// Canonical file order: IDENTITY.md → SOUL.md → USER.md → AGENTS.md.
/// Absent files (None path or file does not exist) are silently skipped.
/// IDENTITY.md is treated as required — returns Err if the file cannot be read.
///
/// Sections are separated by "\n\n---\n\n".
/// The caller writes the returned string to `agent_dir/.claude/system-prompt.txt`.
/// The `.claude/` directory must already exist before calling (created by cmd_up).
pub fn generate_system_prompt(agent: &AgentDef) -> miette::Result<String> {
    // IDENTITY.md is required — error if missing or unreadable.
    let identity_content = std::fs::read_to_string(&agent.identity_path).map_err(|e| {
        miette::miette!(
            "failed to read {}: {e}",
            agent.identity_path.display()
        )
    })?;

    let mut sections = vec![identity_content];

    // Optional files: silently skip if path is None or file does not exist on disk.
    let optional: [Option<&std::path::PathBuf>; 3] = [
        agent.soul_path.as_ref(),
        agent.user_path.as_ref(),
        agent.agents_path.as_ref(),
    ];
    for path_opt in optional {
        if let Some(path) = path_opt {
            if path.exists() {
                let content = std::fs::read_to_string(path).map_err(|e| {
                    miette::miette!("failed to read {}: {e}", path.display())
                })?;
                sections.push(content);
            }
        }
    }

    Ok(sections.join("\n\n---\n\n"))
}

#[cfg(test)]
#[path = "system_prompt_tests.rs"]
mod tests;
