//! Telegram HTML utilities shared by operator-facing platform messages.
//!
//! Agent-authored outbound text is literal/typed and must not be parsed here;
//! the only remaining behavior is HTML splitting for platform-owned markup that
//! uses Telegram-supported tags.

const TELEGRAM_TAGS: &[&str] = &["a", "b", "blockquote", "code", "i", "pre", "s", "u"];

fn is_telegram_tag(name: &str) -> bool {
    TELEGRAM_TAGS.contains(&name)
}

const TELEGRAM_LIMIT: usize = 4096;
const SPLIT_WINDOW_BACKOFF: usize = 400;

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
        let (split_pos, open_tags) = find_split_pos_with_tag_budget(&buf);

        // Close open tags at end of this chunk.
        let mut part = buf[..split_pos].to_string();
        for tag in open_tags.iter().rev() {
            part.push_str("</");
            part.push_str(tag);
            part.push('>');
        }
        parts.push(part);

        // Reopen tags at start of next chunk.
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

fn find_split_pos_with_tag_budget(buf: &str) -> (usize, Vec<String>) {
    let mut candidate_limit = TELEGRAM_LIMIT;

    loop {
        // Snap byte offsets to char boundaries (multi-byte UTF-8 safety).
        let limit = snap_to_char_boundary(buf, candidate_limit);
        let window_start = snap_to_char_boundary(buf, limit.saturating_sub(SPLIT_WINDOW_BACKOFF));
        let split_pos = buf[window_start..limit]
            .rfind('\n')
            .map(|p| window_start + p + 1)
            .unwrap_or(limit);
        let open_tags = find_unclosed_tags(&buf[..split_pos]);
        let closing_len = closing_tags_len(&open_tags);

        if split_pos + closing_len <= TELEGRAM_LIMIT {
            return (split_pos, open_tags);
        }

        candidate_limit = TELEGRAM_LIMIT.saturating_sub(closing_len);
    }
}

fn find_unclosed_tags(html: &str) -> Vec<String> {
    let mut stack: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let tag_raw = &rest[start + 1..start + end];
        if let Some(name) = tag_raw.strip_prefix('/') {
            let name = name.trim();
            if is_telegram_tag(name)
                && let Some(pos) = stack.iter().rposition(|tag| tag == name)
            {
                stack.remove(pos);
            }
        } else if !tag_raw.ends_with('/')
            && let Some(name) = tag_raw.split_whitespace().next()
            && is_telegram_tag(name)
        {
            stack.push(name.to_owned());
        }
        rest = &rest[start + end + 1..];
    }
    stack
}

fn closing_tags_len(tags: &[String]) -> usize {
    tags.iter().map(|tag| tag.len() + 3).sum()
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
