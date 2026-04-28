//! Codegen output contract.
//!
//! Every file written by codegen belongs to exactly one [`CodegenKind`]. The
//! helpers in this module are the only sanctioned writers for codegen files.
//! Direct `std::fs::write` inside `codegen/*` modules is a review-blocking
//! defect.

use std::path::{Path, PathBuf};

fn ensure_parent_dir(path: &Path) -> miette::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            miette::miette!("failed to create parent dir for {}: {e:#}", path.display())
        })?;
    }
    Ok(())
}

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
    ensure_parent_dir(path)?;
    std::fs::write(path, content)
        .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))
}

/// Byte-variant of [`write_regenerated`] for callers with non-UTF-8 content
/// (bundled binary assets, etc.). Identical semantics — unconditional
/// overwrite, creates parent directories.
pub fn write_regenerated_bytes(path: &Path, content: &[u8]) -> miette::Result<()> {
    ensure_parent_dir(path)?;
    std::fs::write(path, content)
        .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))
}

/// No-op if the file exists. Otherwise writes `initial`, creating parent
/// directories as needed. The sanctioned writer for `AgentOwned` outputs.
pub fn write_agent_owned(path: &Path, initial: &str) -> miette::Result<()> {
    if path.exists() {
        return Ok(());
    }
    ensure_parent_dir(path)?;
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
    ensure_parent_dir(path)?;
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
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: home.join("cloudflared-config.yml"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    use crate::agent::AgentDef;
    use std::path::PathBuf;

    fn minimal_agent_fixture(home: &Path, name: &str) -> AgentDef {
        let agent_path = home.join("agents").join(name);
        std::fs::create_dir_all(agent_path.join(".claude")).unwrap();
        std::fs::write(agent_path.join("IDENTITY.md"), "# Test Identity\n").unwrap();
        std::fs::write(
            agent_path.join("agent.yaml"),
            "restart: never\nnetwork_policy: permissive\nsandbox:\n  mode: none\n",
        )
        .unwrap();
        crate::agent::discover_single_agent(&agent_path).unwrap()
    }

    fn run_codegen_for(home: &Path, agent: &AgentDef) {
        let self_exe = PathBuf::from("/usr/local/bin/right");
        crate::codegen::run_single_agent_codegen(home, agent, &self_exe, false).unwrap();
    }

    fn sha256(path: &Path) -> String {
        use sha2::{Digest, Sha256};
        let bytes = std::fs::read(path).unwrap();
        let hash = Sha256::digest(&bytes);
        hash.iter().map(|b| format!("{b:02x}")).collect()
    }

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
    fn write_regenerated_bytes_overwrites_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sub/blob.bin");
        write_regenerated_bytes(&path, &[0u8, 1, 2, 0xff]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), vec![0u8, 1, 2, 0xff]);
        write_regenerated_bytes(&path, &[0xaa, 0xbb]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), vec![0xaa, 0xbb]);
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

    #[test]
    fn regenerated_files_are_idempotent() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_owned();
        let agent = minimal_agent_fixture(&home, "t1");

        run_codegen_for(&home, &agent);

        // Re-discover to pick up the persisted secret; otherwise the stale
        // in-memory `AgentDef` makes `ensure_agent_secret` mint a fresh one on
        // the second run, churning any file derived from it (mcp.json bearer).
        // This matches production: every bot start re-reads `agent.yaml`.
        let agent = crate::agent::discover_single_agent(&agent.path).unwrap();

        let reg = codegen_registry(&agent.path);
        let first: std::collections::HashMap<_, _> = reg
            .iter()
            .filter(|f| matches!(f.kind, CodegenKind::Regenerated(_)))
            .filter(|f| f.path.is_file())
            .map(|f| (f.path.clone(), sha256(&f.path)))
            .collect();

        run_codegen_for(&home, &agent);

        for (path, old_hash) in &first {
            let new_hash = sha256(path);
            assert_eq!(
                &new_hash, old_hash,
                "Regenerated file changed between codegen runs: {}",
                path.display(),
            );
        }
    }

    #[test]
    fn agent_owned_files_preserved_across_codegen() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_owned();
        let agent = minimal_agent_fixture(&home, "t2");

        let settings_local = agent.path.join(".claude/settings.local.json");
        std::fs::create_dir_all(settings_local.parent().unwrap()).unwrap();
        std::fs::write(&settings_local, r#"{"__AGENT__":true}"#).unwrap();

        run_codegen_for(&home, &agent);

        assert_eq!(
            std::fs::read_to_string(&settings_local).unwrap(),
            r#"{"__AGENT__":true}"#,
            "AgentOwned file settings.local.json was overwritten by codegen",
        );
    }

    #[test]
    fn merged_rmw_preserves_unknown_fields_in_claude_json() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_owned();
        let agent = minimal_agent_fixture(&home, "t3");

        let claude_json = agent.path.join(".claude.json");
        std::fs::write(
            &claude_json,
            r#"{"customField":"preserve-me","hasCompletedOnboarding":false}"#,
        )
        .unwrap();

        run_codegen_for(&home, &agent);

        let content = std::fs::read_to_string(&claude_json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["customField"], "preserve-me",
            "MergedRMW must preserve unknown fields"
        );
        assert!(parsed["hasCompletedOnboarding"].is_boolean());
    }

    /// Files that codegen (or its side effects) may create but that are
    /// intentionally outside the codegen contract. Keep this list tight —
    /// every entry is a gap in the upgrade story.
    const KNOWN_EXCEPTIONS: &[&str] = &[
        ".git",
        "data.db",
        "data.db-shm",
        "data.db-wal",
        "bot.sock",
        ".claude/shell-snapshots",
        ".claude/.credentials.json", // symlink, target owned by host
        "inbox",
        "outbox",
        "tmp",
    ];

    fn walk_files_rel(root: &Path, base: &Path, out: &mut Vec<PathBuf>) {
        if !root.exists() {
            return;
        }
        for entry in std::fs::read_dir(root).unwrap().flatten() {
            let p = entry.path();
            let rel = p.strip_prefix(base).unwrap().to_owned();
            if KNOWN_EXCEPTIONS.iter().any(|x| rel.starts_with(x)) {
                continue;
            }
            if p.is_dir() {
                walk_files_rel(&p, base, out);
            } else if p.is_file() || p.is_symlink() {
                out.push(rel);
            }
        }
    }

    #[test]
    fn registry_covers_all_per_agent_writes() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_owned();
        let agent = minimal_agent_fixture(&home, "t4");

        // Snapshot fixture seed files — they exist before codegen and are
        // not codegen outputs. Only files that appear *after* codegen count.
        let mut before = Vec::new();
        walk_files_rel(&agent.path, &agent.path, &mut before);
        let before: std::collections::HashSet<PathBuf> = before.into_iter().collect();

        run_codegen_for(&home, &agent);

        let reg_paths: std::collections::HashSet<PathBuf> = codegen_registry(&agent.path)
            .into_iter()
            .map(|f| f.path.strip_prefix(&agent.path).unwrap().to_owned())
            .collect();

        let mut found = Vec::new();
        walk_files_rel(&agent.path, &agent.path, &mut found);

        let uncovered: Vec<_> = found
            .into_iter()
            .filter(|rel| !before.contains(rel))
            .filter(|rel| !reg_paths.iter().any(|r| rel == r || rel.starts_with(r)))
            .collect();

        assert!(
            uncovered.is_empty(),
            "files produced by codegen not in registry or KNOWN_EXCEPTIONS: {:#?}",
            uncovered,
        );
    }

    /// Files that cross-agent codegen (or its side effects) may create under
    /// `<home>/` but that are intentionally outside the codegen contract.
    /// Keep tight — every entry is a gap in the upgrade story.
    const CROSSAGENT_KNOWN_EXCEPTIONS: &[&str] = &[
        // RuntimeState JSON — persisted runtime bookkeeping (pc_port, token,
        // started_at), not a codegen output. Written by pipeline but not a
        // file the upgrade model governs.
        "run/state.json",
    ];

    fn walk_crossagent_files_rel(root: &Path, base: &Path, out: &mut Vec<PathBuf>) {
        if !root.exists() {
            return;
        }
        for entry in std::fs::read_dir(root).unwrap().flatten() {
            let p = entry.path();
            let rel = p.strip_prefix(base).unwrap().to_owned();
            // Skip per-agent outputs — owned by the per-agent registry test.
            if rel.starts_with("agents") {
                continue;
            }
            if CROSSAGENT_KNOWN_EXCEPTIONS
                .iter()
                .any(|x| rel.starts_with(x))
            {
                continue;
            }
            if p.is_dir() {
                walk_crossagent_files_rel(&p, base, out);
            } else if p.is_file() || p.is_symlink() {
                out.push(rel);
            }
        }
    }

    #[test]
    fn registry_covers_all_crossagent_writes() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_owned();
        let agent = minimal_agent_fixture(&home, "t5");

        // Tunnel config is mandatory — write a minimal one before codegen.
        crate::codegen::pipeline::tests::write_minimal_global_config(&home);

        // Snapshot pre-existing files under home (excluding agents/) — only
        // files created by the codegen call count.
        let mut before = Vec::new();
        walk_crossagent_files_rel(&home, &home, &mut before);
        let before: std::collections::HashSet<PathBuf> = before.into_iter().collect();

        let self_exe = PathBuf::from("/usr/local/bin/right");
        crate::codegen::run_agent_codegen(&home, std::slice::from_ref(&agent), &self_exe, false)
            .unwrap();

        let reg_paths: std::collections::HashSet<PathBuf> = crossagent_codegen_registry(&home)
            .into_iter()
            .map(|f| f.path.strip_prefix(&home).unwrap().to_owned())
            .collect();

        let mut found = Vec::new();
        walk_crossagent_files_rel(&home, &home, &mut found);

        let uncovered: Vec<_> = found
            .into_iter()
            .filter(|rel| !before.contains(rel))
            .filter(|rel| !reg_paths.iter().any(|r| rel == r || rel.starts_with(r)))
            .collect();

        assert!(
            uncovered.is_empty(),
            "cross-agent files produced by codegen not in crossagent_codegen_registry or CROSSAGENT_KNOWN_EXCEPTIONS: {:#?}",
            uncovered,
        );
    }
}
