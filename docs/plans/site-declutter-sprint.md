# Site Declutter — Sprint

Integration: feat/clean-site (worktree `.worktrees/clean-site`)  ·  Base: master @6676cc13
Engine: mimo (model: openai/gpt-5.3-codex, variant: high, pinned)
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
2. [todo] Motion restraint — drop per-section scroll-zoom, logo spin ring, claw pulse, learning-dots, loopspark; keep at most a subtle reveal.
3. [todo] Surface de-chrome — strip neon glows, heavy drop-shadows, backdrop-blur; hairline borders + flat panels across telem/cards/loop/diagram/control-plane.
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

## Open questions

- How far to cut motion: remove the scroll-zoom entirely, or keep a gentler
  version? (Default: remove; the user previously tuned it but now wants calm.)
- Keep the mono uppercase eyebrow/label tags per section, or drop them as
  ornament? (Defer to stage 4 after the heavier noise is gone.)
