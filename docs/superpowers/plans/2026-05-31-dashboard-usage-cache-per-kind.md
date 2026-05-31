# Dashboard Usage Cache-Per-Kind Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore per-kind (per-source) cache visibility on the dashboard Usage tab — `created`/`read` token counts plus a cache hit-rate % — in both the window panels (`UsageView.vue`) and the day-breakdown Sources section (`UsageBreakdown.vue`).

**Architecture:** Frontend-first. A pure `cacheHitRate` helper and a `compactCount` formatter feed a small presentational `CacheSubline.vue` reused by both usage surfaces. The window panels already receive per-source cache tokens (`UsageSourceSummary`); the day-breakdown panel needs three token fields added to `UsageSourcePoint` (Rust struct + TS mirror, populated in `build_daily_series`). No pricing, no `$ saved`.

**Tech Stack:** Rust (`right-dashboard` read-model + `api_types`), Vue 3 + TypeScript (dashboard frontend), vitest + `@vue/server-renderer` for SSR component tests.

**Spec:** `docs/superpowers/specs/2026-05-31-dashboard-usage-cache-per-kind-design.md`

**Branch:** `master` (per user instruction — commit directly).

---

## Conventions for every task

- **Frontend tests** run from the frontend directory:
  `cd crates/right-dashboard/frontend && pnpm exec vitest run <file>`
- **Frontend typecheck:** `cd crates/right-dashboard/frontend && pnpm run typecheck`
- **Backend tests:** `devenv shell -- cargo test -p right-dashboard <filter>`
- **`devenv.nix` exists at repo root**, so all `cargo` commands are prefixed with `devenv shell --`.
- Hit-rate definition (matches the old `format_cache_line`): `cache_read / (input + cache_creation + cache_read)`, `0` when the denominator is `0`.

---

## File Structure

**Frontend (`crates/right-dashboard/frontend/src/`)**
- `format.ts` — add `compactCount(n)` (k/M formatter).
- `components/charts/usageCache.ts` *(new)* — pure `cacheHitRate(tokens)`.
- `components/charts/CacheSubline.vue` *(new)* — presentational subline; renders nothing when `cache_read_tokens === 0`.
- `views/UsageView.vue` — wrap each window-panel source row, drop in `CacheSubline`.
- `components/charts/UsageBreakdown.vue` — wrap each day-panel Sources row, drop in `CacheSubline`; add hit-rate to the per-day Counters cache line.
- `types.ts` — mirror the three new fields onto `UsageSourcePoint`.

**Backend (`crates/right-dashboard/src/`)**
- `api_types.rs` — three `u64` fields on `UsageSourcePoint`; fix two test fixtures.
- `read_model/usage.rs` — accumulate the three fields in `build_daily_series`; new aggregation test.
- `read_model/dashboard_overview.rs` — set the three fields to `0` in the cost-learning-river constructor.

---

### Task 1: `cacheHitRate` pure helper

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/charts/usageCache.ts`
- Test: `crates/right-dashboard/frontend/src/components/charts/usageCache.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/charts/usageCache.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { cacheHitRate } from './usageCache'

