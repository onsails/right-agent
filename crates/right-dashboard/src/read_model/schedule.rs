//! Pure, DB-free presentation helpers for cron schedules. Kept local to the
//! dashboard read-model so the presentation crate does not take a production
//! dependency on the heavy `right-agent` crate (which owns the canonical cron
//! semantics). In `next_run_at` the 5-field→7-field conversion mirrors
//! `crates/bot/src/cron.rs::to_7field` exactly so the next-fire time shown in
//! the dashboard matches what the reconciler actually computes. `describe` does
//! NOT expand to 7-field: `cron_descriptor` parses 5-field expressions natively,
//! so the raw schedule is passed straight through.
//!
//! A malformed schedule never errors the overview: `describe` falls back to
//! the raw string and `next_run_at` returns `None`. Schedules are validated at
//! creation time; here we only render.

use std::str::FromStr;

use chrono::{DateTime, Utc};

/// Human-readable schedule label.
/// - one-shot absolute (`run_at` present) → `Once at <run_at>`
/// - `@immediate` → `Immediately (next tick)`
/// - cron expression → `cron_descriptor` text, falling back to the raw string
pub(crate) fn describe(schedule: &str, run_at: Option<&str>) -> String {
    if let Some(run_at) = run_at {
        return format!("Once at {run_at}");
    }
    if schedule == "@immediate" {
        return "Immediately (next tick)".to_string();
    }
    match cron_descriptor::cronparser::cron_expression_descriptor::get_description_cron(schedule) {
        Ok(desc) => desc,
        Err(_) => schedule.to_string(),
    }
}

/// Next fire time from `now`.
/// - `run_at` present → that instant (absolute one-shot)
/// - `@immediate` → `None` (fires on the next reconcile tick; the label carries
///   the meaning)
/// - cron expression → `cron::Schedule::after(now).next()`
/// - unparseable / no future fire → `None`
pub(crate) fn next_run_at(
    schedule: &str,
    run_at: Option<&str>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if let Some(run_at) = run_at {
        return DateTime::parse_from_rfc3339(run_at)
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
    }
    if schedule == "@immediate" {
        return None;
    }
    let seven_field = format!("0 {} *", schedule.trim());
    let parsed = cron::Schedule::from_str(&seven_field).ok()?;
    parsed.after(&now).next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-06-01T09:00:00Z".parse().unwrap()
    }

    #[test]
    fn describe_returns_human_text_for_valid_cron() {
        let desc = describe("0 8 * * *", None);
        assert_ne!(desc, "0 8 * * *");
        assert!(!desc.is_empty());
        // Anchor on the time so a cron_descriptor regression that drops the hour
        // is caught, without pinning the library's exact phrasing.
        assert!(
            desc.contains("8:00"),
            "expected time in description, got {desc:?}"
        );
    }

    #[test]
    fn describe_falls_back_to_raw_for_unparseable() {
        assert_eq!(describe("not-a-cron", None), "not-a-cron");
    }

    #[test]
    fn describe_handles_run_at_and_immediate() {
        assert_eq!(
            describe("ignored", Some("2026-06-02T10:00:00Z")),
            "Once at 2026-06-02T10:00:00Z"
        );
        assert_eq!(describe("@immediate", None), "Immediately (next tick)");
    }

    #[test]
    fn next_run_at_computes_next_cron_fire() {
        // 08:00 daily, now is 09:00 on 2026-06-01 → next fire is 2026-06-02T08:00:00Z.
        let next = next_run_at("0 8 * * *", None, now()).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-06-02T08:00:00+00:00");
    }

    #[test]
    fn next_run_at_uses_run_at_when_present() {
        let next = next_run_at("ignored", Some("2026-06-02T10:00:00Z"), now()).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-06-02T10:00:00+00:00");
    }

    #[test]
    fn next_run_at_is_none_for_immediate_and_unparseable() {
        assert!(next_run_at("@immediate", None, now()).is_none());
        assert!(next_run_at("not-a-cron", None, now()).is_none());
    }
}
