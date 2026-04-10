# Telegram Markdown Formatting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert Claude Code's Markdown output to Telegram-compatible HTML so formatting (bold, code, links) renders correctly in Telegram instead of showing raw asterisks and backticks.

**Architecture:** Two-part fix — (1) update AGENTS.md template to instruct Claude to use a Telegram-friendly Markdown subset, (2) add a pulldown-cmark → Telegram HTML converter in the bot with ParseMode::Html and plain-text fallback.

**Tech Stack:** pulldown-cmark 0.13 (GFM parser), teloxide ParseMode::Html

---

### File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `crates/bot/src/telegram/markdown.rs` | Markdown→Telegram HTML converter + HTML-aware message splitter |
| Create | `crates/bot/src/telegram/markdown_tests.rs` | Tests for converter and splitter |
| Modify | `crates/bot/src/telegram/mod.rs:1-8` | Add `pub mod markdown;` |
| Modify | `crates/bot/src/telegram/worker.rs:440-465` | Apply conversion, set ParseMode::Html, fallback |
| Modify | `crates/bot/src/telegram/worker.rs:89-115` | Replace `split_message` with HTML-aware version from markdown module |
| Modify | `crates/bot/Cargo.toml:6-31` | Add pulldown-cmark dependency |
| Modify | `Cargo.toml:10-51` | Add pulldown-cmark to workspace dependencies |
| Modify | `templates/right/AGENTS.md:42-46` | Replace vague formatting guidance with explicit Telegram Markdown rules |

---

### Task 1: Add pulldown-cmark dependency

**Files:**
- Modify: `Cargo.toml:10-51` (workspace deps)
- Modify: `crates/bot/Cargo.toml:6-31`

- [ ] **Step 1: Add pulldown-cmark to workspace dependencies**

In `Cargo.toml`, add after the `owo-colors` line:

```toml
pulldown-cmark = { version = "0.13", default-features = false }
```

- [ ] **Step 2: Add pulldown-cmark to bot crate**

In `crates/bot/Cargo.toml`, add after the `notify-debouncer-mini` line:

```toml
pulldown-cmark = { workspace = true }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rightclaw-bot`
Expected: compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/bot/Cargo.toml
git commit -m "deps: add pulldown-cmark for Telegram markdown conversion"
```

---

### Task 2: Write Markdown→Telegram HTML converter with tests (TDD)

**Files:**
- Create: `crates/bot/src/telegram/markdown.rs`
- Create: `crates/bot/src/telegram/markdown_tests.rs`
- Modify: `crates/bot/src/telegram/mod.rs:1-8`

This is the core converter. It uses pulldown-cmark to parse GFM events and renders them as Telegram-compatible HTML.

**Telegram HTML supported tags reference:**
- `<b>bold</b>`, `<i>italic</i>`, `<u>underline</u>`, `<s>strikethrough</s>`
- `<code>inline code</code>`, `<pre>code block</pre>`, `<pre><code class="language-rust">code</code></pre>`
- `<a href="url">text</a>`
- `<blockquote>quote</blockquote>`
- All text outside tags MUST have `<`, `>`, `&` escaped as `&lt;`, `&gt;`, `&amp;`

**Unsupported Markdown features** (render as plain text):
- Tables → not supported by Telegram, render as preformatted text
- Images `![alt](url)` → render as link `<a href="url">alt</a>`
- Horizontal rules `---` → skip entirely
- Headings `# H1` → render as `<b>heading text</b>` + newline

- [ ] **Step 1: Register the module**

In `crates/bot/src/telegram/mod.rs`, add after line 1 (`pub mod attachments;`):

```rust
pub mod markdown;
```

(Keep alphabetical order — `markdown` goes between `handler` and `oauth_callback`.)

- [ ] **Step 2: Write failing tests for the converter**

Create `crates/bot/src/telegram/markdown_tests.rs`:

