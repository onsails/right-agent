# Dashboard Usage tab: cache-per-kind breakdown

- **Date:** 2026-05-31
- **Status:** Approved (brainstorming) — ready for plan
- **Area:** `right-dashboard` (frontend + read-model)

## Problem

The Telegram `/usage` command used to return a text summary that rendered
cache usage **per invocation kind** (💬 Interactive / ⏰ Cron / 🧠 Reflection),
per time window: a `Cache: H% hit rate` line and, in detail mode, raw
`cache-created` / `cache-read` token counts. That renderer is
`crates/right-agent/src/usage/format.rs` (`format_summary_message`,
`render_source`, `format_cache_line`).

`/usage` was later rewired to open the dashboard:
`handle_usage` (`crates/bot/src/telegram/handler.rs:1042`) now delegates to
`handle_dashboard`. Nothing on the live path calls `format_summary_message`
anymore. The dashboard **Usage tab** it opens shows cache only as a single
**per-day total** (`UsageBreakdown.vue:42`), and its per-source / per-model
rows show **cost only** — so the per-kind cache view disappeared from the
user's point of view.

This is not a deleted dashboard widget; the per-kind cache view was orphaned
by the routing change. We restore it **on the dashboard** (where `/usage`
now lands).

## Goal

Show, for each invocation kind (source), the cache token usage on the
dashboard Usage tab:

- `cache_creation_tokens` ("created") and `cache_read_tokens` ("read"),
  formatted compactly (k / M);
- a **hit-rate %** = `cache_read / (input + cache_creation + cache_read)`,
  identical to the old `format_cache_line` definition.

Rendered in **both** surfaces of the tab, consistently:

1. **Window panels** (`UsageView.vue`) — Today / Last 7 days / Last 30 days /
   All time. The per-source rows there iterate `UsageSourceSummary`, which
   already carries the token fields → **frontend-only**.
2. **Day-breakdown panel** (`UsageBreakdown.vue`) — the Sources section. Its
   rows iterate `UsageSourcePoint`, which currently lacks token fields →
   needs a **small backend addition** (below).

The dashboard's source set already includes `reflection` and the three
`learning_*` kinds, so those gain cache rows the old text never had — a free
improvement, no extra work.

## Non-goals

- **No `$ saved` / pricing.** Explicitly cut: it is a counterfactual estimate
  built from a hand-maintained price table (`right-agent/.../pricing.rs`) and
  the old calc was *gross* (it ignored the 1.25× / 2× cache-write premium).
  Decision was on accuracy grounds, not effort. If revisited later, do the
  *net* variant (subtract the 5-minute write premium on `cache_creation`).
- **No per-model cache rows.** "Per kind" means per source. Per-model is out
  of scope (avoids scope creep), even though `UsageModelSummary` has the data.
- **No source-name prettifying** (💬/⏰ emoji labels). Keep raw source strings,
  matching the rest of the current dashboard.
- **Do not remove `format_summary_message`.** It appears orphaned after the
  `/usage`→dashboard rewire (only its own tests call it), but it predates this
  change. Flag it for a separate cleanup; do not delete here.

## Design

### Hit-rate: one shared helper

Add a pure helper, unit-tested directly (dashboard convention: pure decision
logic lives in a `*.ts` helper).

```ts
// frontend/src/components/charts/usageCache.ts
export function cacheHitRate(s: {
  input_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
}): number {
  const bearing = s.input_tokens + s.cache_creation_tokens + s.cache_read_tokens
  return bearing === 0 ? 0 : s.cache_read_tokens / bearing
}
```

Returns a 0–1 ratio; render via the existing `percent()` in `format.ts`.
Structural typing lets the same helper serve `UsageSourceSummary` (window
panels) and `UsageSourcePoint` (day panel) once both carry the three fields.

### Compact token formatter

`format.ts` has `money` / `percent` / `bytes` but no compact integer
formatter. Add one, mirroring `format.rs::format_count`:

```ts
// frontend/src/format.ts
export function compactCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`
  return `${n}`
}
```

### UI shape

Each source row gains a muted subline, shown **only when
`cache_read_tokens > 0`** (matches the old "omit when no cache reads"):

```
interactive                       $4.21
  1.2M created · 8.4M read · 83% hit
cron                              $0.90
  120k created · 1.1M read · 90% hit
