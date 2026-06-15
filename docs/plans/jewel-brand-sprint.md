# Migrate right-ui (CLI) + right-dashboard (Telegram) to Observatory/jewel brand — Sprint

Integration: claude/strange-borg-9c9f27  ·  Base: master
Engine: mimo
Issue: https://github.com/onsails/right-agent/issues/130 (follow-up to #129)
Legend: todo · brainstorming · planned · executing · review · blocked · done

## Stages
1. [done] CLI       — right-ui (Rust): orange→jewel; rail/mark→ruby, cursor→teal, glyphs→semantic, splash wordmark right=ruby/agent=muted. spec:01-cli-spec.md plan:01-cli-plan.md (merged @809d6e3c · review clean · 55/55 right-ui tests)
2. [todo] Dashboard — right-dashboard (Vue): introduce jewel palette tokens; accent→teal, identity→ruby, warm→gold; replace ad-hoc Telegram-blue/scattered hexes.

## Brand reference (authoritative: docs/brand-guidelines.html, v2 jewel)
- base plum `#121016`, panel `#201a26`, lines `#2d2533` / `#3e3146`
- ruby `#c75f88` = identity (word "right" + claw mark)
- teal `#3bb0c4` = action / structure / links
- gold `#cda14b` = warmth / secondary highlight
- text `#f1ece9`, muted `#b6a8b0`, dim `#6f6169`
- semantic: ok `#6bbf59`, warn `#e6c06a`, err `#e2556a`, info `#3bb0c4`

## Notes
- Recolor only — not a structural change. Dashboard keeps `right_ui::*`/`AsyncState`/`CollapsibleSection` primitives and existing component structure.
- right-ui current orange: `ORANGE = #E8632A` const in `crates/right-ui/src/atoms.rs`; `BRAND_ORANGE` in `prompts.rs`; status glyphs in atoms/line/recap/splash/theme.
- Dashboard is NOT literally orange today — it's Telegram-native blue (`#2481cc`/`#17212b`) + scattered hardcoded hexes with no jewel tokens. Migration introduces palette CSS custom properties.

## Decisions log
- 2-stage decomposition (CLI then dashboard); CLI first fixes canonical jewel hexes the dashboard reuses.
- Integration branch = current worktree branch `claude/strange-borg-9c9f27` (no new branch, per user rule).
- Engine: mimo, not pinned → resolve model per stage via mimo-resolve. Per user: always most-capable model + highest effort → openai/gpt-5.4 variant max (review max).
- Stage 01: mimo's first session stalled empty; resume (same handle) completed. A pre-commit hook reformats (rustfmt) — re-stage + retry the commit if it rewrites files.

## Open questions
- (none yet)
