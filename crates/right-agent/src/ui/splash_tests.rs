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
