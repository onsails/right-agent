# Cron Delivery Contract — Make Telegram Delivery Explicit to Cron Agents

## Problem

Cron job sessions sometimes fail to deliver messages because the agent
doesn't realise that its structured output is the Telegram delivery
channel. Instead, it looks for a messaging tool to "send" the Telegram
message — typically a Composio integration if one is installed, a
browser tool otherwise — and either hangs, errors, or asks the user to
clarify which platform to use.

Two production incidents on 2026-05-11 illustrate the failure mode:

- **`andrey-brighty-reminder`** — prompt asked to "tag @brainsmith".
  The agent reasoned: *"I need to clarify which platform the user
  wants to use for tagging @brainsmith — whether it's Telegram, a
  task management system, or something else — since I don't have
  direct messaging capabilities without knowing the specific
  service."* It produced no `notify` and the run recorded
  `delivery_status: silent`.

- **`andrey-airbnb-reminder`** — same trigger phrase "тэгни
  @brainsmith". The agent searched Composio for Telegram tools,
  initiated an OAuth flow, sent the user an OAuth invite link,
  waited for the connection (transport error), and gave up.
  `delivery_status: silent`.

Cron run IDs: `1d15367d-8991-4d8e-9918-75ed5af8820b`,
`b0ff7c4f-0f84-407d-b9d3-21017f6bd737`.

### Root cause

Cron sessions receive the same composite system prompt as worker turns
(base prompt + `OPERATING_INSTRUCTIONS.md` + `IDENTITY/SOUL/USER/TOOLS`
+ MCP instructions). Nothing in that prompt tells the agent that
**when it runs as a cron, its structured output (per the attached
`--json-schema`) is the only delivery channel**. The delivery contract
exists in the runtime (`cron::parse_cron_output` reads `notify.content`
and `cron_delivery.rs` relays it verbatim) but is never communicated to
the agent's prompt.

Compounding factor: cron prompts written by the agent via
`mcp__right__cron_create` often carry imperative messaging verbs
("tag X", "send to Y", "ping me") inherited from the user's natural
phrasing. Those verbs prime the model to reach for an external
messaging tool even when the runtime contract is clear.

## Goals

1. Tell the cron agent, in the system prompt, that its structured
   output IS the Telegram delivery mechanism — no external tool needed.
2. Tell it that the cron turn has no live user — clarifying questions
   are wasted.
3. Reduce the chance that bad cron-prompt phrasing reaches execution
   in the first place by adding authoring guidance to `rightcron`
   SKILL.md.

## Non-Goals

- JSON schema changes (`CRON_SCHEMA_JSON`, `BG_CONTINUATION_SCHEMA_JSON`
  stay as-is).
- Runtime tool blocking (no `--disallowedTools` carve-out for Composio
  or browser tools). Those tools are legitimate inside crons for read
  operations; the problem is ambiguity, not the tools.
- Migration of existing `cron_specs.prompt` rows. Existing crons keep
  their stored prompts. They inherit the new delivery contract via
  the system prompt on their next run.
- Composio-specific language anywhere. Composio is incidental to the
  failure; durable contract phrasing names the runtime, not whichever
  tool the model reached for.
- New MCP tools, new schemas, new on-disk state, sandbox recreation.

## Design

### 1. New compiled-in template

File: `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`.

Exported from `right-codegen` alongside `OPERATING_INSTRUCTIONS` and
`BOOTSTRAP_INSTRUCTIONS`:

```rust
pub const CRON_INSTRUCTIONS: &str =
    include_str!("../templates/right/prompt/CRON_INSTRUCTIONS.md");
```

Template body:

