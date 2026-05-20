use std::time::Duration;

use right_dashboard::api_types::{IdentityFileResponse, IdentityFileSummary, IdentityResponse};
use right_dashboard::identity_files::{
    IDENTITY_FILE_NAMES, IdentityFilesError, read_host_identity_file, read_host_identity_files,
    validate_identity_file_name,
};
use right_openshell::sandbox_exec::SandboxExec;

use super::DashboardState;

const IDENTITY_PREVIEW_LIMIT_BYTES: usize = 64 * 1024;
const SANDBOX_READ_IDENTITY_SCRIPT: &str = r#"file="$1"
[ -e "$file" ] || exit 3
[ -L "$file" ] && exit 3
[ -f "$file" ] || exit 3
head -c "$2" "$file""#;

pub(super) async fn identity_response(
    state: &DashboardState,
) -> Result<IdentityResponse, IdentityFilesError> {
    if let Some(sandbox_exec) = state.sandbox_exec.as_ref() {
        match read_sandbox_identity_files(state, sandbox_exec).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                let warning = Some(format!(
                    "sandbox identity read failed; showing host mirror: {error:#}"
                ));
                return read_host_identity_files(
                    &state.agent_name,
                    &state.agent_dir,
                    "host_mirror",
                    warning,
                    IDENTITY_PREVIEW_LIMIT_BYTES,
                );
            }
        }
    }

    read_host_identity_files(
        &state.agent_name,
        &state.agent_dir,
        "host",
        None,
        IDENTITY_PREVIEW_LIMIT_BYTES,
    )
}

pub(super) async fn identity_file_response(
    state: &DashboardState,
    file_name: &str,
) -> Result<IdentityFileResponse, IdentityFilesError> {
    validate_identity_file_name(file_name)?;
    if let Some(sandbox_exec) = state.sandbox_exec.as_ref() {
        let (file, warning) = match read_sandbox_identity_file(sandbox_exec, file_name).await {
            Ok(Some(file)) => (file, None),
            Ok(None) => (
                host_mirror_or_unavailable(&state.agent_dir, file_name)?,
                Some(format!(
                    "sandbox identity file {file_name} unavailable; showing host mirror when present"
                )),
            ),
            Err(error) => (
                host_mirror_or_unavailable(&state.agent_dir, file_name)?,
                Some(format!(
                    "sandbox identity file read failed; showing host mirror when present: {error:#}"
                )),
            ),
        };
        return Ok(IdentityFileResponse {
            agent: state.agent_name.clone(),
            warning,
            file,
        });
    }

    Ok(IdentityFileResponse {
        agent: state.agent_name.clone(),
        warning: None,
        file: read_host_identity_file(
            &state.agent_dir,
            "host",
            "host",
            file_name,
            IDENTITY_PREVIEW_LIMIT_BYTES,
        )?,
    })
}

async fn read_sandbox_identity_files(
    state: &DashboardState,
    sandbox_exec: &SandboxExec,
) -> miette::Result<IdentityResponse> {
    let mut files = Vec::with_capacity(IDENTITY_FILE_NAMES.len());
    let mut warning_parts = Vec::new();

    for name in IDENTITY_FILE_NAMES {
        let sandbox_path = format!("/sandbox/{name}");
        let limit = (IDENTITY_PREVIEW_LIMIT_BYTES + 1).to_string();
        let command = [
            "sh",
            "-c",
            SANDBOX_READ_IDENTITY_SCRIPT,
            "dashboard-identity-read",
            sandbox_path.as_str(),
            limit.as_str(),
        ];
        let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_TIMEOUT_SECS);
        let result = match tokio::time::timeout(timeout, sandbox_exec.exec(&command)).await {
            Ok(result) => result,
            Err(_) => {
                files.push(host_mirror_or_unavailable(&state.agent_dir, name).map_err(
                    |error| miette::miette!("host mirror read failed for {name}: {error:#}"),
                )?);
                warning_parts.push(format!("{name} unavailable in sandbox"));
                continue;
            }
        };
        match summarize_sandbox_read(name, &sandbox_path, result) {
            Ok(Some(file)) => files.push(file),
            Ok(None) => {
                files.push(host_mirror_or_unavailable(&state.agent_dir, name).map_err(
                    |error| miette::miette!("host mirror read failed for {name}: {error:#}"),
                )?);
                warning_parts.push(format!("{name} unavailable in sandbox"));
            }
            Err(error) => {
                files.push(host_mirror_or_unavailable(&state.agent_dir, name).map_err(
                    |error| miette::miette!("host mirror read failed for {name}: {error:#}"),
                )?);
                warning_parts.push(format!("{name} sandbox read failed: {error:#}"));
            }
        }
    }

    Ok(IdentityResponse {
        agent: state.agent_name.clone(),
        source: if warning_parts.is_empty() {
            "sandbox".to_owned()
        } else {
            "mixed".to_owned()
        },
        warning: if warning_parts.is_empty() {
            None
        } else {
            Some(warning_parts.join("; "))
        },
        files,
    })
}

