//! Tokened ⟨⟨SYSTEM_NOTICE⟩⟩ markers. The token (per-agent, from
//! `right_mcp::credentials::get_or_create_notice_token`) makes the channel
//! unforgeable: the agent obeys a notice only if it carries this token.

/// Wrap `body` in tokened SYSTEM_NOTICE markers.
pub(crate) fn wrap_system_notice(token: &str, body: &str) -> String {
    format!(
        "\u{27e8}\u{27e8}SYSTEM_NOTICE:{token}\u{27e9}\u{27e9}\n{body}\n\u{27e8}\u{27e8}/SYSTEM_NOTICE:{token}\u{27e9}\u{27e9}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_with_token_in_both_markers() {
        let s = wrap_system_notice("deadbeef", "hello");
        assert!(s.starts_with("\u{27e8}\u{27e8}SYSTEM_NOTICE:deadbeef\u{27e9}\u{27e9}"));
        assert!(s.contains("\u{27e8}\u{27e8}/SYSTEM_NOTICE:deadbeef\u{27e9}\u{27e9}"));
        assert!(s.contains("hello"));
    }
}
