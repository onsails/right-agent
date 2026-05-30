# Learning: sandbox skill index + skill-spend observability

**Date:** 2026-05-31
**Status:** Design — pending plan
**Scope:** Right Agent per-turn skill-learning pipeline (`prefilter → probe-writer`), `right-db`, `right-dashboard`.

## Problem

Live investigation of agent `agent-b` (2026-05-30) confirmed the new learning
pipeline now runs end-to-end and produces good skills, but two real defects
remain, plus an observability gap:

1. **Prefilter is blind to learned skills.** `learning_prefilter::collect_host_rightx_skill_index`
   reads `agent_dir/.claude/skills` on the **host**. The probe-writer forks the
   sandbox session and writes the skill to `/sandbox/.claude/skills/` inside the
   **sandbox**; there is no sandbox→host sync of skills. So the prefilter's
   "EXISTING SKILLS" list fed to the Haiku classifier is always empty of learned
   skills. Consequences: the classifier can never return `patch_existing`, and it
   keeps returning `create_new` for the same procedure — producing duplicate
   skills (`rightx-agenda` 05-28 and `rightx-notion-task-agenda` 05-30 are the
   same procedure learned twice).

2. **Probe-writer usage is never recorded.** `usage_events` has zero
   `learning_probe_writer` rows despite successful runs. Root cause: in
   `learning_probe_writer::spawn`, the child's stdout handle is **taken** by
   `wait_for_system_init` (`learning_probe_writer.rs:204`), so the later
   `wait_with_output_or_kill` returns an empty `output.stdout`
   (`learning_probe_writer.rs:241`), `parse_usage_full` returns `None`, and
   `insert_learning_probe_writer` never runs. Cost and cache numbers for the
   probe-writer (and, by the same shape, the curator) are invisible.

3. **No per-skill spend breakdown.** There is no way to answer "how much was
   spent learning a skill vs fixing it vs using it." `usage_events` has no
   `skill_name`, and one foreground turn can use several skills, so a single
   column cannot express usage attribution.

Verified non-issue: prompt caching on the successful probe-writer run worked
(`cache_read` dominates per message, `cache_creation` small). Slowness/timeouts
are not proven to be cache-related; that investigation is **out of scope** here
but is made observable by part 3.

## Goals

- Prefilter sees the skills that actually exist in the sandbox → no duplicate
  creation, `patch_existing` becomes reachable.
- Probe-writer (and curator) usage rows are written reliably, with cache fields.
- A per-skill spend ledger answering **learning vs fixing vs usage** cost, surfaced
  in the dashboard **Knowledge** view and **Usage** tab, including cache usage.

## Non-goals

- Diagnosing/fixing the 300s probe-writer timeout (separate; this design only
  makes it observable).
- Changing skill storage location or adding host↔sandbox skill sync.
- Attributing prefilter ("gate") cost to a specific skill — a gate run precedes
  any skill and is agent-level learning overhead.

## Design

### Part 1 — Prefilter reads the skill index from the sandbox (chosen: A1)

Replace the host-filesystem read with a sandbox read via gRPC
`right_openshell::openshell::exec_in_sandbox(client, sandbox_id, command, timeout)`
(the sanctioned in-sandbox exec; never the `openshell` CLI). The prefilter lists
`/sandbox/.claude/skills/rightx-*/SKILL.md` and reads each file's YAML frontmatter
(`name`, `description`) to build the abbreviated index string already consumed by
`build_prompt`.

- The sandbox is the single source of truth for skill content; no DB duplication,
  no drift, no migration.
- `PrefilterContext` already carries `resolved_sandbox: Option<String>`. It must
  also receive an `OpenShellClient` handle (or a small accessor) from the worker —
  integration point to resolve in the plan; the bot already holds sandbox clients
  for exec/file transfer.
- Bounding rules from the current host reader are preserved (`is_rightx_skill`
  filter, `SKILL_EXCERPT_MAX_*`, `SKILL_INDEX_DESC_MAX_CHARS`, sorted by name).
- FAIL-FAST within the fire-and-forget task: a sandbox-read error logs a warn and
  returns `Skip { reason }` (same contract as today's `skill index failed`). It
  must not silently treat "read failed" as "no skills" — a failed read returning an
  empty index would re-introduce the duplicate-creation bug, so read failure →
  Skip, not empty-index.
- `no-sandbox` agents (`sandbox: mode: none`): fall back to the existing host read.

### Part 2 — Record probe-writer usage reliably

The init-detection reader and the usage-capture reader both want stdout; only one
can own the pipe. Unify them: the single task that drains the child's stdout (and
writes the persisted stream NDJSON) also detects `system/init` and captures the
final `result` event. Persist usage from that captured result via the existing
`parse_usage_full` + `insert_learning_probe_writer` path. Apply the same capture to
the curator if it shares the take-stdout shape.

