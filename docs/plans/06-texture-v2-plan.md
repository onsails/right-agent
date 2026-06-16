# Stage 06 — Texture experiment v2 + remove bg parallax (plan)

Implements `06-texture-v2-spec.md`. (1) Remove background parallax (static bg); (2) remove `fiber`; (3) strengthen cardstock/halftone/crosshatch; (4) add concrete/canvas/grunge/speckle. 7 textures × 2 tones behind the existing `PAPER-EXPERIMENT` switcher. Astro site (`site/`), vanilla CSS + SVG + existing inline `<script>`, no deps. Keep active-section zoom / reveal / spotlight / learning / telemetry.

## Baseline
- From the worktree: `cd site && (npm ci if node_modules absent) && npm run build` → confirm green before edits.

## Task 1 — Remove background parallax
- `site/src/layouts/Landing.astro` `<script>`: delete the `mousemove` listener and the `.fx[data-depth]` translate block in `tick()`; remove `mx/my/tmx/tmy/sy` (and the `scroll` listener if only used for parallax). KEEP the active-section-zoom branch and the rAF that drives it; KEEP `.rev` IntersectionObserver and `.card` spotlight.
- Remove `data-depth` attributes from background layers (telemetry etc.).
- Grep after: `rg -n "mousemove|data-depth" site/` → empty. Confirm no dangling references to removed vars.

## Task 2 — Remove fiber
- `Landing.astro`: remove the fiber `feTurbulence` filter def and the `fiber` switcher chip; set `<html data-tex="cardstock">` default.
- `landing.css`: remove `:root[data-tex="fiber"]` rule.
- JS: default `localStorage['tex']` fallback → `cardstock`.

## Task 3 — Strengthen kept textures (`landing.css`)
- `cardstock` / `halftone` / `crosshatch`: raise opacity (~.12–.22), contrast, and/or density so each clearly reads on `--bg`; adjust `mix-blend-mode` (overlay/soft-light) as needed. Keep legible.

## Task 4 — Add 4 textures (`Landing.astro` defs + `landing.css`, `:root[data-tex=…]`)
- `concrete` — `feTurbulence` fractalNoise low baseFrequency (~0.012–0.05), few octaves → large mottled stone cloudiness; medium opacity.
- `canvas` — two perpendicular fine `repeating-linear-gradient` gratings (H+V), small period, slight offset/alpha → woven linen.
- `grunge` — `feTurbulence` higher octaves + `feComponentTransfer`/`feColorMatrix` to blotchy high-contrast alpha (optional faint rotated scratch lines). Heaviest.
- `speckle` — discrete flecks via `feTurbulence` + `feComponentTransfer` thresholding, or tiled multi-`radial-gradient` fleck field; varied speck sizes.
- All STATIC — no animation/keyframes on `.paper`.

## Task 5 — Switcher (extend, still `PAPER-EXPERIMENT`-delimited)
- `Landing.astro`: texture row now 7 chips (cardstock, halftone, crosshatch, concrete, canvas, grunge, speckle); allow wrap. Tone row unchanged (dark, warm).
- JS: wire the new chips (same `data-tex-choice` handler); default `cardstock`. No new loops; switcher is event-driven.
- `landing.css`: switcher styles accommodate the wider texture row.

## Task 6 — Tone
- `dark`/`warm` unchanged (warm = warm `color-mix`, still dark). Each texture composes with both; verify legibility across all 14 combos, esp. heavy textures over warm.

## Verify (in the worktree)
- `cd site && npm run build` → succeeds; `npx astro check` → 0 errors.
- Grep gates: `rg -n 'data-tex="fiber"|mousemove|data-depth' site/` → EMPTY; `rg -n "PAPER-EXPERIMENT" site/` present; telemetry + active-section logic present.
- Legibility: reason/spot-check H1/sub/cards/code over the 14 texture×tone combos.

## Notes for executor
- Model: openai/gpt-5.4 / max (pinned). Mint a fresh mimo handle for this stage.
- Keep each texture's filter/CSS grouped + `PAPER-EXPERIMENT`-delimited for stage-07 lock-in removal.
- Bolder is the point — fiber failed by being too subtle on dark; make every texture obviously visible while keeping text legible.
- `prek` hook may abort on missing `.pre-commit-config.yaml` → `PREK_ALLOW_NO_CONFIG=1`.
- Do NOT remove cardstock/halftone/crosshatch or any tone — only fiber + parallax go. Textures STATIC.
