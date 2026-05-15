# Learned Skills: Foreground Learning + Nudge Foundation

**Status:** Design approved, implementation pending

## Problem

Right Agent should learn reusable procedures from real work and show the user
visible value when that happens. The first implementation must be small enough
to ship, but it must not be a dead end. Hermes demonstrates why foreground
learning alone is not enough: the active agent can save or patch skills during
the conversation, but periodic nudges make learning an observable system loop
instead of pure prompt compliance.

The design therefore needs two layers from the start:

- Foreground learning: immediate skill creation/update by the active agent.
- Nudge foundation: durable signals, counters, provenance, and background
  review contracts that can power a future review worker without changing the
  foreground schema.

## Goals

- Add a built-in `right-learn-skill` authoring skill.
- Let the foreground agent create and update learned Agent Skills packages.
- Support full skill packages: `SKILL.md`, `scripts/`, `references/`, and
  `assets/`.
- Require `mcp__right__send_progress` before foreground skill file writes.
- Show user-visible learned/updated/used receipts without approval prompts.
- Persist provenance and nudge signals for future background review.
- Keep learned skill content sandbox-local in stage 1.
- Keep the background review worker optional initially while preserving its
  exact data contract.

## Non-goals

- No skill deletion.
- No umbrella skill generation.
- No curator loop for stale/archive/consolidation.
- No approval queue.
- No host import of arbitrary sandbox paths.
- No background review worker required for the first usable release.
- No mutation of platform, bundled, hub-installed, or codegen-owned skills.
- No new MCP write primitive for moving skill package files.

## Decision

Ship foreground learning first, but build the data model and structured output
schema as if background nudges already exist.

The foreground agent owns immediate learning:

1. It solves the user's task.
2. If the experience is reusable, it loads `right-learn-skill`.
3. It calls `mcp__right__send_progress` before writing or patching skill files.
4. It writes or updates a learned skill package in the agent-owned skills tree.
5. It returns a localized receipt in structured output.

The nudge foundation owns reliability:

1. Every foreground invocation records cheap counters and learning provenance.
2. If the foreground agent sees a concrete create/update candidate but does not
   publish a skill, it emits exactly one hidden nudge signal.
3. Signals and counters are persisted even when the background worker is
   disabled.
4. A later background worker can consume the same schema to create or patch
   learned skills from transcript evidence.

## Why Nudges Exist

Nudges are not the first value path. They are the reliability layer.

Foreground learning is prompt-guided. It can fail because skill authoring is a
secondary goal while the active agent is focused on solving the user's task.
Long tool-heavy turns, urgency, user corrections, and context pressure all make
the model more likely to finish the answer and skip persistence.

Nudges make learning observable. A counter threshold does not mean there is
definitely a skill to create. It means the episode was expensive enough that a
bounded review may be worth paying for. The reviewer can inspect the completed
trajectory: failed attempts, user correction, loaded skills, final verified
path, and whether the foreground agent missed a durable lesson.

This mirrors Hermes' architecture: Hermes has foreground skill management and
also a background review/nudge loop. Right keeps the same architectural shape
but starts with tighter write boundaries and sandbox-local learned content.

## Skill Storage

Learned skills use the standard Agent Skills package format and the standard
runtime discovery path:

```text
.claude/skills/<package_name>/
  SKILL.md
  scripts/
  references/
  assets/
```

For sandboxed agents this resolves inside the sandbox, normally:

```text
/sandbox/.claude/skills/<package_name>/
```

The package directory is agent-owned content. Platform/codegen installation may
refresh built-in skill directories, but it must not delete or overwrite
non-built-in skill directories. Existing code already preserves custom skill
directories under `.claude/skills/`; learned skills should use the same
preservation rule.

Learned skills should also update `.claude/skills/installed.json` using the
existing registry convention, with `source: "learned"` and
`path: ".claude/skills/<package_name>"`. Host metadata records provenance and
nudge state; `installed.json` remains the agent-local skill registry.

Host metadata may record that a skill exists, who created it, and which
invocation produced it. Host code must not copy arbitrary files from sandbox
paths into host-controlled locations in stage 1.

If a future MCP metadata hook is added, it accepts `package_name` only. It must
derive and validate the package path itself and never accept an absolute path
from the model.

