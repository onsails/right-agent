# Cron skill learning — design

**Date:** 2026-06-02
**Status:** approved, pre-implementation

## Problem

The per-turn skill-learning pipeline (anchor capture → Haiku prefilter →
probe-writer fork → curator) only runs on **foreground `PromptMode::Normal`
turns**. The gate is explicit at `crates/bot/src/telegram/worker.rs:2131-2132`:

```rust
&& ctx.learning.prefilter_enabled
&& matches!(cc_prompt_mode, Some(crate::cc::prompt::PromptMode::Normal))
```

Cron jobs run as `PromptMode::Cron` (`crates/bot/src/cron.rs:624,660`) and
`crates/bot/src/cron.rs` contains **no** reference to `ProbeAnchor`, the
prefilter, or the probe-writer. Cron turns are therefore a blind spot: no
anchor is captured, the prefilter never runs, and the probe-writer never
forks.

Consequence (observed: `github-tracker` cron, 2026-06-02): a recurring job
regressed from a $0.34 hot-path run to a $1.02 run that detoured through
Composio. The cheap, correct recipe was demonstrated in-session but never
persisted as a skill, because cron turns are invisible to the learner. No
amount of repetition can teach a skill from a path the pipeline never
observes.

## Goal

Let recurring cron runs feed the existing skill-learning pipeline, so a
stable repeating task can be codified into a `rightx-*` skill and stop
regressing across runs.

## Scope

- **In:** recurring crons (`ScheduleKind::Recurring`).
- **Out:** one-shot crons (`ScheduleKind::Immediate`, `ScheduleKind::RunAt`)
  — they do not repeat, so a learned skill cannot amortize.
- **Out (v1):** spike/anomaly gating, cheap-run exemplar selection,
  cron-specific baselines, cron usage-receipt attribution. These are
  deliberate deferrals (see "Deferred").

## Design

Mirror the foreground pipeline. Reuse every downstream component verbatim;
the only new inputs are a cron-built `ProbeAnchor` and a shared entry point.

### 1. Extract the shared post-turn pipeline

The worker's inline post-turn learning block
(`worker.rs:2128-2271` — budget gate → today-spend query → prefilter →
decision→hint mapping → skill-index collection → probe-writer fork) is
extracted into a single function, e.g.:

```
learning_pipeline::run_post_turn(ctx: PostTurnLearningCtx, anchor: ProbeAnchor)
```

Worker and cron both call it. This is the one refactor; it prevents the two
call sites from drifting.

