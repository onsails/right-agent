# Learning Fork-Probe (Stage 2 Replacement)

**Date:** 2026-05-21
**Status:** Design approved; pending implementation plan

## Problem

Stage 2 background learning (`crates/bot/src/learning_episode.rs`, ~1911 LoC; `crates/bot/src/learning_review.rs`, ~808 LoC) exists because the foreground agent does not reliably populate the `learning_signal` / `skill_issue_signal` fields in its structured reply. The schema requires those fields (`crates/right-codegen/src/agent_def.rs:61-103`), but in practice the agent forgets to emit them on learnable turns. Stage 2 papers over that with a second-pass pipeline:

- per-turn `EpisodeSeed` capture;
- debounced `DrainScheduler` ticks pending rows off `learning_episodes`;
- `episode_selector` LLM call picks which slice of the conversation to review;
- `episode_reviewer` LLM call produces a verdict with `should_notify_user`;
- `learning_skill_review` reviewer triggers on skill-receipt creation;
- daily $ budget + circuit breaker gate every LLM hop;
- `skill_review_reports` rows persist the verdict;
- best-effort Telegram side-channel message tells the user "I found a reusable workflow candidate".

The pipeline carries five state-machine columns on `learning_episodes` (`pending` / `selected` / `reviewing` / `reviewed` / `failed`), a debounced drain scheduler, and the circuit-breaker columns on `skill_nudge_state` (`consecutive_review_failures`, `review_circuit_open_until`). Roughly 2700 LoC of orchestration to extract a JSON field the agent was already supposed to produce.

A simpler primitive exists in Claude Code: after a foreground turn ends, fork the just-finished session (`claude -p --resume <main> --fork-session ...`) and run a one-turn classifier prompt — "workflow finished? any learning_signal / skill_issue_signal?". Fork inherits the transcript for free, reuses the prompt cache on the agent's own model, does not mutate the main session, and is gated by ordinary budget/skip logic in the bot worker. If the classifier returns a signal, persist it directly to `skill_nudge_signals`, attributed to the original main session.

This collapses selector + reviewer + drain + episode state machine into one fork. The result is one new module (`crates/bot/src/learning_probe.rs`, target ≤ 500 LoC) plus a source-tracking column on the signals table; the existing background pipeline is gated behind a deprecated `learning.background_review_enabled: false` opt-in flag and remains intact for one release cycle.

The dashboard also needs to surface where each accepted learning signal came from — foreground self-emission vs fork-probe vs `memory_retain` tool vs the deprecated background path — so users can see whether the probe is actually doing work and whether the foreground agent is improving over time.

## Goals

- Replace background `episode_selector` + `episode_reviewer` LLM pipeline with one synchronous fork-probe per foreground user turn.
- Attribute each `skill_nudge_signals` row to its source: `reply_field` (agent self-emit), `fork_probe`, `background_review` (deprecated).
- Expose source breakdown in the dashboard signals widget and add `learning_fork_probe` to `usage_events.source`.
- Deprecate the background pipeline behind a per-agent opt-in flag without deleting code or tables yet.
- Reuse the existing daily $ budget primitive; one budget covers fork-probe and the legacy background path summed.

## Non-goals

- Removing `crates/bot/src/learning_episode.rs`, `crates/bot/src/learning_review.rs`, or the `learning_episodes` / `skill_review_reports` tables. They stay through this release and are dropped by a follow-up cleanup spec after probe-only is validated in production.
- Modifying the agent's structured reply schema. The `learning_signal` / `skill_issue_signal` field shapes stay identical; the probe emits JSON in the same shape.
- Changing `mcp__right__memory_retain` ingestion. The retain-tool path continues to write `skill_nudge_signals` rows independently.
- Worker-side `learning_skill_review` (reviews of skill RECEIPTS after publication) — separate concern, stays as is.
- Surfacing the probe result to the user inside the original turn. The agent's reply is already sent to Telegram before the probe fires; nudges accrue in the dashboard, not as inline messages.

## Architecture

### Trigger

