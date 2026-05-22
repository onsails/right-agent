//! `.usage.json` atomic R/W for skill lifecycle tracking.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LifecycleState {
    Active,
    Stale,
    Archived,
}

impl Default for LifecycleState {
    fn default() -> Self {
        Self::Active
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CreatedBy {
    Foreground,
    ProbeWriter,
    Curator,
    Bundled,
}

impl Default for CreatedBy {
    fn default() -> Self {
        Self::Foreground
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub(crate) struct UsageRecord {
    #[serde(default)]
    pub use_count: u64,
    #[serde(default)]
    pub patch_count: u64,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub last_patched_at: Option<String>,
    #[serde(default)]
    pub state: LifecycleState,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub created_by: CreatedBy,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub archived_at: Option<String>,
    #[serde(default)]
    pub absorbed_into: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Index {
    #[serde(default, flatten)]
    pub skills: BTreeMap<String, UsageRecord>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UsageError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub(crate) fn read_index(path: &Path) -> Result<Index, UsageError> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(Index::default()),
        Ok(s) => Ok(serde_json::from_str(&s)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::default()),
        Err(e) => Err(UsageError::Io(e)),
    }
}

pub(crate) fn write_index(path: &Path, index: &Index) -> Result<(), UsageError> {
    use fs4::FileExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = path.with_extension("lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    lock_file.lock()?;

    let tmp_path = path.with_extension("tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)?;
        let body = serde_json::to_vec_pretty(index)?;
        f.write_all(&body)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, path)?;

    let _ = FileExt::unlock(&lock_file);
    Ok(())
}

fn mutate<F>(path: &Path, mutate_fn: F) -> Result<(), UsageError>
where
    F: FnOnce(&mut Index),
{
    let mut index = read_index(path)?;
    mutate_fn(&mut index);
    write_index(path, &index)
}

pub(crate) fn bump_use(path: &Path, skill_name: &str, now_utc: &str) -> Result<(), UsageError> {
    mutate(path, |idx| {
        let r = idx.skills.entry(skill_name.to_owned()).or_default();
        r.use_count += 1;
        r.last_used_at = Some(now_utc.to_owned());
        if r.state == LifecycleState::Stale {
            r.state = LifecycleState::Active;
        }
    })
}

pub(crate) fn bump_patch(path: &Path, skill_name: &str, now_utc: &str) -> Result<(), UsageError> {
    mutate(path, |idx| {
        let r = idx.skills.entry(skill_name.to_owned()).or_default();
        r.patch_count += 1;
        r.last_patched_at = Some(now_utc.to_owned());
    })
}

pub(crate) fn mark_created(
    path: &Path,
    skill_name: &str,
    created_by: CreatedBy,
    now_utc: &str,
) -> Result<(), UsageError> {
    mutate(path, |idx| {
        let r = idx.skills.entry(skill_name.to_owned()).or_default();
        r.created_by = created_by;
        r.created_at = Some(now_utc.to_owned());
        r.state = LifecycleState::Active;
    })
}

pub(crate) fn mark_archived(
    path: &Path,
    skill_name: &str,
    absorbed_into: Option<&str>,
    now_utc: &str,
) -> Result<(), UsageError> {
    mutate(path, |idx| {
        let r = idx.skills.entry(skill_name.to_owned()).or_default();
        r.state = LifecycleState::Archived;
        r.archived_at = Some(now_utc.to_owned());
        r.absorbed_into = absorbed_into.map(str::to_owned);
    })
}

pub(crate) fn set_pinned(path: &Path, skill_name: &str, pinned: bool) -> Result<(), UsageError> {
    mutate(path, |idx| {
        let r = idx.skills.entry(skill_name.to_owned()).or_default();
        r.pinned = pinned;
    })
}

/// Count skills whose `created_at` OR `last_patched_at` is strictly after
/// `since`. Used by the curator's skill-change-count trigger.
///
/// Both `since` and the index timestamps may use different RFC 3339 formats
/// (e.g. `"2026-05-22T13:45:30.123456789+00:00"` vs `"2026-05-22T13:45:30Z"`).
/// We parse every timestamp as `DateTime<Utc>` to avoid lexicographic
/// mis-ordering at the fractional-seconds / `Z` boundary.
///
/// `since` empty or unparseable → treated as the Unix epoch (every row counts).
/// An index timestamp that fails to parse → treated as "before since" (not counted).
pub(crate) fn count_changes_since(index: &Index, since: &str) -> u32 {
    use chrono::{DateTime, Utc};

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

    let parse_ts = |s: &str| -> Option<DateTime<Utc>> {
        match DateTime::parse_from_rfc3339(s) {
            Ok(dt) => Some(dt.to_utc()),
            Err(e) => {
                tracing::warn!(
                    "count_changes_since: unparseable index timestamp {:?}: {e:#}",
                    s
                );
                None
            }
        }
    };

    let mut n = 0u32;
    for r in index.skills.values() {
        let created_after = r
            .created_at
            .as_deref()
            .and_then(parse_ts)
            .map(|dt| dt > since_dt)
            .unwrap_or(false);
        let patched_after = r
            .last_patched_at
            .as_deref()
            .and_then(parse_ts)
            .map(|dt| dt > since_dt)
            .unwrap_or(false);
        if created_after || patched_after {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_record_default_is_active_foreground() {
        let r = UsageRecord::default();
        assert_eq!(r.use_count, 0);
        assert_eq!(r.state, LifecycleState::Active);
        assert_eq!(r.created_by, CreatedBy::Foreground);
        assert!(!r.pinned);
        assert!(r.last_used_at.is_none());
    }

    #[test]
    fn write_and_read_empty_index_returns_empty_map() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".usage.json");
        let index = Index::default();
        write_index(&path, &index).unwrap();
        let loaded = read_index(&path).unwrap();
        assert!(loaded.skills.is_empty());
    }

    #[test]
    fn write_and_read_one_skill_roundtrips() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".usage.json");
        let mut index = Index::default();
        index.skills.insert(
            "rightx-foo".to_owned(),
            UsageRecord {
                use_count: 5,
                created_by: CreatedBy::ProbeWriter,
                last_used_at: Some("2026-05-22T10:00:00Z".to_owned()),
                ..UsageRecord::default()
            },
        );
        write_index(&path, &index).unwrap();
        let loaded = read_index(&path).unwrap();
        let r = loaded.skills.get("rightx-foo").unwrap();
        assert_eq!(r.use_count, 5);
        assert_eq!(r.created_by, CreatedBy::ProbeWriter);
        assert_eq!(r.last_used_at.as_deref(), Some("2026-05-22T10:00:00Z"));
    }

    #[test]
    fn read_missing_index_returns_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".usage.json");
        let loaded = read_index(&path).unwrap();
        assert!(loaded.skills.is_empty());
    }

    #[test]
    fn write_is_atomic_via_tempfile_rename() {
        use std::io::Read;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".usage.json");
        std::fs::write(&path, "PARTIAL").unwrap();
        let mut index = Index::default();
        index.skills.insert(
            "rightx-foo".to_owned(),
            UsageRecord {
                use_count: 1,
                ..UsageRecord::default()
            },
        );
        write_index(&path, &index).unwrap();
        let mut content = String::new();
        std::fs::File::open(&path)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(
            content.starts_with('{'),
            "atomic write must replace the entire file, not append"
        );
        assert!(
            content.contains("\"use_count\": 1"),
            "new content must be present"
        );
    }

