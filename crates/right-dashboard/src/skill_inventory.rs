use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::api_types::{SkillDetailResponse, SkillGroups, SkillSummary, SkillsResponse};

#[derive(Debug, thiserror::Error)]
pub enum SkillInventoryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid skill name: {0}")]
    InvalidSkillName(String),
    #[error("skill not found: {0}")]
    NotFound(String),
}

pub fn scan_host_skills(
    agent: &str,
    agent_dir: &Path,
    core_skill_names: &[&str],
    source: &str,
    preview_limit_bytes: usize,
) -> Result<SkillsResponse, SkillInventoryError> {
    let mut groups = SkillGroups::default();
    let skills_dir = skills_dir(agent_dir);
    if is_directory_no_symlink(&skills_dir)? {
        for entry in std::fs::read_dir(&skills_dir)? {
            let entry = entry?;
            if !is_directory_no_symlink(&entry.path())? {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !is_valid_skill_name(&name) {
                continue;
            }
            let skill_path = entry.path().join("SKILL.md");
            if !is_regular_file_no_symlink(&skill_path)? {
                continue;
            }
            let (preview, _) = read_bounded_text(&skill_path, preview_limit_bytes)?;
            let group = classify_skill_group(&name, core_skill_names);
            let summary = SkillSummary {
                name,
                group: group.to_owned(),
                path: skill_path
                    .strip_prefix(agent_dir)
                    .unwrap_or(skill_path.as_path())
                    .to_string_lossy()
                    .into_owned(),
                description: parse_skill_description(&preview),
            };
            match group {
                "core" => groups.core.push(summary),
                "learned" => groups.learned.push(summary),
                _ => groups.other.push(summary),
            }
        }
    }
    sort_skill_groups(&mut groups);

    Ok(SkillsResponse {
        agent: agent.to_owned(),
        source: source.to_owned(),
        warning: None,
        groups,
    })
}

pub fn read_host_skill_detail(
    agent: &str,
    agent_dir: &Path,
    skill_name: &str,
    core_skill_names: &[&str],
    preview_limit_bytes: usize,
) -> Result<SkillDetailResponse, SkillInventoryError> {
    validate_skill_name(skill_name)?;
    let skill_dir = skills_dir(agent_dir).join(skill_name);
    if !is_directory_no_symlink(&skill_dir)? {
        return Err(SkillInventoryError::NotFound(skill_name.to_owned()));
    }
    let path = skill_dir.join("SKILL.md");
    if !path.exists() {
        return Err(SkillInventoryError::NotFound(skill_name.to_owned()));
    }
    if !is_regular_file_no_symlink(&path)? {
        return Err(SkillInventoryError::NotFound(skill_name.to_owned()));
    }
    let (content_preview, truncated) = read_bounded_text(&path, preview_limit_bytes)?;
    let group = classify_skill_group(skill_name, core_skill_names).to_owned();
    let skill = SkillSummary {
        name: skill_name.to_owned(),
        group,
        path: path
            .strip_prefix(agent_dir)
            .unwrap_or(path.as_path())
            .to_string_lossy()
            .into_owned(),
        description: parse_skill_description(&content_preview),
    };

    Ok(SkillDetailResponse {
        agent: agent.to_owned(),
        skill,
        content_preview,
        truncated,
    })
}

fn skills_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join(".claude").join("skills")
}

pub fn validate_skill_name(name: &str) -> Result<(), SkillInventoryError> {
    if is_valid_skill_name(name) {
        Ok(())
    } else {
        Err(SkillInventoryError::InvalidSkillName(name.to_owned()))
    }
}

fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.chars().any(char::is_control)
        && name != "."
        && name != ".."
        && !name.contains("..")
}

pub fn classify_skill_group<'a>(name: &str, core_skill_names: &'a [&'a str]) -> &'static str {
    if core_skill_names.iter().any(|core| *core == name) {
        "core"
    } else if name.starts_with("rightx-") {
        "learned"
    } else {
        "other"
    }
}

fn read_bounded_text(
    path: &Path,
    preview_limit_bytes: usize,
) -> Result<(String, bool), SkillInventoryError> {
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    let read_limit = preview_limit_bytes.saturating_add(1) as u64;
    file.by_ref().take(read_limit).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > preview_limit_bytes;
    if truncated {
        bytes.truncate(preview_limit_bytes);
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

fn is_regular_file_no_symlink(path: &Path) -> Result<bool, SkillInventoryError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    Ok(metadata.file_type().is_file())
}

fn is_directory_no_symlink(path: &Path) -> Result<bool, SkillInventoryError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    Ok(metadata.file_type().is_dir())
}

pub fn parse_skill_description(content: &str) -> Option<String> {
    let frontmatter = content.strip_prefix("---\n")?;
    let frontmatter = frontmatter.split_once("\n---")?.0;
    let mut lines = frontmatter.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(raw_value) = line.strip_prefix("description:") else {
            continue;
        };
        let raw_value = raw_value.trim();
        if raw_value.starts_with('|') || raw_value.starts_with('>') {
            let mut parts = Vec::new();
            while let Some(next) = lines.peek() {
                if next.trim().is_empty() {
                    lines.next();
                    continue;
                }
                if !next.starts_with(' ') && !next.starts_with('\t') {
                    break;
                }
                parts.push(lines.next().unwrap().trim().to_owned());
            }
            return normalize_description(parts.join(" "));
        }
        return normalize_description(trim_yaml_scalar(raw_value).to_owned());
    }
    None
}

