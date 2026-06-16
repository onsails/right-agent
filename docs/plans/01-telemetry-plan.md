# Stage 01 — Telemetry meaning (plan)

Implements `01-telemetry-spec.md`. Pure markup + CSS on the Astro site (`site/`). No JS, no deps.

## Baseline
- From the worktree: `cd site && npm ci` (if `node_modules` absent) then `npm run build` once to confirm a green baseline before edits. Record any pre-existing failure.

## Task 1 — Remove `obs · deep-field`
1. `site/src/layouts/Landing.astro`: delete the line `<span class="read r-tr">obs · deep-field</span>` from the `.hud` block. Leave `<span class="read r-bl">secure-by-default</span>` intact.
2. `site/src/styles/landing.css`: in the rule at ~line 90 (`.hud .r-tr{ top:-2px; right:26px } .hud .r-bl{ bottom:-2px; left:26px }`), remove the now-orphaned `.hud .r-tr{ ... }` selector; keep `.hud .r-bl{ ... }`.

## Task 2 — `sig` → animated `learning` indicator
1. `site/src/components/Hero.astro`: replace
   `<span class="sep">·</span><span class="sig">sig ●●●●○</span>`
   with the label + dot-row markup from the spec (`learn-label` + `learn-dots` with 5 `<i>` children, `aria-hidden` on the dot row). Keep the `.sep` `·` before it.
2. `site/src/styles/landing.css`: extend `.eyebrow .sig` to lay out label + dots inline (flex, small gap), and add:
   - `.learn-dots` flex row with small `em` gap.
   - `.learn-dots i`: ~0.4em circle, thin teal border unlit, teal fill + soft glow lit.
   - `@keyframes` fill-wave; per-`nth-child` `animation-delay` for the stagger; infinite loop.
   - `@media (prefers-reduced-motion: reduce)`: animation `none`, static partially-filled state (first 3–4 dots filled).
   - Use only existing custom properties (`--cyan`, `color-mix`). No new hexes.

## Verify (in the worktree)
- `cd site && npm run build` → succeeds.
- Grep gates (must be empty):
  - `rg -n "obs · deep-field" site/` → no matches.
  - `rg -n "r-tr" site/` → no matches.
  - `rg -n '\bsig\b' site/src` → no stray `sig` telemetry label left (constellation/other unrelated uses are fine; confirm none is the old eyebrow label).
- Best-effort visual sanity if a preview is trivially available; otherwise rely on build + grep.

## Notes for executor
- This is the entire stage — small, self-contained, no cross-file logic beyond the three files.
- Do NOT modify the Landing.astro `<script>` (parallax/reveal) — that's Stage 02.
- A pre-commit hook may reformat; if it rewrites files, re-stage and retry the commit.
