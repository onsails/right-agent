# Stage 02 — Motion restraint — Plan

Executor implements this in the stage worktree. Target files:
`site/src/layouts/Landing.astro`, `site/src/components/Hero.astro`,
`site/src/components/SelfEvolve.astro`, `site/src/styles/landing.css`.
Mostly subtractive. Spec: `02-motion-spec.md`. Do NOT touch glows/shadows/
borders/blur except when deleting a rule wholesale per the spec.

## Tasks

1. **`Landing.astro` `<script>` — strip scroll-zoom + card spotlight, keep reveal.**
   - Delete the entire active-section block: `const sections = …` through the
     `tick`/`requestAnimationFrame(tick)` loop and the initial
     `updateActiveSection()` call (everything driving `data-active`).
   - Delete the `document.querySelectorAll('.card').forEach(... pointermove ...)`
     block.
   - **Keep** the `IntersectionObserver`/`.rev` reveal block exactly as-is.
   The script should end up containing only the `.rev` IntersectionObserver.

2. **`Hero.astro` — static hero, drop learning dots.**
   - In the `.eyebrow`, delete `<span class="sep">·</span><span class="sig">…</span>`
     so only `<span>▸ sandboxed multi-agent runtime</span>` remains.
   - Remove the `rev d1`/`rev d2`/`rev d3`/`rev d4`/`rev d5` classes from the
     eyebrow div, `h1`, `.sub` paragraph, `.cta` div, and `.shot` div. Keep every
     other class and all content/markup.

3. **`SelfEvolve.astro` — drop the traveling spark.**
   - Delete the `<div class="loopspark"></div>` line. Keep
     `<div class="looptrack"></div>` and the `.loop rev evolve` container
     (its `rev` reveal stays).

4. **`landing.css` — delete the motion rules.**
   - `.clawwrap::after`: remove the `animation: spin …` declaration; delete
     `@keyframes spin`. (Keep the ring's `border`/`border-radius`.)
   - `.claw`: remove the `animation: pulse …` declaration; delete
     `@keyframes pulse`. (Keep its static `filter: drop-shadow(...)`.)
   - Delete `.eyebrow .sig`, `.eyebrow .learn-label`, `.learn-dots`,
     `.learn-dots i`, and `@keyframes learn-fill-1` … `learn-fill-5`.
   - Delete `.loopspark` and `@keyframes travel`. (Keep `.looptrack`.)
   - `.section`: remove `transform-origin` and the
     `transition: scale …, opacity …`; delete the entire
     `@media (prefers-reduced-motion: no-preference){ .section … }` block.
   - `.card`: delete the `.card::after { … }` rule and the `.card:hover::after`
     rule (cursor spotlight). Keep `.card`, `.card::before`, `.card:hover`.
   - In `@media (prefers-reduced-motion: reduce)`: remove `.loopspark` and
     `.learn-dots i` from the `animation:none` selector list; delete the
     `.section{ opacity:1 …; scale:1 … }` line and the
     `.learn-dots i:nth-child(-n+3){…}` block. Keep the
     `.rev{ opacity:1 !important; transform:none !important }` line.

## Verification (run in the stage worktree)

- `cd site && bun install` (fresh worktree may lack node_modules).
- `cd site && bun run check` → 0 errors.
- `cd site && bun run build` → success (~9 pages, links + pagefind green).
- `rg -n 'data-active|pointermove|loopspark|learn-dots|@keyframes (spin|pulse|travel|learn-fill)' site/src` → no hits.

Leave all changes uncommitted; the stage-runner commits and lands.