## Built-in `right-learn-skill`

Add a built-in skill under:

```text
crates/right-codegen/skills/right-learn-skill/SKILL.md
```

The skill teaches the agent:

- when to create a learned skill;
- when to patch an existing learned skill;
- when not to learn;
- how to name packages;
- how to structure `SKILL.md`;
- when to use `scripts/`, `references/`, and `assets/`;
- how to keep activation descriptions specific and non-noisy;
- how to emit receipts and nudge signals;
- that it must call `mcp__right__send_progress` before any create/update file
  write.

The built-in skill must be installed alongside existing built-ins such as
`right-skills`, `right-cron`, and `right-mcp`.

## Foreground Learn Criteria

The foreground agent should create a new learned skill when one of these
conditions is true and the result is reusable across future sessions:

- `explicit_user_request`: the user asks to learn/save/remember the workflow.
- `multi_step_workflow`: the task required several non-obvious repeated steps.
- `recovered_surprise`: a command/tool/API failed or surprised the agent, then
  the agent found a verified reusable path.
- `user_correction`: the user corrected the approach and the correction is a
  durable gotcha.
- `repeated_tool_pattern`: the agent discovered a tool/API usage pattern likely
  to recur.

The foreground agent should update an existing learned skill when a loaded
learned skill is materially wrong or incomplete:

- `missing_step`
- `stale_command`
- `wrong_api_assumption`
- `overbroad_activation`
- `broken_script`
- `unsafe_instruction`

The foreground agent must not create a skill for:

- one-off task details;
- temporary project progress;
- facts better stored as memory;
- failed attempts that were not verified;
- generic advice not tied to a concrete Right Agent workflow;
- platform, bundled, or hub skill changes.

## Progress and Receipts

Before creating or updating package files, the agent must call:

```text
mcp__right__send_progress(message: string)
```

The message should be short and user-visible, for example:

```text
Learning a reusable skill for Notion database filters.
```

This is not an approval prompt. It only shows that value is being created.

If the progress call returns `rate_limited`, the agent may continue if a recent
progress message already exists for the invocation. If progress is unavailable
or any other progress error occurs, the agent should avoid writing the skill
and emit a nudge signal instead. The user's main task must still complete.

After a successful create/update, structured output includes `skill_receipt`:

```json
{
  "skill_receipt": {
    "kind": "created",
    "package_name": "notion-database-filters",
    "message": "<localized learned-skill receipt>"
  }
}
```

Allowed `kind` values:

- `created`
- `updated`

When a learned skill materially guides a later answer, the agent should return
`used_skill_receipts`:

```json
{
  "used_skill_receipts": [
    {
      "package_name": "notion-database-filters",
      "message": "<localized used-skill receipt>"
    }
  ]
}
```

Receipts are localized by the model, not hardcoded by the backend. The backend
only validates shape and displays the message.

## Structured Output Contract

Extend the foreground reply schema with optional fields:

```json
{
  "content": "Done.",
  "attachments": null,
  "reply_to_message_id": null,
  "skill_receipt": null,
  "used_skill_receipts": [],
  "learning_signal": null,
  "skill_issue_signal": null
}
```

`skill_receipt` and background signals are mutually exclusive:

- If `skill_receipt` is present, ignore `learning_signal` and
  `skill_issue_signal`.
- If both `learning_signal` and `skill_issue_signal` are present, drop both and
  log a schema violation.
- Most turns should set all skill fields to null or empty arrays.

## Nudge Signal Contract

Signals exist only for cases where the foreground agent sees a concrete
learning/update candidate but does not publish a skill.

All signals require:

- no `skill_receipt`;
- exactly one of `learning_signal` or `skill_issue_signal`;
- one create/update trigger from the foreground learn criteria;
- an allowed defer reason;
- at least two useful `event_refs`, or one explicit user request;
- a candidate reusable across future sessions.

Allowed defer reasons:

- `conversation_still_evolving`
- `needs_full_context_review`
- `write_or_publish_failed`
- `needs_existing_skill_diff`

Create candidate:

```json
{
  "learning_signal": {
    "kind": "create_candidate",
    "package_name_hint": "notion-database-filters",
    "trigger": "recovered_surprise",
    "reason_not_written": "needs_full_context_review",
    "event_refs": ["e17", "e19", "e21"],
    "summary": "Reusable Notion filter schema gotcha."
  }
}
```