async fn read_sandbox_identity_file(
    sandbox_exec: &SandboxExec,
    name: &str,
) -> miette::Result<Option<IdentityFileSummary>> {
    validate_identity_file_name(name).map_err(|error| miette::miette!("{error:#}"))?;
    let sandbox_path = format!("/sandbox/{name}");
    let limit = (IDENTITY_PREVIEW_LIMIT_BYTES + 1).to_string();
    let command = [
        "sh",
        "-c",
        SANDBOX_READ_IDENTITY_SCRIPT,
        "dashboard-identity-read",
        sandbox_path.as_str(),
        limit.as_str(),
    ];
    let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_TIMEOUT_SECS);
    let result = match tokio::time::timeout(timeout, sandbox_exec.exec(&command)).await {
        Ok(result) => result,
        Err(_) => {
            return Err(miette::miette!(
                "sandbox identity read for {name} timed out"
            ));
        }
    };
    summarize_sandbox_read(name, &sandbox_path, result)
}

fn summarize_sandbox_read(
    name: &str,
    sandbox_path: &str,
    result: miette::Result<(String, i32)>,
) -> miette::Result<Option<IdentityFileSummary>> {
    let (mut content_preview, exit_code) = result?;
    if exit_code == 3 {
        return Ok(None);
    }
    if exit_code != 0 {
        return Err(miette::miette!(
            "sandbox identity read for {name} exited with code {exit_code}"
        ));
    }
    let truncated = content_preview.len() > IDENTITY_PREVIEW_LIMIT_BYTES;
    if truncated {
        right_dashboard::fs_safety::truncate_to_char_boundary(
            &mut content_preview,
            IDENTITY_PREVIEW_LIMIT_BYTES,
        );
    }
    Ok(Some(IdentityFileSummary {
        name: name.to_owned(),
        source: "sandbox".to_owned(),
        path: sandbox_path.to_owned(),
        exists: true,
        content_preview: Some(content_preview),
        truncated,
    }))
}

fn host_mirror_or_unavailable(
    agent_dir: &std::path::Path,
    name: &str,
) -> Result<IdentityFileSummary, IdentityFilesError> {
    read_host_identity_file(
        agent_dir,
        "host_mirror",
        "unavailable",
        name,
        IDENTITY_PREVIEW_LIMIT_BYTES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_to_char_boundary_handles_split_multibyte_suffix() {
        let mut value = format!("{}é", "a".repeat(IDENTITY_PREVIEW_LIMIT_BYTES - 1));

        right_dashboard::fs_safety::truncate_to_char_boundary(
            &mut value,
            IDENTITY_PREVIEW_LIMIT_BYTES,
        );

        assert_eq!(value.len(), IDENTITY_PREVIEW_LIMIT_BYTES - 1);
        assert!(value.ends_with('a'));
    }

    #[test]
    fn host_mirror_or_unavailable_labels_missing_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("IDENTITY.md"), "# Identity\n").unwrap();

        let existing = host_mirror_or_unavailable(temp.path(), "IDENTITY.md").unwrap();
        let missing = host_mirror_or_unavailable(temp.path(), "SOUL.md").unwrap();

        assert_eq!(existing.source, "host_mirror");
        assert_eq!(existing.content_preview.as_deref(), Some("# Identity\n"));
        assert_eq!(missing.source, "unavailable");
        assert!(!missing.exists);
    }
}
