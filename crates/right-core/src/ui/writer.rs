//! Theme-aware writers. Optional sugar — direct `println!("{}", line.render(theme))`
//! is equivalent.
//!
//! These exist so that future hardening (e.g. piping all UI through a `Sink`
//! trait for capture in tests) has one chokepoint.

use crate::ui::theme::Theme;

/// Write a line to stdout. `theme` is currently unused but kept in the
/// signature so future captures don't need a callsite shape change.
pub fn stdout(_theme: Theme, s: &str) {
    println!("{s}");
}

/// Write a line to stderr. Same rationale as [`stdout`].
pub fn stderr(_theme: Theme, s: &str) {
    eprintln!("{s}");
}
