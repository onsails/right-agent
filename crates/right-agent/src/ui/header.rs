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
