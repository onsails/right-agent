use std::io::ErrorKind;
use std::path::Path;

use include_dir::{Dir, include_dir};
use miette::{IntoDiagnostic as _, WrapErr as _};
use minijinja::Environment;
use minijinja::value::Value as JinjaValue;

use right_agent_config::MemoryProvider;
use right_platform_knobs::{IDLE_THRESHOLD_MIN, IDLE_THRESHOLD_SECS};

use crate::contract::{write_agent_owned, write_merged_rmw, write_regenerated_bytes};

const SKILL_RIGHT_SKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/right-skills");
const SKILL_RIGHT_CRON: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/right-cron");
const SKILL_RIGHT_MCP: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/right-mcp");
const SKILL_RIGHT_MEMORY_FILE: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/right-memory-file");
const SKILL_RIGHT_MEMORY_HINDSIGHT: Dir =
    include_dir!("$CARGO_MANIFEST_DIR/skills/right-memory-hindsight");
const SKILL_RIGHT_REFLECT: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/right-reflect");
const SKILL_RIGHT_LEARN_SKILL: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/right-learn-skill");
const SKILL_RIGHT_COMPOSIO: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/right-composio");

/// Canonical names of Right Agent built-in skills under `.claude/skills/`.
///
/// Single source of truth shared by the installer (`install_builtin_skills`) and
/// the sandbox deployer (`right_platform_store::build_manifest`). Past drift —
/// adding a skill to the installer without updating the deployer — caused
/// right-memory and right-reflect to ship on the host but never reach the sandbox.
/// Both ends now iterate this list; drift is impossible by construction.
pub const BUILTIN_SKILL_NAMES: &[&str] = &[
    "right-skills",
    "right-cron",
    "right-mcp",
    "right-learn-skill",
    "right-memory",
    "right-reflect",
    "right-composio",
];

/// Legacy built-in skill directory names removed during host and sandbox upgrade.
pub const BUILTIN_SKILL_LEGACY_NAMES: &[&str] = &[
    "rightskills",
    "rightcron",
    "rightmcp",
    "rightmemory",
    "rightreflect",
];

fn builtin_skill_dir(
    name: &str,
    memory_provider: &MemoryProvider,
) -> miette::Result<&'static Dir<'static>> {
    match name {
        "right-skills" => Ok(&SKILL_RIGHT_SKILLS),
        "right-cron" => Ok(&SKILL_RIGHT_CRON),
        "right-mcp" => Ok(&SKILL_RIGHT_MCP),
        "right-learn-skill" => Ok(&SKILL_RIGHT_LEARN_SKILL),
        "right-memory" => Ok(if *memory_provider == MemoryProvider::Hindsight {
            &SKILL_RIGHT_MEMORY_HINDSIGHT
        } else {
            &SKILL_RIGHT_MEMORY_FILE
        }),
        "right-reflect" => Ok(&SKILL_RIGHT_REFLECT),
        "right-composio" => Ok(&SKILL_RIGHT_COMPOSIO),
        _ => Err(miette::miette!(
            "unknown builtin skill {name:?} — add an arm to builtin_skill_dir"
        )),
    }
}

/// Install Right Agent built-in skills into an agent's `.claude/skills/` directory.
///
/// Writes all files from each embedded skill directory (SKILL.md, YAML configs, etc.).
/// Always overwrites — ensures agents get the latest built-in skill content after upgrades.
/// Only writes to named built-in paths; other directories under `.claude/skills/` are untouched.
pub fn install_builtin_skills(
    agent_path: &Path,
    memory_provider: &MemoryProvider,
) -> miette::Result<()> {
    let claude_skills_dir = agent_path.join(".claude").join("skills");
    remove_legacy_builtin_skills(&claude_skills_dir)?;

    for name in BUILTIN_SKILL_NAMES {
        let dir = builtin_skill_dir(name, memory_provider)?;
        let target = claude_skills_dir.join(name);
        install_embedded_dir(dir, &target)?;
    }

    // Create-if-absent: preserve user-installed skill registry across restarts
    let installed_json_path = claude_skills_dir.join("installed.json");
    write_agent_owned(&installed_json_path, "{}")?;
    prune_legacy_installed_json_entries(&installed_json_path)?;
    Ok(())
}

