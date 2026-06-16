# Stage 05 — Lock telemetry + paper-texture experiment (spec)

## Goal
Two things on the Astro site (`site/`):

1. **Lock in the telemetry background.** From stage 04 the user picked **B · mono telemetry**. Remove the other three concepts (A wordmark, C manifesto, D blueprint) and the old `BG-EXPERIMENT` switcher; make the telemetry field the **unconditional** background typography (no `data-bg` gate). Keep telemetry exactly as built ("telemetry is fine"), including its faint flicker.
2. **Add a heavy static paper texture** over the dark base so the typography feels printed/tactile. This is a NEW experiment: ship **4 texture types × 2 tones** behind a new switcher so the user compares and later picks (stage 06 locks it in). Everything **static — no animation**.

Vanilla CSS + SVG filters + the existing inline `<script>`; no new dependencies, no external image assets, no new webfonts. Existing jewel custom properties only (no new hexes; warm tone derived via `color-mix`).

## Part 1 — lock telemetry, remove the rest
- `Landing.astro`: remove the wordmark / manifesto / blueprint `.bgfx` layer markup; keep the telemetry `.bgfx` layers. Remove the `BG-EXPERIMENT` switcher markup + its JS init and the `data-bg` attribute on `<html>`.
- `landing.css`: remove the `:root[data-bg="a"|"c"|"d"]` variant CSS, the bg switcher CSS, and any `data-bg` gating on the telemetry layers (telemetry shows unconditionally). Remove the blueprint-numeral rAF branch in the script if it was only for variant D. Keep the shared rAF (parallax + active-section) intact.
- Remove only what these deletions orphan; don't touch unrelated code. Grep `data-bg`/`bgswitch`/`wordmark`/`manifesto`/`blueprint` → empty after.

## Part 2 — paper-texture experiment (PAPER-EXPERIMENT scaffolding)
A full-bleed, fixed, low-opacity **paper texture layer** sits over the dark base, under the page content and under/with the telemetry type, so the whole page reads as ink on textured paper. Driven by two attributes on `<html>`: `data-tex` (which texture) and `data-tone` (paper tone). Both persisted to `localStorage`, restored on load (defaults `data-tex="fiber"`, `data-tone="dark"`).

### Switcher (temporary, `PAPER-EXPERIMENT`-delimited)
- Fixed control, bottom-LEFT, mono/jewel styled. Two labeled rows:
  - **texture:** `fiber` · `cardstock` · `halftone` · `crosshatch`
  - **tone:** `dark` · `warm`
- Clicking a texture sets `data-tex`; clicking a tone sets `data-tone`; persist both; `aria-pressed` on the active chips.
- **Always visible** (textures are static, so reduced-motion is irrelevant; keep the switcher reachable regardless).
- All scaffolding wrapped in `PAPER-EXPERIMENT:start/end` (markup, JS, CSS), namespaced `paper*`/`data-tex`/`data-tone`, for clean stage-06 removal.

### Textures (static; keyed off `:root[data-tex=…]`)
All very textured but low-opacity so H1/sub/cards/code stay fully legible. Prefer `mix-blend-mode: soft-light`/`overlay` to sit naturally on the dark base.
- **fiber** — SVG `feTurbulence type="fractalNoise"` high frequency (≈0.8–1.0), 2 octaves, desaturated to alpha → fine paper fibers. Subtle.
- **cardstock** — coarser turbulence (≈0.35–0.5), higher contrast → rough heavy stock tooth, slightly stronger opacity.
- **halftone** — CSS dot raster: `radial-gradient` dot repeated on a small `background-size` grid (≈4–6px), very low opacity; optionally two slightly offset grids for a CMYK/newsprint feel. The most "print/typographic" texture.
- **crosshatch** — fine `repeating-linear-gradient` hatching (e.g. +45°/−45° crossed) + a faint inset emboss → engraving/letterpress paper.

### Tone (keyed off `:root[data-tone=…]`)
- **dark** — texture over the current plum base; neutral/cool tint. Theme unchanged.
- **warm** — shift the paper toward a warm kraft/parchment: a warm overlay tint (e.g. `color-mix(in srgb, var(--bg), <warm gold/brown> X%)`), slightly lighter/warmer but STILL dark enough that the existing light text stays legible. Do NOT invert to a light theme. Verify contrast of H1/sub/body over the warm tone.

## Relationship to existing layers
- Keep `.void` (base), `.scan`. The new paper texture supersedes the old subtle `.grain` — hide/remove `.grain` when a texture is active (or fold it in) to avoid muddiness; executor's judgment, legibility wins.
- Keep `.depthbloom`/`.nebula`/`.horizon*` ambient unless they clash with a texture; dim if needed.
- Must NOT break: telemetry type, active-section zoom (stage 03), `.rev` reveal, `.card` spotlight, `learning` indicator, parallax.

## Verification
- `cd site && npm run build` succeeds; `npx astro check` 0 errors.
- Grep gates: `rg -n "data-bg|bgswitch|bgfx-wordmark|bgfx-manifesto|bgfx-blueprint" site/` → EMPTY; `rg -n "PAPER-EXPERIMENT" site/` → present (delimited). Telemetry layers still present.
- Preview: switcher (bottom-left) flips 4 textures × 2 tones = 8 combos; texture is clearly felt; warm vs dark visibly differ; text legible over every combo; telemetry typography + zoom + reveal still work.

## Out of scope
- Removing the texture switcher / other textures (stage 06, after the pick).
- Animating the texture (must stay static).
- Light-theme inversion; new webfonts.
