# Dashboard failure counts — gray at zero, click to see every failure

Status: design (approved)
Date: 2026-05-31
Scope owner: andrey

## Goal

Two changes to every place the dashboard shows a failure count:

1. **Zero is calm.** A failure count of `0` renders in neutral gray, not
   red. Color (red) is reserved for the non-zero state that actually has
   something to act on.
2. **Non-zero is a door.** Clicking a non-zero failure count reveals the
   complete, untruncated list of those failures — the list length equals
   the count, so "53" opens exactly 53 rows.

## In scope

Three failure-count `MetricCard`s and their drill-downs:

- **Overview** "Failures" (`OverviewView.vue:65`) → `recent_failures`
  (failed `async_runs`, last 24h, all kinds).
- **Activity** "Failures" (`ActivityView.vue:30`) →
  `summary.failed_recent_cron_count` (recomputed; see Decision 4).
- **Reports / Knowledge** "Failed 7d" (`learning/ReportsView.vue:31`) →
  `lifecycle.failed_or_aborted_7d` (failed/aborted `skill_learning_events`,
  last 7d).

## Out of scope

- `HealthView.vue:51` doctor `fail_count` — rendered as a plain value (not
  a red badge) and already lists the failing checks. Has neither problem.
- Refactoring App.vue's existing per-cron run-detail selection flow. The
  new failures drill-down is additive and self-contained.
- New HTTP routes. The failure lists ship inside existing page payloads;
  run detail reuses the existing `api/v1/runs/{id}` endpoint.
- Pagination of failure lists. Lists are untruncated within their window
  (see Decision 5). Revisit only if an agent routinely produces hundreds
  of failures per window.
- Any prompt, `ARCHITECTURE.md`, or DB-migration change. These are
  read-only additions over existing tables.

## Decisions

| # | Decision |
|---|---|
| 1 | Zero → neutral gray (`MetricCard` default tone). Non-zero → red (`bad`). No green "all clear" state — a failure counter at zero should say nothing, not celebrate. |
| 2 | The count→tone→interactivity mapping is one tested pure helper, `failureMetric(count)`, replacing the three hardcoded `tone="bad"` / inline-ternary call sites. |
| 3 | Failure rows ship **inside the existing page payloads** (overview, activity, learning responses) — no new endpoint and no new loading state *for the list itself*. Each list is untruncated and its length equals the card's count. (Per-row run detail is still fetched on demand via the existing `runDetail(id)`, reusing Activity's current detail-loading pattern.) Rationale: failures are normally few; lazy-fetching a usually-empty list is not worth a new internal route. |
| 4 | **Activity's count semantics change.** Today it counts *cron jobs with a failed run among their last 5 loaded runs* — not failures, and structurally capped at 5/cron so it cannot show "all." It is recomputed to **the number of failed cron runs in the last 7d**, equal to the new `failed_runs` list length. Window: 7d (Overview stays 24h, Reports stays 7d). |
| 5 | Each list is **untruncated within its count's window** and shares the count's window predicate, so `list.len() == count` holds by construction. (`recent_successful_events` truncates at `RECENT_EVENT_LIMIT`; the new `recent_failed_events` must NOT.) |
| 6 | Overview and Activity failure rows are the same `RunSummary` shape and reuse one shared `RunFailureList.vue` with inline error+log expansion via the generic `runDetail(id)` client. Reports rows are `LearningEventSummary` (no run detail) rendered inline. |

## Current state

All three counts come from **different populations**, so cross-navigation
would be wrong — each needs its own list:

| Card | Count source | Window | Failures already in payload? |
|---|---|---|---|
| Overview Failures | `async_runs WHERE status='failed'` | 24h | only as capped `signals` (`kind='run_failure'`, truncated to 30 mixed) |
| Activity Failures | crons with a `status='failed'` run in last-5 `recent_runs` | last-5/cron | only inside per-cron `recent_runs` (≤5/cron) |
| Reports Failed 7d | `skill_learning_events phase='finish' status IN ('failed','aborted')` | 7d | only as capped `recent_learning_signals` (truncated to 30 mixed) |

