# Stage 06 — Polish & fixes — Plan

Executor implements in the stage worktree. Files: `site/src/components/Hero.astro`,
`site/src/pages/index.astro`, `site/src/styles/landing.css`. Spec: `06-polish-spec.md`.
No copy/content changes; no redesign.

## Tasks

1. **Static hero — `Hero.astro`.** Remove the `rev` class from these elements
   (keep all other classes/markup):
   - `<h1 class="rev">` → `<h1>`
   - `<p class="sub rev">` → `<p class="sub">`
   - `<div class="cta rev">` → `<div class="cta">`
   - `<div class="shot rev">` → `<div class="shot">`

2. **a11y — `Hero.astro`.** Change the wrapper `<header class="hero">` → 
   `<section class="hero">` (and its closing `</header>` → `</section>`).

3. **a11y — `index.astro`.** Wrap the primary content in `<main>`: insert `<main>`
   immediately before `<Hero />` and `</main>` immediately AFTER the last
   `<section class="section" id="install">…</section>` but BEFORE `<Footer />`.
   So the structure becomes:
   ```
   <Landing>
     <main>
       <Hero />
       <div class="wrap"> … telem … </div>
       <section class="section"> … </section>
       … (all sections, through #install) …
     </main>
     <Footer />
   </Landing>
   ```
   Do not move/alter the sections themselves — only add the `<main>` open/close.

4. **Simplify tags — `landing.css`.** On the `.eyebrow` rule and the `.label`
   rule, change `letter-spacing: .24em` → `letter-spacing: .14em`. Change nothing
   else on those rules (keep font, text-transform, color, `▸`, `.label::before`).

5. **Prune dead CSS — `landing.css`.** Delete these rule blocks entirely
   (verified 0 usages in `*.astro`):
   - `.status { … }`
   - `.pdot { … }` and `@keyframes blink { … }`
   - `.statusnote { … }`, `.statusnote .sdot { … }`, `.statusnote b { … }`
   - `.bk { … }`, `.bk.tl { … }`, `.bk.tr { … }`, `.bk.bl { … }`, `.bk.br { … }`
   - `.flab { … }`
   KEEP `.cmd`, `.cmd .p`, `.cmd .copy`, `.cmd:hover` (used by InstallBlock) and
   all `.cmds`/`.cmdrow`/`.cmd-ic`/`.cmd-name`/`.cmd-desc` (ControlPlane), and
   `.eyebrow`/`.label`. If `@keyframes blink` has any OTHER referencing selector
   still present after step-5 deletions, leave blink in place (it should not).

## Verification (run in the stage worktree)

- `cd site && bun install && bun run check && bun run build` → all green.
- `rg -n 'class="rev"|sub rev|cta rev|shot rev' site/src/components/Hero.astro` → no hits.
- `rg -n '<header class="hero"' site/src/components/Hero.astro` → no hits; `rg -n '<main>' site/src/pages/index.astro` → present.
- `rg -n '\.status \{|\.pdot|@keyframes blink|\.statusnote|\.bk[ .{]|\.flab' site/src/styles/landing.css` → no hits.
- `rg -n 'class="cmd rev"' site/src/components/InstallBlock.astro` → present.

Leave changes uncommitted; the stage-runner commits and lands.
