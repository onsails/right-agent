//! OpenShell version preflight — verifies the installed CLI binary and
//! the running gateway are both new enough.
//!
//! Wired into bot startup via `crates/bot/src/lib.rs`. Hard-fails the
//! process on mismatch; no quiet degradation.

use semver::Version;
use tonic::transport::Channel;

use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;

/// Minimum supported OpenShell version (both CLI and gateway).
///
/// Below this, the provider gRPC surface is incomplete (no
/// `AttachSandboxProvider` etc.) and Right will not start.
pub const MIN_OPENSHELL_VERSION: Version = Version::new(0, 0, 50);

/// Preflight failure modes. Each variant carries enough context for an
/// actionable diagnostic written to `tracing::error!` before exit.
#[derive(Debug, thiserror::Error)]
pub enum PreflightError {
    #[error(
        "openshell CLI binary not found on PATH (install from https://github.com/NVIDIA/OpenShell)"
    )]
    CliMissing,
    #[error("could not parse `openshell --version` output: {0:?}")]
    CliVersionUnparseable(String),
    #[error(
        "openshell CLI is {found}, need >= {required}; run `brew upgrade openshell` (or your platform equivalent)"
    )]
    CliTooOld { found: Version, required: Version },
    #[error("openshell gateway Health RPC failed: {0}")]
    GatewayUnreachable(#[source] tonic::Status),
    #[error("could not parse openshell gateway version: {0:?}")]
    GatewayVersionUnparseable(String),
    #[error("openshell gateway is {found}, need >= {required}; upgrade your gateway")]
    GatewayTooOld { found: Version, required: Version },
}

/// Parse the output of `openshell --version`, which looks like:
///
/// ```text
/// openshell 0.0.50
/// ```
///
/// Returns the parsed `Version` or a parse-failure string suitable for
/// stuffing into [`PreflightError::CliVersionUnparseable`].
pub fn parse_openshell_cli_version(output: &str) -> Result<Version, String> {
    // Take the first non-empty line; strip a leading "openshell " prefix.
    let first = output
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| format!("could not parse empty output: {output:?}"))?;
    let trimmed = first.trim();
    let rest = trimmed
        .strip_prefix("openshell ")
        .ok_or_else(|| format!("could not parse, expected 'openshell X.Y.Z': {trimmed:?}"))?;
    Version::parse(rest.trim()).map_err(|e| format!("could not parse semver from {rest:?}: {e}"))
}

/// Pure helper used by [`cli_version_check`] and tests. Takes the raw
/// `openshell --version` stdout and returns Ok on `>= MIN_OPENSHELL_VERSION`,
/// `PreflightError::CliTooOld` if older, `CliVersionUnparseable` on garbage.
pub fn cli_version_check_str(output: &str) -> Result<(), PreflightError> {
    let found =
        parse_openshell_cli_version(output).map_err(PreflightError::CliVersionUnparseable)?;
    if found < MIN_OPENSHELL_VERSION {
        return Err(PreflightError::CliTooOld {
            found,
            required: MIN_OPENSHELL_VERSION,
        });
    }
    Ok(())
}

/// Spawn `openshell --version` and check against [`MIN_OPENSHELL_VERSION`].
/// Returns `CliMissing` if the binary isn't on PATH.
pub async fn cli_version_check() -> Result<(), PreflightError> {
    let out = tokio::process::Command::new("openshell")
        .arg("--version")
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                PreflightError::CliMissing
            } else {
                PreflightError::CliVersionUnparseable(format!("spawn failed: {e:#}"))
            }
        })?;
    if !out.status.success() {
        return Err(PreflightError::CliVersionUnparseable(format!(
            "openshell --version exited {} stderr={:?}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr),
        )));
    }
    cli_version_check_str(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
#[path = "preflight_tests.rs"]
mod tests;

// Note: `openshell_preflight` and the gateway Health probe are added
// in later tasks.
#[allow(dead_code)]
async fn _client_handle_for_grpc_module_export(_: OpenShellClient<Channel>) {}
