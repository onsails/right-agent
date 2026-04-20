//! Markdown -> Telegram HTML converter.
//!
//! Uses pulldown-cmark to parse GitHub-Flavored Markdown and renders
//! the subset of HTML that Telegram's Bot API supports.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;

/// Convert GFM Markdown to Telegram-compatible HTML.
pub fn md_to_telegram_html(md: &str) -> String {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(md, opts);

    let mut out = String::with_capacity(md.len());
    let mut list_stack: Vec<ListKind> = Vec::new();
    let mut code_block_lang: Option<String> = None;
    let mut table: Option<TableBuilder> = None;

    for event in parser {
        // While inside a table, collect cells instead of emitting directly.
        if let Some(ref mut tb) = table {
            match event {
                Event::Start(Tag::TableHead) => {
                    tb.in_head = true;
                    tb.current_row = Vec::new();
                }
                Event::End(TagEnd::TableHead) => {
                    // In pulldown-cmark, TableHead may not wrap TableRow —
                    // cells are direct children. Save accumulated cells as header.
                    if !tb.current_row.is_empty() {
                        tb.header = std::mem::take(&mut tb.current_row);
                    }
                    tb.in_head = false;
                }
                Event::Start(Tag::TableRow) => tb.current_row = Vec::new(),
                Event::End(TagEnd::TableRow) => {
                    let row = std::mem::take(&mut tb.current_row);
                    if tb.in_head {
                        tb.header = row;
                    } else {
                        tb.rows.push(row);
                    }
                }
                Event::Start(Tag::TableCell) => tb.current_cell.clear(),
                Event::End(TagEnd::TableCell) => {
                    tb.current_row.push(std::mem::take(&mut tb.current_cell));
                }
                Event::Text(text) => tb.current_cell.push_str(&text),
                Event::Code(code) => {
                    tb.current_cell.push('`');
                    tb.current_cell.push_str(&code);
                    tb.current_cell.push('`');
                }
                Event::SoftBreak | Event::HardBreak => tb.current_cell.push(' '),
                Event::End(TagEnd::Table) => {
                    let built = std::mem::take(&mut *tb);
                    table = None;
                    render_table(&built, &mut out);
                    continue;
                }
                _ => {}
            }
            continue;
        }

        match event {
            Event::Start(Tag::Table(_)) => {
                table = Some(TableBuilder::default());
                continue;
            }
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

            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { .. } => out.push_str("<b>"),
                Tag::Strong => out.push_str("<b>"),
                Tag::Emphasis => out.push_str("<i>"),
                Tag::Strikethrough => out.push_str("<s>"),
                Tag::BlockQuote(_) => out.push_str("<blockquote>"),
                Tag::CodeBlock(kind) => match &kind {
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
                },
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

            Event::Rule => {}
            _ => {}
        }
    }

    let trimmed_len = out.trim_end().len();
    out.truncate(trimmed_len);
    out
}

enum ListKind {
    Ordered(u32),
    Unordered,
}

#[derive(Default)]
struct TableBuilder {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_head: bool,
}

/// Render a markdown table as a `<pre>` block with aligned columns.
fn render_table(tb: &TableBuilder, out: &mut String) {
    let col_count = tb
        .header
        .len()
        .max(tb.rows.iter().map(|r| r.len()).max().unwrap_or(0));
    if col_count == 0 {
        return;
    }

    // Calculate column widths.
    let mut widths = vec![0usize; col_count];
    for (i, cell) in tb.header.iter().enumerate() {
        widths[i] = widths[i].max(cell.len());
    }
    for row in &tb.rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    out.push_str("<pre>");

    // Header row.
    if !tb.header.is_empty() {
        render_table_row(&tb.header, &widths, col_count, out);
        // Separator.
        for (i, &w) in widths.iter().enumerate() {
            if i > 0 {
                out.push_str("-+-");
            }
            for _ in 0..w {
                out.push('-');
            }
        }
        out.push('\n');
    }

    // Data rows.
    for row in &tb.rows {
        render_table_row(row, &widths, col_count, out);
    }

    // Remove trailing newline before closing tag.
    if out.ends_with('\n') {
        out.pop();
    }
    out.push_str("</pre>\n\n");
}

fn render_table_row(cells: &[String], widths: &[usize], col_count: usize, out: &mut String) {
    for (i, width) in widths.iter().enumerate().take(col_count) {
        if i > 0 {
            out.push_str(" | ");
        }
        let cell = cells.get(i).map(|s| s.as_str()).unwrap_or("");
        // HTML-escape cell content (it's inside <pre> so < > & still need escaping).
        let escaped = html_escape(cell);
        out.push_str(&escaped);
        let pad = width.saturating_sub(cell.len());
        for _ in 0..pad {
            out.push(' ');
        }
    }
    out.push('\n');
}

pub(crate) fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    html_escape_into(s, &mut out);
    out
}

pub(crate) fn html_escape_into(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

const TELEGRAM_LIMIT: usize = 4096;

/// Round a byte index down to the nearest char boundary in `s`.
fn snap_to_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

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
        // Snap byte offsets to char boundaries (multi-byte UTF-8 safety).
        let limit = snap_to_char_boundary(&buf, TELEGRAM_LIMIT);
        let window_start = snap_to_char_boundary(&buf, limit.saturating_sub(400));
        let split_pos = buf[window_start..limit]
            .rfind('\n')
            .map(|p| window_start + p + 1)
            .unwrap_or(limit);

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
fn parse_telegram_tag(content: &str) -> Option<String> {
    let trimmed = content.trim_start_matches('/');
    let name = trimmed.split_whitespace().next().unwrap_or(trimmed);
    let name = name
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or(name);

    match name {
        "b" | "i" | "s" | "u" | "code" | "pre" | "a" | "blockquote" => Some(name.to_string()),
        _ => None,
    }
}

/// Strip HTML tags and unescape basic HTML entities.
pub fn strip_html_tags(html: &str) -> String {
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
