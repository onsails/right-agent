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

### 2. Automatic writer — consolidate, then add the directive (Option 2)

**Drift discovered during planning.** The automatic-writer prompt exists in two
divergent places:

- **Canonical-but-dead:** `right_codegen::PROBE_WRITER_ANCHOR_TEMPLATE` +
  `PROBE_WRITER_INSTRUCTIONS` (`crates/right-codegen/src/agent_def.rs:90-137`).
  Rich content with a full "rightx-* skill quality" section. Exported, tested
  (`agent_def_tests.rs`), and documented in `PROMPT_SYSTEM.md` as the source of
  truth — but **nothing consumes its content at runtime** (its marker
  `"USER (target):"` appears nowhere else).
- **Live-but-thin:** `bot::learning_probe_writer::build_user_prompt` builds the
  actual runtime prompt inline (emits `"USER:"`). It has the hint-branching and
  `hint_outcome` machinery but **no skill-quality section at all**.

**Fix the drift first.** Rewire `build_user_prompt` to compose the runtime prompt
from the canonical constants — `PROBE_WRITER_INSTRUCTIONS` (general class-first
protocol + quality) followed by the bot-specific hint block, the `hint_outcome`
instruction, the skill index, and finally `PROBE_WRITER_ANCHOR_TEMPLATE` (with
`{user_msg_text}`/`{assistant_reply_text}` substituted). This collapses the two
definitions into one source of truth that matches `PROMPT_SYSTEM.md`, and the
automatic writer **gains the quality section it currently lacks**.

Constraints on the rewire:

- Preserve the existing hint mechanics and exact tokens the tests pin:
  `PREFILTER HINT: patch_existing` / `create_new`, `TARGET SKILL:`, `TOPIC HINT:`,
  `REASON:`, `hint_outcome`, and the empty-index placeholder
  `"no existing rightx-* skills"`.
- Trim the hint block's redundant create/patch/exit restatement (now covered by
  `PROBE_WRITER_INSTRUCTIONS`) down to the prefilter's specific recommendation
  plus "verify against the protocol above; apply or override."
- **Fix a latent bug while reviving:** `PROBE_WRITER_INSTRUCTIONS` step 2 says
  "patch the skill files via Edit/Write," but the probe-writer's `allowed_tools`
  has no `Edit` (only `Write`, `Read`, `Bash`). Change to "via Read + Write."

**Then add the delegation directive** to `PROBE_WRITER_INSTRUCTIONS`'s
"rightx-* skill quality" bullets:

> If the procedure is multi-step with mechanical or disposable-intermediate
> steps, encode concrete subagent-delegation directives in the steps, naming the
> model tier (`haiku` for purely mechanical, `sonnet` for mechanical work needing
> light comprehension). Do NOT add delegation directives to simple
> single-procedure recipes.

Because the directive lives in `PROBE_WRITER_INSTRUCTIONS` (now actually sent),
no separate inline reminder in `build_user_prompt` is needed — the consolidation
removes the earlier uncertainty about prompt inheritance.

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
  Document (a) that `build_user_prompt` now composes from
  `PROBE_WRITER_ANCHOR_TEMPLATE` + `PROBE_WRITER_INSTRUCTIONS` (no longer an
  inline duplicate), (b) the delegation-authoring behavior, and (c) the
  three-tier model ladder.
- **Tests:**
  - Extend `learning_probe_writer` tests so `build_user_prompt` output contains
    the composed `PROBE_WRITER_INSTRUCTIONS` content (e.g. `"Survey"`) and the
    anchor markers, for both create and patch hints, while still containing the
    pinned hint tokens and `hint_outcome`.
  - Extend `agent_def_tests.rs::probe_writer_instructions_contain_class_first_guidance`
    to assert the delegation directive (`haiku`/`sonnet`) and that the text no
    longer says `Edit/Write`.
  - Extend `skills.rs::right_learn_skill_mentions_protocol_and_boundaries` (or add
    a sibling test) to assert the SKILL.md delegation bullet.
  - Add a test asserting `OPERATING_INSTRUCTIONS` contains both `model: "haiku"`
    and `model: "sonnet"` tiers.

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
