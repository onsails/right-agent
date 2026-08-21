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
- The `then` continuation can deliver to the **originating chat**
  ("report here") by defaulting its target to the server-resolved
  foreground chat — no separate flag, no delivery fan-out.
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
| `then` → originating chat | server-resolved invocation scope (same as `send_message`) | origin columns feeding the continuation's default target |

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

- `run_on`: `"success"` | `"failure"` | `"always"` — **required** when
  `then` is present. The tool errors if `then` is given without `run_on`
  (no implicit default — the agent must choose deliberately). `failure` /
  `always` exist precisely because the triggered run can die; the
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

### Feature 3 — `then` delivers to the originating chat

There is **no standalone report-to-origin flag** and no delivery fan-out
(decided: option b). "Report here" is realized through `then`: the
continuation's target **defaults to the originating chat**.

The originating chat is **resolved server-side** from the foreground
invocation that issued the trigger — the same conversation-scope resolver
that backs `send_message`/`thread_search`. The agent never passes a chat
id for "here". The `cron_trigger` handler resolves it via the existing
foreground-only conversation-scope accessor (which returns a scope **only**
for a foreground invocation — precisely when origin should exist) and
writes it to the `cron_specs` transient origin columns; it flows into the
in-memory `CronSpec` and becomes the `then` continuation's default
`target_chat_id`/`target_thread_id`. An explicit `then.target_chat_id`
(allowlist-validated) overrides it to send elsewhere.

So the channel post and the "how it went" report are two *different*
messages from two different runs (A → channel via its standing target;
the `then` continuation → origin via the resolved origin), not one
message duplicated.

- Delivery passes the agent's **allowlist** (the origin chat is already
  allowlisted — the agent is conversing there).
- When `cron_trigger` is called **from inside a cron turn** (no
  foreground invocation — the legacy hand-off case), there is no origin.
  The `then` continuation then requires an explicit `then.target_chat_id`,
  else it falls back to the triggered cron's standing `target_chat_id`.

## Data model

Migration (idempotent; column adds guarded by `pragma_table_info` per the
migration rules):

- `cron_specs` transient (set by `trigger_spec`, cleared by
  `clear_triggered_at`):
  - `trigger_extra_instruction TEXT NULL`
  - `trigger_then_json TEXT NULL` (serialized `ThenSpec`)
  - `trigger_origin_chat_id INTEGER NULL`
  - `trigger_origin_thread_id INTEGER NULL`

**No `async_runs` columns are added.** The reconciler loads these
transient fields into the in-memory `CronSpec` snapshot (via
`load_specs_from_db`), then `clear_triggered_at` wipes the DB row *before*
`execute_job` runs — exactly the existing `trigger_force_notify`
lifecycle. `execute_job` and its completion handler read `extra_instruction`
/ `then` / origin straight off the in-memory `spec`, which stays in scope
through completion (no early returns). They must be **excluded from
`CronSpec`'s `PartialEq`** (transient state, like `triggered_at` /
`trigger_force_notify`) or the reconciler aborts running jobs on trigger.
The spawned `then` continuation is an ordinary `kind='background'`
`async_runs` row whose `target_chat_id` is the resolved origin/`then`
target — no schema change there.

`ThenSpec` (serde): `{ instruction: String, run_on: RunOn,
notify: bool, target_chat_id: Option<i64>, target_thread_id: Option<i64> }`
with `RunOn = Success | Failure | Always`. `run_on` has **no `Default`** —
deserialization fails if the field is absent, surfacing as a tool error.

### Delivery mechanism (where content goes)

The **destination chat is never in the structured output** — agents
cannot choose the chat (scope invariant). A run's structured output
carries only `delivery.kind` (notify/silent) and `delivery.content` (the
text). The destination is the `async_runs` row's
`target_chat_id`/`target_thread_id`, set server-side; the async delivery
loop reads content from `delivery_json` and address from the row columns.

Consequently the `then` continuation delivers to the origin chat simply
by having its row's `target_chat_id = origin` (server-resolved) — content
via its own structured output, address via the row target. One row, one
destination. Mirroring a *single* run's output to *two* chats (the
report-to-origin variant) is the only case that needs more than the row
target; see Feature 3.

## Tool surface

`mcp__right__cron_trigger` gains optional params:

