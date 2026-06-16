# Stage 08 — Flatten background to monotone paper (spec)

## Problem
The background reads as "dirty brown" and "too gradient." Cause: stacked warm/colored radial-gradient + glow layers — `.void` (4 radial gradients: warm 15% / accent 13% / ink 9% + a dark vignette), `.depthbloom` (warm bloom), `.nebula` (warm/accent/ink blurred clouds), `.horizon-glow` (warm glow line + 60px warm shadow), and the `.horizon`/`.ceiling` perspective grid. Together they cast a muddy warm wash and a gradient field.

## Goal
The background must read like **paper: a flat, even, monotone color field + the paper texture** — no gradients, no warm glow wash, no perspective grid. Keep the texture + color-tone switcher experiment (user is still choosing a texture/color).

Astro site (`site/`). CSS/markup only; no deps; jewel custom properties only (no new hexes).

## Changes
1. **Remove the gradient/glow ambient layers** entirely (markup in `Landing.astro` + CSS in `landing.css`):
   - `.depthbloom`, `.nebula`, `.horizon`, `.ceiling`, `.horizon-glow` — delete the `<div class="fx …">` and their CSS rules. (The `flow`/`sweep` animations are already gone from stage 07.)
2. **Flatten `.void`** to a single uniform fill — remove ALL the radial gradients and the vignette; `.void { background: var(--bg); }` (a flat color). No gradients anywhere in the base.
3. **Monotone paper surface:** the visible background = the flat tone color (`.paper` base, tinted by `data-tone`) + the texture pattern. Ensure:
   - The tone tint is a **flat, uniform color** (already `color-mix` of a token into `--bg`) — no gradient.
   - The **texture modulates lightness only** (neutral light/dark tooth, emboss/grit) and must NOT introduce a competing hue that muddies the tone. Each texture should look like the SAME tone with a tactile surface, not a brown overlay. Tune blend modes / tint so textures stay tonal (monochrome relief), not colored.
4. Keep everything else: telemetry layers (+ flicker), `.scan` already removed, active-section zoom, reveal, spotlight, learning, the HUD frame, and the `PAPER-EXPERIMENT` switcher (17 textures × 7 tones). Background stays fully static.

## Result
A clean, flat, monotone paper field of the selected color, with the selected tactile texture on top — no gradient, no dirty-brown cast.

## Verification
- `cd site && npm run build` succeeds; `npx astro check` 0 errors.
- Grep gates: `rg -n "depthbloom|nebula|horizon-glow|class=\"fx ceiling\"|class=\"fx horizon\"" site/` → EMPTY; `.void` has no `radial-gradient` (`rg -n "radial-gradient" site/src/styles/landing.css` should not include the `.void` rule). `PAPER-EXPERIMENT` switcher + telemetry layers still present.
- No new hex literals.
- Preview: background is a flat monotone paper of the chosen color + texture — no gradient, no warm wash; text legible; switcher still flips textures/colors.

## Out of scope
- Locking in the chosen texture/color (next stage).
- Re-adding any gradient/glow/grid; animating the bg.
- Light-theme inversion; new webfonts.
