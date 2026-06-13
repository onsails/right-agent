# Inline Skill Authoring + Cron "What, Not How" — Design

> Status: brainstorm spec. Pending user review before writing the
> implementation plan.

## Context

Right Agent learns reusable `rightx-*` skills through one runtime path: the
**asynchronous post-turn pipeline** (Haiku prefilter → probe-writer fork →
periodic curator). It runs after foreground `Normal` turns and after
successful **recurring** cron runs (`bot::learning_pipeline::run_post_turn`,
both call sites). One-shot cron runs are excluded.

A second, agent-driven path exists but is switched off by policy. The
`right-learn-skill` skill already drives the `mcp__right__skill_learning_start`
/ `_finish` protocol from inside a live session, and the foreground invocation
is already `is_learning_capable`. The skill's own text hard-gates it:
"Use ONLY when the user explicitly asks … do not invoke this skill based on
your own judgment that a workflow might be reusable." So today the agent can
only write a skill mid-conversation when the user explicitly says "save this";
its own judgment never triggers a write.

Separately, cron `prompt:` text tends to encode procedure ("how") inline. That
procedure rots, can't be improved centrally, and duplicates what the skill
system is for.

## Problem

1. **Cron prompts mix "what" and "how".** A cron `prompt:` should carry the
   *goal* and trust skills to supply the *procedure*. Today nothing teaches
   that split (the existing "Writing Cron Prompts" section only covers
   delivery-imperative phrasing — "send"/"tag" → "output").
2. **The agent cannot self-author skills mid-turn.** Only an explicit user
   request or the async probe writes skills. The live session — which holds the
   richest context about a just-verified "how" — is forbidden from acting on
   its own judgment.
3. **Cron sessions cannot author skills at all** (inline). A cron CC invocation
   registers no learning-capable progress invocation, so
   `skill_learning_start` returns `learning_unavailable`. Only the *async*
   probe covers recurring crons.

## Goals

- Teach the cron skill to write **what** (goal/outcome) and push **how**
  (procedure) into `rightx-*` skills invoked by intent.
- Let the agent author/patch `rightx-*` skills **mid-conversation on its own
  judgment**, in both foreground and cron sessions — not only on explicit user
  request or via the async probe.
- Avoid double-writes: when a skill is created/patched during a turn, the
  async probe for that same turn does not run.

## Non-Goals

- No change to the async prefilter/probe-writer/curator pipeline behavior
  beyond the single "skip this turn" coordination signal.
- No numeric budget cap for inline self-writes in v1 (see Budget below).
- No change to the curator, lifecycle transitions, or dashboard pin surface.
- No change to which skills are eligible (`rightx-*` only; bundled / hub /
  manual / codegen-owned skills stay off-limits to the learning flow).

## Design

Six components. Foreground inline authoring is mostly a prompt-text change;
cron inline authoring is the bulk of the runtime work.

### Component 1 — `right-cron/SKILL.md`: "what, not how"

Add a subsection under "Writing Cron Prompts":

- A cron `prompt:` states the **goal/outcome**. Repeatable procedure lives in a
  `rightx-*` skill the cron loads by intent at fire time. Do **not** inline
  brittle step-by-step procedure into the cron prompt — it rots and can't be
  improved centrally.
- When a cron's "how" is non-trivial, the foreground session that creates the
  cron first ensures a `rightx-*` skill captures the procedure (Component 2),
  then writes the cron's "what". At fire time the cron loads the skill and
  executes. **This already works today** — no cron self-write required for the
  common case.
- Cross-reference `right-learn-skill` for capturing the "how".

This is the only `right-cron` change. Keep it within the prompt-tier brevity
rule (the skill ships to cron invocations).

### Component 2 — `right-learn-skill/SKILL.md`: relax the gate (mechanism 1)

Replace the explicit-user-only gate with a self-judgment trigger:

- **Invoke on** an explicit user request **or** the agent's own judgment when,
  *during this turn*, it verified a reusable "how": non-trivial multi-step
  work, concrete gotchas / exact commands / API quirks, confidence it will
  recur. Strongest trigger: "I just corrected a wrong approach and now know the
  right one."
- **Keep the skip rules** verbatim (one-offs, unverified guesses, trivial
  single-step tasks, failed attempts without a verified path).
- **Keep the `skill_learning_start` / `_finish` protocol** and the `rightx-*`
  package shape unchanged.
- Update the `description` (CSO) to surface self-judgment triggers **without
  summarizing the workflow** (per writing-skills CSO guidance) — e.g. "Use when
  you verified a reusable procedure this turn, or the user asks to save/fix a
  skill."
- Add a one-line note: the post-turn probe defers to you on any turn where you
  authored or patched a skill.

