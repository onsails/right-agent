# Learning Daily Budget and Circuit Breaker

**Date:** 2026-05-21
**Status:** Design approved; pending implementation plan

## Problem

Background learning grinds to a halt under two compounding bugs.

Stage 2 selector (`crates/bot/src/learning_episode.rs:1498`) and worker-side
skill review (`crates/bot/src/telegram/worker.rs:2373`) are both gated by
`try_mark_review_started` (`crates/right-agent/src/learned_skills.rs:535`). The
gate uses a shared SQL counter `skill_nudge_state.daily_review_count` capped at
`LEARNING_EPISODE_REVIEW_DAILY_LIMIT = 12` and `LEARNED_SKILL_REVIEW_DAILY_LIMIT = 12`.
The counter is incremented at *start*, never decremented, and counts failures
the same as successes.

Observed on production agent `him` on 2026-05-20 (UTC):

- 00:01–02:02 UTC: 12 selector invocations failed with `claude exited Some(1)`
  and empty stderr. Most likely caused by `episode_selector_max_budget_usd = 0.10`
  per-call cap (default in `crates/right-agent-config/src/lib.rs:94`) on
  `claude-opus-4-7[1m]` (~$15/M input tokens); the prompt alone exceeded $0.10.
- After 02:02 UTC: `daily_review_count = 12 = daily_limit` → gate returns
  `Skip(DailyLimit)` for every subsequent drain. The `Skip(DailyLimit)` arm at
  `learning_episode.rs:621–635` silently requeues the episode to `pending` with
  `ready_after = now + 90s`. No log, no status change.
- Result: 219 episodes stuck at `pending`, 15 marked `failed`. Dashboard shows
  "all PENDING" until UTC midnight rolls over, at which point 12 more episodes
  are processed (success or failure) and the cycle repeats.

Even with a healthy selector, the structural ceiling of 12 reviews per day
cannot keep up with cron-generated seeds (96/day at 15-minute cadence) plus
effort-threshold seeds. The backlog grows monotonically.

Learning costs are also invisible. `usage_events.source` enumerates only
`interactive`, `cron`, `reflection` — selector, reviewer, and skill-review
invocations record nothing. The user cannot see how much background learning
spends.

## Goals

- Replace per-call $ cap + per-day count with a single per-day $ budget over
  recorded learning spend.
- Stop failures from poisoning the gate.
- Surface learning costs in the usage dashboard with selector/reviewer/
  skill-review broken out.
- Bound the damage from a broken selector via a consecutive-failure circuit
  breaker that auto-recovers and alerts the operator.
- Apply the new gate uniformly to both consumers (Stage 2 drain + worker skill
  review) — one gate contract, one budget.

## Non-goals

- No live diagnosis of why CC exits 1 silently in this spec. After the per-call
  cap is gone, that hypothesis is testable; if it persists, a follow-up spec
  adds `--session-id` plumbing so `--debug-file` lands on a predictable path.
- No destructive ALTER to drop `daily_review_count` / `daily_review_date`
  columns. They become dead, dropped in a later release.
- No automatic backlog cleanup. One-off SQL is documented at the end of this
  spec for `him` and any agent in a similar state.
- No new dashboard visual grouping for `learning_*` sources. Three new source
  blocks render alongside existing ones; visual grouping is a separate
  dashboard task if it becomes a problem.
- No change to `episode_settle_seconds`, `episode_selector_model`, or any
  non-budget knob.

## Decision

Three coordinated changes:

1. **Daily $ budget gate.** A new `LearningConfig.max_daily_budget_usd` (default
   $5.00) replaces both `episode_selector_max_budget_usd` and the daily count
   constants. The gate sums `usage_events.total_cost_usd` for the current UTC
   day across `learning_selector`, `learning_reviewer`, `learning_skill_review`
   sources and blocks new reviews when the budget is exhausted.

2. **Circuit breaker.** New `skill_nudge_state` columns
   `consecutive_review_failures` and `review_circuit_open_until` track recent
   failures. After `circuit_failure_threshold` (default 5) failures in a row,
   the circuit opens for `circuit_cooldown_minutes` (default 60). Any success
   resets both fields. A Telegram alert fires once per 24h when the circuit
   opens.

3. **Learning usage recording.** Selector, reviewer, and skill-review
   invocations record `usage_events` rows with new source values
   `learning_selector`, `learning_reviewer`, `learning_skill_review`. The
   dashboard surfaces them as additional `UsageSourceSummary` entries.

