# Dashboard: sticky tab bar (desktop) + pinned token legend (Usage)

**Date:** 2026-06-03
**Status:** Design approved, pending spec review
**Scope:** `right-dashboard` frontend only — pure layout/CSS. No backend, schema, data, or component-logic changes.

## Problem

On the Usage view the token legend (`TokenLegend.vue` — `input` / `output` /
`cache create` / `cache read` color dots) renders once at the top and scrolls
out of view. The same color dots reappear in the breakdown panel and window
list via `TokenLine.vue` **without labels**, so once the legend has scrolled
away the colors are unreadable.

Separately, on desktop the navigation tabs (`overview` / `knowledge` /
`activity` / `usage` / …) sit in normal flow and scroll away, so switching
views after scrolling requires scrolling back to the top.

## Goals

1. Keep the navigation tabs visible while scrolling **on desktop** (pin to the
   top of the viewport).
2. Keep the token legend visible while scrolling **on the Usage view** (pin to
   the bottom of the viewport).

## Non-goals

- No tooltips / hover affordances (an earlier idea, dropped).
- No change to the topbar (agent name / status pill / display-mode button) — it
  continues to scroll away. Only the tabs pin.
- No change to mobile tab behavior — tabs already pin to the bottom on
  `≤560px` and that stays exactly as-is.
- No backend, data, or `UsageBreakdown`/`TokenLine` logic changes;
  `UsageBreakdown.test.ts` is untouched.

## Current layout facts (verified)

- The page scrolls via `body`; `.app-shell` is a centered block
  (`width: min(1160px, 100%)`, horizontal padding 12px). No nested scroll
  container exists, so `position: sticky`/`fixed` resolve against the viewport.
- `.view-tabs` styles live in the **global** `<style>` block of
  `App.vue` (not scoped to `AppShell.vue`).
- At `≤560px` in `display-normal`, `.app-shell.display-normal .view-tabs` is
  already `position: fixed; bottom: 0; z-index: 20` with
  `background: --tg-theme-secondary-bg-color` and a top border; the shell
  reserves `padding-bottom: calc(78px + env(safe-area-inset-bottom))` for it.
- `TokenLegend` is rendered in `UsageView.vue` (currently line 49) above the
  `two-column` section. Its styles are scoped inside `TokenLegend.vue`.
- The existing breakpoints in `App.vue` are `900px` (two-column collapse) and
  `560px` (mobile bottom nav). We reuse `560px` as the mobile/desktop boundary
  for both features so the new desktop rules are the exact counterpart of the
  existing mobile rules.

## Design

### 1. Sticky tab bar on desktop (`> 560px`)

Make `.view-tabs` stick to the top of the viewport on viewports wider than the
existing mobile breakpoint. This is the desktop mirror of the existing
`≤560px` fixed-bottom rule.

- Add to the base `.view-tabs` rule (or a `@media (min-width: 561px)` rule, to
  avoid affecting the `≤560px` fixed-bottom variant):
  - `position: sticky; top: 0;`
  - `z-index: 20;` (same stacking level as the mobile nav bar)
  - `background: var(--tg-theme-bg-color, #f4f6f8);` so scrolled content does
    not show through the gaps between tab buttons.
  - `border-bottom: 1px solid var(--tg-theme-section_separator_color, …);`
  - Full-bleed background: negative horizontal margin equal to the shell's
    side padding (`margin-inline: -12px;`) plus matching `padding-inline: 12px;`
    and a small vertical padding, so the sticky bar's background spans the full
    shell width instead of leaving 12px transparent gutters. Preserve the
    existing `margin-bottom: 10px` as the space below the bar.
- The breakpoint guard must ensure the `≤560px` fixed-bottom variant is **not**
  overridden. Because the mobile rule is scoped to
  `.app-shell.display-normal .view-tabs` and lives in a `@media (max-width:560px)`
  block (higher specificity + later in the cascade), the simplest safe approach
  is to put the new desktop rules in a `@media (min-width: 561px)` block. This
  guarantees no interaction between the two and keeps the mobile bar bit-for-bit
  unchanged.
- The topbar is intentionally **not** sticky; it scrolls above the pinned tabs.

Sticky validity check: no ancestor of `.view-tabs` sets `overflow` other than
visible (`.app-shell` and `body` do not), so `position: sticky` is honored.
`.view-tabs` keeps its own `overflow-x: auto`, which does not break its own
stickiness.

### 2. Pinned token legend on the Usage view

Move `<TokenLegend />` from the top of `UsageView.vue` to the **end** of its
template (after the `list-stack` section) and pin it to the bottom with
`position: sticky; bottom: 0`.

Rationale for sticky-at-end over `fixed`: a sticky element placed last in the
flow pins to the bottom of the viewport while page content extends below the
fold, and settles into place once the user scrolls to the very bottom — visually
"always at the bottom" — without needing reserved bottom padding and without
overlapping/hiding content.

- The legend gets a pinned-bar treatment (in `TokenLegend.vue` scoped styles,
  or a wrapper class added in `UsageView.vue`):
  - `position: sticky; bottom: 0; z-index: 15;`
  - `background: var(--tg-theme-secondary-bg-color, #ffffff);`
  - `border-top: 1px solid var(--tg-theme-section_separator_color, …);`
  - Full-bleed horizontal treatment matching the tab bar
    (`margin-inline: -12px; padding-inline: 12px;`) plus vertical padding, so it
    reads as a bar, not floating text.
  - Keep `flex-wrap: wrap` so the four legend chips wrap on narrow widths.
- **Mobile collision (`≤560px`):** the bottom edge is occupied by the fixed nav
  bar (height ≈ `78px` incl. safe-area). Add
  `@media (max-width: 560px)` to offset the legend above it:
  `bottom: calc(70px + env(safe-area-inset-bottom));` (a value that clears the
  nav bar; final number tuned visually against the existing `78px` reservation).
  The legend stays visible on mobile and does not overlap the nav bar.
- Scope: the legend lives only inside `UsageView.vue`, so the pinned bar appears
  only on the Usage tab. Other tabs are unaffected.

## Affected files

- `crates/right-dashboard/frontend/src/App.vue` — global `.view-tabs` desktop
  sticky rules (new `@media (min-width: 561px)` block).
- `crates/right-dashboard/frontend/src/views/UsageView.vue` — relocate
  `<TokenLegend />` to the end of the template; optionally add a wrapper class.
- `crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue` —
  scoped sticky-bottom bar styles + mobile offset.

## Verification

- Build the frontend (`pnpm`/`npm` build per the dashboard's toolchain) to
  confirm no TS/template errors.
- Visual check on a wide viewport: scroll the dashboard — tabs stay pinned at
  top, topbar scrolls away; switch tabs works while scrolled.
- Visual check on the Usage tab (wide): scroll the windows list — legend stays
  pinned at the bottom; at full scroll it sits flush at the end.
- Visual check at `≤560px`: tabs still pinned at the bottom (unchanged); the
  legend sits just above the nav bar without overlap.
- Existing `UsageBreakdown.test.ts` still passes (untouched).
- Final: `devenv shell -- cargo test --workspace` (mandatory end-of-work gate;
  no Rust changes expected, so this is a regression guard).
