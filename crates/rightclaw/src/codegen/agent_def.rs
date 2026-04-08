use crate::agent::AgentDef;

/// JSON schema for the structured reply format used by teloxide agents (D-02).
///
/// Agents write replies as JSON conforming to this schema.
/// `content` is required (may be null for media-only replies).
pub const REPLY_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"content":{"type":["string","null"]},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]}"#;

/// System prompt section describing message input/output format for CC agents.
const ATTACHMENT_FORMAT_DOCS: &str = "\n\n## Message Input Format\n\n\
You receive user messages via stdin in one of two formats:\n\n\
1. **Plain text** -- a single message with no attachments\n\
2. **YAML** -- multiple messages or messages with attachments, with a `messages:` root key\n\n\
YAML schema:\n\
```yaml\n\
messages:\n\
  - id: <telegram_message_id>\n\
    ts: <ISO 8601 timestamp>\n\
    text: <message text or caption>\n\
    attachments:\n\
      - type: photo|document|video|audio|voice|video_note|sticker|animation\n\
        path: <absolute path to file>\n\
        mime_type: <MIME type>\n\
        filename: <original filename, documents only>\n\
```\n\n\
Use the Read tool to view images and files at the given paths.\n\n\
## Sending Attachments\n\n\
Write files to /sandbox/outbox/ (or the outbox/ directory in your working directory).\n\
Include them in your JSON response under the `attachments` array.\n\n\
Size limits enforced by the bot:\n\
- Photos: max 10MB\n\
- Documents, videos, audio, voice, animations: max 50MB\n\n\
Do not produce files exceeding these limits. If you need to send large data,\n\
split into multiple smaller files or use a different format.\n";

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
    let optional: [Option<&std::path::PathBuf>; 4] = [
        agent.bootstrap_path.as_ref(),
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
        "---\nname: {}\nmodel: {}\ndescription: \"RightClaw agent: {}\"\n---\n\n{}{}",
        agent.name, model, agent.name, body, ATTACHMENT_FORMAT_DOCS
    ))
}

#[cfg(test)]
#[path = "agent_def_tests.rs"]
mod tests;
