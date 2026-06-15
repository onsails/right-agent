# Stage 02 — right-dashboard jewel recolor — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Recolor the `right-dashboard` Vue frontend to a fixed jewel-dark theme
(brand-forward), with `--jewel-*` CSS tokens as the single source of truth, the
Telegram theme vars overridden to jewel (defeating Telegram's inline injection),
recolored semantics, ruby identity, and a legible ECharts dark theme. No
structural change. Spec: `docs/plans/02-dashboard-spec.md`.

**Architecture:** Jewel tokens live in `App.vue :root`. `telegram.ts` gains
`applyJewelTheme()` that re-points `--tg-theme-*` at `var(--jewel-*)` after
Telegram init. Hardcoded semantic hexes are replaced in place; charts merge a
shared jewel option fragment. Tokens are the source of truth; components keep
using `var(--tg-theme-*)` which now resolves to jewel.

**Tech Stack:** Vue 3 SFC, TypeScript, Vite, Vitest (SSR `renderToString`),
vue-echarts/ECharts, **pnpm**.

**Run commands from the frontend dir.** Use:
`devenv shell -- bash -c 'cd crates/right-dashboard/frontend && pnpm <script>'`
(devenv provides node/pnpm). A `prek`/rustfmt pre-commit hook exists — if a
commit reformats files, re-stage and retry; prefix commits with
`PREK_ALLOW_NO_CONFIG=1`. **Do NOT commit per task** — the stage-runner commits
and lands the whole stage after review + verify.

**Jewel reference (hex):** base `#121016` panel `#201a26` line `#2d2533`
line-2 `#3e3146` · ruby `#c75f88` teal `#3bb0c4` gold `#cda14b` · text `#f1ece9`
muted `#b6a8b0` dim `#6f6169` · ok `#6bbf59` warn `#e6c06a` err `#e2556a`
info `#3bb0c4`.

---

### Task 1: Jewel tokens + Telegram-var defaults in `:root`

**Files:** Modify `crates/right-dashboard/frontend/src/App.vue` (the global
`<style>` block, currently starting at the `:root { --token-* }` rule).

**Step 1 — Replace the `:root` block.** Replace the existing
```
:root {
  --token-input: #6b7b88;
  --token-output: #2481cc;
  --token-create: #b87900;
  --token-read: #0d7a45;
}
```
with the full jewel token set + Telegram-var defaults from the spec
("Authoritative palette → jewel tokens" and the stylesheet-defaults list under
"defeat Telegram's inline theme injection"). Include `--jewel-*`, the
`--jewel-*-bg` tints, the jewel `--token-*` palette, and the
`--tg-theme-*: var(--jewel-*)` default mappings (cover bg-color,
secondary-bg-color, text-color, hint-color, **hint_color** underscore variant,
link-color, button_color, section_separator_color).

**Step 2 — Verify it parses.**
Run: `devenv shell -- bash -c 'cd crates/right-dashboard/frontend && pnpm typecheck'`
Expected: clean (CSS isn't typechecked, but this catches SFC breakage).

---

### Task 2: `applyJewelTheme()` in `telegram.ts` (TDD — the only new logic)

**Files:**
- Modify `crates/right-dashboard/frontend/src/telegram.ts`
- Test `crates/right-dashboard/frontend/src/telegram.test.ts` (create if absent;
  else append).

**Step 1 — Write the failing test.**
```ts
import { describe, expect, it, vi } from 'vitest'
import { applyJewelTheme } from './telegram'

describe('applyJewelTheme', () => {
  it('repoints tg-theme vars at jewel tokens', () => {
    const setProperty = vi.fn()
    const root = { style: { setProperty } } as unknown as HTMLElement
    applyJewelTheme(root)
    expect(setProperty).toHaveBeenCalledWith('--tg-theme-bg-color', 'var(--jewel-base)')
    expect(setProperty).toHaveBeenCalledWith('--tg-theme-button_color', 'var(--jewel-teal)')
    expect(setProperty).toHaveBeenCalledWith('--tg-theme-text-color', 'var(--jewel-text)')
    expect(setProperty).toHaveBeenCalledWith('--tg-theme-secondary-bg-color', 'var(--jewel-panel)')
  })
})
```

**Step 2 — Run, verify red.**
Run: `devenv shell -- bash -c 'cd crates/right-dashboard/frontend && pnpm test telegram'`
Expected: FAIL (`applyJewelTheme` not exported).

**Step 3 — Implement.** In `telegram.ts`:
```ts
const JEWEL_THEME_VARS: ReadonlyArray<[string, string]> = [
  ['--tg-theme-bg-color', 'var(--jewel-base)'],
  ['--tg-theme-secondary-bg-color', 'var(--jewel-panel)'],
  ['--tg-theme-text-color', 'var(--jewel-text)'],
  ['--tg-theme-hint-color', 'var(--jewel-muted)'],
  ['--tg-theme-hint_color', 'var(--jewel-muted)'],
  ['--tg-theme-link-color', 'var(--jewel-teal)'],
  ['--tg-theme-button_color', 'var(--jewel-teal)'],
  ['--tg-theme-section_separator_color', 'var(--jewel-line)'],
]

export function applyJewelTheme(
  root: HTMLElement | undefined = typeof document === 'undefined' ? undefined : document.documentElement,
): void {
  if (!root) return
  for (const [name, value] of JEWEL_THEME_VARS) {
    root.style.setProperty(name, value)
  }
}
```

**Step 4 — Wire into init.** In `initializeTelegramWebApp` (telegram.ts), call
`applyJewelTheme()` so it runs after Telegram's own theme is applied, and also
when there is no Telegram WebApp (no early-return before it). If init lives in
App.vue `onMounted`, call `applyJewelTheme()` there right after
`initializeTelegramWebApp(...)`. Pick whichever the code structure makes
cleanest; it MUST run on every load.

**Step 5 — Run, verify green.**
Run: `devenv shell -- bash -c 'cd crates/right-dashboard/frontend && pnpm test telegram'`
Expected: PASS. Also `pnpm typecheck` clean.

---

### Task 3: Recolor App.vue semantic CSS

**Files:** Modify `crates/right-dashboard/frontend/src/App.vue` (`<style>`).

Replace the light semantic fg/bg literals with jewel tokens (use the tokens from
Task 1; do not reintroduce raw hexes):
- `.status-pill.ok` / `.run-delivery-badge.ok` → `color: var(--jewel-ok); background: var(--jewel-ok-bg);`
- `.status-pill.active`/`.warn`, `.run-delivery-badge.active` → `color: var(--jewel-warn); background: var(--jewel-warn-bg);`
  (`.active` fg may use `var(--jewel-gold)` for warmth — keep bg as warn tint).
- `.status-pill.bad` / `.run-delivery-badge.bad` → `color: var(--jewel-err); background: var(--jewel-err-bg);`
- `.metric-card.ok strong` → `var(--jewel-ok)`; `.metric-card.active strong` → `var(--jewel-gold)`; `.metric-card.bad strong` → `var(--jewel-err)`.
- `.status-dot.ok` → `var(--jewel-ok)`; `.status-dot.active` → `var(--jewel-gold)`; `.status-dot.bad` → `var(--jewel-err)`.
- `.cron-delete` `--danger` fallback `#c0392b` → `var(--jewel-err)` (update both the `border` and `color` fallbacks; or define `--danger: var(--jewel-err)` in `:root` and keep references).

**Step — Verify.** `pnpm typecheck` clean; visually confirm no `#0d7a45/#dff5e8/#8a5a00/#fff0c2/#a42323/#ffe1de/#b87900/#b92b27/#c0392b` remain in App.vue:
Run: `rg -n "#0d7a45|#dff5e8|#8a5a00|#fff0c2|#a42323|#ffe1de|#b87900|#b92b27|#c0392b" crates/right-dashboard/frontend/src/App.vue` → no matches.

---

### Task 4: Recolor McpView.vue + ProvidersView.vue semantic rgba

**Files:** Modify
`crates/right-dashboard/frontend/src/views/McpView.vue` and
`crates/right-dashboard/frontend/src/views/ProvidersView.vue`.

Replace the standalone semantic `rgba()` literals (NOT the `--tg-theme-*`
fallbacks) with jewel tints:
- `rgba(25,135,84,0.35)` (green border) → `rgba(107,191,89,0.4)` (jewel ok).
- `rgba(176,42,55,0.35)` (red border) → `rgba(226,85,106,0.4)` (jewel err).
- `rgba(214,165,26,0.14)` / `rgba(214,165,26,0.4)` (gold bg/border) →
  `rgba(205,161,75,0.14)` / `rgba(205,161,75,0.4)` (jewel gold `#cda14b`).
Leave `rgba(84,102,117,…)` separator fallbacks as-is (they sit inside
`var(--tg-theme-section_separator_color, …)` and the JS override wins); or update
them to a jewel line rgba for consistency — optional, not gated.

**Step — Verify.**
Run: `rg -n "rgba\(25,\s*135,\s*84|rgba\(176,\s*42,\s*55|rgba\(214,\s*165,\s*26" crates/right-dashboard/frontend/src` → no matches.

---

### Task 5: Ruby identity in AppShell

**Files:** Modify `crates/right-dashboard/frontend/src/components/AppShell.vue`.

The topbar `<h1>{{ agent }}</h1>` is the brand identity. Give it ruby:
add/extend a scoped style `.topbar h1 { color: var(--jewel-ruby); }` (or a class
on the h1). Keep weight/size unchanged. Do not ruby-tint other text.

**Step — Verify.** `pnpm typecheck` clean; `pnpm test AppShell` green (the SSR
test asserts structure/text, not color — should pass unchanged).

---

### Task 6: ECharts jewel dark theme

**Files:**
- Create `crates/right-dashboard/frontend/src/components/charts/jewelChart.ts`
- Modify each ECharts chart component (find them:
  `rg -l "AsyncVChart|VChart|registerDashboardCharts" crates/right-dashboard/frontend/src/components/charts`).
- Modify `crates/right-dashboard/frontend/src/components/charts/UsageSpendChart.vue`.

**Step 1 — Create the shared base.** `jewelChart.ts`:
```ts
export const JEWEL_CHART_PALETTE = ['#3bb0c4', '#c75f88', '#cda14b', '#6bbf59', '#b6a8b0', '#e6c06a']

/** Option fragment merged into every dashboard ECharts option for jewel-dark legibility. */
export const jewelChartBase = {
  backgroundColor: 'transparent',
  color: JEWEL_CHART_PALETTE,
  textStyle: { color: '#b6a8b0' },
  title: { textStyle: { color: '#f1ece9' } },
  legend: { textStyle: { color: '#b6a8b0' } },
  tooltip: {
    backgroundColor: '#201a26',
    borderColor: '#2d2533',
    textStyle: { color: '#f1ece9' },
  },
  categoryAxis: { axisLine: { lineStyle: { color: '#2d2533' } }, axisLabel: { color: '#b6a8b0' }, splitLine: { lineStyle: { color: '#2d2533' } } },
  valueAxis: { axisLine: { lineStyle: { color: '#2d2533' } }, axisLabel: { color: '#b6a8b0' }, splitLine: { lineStyle: { color: '#2d2533' } } },
} as const
```

**Step 2 — Apply to each chart's `option` computed.** For every ECharts chart,
spread `jewelChartBase` first and let the chart's own option override structure:
`const option = computed(() => ({ ...jewelChartBase, /* existing option */ }))`,
and for axis-bearing charts, fold the axis defaults into the chart's `xAxis`/
`yAxis`/`tooltip`/`legend` (merge, do not drop the chart's own axis `data`/
`formatter`). Keep each chart's existing series/data untouched. Prefer a shallow
deep-merge by hand per chart (these option objects are small) over a generic
merge util — be careful not to clobber `tooltip.formatter`, `xAxis.data`,
`legend.type`.

**Step 3 — Fix the selected-bar highlight.** In `UsageSpendChart.vue`,
`borderColor: '#111827'` → `borderColor: '#f1ece9'` (visible on dark).

**Step 4 — Verify.**
Run: `devenv shell -- bash -c 'cd crates/right-dashboard/frontend && pnpm test'`
Expected: all green. `rg -n "#111827|#2481cc" crates/right-dashboard/frontend/src` → no matches.

---

### Task 7: Final stage verification (also re-run by stage-runner)

From `crates/right-dashboard/frontend` (via the devenv wrapper):
- `pnpm install --frozen-lockfile` (only if `node_modules` missing).
- `pnpm typecheck` → clean.
- `pnpm test` → all green.
- `pnpm build` → succeeds.
- **Grep gate** (repo root) — none of these standalone literals remain in
  `crates/right-dashboard/frontend/src`:
  `rg -n "#2481cc|#0d7a45|#dff5e8|#8a5a00|#fff0c2|#a42323|#ffe1de|#b87900|#b92b27|#c0392b|#111827" crates/right-dashboard/frontend/src`
  Expected: no matches.

---

## Stage completion gate (handled by stage-runner)
- typecheck + test + build green, grep gate empty.
- Code review (`/code-review max --fix`) clean or all findings resolved.
- Commit on `$BR`, merge `--no-ff` into `claude/strange-borg-9c9f27`, remove
  worktree + delete branch.
- The mandatory **full-workspace** test (`cargo nextest run --workspace` +
  `cargo test --doc --workspace`) is the sprint's final gate, run by the
  conductor AFTER this stage lands.
