# Stage 03 — Surface de-chrome — Plan

Executor implements this in the stage worktree. Target file:
`site/src/styles/landing.css` (ONLY — no markup changes expected). Spec:
`03-dechrome-spec.md`. Work selector-by-selector; keep borders, colors, and flat
panel fills — remove only glows, elevation/gloss shadows, and backdrop-blur.

## Tasks

1. **Strip every neon glow.** For each rule with `box-shadow: 0 0 …<color>` or
   `filter: drop-shadow(0 0 …<color>)`, delete that glow declaration (and the
   whole `filter:` if drop-shadow was its only content). Keep the rule's other
   declarations. Hit at least: `.claw` (delete the `filter`), `.pdot`,
   `.statusnote .sdot`, `.starchip:hover` (drop the box-shadow, keep
   `border-color`/`color`), `.starchip .ico` (delete `filter`), `.starbtn .ico`
   (delete `filter`), `.bk`, `.telem .k::before`, `.label::before`, `.t-teal`,
   `.t-gold`, `.t-ruby`, `.card .cardicon`, `.step.gate` (remove only the
   `0 0 44px …` glow segment), `.step.gate .snum`, `.step.gate .model::before`,
   `.step.curate .model::before`.

2. **Remove heavy elevation + inset-gloss shadows.** Delete the `box-shadow`
   declarations that are large dark drops (`0 30px 70px`, `0 30px 60px`,
   `0 24px 50px`, etc.) and the `inset 0 1px 0 rgba(255,255,255,…)` top-highlights
   from: `.btn`, `.statusnote`, `.cmd`, `.shot img` (keep its `border`),
   `.telem`, `.card`, `.card:hover`, `.hnode`, `.step`, `.step.gate`, `.cp-card`.
   If a rule's `box-shadow` was only these, remove the property entirely.

3. **Delete every `backdrop-filter: blur(…)`** from `.status`, `.statusnote`,
   `.starchip`, `.cmd`, `.telem`, `.card`, `.how-strip`, `.step`, `.cp-card`.
   Leave each rule's `background` as-is.

4. **Flatten card hover.** In `.card`, change `transition: transform .25s,
   border-color .25s, box-shadow .25s;` to `transition: border-color .25s;` and
   ensure no `transform`/`box-shadow` remain on `.card`. In `.card:hover`, remove
   `transform: translateY(-5px)` and the `box-shadow`; keep only
   `border-color: var(--line-2)`.

5. **Remove the card top accent line.** Delete the `.card::before { … }` rule and
   the `.card:hover::before { … }` rule.

Do NOT touch: any `border` / `border-color` / `border-radius`, panel
`background` fills, brand `color:` values, `.compare`/`.cmdrow` hover background
tints, the `.rev` reveal block, or the hero `h1 .glow` ruby word.

## Verification (run in the stage worktree)

- `cd site && bun install` (fresh worktree may lack node_modules).
- `cd site && bun run check` → 0 errors.
- `cd site && bun run build` → success (~9 pages, links + pagefind green).
- Grep gates (all must return nothing):
  - `rg -n 'box-shadow:\s*0 0|drop-shadow\(0 0' site/src/styles`
  - `rg -n 'backdrop-filter' site/src/styles`
  - `rg -n 'inset 0 1px 0 rgba\(255,255,255|0 30px|0 24px' site/src/styles`

Leave all changes uncommitted; the stage-runner commits and lands.
