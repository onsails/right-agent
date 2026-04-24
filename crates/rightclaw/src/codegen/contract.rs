//! Codegen output contract.
//!
//! Every file written by codegen belongs to exactly one [`CodegenKind`]. The
//! helpers in this module are the only sanctioned writers for codegen files.
//! Direct `std::fs::write` inside `codegen/*` modules is a review-blocking
//! defect after this module lands.
//!
//! See `docs/superpowers/specs/2026-04-24-upgrade-migration-model-design.md`.

use std::path::{Path, PathBuf};

/// Category of a codegen output. Drives how changes propagate to running agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodegenKind {
    /// Unconditional overwrite on every bot start.
    Regenerated(HotReload),
    /// Read existing, merge codegen fields in, write back. Preserves unknown fields.
    MergedRMW,
    /// Created by init with an initial payload. Never touched by codegen again.
    AgentOwned,
}

/// How a `Regenerated` change reaches a running sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotReload {
    /// Takes effect on next CC invocation. No sandbox RPC needed.
    BotRestart,
    /// Applied via `openshell policy set --wait` after write. Network-only.
    SandboxPolicyApply,
    /// Boot-time-only (landlock, filesystem). Requires sandbox migration.
    SandboxRecreate,
}

/// An entry in the codegen registry.
#[derive(Debug, Clone)]
pub struct CodegenFile {
    pub kind: CodegenKind,
    pub path: PathBuf,
}

/// Unconditional write — the sanctioned writer for
/// `Regenerated(BotRestart)` and `Regenerated(SandboxRecreate)` outputs.
///
/// `Regenerated(SandboxPolicyApply)` outputs MUST go through
/// [`write_and_apply_sandbox_policy`] instead — there is no other writer for
/// that category, so callers cannot skip `apply_policy`.
///
/// Creates parent directories if absent.
pub fn write_regenerated(path: &Path, content: &str) -> miette::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            miette::miette!("failed to create parent dir for {}: {e:#}", path.display())
        })?;
    }
    std::fs::write(path, content)
        .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))
}

/// No-op if the file exists. Otherwise writes `initial`, creating parent
/// directories as needed. The sanctioned writer for `AgentOwned` outputs.
pub fn write_agent_owned(path: &Path, initial: &str) -> miette::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            miette::miette!("failed to create parent dir for {}: {e:#}", path.display())
        })?;
    }
    std::fs::write(path, initial)
        .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_regenerated_overwrites_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sub/file.txt");
        write_regenerated(&path, "first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        write_regenerated(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    }

    #[test]
    fn write_regenerated_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a/b/c/file.txt");
        write_regenerated(&path, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn write_agent_owned_creates_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("TOOLS.md");
        write_agent_owned(&path, "# default").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# default");
    }

    #[test]
    fn write_agent_owned_preserves_when_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("TOOLS.md");
        std::fs::write(&path, "agent-edited content").unwrap();
        write_agent_owned(&path, "# default (should be ignored)").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "agent-edited content");
    }
}
