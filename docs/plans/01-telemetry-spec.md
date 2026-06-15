# Stage 01 — Telemetry meaning (spec)

## Goal
Make the Hero eyebrow telemetry carry real meaning instead of decorative filler, and drop the empty HUD readout.

Two changes on the Astro marketing site (`site/`):

1. **`sig ●●●●○` → animated `learning` indicator.** The label becomes `learning`, and the dots become a subtle looping animation that reads as "the agent is continuously learning" — a direct visual nod to the product's self-evolution / skill-learning feature. This replaces a label with zero semantic load.
2. **Remove `obs · deep-field`.** Delete the top-right HUD readout entirely; remove the now-orphaned `.r-tr` CSS selector. Keep the bottom-left `secure-by-default` (`.r-bl`) untouched.

## Files
- `site/src/components/Hero.astro` — the `.eyebrow` block (currently line ~24): `<span class="sep">·</span><span class="sig">sig ●●●●○</span>`.
- `site/src/layouts/Landing.astro` — the `.hud` block (currently line ~34): `<span class="read r-tr">obs · deep-field</span>`.
- `site/src/styles/landing.css` — `.eyebrow .sig` rule (line ~120) for the new dots; `.hud .r-tr` selector (line ~90) to remove.

## Behavior — `learning` indicator
- Markup (Hero): replace the static span with a label + a row of dot elements, e.g.
  `<span class="sig"><span class="learn-label">learning</span><span class="learn-dots" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i></span></span>`.
- Animation: a staggered "fill wave" across the dots (each `<i>` pulses filled→empty on a per-dot `animation-delay`), looping infinitely. Conveys ongoing accumulation/learning, not a static bar. Subtle — must not draw the eye off the H1 (brand rule: "not busy").
- Color: teal (`var(--cyan)`, the existing `.sig` color = brand "live rail"). A soft glow (`box-shadow` with `color-mix`) on the lit state is acceptable; keep low.
- Dot geometry: small (~0.4em), `border-radius:50%`, thin teal border when unlit, teal fill when lit. Sized in `em` so it tracks the eyebrow font-size.
- `learning` label keeps the mono/letter-spaced eyebrow styling; lowercase, consistent with `sig` before it.

## Accessibility / motion
- The dots are decorative → `aria-hidden="true"` on the dot container; the word `learning` stays readable text.
- `@media (prefers-reduced-motion: reduce)`: disable the animation; render a static partially-filled state (e.g. first 3–4 dots filled) so it still reads as an indicator.

## Constraints
- Recolor/markup only — no new dependencies, no JS (pure CSS animation). Do not touch the Landing.astro `<script>` parallax/reveal logic.
- Use existing CSS custom properties (`--cyan`, etc.); do not introduce new palette hexes.
- Keep the `·` separator (`.sep`) before the indicator as today.

## Verification
- `cd site && npm run build` (or the repo's site build) succeeds.
- Grep gate: no remaining `obs · deep-field` and no `r-tr` in `site/`.
- Manual/visual (best-effort): eyebrow reads `▸ sandboxed multi-agent runtime · learning ●●●●●(animated)`; top-right HUD corner no longer shows `obs · deep-field`; reduced-motion renders a static indicator.

## Out of scope
- Any scroll/zoom effects (Stage 02).
- Changing `secure-by-default`, the constellation SVG, or the parallax layers.
