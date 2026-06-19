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

use thiserror::Error;

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
