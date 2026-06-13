# Cron orchestration from the foreground: ad-hoc instructions, structured `then` continuation, report-to-origin

**Status:** design / brainstorm output (awaiting review)
**Date:** 2026-06-14
**Author:** Andrey + Claude

## Problem

Driving crons **from a live foreground chat** is clumsy. When the user
wants "run A, then B when A is done," today's agent has to babysit:
trigger A, watch `cron_list_runs`, then trigger B — or it fakes the
dependency with wall-clock offsets plus a shared state file. The riskoff
agent did exactly this: standing `sources-update` (07:17 UTC) and
`news-digest` (08:43 UTC) crons coupled only by a 1.5 h gap and
`sources.json`, then **raced them at trigger time** ("I triggered
news-digest before sources-update had a chance to finish"). It also
rewrote one cron's prompt ~8 times via full `cron_update` calls just to
tweak wording for a single run.

Three distinct frictions, all foreground-side:

1. **No per-run instruction.** A one-off tweak ("this time focus on X")
   forces a full `cron_update` that mutates the stored spec — churn, and
   the change leaks into every future run.
2. **No reliable hand-off.** "Run B after A" relies on A's LLM
   remembering to call `cron_trigger` at the end. If A hits its budget,
   turn limit, or crashes before that final step, B never fires — the
   chain is *advisory*, not guaranteed.
3. **No report to the chat I'm in.** A cron delivers to its standing
   `target_chat_id` (e.g. the channel). When the user triggers it from a
   DM and wants to see the outcome *here*, there is no extra report to
   the originating chat.

### What this is NOT

Not a heartbeat. The "heartbeat like Hermes/OpenClaw" idea was researched
(see workflow `hermes-heartbeat-research`, 2026-06-14). Findings: the
canonical heartbeat (MemGPT/Letta `request_heartbeat`) is *in-process
tool-call chaining*, not a clock — and Letta **deprecated it in v1**
because modern models self-continue natively. A *periodic-tick* heartbeat
(the other meaning) costs a full LLM turn every interval (~$30–100/mo
idle) and keeps persistent context as an injection surface — both clash
with Right's isolated-session security posture. The leverage for our
observed pain is **removing the second trigger / making the hand-off
runtime-guaranteed**, not adding a tick loop. Multiple standing crons
stay normal; this spec fixes how we *orchestrate* them live.

## Goals

- A foreground trigger can append an **ephemeral, this-run-only**
  instruction to a cron's prompt without mutating the stored spec.
- A foreground trigger can attach a **structured `then`** that the
  **runtime guarantees** to run after the triggered run, **in the
  continued context** of that run (so the follow-up sees everything the
  first run did, including a failure).
- A triggered run (and/or its `then`) can deliver an **extra report to
  the originating chat**, resolved server-side, in addition to the cron's
  standing `target_chat_id`.
- Reuse the existing background-continuation substrate; add no new
  scheduler subsystem.

## Non-goals

- No periodic heartbeat / tick lane.
- No general cron-to-cron dependency DAG on standing specs (`depends_on`
  on `cron_specs`). `then` is a per-trigger, single-hop edge, not a
  persistent graph. (Revisit only if real demand appears.)
- No multi-hop structured chains in v1 (a `then` continuation does not
  itself carry another `then`; depth is capped at 1). The continuation is
  a full agent turn and can do arbitrary multi-step work in-context, so
  one structured hop covers the observed cases.

## Design overview

Three additive features on the trigger path, all riding mechanisms that
already exist:

| Feature | Rides on | New surface |
|---|---|---|
| Ephemeral per-run instruction | `force_notify` transient-prepend pattern in `execute_job` (`cron.rs:604`) | `extra_instruction` param + transient column |
| Structured `then` continuation | `spawn_background_continuation` (`background.rs:40`) resuming `source_session_id` with `fork_session: true` | `then` param + per-run carry on `async_runs` |
| Report-to-origin | server-resolved invocation scope (same as `send_message`) + async delivery loop | `report_to_origin` intent + origin columns |

### Feature 1 — ephemeral `extra_instruction`

`mcp__right__cron_trigger(job_name, extra_instruction: "…")`. The string
is stored in a transient column on `cron_specs` alongside `triggered_at`
/ `trigger_force_notify`, prepended to the run prompt in `execute_job`
(exactly where the force-notify `⟨⟨SYSTEM_NOTICE⟩⟩` is prepended today),
and cleared by `clear_triggered_at` after the run. The stored `prompt` is
never touched. This kills the `cron_update` churn for one-off tweaks.

Wrapping: frame it as a system directive, e.g.
`⟨⟨SYSTEM_NOTICE⟩⟩ Extra instruction for this run only: {…} ⟨⟨/SYSTEM_NOTICE⟩⟩`,
prepended before the stored prompt and after any force-notify notice.

### Feature 2 — structured `then`

`mcp__right__cron_trigger(job_name, then: { instruction, run_on?, notify?, target_chat_id?, target_thread_id? })`.

Semantics: when the triggered run **reaches a terminal state matching
`run_on`**, the runtime spawns a background continuation that
**`--resume`s the triggered run's session** (`fork_session: true`, so the
original run's session is left intact) with `then.instruction` as the
user message. Because it forks the run's session, the continuation has
the full context of what the run did — discovered sources, partial
output, or the failure itself.

- `run_on`: `"success"` (default) | `"failure"` | `"always"`. `failure`
  / `always` exist precisely because the triggered run can die; the
  continuation then runs in-context to salvage, recover, or report.
- Delivery: the continuation produces structured output
  (`BG_CONTINUATION_SCHEMA_JSON`) delivered through the normal async
  delivery loop. `notify` forces delivery and skips the idle gate, same
  as elsewhere.
- Target: `then.target_chat_id`/`target_thread_id` default to the
  **originating chat** (Feature 3); an explicit value (allowlist-checked)
  overrides.

Why a continuation, not a cold trigger of a named cron B: the user's
requirement is context continuity ("продолжит контекст предыдущего
рана") and robustness against the called step failing. A cold cron B
re-reads nothing and can fail blind; a forked resume of the run's session
inherits everything. This is the Letta in-run-heartbeat idea realized as
a **runtime-scheduled** resume.

Implementation: on terminal completion of a triggered run (in the cron
completion handler, near `persist_successful_cron_output` /
`complete_background_run`), if the run carries a `then` payload and the
terminal status matches `run_on`, build a `BackgroundRunRequest {
source_session_id = run.run_session_id, run_id = new uuid, prompt =
then.instruction, target_chat_id, target_thread_id }` and call
`spawn_background_continuation` under the run's per-session
`SessionLocks` guard. The continuation is an ordinary `kind='background'`
`async_runs` row, so delivery, failure classification, and startup
recovery all work unchanged.

Depth cap: the spawned continuation never carries its own `then`
(enforced — it is created without one). Prevents runaway chains.

### Feature 3 — report-to-origin

`mcp__right__cron_trigger(job_name, report_to_origin: true)` (and `then`
defaults to origin when present).

The originating chat is **resolved server-side** from the foreground
invocation that issued the trigger — the same conversation-scope resolver
that backs `send_message`/`thread_search`. The agent passes **intent**
(`report_to_origin: true`), never a chat id, so it cannot misroute.
Resolved `(chat_id, thread_id)` is stamped onto the triggered run's
`async_runs` row as origin columns and carried into the `then`
continuation's default target.

- Delivery still passes the agent's **allowlist** (the origin chat is
  already allowlisted — the agent is conversing there).
- When `cron_trigger` is called **from inside a cron turn** (no
  foreground invocation — the legacy hand-off case), there is no origin;
  `report_to_origin` is a documented no-op and `then` falls back to the
  cron's standing target.
- This is *additional* to the cron's standing `target_chat_id`; it does
  not replace it. A run with `report_to_origin` and a different standing
  target produces two deliveries.

## Data model

Migration (idempotent; column adds guarded by `pragma_table_info` per the
migration rules):

- `cron_specs` transient (set by `trigger_spec`, cleared by
  `clear_triggered_at`):
  - `trigger_extra_instruction TEXT NULL`
  - `trigger_then_json TEXT NULL` (serialized `ThenSpec`)
  - `trigger_report_origin INTEGER NOT NULL DEFAULT 0`
  - `trigger_origin_chat_id INTEGER NULL`
  - `trigger_origin_thread_id INTEGER NULL`
- `async_runs` per-run carry (copied at job start, read at completion):
  - `then_json TEXT NULL`
  - `origin_chat_id INTEGER NULL`
  - `origin_thread_id INTEGER NULL`

The transient `cron_specs` fields are copied onto the run's `async_runs`
row in `execute_job` and cleared from `cron_specs` immediately, so a
recurring spec never retains per-trigger state and a scheduled
(non-triggered) run carries none of it. `then`/origin live on the run,
which is the unit that completes.

`ThenSpec` (serde): `{ instruction: String, run_on: RunOn,
notify: bool, target_chat_id: Option<i64>, target_thread_id: Option<i64> }`
with `RunOn = Success | Failure | Always`.

## Tool surface

`mcp__right__cron_trigger` gains optional params:

| Param | Type | Default | Notes |
|---|---|---|---|
| `extra_instruction` | string | – | this-run-only prepend; no spec mutation |
| `then` | object | – | `{ instruction, run_on?, notify?, target_chat_id?, target_thread_id? }` |
| `report_to_origin` | bool | `false` | deliver an extra report to the chat the trigger was issued from; chat resolved server-side |

`then.target_chat_id` (if provided) is allowlist-validated like
`cron_create`'s `target_chat_id`. No new tools; `cron_create`/`cron_update`
unchanged in v1 (a future iteration could let a standing spec carry a
default `then`, but YAGNI now).

## Prompt / skill updates (cite-on-touch)

- `crates/right-codegen/skills/right-cron/SKILL.md`: replace the soft
  "schedule one delayed one-shot that triggers B with notify=true"
  hand-off guidance with the structured `then` (guaranteed,
  context-continuing). Document `extra_instruction` for one-off tweaks
  instead of `cron_update`. Document `report_to_origin` for "run it and
  tell me here." Bump skill `version`.
- `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`
  and the `cron_trigger` tool description (`TRIGGER_TOOL_DESC`,
  `cron_spec.rs:40`): note the new params at prompt-tier brevity.
- `PROMPT_SYSTEM.md`: keep in sync if tool descriptions change.
- `ARCHITECTURE.md`: the MCP Aggregator section already pins the
  scope-resolution invariant; add `cron_trigger`'s origin resolution to
  the "scope comes from the registered invocation, never agent args"
  list (one line). `docs/architecture/sessions.md`: extend the
  force-notify-trigger / background-continuation narration with the
  `then` flow.

## Failure modes & edge cases

- **Triggered job is locked** (already running): trigger is dropped today;
  `then`/`extra_instruction` are dropped with it — unchanged behavior, no
  partial state (transient fields only stamped on a run that actually
  starts).
- **Run crashes before terminal** (`error_during_execution`): treated as a
  terminal failure; `then` with `run_on ∈ {failure, always}` fires;
  `success` does not. The fork resume sees the failed session.
- **Continuation itself fails**: ordinary `kind='background'` failure path
  — `complete_background_run` classifies and delivers a user-facing
  reason; never reflects (avoids re-issuing an overloaded call).
- **Origin absent** (cron-turn caller): `report_to_origin` no-ops, `then`
  uses the standing target. Logged, not errored.
- **Runaway chains**: continuation carries no `then`; depth capped at 1.
- **Scope safety**: origin chat is server-resolved; agent never supplies
  it. `then.target_chat_id` is allowlist-validated.

## Decisions & assumptions to confirm (review gate)

1. **`run_on` default = `success`.** `failure`/`always` are available for
   the "the called step can die" concern. Confirm this is the right
   default, or whether you want `always` as default given that concern.
2. **`then` continuation forks the run's session** (`fork_session: true`),
   leaving the original run intact, rather than mutating it. Assumed
   correct (matches today's background-continuation behavior).
3. **Single structured hop only** (no `then`-of-`then`). Assumed
   sufficient; multi-hop deferred.
4. **`then` lives only on `cron_trigger`, not on standing
   `cron_create`/`cron_update`.** Assumed — this is a foreground
   orchestration feature, not a standing-spec property.
5. **Report-to-origin is additive** (two deliveries when the standing
   target differs), not a redirect. Confirm.

## Testing

- Unit: `ThenSpec` serde round-trip; `run_on` matching against each
  terminal status; transient→`async_runs` copy + `clear_triggered_at`
  clears all new fields; allowlist rejection of a bad `then.target_chat_id`.
- Unit: `extra_instruction` prepend ordering relative to force-notify
  notice; stored `prompt` unchanged after a triggered run.
- Integration (mock CC): triggered run terminal `success` →
  continuation spawned with `source_session_id == run.run_session_id`;
  `failure` + `run_on=success` → no continuation; `failure` +
  `run_on=always` → continuation spawned.
- Origin: trigger from a foreground invocation stamps origin columns;
  trigger from a cron turn leaves them null.
- Final: `cargo nextest run --workspace` + `cargo test --doc --workspace`.

## Verification cadence

Targeted package tests (`-p bot`, `-p right-agent`, `-p right-db`) during
the red/green loop; one full workspace test + doctests at the end. No
full-workspace runs after every edit.
