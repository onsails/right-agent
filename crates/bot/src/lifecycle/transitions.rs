//! Pure-Rust lifecycle state machine over `.usage.json`.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use chrono::{DateTime, Duration, Utc};

use super::usage::{Index, LifecycleState, UsageRecord};

#[derive(Debug, Clone, Copy)]
pub(crate) struct TransitionConfig {
    pub stale_after_days: i64,
    pub archive_after_days: i64,
}

impl Default for TransitionConfig {
    fn default() -> Self {
        Self {
            stale_after_days: 30,
            archive_after_days: 90,
        }
    }
}

/// Apply staleness/archive transitions in-place. Returns count of records that changed state.
pub(crate) fn apply_automatic_transitions(
    index: &mut Index,
    now: DateTime<Utc>,
    config: TransitionConfig,
) -> usize {
    let mut changed = 0;
    for record in index.skills.values_mut() {
        if record.pinned {
            continue;
        }
        if record.state == LifecycleState::Archived {
            continue;
        }
        let latest = latest_activity_at(record);
        let Some(latest) = latest else {
            continue;
        };
        let age = now.signed_duration_since(latest);
        let new_state = if age > Duration::days(config.archive_after_days) {
            LifecycleState::Archived
        } else if age > Duration::days(config.stale_after_days) {
            LifecycleState::Stale
        } else {
            LifecycleState::Active
        };
        if new_state != record.state {
            let became_archived = matches!(new_state, LifecycleState::Archived);
            record.state = new_state;
            if became_archived {
                record.archived_at = Some(now.format("%Y-%m-%dT%H:%M:%SZ").to_string());
            }
            changed += 1;
        }
    }
    changed
}

fn latest_activity_at(r: &UsageRecord) -> Option<DateTime<Utc>> {
    let parse = |s: &str| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    };
    let used = r.last_used_at.as_deref().and_then(parse);
    let patched = r.last_patched_at.as_deref().and_then(parse);
    match (used, patched) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::usage::CreatedBy;
    use super::*;
    use std::collections::BTreeMap;

    fn record_with_used(name: &str, last_used: &str) -> (String, UsageRecord) {
        (
            name.to_owned(),
            UsageRecord {
                last_used_at: Some(last_used.to_owned()),
                created_by: CreatedBy::ProbeWriter,
                ..UsageRecord::default()
            },
        )
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-22T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn active_skill_within_threshold_stays_active() {
        let (n, r) = record_with_used("rightx-fresh", "2026-05-21T00:00:00Z");
        let mut idx = Index {
            skills: BTreeMap::from([(n, r)]),
        };
        let changed = apply_automatic_transitions(&mut idx, now(), TransitionConfig::default());
        assert_eq!(changed, 0);
        assert_eq!(idx.skills["rightx-fresh"].state, LifecycleState::Active);
    }

    #[test]
    fn skill_unused_30_days_becomes_stale() {
        let (n, r) = record_with_used("rightx-aged", "2026-04-21T00:00:00Z");
        let mut idx = Index {
            skills: BTreeMap::from([(n, r)]),
        };
        let changed = apply_automatic_transitions(&mut idx, now(), TransitionConfig::default());
        assert_eq!(changed, 1);
        assert_eq!(idx.skills["rightx-aged"].state, LifecycleState::Stale);
    }

    #[test]
    fn skill_unused_90_days_becomes_archived() {
        let (n, r) = record_with_used("rightx-ancient", "2026-02-20T00:00:00Z");
        let mut idx = Index {
            skills: BTreeMap::from([(n, r)]),
        };
        let changed = apply_automatic_transitions(&mut idx, now(), TransitionConfig::default());
        assert_eq!(changed, 1);
        assert_eq!(idx.skills["rightx-ancient"].state, LifecycleState::Archived);
        assert!(idx.skills["rightx-ancient"].archived_at.is_some());
    }

    #[test]
    fn pinned_skill_is_never_transitioned() {
        let mut r = UsageRecord {
            last_used_at: Some("2026-01-01T00:00:00Z".to_owned()),
            pinned: true,
            created_by: CreatedBy::ProbeWriter,
            ..UsageRecord::default()
        };
        r.state = LifecycleState::Active;
        let mut idx = Index {
            skills: BTreeMap::from([("rightx-pinned".to_owned(), r)]),
        };
        let changed = apply_automatic_transitions(&mut idx, now(), TransitionConfig::default());
        assert_eq!(changed, 0);
        assert_eq!(idx.skills["rightx-pinned"].state, LifecycleState::Active);
    }

    #[test]
    fn already_archived_skill_is_not_re_transitioned() {
        let mut r = UsageRecord {
            last_used_at: Some("2026-02-20T00:00:00Z".to_owned()),
            archived_at: Some("2026-03-01T00:00:00Z".to_owned()),
            created_by: CreatedBy::ProbeWriter,
            ..UsageRecord::default()
        };
        r.state = LifecycleState::Archived;
        let mut idx = Index {
            skills: BTreeMap::from([("rightx-old".to_owned(), r)]),
        };
        let changed = apply_automatic_transitions(&mut idx, now(), TransitionConfig::default());
        assert_eq!(changed, 0);
        assert_eq!(
            idx.skills["rightx-old"].archived_at.as_deref(),
            Some("2026-03-01T00:00:00Z")
        );
    }

    #[test]
    fn latest_activity_uses_max_of_used_and_patched() {
        let (n, mut r) = record_with_used("rightx-mixed", "2026-04-21T00:00:00Z");
        r.last_patched_at = Some("2026-05-15T00:00:00Z".to_owned());
        let mut idx = Index {
            skills: BTreeMap::from([(n, r)]),
        };
        let changed = apply_automatic_transitions(&mut idx, now(), TransitionConfig::default());
        assert_eq!(
            changed, 0,
            "recent patch keeps skill active even if last_used_at is old"
        );
        assert_eq!(idx.skills["rightx-mixed"].state, LifecycleState::Active);
    }

    #[test]
    fn record_with_no_activity_at_all_is_not_transitioned() {
        let r = UsageRecord {
            created_by: CreatedBy::ProbeWriter,
            ..UsageRecord::default()
        };
        let mut idx = Index {
            skills: BTreeMap::from([("rightx-empty".to_owned(), r)]),
        };
        let changed = apply_automatic_transitions(&mut idx, now(), TransitionConfig::default());
        assert_eq!(changed, 0);
    }
}
