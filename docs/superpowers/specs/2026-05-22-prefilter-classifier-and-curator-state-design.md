# Prefilter Classifier + Curator State Refinement

**Date:** 2026-05-22
**Status:** Design
**Predecessor:** `docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md`

## 1. Background

The previous spec landed the closed-loop probe-writer + periodic curator system.
Two weeks of usage have surfaced refinements that this spec addresses:

1. **The Haiku prefilter is text-only.** It receives `user_msg_text` and
   `assistant_reply_text` but no signal about turn cost, depth, or latency.
   These signals correlate strongly with "was this turn nontrivial enough to be
   worth a probe-writer fork." Their absence forces Haiku to guess from prose
   alone.

2. **The prefilter is binary** (`Probe` / `Skip`). The probe-writer then
   re-derives create-vs-patch from scratch despite cheap Haiku context being
   available. This is duplicated work, and it produces an opaque decision trail
   (no record of *why* the system decided to learn).

3. **Curator state lives in `agents/<name>/.claude/skills/.curator_state.json`.**
   That JSON file is the only operational state for an agent that does *not*
   live in `data.db`. It bypasses backup, dashboard reads, future
   circuit-breaker tracking, and atomic multi-write semantics.

4. **Curator triggers only on the 168h interval.** Active agents may accrue
   enough material to warrant earlier consolidation; quiet agents need the time
   fallback. The single interval is too coarse.

## 2. Goals

- Prefilter receives per-turn cost/depth/latency **and** per-agent baselines
  for those metrics, so its decision is adaptive to each agent's normal load.
- Prefilter returns a three-way structured decision (`Skip` /
  `PatchExisting{target}` / `CreateNew{topic_hint}`), giving probe-writer a
  directed hint while retaining writer's right to refuse.
- Curator state moves into `data.db` as a singleton row, opening the door for
  circuit-breaker tracking and spike-evidence logging without further schema
  churn.
- Curator gains a cost-spike trigger (statistical, per-agent) and a
  skill-change-count trigger, with the existing 168h interval as a safety
  fallback for quiet agents.

## 3. Non-goals

