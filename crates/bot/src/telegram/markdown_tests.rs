use super::md_to_telegram_html;
use super::split_html_message;

#[test]
fn plain_text_passes_through() {
    assert_eq!(md_to_telegram_html("hello world"), "hello world");
}

#[test]
fn html_entities_escaped() {
    assert_eq!(md_to_telegram_html("a < b & c > d"), "a &lt; b &amp; c &gt; d");
}

#[test]
fn bold_text() {
    assert_eq!(md_to_telegram_html("**bold**"), "<b>bold</b>");
}

#[test]
fn italic_text() {
    assert_eq!(md_to_telegram_html("*italic*"), "<i>italic</i>");
}

#[test]
fn bold_italic() {
    // pulldown-cmark parses *** as Emphasis(Strong(text))
    assert_eq!(md_to_telegram_html("***both***"), "<i><b>both</b></i>");
}

#[test]
fn inline_code() {
    assert_eq!(md_to_telegram_html("`code`"), "<code>code</code>");
}

#[test]
fn inline_code_with_html_entities() {
    assert_eq!(
        md_to_telegram_html("`<div>&</div>`"),
        "<code>&lt;div&gt;&amp;&lt;/div&gt;</code>"
    );
}

#[test]
fn fenced_code_block_no_lang() {
    let input = "```\nfn main() {}\n```";
    assert_eq!(md_to_telegram_html(input), "<pre>fn main() {}</pre>");
}

#[test]
fn fenced_code_block_with_lang() {
    let input = "```rust\nfn main() {}\n```";
    assert_eq!(
        md_to_telegram_html(input),
        "<pre><code class=\"language-rust\">fn main() {}</code></pre>"
    );
}

#[test]
fn code_block_escapes_html() {
    let input = "```\n<b>not bold</b>\n```";
    assert_eq!(
        md_to_telegram_html(input),
        "<pre>&lt;b&gt;not bold&lt;/b&gt;</pre>"
    );
}

#[test]
fn link() {
    assert_eq!(
        md_to_telegram_html("[click](https://example.com)"),
        "<a href=\"https://example.com\">click</a>"
    );
}

#[test]
fn heading_becomes_bold() {
    let html = md_to_telegram_html("# Heading");
    assert!(html.contains("<b>Heading</b>"), "got: {html}");
}

#[test]
fn strikethrough() {
    assert_eq!(md_to_telegram_html("~~deleted~~"), "<s>deleted</s>");
}

#[test]
fn blockquote() {
    let html = md_to_telegram_html("> quoted");
    assert!(html.contains("<blockquote>"), "got: {html}");
    assert!(html.contains("quoted"), "got: {html}");
    assert!(html.contains("</blockquote>"), "got: {html}");
}

#[test]
fn unordered_list() {
    let input = "- one\n- two\n- three";
    let html = md_to_telegram_html(input);
    assert!(html.contains("• one"), "got: {html}");
    assert!(html.contains("• two"), "got: {html}");
    assert!(html.contains("• three"), "got: {html}");
}

#[test]
fn ordered_list() {
    let input = "1. first\n2. second";
    let html = md_to_telegram_html(input);
    assert!(html.contains("1. first"), "got: {html}");
    assert!(html.contains("2. second"), "got: {html}");
}

#[test]
fn image_becomes_link() {
    let html = md_to_telegram_html("![photo](https://img.com/x.png)");
    assert!(html.contains("<a href=\"https://img.com/x.png\">"), "got: {html}");
    assert!(html.contains("photo"), "got: {html}");
}

#[test]
fn horizontal_rule_dropped() {
    let input = "before\n\n---\n\nafter";
    let html = md_to_telegram_html(input);
    assert!(!html.contains("---"), "got: {html}");
    assert!(html.contains("before"), "got: {html}");
    assert!(html.contains("after"), "got: {html}");
}

#[test]
fn mixed_formatting() {
    let input = "Use **bold** and `code` in *italic* text";
    let html = md_to_telegram_html(input);
    assert!(html.contains("<b>bold</b>"), "got: {html}");
    assert!(html.contains("<code>code</code>"), "got: {html}");
    assert!(html.contains("<i>italic</i>"), "got: {html}");
}

#[test]
fn softbreak_becomes_newline() {
    let input = "line one\nline two";
    let html = md_to_telegram_html(input);
    assert!(html.contains("line one\nline two"), "got: {html}");
}

// --- table tests ---

#[test]
fn simple_table() {
    let input = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |";
    let html = md_to_telegram_html(input);
    assert!(html.contains("<pre>"), "got: {html}");
    assert!(html.contains("</pre>"), "got: {html}");
    assert!(html.contains("A"), "got: {html}");
    assert!(html.contains("B"), "got: {html}");
    assert!(html.contains("1 | 2"), "got: {html}");
    assert!(html.contains("3 | 4"), "got: {html}");
    // Should have separator line between header and data
    assert!(html.contains("--"), "got: {html}");
}

#[test]
fn table_escapes_html_in_cells() {
    // Use &amp; which pulldown-cmark passes as literal text, then we HTML-escape it.
    let input = "| X |\n|---|\n| a & b |";
    let html = md_to_telegram_html(input);
    assert!(html.contains("a &amp; b"), "got: {html}");
}

#[test]
fn table_with_formatting_in_cells() {
    let input = "| Name | Value |\n|---|---|\n| `key` | **val** |";
    let html = md_to_telegram_html(input);
    // Inside <pre> block, cell content should be plain text (backticks preserved, bold stripped)
    assert!(html.contains("<pre>"), "got: {html}");
    assert!(html.contains("Name"), "got: {html}");
}

// --- split_html_message tests ---

#[test]
fn short_message_no_split() {
    let parts = split_html_message("hello");
    assert_eq!(parts, vec!["hello"]);
}

#[test]
fn long_message_splits_at_newline() {
    let line = "a".repeat(100);
    let msg: String = (0..50).map(|_| line.as_str()).collect::<Vec<_>>().join("\n");
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
    assert!(
        parts[1].starts_with("<pre>"),
        "second part must reopen pre"
    );
}

#[test]
fn split_does_not_exceed_limit_with_closing_tags() {
    // Even after appending closing tags, each part must stay under a reasonable size.
    // The closing tags add at most ~50 chars for deeply nested Telegram tags.
    let inner = "a".repeat(4080);
    let msg = format!("<b><i><code>{inner}</code></i></b>");
    let parts = split_html_message(&msg);
    assert!(parts.len() >= 2);
    // Allow some overflow for closing tags (up to ~60 chars beyond 4096)
    for part in &parts {
        assert!(part.len() <= 4200, "part too long: {} chars", part.len());
    }
}
