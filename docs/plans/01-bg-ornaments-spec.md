# Stage 01 — Background ornaments — Spec

## Goal

Strip the decorative background noise from the landing page so content leads.
Today the hero is dominated by three fixed-position ornament layers that sit
behind everything and carry zero information: a fake "telemetry" field, a paper
texture, and a HUD corner-frame with a `secure-by-default` readout. Remove them.
The background becomes a flat, calm `var(--bg)` plum. Keep the dark jewel brand.

This is the first declutter iteration. It is **purely subtractive** — no new
markup, no restyling of content. Motion, glows/shadows, borders, and whitespace
are explicitly deferred to later stages.

## In scope — remove

Markup in `site/src/layouts/Landing.astro` (`<body>`):

- `<div class="fx paper" aria-hidden="true"></div>` — paper texture.
- Both `<div class="fx bgfx bgfx-telemetry bgfx-telemetry-1 …">` and
  `… bgfx-telemetry-2 …` blocks (the three `bgtelem-col`s and three
  `bgtelem-row`s of fake `obs · deep-field`, `sig ●●●●●`, `lat 12ms`,
  `coord …`, `RA … DEC …` text).
- The entire `<div class="hud" aria-hidden="true">…</div>` block — the four
  `.edge` corner brackets and the `secure-by-default` `.read r-bl` label.

Matching dead CSS in `site/src/styles/landing.css` (remove the rules now
orphaned, including their section-header comments):

- HUD frame block: `.hud`, `.hud .edge`, `.hud .tl/.tr/.bl/.br`, `.hud .read`,
  `.hud .r-bl`.
- Background-telemetry block: `.bgfx-telemetry`, `.bgfx-telemetry-1`,
  `.bgtelem-col`, `.bgtelem-row`, the `:nth-child` padding variants,
  the `span` opacity rules, `.bgfx-telemetry-2`, `.bgtelem-flicker`,
  `.bgtelem-flicker-alt`, and `@keyframes bgtelemFlicker`.
- `.paper`, `.paper::before`, `.paper::after`.
- The `@media (max-width: 900px)` block that styles **only**
  `.bgfx-telemetry-1` / `.bgtelem-*` (it becomes entirely dead).
- In the `@media (prefers-reduced-motion: reduce)` rule, drop `.bgtelem-flicker`
  from the `animation: none` selector list (the class no longer exists). Leave
  every other selector in that rule untouched.

## Out of scope — keep exactly as-is (deferred)

- `<div class="fx void"></div>` and `.fx` / `.void` CSS — this **is** the flat
  background we keep.
- Hero `learn-dots` animated indicator and `.section` scroll-zoom — **stage 02**
  (motion).
- `h1 .glow`, card/panel borders, `box-shadow`, `backdrop-filter` — **stage 03**
  (de-chrome).
- `.claw` pulse + `.clawwrap::after` spin ring — **stage 02**.
- Mono eyebrow/label tags, type scale, whitespace — **stage 04**.

## Acceptance criteria

1. The hero renders headline + subhead + CTAs + product screenshot with a flat
   `var(--bg)` background behind them — no ghost telemetry text, no diagonal
   paper hatch, no corner brackets, no `secure-by-default` readout anywhere on
   the page.
2. No CSS selector for `.paper`, `.bgfx`, `.bgfx-telemetry*`, `.bgtelem*`, or
   `.hud` remains in `landing.css` (grep gate). `.fx` and `.void` remain.
3. `cd site && bun run build` succeeds (Astro build + starlight-links-validator
   + pagefind), zero errors.
4. `cd site && bun run check` (astro check) reports 0 errors.
5. No other section's markup or styling changes. Diff is confined to
   `Landing.astro` and `landing.css`.

## Verification (website-only — no Rust/cargo)

- `cd site && bun run check && bun run build` both green.
- `rg -n 'bgtelem|bgfx-telemetry|\.paper|class="hud"|\bhud\b' site/src` returns
  no landing-page ornament hits (the `.fx void` base and unrelated matches are
  fine).
