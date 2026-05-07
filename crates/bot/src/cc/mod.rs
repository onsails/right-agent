//! Claude Code subprocess plumbing shared across bot entry points.
//!
//! Stage D keeps this module inside `right-bot` while moving reusable CC code
//! out of Telegram-specific modules. Stage E extracts this subtree to
//! `right-cc`.

pub(crate) mod attachments_dto;
pub(crate) mod invocation;
pub(crate) mod markdown_utils;
pub(crate) mod prompt;
pub(crate) mod stream;
pub(crate) mod worker_reply;
