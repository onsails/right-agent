# Stage 03 — Finalize FX (plan)

Implements `03-finalize-fx-spec.md`. Two deliverables on the Astro site (`site/`): (A) rework the `learning` indicator to a crisp sequential fill, (B) replace the 3-variant scroll experiment with one explicit active-section zoom, and (C) remove all `FX-EXPERIMENT` scaffolding. Vanilla CSS + the existing inline `<script>`; no deps. Don't break parallax / `.rev` reveal / `.card` spotlight.

## Baseline
- From the worktree: `cd site && (npm ci if node_modules absent) && npm run build` → confirm green before edits.

## Task 1 — `learning` sequential fill (file: `site/src/styles/landing.css`)
- Remove the `@keyframes fill-wave` + staggered per-dot `animation-delay` shimmer approach.
- New behavior: 5 `.learn-dots i` fill left→right discretely. Implementation options (pick the simplest that snaps, no crossfade):
  - one keyframe per position using `steps(1, end)` timing and per-`nth-child` `animation-delay` on a shared loop duration (~2.6s), where each dot is empty until its turn then solid, all reset together; OR
  - a single animation toggling `background` at discrete `%` stops with `steps()`.
- Filled = solid `var(--cyan)`; empty = transparent + thin teal border. No animated glow.
- `@media (prefers-reduced-motion: reduce)`: `animation:none`; first ~3 dots solid (static).

## Task 2 — active-section zoom (files: `Landing.astro` script + `landing.css`)
- CSS: `.section { transition: scale .45s cubic-bezier(.2,.7,.2,1), opacity .45s …; transform-origin:center; }`; default `.section{ scale:.96; opacity:.55; }`, `.section[data-active]{ scale:1.06; opacity:1; }`. Use the individual `scale:` property (NOT transform).
  - NOTE: only apply the recede/zoom when motion is allowed — gate via a root class or `@media (prefers-reduced-motion: no-preference)` so reduced-motion keeps `scale:1; opacity:1`.
- Overflow guard: add `overflow-x: clip` to `html`/`body` (whichever is correct for this layout) so scaled sections never cause a horizontal scrollbar. Verify at desktop + ~375px widths.
- JS (existing `<script>` rAF `tick()`): replace the variant-C nearest-section logic with the unconditional active-section computation — each frame, find the `.section` whose center is nearest viewport center, set `data-active` on it (clear others). Remove the variant-B per-target `scale` loop and the variant-A coupling. Keep parallax. Skip the section-focus branch under `prefers-reduced-motion`.

## Task 3 — remove experiment scaffolding (files: `Landing.astro`, `landing.css`)
- Delete every `FX-EXPERIMENT:start … :end` block: `.fxswitch` markup, the switcher JS init (`FX_KEY`/`getStoredFx`/`setFx`/click listener/`switcher.hidden`), the `:root[data-fx=…]` variant CSS, switcher CSS, and the `[data-fx]` reduced-motion overrides.
- Remove now-unused JS bindings introduced only for the experiment (e.g. `switcher`, `switcherButtons`, `zoomTargets` if no longer referenced, `clearZoomTargets`, `activeFx` plumbing) — keep `sections` + the active-section logic. Remove only what these deletions orphan; leave pre-existing code intact.
- `.evolve` marker on `SelfEvolve.astro`: keep (harmless) or drop if unreferenced — executor's call.

## Verify (in the worktree)
- `cd site && npm run build` → succeeds; `npx astro check` → 0 errors.
- Grep gate (must be empty): `rg -n "FX-EXPERIMENT|data-fx|fxswitch" site/`.
- Confirm no horizontal scrollbar from section scaling (reason about `overflow-x: clip`; spot-check if a preview is trivially available).

## Notes for executor
- Model: openai/gpt-5.4 / max (pinned). Resume mimo handle `scrollfx-9m4k` — you wrote the experiment, now finalize it.
- `prek` pre-commit hook may abort on missing `.pre-commit-config.yaml` → `PREK_ALLOW_NO_CONFIG=1`.
- This consolidates the old stage-03 ("lock-in chosen FX") with the redesign; there is no separate switcher to keep.
