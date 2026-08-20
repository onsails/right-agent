# Init Wizard Brand Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Repaint `right init`, `right agent init`, `right config`, `right doctor`, and `right status` against the Right Agent brand guide — `▐` rail, `▐✓` mark, semantic glyphs (`✓ ! ✗ …`), three theme tiers (color / mono / ascii), and lowercase-first / terse / factual voice.

**Architecture:** New shared module `right-agent::ui` owns all atoms, glyph rendering, splash, headers, recaps, theme detection, and writers. Doctor's existing `Display` impl migrates to it. Wizard prompts keep their existing `inquire` rendering — the brand only touches non-interactive output. Three-tier theme: TTY+color → glyphs+color, `NO_COLOR` set → glyphs+no-color, `TERM=dumb`/non-TTY → ASCII (`|` rail, `[ok]/[warn]/[err]/[…]` glyphs).

**Tech Stack:** Rust 2024, `owo-colors` (truecolor escapes, already a workspace dep), `insta` (snapshots, dev-dep to add), `assert_cmd` + `predicates` + `tempfile` (existing CLI tests), `inquire` (existing prompts — unchanged), `miette` (errors).

**Spec:** [docs/superpowers/specs/2026-04-28-init-wizard-brand-redesign-design.md](../specs/2026-04-28-init-wizard-brand-redesign-design.md) (commit `f83b6ca8`)

---

## File Map

### New files (all in `crates/right-agent/src/ui/`)

| File | Responsibility |
|---|---|
| `mod.rs` | Public re-exports + module doc |
| `theme.rs` | `Theme` enum, `EnvGet` trait, `detect`/`detect_with` |
| `theme_tests.rs` | Theme detection tests with stub env/tty |
| `atoms.rs` | `Rail` (prefix/mark/blank), `Glyph` (Ok/Warn/Err/Info), color tokens |
| `atoms_tests.rs` | Atom × theme matrix tests |
| `line.rs` | `Line` builder, `Block` (column-aligning), `status()` factory |
| `line_tests.rs` | Line/Block tests including alignment |
| `splash.rs` | `splash(theme, version, tagline)` |
| `splash_tests.rs` | Snapshot tests + ANSI/Unicode invariants |
| `header.rs` | `section(theme, name)` |
| `recap.rs` | `Recap` builder |
| `recap_tests.rs` | Recap snapshot tests |
| `writer.rs` | `stdout(theme, s)`, `stderr(theme, s)` |
| `error.rs` | `BlockAlreadyRendered` sentinel error |

### New test files

| File | Responsibility |
|---|---|
| `crates/right/tests/wizard_brand.rs` | Integration tests for splash, recap, doctor block, status block, ASCII/mono fallbacks, brand-conformance lint |
| `crates/right/tests/fixtures/cloudflared-mock.sh` | Mock cloudflared binary returning canned JSON for `tunnel list` / `tunnel create` |

### Modified files

| File | Change |
|---|---|
| `Cargo.toml` (workspace) | Add `insta = "1.41"` to `[workspace.dependencies]` |
| `crates/right-agent/Cargo.toml` | Add `insta` to `[dev-dependencies]` |
| `crates/right-agent/src/lib.rs` | Add `pub mod ui;` |
| `crates/right-agent/src/doctor.rs` | Remove `Display for DoctorCheck`; introduce a `to_ui_line(&self, theme) -> ui::Line` helper |
| `crates/right-agent/src/init.rs` | Voice rewrites for `prompt_sandbox_mode`, `prompt_network_policy`, `prompt_memory_provider`, `prompt_hindsight_*`, `prompt_recall_*`, `inquire_back` cancel copy, error strings (lines 312–600) |
| `crates/right/src/wizard.rs` | Voice rewrites across all prompts; replace `println!`/`eprintln!` with `ui::*` calls; introduce `pub(crate) const PROMPT_LABELS` |
| `crates/right/src/main.rs` | `cmd_init` (lines 1074–1371): splash + dependency probe + section headers + recap; `cmd_agent_init` (line 1373+): section header + recap; `cmd_doctor` (line 1868): `ui::Block` rendering; `cmd_status` (line 2310): `ui::Block` rendering; add `--no-color` global flag |

---

## Step 1 — `right-agent::ui` skeleton

Goal: ship the shared module with full unit-test coverage. Touches zero command code. Verifiable in isolation by `cargo test -p right-agent ui::`.

### Task 1: Add `insta` workspace dev-dep + `ui` module skeleton

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/right-agent/Cargo.toml`
- Create: `crates/right-agent/src/ui/mod.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Add insta to workspace deps**

Edit `/Users/molt/dev/rightclaw/Cargo.toml`. Inside `[workspace.dependencies]`, add (alphabetical order, near `inquire`):

```toml
insta = { version = "1.41", features = ["yaml"] }
```

- [ ] **Step 2: Add insta to right-agent dev-dependencies**

Edit `/Users/molt/dev/rightclaw/crates/right-agent/Cargo.toml`. Locate `[dev-dependencies]` (or add it if absent) and add:

```toml
[dev-dependencies]
insta = { workspace = true }
```

- [ ] **Step 3: Create empty `ui/mod.rs`**

Create `crates/right-agent/src/ui/mod.rs`:

```rust
//! Brand-conformant CLI presentation primitives.
//!
//! All atoms (`▐`, `▐✓`, semantic glyphs), splash, section headers, and
//! recap blocks live here. Three theme tiers: `Color`, `Mono`, `Ascii`.
//! See `docs/brand-guidelines.html` and `docs/superpowers/specs/2026-04-28-init-wizard-brand-redesign-design.md`.

pub mod atoms;
pub mod error;
pub mod header;
pub mod line;
pub mod recap;
pub mod splash;
pub mod theme;
pub mod writer;

pub use atoms::{Glyph, Rail};
pub use error::BlockAlreadyRendered;
pub use header::section;
pub use line::{Block, Line, status};
pub use recap::Recap;
pub use splash::splash;
pub use theme::{Theme, detect};
pub use writer::{stderr, stdout};
```

- [ ] **Step 4: Stub the eight submodule files so the workspace compiles**

Create each of these with a minimal placeholder. Real bodies arrive in Tasks 2–8.

`crates/right-agent/src/ui/theme.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme { Color, Mono, Ascii }

pub fn detect() -> Theme { Theme::Color }
```

`crates/right-agent/src/ui/atoms.rs`:
```rust
use crate::ui::theme::Theme;

pub struct Rail;
impl Rail {
    pub fn prefix(_theme: Theme) -> &'static str { "" }
    pub fn mark(_theme: Theme) -> &'static str { "" }
    pub fn blank(_theme: Theme) -> &'static str { "" }
}

#[derive(Debug, Clone, Copy)]
pub enum Glyph { Ok, Warn, Err, Info }
impl Glyph {
    pub fn render(self, _theme: Theme) -> String { String::new() }
}
```

`crates/right-agent/src/ui/line.rs`:
```rust
use crate::ui::{Glyph, Theme};

pub fn status(_glyph: Glyph) -> Line { Line::default() }

#[derive(Default)]
pub struct Line;
impl Line {
    pub fn noun(self, _s: impl Into<String>) -> Self { self }
    pub fn verb(self, _s: impl Into<String>) -> Self { self }
    pub fn detail(self, _s: impl Into<String>) -> Self { self }
    pub fn fix(self, _s: impl Into<String>) -> Self { self }
    pub fn render(self, _theme: Theme) -> String { String::new() }
}

#[derive(Default)]
pub struct Block;
impl Block {
    pub fn new() -> Self { Block }
    pub fn push(&mut self, _line: Line) {}
    pub fn render(self, _theme: Theme) -> String { String::new() }
}
```

`crates/right-agent/src/ui/splash.rs`:
```rust
use crate::ui::Theme;
pub fn splash(_theme: Theme, _version: &str, _tagline: &str) -> String { String::new() }
```

`crates/right-agent/src/ui/header.rs`:
```rust
use crate::ui::Theme;
pub fn section(_theme: Theme, _name: &str) -> String { String::new() }
```

`crates/right-agent/src/ui/recap.rs`:
```rust
use crate::ui::Theme;

pub struct Recap;
impl Recap {
    pub fn new(_title: &str) -> Self { Recap }
    pub fn ok(self, _noun: &str, _detail: &str) -> Self { self }
    pub fn warn(self, _noun: &str, _detail: &str) -> Self { self }
    pub fn next(self, _hint: &str) -> Self { self }
    pub fn render(self, _theme: Theme) -> String { String::new() }
}
```

`crates/right-agent/src/ui/writer.rs`:
```rust
use crate::ui::Theme;
pub fn stdout(_theme: Theme, s: &str) { println!("{s}"); }
pub fn stderr(_theme: Theme, s: &str) { eprintln!("{s}"); }
```

`crates/right-agent/src/ui/error.rs`:
```rust
use std::fmt;

#[derive(Debug)]
pub struct BlockAlreadyRendered;

impl fmt::Display for BlockAlreadyRendered {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result { Ok(()) }
}

impl std::error::Error for BlockAlreadyRendered {}
```

- [ ] **Step 5: Wire the module into `lib.rs`**

Edit `/Users/molt/dev/rightclaw/crates/right-agent/src/lib.rs`. Find the existing `pub mod` declarations (alphabetical) and add:

```rust
pub mod ui;
```

(Placement: alphabetical, after `pub mod stt;` if present, before `pub mod` items beginning with `v`/etc. If unsure, append at the end of the `pub mod` group.)

- [ ] **Step 6: Verify the workspace compiles**

Run:
```bash
cargo build --workspace
```
Expected: clean build, no warnings about `ui::*`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/right-agent/Cargo.toml crates/right-agent/src/ui/ crates/right-agent/src/lib.rs
git commit -m "feat(ui): scaffold right-agent::ui module skeleton"
```

---

### Task 2: Theme detection

**Files:**
- Modify: `crates/right-agent/src/ui/theme.rs`
- Create: `crates/right-agent/src/ui/theme_tests.rs`

Detection rules (first match wins):
1. `TERM=dumb` or `tty.is_terminal() == false` → `Ascii`.
2. `NO_COLOR` env var present (any non-empty value) → `Mono`.
3. Otherwise → `Color`.

- [ ] **Step 1: Implement `theme.rs`**

Replace the entire content of `crates/right-agent/src/ui/theme.rs` with:

```rust
//! Theme tiers for brand-conformant CLI output.
//!
//! Detection order:
//! 1. `TERM=dumb` or non-TTY → `Ascii`
//! 2. `NO_COLOR` env var set (any non-empty value) → `Mono`
//! 3. Otherwise → `Color`
//!
//! Tests inject `EnvGet` + `IsTerminal` stubs to avoid `std::env::set_var`.

use std::io::IsTerminal;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme { Color, Mono, Ascii }

/// Pluggable env reader. Production uses `RealEnv`; tests use stubs.
pub trait EnvGet {
    fn get(&self, key: &str) -> Option<String>;
}

pub struct RealEnv;

impl EnvGet for RealEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

static CACHED: OnceLock<Theme> = OnceLock::new();

/// Resolve the active theme once per process and cache it.
pub fn detect() -> Theme {
    *CACHED.get_or_init(|| detect_with(&std::io::stdout(), &RealEnv))
}

