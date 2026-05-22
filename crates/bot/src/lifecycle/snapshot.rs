//! Backup `.claude/skills/` to a tar.gz before destructive curator operations.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum SnapshotError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl From<walkdir::Error> for SnapshotError {
    fn from(e: walkdir::Error) -> Self {
        Self::Io(
            e.into_io_error()
                .unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "walkdir error")),
        )
    }
}

/// Produce `<backups_dir>/<utc>/skills.tar.gz` containing the entire `<skills_dir>` tree.
/// Excludes `.archive/` and `.curator_backups/` subdirectories.
pub(crate) fn snapshot_skills(
    skills_dir: &Path,
    backups_dir: &Path,
    now_utc: &str,
) -> Result<PathBuf, SnapshotError> {
    let target_dir = backups_dir.join(now_utc);
    std::fs::create_dir_all(&target_dir)?;
    let archive_path = target_dir.join("skills.tar.gz");
    let f = std::fs::File::create(&archive_path)?;
    let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut builder = tar::Builder::new(gz);
    builder.follow_symlinks(false);

    for entry in walkdir::WalkDir::new(skills_dir)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(name == ".archive" || name == ".curator_backups")
        })
    {
        let entry = entry?;
        let path = entry.path();
        if path == skills_dir {
            continue;
        }
        let rel = path.strip_prefix(skills_dir).unwrap();
        if entry.file_type().is_dir() {
            builder.append_dir(rel, path)?;
        } else if entry.file_type().is_file() {
            let mut file = std::fs::File::open(path)?;
            builder.append_file(rel, &mut file)?;
        }
    }
    let gz = builder.into_inner()?;
    gz.finish()?;
    Ok(archive_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_includes_skill_files_and_excludes_archive() {
        let dir = tempfile::TempDir::new().unwrap();
        let skills = dir.path().join(".claude/skills");
        std::fs::create_dir_all(skills.join("rightx-foo")).unwrap();
        std::fs::write(skills.join("rightx-foo/SKILL.md"), "# foo skill").unwrap();
        std::fs::create_dir_all(skills.join(".archive/rightx-old")).unwrap();
        std::fs::write(skills.join(".archive/rightx-old/SKILL.md"), "# old").unwrap();

        let backups = dir.path().join("curator_backups");
        let archive = snapshot_skills(&skills, &backups, "2026-05-22T12-00-00Z").unwrap();
        assert!(archive.exists());

        let f = std::fs::File::open(&archive).unwrap();
        let gz = flate2::read::GzDecoder::new(f);
        let mut tar = tar::Archive::new(gz);
        let entries: Vec<String> = tar
            .entries()
            .unwrap()
            .filter_map(|e| {
                e.ok()
                    .and_then(|e| e.path().ok().map(|p| p.to_string_lossy().into_owned()))
            })
            .collect();
        assert!(
            entries.iter().any(|p| p.ends_with("rightx-foo/SKILL.md")),
            "expected rightx-foo/SKILL.md in archive entries: {entries:?}"
        );
        assert!(
            !entries.iter().any(|p| p.contains(".archive/")),
            "archive must not include .archive/ subdir: {entries:?}"
        );
    }
}