    fn now_utc() -> String {
        "2026-05-22T12:00:00Z".to_owned()
    }

    #[test]
    fn bump_use_increments_count_and_sets_timestamp() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".usage.json");
        bump_use(&path, "rightx-foo", &now_utc()).unwrap();
        let idx = read_index(&path).unwrap();
        let r = idx.skills.get("rightx-foo").unwrap();
        assert_eq!(r.use_count, 1);
        assert_eq!(r.last_used_at.as_deref(), Some(now_utc().as_str()));
        bump_use(&path, "rightx-foo", &now_utc()).unwrap();
        let r2 = read_index(&path)
            .unwrap()
            .skills
            .remove("rightx-foo")
            .unwrap();
        assert_eq!(r2.use_count, 2);
    }

    #[test]
    fn bump_use_creates_record_if_absent_with_foreground_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".usage.json");
        bump_use(&path, "rightx-new", &now_utc()).unwrap();
        let r = read_index(&path)
            .unwrap()
            .skills
            .remove("rightx-new")
            .unwrap();
        assert_eq!(r.created_by, CreatedBy::Foreground);
        assert_eq!(r.state, LifecycleState::Active);
    }

    #[test]
    fn bump_patch_increments_patch_count_and_timestamp() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".usage.json");
        bump_patch(&path, "rightx-foo", &now_utc()).unwrap();
        let r = read_index(&path)
            .unwrap()
            .skills
            .remove("rightx-foo")
            .unwrap();
        assert_eq!(r.patch_count, 1);
        assert_eq!(r.last_patched_at.as_deref(), Some(now_utc().as_str()));
    }

    #[test]
    fn mark_created_sets_created_by_and_created_at() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".usage.json");
        mark_created(&path, "rightx-foo", CreatedBy::ProbeWriter, &now_utc()).unwrap();
        let r = read_index(&path)
            .unwrap()
            .skills
            .remove("rightx-foo")
            .unwrap();
        assert_eq!(r.created_by, CreatedBy::ProbeWriter);
        assert_eq!(r.created_at.as_deref(), Some(now_utc().as_str()));
        assert_eq!(r.state, LifecycleState::Active);
    }

    #[test]
    fn mark_archived_sets_state_and_optional_absorbed_into() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".usage.json");
        mark_created(&path, "rightx-old", CreatedBy::ProbeWriter, &now_utc()).unwrap();
        mark_archived(&path, "rightx-old", Some("rightx-umbrella"), &now_utc()).unwrap();
        let r = read_index(&path)
            .unwrap()
            .skills
            .remove("rightx-old")
            .unwrap();
        assert_eq!(r.state, LifecycleState::Archived);
        assert_eq!(r.archived_at.as_deref(), Some(now_utc().as_str()));
        assert_eq!(r.absorbed_into.as_deref(), Some("rightx-umbrella"));
    }

    #[test]
    fn set_pinned_toggles_pinned_flag() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".usage.json");
        mark_created(&path, "rightx-foo", CreatedBy::ProbeWriter, &now_utc()).unwrap();
        set_pinned(&path, "rightx-foo", true).unwrap();
        assert!(read_index(&path).unwrap().skills["rightx-foo"].pinned);
        set_pinned(&path, "rightx-foo", false).unwrap();
        assert!(!read_index(&path).unwrap().skills["rightx-foo"].pinned);
    }
}

