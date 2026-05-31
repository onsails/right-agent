# Live dashboard — design

Status: design (approved)
Date: 2026-05-31
Scope owner: andrey

## Goal

Make the dashboard update on its own: the operator never reloads to see
new data. Make "live" the default for any future view, delivered through
one reusable Vue primitive instead of per-view hand-wiring.

## Problem

`App.vue` is the data owner for every tab and hand-wires polling:

- A single `setInterval` (`schedulePolling`) refreshes only `overview` and
  `activity`, and only while one of those tabs is active.
- `usage`, `knowledge`, `identity` load once on tab open, then go stale
  until the operator reloads the whole Mini App.
- `health` is manual by design (expensive OpenShell gRPC; ARCHITECTURE.md
  forbids running doctor/sandbox implicitly).
- `mcp` / `providers` self-fetch once.

Two consequences: most tabs need a reload to show new data, and a new view
is **dead by default** — a developer must add another branch to
`schedulePolling`. The polling/stale/lock machinery (`guarded`,
`applyErrorState`, `pollTimer`) is tangled into `App.vue`, which is already
a god-component.

A recent batch of commits (AsyncState, CollapsibleSection, RunFailureList,
the `*View.test.ts` SSR tests) deliberately made every view **dumb**: data
in via props, interactions out via emits, rendered and asserted through
`@vue/server-renderer`. This investment is load-bearing and must be
preserved.

## Decision

Introduce a polling composable `useLiveResource` as the single "live"
primitive, and adopt a **smart-container / dumb-view split per tab**. A
thin container owns the data (calls `useLiveResource`, owns on-demand
detail fetches and selection state) and passes `data` / `loading` / `error`
down to the existing dumb view as the exact props it already expects. The
dumb views and their SSR tests are unchanged.

Why containers and not composable-calls-in-`App.vue`: containers keep
`App.vue` from being the growth point every new feature must edit (a shared
hot file that generates conflicts across the project's parallel
checkouts/sessions). Each tab becomes self-contained — its container, dumb
view, and tests sit together, and a container is mounted only when its tab
is active, so `v-if` unmount scopes polling to the active tab for free (no
explicit "enabled" gating needed).

## In scope

- New composables: `useLiveResource`, `liveStatus`, `liveConfig`,
  `useRunDetail`.
- New per-tab containers wrapping the existing dumb views.
- Slim `App.vue` to: bootstrap load, tab routing, display mode, and feeding
  the connection pill from `liveStatus`.
- Tabs made live: overview, activity, usage, knowledge (learning + skills),
  identity.
- Health routed through the same primitive but kept manual (no auto-load,
  no poll).

## Out of scope

- **Backend.** No new endpoints, no SSE/WebSocket. Polling hits the
  existing `api/v1/*` GET routes; auth stays the per-request
  `Authorization: tma <initData>` header.
- **MCP / Providers views.** Already self-fetch; not regressed and not
  migrated here. Optional follow-up: route their fetch through
  `useLiveResource` for out-of-band polling (e.g. OAuth completion).
- **Dumb view markup and tests.** Unchanged.

## Architecture

### `useLiveResource(fetcher, opts)` — `composables/useLiveResource.ts`

The single live primitive. Generalizes the hand-rolled
`onMounted → refresh` + `disposed`-guard pattern already in `McpView.vue`,
adding interval polling, visibility pause, and connection reporting.

```ts
interface LiveResourceOptions {
  intervalMs?: number       // 0 = no polling (manual/on-demand); default from liveConfig
  immediate?: boolean       // default true — fetch on mount
  pauseWhenHidden?: boolean // default true — skip ticks while the Mini App is hidden
  reportConnection?: boolean// default true — feed liveStatus
  key?: string              // liveStatus registry key + debug label
}

interface LiveResource<T> {
  data: Ref<T | null>
  error: Ref<string | null>      // set ONLY when data === null (initial load failed)
  loading: Ref<boolean>
  lastUpdatedAt: Ref<string | null>
  refresh: () => Promise<void>   // manual/forced refresh
}

function useLiveResource<T>(fetcher: () => Promise<T>, opts?: LiveResourceOptions): LiveResource<T>
```

