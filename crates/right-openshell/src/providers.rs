//! OpenShell Provider gRPC + CLI wrappers.
//!
//! This module is the SOLE owner of the OpenShell Provider client.
//! All Provider RPCs and `openshell provider` / `openshell sandbox provider`
//! CLI invocations go through here (see ARCHITECTURE.md).

use std::collections::HashMap;
use std::process::Stdio;

use thiserror::Error;
use tokio::process::Command;

/// All provider operation errors. Each is FAIL FAST — never swallowed.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider gateway unreachable: {0:#}")]
    GatewayUnreachable(miette::ErrReport),
    #[error("openshell gRPC: {0:#}")]
    Grpc(String),
    #[error("openshell CLI {cmd:?} exited {status}: {stderr}")]
    Cli {
        cmd: String,
        status: i32,
        stderr: String,
    },
    #[error("provider \"{0}\" not found")]
    NotFound(String),
    #[error("providers_v2_enabled is not on; run `right up` to enable")]
    V2NotEnabled,
    #[error("invalid provider: {0}")]
    Invalid(String),
}

/// Input for create/update.
#[derive(Debug, Clone)]
pub struct ProviderSpec {
    pub name: String,
    pub type_: String, // raw slug
    pub credentials: HashMap<String, String>,
    pub config: HashMap<String, String>,
}

/// Output of get/list. Credentials field is INTENTIONALLY OMITTED — the
/// gateway returns them, but Right never reads or stores them on host.
#[derive(Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub type_: String,
    pub config: HashMap<String, String>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Profile entry surfaced by `/provider-types` to the dashboard.
#[derive(Debug, Clone)]
pub struct ProviderProfile {
    pub type_slug: String,
    pub env_var: String,
    pub display_name: String,
    pub category: ProviderCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCategory {
    Inference,
    Agent,
    SourceControl,
    Messaging,
    Other,
}

/// Return value of [`ensure_v2_enabled`].
pub struct V2EnableResult {
    pub was_already_on: bool,
}

/// Ensure `providers_v2_enabled=true` on the gateway. Idempotent.
///
/// Reads the current flag first; if already `true`, returns without running
/// `settings set`.  A second call on the same gateway will always see
/// `was_already_on = true`.
pub async fn ensure_v2_enabled(
    endpoint: &crate::openshell::GatewayEndpoint,
) -> Result<V2EnableResult, ProviderError> {
    let current = get_v2_flag(endpoint).await?;
    if current {
        return Ok(V2EnableResult {
            was_already_on: true,
        });
    }
    let mut cmd = Command::new("openshell");
    cmd.args([
        "settings",
        "set",
        "--global",
        "--key",
        "providers_v2_enabled",
        "--value",
        "true",
        "--yes",
    ]);
    endpoint.apply_to_cli(&mut cmd);
    let output = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| ProviderError::Cli {
            cmd: "openshell settings set".into(),
            status: -1,
            stderr: e.to_string(),
        })?;
    if !output.status.success() {
        return Err(ProviderError::Cli {
            cmd: "openshell settings set".into(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(V2EnableResult {
        was_already_on: false,
    })
}

async fn get_v2_flag(endpoint: &crate::openshell::GatewayEndpoint) -> Result<bool, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args([
        "settings",
        "get",
        "--global",
        "--key",
        "providers_v2_enabled",
    ]);
    endpoint.apply_to_cli(&mut cmd);
    let out = cmd.output().await.map_err(|e| ProviderError::Cli {
        cmd: "openshell settings get".into(),
        status: -1,
        stderr: e.to_string(),
    })?;
    if !out.status.success() {
        return Ok(false);
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout.contains("true"))
}
