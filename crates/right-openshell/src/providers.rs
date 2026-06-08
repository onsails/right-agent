//! OpenShell Provider gRPC wrappers.
//!
//! This module is the SOLE owner of the OpenShell Provider client.
//! All Provider RPCs go through here (see ARCHITECTURE.md).

use std::collections::HashMap;

use thiserror::Error;
use tonic::transport::Channel;

use crate::openshell_proto::openshell::datamodel::v1 as datamodel;
use crate::openshell_proto::openshell::sandbox::v1 as sandbox_v1;
use crate::openshell_proto::openshell::v1 as proto_v1;
use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;

/// Gateway-global setting key that gates provider-profile network-endpoint
/// composition. Fresh Linux gateways default this to `false`; without it set
/// to `true`, generic-provider credential substitution silently fails (the
/// proxy denies CONNECT because the terminated endpoint is never composed).
pub const PROVIDERS_V2_ENABLED_KEY: &str = "providers_v2_enabled";

/// All provider operation errors. Each is FAIL FAST — never swallowed.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider gateway unreachable: {0:#}")]
    GatewayUnreachable(miette::ErrReport),
    #[error("openshell gRPC: {0:#}")]
    Grpc(String),
    #[error("provider \"{0}\" not found")]
    NotFound(String),
    #[error("invalid provider: {0}")]
    Invalid(String),
}

/// Input for create/update.
#[derive(Clone)]
pub struct ProviderSpec {
    pub name: String,
    pub type_: String, // raw slug
    pub credentials: HashMap<String, String>,
    pub config: HashMap<String, String>,
}

impl std::fmt::Debug for ProviderSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ProviderSpec {{ name: {:?}, type_: {:?}, credentials: <{} redacted>, config: {:?} }}",
            self.name,
            self.type_,
            self.credentials.len(),
            self.config,
        )
    }
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
            type_slug: "right-github".into(),
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

// ────────────────────────────────────────────────────────────────────────────
// gRPC conversion helpers
// ────────────────────────────────────────────────────────────────────────────

/// Map a `tonic::Status` to a `ProviderError`, preserving NotFound
/// semantics. `name_for_not_found` is the resource identifier used in
/// the `ProviderError::NotFound(name)` variant.
fn classify_status(status: tonic::Status, name_for_not_found: &str) -> ProviderError {
    if status.code() == tonic::Code::NotFound {
        ProviderError::NotFound(name_for_not_found.to_string())
    } else {
        ProviderError::Grpc(format!("{}: {}", status.code(), status.message()))
    }
}

/// Convert a wire-level [`datamodel::Provider`] into the host-facing
/// [`Provider`] struct. Credentials and `credential_expires_at_ms` are
/// intentionally dropped — Right never persists them on host.
fn provider_from_proto(p: datamodel::Provider) -> Provider {
    let metadata = p.metadata.unwrap_or_default();
    let updated_at = parse_object_meta_updated_at(&metadata);
    Provider {
        name: metadata.name,
        type_: p.r#type,
        config: p.config,
        updated_at,
    }
}

/// Derive a `DateTime<Utc>` from `ObjectMeta.created_at_ms` (int64
/// milliseconds since Unix epoch). v0.0.50 `ObjectMeta` does NOT have
/// an `updated_at` field — there is no last-modified timestamp on the
/// gateway. `Provider.updated_at` therefore holds the creation time.
fn parse_object_meta_updated_at(
    meta: &datamodel::ObjectMeta,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if meta.created_at_ms <= 0 {
        return None;
    }
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(meta.created_at_ms)
}

