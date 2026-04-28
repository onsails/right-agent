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
    // "right" (5) padded to 11 + 2-space gap = 8 spaces before verb
    assert_eq!(lines[0], "▐  ✓ right        in PATH");
    // "cloudflared" (11) = max, no padding + 2-space gap
    assert_eq!(lines[1], "▐  ! cloudflared  not in PATH");
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
    // "a" (1) padded to 6 + 2-space gap = 7 spaces before verb
    assert_eq!(lines[0], "|  [ok] a       x");
    // "longer" (6) = max, no padding + 2-space gap
    assert_eq!(lines[1], "|  [ok] longer  y");
}
