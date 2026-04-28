//! Bootstrap welcome photo — embedded asset + send-gating predicate.
//!
//! The PNG is embedded at compile time so the bot has no runtime filesystem
//! dependency on the asset. Anchoring on `CARGO_MANIFEST_DIR` keeps the path
//! correct regardless of which file inside the crate references this module.

use teloxide::prelude::*;
use teloxide::types::{InputFile, MessageId, ThreadId};

const WELCOME_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/character-on-coal.png"
));

/// Pure predicate. The welcome photo goes out on the *first* CC invocation in
/// a chat **only when** that invocation is happening in bootstrap mode.
fn should_send(bootstrap_mode: bool, first_turn_in_chat: bool) -> bool {
    bootstrap_mode && first_turn_in_chat
}

/// Send the welcome photo to `chat_id` (and the given thread, if any).
///
/// Fire-and-forget: errors are logged at WARN and do not propagate. The text
/// reply is the contract; the photo is presentation.
pub(crate) async fn send_if_needed(
    bot: &super::BotType,
    chat_id: ChatId,
    eff_thread_id: i64,
    bootstrap_mode: bool,
    first_turn_in_chat: bool,
) {
    if !should_send(bootstrap_mode, first_turn_in_chat) {
        return;
    }
    let file = InputFile::memory(WELCOME_PNG).file_name("welcome.png");
    let mut req = bot.send_photo(chat_id, file);
    if eff_thread_id != 0 {
        req = req.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }
    if let Err(e) = req.await {
        tracing::warn!(%chat_id, eff_thread_id, "bootstrap welcome photo failed: {:#}", e);
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
