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

   The skill index is built by `learning_prefilter::collect_rightx_skill_index`.
   For sandboxed agents it reads `/sandbox/.claude/skills/rightx-*/SKILL.md`
   from inside the agent's sandbox via gRPC `exec_in_sandbox` (a `sh -c`
   frontmatter dump, parsed into name + excerpt). For `sandbox: mode: none`
   agents it reads the host filesystem. Each mode has exactly one source —
   there is no fallback path. A sandbox read error returns
   `PrefilterDecision::Skip` rather than an empty index; an empty index would
   allow the classifier to recommend creating a skill that already exists.

3. **Probe-writer** (`bot::learning_probe_writer`): when the prefilter
   returns non-Skip, the worker forks the main CC session with the decision
   as a directed hint. The writer receives the same skill index (built the
   same way by `collect_rightx_skill_index`; tolerates an empty index). It
   verifies and may patch, create, or refuse. It reports `hint_outcome`
   (`applied_as_hinted` / `applied_differently` / `refused`) back via
   `mcp__right__skill_learning_finish`.

   **Stdout drain:** the probe-writer's stdout is consumed in a single pass.
   A reader reads until `system/init` under the per-session mutex (bounded
   handshake), then a detached task drains the same reader to EOF while
   awaiting the child, capturing the final `result` line. The finish-row
   `invocation_id` links that line to `skill_learning_events` for spend
   attribution. (Prior bug: init-detection consumed stdout, leaving the
   later output read empty → usage and spend rows never recorded.)

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
   below `LearningConfig.max_daily_budget_usd` (default $1.00). When the
   daily budget is exhausted a `learning_skip` row is written
   (`reason='budget'`, `intended_kind=NULL`) **before** the prefilter runs —
   the skip is recorded even though create-vs-patch intent is unknowable at
   that point (the column is kept nullable for a possible future headroom
   design). A non-`skip` prefilter decision gates and directs the
   probe-writer fork. The session mutex on the main session UUID prevents
   concurrent `--resume` against the same transcript; the writer holds it
   only until its `system/init` handshake.

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

## Spend ledger & skip accounting

`data.db.skill_spend(skill_name, kind, cost_usd, cache_read, cache_creation,
invocation_id, ts)` records per-skill learning cost and cache tokens,
separate from the `usage_events` billing source-of-truth.

Four writers, four `kind` values:

| kind | Writer | What it records |
|------|--------|-----------------|
| `create` | probe-writer | exact cost of a create invocation, via `skill_learning_events` finish-row joined by `invocation_id` |
| `patch` | probe-writer | same, for a patch invocation |
| `maintain` | curator | pass cost split evenly across the skills the pass archived (`archived_at == this run's ts`), cost/N each in one transaction; no archived skill → no row |
| `usage` | worker (post-turn) | one row per rightx skill in the turn's `ProbeAnchor.used_skill_receipts`, each carrying the turn's cost/cache — attributed (overlaps when multiple skills used) |

The prefilter's own cost is NOT attributed to any skill (agent-level
learning overhead; stays only in `usage_events`).

`usage` rows are labeled "attributed, not exact" and must never be summed
as an exact agent total. `create`/`patch` rows are exact per invocation.
`maintain` rows split the pass cost evenly across the archived skills
(cost/N each, cache integer-divided), written in one transaction, so summing
`maintain` recovers the exact pass cost.

Dashboard bucketing: `create` → learn, `patch`+`maintain` → fix,
`usage` → usage. Cache columns sum `cache_read` + `cache_creation` over
learning kinds only (`create`/`patch`/`maintain`); `usage` cache is
excluded because those rows are attributed-not-exact.
The Knowledge view surfaces per-skill spend; the Usage tab shows the
budget-skip count and per-source cache columns. (Agent-level
learn/fix/usage rollup totals on the Usage tab are not yet implemented.)

`data.db.learning_skip(reason, intended_kind, chat_id, thread_id, ts)` counts
learning attempts suppressed before running. Today only `reason='budget'`
is written. `reason` is free-form TEXT (no enum constraint). `intended_kind`
is always NULL for `budget` skips because the classifier does not run when
the budget is exhausted.

## Removed paths

The only learning runtime is the prefilter/probe-writer/curator pipeline.
The old Stage 2 selector/reviewer, learning episode tables, nudge-signal
gate, and review reports have been removed from runtime and schema.
Deprecated `agent.yaml` learning keys (`background_review_enabled`,
`circuit_failure_threshold`, `circuit_cooldown_minutes`) are accepted only
for upgrade compatibility and warn at load time. Stage 2 selector/reviewer
calls intentionally omitted `--mcp-config` / `--strict-mcp-config`; that
path no longer ships and the field is silently ignored.