fn remove_legacy_builtin_skills(claude_skills_dir: &Path) -> miette::Result<()> {
    for legacy_name in BUILTIN_SKILL_LEGACY_NAMES {
        remove_path_if_exists(&claude_skills_dir.join(legacy_name))?;
    }
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> miette::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(miette::miette!(
                "failed to inspect legacy builtin skill path {}: {e:#}",
                path.display()
            ));
        }
    };

    let result = if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|e| {
        miette::miette!(
            "failed to remove legacy builtin skill path {}: {e:#}",
            path.display()
        )
    })
}

fn prune_legacy_installed_json_entries(installed_json_path: &Path) -> miette::Result<()> {
    write_merged_rmw(installed_json_path, |existing| {
        let content = existing.unwrap_or("{}");
        let mut value: serde_json::Value = serde_json::from_str(content).map_err(|e| {
            miette::miette!("failed to parse installed.json: {}", format!("{:#}", e))
        })?;
        let mut removed = false;
        if let Some(object) = value.as_object_mut() {
            for legacy_name in BUILTIN_SKILL_LEGACY_NAMES {
                removed |= object.remove(*legacy_name).is_some();
            }
        }
        if !removed {
            return Ok(content.to_owned());
        }
        serde_json::to_string(&value).map_err(|e| {
            miette::miette!(
                "failed to serialize pruned installed.json: {}",
                format!("{:#}", e)
            )
        })
    })
}

/// Recursively write all files from an embedded directory to `target`.
///
/// Markdown files are rendered through minijinja so platform timings (e.g.
/// `idle_threshold_min`) interpolate from the single source of truth in
/// `right-platform-knobs`. Files without `{{ }}` syntax pass through unchanged.
fn install_embedded_dir(dir: &Dir, target: &Path) -> miette::Result<()> {
    let env = skill_template_env();
    let ctx = skill_template_context();
    for file in dir.files() {
        let dest = target.join(file.path());
        let is_markdown = file
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if is_markdown {
            let raw = std::str::from_utf8(file.contents()).into_diagnostic()?;
            let rendered = env
                .render_str(raw, &ctx)
                .into_diagnostic()
                .wrap_err_with(|| format!("rendering skill template {}", file.path().display()))?;
            write_regenerated_bytes(&dest, rendered.as_bytes())?;
        } else {
            write_regenerated_bytes(&dest, file.contents())?;
        }
    }
    for subdir in dir.dirs() {
        install_embedded_dir(subdir, target)?;
    }
    Ok(())
}

/// Minijinja environment used for skill markdown rendering. No filters or
/// templates are pre-loaded — callers pass raw template strings to `render_str`.
fn skill_template_env() -> Environment<'static> {
    Environment::new()
}

