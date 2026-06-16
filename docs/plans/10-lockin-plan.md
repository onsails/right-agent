# Stage 10 — Lock-in final background (plan)

Implements `10-lockin-spec.md`. Bake texture=crosshatch, tone=plum, accent=champagne; delete the `PAPER-EXPERIMENT` switcher + all unused candidate code. Astro site (`site/`), CSS/markup + existing inline `<script>`, no deps. Website-only — NO cargo.

## Baseline
- `cd site && (npm ci if node_modules absent) && npm run build` → green before edits.

## Task 1 — Bake the chosen values
- `landing.css`:
  - Make the `crosshatch` texture rules apply unconditionally to `.paper` (drop the `:root[data-tex="crosshatch"]` gate; inline its declarations onto `.paper`/`.paper::before` as appropriate). Delete all other `:root[data-tex=…]` texture rules.
  - Make the `plum` `--paper-tint` unconditional (drop the `:root[data-tone="plum"]` gate). Delete other `:root[data-tone=…]` rules.
  - Set `--warm` and `--warm-soft` in `:root` to the `champagne` hex values; delete all `:root[data-accent=…]` rules.
- `Landing.astro`: remove `data-tex`/`data-tone`/`data-accent` from `<html>`. Remove the now-unused SVG filter `<defs>` (feTurbulence/lighting) that only served removed textures (crosshatch is pure CSS).

## Task 2 — Remove the switcher
- `Landing.astro`: delete the `PAPER-EXPERIMENT` switcher markup block and its JS init (button arrays, `data-*-choice` handlers, localStorage `tex`/`tone`/`accent` restore/set). Remove all `PAPER-EXPERIMENT:start/end` blocks.
- `landing.css`: delete the switcher CSS (`.paper-chip`, switcher panel, rows) inside the `PAPER-EXPERIMENT` delimiters.
- Remove only JS bindings orphaned by this; keep the shared rAF (active-section) + reveal + spotlight.

## Verify (in the worktree)
- `cd site && npm run build` → succeeds; `npx astro check` → 0 errors. NO cargo.
- Grep gates (MUST be empty): `rg -n "PAPER-EXPERIMENT|data-tex|data-tone|data-accent|paper-chip|data-.*-choice" site/` → no matches.
- Present: crosshatch texture on `.paper`, plum tint, champagne `--warm`/`--warm-soft` in `:root`, telemetry + active-section logic.

## Notes for executor
- Model: openai/gpt-5.4 / max (pinned). Mint a fresh mimo handle.
- Pull the exact `crosshatch` declarations, `plum` `--paper-tint`, and `champagne` `--warm`/`--warm-soft` values from the current (stage-09) `landing.css` before deleting siblings, so the baked-in values match what the user saw.
- `prek` hook may abort on missing `.pre-commit-config.yaml` → `PREK_ALLOW_NO_CONFIG=1`.
- Do not alter the chosen look — this is a pure lock-in/cleanup. Keep telemetry/zoom/reveal/learning/flat-bg intact.
