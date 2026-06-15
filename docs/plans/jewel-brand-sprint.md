# Migrate right-ui (CLI) + right-dashboard (Telegram) to Observatory/jewel brand — Sprint

Integration: claude/strange-borg-9c9f27  ·  Base: master
Engine: mimo
Issue: https://github.com/onsails/right-agent/issues/130 (follow-up to #129)
Legend: todo · brainstorming · planned · executing · review · blocked · done

## Stages
1. [done] CLI       — right-ui (Rust): orange→jewel; rail/mark→ruby, cursor→teal, glyphs→semantic, splash wordmark right=ruby/agent=muted. spec:01-cli-spec.md plan:01-cli-plan.md (merged @809d6e3c · review clean · 55/55 right-ui tests)
2. [done] Dashboard — right-dashboard (Vue): FIXED jewel-dark; --jewel-* tokens + --tg-theme-* override (applyJewelTheme defeats TG inline injection), recolored semantics, ruby identity (AppShell agent name), ECharts jewel theme on all 3 chart consumers. spec:02-dashboard-spec.md plan:02-dashboard-plan.md (merged @964e15de · review clean · 209/209 tests · typecheck+build green · grep gates empty)

## Status: COMPLETE — both stages landed on claude/strange-borg-9c9f27.

### Final full-workspace verification
- `cargo nextest` (workspace minus 3 contended `right` binaries): **2866/2866 pass** (50 ignored live tests skipped), incl. new `right-ui splash_color_wordmark_is_ruby_and_muted`.
- `right` integration binaries run serially: **53/53 pass** (`cli_integration` 40, `home_isolation` 2, `wizard_brand` 11).
- `cargo test --doc --workspace`: green.
- **Pre-existing flakiness (not from this sprint):** a fully-parallel `cargo nextest run --workspace` shows ~12 `right`-crate init/destroy failures from concurrent `cloudflared` tunnel-name collisions + missing local OAuth creds. All pass serially/in isolation; the recolor touches only color values in `right-ui` + dashboard frontend, nothing near tunnels/init/credentials.

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
