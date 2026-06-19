# Stage 03 — Surface de-chrome — Spec

## Goal

Flatten the surfaces. Today every panel, icon, and accent carries a neon glow,
a heavy elevation shadow, and a backdrop-blur — the "instrument" reads as busy.
Strip all of that so panels are **flat fills with a single hairline border**,
icons keep their brand color but lose the halo, and nothing floats on a big dark
shadow. This is the herdr.dev surface quality: hairlines and flat panels, not
glow + blur + drop-shadow.

Keep the dark jewel palette and the brand accent colors (teal/ruby/gold) — they
stay as `color` / `border-color`, just without the glow. Whitespace, type scale,
radius tuning, and dead-CSS pruning are **stage 4**.

## In scope — remove these three chrome categories everywhere in `landing.css`

1. **Neon glows** — every `box-shadow: 0 0 …` colored halo and every
   `filter: drop-shadow(0 0 …)`. Delete the glow; keep the element's `color`,
   `border`, and `background`. Affected (non-exhaustive): `.claw` drop-shadow,
   `.pdot`, `.statusnote .sdot`, `.starchip:hover`, `.starchip .ico`,
   `.starbtn .ico`, `.bk`, `.telem .k::before`, `.label::before`, `.t-teal`/
   `.t-gold`/`.t-ruby`, `.card .cardicon`, `.step.gate` (the `0 0 44px` part),
   `.step.gate .snum`, `.step.gate .model::before`, `.step.curate .model::before`.
2. **Heavy elevation drop-shadows** — every `box-shadow` with a large dark
   offset/blur (`0 30px 70px`, `0 30px 60px`, `0 24px 50px`, etc.) AND every
   `box-shadow: inset 0 1px 0 rgba(255,255,255,…)` top-highlight gloss. Remove
   them so panels are flat. Affected: `.btn`, `.statusnote`, `.cmd`, `.shot img`,
   `.telem`, `.card`, `.card:hover`, `.hnode`, `.step`, `.step.gate` (the inset),
   `.cp-card`.
3. **Backdrop blur** — delete `backdrop-filter: blur(…)` from every selector
   (`.status`, `.statusnote`, `.starchip`, `.cmd`, `.telem`, `.card`,
   `.how-strip`, `.step`, `.cp-card`). The background is flat now, so blur does
   nothing but cost. Leave the panel `background: color-mix(var(--panel) …%, …)`
   as-is — over the flat `--bg` it renders as a flat tint, which is what we want.

## In scope — soften interactions to flat

4. **Card hover → border-only.** Remove `.card`'s `transform: translateY(-5px)`
   hover lift and its hover shadow; `.card:hover` keeps only the
   `border-color: var(--line-2)` change. Update `.card`'s `transition` to drop
   `transform`/`box-shadow` (keep `border-color`).
5. **Remove the `.card::before` top accent-gradient hairline** and its
   `.card:hover::before` rule — decorative.

## Out of scope — keep

- **Hairline borders** (`1px solid var(--line)` / `var(--line-2)`) on every
  panel — these are the structure; keep them all.
- Brand accent **colors** on icons/labels/keys (`color`, `border-color` tints on
  `.t-teal/.t-gold/.t-ruby`, `.cardicon`, `.label::before` bar, `.telem .k::before`
  bar, `.step.gate` border) — keep the color, only the glow goes.
- Flat panel background tints (the `color-mix(... transparent)` fills) — keep.
- `.hbox`/`.how-strip.cred`/`.step.gate` accent **border-color** emphasis — keep
  (color only, no glow/shadow).
- Hero `h1 .glow` ruby accent word + underline — brand, not chrome. Leave (stage 4
  may revisit the skew).
- Border-radius values, whitespace, type scale, the below-fold `.rev` reveal —
  **stage 4** / untouched here.
- `.compare`/`.cmdrow` hover background tints — flat, keep.

## Acceptance criteria

1. No colored glow anywhere: `rg -n 'box-shadow:\s*0 0|drop-shadow\(0 0' site/src/styles`
   returns nothing.
2. No `backdrop-filter` anywhere: `rg -n 'backdrop-filter' site/src/styles`
   returns nothing.
3. No large elevation shadow or inset white-gloss: `rg -n 'inset 0 1px 0 rgba\(255,255,255|0 30px|0 24px' site/src/styles`
   returns nothing.
4. Panels still have their hairline borders and flat tinted fills; icon tiles
   still show their teal/ruby/gold color (border + glyph), just no halo.
5. Card hover changes only the border color — no lift, no shadow.
6. `cd site && bun run check` → 0 errors; `cd site && bun run build` → success.
7. No content/markup change — `landing.css` is the only file touched (plus
   `Hero.astro` only if the `.glow` is genuinely a removed glow, which it is NOT
   — so expect `landing.css` only).

## Verification (website-only — no cargo)

- `cd site && bun run check && bun run build` green.
- The three grep gates above all return empty.
