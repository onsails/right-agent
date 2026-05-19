/// Platform operating instructions, compiled into the binary.
///
/// Injected directly into the system prompt at assembly time.
/// Source: `templates/right/prompt/OPERATING_INSTRUCTIONS.md`
pub const OPERATING_INSTRUCTIONS: &str =
    include_str!("../templates/right/prompt/OPERATING_INSTRUCTIONS.md");

/// Bootstrap instructions, compiled into the binary.
///
/// Injected into the system prompt when bootstrap mode is active
/// (BOOTSTRAP.md exists in agent dir). The on-disk file is only
/// an existence flag — content always comes from this constant.
/// Source: `templates/right/agent/BOOTSTRAP.md`
pub const BOOTSTRAP_INSTRUCTIONS: &str = include_str!("../templates/right/agent/BOOTSTRAP.md");

/// Cron delivery contract, compiled into the binary.
///
/// Injected into the system prompt for `PromptMode::Cron` runs
/// (cron::execute_job — both regular cron jobs and background
/// continuation). Tells the agent that its structured output IS
/// the Telegram delivery channel and that the turn has no live user.
/// Source: `templates/right/prompt/CRON_INSTRUCTIONS.md`
pub const CRON_INSTRUCTIONS: &str = include_str!("../templates/right/prompt/CRON_INSTRUCTIONS.md");

/// JSON schema for the structured reply format used by teloxide agents (D-02).
///
/// Agents write replies as JSON conforming to this schema.
/// `content` is required (may be null for media-only replies).
pub const REPLY_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "content": { "type": ["string", "null"] },
    "reply_to_message_id": { "type": ["integer", "null"] },
    "attachments": {
      "type": ["array", "null"],
      "items": {
        "type": "object",
        "properties": {
          "type": {
            "enum": ["photo", "document", "video", "audio", "voice", "video_note", "sticker", "animation"]
          },
          "path": { "type": "string" },
          "filename": { "type": ["string", "null"] },
          "caption": { "type": ["string", "null"] },
          "media_group_id": { "type": ["string", "null"] }
        },
        "required": ["type", "path"]
      }
    },
    "used_skill_receipts": {
      "type": ["array", "null"],
      "items": {
        "type": "object",
        "properties": {
          "package_name": { "type": "string" },
          "message": { "type": "string" }
        },
        "required": ["package_name", "message"]
      }
    },
    "learning_signal": {
      "type": ["object", "null"],
      "properties": {
        "kind": { "const": "create_candidate" },
        "package_name_hint": { "type": "string" },
        "trigger": {
          "enum": ["explicit_user_request", "multi_step_workflow", "recovered_surprise", "user_correction", "repeated_tool_pattern"]
        },
        "reason_not_written": {
          "enum": ["conversation_still_evolving", "needs_full_context_review", "write_or_publish_failed", "needs_existing_skill_diff"]
        },
        "event_refs": {
          "type": "array",
          "items": { "type": "string" },
          "minItems": 1
        },
        "summary": { "type": "string" }
      },
      "required": ["kind", "package_name_hint", "trigger", "reason_not_written", "event_refs", "summary"]
    },
    "skill_issue_signal": {
      "type": ["object", "null"],
      "properties": {
        "kind": { "const": "update_candidate" },
        "skill_name": { "type": "string" },
        "issue": {
          "enum": ["missing_step", "stale_command", "wrong_api_assumption", "overbroad_activation", "broken_script", "unsafe_instruction"]
        },
        "reason_not_patched": {
          "enum": ["conversation_still_evolving", "needs_full_context_review", "write_or_publish_failed", "needs_existing_skill_diff"]
        },
        "observed_effect": {
          "enum": ["retry_after_tool_error", "retry_after_user_correction", "manual_override", "verified_alternative"]
        },
        "event_refs": {
          "type": "array",
          "items": { "type": "string" },
          "minItems": 1
        },
        "patch_hint": { "type": "string" }
      },
      "required": ["kind", "skill_name", "issue", "reason_not_patched", "observed_effect", "event_refs", "patch_hint"]
    }
  },
  "required": ["content"]
}"#;

/// JSON schema for bootstrap mode — adds `bootstrap_complete` field.
///
/// `bootstrap_complete` is required but the bot does NOT trust it alone —
/// server-side file-presence check (`should_accept_bootstrap`) gates completion.
pub const BOOTSTRAP_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"content":{"type":["string","null"]},"bootstrap_complete":{"type":"boolean"},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content","bootstrap_complete"]}"#;