/// Build a `datamodel::Provider` payload from a host-facing `ProviderSpec`.
fn proto_provider_from_spec(spec: &ProviderSpec) -> datamodel::Provider {
    datamodel::Provider {
        metadata: Some(datamodel::ObjectMeta {
            name: spec.name.clone(),
            ..Default::default()
        }),
        r#type: spec.type_.clone(),
        credentials: spec.credentials.clone(),
        config: spec.config.clone(),
        credential_expires_at_ms: HashMap::new(),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// CRUD wrappers
// ────────────────────────────────────────────────────────────────────────────

pub async fn create_provider(
    client: &mut OpenShellClient<Channel>,
    spec: &ProviderSpec,
) -> Result<Provider, ProviderError> {
    create_provider_proto(client, proto_provider_from_spec(spec)).await
}

async fn create_provider_proto(
    client: &mut OpenShellClient<Channel>,
    provider: datamodel::Provider,
) -> Result<Provider, ProviderError> {
    let name = provider
        .metadata
        .as_ref()
        .map(|m| m.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "<unnamed-provider>".to_string());
    let req = proto_v1::CreateProviderRequest {
        provider: Some(provider),
    };
    let resp = client
        .create_provider(req)
        .await
        .map_err(|s| classify_status(s, &name))?
        .into_inner();
    let p = resp.provider.ok_or_else(|| {
        ProviderError::Grpc("CreateProvider: missing provider in response".into())
    })?;
    Ok(provider_from_proto(p))
}

pub async fn get_provider(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> Result<Provider, ProviderError> {
    get_provider_proto(client, name)
        .await
        .map(provider_from_proto)
}

async fn get_provider_proto(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> Result<datamodel::Provider, ProviderError> {
    let req = proto_v1::GetProviderRequest {
        name: name.to_string(),
    };
    let resp = client
        .get_provider(req)
        .await
        .map_err(|s| classify_status(s, name))?
        .into_inner();
    resp.provider
        .ok_or_else(|| ProviderError::Grpc("GetProvider: missing provider in response".into()))
}

pub async fn update_provider(
    client: &mut OpenShellClient<Channel>,
    spec: &ProviderSpec,
) -> Result<Provider, ProviderError> {
    let req = proto_v1::UpdateProviderRequest {
        provider: Some(proto_provider_from_spec(spec)),
        credential_expires_at_ms: HashMap::new(),
    };
    let resp = client
        .update_provider(req)
        .await
        .map_err(|s| classify_status(s, &spec.name))?
        .into_inner();
    let p = resp.provider.ok_or_else(|| {
        ProviderError::Grpc("UpdateProvider: missing provider in response".into())
    })?;
    Ok(provider_from_proto(p))
}

pub async fn delete_provider(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> Result<(), ProviderError> {
    let req = proto_v1::DeleteProviderRequest {
        name: name.to_string(),
    };
    client
        .delete_provider(req)
        .await
        .map_err(|s| classify_status(s, name))?;
    Ok(())
}

pub async fn list_providers_by_prefix(
    client: &mut OpenShellClient<Channel>,
    prefix: &str,
) -> Result<Vec<Provider>, ProviderError> {
    let resp = client
        .list_providers(proto_v1::ListProvidersRequest {
            // 0 = server default (full list); explicit pagination is not
            // required for typical per-agent fan-out (< few dozen).
            limit: 0,
            offset: 0,
        })
        .await
        .map_err(|s| classify_status(s, "<list>"))?
        .into_inner();
    let mut out = Vec::with_capacity(resp.providers.len());
    for p in resp.providers {
        let name = p
            .metadata
            .as_ref()
            .map(|m| m.name.as_str())
            .unwrap_or_default();
        if name.starts_with(prefix) {
            out.push(provider_from_proto(p));
        }
    }
    Ok(out)
}

// ────────────────────────────────────────────────────────────────────────────
// Sandbox ↔ Provider attachment
// ────────────────────────────────────────────────────────────────────────────

/// Attach a provider to a sandbox so the sandbox can use its credentials.
pub async fn attach_to_sandbox(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    provider_name: &str,
) -> Result<(), ProviderError> {
    let req = proto_v1::AttachSandboxProviderRequest {
        sandbox_name: sandbox_name.to_string(),
        provider_name: provider_name.to_string(),
        expected_resource_version: 0,
    };
    client
        .attach_sandbox_provider(req)
        .await
        .map_err(|s| classify_status(s, provider_name))?;
    Ok(())
}

/// Detach a provider from a sandbox.
pub async fn detach_from_sandbox(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    provider_name: &str,
) -> Result<(), ProviderError> {
    let req = proto_v1::DetachSandboxProviderRequest {
        sandbox_name: sandbox_name.to_string(),
        provider_name: provider_name.to_string(),
        expected_resource_version: 0,
    };
    client
        .detach_sandbox_provider(req)
        .await
        .map_err(|s| classify_status(s, provider_name))?;
    Ok(())
}

/// List provider names currently attached to a sandbox.
pub async fn list_attached(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
) -> Result<Vec<String>, ProviderError> {
    let req = proto_v1::ListSandboxProvidersRequest {
        sandbox_name: sandbox_name.to_string(),
    };
    let resp = client
        .list_sandbox_providers(req)
        .await
        .map_err(|s| classify_status(s, sandbox_name))?
        .into_inner();
    Ok(resp
        .providers
        .into_iter()
        .filter_map(|p| p.metadata.map(|m| m.name))
        .collect())
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
    client: &mut OpenShellClient<Channel>,
    sandbox_id: &str,
) -> Result<HashMap<String, String>, ProviderError> {
    let req = proto_v1::GetSandboxProviderEnvironmentRequest {
        sandbox_id: sandbox_id.to_string(),
    };
    let resp = client
        .get_sandbox_provider_environment(req)
        .await
        .map_err(|s| classify_status(s, sandbox_id))?
        .into_inner();
    Ok(resp.environment)
}

/// Enable the gateway-global `providers_v2_enabled` setting so
/// provider-profile network-endpoint composition (and therefore generic
/// credential substitution) works.
///
/// Idempotent: unconditionally upserts `true` via `UpdateConfig` at global
/// scope. Safe to call on every `right up`. FAIL FAST — any RPC error
/// propagates with its chain preserved.
pub async fn ensure_v2_enabled(client: &mut OpenShellClient<Channel>) -> Result<(), ProviderError> {
    let req = proto_v1::UpdateConfigRequest {
        global: true,
        setting_key: PROVIDERS_V2_ENABLED_KEY.to_string(),
        setting_value: Some(sandbox_v1::SettingValue {
            value: Some(sandbox_v1::setting_value::Value::BoolValue(true)),
        }),
        ..Default::default()
    };
    client
        .update_config(req)
        .await
        .map_err(|s| classify_status(s, PROVIDERS_V2_ENABLED_KEY))?;
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Startup reconciler
// ────────────────────────────────────────────────────────────────────────────

/// Report from a provider reconcile pass.
pub struct ReconcileReport {
    /// Providers that were attached during this pass (were missing from sandbox).
    pub attached: Vec<String>,
    /// Providers that were detached during this pass (were attached but not declared).
    pub detached: Vec<String>,
    /// Declared existing providers whose legacy gateway type was repaired.
    pub repaired: Vec<String>,
    /// Declared providers that do not exist on the gateway (not yet created).
    pub missing: Vec<String>,
    /// Per-provider errors encountered during attach/detach/get/update. Each entry is
    /// `(provider_name, formatted_error)`. Reconcile continues past these so
    /// a single transient failure does not sink the whole pass.
    pub errors: Vec<(String, String)>,
}

/// Returns a recreate payload for a provider whose gateway `type` is still the
/// legacy `"generic"` built-in slug instead of the Right-managed profile ID.
///
/// OpenShell rejects `UpdateProvider` type changes, so repair must delete and
/// recreate the provider. The recreate payload carries the gateway-held
/// credentials/config returned by `GetProvider` and strips gateway-owned
/// metadata (`id`, timestamps, resource_version) before `CreateProvider`.
fn legacy_generic_provider_recreate_payload(
    name: &str,
    provider: &datamodel::Provider,
) -> Option<datamodel::Provider> {
    if provider.r#type != "generic" {
        return None;
    }

    Some(provider_recreate_payload_with_type(
        name,
        provider,
        crate::managed_profiles::generic_provider_profile_id(name),
    ))
}

fn provider_recreate_payload_with_type(
    name: &str,
    provider: &datamodel::Provider,
    type_: String,
) -> datamodel::Provider {
    let labels = provider
        .metadata
        .as_ref()
        .map(|m| m.labels.clone())
        .unwrap_or_default();
    datamodel::Provider {
        metadata: Some(datamodel::ObjectMeta {
            name: name.to_string(),
            labels,
            ..Default::default()
        }),
        r#type: type_,
        credentials: provider.credentials.clone(),
        config: provider.config.clone(),
        credential_expires_at_ms: provider.credential_expires_at_ms.clone(),
    }
}

async fn restore_legacy_provider_after_failed_recreate(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    name: &str,
    original: &datamodel::Provider,
    was_attached: bool,
) -> String {
    let original_payload =
        provider_recreate_payload_with_type(name, original, original.r#type.clone());
    match create_provider_proto(client, original_payload).await {
        Ok(_) => {
            if was_attached {
                match attach_to_sandbox(client, sandbox_name, name).await {
                    Ok(()) => "; rollback restored original provider and attachment".to_string(),
                    Err(e) => format!(
                        "; rollback restored original provider but failed to reattach: {e:#}"
                    ),
                }
            } else {
                "; rollback restored original provider".to_string()
            }
        }
        Err(e) => format!("; rollback failed to restore original provider: {e:#}"),
    }
}

async fn reattach_after_failed_repair_delete(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    name: &str,
    was_attached: bool,
) -> String {
    if !was_attached {
        return String::new();
    }

    match attach_to_sandbox(client, sandbox_name, name).await {
        Ok(()) => "; rollback restored original attachment".to_string(),
        Err(e) => format!("; rollback failed to restore original attachment: {e:#}"),
    }
}

async fn recreate_legacy_generic_provider(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    name: &str,
    original: &datamodel::Provider,
    was_attached: bool,
) -> Result<(), String> {
    let repaired = legacy_generic_provider_recreate_payload(name, original)
        .expect("caller must pass a legacy generic provider");

    if was_attached {
        detach_from_sandbox(client, sandbox_name, name)
            .await
            .map_err(|e| format!("repair-detach: {e:#}"))?;
    }

    if let Err(e) = delete_provider(client, name).await {
        let rollback =
            reattach_after_failed_repair_delete(client, sandbox_name, name, was_attached).await;
        return Err(format!("repair-delete: {e:#}{rollback}"));
    }

    if let Err(e) = create_provider_proto(client, repaired).await {
        let rollback = restore_legacy_provider_after_failed_recreate(
            client,
            sandbox_name,
            name,
            original,
            was_attached,
        )
        .await;
        return Err(format!("repair-create: {e:#}{rollback}"));
    }

    Ok(())
}

/// Reconcile the set of providers attached to `sandbox_name` with the
/// `declared` list from `agent.yaml`.
///
/// - Attaches any declared provider that exists on the gateway but is not yet
///   attached to the sandbox.
/// - Detaches any provider whose name starts with `<agent_prefix>-` that is
///   currently attached but is not in `declared` (stale after a config change).
/// - Records providers that are declared but not yet created on the gateway
///   in `missing` (not an error — they may be created later by the user).
///
/// The function is idempotent: calling it when everything is already in sync
/// produces an empty report after checking the current sandbox attachments and,
/// when declarations are non-empty, ensuring provider v2 is enabled.
///
/// **Partial-failure semantics**: transient attach/detach/get/update errors for
/// individual providers are collected into `ReconcileReport::errors` and the
/// loop continues. `ensure_v2_enabled` failure for a non-empty declaration and
/// `list_attached` failure return `Err`: without the global composition flag
/// and attached set we cannot make safe decisions. Callers should log
/// `report.errors` and schedule a retry so the bot converges on the next
/// reconcile tick.
pub async fn reconcile_for_sandbox(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    agent_prefix: &str,
    declared: &[String],
) -> Result<ReconcileReport, ProviderError> {
    // Provider composition is gated by the gateway-global providers_v2_enabled
    // flag (default false on fresh gateways). Guarantee it before any attach so
    // composition is not silently skipped. Skip when nothing is declared: a
    // detach-only reconcile needs no composition and must not fail on this.
    if !declared.is_empty() {
        ensure_v2_enabled(client).await?;
    }

    let attached = list_attached(client, sandbox_name).await?;
    let declared_set: std::collections::HashSet<&String> = declared.iter().collect();
    let attached_set: std::collections::HashSet<&String> = attached.iter().collect();
    let mut report = ReconcileReport {
        attached: vec![],
        detached: vec![],
        repaired: vec![],
        missing: vec![],
        errors: vec![],
    };
    // Attach declared providers that exist on the gateway but are not yet attached.
    for name in declared {
        match get_provider_proto(client, name).await {
            Ok(provider) => {
                let already_attached = attached_set.contains(name);
                let mut needs_reattach = false;
                if legacy_generic_provider_recreate_payload(name, &provider).is_some() {
                    match recreate_legacy_generic_provider(
                        client,
                        sandbox_name,
                        name,
                        &provider,
                        already_attached,
                    )
                    .await
                    {
                        Ok(()) => {
                            report.repaired.push(name.clone());
                            needs_reattach = already_attached;
                        }
                        Err(e) => {
                            report.errors.push((name.clone(), e));
                            continue;
                        }
                    }
                }

                if needs_reattach || !attached_set.contains(name) {
                    match attach_to_sandbox(client, sandbox_name, name).await {
                        Ok(()) => report.attached.push(name.clone()),
                        Err(e) => report.errors.push((name.clone(), format!("attach: {e:#}"))),
                    }
                }
            }
            Err(ProviderError::NotFound(_)) => report.missing.push(name.clone()),
            Err(e) => report.errors.push((name.clone(), format!("get: {e:#}"))),
        }
    }
    // Detach prefixed providers that are no longer declared.
    let prefix = format!("{agent_prefix}-");
    for name in &attached {
        if name.starts_with(&prefix) && !declared_set.contains(name) {
            match detach_from_sandbox(client, sandbox_name, name).await {
                Ok(()) => report.detached.push(name.clone()),
                Err(e) => report.errors.push((name.clone(), format!("detach: {e:#}"))),
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
#[path = "providers_tests.rs"]
mod providers_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// An upstream OpenShell built-in: not the `generic` fallback and not a
    /// RightClaw-owned `right-*` managed profile.
    fn is_upstream_builtin(p: &ProviderProfile) -> bool {
        p.type_slug != "generic" && !p.type_slug.starts_with("right-")
    }

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
            .filter(|p| is_upstream_builtin(p))
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

    #[test]
    fn provider_spec_debug_redacts_credentials() {
        let mut credentials = HashMap::new();
        credentials.insert(
            "ANTHROPIC_API_KEY".to_string(),
            "super-secret-key".to_string(),
        );
        credentials.insert("ANOTHER_KEY".to_string(), "another-secret".to_string());
        let mut config = HashMap::new();
        config.insert("upstream_host".to_string(), "api.example.com".to_string());
        let spec = ProviderSpec {
            name: "my-provider".to_string(),
            type_: "anthropic".to_string(),
            credentials,
            config,
        };
        let debug_output = format!("{spec:?}");
        assert!(
            !debug_output.contains("super-secret-key"),
            "Debug output must not contain credential value; got: {debug_output}"
        );
        assert!(
            !debug_output.contains("another-secret"),
            "Debug output must not contain credential value; got: {debug_output}"
        );
        assert!(
            debug_output.contains("2 redacted"),
            "Debug output should show credential count; got: {debug_output}"
        );
        assert!(
            debug_output.contains("my-provider"),
            "Debug output should show name; got: {debug_output}"
        );
        assert!(
            debug_output.contains("anthropic"),
            "Debug output should show type_; got: {debug_output}"
        );
        assert!(
            debug_output.contains("upstream_host"),
            "Debug output should show config keys/values; got: {debug_output}"
        );
    }

    #[test]
    fn catalog_has_full_github_and_keeps_builtin() {
        let catalog = profile_catalog();
        let upstream_builtin = catalog.iter().filter(|p| is_upstream_builtin(p)).count();
        assert_eq!(upstream_builtin, 8, "8 upstream built-ins unchanged");

        let gh = catalog
            .iter()
            .find(|p| p.type_slug == "github")
            .expect("github kept");
        let rgh = catalog
            .iter()
            .find(|p| p.type_slug == "right-github")
            .expect("right-github present");
        assert_eq!(gh.env_var, "GITHUB_TOKEN");
        assert_eq!(rgh.env_var, "GITHUB_TOKEN");
        assert_eq!(rgh.display_name, "GitHub");
        assert!(
            catalog.iter().all(|p| p.type_slug != "right-github-write"),
            "old right-github-write removed"
        );
    }
}