Update candidate:

```json
{
  "skill_issue_signal": {
    "kind": "update_candidate",
    "skill_name": "notion-database-filters",
    "issue": "wrong_api_assumption",
    "reason_not_patched": "needs_existing_skill_diff",
    "observed_effect": "retry_after_user_correction",
    "event_refs": ["e12", "e14", "e18"],
    "patch_hint": "Use rich_text filter for this property."
  }
}
```

`learning_signal` can create only. `skill_issue_signal` can patch only. A
future background reviewer may still return `learned=false`.

## Provenance and Telemetry

Stage 1 must persist enough data for future nudges even if the background
worker is disabled.

For each foreground invocation, record:

- agent name;
- root session id;
- invocation id;
- Telegram chat/thread identity as internal ids, not exposed to the agent;
- started/finished timestamps;
- tool iteration count;
- whether any learned skill was loaded or used;
- whether `skill_receipt` was present;
- whether `used_skill_receipts` were present;
- any accepted `learning_signal` or `skill_issue_signal`;
- stable event refs for evidence.

Maintain cheap nudge counters:

- `tool_iters_since_review`
- `turns_since_review`
- `skill_issue_hints_since_review`
- `last_review_at`
- `review_running`

The initial worker can leave `review_running=false` permanently. The important
part is that the counters and signals exist and can be consumed later.

## Event References

Signals must reference events by stable ids, not JSONL line numbers. If the
stream log already has stable event ids, use them. Otherwise, derive
invocation-local ids during stream parsing and persist the mapping with the
invocation metadata.

Event refs should be able to identify:

- user messages;
- assistant messages;
- tool calls;
- tool results;
- command failures;
- retries;
- verification steps;
- final answer.

Nudge review must not rely on tail-only truncation or date-only filtering.
Those filters can drop early surprises or include long low-value spans.

## Future Background Review Contract

The background worker is not required for the first release, but its contract
is fixed now.

Inputs:

- full completed conversation transcript when within budget;
- fallback review bundle when full context is too large;
- invocation id and timestamps;
- timeline index with stable event ids;
- salient events selected by reason, not line count;
- one accepted `learning_signal` or `skill_issue_signal` when present;
- existing learned skill names;
- `right-learn-skill` authoring guidance.

Allowed actions:

- decide whether a reusable workflow exists;
- create learned skill package for `learning_signal`;
- patch learned skill package for `skill_issue_signal`;
- write only under the learned skill package path;
- return structured receipt text in the user's language.

Forbidden actions:

- ask the user questions;
- send user-facing progress messages;
- edit project files outside the learned skill package;
- edit platform, bundled, or hub skills;
- trigger another learning review;
- learn from its own review output.

The background worker sends no user-visible message unless it creates or
updates a skill. If it does create or update a skill, the bot sends the same
receipt style used by foreground learning.

## Nudge Cost Controls

Stage 1 stores the counters; later work can enable the worker. The review gate
should be able to use:

- explicit foreground signals;
- tool iteration thresholds;
- skill issue hints;
- no concurrent review already running;
- minimum interval between reviews;
- per-review model budget;
- per-day review count;
- per-day learning budget;
- coalescing of several busy turns into one review window.

Exact thresholds are implementation tuning, not part of this design. The
architectural requirement is that the nudge loop is budgeted, coalesced, and
silent unless it learns or repairs something.

## Security Boundaries

Learned skills are executable instructions and may include scripts. The design
therefore treats them as agent-owned code inside the agent sandbox.

Boundaries:

- Learned skills may create/update only non-built-in package directories.
- Platform, bundled, hub-installed, and codegen-owned skills are read-only to
  learned-skill flows.
- No API accepts an absolute sandbox path for host ingestion.
- Any future metadata API accepts package names only and validates paths under
  `.claude/skills/`.
- Background review, when enabled, gets a restricted tool surface.
- Background review must not have progress delivery.
- Prompt instructions are not the only guard. Path and source boundaries must
  be enforced in code.

## User Experience

No approvals.

Foreground create/update:

