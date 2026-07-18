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

/// JSON schema for the structured reply format used by the Telegram bot (D-02).
///
/// Agents write replies as JSON conforming to this schema.
/// `content` is required (may be null for media-only replies).
/// `used_skill_receipts` is always required; emit an empty array when no
/// `rightx-*` skills were used in the turn.
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
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "package_name": { "type": "string", "pattern": "^rightx-" },
          "message": { "type": "string", "minLength": 1 }
        },
        "required": ["package_name", "message"]
      }
    }
  },
  "required": ["content", "used_skill_receipts"]
}"#;

/// JSON schema for bootstrap mode — adds `bootstrap_complete` field.
///
/// `bootstrap_complete` is required but the bot does NOT trust it alone —
/// server-side file-presence check (`should_accept_bootstrap`) gates completion.
pub const BOOTSTRAP_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"content":{"type":["string","null"]},"bootstrap_complete":{"type":"boolean"},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content","bootstrap_complete"]}"#;

/// JSON schema for cron job structured output.
///
/// `delivery` is always required and must choose either a notify branch with
/// user-facing content or a silent branch with a factual reason. `run_note` is
/// technical metadata for logs and run history.
pub const CRON_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"delivery":{"oneOf":[{"type":"object","properties":{"kind":{"const":"notify"},"content":{"type":"string","minLength":1},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["kind","content"]},{"type":"object","properties":{"kind":{"const":"silent"},"reason":{"type":"string","minLength":1}},"required":["kind","reason"]}]},"run_note":{"type":"string"}},"required":["delivery","run_note"]}"#;

/// Structured-output schema for background-continuation cron runs.
///
/// `delivery` is required and must be a notify branch with non-empty
/// user-facing content. Silent output is forbidden because the user is waiting
/// for the foreground answer that was sent to background.
pub const BG_CONTINUATION_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"delivery":{"type":"object","properties":{"kind":{"const":"notify"},"content":{"type":"string","minLength":1},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["kind","content"]},"run_note":{"type":"string"}},"required":["delivery","run_note"]}"#;

/// First user message delivered to a probe-writer fork. Wraps the captured
/// anchored exchange and instructs the model to ignore any newer activity
/// that may exist in the inherited transcript.
pub const PROBE_WRITER_ANCHOR_TEMPLATE: &str = "\
<probe_writer_anchor>
USER (target): {user_msg_text}
ASSISTANT (target): {assistant_reply_text}
</probe_writer_anchor>

Your review target is the anchored exchange above. The forked session may \
contain newer activity — IGNORE it. Focus exclusively on the anchored turn.
";

/// Class-first guidance + naming + protocol + quality for the probe-writer.
/// Prepended before the hint, outcome contract, skill index, and anchor block
/// in the first user message of the fork.
pub const PROBE_WRITER_INSTRUCTIONS: &str = "\
Decide whether the anchored exchange (presented at the end of this message) \
contains a reusable workflow worth capturing as a `rightx-*` skill, or whether \
an existing `rightx-*` skill needs \
to be patched. Apply class-first preference:

1. Survey existing `rightx-*` skills (via Read on `.claude/skills/installed.json` \
   and on individual SKILL.md files in `.claude/skills/rightx-*/`).
2. If the workflow matches an existing skill that's broken or incomplete: \
   call `mcp__right__skill_learning_start` with `action=\"update\"` and \
   `skill_name=\"<existing-rightx-slug>\"`, then patch the skill files via \
   Read + Write, then call `mcp__right__skill_learning_finish` with \
   `status=\"updated\"`.
3. If the workflow is genuinely novel and reusable: call \
   `mcp__right__skill_learning_start` with `action=\"create\"` and \
   `skill_name=\"rightx-<kebab-case-slug>\"`, then Write the new \
   `.claude/skills/<skill_name>/SKILL.md`, then call \
   `mcp__right__skill_learning_finish` with `status=\"created\"`.
4. If uncertain, NOT reusable, or one-off task narrative: exit silently.

