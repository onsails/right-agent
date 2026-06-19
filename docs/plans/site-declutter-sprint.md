# Site Declutter — Sprint

Integration: feat/clean-site (worktree `.worktrees/clean-site`)  ·  Base: master @6676cc13
Engine: mimo (model: venice/openai-gpt-55, variant: high, pinned)
Nesting: yes
Legend: todo · brainstorming · planned · executing · review · blocked · done

## Goal

The landing site (`site/`) is overloaded with small decorative details that
distract from the content. Make it **much cleaner**, content-first, in the
spirit of https://herdr.dev/ (flat background, near-zero motion, hairline
borders, generous whitespace, restraint over ornament). Keep the brand
identity — claw logo, jewel palette, lowercase copy — but strip the noise.
Site copy stays English. Iterate stage by stage; the user judges each landed
stage on the dev server before the next.

## Reference principles (from herdr.dev)

- Whitespace dominance; generous vertical rhythm between sections.
- Zero decorative effects — no fake telemetry, no paper texture, no neon glows.
- Motion only where it aids comprehension; no scroll-zoom, no idle animation.
- Hairline borders and flat panels instead of heavy shadows + backdrop blur.
- Content (copy, screenshot, diagram, compare table) leads; chrome recedes.

## Stages

1. [done] Background ornaments — removed both fake-telemetry layers, paper texture, HUD corner-frame + secure-by-default readout; flat calm var(--bg). spec:01-bg-ornaments-spec.md plan:01-bg-ornaments-plan.md (merged @0c40c819 · +1/−152 · 2 files · astro check 0 · bun build green 9 pages · grep gate clean · mimo bg-ornaments-3f9a). **Model: openai/gpt-5.3-codex REJECTED by OpenAI (codex unsupported on a ChatGPT-account) → mimo auto-substituted venice/claude-sonnet-4-6 high. Pin decision pending for stages 2–4.**
2. [done] Motion restraint — removed scroll-zoom (JS active-section + .section scale/opacity), logo spin, claw pulse, learning-dots (+eyebrow sig), loopspark, card cursor-spotlight; hero static + visible on load; kept ONE below-fold .rev reveal. spec:02-motion-spec.md plan:02-motion-plan.md (merged @d5ebebef · +10/−143 · 4 files · astro check 0 · bun build green · grep gate clean · review clean · mimo motion-8b3k venice/openai-gpt-55 high — NO substitution this time).
3. [done] Surface de-chrome — removed all neon glows (0 0 colored box-shadow + drop-shadow), heavy elevation + inset-gloss shadows, backdrop-blur; card hover → border-only; removed `.card::before` accent line; kept hairline borders + flat panel fills + brand colors. spec:03-dechrome-spec.md plan:03-dechrome-plan.md (merged @5e0ed452 · +29/−32 · 1 file landing.css · astro check 0 · bun build green · 3 grep gates empty · review xhigh clean · mimo dechrome-5d2c venice/openai-gpt-55 high).
4. [done] Reading column + narrow container + soften reveal — section `h2` + `.lead` share `--readw` (46rem ≈736px), left+right aligned (verified: both left 59, right 795); `--maxw` 76→70rem (1120px); reveal translateY 24→8px & .9s→.35s. spec:04-rhythm-spec.md plan:04-rhythm-plan.md (merged @71fd041d). **04b correction:** stage 04's `66ch` cap on `h2` was inert (ch is font-size-relative → 66ch@33px ≈1250px > container); fixed to rem-based `--readw`. plan:04b-readwfix-plan.md (merged @9af263ca · astro check 0 · build green · review clean). **ENGINE: `venice/openai-gpt-55` errored at provider before any tool call on BOTH 04 and 04b → mimo-delegate hand-applied (sonnet); diffs were review+build verified. venice/openai-gpt-55 now flaky — switch model if a substantive stage follows.**
5. [done] herdr-style hairline frame — `.topbar` wrapper + nav rule; hero vertical divider (copy|shot, gap:0 + align stretch + `.hero-copy` border-right) + `.hero` border-bottom; `.section` border-bottom (footer `border-top` dropped); screenshot integrated (border+radius removed). spec:05-frame-spec.md plan:05-frame-plan.md (merged @b7dfe320 · 2 files · astro check 0 · build green · review xhigh clean — fixed 2 layout bugs: mobile media-query ordering + `.shot` vertical align). venice errored → ran mimo default model (substitution).
6. [todo] (deferred polish) simplify mono eyebrow/`.label` tags; prune dead CSS (`.status`/`.pdot`/`.statusnote`/`.bk`/`.cmd` if unused); whitespace/vertical rhythm.
7. [todo] (#3 — brand, discuss separately) monospace headline like herdr (currently Chakra Petch display).

Stages are coarse and reorderable; insert/split as the iteration reveals what
still feels cluttered. Each stage is one visible, judgeable diff.

## Verification cadence (website-only)

No Rust tests — website-only work skips cargo. Per stage: `bun run build`
must pass (starlight-links-validator + pagefind run in build); then visual
check on the dev server. Final stage: full `bun run build` + a clean visual
pass over landing + docs.

## Decisions log

- Decomposition + reference principles adopted from the earlier `site/declutter`
  planning session (which produced only this doc, no code). Re-confirmed against
  the user's fresh request 2026-06-19 — same goal, same engine/model.
- Engine mimo, model pinned to openai/gpt-5.3-codex (user: "codex 5 latest"),
  variant high. Explicit pin → no per-stage model ASK.
- Integration branch `feat/clean-site` lives in its own worktree
  (`.worktrees/clean-site`), not on the shared main checkout — repo
  checkout-churn rule. Based off master @6676cc13.
- Dev server: `bun run dev --host 0.0.0.0` from the integration worktree
  (currently :4322; 4321 was in use). User reviews each landed stage there.

## Stage 1 findings (feed later stages)

- **Model pin broken.** `openai/gpt-5.3-codex` (and almost certainly every
  `openai/*-codex` id) is rejected through mimo's ChatGPT-account auth:
  "not supported when using Codex with a ChatGPT account." Stage 1 silently ran
  on `venice/claude-sonnet-4-6` (high) and completed fine. **Pending user
  decision for stages 2–4:** switch the pin to `venice/openai-gpt-53-codex`
  (Venice-hosted codex mirror, uses Venice auth, honours "codex latest") vs.
  accept `venice/claude-sonnet-4-6`.
- **Reveal-on-load nit → stage 2.** Hero `.rev` copy is `opacity:0` at the very
  top on fresh load until the IntersectionObserver fires. Above-the-fold content
  must not require a scroll to appear — make the hero visible immediately;
  reveal only below-the-fold sections (or drop reveal entirely).

## Stage 2 findings

- **Branch-delete quirk (worktree-integration).** `git -C <main> branch -d <BR>`
  checks reachability from `master` (where the stage merge does NOT live), so it
  refuses with a false negative. Next stages: route the delete through the
  integration worktree — `git -C <IWT> branch -d <BR>` — or `-D` after verifying
  the merge commit is an ancestor of `feat/clean-site`.
- **Reveal still reads as motion on fast scroll.** The retained `.rev` fade-up
  (opacity 0→1, translateY 24px, .9s) dims large regions while they catch up,
  then settles to full opacity. PENDING USER: keep / soften / remove.

## Width discussion (2026-06-19, with user)

Measured ours vs herdr: outer container nearly equal (ours 76rem/1216px, herdr
1160px) — sprawl is NOT outer width. Root cause: misaligned columns — our section
`h2` + panels run full ~1142px while `.lead` is 65ch left-aligned → ragged right
edge, content doesn't collect. herdr caps text tightly (h1 ≤760px, lede ≤520px),
spends width deliberately. User agreed: option 1 (unified reading column) + light
option 2 (narrow container) now; #3 (monospace headline) discussed separately.

## Stage 5 findings (feed the polish stage)

- **Hero `.rev` was never actually removed (stage-2 gap).** Stage 2 stripped only
  the `dN` delay suffixes; `class="rev"` still sits on hero `h1`/`.sub`/`.cta`/
  `.shot`, so the hero STILL fades in on load (the recurring "dim then settle"
  in screenshots). Stage-2 "hero visible on load" is NOT actually met. Fix: drop
  the bare `.rev` from those hero elements → truly static hero.
- **a11y: two banner landmarks + no `<main>`.** `.topbar` (`<header>`) and the
  hero (`<header class="hero">`) are both `banner`s; the page has no `<main>`.
  Fix needs `Hero.astro`/`Landing.astro` (rename hero to `<section>`, wrap the
  slot/content in `<main>`). Fold into the polish stage.

## Open questions

- How far to cut motion: remove the scroll-zoom entirely, or keep a gentler
  version? (Default: remove; the user previously tuned it but now wants calm.)
- Keep the mono uppercase eyebrow/label tags per section, or drop them as
  ornament? (Defer to stage 4 after the heavier noise is gone.)
