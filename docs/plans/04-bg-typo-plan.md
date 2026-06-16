# Stage 04 — Background typographic experiment (plan)

Implements `04-bg-typo-spec.md`. Replace the cliché starfield/constellation with 4 switchable typographic background concepts (A wordmark · B telemetry · C manifesto · D blueprint) behind a temporary `BG-EXPERIMENT` switcher. Astro site (`site/`), vanilla CSS + existing inline `<script>`, no deps. Keep dark base + grain + scan; don't break parallax / active-section zoom / reveal / spotlight / learning.

## Baseline
- From the worktree: `cd site && (npm ci if node_modules absent) && npm run build` → confirm green before edits.

## Task 1 — Remove cliché layers
- `site/src/layouts/Landing.astro`: delete the `.fx stars-far twinkle` and `.fx stars-near` divs.
- `site/src/styles/landing.css`: remove `.stars-far`, `.stars-near`, `.twinkle` rules and the `tw` keyframes; remove `stars-far`/`stars-near`/`twinkle` from the reduced-motion `animation:none` list.
- `site/src/components/Hero.astro`: remove the `.constellation` SVG block; `landing.css`: remove `.constellation*` rules.
- Leave `.void`, `.depthbloom`, `.nebula`, `.horizon`, `.ceiling`, `.horizon-glow`, `.grain`, `.scan` in place (ambient depth). Variant CSS may dim `.nebula`/`.depthbloom` if it clashes.

## Task 2 — Background layers (markup)
- `Landing.astro`: add a `BG-EXPERIMENT:start/end`-delimited block of background layers for the 4 concepts, placed with the other `.fx` layers (behind `.wrap`). Give parallax-able layers `data-depth` and ensure the parallax loop animates them (reuse `.fx[data-depth]`, or extend the parallax `querySelectorAll` selector to include the new `.bgfx` layers — keep ONE shared rAF).
- Each concept is shown only when `:root[data-bg]` matches; default hidden otherwise.

## Task 3 — Switcher (temp scaffolding, self-contained)
- `Landing.astro`: `BG-EXPERIMENT`-delimited fixed control bottom-LEFT, 4 buttons A/B/C/D (+ labels wordmark/telemetry/manifesto/blueprint), `data-bg-choice` attrs.
- JS (existing `<script>`, `BG-EXPERIMENT`-delimited): restore `localStorage['bg']` (default `'a'`) → set `document.documentElement.dataset.bg`; wire button clicks; set `aria-pressed`. **Do NOT hide the switcher under reduced-motion** (only animations degrade).
- `landing.css`: switcher styles using jewel custom properties; no new hexes. Namespace `bg*` / `data-bg`.

## Task 4 — Variant A · ghost wordmark
- CSS under `:root[data-bg="a"]`: a few huge `--display` words (brand lexicon) at ~4–8% contrast, oversized to bleed off edges, on 2–3 `data-depth` layers. Static + parallax; optional ultra-slow drift (CSS, disabled under reduced-motion).

## Task 5 — Variant B · mono telemetry field
- Markup + CSS under `:root[data-bg="b"]`: full-bleed `--mono` field of faint instrument readouts (coords/hex/`sig ●●●●●`/`lat`/`obs · deep-field`/`RA…DEC…`). Tiled or columned, very low opacity. A few glyphs flicker/drift via CSS keyframes (cheap, not every char; disabled under reduced-motion).

## Task 6 — Variant C · drifting manifesto
- Markup + CSS under `:root[data-bg="c"]`: 3–4 horizontal bands, each a large faint brand phrase repeated, `translateX` marquee at different speeds/directions per depth (CSS keyframes). Ensure seamless loop (duplicate content). Disabled/paused under reduced-motion.

## Task 7 — Variant D · engineering blueprint
- CSS under `:root[data-bg="d"]`: faint ruled grid (CSS linear-gradients, masked to fade at edges) + a few rotated `--mono` captions + giant faint section numerals. Optional: the numeral tracks the active section (reuse stage-03 nearest-section/`data-active` logic — cheap, no second loop). Static-friendly.

## Task 8 — Reduced motion + isolation
- `@media (prefers-reduced-motion: reduce)`: all variant drift/flicker/marquee `animation:none`; variants render static; switcher stays visible; shared rAF skips per-variant animation work under reduce.
- Exactly one shared rAF drives parallax + active-section (+ optional blueprint numeral). No competing loops. Variant JS branches early-return when their `data-bg` isn't active.

## Verify (in the worktree)
- `cd site && npm run build` → succeeds; `npx astro check` → 0 errors.
- Grep gates: `rg -n "stars-far|stars-near|constellation" site/` → empty; `rg -n "BG-EXPERIMENT" site/` → present (delimited, for stage 05).
- Legibility check (reason/spot-check if preview trivial): H1/sub/cards/code legible over every variant.

## Notes for executor
- Model: openai/gpt-5.4 / max (pinned). Mint/track a fresh mimo handle for this stage.
- Keep each variant's markup/CSS/JS grouped and `BG-EXPERIMENT`-delimited so stage 05 can remove the 3 losers + switcher cleanly.
- `prek` hook may abort on missing `.pre-commit-config.yaml` → `PREK_ALLOW_NO_CONFIG=1`.
- Do NOT remove any variant — the comparison is the deliverable.
