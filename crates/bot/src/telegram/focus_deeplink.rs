//! Encode/decode the `/start` deep-link payload used to launch the focus Mini
//! App from a group or topic.
//!
//! Telegram inline `web_app` buttons only work in private chats, so `/set_focus`
//! issued in a group/topic cannot open the Mini App in place (the `sendMessage`
//! is rejected with `BUTTON_TYPE_INVALID`). Instead the group message carries a
//! plain `url` button to `t.me/<bot>?start=<payload>`; tapping it opens the DM
//! and delivers `/start <payload>`, where `handle_start` re-emits a real
//! `web_app` button scoped to the originating `(chat_id, thread_id)`.
//!
//! The payload must match Telegram's deep-link `start` parameter grammar:
//! `[A-Za-z0-9_-]{1,64}`. We encode the scope as `f<chat_id>_<thread_id>`:
//! `-` (negative supergroup ids), `_` (separator) and digits are all in-set, and
//! the readable form keeps the scope debuggable in logs. Real Telegram ids stay
//! well under 64 chars.

/// Prefix marking a deep-link payload as a focus-scope launch. A single leading
/// char keeps the encoding unambiguous and cheap to test.
const FOCUS_PREFIX: char = 'f';

/// Encode a focus scope into a `/start` deep-link payload.
pub(crate) fn encode_focus_start_param(chat_id: i64, thread_id: i64) -> String {
    format!("{FOCUS_PREFIX}{chat_id}_{thread_id}")
}

/// Decode a `/start` deep-link payload back into `(chat_id, thread_id)`.
///
/// Returns `None` for any payload that is not a well-formed focus scope (no
/// prefix, missing separator, unparsable integers) so the caller can fall back
/// to the plain greeting.
pub(crate) fn decode_focus_start_param(param: &str) -> Option<(i64, i64)> {
    let rest = param.strip_prefix(FOCUS_PREFIX)?;
    let (chat, thread) = rest.split_once('_')?;
    Some((chat.parse().ok()?, thread.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Telegram deep-link `start` parameter grammar: 1-64 chars from this set.
    fn is_valid_start_param(p: &str) -> bool {
        !p.is_empty()
            && p.len() <= 64
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    #[test]
    fn roundtrip_real_world_supergroup_topic() {
        // The exact scope from riskoff.log.2026-06-13: a negative supergroup id
        // and a topic thread id.
        let chat_id = -1003929337699;
        let thread_id = 29;
        let param = encode_focus_start_param(chat_id, thread_id);
        assert!(
            is_valid_start_param(&param),
            "param violates grammar: {param}"
        );
        assert_eq!(decode_focus_start_param(&param), Some((chat_id, thread_id)));
    }

    #[test]
    fn roundtrip_dm_zero_thread() {
        let param = encode_focus_start_param(123456789, 0);
        assert_eq!(decode_focus_start_param(&param), Some((123456789, 0)));
    }

    #[test]
    fn roundtrip_i64_extremes_stay_in_grammar() {
        for (chat, thread) in [(i64::MIN, i64::MAX), (i64::MAX, 0), (i64::MIN, 0)] {
            let param = encode_focus_start_param(chat, thread);
            assert!(
                is_valid_start_param(&param),
                "extreme param violates grammar: {param}"
            );
            assert_eq!(decode_focus_start_param(&param), Some((chat, thread)));
        }
    }

    #[test]
    fn decode_rejects_missing_prefix() {
        assert_eq!(decode_focus_start_param("-1003929337699_29"), None);
    }

    #[test]
    fn decode_rejects_missing_separator() {
        assert_eq!(decode_focus_start_param("f-1003929337699"), None);
    }

    #[test]
    fn decode_rejects_non_numeric() {
        assert_eq!(decode_focus_start_param("fabc_def"), None);
        assert_eq!(decode_focus_start_param("f"), None);
        assert_eq!(decode_focus_start_param(""), None);
    }

    #[test]
    fn deep_link_url_roundtrips_through_telegram_link_format() {
        // Exercises the exact `https://t.me/<bot>?start=<param>` shape that
        // `handle_set_focus` builds for groups: the param must survive URL
        // parsing untouched (no percent-encoding) and decode back to the scope.
        let chat_id = -1003929337699;
        let thread_id = 29;
        let param = encode_focus_start_param(chat_id, thread_id);
        let link = format!("https://t.me/AiPostsTestBot?start={param}");
        let url = url::Url::parse(&link).expect("deep link parses");
        let start = url
            .query_pairs()
            .find(|(k, _)| k == "start")
            .map(|(_, v)| v.into_owned())
            .expect("start param present");
        assert_eq!(start, param, "param must not be percent-encoded");
        assert_eq!(decode_focus_start_param(&start), Some((chat_id, thread_id)));
    }

    #[test]
    fn decode_ignores_unrelated_start_payloads() {
        // A future non-focus deep link must not be misread as a focus scope.
        assert_eq!(decode_focus_start_param("ref_partner42"), None);
    }
}
