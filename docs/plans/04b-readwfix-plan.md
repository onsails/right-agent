# Stage 04b — Reading-column fix (ch → rem) — Plan

Stage 04 narrowed `--maxw` to 70rem and softened the reveal correctly, but the
`max-width: 66ch` cap on the section `h2` was INEFFECTIVE: `ch` is relative to
the element's own font-size, so 66ch at the h2's ~33px font ≈ 1250px (wider than
the container) — the heading still spans full width and does not align with the
`.lead` (which is 744px at its smaller font). Fix with a shared **rem** measure
so heading and lead share the same pixel width and right edge.

Target files: `site/src/styles/tokens.css`, `site/src/styles/landing.css` ONLY.
Surgical — no other changes.

## Tasks

1. In `site/src/styles/tokens.css`, add a token to `:root`:
   `--readw: 46rem;`  (≈736px — a consistent reading column for section text).

2. In `site/src/styles/landing.css`:
   - On the section `h2` rule, change its `max-width: 66ch;` to
     `max-width: var(--readw);`.
   - On the `.lead` rule, change `max-width: 66ch;` to `max-width: var(--readw);`.
   (Both now resolve to the same pixel width regardless of font-size, so the
   heading and the lead below it align on both edges.)

Do not touch the hero `h1`, `.label`, panels, `--maxw` (stays 70rem), or the
`.rev` reveal (already softened in stage 04).

## Verification (run in the stage worktree)

- `cd site && bun install && bun run check && bun run build` → all green.
- `rg -n 'max-width: ?66ch' site/src/styles` → NO hits (both replaced).
- Quick sanity: the section `h2` and `.lead` now have equal `max-width`
  (`var(--readw)` = 46rem).

Leave changes uncommitted; the stage-runner commits and lands.
