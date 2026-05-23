# Dashboard Visual Analytics Design

## Goal

Redesign the Telegram Mini App dashboard so cost, learning, and operational
signals are primarily understood through visual timelines and charts instead of
text-heavy tables.

The first screen should answer:

```text
What changed, what cost money, and what did the agent learn?
```

Detail tabs should then explain the answer without making tables the primary
interface.

## Current System Context

`right-dashboard` owns dashboard DTOs, Telegram Mini App auth helpers, SQLite
read models, Vue/Vite frontend source, and checked-in static assets.
`right-bot::telegram::dashboard` mounts routes, enforces Telegram auth and the
allowlist, checks the agent path, owns bot token custody, and provides
bot-owned runtime reads.

The current v2 dashboard already has top-level tabs:

```text
Overview | Activity | Knowledge | Usage | Identity | Health
```

The current frontend is already componentized, but the Usage and Knowledge
views still rely heavily on table/list presentation:

- `UsageView.vue` renders source rows and model rows from `/api/v1/usage`.
- `KnowledgeView.vue` delegates to Episodes, Reports, and Skills subviews.
- `ReportsView.vue` shows a few metrics, then a report list and detail panel.
- `EpisodesView.vue` shows an episode list and detail panel.

The backend has usable raw ingredients:

- `usage_events` for cost, source, model, API/subscription split, token counts,
  web counters, and timestamps.
- `skill_learning_events` for learning start/finish outcomes.
- `curator_state` for curator trigger and circuit evidence.
- `learning_episodes` and `skill_review_reports` for legacy/report detail.
- `async_runs`, cron specs, and current dashboard activity projections for run
  status and failures.
- injected runtime health summaries for overview health state.

## Selected Approach

Add an explicit visual read-model contract.

This means new dashboard DTOs and read-model projections are allowed where
existing payloads are too table-shaped. The frontend should not infer major
chart semantics from unrelated fields when the backend can project UI-ready
series and signal rows directly.

This approach is better than an overview-only visual layer because Usage and
Knowledge would remain table-heavy. It is better than a full analytics redesign
because this slice remains read-only, local to the dashboard, and avoids
chart-driven navigation, write actions, or speculative anomaly engines.

## Scope

In scope:

- Timeline-first `Overview`.
- Signal-led overview timeline.
- Cost/learning river as the overview secondary visual.
- Redesigned `Usage` tab with spend-over-time as the primary chart.
- Redesigned `Knowledge` learning detail with a learning-flow chart as the
  primary visual.
- New read-only dashboard DTOs and read-model fields to support those visuals.
- ECharts through `echarts` and `vue-echarts`.
- Hover tooltips and local chart selection that updates adjacent details.
- Existing tables/lists retained as secondary inspection and fallback surfaces.

Out of scope:

- Any write operation.
- Budget editing or alert configuration.
- Skill approval, retry, edit, archive, delete, install, or update actions.
- Chart-driven route navigation into runs, reports, or skills.
- Live WebSocket/SSE streaming.
- Multi-agent analytics.
- New dashboard process.
- Full statistical anomaly detection beyond simple threshold/relative signal
  projection from existing data.

## Product Shape

### Overview

`Overview` becomes timeline-first.

The primary view is a signal-led timeline. It shows notable changes rather than
raw run inventory:

- cost spike or high-spend interval;
- learned skill created, patched, refused, failed, or aborted;
- curator trigger, skip, or circuit state when visible from persisted state;
- foreground, cron, or background failure;
- health or sandbox warning from injected runtime summaries;
- significant active run state.

The secondary visual is a cost/learning river. It shows spend over time by
source and overlays learning markers so the user can connect learning activity
to spend changes.

Compact summary chips remain above or beside the timeline:

- active work;
- recent failures;
- today's cost;
- learning activity;
- health/sandbox state.

### Usage

`Usage` becomes spend-over-time first.

The primary chart is a stacked daily cost series by source. The selected day or
window updates adjacent detail panels:

- source breakdown;
- per-model spend;
- API/subscription split;
- token mix;
- web request counters.

The existing source/model rows remain available as a compact detail section,
not the first visual surface.

### Knowledge

`Knowledge / Learning` becomes learning-flow first.

The primary visual shows:

- foreground turn signal count;
- prefilter decisions (`skip`, `create`, `patch`);
- probe-writer outcomes (`applied_as_hinted`, `applied_differently`,
  `refused`, failed/aborted);
- curator triggers;
- resulting skill creates and patches.

Skill lifecycle remains a supporting panel:

- active/stale/archived counts;
- recently used skills;
- created-by provenance where available.

Episodes, Reports, and Skills remain available for inspection, but the first
screen should explain the learning pipeline before asking users to inspect raw
rows.

## Backend Contract

All routes remain under:

```text
/dashboard/<agent>/api/v1
```

### Overview

Extend `GET /overview` with visual fields:

```text
signals: Vec<DashboardSignal>
cost_learning_river: CostLearningRiver
warnings: Vec<DashboardDataWarning>
```

`DashboardSignal`:

```text
id: String
kind: cost_spike | learning_outcome | curator | run_failure | health | active_work
severity: info | warn | bad
occurred_at: String
title: String
detail: Option<String>
source: Option<String>
cost_usd: Option<f64>
related_run_id: Option<String>
related_skill_name: Option<String>
related_report_id: Option<i64>
```

