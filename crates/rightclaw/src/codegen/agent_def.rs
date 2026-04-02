use crate::agent::AgentDef;

/// JSON schema for the structured reply format used by teloxide agents (D-02).
///
/// Agents write replies as JSON conforming to this schema.
/// `content` is required (may be null for media-only replies).
pub const REPLY_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"content":{"type":["string","null"]},"reply_to_message_id":{"type":["integer","null"]},"media_paths":{"type":["array","null"],"items":{"type":"string"}}},"required":["content"]}"#;

/// Generate an agent definition `.md` file for Claude Code's native agent system (AGDEF-01).
///
/// Output format:
/// ```text
/// ---
/// name: <agent.name>
/// model: <model or "inherit">
/// description: "RightClaw agent: <agent.name>"
/// ---
///
/// <identity content>
///
/// ---
///
/// <soul content>
///
/// ---
///
/// <user content>
///
/// ---
///
/// <agents content>
/// ```
///
/// IDENTITY.md is required — returns Err if missing or unreadable.
/// Optional files (soul_path, user_path, agents_path) are silently skipped if None or absent.
/// Body sections are joined with `"\n\n---\n\n"` separator (per D-06).
/// No `tools:` field is emitted in frontmatter (per D-05).
pub fn generate_agent_definition(agent: &AgentDef) -> miette::Result<String> {
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
    for path in optional.into_iter().flatten() {
        if path.exists() {
            let content = std::fs::read_to_string(path).map_err(|e| {
                miette::miette!("failed to read {}: {e}", path.display())
            })?;
            sections.push(content);
        }
    }

    let body = sections.join("\n\n---\n\n");
    let model = agent
        .config
        .as_ref()
        .and_then(|c| c.model.as_deref())
        .unwrap_or("inherit");

    Ok(format!(
        "---\nname: {}\nmodel: {}\ndescription: \"RightClaw agent: {}\"\n---\n\n{}",
        agent.name, model, agent.name, body
    ))
}

#[cfg(test)]
#[path = "agent_def_tests.rs"]
mod tests;
