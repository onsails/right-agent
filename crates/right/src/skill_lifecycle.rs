//! Minimal client for `.usage.json` skill-lifecycle state.
//!
//! The authoritative reader/writer lives in `right-bot::lifecycle::usage`; this
//! module duplicates the on-disk schema and atomic-write contract so the `right`
//! MCP backend can update lifecycle state from `skill_learning_finish` without
//! a cross-crate dependency. Keep the JSON shape in sync with
//! `crates/bot/src/lifecycle/usage.rs`.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use fs4::FileExt;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleState {
    #[serde(other)]
    Active,
}

impl Default for LifecycleState {
    fn default() -> Self {
        Self::Active
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CreatedBy {
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
struct UsageRecord {
    #[serde(default)]
    use_count: u64,
    #[serde(default)]
    patch_count: u64,
    #[serde(default)]
    last_used_at: Option<String>,
    #[serde(default)]
    last_patched_at: Option<String>,
    #[serde(default)]
    state: LifecycleState,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    created_by: CreatedBy,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    archived_at: Option<String>,
    #[serde(default)]
    absorbed_into: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Index {
    #[serde(default, flatten)]
    skills: BTreeMap<String, UsageRecord>,
}

#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

fn read_index(path: &Path) -> Result<Index, UsageError> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(Index::default()),
        Ok(s) => Ok(serde_json::from_str(&s)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::default()),
        Err(e) => Err(UsageError::Io(e)),
    }
}

fn write_index(path: &Path, index: &Index) -> Result<(), UsageError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = path.with_extension("lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    FileExt::lock(&lock_file)?;

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

/// Mark a skill as created by a foreground (explicit-intent) write.
pub fn mark_created_foreground(
    path: &Path,
    skill_name: &str,
    now_utc: &str,
) -> Result<(), UsageError> {
    mutate(path, |idx| {
        let r = idx.skills.entry(skill_name.to_owned()).or_default();
        r.created_by = CreatedBy::Foreground;
        r.created_at = Some(now_utc.to_owned());
        r.state = LifecycleState::Active;
    })
}

/// Increment patch_count and refresh `last_patched_at`.
pub fn bump_patch(path: &Path, skill_name: &str, now_utc: &str) -> Result<(), UsageError> {
    mutate(path, |idx| {
        let r = idx.skills.entry(skill_name.to_owned()).or_default();
        r.patch_count += 1;
        r.last_patched_at = Some(now_utc.to_owned());
    })
}
