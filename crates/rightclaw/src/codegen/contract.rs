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

/// Read-modify-write. `merge_fn` receives `Some(existing)` if the file is
/// present, `None` otherwise, and returns the final content. Merger must
/// preserve unknown fields.
///
/// The sanctioned writer for `MergedRMW` outputs.
pub fn write_merged_rmw<F>(path: &Path, merge_fn: F) -> miette::Result<()>
where
    F: FnOnce(Option<&str>) -> miette::Result<String>,
{
    let existing = if path.exists() {
        Some(std::fs::read_to_string(path).map_err(|e| {
            miette::miette!("failed to read {} for merge: {e:#}", path.display())
        })?)
    } else {
        None
    };
    let content = merge_fn(existing.as_deref())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            miette::miette!("failed to create parent dir for {}: {e:#}", path.display())
        })?;
    }
    std::fs::write(path, content)
        .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))
}

/// The ONLY way to update policy for a running sandbox. Writes `content` to
/// `path`, then applies it via `openshell policy set --wait`. Network-only
/// policy changes hot-reload; filesystem changes require sandbox migration
/// (handled separately by `maybe_migrate_sandbox`).
pub async fn write_and_apply_sandbox_policy(
    sandbox: &str,
    path: &Path,
    content: &str,
) -> miette::Result<()> {
    write_regenerated(path, content)?;
    crate::openshell::apply_policy(sandbox, path).await
}

/// Per-agent codegen outputs. Source of truth for guard tests.
///
/// Every file produced by [`crate::codegen::run_single_agent_codegen`] MUST
/// appear here or in the documented `KNOWN_EXCEPTIONS` inside
/// `registry_covers_all_per_agent_writes`.
pub fn codegen_registry(agent_dir: &Path) -> Vec<CodegenFile> {
    let claude = agent_dir.join(".claude");
    vec![
        CodegenFile {
            kind: CodegenKind::MergedRMW,
            path: agent_dir.join("agent.yaml"),
        },
        CodegenFile {
            kind: CodegenKind::MergedRMW,
            path: agent_dir.join(".claude.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: agent_dir.join("mcp.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::SandboxRecreate),
            path: agent_dir.join("policy.yaml"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("settings.json"),
        },
        CodegenFile {
            kind: CodegenKind::AgentOwned,
            path: claude.join("settings.local.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("reply-schema.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("cron-schema.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("bootstrap-schema.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("system-prompt.md"),
        },
        // Skills are registered as a single tree-rooted entry; the installer
        // manages content-addressed files beneath.
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("skills"),
        },
    ]
}

/// Cross-agent codegen outputs under `<home>/run/` and peers.
pub fn crossagent_codegen_registry(home: &Path) -> Vec<CodegenFile> {
    let run = home.join("run");
    vec![
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: run.join("process-compose.yaml"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: run.join("agent-tokens.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: home.join("scripts").join("cloudflared-start.sh"),
        },
        // cloudflared config path is added in Phase 4 Task 16 when the
        // cross-agent refactor lands — leaving a stub here would become stale
        // if the filename changes.
    ]
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

    #[test]
    fn write_merged_rmw_passes_existing_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"a":1}"#).unwrap();
        write_merged_rmw(&path, |existing| {
            let existing = existing.expect("file should exist");
            assert_eq!(existing, r#"{"a":1}"#);
            Ok(format!("{}+merged", existing))
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"a":1}+merged"#);
    }

    #[test]
    fn write_merged_rmw_passes_none_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("new.json");
        write_merged_rmw(&path, |existing| {
            assert!(existing.is_none());
            Ok("{}".to_owned())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    }

    #[test]
    fn write_merged_rmw_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested/new.json");
        write_merged_rmw(&path, |_| Ok("{}".to_owned())).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn codegen_registry_has_all_expected_categories() {
        let dir = tempdir().unwrap();
        let reg = codegen_registry(dir.path());
        assert!(reg.iter().any(|f| matches!(f.kind, CodegenKind::MergedRMW)));
        assert!(reg.iter().any(|f| matches!(f.kind, CodegenKind::AgentOwned)));
        assert!(reg.iter().any(|f| matches!(f.kind,
            CodegenKind::Regenerated(HotReload::BotRestart))));
        assert!(reg.iter().any(|f| matches!(f.kind,
            CodegenKind::Regenerated(HotReload::SandboxRecreate))));
    }
}
