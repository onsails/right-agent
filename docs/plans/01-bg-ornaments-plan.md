# Stage 01 — Background ornaments — Plan

Executor implements this in the stage worktree. Target files:
`site/src/layouts/Landing.astro` and `site/src/styles/landing.css`. Purely
subtractive; do not add or restyle content. Spec: `01-bg-ornaments-spec.md`.

## Tasks

1. **Edit `site/src/layouts/Landing.astro`** — in `<body>`, delete these three
   ornament layers, keeping `<div class="fx void"></div>` directly above them:
   - the `<div class="fx paper" aria-hidden="true"></div>` line;
   - the entire `<div class="fx bgfx bgfx-telemetry bgfx-telemetry-1" …>…</div>`
     block (its three `bgtelem-col` children);
   - the entire `<div class="fx bgfx bgfx-telemetry bgfx-telemetry-2" …>…</div>`
     block (its three `bgtelem-row` children);
   - the entire `<div class="hud" aria-hidden="true">…</div>` block (four
     `.edge` spans + the `secure-by-default` `.read r-bl` span).

   Leave the `<nav class="topnav">`, the `<slot />`, and the `<script>` exactly
   as they are. Do not touch the active-section / reveal / card-spotlight script
   (deferred to stage 02).

2. **Edit `site/src/styles/landing.css`** — delete the now-orphaned rules and
   their `/* ==== … ==== */` section-header comments:
   - the `/* ==== HUD frame ==== */` block (`.hud` + `.edge` + corners +
     `.read` + `.r-bl`);
   - the `/* ==== background telemetry ==== */` block through
     `@keyframes bgtelemFlicker` (all `.bgfx-telemetry*`, `.bgtelem-*`,
     `.bgtelem-flicker*`);
   - the `.paper` / `.paper::before` / `.paper::after` rules;
   - the `@media (max-width: 900px)` block whose body is only
     `.bgfx-telemetry-1` / `.bgtelem-*` rules.

   Keep `.fx { … }` and `.void { … }` (the flat background base). Keep every
   hero, section, card, loop, compare, control-plane, footer, and `.rev` rule.

3. **Fix the reduced-motion rule** — in the
   `@media (prefers-reduced-motion: reduce)` block, remove only the
   `.bgtelem-flicker` selector from the comma list on the
   `…{ animation:none !important; }` line. Keep `.claw`, `.clawwrap::after`,
   `.pdot`, `.packet`, `.loopspark`, `.statusnote .sdot`, `.learn-dots i`.

## Verification (run in the stage worktree)

- `cd site && bun install` (worktree has no node_modules) — only if build fails
  for a missing-deps reason; the integration worktree already installed, but a
  fresh stage worktree may need it.
- `cd site && bun run check` → 0 errors.
- `cd site && bun run build` → success, 7 pages, links + pagefind green.
- `rg -n 'bgtelem|bgfx-telemetry|\.paper\b|class="hud"' site/src` → no hits.

Leave all changes uncommitted; the stage-runner commits and lands.