describe('cacheHitRate', () => {
  it('returns 0 when there are no input-bearing tokens', () => {
    expect(cacheHitRate({ input_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0 })).toBe(0)
  })
  it('computes reads over (input + creation + read)', () => {
    // 300 / (10 + 50 + 300) = 300/360 ≈ 0.8333 → 83% via percent()
    expect(cacheHitRate({ input_tokens: 10, cache_creation_tokens: 50, cache_read_tokens: 300 })).toBeCloseTo(0.8333, 4)
  })
  it('approaches 1 when reads dominate', () => {
    expect(cacheHitRate({ input_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 500 })).toBe(1)
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/usageCache.test.ts`
Expected: FAIL — cannot resolve `./usageCache` / `cacheHitRate is not a function`.

- [ ] **Step 3: Write minimal implementation**

Create `crates/right-dashboard/frontend/src/components/charts/usageCache.ts`:

```ts
export interface CacheTokens {
  input_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
}

// Hit-rate = cache reads over all input-bearing tokens. Matches the old
// Telegram /usage format_cache_line definition (right-agent usage/format.rs).
export function cacheHitRate(t: CacheTokens): number {
  const bearing = t.input_tokens + t.cache_creation_tokens + t.cache_read_tokens
  return bearing === 0 ? 0 : t.cache_read_tokens / bearing
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/usageCache.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/charts/usageCache.ts crates/right-dashboard/frontend/src/components/charts/usageCache.test.ts
git commit -m "feat(dashboard): add cacheHitRate helper"
```

---

### Task 2: `compactCount` formatter

**Files:**
- Modify: `crates/right-dashboard/frontend/src/format.ts` (append a function)
- Test: `crates/right-dashboard/frontend/src/format.test.ts` *(new)*

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/format.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { compactCount } from './format'

describe('compactCount', () => {
  it('passes small integers through unchanged', () => {
    expect(compactCount(0)).toBe('0')
    expect(compactCount(42)).toBe('42')
    expect(compactCount(999)).toBe('999')
  })
  it('uses a k suffix for thousands', () => {
    expect(compactCount(1_000)).toBe('1.0k')
    expect(compactCount(1_234)).toBe('1.2k')
  })
  it('uses an M suffix for millions', () => {
    expect(compactCount(1_234_567)).toBe('1.2M')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run src/format.test.ts`
Expected: FAIL — `compactCount` is not exported.

- [ ] **Step 3: Write minimal implementation**

Append to `crates/right-dashboard/frontend/src/format.ts`:

```ts
export function compactCount(n: number): string {
  if (n >= 1_000_000) {
    return `${(n / 1_000_000).toFixed(1)}M`
  }
  if (n >= 1_000) {
    return `${(n / 1_000).toFixed(1)}k`
  }
  return `${n}`
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run src/format.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/format.ts crates/right-dashboard/frontend/src/format.test.ts
git commit -m "feat(dashboard): add compactCount token formatter"
```

---

### Task 3: `CacheSubline.vue` presentational component

Renders a muted one-line cache summary, or nothing when there were no cache reads (mirrors the old "omit when no reads"). Reused by both usage surfaces.

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/charts/CacheSubline.vue`
- Test: `crates/right-dashboard/frontend/src/components/charts/CacheSubline.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/charts/CacheSubline.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import CacheSubline from './CacheSubline.vue'

async function render(tokens: Record<string, number>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(CacheSubline, { tokens } as any),
  })
  return renderToString(app)
}

describe('CacheSubline', () => {
  it('renders created/read/hit when there are cache reads', async () => {
    const html = await render({ input_tokens: 10, cache_creation_tokens: 50, cache_read_tokens: 300 })
    expect(html).toContain('created')
    expect(html).toContain('read')
    expect(html).toContain('83%')
  })
  it('renders nothing when there are no cache reads', async () => {
    const html = await render({ input_tokens: 10, cache_creation_tokens: 0, cache_read_tokens: 0 })
    expect(html).not.toContain('created')
    expect(html).not.toContain('read')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/CacheSubline.test.ts`
Expected: FAIL — cannot resolve `./CacheSubline.vue`.

- [ ] **Step 3: Write minimal implementation**

Create `crates/right-dashboard/frontend/src/components/charts/CacheSubline.vue`:

```vue
<script setup lang="ts">
import { compactCount, percent } from '../../format'
import { cacheHitRate, type CacheTokens } from './usageCache'

defineProps<{
  tokens: CacheTokens
}>()
</script>

<template>
  <p v-if="tokens.cache_read_tokens > 0" class="muted-line cache-subline">
    {{ compactCount(tokens.cache_creation_tokens) }} created ·
    {{ compactCount(tokens.cache_read_tokens) }} read ·
    {{ percent(cacheHitRate(tokens)) }} hit
  </p>
</template>

<style scoped>
.cache-subline {
  font-size: 0.72rem;
  margin: 2px 0 0;
  padding-left: 8px;
}
</style>
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/CacheSubline.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/charts/CacheSubline.vue crates/right-dashboard/frontend/src/components/charts/CacheSubline.test.ts
git commit -m "feat(dashboard): add CacheSubline component"
```

---

### Task 4: Window-panel cache sublines (`UsageView.vue`)

The window panels iterate `window.sources` (`UsageSourceSummary`), which already carries `input_tokens`/`cache_creation_tokens`/`cache_read_tokens` — no backend or `types.ts` change needed here.

> **⚠️ DRIFT CORRECTION (discovered during execution, 2026-05-31).** Since this
> plan was written, commit `28436a19` ("show per-skill spend + usage cache/skip
> columns") landed two changes that make the steps below stale:
>
> 1. `UsageView.vue` already renders a **raw, unlabeled** per-source cache span:
>    `<span>{{ source.cache_read_tokens }} / {{ source.cache_creation_tokens }}</span>`
>    inside the `.model-row`. The labeled `CacheSubline` **replaces** this raw
>    span (do not leave both — that would double-render cache).
> 2. `UsageView.test.ts` **already exists** (it is NOT a new file). It has a
>    `budget_skip_count` suite (leave untouched) and a "cache tokens per source"
>    suite asserting the raw `1234` / `567` values. Replacing the raw span with
>    `CacheSubline` turns `1234` into `1.2k` (via `compactCount`), so that
>    existing assertion **will break** and must be rewritten to expect the
>    labeled `created · read · hit` output. Reuse the existing
>    `sourceSummaryStub`/`windowStub`/`usageStub`/`render` helpers — do not
>    duplicate them.
>
> Revised TDD order: (1) rewrite the existing "cache tokens per source" suite to
> expect the `CacheSubline` output and add an omit-when-no-reads case → run, it
> fails (still raw span); (2) add the `CacheSubline` import and replace the raw
> span with the `.usage-source` wrapper → run, it passes; (3) typecheck; (4)
> commit. The verbatim `ts`/`html` snippets in the steps below describe the
> intended component shape but predate the existing test file — follow the
> revised order, not the literal "create new file" wording.

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.vue`
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.test.ts` *(already exists — rewrite the cache suite, keep `budget_skip_count`)*

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/views/UsageView.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import UsageView from './UsageView.vue'
import type { UsageOverviewResponse, UsageSourceSummary } from '../types'

function source(over: Partial<UsageSourceSummary>): UsageSourceSummary {
  return {
    source: 'interactive',
    cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
    turns: 1, invocations: 1, input_tokens: 10, output_tokens: 20,
    cache_creation_tokens: 0, cache_read_tokens: 0,
    web_search_requests: 0, web_fetch_requests: 0, per_model: [],
    ...over,
  }
}

function usage(sources: UsageSourceSummary[]): UsageOverviewResponse {
  return {
    agent: 'agent-b', generated_at: '2026-05-31T00:00:00Z',
    windows: [{
      key: 'today', label: 'Today', sources,
      total_cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
      turns: 1, invocations: 1, input_tokens: 10, output_tokens: 20,
      cache_creation_tokens: 0, cache_read_tokens: 0,
      web_search_requests: 0, web_fetch_requests: 0, per_model: [],
    }],
    selected_window: 'today', daily_series: [], source_series: [], warnings: [],
  }
}

async function render(u: UsageOverviewResponse) {
  const app = createSSRApp({
    render: () => h(UsageView, { usage: u, loading: false, error: null }),
  })
  return renderToString(app)
}

describe('UsageView window panels', () => {
  it('shows a cache subline for a source with cache reads', async () => {
    const html = await render(usage([source({ cache_creation_tokens: 50, cache_read_tokens: 300 })]))
    expect(html).toContain('created')
    expect(html).toContain('83%')
  })
  it('omits the subline for a source with no cache reads', async () => {
    const html = await render(usage([source({ cache_creation_tokens: 0, cache_read_tokens: 0 })]))
    expect(html).not.toContain('created')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run src/views/UsageView.test.ts`
Expected: FAIL — output contains no `created`/`83%` (subline not rendered yet).

- [ ] **Step 3: Add the import**

In `crates/right-dashboard/frontend/src/views/UsageView.vue`, add to the `<script setup>` imports (after the `UsageSpendChart` import):

```ts
import CacheSubline from '../components/charts/CacheSubline.vue'
```

- [ ] **Step 4: Wrap the source rows and drop in `CacheSubline`**

In `UsageView.vue`, replace this block:

```html
        <div class="model-grid">
          <div v-for="source in windowRows(window)" :key="source.source" class="model-row">
            <span>{{ source.source }}</span>
            <strong>{{ money(source.cost_usd) }}</strong>
          </div>
        </div>
```

with:

```html
        <div class="model-grid">
          <div v-for="source in windowRows(window)" :key="source.source" class="usage-source">
            <div class="model-row">
              <span>{{ source.source }}</span>
              <strong>{{ money(source.cost_usd) }}</strong>
            </div>
            <CacheSubline :tokens="source" />
          </div>
        </div>
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run src/views/UsageView.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 6: Typecheck**

Run: `cd crates/right-dashboard/frontend && pnpm run typecheck`
Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/UsageView.vue crates/right-dashboard/frontend/src/views/UsageView.test.ts
git commit -m "feat(dashboard): render cache per kind in usage window panels"
```

---

### Task 5: Backend — per-source cache tokens on `UsageSourcePoint`

Add `input_tokens`, `cache_creation_tokens`, `cache_read_tokens` to `UsageSourcePoint` and populate them in the daily series. The compiler will flag every constructor; all are updated in this task so the crate stays green.

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs:211` (struct) + fixtures at `:764` and `:827`
- Modify: `crates/right-dashboard/src/read_model/usage.rs` (`build_daily_series` + new test)
- Modify: `crates/right-dashboard/src/read_model/dashboard_overview.rs:367` (zero the new fields)

- [ ] **Step 1: Write the failing test**

In `crates/right-dashboard/src/read_model/usage.rs`, inside the `#[cfg(test)] mod tests`, add a new test (the existing `insert_usage` helper writes `input_tokens=10, cache_creation_tokens=5, cache_read_tokens=40` per event):

```rust
    #[tokio::test]
    async fn usage_overview_aggregates_cache_tokens_per_source_in_daily_series() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_usage(&conn, "2026-05-20T08:00:00Z", "interactive", 0.10, "sonnet").await;
        insert_usage(&conn, "2026-05-20T09:00:00Z", "interactive", 0.10, "sonnet").await;

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-20T12:00:00Z".to_owned(),
            },
        )
        .await
        .unwrap();

        let day = response
            .daily_series
            .iter()
            .find(|point| point.date == "2026-05-20")
            .unwrap();
        let interactive = day
            .sources
            .iter()
            .find(|source| source.source == "interactive")
            .unwrap();
        // Two events × (input 10, cache_creation 5, cache_read 40).
        assert_eq!(interactive.input_tokens, 20);
        assert_eq!(interactive.cache_creation_tokens, 10);
        assert_eq!(interactive.cache_read_tokens, 80);
    }
```

- [ ] **Step 2: Run test to verify it fails (compile error)**

Run: `devenv shell -- cargo test -p right-dashboard usage_overview_aggregates_cache_tokens_per_source_in_daily_series`
Expected: FAIL to compile — `no field input_tokens on type UsageSourcePoint`.

- [ ] **Step 3: Add the three fields to the struct**

In `crates/right-dashboard/src/api_types.rs`, change `UsageSourcePoint` (currently lines 211-218):

```rust
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageSourcePoint {
    pub source: String,
    pub cost_usd: f64,
    pub subscription_cost_usd: f64,
    pub api_cost_usd: f64,
    pub turns: u64,
    pub invocations: u64,
    pub input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}
```

- [ ] **Step 4: Populate them in `build_daily_series`**

In `crates/right-dashboard/src/read_model/usage.rs`, in the `source_totals.entry(...).or_insert_with(...)` literal (currently ~lines 250-257), add the three fields initialised to `0`:

```rust
            .or_insert_with(|| UsageSourcePoint {
                source: source.clone(),
                cost_usd: 0.0,
                subscription_cost_usd: 0.0,
                api_cost_usd: 0.0,
                turns: 0,
                invocations: 0,
                input_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            });
```

Then, immediately after the existing `source_entry.invocations += 1;` line, add the accumulation (these locals are already destructured from the row earlier in the loop):

```rust
        source_entry.input_tokens += input_tokens.max(0) as u64;
        source_entry.cache_creation_tokens += cache_creation_tokens.max(0) as u64;
        source_entry.cache_read_tokens += cache_read_tokens.max(0) as u64;
```

- [ ] **Step 5: Zero the new fields in the cost-learning river**

In `crates/right-dashboard/src/read_model/dashboard_overview.rs`, in the `source_totals.entry(...).or_insert_with(|| UsageSourcePoint { ... })` literal (~line 367; its query selects no token columns and the river does not render cache), add:

```rust
            .or_insert_with(|| UsageSourcePoint {
                source: source.clone(),
                cost_usd: 0.0,
                subscription_cost_usd: 0.0,
                api_cost_usd: 0.0,
                turns: 0,
                invocations: 0,
                input_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            });
```

- [ ] **Step 6: Fix the two serialization fixtures**

In `crates/right-dashboard/src/api_types.rs`, both `UsageSourcePoint` literals in the test module need the new fields. At ~line 764 (inside `cost_learning_river`) and ~line 827 (inside `daily_series`), add to each literal (use values matching the surrounding fixture so existing assertions still hold):

```rust
                        // ...existing fields (source, cost_usd, subscription_cost_usd,
                        // api_cost_usd, turns, invocations) unchanged...
                        input_tokens: 10,
                        cache_creation_tokens: 5,
                        cache_read_tokens: 40,
```

(Both fixtures already use a single `interactive` source with `turns`/`invocations` set; the token values above are arbitrary non-zero numbers — no existing assertion in those two tests inspects token fields, so any values compile and keep the tests green.)

- [ ] **Step 7: Run the new test and the package suite**

Run: `devenv shell -- cargo test -p right-dashboard`
Expected: PASS — including the new `usage_overview_aggregates_cache_tokens_per_source_in_daily_series` and all pre-existing `usage.rs` / `api_types.rs` tests.

- [ ] **Step 8: Commit**

```bash
git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model/usage.rs crates/right-dashboard/src/read_model/dashboard_overview.rs
git commit -m "feat(dashboard): aggregate per-source cache tokens in daily series"
```

---

### Task 6: Day-panel cache sublines + per-day hit-rate (`UsageBreakdown.vue`)

Mirror the three fields in TS `UsageSourcePoint`, render a `CacheSubline` under each day-panel Sources row, and append a hit-rate to the existing per-day Counters cache line.

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts:258` (`UsageSourcePoint` interface)
- Modify: `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue`
- Test: `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts` *(new)*

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import UsageBreakdown from './UsageBreakdown.vue'
import type { UsageDailyPoint } from '../../types'

function point(over: Partial<UsageDailyPoint> = {}): UsageDailyPoint {
  return {
    date: '2026-05-31', total_cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
    turns: 1, invocations: 1, input_tokens: 10, output_tokens: 20,
    cache_creation_tokens: 50, cache_read_tokens: 300,
    web_search_requests: 0, web_fetch_requests: 0,
    sources: [{
      source: 'interactive', cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
      turns: 1, invocations: 1, input_tokens: 10, cache_creation_tokens: 50, cache_read_tokens: 300,
    }],
    models: [],
    ...over,
  }
}

async function render(p: UsageDailyPoint | null) {
  const app = createSSRApp({ render: () => h(UsageBreakdown, { point: p }) })
  return renderToString(app)
}

describe('UsageBreakdown cache', () => {
  it('renders a per-source cache subline and a per-day hit-rate', async () => {
    const html = await render(point())
    expect(html).toContain('created')
    expect(html).toContain('83%')
    expect(html).toContain('hit')
  })
  it('omits the per-source subline when that source has no reads', async () => {
    const html = await render(point({
      sources: [{
        source: 'cron', cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
        turns: 1, invocations: 1, input_tokens: 10, cache_creation_tokens: 0, cache_read_tokens: 0,
      }],
    }))
    expect(html).not.toContain('created')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/UsageBreakdown.test.ts`
Expected: FAIL — `created`/`83%` not present (and/or TS error on the source token fields).

- [ ] **Step 3: Mirror the fields in `types.ts`**

In `crates/right-dashboard/frontend/src/types.ts`, change the `UsageSourcePoint` interface (lines 258-265) to:

```ts
export interface UsageSourcePoint {
  source: string
  cost_usd: number
  subscription_cost_usd: number
  api_cost_usd: number
  turns: number
  invocations: number
  input_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
}
```

- [ ] **Step 4: Update `UsageBreakdown.vue`**

Add to the `<script setup>` imports (it already imports `money` from `'../../format'`):

```ts
import { money, percent } from '../../format'
import { cacheHitRate } from './usageCache'
import CacheSubline from './CacheSubline.vue'
```

(Replace the existing `import { money } from '../../format'` line with the `money, percent` form above; keep the existing `UsageDailyPoint` type import.)

In the Counters section, replace the Cache row:

```html
          <div class="model-row">
            <span>Cache</span>
            <strong>{{ count(point.cache_creation_tokens) }} create / {{ count(point.cache_read_tokens) }} read</strong>
          </div>
```

with (append the hit-rate, guarded on reads > 0):

```html
          <div class="model-row">
            <span>Cache</span>
            <strong>
              {{ count(point.cache_creation_tokens) }} create / {{ count(point.cache_read_tokens) }} read<template v-if="point.cache_read_tokens > 0"> · {{ percent(cacheHitRate(point)) }} hit</template>
            </strong>
          </div>
```

In the Sources section, replace:

```html
        <div class="row-list">
          <div v-for="source in (point.sources ?? [])" :key="source.source" class="model-row">
            <span>{{ source.source }}</span>
            <strong>{{ money(source.cost_usd) }}</strong>
          </div>
          <p v-if="(point.sources ?? []).length === 0" class="muted-line">No source spend</p>
        </div>
```

with:

```html
        <div class="row-list">
          <div v-for="source in (point.sources ?? [])" :key="source.source" class="usage-source">
            <div class="model-row">
              <span>{{ source.source }}</span>
              <strong>{{ money(source.cost_usd) }}</strong>
            </div>
            <CacheSubline :tokens="source" />
          </div>
          <p v-if="(point.sources ?? []).length === 0" class="muted-line">No source spend</p>
        </div>
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/UsageBreakdown.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 6: Typecheck**

Run: `cd crates/right-dashboard/frontend && pnpm run typecheck`
Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts
git commit -m "feat(dashboard): render cache per kind in usage day breakdown"
```

---

### Task 7: Full verification

- [ ] **Step 1: Run the entire frontend test suite**

Run: `cd crates/right-dashboard/frontend && pnpm exec vitest run`
Expected: PASS — all suites green (new cache tests + all pre-existing).

- [ ] **Step 2: Frontend typecheck**

Run: `cd crates/right-dashboard/frontend && pnpm run typecheck`
Expected: no errors.

- [ ] **Step 3: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. This also rebuilds the embedded dashboard bundle via `build.rs`, proving the frontend compiles and the bundle regression test still passes.

- [ ] **Step 4: Final review of the diff**

Run: `git diff master --stat` and confirm only the files listed in this plan changed (plus `right-lifecycle/src/lib.rs`, which was already modified before this work and must remain untouched/unstaged).

---

## Self-Review

**Spec coverage:**
- Counts + hit-rate per kind, no `$ saved` → Tasks 1-4, 6. ✓
- Window panels (FE-only, `UsageSourceSummary`) → Task 4. ✓
- Day-breakdown Sources (needs `UsageSourcePoint` token fields) → Tasks 5-6. ✓
- Day-panel per-day hit-rate on existing cache line → Task 6, Step 4. ✓
- `cacheHitRate` definition identical to `format_cache_line` → Task 1. ✓
- Backend: 3 fields + populate in `build_daily_series` + `0` in cost-learning river + fixtures → Task 5. ✓
- Tests: pure helpers unit-tested, components via SSR, backend aggregation test, final workspace test → Tasks 1-3, 4, 6, 7. ✓
- Out of scope (no `$ saved`, no per-model rows, no source-name prettifying, do not delete `format_summary_message`) → respected; no task touches them. ✓

**Placeholder scan:** none — every step has concrete code/commands.

**Type consistency:** `cacheHitRate(t: CacheTokens)` defined in Task 1; `CacheSubline` imports `CacheTokens` (Task 3); `UsageSourceSummary` (already has the 3 fields) and `UsageSourcePoint` (after Task 6 Step 3) both satisfy `CacheTokens` structurally; Rust `UsageSourcePoint` (Task 5) and TS `UsageSourcePoint` (Task 6) carry the same three field names (`input_tokens`, `cache_creation_tokens`, `cache_read_tokens`). ✓
