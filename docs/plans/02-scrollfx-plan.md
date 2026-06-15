# Stage 02 — Scroll FX experiment (plan)

Implements `02-scrollfx-spec.md`. Additive scroll FX on the Astro site (`site/`), 3 switchable variants behind a temp dev switcher. Vanilla CSS + the existing inline `<script>`; no deps. Build on the existing parallax/reveal/spotlight — do not break them.

## Baseline
- From the worktree: `cd site && (npm ci if needed) && npm run build` → confirm green before edits.

## Task 1 — Mark targets
- `site/src/components/SelfEvolve.astro`: ensure the root element has a stable class (add `evolve` if absent). `FeatureCard.astro` (`.card`) and `Diagram.astro` (`.flow`) already have usable classes — leave them.
- Decide the shared JS selector: `.card, .flow, .evolve, .section h2`.

## Task 2 — Switcher (temp scaffolding, self-contained for easy stage-03 removal)
- `site/src/layouts/Landing.astro`: add a small fixed control (bottom-right) with three buttons A/B/C (+ tiny labels: `scale-in` / `focus` / `spotlight`). One clearly-delimited markup block (e.g. wrapped in a `<!-- FX-EXPERIMENT … -->` comment so stage 03 can grep-delete it).
- JS (in the existing `<script>`): on load restore `localStorage['fx']` (default `'a'`) → set `document.documentElement.dataset.fx`; button clicks update both. Hide the switcher when `matchMedia('(prefers-reduced-motion: reduce)').matches`.
- `site/src/styles/landing.css`: switcher styles using existing jewel custom properties; no new hexes.

## Task 3 — Variant A (soft scale-in)
- CSS: under `:root[data-fx="a"]`, targets get an initial `scale: .94` that animates to `scale: 1` when revealed (couple to the `.rev.in` state). Stagger the card grid (reuse `.d1..d5` or `:nth-child`).
- **Use the individual `scale:` property, never `transform`** — `.rev` owns `transform: translateY()`. Confirm reveal translate still fires.

## Task 4 — Variant B (scroll-scrubbed focus)
- JS: a rAF loop (fold into the existing tick if practical) computes each target's distance from viewport center → sets `el.style.scale` in `[.96 … 1.04]` (CSS var `--zoom` or direct `style.scale`). Only active under `:root[data-fx="b"]`.
- Guard: skip the loop entirely under `prefers-reduced-motion`. Keep it compositor-friendly (scale only, batched reads, sparing `will-change`).

## Task 5 — Variant C (active-section spotlight)
- JS: determine the `.section` nearest viewport center; it gets `data-active` (full opacity + `scale: 1.02`), siblings dim (`opacity:.55`, `scale:.985`) via CSS under `:root[data-fx="c"]`. Smooth transitions on opacity/scale. Active under `[data-fx="c"]` only.

## Task 6 — Reduced motion + isolation
- `@media (prefers-reduced-motion: reduce)`: all variants inert (`scale:1`), switcher hidden, B/C rAF bailed; existing reduced-motion `.rev` rule stays authoritative.
- Ensure exactly one rAF loop drives parallax + B + C (don't spawn competing loops).

## Verify (in the worktree)
- `cd site && npm run build` → succeeds; no TS errors from the Landing.astro script.
- Preview sanity (best-effort if a preview is trivially available): switcher flips A/B/C; each behaves per spec; reveal/parallax/spotlight intact; no console errors.
- Grep: the switcher block is delimited (e.g. `FX-EXPERIMENT`) so stage 03 can find+remove it.

## Notes for executor
- Model: openai/gpt-5.4 / max. This stage has real JS logic — be careful the three variants are cleanly isolated by `[data-fx]` and that only one is "live" at a time (B/C loops must early-return when their variant isn't active, so switching has no residual cost).
- Pre-commit hook may reformat; if `prek` aborts on missing `.pre-commit-config.yaml`, use `PREK_ALLOW_NO_CONFIG=1` (pre-existing env condition on this branch).
- Do NOT remove any variant — the comparison is the deliverable.