After every successful foreground user-turn in `crates/bot/src/telegram/worker.rs`, immediately after the assistant reply is sent to Telegram and `archive_assistant_message` has persisted the message, spawn a fire-and-forget `tokio::spawn` task that runs the probe. The task is independent of the worker's response loop — failure or slowness never blocks the user.

Skip conditions, checked before spawning:

- `kind != Foreground` (cron, background-continuation, reflection turns skip).
- Structured reply already contains `learning_signal != null` OR `skill_issue_signal != null` — agent self-emitted, no probe needed (cheap path, `source = 'reply_field'`).
- Daily budget for `learning_fork_probe` + `background_review` summed has reached `learning.max_daily_budget_usd`.
- `learning.fork_probe_enabled = false` (escape hatch for ops).

### Probe invocation

The probe is a session-bearing `ClaudeInvocation` per the existing contract (`ARCHITECTURE.md` → Claude Invocation Contract). Arguments:

```
claude -p \
  --resume <main-session-id> \
  --fork-session \
  --session-id <probe-uuid> \
  --model <agent-main-model> \
  --output-format json \
  --json-schema <FORK_PROBE_SCHEMA> \
  --max-turns 1 \
  --tools "" \
  --mcp-config <agent-mcp-config> \
  --strict-mcp-config \
  --dangerously-skip-permissions \
  "<FORK_PROBE_PROMPT>"
```

`--tools ""` blocks all tool use; `--mcp-config` + `--strict-mcp-config` preserves the session-bearing invariant. `--fork-session` creates an isolated session_id; the main session is unaffected.

Prompt (constant string in `crates/right-codegen/src/agent_def.rs`):

> Review the just-finished turn. Is the workflow complete and is there a reusable learning candidate or a skill-issue worth recording? Emit JSON per the provided schema. Set `learning_signal` and `skill_issue_signal` to null if nothing qualifies.

Schema (new constant `FORK_PROBE_SCHEMA_JSON` in `agent_def.rs`):

```json
{
  "type": "object",
  "properties": {
    "workflow_complete": { "type": "boolean" },
    "learning_signal": <same shape as REPLY_SCHEMA_JSON.learning_signal>,
    "skill_issue_signal": <same shape as REPLY_SCHEMA_JSON.skill_issue_signal>
  },
  "required": ["workflow_complete"]
}
```

`workflow_complete` exists for future telemetry (cron / iteration heuristics); does not gate signal ingestion in v1.

### Model choice

Probe model = agent's main `model` from `agent.yaml`, unless overridden by `learning.probe_model`. Rationale:

- Fork reuses the main session's prompt cache when the same model is used; transcript input cost drops to cache-read pricing (~10× cheaper than base). Switching to Haiku invalidates cache, eating the apparent savings.
- The main model knows the agent's idioms and skills better than a generic small model.

Override path exists so cost-sensitive agents (Opus 4.7 + heavy traffic) can force Haiku and accept the cache miss.

### Result handling

Probe stdout is parsed as the FORK_PROBE_SCHEMA JSON. If `learning_signal != null` or `skill_issue_signal != null`:

- `INSERT INTO skill_nudge_signals` with:
  - `session_id` = **main** session id (not probe session id);
  - `source = 'fork_probe'`;
  - `payload_json` = the relevant signal object;
  - other columns same as existing `record_nudge_signal` path.
- `INSERT INTO usage_events` with `source = 'learning_fork_probe'`, full token/cost breakdown parsed from probe stdout via existing `crate::cc::stream::parse_usage_full`.

If both signals are null, write only the usage row (zero-signal probes still cost money and should be visible).

If parsing fails or the probe exits non-zero, log a `tracing::warn!` with main session id and move on. No retry, no circuit breaker — the daily budget is the only backpressure.

### Cheap-path attribution

When the agent's structured reply already contains `learning_signal`/`skill_issue_signal`, the worker writes the signal row with `source = 'reply_field'` (this is the existing path; we just add the source value). No probe runs.

### Deprecated background path

