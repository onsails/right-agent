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

/// JSON schema shared by every agent-authored standalone delivery.
///
/// Compile-time copy of `right_rich_content::rich_content_schema()`. The two
/// are asserted equivalent by the MCP-side parity test
/// `standalone_delivery_tools_share_authoritative_rich_constraints`
/// (crates/right/src/right_backend_tests.rs), which compares the tool schemas
/// against the authoritative crate schema; change constraints there first and
/// mirror them here.
macro_rules! rich_content_schema {
    () => {
        r##"{"oneOf":[{"type":"object","additionalProperties":false,"properties":{"text":{"type":"string","minLength":1,"maxLength":32768}},"required":["text"]},{"type":"object","additionalProperties":false,"properties":{"blocks":{"type":"array","minItems":1,"items":{"oneOf":[{"type":"object","additionalProperties":false,"properties":{"type":{"const":"paragraph"},"runs":{"$ref":"#/$defs/runs"}},"required":["type","runs"]},{"type":"object","additionalProperties":false,"properties":{"type":{"const":"heading"},"level":{"type":"integer","minimum":1,"maximum":3},"runs":{"$ref":"#/$defs/runs"}},"required":["type","level","runs"]},{"type":"object","additionalProperties":false,"properties":{"type":{"const":"list"},"ordered":{"type":"boolean"},"items":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,"properties":{"runs":{"$ref":"#/$defs/runs"}},"required":["runs"]}}},"required":["type","ordered","items"]},{"type":"object","additionalProperties":false,"properties":{"type":{"const":"quote"},"runs":{"$ref":"#/$defs/runs"}},"required":["type","runs"]},{"type":"object","additionalProperties":false,"properties":{"type":{"const":"code"},"text":{"type":"string","minLength":1,"maxLength":32768},"language":{"type":["string","null"]}},"required":["type","text"]},{"type":"object","additionalProperties":false,"properties":{"type":{"const":"table"},"rows":{"type":"array","minItems":1,"items":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,"properties":{"runs":{"type":"array","items":{"$ref":"#/$defs/run"}}},"required":["runs"]}}}},"required":["type","rows"]}]}}},"required":["blocks"]}]}"##
    };
}

macro_rules! rich_schema_defs {
    () => {
        r##", "$defs":{"run":{"type":"object","additionalProperties":false,"properties":{"text":{"type":"string","minLength":1,"maxLength":32768},"marks":{"type":["array","null"],"uniqueItems":true,"items":{"enum":["bold","italic","strikethrough","code"]}},"link":{"type":["string","null"],"pattern":"^(https?|tg):"}},"required":["text"],"allOf":[{"if":{"properties":{"marks":{"contains":{"const":"code"}}},"required":["marks"]},"then":{"properties":{"marks":{"maxItems":1},"link":{"type":"null"}}}}]},"runs":{"type":"array","minItems":1,"items":{"$ref":"#/$defs/run"}}}"##
    };
}

/// JSON schema for the structured reply format used by the Telegram bot.
pub const REPLY_SCHEMA_JSON: &str = concat!(
    r#"{"type":"object","additionalProperties":false,"properties":{"content":{"oneOf":["#,
    rich_content_schema!(),
    r#",{"type":"null"}]},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","additionalProperties":false,"properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}},"used_skill_receipts":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"package_name":{"type":"string","pattern":"^rightx-"},"message":{"type":"string","minLength":1}},"required":["package_name","message"]}}},"required":["content","used_skill_receipts"]"#,
    rich_schema_defs!(),
    "}"
);

/// JSON schema for bootstrap question and finalization modes.
pub const BOOTSTRAP_SCHEMA_JSON: &str = concat!(
    r#"{"type":"object","additionalProperties":false,"properties":{"content":{"oneOf":["#,
    rich_content_schema!(),
    r#",{"type":"null"}]},"bootstrap_complete":{"type":"boolean"},"bootstrap_stage":{"enum":["user_name","agent_name","nature","vibe","emoji","final"]},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","additionalProperties":false,"properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content","bootstrap_complete","bootstrap_stage"]"#,
    rich_schema_defs!(),
    "}"
);

/// JSON schema for cron job structured output.
pub const CRON_SCHEMA_JSON: &str = concat!(
    r#"{"type":"object","additionalProperties":false,"properties":{"delivery":{"oneOf":[{"type":"object","additionalProperties":false,"properties":{"kind":{"const":"notify"},"content":"#,
    rich_content_schema!(),
    r#","attachments":{"type":["array","null"],"items":{"type":"object","additionalProperties":false,"properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["kind","content"]},{"type":"object","additionalProperties":false,"properties":{"kind":{"const":"silent"},"reason":{"type":"string","minLength":1}},"required":["kind","reason"]}]},"run_note":{"type":"string"}},"required":["delivery","run_note"]"#,
    rich_schema_defs!(),
    "}"
);

/// Structured-output schema for background-continuation cron runs.
pub const BG_CONTINUATION_SCHEMA_JSON: &str = concat!(
    r#"{"type":"object","additionalProperties":false,"properties":{"delivery":{"type":"object","additionalProperties":false,"properties":{"kind":{"const":"notify"},"content":"#,
    rich_content_schema!(),
    r#","attachments":{"type":["array","null"],"items":{"type":"object","additionalProperties":false,"properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["kind","content"]},"run_note":{"type":"string"}},"required":["delivery","run_note"]"#,
    rich_schema_defs!(),
    "}"
);

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
pub fn generate_system_prompt(agent_name: &str, home_dir: &str) -> String {
    let mut prompt = format!(
        "\
You are {agent_name}, an agent running on Right Agent.

Right Agent is a multi-agent runtime for Claude Code built on microsandbox. Each agent runs \
as an independent Claude Code session inside its own hardware-isolated microVM with a network \
egress policy. Agents have persistent memory, scheduled tasks (cron), and tool management via MCP.

Source: https://github.com/onsails/right-agent

## Environment

- Agent name: {agent_name}
- Sandbox: microsandbox microVM (hardware-isolated VM with a network egress policy)
- Home / working directory: {home_dir}

## MCP

You are connected to the `right` MCP server for persistent memory, cron job management, \
external MCP server visibility, and foreground progress updates. Use \
`mcp__right__rightmeta__mcp_list` to see configured external servers.\n\
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
they do NOT see tool calls, intermediate text, or thinking. Only structured RichContent \
`content` is delivered; assistant text blocks never reach the user, and `content: null` \
sends nothing. Never say \"see above\" or reference previous output. If you gathered \
data, include it in `content`.

A turn is work done, then reported. When your reply promises an action you can take \
now, the turn is unfinished: take it with your tools, then report the result. Defer \
only work you cannot finish now, and only by scheduling a cron in the same turn; a \
promise backed by neither action nor a schedule leaves the turn incomplete.
"
    );

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

    prompt
}

#[cfg(test)]
#[path = "agent_def_tests.rs"]
mod tests;