| Param | Type | Default | Notes |
|---|---|---|---|
| `extra_instruction` | string | – | this-run-only prepend; no spec mutation |
| `then` | object | – | `{ instruction, run_on (required), notify?, target_chat_id?, target_thread_id? }` |

`then.run_on` is required (tool errors if absent). `then.target_chat_id`
defaults to the **server-resolved originating chat**; if provided
explicitly it is allowlist-validated like `cron_create`'s
`target_chat_id`. No standalone report-to-origin param. No new tools;
`cron_create`/`cron_update` unchanged in v1 (a future iteration could let
a standing spec carry a default `then`, but YAGNI now).

## Prompt / skill updates (cite-on-touch)

- `crates/right-codegen/skills/right-cron/SKILL.md`: replace the soft
  "schedule one delayed one-shot that triggers B with notify=true"
  hand-off guidance with the structured `then` (guaranteed,
  context-continuing). Document `extra_instruction` for one-off tweaks
  instead of `cron_update`. Document the `then`-to-origin pattern for
  "run it and tell me here." Bump skill `version`.
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
- **Origin absent** (cron-turn caller): no origin to default to; `then`
  uses an explicit `then.target_chat_id` or falls back to the triggered
  cron's standing target. Logged, not errored.
- **Runaway chains**: continuation carries no `then`; depth capped at 1.
- **Scope safety**: origin chat is server-resolved; agent never supplies
  it. `then.target_chat_id` is allowlist-validated.

## Known v1 limitations (from code review)

- **`run_on: failure`/`always` covers CC-ran-but-failed only.** The `then`
  continuation *forks the triggered run's session*, so it can fire only when a
  session exists: the `CronStreamOutcome::Failed` arm and the unparseable-output
  parse-failure arm. Pre-CC-start infra failures (process spawn failed, missing
  `claude` binary, sandbox guard, DB-open failure) record the run `failed` but
  fire no continuation — there is nothing to fork. Acceptable for v1.
- **`then.notify` is emphasis-only.** The background-continuation schema forces
  `delivery.kind=notify` and the row sets `delivery_required=true`, so a `then`
  continuation always delivers a message. `notify` only adds a prompt-emphasis
  directive; forcing idle-gate skip via the row's `force_notify` column is a
  follow-up. The DTO description says so.
- **One DB connection per fired continuation.** `insert_then_continuation_row`
  opens its own `right_db` connection rather than reusing the cron run's open
  `conn` — a minor avoidable open on the cron path; reuse is a follow-up.

## Decisions & assumptions

Resolved:

- **`run_on` is required** (no default; tool errors if `then` is present
  without it). ✓ confirmed 2026-06-14.
- **Single structured hop only** (no `then`-of-`then`). ✓ confirmed
  2026-06-14.
- **Report-to-origin via `then`** (option b). ✓ confirmed 2026-06-14.
  No standalone flag, no delivery fan-out; the `then` continuation's
  target defaults to the server-resolved originating chat.

Still to confirm:

1. **`then` continuation forks the run's session** (`fork_session: true`),
   leaving the original run intact, rather than mutating it. Assumed
   correct (matches today's background-continuation behavior).
2. **`then` lives only on `cron_trigger`, not on standing
   `cron_create`/`cron_update`.** Assumed — this is a foreground
   orchestration feature, not a standing-spec property.

## Testing

- Unit: `ThenSpec` serde round-trip; missing `run_on` fails
  deserialization; `run_on` matching against each terminal status;
  `trigger_spec` writes all transient fields and `clear_triggered_at`
  clears them; `load_specs_from_db` carries them into `CronSpec` while
  `PartialEq` ignores them; allowlist rejection of a bad
  `then.target_chat_id`.
- Unit: `extra_instruction` prepend ordering relative to the force-notify
  notice; stored `prompt` unchanged after a triggered run.
- Integration (mock CC): triggered run terminal `success` →
  continuation spawned with `source_session_id == run_id` (A's session);
  `failure` + `run_on=success` → no continuation; `failure` +
  `run_on=always` → continuation spawned.
- Origin: `trigger_spec` invoked with a resolved foreground scope writes
  origin columns; invoked from a cron-turn caller (no scope) leaves them
  null and the continuation falls back to the standing target.
- Final: `cargo nextest run --workspace` + `cargo test --doc --workspace`.

## Verification cadence

Targeted package tests (`-p bot`, `-p right-agent`, `-p right-db`) during
the red/green loop; one full workspace test + doctests at the end. No
full-workspace runs after every edit.