```rust
use super::markdown::md_to_telegram_html;

#[test]
fn plain_text_passes_through_escaped() {
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
    assert_eq!(md_to_telegram_html("***both***"), "<b><i>both</i></b>");
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
fn link_with_entities_in_text() {
    assert_eq!(
        md_to_telegram_html("[a < b](https://example.com)"),
        "<a href=\"https://example.com\">a &lt; b</a>"
    );
}

#[test]
fn heading_becomes_bold() {
    assert_eq!(md_to_telegram_html("# Heading"), "<b>Heading</b>\n");
}

#[test]
fn h2_becomes_bold() {
    assert_eq!(md_to_telegram_html("## Sub"), "<b>Sub</b>\n");
}

#[test]
fn strikethrough() {
    assert_eq!(md_to_telegram_html("~~deleted~~"), "<s>deleted</s>");
}

#[test]
fn blockquote() {
    assert_eq!(md_to_telegram_html("> quoted"), "<blockquote>quoted</blockquote>");
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
    assert_eq!(
        md_to_telegram_html("![photo](https://img.com/x.png)"),
        "<a href=\"https://img.com/x.png\">photo</a>"
    );
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
fn paragraph_separation() {
    let input = "first paragraph\n\nsecond paragraph";
    let html = md_to_telegram_html(input);
    assert!(html.contains("first paragraph\n\nsecond paragraph"), "got: {html}");
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
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot markdown_tests -- --nocapture 2>&1 | head -50`
Expected: compilation error (module doesn't exist yet)

- [ ] **Step 4: Write the converter implementation**

Create `crates/bot/src/telegram/markdown.rs`:

```rust
//! Markdown → Telegram HTML converter.
//!
//! Uses pulldown-cmark to parse GitHub-Flavored Markdown and renders
//! the subset of HTML that Telegram's Bot API supports.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, CodeBlockKind};

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;

/// Convert GFM Markdown to Telegram-compatible HTML.
///
/// Supported output tags: `<b>`, `<i>`, `<s>`, `<u>`, `<code>`, `<pre>`,
/// `<a href="...">`, `<blockquote>`.
///
/// Unsupported elements (tables, images, HRs) degrade gracefully to
/// plain text or links.
pub fn md_to_telegram_html(md: &str) -> String {
    let opts = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(md, opts);

    let mut out = String::with_capacity(md.len());
    let mut list_stack: Vec<ListKind> = Vec::new();
    let mut code_block_lang: Option<String> = None;

    for event in parser {
        match event {
            // ── Inline text ──────────────────────────────────────────
            Event::Text(text) => {
                html_escape_into(&text, &mut out);
            }
            Event::Code(code) => {
                out.push_str("<code>");
                html_escape_into(&code, &mut out);
                out.push_str("</code>");
            }
            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push('\n'),

            // ── Block/inline tags ────────────────────────────────────
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { .. } => out.push_str("<b>"),
                Tag::Strong => out.push_str("<b>"),
                Tag::Emphasis => out.push_str("<i>"),
                Tag::Strikethrough => out.push_str("<s>"),
                Tag::BlockQuote(_) => out.push_str("<blockquote>"),
                Tag::CodeBlock(kind) => {
                    match &kind {
                        CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                            code_block_lang = Some(lang.to_string());
                            out.push_str(&format!(
                                "<pre><code class=\"language-{}\">",
                                html_escape(lang)
                            ));
                        }
                        _ => {
                            code_block_lang = None;
                            out.push_str("<pre>");
                        }
                    }
                }
                Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                    out.push_str(&format!("<a href=\"{}\">", html_escape(&dest_url)));
                }
                Tag::List(start) => {
                    let kind = match start {
                        Some(n) => ListKind::Ordered(n as u32),
                        None => ListKind::Unordered,
                    };
                    list_stack.push(kind);
                }
                Tag::Item => {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    match list_stack.last_mut() {
                        Some(ListKind::Unordered) => out.push_str("• "),
                        Some(ListKind::Ordered(n)) => {
                            out.push_str(&format!("{}. ", n));
                            *n += 1;
                        }
                        None => {}
                    }
                }
                _ => {}
            },

            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => out.push_str("\n\n"),
                TagEnd::Heading(_) => out.push_str("</b>\n"),
                TagEnd::Strong => out.push_str("</b>"),
                TagEnd::Emphasis => out.push_str("</i>"),
                TagEnd::Strikethrough => out.push_str("</s>"),
                TagEnd::BlockQuote(_) => out.push_str("</blockquote>"),
                TagEnd::CodeBlock => {
                    // Remove trailing newline inside code block
                    if out.ends_with('\n') {
                        out.pop();
                    }
                    if code_block_lang.take().is_some() {
                        out.push_str("</code></pre>");
                    } else {
                        out.push_str("</pre>");
                    }
                }
                TagEnd::Link | TagEnd::Image => out.push_str("</a>"),
                TagEnd::List(_) => {
                    list_stack.pop();
                }
                TagEnd::Item => {}
                _ => {}
            },

            Event::Rule => {} // Drop horizontal rules
            _ => {}
        }
    }

    // Trim trailing whitespace/newlines
    let trimmed_len = out.trim_end().len();
    out.truncate(trimmed_len);
    out
}

enum ListKind {
    Ordered(u32),
    Unordered,
}

/// Escape `<`, `>`, `&` for Telegram HTML.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    html_escape_into(s, &mut out);
    out
}

fn html_escape_into(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot markdown_tests -- --nocapture`
Expected: all tests pass. Fix any failures — the exact output format for paragraphs and lists may need minor adjustments (trailing newlines, spacing).

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/markdown.rs crates/bot/src/telegram/markdown_tests.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): add pulldown-cmark Markdown→Telegram HTML converter"
```

---

### Task 3: HTML-aware message splitting with tests (TDD)

**Files:**
- Modify: `crates/bot/src/telegram/markdown.rs`
- Modify: `crates/bot/src/telegram/markdown_tests.rs`

The current `split_message` in `worker.rs:95-115` splits on raw byte length at newline boundaries. With HTML, we need to:
1. Not split inside `<pre>...</pre>` blocks
2. Close open tags at split boundary, reopen them in the next part
3. Still respect the 4096-char Telegram limit

- [ ] **Step 1: Write failing tests for HTML-aware splitting**

Append to `crates/bot/src/telegram/markdown_tests.rs`:

```rust
use super::markdown::split_html_message;