### Component 3 — Probe coordination: skip on same-turn write (runtime)

- Add `skill_written_this_turn: bool` to `ProbeAnchor`
  (`crates/bot/src/telegram/worker.rs`).
- Both anchor-capture sites set it: query `skill_learning_events` for a
  `finish` row with `status ∈ {created, updated}` tied to **this turn's
  invocation_id** since turn start. Foreground worker and
  `bot::cron::execute_job` each set the flag.
- `run_post_turn` (`crates/bot/src/learning_pipeline.rs`) returns early — before
  the budget gate and prefilter — when the flag is set. Full skip, no
  `learning_skip` row (this is not a budget skip).

This makes the async probe the safety net for turns where the agent did **not**
self-capture, never a second writer on a turn the agent already handled.

### Component 4 — Cron inline authoring (runtime)

Cron sessions cannot author skills today. Enable it:

- **New learning-capable invocation kind** for cron — `ProgressInvocationKind`
  gains a `Cron` variant. `is_learning_capable()` includes it;
  `sends_learning_messages()` does **not** (no live user — the skill persists
  silently, no user-visible learning receipt). Mirror in
  `ProgressInvocationKindDto`.
- **Register the cron CC invocation** with that kind, with a per-invocation
  mcp-config carrying `X-Right-Invocation` (same mechanism the probe-writer /
  curator use via `register_non_foreground_invocation`).
- **Whitelist** `skill_learning_start`, `skill_learning_finish`, `Write`,
  `Read` in the cron invocation's tool set so the inline write can happen.
- Cron inline writes obey the same `right-learn-skill` trigger and skip rules.
  Component 3's flag then suppresses the async post-cron probe for that run.

### Component 5 — Provenance: `cron` (runtime)

`right_lifecycle::CreatedBy` gains a `Cron` variant (DB code e.g. `c`). Cron
inline writes stamp `created_by = cron`; foreground inline writes keep
`foreground`. Curator skip rules and dashboard provenance labels learn the new
value. Distinguishing cron-authored skills keeps lifecycle/dashboard analytics
honest.

### Component 6 — Docs

- `PROMPT_SYSTEM.md` — sync the relaxed `right-learn-skill` policy and the cron
  "what, not how" guidance.
- `docs/architecture/learning.md` — document the inline self-write path, the
  same-turn probe-skip, the `Cron` invocation kind, and `cron` provenance.
- `ARCHITECTURE.md` — one-line invariant: the async probe does not run on a
  turn where the agent authored/patched a skill. (Clears the rule/enforcement/
  brevity tests; everything else stays in `learning.md`.)

## Budget & Metering

Inline self-writes (foreground and cron) run in the live turn, not a separate
fork, so they are **not** metered against `learning.max_daily_budget_usd` —
that budget gates the probe-writer fork's spend. The live turn's tokens are the
only cost. v1 adds no separate numeric cap on inline writes; the
`right-learn-skill` skip rules and the agent's judgment are the control. The
probe budget gate is unchanged.

`skill_spend` create/patch rows are written today by the probe-writer joined by
`invocation_id`. For inline writes there is no separate fork invocation. v1
decision: record the lifecycle event (`skill_learning_events` finish) and
provenance, but do **not** synthesize `create`/`patch` `skill_spend` rows for
inline writes (their cost is folded into the live turn's `usage_events`). The
implementation plan confirms this against the dashboard spend bucketing.

## Testing Strategy

TDD cadence per `AGENTS.md`: targeted tests during the loop, one full workspace
run at the end.

- **Component 3 (probe-skip):** unit test that `run_post_turn` returns early
  when `skill_written_this_turn` is set (no budget skip row, no prefilter). Test
  the anchor-capture flag logic against `skill_learning_events` fixtures.
- **Component 4 (cron kind):** `is_learning_capable`/`sends_learning_messages`
  table test includes `Cron`. Test that a cron-kind invocation passes the
  `skill_learning_start` gate and a non-registered cron invocation still fails
  closed.
- **Component 5 (provenance):** round-trip `CreatedBy::Cron` ↔ DB code; curator
  skip / dashboard label tests cover the new value.
- **Components 1–2 (skill text):** if `right-codegen` has skill-presence /
  contract tests, assert the new guidance lines and the relaxed description are
  present; otherwise a behavioral subagent check that the agent self-authors on
  a verified-how scenario and skips on a one-off (writing-skills RED/GREEN).
- **Docs:** keep `ARCHITECTURE.md` under the 40k budget;
  `registry_covers_all_per_agent_writes` unaffected (no new codegen output).

## Open Questions

None blocking. Provenance value (`cron`), staging (single stage, both paths),
and probe-skip semantics (full skip) are decided.