## Schema

Migration in `crates/right-db/src/sql/`:

```sql
-- skill_nudge_state: circuit-breaker fields
ALTER TABLE skill_nudge_state ADD COLUMN consecutive_review_failures INTEGER NOT NULL DEFAULT 0;
ALTER TABLE skill_nudge_state ADD COLUMN review_circuit_open_until TEXT;
```

Idempotent via `pragma_table_info` check per `ARCHITECTURE.md` → SQLite Rules.

`daily_review_count` and `daily_review_date` stay in the schema. The code stops
reading and writing them. Removed in a later release.

`usage_events.source` is unconstrained TEXT — no migration needed for new
source values.

## Gate logic

`ReviewGateInput` becomes:

```rust
pub struct ReviewGateInput<'a> {
    pub signal_trigger: Option<ReviewTriggerKind>,
    pub daily_budget_usd: f64,
    pub now_utc: &'a str,  // RFC3339
}
```

`review_gate_decision_in_tx` (`crates/right-agent/src/learned_skills.rs:494`):

```text
read (review_running, review_circuit_open_until, tool_iters, interval)
   FROM skill_nudge_state WHERE agent_name = ?

if review_running != 0:
    return Skip(AlreadyRunning)

if review_circuit_open_until is set:
    if review_circuit_open_until > now_utc:
        return Skip(CircuitOpen)
    else:
        -- Window expired. Clear both fields so the next attempt has a fresh
        -- threshold-sized failure budget. Otherwise consecutive_review_failures
        -- stays elevated and the first post-cooldown failure immediately
        -- reopens the circuit.
        UPDATE skill_nudge_state SET
            review_circuit_open_until = NULL,
            consecutive_review_failures = 0
        WHERE agent_name = ?1

today_start = now_utc[..10] + "T00:00:00Z"
spent = SELECT COALESCE(SUM(total_cost_usd), 0.0) FROM usage_events
        WHERE source IN ('learning_selector', 'learning_reviewer', 'learning_skill_review')
          AND ts >= today_start
if spent >= daily_budget_usd:
    return Skip(DailyBudget)

if signal_trigger is Some:
    return Start(trigger)
if interval > 0 and tool_iters >= interval:
    return Start(EffortThreshold)
return Skip(BelowThreshold)
```

`ReviewGateDecision` adds `Skip(DailyBudget)` and `Skip(CircuitOpen)`; removes
`Skip(DailyLimit)`.

`try_mark_review_started` keeps the same shape but its UPDATE on Start sets
only `review_running = 1`. `daily_review_count` is not touched.

`mark_review_finished` and `mark_review_finished_in_tx` extend their UPDATE:

```sql
UPDATE skill_nudge_state SET
    review_running = 0,
    consecutive_review_failures = 0,
    review_circuit_open_until = NULL,
    -- existing fields unchanged
WHERE agent_name = ?1
```

New helper `record_review_failure(conn, agent_name, now_utc, threshold, cooldown_minutes)`
replaces the failure-path call to `clear_review_running`. It atomically:

```sql
UPDATE skill_nudge_state SET
    review_running = 0,
    consecutive_review_failures = consecutive_review_failures + 1,
    review_circuit_open_until = CASE
        WHEN consecutive_review_failures + 1 >= ?2
        THEN strftime('%Y-%m-%dT%H:%M:%SZ', datetime(?3, '+' || ?4 || ' minutes'))
        ELSE review_circuit_open_until
    END
WHERE agent_name = ?1
```

Returns `(new_failure_count: i64, opened_circuit: bool)` so callers can emit
the Telegram alert when `opened_circuit == true`. The helper computes
`opened_circuit` by reading the previous count inside the same transaction:
`opened_circuit = (previous_count < threshold) && (previous_count + 1 >= threshold)`.
This bounds alert-firing to the exact transition, not every subsequent
failure while the circuit stays open.

`clear_review_running` stays for the shutdown path (worker.rs:2585) — it
clears `review_running` only, with the existing semantics: no review actually
finished, no count to bump.

## Usage recording

Three new helpers in `crates/right-agent/src/usage/insert.rs`:

```rust
pub fn insert_learning_selector(conn, b: &UsageBreakdown, episode_id: i64) -> Result<(), UsageError>
pub fn insert_learning_reviewer(conn, b: &UsageBreakdown, episode_id: i64) -> Result<(), UsageError>
pub fn insert_learning_skill_review(conn, b: &UsageBreakdown, chat_id: i64, thread_id: i64) -> Result<(), UsageError>
```

