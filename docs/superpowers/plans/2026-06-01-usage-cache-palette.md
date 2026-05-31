# Usage Cache Palette Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Usage tab's flat token text with a shared 4-color token palette — colored `input/output/cache-create/cache-read` numbers plus an honest cache hit-rate bar — shown both in aggregate and per source, and remove the `Web` counters.

**Architecture:** A pure `tokenBar.ts` computes hit-bar segment widths (with a min-width clamp so tiny segments stay visible). A single `TokenLine.vue` renders the colored numbers + bar (full or `compact`); a one-line `TokenLegend.vue` explains the colors once. Both replace `CacheSubline.vue` at every call site. The backend gains `output_tokens` on the daily per-source point so per-source output is available by day.

**Tech Stack:** Rust (`right-dashboard` read models, `turso`/`right-db`), Vue 3 + TypeScript (Telegram Mini App), Vitest + `@vue/server-renderer` for SSR component tests. Frontend is embedded into the Rust binary by `build.rs` (`include_dir!`), so `cargo build -p right-dashboard` rebuilds the bundle.

Spec: `docs/superpowers/specs/2026-06-01-usage-cache-palette-design.md`.

**Conventions:**
- Commit per task (Conventional Commits, scope `dashboard`).
- Frontend commands run from `crates/right-dashboard/frontend` (pnpm; lockfile is `pnpm-lock.yaml`).
- Rust commands run via `devenv shell -- cargo …`.

---

## Task 1: Backend — `output_tokens` on the daily per-source point

Adds `output_tokens` to `UsageSourcePoint` and aggregates it in the daily series. Because the field is required, every `UsageSourcePoint { … }` literal must set it (struct def + 4 constructors + TS type).

**Files:**
- Test: `crates/right-dashboard/src/read_model/usage.rs` (tests module, after line 1169)
- Modify: `crates/right-dashboard/src/api_types.rs` (struct at 215; test fixtures at ~807, ~873)
- Modify: `crates/right-dashboard/src/read_model/usage.rs:276` (constructor) and `:295` (accumulation)
- Modify: `crates/right-dashboard/src/read_model/dashboard_overview.rs:373` (constructor)
- Modify: `crates/right-dashboard/frontend/src/types.ts:261` (`UsageSourcePoint`)

- [ ] **Step 1: Write the failing test**

In `crates/right-dashboard/src/read_model/usage.rs`, inside `mod tests`, immediately after `usage_overview_aggregates_cache_tokens_per_source_in_daily_series` (after line 1169):

```rust
    #[tokio::test]
    async fn usage_overview_aggregates_output_tokens_per_source_in_daily_series() {
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
        // Two events × output 20 (see insert_usage VALUES).
        assert_eq!(interactive.output_tokens, 40);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-dashboard usage_overview_aggregates_output_tokens_per_source_in_daily_series`
Expected: FAIL — compile error `E0609: no field 'output_tokens' on type 'UsageSourcePoint'`.

- [ ] **Step 3: Add the field to the struct**

In `crates/right-dashboard/src/api_types.rs`, in `pub struct UsageSourcePoint` (line 215), after `pub input_tokens: u64,` (line 222) add:

```rust
    pub output_tokens: u64,
```

- [ ] **Step 4: Initialize and accumulate it in the daily series**

In `crates/right-dashboard/src/read_model/usage.rs`, in the `UsageSourcePoint` constructor (the `.or_insert_with(|| UsageSourcePoint { … })` at line 276), after `input_tokens: 0,` (line 283) add:

```rust
                output_tokens: 0,
```

Then, after the per-source `input_tokens` accumulation at line 295 (`source_entry.input_tokens += input_tokens.max(0) as u64;`) add:

```rust
        source_entry.output_tokens += output_tokens.max(0) as u64;
```

(`output_tokens` is already bound in the row destructuring at line 240 and used for the point-level total; no SQL change is needed.)

