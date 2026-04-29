//! Bootstrap welcome photo — embedded asset + send-gating predicate.
//!
//! The PNG is embedded at compile time so the bot has no runtime filesystem
//! dependency on the asset. Anchoring on `CARGO_MANIFEST_DIR` keeps the path
//! correct regardless of which file inside the crate references this module.

use teloxide::prelude::*;
use teloxide::types::{InputFile, MessageId, ReplyParameters, ThreadId};

const WELCOME_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/character-on-coal.png"
));

// Telegram caption hard limit; HTML tags count toward it.
const CAPTION_LIMIT: usize = 1024;

/// Pure predicate. The welcome photo goes out on the *first* CC invocation in
/// a chat **only when** that invocation is happening in bootstrap mode.
fn should_send(bootstrap_mode: bool, first_turn_in_chat: bool) -> bool {
    bootstrap_mode && first_turn_in_chat
}

/// Send the welcome photo, optionally attaching `caption_html` as the photo
/// caption so image + first reply land as a single Telegram message.
///
/// Returns `true` iff the photo was sent **with** the caption — in which case
/// the caller must skip that text part in its own message loop. Returns
/// `false` if the photo was skipped, sent without caption (caption too long
/// or absent), or if the send failed. Errors are logged at WARN and never
/// propagate; the text reply is the contract, the photo is presentation.
pub(crate) async fn send_if_needed(
    bot: &super::BotType,
    chat_id: ChatId,
    eff_thread_id: i64,
    bootstrap_mode: bool,
    first_turn_in_chat: bool,
    caption_html: Option<&str>,
    reply_to: Option<i32>,
) -> bool {
    if !should_send(bootstrap_mode, first_turn_in_chat) {
        return false;
    }
    let file = InputFile::memory(WELCOME_PNG).file_name("welcome.png");
    let mut req = bot.send_photo(chat_id, file);

    let caption_attached = match caption_html {
        Some(html) if html.chars().count() <= CAPTION_LIMIT => {
            req = req
                .caption(html.to_owned())
                .parse_mode(teloxide::types::ParseMode::Html);
            true
        }
        _ => false,
    };

    if eff_thread_id != 0 {
        req = req.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }
    if let Some(id) = reply_to {
        req = req.reply_parameters(ReplyParameters {
            message_id: MessageId(id),
            ..Default::default()
        });
    }

    match req.await {
        Ok(_) => caption_attached,
        Err(e) => {
            tracing::warn!(%chat_id, eff_thread_id, "bootstrap welcome photo failed: {:#}", e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicate_only_true_when_both_flags_true() {
        assert!(!should_send(false, false));
        assert!(!should_send(false, true));
        assert!(!should_send(true, false));
        assert!(should_send(true, true));
    }

    #[test]
    fn welcome_png_starts_with_png_magic() {
        // PNG signature: 89 50 4E 47 0D 0A 1A 0A
        assert!(WELCOME_PNG.len() > 8, "PNG asset is empty or truncated");
        assert_eq!(
            &WELCOME_PNG[..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            "PNG magic bytes mismatch — asset is not a PNG"
        );
    }
}
