# Curator Observability & Safety — Design

**Date:** 2026-06-15
**Status:** Draft (brainstorm output, pre-plan)
**Owner:** andrey

## Problem

The periodic skill curator is **already built, wired, and enabled by default** —
not deferred. A per-agent 60s ticker (`crates/bot/src/lib.rs:1051`) runs
`learning_curator::run_if_due`; `curator_enabled` defaults `true` and
`curator_paused` defaults `false` (`crates/right-agent-config/src/lib.rs:104,119`).
It applies automatic `stale→archive` transitions and runs an LLM consolidation
pass (`CURATOR_SYSTEM_PROMPT`, `crates/right-codegen/src/agent_def.rs:145`) that
umbrella-merges near-duplicate `rightx-*` skills, demotes narrow skills into an
umbrella's `references/`, and archives them with `absorbed_into`. It redirects
cron links on archive (`maintain_cron_links_for_archived`). This is exactly the
behavior the v0.4.1 article ("Self-evolving cron jobs") describes.

The gap is not the mechanism — it is **proof and safety**:

1. **Invisible.** `curator_state` is a singleton (last run only). There is **no
   run history**, so we cannot show "the curator ran N times, merged these,
   archived those, here's the cost/why." The existing dashboard "Curator
   triggered" signal (`read_model/learning.rs:513`) reads that singleton and is
   effectively a single bit. The article's consolidation claim is therefore
   **unbacked by any data** in any deployment.
2. **Circuit breaker is a no-op.** `consecutive_failures` increments and resets,
   but `circuit_open_until` is **never set by runtime** — only by tests
   (`learning_curator.rs:510` `TODO(Phase-2)`). A persistently-failing curator
   retries every cooldown forever and burns budget on a fork that is guaranteed
   to fail.
3. **Idle gate is dead.** The ticker passes `latest_user_activity_at = None`
   (`lib.rs:1091`), so the designed `min_idle_hours=2` gate never fires. The
   curator can mutate the skills directory (`mv` into `.archive/`, umbrella
   edits) while a foreground turn is actively reading those same skills.
4. **No cautious mode.** Hermes's own guidance (our research,
   `docs/research/hermes-ai-skills.html`) is "ship the curator report-only first,
   enable writes once you trust it." RightClaw shipped write-mode first with no
   report-only option.

## Goals

- **A1 — Observability:** a curator run-history table + a Knowledge-view dashboard
  panel that proves the curator runs and shows what each pass consolidated /
  archived and why.
- **B1 — Circuit breaker:** make `circuit_open_until` actually open after repeated
  failures (fixed cooldown), preserving the existing gate.
- **B2 — Idle gate:** feed the ticker a real activity timestamp so `min_idle_hours`
  works.
- **B3 — `curator_mode: apply | report_only`:** an opt-in cautious mode that
  proposes consolidations without writing, surfaced in the dashboard.
- Preserve the **archive-not-delete** invariant across every path.

## Non-goals

- **A2 — prefilter precision/recall.** Persisting every prefilter
  `Skip/Patch/Create` decision and joining to curator verdicts is about the
  *prefilter*, not the curator, and roughly doubles the work. Deferred (open a
  follow-up issue if wanted).
- **Per-item approval.** `report_only` is **observational**: the operator reads
  the plan and switches the agent to `apply` to execute. A per-proposal "Apply"
  button is a full approval workflow — deferred.
- **Recover from archive.** Restoring an archived skill to active is
  onsails/right-agent#134. This spec only *guarantees* archives stay recoverable.
- **Consolidation quality evaluation/tuning.** onsails/right-agent#132 — blocked
  on A1's telemetry.
- **Changing the consolidation logic / `CURATOR_SYSTEM_PROMPT` itself.**

## Background: what already exists (do not rebuild)

| Piece | Location |
|---|---|
| Per-agent 60s ticker | `crates/bot/src/lib.rs:1051` |
| Gate (`should_run_now`, cost-spike / skill-change / time-fallback + cooldown/idle/circuit) | `crates/bot/src/learning_curator.rs:88` |
| `curator_state` singleton (gate working state) | migration `v29_*.sql` |
| Automatic `stale→archive` transitions | `right_lifecycle::apply_automatic_transitions` |
| LLM consolidation pass + prompt | `learning_curator.rs:568`, `agent_def.rs:145` |
| Pre-pass snapshot (excludes `.archive/`, `.curator_backups/`) | `crates/bot/src/lifecycle/snapshot.rs` |
| `maintain` spend attribution | `learning_curator.rs:149` |
| Cron-link redirect/drop on archive | `learning_curator.rs:631` |
| Existing (weak) dashboard signals | `read_model/learning.rs:513,669` |

## Design

### A1 — Curator run history + dashboard

