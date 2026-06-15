# Stage 02 — Scroll FX experiment (spec)

## Goal
Add scroll-driven zoom/scale emphasis to the landing page's significant content, as an **experiment**: ship **3 switchable variants** behind a temporary dev switcher so the user can compare them live in preview and pick one. Stage 03 (separate) removes the losers + switcher.

This is additive on top of the existing FX in `site/src/layouts/Landing.astro`'s `<script>` (pointer/scroll parallax on `.fx[data-depth]`; `.rev` reveal via IntersectionObserver; `.card` spotlight). **Do not break or remove any of that** — layer on top.

## Targets ("significant parts")
Apply the effect to this set only (NOT the hero screenshot, per user):
- the 6 feature cards — `.card` (rendered by `FeatureCard.astro`, root `<article class="card rev">`)
- the flow diagram — `.flow` (`Diagram.astro` root `<div class="flow rev">`)
- the self-evolution block — `SelfEvolve.astro` root (give it a stable class if it lacks one, e.g. `.evolve`)
- section headings — `.section h2`

Define one JS target selector covering these, e.g. `.card, .flow, .evolve, .section h2`.

## The switcher (temporary scaffolding)
- A small fixed control (bottom-right), mono/jewel-styled, listing **A / B / C** (+ short labels). Clicking sets `document.documentElement.dataset.fx = 'a'|'b'|'c'` and persists to `localStorage`. On load, restore from `localStorage` (default `a`).
- Clearly an experiment affordance — it (and all three variants' dead code) gets removed in stage 03. Keep it self-contained and easy to delete (one markup block + one CSS block + one JS init).
- Hidden when `prefers-reduced-motion: reduce` (the effects are off anyway).

## Variants (all keyed off `[data-fx]` on `<html>`)
- **A — soft scale-in** (one-shot, reveal-coupled): when a target first enters the viewport, animate `scale: .94 → 1` together with the existing `.rev` opacity/translate. Stagger across the 6-card grid (reuse `.d1..d5` delays or nth-child). Calmest; pure IntersectionObserver, no continuous loop.
- **B — scroll-scrubbed focus** (continuous): each target's `scale` tracks its distance from viewport center — grows approaching center (max ≈ `1.04`), eases back leaving (≈ `.96`). Drive from a rAF loop (fold into the existing rAF tick if practical). "Telescope focusing."
- **C — active-section spotlight**: the `.section` nearest viewport center gets full opacity + slight scale-up (≈ `1.02`); sibling sections dim (opacity ≈ `.55`) and shrink (≈ `.985`). Emphasizes the section being read. Transition smoothly as the active section changes.

## Critical technical constraint
`.rev` already uses `transform: translateY(24px)`. **Do NOT drive the zoom through `transform`** — it will clobber the reveal translate. Use the **individual `scale:` CSS property** (e.g. `scale: var(--zoom, 1)`), which composes independently with `transform`. Variants set `--zoom` (B/C) or animate `scale` (A) without touching `transform`. Verify the reveal translate still works with the zoom applied.

## Accessibility / perf
- `@media (prefers-reduced-motion: reduce)`: all three variants inert — targets render at `scale: 1`, existing reduced-motion `.rev` rule (instant visible) stays authoritative; switcher hidden.
- Continuous variants (B/C) must be cheap: single shared rAF, `transform`/`scale` only (compositor-friendly), `will-change` used sparingly, reads batched. No layout thrash. Bail the rAF when `prefers-reduced-motion`.
- Must not regress the existing parallax/reveal/spotlight.

## Constraints
- No new dependencies; vanilla CSS + the existing inline `<script>` (TypeScript in Landing.astro). Astro components only.
- Use existing jewel custom properties for any switcher styling; no new palette hexes.
- Keep edits localized: `Landing.astro` (script + switcher markup), `landing.css` (variant + switcher styles), `SelfEvolve.astro` (class marker only if needed), `FeatureCard.astro`/`Diagram.astro` already carry usable classes.

## Verification
- `cd site && <build>` succeeds.
- Manual/preview (the whole point): switcher toggles A/B/C; each visibly behaves per its description; reveal/parallax/spotlight still work; reduced-motion (emulated) disables effects + hides switcher.
- No console errors; scroll stays smooth (no jank) with B/C active.

## Out of scope
- Removing variants / switcher (stage 03, after the pick).
- Hero screenshot zoom; touching telemetry (stage 01).
- Any new sections or copy.
