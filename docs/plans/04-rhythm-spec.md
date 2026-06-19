# Stage 04 — Reading column + narrow container + soften reveal — Spec

## Goal

Make the page read as **one collected column** instead of edge-to-edge sprawl.
Diagnosis (measured ours vs herdr.dev): our outer container (76rem/1216px) is
about the same as herdr (1160px) — the sprawl comes from **misaligned columns**:
section headings (`h2`) and panel rows run the full ~1142px while lead text is
capped at 65ch and left-aligned, so the right edge is ragged and nothing lines
up. herdr keeps text in a tight, consistent column (h1 ≤760px, lede ≤520px) and
spends width deliberately.

Fix = give section text one shared reading measure, narrow the overall container
a touch, and soften the scroll reveal. Agreed scope with the user: **option 1
(unified reading column) + light option 2 (narrower container) + reveal soften.**
Deferred to a later discussion: monospace headline (#3, brand change), eyebrow/
label tag restyle, dead-CSS prune.

## In scope

1. **Unified text reading measure.** In `landing.css`, give section headings and
   leads one shared max-width so they align on both edges:
   - Add a token `--readw` (~`66ch`) — or use `66ch` directly.
   - `.section h2` (the section-level `h2` rule) → `max-width: 66ch;` (currently
     uncapped → it stretches the full container).
   - `.lead` → `max-width: 66ch;` (currently `65ch` — unify to the same value).
   - Both are direct children of `.wrap`, already left-aligned, so capping the
     width lines up their left **and** right edges into one column.
   - Do NOT cap the hero `h1` (it lives in the hero 2-col grid and is already
     column-bound) — only the section `h2`.

2. **Light container narrow.** In `tokens.css`, `--maxw: 76rem` → `70rem`
   (≈1120px). Everything (hero grid, panel rows, text) tightens uniformly; more
   breathing room at the sides. Panels stay within this column.

3. **Soften the scroll reveal.** In `landing.css` `.rev`:
   - `transform: translateY(24px)` → `translateY(8px)`.
   - transition duration `.9s` → `.35s` (both the opacity and transform timings).
   - Keep the `.rev.in`, `.rev.dN` stagger delays and the reduced-motion override
     as-is. Result: a quick, barely-there fade with no heavy dimming on scroll.

## Out of scope — keep / defer

- Brand accent colors, hairline borders, flat panel fills, the de-chromed
  surfaces from stage 3 — untouched.
- Monospace headline (#3) — separate brand discussion, NOT now.
- Eyebrow/`.label` tag restyle and dead-CSS prune (`.status`/`.pdot`/
  `.statusnote`/`.bk`/`.cmd` if unused) — deferred; do not touch here.
- Vertical rhythm / section padding — leave as-is this round.
- No markup/content changes — `tokens.css` + `landing.css` only.

## Acceptance criteria

1. Section heading (`h2`) and the lead paragraph below it share the same left
   edge AND the same right edge (both ≤ ~66ch); no heading stretches the full
   container width any more.
2. Overall content column is ≈1120px (`--maxw: 70rem`); side margins visibly
   wider than before on a wide viewport.
3. Scroll reveal is a quick subtle fade (≤8px rise, ~.35s) — content does not sit
   visibly dimmed while scrolling.
4. Hero still renders fully on load; hero 2-col layout intact (just slightly
   narrower).
5. `cd site && bun run check` → 0 errors; `cd site && bun run build` → success.
6. Only `tokens.css` and `landing.css` changed.

## Verification (website-only — no cargo)

- `cd site && bun run check && bun run build` green.
- `rg -n 'max-width:\s*76rem|--maxw:\s*76rem' site/src` → no hits (the old width
  is gone).