**New table `curator_runs`** (migration **v48**), append-only history. Distinct
from `curator_state`, which **stays** the gate's mutable working state (last run,
failures, circuit). The curator writes **one `curator_runs` row per *executed*
pass** — including zero-outcome passes ("ran, nothing to merge"), since proving
the curator is alive is half the point. Best-effort at the learning boundary: an
insert failure warns and continues, never aborts the pass.

Per-row content: `run_at`, `trigger` (`cost_spike` / `skill_change` /
`time_fallback`) + `trigger_evidence_json`, `mode` (`apply` / `report_only`),
`status` (`success` / `failed` / `proposed`), `cost_usd` + cache token columns,
outcome counts (consolidations, archives), a short `summary`, an `actions_json`
blob, and `invocation_id`.

- **apply-mode counts/actions** are derived from the pass's lifecycle diff
  (`archived_skill_names`, the subset with `absorbed_into` set) plus the curator
  invocation's `skill_learning_events` rows (create/update of umbrellas). Exact
  derivation lands in the plan.
- **report_only actions** come from the structured plan (see B3); the row is
  written `status='proposed'` with `actions_json` = the proposed plan and zero
  applied counts.

**Dashboard panel** (Knowledge view): a run timeline (when / trigger / status /
cost), consolidation **lineage** projected from `skill_lifecycle.absorbed_into`
("`A` + `B` → umbrella `C`"), and recent archives. Rendered through the mandated
primitives (`AsyncState.vue`, `CollapsibleSection.vue`); pure decision logic in a
`*.ts` helper, SSR component test. The existing singleton-based "Curator
triggered" overview signal is superseded by the run-history source.

### B1 — Circuit breaker

A pure, unit-tested function decides the post-failure circuit state:

```
next_circuit_open_until(consecutive_failures, threshold, cooldown_hours, now)
    -> Option<DateTime<Utc>>   // Some(now + cooldown) once failures >= threshold
```

Wiring in `run_if_due`'s state update: the several early-return failure branches
(invocation registration, command build, spawn/wait) are consolidated so the
**failure path is single-sourced** — increment `consecutive_failures`, then apply
`next_circuit_open_until`. A successful pass resets `consecutive_failures = 0` and
clears `circuit_open_until` (already the case). `consecutive_failures` **persists
across circuit opens**, so a permanently-broken curator keeps re-opening at the
fixed cadence rather than hammering every cooldown; the A1 panel shows the streak
so an operator can `paused` it. The existing `cheap_skip` `SkipCircuitOpen` gate
is unchanged — it finally has a runtime writer.

Config: `curator_circuit_failure_threshold` (default **3**),
`curator_circuit_cooldown_hours` (default **24**, fixed — not exponential).

### B2 — Idle gate

Pass the existing `IdleTimestamp` Arc (`lib.rs:960`, an `AtomicI64` already
plumbed into the delivery loop) into the curator ticker instead of `None`,
converting to `DateTime<Utc>` for `latest_user_activity_at`. `min_idle_hours`
stays `2`. Semantics caveat recorded in the plan: `IdleTimestamp` tracks "last
delivery," a sufficient proxy for "conversation recently active"; a purpose-built
"last foreground turn at" signal is explicitly **not** built here.

### B3 — `curator_mode: apply | report_only`

New `LearningConfig` field `curator_mode`, default **`apply`** (current behavior —
no regression for deployed agents). Like the other `curator_*` config fields
(except `model`), a change is **applied on graceful restart** via `config_watcher`,
not in-place: the ticker captures a frozen `config.learning` clone at spawn
(`lib.rs:1045`), so a `curator_mode` edit respawns the ticker through the normal
config-change restart. (This corrects an earlier claim of per-tick hot-reload.)

`report_only` behavior:

- The gate runs identically. The automatic `stale→archive` transitions are
  **computed but not written** — surfaced as proposed actions, not applied.
- The LLM pass runs in a **read-only invocation**: `allowed_tools: ["Read"]`
  (no `Bash`, no `skill_learning_start/finish`), `--json-schema` set to a new
  **curator-plan schema** so the model returns a structured plan
  (`[{kind: merge|demote|archive, skills:[...], target, rationale}]`) instead of
  executing writes. The inventory it reasons over is already in the prompt;
  `Read` lets it inspect specific `SKILL.md` bodies. It is still session-bearing,
  so it **registers a `Curator` invocation and passes `--mcp-config` /
  `--strict-mcp-config`** per the `ClaudeInvocation` invariant — it simply grants
  no MCP tools, so the contract holds without a no-MCP architecture exception.
- The plan is persisted as a `curator_runs` row with `status='proposed'`. Nothing
  on disk or in `skill_lifecycle` changes.
- To execute, the operator switches the agent to `apply`; the next pass runs the
  normal write path.

This unifies cleanly with A1: report-only output *is* a `curator_runs` row.

## Data model