Behavior:

- **Mount:** if `immediate`, run `refresh()`. Start the interval when
  `intervalMs > 0`. Register with `liveStatus` under `key`.
- **Tick:** call `refresh()` unless (`pauseWhenHidden && document.hidden`)
  or a fetch is already in flight (overlap guard — ticks never stack).
- **Visibility:** on `hidden → visible`, run an immediate catch-up
  `refresh()` and resume ticking. While hidden, ticks no-op.
- **Race safety:** a `disposed` flag plus a monotonic generation counter
  (as in `McpView`) discard any response that resolves after unmount or
  after a newer `refresh()`.
- **Unmount:** set `disposed`, clear the interval, remove the visibility
  listener, deregister from `liveStatus`.

**error vs stale rule** (so `AsyncState`, priority `error > content`, keeps
showing data through a transient poll failure):

- `error.value` is set only while `data.value === null` — i.e. the first
  load failed and there is nothing to show.
- When `data.value !== null` and a refresh fails, `error` stays null, the
  last-good data stays rendered, and the failure surfaces only in the
  global pill as `stale`/`offline`. This reproduces today's
  `applyErrorState` behavior, now automatic and per-resource.

### `liveStatus` — `composables/liveStatus.ts`

Module-level reactive registry feeding the one connection pill in
`AppShell`. Each live resource registers on mount and reports its latest
**settled** state on each fetch outcome; background refreshes do not report
transient `loading`, so the pill never flickers every interval.

- `ConnectionState = 'loading' | 'live' | 'stale' | 'offline' | 'locked'`
  (moved here from `App.vue`).
- Outcome → state: success → `live`; 401/403 (`DashboardApiError.isLocked`)
  → `locked`; network/status-0 → `offline` (no data) or `stale` (had data);
  other error → `stale` (had data) or `offline`.
- `reduceConnectionState(states[])` — pure, priority
  `locked > offline > stale > loading > live` ("show the worst"; lock is
  global, so one 401 dominates). Empty registry falls back to the last
  non-empty global state (**sticky**) so the pill does not blank on a
  manual tab (health) where no resource is polling.
- `globalLastUpdatedAt` = most recent successful settle, shown as
  "updated Ns ago". Honest decay replaces today's always-on heartbeat —
  see Decisions.

`reduceConnectionState` and the outcome→state classifier are pure and
unit-tested directly.

### `liveConfig` — `composables/liveConfig.ts`

`provide`/`inject` of the server-configured interval so containers need not
prop-drill it. `App.vue` calls `provideLiveConfig({ intervalMs })` from
`bootstrap.refresh_interval_secs`; `useLiveResource` does
`inject(LiveConfigKey, DEFAULT_INTERVAL_MS)`. A per-call `intervalMs`
option overrides (identity uses a long interval; health uses 0).

### Run detail — no shared composable

`RunFailureList.vue` already self-contains failure-run detail: it imports
`runDetail`, owns `selectedId`/`detail`/`loading`/`error`, and is
race-guarded. Both Overview and Activity render it for their failure cards,
and it needs no container involvement — **left unchanged**.

The only App-driven run selection is ActivityView's **main** cron run list
(`crons[].recent_runs`), wired today through `App.vue`'s `selectedRun` /
`selectRun` / `runDetail`. That has exactly one consumer, so it moves
**inline into `ActivityContainer`** (no shared composable — YAGNI). The
reconcile-on-poll check is extracted as a pure helper
`activityContainsRun(activity, runId)` in `views/activitySelection.ts` and
unit-tested; the container calls it in a `watch(activity.data, …)` to clear
the selection when the polled list drops the selected run.

