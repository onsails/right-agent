# Stage 07 — Texture experiment v3 (plan)

Implements `07-texture-v3-spec.md`. (1) Kill remaining bg motion (`.scan` + grid `flow`); (2) add 10 tactile relief textures; (3) expand `data-tone` to a ~7-color palette derived via `color-mix` of jewel tokens. ~17 textures × ~7 colors behind the existing `PAPER-EXPERIMENT` switcher. Astro site (`site/`), vanilla CSS + SVG + existing inline `<script>`, no deps, no new hexes. Keep telemetry/zoom/reveal/spotlight/learning.

## Baseline
- `cd site && (npm ci if node_modules absent) && npm run build` → green before edits.

## Task 1 — Kill background motion
- `Landing.astro`: delete `<div class="fx scan">`.
- `landing.css`: delete `.scan` rule + `@keyframes sweep`; remove `.scan` from the reduced-motion `animation:none` list. Remove `animation: flow …` from `.ceiling` and `.horizon` (delete `@keyframes flow` if then unused); keep those elements static. Keep `.void`/`.paper`/`.depthbloom`/`.nebula`/telemetry(+flicker).
- Grep after: `rg -n "@keyframes sweep|@keyframes flow|class=\"fx scan\"|animation: *flow" site/` → empty.

## Task 2 — 10 tactile textures (`Landing.astro` SVG defs + `landing.css`, `:root[data-tex=…]`)
- Add: kraft, coldpress, letterpress, corrugated, deckle, sandpaper, burlap, slate, leather, cork.
- Each STATIC, and each with a tactile relief cue: `mix-blend-mode` (overlay/soft-light/hard-light) + paired highlight/shadow or emboss/deboss (layered offset gradients, or SVG `feDiffuseLighting`/`feSpecularLighting`/`feDisplacementMap` over `feTurbulence`). Clearly visible on `--bg`; keep text legible.
- Group each texture's filter+CSS, `PAPER-EXPERIMENT`-delimited.

## Task 3 — Color palette (`landing.css` `:root[data-tone=…]`)
- Expand to: `plum`(default), `kraft`, `slate`, `sepia`, `olive`, `wine`, `ink`.
- Each tone = `color-mix(in srgb, var(--bg), <existing jewel token> X%)` for the paper base tint + the texture tint. NO new hexes. Keep dark enough for existing light text.
- Preserve current behavior: `dark`→`plum`, `warm`→`kraft` (rename chips; keep localStorage migration sane — unknown stored value falls back to default).

## Task 4 — Switcher (extend, `PAPER-EXPERIMENT`-delimited)
- `Landing.astro`: texture row ~17 chips (wrap), color row ~7 chips (wrap), `data-tex-choice`/`data-tone-choice`.
- JS: wire all chips (same handlers); defaults `tex=cardstock`, `tone=plum`; unknown stored values fall back to defaults. Always visible. No new loops.
- `landing.css`: switcher styles accommodate wider/wrapping rows.

## Verify (in the worktree)
- `cd site && npm run build` → succeeds; `npx astro check` → 0 errors.
- Grep gates: bg-motion grep (Task 1) empty; `PAPER-EXPERIMENT` present; telemetry present.
- Confirm no new hex literals added for tones (`color-mix` of tokens only).
- Legibility: reason/spot-check H1/sub/cards/code across a sample of texture×color combos (esp. heavy textures over light-ish tones).

## Notes for executor
- Model: openai/gpt-5.4 / max (pinned). Mint a fresh mimo handle.
- "Tactile" is the bar — each new texture must read as a touchable rough surface (relief), not a flat veil. Bolder than the removed `fiber`.
- Keep texture/color CSS grouped + `PAPER-EXPERIMENT`-delimited for stage-08 lock-in removal.
- `prek` hook may abort on missing `.pre-commit-config.yaml` → `PREK_ALLOW_NO_CONFIG=1`.
- Do NOT remove the existing 7 textures or break telemetry/zoom. Background must end up fully static (only telemetry flicker remains).