```markdown
## Cron Delivery Contract

You are executing as a scheduled task — there is no live user at the
other end of this turn. Two rules differ from a normal chat turn:

### 1. Your structured output IS the Telegram message

Delivery happens automatically: the runtime reads your output (per the
attached JSON schema) and sends `notify.content` to Telegram. You don't
call a tool to deliver — you produce the text.

- Non-null `notify` with non-empty `content` → message delivered.
- Null `notify` (only valid when the schema permits it) → silent run.
  Put a short factual reason in `no_notify_reason` (e.g. "no changes
  since last run"). Silent runs are visible to the user via
  `mcp__right__cron_list_runs`.

Do not use external messaging tools or a browser to send Telegram
messages — the runtime is the only delivery path. Every such attempt
wastes budget and never reaches the user.

`@username` inside `notify.content` is plain text. The runtime sends
the message; the Telegram client renders the mention.

### 2. No clarifying questions

There is no live user to answer questions during this turn. If the
task is ambiguous:

- Pick a sensible default, do the work, and explain what you chose in
  `notify.content` so the user can correct it next turn.
- Or, if your schema permits, set `notify: null` with
  `no_notify_reason` describing what blocked you.

Don't end `notify.content` with a question expecting a reply — the
user receives a one-off cron message, not a chat.
```

The phrasing is schema-agnostic: "only valid when the schema permits
it" lets the same template cover both `CRON_SCHEMA_JSON` (silent
allowed) and `BG_CONTINUATION_SCHEMA_JSON` (`notify` required,
`minLength: 1`).

### 2. Prompt mode selector

Replace the `bootstrap_mode: bool` parameter of
`build_prompt_assembly_script` in `crates/bot/src/cc/prompt.rs` with a
local enum:

```rust
pub(crate) enum PromptMode {
    /// Worker turns and cron_delivery.rs relay turns.
    Normal,
    /// First-run bootstrap turn (writes IDENTITY/SOUL/USER files).
    Bootstrap,
    /// cron::execute_job — both CRON_SCHEMA_JSON and
    /// BG_CONTINUATION_SCHEMA_JSON runs.
    Cron,
}
```

Three exhaustive variants. Two bools would have an invalid
`bootstrap && cron` state; the enum makes the contract explicit.

### 3. Assembly behaviour per mode

| Mode | Sections after the base prompt |
|---|---|
| `Normal` | `## Operating Instructions` (compiled-in) → IDENTITY/SOUL/USER/TOOLS from disk → MCP instructions → memory (if `MemoryMode` is set) |
| `Bootstrap` | `## Bootstrap Instructions` (compiled-in). Unchanged from today. |
| `Cron` | `## Operating Instructions` (compiled-in) → **`## Cron Delivery Contract` (`CRON_INSTRUCTIONS`, compiled-in)** → IDENTITY/SOUL/USER/TOOLS from disk → MCP instructions. Memory section skipped (cron already skips memory injection — `cron.rs` passes `memory_mode = None`). |

The cron contract is appended after Operating Instructions and before
identity files. Reasoning:

- It belongs to the *platform* contract, so it sits with Operating
  Instructions.
- Placing it before identity keeps identity/personality blocks closest
  to the actual task, which preserves their salience.
- Prompt-cache stays warm across cron runs (template content is fixed
  per build).

### 4. Callsite changes

Three callsites, mechanical updates:

| File | Function | Mode |
|---|---|---|
| `crates/bot/src/cron.rs` | `run_cron_task` (sandbox branch) | `PromptMode::Cron` |
| `crates/bot/src/cron.rs` | `run_cron_task` (no-sandbox branch) | `PromptMode::Cron` |
| `crates/bot/src/telegram/worker.rs` | every `build_prompt_assembly_script` call | `PromptMode::Bootstrap` if the existing `bootstrap` flag is true, else `PromptMode::Normal` |
| `crates/bot/src/cron_delivery.rs` | delivery turn (Haiku relay) | `PromptMode::Normal` |

`cron_delivery.rs` runs a non-cron turn whose only job is to relay
the cron's `notify.content` verbatim to the user — the delivery
contract doesn't apply to it.

### 5. `rightcron` SKILL.md edit

File: `crates/right-codegen/skills/rightcron/SKILL.md`.

Add a new section "Writing Cron Prompts" between "Creating a Cron
Job" and "Editing a Cron Job":

