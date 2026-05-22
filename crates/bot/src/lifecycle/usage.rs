//! `.usage.json` atomic R/W for skill lifecycle tracking.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

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
}
