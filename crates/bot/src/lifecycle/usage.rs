//! DB-backed skill lifecycle helpers used by bot-side orchestration.

use chrono::{DateTime, Utc};

pub(crate) fn list(
    conn: &rusqlite::Connection,
) -> right_lifecycle::Result<Vec<right_lifecycle::SkillLifecycleRow>> {
    right_lifecycle::list(conn)
}

/// Count skills whose `created_at` OR `last_patched_at` is strictly after
/// `since`. Used by the curator's skill-change-count trigger.
///
/// `since` empty or unparseable is treated as the Unix epoch. Row timestamps
/// are already parsed by `right_lifecycle`, so malformed DB rows surface before
/// this helper runs.
pub(crate) fn count_changes_since(rows: &[right_lifecycle::SkillLifecycleRow], since: &str) -> u32 {
    let since_dt: DateTime<Utc> = if since.is_empty() {
        DateTime::UNIX_EPOCH
    } else {
        match DateTime::parse_from_rfc3339(since) {
            Ok(dt) => dt.to_utc(),
            Err(e) => {
                tracing::warn!(
                    "count_changes_since: unparseable `since` {:?}: {e:#}",
                    since
                );
                DateTime::UNIX_EPOCH
            }
        }
    };

    rows.iter()
        .filter(|row| {
            let created_after = row.created_at.map(|dt| dt > since_dt).unwrap_or(false);
            let patched_after = row.last_patched_at.map(|dt| dt > since_dt).unwrap_or(false);
            created_after || patched_after
        })
        .count() as u32
}

#[cfg(test)]
mod count_tests {
    use super::*;

    fn utc(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn row(
        skill_name: &str,
        created_at: Option<&str>,
        last_patched_at: Option<&str>,
    ) -> right_lifecycle::SkillLifecycleRow {
        right_lifecycle::SkillLifecycleRow {
            skill_name: skill_name.to_owned(),
            state: right_lifecycle::LifecycleState::Active,
            pinned: false,
            created_by: right_lifecycle::CreatedBy::ProbeWriter,
            use_count: 0,
            patch_count: 0,
            created_at: created_at.map(utc),
            last_used_at: None,
            last_patched_at: last_patched_at.map(utc),
            archived_at: None,
            absorbed_into: None,
        }
    }

    #[test]
    fn counts_skills_created_after_since() {
        let rows = vec![
            row("rightx-old", Some("2026-05-20T00:00:00Z"), None),
            row("rightx-new", Some("2026-05-22T00:00:00Z"), None),
        ];
        assert_eq!(count_changes_since(&rows, "2026-05-21T00:00:00Z"), 1);
    }

    #[test]
    fn counts_skills_patched_after_since() {
        let rows = vec![row(
            "rightx-patched",
            Some("2026-05-01T00:00:00Z"),
            Some("2026-05-22T00:00:00Z"),
        )];
        assert_eq!(count_changes_since(&rows, "2026-05-21T00:00:00Z"), 1);
    }

    #[test]
    fn mixed_rfc3339_since_formats_do_not_create_false_positive() {
        let rows = vec![row(
            "rightx-same-second",
            Some("2026-05-21T00:00:00Z"),
            None,
        )];
        let since = "2026-05-21T00:00:00.123456789+00:00";
        assert_eq!(
            count_changes_since(&rows, since),
            0,
            "same-second Z timestamp before fractional since must not count"
        );
    }
}
