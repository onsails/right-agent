use super::*;

#[test]
fn recap_minimal_mono() {
    let s = Recap::new("ready")
        .ok("tunnel", "right.example.com")
        .next("right up")
        .render(Theme::Mono);
    // section("ready"): "▐"(1) + " "(1) + "ready"(5) + " "(1) = 8 used; dashes = 48-8 = 40.
    let expected = "▐ ready ────────────────────────────────────────\n▐\n▐  ✓ tunnel  right.example.com\n▐\n▐  next: right up";
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
    // Three status lines noun-aligned. max(noun.len()) across {"agent" (5), "tunnel" (6), "memory" (6)} = 6.
    // After padding, each noun occupies 6 cells, then "  " gap, then verb.
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

#[test]
fn recap_init_pc_not_running() {
    let s = Recap::new("ready")
        .ok("agent", "test created")
        .next("right up")
        .render(Theme::Mono);
    assert!(s.contains("✓ agent"));
    assert!(s.contains("next: right up"));
    assert!(!s.contains("send /start"));
    assert!(!s.contains("⚠ reload"));
    assert!(!s.contains("! reload"));
}

#[test]
fn recap_init_pc_running_ok() {
    let s = Recap::new("ready")
        .ok("agent", "test created")
        .next("send /start to your bot in Telegram")
        .render(Theme::Mono);
    assert!(s.contains("✓ agent"));
    assert!(s.contains("next: send /start to your bot in Telegram"));
    assert!(!s.contains("right up"));
    assert!(!s.contains("⚠ reload"));
}

#[test]
fn recap_init_pc_reload_failed() {
    let s = Recap::new("ready")
        .ok("agent", "test created")
        .warn("reload", "failed to add to running right")
        .next("right restart")
        .render(Theme::Mono);
    assert!(s.contains("✓ agent"));
    // The warn glyph rendering is theme-dependent; check for the noun/detail pair.
    assert!(s.contains("reload"));
    assert!(s.contains("failed to add to running right"));
    assert!(s.contains("next: right restart"));
}
