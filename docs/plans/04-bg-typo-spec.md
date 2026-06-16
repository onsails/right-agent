# Stage 04 — Background typographic experiment (spec)

## Goal
The current background (starfield + constellation + nebula) reads as a cliché "space landing." Replace it with a **typographic background structure**. This is an EXPERIMENT: ship **all 4 concepts** behind a temporary switcher so the user compares them live and picks one (stage 05 locks in the pick + removes the rest).

Astro site (`site/`). Vanilla CSS + the existing inline `<script>`; no new dependencies, no webfonts beyond those already used (`--display`, `--mono`, `--sans`).

## Scope of replacement
- **Remove** the cliché layers: `.stars-far`, `.stars-near`, `.twinkle` (in `Landing.astro` + their CSS) and the Hero `.constellation` SVG (`Hero.astro` + its CSS).
- **Keep** the dark jewel atmosphere: `.void` (plum base), `.grain`, `.scan`. Also keep `.depthbloom` and the `.horizon`/`.ceiling`/`.horizon-glow` perspective glow UNLESS a given variant overrides them (see per-variant notes) — they are not "stars," they're ambient depth. Default: keep them; let variant CSS dim them if it clashes.
- Add a new set of background layers for the typographic concepts. They live alongside `.void`/`.grain`/`.scan` and participate in the existing pointer/scroll **parallax** (reuse `.fx[data-depth]`, or extend the parallax `querySelector` to include the new layers — keep ONE shared rAF).
- Must NOT break: the active-section zoom (stage 03), `.rev` reveal, `.card` spotlight, the `learning` indicator.

## The switcher (temporary `BG-EXPERIMENT` scaffolding)
- Small fixed control (bottom-LEFT, to not collide with anything; mono/jewel styled) listing **A / B / C / D** with short labels (`wordmark` / `telemetry` / `manifesto` / `blueprint`).
- Click sets `document.documentElement.dataset.bg = 'a'|'b'|'c'|'d'`, persisted to `localStorage['bg']`; restore on load (default `'a'`).
- **Always visible during the experiment** — do NOT hide it under `prefers-reduced-motion` (that confused the last experiment). Only the *animations* inside variants degrade under reduced-motion; the static typographic structure still shows so it's comparable.
- All scaffolding wrapped in `BG-EXPERIMENT:start/end` delimiters (markup, JS, CSS) for clean stage-05 removal. Namespace everything `bg*`/`data-bg` to avoid any collision with removed `FX-EXPERIMENT` names.

## Variants (keyed off `:root[data-bg=…]`; only the active one is visible)
Each is a full-bleed layer (or small set of layers) behind content; very low contrast so headline/body stay legible (brand rule: "atmosphere serves legibility, not busy"). Use brand vocabulary, jewel custom properties only (no new hexes).

- **A · ghost wordmark** — a few huge `--display` words from the brand lexicon (`RIGHT`, `AGENT`, `SANDBOXED`, `SECURE`, `ISOLATED`) at very low contrast (~4–8% against bg), oversized so they bleed off the edges, distributed across 2–3 `data-depth` parallax layers. Mostly static + parallax; optional ultra-slow drift. Poster/editorial.
- **B · mono telemetry field** — a full-bleed `--mono` field of faint instrument readouts: coordinates, hex, short telemetry (`sig ●●●●●`, `lat 12ms`, `obs · deep-field`, `0x7f3a…`, `RA 14h · DEC +12°`). Tiled/columned, very low opacity. A handful of glyphs flicker/drift on a slow loop (cheap; not every char). Terminal/instrument feel.
- **C · drifting manifesto** — 3–4 horizontal bands, each a large faint line repeating a brand phrase (`sandboxed multi-agent runtime`, `credentials stay outside the box`, `secure by default`, `the box is closed; you just use it`), translating horizontally (marquee) at different speeds/directions per depth. Kinetic typography. Pause/limit under reduced-motion.
- **D · engineering blueprint** — a faint ruled grid (CSS gradients, fades at edges) + a few rotated `--mono` captions (e.g. `fig.01 · runtime`, `scale 1:1`, `rev a`) + giant faint section numerals. Optionally the numeral tracks the active section (reuse stage-03's nearest-section logic); if so, keep it cheap. Structural/precise.

## Accessibility / perf
- `@media (prefers-reduced-motion: reduce)`: disable all drift/flicker/marquee animations (variants render static); the switcher stays visible; reuse/extend the existing reduced-motion rule. The shared rAF must skip any per-variant animation work under reduce.
- Keep it cheap: prefer CSS animations for drift/marquee; the rAF only does what it already does (parallax + active-section) plus, at most, light per-variant work that early-returns when that variant isn't active. No layout thrash; compositor-friendly transforms/opacity only.
- Low opacity everywhere — verify H1, sub copy, cards, code blocks remain fully legible over every variant.

## Verification
- `cd site && npm run build` succeeds; `npx astro check` 0 errors.
- Grep: `rg -n "stars-far|stars-near|constellation" site/` → empty (cliché layers gone). `BG-EXPERIMENT` blocks present + delimited.
- Preview (the point): switcher flips A/B/C/D; each is a distinct typographic background; text stays legible; parallax + active-section zoom + reveal still work; reduced-motion disables motion but keeps the static structure + switcher.

## Out of scope
- Removing variants / switcher (stage 05, after the pick).
- Touching telemetry copy or the FX zoom (settled stages 01/03).
- New webfonts.
