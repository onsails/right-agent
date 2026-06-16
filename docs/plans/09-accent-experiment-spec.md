# Stage 09 — Text accent color experiment (spec)

## Problem
The secondary text accent — `--warm` / `--warm-soft` (gold `#cda14b`) — reads as a dirty brown-rust on the dark base. The user wants several **harmonious** replacement candidates to compare.

## Scope
`--warm` / `--warm-soft` drive the warm secondary accent across the page: `.eyebrow`, `.label` (+ `::before`), `.card .tick`, `.cmd .p` / `.cmd-name`, `.telem .k::before`, `.starchip`/`.starbtn` icons, `.statusnote .sdot`, `.packet`, `.step.curate .model`, etc. Swapping these two tokens cascades to all of them.

**Do NOT touch** the identity ruby (`--ink`: word "right" + claw) or the teal action color (`--accent`/`--cyan`: links, HUD edges, status). Only the warm accent changes — that keeps the palette coherent.

## Goal
Add an **accent switcher** (a third row, `accent`, in the existing `PAPER-EXPERIMENT` switcher panel, bottom-left) that overrides `--warm` + `--warm-soft` on `<html data-accent=…>`, so the user can compare harmonious candidates and pick one (final lock-in stage removes the switcher and bakes the choice). Persist to `localStorage['accent']`.

New hex values ARE expected here (the whole point is new accent colors) — that's an allowed exception to the no-new-hex rule, scoped to the accent candidates. Keep each candidate harmonious with the jewel palette (ruby `--ink`, teal `--accent`, cream `--text`, plum `--bg`) and legible on the dark base.

## Candidates (each sets `--warm` + a lighter `--warm-soft`)
Build these (tune for harmony/legibility; values are starting points):
- `gold` — current `#cda14b` / soft `#e6cf8e` (baseline reference, so the user can compare against today).
- `champagne` — cleaner light gold, less brown (~`#d8c489` / `#ece0b4`).
- `amber` — brighter warm honey (~`#e0a14e` / `#f0c489`).
- `rose` — blush rose, analogous to ruby (~`#d98aa6` / `#edb9c9`).
- `coral` — warm coral/salmon (~`#e0917a` / `#f0b6a6`).
- `sage` — muted green, complementary (~`#9cbf95` / `#c4d8bf`).
- `lilac` — soft lavender, harmonizes with plum (~`#b3a4e6` / `#d3c9f1`).

Default `data-accent="champagne"` (clean, closest to today's warmth without the dirt). Adjust any value if it clashes; harmony + legibility win.

## Implementation
- `Landing.astro`: add an `accent` row to the `PAPER-EXPERIMENT` switcher (chips `gold/champagne/amber/rose/coral/sage/lilac`), `data-accent-choice`. Wire JS like the existing texture/tone rows: restore `localStorage['accent']` (default `champagne`), set `document.documentElement.dataset.accent`, `aria-pressed`, unknown value falls back to default. Always visible.
- `landing.css`: `:root[data-accent="…"] { --warm: …; --warm-soft: …; }` per candidate. No other rules change (all usages already read the tokens).
- Keep within the existing `PAPER-EXPERIMENT` delimiters (or an analogous `ACCENT-EXPERIMENT` block) so the final lock-in stage can remove the switcher + collapse the chosen accent into the base tokens.

## Verification
- `cd site && npm run build` succeeds; `npx astro check` 0 errors. (Website-only — NO cargo/Rust tests.)
- Preview: the `accent` row flips the warm accent across eyebrow/labels/ticks/cmd/star/telem consistently; each candidate is harmonious and legible; ruby identity + teal links unchanged.

## Out of scope
- Locking in the chosen accent / removing the switcher (final stage).
- Changing ruby or teal; restructuring the switcher beyond adding the row.
