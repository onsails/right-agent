# Usage tab: cache-aware token palette

- **Date:** 2026-06-01
- **Status:** Approved (brainstorm); ready for implementation plan
- **Surface:** `right-dashboard` (Telegram Mini App) — Usage tab
- **Discussion language:** Russian; spec in English to match repo convention.

## Problem

The Usage tab shows token activity as flat text. Cache tokens are buried in
a muted `CacheSubline` that only appears when `cache_read > 0`, and there is
no way to compare cache behavior across sources (foreground, cron, learning)
at a glance. The `Web` counter (search/fetch requests) occupies a row nobody
needs. Nothing is color-coded, so reading the token mix takes effort.

## Goals

- Color-code the four token types — input (miss), output, cache-create,
  cache-read — with one shared palette, so the mix is scannable.
- Show cache composition **per source** (foreground/interactive, cron,
  `learning_*`, reflection, and runtime-discovered sources), not only in
  aggregate.
- Make hit-rate and cache-create-vs-read visually obvious without lying about
  magnitude.
- Remove the `Web` counters from the Usage UI.
- Do not bloat the tab: per-source detail stays a single line; the legend
  appears once.

## Non-goals

- No color treatment for the `Models` block (stays cost-only).
- No palette change to the spend chart (it is colored by *source*, a
  different axis).
- No removal of `web_*` columns from `usage_events` or the ingestion path —
  UI removal only, fully reversible, no migration.
- No new aggregation windows or time ranges.

## Approach (chosen: Variant 1 — colored numbers + thin hit bar)

`cache_read` routinely dominates input-bearing tokens (often 90%+). A naive
4-segment proportional bar collapses into one color and hides input/output/
create. So:

- **Exact numbers, color-coded.** Each token type is printed as a compact
  number prefixed by a colored dot. No magnitude distortion.
- **One honest proportional element: the hit bar.** A thin stacked bar over
  the *input-bearing* trio only — `miss │ create │ read` — where the read
  fraction is the hit rate. This is the one place proportion is meaningful,
  and it is exactly the cache story.
- `output` is generation, a separate axis; it appears only as a colored
  number, never in the hit bar.

Rejected alternatives: bar-hero with hidden numbers (loses density); full
4-color proportional bar (input/output/create invisible under read).

## Palette & theming

Four CSS custom properties, defined once in global styles (`App.vue`), reusing
the app's existing semantic hues so they read on both light and Telegram dark
themes:

| token | var | value | rationale |
|---|---|---|---|
| input / miss | `--token-input` | `#6b7b88` (hint grey) | raw input, cache miss |
| output | `--token-output` | `#2481cc` (accent blue) | generation |
| cache_create | `--token-create` | `#b87900` (amber) | cache write, full price |
| cache_read | `--token-read` | `#0d7a45` (green) | cache hit, cheap reuse |

These exact hexes are already used elsewhere in `App.vue` (status dots,
tone classes), so the palette is visually coherent and proven legible.

## Components

### `tokenBar.ts` (pure logic, unit-tested)

```ts
export interface TokenCounts {
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
}

// Stacked widths for the input-bearing trio, summing to 1.
// null when there are no input-bearing tokens (caller hides the bar).
export interface HitSegments { miss: number; create: number; read: number }

export function hitSegments(t: TokenCounts, minWidth?: number): HitSegments | null
```

- `bearing = input + cache_create + cache_read`. If `bearing === 0` → return
  `null` (no bar; numbers still render).
- Raw fractions `f = count / bearing`.
- **Min-width clamp** (default `minWidth = 0.04`): every *nonzero* segment is
  bumped to at least `minWidth`; the resulting overflow is removed from
  segments above `minWidth`, proportional to their excess over `minWidth`, so
  the widths still sum to 1 and no bumped segment falls back below the floor.
  Zero segments stay 0. This keeps a 0.5% `miss`/`create` visible at 99% hit
  without distorting the printed numbers.
- Hit rate reuses `cacheHitRate` from `usageCache.ts`
  (`read / (input + create + read)`).

### `TokenLine.vue`

Props: `{ tokens: TokenCounts; compact?: boolean }`.

- **Full** (aggregate, in the daily breakdown): row of four colored numbers
  (`in / out / create / read`, via `count`) + a hit-bar row with the `%`.
- **Compact** (per source / per window-source): one line — four colored
  mini-numbers (via `compactCount`) + inline hit bar + `%`.
- Bar segments use `--token-input` (miss), `--token-create`, `--token-read`.
  When `hitSegments` is `null`, render numbers only.

### `TokenLegend.vue`

A single line: `● in  ● out  ● create  ● read` in palette colors. Rendered
**once** near the top of the Usage tab. Per-source rows rely on fixed column
order, so they need no repeated legend.

### Removed: `CacheSubline.vue` (+ `CacheSubline.test.ts`)

`TokenLine compact` supersedes it at every call site. Both files become unused
as a direct result of this change and are deleted. `usageCache.ts`
(`cacheHitRate`, `CacheTokens`) stays — still used by `TokenLine`.

## Surfaces (before → after)

1. **`UsageBreakdown.vue`** (daily detail panel)
   - Counters block `Tokens / Cache / Web` (3 rows, lines 35–53) → `TokenLine`
     (full) bound to the daily point. **`Web` row removed.**
   - Per-source `CacheSubline` (line 62) → `TokenLine compact`.
   - `Models` block unchanged.

