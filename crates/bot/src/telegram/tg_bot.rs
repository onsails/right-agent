//! Frankenstein Telegram client wrapper.
//!
//! This is the **only** module that imports `frankenstein::client_reqwest`.
//! It centralizes everything we want uniform across every outbound Telegram
//! call:
//!
//! - A single [`TgError`] type with a stable `Display` chain.
//! - Outbound rate limiting via [`Throttle`] (global + per-chat), approximating
//!   the limits teloxide's `Throttle` adaptor used to enforce.
//! - A cached `get_me` identity resolved once at [`RightBot::connect`] time,
//!   replacing teloxide's `CacheMe` adaptor.
//! - Uniform defaults: HTML parse-mode for our message/edit helpers, optional
//!   thread-id threading, single 429 retry honoring `retry_after`.
//!
//! Sibling `telegram::*` modules call into [`RightBot`] rather than touching
//! `frankenstein` directly.

use std::num::NonZeroU32;
use std::time::Duration;

use dashmap::DashMap;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use thiserror::Error;
use tokio::time::Instant;

/// Error type for every outbound Telegram operation routed through [`RightBot`].
///
/// Both `frankenstein::Error` and `reqwest::Error` already implement `Display`
/// with their own source chains, so the plain `{0}` formatting is sufficient
/// here — the `{:#}` alternate-Display rule in `AGENTS.rust.md` is specifically
/// for flattening `anyhow::Error` context chains, which neither of these is.
#[derive(Debug, Error)]
pub(crate) enum TgError {
    #[error("telegram api: {0}")]
    Api(#[from] frankenstein::Error),
    #[error("file download: {0}")]
    Download(#[from] reqwest::Error),
    #[error("{0}")]
    Other(String),
}

/// Global + per-chat outbound rate gate, approximating teloxide's `Limits`.
///
/// The global limiter is a [`governor`] direct rate limiter; the per-chat gate
/// is a simple last-send timestamp map enforcing a minimum interval per chat.
/// Both use `tokio::time::Instant` so the gate advances correctly under the
/// tokio test clock (`start_paused = true`).
pub(crate) struct Throttle {
    global: RateLimiter<NotKeyed, InMemoryState, DefaultClock>,
    per_chat_interval: Duration,
    last_per_chat: DashMap<i64, Instant>,
}

impl Throttle {
    pub(crate) fn new(global_per_sec: u32, per_chat_interval: Duration) -> Self {
        let quota =
            Quota::per_second(NonZeroU32::new(global_per_sec).expect("global_per_sec must be > 0"));
        Self {
            global: RateLimiter::direct(quota),
            per_chat_interval,
            last_per_chat: DashMap::new(),
        }
    }

    /// Block until it is permissible to send to `chat_id`: first wait out the
    /// per-chat interval, then acquire a global token.
    pub(crate) async fn acquire(&self, chat_id: i64) {
        loop {
            let now = Instant::now();
            let wait = match self.last_per_chat.get(&chat_id) {
                Some(prev) => self
                    .per_chat_interval
                    .checked_sub(now.duration_since(*prev)),
                None => None,
            };
            match wait {
                Some(d) if !d.is_zero() => tokio::time::sleep(d).await,
                _ => break,
            }
        }
        self.last_per_chat.insert(chat_id, Instant::now());
        self.global.until_ready().await;
    }
}

#[cfg(test)]
mod throttle_tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::Instant;

    #[tokio::test(start_paused = true)]
    async fn per_chat_gate_spaces_same_chat_sends() {
        let t = Throttle::new(30, Duration::from_millis(1000));
        let start = Instant::now();
        t.acquire(100).await;
        t.acquire(100).await;
        assert!(start.elapsed() >= Duration::from_millis(1000));
    }

    #[tokio::test(start_paused = true)]
    async fn different_chats_do_not_block_each_other_on_per_chat_gate() {
        let t = Throttle::new(30, Duration::from_millis(1000));
        let start = Instant::now();
        t.acquire(1).await;
        t.acquire(2).await;
        assert!(start.elapsed() < Duration::from_millis(1000));
    }
}