This removes the second `output.stdout` read; usage is parsed from the stream we
already persist (the same source from which cache numbers were recovered during
investigation).

### Part 3 — `skill_spend` ledger

`usage_events` stays the raw billing source of truth, unchanged. A new narrow
attribution table records per-skill spend by kind:

```sql
CREATE TABLE IF NOT EXISTS skill_spend (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  skill_name     TEXT NOT NULL,
  kind           TEXT NOT NULL CHECK (kind IN ('create','patch','maintain','usage')),
  cost_usd       REAL NOT NULL DEFAULT 0.0,
  cache_read     INTEGER NOT NULL DEFAULT 0,
  cache_creation INTEGER NOT NULL DEFAULT 0,
  invocation_id  TEXT,
  ts             TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_skill_spend_skill_kind ON skill_spend(skill_name, kind);
CREATE INDEX IF NOT EXISTS idx_skill_spend_ts ON skill_spend(ts);
```

Migration **v38** (next free version), idempotent (`CREATE TABLE/INDEX IF NOT
EXISTS`), registered in `right_db::migrations::MIGRATIONS`.

Writers:

- **probe-writer**, on its captured result (part 2): write one row with
  `kind = 'create'` or `'patch'`, derived from the `skill_learning_events.action`
  it recorded this invocation (`create`/`update` → `create`/`patch`), tagged with
  `skill_name` and `invocation_id`. Exact, 1 row.
- **curator**: `kind = 'maintain'`, one row per skill it touched, when known;
  otherwise no row (curator-wide overhead stays in `usage_events` only).
- **worker (usage)**: at post-turn time, where `ProbeAnchor.used_skill_receipts`
  and the turn's billing are both known, write one `kind = 'usage'` row **per
  rightx skill receipt**, each carrying the turn's cost/cache. This is attributed
  (it overlaps across skills when a turn used several); the dashboard labels it as
  attributed, not exact. Non-rightx receipts are ignored.

Prefilter "gate" cost is **not** written to `skill_spend`; it remains in
`usage_events` as agent-level learning overhead.

Writes are single-statement inserts (no transaction needed per the Transaction
Rule). Insert failure logs a warn and is swallowed only at the fire-and-forget
boundary — the ledger is observability, never on the user-visible path, and must
not fail a turn.

### Part 4 — Knowledge view: per-skill learn/fix/usage + cache

`right_dashboard::read_model::learning::skill_lifecycle_overview` LEFT JOINs a
`skill_spend` aggregate by `skill_name`:

```sql
SELECT skill_name,
       SUM(CASE WHEN kind='create'  THEN cost_usd END)               AS learn_usd,
       SUM(CASE WHEN kind IN ('patch','maintain') THEN cost_usd END) AS fix_usd,
       SUM(CASE WHEN kind='usage'   THEN cost_usd END)               AS usage_usd,
       SUM(cache_read) AS cache_read, SUM(cache_creation) AS cache_creation
FROM skill_spend GROUP BY skill_name
```

Each skill card in the Knowledge view gains `learn`, `fix`, `usage` cost and cache
read/creation. Surfaced through `api_types.rs`; the Vue Knowledge view renders the
three buckets. Skills with no spend rows render zeros via the existing
`AsyncState`/empty handling (no `'not loaded'` placeholders).

### Part 5 — Usage tab: buckets + cache columns

`learning_probe_writer` is already in `right_agent::usage::LEARNING_SOURCES` and the
dashboard `SOURCES` array (kept in sync by
`usage_overview_sources_match_learning_sources_constant`); once part 2 lands, the
existing row populates. Add to the Usage tab: the three `skill_spend` buckets
(learn/fix/usage totals for the agent) and `cache_read`/`cache_creation` columns so
a future cold-cache probe-writer run is visible at a glance.

## Components & files