/// JSON schema for cron job structured output.
///
/// `summary` is always required. `notify` is null when the cron ran silently
/// (no user notification needed). When `notify` is present, `content` is required.
/// `no_notify_reason` is required when `notify` is null — a short factual explanation
/// of why there is nothing to report (e.g. "No changes since last run").
pub const CRON_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"notify":{"type":["object","null"],"properties":{"content":{"type":"string"},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]},"summary":{"type":"string"},"no_notify_reason":{"type":["string","null"]}},"required":["summary"]}"#;

/// Structured-output schema for background-continuation cron runs.
///
/// `notify` is required and non-null; `notify.content` must be a non-empty
/// string. `summary` is required (kept for log/analytics parity with
/// `CRON_SCHEMA_JSON`). `no_notify_reason` is absent — silence is not a
/// valid outcome for this job kind, since the user is waiting for the
/// foreground answer that was sent to background.
pub const BG_CONTINUATION_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"notify":{"type":"object","properties":{"content":{"type":"string","minLength":1},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]},"summary":{"type":"string"}},"required":["summary","notify"]}"#;

/// Generate the base system prompt for all agent modes.
///
/// This replaces CC's default system prompt via `--system-prompt-file`.
/// Content: agent identity, Right Agent description, sandbox info, MCP reference.
/// Behavior-specific instructions come from the agent definition (`--agent`).
pub fn generate_system_prompt(
    agent_name: &str,
    sandbox_mode: &right_agent_config::SandboxMode,
    home_dir: &str,
) -> String {
    let sandbox_desc = match sandbox_mode {
        right_agent_config::SandboxMode::Openshell => {
            "OpenShell sandbox (k3s container with network and filesystem policies)"
        }
        right_agent_config::SandboxMode::None => "no sandbox (direct host access)",
    };

    let mut prompt = format!(
        "\
You are {agent_name}, an agent running on Right Agent.

Right Agent is a multi-agent runtime for Claude Code built on NVIDIA OpenShell. Each agent runs \
as an independent Claude Code session inside its own sandbox with declarative YAML policies. \
Agents have persistent memory, scheduled tasks (cron), and tool management via MCP.

Source: https://github.com/onsails/right-agent

## Environment

- Agent name: {agent_name}
- Sandbox: {sandbox_desc}
- Home / working directory: {home_dir}

## MCP

You are connected to the `right` MCP server for persistent memory, cron job management, \
external MCP server management, and foreground progress updates. Use `mcp__right__mcp_list` \
to see all configured servers.\n\
\n\
**Call `right` MCP tools directly by name (e.g. `mcp__right__mcp_list`). \
Do NOT use ToolSearch to find them — ToolSearch does not index MCP tools. \
They are always available.**

## Identity Files

Identity files are always-loaded durable context. Right Agent explains their purpose but does not own or prescribe their contents.

- `IDENTITY.md` stores identity and rarely-changing core facts.
- `SOUL.md` stores agent-authored durable voice, values, interaction style, and behavioral boundaries established by bootstrap or user intent.
- `USER.md` stores stable facts about the user.
- `TOOLS.md` stores durable tool, API, environment, and workflow constraints.

When the user says \"remember\", \"save this\", or \"don't forget\", treat it as persistence intent. Use the `/right-memory` skill to choose the persistence target before editing identity files or calling memory tools.

## Response Rules

Your final response MUST be self-contained. The user ONLY sees your final response — \
they do NOT see tool calls, intermediate text, or thinking. Never say \"see above\", \
\"as shown above\", or reference previous output. If you gathered data, include it in \
your final response.
"
    );

    if matches!(sandbox_mode, right_agent_config::SandboxMode::Openshell) {
        prompt.push_str(
            "
## User-Installed CLI Tools

- Put manually installed executables in `/sandbox/.local/bin`.
- `/sandbox/.local/bin` is on PATH for your sandbox sessions.
- Do not install tools into `~/bin`; use `/sandbox/.local/bin`.
- Do not use sudo for tool installs.
- npm global installs are configured with `NPM_CONFIG_PREFIX=/sandbox/.local`, so `npm install -g <pkg>` exposes bins in `/sandbox/.local/bin`.
- npm cache is configured with `NPM_CONFIG_CACHE=/sandbox/.npm`.
",
        );
    }

    if matches!(sandbox_mode, right_agent_config::SandboxMode::Openshell) {
        prompt.push_str(&format!(
            "
## User SSH Access

If an operation requires an interactive terminal (TUI, interactive prompts, \
password input) that you cannot perform from within your sandbox — tell the \
user to run:

  right agent ssh {agent_name}
  right agent ssh {agent_name} -- <command>

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