`episode_id` is written into the existing `job_name` column to keep the
dashboard linkable to episode detail without adding a column.

Cost extraction reuses the same path as worker/cron — CC `--output-format json`
returns a final JSON with `total_cost_usd` and `model_usage_json` (per the
`UsageBreakdown` parser already in `right-agent/src/usage`). The selector
already parses CC stdout for the structured output payload; the parser is
extended to also produce a `UsageBreakdown` from the same JSON envelope.

Failure path records nothing. The CC subprocess returning exit 1 with empty
stderr usually means it died before producing any usage info. The circuit
breaker handles spam.

## Config

`crates/right-agent-config/src/lib.rs::LearningConfig`:

```rust
pub struct LearningConfig {
    pub episode_selector_model: Option<String>,
    pub episode_settle_seconds: u64,

    pub max_daily_budget_usd: f64,               // default 5.00
    pub circuit_failure_threshold: u32,          // default 5
    pub circuit_cooldown_minutes: u32,           // default 60

    // Soft-deprecated. Kept as Option<f64> for backward compat. Not read by
    // any code. A warn-log fires at agent load if Some(_). Removed in a
    // later release.
    #[serde(default)]
    pub episode_selector_max_budget_usd: Option<f64>,
}
```

All three new fields use `#[serde(default = "...")]` with `default_*`
functions and `deserialize_with` validators (positive_finite for `f64`,
positive for `u32`).

`agent.yaml::learning.max_daily_budget_usd` overrides the default per-agent.

Removed constants (no replacement):
- `LEARNING_EPISODE_REVIEW_DAILY_LIMIT` (`learning_episode.rs:25`)
- `LEARNED_SKILL_REVIEW_DAILY_LIMIT` (`worker.rs:55`)

Both call sites of `try_mark_review_started` pass
`daily_budget_usd: runtime.learning.max_daily_budget_usd` instead of
`daily_limit: <const>`.

## Telegram alert

Shared dedup helpers move from `crates/bot/src/telegram/memory_alerts.rs` to
a new `crates/bot/src/telegram/alerts.rs`:

```rust
pub(crate) fn should_fire(db: &Path, alert_type: &str) -> bool
pub(crate) fn record_fire(db: &Path, alert_type: &str)
```

`memory_alerts.rs` keeps the watcher loop and uses the new module. A new
`crates/bot/src/telegram/learning_alerts.rs` exposes:

```rust
pub(crate) async fn maybe_alert_circuit_open(
    bot: &teloxide::Bot,
    db: &Path,
    agent_name: &str,
    allowlist_path: &Path,
    last_failure_reason: &str,
) -> Result<()>
```

Called from `learning_episode.rs::mark_claimed_episode_failed` and
`worker.rs::run_background_learned_skill_review` failure paths when
`record_review_failure` returns `opened_circuit = true`.

Alert text (HTML, via `tg::*` helpers):

```text
❌ Learning review circuit opened

Selector failed 5× in a row. New reviews paused for 1 hour.

Last error: <truncated to 200 chars>
➡️ Check ~/.right/logs/<agent>.log for details.
```

Recipient: first chat from `allowlist.yaml` for the agent. `alert_type` key:
`"learning_circuit_open"`. 24-hour dedup window.

## Dashboard

`crates/right-dashboard/src/read_model/usage.rs`:

```rust
const SOURCES: [&str; 6] = [
    "interactive",
    "cron",
    "reflection",
    "learning_selector",
    "learning_reviewer",
    "learning_skill_review",
];
```

No type changes. `UsageWindow.sources` and `UsageWindow.per_model` already
aggregate over arbitrary source values. The Mini App renders three new
`UsageSourceSummary` blocks alongside the existing three.

Visual grouping (one collapsed "learning" block) is out of scope. If three
extra blocks crowd the view, a separate dashboard task handles UI grouping.

## Per-call cap removed

`episode_selector_max_budget_usd` is no longer passed to
`ClaudeInvocation::max_budget_usd`. Selector and reviewer rely on:

- `--max-turns 3` for selector / `--max-turns N` for reviewer (existing).
- The daily budget for total spend bounds.
- The circuit breaker for runaway failure spam.

If a single invocation grows unexpectedly large (e.g., a giant corpus), the
post-hoc cost lands in `usage_events` and the next gate query catches it.

## Testing

### Unit (right-agent)

