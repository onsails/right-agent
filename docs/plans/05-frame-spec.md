# Stage 05 — herdr-style hairline frame — Spec

## Goal

Add the structural "frame" the landing currently lacks — the engineering-sheet
grid of hairline rules that gives herdr.dev its collected, framed feel. herdr
(measured) uses: a `border-bottom` hairline under the header, a `border-bottom`
under the hero, and a `border-right` on the hero's left column (the vertical
divider between copy and the visual). We replicate that with our dark hairline
`var(--line)`, plus full-bleed rules between content sections.

This is **additive** (not declutter): we are putting structure back, but as
functional hairlines, NOT the decorative HUD corner-brackets removed in stage 1.
No textures, no glows — just `1px solid var(--line)` rules. User approved: full
page grid + integrate the hero screenshot into the frame (drop its own border).

## In scope

1. **Nav rule (full-bleed).** Wrap the existing nav's `.wrap` in a full-width
   `<header class="topbar">` and give `.topbar` `border-bottom: 1px solid
   var(--line)`. The line spans the viewport, content stays centered in `.wrap`.

2. **Hero vertical divider + bottom rule.**
   - `.hero-grid`: `gap: 0`, `align-items: stretch` (so the divider spans the
     full hero height).
   - `.hero-copy`: `border-right: 1px solid var(--line)`, `padding-right: 3rem`,
     and vertically center its content (`display:flex; flex-direction:column;
     justify-content:center`) so removing the gap doesn't shift it.
   - `.shot`: `padding-left: 3rem` (breathing room on the visual side).
   - `header.hero`: `border-bottom: 1px solid var(--line)` (full-bleed line under
     the hero).
   - In the existing `@media (max-width:900px)` block (grid → 1 col): reset the
     divider — `.hero-copy { border-right:none; padding-right:0; }` and
     `.shot { padding-left:0; }`.

3. **Section rules (full-bleed).** `.section { border-bottom: 1px solid
   var(--line); }` — a hairline at each section's bottom edge, forming the grid
   between sections. Since the last section (`#install`) now draws the line above
   the footer, remove the footer's redundant top rule:
   `footer.sitefooter` → delete its `border-top` declaration.

4. **Integrate the hero screenshot.** `.shot img`: remove `border-radius: 14px`
   and `border: 1px solid var(--line)` so the screenshot sits cleanly in the
   right cell, framed by the divider + hero rule (no double frame).

## Out of scope — keep

- The `.telem` stat strip stays its own bordered card (deliberate element between
  hero and sections) — do not reframe it this stage.
- All stage 1–4 work: flat bg, no motion (subtle reveal), flat de-chromed panels,
  reading column (`--readw`), `--maxw: 70rem`. Untouched.
- No new textures/grids/glows. Hairline `var(--line)` only.
- No copy/content changes. Files: `Landing.astro` (nav wrapper) + `landing.css`.

## Acceptance criteria

1. A hairline rule runs under the navbar, full viewport width.
2. The hero shows a vertical hairline between the copy (left) and the screenshot
   (right), spanning the hero height, with comfortable padding on both sides; a
   hairline runs under the hero.
3. Hairline rules separate each content section (no double line above the
   footer).
4. The hero screenshot has no border/rounded corners of its own — it sits in the
   framed right cell.
5. On mobile (≤900px) the hero is single-column with no vertical divider and no
   broken padding.
6. `cd site && bun run check` → 0 errors; `cd site && bun run build` → success.
7. Only `Landing.astro` (nav wrapper added) and `landing.css` change; no
   content/copy edits.

## Verification (website-only — no cargo)

- `cd site && bun run check && bun run build` green.
- Visual: nav rule + hero divider + hero rule + section rules all present; hero
  screenshot borderless; mobile single-column clean.