The capped signal lists are why a guaranteed-complete drill-down (Decision
3/5) needs dedicated, untruncated list fields rather than reusing
`signals` / `recent_learning_signals`.

Response structs live in `crates/right-dashboard/src/api_types.rs`; the
TS mirror is `frontend/src/types.ts`. Tone colors (`App.vue`): `bad`
`#a42323`, `ok` `#0d7a45`, `active` amber, default = inherited neutral.

## Design

### Backend — read-model additions (`crates/right-dashboard/src`)

Each list query reuses its count's exact window predicate (the existing
`coarse_timestamp_bounds` + precise in-Rust window filter) so length and
count stay equal.

1. **`api_types.rs`**
   - `DashboardOverviewResponse` += `recent_failed_runs: Vec<RunSummary>`.
   - `OverviewResponse` (activity) += `failed_runs: Vec<RunSummary>`.
   - `LearningLifecycle` += `recent_failed_events: Vec<LearningEventSummary>`.
   - `OverviewSummary.failed_recent_cron_count` keeps its name (minimal
     churn) but its meaning is now "failed cron runs in 7d" == `failed_runs.len()`.

2. **`read_model/dashboard_overview.rs`** — add `recent_failed_runs(conn,
   generated_at)`: failed `async_runs` in the same 24h window as
   `recent_failure_count`, untruncated, newest-first, mapped to
   `RunSummary`. Reuse the activity `RunSummary` row mapper — hoist
   `RUN_SUMMARY_COLUMNS` / `run_summary_from_row` to a shared read-model
   helper rather than duplicating the column list. `signals` is unchanged.

