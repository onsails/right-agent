# Stage 09 — Text accent color experiment (plan)

Implements `09-accent-experiment-spec.md`. Add an `accent` switcher row that overrides `--warm`/`--warm-soft` with 7 harmonious candidates. Astro site (`site/`), CSS/markup + the existing inline `<script>`, no deps. Website-only — NO cargo/Rust tests.

## Baseline
- `cd site && (npm ci if node_modules absent) && npm run build` → green before edits.

## Task 1 — Accent candidates (CSS)
- `landing.css`: add `:root[data-accent="<name>"] { --warm: <hex>; --warm-soft: <hex>; }` for: `gold` (current `#cda14b`/`#e6cf8e`), `champagne`, `amber`, `rose`, `coral`, `sage`, `lilac` (starting values in the spec; tune for harmony + legibility on the dark base). These override the two tokens; all existing `var(--warm…)` usages inherit automatically.
- Set `<html data-accent="champagne">` as the default in `Landing.astro`.

## Task 2 — Switcher row (markup + JS)
- `Landing.astro`: add a third labeled row `accent` to the existing bottom-left switcher panel, 7 chips with `data-accent-choice` (gold/champagne/amber/rose/coral/sage/lilac). Keep it inside the experiment delimiters.
- JS (existing `<script>`): mirror the texture/tone wiring — restore `localStorage['accent']` (default `champagne`, unknown → default), set `document.documentElement.dataset.accent`, set `aria-pressed`, wire clicks. Always visible. No new loops.
- `landing.css`: switcher styles accommodate the third row.

## Verify (in the worktree)
- `cd site && npm run build` → succeeds; `npx astro check` → 0 errors. NO cargo.
- Preview/reason: the accent row flips `--warm` consistently across eyebrow/label/tick/cmd/star/telem; ruby (`--ink`) + teal (`--accent`) unchanged; each candidate legible.

## Notes for executor
- Model: openai/gpt-5.4 / max (pinned). Mint a fresh mimo handle.
- New hexes are expected ONLY for the accent candidate definitions (allowed exception). Do not introduce hexes elsewhere.
- Only `--warm`/`--warm-soft` change per accent; do not edit individual component rules.
- `prek` hook may abort on missing `.pre-commit-config.yaml` → `PREK_ALLOW_NO_CONFIG=1`.
- Keep texture/tone switcher + telemetry/zoom intact; keep everything `PAPER-EXPERIMENT`-delimited for final lock-in.
