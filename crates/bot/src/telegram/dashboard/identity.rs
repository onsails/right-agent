use std::time::Duration;

use crate::sandbox::{Sandbox, exec_argv};
use right_dashboard::api_types::{IdentityFileResponse, IdentityFileSummary, IdentityResponse};
use right_dashboard::identity_files::{
    IDENTITY_FILE_NAMES, IdentityFilesError, read_host_identity_file, validate_identity_file_name,
};

use super::DashboardState;

mod identity_parse;

use identity_parse::{STATE_SANDBOX_UNREACHABLE, identity_state, parse_combined_identity_read};

const IDENTITY_PREVIEW_LIMIT_BYTES: usize = 64 * 1024;

/// Combined read of identity files in a single round trip. `$1` is the byte
/// limit per file (the caller passes `IDENTITY_PREVIEW_LIMIT_BYTES + 1` so the
/// parser can detect truncation). `$2` is an optional single-file filter: when
/// non-empty, only that file is read (the per-file detail route needs one file,
/// not all three). Each emitted file is a header line
/// `RIGHT_IDENTITY <name> <PRESENT|ABSENT> <byte_count>\n`; present files are
/// followed by exactly `byte_count` content bytes and a trailing `\n`. The
/// file list is hardcoded here to mirror `IDENTITY_FILE_NAMES` — keep both in
/// sync.
const SANDBOX_READ_IDENTITY_SCRIPT: &str = r#"limit="$1"
only="$2"
for f in IDENTITY.md SOUL.md USER.md; do
  if [ -n "$only" ] && [ "$f" != "$only" ]; then continue; fi
  p="/sandbox/$f"
  if [ -e "$p" ] && [ ! -L "$p" ] && [ -f "$p" ]; then
    n=$(head -c "$limit" "$p" | wc -c | tr -d ' ')
    printf 'RIGHT_IDENTITY %s PRESENT %s\n' "$f" "$n"
    head -c "$limit" "$p"
    printf '\n'
  else
    printf 'RIGHT_IDENTITY %s ABSENT 0\n' "$f"
  fi
done"#;

pub(super) async fn identity_response(
    state: &DashboardState,
) -> Result<IdentityResponse, IdentityFilesError> {
    // Every agent is sandboxed, so startup always captures a sandbox handle.
    // A missing one means the sandbox never came up: report it as unreachable
    // rather than passing host files off as live.
    let Some(sandbox) = state.sandbox() else {
        return host_mirror_unreachable_response(
            &state.agent_name,
            &state.agent_dir,
            &miette::miette!("sandbox handle unavailable"),
        )
        .map_err(|error| IdentityFilesError::Io(std::io::Error::other(format!("{error:#}"))));
    };
    // The combined read maps its own failure to a `sandbox_unreachable`
    // response, so any `Err` here is a host-mirror read failure that must
    // propagate rather than masquerade as unreachable.
    read_sandbox_identity_files(&state.agent_name, &state.agent_dir, &sandbox)
        .await
        .map_err(|error| IdentityFilesError::Io(std::io::Error::other(format!("{error:#}"))))
}

pub(super) async fn identity_file_response(
    state: &DashboardState,
    file_name: &str,
) -> Result<IdentityFileResponse, IdentityFilesError> {
    validate_identity_file_name(file_name)?;
    let (file, warning) = match state.sandbox() {
        // No sandbox handle: the sandbox never came up. Same shape as an
        // unreachable sandbox — host mirror, clearly labelled.
        None => (
            read_host_identity_file(
                &state.agent_dir,
                STATE_SANDBOX_UNREACHABLE,
                STATE_SANDBOX_UNREACHABLE,
                file_name,
                IDENTITY_PREVIEW_LIMIT_BYTES,
            )?,
            Some("sandbox unreachable; showing host mirror when present".to_owned()),
        ),
        Some(sandbox) => match read_sandbox_identity_file(&sandbox, file_name).await {
            Ok(Some(file)) => (file, None),
            // Absent in the sandbox: show the host mirror when present
            // (host_mirror) otherwise mark it not_authored.
            Ok(None) => (
                read_host_identity_file(
                    &state.agent_dir,
                    identity_state(false, true),
                    identity_state(false, false),
                    file_name,
                    IDENTITY_PREVIEW_LIMIT_BYTES,
                )?,
                None,
            ),
            // Sandbox unreachable: show the host mirror but label it as such.
            Err(error) => (
                read_host_identity_file(
                    &state.agent_dir,
                    STATE_SANDBOX_UNREACHABLE,
                    STATE_SANDBOX_UNREACHABLE,
                    file_name,
                    IDENTITY_PREVIEW_LIMIT_BYTES,
                )?,
                Some(format!(
                    "sandbox unreachable; showing host mirror when present: {error:#}"
                )),
            ),
        },
    };
    Ok(IdentityFileResponse {
        agent: state.agent_name.clone(),
        warning,
        file,
    })
}