/// Variables exposed to skill markdown templates. Keep this list small and
/// only add values that are user-meaningful (numbers users see in UX text).
fn skill_template_context() -> JinjaValue {
    JinjaValue::from_serialize(serde_json::json!({
        "idle_threshold_secs": IDLE_THRESHOLD_SECS,
        "idle_threshold_min": IDLE_THRESHOLD_MIN,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn installs_skills_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        assert!(
            dir.path()
                .join(".claude/skills/right-skills/SKILL.md")
                .exists(),
            "right-skills/SKILL.md should exist"
        );
    }

    #[test]
    fn installs_right_cron_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        assert!(
            dir.path()
                .join(".claude/skills/right-cron/SKILL.md")
                .exists(),
            "right-cron/SKILL.md should exist"
        );
    }

    #[test]
    fn right_cron_skill_interpolates_idle_threshold() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/right-cron/SKILL.md")).unwrap();
        // Template tokens must be fully rendered.
        assert!(
            !content.contains("{{"),
            "rendered SKILL.md still contains template tokens"
        );
        // The idle-threshold value must come from the central constant.
        let needle = format!("{IDLE_THRESHOLD_MIN} minutes");
        assert!(
            content.contains(&needle),
            "rendered SKILL.md should mention {needle}"
        );
        // The buggy "Confirm:" directives must be gone.
        assert!(
            !content.contains("Confirm:"),
            "Confirm: directives should be removed from right-cron SKILL.md"
        );
        // The stale ~60-second claim must be gone.
        assert!(
            !content.contains("~60 seconds") && !content.contains("60-second"),
            "stale 60-second references must be removed"
        );
    }

    #[test]
    fn installs_right_mcp_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        assert!(
            dir.path()
                .join(".claude/skills/right-mcp/SKILL.md")
                .exists(),
            "right-mcp/SKILL.md should exist"
        );
    }

    #[test]
    fn installs_right_learn_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let path = dir.path().join(".claude/skills/right-learn-skill/SKILL.md");
        assert!(path.exists(), "right-learn-skill/SKILL.md should exist");
    }

    #[test]
    fn right_learn_skill_mentions_protocol_and_boundaries() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/right-learn-skill/SKILL.md"))
                .unwrap();

        for needle in [
            "mcp__right__skill_learning_start",
            "mcp__right__skill_learning_finish",
            right_mcp::LEARNED_SKILL_PREFIX,
            ".claude/skills/",
            "source: \"learned\"",
            "Do not call mcp__right__send_progress just to announce learning",
            "LLM-authored receipt message",
            "Only write or patch skill files after the start call succeeds",
            "Preserve all existing installed.json entries",
            "custom, manually installed, hub-installed",
            "scripts/",
            "references/",
            "assets/",
        ] {
            assert!(
                content.contains(needle),
                "right-learn-skill must mention {needle:?}"
            );
        }
    }

    #[test]
    fn right_mcp_includes_known_endpoints_yaml() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let yaml_path = dir
            .path()
            .join(".claude/skills/right-mcp/known-endpoints.yaml");
        assert!(yaml_path.exists(), "known-endpoints.yaml should exist");
        let content = std::fs::read_to_string(&yaml_path).unwrap();
        assert!(
            content.contains("composio"),
            "known-endpoints.yaml should contain composio entry"
        );
    }

    #[test]
    fn installs_installed_json() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/installed.json")).unwrap();
        assert_eq!(content, "{}");
    }

    #[test]
    fn install_is_idempotent() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        // Second call must not error
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        assert!(
            dir.path()
                .join(".claude/skills/right-skills/SKILL.md")
                .exists()
        );
        assert!(
            dir.path()
                .join(".claude/skills/right-cron/SKILL.md")
                .exists()
        );
    }

    #[test]
    fn installed_json_preserves_existing_content() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();

        // Simulate user installing a skill (modifies installed.json)
        let installed_path = dir.path().join(".claude/skills/installed.json");
        std::fs::write(&installed_path, r#"{"my-skill":"1.0"}"#).unwrap();

        // Second call must NOT overwrite
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();

        let content = std::fs::read_to_string(&installed_path).unwrap();
        assert_eq!(
            content, r#"{"my-skill":"1.0"}"#,
            "installed.json must not be overwritten on subsequent install_builtin_skills calls"
        );
    }

    #[test]
    fn installed_json_preserves_existing_format_when_no_legacy_entries_exist() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join(".claude/skills");
        std::fs::create_dir_all(&skills_dir).unwrap();

        let installed_path = skills_dir.join("installed.json");
        let installed_content = "{\n  \"my-skill\": \"1.0\"\n}\n";
        std::fs::write(&installed_path, installed_content).unwrap();

        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();

        assert_eq!(
            std::fs::read_to_string(&installed_path).unwrap(),
            installed_content,
            "installed.json must not be rewritten when no legacy entries are present"
        );
    }

    #[test]
    fn installed_json_created_on_first_call() {
        let dir = tempdir().unwrap();
        let installed_path = dir.path().join(".claude/skills/installed.json");
        assert!(
            !installed_path.exists(),
            "should not exist before first call"
        );
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let content = std::fs::read_to_string(&installed_path).unwrap();
        assert_eq!(
            content, "{}",
            "first call should create installed.json with empty object"
        );
    }

    #[test]
    fn install_does_not_remove_user_skills() {
        let dir = tempdir().unwrap();
        // Create a user skill before install
        let user_skill_dir = dir.path().join(".claude/skills/my-custom-skill");
        std::fs::create_dir_all(&user_skill_dir).unwrap();
        std::fs::write(user_skill_dir.join("SKILL.md"), "my custom skill").unwrap();

        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();

        assert!(
            dir.path()
                .join(".claude/skills/my-custom-skill/SKILL.md")
                .exists(),
            "user skills should be preserved"
        );
    }

    #[test]
    fn install_removes_legacy_builtin_skill_dirs_and_preserves_user_content() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join(".claude/skills");

        let legacy_names = [
            "rightskills",
            "rightcron",
            "rightmcp",
            "rightmemory",
            "rightreflect",
        ];
        for legacy_name in legacy_names {
            let legacy_dir = skills_dir.join(legacy_name);
            std::fs::create_dir_all(&legacy_dir).unwrap();
            std::fs::write(legacy_dir.join("SKILL.md"), "legacy built-in").unwrap();
        }

        let user_skill_dir = skills_dir.join("my-custom-skill");
        std::fs::create_dir_all(&user_skill_dir).unwrap();
        std::fs::write(user_skill_dir.join("SKILL.md"), "user skill").unwrap();

        let installed_json = skills_dir.join("installed.json");
        let installed_content = r#"{"my-custom-skill":"1.0"}"#;
        std::fs::write(&installed_json, installed_content).unwrap();

        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();

        for builtin_name in BUILTIN_SKILL_NAMES {
            assert!(
                skills_dir.join(builtin_name).join("SKILL.md").exists(),
                "new builtin skill dir must exist: {builtin_name}"
            );
        }
        for legacy_name in legacy_names {
            assert!(
                !skills_dir.join(legacy_name).exists(),
                "legacy builtin skill dir must be removed: {legacy_name}"
            );
        }
        assert!(
            user_skill_dir.join("SKILL.md").exists(),
            "user skill dir must be preserved"
        );
        assert_eq!(
            std::fs::read_to_string(installed_json).unwrap(),
            installed_content,
            "installed.json user content must be preserved"
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_removes_legacy_builtin_skill_symlinks_without_removing_target() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join(".claude/skills");
        std::fs::create_dir_all(&skills_dir).unwrap();

        let target_dir = dir.path().join("legacy-target");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("SKILL.md"), "legacy symlink target").unwrap();
        std::os::unix::fs::symlink(&target_dir, skills_dir.join("rightcron")).unwrap();

        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();

        assert!(
            !skills_dir.join("rightcron").exists(),
            "legacy symlink path must be removed"
        );
        assert!(
            target_dir.join("SKILL.md").exists(),
            "removing a legacy symlink must not remove its target"
        );
        assert!(
            skills_dir.join("right-cron/SKILL.md").exists(),
            "new right-cron skill must be installed"
        );
    }

    #[test]
    fn install_prunes_legacy_builtin_entries_from_installed_json() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join(".claude/skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("installed.json"),
            r#"{"rightcron":"builtin","my-custom-skill":"1.0","rightmcp":"builtin"}"#,
        )
        .unwrap();

        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();

        let content = std::fs::read_to_string(skills_dir.join("installed.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({"my-custom-skill":"1.0"}),
            "obsolete builtin entries must be removed without dropping user entries"
        );
    }

    /// Contract: every name in `BUILTIN_SKILL_NAMES` must be installed to disk,
    /// and `builtin_skill_dir` must recognize each name (no `unknown builtin skill`
    /// error). This is the single point that prevents drift between the installer
    /// and the platform-store deployer (which both iterate `BUILTIN_SKILL_NAMES`).
    #[test]
    fn installer_covers_every_builtin_skill_name() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        for name in BUILTIN_SKILL_NAMES {
            assert!(
                builtin_skill_dir(name, &MemoryProvider::File).is_ok(),
                "builtin_skill_dir missing arm for {name}"
            );
            assert!(
                dir.path()
                    .join(".claude/skills")
                    .join(name)
                    .join("SKILL.md")
                    .exists(),
                "{name}/SKILL.md not installed — BUILTIN_SKILL_NAMES out of sync with installer"
            );
        }
    }

    /// Verify every file in the source skills/ directories is embedded and installed.
    /// Catches cases where a new file is added to a skill but not picked up by include_dir.
    #[test]
    fn all_source_skill_files_are_installed() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();

        // (source_dir_name, installed_dir_name)
        let skills: &[(&str, &str)] = &[
            ("right-skills", "right-skills"),
            ("right-cron", "right-cron"),
            ("right-mcp", "right-mcp"),
            ("right-learn-skill", "right-learn-skill"),
            ("right-memory-file", "right-memory"),
            ("right-reflect", "right-reflect"),
        ];
        for (source_name, installed_name) in skills {
            let source_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("skills")
                .join(source_name);
            let target_dir = dir.path().join(".claude/skills").join(installed_name);

            for entry in walkdir::WalkDir::new(&source_dir) {
                let entry = entry.unwrap();
                if !entry.file_type().is_file() {
                    continue;
                }
                let rel = entry.path().strip_prefix(&source_dir).unwrap();
                let installed = target_dir.join(rel);
                assert!(
                    installed.exists(),
                    "skill file {source_name}/{} not installed at {installed_name}/",
                    rel.display()
                );
            }
        }
    }

    #[test]
    fn installs_right_memory_file_variant() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/right-memory/SKILL.md"))
                .unwrap();
        assert!(
            content.contains("MEMORY.md"),
            "file variant must reference MEMORY.md"
        );
        assert!(
            !content.contains("memory_retain"),
            "file variant must NOT reference MCP tools"
        );
    }

    #[test]
    fn installs_right_memory_hindsight_variant() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::Hindsight).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/right-memory/SKILL.md"))
                .unwrap();
        assert!(
            content.contains("mcp__right__memory_retain"),
            "hindsight variant must reference the agent-facing MCP retain tool name"
        );
        assert!(
            !content.contains("`memory_retain`"),
            "hindsight variant must not teach the bare MCP retain tool name"
        );
        assert!(
            !content.contains("Edit and Write tools to manage MEMORY.md"),
            "hindsight variant must NOT reference Edit/Write for MEMORY.md"
        );
    }

    #[test]
    fn installs_right_composio_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        assert!(
            dir.path()
                .join(".claude/skills/right-composio/SKILL.md")
                .exists(),
            "right-composio/SKILL.md should exist"
        );
    }

    #[test]
    fn right_composio_skill_documents_workbench_discipline() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/right-composio/SKILL.md"))
                .unwrap();
        // Frontmatter must declare the skill name CC's selector matches on.
        assert!(
            content.contains("name: right-composio"),
            "SKILL.md must declare name: right-composio in frontmatter"
        );
        // Workbench discipline is the load-bearing reason the skill exists.
        assert!(
            content.contains("sync_response_to_workbench"),
            "SKILL.md must document sync_response_to_workbench"
        );
        assert!(
            content.contains("COMPOSIO_MULTI_EXECUTE_TOOL"),
            "SKILL.md must reference the MULTI_EXECUTE tool by name"
        );
        // Auth pitfall must defer to the main MCP Error Diagnosis section,
        // not duplicate /mcp auth advice (per the 2026-05-06 spec).
        assert!(
            content.contains("Do NOT suggest `/mcp auth composio`"),
            "SKILL.md must steer the agent away from suggesting /mcp auth composio"
        );
    }

    #[test]
    fn installs_right_reflect_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        assert!(
            dir.path()
                .join(".claude/skills/right-reflect/SKILL.md")
                .exists(),
            "right-reflect/SKILL.md should exist"
        );
    }

    #[test]
    fn right_reflect_skill_frontmatter_is_valid() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/right-reflect/SKILL.md"))
                .unwrap();
        assert!(content.starts_with("---\n"), "frontmatter must start file");
        assert!(content.contains("name: right-reflect"), "must declare name");
        assert!(
            content.contains("/sandbox/.claude/projects/-sandbox/"),
            "must reference the JSONL path"
        );
    }
}
