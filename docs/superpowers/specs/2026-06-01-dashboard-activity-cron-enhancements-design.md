# Dashboard Activity cron enhancements

**Date:** 2026-06-01
**Status:** Approved design, pre-plan
**Scope:** Enrich the existing Activity tab's cron view in place. No new tab.

## Problem

Cron jobs are already visible in the dashboard under the **Activity** tab
(`ActivityView.vue`, fed by `read_model/activity.rs::activity_overview`). The
premise "no way to see crons" is false — but the existing view is too thin and
has one interaction bug:

1. **Schedule is shown raw.** The card renders `{{ cron.schedule }}` — e.g.
   `0 8 * * *` instead of "At 08:00 AM, every day". A human-readable helper
   (`right_agent::cron_spec::describe_schedule`, via the `cron_descriptor`
   crate) exists but is not wired to the dashboard.
2. **No next-fire time for recurring jobs.** The "Next" cell reads
   `cron.run_at`, which is non-null only for one-shot `RunAt` jobs. Recurring
   crons show nothing — you cannot see when one fires next.
3. **No actual spend.** The card shows the per-run cap (`max_budget_usd`) but
   not what the job has actually cost. There is no way to see or sort by spend.
4. **Expanded run detail will not collapse.** `ActivityContainer.selectRun`
   always assigns `selectedRunId = id`; a second click on the same run does not
   close it.
5. **No delete.** A cron can only be removed by the agent's `cron_delete` MCP
   tool; the operator has no UI affordance.

## Goals

- Human-readable schedule + next-fire time, consistent with what the
  reconciler actually computes.
- Per-cron actual spend over **last 24h** and **last 7d**, with a sort control.
- Toggle (collapse) of expanded run detail.
- Delete-cron button with confirmation, routed through a bot-owned domain
  function (write-contract compliant).

## Non-goals

- No new dashboard tab; no rename of "Activity".
- No cron **create/edit** UI (delete only).
- No prompt text in the card (YAGNI; trivial to add later as one field).
- No change to the failures block, background-runs section, or run-detail
  panel contents beyond the collapse fix.

## Resolved decisions

- **Next-run for recurring:** included.
- **Prompt in card:** excluded (YAGNI).
- **Default sort:** `Name`; selector also offers `Spend 24h ↓`, `Spend 7d ↓`.
- **Confirm mechanism:** `Telegram.WebApp.showConfirm`, fallback to
  `window.confirm` outside Telegram.
- **Schedule/next-fire helper placement:** a small local module in
  `right-dashboard` (`read_model/schedule.rs`) built on the `cron` and
  `cron_descriptor` workspace crates, rather than adding a production
  dependency on the heavy `right-agent` crate (it is currently only a
  dev-dependency of `right-dashboard`). The duplicated logic is ~10 lines of
  stable cron-format handling, pinned by a unit test against the reconciler's
  expected next-fire (see Testing). The delete **route handler** lives in the
  bot crate, which already depends on `right-agent`, so the delete path reuses
  `right_agent::cron_spec::delete_spec` with no new dependency.

## Data model

### `CronCard` (api_types.rs + frontend `types.ts`) — new fields

| Field | Type | Source |
|---|---|---|
| `schedule_human` | `String` | `describe_schedule` for cron exprs; absolute time for `RunAt`; "Immediately (next tick)" for `@immediate` |
| `next_run_at` | `Option<String>` (RFC3339) | recurring/one-shot cron: `cron::Schedule::after(now).next()`; `RunAt`: the `run_at` value; `@immediate`: `None` (label conveys imminence); no future fire: `None` |
| `spend_24h_usd` | `f64` | sum of `usage_events.total_cost_usd` for this `job_name` since `now − 24h` |
| `spend_7d_usd` | `f64` | same, since `now − 7d` |

The raw `schedule` field stays on `CronCard` (used as secondary/title text).
`run_at`, `target_chat_id`, `target_thread_id`, `max_budget_usd`,
`recurring` are unchanged.

### Spend query (read_model/activity.rs)

One grouped query over the 7-day window, conditional-summed into both windows,
keyed by `job_name`, then mapped onto the already-loaded cron rows (missing →
`0.0`):

```sql
SELECT job_name,
       SUM(CASE WHEN ts >= :since_24h THEN total_cost_usd ELSE 0 END) AS spend_24h,
       SUM(total_cost_usd) AS spend_7d
FROM usage_events
WHERE job_name IN (<loaded job names>) AND ts >= :since_7d
GROUP BY job_name
```

`since_24h` / `since_7d` derive from `input.generated_at` (already parsed to
UTC by the existing `today_cost_usd` path). Reuse the same RFC3339 string
comparison discipline already documented in `today_cost_usd`.

### `read_model/schedule.rs` (new)

Pure functions, no DB:

- `describe(schedule: &str, run_at: Option<&str>, recurring: bool) -> String`
- `next_run_at(schedule: &str, run_at: Option<&str>, recurring: bool, now: DateTime<Utc>) -> Option<DateTime<Utc>>`

`next_run_at` mirrors the reconciler's parsing exactly: convert the 5-field
expression to 7-field the same way `crates/bot/src/cron.rs::to_7field` does
(prepend seconds `0`, append year `*`), parse with `cron::Schedule::from_str`,
call `.after(&now).next()`. `@immediate` → `None`; `RunAt` (run_at present) →
parse `run_at`.

## Backend write path — delete

New route in `crates/bot/src/telegram/dashboard.rs`, mirroring the skill-pin
write precedent (`handle_pin_skill`):

