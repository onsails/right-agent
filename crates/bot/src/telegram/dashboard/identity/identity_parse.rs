//! Pure parser for the coalesced sandbox identity read.

/// One file's result from the combined sandbox read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ParsedIdentityRead {
    pub name: String,
    pub present: bool,
    pub content: String,
    /// True when the file was longer than the requested preview limit.
    pub truncated: bool,
}

/// Combined-read framing: each file is emitted as a header line
/// `RIGHT_IDENTITY <name> <PRESENT|ABSENT> <byte_count>\n` followed by exactly
/// `byte_count` content bytes and a trailing `\n`. `preview_limit` is the
/// number of content bytes requested per file (the script asks for
/// `preview_limit + 1` so truncation is detectable).
pub(super) fn parse_combined_identity_read(
    stdout: &str,
    preview_limit: usize,
) -> Vec<ParsedIdentityRead> {
    let mut out = Vec::new();
    let bytes = stdout.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        let Some(nl) = bytes[idx..].iter().position(|&b| b == b'\n') else {
            break;
        };
        let header = &stdout[idx..idx + nl];
        idx += nl + 1;
        let mut parts = header.splitn(4, ' ');
        if parts.next() != Some("RIGHT_IDENTITY") {
            continue;
        }
        let (Some(name), Some(file_state), Some(count)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Ok(n) = count.trim().parse::<usize>() else {
            continue;
        };
        if file_state == "ABSENT" {
            out.push(ParsedIdentityRead {
                name: name.to_owned(),
                present: false,
                content: String::new(),
                truncated: false,
            });
            continue;
        }
        // `n` is the `wc -c` byte count computed in-sandbox on raw bytes, but
        // `stdout` here is `from_utf8_lossy` output: a multibyte codepoint split
        // by the in-sandbox `head -c` cut decodes to U+FFFD (3 bytes), so
        // `idx + n` can land mid-codepoint. Clamp DOWN to a char boundary so the
        // slice never panics and `idx` stays on a boundary for the next header.
        let mut end = (idx + n).min(bytes.len());
        while end > idx && !stdout.is_char_boundary(end) {
            end -= 1;
        }
        let mut content = stdout[idx..end].to_owned();
        idx = end;
        if idx < bytes.len() && bytes[idx] == b'\n' {
            idx += 1;
        }
        let truncated = n > preview_limit;
        if truncated {
            // Match the host-read path: cut down to the nearest char boundary
            // at or below `preview_limit`, never overshooting it.
            right_dashboard::fs_safety::truncate_to_char_boundary(&mut content, preview_limit);
        }
        out.push(ParsedIdentityRead {
            name: name.to_owned(),
            present: true,
            content,
            truncated,
        });
    }
    out
}

/// Per-file state for a sandboxed agent given sandbox + host-mirror presence.
/// Timeout/exec errors are handled by the caller (mapped to
/// `sandbox_unreachable`); this only covers a successful combined read.
pub(super) fn identity_state(sandbox_present: bool, host_present: bool) -> &'static str {
    match (sandbox_present, host_present) {
        (true, _) => "sandbox",
        (false, true) => "host_mirror",
        (false, false) => "not_authored",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_present_and_absent_files() {
        // Header byte counts must equal the actual content byte length: "# hi\n"
        // is 5 bytes and "you\n" is 4 bytes.
        let stdout = "RIGHT_IDENTITY IDENTITY.md PRESENT 5\n# hi\n\n\
                      RIGHT_IDENTITY SOUL.md ABSENT 0\n\
                      RIGHT_IDENTITY USER.md PRESENT 4\nyou\n\n";
        let parsed = parse_combined_identity_read(stdout, 64 * 1024);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].name, "IDENTITY.md");
        assert!(parsed[0].present);
        assert_eq!(parsed[0].content, "# hi\n");
        assert!(!parsed[1].present);
        assert_eq!(parsed[2].content, "you\n");
    }

    #[test]
    fn marks_truncated_when_over_limit() {
        let stdout = "RIGHT_IDENTITY IDENTITY.md PRESENT 4\nabcd\n";
        let parsed = parse_combined_identity_read(stdout, 3);
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].truncated);
        assert_eq!(parsed[0].content, "abc");
    }

    #[test]
    fn does_not_split_multibyte_at_truncation_boundary() {
        // "é" is 2 bytes; with content "aé" (3 bytes) over a 2-byte limit the
        // cut must land on the char boundary after "a", not mid-codepoint.
        let stdout = "RIGHT_IDENTITY IDENTITY.md PRESENT 3\naé\n";
        let parsed = parse_combined_identity_read(stdout, 2);
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].truncated);
        assert_eq!(parsed[0].content, "a");
    }

    #[test]
    fn does_not_panic_when_byte_count_splits_a_codepoint() {
        // `wc -c` (raw bytes, in-sandbox) can disagree with the from_utf8_lossy
        // string the parser sees, landing `idx + n` mid-codepoint. The parser must
        // clamp to a char boundary instead of panicking on the str slice.
        // "é" is 2 bytes (0xC3 0xA9); claiming PRESENT 1 points idx+1 between them.
        let stdout = "RIGHT_IDENTITY IDENTITY.md PRESENT 1\né\n";
        let parsed = parse_combined_identity_read(stdout, 64 * 1024);
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].present);
        // Clamped down to the char boundary at idx → empty content, and crucially
        // no panic. The result is valid UTF-8 by construction (it is a String).
        assert_eq!(parsed[0].content, "");
        assert!(!parsed[0].truncated);
    }

    #[test]
    fn maps_states_from_sandbox_and_host_presence() {
        assert_eq!(identity_state(true, true), "sandbox");
        assert_eq!(identity_state(true, false), "sandbox");
        assert_eq!(identity_state(false, true), "host_mirror");
        assert_eq!(identity_state(false, false), "not_authored");
    }
}
