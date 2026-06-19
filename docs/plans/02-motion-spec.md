# Stage 02 — Motion restraint — Spec

## Goal

Cut the idle, scroll-driven, and cursor-following motion so the page reads calm
and static — the herdr.dev quality of "motion only where it aids comprehension."
Content must be present immediately, not animate itself into view. Keep exactly
one gentle effect: a subtle reveal on **below-the-fold** sections.

Mostly subtractive. Dark jewel brand and all content stay. Glows, shadows,
borders, blur, and dead CSS are **stage 3/4** — do not touch them here except
where a rule is being deleted wholesale for motion reasons.

## In scope — remove this motion

1. **Section scroll-zoom** (the most distracting). In `Landing.astro` `<script>`:
   delete the entire active-section machinery — `sections`, `activeSection`,
   `clearActiveSection`, `updateActiveSection`, `tick`, both
   `requestAnimationFrame(...)` calls, and the initial `updateActiveSection()`.
   In `landing.css`: from `.section`, drop `transform-origin` and the
   `transition: scale …, opacity …`; delete the whole
   `@media (prefers-reduced-motion: no-preference){ .section{scale:.96;opacity:.55} .section[data-active]{scale:1.06;opacity:1} }`
   block. `.section` keeps only its `padding`, `position`, `z-index`.
2. **Logo spin ring.** Remove `animation: spin …` from `.clawwrap::after` and
   delete `@keyframes spin`. Keep the static ring border (de-chrome is stage 3).
3. **Claw pulse.** Remove `animation: pulse …` from `.claw` and delete
   `@keyframes pulse`. Keep the static `filter: drop-shadow(...)` (stage 3).
4. **Animated "learning" dots.** In `Hero.astro` delete the eyebrow's learning
   span: `<span class="sep">·</span><span class="sig">…learn-label…learn-dots…</span>`
   — the eyebrow becomes just `<span>▸ sandboxed multi-agent runtime</span>`. In
   `landing.css` delete `.eyebrow .sig`, `.eyebrow .learn-label`, `.learn-dots`,
   `.learn-dots i`, the `learn-fill-1..5` keyframes, and the
   `.learn-dots i:nth-child(-n+3)` reduced-motion fallback.
5. **Self-evolution "loopspark".** In `SelfEvolve.astro` delete the
   `<div class="loopspark"></div>` line (keep `<div class="looptrack"></div>` —
   it is a static connector). In `landing.css` delete `.loopspark` and
   `@keyframes travel`.
6. **Card cursor-spotlight.** In `Landing.astro` `<script>` delete the
   `document.querySelectorAll('.card')…addEventListener('pointermove', …)` block.
   In `landing.css` delete `.card::after` (the `--gx/--gy` radial glow) and the
   `.card:hover::after` rule.

## In scope — fix hero on load

7. **Hero visible immediately.** In `Hero.astro` remove the `rev d1`/`rev d2`/…/
   `rev d5` classes from the eyebrow, `h1`, `.sub`, `.cta`, and `.shot` so the
   hero renders fully on load with no entrance animation. (Leave the elements and
   all other classes intact.)

## Out of scope — keep

- **Below-the-fold reveal.** Keep the `IntersectionObserver` `.rev` block in
  `Landing.astro` and the `.rev` / `.rev.in` / `.rev.dN` CSS. Section content
  (`.section .rev`, `.loop.rev`, cards) keeps its gentle one-time fade-up. This
  is the single allowed motion.
- `.card:hover` lift (translateY + border/shadow) — interaction feedback, not
  idle motion. Keep.
- Static logo ring, static claw glow, card `::before` hairline, panel
  borders/shadows/blur — **stage 3**.
- `.pdot` / `.statusnote .sdot` blink and `@keyframes blink`, `.packet` — these
  reference elements not rendered on the landing page; leave them (dead-CSS prune
  is stage 4). Just remove now-deleted selectors from the reduced-motion list.

## Reduced-motion rule cleanup

In `@media (prefers-reduced-motion: reduce)`: remove the now-deleted selectors
(`.loopspark`, `.learn-dots i`) from the `animation:none` list and delete the
`.section{ opacity:1; scale:1 }` line (section zoom is gone) and the
`.learn-dots i:nth-child(-n+3)` block. **Keep** `.rev{ opacity:1!important;
transform:none!important }` (reduced-motion users still get content instantly).

## Acceptance criteria

1. Hero headline + subhead + CTAs + screenshot are visible on first paint at the
   top of the page, with **no** scroll and no fade-in needed.
2. Scrolling does not scale or dim sections; all sit at full size/opacity.
3. No spinning ring on the logo, no pulsing claw glow, no animated learning dots
   (the eyebrow is a single static phrase), no traveling dot in the
   self-evolution loop, no cursor-following glow on cards.
4. Below-the-fold sections still do one subtle fade-up reveal on scroll.
5. `cd site && bun run check` → 0 errors; `cd site && bun run build` → success.
6. Grep gate clean: `rg -n 'data-active|pointermove|loopspark|learn-dots|@keyframes (spin|pulse|travel|learn-fill)' site/src` → no hits.

## Verification (website-only — no cargo)

- `cd site && bun run check && bun run build` green.
- Grep gate above returns nothing.
