# Stage 08 — Flatten background to monotone paper (plan)

Implements `08-flat-bg-spec.md`. Remove the gradient/glow ambient layers, flatten `.void`, keep texture+tone monotone. Astro site (`site/`), CSS/markup only, no deps, no new hexes. Keep telemetry/zoom/reveal/spotlight/learning/HUD + the `PAPER-EXPERIMENT` switcher (17 textures × 7 tones).

## Baseline
- `cd site && (npm ci if node_modules absent) && npm run build` → green before edits.

## Task 1 — Remove gradient/glow layers
- `site/src/layouts/Landing.astro`: delete `<div class="fx depthbloom">`, `<div class="fx nebula">`, `<div class="fx ceiling">`, `<div class="fx horizon">`, `<div class="fx horizon-glow">`.
- `site/src/styles/landing.css`: delete the `.depthbloom`, `.nebula`, `.horizon`, `.ceiling`, `.horizon, .ceiling {…}`, and `.horizon-glow` rules.
- Grep after: `rg -n "depthbloom|nebula|horizon-glow|\.ceiling|\.horizon\b" site/` → empty (aside from any unrelated word matches; the layers/rules gone).

## Task 2 — Flatten `.void`
- `landing.css`: replace the `.void` multi-radial-gradient + vignette background with a flat `background: var(--bg);`. No gradients.

## Task 3 — Keep texture/tone monotone
- `landing.css`: verify each `:root[data-tone=…]` tint is a flat uniform color (color-mix of a token into `--bg`) — no gradient.
- Adjust textures so they read as tonal relief only (lightness modulation), not a colored/brown veil: tune `mix-blend-mode` and any tint so the texture keeps the tone's hue (monochrome tooth). The surface should look like the same flat tone with a tactile finish.
- All static; do not touch the telemetry layers, switcher, or rAF.

## Verify (in the worktree)
- `cd site && npm run build` → succeeds; `npx astro check` → 0 errors.
- Grep gates: Task-1 layers gone; `.void` has no `radial-gradient`; `PAPER-EXPERIMENT` + telemetry present; no new hex literals.
- Legibility: reason/spot-check H1/sub/cards/code over a few texture×tone combos on the now-flat base.

## Notes for executor
- Model: openai/gpt-5.4 / max (pinned). Mint a fresh mimo handle.
- The goal is a flat, even, monotone PAPER field — no gradient, no warm wash. If a texture still muddies the tone, neutralize its tint (keep relief, drop color).
- `prek` hook may abort on missing `.pre-commit-config.yaml` → `PREK_ALLOW_NO_CONFIG=1`.
- Do not remove the texture/tone switcher or telemetry; do not re-introduce gradients.
