use std::path::Path;

use crate::api_types::{IdentityFileSummary, IdentityResponse};
use crate::fs_safety::{is_regular_file_no_symlink, read_bounded_text};

pub const IDENTITY_FILE_NAMES: [&str; 3] = ["IDENTITY.md", "SOUL.md", "USER.md"];

#[derive(Debug, thiserror::Error)]
pub enum IdentityFilesError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid identity file name: {0}")]
    InvalidFileName(String),
}

pub fn read_host_identity_files(
    agent: &str,
    agent_dir: &Path,
    source: &str,
    warning: Option<String>,
    preview_limit_bytes: usize,
) -> Result<IdentityResponse, IdentityFilesError> {
    let mut files = Vec::with_capacity(IDENTITY_FILE_NAMES.len());
    for name in IDENTITY_FILE_NAMES {
        files.push(read_host_identity_file(
            agent_dir,
            source,
            missing_source_for(source),
            name,
            preview_limit_bytes,
        )?);
    }

    Ok(IdentityResponse {
        agent: agent.to_owned(),
        source: source.to_owned(),
        warning,
        files,
    })
}

pub fn validate_identity_file_name(name: &str) -> Result<(), IdentityFilesError> {
    if IDENTITY_FILE_NAMES.contains(&name) {
        Ok(())
    } else {
        Err(IdentityFilesError::InvalidFileName(name.to_owned()))
    }
}

pub fn read_host_identity_file(
    agent_dir: &Path,
    source: &str,
    missing_source: &str,
    name: &str,
    preview_limit_bytes: usize,
) -> Result<IdentityFileSummary, IdentityFilesError> {
    validate_identity_file_name(name)?;
    let path = agent_dir.join(name);
    let (exists, content_preview, truncated) = if is_regular_file_no_symlink(&path)? {
        let (content, truncated) = read_bounded_text(&path, preview_limit_bytes)?;
        (true, Some(content), truncated)
    } else {
        (false, None, false)
    };

    Ok(IdentityFileSummary {
        name: name.to_owned(),
        source: if exists { source } else { missing_source }.to_owned(),
        path: name.to_owned(),
        exists,
        content_preview,
        truncated,
    })
}

fn missing_source_for(source: &str) -> &str {
    if source == "host_mirror" {
        "unavailable"
    } else {
        source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_host_identity_files_reports_sources_per_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("IDENTITY.md"), "# Identity\n").unwrap();
        std::fs::write(temp.path().join("SOUL.md"), "# Soul\n").unwrap();

        let response = read_host_identity_files("alpha", temp.path(), "host", None, 64).unwrap();

        assert_eq!(response.agent, "alpha");
        assert_eq!(response.source, "host");
        assert_eq!(response.files.len(), 3);
        assert_eq!(response.files[0].name, "IDENTITY.md");
        assert_eq!(response.files[0].source, "host");
        assert!(response.files[0].exists);
        assert_eq!(
            response.files[0].content_preview.as_deref(),
            Some("# Identity\n")
        );
        assert!(response.files[1].exists);
        assert!(!response.files[2].exists);
    }

    #[test]
    fn read_host_identity_files_marks_missing_host_mirror_unavailable() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("IDENTITY.md"), "# Identity\n").unwrap();

        let response =
            read_host_identity_files("alpha", temp.path(), "host_mirror", None, 64).unwrap();

        assert_eq!(response.source, "host_mirror");
        assert_eq!(response.files[0].source, "host_mirror");
        assert_eq!(response.files[1].source, "unavailable");
        assert!(!response.files[1].exists);
    }

    #[test]
    fn validate_identity_file_rejects_path_traversal() {
        let err = validate_identity_file_name("../IDENTITY.md")
            .expect_err("path traversal must be rejected");

        assert!(matches!(err, IdentityFilesError::InvalidFileName(_)));
    }

    #[test]
    fn read_host_identity_files_marks_truncated_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("IDENTITY.md"), "abcdef").unwrap();

        let response = read_host_identity_files("alpha", temp.path(), "host", None, 3).unwrap();

        assert_eq!(response.files[0].content_preview.as_deref(), Some("abc"));
        assert!(response.files[0].truncated);
    }

    #[cfg(unix)]
    #[test]
    fn read_host_identity_files_rejects_symlinked_file() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("secret.txt"), "secret").unwrap();
        symlink(
            temp.path().join("secret.txt"),
            temp.path().join("IDENTITY.md"),
        )
        .unwrap();

        let response = read_host_identity_files("alpha", temp.path(), "host", None, 64).unwrap();

        assert!(!response.files[0].exists);
        assert_eq!(response.files[0].content_preview, None);
    }
}
