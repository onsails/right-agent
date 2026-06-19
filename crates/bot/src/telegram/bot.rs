//! Bot construction helper.

use super::BotType;

/// Construct an auxiliary send-only [`BotType`] (no `get_me` round-trip).
///
/// Used for the menu, webhook-register, supervisor, delivery, and focus-notifier
/// bots that only send and never need the bot's own identity. The
/// identity-bearing dispatcher bot is built via `RightBot::connect` inside
/// `run_telegram`.
pub(crate) fn build_bot(token: String) -> BotType {
    super::tg_bot::RightBot::new(token)
}