- [ ] **Step 5: Fix the remaining constructors**

In `crates/right-dashboard/src/read_model/dashboard_overview.rs`, in the `UsageSourcePoint` constructor at line 373, after `input_tokens: 0,` (line 382) add (keeping the existing "intentionally zero" comment intent — this query selects no token columns):

```rust
                output_tokens: 0,
```

In `crates/right-dashboard/src/api_types.rs`, both test fixtures build `UsageSourcePoint { … }` literals (≈ line 807 and ≈ line 873). In each literal, after its `input_tokens:` line add:

```rust
                    output_tokens: 0,
```

- [ ] **Step 6: Mirror the field in the TS type**

In `crates/right-dashboard/frontend/src/types.ts`, in `export interface UsageSourcePoint` (line 261), after `input_tokens: number` (line 268) add:

```ts
  output_tokens: number
```

- [ ] **Step 7: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-dashboard usage_overview_aggregates_output_tokens_per_source_in_daily_series`
Expected: PASS.

Then confirm nothing else broke in the crate:
Run: `devenv shell -- cargo test -p right-dashboard`
Expected: PASS (including `usage_overview_aggregates_cache_tokens_per_source_in_daily_series` and `usage_overview_sources_match_learning_sources_constant`).

- [ ] **Step 8: Commit**

```bash
git add crates/right-dashboard/src/api_types.rs \
        crates/right-dashboard/src/read_model/usage.rs \
        crates/right-dashboard/src/read_model/dashboard_overview.rs \
        crates/right-dashboard/frontend/src/types.ts