Signals are ordered newest first, capped for the mobile dashboard, and stable
enough for Vue `key` usage. The overview returns at most 30 signals.

`CostLearningRiver`:

```text
window: String
points: Vec<CostLearningPoint>
series: Vec<CostLearningSeries>
markers: Vec<LearningMarker>
```

`CostLearningPoint` contains a timestamp bucket and cost by source.
`LearningMarker` contains timestamp, marker kind, label, severity, optional
skill name, and optional cost/source association.

The default river window is the last 30 calendar days. Cost-spike signals are
projected when a non-zero daily bucket is at least twice the median non-zero
daily cost in that same 30-day window, or when persisted curator spike evidence
already identifies a learning-specific spike.

### Usage

Extend `GET /usage` with chart-ready series:

```text
daily_series: Vec<UsageDailyPoint>
source_series: Vec<UsageSourceSeries>
selected_window: String
warnings: Vec<DashboardDataWarning>
```

`UsageDailyPoint`:

```text
date: String
total_cost_usd: f64
subscription_cost_usd: f64
api_cost_usd: f64
turns: u64
invocations: u64
input_tokens: u64
output_tokens: u64
cache_creation_tokens: u64
cache_read_tokens: u64
web_search_requests: u64
web_fetch_requests: u64
sources: Vec<UsageSourcePoint>
models: Vec<UsageModelSummary>
```

Use the existing Usage windows for summary totals. The default chart window is
last 30 days; today, last 7 days, last 30 days, and all time summaries remain
available.

### Knowledge Learning

Extend `GET /knowledge/learning/overview` with flow fields:

```text
flow_nodes: Vec<LearningFlowNode>
flow_edges: Vec<LearningFlowEdge>
recent_learning_signals: Vec<LearningSignalPoint>
warnings: Vec<DashboardDataWarning>
```

`LearningFlowNode`:

```text
id: String
label: String
kind: signal | prefilter | writer | curator | skill
count: i64
severity: info | warn | bad
```

`LearningFlowEdge`:

```text
source: String
target: String
count: i64
```

The flow is a projection, not a perfect causal graph. Edges represent observed
pipeline transitions over the selected window when the data can support them.
When source data cannot support a transition, the read model omits that edge and
adds a warning instead of inventing a count.

The default learning-flow window is last 7 days, matching the existing lifecycle
summary window. `recent_learning_signals` is capped at 30 rows.

`DashboardDataWarning`:

```text
source: String
kind: partial_data | malformed_json | unavailable
message: String
```

## Data Rules

The dashboard must not fake zeros for missing optional sources. It should
differentiate:

- valid zero count;
- missing source table or missing optional row;
- malformed optional JSON;
- source unavailable because the runtime summary has not been fetched.

Malformed `model_usage_json` or curator evidence JSON is skipped with a logged
warning and a visible partial-data warning when it affects a chart.

Overview health must not run doctor implicitly. It continues to use injected
runtime state.

## Frontend Architecture

Add `echarts` and `vue-echarts` to the dashboard frontend.

Use on-demand ECharts imports to keep the bundle bounded:

- `CanvasRenderer`;
- `BarChart`;
- `LineChart`;
- `ThemeRiverChart`;
- `SankeyChart` for the learning flow;
- `GridComponent`;
- `TooltipComponent`;
- `LegendComponent`;
- `DatasetComponent`;
- `DataZoomComponent`;
- `GraphicComponent`.

Use `VChart` with `autoresize` for mobile and Telegram viewport changes.

Add focused frontend components:

```text
components/charts/SignalTimeline.vue
components/charts/CostLearningRiver.vue
components/charts/UsageSpendChart.vue
components/charts/UsageBreakdown.vue
components/charts/LearningFlowChart.vue
components/charts/LearningSignalPanel.vue
```

Charts support:

- hover tooltips;
- selecting one point, marker, node, or edge to update adjacent details;
- empty states with stable height;
- partial-data notices;
- no direct chart-to-route navigation.

The existing list/detail components remain, but the top of each redesigned view
should be chart-led.

## Error Handling

- API query failures return existing dashboard JSON errors and drive the current
  stale/offline/locked UI states.
- Missing optional signal data returns partial payload warnings, not fabricated
  empty data.
- Empty chart payloads render explicit empty states.
- Chart components preserve stable dimensions during loading, empty, partial,
  and error states.
- Tooltip text must be backend-derived or formatted from typed DTO fields; do
  not parse display strings for meaning.

## Testing And Verification

Rust read-model tests:

- signal timeline ordering and cap behavior;
- cost/learning river bucketing by source;
- usage daily series aggregation;
- API/subscription split aggregation;
- learning-flow node/edge counts;
- malformed JSON handling for model usage and curator evidence;
- learning source list sync with `right_agent::usage::LEARNING_SOURCES`.

Dashboard API handler tests:

- authorized overview returns visual fields;
- authorized usage returns daily series;
- authorized learning overview returns flow fields;
- auth rejection remains unchanged.

Frontend verification:

- TypeScript DTO updates compile.
- Dashboard frontend build passes.
- Chart components render stable empty, loading, and populated states.

Final verification for implementation remains:

```text
devenv shell -- cargo test --workspace
```

The implementation plan should use targeted Rust/frontend checks while
iterating, then run the mandatory full workspace test at the end.
