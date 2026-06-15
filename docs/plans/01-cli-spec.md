# Stage 01 — right-ui (CLI) → Observatory/jewel — Spec

## Goal
Recolor the `right-ui` crate from the old "coal & fire" orange brand to the
Observatory/jewel palette, matching `docs/brand-guidelines.html` (v2). Recolor
only — no structural/API changes. `NO_COLOR` (Mono) and `TERM=dumb`/non-TTY
(Ascii) tiers must keep producing identical plain output (no ANSI).

## Authoritative palette (from brand guide)
- ruby `#c75f88` = identity (mark + the word `right`)
- teal `#3bb0c4` = action / structure / links / info
- muted `#b6a8b0` = secondary text (the word `agent`)
- semantic: ok `#6bbf59`, warn `#e6c06a`, err `#e2556a`, info `#3bb0c4`
- gold `#cda14b` = warmth/secondary — **not used in the CLI** (YAGNI; decided).

## Color surface (the only callsites — verified by grep)
All ANSI color lives in two files; `splash.rs` adds wordmark coloring.
1. `crates/right-ui/src/atoms.rs`
   - `ORANGE` const `#E8632A` → replace with `RUBY` `#c75f88`. Used by
     `Rail::{prefix,mark,blank}` for the rail `▐` and claw mark `▐✓`.
     Brand guide: "the mark is always rendered in ruby."
   - Semantic glyph consts corrected to brand hexes:
     - `OK` `#6BBF59` → unchanged (already brand `#6bbf59`).
     - `WARN` `#D9A82A` → `#E6C06A`.
     - `ERR` `#E03C3C` → `#E2556A`.
     - `INFO` `#4A90E2` (blue) → `#3BB0C4` (teal).
2. `crates/right-ui/src/prompts.rs`
   - `BRAND_ORANGE` (currently aliases `ORANGE`) → new `TEAL` `#3bb0c4`
     for the highlighted-option cursor `>` (action/focal). Rename the const to
     `CURSOR_TEAL` (or keep a clearly-named teal const); stop importing `ORANGE`.
3. `crates/right-ui/src/splash.rs`
   - Line 1 currently pushes `"right agent v<version>"` as **plain text**.
     Apply the brand wordmark in `Theme::Color` only:
     - `right` → ruby `#c75f88`
     - ` agent` → muted `#b6a8b0`
     - ` v<version>` → default (uncolored).
   - The leading `▐✓ ` (claw mark) already comes from `Rail::mark` (now ruby).
   - `Theme::Mono` and `Theme::Ascii`: wordmark stays plain (no ANSI) — output
     byte-identical to today.

## Naming / constants
- Introduce shared jewel constants once (in `atoms.rs`) and reuse: `RUBY`,
  `TEAL`, `MUTED` as `(u8,u8,u8)` tuples, matching the existing const style.
- `prompts.rs` builds its `comfy`/`inquire` `Color::Rgb` from the shared tuple
  rather than redefining the literal, to keep one source of truth.
- Update doc comments that say "orange" (atoms.rs header, prompts.rs header) to
  "ruby"/"teal" as appropriate.

## Out of scope
- No new themes, no `--no-color` flag work, no changes to detection logic in
  `theme.rs`, no changes to `line.rs`/`recap.rs`/`header.rs`/`writer.rs`
  (they delegate to atoms and need no edits).
- Dashboard (Stage 02).

## Verification criteria
- `cargo build -p right-ui` clean.
- `cargo nextest run -p right-ui` green after test updates.
- Tests to update (all in `crates/right-ui/src/*_tests.rs`):
  - `atoms_tests.rs`: any assertion embedding the orange truecolor escape
    (`\x1b[38;2;232;99;42m`) or referencing `ORANGE`; update to ruby
    `\x1b[38;2;199;95;136m`; update warn/err/info escape assertions to new hexes.
  - `prompts_tests.rs` (in `prompts.rs` `#[cfg(test)]`): `BRAND_ORANGE`
    assertion → new teal const.
  - `splash_tests.rs`: add/adjust a `Theme::Color` assertion proving the
    wordmark carries ruby+muted ANSI; assert Mono/Ascii lines are byte-identical
    to today (`"▐✓ right agent v0.10.2"` / `"|* right agent v0.10.2"`).
- Grep gate: no `0xE8, 0x63, 0x2A` / `#E8632A` / `ORANGE` / "orange" remains in
  `crates/right-ui/src` (except possibly a CHANGELOG-style comment — none expected).
- Brand-conformance rule (AGENTS.md): all user-facing output still flows through
  `right_ui::*`; this stage only changes hex values, not the routing.

## TDD note
For the wordmark (new behavior), write the failing `splash.rs` Color-theme test
first (assert ruby+muted ANSI present), confirm red, then implement. The const
swaps in atoms/prompts are mechanical — update tests and impl together.