### Container / dumb-view split

| Tab | Container (new, smart) | Dumb view (unchanged) | Container owns |
|---|---|---|---|
| Overview | `OverviewContainer.vue` | `OverviewView.vue` | `useLiveResource(dashboardOverview)` + `useLiveResource(activityOverview)` (no run selection — failures use self-contained `RunFailureList`) |
| Activity | `ActivityContainer.vue` | `ActivityView.vue` | `useLiveResource(activityOverview)`; main-list run selection + `runDetail` inline; `activityContainsRun` reconcile in a `watch` |
| Usage | `UsageContainer.vue` | `UsageView.vue` | `useLiveResource(usageOverview)` |
| Knowledge | `KnowledgeContainer.vue` (replaces dumb `KnowledgeView.vue`) | `ReportsView.vue`, `SkillsView.vue` | subtab nav + routes to the two sub-containers below |
| · Learning | `ReportsContainer.vue` | `ReportsView.vue` | `useLiveResource(learningOverview)` |
| · Skills | `SkillsContainer.vue` | `SkillsView.vue` | `useLiveResource(skillsOverview)`, skill selection + detail + pin |
| Identity | `IdentityContainer.vue` | `IdentityView.vue` | `useLiveResource(identityFiles, { intervalMs: 30000 })`, file selection + detail |
| Health | `HealthContainer.vue` | `HealthView.vue` | `useLiveResource(doctorStatus, {immediate:false,intervalMs:0,reportConnection:false})` + same for sandbox; buttons call `refresh()` |

`KnowledgeView.vue` (the current dumb orchestrator: subtab nav + routing)
has no test; it becomes `KnowledgeContainer.vue`. The dumb leaves
`ReportsView.vue` / `SkillsView.vue` and their tests stay. Knowledge
sub-containers under `v-if` of the subtab give the same unmount-scopes-
polling property at the subtab level.

### `App.vue` after

Owns only: one `useLiveResource(bootstrap, { intervalMs: 0, key: 'bootstrap' })`
(load-once; reports lock/offline to the pill), `provideLiveConfig`,
`activeTab`, display mode + Telegram init, and the `AppShell` pill props
read from `liveStatus`. Renders `<XContainer v-if="activeTab === '…'">`.

Removed from `App.vue`: every `*Data` ref, `schedulePolling`, `pollTimer`,
`guarded`, `applyErrorState`, all `refresh*` wrappers, all `select*` and
detail-loading refs, and all data prop-drilling. `AppShell.vue` is
unchanged — it still receives `connection-state` / `message` /
`last-updated-at` as props, now sourced from `liveStatus`.

### Per-resource cadence

| Resource | Interval | Notes |
|---|---|---|
| bootstrap | once | static-ish; agent/features/interval |
| dashboardOverview, activityOverview, usageOverview, learningOverview, skillsOverview | default = `bootstrap.refresh_interval_secs` (server-configured, falls back to 5s) | cheap SQL aggregations |
| identityFiles | 30s | near-static (agent-owned files) |
| doctorStatus, sandboxStats | manual (0) | expensive gRPC; button-triggered |
| run/skill/identity detail | on demand | parameterized fetch on click, not polled |

## Testing

Follows the project convention (`McpView`): pure logic in `.ts`, unit-
tested directly; SSR `renderToString` for dumb views; thin glue untested.

- **Unit (pure):** `reduceConnectionState` + outcome→state classifier
  (`liveStatus.test.ts`); `activityContainsRun`
  (`activitySelection.test.ts`); any extracted tick/visibility predicate.
- **SSR (unchanged):** existing `*View.test.ts` keep passing — the dumb
  views' prop contracts are untouched.
- **Glue (untested):** the `useLiveResource` wiring (timers, listeners,
  `onMounted`) and the containers — same status as `McpView`'s `onMounted`.

## File inventory

**New**

