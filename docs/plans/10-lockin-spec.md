# Stage 10 — Lock-in final background (spec)

## Decision
User picked the final combo: **texture = `crosshatch`**, **tone = `plum`**, **accent = `champagne`**. Bake it in and remove all experiment scaffolding.

## Changes (Astro site, `site/`)
1. **Remove the `PAPER-EXPERIMENT` switcher entirely** — the bottom-left control markup, its JS (localStorage/click wiring for tex/tone/accent), and its CSS. Remove every `PAPER-EXPERIMENT:start/end` block.
2. **Remove `data-tex`/`data-tone`/`data-accent`** attributes from `<html>` and all `:root[data-tex=…]` / `:root[data-tone=…]` / `:root[data-accent=…]` gated rules — EXCEPT collapse the chosen ones to unconditional:
   - **Texture:** keep only the `crosshatch` texture; apply it unconditionally to `.paper`. Delete all other texture rules and any now-unused SVG `feTurbulence`/lighting filter `<defs>` (crosshatch is pure CSS gradients — most/all SVG filter defs become dead; remove them).
   - **Tone:** keep only `plum`; make the plum `--paper-tint` unconditional. Delete the other tone rules.
   - **Accent:** collapse the `champagne` values into `:root` — set `--warm`/`--warm-soft` to champagne's hexes directly. Delete all `:root[data-accent=…]` rules.
3. Keep everything else exactly as-is: flat `.void` base, the `.paper` layer (now unconditionally crosshatch + plum), telemetry layers (+flicker), active-section zoom, `.rev` reveal, `.card` spotlight, `learning`, HUD frame. Background stays fully static.
4. Remove now-orphaned JS bindings (switcher button arrays, choice handlers, localStorage keys). Remove only what these deletions orphan.

## Result
The landing ships with a single fixed look: flat plum paper with a crosshatch/letterpress texture and a champagne warm accent — no switcher, no dead candidate code.

## Verification
- `cd site && npm run build` succeeds; `npx astro check` 0 errors. (Website-only — NO cargo.)
- Grep gates (MUST be empty): `rg -n "PAPER-EXPERIMENT|data-tex|data-tone|data-accent|fxswitch|paper-chip|data-.*-choice" site/` → no matches.
- Present: the crosshatch texture CSS on `.paper`, plum `--paper-tint`, champagne `--warm`/`--warm-soft` in `:root`, telemetry layers, active-section logic.
- Preview: background = flat plum paper + crosshatch texture + champagne text accent; no switcher; everything else unchanged.

## Out of scope
- Any further visual change to the chosen look (tweak later if needed).
- The eventual master merge / PostHog conflict (handled at PR time).
