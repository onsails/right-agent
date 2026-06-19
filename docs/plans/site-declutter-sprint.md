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
4. [todo] Rhythm & polish — whitespace + type scale, simplify mono eyebrow/label tags, prune dead CSS, final build + visual check.

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

## Open questions

- How far to cut motion: remove the scroll-zoom entirely, or keep a gentler
  version? (Default: remove; the user previously tuned it but now wants calm.)
- Keep the mono uppercase eyebrow/label tags per section, or drop them as
  ornament? (Defer to stage 4 after the heavier noise is gone.)
