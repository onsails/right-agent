//! Full splash header — `▐✓ right agent vX.Y.Z` + tagline + blank rail.

use owo_colors::OwoColorize;

use crate::atoms::{MUTED, RUBY, Rail};
use crate::theme::Theme;

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

/// Three-line splash: `▐✓ right agent v<version>` / `▐  <tagline>` / `▐`.
/// No trailing newline after the third line. Reserved for `right init`.
pub fn splash(theme: Theme, version: &str, tagline: &str) -> String {
    let mut out = String::new();
    // Line 1: ▐✓ right agent v0.10.2
    out.push_str(&Rail::mark(theme));
    out.push(' ');
    out.push_str(&wordmark(theme));
    out.push_str(" v");
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
