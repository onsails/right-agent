# Landing page polish (site/, Astro) — Sprint

Integration: claude/festive-feynman-235990  ·  Base: master
Engine: mimo
Legend: todo · brainstorming · planned · executing · review · blocked · done

## Stages
1. [done] Telemetry meaning — Hero `sig ●●●●○` → animated `learning` indicator (ties to self-evolution); removed `obs · deep-field` HUD readout + orphaned `.r-tr` CSS. spec:01-telemetry-spec.md plan:01-telemetry-plan.md (merged @5e397a3f · stage @03b024d7 · bun build green 7 pages · grep gates clean · mimo telemetry-4b7q openai/gpt-5.4 max)
2. [todo]    Scroll FX / zoom — scroll-driven zoom/scale on significant sections, on top of existing parallax + `.rev` reveal. Needs its own brainstorm (user wants to experiment).

## Context
- Target is the marketing site at `site/` (Astro), NOT right-ui or right-dashboard. The jewel-brand-sprint (CLI + dashboard) is a separate, completed milestone.
- Existing landing FX (`site/src/layouts/Landing.astro` `<script>`): pointer/scroll parallax on `.fx[data-depth]` layers; `.rev` reveal via IntersectionObserver; card spotlight via `--gx/--gy`. Stage 02 builds on these.
- Observatory/jewel brand vocabulary is authoritative in `docs/brand-guidelines.html` (§04 Observatory motifs). Brand rule: "atmosphere serves legibility, the instrument is precise, not busy."

## Decisions log
- Milestone run as a sprint per user; 2-stage decomposition, telemetry first (user: "сначала разберись с sig и obs", zoom needs experimentation).
- Integration branch = current worktree branch `claude/festive-feynman-235990` (no new branch, per user rule).
- Engine: mimo, not pinned → resolve model per stage via mimo-resolve. Standing user preference (memory `sprint-executor-model-preference`): always most-capable model + highest effort.
- Stage 01 content (brainstormed in main):
  - `sig` carried zero semantic load; repurpose to `learning` — an animated dot indicator evoking the agent's self-evolution / skill-learning. teal ("live rail" per brand). Respect `prefers-reduced-motion`.
  - `obs · deep-field` removed entirely (user). Its `.r-tr` HUD slot CSS becomes orphaned → remove. Keep `secure-by-default` (`.r-bl`).
  - Rejected: filling telemetry with fake live numbers (`lat 12ms · 3 agents online`) — still zero real meaning.

- Stage 01 stage-runner deviations (logged for stage 02):
  - The stage-runner instance ran WITHOUT the `Agent` tool, contrary to the skill contract. It executed mimo via a foreground Bash launcher (handle `telemetry-4b7q`) and performed the code-review **inline against the rubric** (3-file diff, clean) instead of in a nested subagent. The available `/code-review` is GitHub-PR-oriented (posts `gh` comments), not an in-worktree `--fix` reviewer. Acceptable for a trivial recolor; for the larger stage 02, confirm a real review subagent path first.
  - mimo's first run hit a transient OpenAI `server_error` (empty diff) → resumed the same handle once → completed.
  - `prek` pre-commit hook aborted on a missing `.pre-commit-config.yaml` (untracked on this branch, pre-existing env condition) → used `PREK_ALLOW_NO_CONFIG=1` escape hatch.

## Open questions
- (Stage 02) exact zoom character — user leaning to experiment; brainstorm when reached. Earlier options surfaced: soft scale-in vs scroll-scrubbed vs pinned/cinematic.
