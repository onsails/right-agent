# Knowledge & Overview Clarity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the dashboard from presenting learning refusals as failures or as cryptic chips, and make the Knowledge → Learning subtab readable (failures honest, refusals muted, lists side-by-side and expandable).

**Architecture:** Backend read-model (`right-dashboard`) gains an honest failed/refused split keyed on `hint_outcome`; the Overview cost river stops emitting learning markers. Frontend drops the Overview marker overlay and rebuilds the Learning subtab layout with two always-visible, expandable panels plus a muted refusals caption.

**Tech Stack:** Rust (edition 2024, `right-dashboard` crate, turso/SQLite read model), Vue 3 + TypeScript, Vitest SSR component tests (`@vue/server-renderer`), ECharts.

**Source spec:** `docs/superpowers/specs/2026-06-04-knowledge-overview-clarity-design.md`

**Execute in a worktree** (project convention — shared checkout churns master). Run one baseline check first:
`devenv shell -- cargo test -p right-dashboard` and `cd crates/right-dashboard/frontend && pnpm vitest run`. Record any pre-existing failures before starting.

---

## File Structure

Backend:
- `crates/right-dashboard/src/read_model/learning.rs` — failed/refused split, counts, samples.
- `crates/right-dashboard/src/read_model/dashboard_overview.rs` — drop learning/curator markers from the Overview river.
- `crates/right-dashboard/src/api_types.rs` — rename `failed_or_aborted_7d`→`failed_7d`; add `refused_7d`, `recent_refused_events`.

Frontend:
- `crates/right-dashboard/frontend/src/types.ts` — mirror the lifecycle field changes.
- `crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue` — remove marker chips + scatter pins + marker tooltip/label code.
- `crates/right-dashboard/frontend/src/views/OverviewView.vue` — remove marker-detail block + select-marker plumbing.
- `crates/right-dashboard/frontend/src/components/FailedSkillList.vue` — panel wrapper, expandable rows, explainer caption.
- `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue` — new layout, non-interactive Failed card, refusals caption.
- Test files alongside each.

---

## Task 1: Backend — honest failed/refused classification in `learning.rs`

**Files:**
- Modify: `crates/right-dashboard/src/read_model/learning.rs`
- Test: same file's `#[cfg(test)] mod tests`

The shared window query currently binds two statuses and never reads
`hint_outcome`, so `aborted`+`refused` no-ops are mislabelled failures.
Generalize the helper to take a static SQL status predicate, then add a
refused query.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `learning.rs`:

```rust
#[tokio::test]
async fn learning_lifecycle_excludes_refusals_from_failed_and_counts_them_separately() {
    let (_dir, conn) = fixture().await;
    // 1 genuine failure, 1 genuine abort (not refused), 2 refusals.
    conn.execute(
        "INSERT INTO skill_learning_events
            (invocation_id, agent_name, action, skill_name, phase, status,
             hint_outcome, message, summary, event_refs_json, created_at)
         VALUES
            ('i1','alpha','update','rightx-a','finish','failed',
             NULL,'boom',NULL,'[]','2026-06-03T09:00:00Z'),
            ('i2','alpha','update','rightx-b','finish','aborted',
             'error','crash',NULL,'[]','2026-06-03T09:05:00Z'),
            ('i3','alpha','update','rightx-c','finish','aborted',
             'refused','already covered',NULL,'[]','2026-06-03T09:10:00Z'),
            ('i4','alpha','update','rightx-c','finish','aborted',
             'refused','already covered','dup','[]','2026-06-03T09:15:00Z')",
        [],
    )
    .await
    .unwrap();

    let now = parse_utc("2026-06-04T10:00:00Z").unwrap();
    let since = now - Duration::days(7);
    let lifecycle = learning_lifecycle(&conn, "alpha", &since, &now)
        .await
        .unwrap();

    assert_eq!(lifecycle.failed_7d, 2);
    assert_eq!(lifecycle.refused_7d, 2);
    assert!(
        lifecycle
            .recent_failed_events
            .iter()
            .all(|e| e.skill_name != "rightx-c"),
        "refusals must not appear in failed list"
    );
    assert!(
        lifecycle
            .recent_refused_events
            .iter()
            .all(|e| e.skill_name == "rightx-c")
    );
}
```