```

- `UsageView.vue` window panels: wrap the existing `source → cost` row so a
  conditional `<p class="muted-line">` subline can sit beneath it.
- `UsageBreakdown.vue` Sources section: same subline beneath each
  `source → cost` row.
- `UsageBreakdown.vue` Counters section: the existing per-day cache line
  (`{create} create / {read} read`) gains `· {percent(cacheHitRate(point))} hit`
  for consistency (`UsageDailyPoint` already has the three day-total fields).

Use existing classes (`muted-line`, `model-row`); no new AsyncState/empty
primitives are needed (these are rows inside already-resolved panels).

### Backend: add token fields to `UsageSourcePoint`

`UsageSourcePoint` (`crates/right-dashboard/src/api_types.rs:211`) currently:
`source, cost_usd, subscription_cost_usd, api_cost_usd, turns, invocations`.

Add three `u64` fields: `input_tokens`, `cache_creation_tokens`,
`cache_read_tokens`.

Populate them in `build_daily_series`
(`crates/right-dashboard/src/read_model/usage.rs`): the per-row values are
already destructured in the loop (`input_tokens`, `cache_creation_tokens`,
`cache_read_tokens` ≈ usage.rs:241-244), and the `UsageSourcePoint` is built
at ~usage.rs:250-257. Accumulate the three fields into `source_entry` there
(initialise to 0 in the `or_insert_with`).

**`UsageSourcePoint` is reused by two structs** —
`UsageDailyPoint.sources` (api_types.rs:165) and
`CostLearningPoint.sources` (api_types.rs:206). Adding fields is
backward-compatible, but **every constructor must set them**:

- `read_model/usage.rs` `build_daily_series` (the real populate site, ~250).
- `read_model/dashboard_overview.rs:367` builds `CostLearningPoint.sources`
  for the cost-learning river. Its query selects no token columns (only
  `ts, source, cost, turns, api_key_source`) and the river does not render
  cache, so set the three new fields to `0` there.
- Test fixtures at api_types.rs:764 and 827.

Mirror the three new fields in the TS `UsageSourcePoint` interface
(`frontend/src/types.ts:258`).

## Files touched

**Backend (`right-dashboard`)**
- `src/api_types.rs` — 3 fields on `UsageSourcePoint`; fix fixtures at 764, 827.
- `src/read_model/usage.rs` — accumulate the 3 fields in `build_daily_series`.
- `src/read_model/dashboard_overview.rs` — set the 3 new fields to `0` in the
  cost-learning-river `UsageSourcePoint` (~line 367).

**Frontend (`right-dashboard/frontend`)**
- `src/types.ts` — 3 fields on `UsageSourcePoint`.
- `src/format.ts` — `compactCount`.
- `src/components/charts/usageCache.ts` *(new)* — `cacheHitRate`.
- `src/views/UsageView.vue` — per-source cache subline in window panels.
- `src/components/charts/UsageBreakdown.vue` — per-source cache subline in
  Sources; hit-rate on the per-day Counters cache line.

## Testing

Follow the dashboard convention: pure logic unit-tested, components via Vue
SSR `renderToString`.

- **`usageCache` unit** (`usageCache.test.ts`): zero-bearing → `0`; the
  `input 10 / create 50 / read 300` case → `300/360 ≈ 0.833`; a high-hit case.
- **`compactCount` unit**: `42 → "42"`, `1_234 → "1.2k"`,
  `1_234_567 → "1.2M"`.
- **`UsageView` SSR** (`UsageView.test.ts`, new): a window with a source where
  `cache_read_tokens > 0` renders the subline (contains "read" and "%"); a
  source with `cache_read_tokens === 0` renders no subline.
- **`UsageBreakdown` SSR**: selected day with cache reads renders per-source
  sublines and the per-day hit-rate; the existing tests still pass.
- **Backend**: `cargo test -p right-dashboard` — the read-model tests in
  `usage.rs` already assert per-source daily aggregation; extend one to assert
  the new token fields are summed per source.

### Verification cadence

- During dev: run the **targeted** suite for the layer being changed —
  `pnpm test` (vitest) in `crates/right-dashboard/frontend` for FE, and
  `devenv shell -- cargo test -p right-dashboard` for the backend field
  addition. TDD: write the failing helper/SSR test first.
- Final, mandatory: `devenv shell -- cargo test --workspace`. The dashboard
  bundle is built by `build.rs`, so the workspace build also proves the
  frontend compiles. Targeted tests do not substitute for this.

## Risks / notes

- Adding fields to `UsageSourcePoint` ripples to `CostLearningPoint`; the
  compiler flags every unset literal, so this is mechanically safe. The
  cost-learning river (dashboard_overview.rs) has no token columns in its
  query and does not render cache — `0` there is correct, not a placeholder.
- Hit-rate uses source-level totals (`input_tokens`, `cache_creation_tokens`,
  `cache_read_tokens`), not a re-derivation from `per_model`. These are summed
  from the same rows, so the denominator matches what the old text used.
- Compact formatting (`k`/`M`) can collide visually with the day panel's
  existing full-grouped counter for the per-day total. Acceptable: the per-day
  total stays as-is; only the new per-source sublines use compact form.
