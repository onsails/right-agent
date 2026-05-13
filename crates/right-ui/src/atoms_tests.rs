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
            Rail::prefix(theme),
            Rail::mark(theme),
            Rail::blank(theme),
            Glyph::Ok.render(theme),
            Glyph::Warn.render(theme),
            Glyph::Err.render(theme),
            Glyph::Info.render(theme),
        ] {
            assert!(!s.contains(ESC), "theme {theme:?} string {s:?} contains ANSI escape");
        }
    }
}
