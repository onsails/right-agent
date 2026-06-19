# Stage 06 — Polish & fixes — Spec

Four small, independent cleanups bundled. Files: `Hero.astro`, `index.astro`,
`landing.css`. No visual redesign — fixes + a light tag tweak + dead-code prune.

## 1. Static hero (finish the stage-2 goal)

The hero still fades in on load because `class="rev"` remains on its elements
(stage 2 only stripped the `dN` suffixes). Remove the bare `rev` class from the
hero `h1`, `.sub`, `.cta`, and `.shot` in `Hero.astro` so the hero renders fully
on first paint with no entrance animation. (The eyebrow already has no `rev`.)
Below-the-fold `.rev` reveals on sections/telem are unchanged.

## 2. a11y — one banner + a `<main>`

Currently `.topbar` (`<header>`) and the hero (`<header class="hero">`) are both
`banner` landmarks, and there is no `<main>`.
- `Hero.astro`: change `<header class="hero">` → `<section class="hero">`
  (CSS targets `.hero` by class, so styling is unaffected). The closing tag too.
- `index.astro`: wrap the primary content — `<Hero />`, the telem `.wrap`, and all
  `<section class="section">` — in a single `<main>`. Keep `<Footer />` AFTER
  `</main>` (so the footer's nearest ancestor is body → it stays a `contentinfo`
  landmark). Result: one `banner` (.topbar), one `main`, one `contentinfo`.
- No CSS needed for `<main>` (transparent block).

## 3. Simplify the mono eyebrow / label tags (light)

The eyebrow and section labels use very wide tracking (`letter-spacing: .24em`)
which reads as shouty. Reduce it to `.14em` on `.eyebrow` and `.label` only.
Keep their font, uppercase, color, the `▸` marker, and the `.label::before` dash.
Nothing else about these tags changes. (Light touch — user can iterate.)

## 4. Prune dead CSS (verified unused — 0 `.astro` refs)

Remove these now-orphaned rules from `landing.css` (each confirmed to have NO
`class="…"` usage anywhere in `site/src/**/*.astro`):
- `.status`
- `.pdot` and `@keyframes blink` (blink was used only by `.pdot` / `.statusnote .sdot`)
- `.statusnote`, `.statusnote .sdot`, `.statusnote b`
- `.bk`, `.bk.tl`, `.bk.tr`, `.bk.bl`, `.bk.br`
- `.flab`

DO NOT remove `.cmd` / `.cmd .p` / `.cmd .copy` (used by `InstallBlock.astro`),
nor `.cmds` / `.cmdrow` / `.cmd-ic` / `.cmd-name` / `.cmd-desc` (ControlPlane),
nor `.eyebrow` / `.label` (used).

## Out of scope — keep

- All stages 1–5: flat bg, no motion (subtle section reveal stays), de-chrome,
  reading column (`--readw`), `--maxw: 70rem`, the hairline frame.
- #3 monospace headline — separate brand discussion, NOT here.
- The `.glow` hero accent word — untouched.

## Acceptance criteria

1. Hero headline/sub/CTAs/screenshot are visible on first paint, no fade-in
   (no `rev` on hero elements).
2. Exactly one `banner` landmark (the topbar), one `<main>`, and the footer
   remains a `contentinfo`. Hero is a `<section>`.
3. Eyebrow + section labels have tighter tracking (`.14em`); marker/dash intact.
4. The six dead rule groups above are gone; `.cmd*`, `.eyebrow`, `.label`, and all
   used classes remain.
5. `cd site && bun run check` → 0 errors; `cd site && bun run build` → success.
6. Files changed: `Hero.astro`, `index.astro`, `landing.css` only. No copy edits.

## Verification (website-only — no cargo)

- `cd site && bun run check && bun run build` green.
- `rg -n 'class="rev"|sub rev|cta rev|shot rev' site/src/components/Hero.astro` → no hits.
- `rg -n '<header class="hero"|<main>' site/src` → hero header gone; `<main>` present.
- `rg -n '\.status \{|\.pdot|@keyframes blink|\.statusnote|\.bk|\.flab' site/src/styles/landing.css` → no hits.
- `rg -n 'class="cmd rev"' site/src/components/InstallBlock.astro` → still present (proof .cmd kept).