/// Read every identity file from the sandbox in a single round trip.
///
/// On success each file is mapped to one of `sandbox` (live, present in the
/// sandbox), `host_mirror` (absent in sandbox but a host debug mirror exists),
/// or `not_authored` (absent everywhere). When the single combined read fails
/// — timeout, exec error, or non-zero exit — the sandbox is unreachable as a
/// whole: every file is labelled `sandbox_unreachable` and host-mirror content
/// is shown (clearly the mirror, never claimed live).
async fn read_sandbox_identity_files(
    agent_name: &str,
    agent_dir: &std::path::Path,
    sandbox: &Sandbox,
) -> miette::Result<IdentityResponse> {
    let stdout = match run_combined_identity_read(sandbox, None).await {
        Ok(stdout) => stdout,
        Err(error) => return host_mirror_unreachable_response(agent_name, agent_dir, &error),
    };

    let parsed = parse_combined_identity_read(&stdout, IDENTITY_PREVIEW_LIMIT_BYTES);
    let mut files = Vec::with_capacity(IDENTITY_FILE_NAMES.len());
    let mut all_live = true;

    for name in IDENTITY_FILE_NAMES {
        match parsed.iter().find(|file| file.name == name) {
            Some(file) if file.present => {
                files.push(IdentityFileSummary {
                    name: name.to_owned(),
                    source: identity_state(true, false).to_owned(),
                    path: format!("/sandbox/{name}"),
                    exists: true,
                    content_preview: Some(file.content.clone()),
                    truncated: file.truncated,
                });
            }
            // Absent in the sandbox (or omitted from the framing): fall back to
            // the host mirror. `read_host_identity_file` probes presence and
            // labels host_mirror when present, not_authored when missing.
            _ => {
                files.push(
                    read_host_identity_file(
                        agent_dir,
                        identity_state(false, true),
                        identity_state(false, false),
                        name,
                        IDENTITY_PREVIEW_LIMIT_BYTES,
                    )
                    .map_err(|error| {
                        miette::miette!("host mirror read failed for {name}: {error:#}")
                    })?,
                );
                all_live = false;
            }
        }
    }

    Ok(IdentityResponse {
        agent: agent_name.to_owned(),
        source: if all_live {
            "sandbox".to_owned()
        } else {
            "mixed".to_owned()
        },
        warning: None,
        files,
    })
}

/// Run the combined identity read with the dashboard sandbox timeout, mapping
/// timeout and non-zero exit into an error so the caller drops to the
/// `sandbox_unreachable` branch. `name_filter` reads only that one file when
/// `Some` (the per-file detail route), all files when `None`.
async fn run_combined_identity_read(
    sandbox: &Sandbox,
    name_filter: Option<&str>,
) -> miette::Result<String> {
    let limit = (IDENTITY_PREVIEW_LIMIT_BYTES + 1).to_string();
    let command = [
        "sh",
        "-c",
        SANDBOX_READ_IDENTITY_SCRIPT,
        "dashboard-identity-read",
        limit.as_str(),
        name_filter.unwrap_or(""),
    ];
    let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_TIMEOUT_SECS);
    let run = exec_argv(sandbox, &command);
    let (stdout, exit_code) = match tokio::time::timeout(timeout, run).await {
        Ok(result) => result?,
        Err(_) => return Err(miette::miette!("sandbox identity read timed out")),
    };
    if exit_code != 0 {
        return Err(miette::miette!(
            "sandbox identity read exited with code {exit_code}"
        ));
    }
    Ok(stdout)
}

