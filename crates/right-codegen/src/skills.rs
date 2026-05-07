use std::path::Path;

use include_dir::{Dir, include_dir};
use miette::{IntoDiagnostic as _, WrapErr as _};
use minijinja::Environment;
use minijinja::value::Value as JinjaValue;

use right_core::agent_types::MemoryProvider;
use right_core::time_constants::{IDLE_THRESHOLD_MIN, IDLE_THRESHOLD_SECS};

use crate::contract::{write_agent_owned, write_regenerated_bytes};

const SKILL_RIGHTSKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/rightskills");
const SKILL_RIGHTCRON: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/rightcron");
const SKILL_RIGHTMCP: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/rightmcp");
const SKILL_RIGHTMEMORY_FILE: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/rightmemory-file");
const SKILL_RIGHTMEMORY_HINDSIGHT: Dir =
    include_dir!("$CARGO_MANIFEST_DIR/skills/rightmemory-hindsight");

/// Install Right Agent built-in skills into an agent's `.claude/skills/` directory.
///
/// Writes all files from each embedded skill directory (SKILL.md, YAML configs, etc.).
/// Always overwrites — ensures agents get the latest built-in skill content after upgrades.
/// Only writes to named built-in paths; other directories under `.claude/skills/` are untouched.
pub fn install_builtin_skills(
    agent_path: &Path,
    memory_provider: &MemoryProvider,
) -> miette::Result<()> {
    let rightmemory_dir: &Dir = if *memory_provider == MemoryProvider::Hindsight {
        &SKILL_RIGHTMEMORY_HINDSIGHT
    } else {
        &SKILL_RIGHTMEMORY_FILE
    };
    let skills: &[(&str, &Dir)] = &[
        ("rightskills", &SKILL_RIGHTSKILLS),
        ("rightcron", &SKILL_RIGHTCRON),
        ("rightmcp", &SKILL_RIGHTMCP),
        ("rightmemory", rightmemory_dir),
    ];
    let claude_skills_dir = agent_path.join(".claude").join("skills");

    for (name, dir) in skills {
        let target = claude_skills_dir.join(name);
        install_embedded_dir(dir, &target)?;
    }

    // Create-if-absent: preserve user-installed skill registry across restarts
    let installed_json_path = claude_skills_dir.join("installed.json");
    write_agent_owned(&installed_json_path, "{}")?;
    Ok(())
}

/// Recursively write all files from an embedded directory to `target`.
///
/// Markdown files are rendered through minijinja so platform timings (e.g.
/// `idle_threshold_min`) interpolate from the single source of truth in
/// `cron_spec`. Files without `{{ }}` syntax pass through unchanged.
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
                .join(".claude/skills/rightskills/SKILL.md")
                .exists(),
            "rightskills/SKILL.md should exist"
        );
    }

    #[test]
    fn installs_rightcron_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        assert!(
            dir.path()
                .join(".claude/skills/rightcron/SKILL.md")
                .exists(),
            "rightcron/SKILL.md should exist"
        );
    }

    #[test]
    fn rightcron_skill_interpolates_idle_threshold() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/rightcron/SKILL.md")).unwrap();
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
            "Confirm: directives should be removed from rightcron SKILL.md"
        );
        // The stale ~60-second claim must be gone.
        assert!(
            !content.contains("~60 seconds") && !content.contains("60-second"),
            "stale 60-second references must be removed"
        );
    }

    #[test]
    fn installs_rightmcp_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        assert!(
            dir.path().join(".claude/skills/rightmcp/SKILL.md").exists(),
            "rightmcp/SKILL.md should exist"
        );
    }

    #[test]
    fn rightmcp_includes_known_endpoints_yaml() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let yaml_path = dir
            .path()
            .join(".claude/skills/rightmcp/known-endpoints.yaml");
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
                .join(".claude/skills/rightskills/SKILL.md")
                .exists()
        );
        assert!(
            dir.path()
                .join(".claude/skills/rightcron/SKILL.md")
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

    /// Verify every file in the source skills/ directories is embedded and installed.
    /// Catches cases where a new file is added to a skill but not picked up by include_dir.
    #[test]
    fn all_source_skill_files_are_installed() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();

        // (source_dir_name, installed_dir_name)
        let skills: &[(&str, &str)] = &[
            ("rightskills", "rightskills"),
            ("rightcron", "rightcron"),
            ("rightmcp", "rightmcp"),
            ("rightmemory-file", "rightmemory"),
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
    fn installs_rightmemory_file_variant() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/rightmemory/SKILL.md"))
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
    fn installs_rightmemory_hindsight_variant() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::Hindsight).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/rightmemory/SKILL.md"))
                .unwrap();
        assert!(
            content.contains("memory_retain"),
            "hindsight variant must reference MCP tools"
        );
        assert!(
            !content.contains("Edit and Write tools to manage MEMORY.md"),
            "hindsight variant must NOT reference Edit/Write for MEMORY.md"
        );
    }
}