- **Outcome tracking** (linking each prefilter decision to "skill kept /
  merged / archived" verdicts from curator). Designed for, but deferred to a
  separate Phase-2 spec. The structured `PrefilterDecision` enum and
  `curator_state.last_spike_evidence_json` field exist precisely so a later
  spec can land outcome tracking without re-architecting.
- **Auto-skip pre-gate in Rust** (skipping Haiku entirely for trivial turns
  based on absolute thresholds). Listed in §11 as a v2 optimization once
  baseline tooling exists.
- **Migrating existing `.curator_state.json`** to the new table. Per user
  decision: discard. New agents and existing agents both start fresh with
  the DB-backed state. The orphan file is harmless (no reader will look at
  it after this lands).
- **Replacing Haiku in the prefilter with rule-based scoring.** Text context
  remains load-bearing; raw numbers alone cannot distinguish "user discovered
  a non-obvious gotcha in 2 turns" from "trivial echo in 2 turns."

## 4. Design

### 4.1 Turn baselines (per-agent, computed on-demand)

A new `right_agent::usage::turn_baseline` module computes percentiles of
foreground-source `usage_events` over a configurable window (default 14 days).
It returns either `Baseline { p50, p90, p99, sample_size, window_days }` for
each of three metrics, or `Insufficient { sample_size, window_days }` when
fewer than 20 foreground turns are recorded in the window.

```rust
// crates/right-agent/src/usage/turn_baseline.rs (new)
pub struct TurnBaselines {
    pub sample_size: u32,
    pub window_days: u32,
    pub cost_usd: BaselineMetric<f64>,
    pub num_turns: BaselineMetric<u32>,
    pub wall_elapsed_ms: BaselineMetric<u64>,
}

pub enum BaselineMetric<T> {
    Insufficient { sample_size: u32 },
    Available { p50: T, p90: T, p99: T },
}

pub fn compute(
    conn: &rusqlite::Connection,
    window_days: u32,
    min_sample: u32,
) -> Result<TurnBaselines, UsageError>;
```

Implementation: a single SQL fetch of `(total_cost_usd, num_turns,
wall_elapsed_ms)` for `source='foreground'` rows in the window, then
in-memory percentile computation. Worst-case row count is bounded
(`<= turns_per_day * 14`), so the cost is trivial.

**Cold start:** if `sample_size < min_sample` (default 20), all three
metrics return `Insufficient`. The prefilter falls back to raw values
without baseline context, and Haiku's prompt explicitly says "baseline
insufficient."

### 4.2 `ProbeAnchor` extension

`crates/bot/src/telegram/worker.rs::ProbeAnchor` gains four fields:

```rust
pub(crate) struct ProbeAnchor {
    // existing
    pub user_msg_text: String,
    pub assistant_reply_text: String,
    pub main_session_uuid: String,
    pub captured_at: DateTime<Utc>,
    pub chat_id: i64,
    pub thread_id: i64,

    // new
    pub num_turns: u32,
    pub total_cost_usd: f64,
    pub wall_elapsed_ms: u64,
    pub used_skill_receipts: Vec<String>,
}
```

`num_turns` and `total_cost_usd` are already available from the result event
(`crates/bot/src/cc/stream.rs::UsageBreakdown`). `wall_elapsed_ms` is computed
in the worker from the turn's start instant to the result event arrival;
it never goes into the DB at this stage (see §4.5 for the DB column).
`used_skill_receipts` is the list of `rightx-*` skill names the foreground
turn used, parsed from CC's `mcp__right__use_skill` receipts during stream
processing.

The baseline is *not* embedded in `ProbeAnchor`. `learning_prefilter::run()`
computes it just before building the prompt; `build_prompt` gains a second
argument: `fn build_prompt(anchor: &ProbeAnchor, baselines: &TurnBaselines)
-> String`. The anchor remains a self-contained per-turn snapshot.

### 4.3 Prefilter as three-mode classifier

`crates/bot/src/learning_prefilter.rs::PrefilterDecision` becomes:

```rust
pub(crate) enum PrefilterDecision {
    Skip { reason: String },
    PatchExisting { target_skill: String, reason: String },
    CreateNew { topic_hint: String, reason: String },
}
```

The JSON schema enforced via `--json-schema`:

```json
{
  "type": "object",
  "properties": {
    "decision": {
      "type": "string",
      "enum": ["skip", "patch_existing", "create_new"]
    },
    "target_skill": { "type": "string", "pattern": "^rightx-[a-z0-9-]+$" },
    "topic_hint": { "type": "string", "maxLength": 120 },
    "reason": { "type": "string", "maxLength": 400 }
  },
  "required": ["decision", "reason"]
}
```

- `target_skill` is required iff `decision == "patch_existing"`.
- `topic_hint` is required iff `decision == "create_new"`.
- Validation in Rust enforces these conditional requireds after parsing;
  malformed output (e.g. patch without target) is logged and returns `Skip`.

The Haiku prompt is restructured around the receipts split:

- If `anchor.used_skill_receipts.is_empty()`: the prompt frames the question as
  "should a *new* skill be created?" Section listing existing skill index is
  abbreviated to short descriptions only.
- If receipts are non-empty: the prompt frames as "should one of these be
  patched, or does the turn reveal a procedure beyond their scope?" The cited
  skills' descriptions are quoted; everything else is abbreviated.

The "skill index summary" is a new derived view: one line per skill
(`<name>: <one-line description>`), where description comes from the
`description` field in each `rightx-*/SKILL.md` frontmatter. The probe-writer
already has `collect_host_rightx_skill_index` returning full content; the
prefilter shares the underlying scan but applies a summary projection. The
projection lives next to the existing helper.

In both modes, the turn-stats block is identical:

```
TURN STATS (this turn vs agent's 14d foreground baseline, n=247):
  num_turns:  18      (P50=4,    P90=12,    P99=24)
  cost:       $0.42   (P50=$0.03, P90=$0.18, P99=$0.95)
  elapsed:    28s     (P50=6s,   P90=22s,   P99=58s)
```

Or, on cold start:

```
TURN STATS (baseline insufficient, only n=8 prior turns):
  num_turns: 18, cost: $0.42, elapsed: 28s
```

The Haiku model and timeout remain unchanged (`claude-haiku-4-5-20251001`,
30s). The schema change is the only material wire-format change.

### 4.4 Probe-writer accepts directed hints

`crates/bot/src/learning_probe_writer.rs::ProbeWriterContext` gains an
`incoming_hint: PrefilterDecision` field (always `PatchExisting` or
`CreateNew` — `Skip` never reaches the writer).

The writer's prompt is templated based on hint:

- **`PatchExisting{target_skill, reason}`**: the prompt frames the task as
  "verify and patch `<target_skill>` if and only if you confirm the gap
  described in `reason`; otherwise exit silently." The skill index is still
  provided (writer can fall back to create-new if patch is unjustified), but
  the writer's first action is expected to be reading the target skill's
  `SKILL.md`.

- **`CreateNew{topic_hint, reason}`**: the prompt frames as "create a new
  rightx-* skill iff no existing skill covers `<topic_hint>`; otherwise
  patch the closest match or exit silently."

The writer's refusal path ("hint is wrong, no action taken") is a valid exit.
It's logged with the original hint's `reason` so the decision trail is
auditable. The `skill_learning_finish` MCP tool already returns a structured
result; it gains a `hint_outcome` field with values `applied_as_hinted`,
`applied_differently`, or `refused` for telemetry.

### 4.5 Schema migration: `wall_elapsed_ms` on `usage_events`

One new column on `usage_events`:

```sql
ALTER TABLE usage_events ADD COLUMN wall_elapsed_ms INTEGER;
```

Migration is idempotent (checks `pragma_table_info` before adding, per
`crates/right-db/src/migrations.rs` conventions). Existing rows have
`wall_elapsed_ms = NULL`, which baseline computation treats as "exclude from
window." The 14-day baseline window self-cleans once new turns accumulate.

Worker measures wall-clock from the moment the foreground CC subprocess is
spawned to the moment the result event arrives, and writes it into the
`UsageBreakdown` struct (`crates/bot/src/cc/stream.rs`). `insert_foreground`
(or equivalent) writes the value to the new column.

The column is **only** populated for `source='foreground'`. Baselines are
foreground-only; cron/learning/curator wall times are not relevant to the
prefilter's decision and are left `NULL`. The insert helpers in
`crates/right-agent/src/usage/insert.rs` accept `wall_elapsed_ms: Option<u64>`
and pass through.

### 4.6 Curator state in `data.db`

New table via `right-db` migration:

```sql
CREATE TABLE IF NOT EXISTS curator_state (
    agent_singleton_id INTEGER PRIMARY KEY CHECK (agent_singleton_id = 1),
    last_run_at        TEXT,
    last_run_status    TEXT,  -- 'success' | 'failed' | 'skipped'
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    circuit_open_until TEXT,
    last_spike_evidence_json TEXT
);
```

The `CHECK (agent_singleton_id = 1)` constraint enforces singleton semantics.
Reads use `SELECT ... WHERE agent_singleton_id = 1`; writes use
`INSERT OR REPLACE INTO curator_state (agent_singleton_id, ...)
VALUES (1, ...)`.

`crates/bot/src/learning_curator.rs::CuratorState` gains fields to mirror the
new schema columns:

```rust
pub(crate) struct CuratorState {
    pub last_run_at: Option<String>,
    pub last_run_status: Option<String>,
    pub consecutive_failures: u32,
    pub circuit_open_until: Option<String>,
    pub last_spike_evidence_json: Option<String>,
}
```

The existing `load_state`, `save_state`, and `curator_state_path` file-backed
helpers are replaced with DB-backed equivalents. The new signatures:

```rust
pub(crate) fn load_state(conn: &Connection) -> Result<CuratorState, ...>;
pub(crate) fn save_state(conn: &Connection, state: &CuratorState) -> Result<(), ...>;
```

`run_if_due` no longer takes an `agent_dir` for state purposes; it takes a
`Connection` (already available in `CuratorContext` via `agent_db_dir`).

The existing `.curator_state.json` file is **not** read on startup. If
present from a prior run, it sits as an orphan. The first DB-backed
`load_state` returns the default (`last_run_at: None`), seeds it on first
tick, and the system continues. Operators concerned about cleanup can
remove the file by hand; we will not bake automatic cleanup into the bot.

### 4.7 Curator trigger refinement

`should_run_now` (`crates/bot/src/learning_curator.rs`) is extended from a
single time-based gate to a multi-signal OR. Order of checks:

1. `enabled && !paused` — config-level gates.
2. `circuit_open_until` — if set and in the future, return `Skip`.
3. `min_idle_hours` — recent foreground activity gates.
4. `min_cooldown_hours` — a new field on `CuratorConfig`, default 12h.
   If `(now - last_run_at) < min_cooldown_hours`, return `Skip` regardless
   of other triggers. This prevents back-to-back runs on bursty days.
5. Any of the following returns `Run`:
   - **Cost spike:** sum of `total_cost_usd` for
     `source='learning_probe_writer'` over the last 24h, compared to the
     14-day P50 of daily probe-writer cost for this agent. Trigger when
     `today_sum >= k * p50_daily` (k=3.0, configurable) **and**
     `today_sum >= min_floor_usd` ($0.05 default). The floor handles
     low-activity agents where the P50 is near zero.
   - **Skill-change count:** count of skills in `.usage.json` where
     `created_at > last_run_at` OR `last_patched_at > last_run_at` is
     `>= curator_skill_change_threshold` (default 3).
   - **Time fallback:** `(now - last_run_at) >= curator_interval_hours`
     (default 168h, unchanged).
6. Otherwise, return `Skip`.

When a non-time trigger fires (cost spike or skill-change count), the
evidence is captured into `curator_state.last_spike_evidence_json` so the
dashboard and logs can show *why* the curator woke early. Format:

```json
{
  "trigger": "cost_spike" | "skill_change_count" | "time_fallback",
  "computed_at": "2026-05-22T14:00:00Z",
  "details": {
    "today_cost_usd": 0.42,
    "baseline_p50_usd": 0.08,
    "k": 3.0,
    "min_floor_usd": 0.05
  }  // for cost_spike; per-trigger shape varies
}
```

### 4.8 Configuration additions

`LearningConfig` (`crates/right-agent-config/src/lib.rs`) gains:

```rust
pub curator_cost_spike_k: f64,                   // default 3.0
pub curator_cost_spike_baseline_days: u32,       // default 14
pub curator_cost_spike_min_floor_usd: f64,       // default 0.05
pub curator_skill_change_threshold: u32,         // default 3
pub curator_min_cooldown_hours: u32,             // default 12
pub baseline_window_days: u32,                   // default 14 (prefilter)
pub baseline_min_sample: u32,                    // default 20 (prefilter)
```

All have `#[serde(default = ...)]` so existing agents pick up defaults
without `agent.yaml` edits. The wizard
(`crates/right/src/wizard.rs::cmd_agent_config`) gains prompts for each.

## 5. ASCII diagram

```
══════════════════════════════════════════════════════════════════════════════════
                     PER-TURN LOOP (on every foreground reply)
══════════════════════════════════════════════════════════════════════════════════

  ┌──────────────┐      ┌──────────────────────────────┐
  │  User msg    │ ───▶ │  Foreground CC turn          │
  │  (Telegram)  │      │  claude -p --resume <sess>   │
  └──────────────┘      └──────────────┬───────────────┘
                                       │
                          Worker captures ProbeAnchor:
                            • user_text, assistant_text
                            • num_turns, cost_usd, elapsed_ms        ← TURN STATS
                            • used_skill_receipts: [rightx-foo,...]
                                       │
                                       ▼
                        ┌──────────────────────────────┐
                        │  Reply sent to Telegram      │
                        │  (user unblocked here)       │
                        └──────────────┬───────────────┘
                                       │ fire-and-forget (tokio::spawn)
                                       ▼
                        ┌──────────────────────────────────┐
                        │  PREFILTER  (Haiku, ~$0.002)     │
                        │                                  │
                        │  Compute per-agent baselines from│
                        │   usage_events (14d, foreground) │
                        │                                  │
                        │  In:                             │
                        │   TURN STATS + percentiles       │
                        │   + used_skill_receipts          │
                        │   + skill index summary          │
                        │   + user/assistant text          │
                        │                                  │
                        │  Logic:                          │
                        │   receipts empty?                │
                        │     → "create new?"              │
                        │   receipts non-empty?            │
                        │     → "patch one? or new?"       │
                        │                                  │
                        │  Out: PrefilterDecision          │
                        └─┬──────────┬──────────┬──────────┘
                          │          │          │
                  ┌───────┘          │          └────────┐
                  ▼                  ▼                   ▼
            ┌──────────┐   ┌──────────────────┐  ┌───────────────────┐
            │   Skip   │   │  PatchExisting   │  │   CreateNew       │
            │ {reason} │   │  {target_skill,  │  │  {topic_hint,     │
            └──────────┘   │   reason}        │  │   reason}         │
                           └────────┬─────────┘  └─────────┬─────────┘
                                    │                      │
                                    └──────────┬───────────┘
                                               ▼
                        ┌──────────────────────────────────┐
                        │  PROBE-WRITER (Sonnet, ~$0.30)   │
                        │  claude -p --resume <sess>       │
                        │         --fork-session           │
                        │                                  │
                        │  Receives directed hint:         │
                        │   PatchExisting → focus target   │
                        │   CreateNew     → focus topic    │
                        │                                  │
                        │  Verifies hint OR refuses;       │
                        │  writes inside /sandbox:         │
                        │   .claude/skills/                │
                        │     rightx-<slug>/SKILL.md       │
                        │                                  │
                        │  skill_learning_finish reports:  │
                        │   hint_outcome (applied/         │
                        │   different/refused)             │
                        └──────────────┬───────────────────┘
                                       │
                       ┌───────────────┼────────────────────┐
                       ▼               ▼                    ▼
              ┌─────────────┐ ┌─────────────────┐ ┌──────────────────┐
              │ Reverse-sync│ │ .usage.json     │ │  Telegram        │
              │ to host:    │ │ bump:           │ │  receipt line:   │
              │  agents/    │ │  created_at /   │ │   "💡 ...        │
              │   .claude/  │ │  last_patched_at│ │    (rightx-foo)" │
              │   skills/   │ │  created_by =   │ │                  │
              │   rightx-*  │ │   probe_writer  │ │                  │
              └─────────────┘ └─────────────────┘ └──────────────────┘


══════════════════════════════════════════════════════════════════════════════════
                  PERIODIC LOOP (curator, orthogonal, ~168h)
══════════════════════════════════════════════════════════════════════════════════

  ┌────────────────────┐
  │  60s tokio ticker  │ ────── on every tick ──────┐
  │  (bot lib.rs)      │                            │
  └────────────────────┘                            ▼
                                ┌──────────────────────────────────┐
                                │  Read state: data.db             │
                                │   curator_state singleton row    │
                                │   { last_run_at, last_status,    │
                                │     consecutive_failures,        │
                                │     circuit_open_until,          │
                                │     last_spike_evidence_json }   │
                                └──────────────┬───────────────────┘
                                               │
                                               ▼
                                ┌──────────────────────────────────┐
                                │  Gate (in order):                │
                                │   1. enabled & not paused        │
                                │   2. circuit closed              │
                                │   3. min_idle_hours              │
                                │   4. min_cooldown_hours          │
                                │   5. trigger? (any-of):          │
                                │      ─ cost spike vs 14d P50     │
                                │      ─ skill-change count ≥ 3    │
                                │      ─ 168h time fallback        │
                                └──────────────┬───────────────────┘
                                               │ all pass
                                               ▼
                                ┌──────────────────────────────────┐
                                │  CURATOR RUN                     │
                                │                                  │
                                │  a) tar.gz snapshot:             │
                                │     curator_backups/<ts>/        │
                                │     skills.tar.gz                │
                                │                                  │
                                │  b) auto-transitions:            │
                                │     stale (30d no use)           │
                                │     archive (90d no use)         │
                                │     write to .usage.json         │
                                │                                  │
                                │  c) LLM consolidation:           │
                                │     claude -p (Sonnet, ~$$$)     │
                                │     NEW session, NO MCP config   │
                                │     → merge duplicates           │
                                │     → archive low-use            │
                                │     → patch metadata             │
                                │                                  │
                                │  d) update curator_state:        │
                                │     last_run_at = now            │
                                │     last_status = ...            │
                                │     reset failure counters       │
                                │     last_spike_evidence_json     │
                                └──────────────────────────────────┘


══════════════════════════════════════════════════════════════════════════════════
                                SHARED STATE
══════════════════════════════════════════════════════════════════════════════════

  data.db (per-agent SQLite):

  usage_events (per CC invocation):
   ts, source, session_uuid, total_cost_usd, num_turns,
   wall_elapsed_ms (NEW, NULL for non-foreground)
   sources: foreground, cron, learning_prefilter,
            learning_probe_writer, learning_curator, ...

   read by:
    • prefilter   (per-agent baselines, daily budget cap)
    • curator gate (14d P50 baseline for cost-spike trigger)
    • dashboard

  curator_state (singleton, NEW):
   last_run_at, last_run_status, consecutive_failures,
   circuit_open_until, last_spike_evidence_json

  Filesystem (per-agent, host side):
   agents/<name>/.claude/skills/
     rightx-<slug>/SKILL.md      ← skill files (mirrored from box)
     .usage.json                 ← lifecycle index (per-skill)
     .archive/                   ← archived skills
     curator_backups/<ts>/       ← curator tar.gz snapshots
     .curator_state.json         ← ORPHAN after this spec; ignored
```

## 6. Files touched

**New:**
- `crates/right-agent/src/usage/turn_baseline.rs` — `TurnBaselines`,
  `BaselineMetric<T>`, `compute()`.
- `crates/right-db/src/sql/v<next>.sql` (or migration in registry) — schema
  for `wall_elapsed_ms` and `curator_state`.

**Modified (Rust):**
- `crates/right-agent-config/src/lib.rs` — new `LearningConfig` fields with
  `#[serde(default)]`.
- `crates/right-agent/src/usage/mod.rs` — re-export `turn_baseline`.
- `crates/right-agent/src/usage/insert.rs` — `wall_elapsed_ms` parameter
  threading.
- `crates/right-db/src/migrations.rs` — register new migration.
- `crates/bot/src/cc/stream.rs` — `UsageBreakdown` gains `wall_elapsed_ms`.
- `crates/bot/src/telegram/worker.rs` — `ProbeAnchor` extension; measure
  wall-elapsed; parse `use_skill` receipts; pass new fields when constructing
  the anchor; write `wall_elapsed_ms` to `usage_events`.
- `crates/bot/src/learning_prefilter.rs` — `PrefilterDecision` enum reshape,
  new JSON schema, restructured prompt with baselines + receipts + skill
  index summary, parser validation for conditional requireds.
- `crates/bot/src/learning_probe_writer.rs` — accept `incoming_hint`, branch
  prompt template, propagate hint to `skill_learning_finish` payload for
  outcome reporting.
- `crates/bot/src/learning_curator.rs` — DB-backed `load_state` / `save_state`;
  new `should_run_now` signals (cost spike, skill-change count); evidence
  capture into `last_spike_evidence_json`.
- `crates/right/src/right_backend.rs` — `skill_learning_finish` accepts
  `hint_outcome` field; logs and records to usage trail.
- `crates/right/src/skill_lifecycle.rs` — no functional change; may grow
  `hint_outcome` helper if telemetry needs it.
- `crates/right/src/wizard.rs` — `cmd_agent_config` prompts for new
  LearningConfig fields.
- `crates/right-codegen/src/agent_def.rs` — update
  `PREFILTER_SCHEMA_JSON` constant and any JSON schemas embedded in prompts;
  update probe-writer system prompt template for hint-aware sections.

**Modified (Docs):**
- `ARCHITECTURE.md` — update "Skill learning loop" subsection to reflect
  3-mode prefilter, hint propagation, DB-backed curator state, multi-signal
  curator trigger.
- `PROMPT_SYSTEM.md` — update `PREFILTER_SCHEMA_JSON` section; describe
  TURN STATS / baseline rendering; describe hint-aware probe-writer prompt
  branches.
- `docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md`
  — add a header note pointing forward to this spec as a successor.

## 7. Verification cadence

Per project conventions (`AGENTS.md` → Verification cadence):

- **Baseline at worktree start**: targeted tests for the touched crates
  (`right-agent-config`, `right-db`, `right-agent::usage`, `bot::learning_*`).
- **Per-task**: narrowest useful command — typically
  `devenv shell -- cargo test -p <crate> <filter>` after each TDD cycle.
- **End of work**: `devenv shell -- cargo test --workspace`. Mandatory.

The plan derived from this spec must encode this cadence: no full-workspace
tests after every small step; one final full-workspace test before commit
batches close.

## 8. Test plan (sketched; full detail in the plan doc)

**`turn_baseline`:**
- Empty `usage_events` → all metrics `Insufficient { sample_size: 0 }`.
- 5 foreground rows, `min_sample=20` → `Insufficient { 5 }`.
- 50 foreground rows with known distribution → P50/P90/P99 match expected.
- Mix of foreground + cron rows → cron rows excluded.
- `wall_elapsed_ms = NULL` rows excluded from elapsed baseline only;
  cost/turns still computed.

**Prefilter parser:**
- Valid `skip` → `PrefilterDecision::Skip`.
- Valid `patch_existing` with target → `PatchExisting { target_skill, reason }`.
- Valid `create_new` with topic_hint → `CreateNew { topic_hint, reason }`.
- `patch_existing` without `target_skill` → `Skip` + warn log.
- `create_new` without `topic_hint` → `Skip` + warn log.
- Malformed JSON → `Skip`.
- `target_skill` not matching `^rightx-` pattern → `Skip` + warn log.

**Prefilter prompt:**
- Receipts empty → "create new?" framing visible in prompt.
- Receipts non-empty → "patch or new?" framing; cited skills appear with
  descriptions.
- Baseline available → percentile lines present.
- Baseline insufficient → "baseline insufficient, only n=X" line present.

**Curator state DB:**
- Fresh agent → `load_state` returns default; `save_state` then `load_state`
  round-trips.
- `INSERT OR REPLACE` semantics: two saves leave one row.
- `CHECK (agent_singleton_id = 1)` rejects other IDs.
- Migration adds column + table idempotently; running twice is no-op.

**Curator trigger:**
- Time fallback only: `(now - last_run_at) >= 168h`, no spike, no change-count
  → `Run` with `trigger=time_fallback`.
- Cost spike: 14d P50 = $0.05, today = $0.20, k=3.0 → `Run` with evidence.
- Cost spike below floor: P50 = $0.001, today = $0.10, floor = $0.05 → `Run`.
- Cost spike below floor *and* below k*P50: P50=$0.001, today=$0.001 → `Skip`.
- Skill-change count >= 3, < interval → `Run` with `trigger=skill_change_count`.
- All triggers fire, but cooldown active → `Skip`.
- `circuit_open_until` in future → `Skip` regardless of triggers.

**Probe-writer hint propagation:**
- `PatchExisting` prompt contains target skill name.
- `CreateNew` prompt contains topic hint.
- `hint_outcome=applied_as_hinted` reported when writer patches the target.
- `hint_outcome=applied_differently` reported when writer creates new despite
  patch hint.
- `hint_outcome=refused` reported when writer exits without writing.

## 9. Backward compatibility

- New `LearningConfig` fields have `#[serde(default)]` — existing `agent.yaml`
  loads unchanged.
- `wall_elapsed_ms` column is nullable; old rows accept `NULL`; baseline
  computation excludes NULL rows from the elapsed metric only.
- `.curator_state.json` orphan: no migration, no code reads it after this
  spec lands. Operators may delete by hand.
- `PrefilterDecision` enum reshape is a breaking change for the prefilter
  module only — no downstream caller serializes the variant set.
- `PREFILTER_SCHEMA_JSON` is rewritten; the wire format Haiku sees changes
  on the bot-restart that picks up the new code. No data migration needed
  (no historical decisions stored in DB yet).

## 10. Risks

- **Haiku misclassification on `target_skill`.** Mitigation: probe-writer
  verifies and may refuse; refusal path is logged with `hint_outcome=refused`.
  Outcome telemetry (deferred Phase-2) will reveal misclassification rates.
- **Baseline computation cost in hot path.** A 14-day foreground SQL on every
  prefilter run. Worst-case row count is bounded; if profiling shows a
  problem, cache results in `LearningConfig` with a 1-hour TTL.
- **Cost spike on a single expensive turn.** A $5 probe-writer turn could
  alone exceed the trigger threshold. Mitigation: `min_cooldown_hours` (12h
  default) prevents back-to-back curator runs; `consecutive_failures` circuit
  caps damage if the curator itself starts failing.
- **`.usage.json` race with curator.** Curator reads + writes; probe-writer
  also writes (bump_create / bump_patch). Existing `fs4` advisory-lock +
  tempfile-rename pattern handles this; no new locking needed.

## 11. Phase-2 (out of scope for this spec)

These are designed-for but explicitly deferred:

1. **Outcome tracking table.** `learning_outcomes` records each prefilter
   decision (with `decision`, `target_skill`/`topic_hint`, `reason`) and
   later joins to curator verdicts (`kept`, `merged_into`, `archived_unused`).
   Generates a per-agent precision/recall view for the dashboard.
2. **Auto-skip pre-gate.** A Rust-side gate that bypasses Haiku entirely when
   `num_turns < N && cost < $X && elapsed < Y && receipts.is_empty()`. Saves
   the prefilter invocation cost on trivial turns. Thresholds derived from
   the agent's own baseline percentiles.
3. **Outcome-driven prompt calibration.** Long-term: feed back outcome
   statistics into the prefilter prompt as system-prompt context ("you have
   historically misclassified create-new in domain X N times — be cautious").

## 12. Open questions

1. **Does `right agent rebootstrap` preserve `data.db`?** It should — the DB
   is part of the agent's identity, not codegen — but worth verifying so the
   new `curator_state` table survives. Verification step in the plan.
2. **Should `last_spike_evidence_json` cap size?** A small TEXT column is
   fine, but if evidence schemas grow unbounded, we should add a size cap
   in the writer.
3. **Hot-reload classification for new `LearningConfig` fields.** The
   `min_idle_hours`, `min_cooldown_hours`, etc. are read by the curator
   ticker each iteration, so they hot-reload naturally via the existing
   `Arc<ArcSwap<LearningConfig>>` pattern. Confirm in implementation.