When `learning.background_review_enabled = true` (opt-in, off by default), the existing pipeline runs in parallel with the fork-probe. Selector + reviewer continue to write `skill_nudge_signals` with `source = 'background_review'` (new constant for the existing insert sites). Daily budget gate covers both `learning_fork_probe` and the existing `learning_selector` + `learning_reviewer` + `learning_skill_review` sources summed. Operators can compare the two paths in the dashboard for one release cycle before the cleanup spec drops the background code.

`DrainScheduler::spawn` is wrapped in a runtime check: when `background_review_enabled = false`, spawn is replaced with a no-op handle. The Telegram dispatch (`crates/bot/src/telegram/dispatch.rs`) and cron startup (`crates/bot/src/cron.rs`) continue to thread the `Arc<DrainScheduler>` through ctx, but `schedule_drain()` calls are inert when the underlying scheduler is the no-op variant. This avoids churn in callers.

## Data Model

### `skill_nudge_signals` source column

Migration `v27_skill_nudge_signals_source.sql` (idempotent Rust hook):

```sql
ALTER TABLE skill_nudge_signals
  ADD COLUMN source TEXT NOT NULL DEFAULT 'reply_field';
CREATE INDEX IF NOT EXISTS idx_skill_nudge_signals_source
  ON skill_nudge_signals(source);
```

Source enum (string at DB level):

| Value | When written |
|---|---|
| `reply_field` | Agent's structured reply contained the signal; bot worker ingested it (current sole production path). |
| `fork_probe` | Post-turn fork classifier returned a non-null signal. |
| `background_review` | Deprecated Stage 2 selector/reviewer wrote it; only present when `background_review_enabled = true`. |

The default `reply_field` for the column matches the only current production path; pre-existing rows are accurately backfilled.

### `usage_events.source` value

Add `learning_fork_probe` to the const list `right_agent::usage::LEARNING_SOURCES` so the existing daily-budget gate (`right_agent::learned_skills::review_gate_decision_in_tx`) and the dashboard `SOURCES` array (`crates/right-dashboard/src/read_model/usage.rs`) both pick it up automatically. The existing cross-crate consistency test (`usage_overview_sources_match_learning_sources_constant`) catches drift.

No DDL change; `usage_events.source` is already TEXT.

### Pre-existing dead columns

`skill_nudge_state.consecutive_review_failures` and `review_circuit_open_until` (added in `2026-05-21-learning-daily-budget-circuit-breaker`) are not used by the fork-probe path. They keep working for the deprecated background path. Drop happens in a future cleanup migration once background is removed.

`skill_nudge_state.daily_review_count` and `daily_review_date` remain in the schema. Already dead pre-this-spec.

### `learning_episodes` table

Untouched. The deprecated background path keeps writing rows when enabled. New code never reads or writes the table.

## Configuration

`LearningConfig` (`crates/right-agent-config/src/lib.rs`) gains:

```yaml
learning:
  probe_model: ~                       # null → use agent.model
  fork_probe_enabled: true             # ops escape hatch
  background_review_enabled: false     # deprecated path opt-in
  max_daily_budget_usd: 1.00           # unchanged; gates probe + bg summed
  episode_selector_model: ~            # deprecated; ignored when bg disabled
  episode_settle_seconds: ~            # deprecated; ignored when bg disabled
  circuit_failure_threshold: 5         # used only by deprecated bg path
  circuit_cooldown_minutes: 60         # used only by deprecated bg path
```

`agent config` wizard (`crates/right/src/wizard.rs::learning_setup`) prompts:

- `probe_model` (string, blank → agent.model). New prompt.
- `fork_probe_enabled` (bool, default `true`). New prompt.
- `background_review_enabled` (bool, default `false`). New prompt with explanatory note: "Deprecated. Enable only for parity testing or until cleanup spec lands."
- `max_daily_budget_usd` (float, default 1.0). Existing prompt; description updated.

Deprecated knobs (`episode_selector_model`, `episode_settle_seconds`, circuit knobs) become wizard-skipped when `background_review_enabled = false`; the values stay readable so re-enabling restores prior behavior without re-prompting.

## Cost Model

Per-probe cost depends on transcript size and model:

