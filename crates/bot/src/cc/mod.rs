//! Claude Code subprocess plumbing shared across bot entry points.
//!
//! Stage D keeps this module inside `right-bot` while moving reusable CC code
//! out of Telegram-specific modules. Stage E extracts this subtree to
//! `right-cc`.

pub mod attachments_dto;
pub mod invocation;
pub mod markdown_utils;
pub mod prompt;
pub mod stream;
pub mod worker_reply;