`learned_skills.rs::tests`:

- `gate_skips_when_daily_budget_exceeded`
- `gate_skips_when_circuit_open`
- `gate_starts_when_circuit_window_expired_and_clears_field`
- `gate_sums_only_today_utc_learning_sources` — vary `ts` and `source` to
  prove date filter + source filter.
- `record_review_failure_increments_and_opens_at_threshold`
- `record_review_failure_does_not_reopen_already_open_circuit`
- `mark_review_finished_resets_circuit_and_failures`

### Unit (bot)

`learning_episode_tests.rs`:

- `failed_selector_writes_no_usage_event` — mock selector returns `Err`,
  assert no row in `usage_events`.
- `successful_selector_records_learning_selector_event` — mock selector
  returns `Ok` with known `UsageBreakdown`, assert row inserted with
  `source = "learning_selector"`.
- `circuit_open_skips_drain_silently_and_requeues` — `review_circuit_open_until`
  in the future, drain pass leaves episode `pending` with bumped `ready_after`,
  no log error.

### Unit (dashboard)

`usage.rs::tests`:

- `usage_overview_includes_learning_sources` — insert three event types,
  assert all surfaced in `UsageWindow.sources`.

### Verification cadence

- Per crate after each commit: `devenv shell -- cargo test -p <crate>`.
- After all commits: `devenv shell -- cargo test --workspace` (mandatory final).
- Post-deploy manual: trigger one selector on a test agent, verify
  `usage_events` row + dashboard rendering + that a forced selector failure
  does not consume budget.

## Commit sequence

1. `feat(db): add circuit-breaker fields to skill_nudge_state`
2. `feat(agent): record learning costs in usage_events`
3. `feat(agent): daily-budget review gate with circuit breaker`
4. `feat(config): max_daily_budget_usd and circuit knobs in LearningConfig`
5. `refactor(bot): split telegram alert dedup into reusable module`
6. `feat(bot): telegram alert on learning circuit open`
7. `feat(dashboard): include learning sources in usage overview`
8. `docs(architecture): document new gate contract and learning sources`

## Operational notes

After this change ships, `him` needs a one-time cleanup:

```sql
-- 1. Free stuck pending episodes older than 24h.
UPDATE learning_episodes
SET status = 'no_episode',
    selector_output_json = json_object('status', 'no_episode', 'reason', 'stale_cleanup'),
    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')
WHERE agent_name = 'him'
  AND status = 'pending'
  AND created_at < datetime('now', '-24 hours');

-- 2. Reset gate state so the next drain starts clean.
UPDATE skill_nudge_state SET
    daily_review_count = 0,
    daily_review_date = NULL,
    consecutive_review_failures = 0,
    review_circuit_open_until = NULL,
    review_running = 0
WHERE agent_name = 'him';
```

Run before the first `right restart him` after the new code is deployed.

## Future work

If after removing the per-call cap the selector still exits 1 silently:

1. Generate a `--session-id` per selector and reviewer invocation
   (`learning_episode.rs::run_episode_selector`, `run_episode_review_invocation`).
   Currently both set `new_session_id: None`, which means `--debug-file` is
   omitted even when `--debug` is on.
2. On failure, `openshell sandbox download` the debug file to
   `~/.right/logs/learning/<episode_id>-<sid>.log`.
3. Open a dedicated spec for selector debug capture.

A later release should drop the dead `daily_review_count` /
`daily_review_date` columns from `skill_nudge_state` and remove the
soft-deprecated `episode_selector_max_budget_usd` config field.

## Risks

- **CC JSON cost extraction.** The new `insert_learning_*` helpers rely on
  `total_cost_usd` and `model_usage_json` in CC's `--output-format json`
  envelope. If CC changes the schema, parsing breaks silently (failure path
  writes nothing). Mitigated by tests against the current schema.
- **Source filter via `IN (...)`.** Future learning-adjacent sources must be
  explicitly added to a single source-of-truth constant. Introduce
  `right-agent::usage::LEARNING_SOURCES: &[&str] = &["learning_selector",
  "learning_reviewer", "learning_skill_review"]` and have both the gate
  query (built dynamically from the slice) and the dashboard `SOURCES` array
  reference it. A test asserts that every learning source appears in the
  dashboard's `SOURCES` constant.
- **Backward compatibility.** Existing `agent.yaml` files with
  `episode_selector_max_budget_usd: 0.10` keep loading. A warn log appears at
  bot start. No functional impact.