```sql
-- v48_curator_runs.sql
CREATE TABLE IF NOT EXISTS curator_runs (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    run_at                 TEXT NOT NULL,
    trigger                TEXT NOT NULL,          -- cost_spike | skill_change | time_fallback
    trigger_evidence_json  TEXT,
    mode                   TEXT NOT NULL,          -- apply | report_only
    status                 TEXT NOT NULL,          -- success | failed | proposed
    cost_usd               REAL NOT NULL DEFAULT 0,
    cache_read             INTEGER NOT NULL DEFAULT 0,
    cache_creation         INTEGER NOT NULL DEFAULT 0,
    consolidations         INTEGER NOT NULL DEFAULT 0,
    archives               INTEGER NOT NULL DEFAULT 0,
    summary                TEXT,
    actions_json           TEXT NOT NULL DEFAULT '[]',
    invocation_id          TEXT,
    created_at             TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_curator_runs_run_at ON curator_runs(run_at);
```

Idempotent (`CREATE TABLE/INDEX IF NOT EXISTS`); registered in
`right_db::migrations::MIGRATIONS`. `curator_state` is unchanged.

## Configuration

New `LearningConfig` fields (`crates/right-agent-config/src/lib.rs`), all with
backward-compatible defaults that preserve current behavior:

| Field | Default | Effect |
|---|---|---|
| `curator_circuit_failure_threshold` | `3` | failures before circuit opens |
| `curator_circuit_cooldown_hours` | `24` | fixed circuit cooldown |
| `curator_mode` | `apply` | `apply` (write) or `report_only` (propose) |

Wizard prompts updated for the three knobs (init wizard already prompts curator
settings — `feat(wizard)` `4e80122e`). These are `MergedRMW` `agent.yaml` fields;
no new codegen output.

## Invariants & cross-cutting

- **Archive-not-delete** is preserved on every path. `report_only` writes nothing;
  `apply` keeps the existing archive-only behavior (state flip + `mv` into
  `.archive/` + pre-pass snapshot). No code in this spec deletes a skill package
  or a `skill_lifecycle` row.
- **Best-effort learning boundary.** `curator_runs` inserts and circuit-state
  writes follow the existing pattern: warn-and-continue, never abort the pass —
  except the `curator_state` save, which already retries once on transient
  `DbError` (load-bearing gate accounting).
- **Transaction rule.** Any multi-write step (e.g. writing a `curator_runs` row
  alongside `curator_state`) uses a single immediate transaction per
  `right-db` rules.
- **Doc sync (cite-on-touch):** update `docs/architecture/learning.md` (curator
  section), `PROMPT_SYSTEM.md` (if the report-only plan prompt/schema is added to
  the prompting system), and `ARCHITECTURE.md` only if a new invariant/contract
  needs prescribing (the `curator_mode` write/no-write contract is a candidate —
  ≤3 sentences or a satellite link).

## Verification cadence

Targeted intermediate checks (TDD red/green per slice), one final full workspace
run.

- **Intermediate (per slice):**
  - `cargo nextest run -p right-db` — v48 migration idempotency + round-trip.
  - `cargo nextest run -p right-bot learning_curator` — `next_circuit_open_until` pure
    fn; circuit opens after threshold and resets on success; `report_only` gate
    produces a `proposed` row and writes nothing; idle gate fed a real timestamp
    skips when active.
  - `cargo nextest run -p right-agent-config` — new field defaults preserve
    current behavior.
  - `cargo nextest run -p right-dashboard` — `curator_runs` read_model projection
    + SSR component test for the panel.
- **Final (mandatory, from any worktree touched):**
  - `devenv shell -- cargo nextest run --workspace`
  - `devenv shell -- cargo test --doc --workspace`

## Files touched

- `crates/right-db/src/sql/v48_curator_runs.sql` + `migrations.rs` registration.
- `crates/right-agent-config/src/lib.rs` — 3 fields + defaults + wizard
  serialization; deprecation table untouched.
- `crates/bot/src/learning_curator.rs` — circuit pure fn + single-sourced failure
  path; `report_only` read-only invocation branch; `curator_runs` writer;
  count/action derivation.
- `crates/bot/src/lib.rs` — pass `IdleTimestamp` Arc into the ticker.
- `crates/right-codegen/src/agent_def.rs` — curator-plan JSON schema + report-only
  prompt variant.
- `crates/right-dashboard/src/read_model/` + Vue view — run-history projection and
  panel (+ `*.ts` helper, SSR test).
- `crates/right/src/wizard.rs` — prompts for the new knobs.
- Docs: `docs/architecture/learning.md`, `PROMPT_SYSTEM.md`, `ARCHITECTURE.md`
  (only if a contract is added).

## Open questions (resolve in plan)

1. **`actions_json` shape** shared by apply (applied actions) and report_only
   (proposed actions) — one schema with an `applied: bool`, or distinct shapes.
2. **Curator-plan JSON schema** exact fields for `report_only` output.
3. **apply-mode count derivation** precise source(s) — lifecycle diff vs
   `skill_learning_events` join — and how "consolidations" vs "archives" are
   distinguished without double-counting an absorbed-then-archived skill.
