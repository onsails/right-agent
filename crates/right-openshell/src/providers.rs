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

/// Return the hardcoded catalog of known provider profiles.
///
/// `claude` and `outlook` are intentionally excluded:
/// - `claude` is the built-in Claude Code identity — not user-configurable.
/// - `outlook` was found on the gateway but is out of scope for Right.
///
/// `generic` is included as an escape hatch for any provider not in the list.
pub fn profile_catalog() -> Vec<ProviderProfile> {
    vec![
        ProviderProfile {
            type_slug: "anthropic".into(),
            display_name: "Anthropic API".into(),
            category: ProviderCategory::Inference,
            env_var: "ANTHROPIC_API_KEY".into(),
        },
        ProviderProfile {
            type_slug: "openai".into(),
            display_name: "OpenAI".into(),
            category: ProviderCategory::Inference,
            env_var: "OPENAI_API_KEY".into(),
        },
        ProviderProfile {
            type_slug: "nvidia".into(),
            display_name: "NVIDIA".into(),
            category: ProviderCategory::Inference,
            env_var: "NVIDIA_API_KEY".into(),
        },
        ProviderProfile {
            type_slug: "codex".into(),
            display_name: "Codex".into(),
            category: ProviderCategory::Agent,
            env_var: "OPENAI_API_KEY".into(),
        },
        ProviderProfile {
            type_slug: "copilot".into(),
            display_name: "GitHub Copilot".into(),
            category: ProviderCategory::Agent,
            env_var: "COPILOT_GITHUB_TOKEN".into(),
        },
        ProviderProfile {
            type_slug: "opencode".into(),
            display_name: "OpenCode".into(),
            category: ProviderCategory::Agent,
            env_var: "OPENCODE_API_KEY".into(),
        },
        ProviderProfile {
            type_slug: "github".into(),
            display_name: "GitHub".into(),
            category: ProviderCategory::SourceControl,
            env_var: "GITHUB_TOKEN".into(),
        },
        ProviderProfile {
            type_slug: "gitlab".into(),
            display_name: "GitLab".into(),
            category: ProviderCategory::SourceControl,
            env_var: "GITLAB_TOKEN".into(),
        },
        ProviderProfile {
            type_slug: "generic".into(),
            display_name: "Generic".into(),
            category: ProviderCategory::Other,
            env_var: String::new(),
        },
    ]
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

// ────────────────────────────────────────────────────────────────────────────
// CRUD wrappers
// ────────────────────────────────────────────────────────────────────────────

pub async fn create_provider(
    endpoint: &crate::openshell::GatewayEndpoint,
    spec: &ProviderSpec,
) -> Result<Provider, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args([
        "provider",
        "create",
        "--name",
        &spec.name,
        "--type",
        &spec.type_,
    ]);
    for (k, v) in &spec.credentials {
        cmd.arg("--credential").arg(format!("{k}={v}"));
    }
    for (k, v) in &spec.config {
        cmd.arg("--config").arg(format!("{k}={v}"));
    }
    cmd.arg("--output").arg("json");
    endpoint.apply_to_cli(&mut cmd);
    let out = run_cli(cmd, "openshell provider create").await?;
    parse_provider_json(&out)
}

pub async fn get_provider(
    endpoint: &crate::openshell::GatewayEndpoint,
    name: &str,
) -> Result<Provider, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["provider", "get", "--name", name, "--output", "json"]);
    endpoint.apply_to_cli(&mut cmd);
    let out = cmd
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .await
        .map_err(|e| ProviderError::Cli {
            cmd: "openshell provider get".into(),
            status: -1,
            stderr: e.to_string(),
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("not found") || stderr.contains("NotFound") {
            return Err(ProviderError::NotFound(name.to_string()));
        }
        return Err(ProviderError::Cli {
            cmd: "openshell provider get".into(),
            status: out.status.code().unwrap_or(-1),
            stderr: stderr.into_owned(),
        });
    }
    parse_provider_json(&out.stdout)
}

