//! Time-related constants that cross crate boundaries.
//!
//! `IDLE_THRESHOLD_SECS` is user-meaningful: it answers "why didn't the cron
//! notification arrive yet?" Pending notifications are held until the chat
//! has been idle for this long within CC's 5-min prompt cache TTL.

/// Idle threshold in seconds before pending cron notifications are delivered.
pub const IDLE_THRESHOLD_SECS: i64 = 180;

/// Human-readable form for prose ("3 min" reads better than "180 s").
pub const IDLE_THRESHOLD_MIN: i64 = IDLE_THRESHOLD_SECS / 60;
