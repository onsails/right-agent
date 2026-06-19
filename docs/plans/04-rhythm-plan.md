# Stage 04 — Reading column + narrow container + soften reveal — Plan

Executor implements this in the stage worktree. Target files:
`site/src/styles/tokens.css` and `site/src/styles/landing.css` ONLY. Spec:
`04-rhythm-spec.md`. Small, surgical change — no markup, no other rules.

## Tasks

1. **Narrow the container.** In `site/src/styles/tokens.css`, change
   `--maxw: 76rem;` to `--maxw: 70rem;`. Leave every other token unchanged.

2. **Unify the section text reading measure.** In `site/src/styles/landing.css`:
   - On the section-level `h2` rule (the one with
     `font-size: clamp(1.6rem, 3vw, 2.1rem)` — NOT the hero `h1`), add
     `max-width: 66ch;`.
   - On the `.lead` rule, change `max-width: 65ch;` to `max-width: 66ch;`.
   Do not touch any other selector's width. Do not add a max-width to `h1`,
   `.label`, or the panels.

3. **Soften the reveal.** In `site/src/styles/landing.css`, the `.rev` rule:
   - change `transform: translateY(24px)` to `transform: translateY(8px)`;
   - change the transition from `.9s` to `.35s` for BOTH the opacity and
     transform (e.g. `transition: opacity .35s cubic-bezier(.2,.7,.2,1),
     transform .35s cubic-bezier(.2,.7,.2,1);`).
   Leave `.rev.in`, the `.rev.d1..d5` delays, and the
   `@media (prefers-reduced-motion: reduce)` `.rev{...}` line unchanged.

## Verification (run in the stage worktree)

- `cd site && bun install` (fresh worktree may lack node_modules).
- `cd site && bun run check` → 0 errors.
- `cd site && bun run build` → success (~9 pages, links + pagefind green).
- `rg -n '76rem|translateY\(24px\)|max-width: ?65ch' site/src/styles` → no hits
  (old width, old reveal distance, old lead measure all replaced).

Leave all changes uncommitted; the stage-runner commits and lands.
