# Overview Chart Time-Range Zoom Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add horizontal time-range zoom (pinch/wheel + draggable slider) to the `CostLearningRiver` chart on the Overview view so a dense 30-day window can be narrowed.

**Architecture:** Single-file, presentation-only change. The ECharts `DataZoomComponent` is already registered in `charts.ts`; we only add a `dataZoom` block to the chart `option` (both entries on `singleAxisIndex: 0`) and shift the bottom layout so the slider does not overlap the legend. No backend, no new components, no new echarts module registration.

**Tech Stack:** Vue 3 (`<script setup lang="ts">`), `vue-echarts` + tree-shaken `echarts/core`, Vite, vitest, vue-tsc.

---

### Task 1: Add dataZoom (slider + inside) to CostLearningRiver option

**Files:**
- Modify: `crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue` (the `option` computed, currently lines 107-156)

Context: The returned option object today is:

```ts
return {
  tooltip: { trigger: 'axis', renderMode: 'richText', formatter: formatTooltip },
  legend: { type: 'scroll', bottom: 0 },
  singleAxis: {
    type: 'time',
    top: 16,
    bottom: 42,
    axisLabel: { hideOverlap: true },
  },
  series: [ /* themeRiver + optional scatter markers */ ],
}
```

The `dataZoom` array goes on this same object; the scatter markers use
`coordinateSystem: 'singleAxis'`, so they zoom with the stream for free.

- [ ] **Step 1: Establish a green baseline**

Run (from `crates/right-dashboard/frontend`):

```bash
npm run typecheck && npm run test
```

Expected: typecheck passes, existing vitest suite passes. Record any
pre-existing failure before changing anything; do not attribute it to this
task.

- [ ] **Step 2: Shift the single axis up to make room for the slider**

In `CostLearningRiver.vue`, change the `singleAxis.bottom` value from `42`
to `52`:

```ts
    singleAxis: {
      type: 'time',
      top: 16,
      bottom: 52,
      axisLabel: { hideOverlap: true },
    },
```

- [ ] **Step 3: Add the dataZoom block**

Insert a `dataZoom` array into the returned option object, immediately
after the `singleAxis` block and before `series`:

```ts
    dataZoom: [
      {
        type: 'inside',
        singleAxisIndex: 0,
        zoomOnMouseWheel: true,
        moveOnMouseWheel: false,
        start: 0,
        end: 100,
      },
      {
        type: 'slider',
        singleAxisIndex: 0,
        bottom: 26,
        height: 18,
        start: 0,
        end: 100,
      },
    ],
```

Rationale for each field:
- `singleAxisIndex: 0` — the chart uses `singleAxis` (not `xAxis`); this
  binds zoom to that time axis. Both the themeRiver and the scatter markers
  ride it.
- `inside`: `zoomOnMouseWheel: true` gives desktop wheel-zoom; two-finger
  pinch on touch is the ECharts default. `moveOnMouseWheel: false` keeps
  the wheel as zoom (not pan).
- `slider` `bottom: 26, height: 18` — sits above the `bottom: 0` legend and
  below the axis (now at `bottom: 52`), all inside the shared 240px
  `.dashboard-chart` height.
- `start: 0, end: 100` — first paint shows the full window, matching
  current behavior.

- [ ] **Step 4: Typecheck and run the existing suite**

Run (from `crates/right-dashboard/frontend`):

```bash
npm run typecheck && npm run test
```

Expected: PASS — same result as the Step 1 baseline (no new failures). This
confirms the option object still type-checks and no SSR/component test
regressed.

- [ ] **Step 5: Build the frontend**

Run (from `crates/right-dashboard/frontend`):

```bash
npm run build
```

Expected: `vue-tsc --noEmit` passes and `vite build` completes without
errors.

- [ ] **Step 6: Manual verification**

Run the dashboard and open the Overview view. Confirm:
- Mobile-width viewport: two-finger pinch on the chart narrows the time
  range; the slider band appears below the stream and above the legend; its
  handles drag the window; dragging both handles back to the edges restores
  the full 30-day window.
- Desktop: mouse wheel over the chart zooms the time range; the slider
  drags the window.
- In both cases the orange learning-marker pins move with the stream (they
  stay aligned to their timestamps as you zoom).
- The legend remains visible and is not overlapped by the slider.

If the `inside` one-finger pan on the chart proves disruptive to page
scroll on mobile, the documented fallback is to drop the `inside` entry and
keep only the `slider` entry — but do not do this preemptively.

- [ ] **Step 7: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue
git commit -m "feat(dashboard): add time-range zoom to overview cost/learning chart"
```

---

## Self-Review

- **Spec coverage:** Horizontal-only zoom (slider + inside on
  `singleAxisIndex: 0`) — Task 1 Step 3. Layout relayout (`singleAxis.bottom`
  42→52, slider `bottom:26/height:18`, legend unchanged) — Steps 2-3.
  Default full window (`start:0/end:100`) — Step 3. Markers zoom for free —
  noted, verified Step 6. No new unit test (declarative config) — reflected
  in Steps 4-6 (typecheck/build/manual). Single file, no backend — header
  and Files. All spec points covered.
- **Placeholder scan:** None — every code step shows the exact code and
  exact command with expected output.
- **Type consistency:** Field names (`singleAxisIndex`, `zoomOnMouseWheel`,
  `moveOnMouseWheel`, `start`, `end`, `bottom`, `height`) are ECharts
  dataZoom option keys, used consistently across both entries.
