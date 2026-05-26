# Skill Learning

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

The skill-learning pipeline is a per-turn writer plus periodic curator.
Load-bearing rules — `ClaudeInvocation` invariants, `LEARNING_SOURCES`
extension contract, dashboard pin-only operator surface — stay in
`ARCHITECTURE.md`.

## Per-turn pipeline

Replaces the prior fork-probe classifier.

1. **Anchor capture** (`bot::telegram::worker`): after the foreground
   assistant reply is sent, the worker captures a `ProbeAnchor` (user text,
   assistant text, main session UUID, captured_at, chat/thread, **num_turns,
   total_cost_usd, wall_elapsed_ms, used_skill_receipts**) for downstream
   consumption.

2. **Prefilter** (`bot::learning_prefilter`): a Haiku classifier returns a
   structured three-way decision —
   `Skip{reason}` / `PatchExisting{target_skill, reason}` /
   `CreateNew{topic_hint, reason}`. The prompt embeds per-agent baselines
   (P50/P90/P99 over 14d foreground turns) for `num_turns`, `total_cost_usd`,
   and `wall_elapsed_ms`, plus a one-line-per-skill index summary. Baselines
   are computed on demand by `right_agent::usage::turn_baseline::compute`.

3. **Probe-writer** (`bot::learning_probe_writer`): when the prefilter
   returns non-Skip, the worker forks the main CC session with the decision
   as a directed hint. The writer verifies and may patch, create, or refuse.
   It reports `hint_outcome` (`applied_as_hinted` / `applied_differently` /
   `refused`) back via `mcp__right__skill_learning_finish`.

4. **Curator** (`bot::learning_curator`): per-agent 60s ticker reads state
   from the `curator_state` singleton row in `data.db`. The gate is
   multi-signal: cost spike (today's `learning_probe_writer` cost vs
   `k * 14d P50` with a floor), skill-change count (≥ N skills
   created/patched since last run), or the 168h time fallback. A
   `min_cooldown_hours` floor blocks all triggers including the time
   fallback. Trigger evidence is captured in `last_spike_evidence_json`.

## Invocation contract notes

The probe-writer fork IS session-bearing — it forks the main session
(`--fork-session --resume <main>`) so it can preserve `--mcp-config` +
`--strict-mcp-config` and inherit the transcript via prompt cache. Tools are
narrowed at runtime via `--allowedTools Write,Read,Bash,
mcp__right__skill_learning_start,mcp__right__skill_learning_finish`.
Background learning writers MUST register an invocation identity
(`ProbeWriter` or `Curator`) and use the resulting per-invocation MCP config
with `X-Right-Invocation` before calling learning MCP tools.

The Haiku prefilter and the periodic curator are independent CC invocations.
The prefilter is non-session-bearing (`--tools ""`, JSON schema). The curator
forks a fresh session (no `--resume`) with the curator system prompt and a
narrow tool whitelist.

## Gate ordering

Two independent gates run today.

1. **Prefilter + probe-writer gate** (per turn, in worker): runs only when
   the prefilter is enabled, the foreground turn was a Normal prompt mode,
   and today's spend across `right_agent::usage::LEARNING_SOURCES`
   (`learning_prefilter`, `learning_probe_writer`, `learning_curator`) is
   below `LearningConfig.max_daily_budget_usd` (default $1.00). A non-`skip`
   prefilter decision gates and directs the probe-writer fork. The session
   mutex on the main session UUID prevents concurrent `--resume` against the
   same transcript; the writer holds it only until its `system/init`
   handshake.

2. **Curator gate** (periodic, agent ticker, pure logic in
   `bot::learning_curator::should_run_now`): order is `enabled` → `!paused`
   → `circuit_open_until` (skip if in future) → `min_idle_hours` (skip if
   any chat activity within window) → `min_cooldown_hours` (blocks ALL
   triggers below) → trigger priority **CostSpike > SkillChangeCount >
   TimeFallback**. First-ever runs seed `last_run_at` in `curator_state` and
   defer (Hermes pattern). State (`last_run_at`, `last_run_status`,
   `consecutive_failures`, `circuit_open_until`, `last_spike_evidence_json`)
   lives in the per-agent `curator_state` singleton row.

## Lifecycle storage

Lifecycle mutable state lives in per-agent `data.db.skill_lifecycle` via
`right-lifecycle`, not `.usage.json`. `skill_learning_events` remains the
append-only audit log for start/finish tool calls, while skill package
content remains under `.claude/skills/<skill_name>/SKILL.md`. Foreground
usage is recorded only from `used_skill_receipts`.

`skill_lifecycle` is the source of truth for active/stale/archived status,
`created_by` provenance (foreground / probe_writer / curator / bundled),
usage/patch counters, and the operator pin flag. The dashboard reads this
table for lifecycle overview and is the only operator pin/unpin surface.
Curator transitions read/write DB rows and skip pinned rows.

## Removed paths

The only learning runtime is the prefilter/probe-writer/curator pipeline.
The old Stage 2 selector/reviewer, learning episode tables, nudge-signal
gate, and review reports have been removed from runtime and schema.
Deprecated `agent.yaml` learning keys (`background_review_enabled`,
`circuit_failure_threshold`, `circuit_cooldown_minutes`) are accepted only
for upgrade compatibility and warn at load time. Stage 2 selector/reviewer
calls intentionally omitted `--mcp-config` / `--strict-mcp-config`; that
path no longer ships and the field is silently ignored.
