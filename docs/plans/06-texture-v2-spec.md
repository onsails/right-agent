# Stage 06 — Texture experiment v2 + remove bg parallax (spec)

## Goal
Iterate on the stage-05 paper-texture experiment per user feedback:
1. **Remove the background parallax** — the pointer/scroll drift of background layers reads as a dated (≈2014) effect. Background becomes **static**.
2. **Remove the `fiber` texture** — too subtle to read on the dark base.
3. **Keep `cardstock` / `halftone` / `crosshatch`** but make them clearly stronger (the fiber lesson: on the dark base, textures must be bolder — raise opacity/contrast/density so the tooth genuinely reads).
4. **Add 4 heavier textures:** `concrete` (mottled stone/marble), `canvas` (woven linen), `grunge` (distress), `speckle` (kraft flecks).

Net texture set behind the existing `PAPER-EXPERIMENT` switcher: **7 textures × 2 tones (dark/warm)**. Still an experiment (user compares, picks later → stage 07 lock-in). Everything **static — no animation**.

Astro site (`site/`). Vanilla CSS + SVG filters + the existing inline `<script>`; no new deps/assets/webfonts; jewel custom properties only (no new hexes).

## Part 1 — remove background parallax (make bg static)
- `Landing.astro` `<script>`: remove the `mousemove` handler and the per-layer translate block inside the rAF `tick()` (the `el.style.transform = translate3d(... mx/my/sy ...)` for `.fx[data-depth]`). Remove `mx/my/tmx/tmy/sy` plumbing used only for parallax.
- Remove `data-depth` attributes from background layers (telemetry layers etc.) — they no longer move.
- **KEEP** the active-section zoom logic (stage 03) — it lives in the same rAF; preserve it (keep the rAF running for it). KEEP `.rev` reveal, `.card` spotlight, `learning`, and the telemetry layers' own faint flicker (that's not parallax).
- Result: background is visually static; only the active-section zoom + reveal + telemetry flicker remain as motion. Verify nothing references removed bindings.

## Part 2 — texture set changes
- **Remove `fiber`:** drop its `feTurbulence` filter, `:root[data-tex="fiber"]` CSS, and the `fiber` switcher chip. Change the default `data-tex` to `cardstock`.
- **Strengthen kept three** so they clearly read on `--bg` (#121016): raise opacity (roughly into ~.12–.22 territory), contrast, and/or pattern density; tune `mix-blend-mode` (overlay/soft-light) so the texture is obvious without harming legibility.
- **Add four (static, keyed off `:root[data-tex=…]`):**
  - `concrete` — coarse low-frequency `feTurbulence` (fractalNoise, large mottled blobs, e.g. baseFrequency ~0.012–0.05) + medium opacity → stone/stucco/marble cloudiness. Clearly visible.
  - `canvas` — woven linen: two perpendicular fine `repeating-linear-gradient` gratings (H + V) at a small period, slightly offset/alpha to mimic over-under weave. Visible fabric tooth.
  - `grunge` — heavy distress: `feTurbulence` higher octaves + `feComponentTransfer`/`feColorMatrix` to blotchy high-contrast alpha (optionally a few faint rotated scratch lines). The heaviest/"dirtiest". Static.
  - `speckle` — recycled kraft flecks: discrete specks — e.g. `feTurbulence` + `feComponentTransfer` thresholding to scattered dots of varied size, or a tiled multi-`radial-gradient` fleck field. Visibly speckled.

## Switcher
- The existing `PAPER-EXPERIMENT` switcher (bottom-left) now lists **7 texture chips** (cardstock, halftone, crosshatch, concrete, canvas, grunge, speckle) + the **2 tone chips** (dark, warm). Let the texture row wrap. Keep localStorage persistence (`tex` default now `cardstock`, `tone` default `dark`); always visible.

## Tone
- `dark` / `warm` unchanged from stage 05 (warm = warm kraft `color-mix`, still dark, no inversion). Each texture composes with both tones; verify all 14 combos keep H1/sub/cards/code legible — especially the heavier textures over `warm`.

## Verification
- `cd site && npm run build` succeeds; `npx astro check` 0 errors.
- Grep gates: `rg -n "data-tex=\"fiber\"|mousemove|data-depth" site/` → EMPTY (fiber gone, parallax gone); `rg -n "PAPER-EXPERIMENT" site/` present; telemetry layers present; active-section zoom logic present.
- Preview: background no longer drifts on mouse/scroll; switcher offers 7 textures × 2 tones; every texture is clearly visible/textured; text legible over all combos; section-zoom + reveal still work.

## Out of scope
- Locking in a final texture / removing the switcher (stage 07).
- Animating textures or re-adding parallax.
- Light-theme inversion; new webfonts.
