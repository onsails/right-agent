pub(crate) const LOCATOR_MAX: usize = 120;
pub(crate) const REPLY_BODY_INLINE_MAX: usize = 500;
pub(crate) const IN_CONTEXT_WINDOW: i64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplyRender {
    OwnPrevious,
    Locator { text: String },
    Full { text: String },
    Truncated { text: String, reply_to_id: i32 },
    NoText,
}

fn truncate_with_ellipsis(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

pub(crate) fn decide_reply_render(
    reply_to_id: i32,
    target_text: Option<&str>,
    is_bot_target: bool,
    is_latest_assistant: bool,
    is_recent_routed_user: bool,
) -> ReplyRender {
    let Some(text) = target_text.map(str::trim).filter(|text| !text.is_empty()) else {
        return ReplyRender::NoText;
    };

    if is_bot_target && is_latest_assistant {
        return ReplyRender::OwnPrevious;
    }

    if is_bot_target || is_recent_routed_user {
        return ReplyRender::Locator {
            text: truncate_with_ellipsis(text, LOCATOR_MAX),
        };
    }

    if text.chars().count() <= REPLY_BODY_INLINE_MAX {
        ReplyRender::Full {
            text: text.to_owned(),
        }
    } else {
        ReplyRender::Truncated {
            text: truncate_with_ellipsis(text, REPLY_BODY_INLINE_MAX),
            reply_to_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_freshest_message_renders_as_own_previous() {
        let render = decide_reply_render(10, Some("latest assistant"), true, true, false);

        assert_eq!(render, ReplyRender::OwnPrevious);
    }

    #[test]
    fn bot_older_message_renders_as_locator() {
        let render = decide_reply_render(10, Some("older assistant"), true, false, false);

        assert_eq!(
            render,
            ReplyRender::Locator {
                text: "older assistant".to_owned()
            }
        );
    }

    #[test]
    fn in_context_user_message_renders_as_locator() {
        let render = decide_reply_render(11, Some("nearby user request"), false, false, true);

        assert_eq!(
            render,
            ReplyRender::Locator {
                text: "nearby user request".to_owned()
            }
        );
    }

    #[test]
    fn out_of_context_short_user_message_renders_full() {
        let render = decide_reply_render(12, Some("Сравни по времени в море"), false, false, false);

        assert_eq!(
            render,
            ReplyRender::Full {
                text: "Сравни по времени в море".to_owned()
            }
        );
    }

    #[test]
    fn out_of_context_long_user_message_renders_truncated_with_reply_id_and_ellipsis() {
        let text = "a".repeat(REPLY_BODY_INLINE_MAX + 1);

        let render = decide_reply_render(13, Some(&text), false, false, false);

        assert_eq!(
            render,
            ReplyRender::Truncated {
                text: format!("{}…", "a".repeat(REPLY_BODY_INLINE_MAX)),
                reply_to_id: 13
            }
        );
    }

    #[test]
    fn empty_whitespace_or_missing_target_renders_no_text() {
        assert_eq!(
            decide_reply_render(14, None, false, false, false),
            ReplyRender::NoText
        );
        assert_eq!(
            decide_reply_render(14, Some(""), false, false, false),
            ReplyRender::NoText
        );
        assert_eq!(
            decide_reply_render(14, Some(" \n\t "), false, false, false),
            ReplyRender::NoText
        );
    }

    #[test]
    fn long_in_context_locator_truncates() {
        let text = "b".repeat(LOCATOR_MAX + 1);

        let render = decide_reply_render(15, Some(&text), false, false, true);

        assert_eq!(
            render,
            ReplyRender::Locator {
                text: format!("{}…", "b".repeat(LOCATOR_MAX))
            }
        );
    }
}
