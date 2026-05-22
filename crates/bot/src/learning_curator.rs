//! Periodic skill curator: backup + automatic transitions + LLM consolidation pass.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct CuratorState {
    pub last_run_at: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CuratorConfig {
    pub enabled: bool,
    pub paused: bool,
    pub interval_hours: u32,
    pub min_idle_hours: u32,
    pub stale_after_days: u32,
    pub archive_after_days: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CuratorGateDecision {
    Run,
    SkipDisabled,
    SkipPaused,
    SkipIntervalNotElapsed,
    SkipChatNotIdle,
}

/// Pure gate decision. No I/O.
pub(crate) fn should_run_now(
    config: CuratorConfig,
    state: &CuratorState,
    now: DateTime<Utc>,
    latest_user_activity_at: Option<DateTime<Utc>>,
) -> CuratorGateDecision {
    if !config.enabled {
        return CuratorGateDecision::SkipDisabled;
    }
    if config.paused {
        return CuratorGateDecision::SkipPaused;
    }
    if let Some(last) = state.last_run_at.as_deref() {
        if let Ok(last_dt) = DateTime::parse_from_rfc3339(last) {
            let last_dt = last_dt.with_timezone(&Utc);
            if now - last_dt < Duration::hours(config.interval_hours as i64) {
                return CuratorGateDecision::SkipIntervalNotElapsed;
            }
        }
    } else {
        // First-ever run: caller seeds last_run_at and defers one interval.
        return CuratorGateDecision::SkipIntervalNotElapsed;
    }
    if let Some(latest) = latest_user_activity_at
        && now - latest < Duration::hours(config.min_idle_hours as i64)
    {
        return CuratorGateDecision::SkipChatNotIdle;
    }
    CuratorGateDecision::Run
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CuratorConfig {
        CuratorConfig {
            enabled: true,
            paused: false,
            interval_hours: 168,
            min_idle_hours: 2,
            stale_after_days: 30,
            archive_after_days: 90,
        }
    }

    fn dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn disabled_skips() {
        let mut c = cfg();
        c.enabled = false;
        assert_eq!(
            should_run_now(
                c,
                &CuratorState::default(),
                dt("2026-05-22T00:00:00Z"),
                None
            ),
            CuratorGateDecision::SkipDisabled
        );
    }

    #[test]
    fn paused_skips() {
        let mut c = cfg();
        c.paused = true;
        assert_eq!(
            should_run_now(
                c,
                &CuratorState::default(),
                dt("2026-05-22T00:00:00Z"),
                None
            ),
            CuratorGateDecision::SkipPaused
        );
    }

    #[test]
    fn first_run_defers_one_interval() {
        let state = CuratorState { last_run_at: None };
        assert_eq!(
            should_run_now(cfg(), &state, dt("2026-05-22T00:00:00Z"), None),
            CuratorGateDecision::SkipIntervalNotElapsed
        );
    }

    #[test]
    fn within_interval_skips() {
        let state = CuratorState {
            last_run_at: Some("2026-05-21T00:00:00Z".to_owned()),
        };
        assert_eq!(
            should_run_now(cfg(), &state, dt("2026-05-22T00:00:00Z"), None),
            CuratorGateDecision::SkipIntervalNotElapsed
        );
    }

    #[test]
    fn after_interval_runs_when_idle() {
        let state = CuratorState {
            last_run_at: Some("2026-05-01T00:00:00Z".to_owned()),
        };
        assert_eq!(
            should_run_now(cfg(), &state, dt("2026-05-22T00:00:00Z"), None),
            CuratorGateDecision::Run
        );
    }

    #[test]
    fn chat_active_within_min_idle_skips() {
        let state = CuratorState {
            last_run_at: Some("2026-05-01T00:00:00Z".to_owned()),
        };
        let now = dt("2026-05-22T00:00:00Z");
        let just_now = now - Duration::minutes(30);
        assert_eq!(
            should_run_now(cfg(), &state, now, Some(just_now)),
            CuratorGateDecision::SkipChatNotIdle
        );
    }
}