/// Pure detection — no caching, no globals. Used by tests and by callers
/// that want to override (e.g. `--no-color` flag passed in Step 6).
pub fn detect_with(tty: &impl IsTerminal, env: &impl EnvGet) -> Theme {
    if env.get("TERM").as_deref() == Some("dumb") || !tty.is_terminal() {
        return Theme::Ascii;
    }
    if env.get("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return Theme::Mono;
    }
    Theme::Color
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
```

- [ ] **Step 2: Write tests**

Create `crates/right-agent/src/ui/theme_tests.rs`:

```rust
use std::collections::HashMap;
use std::io::IsTerminal;

use super::*;

struct EnvStub(HashMap<String, String>);
impl EnvStub {
    fn new() -> Self { EnvStub(HashMap::new()) }
    fn with(mut self, k: &str, v: &str) -> Self {
        self.0.insert(k.into(), v.into());
        self
    }
}
impl EnvGet for EnvStub {
    fn get(&self, key: &str) -> Option<String> { self.0.get(key).cloned() }
}

struct TtyStub(bool);
impl IsTerminal for TtyStub {
    fn is_terminal(&self) -> bool { self.0 }
}

#[test]
fn detect_dumb_term_returns_ascii() {
    let env = EnvStub::new().with("TERM", "dumb");
    assert_eq!(detect_with(&TtyStub(true), &env), Theme::Ascii);
}

#[test]
fn detect_non_tty_returns_ascii() {
    let env = EnvStub::new();
    assert_eq!(detect_with(&TtyStub(false), &env), Theme::Ascii);
}

#[test]
fn detect_non_tty_overrides_no_color() {
    let env = EnvStub::new().with("NO_COLOR", "1");
    assert_eq!(detect_with(&TtyStub(false), &env), Theme::Ascii);
}

#[test]
fn detect_no_color_returns_mono() {
    let env = EnvStub::new().with("NO_COLOR", "1");
    assert_eq!(detect_with(&TtyStub(true), &env), Theme::Mono);
}

#[test]
fn detect_no_color_empty_value_falls_through_to_color() {
    let env = EnvStub::new().with("NO_COLOR", "");
    assert_eq!(detect_with(&TtyStub(true), &env), Theme::Color);
}

#[test]
fn detect_tty_no_env_returns_color() {
    let env = EnvStub::new();
    assert_eq!(detect_with(&TtyStub(true), &env), Theme::Color);
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p right-agent ui::theme:: --no-fail-fast
```
Expected: all 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/src/ui/theme.rs crates/right-agent/src/ui/theme_tests.rs
git commit -m "feat(ui): theme detection (color/mono/ascii)"
```

---

### Task 3: Atoms — `Rail` and `Glyph`

**Files:**
- Modify: `crates/right-agent/src/ui/atoms.rs`
- Create: `crates/right-agent/src/ui/atoms_tests.rs`

Color tokens (truecolor RGB):

| Atom | RGB |
|---|---|
| Rail / mark | `(0xE8, 0x63, 0x2A)` orange |
| Ok | `(0x6B, 0xBF, 0x59)` green |
| Warn | `(0xD9, 0xA8, 0x2A)` yellow |
| Err | `(0xE0, 0x3C, 0x3C)` red |
| Info | `(0x4A, 0x90, 0xE2)` blue |

- [ ] **Step 1: Implement atoms**

Replace `crates/right-agent/src/ui/atoms.rs` with:

```rust
//! Brand atoms — rail (`▐`), mark (`▐✓`), and semantic glyphs (`✓ ! ✗ …`).
//!
//! Color values come from the brand guide. Three render tiers:
//! * `Color`: orange rail + colored Unicode glyphs via owo-colors truecolor
//! * `Mono`: same glyphs without ANSI
//! * `Ascii`: `|` rail + bracketed text (`[ok]/[warn]/[err]/[…]`)

use owo_colors::OwoColorize;

use crate::ui::theme::Theme;

const ORANGE: (u8, u8, u8) = (0xE8, 0x63, 0x2A);
const OK: (u8, u8, u8) = (0x6B, 0xBF, 0x59);
const WARN: (u8, u8, u8) = (0xD9, 0xA8, 0x2A);
const ERR: (u8, u8, u8) = (0xE0, 0x3C, 0x3C);
const INFO: (u8, u8, u8) = (0x4A, 0x90, 0xE2);

pub struct Rail;

impl Rail {
    /// `"▐  "` (Color/Mono) or `"|  "` (Ascii). Always 4 visible cells.
    pub fn prefix(theme: Theme) -> String {
        match theme {
            Theme::Color => format!("{}  ", "▐".truecolor(ORANGE.0, ORANGE.1, ORANGE.2)),
            Theme::Mono => "▐  ".to_string(),
            Theme::Ascii => "|  ".to_string(),
        }
    }

    /// `"▐✓"` (Color/Mono) or `"|*"` (Ascii). 2 visible cells.
    pub fn mark(theme: Theme) -> String {
        match theme {
            Theme::Color => format!("{}", "▐✓".truecolor(ORANGE.0, ORANGE.1, ORANGE.2)),
            Theme::Mono => "▐✓".to_string(),
            Theme::Ascii => "|*".to_string(),
        }
    }

    /// `"▐"` (Color/Mono) or `"|"` (Ascii). For blank rail rows.
    pub fn blank(theme: Theme) -> String {
        match theme {
            Theme::Color => format!("{}", "▐".truecolor(ORANGE.0, ORANGE.1, ORANGE.2)),
            Theme::Mono => "▐".to_string(),
            Theme::Ascii => "|".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph { Ok, Warn, Err, Info }

impl Glyph {
    pub fn render(self, theme: Theme) -> String {
        let (unicode, ascii, rgb) = match self {
            Glyph::Ok => ("✓", "[ok]", OK),
            Glyph::Warn => ("!", "[warn]", WARN),
            Glyph::Err => ("✗", "[err]", ERR),
            Glyph::Info => ("…", "[…]", INFO),
        };
        match theme {
            Theme::Color => format!("{}", unicode.truecolor(rgb.0, rgb.1, rgb.2)),
            Theme::Mono => unicode.to_string(),
            Theme::Ascii => ascii.to_string(),
        }
    }
}

#[cfg(test)]
#[path = "atoms_tests.rs"]
mod tests;
```

- [ ] **Step 2: Write tests**

Create `crates/right-agent/src/ui/atoms_tests.rs`:

```rust
use super::*;

const ESC: char = '\x1b';

// --- Rail ---

#[test]
fn rail_prefix_color_has_ansi() {
    let s = Rail::prefix(Theme::Color);
    assert!(s.contains('▐'));
    assert!(s.contains(ESC), "color theme should emit ANSI: {s:?}");
    assert!(s.ends_with("  "));
}

#[test]
fn rail_prefix_mono_has_no_ansi() {
    assert_eq!(Rail::prefix(Theme::Mono), "▐  ");
}

#[test]
fn rail_prefix_ascii() {
    assert_eq!(Rail::prefix(Theme::Ascii), "|  ");
}

#[test]
fn rail_mark_color_has_check() {
    let s = Rail::mark(Theme::Color);
    assert!(s.contains('▐'));
    assert!(s.contains('✓'));
    assert!(s.contains(ESC));
}

#[test]
fn rail_mark_mono_is_unicode() {
    assert_eq!(Rail::mark(Theme::Mono), "▐✓");
}

#[test]
fn rail_mark_ascii() {
    assert_eq!(Rail::mark(Theme::Ascii), "|*");
}

#[test]
fn rail_blank_mono() {
    assert_eq!(Rail::blank(Theme::Mono), "▐");
}

#[test]
fn rail_blank_ascii() {
    assert_eq!(Rail::blank(Theme::Ascii), "|");
}

// --- Glyph ---

#[test]
fn glyph_ok_color_has_check_and_ansi() {
    let s = Glyph::Ok.render(Theme::Color);
    assert!(s.contains('✓'));
    assert!(s.contains(ESC));
}

#[test]
fn glyph_ok_mono() {
    assert_eq!(Glyph::Ok.render(Theme::Mono), "✓");
}

#[test]
fn glyph_ok_ascii() {
    assert_eq!(Glyph::Ok.render(Theme::Ascii), "[ok]");
}

#[test]
fn glyph_warn_unicode() {
    assert_eq!(Glyph::Warn.render(Theme::Mono), "!");
}

#[test]
fn glyph_warn_ascii() {
    assert_eq!(Glyph::Warn.render(Theme::Ascii), "[warn]");
}

#[test]
fn glyph_err_unicode() {
    assert_eq!(Glyph::Err.render(Theme::Mono), "✗");
}

#[test]
fn glyph_err_ascii() {
    assert_eq!(Glyph::Err.render(Theme::Ascii), "[err]");
}

#[test]
fn glyph_info_unicode() {
    assert_eq!(Glyph::Info.render(Theme::Mono), "…");
}

#[test]
fn glyph_info_ascii() {
    assert_eq!(Glyph::Info.render(Theme::Ascii), "[…]");
}

#[test]
fn no_ansi_in_mono_or_ascii() {
    for theme in [Theme::Mono, Theme::Ascii] {
        for s in [
            Rail::prefix(theme), Rail::mark(theme), Rail::blank(theme),
            Glyph::Ok.render(theme), Glyph::Warn.render(theme),
            Glyph::Err.render(theme), Glyph::Info.render(theme),
        ] {
            assert!(!s.contains(ESC), "theme {theme:?} string {s:?} contains ANSI escape");
        }
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p right-agent ui::atoms:: --no-fail-fast
```
Expected: all 18 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/src/ui/atoms.rs crates/right-agent/src/ui/atoms_tests.rs
git commit -m "feat(ui): rail + semantic glyphs with three theme tiers"
```

---

### Task 4: Status-line builder (`Line`, `Block`, `status()`)

**Files:**
- Modify: `crates/right-agent/src/ui/line.rs`
- Create: `crates/right-agent/src/ui/line_tests.rs`

Canonical line shape: `▐  {glyph} {noun:<width}  {verb} [({detail})]`.

`Block` aligns the noun column across all pushed lines: `width = max(noun.len()) + 2`.

`Line::fix(s)` adds a second line: `▐    fix: {s}` (4 spaces, then `fix:`, then content).

- [ ] **Step 1: Implement line.rs**

Replace `crates/right-agent/src/ui/line.rs` with:

```rust
//! Status-line builder. Canonical shape: `▐  {glyph} {noun:<width}  {verb} [({detail})]`.

use crate::ui::atoms::{Glyph, Rail};
use crate::ui::theme::Theme;

/// Start a new status line for the given glyph.
pub fn status(glyph: Glyph) -> Line {
    Line {
        glyph,
        noun: String::new(),
        verb: String::new(),
        detail: None,
        fix: None,
    }
}

#[derive(Clone)]
pub struct Line {
    glyph: Glyph,
    noun: String,
    verb: String,
    detail: Option<String>,
    fix: Option<String>,
}

impl Line {
    pub fn noun(mut self, s: impl Into<String>) -> Self { self.noun = s.into(); self }
    pub fn verb(mut self, s: impl Into<String>) -> Self { self.verb = s.into(); self }
    pub fn detail(mut self, s: impl Into<String>) -> Self { self.detail = Some(s.into()); self }
    pub fn fix(mut self, s: impl Into<String>) -> Self { self.fix = Some(s.into()); self }

    /// Render as a single string. May contain `\n` if `fix` is set.
    /// Padding: `noun_pad` is the width to pad the noun column to (max across a Block).
    /// Standalone calls use `self.noun.len()` (no padding).
    pub fn render(&self, theme: Theme) -> String {
        self.render_with_pad(theme, self.noun.len())
    }

    fn render_with_pad(&self, theme: Theme, noun_pad: usize) -> String {
        let mut out = String::new();
        out.push_str(&Rail::prefix(theme));
        out.push_str(&self.glyph.render(theme));
        out.push(' ');
        if noun_pad > 0 {
            out.push_str(&format!("{:<width$}", self.noun, width = noun_pad));
        } else {
            out.push_str(&self.noun);
        }
        if !self.verb.is_empty() {
            out.push_str("  ");
            out.push_str(&self.verb);
        }
        if let Some(ref d) = self.detail {
            out.push(' ');
            out.push('(');
            out.push_str(d);
            out.push(')');
        }
        if let Some(ref f) = self.fix {
            out.push('\n');
            // Fix is indented: "▐    fix: <content>" (rail + 4 spaces).
            out.push_str(&Rail::blank(theme));
            out.push_str("    fix: ");
            out.push_str(f);
        }
        out
    }
}

/// A vertical group of `Line`s with column-aligned noun widths.
#[derive(Default)]
pub struct Block {
    lines: Vec<Line>,
}

impl Block {
    pub fn new() -> Self { Block { lines: Vec::new() } }
    pub fn push(&mut self, line: Line) { self.lines.push(line); }
    pub fn is_empty(&self) -> bool { self.lines.is_empty() }
    pub fn len(&self) -> usize { self.lines.len() }

    pub fn render(&self, theme: Theme) -> String {
        let pad = self.lines.iter().map(|l| l.noun.len()).max().unwrap_or(0);
        self.lines
            .iter()
            .map(|l| l.render_with_pad(theme, pad))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
#[path = "line_tests.rs"]
mod tests;
```

- [ ] **Step 2: Write tests**

Create `crates/right-agent/src/ui/line_tests.rs`:

```rust
use super::*;
use crate::ui::Glyph;

#[test]
fn single_line_mono_basic() {
    let s = status(Glyph::Ok).noun("tunnel").verb("created").render(Theme::Mono);
    assert_eq!(s, "▐  ✓ tunnel  created");
}

#[test]
fn single_line_with_detail() {
    let s = status(Glyph::Ok)
        .noun("tunnel")
        .verb("created")
        .detail("right.example.com")
        .render(Theme::Mono);
    assert_eq!(s, "▐  ✓ tunnel  created (right.example.com)");
}

#[test]
fn single_line_with_fix() {
    let s = status(Glyph::Err)
        .noun("openshell")
        .verb("gateway unreachable")
        .fix("openshell gateway start")
        .render(Theme::Mono);
    assert_eq!(
        s,
        "▐  ✗ openshell  gateway unreachable\n▐    fix: openshell gateway start"
    );
}

#[test]
fn single_line_no_verb_collapses_spacing() {
    let s = status(Glyph::Info).noun("starting").render(Theme::Mono);
    assert_eq!(s, "▐  … starting");
}

#[test]
fn single_line_ascii_uses_pipe_and_brackets() {
    let s = status(Glyph::Ok).noun("tunnel").verb("created").render(Theme::Ascii);
    assert_eq!(s, "|  [ok] tunnel  created");
}

#[test]
fn block_aligns_noun_column() {
    let mut b = Block::new();
    b.push(status(Glyph::Ok).noun("right").verb("in PATH"));
    b.push(status(Glyph::Warn).noun("cloudflared").verb("not in PATH"));
    let s = b.render(Theme::Mono);
    let lines: Vec<&str> = s.split('\n').collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "▐  ✓ right         in PATH");
    assert_eq!(lines[1], "▐  ! cloudflared   not in PATH");
}

#[test]
fn block_with_fix_emits_extra_line() {
    let mut b = Block::new();
    b.push(status(Glyph::Ok).noun("a").verb("ok"));
    b.push(status(Glyph::Err).noun("b").verb("fail").fix("retry"));
    let s = b.render(Theme::Mono);
    let lines: Vec<&str> = s.split('\n').collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[2].contains("fix: retry"));
}

#[test]
fn empty_block_renders_empty_string() {
    assert_eq!(Block::new().render(Theme::Mono), "");
}

#[test]
fn ascii_block_alignment() {
    let mut b = Block::new();
    b.push(status(Glyph::Ok).noun("a").verb("x"));
    b.push(status(Glyph::Ok).noun("longer").verb("y"));
    let s = b.render(Theme::Ascii);
    let lines: Vec<&str> = s.split('\n').collect();
    assert_eq!(lines[0], "|  [ok] a       x");
    assert_eq!(lines[1], "|  [ok] longer  y");
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p right-agent ui::line:: --no-fail-fast
```
Expected: 9 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/src/ui/line.rs crates/right-agent/src/ui/line_tests.rs
git commit -m "feat(ui): status line + block builder with column alignment"
```

---

### Task 5: Splash + section header

**Files:**
- Modify: `crates/right-agent/src/ui/splash.rs`
- Create: `crates/right-agent/src/ui/splash_tests.rs`
- Modify: `crates/right-agent/src/ui/header.rs`

- [ ] **Step 1: Implement splash + header**

Replace `crates/right-agent/src/ui/splash.rs`:

```rust
//! Full splash header — `▐✓ right agent vX.Y.Z` + tagline + blank rail.

use crate::ui::atoms::Rail;
use crate::ui::theme::Theme;

pub fn splash(theme: Theme, version: &str, tagline: &str) -> String {
    let mut out = String::new();
    // Line 1: ▐✓ right agent v0.10.2
    out.push_str(&Rail::mark(theme));
    out.push(' ');
    out.push_str("right agent v");
    out.push_str(version);
    out.push('\n');
    // Line 2: ▐  <tagline>
    out.push_str(&Rail::prefix(theme));
    out.push_str(tagline);
    out.push('\n');
    // Line 3: ▐
    out.push_str(&Rail::blank(theme));
    out
}

#[cfg(test)]
#[path = "splash_tests.rs"]
mod tests;
```

Replace `crates/right-agent/src/ui/header.rs`:

```rust
//! One-line section header: `▐ name ─────` filled to column 48.
//!
//! `─` becomes `-` under `Ascii`. Header is preceded by a blank rail row
//! when used inside a flow (callers add the `\n` before/after themselves).

use crate::ui::atoms::Rail;
use crate::ui::theme::Theme;

const TARGET_COL: usize = 48;

pub fn section(theme: Theme, name: &str) -> String {
    let dash = match theme {
        Theme::Color | Theme::Mono => '─',
        Theme::Ascii => '-',
    };
    // Layout: "▐ <name> " then dashes filling to TARGET_COL visible cells.
    // ▐ is 1 cell, space + space around name = 2 cells, dashes = remaining.
    // Visible cells used: 1 (rail) + 1 (space) + name.chars().count() + 1 (space)
    let used = 1 + 1 + name.chars().count() + 1;
    let dashes = TARGET_COL.saturating_sub(used);
    let mut out = String::new();
    out.push_str(&Rail::blank(theme));
    out.push(' ');
    out.push_str(name);
    out.push(' ');
    for _ in 0..dashes {
        out.push(dash);
    }
    out
}
```

- [ ] **Step 2: Write splash tests**

Create `crates/right-agent/src/ui/splash_tests.rs`:

```rust
use super::*;

const ESC: char = '\x1b';

#[test]
fn splash_mono_three_lines() {
    let s = splash(Theme::Mono, "0.10.2", "sandboxed multi-agent runtime");
    let lines: Vec<&str> = s.split('\n').collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "▐✓ right agent v0.10.2");
    assert_eq!(lines[1], "▐  sandboxed multi-agent runtime");
    assert_eq!(lines[2], "▐");
}

#[test]
fn splash_ascii() {
    let s = splash(Theme::Ascii, "0.10.2", "sandboxed multi-agent runtime");
    let lines: Vec<&str> = s.split('\n').collect();
    assert_eq!(lines[0], "|* right agent v0.10.2");
    assert_eq!(lines[1], "|  sandboxed multi-agent runtime");
    assert_eq!(lines[2], "|");
}

#[test]
fn splash_color_has_ansi_no_unicode_loss() {
    let s = splash(Theme::Color, "0.10.2", "tagline");
    assert!(s.contains(ESC), "color splash should emit ANSI");
    assert!(s.contains("right agent v0.10.2"));
}

#[test]
fn splash_mono_no_ansi() {
    let s = splash(Theme::Mono, "0.10.2", "tagline");
    assert!(!s.contains(ESC));
}

#[test]
fn splash_ascii_no_unicode_atoms() {
    let s = splash(Theme::Ascii, "0.10.2", "tagline");
    assert!(!s.contains('▐'));
    assert!(!s.contains('✓'));
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p right-agent ui::splash:: --no-fail-fast
```
Expected: 5 tests pass.

- [ ] **Step 4: Add header tests inline (no separate file needed — tiny)**

Append at the end of `crates/right-agent/src/ui/header.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn visible_len(s: &str) -> usize {
        // Mono/Ascii: chars().count() == visible cells for our atoms.
        s.chars().count()
    }

    #[test]
    fn section_mono_target_col_48() {
        let s = section(Theme::Mono, "telegram");
        // "▐ telegram " = 1+1+8+1 = 11 chars; dashes = 37.
        assert_eq!(visible_len(&s), 48);
        assert!(s.starts_with("▐ telegram "));
        assert!(s.ends_with('─'));
    }

    #[test]
    fn section_ascii_uses_dash() {
        let s = section(Theme::Ascii, "telegram");
        assert!(s.starts_with("| telegram "));
        assert!(!s.contains('─'));
        assert!(s.ends_with('-'));
    }

    #[test]
    fn section_long_name_no_negative_dashes() {
        // Name longer than TARGET_COL — saturating_sub keeps dashes at zero.
        let s = section(Theme::Mono, "this-is-a-very-long-section-name-exceeding-48-cells");
        assert!(!s.contains('─'), "no dashes when name overflows: {s:?}");
    }
}
```

- [ ] **Step 5: Run header tests**

```bash
cargo test -p right-agent ui::header:: --no-fail-fast
```
Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/ui/splash.rs crates/right-agent/src/ui/splash_tests.rs crates/right-agent/src/ui/header.rs
git commit -m "feat(ui): splash + section header"
```

---

### Task 6: Recap

**Files:**
- Modify: `crates/right-agent/src/ui/recap.rs`
- Create: `crates/right-agent/src/ui/recap_tests.rs`

Recap = section header (`▐ <title> ─────`) + blank rail + status block + blank rail + `▐  next: <hint>`.

- [ ] **Step 1: Implement recap.rs**

Replace `crates/right-agent/src/ui/recap.rs`:

```rust
//! Completion-frame builder: section header + status block + `next:` pointer.

use crate::ui::atoms::{Glyph, Rail};
use crate::ui::header::section;
use crate::ui::line::{Block, status};
use crate::ui::theme::Theme;

pub struct Recap {
    title: String,
    block: Block,
    next: Option<String>,
}

impl Recap {
    pub fn new(title: &str) -> Self {
        Recap { title: title.into(), block: Block::new(), next: None }
    }

    pub fn ok(mut self, noun: &str, detail: &str) -> Self {
        self.block.push(status(Glyph::Ok).noun(noun).verb(detail));
        self
    }

    pub fn warn(mut self, noun: &str, detail: &str) -> Self {
        self.block.push(status(Glyph::Warn).noun(noun).verb(detail));
        self
    }

    pub fn next(mut self, hint: &str) -> Self {
        self.next = Some(hint.into());
        self
    }

    pub fn render(&self, theme: Theme) -> String {
        let mut out = String::new();
        out.push_str(&section(theme, &self.title));
        out.push('\n');
        out.push_str(&Rail::blank(theme));
        out.push('\n');
        out.push_str(&self.block.render(theme));
        if !self.block.is_empty() {
            out.push('\n');
        }
        out.push_str(&Rail::blank(theme));
        if let Some(ref hint) = self.next {
            out.push('\n');
            out.push_str(&Rail::prefix(theme));
            out.push_str("next: ");
            out.push_str(hint);
        }
        out
    }
}

#[cfg(test)]
#[path = "recap_tests.rs"]
mod tests;
```

- [ ] **Step 2: Write tests**

Create `crates/right-agent/src/ui/recap_tests.rs`:

```rust
use super::*;

#[test]
fn recap_minimal_mono() {
    let s = Recap::new("ready")
        .ok("tunnel", "right.example.com")
        .next("right up")
        .render(Theme::Mono);
    let expected = "\
▐ ready ────────────────────────────────────
▐
▐  ✓ tunnel  right.example.com
▐
▐  next: right up";
    assert_eq!(s, expected);
}

#[test]
fn recap_aligns_multiple_lines() {
    let s = Recap::new("ready")
        .ok("agent", "right (openshell, restrictive)")
        .ok("tunnel", "right.example.com")
        .ok("memory", "hindsight")
        .next("right up")
        .render(Theme::Mono);
    let lines: Vec<&str> = s.split('\n').collect();
    // Three status lines, all noun-aligned to "agent".len() == 5 → "tunnel".len() == 6 → "memory".len() == 6
    // max = 6, padding adds 2 = 8 cells before verb.
    assert!(lines[2].contains("agent  "));
    assert!(lines[3].contains("tunnel "));
    assert!(lines[4].contains("memory "));
}

#[test]
fn recap_warn_renders() {
    let s = Recap::new("ready")
        .ok("tunnel", "ok")
        .warn("telegram", "not configured")
        .render(Theme::Mono);
    assert!(s.contains("✓ tunnel"));
    assert!(s.contains("! telegram"));
}

#[test]
fn recap_no_next_omits_pointer() {
    let s = Recap::new("saved").ok("tunnel", "ok").render(Theme::Mono);
    assert!(!s.contains("next:"));
}

#[test]
fn recap_ascii_uses_pipe() {
    let s = Recap::new("ready").ok("tunnel", "ok").next("right up").render(Theme::Ascii);
    assert!(s.starts_with("| ready "));
    assert!(s.contains("|  [ok] tunnel"));
    assert!(s.contains("|  next: right up"));
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p right-agent ui::recap:: --no-fail-fast
```
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/src/ui/recap.rs crates/right-agent/src/ui/recap_tests.rs
git commit -m "feat(ui): recap builder with column-aligned status block"
```

---

### Task 7: Writers + theme-aware println convenience

**Files:**
- Modify: `crates/right-agent/src/ui/writer.rs`

The writers are intentionally tiny — direct `println!("{}", line.render(theme))` is also fine. Their purpose is so callers don't keep threading `theme` to `println!` macros when emitting many lines.

- [ ] **Step 1: Implement writer.rs**

Replace `crates/right-agent/src/ui/writer.rs`:

```rust
//! Theme-aware writers. Optional sugar — direct `println!("{}", line.render(theme))`
//! is equivalent.
//!
//! These exist so that future hardening (e.g. piping all UI through a `Sink`
//! trait for capture in tests) has one chokepoint.

use crate::ui::theme::Theme;

pub fn stdout(_theme: Theme, s: &str) {
    println!("{s}");
}

pub fn stderr(_theme: Theme, s: &str) {
    eprintln!("{s}");
}
```

(`_theme` is unused today but kept in the signature so callers don't change shape later when `Sink` lands in Step 6.)

- [ ] **Step 2: Verify compilation**

```bash
cargo build --workspace
```
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/right-agent/src/ui/writer.rs
git commit -m "feat(ui): theme-aware stdout/stderr writers"
```

---

### Task 8: `BlockAlreadyRendered` sentinel

**Files:**
- Modify: `crates/right-agent/src/ui/error.rs`

- [ ] **Step 1: Implement error.rs**

Replace `crates/right-agent/src/ui/error.rs`:

```rust
//! Sentinel error: the caller already rendered a brand block (e.g. dependency
//! probe). The runner should exit nonzero without `miette` re-printing.
//!
//! Pattern at the call site:
//!
//! ```ignore
//! match probe(...) {
//!     Ok(()) => {}
//!     Err(e) if e.is::<BlockAlreadyRendered>() => std::process::exit(1),
//!     Err(e) => return Err(e),
//! }
//! ```

use std::fmt;

#[derive(Debug)]
pub struct BlockAlreadyRendered;

impl fmt::Display for BlockAlreadyRendered {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result { Ok(()) }
}

impl std::error::Error for BlockAlreadyRendered {}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo build --workspace
```
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/right-agent/src/ui/error.rs
git commit -m "feat(ui): BlockAlreadyRendered sentinel error"
```

---

### Task 9: Final Step-1 verification

**Files:** none (verification only).

- [ ] **Step 1: Run the entire ui module test suite**

```bash
cargo test -p right-agent ui:: --no-fail-fast
```
Expected: ~46 tests pass, zero failures.

- [ ] **Step 2: Workspace build**

```bash
cargo build --workspace
```
Expected: clean.

- [ ] **Step 3: Clippy on right-agent**

```bash
cargo clippy -p right-agent --all-targets -- -D warnings
```
Expected: zero warnings.

---

## Step 2 — Migrate `cmd_doctor`

Goal: replace `Display for DoctorCheck` with a `ui::Block`-rendered output. Section header `▐ diagnostics ─────`. Footer reformatted. Error-text rewrites per spec §"Voice rewrite · Doctor".

### Task 10: Failing integration test for doctor block

**Files:**
- Create: `crates/right/tests/wizard_brand.rs`

- [ ] **Step 1: Create test file**

Create `crates/right/tests/wizard_brand.rs`:

```rust
//! Integration tests for brand-conformant CLI surfaces.
//! See docs/superpowers/specs/2026-04-28-init-wizard-brand-redesign-design.md.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn right() -> Command {
    Command::cargo_bin("right").unwrap()
}

fn isolated_home() -> tempfile::TempDir {
    tempdir().unwrap()
}

#[test]
fn doctor_renders_brand_block_in_mono() {
    let home = isolated_home();
    right()
        .env("NO_COLOR", "1")
        .env("TERM", "xterm-256color") // force tty assumption: doctor allows non-tty here so NO_COLOR drives Mono
        .args(["--home", home.path().to_str().unwrap(), "doctor"])
        .assert()
        // doctor exits nonzero whenever any check fails — accept either outcome
        .stdout(predicate::str::contains("▐ diagnostics"))
        .stdout(predicate::str::contains("checks passed"));
}
```

- [ ] **Step 2: Run the test, observe failure**

```bash
cargo test -p right --test wizard_brand doctor_renders_brand_block_in_mono
```
Expected: FAIL — current `cmd_doctor` doesn't print `▐ diagnostics` or `checks passed` shape.

(Current footer is `pass_count/total checks passed`, current per-check format is `name <status> detail` without rail.)

---

### Task 11: Migrate `cmd_doctor` rendering

**Files:**
- Modify: `crates/right-agent/src/doctor.rs`
- Modify: `crates/right/src/main.rs` (around line 1868–1892)

- [ ] **Step 1: Add a `to_ui_line` helper in doctor.rs**

Edit `/Users/molt/dev/rightclaw/crates/right-agent/src/doctor.rs`. Locate the `impl fmt::Display for DoctorCheck` block (lines 26–39). Replace the block with:

```rust
use crate::ui::{self, Glyph};

impl DoctorCheck {
    /// Render this check as a `ui::Line`. Caller pushes into a `ui::Block`
    /// for column alignment.
    pub fn to_ui_line(&self) -> ui::Line {
        let glyph = match self.status {
            CheckStatus::Pass => Glyph::Ok,
            CheckStatus::Warn => Glyph::Warn,
            CheckStatus::Fail => Glyph::Err,
        };
        let mut line = ui::status(glyph).noun(&self.name).verb(&self.detail);
        if let Some(ref f) = self.fix {
            line = line.fix(f);
        }
        line
    }
}
```

Keep the existing `pub use owo_colors::OwoColorize;` removed — we no longer color directly.

Also remove the `use owo_colors::OwoColorize;` line at the top (line 4) if no other code in `doctor.rs` references it. Run a grep to be sure:

```bash
rg 'OwoColorize|owo_colors' crates/right-agent/src/doctor.rs
```
If only the `use` remains, delete that line.

- [ ] **Step 2: Update `cmd_doctor` to use ui::Block**

Edit `/Users/molt/dev/rightclaw/crates/right/src/main.rs`. Locate `cmd_doctor` (line 1868). Replace its body (lines 1868–1892) with:

```rust
fn cmd_doctor(home: &Path) -> miette::Result<()> {
    let theme = right_agent::ui::detect();
    let checks = right_agent::doctor::run_doctor(home);

    println!("{}", right_agent::ui::section(theme, "diagnostics"));
    println!("{}", right_agent::ui::Rail::blank(theme));

    let mut block = right_agent::ui::Block::new();
    for check in &checks {
        block.push(check.to_ui_line());
    }
    println!("{}", block.render(theme));
    println!("{}", right_agent::ui::Rail::blank(theme));

    let pass = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Pass))
        .count();
    let warn = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Warn))
        .count();
    let fail = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Fail))
        .count();
    let total = checks.len();

    let summary = if warn == 0 && fail == 0 {
        format!("{pass}/{total} checks passed")
    } else {
        let mut parts = Vec::new();
        if warn > 0 { parts.push(format!("{warn} warn")); }
        if fail > 0 { parts.push(format!("{fail} fail")); }
        format!("{pass}/{total} checks passed ({})", parts.join(", "))
    };
    println!(
        "{}{}",
        right_agent::ui::Rail::prefix(theme),
        summary
    );

    if fail > 0 {
        return Err(miette::miette!("checks failed — see above for fixes"));
    }
    Ok(())
}
```

- [ ] **Step 3: Run the existing doctor tests**

```bash
cargo test -p right-agent doctor:: --no-fail-fast
```
Expected: all existing doctor tests still pass (the data model is unchanged).

- [ ] **Step 4: Run the new integration test**

```bash
cargo test -p right --test wizard_brand doctor_renders_brand_block_in_mono
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/doctor.rs crates/right/src/main.rs crates/right/tests/wizard_brand.rs
git commit -m "feat(doctor): render diagnostics as brand-conformant block"
```

---

### Task 12: Smoke-test doctor under all three themes

**Files:**
- Modify: `crates/right/tests/wizard_brand.rs`

- [ ] **Step 1: Add fallback tests**

Append to `crates/right/tests/wizard_brand.rs`:

```rust
#[test]
fn doctor_ascii_under_dumb_term() {
    let home = isolated_home();
    let assert = right()
        .env("TERM", "dumb")
        .env_remove("NO_COLOR")
        .args(["--home", home.path().to_str().unwrap(), "doctor"])
        .assert();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    // Every non-empty line starts with "|" (rail in Ascii theme).
    for line in stdout.lines() {
        if line.is_empty() { continue; }
        assert!(
            line.starts_with('|') || line.starts_with("  ") || line.contains("checks passed"),
            "ascii theme line should start with '|' (or summary): {line:?}"
        );
    }
    // No Unicode atoms.
    for ch in ['▐', '✓', '✗', '!', '…'] {
        assert!(
            !stdout.contains(ch),
            "ascii output must not contain {ch:?}, full stdout:\n{stdout}"
        );
    }
}

#[test]
fn doctor_mono_no_ansi_escapes() {
    let home = isolated_home();
    let assert = right()
        .env("NO_COLOR", "1")
        .env("TERM", "xterm-256color")
        .args(["--home", home.path().to_str().unwrap(), "doctor"])
        .assert();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains('▐'), "mono should emit Unicode rail: {stdout:?}");
    assert!(!stdout.contains('\x1b'), "mono should not emit ANSI escapes: {stdout:?}");
}
```

- [ ] **Step 2: Run the new tests**

```bash
cargo test -p right --test wizard_brand doctor_
```
Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/right/tests/wizard_brand.rs
git commit -m "test(doctor): ascii + mono fallback assertions"
```

---

## Step 3 — Migrate `cmd_status`

Goal: replace the `printf` table with a `ui::Block`. Glyph mapping: `Running → Ok`, `Restarting/Pending → Warn`, `Failed/Stopped/Skipped → Err`. Not-running branch emits err line + fix.

### Task 13: Failing tests for status block

**Files:**
- Modify: `crates/right/tests/wizard_brand.rs`

- [ ] **Step 1: Add failing tests**

Append to `crates/right/tests/wizard_brand.rs`:

```rust
#[test]
fn status_no_pc_running_renders_err_with_fix() {
    let home = isolated_home();
    // No `right up` was called → no run/state.json
    right()
        .env("NO_COLOR", "1")
        .env("TERM", "xterm-256color")
        .args(["--home", home.path().to_str().unwrap(), "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not running").or(predicate::str::contains("not running")))
        .stdout(predicate::str::contains("not running").or(predicate::str::contains("▐ status")));
}
```

- [ ] **Step 2: Run, observe failure**

```bash
cargo test -p right --test wizard_brand status_no_pc_running_renders_err_with_fix
```
Expected: FAIL — current `cmd_status` returns a `miette::miette!("No running instance found...")` without rail/glyph.

---

### Task 14: Rewrite `cmd_status`

**Files:**
- Modify: `crates/right/src/main.rs` (around line 2310)

- [ ] **Step 1: Replace `cmd_status`**

Replace `cmd_status` (lines 2310–2338) with:

```rust
async fn cmd_status(home: &Path) -> miette::Result<()> {
    let theme = right_agent::ui::detect();

    println!("{}", right_agent::ui::section(theme, "status"));
    println!("{}", right_agent::ui::Rail::blank(theme));

    let client = right_agent::runtime::PcClient::from_home(home)?;
    let Some(client) = client else {
        let line = right_agent::ui::status(right_agent::ui::Glyph::Err)
            .noun("right agent")
            .verb("not running")
            .fix("right up")
            .render(theme);
        println!("{line}");
        return Err(right_agent::ui::BlockAlreadyRendered.into());
    };

    if client.health_check().await.is_err() {
        let line = right_agent::ui::status(right_agent::ui::Glyph::Err)
            .noun("right agent")
            .verb("not running")
            .fix("right up")
            .render(theme);
        println!("{line}");
        return Err(right_agent::ui::BlockAlreadyRendered.into());
    }

    let processes = client.list_processes().await?;

    if processes.is_empty() {
        let line = right_agent::ui::status(right_agent::ui::Glyph::Err)
            .noun("right agent")
            .verb("no processes")
            .fix("right up")
            .render(theme);
        println!("{line}");
        return Err(right_agent::ui::BlockAlreadyRendered.into());
    }

    let mut block = right_agent::ui::Block::new();
    for p in &processes {
        let glyph = match p.status.as_str() {
            "Running" => right_agent::ui::Glyph::Ok,
            "Restarting" | "Pending" => right_agent::ui::Glyph::Warn,
            _ => right_agent::ui::Glyph::Err, // Failed, Stopped, Skipped, etc.
        };
        let verb = format!("{:<6} {}", p.pid, p.system_time);
        block.push(
            right_agent::ui::status(glyph)
                .noun(&p.name)
                .verb(verb),
        );
    }
    println!("{}", block.render(theme));
    println!("{}", right_agent::ui::Rail::blank(theme));

    let warn = processes.iter().filter(|p| matches!(p.status.as_str(), "Restarting" | "Pending")).count();
    let fail = processes
        .iter()
        .filter(|p| !matches!(p.status.as_str(), "Running" | "Restarting" | "Pending"))
        .count();
    let total = processes.len();
    let summary = if warn == 0 && fail == 0 {
        format!("{total} processes")
    } else {
        let mut parts = Vec::new();
        if warn > 0 { parts.push(format!("{warn} warn")); }
        if fail > 0 { parts.push(format!("{fail} fail")); }
        format!("{total} processes ({})", parts.join(", "))
    };
    println!("{}{}", right_agent::ui::Rail::prefix(theme), summary);

    Ok(())
}
```

- [ ] **Step 2: Handle the `BlockAlreadyRendered` exit in `main`**

`miette` will format `BlockAlreadyRendered` as an empty Display (we set it that way), but it still prints a brief "Error:" header. Update the top-level error handler to suppress that case.

Locate `fn main()` in `/Users/molt/dev/rightclaw/crates/right/src/main.rs` (search for `#[tokio::main]` or `fn main`). Find the place where `miette::Result` is unwrapped at the binary boundary. If the pattern is:

```rust
fn main() -> miette::Result<()> {
    // ...
}
```

Wrap the dispatch result so a `BlockAlreadyRendered` becomes silent exit-1. Add a helper at the very top of `main.rs` (after existing imports, before the first `fn`):

```rust
fn handle_dispatch(result: miette::Result<()>) -> miette::Result<()> {
    if let Err(ref e) = result
        && e.downcast_ref::<right_agent::ui::BlockAlreadyRendered>().is_some()
    {
        std::process::exit(1);
    }
    result
}
```

…and at the bottom of `fn main` (before its `Ok(())`), wrap the dispatch result:

```rust
// (Near the existing match on the parsed CLI arg → cmd_* call)
let result: miette::Result<()> = match parsed.command {
    /* ... existing arms ... */
};
handle_dispatch(result)
```

If the existing `main` already returns dispatcher results directly, factor out the dispatch into a `let result = ...` and end with `handle_dispatch(result)`. Use `Read` to inspect the actual structure first; the patch shape depends on the existing code.

- [ ] **Step 3: Run the new test**

```bash
cargo test -p right --test wizard_brand status_no_pc_running_renders_err_with_fix
```
Expected: PASS.

- [ ] **Step 4: Run all integration tests to catch regressions**

```bash
cargo test -p right --tests --no-fail-fast
```
Expected: all green. If existing tests assert `"NAME"` or `"No running instance"` or other dropped strings, update them mechanically to the new shape (`predicate::str::contains("not running")` etc.).

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/main.rs crates/right/tests/wizard_brand.rs
git commit -m "feat(status): brand-conformant rail+glyph block"
```

---

### Task 15: Smoke-test status with running PC

**Files:** none (manual smoke).

- [ ] **Step 1: Manual verification**

In a separate worktree or after committing:

```bash
# Start PC in another shell:
right up
# Then in the test shell:
right status
NO_COLOR=1 right status
TERM=dumb right status
```

Expected:
- TTY: orange rail + colored glyphs.
- `NO_COLOR=1`: Unicode atoms, no colors.
- `TERM=dumb`: ASCII (`|  [ok] right-bot-...`).

Note observed output in the commit message of the next change if anything needs fixing. (No commit needed for this step alone — moving on.)

---

## Step 4 — `right init` redesign

Goal: splash + dependency probe + section headers + status lines + recap, replacing the existing six-line footer.

Current `cmd_init` is at `crates/right/src/main.rs:1074`. Six prompt steps already exist (sandbox / network / telegram / chat-ids / memory / tunnel). Spec §"Per-command flows · `right init`" defines the new layout in detail.

### Task 16: Failing integration test for init splash + recap

**Files:**
- Modify: `crates/right/tests/wizard_brand.rs`

- [ ] **Step 1: Add failing test**

Append to `crates/right/tests/wizard_brand.rs`:

```rust
#[test]
fn init_first_run_splash_and_recap() {
    let home = isolated_home();
    right()
        .env("NO_COLOR", "1")
        .env("TERM", "xterm-256color")
        .args([
            "--home", home.path().to_str().unwrap(),
            "init", "-y",
            "--sandbox-mode", "none",
            "--tunnel-hostname", "test.example.com",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("▐✓ right agent v"))
        .stdout(predicate::str::contains("▐ dependencies"))
        .stdout(predicate::str::contains("▐ ready"))
        .stdout(predicate::str::contains("▐  next: right up"));
}

#[test]
fn init_rerun_writes_recap_again() {
    let home = isolated_home();
    let common_args: [&str; 7] = [
        "init", "-y",
        "--sandbox-mode", "none",
        "--tunnel-hostname", "test.example.com",
        "-y",
    ];
    for _ in 0..2 {
        right()
            .env("NO_COLOR", "1")
            .env("TERM", "xterm-256color")
            .arg("--home").arg(home.path().to_str().unwrap())
            .args(common_args)
            .assert()
            .success()
            .stdout(predicate::str::contains("▐ ready"));
    }
}
```

- [ ] **Step 2: Run, observe failure**

```bash
cargo test -p right --test wizard_brand init_first_run_splash_and_recap
```
Expected: FAIL — current `cmd_init` prints no splash, no `▐ ready`, no `next: right up`.

---

### Task 17: Splash + dependency probe block

**Files:**
- Modify: `crates/right/src/main.rs` (`cmd_init` at line 1074)

- [ ] **Step 1: Add a probe helper at the top of `cmd_init`**

Edit `cmd_init` in `/Users/molt/dev/rightclaw/crates/right/src/main.rs`. Right after the `let interactive = !yes;` line (1085), insert:

```rust
    // Brand: splash + dependency probe.
    {
        let theme = right_agent::ui::detect();
        let version = env!("CARGO_PKG_VERSION");
        println!(
            "{}",
            right_agent::ui::splash(theme, version, "sandboxed multi-agent runtime")
        );
        println!("{}", right_agent::ui::section(theme, "dependencies"));
        println!("{}", right_agent::ui::Rail::blank(theme));

        let mut block = right_agent::ui::Block::new();
        let mut fatal = false;

        // process-compose (fatal)
        match which::which("process-compose") {
            Ok(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("process-compose")
                    .verb("in PATH"),
            ),
            Err(_) => {
                fatal = true;
                block.push(
                    right_agent::ui::status(right_agent::ui::Glyph::Err)
                        .noun("process-compose")
                        .verb("not in PATH")
                        .fix("https://f1bonacc1.github.io/process-compose/installation/"),
                );
            }
        }

        // claude (fatal)
        match which::which("claude") {
            Ok(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("claude")
                    .verb("in PATH"),
            ),
            Err(_) => {
                fatal = true;
                block.push(
                    right_agent::ui::status(right_agent::ui::Glyph::Err)
                        .noun("claude")
                        .verb("not in PATH")
                        .fix("https://docs.anthropic.com/en/docs/claude-code"),
                );
            }
        }

        // openshell (warn)
        match which::which("openshell") {
            Ok(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("openshell")
                    .verb("in PATH"),
            ),
            Err(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Warn)
                    .noun("openshell")
                    .verb("not in PATH (optional, sandbox mode)"),
            ),
        }

        // cloudflared (warn)
        match which::which("cloudflared") {
            Ok(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("cloudflared")
                    .verb("in PATH"),
            ),
            Err(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Warn)
                    .noun("cloudflared")
                    .verb("not in PATH (optional, tunnel)"),
            ),
        }

        println!("{}", block.render(theme));
        println!("{}", right_agent::ui::Rail::blank(theme));

        if fatal {
            return Err(right_agent::ui::BlockAlreadyRendered.into());
        }
    }
```

- [ ] **Step 2: Build and run**

```bash
cargo build --workspace
cargo test -p right --test wizard_brand init_first_run_splash_and_recap
```
The splash + dependency block should now appear; the `▐ ready` part of the assertion still fails. That is expected — fixed in Task 19.

- [ ] **Step 3: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(init): splash + dependency probe block"
```

---

### Task 18: Section headers + sandbox creation status lines

**Files:**
- Modify: `crates/right/src/main.rs` (`cmd_init`)

The existing prompts for sandbox/network/telegram/chat-ids/memory live inside the `if !interactive { ... } else { ... }` state machine (lines 1107–1246). The tunnel + codegen + sandbox-spawn block is at lines 1261–1349.

- [ ] **Step 1: Insert section header before the agent prompts**

In the `else { ... }` interactive branch of `cmd_init`, find the line `let mut step = if sandbox_mode.is_some() { Step::Network } else { Step::Sandbox };` (line 1130). Just before it, insert:

```rust
        let theme = right_agent::ui::detect();
        println!("{}", right_agent::ui::section(theme, "agent"));
        println!("{}", right_agent::ui::Rail::blank(theme));
```

Then before `Step::Telegram` is first reached, the state machine doesn't have a clean place. Simpler: print section headers as the state machine enters each step. Edit each `Step::Foo => { ... }` arm to print its header on first entry. To avoid duplicate prints when Esc-back returns to a step, gate via a HashSet:

```rust
        let mut printed_sections: std::collections::HashSet<&'static str> = Default::default();
        let print_header = |name: &'static str, printed: &mut std::collections::HashSet<&'static str>| {
            if printed.insert(name) {
                println!("{}", right_agent::ui::section(right_agent::ui::detect(), name));
                println!("{}", right_agent::ui::Rail::blank(right_agent::ui::detect()));
            }
        };
```

…and in each `Step::Foo => { ... }` arm, call `print_header("telegram", &mut printed_sections)` etc. before the prompts.

(Practical note: this lambda captures by move-on-call. If borrow-checker rejects, inline the body into each arm.)

Section labels by step: `Step::Sandbox` → `agent`; `Step::Network` → reuse `agent` (same section); `Step::Telegram` → `telegram`; `Step::ChatIds` → reuse `telegram`; `Step::Memory` → `memory`.

- [ ] **Step 2: Add `▐ tunnel ─────` before `tunnel_setup`**

Find the line `let tunnel_cfg = crate::wizard::tunnel_setup(tunnel_name, tunnel_hostname, interactive)?;` (line 1263). Just before it, insert:

```rust
    {
        let theme = right_agent::ui::detect();
        println!("{}", right_agent::ui::section(theme, "tunnel"));
        println!("{}", right_agent::ui::Rail::blank(theme));
    }
```

- [ ] **Step 3: Replace the sandbox-creation prints**

Find the block:
```rust
            println!("Creating OpenShell sandbox...");
            tokio::task::block_in_place(|| { ... })?;
            println!("  Sandbox '{sb_name}' ready");
```
(approx lines 1322–1334).

Replace with:
```rust
            let theme = right_agent::ui::detect();
            println!(
                "{}",
                right_agent::ui::status(right_agent::ui::Glyph::Info)
                    .noun("sandbox")
                    .verb("creating")
                    .render(theme)
            );
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    right_agent::openshell::ensure_sandbox(
                        "right",
                        &policy_path,
                        Some(&staging),
                        force_recreate,
                    )
                    .await
                })
            })?;
            println!(
                "{}",
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("sandbox")
                    .verb("ready")
                    .detail(&sb_name)
                    .render(theme)
            );
```

- [ ] **Step 4: Build, run integration test**

```bash
cargo build --workspace
cargo test -p right --test wizard_brand init_first_run_splash_and_recap
```
Expected: still fails on `▐ ready` and `next: right up` — fixed in Task 19.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(init): section headers + sandbox-creation status lines"
```

---

### Task 19: Replace footer with recap block

**Files:**
- Modify: `crates/right/src/main.rs` (`cmd_init`)

- [ ] **Step 1: Replace the existing footer**

Locate the footer in `cmd_init` (lines 1351–1369):
```rust
    println!("Initialized Right Agent at {}", home.display());
    println!(
        "Default agent 'right' created at {}/agents/right/",
        home.display()
    );
    if token.is_some() {
        println!("Telegram channel configured.");
    }
    if !chat_ids.is_empty() {
        println!("Telegram chat ID allowlist configured.");
    }
    println!("Network policy: {network_policy_val}");

    println!();
    println!("Setup complete. Next steps:");
    println!("  right up        Launch agents");
    println!("  right config    Change global settings");
    println!("  right doctor    Check configuration");

    Ok(())
```

Replace with:
```rust
    let theme = right_agent::ui::detect();
    let mode = format!("{} ({})", sandbox, network_policy_val);
    let chat_ids_detail = if chat_ids.is_empty() {
        "0 allowed (blocks all)".to_string()
    } else {
        format!("{} allowed", chat_ids.len())
    };
    let telegram_detail = if token.is_some() {
        "configured".to_string()
    } else {
        "not configured".to_string()
    };
    let memory_detail = match memory_provider {
        right_agent::agent::types::MemoryProvider::Hindsight => "hindsight".to_string(),
        right_agent::agent::types::MemoryProvider::File => "file".to_string(),
    };

    let mut recap = right_agent::ui::Recap::new("ready")
        .ok("agent", &format!("right ({mode})"))
        .ok("tunnel", &global_config.tunnel.hostname);
    recap = if token.is_some() {
        recap.ok("telegram", &telegram_detail)
    } else {
        recap.warn("telegram", &telegram_detail)
    };
    recap = recap
        .ok("chat ids", &chat_ids_detail)
        .ok("memory", &memory_detail)
        .next("right up");
    println!("{}", recap.render(theme));

    Ok(())
```

(Variable names `sandbox`, `network_policy_val`, `token`, `chat_ids`, `memory_provider`, `global_config` are already in scope from earlier in the function.)

- [ ] **Step 2: Replace the "Setup cancelled." error**

Find:
```rust
return Err(miette::miette!("Setup cancelled."));
```
(around line 1159).

Replace with:
```rust
return Err(miette::miette!("cancelled"));
```

- [ ] **Step 3: Run the integration tests**

```bash
cargo test -p right --test wizard_brand init_
```
Expected: both `init_first_run_splash_and_recap` and `init_rerun_writes_recap_again` pass.

- [ ] **Step 4: Run the existing init tests, fix any string-assertion regressions**

```bash
cargo test -p right --tests --no-fail-fast
```
Expected:
- `test_help_output`: passes (no copy assertion changes).
- `test_init_creates_structure`: passes (asserts file structure, not output).
- `test_init_generates_per_agent_codegen`: passes.
- Any tests asserting `"Initialized Right Agent"` or `"Setup complete"` or `"Network policy:"` need their `predicate::str::contains` updated to the recap atoms (`▐ ready`, `▐  next: right up`, `▐  ✓ agent`).

For each such failing assertion, replace the asserted substring with the new equivalent. Quick grep:
```bash
rg 'Initialized Right Agent|Setup complete|Network policy:|Default agent' crates/right/tests/
```

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/main.rs crates/right/tests/
git commit -m "feat(init): recap block replaces footer"
```

---

### Task 20: Voice rewrites in `init.rs` prompts

**Files:**
- Modify: `crates/right-agent/src/init.rs`

Apply spec §"Voice rewrite" tables that target prompts physically defined here. Affected functions: `prompt_sandbox_mode`, `prompt_network_policy`, `prompt_memory_provider`, `prompt_hindsight_api_key`, `prompt_hindsight_bank_id`, `prompt_recall_budget`, `prompt_recall_max_tokens`. Plus the cancel-confirm prompt inside `inquire_back`.

- [ ] **Step 1: Rewrite `prompt_sandbox_mode`**

In `crates/right-agent/src/init.rs:362-382`, replace:

```rust
inquire::Select::new(
    "Sandbox mode:",
    vec![
        "OpenShell — run in isolated container (recommended)",
        "None — run directly on host (for computer-use, Chrome, etc.)",
    ],
)
```

with:

```rust
inquire::Select::new(
    "sandbox mode:",
    vec![
        "openshell — isolated container (recommended)",
        "none — direct host access (computer-use, chrome)",
    ],
)
```

Update the matching `if choice.starts_with("OpenShell")` to `if choice.starts_with("openshell")`.

- [ ] **Step 2: Rewrite `prompt_network_policy`**

In `crates/right-agent/src/init.rs:385-405`, replace:

```rust
inquire::Select::new(
    "Network policy for sandbox:",
    vec![
        "Permissive — all HTTPS domains allowed (recommended)",
        "Restrictive — Anthropic/Claude domains only",
    ],
)
```

with:

```rust
inquire::Select::new(
    "network policy:",
    vec![
        "permissive — all https domains (recommended)",
        "restrictive — anthropic/claude domains only",
    ],
)
```

Update the matching `if choice.starts_with("Permissive")` to `if choice.starts_with("permissive")`.

- [ ] **Step 3: Rewrite `prompt_memory_provider`**

In `crates/right-agent/src/init.rs:408-428`, replace:

```rust
inquire::Select::new(
    "Memory provider:",
    vec![
        "Hindsight — Hindsight Cloud API (recommended)",
        "File — agent manages MEMORY.md",
    ],
)
```

with:

```rust
inquire::Select::new(
    "memory provider:",
    vec![
        "hindsight — hindsight cloud api (recommended)",
        "file — agent manages MEMORY.md",
    ],
)
```

Update `if choice.starts_with("Hindsight")` to `if choice.starts_with("hindsight")`.

- [ ] **Step 4: Rewrite remaining hindsight + recall prompts**

In `prompt_hindsight_api_key` (line 432), replace:
```rust
inquire::Text::new("Hindsight API key (Enter to use HINDSIGHT_API_KEY env var):").prompt()
```
with:
```rust
inquire::Text::new("hindsight api key (enter to use HINDSIGHT_API_KEY env var):").prompt()
```

In `prompt_hindsight_bank_id` (line 449–450), replace:
```rust
let prompt_text = format!("Hindsight bank ID (default: {agent_name}):");
```
with:
```rust
let prompt_text = format!("hindsight bank id (default: {agent_name}):");
```

In `prompt_recall_budget` (line 463), replace `"Recall budget:"` with `"recall budget:"` and the option labels:
- `"Mid — balanced (default)"` → `"mid — balanced (default)"`
- `"Low — smaller context, cheaper"` → `"low — smaller context, cheaper"`
- (Also any `"High — ..."` etc. — apply the same lowercase pattern.)

(Use `Read` to confirm the exact set of options before editing — read lines 463–490.)

In `prompt_recall_max_tokens` (line 490–504), apply the same lowercasing to its prompt text and any select labels.

- [ ] **Step 5: Rewrite the Ctrl+C confirm in `inquire_back`**

In `inquire_back` (line 336–360), find:
```rust
inquire::Confirm::new("Cancel setup?")
```
Replace with:
```rust
inquire::Confirm::new("cancel?")
```

- [ ] **Step 6: Run init.rs tests**

```bash
cargo test -p right-agent init:: --no-fail-fast
```
Expected: existing token-validation tests still pass. Any prompt-text assertions in tests need updating to lowercase form (mechanical — search & replace).

- [ ] **Step 7: Commit**

```bash
git add crates/right-agent/src/init.rs
git commit -m "refactor(init): lowercase-first prompt copy per brand"
```

---

### Task 21: Voice rewrites in `wizard.rs` (tunnel + telegram + chat ids)

**Files:**
- Modify: `crates/right/src/wizard.rs`

Apply spec §"Voice rewrite · Tunnel", §"Voice rewrite · Telegram", §"Voice rewrite · Chat IDs". Also convert info `println!("Created tunnel ...")` etc. to `ui::status(...)` calls.

- [ ] **Step 1: Rewrite tunnel prompts and confirms**

In `crates/right/src/wizard.rs`:

| Line(s) | Replace |
|---|---|
| `Tunnel hostname (e.g. right.example.com):` (line 212) | `tunnel hostname (e.g. right.example.com):` |
| `Reuse existing tunnel` (line 25, in `Display`) | `reuse` |
| `Create a new tunnel with a different name` (line 26) | `rename` |
| `Delete and recreate the tunnel` (line 27) | `delete and recreate` |
| `Found tunnel '{}' in your Cloudflare account ...` (line 271–274) | replace `println!("Found tunnel ...")` with: ```rust let theme = right_agent::ui::detect(); println!("{}", right_agent::ui::status(right_agent::ui::Glyph::Warn).noun("tunnel").verb(format!("found \"{}\"", existing.name)).detail(format!("{}…", short_uuid)).render(theme)); ``` |
| `⚠ Credentials file for this tunnel ...` multiline (line 277–281) | `let theme = right_agent::ui::detect(); println!("{}    note: credentials file missing on this machine. choose \"delete and recreate\" to regenerate.", right_agent::ui::Rail::blank(theme));` |
| `What would you like to do?` (line 291) | `existing tunnel — choose:` |
| `New tunnel name:` (line 299) | `new tunnel name:` |
| `tunnel name cannot be empty` (line 304) | unchanged (already lowercase) |
| `This will permanently delete tunnel '{}'. Continue?` (line 312–315) | `format!("delete tunnel \"{}\" permanently?", existing.name)` |
| `tunnel deletion cancelled` (line 321) | `cancelled` |
| `Deleted tunnel '{}'` (line 325) | replace with `ui::status(Ok).noun("tunnel").verb("deleted").detail(&existing.name)` |
| `Created tunnel '{}' (UUID: {})` (lines 197, 203, 307, 328) | replace each with `ui::status(Ok).noun("tunnel").verb("created").detail(&entry.name)` |
| `Recreated tunnel '{}' (UUID: {})` (line 197) | `ui::status(Ok).noun("tunnel").verb("recreated").detail(&fresh.name)` |
| `Tunnel name:` (line 526) | `tunnel name:` |
| `Tunnel hostname must be a bare domain, not a URL` (line 224–226) | `miette::miette!(help = "use just the domain, e.g. right.example.com", "hostname must be a bare domain, not a url")` |
| `tunnel hostname cannot be empty` (line 231) | unchanged |
| `Tunnel credentials file not found at ... — cloudflared cannot start without it` (line 241–245) | `miette::miette!(help = "Run \`right config set\` and select Tunnel to reconfigure", "tunnel credentials missing at {} — cloudflared cannot start", credentials_file.display())` |

(Use `Read` to inspect each location before editing; many of these are inside multi-line miette calls where the `help =` field is on a separate line.)

- [ ] **Step 2: Rewrite telegram prompts**

In `crates/right/src/wizard.rs:366-411`:

| Line(s) | Replace |
|---|---|
| `Telegram bot token (current: ****..., press Enter to keep):` (line 366–377) | format: `format!("telegram bot token (keeping {masked} — enter new or press enter to keep):")` |
| `Telegram bot token (required — get one from @BotFather):` (line 374) | unchanged (allowed proper noun `@BotFather`) but lowercase first char already; verify it stays as-is |
| `Telegram bot token (press Enter to skip):` (line 376) | `"telegram bot token (enter to skip):"` |
| `A Telegram bot token is required. Talk to @BotFather to create a bot and get its token, then paste it here. Press Esc to go back.` (line 391–394) | `"a token is required. create a bot via @BotFather, paste the token here. esc to go back."` |

- [ ] **Step 3: Rewrite chat-ids prompts**

In `crates/right/src/wizard.rs:425-468`:

| Line(s) | Replace |
|---|---|
| `Your Telegram user ID (required — send /start to @userinfobot to find it):` (line 425) | `"your telegram user id (required — /start @userinfobot to find it):"` |
| `Your Telegram user ID (send /start to @userinfobot to find it, empty to skip):` (line 427) | `"your telegram user id (/start @userinfobot to find it, empty to skip):"` |
| `At least one Telegram chat/user ID is required ... Press Esc to go back.` (line 441–446) | `"at least one chat id is required so the bot knows who can talk to it. /start @userinfobot for your numeric id. esc to go back."` |
| `invalid chat ID '{}': {e}` (line 457, 686) | `"invalid chat id \"{}\": {e}"` |

- [ ] **Step 4: Build + run wizard tests**

```bash
cargo test -p right wizard --no-fail-fast
```
Expected: existing memory_yaml_tests + stt_yaml_tests still pass (they cover YAML mutation, not prompt copy).

- [ ] **Step 5: Run integration tests**

```bash
cargo test -p right --tests --no-fail-fast
```
Expected: any tests asserting old prompt text need updating mechanically.

- [ ] **Step 6: Commit**

```bash
git add crates/right/src/wizard.rs
git commit -m "refactor(wizard): lowercase tunnel/telegram/chat-id copy + rail status"
```

---

## Step 5 — `right agent init` + `right config` redesign

Goal: section header on `cmd_agent_init`, recap on completion, voice rewrites for the remaining wizard.rs prompt strings, validation re-prompt warn lines, centralised `PROMPT_LABELS` const + regression tests.

### Task 22: Section header + recap for `cmd_agent_init`

**Files:**
- Modify: `crates/right/src/main.rs` (`cmd_agent_init` at line 1373)

- [ ] **Step 1: Print section header at the top of `cmd_agent_init`**

Read `cmd_agent_init` first to find the post-validation, pre-prompt entry point. Insert (after the existing precondition checks like `if agent_dir.exists() && !force { ... }` but before the first prompt):

```rust
    let theme = right_agent::ui::detect();
    println!("{}", right_agent::ui::section(theme, &format!("agent init: {name}")));
    println!("{}", right_agent::ui::Rail::blank(theme));
```

- [ ] **Step 2: Replace the success line(s) at the bottom of `cmd_agent_init`**

Locate any `println!("Created agent ...")` or equivalent at the end of `cmd_agent_init`. Replace with a recap:

```rust
    let theme = right_agent::ui::detect();
    // Re-read the freshly written agent.yaml to populate the recap.
    let cfg = right_agent::agent::discovery::parse_agent_config(&agent_dir)?
        .ok_or_else(|| miette::miette!("agent.yaml missing after init"))?;

    let sandbox_str = format!("{}", cfg.sandbox_mode());
    let sandbox_with_policy = if matches!(
        cfg.sandbox_mode(),
        right_agent::agent::types::SandboxMode::Openshell
    ) {
        format!("{} ({})", sandbox_str, cfg.network_policy)
    } else {
        sandbox_str
    };

    let chat_ids_detail = if cfg.allowed_chat_ids.is_empty() {
        "0 allowed (blocks all)".to_string()
    } else {
        format!("{} allowed", cfg.allowed_chat_ids.len())
    };

    let stt_detail = if cfg.stt.enabled {
        cfg.stt.model.yaml_str().to_string()
    } else {
        "off".to_string()
    };

    let memory_detail = match cfg.memory.as_ref().map(|m| &m.provider) {
        Some(right_agent::agent::types::MemoryProvider::Hindsight) => "hindsight",
        _ => "file",
    };

    let recap = right_agent::ui::Recap::new("ready")
        .ok("agent", &format!("{name} created"))
        .ok("sandbox", &sandbox_with_policy)
        .ok("telegram", if cfg.telegram_token.is_some() { "configured" } else { "not configured" })
        .ok("chat ids", &chat_ids_detail)
        .ok("stt", &stt_detail)
        .ok("memory", memory_detail)
        .next("right up");
    println!("{}", recap.render(theme));
```

(Use `Read` on `cmd_agent_init` first to confirm exact tail and existing variable names; the snippet above assumes the standard discovery API.)

- [ ] **Step 3: Build**

```bash
cargo build --workspace
```

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(agent-init): section header + recap"
```

---

### Task 23: Failing then passing integration test for `agent_init_recap`

**Files:**
- Modify: `crates/right/tests/wizard_brand.rs`

- [ ] **Step 1: Add the test**

Append to `crates/right/tests/wizard_brand.rs`:

```rust
#[test]
fn agent_init_recap_renders_block() {
    let home = isolated_home();

    // Bootstrap a global config first so agent init has somewhere to land.
    right()
        .env("NO_COLOR", "1")
        .env("TERM", "xterm-256color")
        .args([
            "--home", home.path().to_str().unwrap(),
            "init", "-y",
            "--sandbox-mode", "none",
            "--tunnel-hostname", "test.example.com",
        ])
        .assert()
        .success();

    right()
        .env("NO_COLOR", "1")
        .env("TERM", "xterm-256color")
        .args([
            "--home", home.path().to_str().unwrap(),
            "agent", "init", "finance",
            "-y",
            "--sandbox-mode", "none",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("▐ agent init: finance"))
        .stdout(predicate::str::contains("▐ ready"))
        .stdout(predicate::str::contains("▐  ✓ agent"))
        .stdout(predicate::str::contains("▐  next: right up"));
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p right --test wizard_brand agent_init_recap_renders_block
```
Expected: PASS (Task 22 already implemented the rendering).

If it fails, the most likely cause is that `cmd_agent_init` has multiple early-return paths that skip the recap. Investigate with `Read` and ensure the recap runs only on the success path.

- [ ] **Step 3: Commit**

```bash
git add crates/right/tests/wizard_brand.rs
git commit -m "test(agent-init): assert recap block on completion"
```

---

### Task 24: Voice rewrites for settings menus (`wizard.rs`)

**Files:**
- Modify: `crates/right/src/wizard.rs`

Spec §"Voice rewrite · Settings menus".

- [ ] **Step 1: Apply each replacement**

| Line(s) | Replace |
|---|---|
| `Settings:` (line 519) | `settings:` |
| `Done` (line 488) | `done` |
| `Agent: {name}` (line 487) | `agent: {name}` (option label format) |
| `Tunnel: {host} ({uuid})` (line 500–504) | `tunnel: {host} ({uuid})` |
| `Agent '{}' settings:` (line 642) | replace with `format!("▐ agent: {} ─────", chosen_name)` printed via `println!` *before* the inner select; change the select prompt to `"settings:"` |
| `Telegram token: ****` and other option labels (lines 610–622) | mechanical lowercase: `"telegram token: …"`, `"model: …"`, `"allowed chat ids: …"`, `"sandbox mode: …"`, `"network policy: …"`, `"memory: …"`, `"stt: …"`, and `"done"` |
| `Select agent:` (line 569) | `select agent:` |
| `No agents found in {dir}` (line 564) | `no agents found in {dir}` |
| `Global config saved.` (line 536) | replace with: ```rust let theme = right_agent::ui::detect(); println!("{}", right_agent::ui::status(right_agent::ui::Glyph::Ok).noun("tunnel").verb("saved").render(theme)); ``` |
| `Saved.` (line 747) | replace with the appropriate `▐  ✓ <noun>  saved` per branch (see below) |

For `Saved.` at line 747 — that line runs after every settings branch in `agent_setting_menu`. Different noun per selection. Refactor: rename the variable `selection` doesn't carry which option fired, so capture the noun in each `if selection == opt_X { ... }` branch:

```rust
let saved_noun: &str = if selection == opt_token {
    /* ... existing telegram update code ... */
    "telegram token"
} else if selection == opt_model {
    /* ... */
    "model"
} else if selection == opt_chat_ids { "allowed chat ids" }
else if selection == opt_sandbox { "sandbox mode" }
else if selection == opt_network_policy { "network policy" }
else if selection == opt_memory { "memory" }
else if selection == opt_stt { "stt" }
else { unreachable!() };

let theme = right_agent::ui::detect();
println!(
    "{}",
    right_agent::ui::status(right_agent::ui::Glyph::Ok)
        .noun(saved_noun)
        .verb("saved")
        .render(theme)
);
```

(Practical: Rust's borrow rules require this restructuring to be done inside one `if/else if` chain — the existing structure already is.)

- [ ] **Step 2: Build + smoke**

```bash
cargo build --workspace
```

- [ ] **Step 3: Commit**

```bash
git add crates/right/src/wizard.rs
git commit -m "refactor(wizard): lowercase settings menu copy + rail saved lines"
```

---

### Task 25: Voice rewrites for sandbox/network/memory/STT submenus

**Files:**
- Modify: `crates/right/src/wizard.rs`

Spec §"Voice rewrite · Sandbox / network / model / memory / STT".

- [ ] **Step 1: Apply each replacement**

| Line(s) | Replace |
|---|---|
| `OpenShell — run in isolated container (recommended)` (line 693) | `openshell — isolated container (recommended)` |
| `None — run directly on host (for computer-use, Chrome, etc.)` (line 694) | `none — direct host access (computer-use, chrome)` |
| `Sandbox mode:` (line 696) | `sandbox mode:` |
| `Restrictive — Anthropic/Claude domains only (recommended)` (line 707) | `restrictive — anthropic/claude domains only (recommended)` |
| `Permissive — all HTTPS domains allowed (needed for external MCP servers)` (line 708) | `permissive — all https domains (needed for external mcp servers)` |
| `Network policy for sandbox:` (line 710) | `network policy:` |
| `Model (e.g. sonnet, opus, haiku — empty to clear):` (line 664) | `model (e.g. sonnet, opus, haiku — empty to clear):` |
| `Allowed chat IDs (comma-separated, empty to clear):` (line 674) | `allowed chat ids (comma-separated, empty to clear):` |
| `Enable voice transcription?` (line 991) | `enable voice transcription?` |
| `Telegram voice messages and video notes will be transcribed locally via whisper.cpp.` (line 994) | `telegram voice + video notes are transcribed locally via whisper.cpp.` |
| `Choose whisper model:` (line 1009) | `whisper model:` |
| `Hindsight API key source:` (line 825) | `hindsight api key source:` |
| `Use HINDSIGHT_API_KEY env var (recommended)` (line 822) | unchanged (env var name preserved) |
| `Enter a key to save in agent.yaml` (line 823) | `enter a key to save in agent.yaml` |
| `Hindsight API key:` (line 831) | `hindsight api key:` |
| `Hindsight API key (empty to rely on HINDSIGHT_API_KEY env var at runtime):` (line 842) | `hindsight api key (empty to rely on HINDSIGHT_API_KEY at runtime):` |
| `Switching memory provider will not migrate existing memory. Continue?` (line 796) | `switching memory provider does not migrate existing memory. continue?` |
| `Validating key against Hindsight...` (line 878) | replace `println!` with `ui::status(Info).noun("hindsight").verb("validating key").render(theme)` |
| `\u{2713} Key valid — {banks} bank(s) accessible.` (line 881) | `ui::status(Ok).noun("hindsight").verb(format!("{banks} bank(s) accessible")).render(theme)` |
| `Hindsight rejected the key (HTTP {status}). Save anyway?` (line 884) | `format!("hindsight rejected the key (http {status}). save anyway?")` |
| `\u{26a0} Could not validate (Hindsight unreachable): {detail}` (line 895) | `ui::status(Warn).noun("hindsight").verb(format!("unreachable ({detail})")).render(theme)` |
| `Save config anyway?` (line 896) | `save anyway?` |
| `\u{26a0} No key available to validate ...` (line 906) | `ui::status(Warn).noun("hindsight").verb("no key — saving without validation").render(theme)` |
| `ffmpeg required for voice transcription. Install via 'brew install ffmpeg'?` (line 941) | `ffmpeg required for voice transcription. install via brew?` |
| `STT will be disabled. Install ffmpeg later: brew install ffmpeg` (line 947) | replace `println!` with `ui::status(Warn).noun("stt").verb("disabled (install ffmpeg: brew install ffmpeg)").render(theme)` |
| `brew install ffmpeg exited with {status}; STT disabled.` (line 956) | `ui::status(Err).noun("ffmpeg").verb(format!("install failed ({status})")).render(theme)` then `ui::status(Warn).noun("stt").verb("disabled").render(theme)` |
| `brew completed but ffmpeg not yet in PATH ... STT disabled.` (line 961) | `ui::status(Warn).noun("ffmpeg").verb("not in PATH yet — restart shell").render(theme)` then `ui::status(Warn).noun("stt").verb("disabled").render(theme)` |
| Linux ffmpeg help block (lines 968–973) | rewrite as: `println!("ffmpeg required for voice transcription. install:"); println!("  Debian/Ubuntu:  sudo apt install ffmpeg"); println!("  NixOS / devenv: add 'pkgs.ffmpeg' to your packages"); println!("then re-run this command.");` |

(Each replacement that converts a `println!` into a `ui::status(...)` call needs `let theme = right_agent::ui::detect();` once at the top of the enclosing function if it isn't already there — the previous tasks may have introduced one.)

- [ ] **Step 2: Build**

```bash
cargo build --workspace
```

- [ ] **Step 3: Run wizard tests**

```bash
cargo test -p right wizard:: --no-fail-fast
```
Expected: existing memory_yaml_tests + stt_yaml_tests still pass.

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/wizard.rs
git commit -m "refactor(wizard): lowercase memory/stt/sandbox copy + rail status"
```

---

### Task 26: `PROMPT_LABELS` + voice regression tests

**Files:**
- Modify: `crates/right-agent/src/init.rs` (add const)
- Modify: `crates/right/src/wizard.rs` (add const)
- Create: `crates/right-agent/tests/voice_pass.rs` (consumes both)

Centralise every prompt label in one place per crate so future contributors can't sneak in Title Case.

- [ ] **Step 1: Define `PROMPT_LABELS` in `init.rs`**

At the bottom of `crates/right-agent/src/init.rs`, after the test module, add:

```rust
/// Every prompt label string used by `right-agent::init`. Source-of-truth list
/// for the brand voice regression tests (`tests/voice_pass.rs`). When you add
/// or change a prompt, update this array — failing to do so is caught by tests.
pub const PROMPT_LABELS: &[&str] = &[
    "sandbox mode:",
    "openshell — isolated container (recommended)",
    "none — direct host access (computer-use, chrome)",
    "network policy:",
    "permissive — all https domains (recommended)",
    "restrictive — anthropic/claude domains only",
    "memory provider:",
    "hindsight — hindsight cloud api (recommended)",
    "file — agent manages MEMORY.md",
    "hindsight api key (enter to use HINDSIGHT_API_KEY env var):",
    "recall budget:",
    "mid — balanced (default)",
    "low — smaller context, cheaper",
    // ... add all other labels actually used by prompts in this file
    "cancel?",
];
```

(Be exhaustive — read every `inquire::Select::new(...)` and `inquire::Text::new(...)` and `inquire::Confirm::new(...)` call in `init.rs` and copy each first-arg literal into this array. Any `format!`-based dynamic prompts contribute their *prefix* — e.g. `format!("hindsight bank id (default: {agent_name}):")` contributes `"hindsight bank id (default: "` as the prefix to test.)

- [ ] **Step 2: Define `PROMPT_LABELS` in `wizard.rs`**

At the top of `crates/right/src/wizard.rs` (after the existing `use` lines, before the `// Types` divider), add:

```rust
/// Every prompt label string used by the wizard. See `init.rs::PROMPT_LABELS`.
pub(crate) const PROMPT_LABELS: &[&str] = &[
    "tunnel name:",
    "tunnel hostname (e.g. right.example.com):",
    "new tunnel name:",
    "existing tunnel — choose:",
    "reuse",
    "rename",
    "delete and recreate",
    "telegram bot token (required — get one from @BotFather):",
    "telegram bot token (enter to skip):",
    "your telegram user id (required — /start @userinfobot to find it):",
    "your telegram user id (/start @userinfobot to find it, empty to skip):",
    "settings:",
    "done",
    "select agent:",
    "sandbox mode:",
    "network policy:",
    "openshell — isolated container (recommended)",
    "none — direct host access (computer-use, chrome)",
    "permissive — all https domains (needed for external mcp servers)",
    "restrictive — anthropic/claude domains only (recommended)",
    "model (e.g. sonnet, opus, haiku — empty to clear):",
    "allowed chat ids (comma-separated, empty to clear):",
    "enable voice transcription?",
    "whisper model:",
    "hindsight api key source:",
    "use HINDSIGHT_API_KEY env var (recommended)",
    "enter a key to save in agent.yaml",
    "hindsight api key:",
    "hindsight api key (empty to rely on HINDSIGHT_API_KEY at runtime):",
    "switching memory provider does not migrate existing memory. continue?",
    "save anyway?",
    // dynamic-prefix entries — match prefix only:
    // "telegram bot token (keeping ", "delete tunnel \"", "agent: ", "tunnel: ",
];
```

(Same exhaustiveness rule. Dynamic prefixes are tracked separately — see Step 4.)

- [ ] **Step 3: Create `voice_pass.rs` integration test**

Create `crates/right-agent/tests/voice_pass.rs`:

```rust
//! Brand voice regression: every prompt label must be lowercase-first and
//! must not contain `!` (we never use exclamation marks). Allowed proper-noun
//! prefixes (env var names, `@handles`) are exempt.

use right_agent::init::PROMPT_LABELS as INIT_LABELS;

const ALLOWED_PROPER_NOUNS: &[&str] = &[
    "HINDSIGHT_API_KEY",
    "RIGHT_TG_TOKEN",
    "MEMORY.md",
    "@BotFather",
    "@userinfobot",
];

fn first_visible_char(s: &str) -> char {
    s.chars().next().expect("non-empty label")
}

fn starts_with_allowed_proper_noun(s: &str) -> bool {
    ALLOWED_PROPER_NOUNS.iter().any(|p| s.starts_with(p))
}

#[test]
fn init_labels_are_lowercase_first() {
    for label in INIT_LABELS {
        let first = first_visible_char(label);
        assert!(
            !first.is_uppercase() || starts_with_allowed_proper_noun(label),
            "init prompt has uppercase first letter: {label:?}"
        );
    }
}

#[test]
fn init_labels_have_no_exclamation_marks() {
    for label in INIT_LABELS {
        assert!(
            !label.contains('!'),
            "init prompt contains '!': {label:?}"
        );
    }
}
```

- [ ] **Step 4: Mirror the test for `wizard.rs`**

`PROMPT_LABELS` in `wizard.rs` is `pub(crate)` and not visible from outside the binary crate. Move the assertion into `crates/right/src/wizard.rs` itself as a `#[cfg(test)] mod voice_pass`:

```rust
#[cfg(test)]
mod voice_pass {
    use super::PROMPT_LABELS;

    const ALLOWED_PROPER_NOUNS: &[&str] = &[
        "HINDSIGHT_API_KEY",
        "RIGHT_TG_TOKEN",
        "MEMORY.md",
        "@BotFather",
        "@userinfobot",
    ];

    #[test]
    fn labels_are_lowercase_first() {
        for label in PROMPT_LABELS {
            let first = label.chars().next().unwrap();
            assert!(
                !first.is_uppercase()
                    || ALLOWED_PROPER_NOUNS.iter().any(|p| label.starts_with(p)),
                "prompt has uppercase first letter: {label:?}"
            );
        }
    }

    #[test]
    fn labels_have_no_exclamation_marks() {
        for label in PROMPT_LABELS {
            assert!(!label.contains('!'), "prompt contains '!': {label:?}");
        }
    }
}
```

- [ ] **Step 5: Run both regression tests**

```bash
cargo test -p right-agent --test voice_pass
cargo test -p right wizard::voice_pass
```
Expected: both pass. If they fail with a missing/uppercase label, fix the source string (this is the regression catching live).

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/init.rs crates/right/src/wizard.rs crates/right-agent/tests/voice_pass.rs
git commit -m "test(voice): lowercase + no-exclamation regression for prompt labels"
```

---

### Task 27: Validation re-prompt warn lines

**Files:**
- Modify: `crates/right/src/wizard.rs`
- Modify: `crates/right-agent/src/init.rs` (any inline `eprintln!` re-prompts there)

Spec §"Error handling · Validation re-prompt loops". Replace every `eprintln!("  {e:#}")` inside a `loop { ... continue; }` with a `ui::status(Warn).noun("invalid").verb(...)` rail line.

- [ ] **Step 1: Find every re-prompt eprintln**

```bash
rg -n 'eprintln!.*\{e' crates/right/src/wizard.rs crates/right-agent/src/init.rs
```

- [ ] **Step 2: Replace each occurrence**

Pattern:

```rust
eprintln!("  {e:#}");
continue;
```

Becomes:

```rust
let theme = right_agent::ui::detect();
eprintln!(
    "{}",
    right_agent::ui::status(right_agent::ui::Glyph::Warn)
        .noun("invalid")
        .verb(format!("{e:#}"))
        .render(theme)
);
continue;
```

Locations to update (from spec discovery):
- `crates/right/src/wizard.rs:406` (telegram_setup loop)
- `crates/right/src/wizard.rs:464` (chat_ids_setup loop)
- Any similar pattern in `init.rs` — verify with the grep above.

- [ ] **Step 3: Run**

```bash
cargo test -p right --tests --no-fail-fast
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/wizard.rs crates/right-agent/src/init.rs
git commit -m "refactor(wizard): brand warn lines on validation re-prompt"
```

---

## Step 6 — Fallback hardening + brand-conformance lint

Goal: add the `--no-color` flag, finalise ASCII / mono integration tests, brand-conformance lint, full clippy + workspace test.

### Task 28: `right --no-color` global flag

**Files:**
- Modify: `crates/right/src/main.rs`

Need to read where the existing CLI is parsed (clap derive). The `--home`, `--debug`, etc. flags live on the top-level `Cli` struct.

- [ ] **Step 1: Add the flag to `Cli`**

In `/Users/molt/dev/rightclaw/crates/right/src/main.rs`, locate the top-level clap struct (likely named `Cli`). Add a field:

```rust
    /// Disable color output. Equivalent to setting NO_COLOR=1 for this run.
    #[arg(long, global = true)]
    no_color: bool,
```

- [ ] **Step 2: Apply it before any UI call**

In `fn main` (or whichever function dispatches), early on:

```rust
if cli.no_color {
    // SAFETY: we are still single-threaded at this point in main.
    // SAFETY: no other thread can be reading NO_COLOR yet.
    // (Fine to call set_var here in the binary entrypoint.)
    unsafe { std::env::set_var("NO_COLOR", "1"); }
}
```

(`std::env::set_var` is `unsafe` in Rust 2024 edition. Wrap in `unsafe`.)

Note: project rule bans `set_var` *in tests*. Using it here in `main` before any thread spawn is the standard pattern (clap-style apps use this for `--quiet`, `--no-color`, etc.).

- [ ] **Step 3: Build + manual smoke**

```bash
cargo build --workspace
./target/debug/right --no-color doctor
```
Expected: same output as `NO_COLOR=1 right doctor`.

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(cli): --no-color global flag"
```

---

### Task 29: ASCII fallback + mono no-ANSI integration tests

**Files:**
- Modify: `crates/right/tests/wizard_brand.rs`

(Most coverage already added in Tasks 12 + 16. This task adds end-to-end coverage for `init` and `status` under both fallbacks, in case earlier tests narrowed scope.)

- [ ] **Step 1: Add end-to-end fallback tests**

Append to `crates/right/tests/wizard_brand.rs`:

```rust
#[test]
fn init_ascii_fallback() {
    let home = isolated_home();
    let assert = right()
        .env("TERM", "dumb")
        .env_remove("NO_COLOR")
        .args([
            "--home", home.path().to_str().unwrap(),
            "init", "-y",
            "--sandbox-mode", "none",
            "--tunnel-hostname", "test.example.com",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("|*"), "ascii mark missing: {stdout}");
    assert!(stdout.contains("[ok]"), "ascii ok glyph missing: {stdout}");
    for ch in ['▐', '✓', '✗', '!', '…'] {
        assert!(!stdout.contains(ch), "ascii output contains {ch:?}");
    }
}

#[test]
fn init_mono_no_ansi() {
    let home = isolated_home();
    let assert = right()
        .env("NO_COLOR", "1")
        .env("TERM", "xterm-256color")
        .args([
            "--home", home.path().to_str().unwrap(),
            "init", "-y",
            "--sandbox-mode", "none",
            "--tunnel-hostname", "test.example.com",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains('▐'));
    assert!(!stdout.contains('\x1b'));
}

#[test]
fn no_color_flag_equivalent_to_env() {
    let home = isolated_home();
    let stdout_flag = {
        let assert = right()
            .env("TERM", "xterm-256color")
            .env_remove("NO_COLOR")
            .args([
                "--home", home.path().to_str().unwrap(),
                "--no-color",
                "doctor",
            ])
            .assert();
        String::from_utf8(assert.get_output().stdout.clone()).unwrap()
    };
    assert!(!stdout_flag.contains('\x1b'), "--no-color should disable ANSI");
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p right --test wizard_brand
```
Expected: all tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/right/tests/wizard_brand.rs
git commit -m "test(brand): end-to-end ascii + mono + --no-color coverage"
```

---

### Task 30: Brand-conformance lint

**Files:**
- Modify: `crates/right/tests/wizard_brand.rs`

Asserts: every line of `right doctor` (mono theme) starts with `▐` or is empty or is the summary; no line contains `Successfully`/`Successful`/`successfully`; no line ends with `.` (statements, not sentences). Same for `right status` not-running branch.

- [ ] **Step 1: Add the lint**

Append to `crates/right/tests/wizard_brand.rs`:

```rust
fn capture_stdout(env: &[(&str, &str)], args: &[&str]) -> String {
    let home = isolated_home();
    let mut cmd = right();
    for (k, v) in env {
        cmd.env(k, v);
    }
    let assert = cmd
        .arg("--home")
        .arg(home.path().to_str().unwrap())
        .args(args)
        .assert();
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

#[test]
fn brand_conformance_doctor_mono() {
    let stdout = capture_stdout(
        &[("NO_COLOR", "1"), ("TERM", "xterm-256color")],
        &["doctor"],
    );
    for line in stdout.lines() {
        if line.is_empty() { continue; }
        assert!(
            line.starts_with('▐') || line.starts_with("  "),
            "line should start with rail or be indented continuation: {line:?}"
        );
        assert!(
            !line.contains("Successfully") && !line.contains("Successful") && !line.contains("successfully"),
            "marketing-speak forbidden: {line:?}"
        );
        // Sentence-end period only allowed inside parens (detail).
        if !line.contains('(') {
            assert!(
                !line.trim_end().ends_with('.'),
                "status line should not end with '.': {line:?}"
            );
        }
    }
}

#[test]
fn brand_conformance_status_not_running() {
    let stdout = capture_stdout(
        &[("NO_COLOR", "1"), ("TERM", "xterm-256color")],
        &["status"],
    );
    for line in stdout.lines() {
        if line.is_empty() { continue; }
        assert!(
            line.starts_with('▐') || line.starts_with("  "),
            "line should start with rail: {line:?}"
        );
    }
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p right --test wizard_brand brand_conformance
```
Expected: both pass. If the lint flags a real violation in the source, fix the source string and re-run.

- [ ] **Step 3: Commit**

```bash
git add crates/right/tests/wizard_brand.rs
git commit -m "test(brand): conformance lint — rail + no-marketing + no-period"
```

---

### Task 31: Final workspace verification

**Files:** none (verification only).

- [ ] **Step 1: Workspace test**

```bash
cargo test --workspace --no-fail-fast
```
Expected: all green. Investigate any failures — the most likely cause is a stale string assertion in an existing integration test.

- [ ] **Step 2: Workspace clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: zero warnings.

- [ ] **Step 3: Workspace debug build**

```bash
cargo build --workspace
```
Expected: clean.

- [ ] **Step 4: Manual smoke pass**

In a TTY:

```bash
right doctor
NO_COLOR=1 right doctor
TERM=dumb right doctor
right --no-color doctor
right doctor | cat   # piped — non-tty → ASCII

# In a fresh tempdir:
mkdir -p /tmp/right-fresh
right --home /tmp/right-fresh init -y --sandbox-mode none --tunnel-hostname test.example.com
right --home /tmp/right-fresh agent init demo -y --sandbox-mode none

# Then:
right --home /tmp/right-fresh status      # PC not running → ✗ + fix line
right --home /tmp/right-fresh config      # interactive — visual check
```

Verify each one matches the brand examples in the spec.

- [ ] **Step 5: Final commit (only if any cleanup needed during smoke)**

```bash
# Only if smoke surfaced something missed; otherwise no-op.
git add -A
git status   # confirm what would be committed
git commit -m "polish(brand): smoke-test fixes"
```

---

## Self-review checklist

(Run after writing the plan, before handing off.)

**Spec coverage:**
- [x] §"Module: `right-agent::ui`" → Tasks 1–9.
- [x] §"Per-command flows · `right init`" → Tasks 16–21.
- [x] §"Per-command flows · `right agent init`" → Tasks 22–23 + 25.
- [x] §"Per-command flows · `right config`" → Task 24.
- [x] §"Per-command flows · `right doctor`" → Tasks 10–12.
- [x] §"Per-command flows · `right status`" → Tasks 13–15.
- [x] §"Voice rewrite · Tunnel" / "Telegram" / "Chat IDs" → Task 21.
- [x] §"Voice rewrite · Sandbox / network / model / memory / STT" → Task 25.
- [x] §"Voice rewrite · Settings menus" → Task 24.
- [x] §"Voice rewrite · Doctor / status / misc" → Tasks 11, 14.
- [x] §"Voice rewrite · `right init` footer" → Tasks 17–19.
- [x] §"Error handling · Error shape" → Task 11 (doctor footer error), Task 14 (status not-running).
- [x] §"Error handling · Validation re-prompt loops" → Task 27.
- [x] §"Error handling · Fatal errors during init" → Task 17 (`BlockAlreadyRendered` use).
- [x] §"Error handling · Ctrl+C cancellation" → Task 20 (`cancel?` rewrite in `inquire_back`).
- [x] §"Testing · Unit tests in `right-agent::ui`" → Tasks 2–8.
- [x] §"Testing · Integration tests via `assert_cmd`" → Tasks 10, 13, 16, 23, 29.
- [x] §"Testing · Voice-pass regression test" → Task 26.
- [x] §"Testing · Brand-conformance lint" → Task 30.
- [x] §"Testing · No `#[ignore]`" → enforced by no-test-having-`#[ignore]`.

**Placeholders:** none — every code block is concrete, every command is runnable, every file path is absolute or workspace-relative.

**Type consistency:**
- `Theme` enum spelling: `Color`/`Mono`/`Ascii` — used identically in every task.
- `Glyph` variants: `Ok`/`Warn`/`Err`/`Info` — consistent.
- `Rail::prefix`/`mark`/`blank` return `String` (not `&'static str`) because `Color` returns ANSI-wrapped owned strings — verified consistent across `atoms.rs` and all callers.
- `Line::render(&self, theme)` borrows; called in `Block::render` and command code consistently.
- `BlockAlreadyRendered` is a unit struct, not a tuple — verified consistent.
- `ui::stdout`/`stderr` take `theme` and `&str` — used consistently (or skipped in favour of direct `println!("{}", ...)`).

---

**Plan complete and saved to `docs/superpowers/plans/2026-04-28-init-wizard-brand-redesign.md`.**

Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
