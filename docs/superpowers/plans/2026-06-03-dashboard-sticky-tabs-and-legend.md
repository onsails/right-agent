# Dashboard sticky tabs + pinned Usage legend — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pin the navigation tabs to the top of the viewport on desktop, and pin the token legend to the bottom of the viewport on the Usage view, so both stay visible while scrolling.

**Architecture:** Pure CSS/layout changes in the `right-dashboard` Vue frontend. Desktop tab pinning is a `@media (min-width: 561px)` mirror of the existing `≤560px` fixed-bottom mobile rule in `App.vue`'s global stylesheet. The legend is relocated to the end of `UsageView.vue` and pinned with `position: sticky; bottom: 0` via scoped styles in `TokenLegend.vue`, offset above the mobile nav bar on small screens.

**Tech Stack:** Vue 3 SFC, Vite, vue-tsc (typecheck), Vitest (SSR component tests), pnpm. Spec: `docs/superpowers/specs/2026-06-03-dashboard-sticky-tabs-and-legend-design.md`.

**Note on testing:** Layout/sticky behavior cannot be meaningfully unit-tested (no DOM layout engine in the SSR test harness). The existing `TokenLegend.test.ts` and `UsageBreakdown.test.ts` are regression guards that must stay green; correctness of the pinning itself is verified by `pnpm typecheck`/`build` (no template/TS errors) plus the manual visual checklist in the final task. There is therefore no failing-test-first step for the CSS — this is an intentional, stated exception to TDD for untestable layout CSS.

All commands run from `crates/right-dashboard/frontend/`. If `pnpm` is not on `$PATH`, prefix with `devenv shell -- ` from the repo root.

---

## File Structure

- `crates/right-dashboard/frontend/src/App.vue` — global `<style>`; owns `.view-tabs` layout. **Modify:** add desktop sticky-top rules.
- `crates/right-dashboard/frontend/src/views/UsageView.vue` — Usage view template. **Modify:** relocate `<TokenLegend />` from the top to the end of the `AsyncState` slot.
- `crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue` — legend component (sole consumer is `UsageView`). **Modify:** scoped styles → sticky-bottom bar + mobile offset.

---

## Task 1: Baseline — confirm tests green before changes

**Files:** none (verification only)

- [ ] **Step 1: Run the dashboard frontend test suite**

Run: `pnpm install --frozen-lockfile && pnpm test`
Expected: PASS (all existing Vitest suites green, including `TokenLegend.test.ts` and `UsageBreakdown.test.ts`). Record any pre-existing failures before proceeding.

- [ ] **Step 2: Run typecheck to capture a clean baseline**

Run: `pnpm typecheck`
Expected: PASS (no `vue-tsc` errors).

---

## Task 2: Pin navigation tabs to the top on desktop (`> 560px`)

**Files:**
- Modify: `crates/right-dashboard/frontend/src/App.vue` (global `<style>`, after the `.view-tabs { overflow-x: auto; }` rule near line 268-270)

- [ ] **Step 1: Add the desktop sticky-top media block**

In `App.vue`'s global `<style>`, locate:

```css
.view-tabs {
  overflow-x: auto;
}
```

Immediately after that rule, add:

```css
@media (min-width: 561px) {
  .view-tabs {
    position: sticky;
    top: 0;
    z-index: 20;
    margin-inline: -12px;
    padding: 8px 12px;
    background: var(--tg-theme-bg-color, #f4f6f8);
    border-bottom: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  }
}
```

Rationale: `margin-inline: -12px` + `padding-inline: 12px` makes the bar background full-bleed across `.app-shell`'s 12px side padding while keeping the buttons inset. `top: 0` pins to the viewport. The existing base `.view-tabs { margin-bottom: 10px; }` is preserved (spacing below the bar). The `≤560px` fixed-bottom rule lives in a separate `@media (max-width: 560px)` block, so this `min-width: 561px` block never overrides it — mobile stays bit-for-bit unchanged.

- [ ] **Step 2: Typecheck and build**

Run: `pnpm typecheck && pnpm build`
Expected: PASS (no errors). The build confirms the SFC `<style>` parses.

- [ ] **Step 3: Visual verification (wide viewport)**

Serve the built app or run `pnpm dev` and open the dashboard at a window width > 560px. Scroll down on any tab.
Expected: the topbar (agent name / status pill) scrolls up and away; the tab row stays pinned at the top of the viewport with a solid background and a bottom separator; content scrolls cleanly underneath (no content showing through the gaps between tab buttons). Switching tabs while scrolled works.