pub async fn update_provider(
    endpoint: &crate::openshell::GatewayEndpoint,
    spec: &ProviderSpec,
) -> Result<Provider, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["provider", "update", "--name", &spec.name]);
    for (k, v) in &spec.credentials {
        cmd.arg("--credential").arg(format!("{k}={v}"));
    }
    for (k, v) in &spec.config {
        cmd.arg("--config").arg(format!("{k}={v}"));
    }
    cmd.arg("--output").arg("json");
    endpoint.apply_to_cli(&mut cmd);
    let out = run_cli(cmd, "openshell provider update").await?;
    parse_provider_json(&out)
}

pub async fn delete_provider(
    endpoint: &crate::openshell::GatewayEndpoint,
    name: &str,
) -> Result<(), ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["provider", "delete", "--name", name, "--yes"]);
    endpoint.apply_to_cli(&mut cmd);
    let _ = run_cli(cmd, "openshell provider delete").await?;
    Ok(())
}

pub async fn list_providers_by_prefix(
    endpoint: &crate::openshell::GatewayEndpoint,
    prefix: &str,
) -> Result<Vec<Provider>, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["provider", "list", "--output", "json"]);
    endpoint.apply_to_cli(&mut cmd);
    let out = run_cli(cmd, "openshell provider list").await?;
    let arr: Vec<serde_json::Value> = serde_json::from_slice(&out)
        .map_err(|e| ProviderError::Grpc(format!("parse provider list: {e:#}")))?;
    let mut providers = Vec::new();
    for v in arr {
        if let Some(name) = v
            .get("metadata")
            .and_then(|m| m.get("name"))
            .and_then(|n| n.as_str())
            && name.starts_with(prefix)
        {
            providers.push(provider_from_json(&v)?);
        }
    }
    Ok(providers)
}

// ────────────────────────────────────────────────────────────────────────────
// Sandbox ↔ Provider attachment
// ────────────────────────────────────────────────────────────────────────────

/// Attach a provider to a sandbox so the sandbox can use its credentials.
///
/// Note: the JSON shape of `openshell sandbox provider list` has not been
/// confirmed against a live gateway. JSON parsing in `list_attached` will need
/// CI verification (Task 10b).
pub async fn attach_to_sandbox(
    endpoint: &crate::openshell::GatewayEndpoint,
    sandbox_name: &str,
    provider_name: &str,
) -> Result<(), ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["sandbox", "provider", "attach", sandbox_name, provider_name]);
    endpoint.apply_to_cli(&mut cmd);
    let _ = run_cli(cmd, "openshell sandbox provider attach").await?;
    Ok(())
}

/// Detach a provider from a sandbox.
pub async fn detach_from_sandbox(
    endpoint: &crate::openshell::GatewayEndpoint,
    sandbox_name: &str,
    provider_name: &str,
) -> Result<(), ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["sandbox", "provider", "detach", sandbox_name, provider_name]);
    endpoint.apply_to_cli(&mut cmd);
    let _ = run_cli(cmd, "openshell sandbox provider detach").await?;
    Ok(())
}

/// List provider names currently attached to a sandbox.
///
/// Note: JSON field path (`name`) assumed from `openshell sandbox provider list
/// --output json`. CI verification required (Task 10b).
pub async fn list_attached(
    endpoint: &crate::openshell::GatewayEndpoint,
    sandbox_name: &str,
) -> Result<Vec<String>, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args([
        "sandbox",
        "provider",
        "list",
        sandbox_name,
        "--output",
        "json",
    ]);
    endpoint.apply_to_cli(&mut cmd);
    let out = run_cli(cmd, "openshell sandbox provider list").await?;
    let arr: Vec<serde_json::Value> = serde_json::from_slice(&out)
        .map_err(|e| ProviderError::Grpc(format!("parse attached: {e:#}")))?;
    let mut names = Vec::new();
    for v in arr {
        if let Some(n) = v.get("name").and_then(|s| s.as_str()) {
            names.push(n.to_string());
        }
    }
    Ok(names)
}