git commit -m "feat(dashboard): track output_tokens per source in daily usage series"
```

---

## Task 2: Frontend — token palette CSS variables

Four shared color tokens so `TokenLine`/`TokenLegend` segments and dots have colors. Defined in the global (non-scoped) `<style>` of `App.vue`.

**Files:**
- Modify: `crates/right-dashboard/frontend/src/App.vue:115` (top of global `<style>`)

- [ ] **Step 1: Add the palette block**

In `crates/right-dashboard/frontend/src/App.vue`, immediately after the `<style>` opening tag (line 115) and before `body {` (line 120), insert:

```css
:root {
  --token-input: #6b7b88;
  --token-output: #2481cc;
  --token-create: #b87900;
  --token-read: #0d7a45;
}
```

These exact hexes already appear elsewhere in `App.vue` (status dots, tone classes), so the palette is consistent and legible on both light and Telegram dark backgrounds.

- [ ] **Step 2: Verify the bundle still typechecks/builds**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vue-tsc --noEmit)`
Expected: no errors (CSS-only change).

- [ ] **Step 3: Commit**

```bash
git add crates/right-dashboard/frontend/src/App.vue
git commit -m "feat(dashboard): add token-type color palette variables"
```

---

## Task 3: Frontend — `tokenBar.ts` pure hit-bar math (TDD)

Computes stacked widths for the input-bearing trio (`miss│create│read`) with a min-width clamp so tiny nonzero segments stay visible.

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/charts/tokenBar.ts`
- Test: `crates/right-dashboard/frontend/src/components/charts/tokenBar.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/charts/tokenBar.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { hitSegments, type TokenCounts } from './tokenBar'

function counts(over: Partial<TokenCounts> = {}): TokenCounts {
  return { input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0, ...over }
}

describe('hitSegments', () => {
  it('returns null when there are no input-bearing tokens', () => {
    // output alone is not input-bearing.
    expect(hitSegments(counts({ output_tokens: 99 }))).toBeNull()
  })

  it('returns raw proportions summing to 1 when every segment clears the floor', () => {
    const s = hitSegments(counts({ input_tokens: 100, cache_creation_tokens: 100, cache_read_tokens: 100 }))!
    expect(s.miss).toBeCloseTo(1 / 3, 6)
    expect(s.create).toBeCloseTo(1 / 3, 6)
    expect(s.read).toBeCloseTo(1 / 3, 6)
    expect(s.miss + s.create + s.read).toBeCloseTo(1, 6)
  })

  it('bumps a tiny nonzero segment to the floor and still sums to 1', () => {
    // bearing 360; raw miss = 10/360 ≈ 0.0278 < 0.04 floor.
    const s = hitSegments(counts({ input_tokens: 10, cache_creation_tokens: 50, cache_read_tokens: 300 }))!
    expect(s.miss).toBeCloseTo(0.04, 6)
    expect(s.miss + s.create + s.read).toBeCloseTo(1, 6)
    expect(s.read).toBeLessThan(300 / 360) // donor gave up width
    expect(s.read).toBeGreaterThan(s.create)
  })

  it('bumps two tiny segments and renormalizes via the single donor', () => {
    const s = hitSegments(counts({ input_tokens: 5, cache_creation_tokens: 5, cache_read_tokens: 990 }))!
    expect(s.miss).toBeCloseTo(0.04, 6)
    expect(s.create).toBeCloseTo(0.04, 6)
    expect(s.read).toBeCloseTo(0.92, 6)
    expect(s.miss + s.create + s.read).toBeCloseTo(1, 6)
  })

  it('keeps zero segments at zero', () => {
    const s = hitSegments(counts({ cache_read_tokens: 500 }))!
    expect(s.miss).toBe(0)
    expect(s.create).toBe(0)
    expect(s.read).toBe(1)
  })

  it('leaves a lone segment at full width (never below floor)', () => {
    const s = hitSegments(counts({ input_tokens: 100 }))!
    expect(s.miss).toBe(1)
    expect(s.create).toBe(0)
    expect(s.read).toBe(0)
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/tokenBar.test.ts)`
Expected: FAIL — cannot resolve module `./tokenBar`.

- [ ] **Step 3: Implement `tokenBar.ts`**

Create `crates/right-dashboard/frontend/src/components/charts/tokenBar.ts`:

```ts
// Token counts for one row of usage (a daily point, a source, or a window).
// `cacheHitRate` in usageCache.ts consumes the input-bearing subset of this.
export interface TokenCounts {
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
}

// Stacked widths (fractions summing to 1) for the input-bearing trio.
export interface HitSegments {
  miss: number
  create: number
  read: number
}

// Minimum visible width for any nonzero segment, so a 0.5% miss/create does
// not vanish under a dominant cache_read. Tuning constant.
const DEFAULT_MIN_WIDTH = 0.04

// Returns null when there are no input-bearing tokens (caller hides the bar).
export function hitSegments(t: TokenCounts, minWidth: number = DEFAULT_MIN_WIDTH): HitSegments | null {
  const raw = {
    miss: Math.max(0, t.input_tokens),
    create: Math.max(0, t.cache_creation_tokens),
    read: Math.max(0, t.cache_read_tokens),
  }
  const bearing = raw.miss + raw.create + raw.read
  if (bearing === 0) {
    return null
  }

  const keys = ['miss', 'create', 'read'] as const
  const result: HitSegments = {
    miss: raw.miss / bearing,
    create: raw.create / bearing,
    read: raw.read / bearing,
  }

  // Bump nonzero segments below the floor up to minWidth; remove the overflow
  // from donor segments above the floor, proportional to their slack.
  const bumped = keys.filter((k) => result[k] > 0 && result[k] < minWidth)
  if (bumped.length === 0) {
    return result
  }

  let deficit = 0
  for (const k of bumped) {
    deficit += minWidth - result[k]
    result[k] = minWidth
  }
  const donors = keys.filter((k) => result[k] > minWidth)
  const slack = donors.reduce((sum, k) => sum + (result[k] - minWidth), 0)
  if (slack > 0) {
    for (const k of donors) {
      result[k] -= deficit * ((result[k] - minWidth) / slack)
    }
  }
  return result
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/tokenBar.test.ts)`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/charts/tokenBar.ts \
        crates/right-dashboard/frontend/src/components/charts/tokenBar.test.ts
git commit -m "feat(dashboard): add tokenBar hit-segment math with min-width clamp"
```

---

## Task 4: Frontend — `TokenLine.vue` (TDD via SSR)

Renders four colored token numbers and the hit-rate bar. `full` layout (two rows) for aggregates; `compact` (one row) per source.

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/charts/TokenLine.vue`
- Test: `crates/right-dashboard/frontend/src/components/charts/TokenLine.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/charts/TokenLine.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import TokenLine from './TokenLine.vue'
import type { TokenCounts } from './tokenBar'

function counts(over: Partial<TokenCounts> = {}): TokenCounts {
  return { input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0, ...over }
}

async function render(tokens: TokenCounts, compact = false) {
  const app = createSSRApp({ render: () => h(TokenLine, { tokens, compact }) })
  return renderToString(app)
}

describe('TokenLine', () => {
  it('renders all four token counts and the hit-rate bar', async () => {
    const html = await render(
      counts({ input_tokens: 10, output_tokens: 20, cache_creation_tokens: 50, cache_read_tokens: 300 }),
    )
    expect(html).toContain('10')
    expect(html).toContain('20')
    expect(html).toContain('50')
    expect(html).toContain('300')
    expect(html).toContain('hit-bar')
    expect(html).toContain('83%')
  })

  it('omits the hit bar when there are no input-bearing tokens', async () => {
    const html = await render(counts({ output_tokens: 20 }))
    expect(html).toContain('token-line')
    expect(html).not.toContain('hit-bar')
  })

  it('renders the compact layout class when compact', async () => {
    const html = await render(counts({ input_tokens: 10, cache_read_tokens: 90 }), true)
    expect(html).toContain('compact')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/TokenLine.test.ts)`
Expected: FAIL — cannot resolve `./TokenLine.vue`.

- [ ] **Step 3: Implement `TokenLine.vue`**

Create `crates/right-dashboard/frontend/src/components/charts/TokenLine.vue`:

```vue
<script setup lang="ts">
import { computed } from 'vue'
import { compactCount, percent } from '../../format'
import { cacheHitRate } from './usageCache'
import { hitSegments, type TokenCounts } from './tokenBar'

const props = defineProps<{
  tokens: TokenCounts
  compact?: boolean
}>()

const segments = computed(() => hitSegments(props.tokens))
const hit = computed(() => percent(cacheHitRate(props.tokens)))
</script>

<template>
  <div class="token-line" :class="{ compact }">
    <div class="token-nums">
      <span class="tok tok-input">{{ compactCount(tokens.input_tokens) }}</span>
      <span class="tok tok-output">{{ compactCount(tokens.output_tokens) }}</span>
      <span class="tok tok-create">{{ compactCount(tokens.cache_creation_tokens) }}</span>
      <span class="tok tok-read">{{ compactCount(tokens.cache_read_tokens) }}</span>
    </div>
    <div v-if="segments" class="token-hit">
      <span class="hit-bar" role="img" :aria-label="`cache hit ${hit}`">
        <span class="seg seg-miss" :style="{ width: `${segments.miss * 100}%` }" />
        <span class="seg seg-create" :style="{ width: `${segments.create * 100}%` }" />
        <span class="seg seg-read" :style="{ width: `${segments.read * 100}%` }" />
      </span>
      <span class="hit-pct">{{ hit }}</span>
    </div>
  </div>
</template>

<style scoped>
.token-line {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.token-line.compact {
  flex-direction: row;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.token-nums {
  display: flex;
  gap: 8px;
  font-size: 0.72rem;
}
.tok {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  color: var(--tg-theme-text-color, #17212b);
}
.tok::before {
  content: '';
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--dot);
}
.tok-input { --dot: var(--token-input); }
.tok-output { --dot: var(--token-output); }
.tok-create { --dot: var(--token-create); }
.tok-read { --dot: var(--token-read); }
.token-hit {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 0.72rem;
}
.hit-bar {
  display: inline-flex;
  height: 8px;
  min-width: 120px;
  flex: 1;
  border-radius: 4px;
  overflow: hidden;
  background: var(--tg-theme-secondary-bg-color, #e8edf1);
}
.compact .hit-bar {
  min-width: 80px;
  max-width: 160px;
  flex: 0 1 140px;
}
.seg {
  display: block;
  height: 100%;
}
.seg-miss { background: var(--token-input); }
.seg-create { background: var(--token-create); }
.seg-read { background: var(--token-read); }
.hit-pct {
  color: var(--tg-theme-hint-color, #6b7b88);
}
</style>
```

- [ ] **Step 4: Run test to verify it passes**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/TokenLine.test.ts)`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/charts/TokenLine.vue \
        crates/right-dashboard/frontend/src/components/charts/TokenLine.test.ts
git commit -m "feat(dashboard): add TokenLine colored token + hit-bar component"
```

---

## Task 5: Frontend — `TokenLegend.vue` (TDD via SSR)

One-line legend explaining the four colors. Rendered once at the top of the Usage tab.

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue`
- Test: `crates/right-dashboard/frontend/src/components/charts/TokenLegend.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/charts/TokenLegend.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import TokenLegend from './TokenLegend.vue'

describe('TokenLegend', () => {
  it('labels all four token types', async () => {
    const app = createSSRApp({ render: () => h(TokenLegend) })
    const html = await renderToString(app)
    expect(html).toContain('token-legend')
    expect(html).toContain('input')
    expect(html).toContain('output')
    expect(html).toContain('cache create')
    expect(html).toContain('cache read')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/TokenLegend.test.ts)`
Expected: FAIL — cannot resolve `./TokenLegend.vue`.

- [ ] **Step 3: Implement `TokenLegend.vue`**

Create `crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue`:

```vue
<script setup lang="ts"></script>

<template>
  <div class="token-legend">
    <span class="lg lg-input">input</span>
    <span class="lg lg-output">output</span>
    <span class="lg lg-create">cache create</span>
    <span class="lg lg-read">cache read</span>
  </div>
</template>

<style scoped>
.token-legend {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  font-size: 0.72rem;
  color: var(--tg-theme-hint-color, #6b7b88);
  margin: 0 0 4px;
}
.lg {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.lg::before {
  content: '';
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--dot);
}
.lg-input { --dot: var(--token-input); }
.lg-output { --dot: var(--token-output); }
.lg-create { --dot: var(--token-create); }
.lg-read { --dot: var(--token-read); }
</style>
```

- [ ] **Step 4: Run test to verify it passes**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/TokenLegend.test.ts)`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue \
        crates/right-dashboard/frontend/src/components/charts/TokenLegend.test.ts
git commit -m "feat(dashboard): add TokenLegend palette key"
```

---

## Task 6: Frontend — wire `UsageBreakdown.vue` (counters → TokenLine, remove Web)

Replaces the `Tokens / Cache / Web` counters block with one `TokenLine` (full), removes the `Web` row, and swaps the per-source `CacheSubline` for `TokenLine compact`.

**Files:**
- Modify: `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue`
- Test: `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts`

- [ ] **Step 1: Update the test (new behavior first)**

Replace the entire body of `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts` with:

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
      turns: 1, invocations: 1, input_tokens: 10, output_tokens: 20,
      cache_creation_tokens: 50, cache_read_tokens: 300,
    }],
    models: [],
    ...over,
  }
}

async function render(p: UsageDailyPoint | null) {
  const app = createSSRApp({ render: () => h(UsageBreakdown, { point: p }) })
  return renderToString(app)
}

describe('UsageBreakdown tokens', () => {
  it('renders the per-day token line, hit-rate and a per-source token line', async () => {
    const html = await render(point())
    expect(html).toContain('token-line')
    expect(html).toContain('hit-bar')
    expect(html).toContain('83%')
    expect(html).toContain('interactive')
  })

  it('no longer renders the Web counters or the old cache subline', async () => {
    const html = await render(point())
    expect(html).not.toContain('Web')
    expect(html).not.toContain('created') // CacheSubline removed (note: 'seg-create' has no 'd')
  })

  it('omits a source hit bar when that source has no input-bearing tokens but keeps the per-day bar', async () => {
    const html = await render(point({
      sources: [{
        source: 'cron', cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
        turns: 1, invocations: 1, input_tokens: 0, output_tokens: 0,
        cache_creation_tokens: 0, cache_read_tokens: 0,
      }],
    }))
    expect(html).toContain('cron')
    expect(html).toContain('hit-bar') // per-day Counters still input-bearing
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/UsageBreakdown.test.ts)`
Expected: FAIL — current component still renders `Web` and the `created` subline; `token-line` absent.

- [ ] **Step 3: Rewrite `UsageBreakdown.vue`**

Replace the full contents of `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue` with:

```vue
<script setup lang="ts">
import { money } from '../../format'
import TokenLine from './TokenLine.vue'
import type { UsageDailyPoint } from '../../types'

defineProps<{
  point: UsageDailyPoint | null
}>()
</script>

<template>
  <aside class="panel detail-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Breakdown</p>
        <h2>{{ point?.date ?? 'None selected' }}</h2>
      </div>
      <strong>{{ money(point?.total_cost_usd) }}</strong>
    </header>
    <p v-if="!point" class="muted-line">Select a day</p>
    <template v-else>
      <dl class="meta-grid compact">
        <div><dt>Subscription</dt><dd>{{ money(point.subscription_cost_usd) }}</dd></div>
        <div><dt>API</dt><dd>{{ money(point.api_cost_usd) }}</dd></div>
        <div><dt>Turns</dt><dd>{{ point.turns }}</dd></div>
        <div><dt>Calls</dt><dd>{{ point.invocations }}</dd></div>
      </dl>
      <section class="text-block">
        <h3>Tokens</h3>
        <TokenLine :tokens="point" />
      </section>
      <section class="text-block">
        <h3>Sources</h3>
        <div class="row-list">
          <div v-for="source in (point.sources ?? [])" :key="source.source" class="usage-source">
            <div class="model-row">
              <span>{{ source.source }}</span>
              <strong>{{ money(source.cost_usd) }}</strong>
            </div>
            <TokenLine :tokens="source" compact />
          </div>
          <p v-if="(point.sources ?? []).length === 0" class="muted-line">No source spend</p>
        </div>
      </section>
      <section class="text-block">
        <h3>Models</h3>
        <div class="row-list">
          <div v-for="model in (point.models ?? [])" :key="model.model" class="model-row">
            <span>{{ model.model }}</span>
            <strong>{{ money(model.cost_usd) }}</strong>
          </div>
          <p v-if="(point.models ?? []).length === 0" class="muted-line">No model spend</p>
        </div>
      </section>
    </template>
  </aside>
</template>
```

(`point` is `UsageDailyPoint` and each `source` is `UsageSourcePoint` — both structurally satisfy `TokenCounts` after Task 1. The old `cacheHitRate`/`percent`/`count`/`CacheSubline` imports are gone.)

- [ ] **Step 4: Run test to verify it passes**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/UsageBreakdown.test.ts)`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue \
        crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts
git commit -m "feat(dashboard): use TokenLine in usage breakdown, drop Web counters"
```

---

## Task 7: Frontend — wire `UsageView.vue` (legend + per-source TokenLine)

Adds `TokenLegend` once at the top and swaps the per-source `CacheSubline` in window rows for `TokenLine compact`.

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.vue`
- Test: `crates/right-dashboard/frontend/src/views/UsageView.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/views/UsageView.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import UsageView from './UsageView.vue'
import type { UsageOverviewResponse } from '../types'

function usage(): UsageOverviewResponse {
  return {
    agent: 'agent-b',
    generated_at: '2026-05-31T00:00:00Z',
    selected_window: 'today',
    windows: [{
      key: 'today', label: 'Today',
      total_cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
      turns: 1, invocations: 1, input_tokens: 10, output_tokens: 20,
      cache_creation_tokens: 50, cache_read_tokens: 300,
      web_search_requests: 0, web_fetch_requests: 0, per_model: [],
      budget_skip_count: 0,
      sources: [{
        source: 'interactive', cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
        turns: 1, invocations: 1, input_tokens: 10, output_tokens: 20,
        cache_creation_tokens: 50, cache_read_tokens: 300,
        web_search_requests: 0, web_fetch_requests: 0, per_model: [],
      }],
    }],
    daily_series: [], // empty → spend chart shows its empty state, no echarts mount under SSR
    source_series: [],
    warnings: [],
  }
}

async function render(u: UsageOverviewResponse | null) {
  const app = createSSRApp({ render: () => h(UsageView, { usage: u, loading: false, error: null }) })
  return renderToString(app)
}

describe('UsageView', () => {
  it('renders the token legend and a per-source token line', async () => {
    const html = await render(usage())
    expect(html).toContain('token-legend')
    expect(html).toContain('token-line')
    expect(html).toContain('interactive')
    expect(html).not.toContain('created') // CacheSubline removed
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run src/views/UsageView.test.ts)`
Expected: FAIL — `token-legend` absent; `CacheSubline` still imported.

- [ ] **Step 3: Edit `UsageView.vue`**

In `crates/right-dashboard/frontend/src/views/UsageView.vue`:

Replace the import line (line 6):

```ts
import CacheSubline from '../components/charts/CacheSubline.vue'
```

with:

```ts
import TokenLine from '../components/charts/TokenLine.vue'
import TokenLegend from '../components/charts/TokenLegend.vue'
```

Add the legend immediately after the `warnings` `<section>` (after line 46, before the `two-column` section at line 48):

```html
    <TokenLegend />
```

Replace the per-source subline (line 76):

```html
            <CacheSubline :tokens="source" />
```

with:

```html
            <TokenLine :tokens="source" compact />
```

(`source` is `UsageSourceSummary`, which already carries `output_tokens` and the cache fields → satisfies `TokenCounts`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run src/views/UsageView.test.ts)`
Expected: PASS.

> If the import of `UsageSpendChart` → `AsyncVChart` breaks SSR (echarts touching `window` at import time), the empty `daily_series` already routes to the chart's empty-state branch; if it still fails, wrap the test render so the chart is not imported is NOT acceptable — instead confirm `AsyncVChart` uses `defineAsyncComponent` (lazy) and report. Do not stub production code to pass a test.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/UsageView.vue \
        crates/right-dashboard/frontend/src/views/UsageView.test.ts
git commit -m "feat(dashboard): add token legend and per-source TokenLine to usage view"
```

---

## Task 8: Frontend — delete the now-unused `CacheSubline`

`TokenLine compact` has replaced `CacheSubline` everywhere. Remove the dead component and its test.

**Files:**
- Delete: `crates/right-dashboard/frontend/src/components/charts/CacheSubline.vue`
- Delete: `crates/right-dashboard/frontend/src/components/charts/CacheSubline.test.ts`

- [ ] **Step 1: Confirm no remaining references**

Run: `rg -n "CacheSubline" crates/right-dashboard/frontend/src`
Expected: only the two files about to be deleted (no imports elsewhere). If anything else matches, fix that call site first.

- [ ] **Step 2: Delete the files**

```bash
git rm crates/right-dashboard/frontend/src/components/charts/CacheSubline.vue \
       crates/right-dashboard/frontend/src/components/charts/CacheSubline.test.ts
```

- [ ] **Step 3: Run the full frontend suite + typecheck**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run)`
Expected: PASS (no missing-import failures; `CacheSubline.test.ts` is gone).

Run: `(cd crates/right-dashboard/frontend && pnpm exec vue-tsc --noEmit)`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git commit -m "refactor(dashboard): remove CacheSubline superseded by TokenLine"
```

---

## Task 9: Final verification

Mandatory full-suite checks before declaring done (per AGENTS.rust.md and project verification cadence).

- [ ] **Step 1: Frontend — full unit suite**

Run: `(cd crates/right-dashboard/frontend && pnpm exec vitest run)`
Expected: PASS — includes `tokenBar`, `TokenLine`, `TokenLegend`, `UsageBreakdown`, `UsageView`.

- [ ] **Step 2: Frontend — typecheck + build (mirrors `build.rs`)**

Run: `(cd crates/right-dashboard/frontend && pnpm run build)`
Expected: `vue-tsc --noEmit` passes and `vite build` produces `dist` with no errors.

- [ ] **Step 3: Backend — crate tests**

Run: `devenv shell -- cargo test -p right-dashboard`
Expected: PASS — including the new `usage_overview_aggregates_output_tokens_per_source_in_daily_series` and the unchanged `usage_overview_sources_match_learning_sources_constant`.

- [ ] **Step 4: Confirm the embedded bundle rebuilds**

Run: `devenv shell -- cargo build -p right-dashboard`
Expected: `build.rs` reinstalls/builds the frontend and embeds it via `include_dir!`; build succeeds.

- [ ] **Step 5: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Record any pre-existing unrelated failures separately.

- [ ] **Step 6: Final commit (if anything outstanding)**

All task commits should already be in place. If any tracked change remains:

```bash
git status --short
git commit -am "chore(dashboard): finalize usage cache palette"
```

---

## Self-Review

**Spec coverage**

- Colored palette for all four token types → Task 2 (vars), Task 4 (`TokenLine` dots/segments), Task 5 (legend). ✓
- Honest hit bar with min-width clamp → Task 3 (`tokenBar.ts`). ✓
- Per-source cache detail (foreground/cron/learning/discovered) → Task 6 (breakdown per-source) + Task 7 (window rows). ✓
- Remove `Web` from UI → Task 6. ✓
- No bloat (per-source single line, legend once) → `TokenLine compact` replaces `CacheSubline` 1:1; legend added once in Task 7. ✓
- Backend `output_tokens` on `UsageSourcePoint` (+ all constructors, + TS type) → Task 1. ✓
- `web_*` columns/ingestion untouched → no task removes them. ✓
- Non-goals respected: Models stay cost-only (Task 6 keeps Models block as-is); spend chart palette untouched (no task edits `UsageSpendChart.vue`). ✓
- Tests: `tokenBar` math, SSR `TokenLine`/`TokenLegend`, updated `UsageBreakdown`, new `UsageView`, Rust aggregation, learning-sources constant kept green → Tasks 1,3,4,5,6,7,9. ✓

**Placeholder scan:** No TBD/TODO/"handle edge cases"/"similar to". Every code step contains full code; every command lists expected output. ✓

**Type consistency:** `TokenCounts` defined in Task 3 and consumed identically in Tasks 4/6/7. `hitSegments` returns `HitSegments | null`, consumed as `segments` (null-guarded) in Task 4. `output_tokens: u64` (Rust) / `output_tokens: number` (TS) added consistently in Task 1 and present in every fixture used by later tests (Task 6 `point()` source, Task 7 `usage()` window+source). `compactCount`/`percent`/`money`/`cacheHitRate` are existing exports used with their current signatures. ✓
