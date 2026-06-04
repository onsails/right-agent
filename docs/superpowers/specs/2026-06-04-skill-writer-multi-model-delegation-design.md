# Skill-Writer Multi-Model Delegation Awareness — Design

**Date:** 2026-06-04
**Status:** Approved, pending implementation plan

## Problem

RightClaw's skill writer authors `rightx-*` skills through two surfaces:

1. **Automatic** — the post-turn probe-writer fork
   (`crates/bot/src/learning_probe_writer.rs::build_user_prompt`). Its prompt is
   the sole authoring instruction for routine learning.
2. **Explicit** — `crates/right-codegen/skills/right-learn-skill/SKILL.md`,
   loaded only when the user directs the agent to save/fix a skill. Its "Skill
   Quality" section governs what goes into a written `SKILL.md`.

Both surfaces record "exact steps that worked" but neither pre-marks which steps
a future agent should hand to a cheaper subagent. The writer saw the actual turn
and knows the procedure's shape — which steps are mechanical, which produce
intermediates that don't matter to the final outcome. A future agent re-deriving
that delegation decision from scratch, possibly in a thinner context, wastes both
judgment and tokens.

Separately, `OPERATING_INSTRUCTIONS.md` already teaches runtime delegation but
sanctions only one cheap tier (`sonnet`). There is no `haiku` rung for purely
mechanical work.

This change is **authoring guidance**, not runtime mechanics. Agents already know
*how* to dispatch subagents (`OPERATING_INSTRUCTIONS.md` lines ~70–97). The gap is
that the writer does not bake the delegation decision it already observed into the
skills it produces.

## Scope

In scope:

- Add a `haiku` rung to the canonical model-tier ladder in
  `OPERATING_INSTRUCTIONS.md`.
- Teach both writer surfaces to bake **concrete** delegation directives into
  multi-step skills that have mechanical or disposable-intermediate steps.
- Keep simple single-procedure recipes free of delegation boilerplate.
- Sync `PROMPT_SYSTEM.md` and tests.

Out of scope:

- Teaching the agent how to delegate at runtime (already covered).
- Introducing `haiku` anywhere except the one ladder and the written-skill
  directives.
- The prefilter and curator — they do not author skill content.

## Model-tier ladder (canonical)

The ladder lives once, in `OPERATING_INSTRUCTIONS.md`. Both writer surfaces
reference it.

| Tier | When | Examples |
|------|------|----------|
| `model: "haiku"` | Purely mechanical, easily-verifiable output | Format conversion, field extraction, mechanical file reads |
| `model: "sonnet"` | Mechanical but needs light comprehension | Summarization, source sweeps, research loops where only the conclusion matters |
| default / main | Judgment calls | Design decisions, ambiguous-spec interpretation, anything you want your strongest model on |

Downgrades are free savings when the agent's main model is opus; the `sonnet`
rung is a no-op when main is already sonnet; `haiku` is always a downgrade.

The main session reviews subagent output before using it, so `haiku`'s weakness
is bounded to steps whose output the main session can cheaply verify.

## Design — Approach A (DRY ladder + thin writer hooks)

Chosen over two alternatives:

- **B — self-contained writers** (restate the full ladder in every surface):
  duplicates the ladder in three places, violating the repo's "no duplication
  across the composite prompt" rule and prompt-tier brevity. Rejected.
- **C — shared reference doc skills link to**: over-engineered for three
  sentences. Rejected (YAGNI).

### 1. `OPERATING_INSTRUCTIONS.md` — add the haiku rung

Rework the existing single-tier sentence (~line 97) into the three-tier ladder
above, stated in ~3 sentences to respect prompt-tier brevity. This is the
canonical ladder both writer surfaces point at.

This file is prompt-tier — paid on every turn — so the rewrite must not grow
materially beyond the sentence it replaces.

### 2. Automatic writer — `build_user_prompt`

Add one instruction in the **shared** block (after the hint branches, alongside
the existing `hint_outcome` instruction) so it applies to both `create` and
`patch` hints:

> When the captured procedure is multi-step with mechanical or
> disposable-intermediate steps, write those steps as concrete subagent-delegation
> directives naming the model tier (`haiku` for purely mechanical, `sonnet` for
> mechanical work needing light comprehension). Do not add delegation boilerplate
> to simple single-procedure recipes.

Include a compact one-line tier reminder inline. Rationale: the probe-writer runs
as a forked/resumed session, and it is not yet confirmed that the fork inherits
`OPERATING_INSTRUCTIONS.md` verbatim into its system prompt. Writer output quality
matters, so the prompt restates the tiers in one line rather than relying on
inheritance.

**Implementation note:** during implementation, verify whether the forked
probe-writer session inherits the composite system prompt (and thus the ladder).
If confirmed, shrink the inline reminder to a pointer ("per the operating
instructions' model ladder") to avoid duplication.

### 3. Explicit writer — `right-learn-skill/SKILL.md`

Add a bullet to the "Skill Quality" section mirroring (2). The agent invoking
this skill is in a normal foreground turn and always has
`OPERATING_INSTRUCTIONS.md` in context, so this bullet references the ladder
rather than restating it.

### Hard gate against bloat

Every surface states the negative explicitly: **simple single-procedure recipes
get no delegation directives.** A delegation directive appears only when steps are
genuinely mechanical or produce intermediates that do not matter to the outcome.

## Sync obligations

- **`PROMPT_SYSTEM.md`** — mandatory update when prompt generation changes.
  Document the writer's delegation-authoring behavior and the three-tier ladder.
- **Tests:**
  - Extend `learning_probe_writer` tests to assert the delegation instruction is
    present in `build_user_prompt` output (hint-agnostic — present for both create
    and patch hints).
  - Add a test asserting the `right-learn-skill/SKILL.md` "Skill Quality" bullet
    exists (or extend an existing SKILL.md content test if one exists).
  - Confirm `agent_def_tests.rs` still passes with the reworded
    `OPERATING_INSTRUCTIONS.md` line; update any exact-content assertion it holds
    on that line.

## Verification cadence

- Targeted during implementation:
  `devenv shell -- cargo test -p bot learning_probe_writer` and
  `devenv shell -- cargo test -p right-codegen` (settings/agent_def).
- Final, mandatory: `devenv shell -- cargo test --workspace`.

## Non-conflict note

The user's global "never use haiku for subagents" rule governs the development
assistant during repo work. This design authorizes `haiku` for the RightClaw
**runtime agents'** learned skills — a distinct context the user explicitly
approved. The two do not conflict.
