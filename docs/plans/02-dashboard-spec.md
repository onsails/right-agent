# Stage 02 — right-dashboard (Telegram Mini App) → Observatory/jewel — Spec

## Goal
Recolor the `right-dashboard` Vue frontend to a **fixed jewel dark** theme
(brand-forward, decided), matching `docs/brand-guidelines.html` (v2). The
dashboard must always render the dark plum jewel palette regardless of the
user's Telegram light/dark theme. Structure, components, and the
`AsyncState`/`CollapsibleSection` primitives are unchanged — this is color only.

Frontend root: `crates/right-dashboard/frontend`. Package manager: **pnpm**.

## Authoritative palette → jewel tokens (source of truth)
Define once in `App.vue`'s global `<style>` `:root`:
```
--jewel-base:  #121016;  /* page background (plum) */
--jewel-panel: #201a26;  /* cards / secondary bg */
--jewel-line:  #2d2533;  /* borders / separators */
--jewel-line-2:#3e3146;  /* stronger border */
--jewel-ruby:  #c75f88;  /* IDENTITY — agent name / brand mark */
--jewel-teal:  #3bb0c4;  /* ACTION — accent, links, active tab, selection */
--jewel-gold:  #cda14b;  /* warmth / secondary highlight */
--jewel-text:  #f1ece9;  /* primary text */
--jewel-muted: #b6a8b0;  /* hint / secondary text */
--jewel-dim:   #6f6169;  /* faint text */
--jewel-ok:    #6bbf59;  --jewel-warn: #e6c06a;  --jewel-err: #e2556a;  --jewel-info: #3bb0c4;
/* dark semantic tints for pill/badge backgrounds (translucent over panel) */
--jewel-ok-bg:   rgba(107,191,89,0.16);
--jewel-warn-bg: rgba(230,192,106,0.16);
--jewel-err-bg:  rgba(226,85,106,0.16);
/* chart token palette (distinct hues on dark) */
--token-input:  #b6a8b0;  --token-output: #3bb0c4;  --token-create: #cda14b;  --token-read: #6bbf59;
```

## The core problem: defeat Telegram's inline theme injection
The dashboard reads `var(--tg-theme-*, <fallback>)` everywhere. Telegram injects
`--tg-theme-*` as **inline styles on `document.documentElement`**, which beat any
stylesheet `:root`. Two-layer fix so the jewel theme always wins and there is no
light flash:

1. **Stylesheet defaults** — in `:root`, after the `--jewel-*` tokens, set the
   Telegram vars the app uses to jewel:
   ```
   --tg-theme-bg-color: var(--jewel-base);
   --tg-theme-secondary-bg-color: var(--jewel-panel);
   --tg-theme-text-color: var(--jewel-text);
   --tg-theme-hint-color: var(--jewel-muted);
   --tg-theme-hint_color: var(--jewel-muted);     /* Spinner.vue uses underscore form */
   --tg-theme-link-color: var(--jewel-teal);
   --tg-theme-button_color: var(--jewel-teal);
   --tg-theme-section_separator_color: var(--jewel-line);
   ```
2. **JS override after Telegram init** — add `applyJewelTheme(root?: HTMLElement)`
   to `telegram.ts` that calls `documentElement.style.setProperty('--tg-theme-…','var(--jewel-…)')`
   for each of the vars above. Call it from `initializeTelegramWebApp` (and ensure
   it runs even when there is no Telegram WebApp). This re-applies jewel *after*
   Telegram's inline injection, so the user's Telegram theme never leaks through.

Net effect: existing component CSS keeps using `var(--tg-theme-*)`, but those now
resolve to jewel — minimal structural churn, guaranteed brand look. The
`--jewel-*` tokens remain the single source of truth.

