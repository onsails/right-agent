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