#[test]
fn short_message_no_split() {
    let parts = split_html_message("hello");
    assert_eq!(parts, vec!["hello"]);
}

#[test]
fn long_message_splits_at_newline() {
    // Build a message just over 4096 chars
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
    // Build: <b> + 4090 chars of 'a' + </b>  — total > 4096
    let inner = "a".repeat(4090);
    let msg = format!("<b>{inner}</b>");
    let parts = split_html_message(&msg);
    assert!(parts.len() >= 2);
    // First part must close the bold tag
    assert!(parts[0].ends_with("</b>"), "first part: {}", &parts[0][parts[0].len()-20..]);
    // Second part must reopen it
    assert!(parts[1].starts_with("<b>"), "second part start: {}", &parts[1][..20.min(parts[1].len())]);
}

#[test]
fn split_preserves_pre_block_if_possible() {
    // <pre> block under 4096 — should not be split
    let code = "x\n".repeat(100);
    let msg = format!("text before\n<pre>{code}</pre>\ntext after");
    assert!(msg.len() < 4096);
    let parts = split_html_message(&msg);
    assert_eq!(parts.len(), 1);
}

#[test]
fn split_handles_pre_block_over_limit() {
    // <pre> block over 4096 — must split but close/reopen <pre>
    let code = "x".repeat(5000);
    let msg = format!("<pre>{code}</pre>");
    let parts = split_html_message(&msg);
    assert!(parts.len() >= 2);
    assert!(parts[0].contains("<pre>"), "first part missing <pre>");
    assert!(parts[0].ends_with("</pre>"), "first part must close pre: ...{}", &parts[0][parts[0].len().saturating_sub(20)..]);
    assert!(parts[1].starts_with("<pre>"), "second part must reopen pre");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot split_html -- --nocapture 2>&1 | head -20`
Expected: compilation error (function doesn't exist)

- [ ] **Step 3: Implement HTML-aware message splitting**

Add to `crates/bot/src/telegram/markdown.rs`:

```rust
const TELEGRAM_LIMIT: usize = 4096;

/// Split an HTML message at the Telegram 4096-char limit.
///
/// Tracks open tags and closes/reopens them across split boundaries.
/// Avoids splitting inside `<pre>` blocks when possible.
pub fn split_html_message(html: &str) -> Vec<String> {
    if html.len() <= TELEGRAM_LIMIT {
        return vec![html.to_string()];
    }

    let mut parts: Vec<String> = Vec::new();
    let mut remaining = html;

    while remaining.len() > TELEGRAM_LIMIT {
        // Find split point: last \n in final 400 chars before boundary.
        // Wider window than plain-text split (200) to accommodate closing tags.
        let cut = &remaining[..TELEGRAM_LIMIT];
        let window_start = TELEGRAM_LIMIT.saturating_sub(400);
        let split_pos = cut[window_start..]
            .rfind('\n')
            .map(|p| window_start + p + 1)
            .unwrap_or(TELEGRAM_LIMIT);

        let chunk = &remaining[..split_pos];

        // Find unclosed tags in this chunk
        let open_tags = find_unclosed_tags(chunk);

        // Build the part: chunk + closing tags
        let mut part = chunk.to_string();
        for tag in open_tags.iter().rev() {
            part.push_str(&format!("</{tag}>"));
        }
        parts.push(part);

        // Next part starts with reopening tags
        let mut prefix = String::new();
        for tag in &open_tags {
            prefix.push_str(&format!("<{tag}>"));
        }
        let rest = &remaining[split_pos..];
        remaining = // can't easily prepend to a &str, so we need a different approach
        // We'll collect remaining into an owned string with prefix
        // This is a bit awkward — restructure to work with owned strings
    }

    // ... (see full implementation below)
    parts
}
```

Actually, the splitting logic needs to work with owned strings since we prepend tags. Here's the complete implementation:

```rust
const TELEGRAM_LIMIT: usize = 4096;

/// Split an HTML message at the Telegram 4096-char limit.
///
/// Tracks open tags and closes/reopens them across split boundaries.
pub fn split_html_message(html: &str) -> Vec<String> {
    if html.len() <= TELEGRAM_LIMIT {
        return vec![html.to_string()];
    }

    let mut parts: Vec<String> = Vec::new();
    let mut buf = html.to_string();

    while buf.len() > TELEGRAM_LIMIT {
        let window_start = TELEGRAM_LIMIT.saturating_sub(400);
        let split_pos = buf[window_start..TELEGRAM_LIMIT]
            .rfind('\n')
            .map(|p| window_start + p + 1)
            .unwrap_or(TELEGRAM_LIMIT);

        let chunk = &buf[..split_pos];
        let open_tags = find_unclosed_tags(chunk);

        // Close open tags at end of this chunk
        let mut part = chunk.to_string();
        for tag in open_tags.iter().rev() {
            part.push_str("</");
            part.push_str(tag);
            part.push('>');
        }
        parts.push(part);

        // Reopen tags at start of next chunk
        let mut next = String::new();
        for tag in &open_tags {
            next.push('<');
            next.push_str(tag);
            next.push('>');
        }
        next.push_str(&buf[split_pos..]);
        buf = next;
    }

    if !buf.is_empty() {
        parts.push(buf);
    }
    parts
}

/// Find tags that are opened but not closed in the given HTML fragment.
///
/// Returns tag names in order of opening (outermost first).
/// Only tracks Telegram-supported tags: b, i, s, u, code, pre, a, blockquote.
fn find_unclosed_tags(html: &str) -> Vec<String> {
    let mut stack: Vec<String> = Vec::new();
    let mut i = 0;
    let bytes = html.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(end) = html[i..].find('>') {
                let tag_content = &html[i + 1..i + end];
                if let Some(tag_name) = parse_telegram_tag(tag_content) {
                    if tag_content.starts_with('/') {
                        // Closing tag — pop from stack
                        if let Some(pos) = stack.iter().rposition(|t| *t == tag_name) {
                            stack.remove(pos);
                        }
                    } else {
                        // Opening tag
                        stack.push(tag_name);
                    }
                }
                i += end + 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    stack
}

/// Extract Telegram-supported tag name from tag content (without < >).
///
/// Handles both opening (`b`, `a href="..."`) and closing (`/b`, `/a`) tags.
/// Returns None for non-Telegram tags or entities like `&amp;`.
fn parse_telegram_tag(content: &str) -> Option<String> {
    let trimmed = content.trim_start_matches('/');
    let name = trimmed.split_whitespace().next().unwrap_or(trimmed);
    let name = name.split(|c: char| !c.is_alphanumeric()).next().unwrap_or(name);

    match name {
        "b" | "i" | "s" | "u" | "code" | "pre" | "a" | "blockquote" => {
            Some(name.to_string())
        }
        _ => None,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot split_html -- --nocapture`
Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/markdown.rs crates/bot/src/telegram/markdown_tests.rs
git commit -m "feat(bot): add HTML-aware message splitting for Telegram"
```

---

### Task 4: Integrate converter into worker.rs send path

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:89-115` (remove old split_message or keep as fallback)
- Modify: `crates/bot/src/telegram/worker.rs:440-465` (apply conversion + ParseMode::Html)
- Modify: `crates/bot/src/telegram/worker.rs:624-637` (update send_tg helper)

- [ ] **Step 1: Update the main reply-sending block**

In `crates/bot/src/telegram/worker.rs`, find the block at lines 440-465 where CC replies are sent. Replace:

```rust
                    if let Some(content) = output.content {
                        let parts = split_message(&content);
                        tracing::info!(
                            ?key,
                            content_len = content.len(),
                            parts = parts.len(),
                            ?reply_to,
                            "sending reply to Telegram"
                        );
                        for part in parts {
                            let mut send = ctx.bot.send_message(tg_chat_id, &part);
                            if eff_thread_id != 0 {
                                send = send.message_thread_id(ThreadId(MessageId(
                                    eff_thread_id as i32,
                                )));
                            }
                            if let Some(ref_id) = reply_to {
                                send = send.reply_parameters(ReplyParameters {
                                    message_id: MessageId(ref_id),
                                    ..Default::default()
                                });
                            }
                            if let Err(e) = send.await {
                                tracing::error!(?key, "failed to send Telegram reply: {:#}", e);
                            }
                        }
                    }
```

With:

```rust
                    if let Some(content) = output.content {
                        let html = super::markdown::md_to_telegram_html(&content);
                        let parts = super::markdown::split_html_message(&html);
                        tracing::info!(
                            ?key,
                            content_len = content.len(),
                            html_len = html.len(),
                            parts = parts.len(),
                            ?reply_to,
                            "sending reply to Telegram"
                        );
                        for part in &parts {
                            let mut send = ctx.bot.send_message(tg_chat_id, part);
                            send = send.parse_mode(teloxide::types::ParseMode::Html);
                            if eff_thread_id != 0 {
                                send = send.message_thread_id(ThreadId(MessageId(
                                    eff_thread_id as i32,
                                )));
                            }
                            if let Some(ref_id) = reply_to {
                                send = send.reply_parameters(ReplyParameters {
                                    message_id: MessageId(ref_id),
                                    ..Default::default()
                                });
                            }
                            if let Err(e) = send.await {
                                tracing::warn!(?key, "HTML send failed, retrying as plain text: {:#}", e);
                                // Fallback: strip HTML tags, send as plain text
                                let plain = strip_html_tags(part);
                                let mut fallback = ctx.bot.send_message(tg_chat_id, &plain);
                                if eff_thread_id != 0 {
                                    fallback = fallback.message_thread_id(ThreadId(MessageId(
                                        eff_thread_id as i32,
                                    )));
                                }
                                if let Some(ref_id) = reply_to {
                                    fallback = fallback.reply_parameters(ReplyParameters {
                                        message_id: MessageId(ref_id),
                                        ..Default::default()
                                    });
                                }
                                if let Err(e2) = fallback.await {
                                    tracing::error!(?key, "plain text fallback also failed: {:#}", e2);
                                }
                            }
                        }
                    }
```

- [ ] **Step 2: Add strip_html_tags helper in worker.rs**

Add near the other helpers (around line 88, after the existing `split_message`):

```rust
/// Strip HTML tags for plain-text fallback when Telegram rejects HTML.
/// Also decodes common entities back to their characters.
fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rightclaw-bot`
Expected: compiles. The old `split_message` may now have unused warnings — keep it for now (used by tests), or remove if no other callers exist.

- [ ] **Step 4: Remove old split_message if unused**

Check if `split_message` is used anywhere else:

Run: `rg 'split_message' crates/bot/src/ --type rust`

If only used in tests of `split_message` itself, remove the function and its tests. The HTML-aware `split_html_message` in `markdown.rs` replaces it.

If still used elsewhere (e.g. `send_tg` helper or error messages), keep it.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): send Telegram replies as HTML with plain-text fallback"
```

---

### Task 5: Update AGENTS.md template with Telegram formatting guidance

**Files:**
- Modify: `templates/right/AGENTS.md:42-46`

- [ ] **Step 1: Update the Communication section**

In `templates/right/AGENTS.md`, replace lines 42-46:

```markdown
## Communication

You communicate via Telegram. Messages may include photos, documents, and other attachments.
Be concise — Telegram is a chat medium, not a document viewer.
Use markdown sparingly — Telegram supports limited formatting.
```

With:

```markdown
## Communication

You communicate via Telegram. Messages may include photos, documents, and other attachments.
Be concise — Telegram is a chat medium, not a document viewer.

### Formatting

Use standard Markdown — the bot converts it to Telegram HTML automatically.

**Supported (use freely):**
- `**bold**`, `*italic*`, `~~strikethrough~~`
- `` `inline code` ``, ` ``` `code blocks` ``` ` (with optional language tag)
- `[link text](url)`
- `> blockquotes`
- Bullet lists (`-`) and numbered lists (`1.`)

**Avoid (won't render well in Telegram):**
- Tables — use code blocks or plain text instead
- Nested lists deeper than one level
- Horizontal rules (`---`)
- HTML tags — write Markdown, not HTML
- Headings (`#`, `##`) — use **bold text** for section structure instead
```

- [ ] **Step 2: Commit**

```bash
git add templates/right/AGENTS.md
git commit -m "docs(agents): add explicit Telegram formatting guidance to AGENTS.md"
```

---

### Task 6: Build workspace and run full test suite

**Files:** None (verification only)

- [ ] **Step 1: Build the entire workspace**

Run: `cargo build --workspace`
Expected: clean build, no errors

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: all tests pass

- [ ] **Step 4: Fix any issues found in steps 1-3, then commit fixes**

---

### Task 7: Manual testing with Telegram (optional but recommended)

This task is manual — deploy the bot and verify formatting works end-to-end.

- [ ] **Step 1: Start the bot with an agent**

Run: `rightclaw up --agents <test-agent>`

- [ ] **Step 2: Send test messages that trigger formatted replies**

Ask the agent questions that produce:
- Bold text and inline code
- Code blocks with language tags
- Links
- Bullet lists
- Long messages (>4096 chars) with code blocks

- [ ] **Step 3: Verify in Telegram**

Check that:
- Bold renders as bold (not `**asterisks**`)
- Code renders in monospace
- Code blocks have syntax highlighting hint
- Long messages split cleanly without broken tags
- If any message fails HTML parsing, it falls back to readable plain text