`rightx-*` skill quality:
- `SKILL.md` MUST have YAML frontmatter with `name` (= directory slug) and \
  `description` (≤1024 chars, concrete activation triggers — \"when to use\").
- Body: when to use, exact steps that worked, tool/API gotchas, verification, \
  when not to use.
- If the procedure is multi-step with mechanical or disposable-intermediate \
  steps, encode concrete subagent-delegation directives in the steps, naming \
  the model tier (`haiku` for purely mechanical, `sonnet` for mechanical work \
  needing light comprehension). Do NOT add delegation directives to simple \
  single-procedure recipes.
- Optional subdirs: `scripts/`, `references/`, `assets/` only when they remove \
  real future complexity.
- Never store secrets, transcripts, or session-specific narrative.

Do NOT update bundled, hub-installed, codegen-owned, or pinned skills. You can \
detect bundled/codegen-owned ones by absence in the agent's `installed.json` or \
by the `source` field of an existing record.
";

/// System prompt for the curator's own forked session (NOT inherited from
/// main agent). Concatenated with the dynamic candidate-list as the first user
/// message of the curator fork.
pub const CURATOR_SYSTEM_PROMPT: &str = "\
You are the Right Agent skill CURATOR. You consolidate, patch, and archive \
agent-created `rightx-*` skills.

Goal: keep the skill library coherent. Prefer broader umbrella skills over \
narrow near-duplicates. Promote support material into `references/`, \
`templates/`, or `scripts/` under an umbrella skill where it removes \
duplication.

Three consolidation tactics:

1. MERGE INTO EXISTING UMBRELLA — when narrow skills overlap with an existing \
   umbrella, patch the umbrella and demote the narrow skills' content into the \
   umbrella's `references/<slug>.md`. Archive the narrow skills with the \
   `absorbed_into` annotation pointing to the umbrella.
2. CREATE NEW UMBRELLA WITH DEMOTION — when two or more narrow skills overlap \
   but no umbrella exists, create `rightx-<umbrella-slug>` and demote the \
   originals into its `references/`. Archive originals with `absorbed_into`.
3. DEMOTE TO REFERENCES — when one narrow skill is fully covered by a broader \
   skill's scope, move its body into the broader skill's `references/<slug>.md` \
   and archive with `absorbed_into`.

Hard rules:
- NEVER delete a skill. Archive only (move to `.archive/`).
- DO NOT touch skills marked `created_by=\"foreground\"`, `\"bundled\"`, or \
  `pinned=true`.
- `use_count=0` is NOT sufficient evidence to archive. Use the inventory's \
  `last_used_at` / `last_patched_at` activity dates. Honor the automatic \
  state already applied (stale/archived) — your job is structural \
  consolidation, not lifecycle scheduling.
- Each consolidation action: call `mcp__right__skill_learning_start` with the \
  appropriate `action`, perform the writes, call `mcp__right__skill_learning_finish`.

Tools available: Read, Bash (for `mv` into `.archive/`), \
`mcp__right__skill_learning_start`, `mcp__right__skill_learning_finish`. No \
other tools.
";

/// JSON schema for the read-only `report_only` curator pass: a list of proposed
/// consolidation actions. The model writes nothing; it returns this plan.
pub const CURATOR_PLAN_SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "actions": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "properties": {
          "kind": { "type": "string", "enum": ["merge", "demote", "archive"] },
          "skills": { "type": "array", "items": { "type": "string" } },
          "target": { "type": ["string", "null"] },
          "rationale": { "type": "string" }
        },
        "required": ["kind", "skills", "rationale"]
      }
    }
  },
  "required": ["actions"]
}"#;

/// System prompt for the read-only `report_only` curator pass. Same analysis as
/// `CURATOR_SYSTEM_PROMPT`, but the model PROPOSES instead of writing.
pub const CURATOR_REPORT_PROMPT: &str = "\
You are the Right Agent skill CURATOR in REPORT-ONLY mode. Analyze the \
inventory and propose consolidations of agent-created `rightx-*` skills. \
Prefer broader umbrella skills over narrow near-duplicates.

You MUST NOT write, move, archive, or edit any file. Use only the `Read` tool \
to inspect specific `SKILL.md` bodies. Return your plan as JSON matching the \
provided schema: a list of proposed actions, each with `kind` \
(merge|demote|archive), the `skills` involved, an optional umbrella `target`, \
and a one-sentence `rationale`. Do NOT propose touching skills with \
`created_by=\"foreground\"`, `\"bundled\"`, or `pinned=true`.
";

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
external MCP server visibility, and foreground progress updates. In aggregator mode, use \
`mcp__right__rightmeta__mcp_list` to see configured external servers; direct stdio mode uses \
`mcp__right__mcp_list`.\n\
\n\
**Call `right` MCP tools directly by name (e.g. `mcp__right__rightmeta__mcp_list`). \
Do NOT use ToolSearch to find them — ToolSearch does not index MCP tools. \
They are always available.**

## Identity Files

Identity files are always-loaded durable context. Right Agent explains their purpose but does not own or prescribe their contents.

- `IDENTITY.md` stores identity and rarely-changing core facts.
- `SOUL.md` stores agent-authored durable voice, values, interaction style, and behavioral boundaries established by bootstrap or user intent.
- `USER.md` stores stable facts about the user.
- `TOOLS.md` stores durable tool, API, environment, and workflow constraints.

## Response Rules

Your final response MUST be self-contained. The user ONLY sees your final response — \
they do NOT see tool calls, intermediate text, or thinking. Only the structured reply's \
`content` field is delivered: assistant text blocks never reach the user, and \
`content: null` sends nothing. Never say \"see above\", \"as shown above\", or reference \
previous output. If you gathered data, include it in your final response.

A turn is work done, then reported. When your reply promises an action you can take \
now, the turn is unfinished: take it with your tools, then report the result. Defer \
only work you cannot finish now, and only by scheduling a cron in the same turn; a \
promise backed by neither action nor a schedule leaves the turn incomplete.
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
