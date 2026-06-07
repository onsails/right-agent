# Dashboard Usage Ranges and Cron Job Breakdown

- **Date:** 2026-06-07
- **Status:** Approved design - ready for implementation plan
- **Area:** `right-dashboard` backend read model and frontend Usage tab

## Problem

The dashboard Usage tab currently receives every usage window and renders a
multi-window list. The requested behavior is a single selected Usage mode:
`Today`, last 3 days, last 7 days, last 30 days, or all time, with `Last 7 days`
selected by default. The tab should not show all periods at once.

The tab also aggregates cron spend only as the broad `cron` source. Operators
need to see which cron jobs are responsible for usage inside the selected
period.

## Goals

- Add Usage range modes: `today`, `last_3_days`, `last_7_days`,
  `last_30_days`, and `all_time`.
- Default the Usage tab to `last_7_days`.
- Render one selected period at a time.
- Make the selected period a backend query parameter, not frontend-only
  filtering.
- Add a cron-job breakdown for the selected period, grouped by `job_name`.
- Keep timezone-aware local calendar semantics from the existing Usage API.

## Non-Goals

- Do not add a persisted user preference for the selected range.
- Do not include cron-parent `reflection` rows in the cron job breakdown.
- Do not change `usage_events` storage or add a migration.
- Do not redesign pricing, cache hit-rate, or token visualization.
- Do not change Overview or Activity usage cards.

## Decisions

Use a backend-selected range. The frontend calls:

```text
GET /dashboard/{agent}/api/v1/usage?timezone=<iana>&range=<range-key>
```

Allowed range keys:

- `today`
- `last_3_days`
- `last_7_days`
- `last_30_days`
- `all_time`

If `range` is absent or empty, the backend uses `last_7_days`. If `range` is
invalid, the backend uses `last_7_days` and emits a `DashboardDataWarning` with
source `usage.range` and kind `invalid_range`.

Use local calendar windows:

- `today`: local current day start through `generated_at`.
- `last_3_days`: local start of day two days before today through
  `generated_at`.
- `last_7_days`: local start of day six days before today through
  `generated_at`.
- `last_30_days`: local start of day twenty-nine days before today through
  `generated_at`.
- `all_time`: no lower bound, through `generated_at`.

The response should represent one selected range. Do not overload the existing
`selected_window` string field with an object. Add an explicit selected window
object and keep legacy fields only where useful for compatibility:

```text
selected_range: string
window: UsageWindow
daily_series: UsageDailyPoint[]
source_series: UsageSourceSeries[]
cron_jobs: UsageCronJobSummary[]
```

`windows` may remain temporarily as a one-element compatibility field, but the
frontend must not use it. `selected_window` may remain as the selected range key
for old consumers during the transition.

## Cron Job Breakdown

Cron breakdown is computed only from `usage_events.source = 'cron'`. Rows where
`source = 'reflection'` and `job_name` is set remain part of the existing
`reflection` source accounting, not the cron-job breakdown.

Add:

```text
UsageCronJobSummary {
  job_name: string
  cost_usd: number
  subscription_cost_usd: number
  api_cost_usd: number
  turns: number
  invocations: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
  web_search_requests: number
  web_fetch_requests: number
  per_model: UsageModelSummary[]
}
```

Group by `job_name`. If a cron usage row has `job_name IS NULL`, render it as
`(unknown job)` instead of dropping it silently. Sort cron jobs by `cost_usd`
descending, then `job_name` ascending.

The aggregation should reuse the same parsing and token/cost accounting rules
as source summaries. Per-model totals come from `model_usage_json`, matching
the existing source and window summaries.

## UI Design

The Usage tab renders a segmented control at the top:

```text
Today | 3 days | 7 days | 30 days | All time
```

The active segment defaults to `7 days`. Switching a segment updates local UI
state and refetches Usage with the same browser timezone and the selected
`range` key. No backend config is written.

The tab shows only the selected period:

- The spend chart title uses `window.label`, not a hardcoded `Last 30 days`.
- The selected-day breakdown uses `daily_series` from the selected range.
- The summary panel renders `window`, not every available window.
- Source rows and token lines keep the current visual treatment.
- A cron-job breakdown section renders `cron_jobs` for the selected range.

When the selected range changes, reset the selected day to the latest date in
the new `daily_series`. If `daily_series` is empty, clear the selected day.

For `all_time`, daily buckets run from the first recorded usage event through
`generated_at`, using local calendar labels. This is exact rather than capped.
If all-time history later becomes too large, downsampling is a separate feature.

## Data Flow

1. `UsageContainer.vue` stores the selected range in local state, defaulting to
   `last_7_days`.
2. `usageOverview()` accepts `{ timezone, range }` and appends both query
   parameters.
3. The dashboard handler deserializes `range` into `UsageOverviewInput`.
4. The read model resolves the effective range and timezone, computes one
   `UsageWindowRange`, and converts local bounds to UTC for SQL filtering.
5. The read model builds `window`, `daily_series`, `source_series`, and
   `cron_jobs` from the selected bounds.
6. `UsageView.vue` renders the selected range control, selected window summary,
   selected chart, day breakdown, and cron-job rows.

## Error Handling

Invalid or missing timezone behavior stays as-is: use `UTC`, return data, and
emit a warning.

Invalid range is non-fatal: use `last_7_days`, return data, and emit a warning.
The frontend should render the effective `selected_range` from the response so
the active segment reflects backend fallback.

Timestamp parse errors, database errors, and malformed model JSON behavior stay
consistent with the current read model. Malformed model JSON in daily series
continues to warn and skip model detail where the current code does so.

## Testing

Backend tests:

- Default usage range is `last_7_days`.
- `last_3_days` uses local calendar semantics, not rolling 72 hours.
- Invalid range falls back to `last_7_days` and emits a `usage.range`
  `invalid_range` warning.
- `daily_series` length follows the selected range for `today`, `last_3_days`,
  `last_7_days`, and `last_30_days`.
- `all_time` buckets from the first recorded usage local date through
  `generated_at`.
- `cron_jobs` groups only `source = 'cron'` rows by `job_name`.
- Cron-parent `reflection` rows with `job_name` do not appear in `cron_jobs`.
- `job_name IS NULL` cron rows appear as `(unknown job)`.

Frontend tests:

- `usageOverview()` sends both `timezone` and `range`.
- `UsageContainer` defaults to `last_7_days`.
- Clicking a range segment refetches with the selected range.
- `UsageView` renders only the selected `window`.
- The spend chart title follows `window.label`.
- Cron job rows render cost and token lines.
- Invalid backend fallback updates the active segment from `selected_range`.

Verification cadence for implementation:

- Run targeted Rust read-model tests after backend changes.
- Run targeted frontend tests after API and component changes.
- Before declaring implementation complete, run
  `devenv shell -- cargo test --workspace`.

## Risks

- Changing the response shape can break fixtures or old frontend assumptions.
  Keep `windows` as a one-element compatibility field during the transition and
  move the new UI to `window`.
- `all_time` daily bucketing can become large if an agent accumulates long
  history. Exact bucketing is the correct first implementation; cap or
  downsample only with a separate product decision.
- `job_name IS NULL` should be rare for `source = 'cron'`, but the schema allows
  it. Rendering `(unknown job)` makes bad data observable instead of hiding it.