- `composables/useLiveResource.ts`
- `composables/liveStatus.ts` (+ `ConnectionState`, `reduceConnectionState`)
- `composables/liveConfig.ts`
- `views/activitySelection.ts` (pure `activityContainsRun`)
- `composables/liveStatus.test.ts`, `views/activitySelection.test.ts`
- `views/OverviewContainer.vue`, `views/ActivityContainer.vue`,
  `views/UsageContainer.vue`, `views/IdentityContainer.vue`,
  `views/HealthContainer.vue`, `views/KnowledgeContainer.vue`,
  `views/SkillsContainer.vue`, `views/learning/ReportsContainer.vue`

**Modified**

- `App.vue` — slimmed to routing + bootstrap + pill.
- `types.ts` — `ConnectionState` moves out (re-exported if needed).

**Removed/renamed**

- `views/KnowledgeView.vue` → `views/KnowledgeContainer.vue` (now smart).

**Unchanged**

- `components/AppShell.vue`, `components/RunFailureList.vue` (already
  self-contained), and all dumb `*View.vue` + their tests.

## Verification cadence

- **Intermediate (targeted):** `pnpm -C crates/right-dashboard/frontend test`
  (vitest) and `pnpm -C crates/right-dashboard/frontend typecheck`
  (`vue-tsc`) after each container slice; run the new pure-logic tests
  red→green first (TDD).
- **Final (mandatory):** `devenv shell -- cargo test --workspace` (per
  AGENTS.md) — this also drives the frontend build via
  `right-dashboard/build.rs` (`vite build`), proving the SPA compiles.

## Migration sequencing

1. Primitives first, TDD: `liveStatus` (+ pure reduce/classifier),
   `liveConfig`, `useLiveResource`, `useRunDetail` (+ pure
   `reconcileSelection`).
2. One container at a time, simplest first (Usage → Identity → Overview/
   Activity via `useRunDetail` → Knowledge split → Health). After each,
   delete the now-dead App.vue state for that tab and run targeted tests.
3. Final App.vue slim-down once all tabs are containerized.
4. Full workspace test.

## Decisions (flippable at spec review)

- **No dedicated heartbeat.** The pill reflects the last settled outcome
  across tabs plus an "updated Ns ago" timestamp (sticky). This drops
  today's always-on `overview` poll that kept the pill green on every tab.
  Rationale: honest, and avoids a double-fetch wart on the default tab.
  Flip = add one always-on `useLiveResource(dashboardOverview, {reportConnection:true})`
  in `App.vue` whose data is ignored.
- **MCP/Providers stay as-is** (already self-fetch). Migrating them to the
  composable for out-of-band polling is a follow-up.

## Alternatives considered

- **SSE / WebSocket push.** True real-time, but requires threading a
  broadcast bus through worker/cron/usage/learning in the bot, plus
  query-param or fetch-stream auth (EventSource can't send the `tma`
  header). Backend surface and security cost outweigh a few seconds of
  latency for a single-operator Mini App. Rejected; the polling composable
  leaves a clean seam to add invalidation signals later.
- **Self-fetching dumb views.** Would break the new `*View.test.ts` SSR
  tests (which pass props) and move fetching into `onMounted`, which SSR
  can't exercise — a testability regression. Rejected.
- **Composable calls kept in `App.vue` (no containers).** Lower churn now,
  but `App.vue` stays the per-tab growth point and shared hot file.
  Rejected for maintainability.

## Future-proofing: adding a new live tab

1. Write a dumb `FooView.vue` (props in, emits out) + its SSR test.
2. Write a thin `FooContainer.vue`:
   `const foo = useLiveResource(fooOverview)` → `<FooView :data="foo.data" :loading="foo.loading" :error="foo.error" />`.
3. Register the tab and add `<FooContainer v-if="activeTab === 'foo'" />`
   to `App.vue`.

Live polling, visibility pause, stale handling, and the connection pill
come for free from the primitive — no `App.vue` polling logic to touch.
