# Overview chart time-range zoom — design

## Problem

The Overview view (Telegram Mini App dashboard) shows the
`CostLearningRiver` chart — an ECharts `themeRiver` over a time
`singleAxis` for the `last_30_days` window, with learning-marker scatter
points on the same axis. The window is dense and there is no way to narrow
it: the user cannot inspect a shorter span (e.g. the last few days out of
30). We need horizontal time-range zoom.

## Scope

Horizontal zoom only — narrow the time range. No vertical zoom, no
generalized panning beyond what the zoom interaction implies. Single
chart: `CostLearningRiver`. No backend changes.

## Approach (chosen: slider + inside)

The `DataZoomComponent` is already registered in
`crates/right-dashboard/frontend/src/charts.ts`; only the chart `option`
needs configuration. Add zoom that works on both touch (Telegram Mini App,
primary) and desktop (mouse).

- `inside` dataZoom — mouse-wheel zoom on desktop, two-finger pinch on
  touch (ECharts default touch behavior).
- `slider` dataZoom — visible draggable band with handles; gives an
  explicit affordance and resets by dragging handles back to the edges.

Rejected alternatives:
- **slider only** — predictable and never traps page scroll, but no
  natural pinch/wheel. Kept as the one-line fallback if the `inside`
  one-finger pan on the chart proves annoying on mobile.
- **inside only** — no visible affordance or reset; one-finger pan on the
  chart can capture page-scroll gestures. Rejected for lacking discoverable
  control.

Accepted tradeoff: with `inside`, one-finger drag *on the chart area*
pans instead of scrolling the page (ECharts offers no touch "pinch-only"
flag). Scrolling outside the chart is unaffected; the slider is the
primary control.

## Changes

Single file: `crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue`,
`option` computed property.

1. Add a `dataZoom` array, both entries targeting the single axis:
   - `{ type: 'inside', singleAxisIndex: 0, zoomOnMouseWheel: true, moveOnMouseWheel: false }`
   - `{ type: 'slider', singleAxisIndex: 0, bottom: 26, height: 18 }`
   Default window is full (`start: 0`, `end: 100`) — preserves current
   first-paint behavior.

2. Relayout the bottom band so the slider does not overlap the legend
   (chart height is the shared `.dashboard-chart` 240px):
   - `singleAxis.bottom`: `42` → `52`
   - slider: `bottom: 26`, `height: 18`
   - legend: unchanged (`bottom: 0`)

   The themeRiver stream loses ~10px of height; it fits within 240px.

## Behavior / edge cases

- Scatter learning-markers share `singleAxisIndex: 0`, so they zoom with
  the stream automatically — no extra wiring.
- Empty or all-zero data: `option` already returns `null` before the
  `dataZoom` block is reached, so the change is inert there.
- Single bucket: dataZoom is harmless (nothing to narrow).
- Tooltip (`trigger: 'axis'`) and marker click selection are unaffected by
  dataZoom.

## Testing & verification

The change is declarative chart configuration with no new branching
logic, and the component has no existing `option`-builder unit test. No new
unit test is added (consistent with the dashboard test convention, which
extracts *decision* logic to `*.ts` helpers — there is none here).

Verification, from `crates/right-dashboard/frontend`:

1. Targeted: `npm run typecheck` and `npm run test` (existing vitest suite
   stays green).
2. Build: `npm run build`.
3. Manual: run the dashboard, open Overview, confirm on a mobile-width
   viewport (pinch + slider) and on desktop (mouse wheel + slider drag)
   that the time range narrows and the markers track the zoom; dragging the
   slider handles back to the edges restores the full window.

No Rust is touched, so the workspace `cargo test` gate does not apply to
this change; the frontend pipeline above is the equivalent final check.
