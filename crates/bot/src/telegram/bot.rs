use teloxide::adaptors::{CacheMe, Throttle};
use teloxide::adaptors::throttle::Limits;
use teloxide::prelude::*;

/// Construct the bot adaptor with correct ordering: CacheMe<Throttle<Bot>>.
///
/// Ordering per BOT-03 and teloxide issue #516:
/// `.throttle(Limits::default()).cache_me()` — Throttle is inner, CacheMe is outer.
/// NEVER use Throttle<CacheMe<Bot>> — deadlock risk.
pub fn build_bot(token: String) -> CacheMe<Throttle<Bot>> {
    Bot::new(token)
        .throttle(Limits::default())
        .cache_me()
}
