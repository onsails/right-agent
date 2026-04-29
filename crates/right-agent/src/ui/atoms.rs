//! Brand atoms — rail (`▐`), mark (`▐✓`), and semantic glyphs (`✓ ! ✗ …`).
//!
//! Color values come from the brand guide. Three render tiers:
//! * `Color`: orange rail + colored Unicode glyphs via owo-colors truecolor
//! * `Mono`: same glyphs without ANSI
//! * `Ascii`: `|` rail + bracketed text (`[ok]/[warn]/[err]/[…]`)

use owo_colors::OwoColorize;

use crate::ui::theme::Theme;

pub(crate) const ORANGE: (u8, u8, u8) = (0xE8, 0x63, 0x2A);
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
