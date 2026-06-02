# Force-notify cron trigger

**Date:** 2026-06-02
**Status:** Design approved, pending implementation plan

## Problem

Agents often need to force-run an existing cron job — to verify it works
and to guarantee the user sees the result. The `cron_trigger` MCP tool
already fires a job on demand (it sets `triggered_at`; the reconciler runs
the job on the next tick). But delivery of the forced run's result is
blocked by two gates that a plain trigger cannot bypass:

1. **The job's own decision.** Each cron turn emits `delivery.kind =
   notify | silent`. A `silent` decision marks the run
   `delivery_required = 0` and the user sees nothing
   (`cron.rs::persist_successful_cron_output`).
2. **The idle gate.** Even a `notify` result is held until the chat has
   been idle for `IDLE_THRESHOLD_SECS`
   (`async_delivery.rs::should_wait_for_idle`). A user actively chatting
   and waiting on a forced check does not receive it until the chat goes
   quiet.

The trigger tool's own description tells the agent delivery is conditional
and idle-gated. So the agent works around it by creating a **second cron
that watches the first** — a costly, indirect hack. This design replaces
that hack with a first-class force-notify trigger.

## Goal

`cron_trigger(job_name, notify=true)` runs the job immediately and
guarantees a prompt Telegram message with a substantive report,
overriding both gates. The result lands in the main CC session (existing
delivery path), so the agent can follow up with `cron_show_run` /
`cron_list_runs` to read logs for debugging.

Scope: agent + MCP only. No user-facing Telegram `/cron trigger` command.

## Non-goals

- Synchronous trigger (returning the run's output inline to the calling
  turn). Delivery stays async through the main session.
- Background continuations (`async_runs.kind = 'background'`). Cron only.
- Changing scheduled-run behavior. The force-notify flag is transient and
  affects only the one forced run.

## Design

### MCP surface

`CronTriggerParams` gains an optional flag, fully backward-compatible:

```rust
pub struct CronTriggerParams {
    pub job_name: String,
    #[serde(default)]
    pub notify: bool,
}
```

- `notify = false` (default): today's behavior, unchanged.
- `notify = true`: force-notify run (below).

### State plumbing

Two idempotent column additions (pragma-checked per the migration rules
in `ARCHITECTURE.md`):

- `cron_specs.trigger_force_notify INTEGER NOT NULL DEFAULT 0` — set
  together with `triggered_at` by `trigger_spec`, cleared together. Added
  to the `CronSpec` struct and the spec-load `SELECT`. Excluded from
  spec-equality comparison, exactly like `triggered_at`, so toggling it
  never aborts an already-running job.
- `async_runs.force_notify INTEGER NOT NULL DEFAULT 0` — stamped on the
  run row at `insert_running_run`, read by the delivery loop.

Both columns default to `0`, so already-deployed agents adopt the change
on `right restart` with no migration step (per the upgrade model).

### Flow when `notify = true`

1. `trigger_spec(conn, job_name, force_notify=true)` →
   `UPDATE cron_specs SET triggered_at = now, trigger_force_notify = 1
   WHERE job_name = ?`.
2. Reconciler triggered branch (`cron.rs`, the `spec.triggered_at.is_some()`
   loop) reads both fields from the loaded spec, clears both atomically
   (extend `clear_triggered_at` to reset `trigger_force_notify = 0` in the
   same `UPDATE`), runs the existing lock check, and spawns
   `execute_job(force_notify = true)`.
3. `execute_job` gains a `force_notify: bool` parameter. When set:
   - Prepend a one-line directive to the run prompt:
     `⟨⟨SYSTEM_NOTICE⟩⟩ Manual verification trigger: always emit
     delivery.kind="notify" with a complete report of what you found; do
     not go silent. ⟨⟨/SYSTEM_NOTICE⟩⟩`
   - Pass `force_notify` to `insert_running_run`, which writes
     `force_notify = 1` on the `async_runs` row.
4. On successful completion, `persist_successful_cron_output` receives
   `force_notify`. When set:
   - `delivery_required = 1` always (override the silent mapping).
   - If the run still chose `silent`, rewrite `delivery_json` to a notify
     decision carrying the silent `reason` as content (reuse the
     `notify_delivery_json` helper at `cron.rs:88`). This guarantees
     `format_async_yaml` has content to deliver even if the model ignored
     the directive.
   - The failure path is untouched — failed runs already notify via the
     reflection summary.
5. Delivery loop (`async_delivery.rs::run_delivery_once`): gate the idle
   wait on the flag — `if !pending.force_notify && should_wait_for_idle(mode,
   idle_for)`. Forced rows skip the idle gate and deliver promptly through
   the main session. `PendingAsyncResult` gains a `force_notify: bool`
   field; `fetch_pending_batch` and `deduplicate_job` select the column.
   `should_wait_for_idle` stays a pure function; the flag is applied at the
   call site.

### Agent guidance

This is the teaching that retires the watcher-cron hack. Update:

- `TRIGGER_TOOL_DESC` (`right-agent/src/cron_spec.rs`) — the source of
  truth for the tool description — to document `notify=true`: "force a
  verification report regardless of the job's own silent decision and the
  idle gate — use this instead of creating a second cron to watch a job."
  The `#[tool(description = ...)]` literal on `cron_trigger` in
  `memory_server.rs` must be updated to match verbatim; the
  `cron_trigger_description_matches_const` test enforces this equality.
