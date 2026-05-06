//! Stable platform-foundation modules for `right`.
//!
//! Bottom-of-stack crate. Other crates depend on it; it depends on
//! nothing in this workspace. Modules here change rarely; incremental
//! edits to `right-codegen`, `right-memory`, `right-mcp`, or
//! `right-cc` should not invalidate this crate's build cache.

pub mod config;
pub mod error;
#[cfg(unix)]
pub mod process_group;
pub mod ui;
