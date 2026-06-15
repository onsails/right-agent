# Stage 01 — right-ui (CLI) jewel recolor — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Recolor `right-ui` from coal/fire orange (`#E8632A`) to the
Observatory/jewel palette — ruby identity, teal action, corrected semantic
glyphs, and a brand wordmark in the splash — without any structural change.

**Architecture:** All ANSI color originates in `crates/right-ui/src/atoms.rs`
(palette consts + `Rail` + `Glyph`) and `prompts.rs` (cursor). `splash.rs` gains
theme-aware wordmark coloring that reuses the atoms palette consts. Mono/Ascii
tiers stay byte-identical (no ANSI). Spec: `docs/plans/01-cli-spec.md`.

**Tech Stack:** Rust (edition 2024), `owo-colors` (`truecolor`), `inquire`
(`Color::Rgb`), `cargo nextest`.

**Jewel RGB reference (decimal, for `truecolor` escapes `\x1b[38;2;R;G;Bm`):**
- ruby `#c75f88` = (199, 95, 136)  →  escape `\x1b[38;2;199;95;136m`
- teal `#3bb0c4` = (59, 176, 196)  →  escape `\x1b[38;2;59;176;196m`
- muted `#b6a8b0` = (182, 168, 176)
- ok `#6bbf59` = (107, 191, 89)  (unchanged)
- warn `#e6c06a` = (230, 192, 106)
- err `#e2556a` = (226, 85, 106)
- info `#3bb0c4` = (59, 176, 196)  (== teal)

**Commit policy:** Do NOT `git commit` per task. The sprint stage-runner commits
and lands the whole stage after review + verify (mechanics §7). Leave the working
tree dirty; just keep each task green.

---

### Task 1: atoms.rs — jewel palette constants

**Files:**
- Modify: `crates/right-ui/src/atoms.rs`
- Test: `crates/right-ui/src/atoms_tests.rs`

**Step 1 — Write failing regression tests.** Append to `atoms_tests.rs`:

```rust
// --- jewel palette regression (truecolor escapes) ---

#[test]
fn rail_is_ruby() {
    // ruby #c75f88 = (199, 95, 136)
    let s = Rail::prefix(Theme::Color);
    assert!(s.contains("\x1b[38;2;199;95;136m"), "rail not ruby: {s:?}");
}

#[test]
fn mark_is_ruby() {
    let s = Rail::mark(Theme::Color);
    assert!(s.contains("\x1b[38;2;199;95;136m"), "mark not ruby: {s:?}");
}

#[test]
fn glyph_semantic_hexes() {
    assert!(Glyph::Ok.render(Theme::Color).contains("\x1b[38;2;107;191;89m"));
    assert!(Glyph::Warn.render(Theme::Color).contains("\x1b[38;2;230;192;106m"));
    assert!(Glyph::Err.render(Theme::Color).contains("\x1b[38;2;226;85;106m"));
    assert!(Glyph::Info.render(Theme::Color).contains("\x1b[38;2;59;176;196m"));
}
```

**Step 2 — Run, verify red.**
Run: `devenv shell -- cargo nextest run -p right-ui rail_is_ruby mark_is_ruby glyph_semantic_hexes`
Expected: FAIL (still orange `232;99;42`, warn `217;168;42`, etc.).

**Step 3 — Edit the consts and rename.** In `atoms.rs`:
- Replace `pub(crate) const ORANGE: (u8, u8, u8) = (0xE8, 0x63, 0x2A);`
  with `pub(crate) const RUBY: (u8, u8, u8) = (0xC7, 0x5F, 0x88);`
- Add `pub(crate) const MUTED: (u8, u8, u8) = (0xB6, 0xA8, 0xB0);`
  and `pub(crate) const TEAL: (u8, u8, u8) = (0x3B, 0xB0, 0xC4);`
  (TEAL/MUTED are used by prompts.rs / splash.rs in later tasks.)
- `const OK` stays `(0x6B, 0xBF, 0x59)`.
- `const WARN: (u8,u8,u8) = (0xE6, 0xC0, 0x6A);`
- `const ERR:  (u8,u8,u8) = (0xE2, 0x55, 0x6A);`
- `const INFO: (u8,u8,u8) = (0x3B, 0xB0, 0xC4);`
- Replace the three `ORANGE.0, ORANGE.1, ORANGE.2` uses in `Rail` with
  `RUBY.0, RUBY.1, RUBY.2`.
- Update the module doc comment (line ~4) "orange rail" → "ruby rail".

> Note: `TEAL`/`MUTED` may trip `dead_code` until Tasks 2–3 use them. If you do
> Task 1 in isolation and the crate fails to compile on the unused const, add the
> consts in the task that first consumes them instead, or complete Tasks 1–3 as
> one edit before the first full build. Either ordering is fine; keep them in
> `atoms.rs` as the single palette source.

**Step 4 — Run, verify green.**
Run: `devenv shell -- cargo nextest run -p right-ui`
Expected: PASS (existing atoms tests + new regression tests).

---

### Task 2: prompts.rs — highlighted cursor → teal

**Files:**
- Modify: `crates/right-ui/src/prompts.rs` (impl + its inline `#[cfg(test)] mod tests`)

