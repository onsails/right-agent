//! Volatile platform knobs with agent-facing or UX-facing effects.
//!
//! `IDLE_THRESHOLD_SECS` is the UX-politeness gate on cron notification
//! delivery: pending notifications are held until the chat has been idle
//! for this long, so a cron result never interrupts an active conversation.
//! Correctness against `--resume` races is handled separately by the
//! per-session mutex (see `docs/architecture/sessions.md`); this constant
//! is purely about UX.
//!
//! Implication for the agent: any delivery the user is expecting (e.g. a
//! "remind me in N minutes" reminder) cannot arrive sooner than
//! `IDLE_THRESHOLD_SECS` of chat idle, regardless of `run_at`. The agent
//! must not promise faster delivery — see `OPERATING_INSTRUCTIONS.md` and
//! the `/rightcron` skill.

#![warn(unreachable_pub)]

/// Idle threshold in seconds before pending cron notifications are delivered.
pub const IDLE_THRESHOLD_SECS: i64 = 120;

/// Human-readable form for prose ("2 min" reads better than "120 s").
pub const IDLE_THRESHOLD_MIN: i64 = IDLE_THRESHOLD_SECS / 60;
