/// Content `.md` files that agent definitions reference via `@./FILE.md`.
///
/// These live at the agent root but are copied into `.claude/agents/` by codegen
/// so CC can resolve the `@` references (which are relative to the agent def file).
/// Also used by the bot's sync module for forward/reverse sync with the sandbox.
pub const CONTENT_MD_FILES: &[&str] = &[
    "BOOTSTRAP.md",
    "AGENTS.md",
    "TOOLS.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
];

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
pub const CRON_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"notify":{"type":["object","null"],"properties":{"content":{"type":"string"},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]},"summary":{"type":"string"}},"required":["summary"]}"#;

/// Generate a normal-mode agent definition with `@` file references.
///
/// Order is cache-optimized: static files first (AGENTS, SOUL), dynamic last (USER, TOOLS).
/// CC resolves `@` references relative to the agent def file location (`.claude/agents/`).
/// Content files are copied there by codegen, so `@./FILE.md` resolves correctly.
pub fn generate_agent_definition(name: &str, model: Option<&str>) -> String {
    let model = model.unwrap_or("inherit");
    format!(
        "\
---
name: {name}
model: {model}
description: \"RightClaw agent: {name}\"
---

@./AGENTS.md

---

@./SOUL.md

---

@./IDENTITY.md

---

@./USER.md

---

@./TOOLS.md
"
    )
}

/// Generate a bootstrap-mode agent definition with only the bootstrap prompt.
///
/// Used when BOOTSTRAP.md exists in the agent directory (first-run onboarding).
/// No identity files — bootstrap is the sole context.
/// Content files are copied into `.claude/agents/` by codegen.
pub fn generate_bootstrap_definition(name: &str, model: Option<&str>) -> String {
    let model = model.unwrap_or("inherit");
    format!(
        "\
---
name: {name}-bootstrap
model: {model}
description: \"RightClaw agent bootstrap: {name}\"
---

@./BOOTSTRAP.md
"
    )
}

/// Generate the base system prompt for all agent modes.
///
/// This replaces CC's default system prompt via `--system-prompt-file`.
/// Content: agent identity, RightClaw description, sandbox info, MCP reference.
/// Behavior-specific instructions come from the agent definition (`--agent`).
pub fn generate_system_prompt(agent_name: &str, sandbox_mode: &crate::agent::types::SandboxMode) -> String {
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

## MCP

You are connected to the `right` MCP server for persistent memory, cron job management, \
and external MCP server management. Use `mcp_list` to see all configured servers.

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
