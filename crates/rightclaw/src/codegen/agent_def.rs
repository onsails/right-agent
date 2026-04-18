/// Platform operating instructions, compiled into the binary.
///
/// Injected directly into the system prompt at assembly time.
/// Source: `templates/right/prompt/OPERATING_INSTRUCTIONS.md`
pub const OPERATING_INSTRUCTIONS: &str =
    include_str!("../../templates/right/prompt/OPERATING_INSTRUCTIONS.md");

/// Bootstrap instructions, compiled into the binary.
///
/// Injected into the system prompt when bootstrap mode is active
/// (BOOTSTRAP.md exists in agent dir). The on-disk file is only
/// an existence flag — content always comes from this constant.
/// Source: `templates/right/agent/BOOTSTRAP.md`
pub const BOOTSTRAP_INSTRUCTIONS: &str =
    include_str!("../../templates/right/agent/BOOTSTRAP.md");

/// JSON schema for the structured reply format used by teloxide agents (D-02).
///
/// Agents write replies as JSON conforming to this schema.
/// `content` is required (may be null for media-only replies).
pub const REPLY_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"content":{"type":["string","null"]},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]}"#;

/// JSON schema for bootstrap mode — adds `bootstrap_complete` field.
///
/// `bootstrap_complete` is required but the bot does NOT trust it alone —
/// server-side file-presence check (`should_accept_bootstrap`) gates completion.
pub const BOOTSTRAP_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"content":{"type":["string","null"]},"bootstrap_complete":{"type":"boolean"},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content","bootstrap_complete"]}"#;

/// JSON schema for cron job structured output.
///
/// `summary` is always required. `notify` is null when the cron ran silently
/// (no user notification needed). When `notify` is present, `content` is required.
/// `no_notify_reason` is required when `notify` is null — a short factual explanation
/// of why there is nothing to report (e.g. "No changes since last run").
pub const CRON_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"notify":{"type":["object","null"],"properties":{"content":{"type":"string"},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]},"summary":{"type":"string"},"no_notify_reason":{"type":["string","null"]}},"required":["summary"]}"#;

/// Generate the base system prompt for all agent modes.
///
/// This replaces CC's default system prompt via `--system-prompt-file`.
/// Content: agent identity, RightClaw description, sandbox info, MCP reference.
/// Behavior-specific instructions come from the agent definition (`--agent`).
pub fn generate_system_prompt(agent_name: &str, sandbox_mode: &crate::agent::types::SandboxMode, home_dir: &str) -> String {
    let sandbox_desc = match sandbox_mode {
        crate::agent::types::SandboxMode::Openshell => "OpenShell sandbox (k3s container with network and filesystem policies)",
        crate::agent::types::SandboxMode::None => "no sandbox (direct host access)",
    };

    let mut prompt = format!(
        "\
You are {agent_name}, a RightClaw agent.

RightClaw is a multi-agent runtime for Claude Code built on NVIDIA OpenShell. Each agent runs \
as an independent Claude Code session inside its own sandbox with declarative YAML policies. \
Agents have persistent memory, scheduled tasks (cron), and tool management via MCP.

Source: https://github.com/onsails/rightclaw

## Environment

- Agent name: {agent_name}
- Sandbox: {sandbox_desc}
- Home / working directory: {home_dir}

## MCP

You are connected to the `right` MCP server for persistent memory, cron job management, \
and external MCP server management. Use `mcp__right__mcp_list` to see all configured servers.\n\
\n\
**Call `right` MCP tools directly by name (e.g. `mcp__right__mcp_list`). \
Do NOT use ToolSearch to find them — ToolSearch does not index MCP tools. \
They are always available.**

## Response Rules

Your final response MUST be self-contained. The user ONLY sees your final response — \
they do NOT see tool calls, intermediate text, or thinking. Never say \"see above\", \
\"as shown above\", or reference previous output. If you gathered data, include it in \
your final response.
"
    );

    if matches!(sandbox_mode, crate::agent::types::SandboxMode::Openshell) {
        prompt.push_str(&format!(
            "
## User SSH Access

If an operation requires an interactive terminal (TUI, interactive prompts, \
password input) that you cannot perform from within your sandbox — tell the \
user to run:

  rightclaw agent ssh {agent_name}
  rightclaw agent ssh {agent_name} -- <command>

Examples:
- `gh auth login`
- `gcloud auth login`
- `npm login`
- Any command with interactive prompts or TUI

Always provide the exact command with the `--` separator when passing a specific command.
"
        ));
    }

    prompt
}

#[cfg(test)]
#[path = "agent_def_tests.rs"]
mod tests;