## Hardcoded hexes to replace (recolor in place)
Replace every literal below (full inventory — verified by grep) with jewel
tokens. None are `--tg-theme-*`, so they need direct edits.
- **App.vue** `:root` token block (`--token-*`) → jewel chart palette above.
- **App.vue** status semantics — `.status-pill`, `.metric-card`, `.status-dot`,
  `.run-delivery-badge` (`.ok/.active/.warn/.bad`): replace the light fg/bg pairs
  (`#0d7a45/#dff5e8`, `#8a5a00/#fff0c2`, `#a42323/#ffe1de`, `#b87900`, `#b92b27`)
  with jewel: ok→`--jewel-ok`/`--jewel-ok-bg`, warn+active→`--jewel-warn`/`--jewel-warn-bg`
  (active may use `--jewel-gold` fg for warmth), bad→`--jewel-err`/`--jewel-err-bg`.
- **App.vue** `--danger` fallback `#c0392b` → `--jewel-err`.
- **McpView.vue / ProvidersView.vue** semantic `rgba(...)` backgrounds/borders
  (`rgba(25,135,84,…)` green, `rgba(176,42,55,…)` red, `rgba(214,165,26,…)` gold)
  → jewel ok / err / gold tints.
- **UsageSpendChart.vue** `borderColor: '#111827'` (selected-bar highlight, an
  near-black invisible on plum) → a light/teal highlight, e.g. `#f1ece9` or
  `#3bb0c4`.

## ECharts dark theming (charts must be legible on plum)
Charts currently use ECharts' default light palette + dark axis text → unreadable
on jewel-dark. Add a shared jewel chart base and apply to every ECharts chart
(those rendering through `AsyncVChart`/`VChart` — enumerate via grep):
- `backgroundColor: 'transparent'`
- `textStyle.color` → `#f1ece9`; axis line/label/splitLine → `#2d2533` / `#b6a8b0`
- tooltip: bg `#201a26`, border `#2d2533`, text `#f1ece9`
- series color palette: `['#3bb0c4','#c75f88','#cda14b','#6bbf59','#b6a8b0','#e6c06a']`
Implement as a small exported helper (e.g. `src/components/charts/jewelChart.ts`)
merged into each chart's `option` computed (robust regardless of the
`AsyncVChart` wrapper). Register-theme is acceptable only if the wrapper forwards
`theme`; otherwise merge into option.

## Identity (ruby)
The agent name is the brand identity element: `AppShell.vue` `<h1>{{ agent }}</h1>`
in the topbar → ruby (`var(--jewel-ruby)`). Accent/action (active tab, selected
row, links) → teal (already covered via `--tg-theme-button_color` → teal). Do not
ruby-tint anything else; identity stays scarce.

## Out of scope
- No layout/structure/markup changes, no component additions beyond the chart
  theme helper and the `applyJewelTheme` function.
- No changes to data flow, API, or the `right_ui::*`/Rust side.
- Light-theme support is intentionally dropped (fixed dark, per decision).

## Verification criteria (from `crates/right-dashboard/frontend`)
- `pnpm install --frozen-lockfile` (if deps not present).
- `pnpm typecheck` clean (`vue-tsc --noEmit`).
- `pnpm test` green (Vitest SSR). No existing test asserts colors (verified), so
  none should break; if any do, fix the assertion to the jewel value.
- `pnpm build` succeeds (`vue-tsc && vite build`).
- Grep gate — these must NOT remain anywhere in `src` (replaced by jewel):
  `#2481cc` (TG blue), `#0d7a45`, `#dff5e8`, `#8a5a00`, `#fff0c2`, `#a42323`,
  `#ffe1de`, `#b87900`, `#b92b27`, `#c0392b`, `#111827`, `#6b7b88` (as a
  color literal). Telegram light fallbacks inside `var(--tg-theme-x, …)` may be
  updated to jewel or left (JS override wins); the grep gate targets the
  standalone semantic/accent literals above.
- Dashboard frontend-primitive rule (AGENTS.md): `AsyncState`/`CollapsibleSection`
  usage unchanged; no raw placeholder text introduced.

## TDD note
This is presentational CSS/JS with SSR tests that assert structure, not color, so
strict red/green per color is impractical. Write ONE new unit test for
`applyJewelTheme` (the only new logic): given a stub `documentElement`/root with a
`style.setProperty` spy, assert it sets `--tg-theme-bg-color` to `var(--jewel-base)`
etc. Everything else is verified by typecheck + build + the grep gate.
