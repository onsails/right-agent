//! Bootstrap welcome photo — embedded asset + send-gating predicate.
//!
//! The PNG is embedded at compile time so the bot has no runtime filesystem
//! dependency on the asset. The path is crate-relative through the
//! `crates/bot/assets` symlink so `cargo package --verify` resolves it from
//! the dereferenced tarball copy as well as from the working tree.

const WELCOME_PNG: &[u8] = include_bytes!("../../assets/character-on-coal.png");

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
    chat_id: i64,
    eff_thread_id: i64,
    bootstrap_mode: bool,
    first_turn_in_chat: bool,
    caption_html: Option<&str>,
    reply_to: Option<i32>,
) -> bool {
    if !should_send(bootstrap_mode, first_turn_in_chat) {
        return false;
    }

    // Attach the caption only when it fits Telegram's caption limit; otherwise
    // the caller still sends it as a separate text part.
    let caption = caption_html.filter(|html| html.chars().count() <= CAPTION_LIMIT);
    let caption_attached = caption.is_some();
    let thread = (eff_thread_id != 0).then_some(eff_thread_id as i32);

    match bot
        .send_photo_bytes(
            chat_id,
            WELCOME_PNG,
            "welcome.png",
            caption,
            true,
            thread,
            reply_to,
        )
        .await
    {
        Ok(_) => caption_attached,
        Err(e) => {
            tracing::warn!(chat_id, eff_thread_id, "bootstrap welcome photo failed: {:#}", e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn predicate_only_true_when_both_flags_true() {
        assert!(!should_send(false, false));
        assert!(!should_send(false, true));
        assert!(!should_send(true, false));
        assert!(should_send(true, true));
    }

    #[tokio::test]
    async fn welcome_png_starts_with_png_magic() {
        // PNG signature: 89 50 4E 47 0D 0A 1A 0A
        assert!(WELCOME_PNG.len() > 8, "PNG asset is empty or truncated");
        assert_eq!(
            &WELCOME_PNG[..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            "PNG magic bytes mismatch — asset is not a PNG"
        );
    }
}