2. **`UsageView.vue`** (window list: today / 7d / 30d / all-time)
   - Per-source `CacheSubline` (line 76) → `TokenLine compact`.
   - Add `TokenLegend` once, above the two-column section.

## Semantics

- Hit bar covers input-bearing only (`miss│create│read`); `hit% =
  read/(input+create+read)`.
- `output` never enters the bar.
- A source with zero tokens renders no `TokenLine`. When `bearing === 0`, the
  bar is hidden but the numbers still print.
- Zero-valued types still print `0` so columns stay aligned and scannable.

## Backend changes

The daily per-source struct lacks `output_tokens`, so per-source `output`
cannot be shown by day. Add it.

- **`crates/right-dashboard/src/api_types.rs`** — add `pub output_tokens: u64`
  to `UsageSourcePoint` (after `input_tokens`).
- **`crates/right-dashboard/src/read_model/usage.rs`**
  - The daily `UsageSourcePoint` constructor (≈ lines 276–286): add
    `output_tokens: 0`.
  - After the per-source `input_tokens` accumulation (≈ line 295): add
    `source_entry.output_tokens += output_tokens.max(0) as u64;`. The row
    already SELECTs and binds `output_tokens` (used for the point-level total),
    so no SQL change is needed.
- **All other `UsageSourcePoint { … }` constructors must set the new field.**
  `UsageSourcePoint` is also embedded in `CostLearningPoint` (cost-learning
  river). The plan must `rg "UsageSourcePoint \{"` and update every site; the
  added field is additive (harmless where output is not otherwise tracked —
  set `0`).
- **`crates/right-dashboard/frontend/src/types.ts`** — add
  `output_tokens: number` to the TS `UsageSourcePoint`.

Window-level `UsageSourceSummary` already carries `output_tokens` and all
cache fields — no change.

## Web removal

UI only. Delete the `Web` row in `UsageBreakdown.vue`. Leave `web_search_requests`
/ `web_fetch_requests` in the Rust structs, TS types, DB, and ingestion
untouched (unused by the view; reversible without migration).

## File-by-file change list

New:
- `crates/right-dashboard/frontend/src/components/charts/tokenBar.ts`
- `crates/right-dashboard/frontend/src/components/charts/tokenBar.test.ts`
- `crates/right-dashboard/frontend/src/components/charts/TokenLine.vue`
- `crates/right-dashboard/frontend/src/components/charts/TokenLine.test.ts`
- `crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue`

Modified:
- `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue`
- `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts`
  (drop `Web` assertion; assert `TokenLine` present)
- `crates/right-dashboard/frontend/src/views/UsageView.vue`
- `crates/right-dashboard/frontend/src/types.ts`
- `crates/right-dashboard/frontend/src/App.vue` (palette CSS vars)
- `crates/right-dashboard/src/api_types.rs`
- `crates/right-dashboard/src/read_model/usage.rs` (+ test)

Deleted:
- `crates/right-dashboard/frontend/src/components/charts/CacheSubline.vue`
- `crates/right-dashboard/frontend/src/components/charts/CacheSubline.test.ts`

## Testing

- **`tokenBar.test.ts`** (pure): `bearing === 0 → null`; plain proportions sum
  to 1; min-width clamp bumps a tiny nonzero segment to the floor and re-sums
  to 1; zero segments stay 0; clamp leaves already-large segments above floor.
- **`TokenLine.test.ts`** (Vue SSR via `@vue/server-renderer`): renders four
  numbers; renders hit `%` when `bearing > 0`; omits the bar when
  `bearing === 0`; compact vs full output differ.
- **`UsageBreakdown.test.ts`**: no `Web` text; `TokenLine` rendered for
  aggregate and per source.
- **Rust** (`read_model/usage.rs` tests): per-source daily `output_tokens`
  aggregates across events. Keep
  `usage_overview_sources_match_learning_sources_constant` green.

## Verification cadence

- Intermediate (per TDD slice): frontend
  `cd crates/right-dashboard/frontend && npx vitest run <file>`; backend
  `devenv shell -- cargo test -p right-dashboard <filter>`.
- Frontend typecheck: `npx vue-tsc --noEmit` (or the project's existing
  typecheck script) after type changes.
- Final (mandatory): `devenv shell -- cargo test --workspace` **and**
  `cd crates/right-dashboard/frontend && npx vitest run`.

## Mockup (target)

```
Breakdown · 2026-05-31                         $2.65
 Subscription $2.10   API $0.55   Turns 14   Calls 9
 ●in 120k   ●out 30k   ●create 200k   ●read 3.2M
 hit  [▏▎██████████████████████]  91%

 Sources
  foreground  $2.10   ●120k ●30k ●200k ●3.2M  [▏▎███████████]  91%
  cron        $0.40   ●12k  ●3k  ●40k  ●180k  [▎▍██████████]   86%
  learning_*  $0.15   ●8k   ●1k  ●90k  ●20k   [█████▏▎░░░]     17%

 Models
  opus-4.8    $2.50
  haiku-4.5   $0.15
```

Legend (once, top of tab): `● in   ● out   ● create   ● read`

## Open risks

- **Constructor fan-out:** adding `output_tokens` to `UsageSourcePoint` breaks
  every constructor until updated — mitigated by an explicit `rg` sweep in the
  plan.
- **Clamp vs truth:** the min-width clamp distorts bar proportions for tiny
  segments by design; printed numbers stay exact, and the distortion is capped
  at `minWidth`.
- **Dark theme:** palette uses fixed hexes already proven in `App.vue`;
  acceptable on Telegram dark backgrounds.