async fn run_cli(mut cmd: Command, label: &str) -> Result<Vec<u8>, ProviderError> {
    let out = cmd
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .await
        .map_err(|e| ProviderError::Cli {
            cmd: label.into(),
            status: -1,
            stderr: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(ProviderError::Cli {
            cmd: label.into(),
            status: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(out.stdout)
}

fn parse_provider_json(bytes: &[u8]) -> Result<Provider, ProviderError> {
    let v: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::Grpc(format!("parse provider: {e:#}")))?;
    provider_from_json(&v)
}

fn provider_from_json(v: &serde_json::Value) -> Result<Provider, ProviderError> {
    let name = v
        .get("metadata")
        .and_then(|m| m.get("name"))
        .and_then(|n| n.as_str())
        .ok_or_else(|| ProviderError::Grpc("missing metadata.name".into()))?;
    let type_ = v
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or_else(|| ProviderError::Grpc("missing type".into()))?;
    let mut config = HashMap::new();
    if let Some(obj) = v.get("config").and_then(|c| c.as_object()) {
        for (k, val) in obj {
            if let Some(s) = val.as_str() {
                config.insert(k.clone(), s.to_string());
            }
        }
    }
    let updated_at = v
        .get("metadata")
        .and_then(|m| m.get("updated_at"))
        .and_then(|u| u.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));
    Ok(Provider {
        name: name.to_string(),
        type_: type_.to_string(),
        config,
        updated_at,
    })
}

// ────────────────────────────────────────────────────────────────────────────
// gRPC wrappers
// ────────────────────────────────────────────────────────────────────────────

/// Fetch the env-var map that will be injected into the sandbox.
///
/// SAFETY: the returned values are opaque placeholders (e.g.
/// `openshell:resolve:env:v<digits>_<NAME>`) — never log them. They look
/// secret-shaped to operators and create false alarms in audits.
pub async fn get_sandbox_provider_environment(
    _endpoint: &crate::openshell::GatewayEndpoint,
    sandbox_id: &str,
) -> Result<HashMap<String, String>, ProviderError> {
    // TODO: thread GatewayEndpoint override into connect_grpc when needed.
    // For now, connect_grpc reads OPENSHELL_GATEWAY_ENDPOINT itself.
    let mtls_dir = crate::openshell::default_mtls_dir();
    let mut client = crate::openshell::connect_grpc(&mtls_dir)
        .await
        .map_err(ProviderError::GatewayUnreachable)?;
    let resp = client
        .get_sandbox_provider_environment(
            crate::openshell_proto::openshell::v1::GetSandboxProviderEnvironmentRequest {
                sandbox_id: sandbox_id.to_owned(),
            },
        )
        .await
        .map_err(|s| ProviderError::Grpc(format!("{s:#}")))?
        .into_inner();
    Ok(resp.environment)
}

// ────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_excludes_claude() {
        let catalog = profile_catalog();
        assert!(!catalog.iter().any(|p| p.type_slug == "claude"));
    }

    #[test]
    fn catalog_has_8_built_in_plus_generic() {
        let catalog = profile_catalog();
        let built_in: Vec<&str> = catalog
            .iter()
            .filter(|p| p.type_slug != "generic")
            .map(|p| p.type_slug.as_str())
            .collect();
        assert_eq!(built_in.len(), 8);
        for expected in [
            "anthropic",
            "codex",
            "copilot",
            "github",
            "gitlab",
            "nvidia",
            "openai",
            "opencode",
        ] {
            assert!(built_in.contains(&expected), "missing {expected}");
        }
        assert!(catalog.iter().any(|p| p.type_slug == "generic"));
    }

    #[test]
    fn catalog_anthropic_uses_anthropic_api_key() {
        let entry = profile_catalog()
            .into_iter()
            .find(|p| p.type_slug == "anthropic")
            .unwrap();
        assert_eq!(entry.env_var, "ANTHROPIC_API_KEY");
    }
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