#[cfg(test)]
mod count_tests {
    use super::*;

    #[test]
    fn counts_skills_created_after_since() {
        let mut idx = Index::default();
        let mut r = UsageRecord::default();
        r.created_at = Some("2026-05-22T12:00:00Z".into());
        idx.skills.insert("rightx-new".into(), r);
        let mut older = UsageRecord::default();
        older.created_at = Some("2026-05-20T12:00:00Z".into());
        idx.skills.insert("rightx-old".into(), older);
        assert_eq!(count_changes_since(&idx, "2026-05-21T00:00:00Z"), 1);
    }

    #[test]
    fn counts_skills_patched_after_since() {
        let mut idx = Index::default();
        let mut r = UsageRecord::default();
        r.created_at = Some("2026-04-01T00:00:00Z".into());
        r.last_patched_at = Some("2026-05-22T12:00:00Z".into());
        idx.skills.insert("rightx-patched".into(), r);
        assert_eq!(count_changes_since(&idx, "2026-05-21T00:00:00Z"), 1);
    }

    /// Regression: `since` is in RFC 3339 format with a fractional-seconds
    /// `+00:00` suffix (as written by `Utc::now().to_rfc3339()`), while index
    /// timestamps use the `"%Y-%m-%dT%H:%M:%SZ"` format.  At byte position 19
    /// `Z` (0x5A) sorts higher than `.` (0x2E) and `+` (0x2B), so a naïve
    /// lexicographic comparison incorrectly treats a skill timestamped BEFORE
    /// `since` as coming after it.
    #[test]
    fn count_changes_since_mixed_rfc3339_formats_no_false_positive() {
        // `since` uses the rfc3339() format with nanoseconds + offset.
        let since = "2026-05-22T13:45:30.123456789+00:00";

        let mut idx = Index::default();

        // Skill created 1 second BEFORE `since` — must NOT be counted.
        let mut before = UsageRecord::default();
        before.created_at = Some("2026-05-22T13:45:29Z".into()); // Z format, 1s earlier
        idx.skills.insert("rightx-before".into(), before);

        assert_eq!(
            count_changes_since(&idx, since),
            0,
            "skill created before `since` must not be counted (was producing 1 with byte-cmp)"
        );

        // Skill created 1 second AFTER `since` — must be counted.
        let mut after = UsageRecord::default();
        after.created_at = Some("2026-05-22T13:45:31Z".into()); // Z format, 1s later
        idx.skills.insert("rightx-after".into(), after);

        assert_eq!(
            count_changes_since(&idx, since),
            1,
            "only the skill created after `since` should be counted"
        );
    }
}