fn trim_yaml_scalar(value: &str) -> &str {
    value.trim().trim_matches('"').trim_matches('\'').trim()
}

fn normalize_description(value: String) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub fn sort_skill_groups(groups: &mut SkillGroups) {
    groups
        .core
        .sort_by(|left, right| left.name.cmp(&right.name));
    groups
        .learned
        .sort_by(|left, right| left.name.cmp(&right.name));
    groups
        .other
        .sort_by(|left, right| left.name.cmp(&right.name));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str, body: &str) {
        let dir = root.join(".claude").join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn scan_host_skills_groups_core_learned_and_other() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(
            temp.path(),
            "right-cron",
            "---\nname: right-cron\ndescription: Core cron control.\n---\n# Cron\n",
        );
        write_skill(
            temp.path(),
            "rightx-oauth-debugging",
            "---\nname: rightx-oauth-debugging\ndescription: >-\n  Debug OAuth callback setup.\n  Verify redirects.\n---\n# OAuth\n",
        );
        write_skill(
            temp.path(),
            "hub-browser",
            "---\nname: hub-browser\ndescription: Browser automation from hub.\n---\n# Browser\n",
        );

        let response = scan_host_skills("alpha", temp.path(), &["right-cron"], "host", 1024)
            .expect("scan skills");

        assert_eq!(response.agent, "alpha");
        assert_eq!(response.source, "host");
        assert_eq!(response.groups.core.len(), 1, "{response:#?}");
        assert_eq!(response.groups.learned.len(), 1, "{response:#?}");
        assert_eq!(response.groups.other.len(), 1, "{response:#?}");
        assert_eq!(response.groups.core[0].name, "right-cron");
        assert_eq!(
            response.groups.core[0].description.as_deref(),
            Some("Core cron control.")
        );
        assert_eq!(response.groups.learned[0].name, "rightx-oauth-debugging");
        assert_eq!(
            response.groups.learned[0].description.as_deref(),
            Some("Debug OAuth callback setup. Verify redirects.")
        );
        assert_eq!(response.groups.other[0].name, "hub-browser");
    }

    #[test]
    fn read_host_skill_detail_rejects_path_traversal() {
        let temp = tempfile::tempdir().expect("tempdir");

        let err = read_host_skill_detail("alpha", temp.path(), "../secret", &[], 1024)
            .expect_err("path traversal must be rejected");

        assert!(matches!(err, SkillInventoryError::InvalidSkillName(_)));
    }

    #[test]
    fn read_host_skill_detail_returns_bounded_preview() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(
            temp.path(),
            "rightx-large",
            "---\nname: rightx-large\ndescription: Large skill.\n---\nabcdef",
        );

        let response =
            read_host_skill_detail("alpha", temp.path(), "rightx-large", &[], 24).unwrap();

        assert_eq!(response.agent, "alpha");
        assert_eq!(response.skill.name, "rightx-large");
        assert_eq!(response.skill.group, "learned");
        assert!(response.truncated);
        assert_eq!(response.content_preview.len(), 24);
    }

    #[cfg(unix)]
    #[test]
    fn read_host_skill_detail_rejects_symlinked_skill_file() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let secret = temp.path().join("secret.txt");
        std::fs::write(&secret, "secret material").unwrap();
        let skill_dir = temp
            .path()
            .join(".claude")
            .join("skills")
            .join("rightx-link");
        std::fs::create_dir_all(&skill_dir).unwrap();
        symlink(&secret, skill_dir.join("SKILL.md")).unwrap();

        let err = read_host_skill_detail("alpha", temp.path(), "rightx-link", &[], 1024)
            .expect_err("symlinked SKILL.md must not be readable");

        assert!(matches!(err, SkillInventoryError::NotFound(_)));

        let response = scan_host_skills("alpha", temp.path(), &[], "host", 1024).unwrap();
        assert!(response.groups.learned.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn read_host_skill_detail_rejects_symlinked_skill_directory() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("SKILL.md"), "outside material").unwrap();
        let skills_dir = temp.path().join(".claude").join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        symlink(&outside, skills_dir.join("rightx-link")).unwrap();

        let err = read_host_skill_detail("alpha", temp.path(), "rightx-link", &[], 1024)
            .expect_err("symlinked skill dir must not be readable");

        assert!(matches!(err, SkillInventoryError::NotFound(_)));

        let response = scan_host_skills("alpha", temp.path(), &[], "host", 1024).unwrap();
        assert!(response.groups.learned.is_empty());
    }
}
