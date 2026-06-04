# Usage Panel: Local-Time Ranges and Token Legends

**Status:** Design approved, implementation plan written

## Problem

The Usage tab labels a window as `Today`, but the backend currently computes
that window from the UTC date. A user in a non-UTC timezone can see foreground
chat spend from their local day omitted from `Today`, even though the
underlying `usage_events` accounting is correct.

The panel also shows token color meaning only in a sticky bottom legend. Source
rows and selected-day details use the same token colors near the top of the
page, so users must infer or scroll to decode input/output/cache values.

## Goals

- Make Usage windows and daily chart buckets use the viewer's local calendar
  timezone.
- Show explicit time ranges for every Usage window.
- Duplicate the token legend at the top and bottom of the Usage tab.
- Keep source spend and token accounting server-side.
- Limit scope to the Usage tab.

## Non-Goals

- Do not change Overview or Activity `Today` cards in this spec.
- Do not add a persistent agent/operator timezone setting.
- Do not change `usage_events` storage or timestamp format.
- Do not move accounting aggregation into the frontend.
- Do not redesign the source spend chart legend.

## Decisions

Use a timezone-aware Usage API. The frontend sends the browser's IANA timezone
from `Intl.DateTimeFormat().resolvedOptions().timeZone` to
`/dashboard/{agent}/api/v1/usage`. The backend uses that timezone to compute
calendar windows and daily buckets, then converts local bounds to UTC instants
for SQL filtering.

Window semantics are local calendar windows:

- `Today`: local current day `00:00` through generated-at.
- `Last 7 days`: local start of day six days before today through generated-at.
- `Last 30 days`: local start of day twenty-nine days before today through
  generated-at.
- `All time`: no lower bound; upper bound is generated-at.

Each window carries explicit range metadata:

- `range_start`: RFC3339 timestamp for the effective lower bound, or `null` for
  all-time.
- `range_end`: RFC3339 timestamp for generated-at in the selected timezone.
- `range_label`: human-readable label such as
  `Asia/Dubai · Jun 4 00:00-20:47`.

The top-level Usage response includes the effective `timezone` string. If the
client omits timezone or sends an invalid value, the backend falls back to
`UTC` and includes a dashboard warning that the timezone was invalid or absent.

## UI Design

The Usage tab shows one global `TokenLegend` above the chart/breakdown area and
keeps the existing sticky bottom token legend. The top legend is not sticky.

Each window header shows:

- eyebrow: window key, for example `today`;
- title: window label, for example `Today`;
- subline: `window.range_label`;
- right side: total cost.

The selected-day breakdown should show a local range in its header instead of
only a date. Daily point dates remain `YYYY-MM-DD`, interpreted in the response
timezone. The frontend can format the selected-day range from the selected
date, effective timezone, and response `generated_at`: full past days show
`00:00-23:59`, while the current local day ends at generated-at local time.

The ECharts source legend remains inside the spend chart. It is separate from
the token legend and is not duplicated by this design.

## Data Flow

1. Vue calls `usageOverview()` and appends `timezone=<browser IANA timezone>`.
2. The dashboard handler validates the query parameter and builds
   `UsageOverviewInput { timezone }`.
3. The read model computes local bounds and daily labels using a timezone-aware
   library, not manual offset arithmetic.
4. SQL queries continue to read `usage_events.ts` and filter by UTC-converted
   coarse bounds before precise timestamp parsing.
5. The response returns local-window totals, local daily series, effective
   timezone, and explicit range labels.
6. Vue renders top and bottom token legends, window range sublines, and selected
   day range text.

## Error Handling

Invalid or missing timezone does not break the panel. The backend uses `UTC`,
returns successful Usage data, and includes a `DashboardDataWarning`.

Timestamp parse errors, database errors, and other read-model failures retain
the current fail-fast behavior and should still surface as API errors.

## Testing

Backend tests:

- `Asia/Dubai` `Today` includes UTC-previous-evening rows that fall after local
  midnight.
- `Last 7 days` and `Last 30 days` start at local midnight, not rolling
  7x24/30x24 hour windows.
- invalid timezone falls back to `UTC` and emits a warning.
- response windows include `range_start`, `range_end`, and `range_label`.

Frontend tests:

- `usageOverview()` sends the browser timezone query parameter.
- `UsageView` renders both top and bottom token legends.
- window cards render `range_label` under the title.
- selected-day breakdown renders a local range.

Verification cadence for implementation:

- Run targeted Rust and Vue tests after the feature slice.
- Before declaring implementation complete, run
  `devenv shell -- cargo test --workspace`.