- [ ] **Step 4: Visual verification (mobile, ≤560px)**

Resize the window to ≤560px (or device emulation).
Expected: tabs are still pinned to the **bottom** as before (the mobile nav bar is unchanged); the new top-sticky behavior is absent at this width.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/App.vue
git commit -m "feat(dashboard): pin nav tabs to top on desktop"
```

---

## Task 3: Pin the token legend to the bottom on the Usage view

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.vue` (relocate `<TokenLegend />`)
- Modify: `crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue` (scoped styles)

- [ ] **Step 1: Move `<TokenLegend />` to the end of the Usage view**

In `UsageView.vue`, remove the legend from its current position (the `<TokenLegend />` line directly above `<section class="two-column wide-main">`):

```html
    <TokenLegend />

    <section class="two-column wide-main">
```

becomes:

```html
    <section class="two-column wide-main">
```

Then add `<TokenLegend />` as the **last** element inside the `<AsyncState>` slot, immediately after the closing `</section>` of the `list-stack` block and before `</AsyncState>`:

```html
      <article v-if="(usage?.windows ?? []).length === 0" class="empty-panel">No usage data for period</article>
    </section>

    <TokenLegend />
  </AsyncState>
```

The `import TokenLegend from '../components/charts/TokenLegend.vue'` line stays unchanged (still used).

- [ ] **Step 2: Turn the legend into a sticky bottom bar**

In `TokenLegend.vue`, replace the existing `.token-legend` rule:

```css
.token-legend {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  font-size: 0.72rem;
  color: var(--tg-theme-hint-color, #6b7b88);
  margin: 0 0 4px;
}
```

with:

```css
.token-legend {
  position: sticky;
  bottom: 0;
  z-index: 15;
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  font-size: 0.72rem;
  color: var(--tg-theme-hint-color, #6b7b88);
  margin: 8px -12px 0;
  padding: 8px 12px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  border-top: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
}

@media (max-width: 560px) {
  .token-legend {
    bottom: calc(70px + env(safe-area-inset-bottom));
  }
}
```

Rationale: placed last in flow, `position: sticky; bottom: 0` pins the legend to the viewport bottom while content extends below the fold and settles in place at full scroll — "always at the bottom" without reserved padding or content overlap. `margin: 8px -12px 0` + `padding: 8px 12px` give the same full-bleed bar treatment as the desktop tab bar. On `≤560px` the `bottom` offset lifts it above the fixed mobile nav bar (≈78px reserved by the shell) so they do not overlap. The `.lg*` dot rules below are unchanged.

- [ ] **Step 3: Typecheck and build**

Run: `pnpm typecheck && pnpm build`
Expected: PASS (no errors).

- [ ] **Step 4: Run the component tests (regression guard)**

Run: `pnpm test`
Expected: PASS — `TokenLegend.test.ts` still finds `token-legend` + the four labels (markup unchanged), `UsageBreakdown.test.ts` unaffected.

- [ ] **Step 5: Visual verification (wide viewport, Usage tab)**

Open the Usage tab at width > 560px and scroll the windows list.
Expected: the legend bar stays pinned at the bottom of the viewport with a top separator and solid background; at full scroll it sits flush at the end of the content; it does not overlap the breakdown or window list.

- [ ] **Step 6: Visual verification (mobile, ≤560px, Usage tab)**

Resize to ≤560px on the Usage tab and scroll.
Expected: the legend sits just **above** the fixed bottom nav bar without overlapping it; both remain readable.

- [ ] **Step 7: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/UsageView.vue crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue
git commit -m "feat(dashboard): pin token legend to bottom on Usage view"
```

---

## Task 4: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Frontend suite + build**

Run (from `crates/right-dashboard/frontend/`): `pnpm test && pnpm build`
Expected: PASS (all green, clean build).

- [ ] **Step 2: Workspace regression gate**

Run (from repo root): `devenv shell -- cargo test --workspace`
Expected: PASS. No Rust changed; this is the mandatory end-of-work regression guard per project convention. Record any pre-existing/flaky failures (see the known parallel-load flakes) and re-run isolated if needed.

- [ ] **Step 3: Confirm all checkboxes complete and the two feature commits are present**

Run: `git log --oneline -3`
Expected: shows the legend commit, the tabs commit, and the spec commit.
