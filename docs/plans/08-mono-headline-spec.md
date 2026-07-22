# Stage 08 — Monospace headline (herdr #3) — Spec

## Goal

Give the landing headlines the monospace, code-editor character that defines
herdr.dev. Switch the hero `h1` and the section `h2` headings from the Chakra
Petch display face to **JetBrains Mono** (already loaded; herdr uses this exact
font). Live preview confirmed: JetBrains Mono at the headline size reads clean,
narrow, and pairs naturally with the mono eyebrow above it; Martian Mono was too
wide/heavy (nearly overflowed the column) and was rejected.

Scope is the **headlines only** — not the wordmark, card titles, step titles, or
docs headings. Big headings go mono (herdr feel); smaller nested titles stay in
the display sans so the hierarchy stays legible and we don't re-introduce noise.

## In scope

1. **New token.** In `tokens.css`, add
   `--headline: 'JetBrains Mono Variable', ui-monospace, monospace;`
   (JetBrains Mono is already `@import`ed in `fonts.css` — no new import).

2. **Hero `h1` → mono.** In `landing.css`, the `h1` rule:
   `font-family: var(--display)` → `var(--headline)`, and tighten
   `letter-spacing: -.02em` → `-.04em` (mono needs a tighter track at size).
   Keep font-size clamp, line-height, weight 700, margin, `max-width: 20ch`,
   color.

3. **Section `h2` → mono.** The `h2` rule: `font-family: var(--display)` →
   `var(--headline)`, `letter-spacing: -.01em` → `-.03em`. Keep everything else
   (size clamp, weight, margin, color, `max-width: var(--readw)`).

4. **De-skew the accent word.** The hero `.glow` span currently has
   `transform: skewX(-18deg)`; a skewed italic-like slant reads badly in a
   monospace face (previewed). Remove the `transform: skewX(-18deg)` declaration
   from `h1 .glow`. Keep its `color: var(--ink)`, `display:inline-block`,
   `position:relative`, and the `h1 .glow::after` teal underline unchanged.

## Out of scope — keep as Chakra Petch (`--display`)

- `.wm` (brand wordmark "right agent") — identity, separate decision.
- `.card h3`, `.hname`, `.step .stitle` — small nested sub-titles.
- `starlight.css` docs headings.
- Everything from stages 1–7 (flat bg, motion, de-chrome, reading column, frame,
  polish, icon noise). Untouched.
- Copy/content — unchanged.

## Acceptance criteria

1. Hero `h1` and every section `h2` render in JetBrains Mono; the mono eyebrow +
   mono headline read as one coherent system.
2. The "messaging it" accent word is upright (no skew), still ruby with the teal
   underline.
3. Wordmark, card titles, step titles, and docs headings are still Chakra Petch
   (unchanged).
4. Headline still fits its column (no overflow) at desktop and mobile.
5. `cd site && bun run check` → 0 errors; `cd site && bun run build` → success.
6. Files changed: `tokens.css`, `landing.css` only.

## Verification (website-only — no cargo)

- `cd site && bun run check && bun run build` green.
- `rg -n '\--headline' site/src/styles/tokens.css` → present.
- `rg -n 'var\(--headline\)' site/src/styles/landing.css` → exactly the h1 and h2
  rules (2 hits).
- `rg -n 'skewX' site/src/styles/landing.css` → no hits (glow de-skewed).
- Visual: hero + section headings mono; wordmark/card titles still Chakra Petch.
