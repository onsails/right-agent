use std::path::Path;

const SKILL_CLAWHUB: &str = include_str!("../../../../skills/clawhub/SKILL.md");
const SKILL_RIGHTCRON: &str = include_str!("../../../../skills/cronsync/SKILL.md");

/// Install RightClaw built-in skills into an agent's `.claude/skills/` directory.
///
/// Writes `clawhub/SKILL.md`, `rightcron/SKILL.md`, and `installed.json`.
/// Always overwrites — ensures agents get the latest built-in skill content after upgrades.
/// Only writes to named built-in paths; other directories under `.claude/skills/` are untouched.
pub fn install_builtin_skills(agent_path: &Path) -> miette::Result<()> {
    let built_in_skills: &[(&str, &str)] = &[
        ("clawhub/SKILL.md", SKILL_CLAWHUB),
        ("rightcron/SKILL.md", SKILL_RIGHTCRON),
    ];
    let claude_skills_dir = agent_path.join(".claude").join("skills");
    for (skill_path, content) in built_in_skills {
        let path = claude_skills_dir.join(skill_path);
        std::fs::create_dir_all(path.parent().unwrap())
            .map_err(|e| miette::miette!("failed to create skill directory: {e:#}"))?;
        std::fs::write(&path, content)
            .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))?;
    }
    std::fs::write(claude_skills_dir.join("installed.json"), "{}")
        .map_err(|e| miette::miette!("failed to write installed.json: {e:#}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn installs_clawhub_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path()).unwrap();
        assert!(
            dir.path().join(".claude/skills/clawhub/SKILL.md").exists(),
            "clawhub/SKILL.md should exist"
        );
    }

    #[test]
    fn installs_rightcron_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path()).unwrap();
        assert!(
            dir.path().join(".claude/skills/rightcron/SKILL.md").exists(),
            "rightcron/SKILL.md should exist"
        );
    }

    #[test]
    fn installs_installed_json() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path()).unwrap();
        let content =
            std::fs::read_to_string(dir.path().join(".claude/skills/installed.json")).unwrap();
        assert_eq!(content, "{}");
    }

    #[test]
    fn install_is_idempotent() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path()).unwrap();
        // Second call must not error
        install_builtin_skills(dir.path()).unwrap();
        assert!(dir.path().join(".claude/skills/clawhub/SKILL.md").exists());
        assert!(dir.path().join(".claude/skills/rightcron/SKILL.md").exists());
    }

    #[test]
    fn install_does_not_remove_user_skills() {
        let dir = tempdir().unwrap();
        // Create a user skill before install
        let user_skill_dir = dir.path().join(".claude/skills/my-custom-skill");
        std::fs::create_dir_all(&user_skill_dir).unwrap();
        std::fs::write(user_skill_dir.join("SKILL.md"), "my custom skill").unwrap();

        install_builtin_skills(dir.path()).unwrap();

        assert!(
            dir.path()
                .join(".claude/skills/my-custom-skill/SKILL.md")
                .exists(),
            "user skills should be preserved"
        );
    }
}
