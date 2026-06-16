# Stage 03 — Finalize FX (spec)

Supersedes the stage-02 experiment. User feedback on the 3 variants: "ничего не нравится; нужно более явное увеличение секций; learning должна быть чётче — просто заполняющиеся кружки, не переливающаяся анимация."

Two deliverables, both on the Astro site (`site/`):

## A. `learning` indicator — crisp sequential fill
Replace the current staggered "fill-wave" (reads as shimmer) with a **discrete sequential fill**:
- 5 dots start empty (hollow, thin teal border).
- Fill left→right one at a time, each snapping to solid `var(--cyan)` (instant, NOT a crossfade/opacity tween — use `steps(1)` / discrete keyframe).
- After the 5th fills, hold full briefly (~0.5–0.7s), then all reset to empty and repeat. Total loop ~2.5–3s.
- No animated glow/shimmer. A subtle *static* fill is fine; nothing pulsing.
- `prefers-reduced-motion: reduce`: static state, ~3 dots filled, no animation.
- Files: `site/src/styles/landing.css` (`.learn-dots` rules — replace the `@keyframes fill-wave` + per-dot delay approach). Hero markup (5 `<i>` dots) stays as-is.

## B. Section zoom — explicit active-section focus
Replace the A/B/C variants with ONE pronounced effect on whole `.section` blocks:
- The `.section` whose vertical center is nearest the viewport center is the **active** one: scales up clearly to **~1.06** and is full opacity.
- Non-active sections recede: scale **~0.96** and opacity **~0.55**.
- Smooth transition (~0.4–0.45s ease) as the active section changes while scrolling.
- Driven continuously by the existing shared rAF loop in `Landing.astro`'s `<script>` (reuse the nearest-section logic that variant C had; this replaces it). Toggle active via a `data-active` attribute on the section; CSS does the scale/opacity.
- **Use the individual `scale:` CSS property, never `transform`** (so it composes with `.rev`'s `transform: translateY` and the existing parallax). Apply to `.section` with `transform-origin: center`.
- **Overflow safety:** scaling sections up must NOT introduce a horizontal scrollbar. Add `overflow-x: clip` (or equivalent) on the page root so scaled/shrunk sections never cause horizontal scroll. Verify no horizontal scrollbar appears at desktop and mobile widths.
- `prefers-reduced-motion: reduce`: no zoom — all sections at `scale:1`, opacity 1; the section-focus rAF branch is skipped.

## C. Remove the experiment scaffolding
Delete entirely (all `FX-EXPERIMENT`-delimited blocks):
- The `.fxswitch` switcher markup in `Landing.astro` and its `localStorage`/click JS init.
- The A/B/C variant CSS (`:root[data-fx="a"|…]` blocks) and the switcher CSS in `landing.css`.
- The `[data-fx]`-keyed reduced-motion overrides specific to the experiment.
- The final effect (section B) is unconditional (not behind `data-fx`); no switcher remains.
- Keep the `.evolve` class marker added to `SelfEvolve.astro` only if the final effect still needs it; otherwise it's harmless to keep.

## Constraints
- No new dependencies; vanilla CSS + the existing inline `<script>`. Existing jewel custom properties only; no new hexes.
- Do NOT break the existing parallax (`.fx[data-depth]`), `.rev` reveal, or `.card` spotlight.
- Keep the shared single rAF loop; section-focus is one branch of it (runs when not reduced-motion).

## Verification
- `cd site && npm run build` succeeds; `npx astro check` 0 errors.
- Grep gates (must be empty): `rg -n "FX-EXPERIMENT|data-fx|fxswitch" site/` → no matches.
- Preview (best-effort): active section is visibly larger/brighter, neighbors smaller/dimmer, transition smooth on scroll; no horizontal scrollbar; learning dots fill sequentially then reset (crisp, no shimmer); reduced-motion emulation disables both effects.

## Out of scope
- Hero screenshot zoom; telemetry copy (settled in stage 01).
- Tuning exact magnitudes beyond the spec — that's a fast follow if the user wants more/less after seeing it.