**Precondition boundary:** the per-call-site eligibility check stays at the
call site — worker keeps its `PromptMode::Normal` guard, cron applies its
`Recurring` guard — and `run_post_turn` is invoked only once that passes.
The shared function begins at the **budget gate** and owns everything from
there down: today-spend query, `learning_skip` row on exhaustion, prefilter,
decision→hint mapping, skill-index collection, probe-writer fork
(fire-and-forget spawn, session mutex on the anchor's `main_session_uuid`).
Behavior is byte-for-byte the current worker block for the foreground path.

`PostTurnLearningCtx` carries the fields the block already closes over:
`agent_dir`, `agent_db_dir`, `agent_name`, `ssh_config_path`,
`resolved_sandbox`, `internal_client`, `learning` config, `prefilter_model`,
`probe_writer_model_override`, `model_arc`, `debug_flag`, `session_locks`.

### 2. Anchor capture in cron

In `cron.rs::execute_job`, **after** `consume_cron_stream` reports success
and **only when** `spec.schedule_kind` is `Recurring`, build a `ProbeAnchor`:

| field | cron source |
|-------|-------------|
| `user_msg_text` | the cron job prompt text |
| `assistant_reply_text` | the run's final `result` text |
| `main_session_uuid` | `run_id` (the cron run's CC session id) |
| `captured_at` | now (UTC) |
| `chat_id` / `thread_id` | the job's delivery target |
| `num_turns` | from the result event (already parsed near `cron.rs:1317`) |
| `total_cost_usd` | from the result event |
| `wall_elapsed_ms` | measured around the run (spawn → result) |
| `used_skill_receipts` | `vec![]` (v1 — cron has no reply-schema receipts) |

Then `learning_pipeline::run_post_turn(ctx, anchor)`, fire-and-forget.

The cron CC session is created fresh per run (`cron.rs:560` —
`resume_session_id: None, new_session_id: run_id, fork_session: false`), so
`run_id` is a valid, resumable session id immediately after the run within
the same sandbox.

### 3. Gate (cron variant)

The budget gate inside `run_post_turn` is identical for both call sites. The
difference is only the call-site precondition: cron applies `Recurring`
where worker applies `PromptMode::Normal`. Cron gating is:

- `learning.prefilter_enabled`
- `spec.schedule_kind == Recurring` (enforced at the call site, before the
  anchor is built)
- today's spend across `LEARNING_SOURCES`
  (`learning_prefilter`, `learning_probe_writer`, `learning_curator`) below
  `learning.max_daily_budget_usd` (default $1.00). On exhaustion, write a
  `learning_skip(reason='budget', intended_kind=NULL)` row — identical to
  foreground.

No new `LEARNING_SOURCES` entry: cron learning runs **are**
`learning_prefilter` + `learning_probe_writer` invocations and are already
covered by the budget gate and dashboard sources array. The cron job's own
run cost is ordinary cron spend, not learning spend, so a $1+ job run does
not by itself exhaust the learning budget.

### 4. Probe-writer fork

Unchanged. `learning_probe_writer::run` forks `anchor.main_session_uuid`
(`--fork-session --resume`, `learning_probe_writer.rs:117-119,192`). With
`main_session_uuid = run_id` and the same `ssh_config_path` /
`resolved_sandbox` the cron run used, the fork resumes the cron run's
session inside its sandbox. The writer already acquires the session mutex on
that uuid.

### 5. Baseline — explicit v1 simplification

The prefilter prompt embeds P50/P90/P99 from
`right_agent::usage::turn_baseline::compute`, which aggregates **foreground**
turns. Cron runs are typically pricier and longer, so a cron run will often
read as above-P90. v1 **reuses the foreground baseline as-is** rather than
introducing cron-specific baselines.

Effect: cost reads as high → a mild bias toward learning. This is
acceptable because the Haiku prefilter gates on **reusability** (is there a
codifiable procedure, and does a matching skill already exist?), not on cost
alone, and the daily budget caps any runaway. If cron over-triggers in
practice, the follow-up is per-job or cron-population baselines — out of
scope here.

## Data flow

```
recurring cron tick
  → execute_job runs claude -p (PromptMode::Cron, new session = run_id)
  → consume_cron_stream → Success { result text, num_turns, total_cost_usd }
  → [schedule_kind == Recurring] build ProbeAnchor{ main_session_uuid=run_id, … }
  → learning_pipeline::run_post_turn(ctx, anchor)   (fire-and-forget)
       → budget gate (LEARNING_SOURCES vs max_daily_budget_usd)
            exhausted → learning_skip row, stop
       → Haiku prefilter (foreground baselines, sandbox skill index)
            Skip → stop
            PatchExisting / CreateNew → probe-writer fork of run_id
                 → may patch / create a rightx-* skill in the sandbox
```

## Error handling

- Anchor capture and the pipeline spawn are fire-and-forget; failures are
  logged and never affect cron delivery or the cron run record. (Cron
  success/failure recording is already complete before learning fires.)
- Prefilter skill-index read error → `PrefilterDecision::Skip` (existing
  behavior — never an empty index).
- A non-`Recurring` schedule kind short-circuits before anchor construction.

## Testing

- **Unit:** anchor is built only for `Recurring` (one-shot kinds produce
  none); budget-exhausted path writes a `learning_skip` row with
  `reason='budget'`, `intended_kind=NULL`.
- **Reuse:** existing prefilter and probe-writer unit tests cover the
  downstream stages unchanged.
- **Live (ignored, CI-explicit):** one `ci_claude_` / `ci_openshell_`-prefixed
  test that a recurring cron run produces a non-Skip prefilter decision and
  that a `CreateNew` lands a `rightx-*` skill in the sandbox skill index.
  Verifies risk (a) below.
- **Final:** `devenv shell -- cargo test --workspace` (mandatory).

Cadence: write the narrowest failing test first per stage, run targeted
`-p bot` tests during the loop, full workspace test only at the end.

## Risks to verify during implementation

a. **Forkability of a just-finished cron session inside the sandbox.**
   Foreground proves the fork shape works, but cron's fresh-session-per-run
   path needs a live check that `--fork-session --resume run_id` resolves the
   transcript inside the sandbox right after the run. Covered by the live
   test.
b. **`execute_job` scope** has `wall_elapsed_ms`, `chat_id`, `thread_id`
   available (or cheaply derivable) for the anchor.

## Deferred (revisit only on observed need)

- Spike/anomaly gating instead of every-run prefilter.
- Cheap-run exemplar selection (fork the last cheap run rather than the
  current one).
- Cron-specific or per-job baselines.
- Cron usage-receipt attribution (`skill_spend` `usage` rows for cron).

## Docs to update

- `docs/architecture/learning.md` — gate-ordering section: a second trigger
  source (recurring cron, no `PromptMode::Normal` requirement).
- `ARCHITECTURE.md` — one-line update to the skill-learning contract noting
  recurring cron as a learning trigger. No `PROMPT_SYSTEM.md` change (prompt
  assembly is unchanged).
