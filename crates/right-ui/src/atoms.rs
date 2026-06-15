//! Brand atoms — rail (`▐`), mark (`▐✓`), and semantic glyphs (`✓ ! ✗ …`).
//!
//! Color values come from the brand guide. Three render tiers:
//! * `Color`: ruby rail + colored Unicode glyphs via owo-colors truecolor
//! * `Mono`: same glyphs without ANSI
//! * `Ascii`: `|` rail + bracketed text (`[ok]/[warn]/[err]/[…]`)

use owo_colors::OwoColorize;

use crate::theme::Theme;

pub(crate) const RUBY: (u8, u8, u8) = (0xC7, 0x5F, 0x88);
pub(crate) const MUTED: (u8, u8, u8) = (0xB6, 0xA8, 0xB0);
pub(crate) const TEAL: (u8, u8, u8) = (0x3B, 0xB0, 0xC4);
const OK: (u8, u8, u8) = (0x6B, 0xBF, 0x59);
const WARN: (u8, u8, u8) = (0xE6, 0xC0, 0x6A);
const ERR: (u8, u8, u8) = (0xE2, 0x55, 0x6A);
const INFO: (u8, u8, u8) = (0x3B, 0xB0, 0xC4);

pub struct Rail;

impl Rail {
    /// `"▐  "` (Color/Mono) or `"|  "` (Ascii). Always 4 visible cells.
    pub fn prefix(theme: Theme) -> String {
        match theme {
            Theme::Color => format!("{}  ", "▐".truecolor(RUBY.0, RUBY.1, RUBY.2)),
            Theme::Mono => "▐  ".to_string(),
            Theme::Ascii => "|  ".to_string(),
        }
    }

    /// `"▐✓"` (Color/Mono) or `"|*"` (Ascii). 2 visible cells.
    pub fn mark(theme: Theme) -> String {
        match theme {
            Theme::Color => format!("{}", "▐✓".truecolor(RUBY.0, RUBY.1, RUBY.2)),
            Theme::Mono => "▐✓".to_string(),
            Theme::Ascii => "|*".to_string(),
        }
    }

    /// `"▐"` (Color/Mono) or `"|"` (Ascii). For blank rail rows.
    pub fn blank(theme: Theme) -> String {
        match theme {
            Theme::Color => format!("{}", "▐".truecolor(RUBY.0, RUBY.1, RUBY.2)),
            Theme::Mono => "▐".to_string(),
            Theme::Ascii => "|".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Ok,
    Warn,
    Err,
    Info,
}

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