| Area | File(s) | Change |
|---|---|---|
| Prefilter index | `crates/bot/src/learning_prefilter.rs` | replace `collect_host_rightx_skill_index` with sandbox gRPC read; host fallback for no-sandbox |
| Prefilter ctx | `crates/bot/src/telegram/worker.rs` | pass OpenShell client/handle into `PrefilterContext` |
| Probe-writer usage | `crates/bot/src/learning_probe_writer.rs` | unify stdout drain (init + result capture); write `skill_spend` create/patch |
| Curator usage | `crates/bot/src/learning_curator.rs` | write `skill_spend` maintain (when skill known) |
| Usage rows | `crates/right-agent/src/usage/insert.rs` | `insert_skill_spend(...)` helper |
| Worker usage spend | `crates/bot/src/telegram/worker.rs` | post-turn `kind='usage'` rows per rightx receipt |
| Schema | `crates/right-db/src/sql/v38_skill_spend.sql`, `migrations.rs` | new table + indexes (idempotent) |
| Knowledge read | `crates/right-dashboard/src/read_model/learning.rs`, `api_types.rs` | per-skill spend aggregate |
| Usage read | `crates/right-dashboard/src/read_model/usage.rs`, `api_types.rs` | buckets + cache columns |
| Frontend | `right-dashboard` Vue Knowledge + Usage views | render buckets/cache via `AsyncState` |
| Docs | `ARCHITECTURE.md`/`docs/architecture/learning.md`, `PROMPT_SYSTEM.md` | skill-index-from-sandbox rule; `skill_spend` ledger |

## Error handling

- Learning is fire-and-forget; existing contract (log warn + `Skip`) is preserved.
- Part 1 read failure → `Skip`, **never** empty-index (would re-trigger duplicates).
- `skill_spend` insert failure → warn + swallow at the fire-and-forget boundary
  only; never propagate into a user turn.
- `anyhow`/`thiserror` error chains formatted with `{:#}`.

## Testing (TDD; targeted intermediate, one final full workspace test)

- Prefilter: unit test that a sandbox-provided index yields `patch_existing` for an
  existing skill and not a duplicate `create_new`; read-failure yields `Skip`, not
  empty index. (`-p right-bot`)
- Probe-writer usage: regression that a completed run writes a `learning_probe_writer`
  `usage_events` row **and** a `skill_spend` `create`/`patch` row with `skill_name`.
  (`-p right-bot`)
- Worker usage spend: a turn with two rightx receipts writes two `skill_spend`
  `usage` rows. (`-p right-bot`)
- Migration: v38 idempotency test alongside existing `vNN_*` tests. (`-p right-db`)
- Dashboard: SSR tests for Knowledge per-skill buckets/cache and Usage buckets/cache;
  keep `usage_overview_sources_match_learning_sources_constant` green. (`-p right-dashboard`)
- **Final (mandatory):** `devenv shell -- cargo test --workspace`.

## Build sequence

1. v38 migration + `skill_spend` table (+ idempotency test).
2. `insert_skill_spend` helper in `right-agent`.
3. Probe-writer stdout unification + usage/`skill_spend` write (part 2 + create/patch).
4. Worker post-turn `usage` spend rows.
5. Curator `maintain` spend.
6. Prefilter sandbox skill index (part 1) + worker client wiring.
7. Dashboard Knowledge + Usage read models + api_types.
8. Frontend Knowledge + Usage rendering.
9. Docs (`ARCHITECTURE.md`/satellite, `PROMPT_SYSTEM.md`).
10. Final full workspace test.

## Risks / open questions

- **Prefilter client wiring:** the prefilter must obtain an `OpenShellClient` +
  `sandbox_id`. If no client is readily threadable to the worker's post-turn task,
  the plan may instead read the index over the same ssh path the prefilter already
  uses for `build_claude_command` (still in-sandbox, not the `openshell` CLI). gRPC
  is preferred per conventions; ssh-cat is the fallback if client threading is
  costly.
- **Usage attribution overlap:** `kind='usage'` rows double-count cost across skills
  when a turn uses several. This is intentional and must be labeled "attributed" in
  the UI; do not sum usage across skills and present it as an exact agent total.
- **Curator skill granularity:** if a curator pass can't attribute work to a single
  skill, it writes no `skill_spend` row (stays in `usage_events`), to avoid invented
  numbers.
- **Budget gate behavior change:** the daily learning budget (`today_spend_usd`
  sums `usage_events` over `LEARNING_SOURCES`, which already includes
  `learning_probe_writer`) currently sees ~$0 from the probe-writer because its rows
  are never written. After part 2, the probe-writer's real cost (the expensive part
  of the pipeline) counts toward the `max_daily_budget_usd` default of $1/day, so the
  gate will trip sooner and more honestly. No code change required, but the $1 default
  may warrant revisiting once true spend is visible.