/// Build the whole-panel response when the sandbox could not be read at all.
/// Every file is labelled `sandbox_unreachable`; host-mirror content is shown
/// when present but is never presented as live.
fn host_mirror_unreachable_response(
    agent_name: &str,
    agent_dir: &std::path::Path,
    error: &miette::Report,
) -> miette::Result<IdentityResponse> {
    let mut files = Vec::with_capacity(IDENTITY_FILE_NAMES.len());
    for name in IDENTITY_FILE_NAMES {
        files.push(
            read_host_identity_file(
                agent_dir,
                STATE_SANDBOX_UNREACHABLE,
                STATE_SANDBOX_UNREACHABLE,
                name,
                IDENTITY_PREVIEW_LIMIT_BYTES,
            )
            .map_err(|error| miette::miette!("host mirror read failed for {name}: {error:#}"))?,
        );
    }
    Ok(IdentityResponse {
        agent: agent_name.to_owned(),
        source: STATE_SANDBOX_UNREACHABLE.to_owned(),
        warning: Some(format!(
            "sandbox unreachable; showing host mirror: {error:#}"
        )),
        files,
    })
}

/// Read a single identity file from the sandbox via the combined read.
/// `Ok(Some(_))` = present in the sandbox; `Ok(None)` = absent (caller maps to
/// host_mirror/not_authored); `Err(_)` = sandbox unreachable.
async fn read_sandbox_identity_file(
    sandbox: &Sandbox,
    name: &str,
) -> miette::Result<Option<IdentityFileSummary>> {
    validate_identity_file_name(name).map_err(|error| miette::miette!("{error:#}"))?;
    let stdout = run_combined_identity_read(sandbox, Some(name)).await?;
    let parsed = parse_combined_identity_read(&stdout, IDENTITY_PREVIEW_LIMIT_BYTES);
    match parsed.iter().find(|file| file.name == name) {
        Some(file) if file.present => Ok(Some(IdentityFileSummary {
            name: name.to_owned(),
            source: identity_state(true, false).to_owned(),
            path: format!("/sandbox/{name}"),
            exists: true,
            content_preview: Some(file.content.clone()),
            truncated: file.truncated,
        })),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_read_script_covers_every_identity_file_name() {
        for name in IDENTITY_FILE_NAMES {
            assert!(
                SANDBOX_READ_IDENTITY_SCRIPT.contains(name),
                "combined-read script is missing identity file `{name}`; \
                 keep the script's hardcoded list in sync with IDENTITY_FILE_NAMES",
            );
        }
    }

    #[tokio::test]
    async fn truncate_to_char_boundary_handles_split_multibyte_suffix() {
        let mut value = format!("{}é", "a".repeat(IDENTITY_PREVIEW_LIMIT_BYTES - 1));

        right_dashboard::fs_safety::truncate_to_char_boundary(
            &mut value,
            IDENTITY_PREVIEW_LIMIT_BYTES,
        );

        assert_eq!(value.len(), IDENTITY_PREVIEW_LIMIT_BYTES - 1);
        assert!(value.ends_with('a'));
    }

    #[tokio::test]
    async fn unreachable_response_labels_every_file_sandbox_unreachable() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("IDENTITY.md"), "# Identity\n").unwrap();

        let response =
            host_mirror_unreachable_response("alpha", temp.path(), &miette::miette!("boom"))
                .unwrap();

        assert_eq!(response.source, "sandbox_unreachable");
        assert!(response.warning.is_some());
        assert!(
            response
                .files
                .iter()
                .all(|file| file.source == "sandbox_unreachable")
        );
        // Host-mirror content is still shown for files that exist on the host.
        let identity = response
            .files
            .iter()
            .find(|file| file.name == "IDENTITY.md")
            .unwrap();
        assert_eq!(identity.content_preview.as_deref(), Some("# Identity\n"));
        assert!(identity.exists);
        // Missing files are present-but-empty in the list, still unreachable.
        let soul = response
            .files
            .iter()
            .find(|file| file.name == "SOUL.md")
            .unwrap();
        assert!(!soul.exists);
        assert_eq!(soul.source, "sandbox_unreachable");
    }
}