```markdown
## Writing Cron Prompts

The cron runs as a separate, non-interactive session — its only
delivery channel is its structured output. Phrase the `prompt:` as a
**task that produces text**, not as an imperative messaging action.
Imperative verbs like "send", "tag", "notify", "ping" prime the cron
agent to look for an external messaging tool.

| User said                            | Store as                                                  |
|--------------------------------------|-----------------------------------------------------------|
| "Tag @bob with a reminder about X"   | "Output a reminder about X, mentioning @bob"              |
| "Send a message to @alice at 9am"    | "Output a heads-up about <topic>, addressed to @alice"   |
| "Ping me when Y happens"             | "Check Y. If it happened, output a notification about it" |
| "Notify the channel about Z"         | "Output a notification about Z"                           |

`@username` is fine as plain text — it ends up in the delivered
message and Telegram renders it as a mention. Don't strip the user's
content or schedule; only rephrase the delivery-imperative verbs.
```

The skill's `version:` frontmatter bumps from `3.2.0` to `3.3.0` to
mark the content change.

### 6. PROMPT_SYSTEM.md updates

- Replace the `bootstrap_mode: true/false` column in the "Callers"
  table with a `mode` column showing `Normal | Bootstrap | Cron`.
- Add a "Cron mode" subsection under "Prompt Structure" showing the
  cron layout (base → Operating Instructions → Cron Delivery
  Contract → identity files → MCP instructions, with memory skipped).
- Note `CRON_INSTRUCTIONS` is compiled-in via `include_str!`,
  identical lifecycle to `OPERATING_INSTRUCTIONS`.

### 7. Test coverage

In `crates/bot/src/cc/prompt.rs` tests:

- Migrate every existing call site (~12) from `bootstrap_mode: bool`
  to the new `PromptMode` enum. Mechanical; no behaviour shift.
- New positive test `cron_mode_includes_cron_delivery_contract` —
  assembles a script with `PromptMode::Cron` and asserts the output
  contains `## Cron Delivery Contract` and the marker phrase
  `structured output IS the Telegram message`.
- New negative test `normal_mode_omits_cron_delivery_contract` —
  ensures the contract does not leak into worker or delivery turns.
- New invariant test `cron_mode_does_not_emit_memory_section` —
  confirms the cron path still skips memory injection (regression
  guard).

`agent_def_tests.rs` needs no changes — `generate_system_prompt` is
unchanged.

## Upgrade & Migration

Pure code change. The new template is compiled into the binary via
`include_str!`. No on-disk state migrates, no sandbox is recreated,
no schema bump. On bot restart, every already-running cron spec
inherits the contract on its next run.

Per the codegen-category model in `ARCHITECTURE.md`: the new
compiled-in const is platform-side content with no per-agent
codegen output, so no registry entry is needed.

## Risks

- **Prompt drift.** The contract text duplicates information that is
  also encoded in the JSON schema (`notify.content` required when
  `notify` is non-null). If the schemas evolve, the template must
  follow. Mitigation: keep the wording schema-agnostic ("per the
  attached JSON schema"); concrete field names appear only where they
  must, and a single PROMPT_SYSTEM.md cross-reference points readers
  at both.
- **Skill versioning.** `rightcron` SKILL.md is bundled per-agent at
  init time and re-copied on platform-store sync. Existing agents
  pick up the new section on the next bot restart (platform-store is
  content-addressed). No agent-side action required.

## Cross-references

- `PROMPT_SYSTEM.md` — prompt assembly mechanics.
- `ARCHITECTURE.md` — § "Cron Schedule Kinds", § "Reflection
  Primitive", § "Upgrade & Migration Model".
- `crates/right-codegen/src/agent_def.rs` — `CRON_SCHEMA_JSON`,
  `BG_CONTINUATION_SCHEMA_JSON`.
- `crates/bot/src/cron.rs` — `run_cron_task`, `parse_cron_output`.
- `crates/bot/src/cron_delivery.rs` — Haiku relay turn.
- GitHub issue: <https://github.com/onsails/right-agent/issues/48>.