**Step 1 — Update the test first.** In the `tests` module:
- `color_theme_only_colors_highlighted_cursor`: the assertion
  `assert_eq!(cfg.highlighted_option_prefix.style.fg, Some(BRAND_ORANGE));`
  becomes `Some(CURSOR_TEAL)` (new const name from Step 2).

**Step 2 — Edit impl.**
- Change `use crate::atoms::ORANGE;` → `use crate::atoms::TEAL;`
- Replace the `BRAND_ORANGE` const with:
  ```rust
  const CURSOR_TEAL: Color = Color::Rgb { r: TEAL.0, g: TEAL.1, b: TEAL.2 };
  ```
- Update both references (`with_fg(BRAND_ORANGE)` and the test) to `CURSOR_TEAL`.
- Update the module doc comment (lines ~6–9): "brand orange" → "brand teal";
  drop the now-stale parenthetical about orange if it no longer reads true.

**Step 3 — Run, verify green.**
Run: `devenv shell -- cargo nextest run -p right-ui prompts`
Expected: PASS.

---

### Task 3: splash.rs — brand wordmark (`right` ruby + `agent` muted)

**Files:**
- Modify: `crates/right-ui/src/splash.rs`
- Test: `crates/right-ui/src/splash_tests.rs`

**Step 1 — Update/replace tests.** The current Color-theme test asserts the
contiguous substring `"right agent v0.10.2"`, which is no longer contiguous once
ANSI is interleaved. Replace `splash_color_has_ansi_no_unicode_loss` with:

```rust
#[test]
fn splash_color_wordmark_is_ruby_and_muted() {
    let s = splash(Theme::Color, "0.10.2", "tagline");
    assert!(s.contains(ESC), "color splash should emit ANSI");
    // "right" in ruby, "agent" in muted; version stays plain.
    assert!(s.contains("\x1b[38;2;199;95;136m"), "wordmark 'right' not ruby: {s:?}");
    assert!(s.contains("\x1b[38;2;182;168;176m"), "wordmark 'agent' not muted: {s:?}");
    assert!(s.contains("v0.10.2"), "version text missing: {s:?}");
}
```
Leave `splash_mono_three_lines`, `splash_ascii`, `splash_mono_no_ansi`,
`splash_ascii_no_unicode_atoms` unchanged — Mono/Ascii output must stay
byte-identical (`"▐✓ right agent v0.10.2"` / `"|* right agent v0.10.2"`).

**Step 2 — Run, verify red.**
Run: `devenv shell -- cargo nextest run -p right-ui splash`
Expected: FAIL on the new ruby/muted assertions.

**Step 3 — Implement wordmark coloring.** In `splash.rs`:
- Add `use owo_colors::OwoColorize;` and `use crate::atoms::{MUTED, RUBY};`.
- Add a private helper:
  ```rust
  /// Brand wordmark: `right` (ruby) + `agent` (muted) in Color; plain otherwise.
  fn wordmark(theme: Theme) -> String {
      match theme {
          Theme::Color => format!(
              "{} {}",
              "right".truecolor(RUBY.0, RUBY.1, RUBY.2),
              "agent".truecolor(MUTED.0, MUTED.1, MUTED.2),
          ),
          Theme::Mono | Theme::Ascii => "right agent".to_string(),
      }
  }
  ```
- In `splash`, replace the line-1 construction
  `out.push_str("right agent v"); out.push_str(version);`
  with:
  ```rust
  out.push_str(&wordmark(theme));
  out.push_str(" v");
  out.push_str(version);
  ```
  (Mark `▐✓ ` and the trailing space before the wordmark are unchanged.)

**Step 4 — Run, verify green.**
Run: `devenv shell -- cargo nextest run -p right-ui splash`
Expected: PASS. Confirm Mono line is still exactly `"▐✓ right agent v0.10.2"`.

---

### Task 4: cleanup + grep gate + final stage verification

**Step 1 — Grep gate.** From repo root:
Run: `rg -n -i "0xE8, 0x63, 0x2A|#E8632A|ORANGE|orange" crates/right-ui/src`
Expected: NO matches (all renamed; doc comments updated). If any remain, fix the
wording/value; "orange" must not appear in `right-ui` source.

**Step 2 — Build clean.**
Run: `devenv shell -- cargo build -p right-ui`
Expected: no warnings about unused `TEAL`/`MUTED`/`RUBY` (all consumed).

**Step 3 — Full crate test.**
Run: `devenv shell -- cargo nextest run -p right-ui`
Expected: ALL green.

**Step 4 — Clippy (crate-scoped).**
Run: `devenv shell -- cargo clippy -p right-ui -- -D warnings`
Expected: clean.

---

## Stage completion gate (handled by stage-runner, not per-task)
- `cargo nextest run -p right-ui` green, `cargo clippy -p right-ui` clean,
  grep gate empty.
- Code review (`/code-review high --fix`) clean or all findings resolved.
- Then commit + merge `$BR` into integration via `--no-ff` + remove worktree.
- The mandatory full-workspace test (`cargo nextest run --workspace` +
  `cargo test --doc --workspace`) is run once at the END of the whole sprint
  (after Stage 02), not per stage.