- Opus 4.7 with cache hit on 50K transcript: ~$0.075 input (cache-read at $1.50/MTok) + ~$0.04 output (500 tokens × $75/MTok) = **~$0.12/probe**.
- Same transcript without cache (e.g., probe model differs from main): ~$0.75 input (base $15/MTok) + ~$0.04 output = **~$0.79/probe**. Effectively prohibitive on a $1/day cap.
- Haiku 4.5 fresh read: ~$0.05 input + ~$0.0025 output = **~$0.05/probe**. No cache regardless.

At $1/day default cap and Opus + cache: ~7–10 probes/day → ~$0.10/turn for the first 10 turns/day, $0 thereafter (cheap-path noop). Chatty agents will saturate; operators raise `max_daily_budget_usd` to fit observed volume.

Cheap path (agent self-emitted): **$0/probe**. Goal over time is that agent improves at self-emission and the cheap path covers an increasing share of turns.

Daily-budget gate is the only backpressure; no per-call cap, no circuit breaker for probes. A probe failure is silent and probes do not retry.

## Failure Modes

| Failure | Behavior |
|---|---|
| Probe exits non-zero | `tracing::warn!`; no signal written; no usage row written (CC didn't reach the API). User reply already delivered, unaffected. |
| Probe stdout fails JSON-schema parse | `tracing::warn!` with stdout excerpt; no signal written; usage row written from any partial stream-json frames if available. |
| Probe times out | Same as exits non-zero. Probe gets a hard 60s `tokio::time::timeout`. |
| Daily budget exceeded mid-probe | Probe completes (already paid the cost); subsequent probes that same day are gated out. |
| Main session does not exist (race with deletion) | Probe fails to fork; `tracing::warn!`; no signal. |
| Agent config flips `fork_probe_enabled` mid-run | New probes after config reload (`AgentSettings.fork_probe_enabled: Arc<AtomicBool>`) honor the new value; in-flight probes finish. Treat as hot-reloadable in `config_watcher::diff_classify`. |
| Schema-mismatched stdout (model emits extra fields) | JSON parser tolerates extras; only the declared fields are read. |

No reflection turn is triggered for probe failures — reflection is for foreground turns visible to the user.

## Migration

### Schema

One forward migration v27 (idempotent ALTER + idempotent CREATE INDEX). No data rewrite, no dropping columns. See Data Model section.

### Config

Existing `agent.yaml` files without `learning.fork_probe_enabled` / `background_review_enabled` deserialize to the new defaults (`true` / `false`). This is a **behavior change for existing agents** with background previously implicitly on: background turns off by default, fork-probe turns on by default. WARN logged at bot startup once per agent when first observing a `learning_episodes` row newer than 24h on a deployment where `background_review_enabled = false`:

> Background learning is deprecated and now disabled by default. Set `learning.background_review_enabled: true` in `agents/<name>/agent.yaml` to restore the prior pipeline; otherwise post-turn fork-probe takes over. Cleanup spec will drop the background code in a future release.

### Existing `learning_episodes` rows

Untouched. New code never reads them. They remain visible in the dashboard "learning episodes" view (for the deprecated path) and continue to drain only if `background_review_enabled = true`.

### Re-enabling background

`right agent config <name>` or direct `agent.yaml` edit to `background_review_enabled: true` + `right restart <name>` revives the legacy path. Fork-probe and background both run; budget is summed.

## Self-healing and upgrade

Per `AGENTS.md` upgrade-friendly design: new config fields default to backward-compatible-ish values. Background goes off by default; that is the **intentional** deprecation, not an accidental break. The bot-startup WARN gives operators the migration directive. No sandbox recreation, no DB destruction, no `right agent init`.

## Telemetry & Dashboard

### Source breakdown widget

The existing `signals_accepted_24h` widget (`crates/right-dashboard/src/read_model/learning.rs`) gains a stacked breakdown by `source` over the last 24h. SQL:

```sql
SELECT source, COUNT(*) FROM skill_nudge_signals
WHERE agent_name = ?1 AND accepted_at >= ?2
GROUP BY source
```

Render in the mini-app as a 3-bar bar-chart (`reply_field` / `fork_probe` / `background_review`). Empty bars allowed.

### Cost surface

`crates/right-dashboard/src/read_model/usage.rs::SOURCES` gains `learning_fork_probe` automatically via `LEARNING_SOURCES` (the cross-crate consistency test enforces this). Usage overview shows per-source $-spent.

### Probe efficacy metric

New read-model entry: `fork_probe_signal_rate_24h` = `count(source='fork_probe') / count(distinct foreground turns in 24h)`. Surfaces as a percentage on the learning dashboard. Goal: monitor downward over time as agent improves self-emission and reply_field grows.

## Testing

### Unit tests

- `learning_probe::should_run_probe` truth table: foreground turn, kind != foreground, reply has signal, reply lacks signal, budget exceeded, fork_probe_enabled = false.
- `learning_probe::parse_fork_probe_output` accepts non-null signals, null signals, malformed JSON, extra fields.
- `learning_probe::compute_signal_source` returns `reply_field` when structured reply has it, `fork_probe` when probe emits it, `background_review` for legacy insert sites.
- `record_nudge_signal` accepts and persists the new `source` argument; existing callers updated.
- `LearningConfig` deserializes pre-v27 `agent.yaml` (no `fork_probe_enabled` / `background_review_enabled` fields) to the new defaults.
- v27 migration idempotency: re-run twice on a database that already has the `source` column.

### Integration tests

- One end-to-end test (mocked CC probe binary, no live sandbox required): worker delivers a reply with no signal → fork-probe spawns → mock CC returns a `learning_signal` JSON → `skill_nudge_signals` row appears with `source = 'fork_probe'` and `session_id` matching the main session.
- One test: worker delivers a reply with `learning_signal` already populated → probe does NOT spawn → row has `source = 'reply_field'`.
- One test: `background_review_enabled = true` → both background drain and fork-probe run; two rows appear with distinct sources; budget gate sums both.
- One test: budget exhaustion mid-day → subsequent probes skip; usage_events show only the rows that paid.

Test cadence per `AGENTS.rust.md`: targeted `cargo test -p right-bot learning_probe`, `cargo test -p right-agent learned_skills`, `cargo test -p right-db v27` during development. Full `cargo test --workspace` before declaring done.

### Live-CC tests

The probe touches a real `claude -p` binary. One `ci_claude_` test asserts that a fork-probe against a known-good transcript returns parsable JSON within timeout, gated by the live-CC ignore filter per `crates/right/tests/ci_ignored_contract.rs`.

## Open questions

These are flagged for implementation-time validation, not blockers on this spec:

1. **`--fork-session` behavior under CC 2.1.x**: needs an empirical smoke test that fork does not write to the parent session JSONL. Documented as part of the live-CC test.
2. **`--tools ""` interaction with `--mcp-config`**: needs verification that an empty tools allowlist actually disables MCP tool listings when `--strict-mcp-config` is on. If not, fallback to `--disallowedTools <full-list>` enumerated from the agent's MCP config at probe-spawn time.
3. **Probe latency on heavy turns**: 50K-token transcript + Opus + cache-read ≈ 3–6s wall-clock. The probe runs async, so user-visible latency is unaffected, but if probe wall-clock pushes past the 60s timeout for chatty agents, we will need a turn-truncation heuristic in the prompt (e.g., "review only the last 10 messages"). Defer until observed.

These are deliberately NOT in scope for v1:

- Inline nudging the user about the just-found signal. Probe is dashboard-visible only.
- Automatic skill-file creation from a fork-probe signal. The signal is a CANDIDATE record; skill-file creation remains a separate agent-driven path.

## Implementation handoff

The implementation plan derived from this spec must encode:

- v27 migration as the first task, with the idempotency test.
- New `crates/bot/src/learning_probe.rs` module with strict LoC budget (target ≤500).
- `LearningConfig` field additions with backward-compatible deserialization.
- `record_nudge_signal` source-argument refactor across the existing insert site plus two new ones (fork-probe, background-review legacy).
- Dashboard read-model and widget updates.
- Wizard prompt updates.
- Bot-startup deprecation WARN.
- TDD red/green for each behavior change; targeted test runs per `AGENTS.rust.md` cadence; one final `cargo test --workspace` at the end.