3. **`read_model/activity.rs`** — add `failed_cron_runs(conn, since_7d,
   now)`: `WHERE ar.kind='cron' AND ar.status='failed'`, windowed on
   `COALESCE(ar.finished_at, ar.updated_at, ar.created_at)` over the last
   7d (same coalesced timestamp the Overview run-failure window uses),
   untruncated, newest-first. Set `summary.failed_recent_cron_count =
   failed_runs.len()`. The existing `cron_runs` (LIMIT 5 per cron) and the
   per-cron cards are untouched. (The field name is retained to limit
   churn; its meaning is now "failed cron runs in 7d," not "crons with a
   recent failure.")

4. **`read_model/learning.rs`** — add `recent_failed_events(conn, agent,
   since_7d, now)`: mirrors `recent_successful_events` but `status IN
   ('failed','aborted')` and **no `truncate`**. Field carries
   `LearningEventSummary { skill_name, action, status, message, summary,
   created_at }`.

### Frontend — shared mechanics (`frontend/src`)

- **`components/failureMetric.ts`** (new, unit-tested): pure helper.
  `failureMetric(count) → { tone: 'default' | 'bad'; interactive: boolean }`
  — `count <= 0 → { 'default', false }`, `count > 0 → { 'bad', true }`.

- **`components/MetricCard.vue`**: add optional `interactive?: boolean`
  (default `false`). When `interactive`, the root renders as a focusable
  `<button>` (keyboard-accessible, with hover/focus affordance and a small
  caret) and emits `select`; otherwise it stays the current static
  `<article>`. Non-interactive callers are unchanged.

- **`components/CollapsibleSection.vue`**: add an optional controlled
  `open` prop + `update:open` emit (`v-model:open`), defaulting to the
  current internal `ref(defaultOpen)` behavior when unbound. This lets a
  clicked `MetricCard` open the section while the section's own chevron
  still toggles the same state. Backward-compatible.

- **`components/RunFailureList.vue`** (new): props `runs: RunSummary[]`.
  Self-contained: owns `selectedRunId` / detail / loading / error, fetches
  on row click via the existing `runDetail(id)` client, and renders run
  rows + inline `RunDetailResponse` (status dot, kind, short id, time,
  error message, log) — the same detail markup Activity already uses.
  Loading/empty/error go through `AsyncState`. Shared by Overview and
  Activity.

### Frontend — per-view wiring

Each view holds a `failuresOpen` ref, binds the card via `failureMetric`,
and renders one `CollapsibleSection` (title "Failures", count badge) below
the metric grid when the count is non-zero:

- **`OverviewView.vue`**: card `Failures` →
  `failureMetric(overview.recent_failures)`, `@select` toggles
  `failuresOpen`; section body = `<RunFailureList :runs="overview.recent_failed_runs" />`.
- **`ActivityView.vue`**: card `Failures` →
  `failureMetric(overview.summary.failed_recent_cron_count)`; section body
  = `<RunFailureList :runs="overview.failed_runs" />`. Per-cron run rows
  unchanged.
- **`learning/ReportsView.vue`**: card `Failed 7d` →
  `failureMetric(lifecycle.failed_or_aborted_7d)`; section body = inline
  failed-event rows (skill, action, `StatusPill` `bad`, message) over
  `lifecycle.recent_failed_events`, mirroring `LearningSignalPanel` row
  markup.

Conforms to the dashboard-primitives rule: `AsyncState` for
loading/empty/error, `CollapsibleSection` for the grouped list, decision
logic extracted to a tested `.ts` helper. No raw placeholder text.

## Data flow

Page load → existing read-model builder runs the new list query alongside
the existing count query (same window) → list rides in the existing JSON
response → view renders the gray/red `MetricCard` from `failureMetric` →
user clicks a red card → `failuresOpen = true` → `CollapsibleSection`
expands the already-delivered list → (run lists only) clicking a row
fetches `api/v1/runs/{id}` and expands error+log inline.

## Testing

Targeted first, full workspace last (per AGENTS.md cadence).

**Backend** (`cargo test -p right-dashboard`):
- `dashboard_overview`: `recent_failed_runs` contents, newest-first order,
  window alignment (`recent_failed_runs.len() == recent_failures`), empty.
- `activity`: failed-cron-runs list contents + 7d window; assert
  `failed_recent_cron_count == failed_runs.len()`. **Update** the existing
  `failed_recent_cron_count` assertion to the new semantics.
- `learning`: `recent_failed_events` includes failed AND aborted, is
  untruncated past `RECENT_EVENT_LIMIT`, excludes created/updated; assert
  `recent_failed_events.len() == failed_or_aborted_7d`.

**Frontend** (vitest / Vue SSR `renderToString`):
- `failureMetric.test.ts`: 0 → `{default,false}`; positive → `{bad,true}`.
- `MetricCard`: interactive renders `<button>` and emits `select`;
  non-interactive renders `<article>` (regression).
- `CollapsibleSection`: `v-model:open` controls visibility; unbound still
  uses `defaultOpen`.
- `RunFailureList`: renders rows; row click expands detail (mocked
  `runDetail`); empty state.
- `ReportsView`: failed-event rows render with `bad` pill.

**Final (mandatory):** `devenv shell -- cargo test --workspace` and the
frontend test/build (`build.rs` bundles the SPA).

## Files touched

- `crates/right-dashboard/src/api_types.rs` — 3 list fields.
- `crates/right-dashboard/src/read_model/dashboard_overview.rs` — new list fn.
- `crates/right-dashboard/src/read_model/activity.rs` — failed-cron-runs fn + count recompute; hoist `RUN_SUMMARY_COLUMNS`/`run_summary_from_row` to a shared helper.
- `crates/right-dashboard/src/read_model/learning.rs` — untruncated failed-events fn.
- `frontend/src/types.ts` — mirror the 3 fields.
- `frontend/src/components/failureMetric.ts` (+ `.test.ts`) — new helper.
- `frontend/src/components/MetricCard.vue` — interactivity.
- `frontend/src/components/CollapsibleSection.vue` — controlled `open`.
- `frontend/src/components/RunFailureList.vue` (+ test) — new shared list.
- `frontend/src/views/OverviewView.vue`, `views/ActivityView.vue`, `views/learning/ReportsView.vue` — wire card + section.