```
DELETE /dashboard/{agent}/api/v1/crons/{job_name}  -> handle_delete_cron
```

`handle_delete_cron`:
1. `authenticate_api(&state, &agent, &headers)`.
2. Resolve `agent_dir` from `state` (same as the skills handler).
3. `let conn = right_db::open_connection(&agent_dir, false).await?;`
4. `right_agent::cron_spec::delete_spec(&conn, &job_name, &agent_dir).await`
   — deletes the `cron_specs` row and the lock file; returns `Err` with
   "not found" when the row is absent.
5. Map `Ok` → `200 { "deleted": true, "job_name": ... }`; "not found" →
   `404`; other errors → `500` with `format!("{e:#}")` (preserve chain).

`async_runs` history is intentionally left intact (audit trail; one-shot rows
already self-delete via the reconciler's `OneShotSpecDeleter`). This is
behaviorally identical to the existing agent-initiated `cron_delete`, so
reconciler teardown is already handled — no new reconciler logic.

Write-contract compliance: the handler calls the bot-owned domain function
`delete_spec`, not hand-rolled SQL, exactly as `handle_pin_skill` delegates to
its domain helper.

## Frontend

### `ActivityView.vue`

- **Header:** show `schedule_human` as the primary schedule line; raw
  `schedule` becomes the `title`/muted secondary. Add a small muted target hint
  (`→ {target_chat_id}[/{thread}]`) here, freeing the meta-grid.
- **meta-grid (4 cells):** `Next` (`next_run_at` formatted, or "—" when
  absent; `@immediate` semantics are carried by `schedule_human`, so the cell
  never needs to special-case schedule kind) · `24h` (`money(spend_24h_usd)`) ·
  `7d` (`money(spend_7d_usd)`) · `Cap` (`money(max_budget_usd)`). Drop the old
  `Target` and `Recent` cells (target moved to header; recent count is
  redundant with the runs list).
- **Sort control:** a compact segmented control / `<select>` above the cron
  list: `Name` (default) · `Spend 24h` · `Spend 7d`. Sorting is client-side
  (cron lists are small). The comparator is extracted to
  `views/cronSort.ts` (`sortCrons(crons, mode)`), unit-tested directly.
- **Delete:** a delete button in the card `panel-head` next to the
  `StatusPill`. On click → confirm (see below) → `deleteCron(job_name)` →
  on success `refresh()` the overview; on error surface via the existing
  error channel.

### `ActivityContainer.vue` — collapse fix

`selectRun` toggles: if `selectedRunId.value === run.id`, clear
`selectedRunId`/`selectedRun`/`detailError` and return early; otherwise the
current fetch logic runs. The toggle predicate is trivial and covered by a
container/SSR test. Both the inline detail and the aside panel are keyed on
`selectedRunId`, so clearing it collapses both.

### Confirm helper (`telegram.ts`)

`confirmAction(message: string): Promise<boolean>` — resolve via
`window.Telegram?.WebApp?.showConfirm(message, cb)` when present, else
`Promise.resolve(window.confirm(message))`. Keeps the destructive-action
affordance idiomatic for the Mini App and testable (inject/stub the WebApp).

### `api.ts` / `types.ts`

- `deleteCron(jobName: string): Promise<{ deleted: boolean; job_name: string }>`
  → `DELETE api/v1/crons/${encodeURIComponent(jobName)}` (mirrors
  `setSkillPinned`'s request shape).
- Add the four new `CronCard` fields to `types.ts`.

## Testing

Targeted during development; one full `cargo test --workspace` at the end
(project cadence).

**Rust (read-model):**
- spend windows: events inside/outside 24h and 7d map to the right card; jobs
  with no events → `0.0`; midnight-boundary event included (reuse the existing
  RFC3339-format regression discipline).
- `schedule::describe` / `next_run_at` for each kind: recurring cron, one-shot
  cron, `RunAt`, `@immediate`, and an expression with no future fire (`None`).
- a pin/equivalence test asserting `next_run_at("0 8 * * *", …)` lands at the
  next 08:00 UTC — guards against drift from the reconciler's `to_7field`.

**Rust (delete route):** success (row + lock file gone), `404` not-found,
auth-failure rejected, and idempotent second delete → `404`. Follow the
`handle_pin_skill` test setup.

**Frontend (Vitest / SSR):**
- `cronSort.ts`: ordering for each mode, stable tie-break by name.
- collapse toggle: second `selectRun` on the same id clears selection.
- `confirmAction`: resolves `true`/`false` from a stubbed `showConfirm`, and
  falls back when WebApp is absent.
- delete flow: confirm → calls `deleteCron` → triggers refresh; declined
  confirm → no call.

## Touch list

- `crates/right-dashboard/src/read_model/activity.rs` (spend, new fields)
- `crates/right-dashboard/src/read_model/schedule.rs` (new)
- `crates/right-dashboard/src/api_types.rs` (`CronCard` fields)
- `crates/right-dashboard/Cargo.toml` (`cron`, `cron-descriptor` deps)
- `crates/bot/src/telegram/dashboard.rs` (delete route + handler + test)
- `crates/right-dashboard/frontend/src/views/ActivityView.vue`
- `crates/right-dashboard/frontend/src/views/ActivityContainer.vue`
- `crates/right-dashboard/frontend/src/views/cronSort.ts` (new)
- `crates/right-dashboard/frontend/src/telegram.ts` (`confirmAction`)
- `crates/right-dashboard/frontend/src/api.ts`, `types.ts`
- Tests alongside each.