1. Agent sends one progress message before writing.
2. Agent completes the user's task.
3. Final response includes normal answer content.
4. Bot appends the learned/updated receipt as a compact line in the same final
   Telegram message when formatting allows.
5. If Telegram splitting makes same-message display awkward, the bot may send a
   single adjacent receipt message. It must not ask for approval.

Future background create/update:

1. User already received the main answer.
2. Background review runs silently.
3. If it learns or repairs a skill, bot sends one standalone receipt message.
4. If it does nothing, user sees nothing.

Used-skill notification:

- When a learned skill materially guides a future answer, the final response
  includes a used-skill receipt.
- Do not announce built-in skills as learned skills.
- Do not announce a skill that was loaded but did not materially affect the
  answer.

## Files and Documentation to Touch

Expected implementation areas:

- `crates/right-codegen/skills/right-learn-skill/SKILL.md`
- `crates/right-codegen/src/skills.rs`
- `crates/right-codegen/src/agent_def.rs`
- `crates/right-codegen/src/agent_def_tests.rs`
- foreground structured reply schema generation
- foreground structured reply parsing in `crates/bot/src/cc/worker_reply.rs`
- Telegram delivery path for receipts
- invocation metadata and nudge counter persistence
- stream/event ref mapping
- `PROMPT_SYSTEM.md`
- `docs/architecture/sandbox.md`
- `docs/architecture/sessions.md`
- possibly `docs/architecture/mcp.md` if a metadata MCP hook is added

If any MCP tool is added, update `with_instructions()` in both
`memory_server.rs` and `aggregator.rs`, and use the full CC-visible
`mcp__right__<tool>` name in agent-facing text.

## Testing Strategy

Use TDD for behavior changes.

Targeted tests:

- built-in skill installer includes `right-learn-skill`;
- installer preserves non-built-in learned skill directories;
- reply schema accepts absent skill fields;
- reply schema accepts valid `skill_receipt`;
- reply schema accepts `used_skill_receipts`;
- reply schema drops/logs both background signals when both are present;
- `skill_receipt` causes background signals to be ignored;
- signal validation requires allowed defer reason and event refs;
- progress instructions mention `mcp__right__send_progress` before skill writes;
- background/cron/reflection paths do not have progress available for learned
  skill authoring;
- nudge counters update from foreground invocations without running a worker;
- learned skill receipts are delivered without approval prompts.

Integration tests where practical:

- create a temporary agent directory with `.claude/skills/custom-skill`, run
  built-in skill installation, verify the custom skill remains;
- simulate a foreground structured response with `skill_receipt`, verify user
  delivery includes the receipt;
- simulate a foreground response with `learning_signal`, verify metadata is
  persisted but no user message is sent when worker is disabled.

Final verification for implementation must include:

```text
devenv shell -- cargo test --workspace
```

## Rollout

Existing agents must not need recreation.

On upgrade:

- install the new built-in `right-learn-skill` during normal codegen/sync;
- preserve all existing `.claude/skills/<name>/` directories not owned by
  platform/codegen;
- initialize new metadata/counter storage with empty defaults;
- keep background review disabled until explicitly enabled by a later feature;
- allow foreground learning immediately after the built-in skill is synced.

## Risks and Mitigations

Risk: noisy skill creation.

Mitigation: `right-learn-skill` must include strict create/update/skip
criteria, and default structured output is no skill and no signal.

Risk: prompt injection persists into learned skills.

Mitigation: write boundaries are enforced by package path and source type;
background worker has a restricted tool surface when enabled; no platform or
hub skill mutation.

Risk: foreground learning blocks the user's answer.

Mitigation: stage 1 relies on foreground only for obvious reusable workflows.
If packaging would be complex or conversation is still evolving, emit a nudge
signal instead of writing immediately.

Risk: background review later becomes expensive.

Mitigation: counters, coalescing, and budget fields are part of the foundation
before the worker is enabled.

Risk: receipts become noisy.

Mitigation: only show create/update receipts after successful publish, and only
show used-skill receipts when a learned skill materially guided the answer.

## Open Questions Deferred to Implementation

- Whether nudge counters live in the existing bot database or a new learned
  skills table group.
- Exact nudge thresholds and budgets.
- Exact background model/provider selection when the worker is enabled.
