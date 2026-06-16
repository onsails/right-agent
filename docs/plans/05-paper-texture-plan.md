# Stage 05 — Lock telemetry + paper-texture experiment (plan)

Implements `05-paper-texture-spec.md`. (1) Lock the telemetry background, remove A/C/D + the old switcher; (2) add a heavy STATIC paper texture (4 textures × 2 tones) behind a new `PAPER-EXPERIMENT` switcher. Astro site (`site/`), vanilla CSS + SVG filters + existing inline `<script>`, no deps/assets/webfonts. Don't break telemetry / active-section zoom / reveal / spotlight / learning / parallax.

## Baseline
- From the worktree: `cd site && (npm ci if node_modules absent) && npm run build` → confirm green before edits.

## Task 1 — Lock telemetry, remove A/C/D + bg switcher
- `site/src/layouts/Landing.astro`: delete the `.bgfx-wordmark*`, `.bgfx-manifesto*`, `.bgfx-blueprint*` layer markup; keep `.bgfx-telemetry*`. Delete the `BG-EXPERIMENT` switcher markup + JS init; remove `data-bg` from `<html>`.
- `site/src/styles/landing.css`: remove `:root[data-bg="a"|"c"|"d"]` blocks, the bg switcher CSS, and `data-bg` gating on telemetry (it renders unconditionally). Remove the blueprint-numeral rAF branch if it existed only for D.
- Keep the shared rAF (parallax + active-section). Remove orphaned JS bindings only.
- Grep after: `rg -n "data-bg|bgswitch|bgfx-wordmark|bgfx-manifesto|bgfx-blueprint" site/` → empty.

## Task 2 — Paper texture layer + SVG filters
- `Landing.astro`: add a `PAPER-EXPERIMENT:start/end` full-bleed fixed layer (e.g. `.paper`) over `.void`, under content. Include an inline SVG `<defs>` with `feTurbulence` filters for `fiber` and `cardstock` (hidden `<svg width=0 height=0>`).
- The layer's texture is selected by `:root[data-tex]`; halftone/crosshatch are pure CSS, fiber/cardstock use the SVG filters (or data-URI turbulence backgrounds).

## Task 3 — Texture variants (CSS, `:root[data-tex=…]`)
- `fiber`: feTurbulence fractalNoise ~0.85 / 2 octaves, desaturated, opacity ~.06–.10, `mix-blend-mode: soft-light`.
- `cardstock`: feTurbulence ~0.4, higher contrast, opacity ~.10–.16, soft-light/overlay.
- `halftone`: `radial-gradient` dot, `background-size` ~5px (optionally a 2nd offset grid), opacity ~.06–.10.
- `crosshatch`: crossed `repeating-linear-gradient` (+45°/−45°) fine lines + faint inset emboss, low opacity.
- All static — NO animation. Low opacity; legibility wins.

## Task 4 — Tone (CSS, `:root[data-tone=…]`)
- `dark` (default): neutral tint over plum base; theme unchanged.
- `warm`: warm kraft/parchment overlay via `color-mix(in srgb, var(--bg), <warm> X%)` — slightly lighter/warmer, still dark; verify H1/sub/body legibility. No light-theme inversion.
- Tone composes with any texture (tone tints the paper; texture adds tooth).

## Task 5 — Switcher (temp `PAPER-EXPERIMENT` scaffolding)
- `Landing.astro`: fixed bottom-LEFT control, two rows — texture (fiber/cardstock/halftone/crosshatch) and tone (dark/warm) — `data-tex-choice` / `data-tone-choice` chips.
- JS (existing `<script>`, delimited): restore `localStorage['tex']` (default `fiber`) + `localStorage['tone']` (default `dark`) → set `document.documentElement.dataset.tex/.tone`; wire clicks; set `aria-pressed`. Always visible (no reduced-motion gate).
- `landing.css`: switcher styles via jewel custom properties; namespace `paper*`/`data-tex`/`data-tone`.

## Task 6 — Integrate with existing layers
- The paper texture supersedes the subtle `.grain`: hide `.grain` while a texture is active (or remove it), to avoid muddiness — legibility/clarity wins.
- Dim `.depthbloom`/`.nebula`/`.horizon*` only if a texture clashes; otherwise leave.
- Keep one shared rAF; no new loops (textures are static, no JS animation).

## Verify (in the worktree)
- `cd site && npm run build` → succeeds; `npx astro check` → 0 errors.
- Grep gates: A/C/D + bg switcher gone (Task 1 grep empty); `rg -n "PAPER-EXPERIMENT" site/` present; telemetry layers still present.
- Legibility: reason/spot-check H1/sub/cards/code over all 8 texture×tone combos.

## Notes for executor
- Model: openai/gpt-5.4 / max (pinned). Mint a fresh mimo handle for this stage.
- Keep texture/tone CSS + switcher grouped and `PAPER-EXPERIMENT`-delimited so stage 06 removes the losers + switcher cleanly.
- SVG `feTurbulence` is a static one-time paint — fine for perf; keep tile sizes reasonable.
- `prek` hook may abort on missing `.pre-commit-config.yaml` → `PREK_ALLOW_NO_CONFIG=1`.
- Do NOT remove any texture/tone — the comparison is the deliverable. Textures must be STATIC.