(If `Duration`/`parse_utc` are not already imported in the test module, add `use chrono::Duration;` and `use super::super::parse_utc;` mirroring the existing test imports in the file — check the top of the existing `mod tests` and reuse its import style.)

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-dashboard learning_lifecycle_excludes_refusals`
Expected: FAIL to compile (`failed_7d`, `refused_7d`, `recent_refused_events` do not exist yet) — that compile failure counts as red.

- [ ] **Step 3: Generalize the window helper to a status predicate**

In `learning.rs`, change `learning_events_in_window` to take a static SQL
predicate instead of a 2-status array. Replace its signature and the
`status IN (?4, ?5)` clause:

```rust
async fn learning_events_in_window(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
    status_predicate: &str, // static internal SQL, never user input
    limit: Option<usize>,
) -> Result<(usize, Vec<LearningEventSummary>), ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since_7d, now);
    let sql = format!(
        "SELECT id, skill_name, action, status, message, summary, created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND ({status_predicate})
           AND created_at >= ?2
           AND created_at <= ?3
         ORDER BY created_at DESC, id DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![agent, coarse_since, coarse_until], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .await?;
    // ... body below the query is unchanged (the events Vec build, sort,
    // total, truncate). Keep it exactly as-is.
```

- [ ] **Step 4: Point the three callers at predicates**

`recent_successful_events`:

```rust
learning_events_in_window(
    conn, agent, since_7d, now,
    "status IN ('created','updated')",
    Some(RECENT_EVENT_LIMIT as usize),
)
.await
.map(|(_total, events)| events)
```

(adapt to its current return shape — it returns `Vec`, so keep the
`.map(|(_, events)| events)` or destructure as the existing code does.)

`recent_failed_events`:

```rust
learning_events_in_window(
    conn, agent, since_7d, now,
    "status='failed' OR (status='aborted' AND COALESCE(hint_outcome,'') <> 'refused')",
    Some(super::FAILURE_SAMPLE_LIMIT),
)
.await
```

Add `recent_refused_events`:

```rust
async fn recent_refused_events(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<(usize, Vec<LearningEventSummary>), ReadModelError> {
    learning_events_in_window(
        conn, agent, since_7d, now,
        "status='aborted' AND hint_outcome='refused'",
        Some(super::FAILURE_SAMPLE_LIMIT),
    )
    .await
}
```

- [ ] **Step 5: Wire counts into `learning_lifecycle`**

Replace the body of `learning_lifecycle`:

```rust
let (failed_7d, recent_failed_events) =
    recent_failed_events(conn, agent, since_7d, now).await?;
let (refused_7d, recent_refused_events) =
    recent_refused_events(conn, agent, since_7d, now).await?;
Ok(LearningLifecycle {
    created_7d: writer_status_count_in_window(conn, agent, "created", since_7d, now).await?,
    updated_7d: writer_status_count_in_window(conn, agent, "updated", since_7d, now).await?,
    failed_7d: failed_7d as i64,
    refused_7d: refused_7d as i64,
    recent_successful_events: recent_successful_events(conn, agent, since_7d, now).await?,
    recent_failed_events,
    recent_refused_events,
    candidate_skill_names_7d: candidate_skill_names(conn, agent, since_7d, now).await?,
})
```

This will not compile until Task 2 changes the struct — that is expected;
Task 1 and Task 2 land in one commit. Proceed to Task 2 before building.

---

## Task 2: Backend — API struct rename + new fields

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs` (struct ~355-362 and its serialize test ~930-988)

- [ ] **Step 1: Update the struct**

In `LearningLifecycle`:

```rust
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningLifecycle {
    pub created_7d: i64,
    pub updated_7d: i64,
    pub failed_7d: i64,
    pub refused_7d: i64,
    pub recent_successful_events: Vec<LearningEventSummary>,
    pub recent_failed_events: Vec<LearningEventSummary>,
    pub recent_refused_events: Vec<LearningEventSummary>,
    pub candidate_skill_names_7d: Vec<String>,
}
```

- [ ] **Step 2: Fix the api_types serialize test fixture**

In the `LearningLifecycle { .. }` literal inside this file's tests (~936),
replace `failed_or_aborted_7d: 0,` with `failed_7d: 0,` and add
`refused_7d: 0,` and `recent_refused_events: vec![],`.

- [ ] **Step 3: Build + run the backend tests**

Run: `devenv shell -- cargo test -p right-dashboard learning_lifecycle_excludes_refusals`
Expected: PASS.

- [ ] **Step 4: Run the full crate test to catch other `failed_or_aborted_7d` references**

Run: `devenv shell -- cargo test -p right-dashboard`
Expected: PASS. If any test or call site still names `failed_or_aborted_7d`, update it to `failed_7d` (search: `rg failed_or_aborted_7d crates/right-dashboard/src`).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/src/read_model/learning.rs crates/right-dashboard/src/api_types.rs
git commit -m "feat(dashboard): split learning failures from refusals in read model"
```

---

## Task 3: Backend — drop learning markers from the Overview river

**Files:**
- Modify: `crates/right-dashboard/src/read_model/dashboard_overview.rs`
- Test: same file's `#[cfg(test)] mod tests`

The Overview cost river must stop carrying learning/curator markers (the
chips and pins the user flagged). Cost-spike *signals* stay.

- [ ] **Step 1: Update the failing test expectations**

In `dashboard_overview_builds_signal_timeline_and_cost_river`, replace the
final marker assertion:

```rust
assert!(
    response.cost_learning_river.markers.is_empty(),
    "overview river must not carry learning markers"
);
```

In `dashboard_overview_projects_refused_learning_outcomes`, delete the
`response.cost_learning_river.markers.iter().any(...)` assertion block and
replace with:

```rust
assert!(response.cost_learning_river.markers.is_empty());
```

In `dashboard_overview_projects_curator_state_and_warns_on_malformed_evidence`,
delete the `markers.iter().any(|marker| marker.kind == "cost_spike" ...)`
assertion (the cost-spike *signal* assertion just above it stays).

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo test -p right-dashboard dashboard_overview_builds_signal_timeline`
Expected: FAIL (markers still populated).

- [ ] **Step 3: Stop assembling markers**

In `cost_learning_river()`, remove the `markers` computation and return an
empty vec:

```rust
let series = build_cost_series(&points);
Ok((
    CostLearningRiver {
        window: RIVER_WINDOW.to_owned(),
        points,
        series,
        markers: Vec::new(),
    },
    Vec::new(),
))
```

Delete the now-unused `learning_markers` function.

In `dashboard_overview()`, remove the line
`cost_learning_river.markers.extend(curator_markers);` and stop binding
`curator_markers` (replace the destructure with `let (curator_signals, _curator_markers, curator_warnings) = curator_projection(...)`; or change `curator_projection` to stop returning markers — minimal is to ignore them). In `curator_projection`, remove the `markers.push(LearningMarker { .. })` block (curator cost-spike marker); keep the cost-spike *signal* push. Leave the `CuratorProjection` tuple's marker slot returning `Vec::new()` if you keep the signature, or simplify the signature to drop it. Verify with `rg LearningMarker crates/right-dashboard/src` that no populated path remains.

- [ ] **Step 4: Run to verify it passes**

Run: `devenv shell -- cargo test -p right-dashboard dashboard_overview`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/src/read_model/dashboard_overview.rs
git commit -m "feat(dashboard): drop learning markers from Overview cost river"
```

---

## Task 4: Frontend — mirror lifecycle types

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts` (`LearningLifecycle` ~388-396)

- [ ] **Step 1: Update the interface**

```ts
export interface LearningLifecycle {
  created_7d: number
  updated_7d: number
  failed_7d: number
  refused_7d: number
  recent_successful_events: LearningEventSummary[]
  recent_failed_events: LearningEventSummary[]
  recent_refused_events: LearningEventSummary[]
  candidate_skill_names_7d: string[]
}
```

- [ ] **Step 2: Type-check**

Run: `cd crates/right-dashboard/frontend && pnpm vue-tsc --noEmit`
Expected: errors only in `ReportsView.vue` / its test referencing
`failed_or_aborted_7d` (fixed in Task 6). No other crate-wide breakage.

(No standalone commit — lands with Task 6.)

---

## Task 5: Frontend — strip the Overview marker overlay

**Files:**
- Modify: `crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue`
- Modify: `crates/right-dashboard/frontend/src/views/OverviewView.vue`

- [ ] **Step 1: Simplify `CostLearningRiver.vue`**

Remove the marker overlay entirely. The component keeps only the
theme-river. Replace the whole file with:

```vue
<script setup lang="ts">
import { computed } from 'vue'
import AsyncVChart from './AsyncVChart.vue'
import { money } from '../../format'
import type { CostLearningRiver } from '../../types'

type ThemeRiverDatum = [bucket: string, costUsd: number, source: string]

interface TooltipDatum {
  data?: unknown
  seriesName?: string
}

const props = defineProps<{
  river: CostLearningRiver | null
}>()

function isTooltipDatum(value: unknown): value is TooltipDatum {
  return typeof value === 'object' && value !== null
}

function isThemeRiverDatum(value: unknown): value is ThemeRiverDatum {
  return Array.isArray(value) &&
    typeof value[0] === 'string' &&
    typeof value[1] === 'number' &&
    typeof value[2] === 'string'
}

function formatTooltip(params: unknown): string {
  const rows = Array.isArray(params) ? params : [params]
  return rows.map((row) => {
    if (!isTooltipDatum(row)) {
      return ''
    }
    if (isThemeRiverDatum(row.data)) {
      return `${row.data[2] || row.seriesName || 'source'}: ${money(row.data[1])}`
    }
    return ''
  }).filter(Boolean).join('\n')
}

const option = computed(() => {
  const river = props.river
  if (!river || river.points.length === 0) {
    return null
  }

  const data: ThemeRiverDatum[] = river.points.flatMap((point) =>
    (point.sources ?? []).map((source): ThemeRiverDatum => [point.bucket, source.cost_usd, source.source]),
  )

  if (data.length === 0) {
    return null
  }

  return {
    tooltip: {
      trigger: 'axis',
      renderMode: 'richText',
      formatter: formatTooltip,
    },
    legend: {
      type: 'scroll',
      bottom: 0,
    },
    singleAxis: {
      type: 'time',
      top: 16,
      bottom: 42,
      axisLabel: { hideOverlap: true },
    },
    series: [
      {
        type: 'themeRiver',
        emphasis: { focus: 'series' },
        data,
      },
    ],
  }
})
</script>

<template>
  <section class="panel chart-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Cost and learning</p>
        <h2>{{ river?.window ?? 'last_30_days' }}</h2>
      </div>
    </header>

    <div v-if="!option" class="chart-empty">No cost data</div>
    <AsyncVChart
      v-else
      class="dashboard-chart"
      :option="option"
      autoresize
    />
  </section>
</template>
```

- [ ] **Step 2: Remove marker plumbing from `OverviewView.vue`**

In `OverviewView.vue`:
- Delete the `<template v-if="selectedMarker"> ... </template>` block (the
  marker-detail header + `dl.meta-grid`).
- Remove `@select-marker="selectMarker"` from `<CostLearningRiver>` (it
  becomes `<CostLearningRiver :river="overview?.cost_learning_river ?? null" />`).
- Delete the script bits: `selectedMarkerId`, `selectedMarker`,
  `markerCostLabel`, `selectMarker`, and the `selectedMarkerId.value = null`
  line inside `selectSignal`. Remove now-unused imports `LearningMarker`
  and `shortDate` **only if** no longer referenced (signal detail still
  uses `shortDate` via `SignalTimeline`, but `OverviewView` itself may no
  longer reference it — check and drop only if unused).

- [ ] **Step 3: Type-check + run Overview tests**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run src/views/OverviewView.test.ts`
Expected: PASS (the test passes `cost_learning_river: null`, so no marker
assertions break). If `pnpm vue-tsc --noEmit` flags an unused import in
`OverviewView.vue`, remove it.

- [ ] **Step 4: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue crates/right-dashboard/frontend/src/views/OverviewView.vue crates/right-dashboard/frontend/src/types.ts
git commit -m "feat(dashboard): remove learning-marker overlay from Overview chart"
```

---

## Task 6: Frontend — Learning subtab layout, expandable failures, refusals caption

**Files:**
- Modify: `crates/right-dashboard/frontend/src/components/FailedSkillList.vue`
- Modify: `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue`
- Test: `crates/right-dashboard/frontend/src/views/learning/ReportsView.test.ts` (rewrite)

### 6a — FailedSkillList becomes an expandable panel

- [ ] **Step 1: Rewrite `FailedSkillList.vue`**

```vue
<script setup lang="ts">
import { computed, ref } from 'vue'

import { shortDate } from '../format'
import type { LearningEventSummary } from '../types'
import StatusPill from './StatusPill.vue'
import { failureSampleLabel } from './failureSampleLabel'

const props = defineProps<{
  events: LearningEventSummary[]
  total: number
}>()

const sampleLabel = computed(() => failureSampleLabel(props.total, props.events.length))
const selectedKey = ref<string | null>(null)

function keyFor(event: LearningEventSummary): string {
  return `${event.skill_name}:${event.created_at}`
}

function toggle(key: string): void {
  selectedKey.value = selectedKey.value === key ? null : key
}
</script>

<template>
  <aside class="panel detail-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Failed skills</p>
        <h2>Recent failures</h2>
      </div>
    </header>
    <p class="muted-line explainer">
      A failed skill is a learning attempt that errored out. It is not a refusal
      — a refusal means the skill already covered the request.
    </p>
    <p v-if="events.length === 0" class="muted-line">No failures</p>
    <p v-if="sampleLabel" class="muted-line">{{ sampleLabel }}</p>
    <div v-if="events.length > 0" class="row-list">
      <template v-for="event in events" :key="keyFor(event)">
        <button
          type="button"
          class="data-row"
          :class="{ selected: selectedKey === keyFor(event) }"
          :aria-expanded="selectedKey === keyFor(event)"
          @click="toggle(keyFor(event))"
        >
          <span class="row-main">
            <strong>{{ event.skill_name }}</strong>
            <small>{{ event.action }} / {{ shortDate(event.created_at) }}</small>
            <small v-if="event.message" class="run-note-preview">{{ event.message }}</small>
          </span>
          <span class="row-side">
            <StatusPill :status="event.status" />
          </span>
        </button>
        <dl v-if="selectedKey === keyFor(event)" class="meta-grid compact signal-detail">
          <div><dt>When</dt><dd>{{ shortDate(event.created_at) }}</dd></div>
          <div><dt>Action</dt><dd>{{ event.action }}</dd></div>
          <div><dt>Status</dt><dd>{{ event.status }}</dd></div>
          <div v-if="event.message" class="signal-detail-text"><dt>Message</dt><dd>{{ event.message }}</dd></div>
          <div v-if="event.summary" class="signal-detail-text"><dt>Summary</dt><dd>{{ event.summary }}</dd></div>
        </dl>
      </template>
    </div>
  </aside>
</template>

<style scoped>
.explainer {
  margin-bottom: 8px;
}
.signal-detail {
  padding: 8px 12px 14px;
  background: var(--tg-theme-secondary-bg-color, rgba(127, 127, 127, 0.1));
  border-radius: 0 0 10px 10px;
  margin-bottom: 8px;
}
.signal-detail-text {
  grid-column: 1 / -1;
}
</style>
```

### 6b — ReportsView layout

- [ ] **Step 2: Rewrite the ReportsView test (red first)**

Replace `ReportsView.test.ts` with tests matching the new layout: Failed
card is always a non-interactive `article`; the failed list and refusals
caption render inline (no CollapsibleSection / count-badge gating).

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import ReportsView from './ReportsView.vue'

function learning(over: Record<string, unknown> = {}) {
  return {
    agent: 'a',
    generated_at: '2026-05-31T12:00:00Z',
    flow_nodes: [],
    flow_edges: [],
    recent_learning_signals: [],
    warnings: [],
    lifecycle: {
      created_7d: 0,
      updated_7d: 0,
      failed_7d: 0,
      refused_7d: 0,
      recent_successful_events: [],
      recent_failed_events: [],
      recent_refused_events: [],
      candidate_skill_names_7d: [],
      ...over,
    },
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(ReportsView, props as any),
  })
  return renderToString(app)
}

describe('ReportsView learning layout', () => {
  it('renders Failed skills panel always visible and non-interactive Failed card', async () => {
    const html = await render({ learning: learning() })
    expect(html).toContain('Failed skills')
    expect(html).toContain('A failed skill is a learning attempt that errored out')
    expect(html).not.toContain('metric-card-interactive')
    expect(html).not.toContain('count-badge')
  })

  it('lists failed events and a refusals caption', async () => {
    const html = await render({
      learning: learning({
        failed_7d: 1,
        refused_7d: 5,
        recent_failed_events: [{
          skill_name: 'rightx-a', action: 'update', status: 'failed',
          message: 'boom', summary: null, created_at: '2026-05-31T11:00:00Z',
        }],
      }),
    })
    expect(html).toContain('rightx-a')
    expect(html).toContain('Refused 5')
  })

  it('hides the refusals caption when there are none', async () => {
    const html = await render({ learning: learning({ refused_7d: 0 }) })
    expect(html).not.toContain('Refused 0')
  })
})
```

- [ ] **Step 3: Run to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run src/views/learning/ReportsView.test.ts`
Expected: FAIL (old ReportsView still uses CollapsibleSection / `failed_or_aborted_7d`).

- [ ] **Step 4: Rewrite `ReportsView.vue`**

```vue
<script setup lang="ts">
import { computed } from 'vue'

import LearningFlowChart from '../../components/charts/LearningFlowChart.vue'
import LearningSignalPanel from '../../components/charts/LearningSignalPanel.vue'
import FailedSkillList from '../../components/FailedSkillList.vue'
import MetricCard from '../../components/MetricCard.vue'
import { failureMetric } from '../../components/failureMetric'
import type { LearningOverviewResponse } from '../../types'

const props = defineProps<{
  learning: LearningOverviewResponse | null
}>()

const failures = computed(() => failureMetric(props.learning?.lifecycle.failed_7d ?? 0))
const refusedCount = computed(() => props.learning?.lifecycle.refused_7d ?? 0)
</script>

<template>
  <section v-if="learning?.warnings.length" class="notice">
    <strong>Partial data</strong>
    <span v-for="warning in learning.warnings" :key="`${warning.source}:${warning.kind}:${warning.message}`">
      {{ warning.message }}
    </span>
  </section>

  <section class="chart-panel-wrap">
    <LearningFlowChart
      :nodes="learning?.flow_nodes ?? []"
      :edges="learning?.flow_edges ?? []"
    />
  </section>

  <section class="metric-grid">
    <MetricCard label="Created 7d" :value="learning?.lifecycle.created_7d ?? 0" tone="ok" />
    <MetricCard label="Updated 7d" :value="learning?.lifecycle.updated_7d ?? 0" tone="active" />
    <MetricCard label="Failed 7d" :value="learning?.lifecycle.failed_7d ?? 0" :tone="failures.tone" />
  </section>

  <section class="two-column">
    <LearningSignalPanel :signals="learning?.recent_learning_signals ?? []" />
    <FailedSkillList
      :events="learning?.lifecycle.recent_failed_events ?? []"
      :total="learning?.lifecycle.failed_7d ?? 0"
    />
  </section>

  <p v-if="refusedCount > 0" class="muted-line refusals-caption">
    Refused {{ refusedCount }} — the skill already covered the request; nothing changed.
  </p>
</template>

<style scoped>
.chart-panel-wrap {
  margin-bottom: 10px;
}
.refusals-caption {
  margin-top: 8px;
}
</style>
```

Note: `failureMetric.tone` is still used for the card colour; the
`interactive` field is simply not read anymore. `LearningSignalPanel` and
`FailedSkillList` are both `aside.panel.detail-panel`, so the existing
`.two-column` grid (`1fr | 0.85fr`) renders them side by side. If both
panels read better at equal width, change the section class to a custom
`learning-lists` grid with `grid-template-columns: repeat(2, minmax(0,1fr))`
in `App.vue` — optional polish, confirm visually.

- [ ] **Step 5: Run to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run src/views/learning/ReportsView.test.ts`
Expected: PASS.

- [ ] **Step 6: Full frontend test + type-check**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run && pnpm vue-tsc --noEmit`
Expected: PASS, no type errors. Fix any remaining `failed_or_aborted_7d`
references (`rg failed_or_aborted_7d crates/right-dashboard/frontend/src`).

- [ ] **Step 7: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/FailedSkillList.vue crates/right-dashboard/frontend/src/views/learning/ReportsView.vue crates/right-dashboard/frontend/src/views/learning/ReportsView.test.ts
git commit -m "feat(dashboard): side-by-side expandable learning signals + failed skills with refusals caption"
```

---

## Task 7: Final verification

- [ ] **Step 1: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS (record any pre-existing unrelated failures noted at baseline).

- [ ] **Step 2: Full frontend suite + production type-check**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run && pnpm build`
Expected: PASS, build succeeds.

- [ ] **Step 3: Manual smoke against `him` (if a live dashboard is available)**

- Overview chart: no chips, no pins under the cost river.
- Knowledge → Learning: "Failed 7d" shows `0`; a "Refused 5 — …" caption
  appears below the two panels; Learning signals and Failed skills sit
  side by side; clicking a failed skill (once one exists) expands detail.

---

## Self-Review

**Spec coverage:**
- Overview marker removal → Task 3 (backend) + Task 5 (frontend). ✓
- Honest failed/refused split → Task 1 + Task 2. ✓
- Refusals as muted line → Task 6b (caption, gated on `refused_7d > 0`). ✓
- Side-by-side always-visible lists → Task 6b. ✓
- Failed rows expandable → Task 6a. ✓
- "Failed skill" explainer → Task 6a (explainer caption). ✓

**Open visual polish (non-blocking):** equal-width vs 1fr/0.85fr for the
two learning panels — decided visually in Task 6b Step 4.

**Type consistency:** `failed_7d`, `refused_7d`, `recent_refused_events`
used identically across `api_types.rs` (Task 2), `types.ts` (Task 4),
`learning.rs` assembly (Task 1), and both Vue views/tests (Task 6).
`learning_events_in_window(status_predicate: &str)` signature matches all
three callers (Task 1).
