# Migrate right-ui (CLI) + right-dashboard (Telegram) to Observatory/jewel brand — Sprint

Integration: claude/strange-borg-9c9f27  ·  Base: master
Engine: mimo
Issue: https://github.com/onsails/right-agent/issues/130 (follow-up to #129)
Legend: todo · brainstorming · planned · executing · review · blocked · done

## Stages
1. [todo] CLI       — right-ui (Rust): replace orange accents → jewel semantic hexes; keep NO_COLOR/TERM=dumb fallbacks.
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
- Engine: mimo, not pinned → resolve model per stage via mimo-resolve.

## Open questions
- (none yet)
