use super::snap_to_char_boundary;
use super::split_html_message;

#[test]
fn short_message_no_split() {
    let parts = split_html_message("hello");
    assert_eq!(parts, vec!["hello"]);
}

#[test]
fn long_message_splits_at_newline() {
    let line = "a".repeat(100);
    let msg: String = (0..50)
        .map(|_| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(msg.len() > 4096);
    let parts = split_html_message(&msg);
    assert!(parts.len() >= 2);
    for part in &parts {
        assert!(part.len() <= 4096, "part too long: {} chars", part.len());
    }
}

#[test]
fn split_closes_open_bold_tag() {
    let inner = "a".repeat(4090);
    let msg = format!("<b>{inner}</b>");
    let parts = split_html_message(&msg);
    assert!(parts.len() >= 2);
    assert!(
        parts[0].ends_with("</b>"),
        "first part end: ...{}",
        &parts[0][parts[0].len().saturating_sub(20)..]
    );
    assert!(
        parts[1].starts_with("<b>"),
        "second part start: {}",
        &parts[1][..20.min(parts[1].len())]
    );
}

#[test]
fn split_preserves_pre_block_under_limit() {
    let code = "x\n".repeat(100);
    let msg = format!("text before\n<pre>{code}</pre>\ntext after");
    assert!(msg.len() < 4096);
    let parts = split_html_message(&msg);
    assert_eq!(parts.len(), 1);
}

#[test]
fn split_handles_pre_block_over_limit() {
    let code = "x".repeat(5000);
    let msg = format!("<pre>{code}</pre>");
    let parts = split_html_message(&msg);
    assert!(parts.len() >= 2);
    assert!(parts[0].contains("<pre>"), "first part missing <pre>");
    assert!(
        parts[0].ends_with("</pre>"),
        "first part must close pre: ...{}",
        &parts[0][parts[0].len().saturating_sub(20)..]
    );
    assert!(parts[1].starts_with("<pre>"), "second part must reopen pre");
}

#[test]
fn split_pre_block_parts_stay_within_telegram_limit() {
    let code = "x".repeat(5000);
    let msg = format!("<pre>{code}</pre>");
    let parts = split_html_message(&msg);

    assert!(parts.len() >= 2);
    for part in &parts {
        assert!(part.len() <= 4096, "part too long: {} chars", part.len());
    }
}

#[test]
fn split_does_not_exceed_limit_with_closing_tags() {
    // Even after appending closing tags, each part must stay under Telegram's limit.
    let inner = "a".repeat(4080);
    let msg = format!("<b><i><code>{inner}</code></i></b>");
    let parts = split_html_message(&msg);
    assert!(parts.len() >= 2);
    for part in &parts {
        assert!(part.len() <= 4096, "part too long: {} chars", part.len());
    }
}

#[test]
fn split_html_message_multibyte_no_panic() {
    // Regression: the split window's byte offset can land inside a multibyte
    // character; every probe must snap down before slicing.
    let target = 3696; // 4096 - SPLIT_WINDOW_BACKOFF
    let prefix = "x".repeat(target - 1);
    let mid = "—"; // 3 bytes; byte `target` is inside it
    let suffix = "y".repeat(4096 - (target - 1) - mid.len() + 500);
    let msg = format!("{prefix}{mid}{suffix}");
    assert!(msg.len() > 4096);
    assert!(
        !msg.is_char_boundary(target),
        "byte {target} should be inside the em-dash"
    );
    let parts = split_html_message(&msg);
    assert!(parts.len() >= 2);
    let joined: String = parts.join("");
    assert_eq!(joined, msg);
}

#[test]
fn split_html_message_ignores_non_telegram_tags() {
    let inner = "a".repeat(4090);
    let msg = format!("<custom>{inner}</custom>");
    let parts = split_html_message(&msg);
    assert!(parts.len() >= 2);
    assert_eq!(parts.join(""), msg);
    assert!(
        !parts[0].contains("</custom>"),
        "non-Telegram tags must not be closed across chunks: {parts:?}"
    );
}

#[test]
fn snap_to_char_boundary_basic() {
    let s = "abc—def"; // '—' occupies bytes 3..6
    assert_eq!(snap_to_char_boundary(s, 3), 3); // exact boundary
    assert_eq!(snap_to_char_boundary(s, 4), 3); // inside '—'
    assert_eq!(snap_to_char_boundary(s, 5), 3); // inside '—'
    assert_eq!(snap_to_char_boundary(s, 6), 6); // after '—'
    assert_eq!(snap_to_char_boundary(s, 100), s.len()); // beyond end
}
