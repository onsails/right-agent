# Stage 07 — Texture experiment v3: kill bg motion, +10 tactile textures, color palette (spec)

## Goal
Iterate the paper-texture experiment again, per user feedback:
1. **Remove the remaining background motion** — the descending top highlight (`.scan` sweep) is unwanted; the perspective grid (`.ceiling`/`.horizon` `flow`) also drifts. Make the background **fully static**.
2. **Add 10 more tactile textures** — rough paper "you could touch," with relief/effects (emboss/deboss/grit), not flat noise.
3. **Add a color palette** — expand the tone toggle from `dark`/`warm` to several color variants.

Net: ~17 textures × ~7 color tones behind the existing `PAPER-EXPERIMENT` switcher. Still an experiment (user compares, picks later → stage 08 lock-in). Everything **static — no animation on the background**.

Astro site (`site/`). Vanilla CSS + SVG filters + the existing inline `<script>`; no new deps/assets/webfonts; **jewel custom properties only — no new hexes** (derive color tones via `color-mix` of existing tokens).

## Part 1 — kill background motion
- Remove `.scan` entirely: the `<div class="fx scan">` in `Landing.astro`, the `.scan` CSS rule, the `@keyframes sweep`, and `.scan` from the reduced-motion `animation:none` list.
- Freeze the perspective grid: remove the `animation: flow …` from `.ceiling` and `.horizon` (keep them static, or drop the `@keyframes flow` if then unused). Keep `.ceiling`/`.horizon`/`.horizon-glow` as static elements (do NOT delete them) unless they visibly clash with the textures — if they clash, dim them.
- Keep: `.void` base, the static `.paper` texture, `.depthbloom`/`.nebula` (static gradients), the telemetry layers + their approved faint flicker (`bgtelemFlicker` stays — user said "telemetry is fine"). Content animations (claw, status dots, diagram packet, learning fill, card spotlight) are NOT background — leave them.
- After: nothing in the background moves except the telemetry flicker. Grep `@keyframes sweep` / `.scan` → gone.

## Part 2 — 10 new tactile textures
Each must read as a physical, touchable rough surface — use `mix-blend-mode` (overlay/soft-light/hard-light) PLUS a tactile relief cue (a paired highlight+shadow / emboss / inset deboss, e.g. layered offset gradients or `feDiffuseLighting`/`feSpecularLighting` on turbulence) so it feels raised/pressed, not a flat overlay. Bolder than the v1 fiber failure — clearly visible on the dark base while keeping text legible.

Add (keyed off `:root[data-tex=…]`, all STATIC):
1. `kraft` — coarse kraft fiber + soft emboss.
2. `coldpress` — watercolor cold-press tooth (turbulence + bump relief).
3. `letterpress` — debossed relief (inset shadow) over fine grain.
4. `corrugated` — cardboard ridges (repeating ridges + directional shadow).
5. `deckle` — handmade fibrous mottle, soft deckled feel.
6. `sandpaper` — dense coarse grit (high-freq turbulence, higher opacity).
7. `burlap` — coarse woven hessian (heavy double weave + shadow).
8. `slate` — layered mineral striations + subtle sheen.
9. `leather` — pebbled grain (cellular turbulence).
10. `cork` — granular speckle (medium flecks + emboss).

(These join the existing 7: cardstock, halftone, crosshatch, concrete, canvas, grunge, speckle.)

## Part 3 — color palette (expand `data-tone`)
Replace the 2-value `data-tone` (dark/warm) with a palette of ~7, each a tinted-dark paper that keeps the existing light text legible (NO light-theme inversion). **Derive every tone via `color-mix(in srgb, var(--bg), <existing jewel token> X%)` — no new hexes.** Suggested set (map to available tokens — plum base, gold/warm, teal/cyan, ruby/accent, ok-green, info):
- `plum` (base, default) · `kraft` (warm/gold) · `slate` (teal/cool) · `sepia` (warm, browner) · `olive` (ok-green) · `wine` (ruby/accent) · `ink` (deep cool/info).
- The tone tints the paper base + the texture's tint; texture choice and tone compose independently.
- Keep `dark`→`plum` and `warm`→`kraft` semantics so nothing regresses; rename chips accordingly.

## Switcher
- Existing `PAPER-EXPERIMENT` switcher (bottom-left): texture row now ~17 chips (wraps), color row ~7 chips (wraps). localStorage keys `tex` (default `cardstock`) + `tone` (default `plum`/`dark`). Always visible. Keep `BG`/`paper` namespacing + `PAPER-EXPERIMENT` delimiters for stage-08 removal.

## Verification
- `cd site && npm run build` succeeds; `npx astro check` 0 errors.
- Grep gates: `rg -n "@keyframes sweep|class=\"fx scan\"" site/` → EMPTY; `rg -n "animation: *flow" site/` → EMPTY (grid frozen); `rg -n "PAPER-EXPERIMENT" site/` present; telemetry layers present.
- No new hex colors introduced (tones via `color-mix` of tokens). `rg -n "#[0-9a-fA-F]{3,6}" site/src/styles/landing.css` should not grow with new literals for tones.
- Preview: background fully static (no descending highlight, no grid drift); switcher offers ~17 textures × ~7 colors; new textures feel tactile/raised; text legible across combos.

## Out of scope
- Locking in / removing the switcher (stage 08).
- Animating textures; re-adding scan/parallax/grid motion.
- Light-theme inversion; new webfonts.