- The tool-list instruction block in `memory_server.rs` (the
  `mcp__right__cron_trigger: …` line) to mention the `notify` flag.
- `PROMPT_SYSTEM.md` (keep in sync with the MCP instruction surface).
- The cron section of `docs/architecture/sessions.md` (cite-on-touch).

Note: `aggregator.rs::with_instructions` covers only the memory/search
tools and is **not** touched by this change.

`ARCHITECTURE.md` is not modified: this is a behavioral addition, not a new
contract or review-blocking rule, so the detail lives in the satellite.

## Known limits (documented, not fixed)

- **Force-trigger while locked is dropped.** If the job is already running
  when triggered, the trigger (and its force-notify flag) is cleared and
  skipped — identical to a plain trigger today. The in-flight run delivers
  per its own decision.
- **Recurring jobs:** the flag is transient, so only the forced run
  notifies; scheduled runs are unaffected.

## Tests (TDD)

Write each failing test first.

- `trigger_spec` sets both `triggered_at` and `trigger_force_notify`;
  `clear_triggered_at` resets both; the loaded `CronSpec` carries
  `trigger_force_notify`.
- Forced + silent run: `persist_successful_cron_output` with
  `force_notify=true` and a silent decision yields `delivery_required=1`,
  `delivery_status='pending'`, and a `delivery_json` of kind `notify` whose
  content is the silent `reason`.
- Forced + notify run: persists normally with `force_notify=1` on the row.
- Delivery loop skips the idle gate for `force_notify=1` rows; still gates
  non-forced rows (regression).
- Non-forced regression: silent still maps to `delivery_required=0` /
  `delivery_status='none'`.
- `CronTriggerParams` deserializes with `notify` defaulting to `false` when
  absent.
- Both migrations are idempotent (re-running against a schema that already
  has the columns is a no-op).

## Verification cadence

- Targeted package tests during implementation:
  `devenv shell -- cargo test -p right-agent`,
  `devenv shell -- cargo test -p right-bot`,
  `devenv shell -- cargo test -p right` after the relevant slices.
- Final, mandatory: `devenv shell -- cargo test --workspace`.

## Files touched

| File | Change |
|---|---|
| `crates/right-db/src/migrations.rs` | Add `cron_specs.trigger_force_notify` and `async_runs.force_notify` migrations (idempotent) |
| `crates/right-agent/src/cron_spec.rs` | `CronSpec.trigger_force_notify`; spec-load `SELECT`; equality exclusion; `trigger_spec(force_notify)`; `clear_triggered_at` resets both; `TRIGGER_TOOL_DESC` |
| `crates/bot/src/cron.rs` | Reconciler triggered branch reads/passes flag; `execute_job(force_notify)` prompt directive; `insert_running_run` writes `force_notify`; `persist_successful_cron_output` override |
| `crates/bot/src/async_delivery.rs` | `PendingAsyncResult.force_notify`; select in `fetch_pending_batch` + `deduplicate_job`; idle-gate bypass at call site |
| `crates/right/src/memory_server.rs` | `CronTriggerParams.notify` (shared struct); `cron_trigger` `#[tool(description)]` literal + tool-list instruction block; pass `notify` to `trigger_spec` |
| `crates/right/src/right_backend.rs` | `call_cron_trigger` passes `params.notify` to `trigger_spec` (line ~538); tool description |
| `PROMPT_SYSTEM.md` | Sync trigger-tool description |
| `docs/architecture/sessions.md` | Cron section: force-notify trigger |
