//! RightClaw-owned OpenShell provider profiles.
//!
//! This module owns the OpenShell **ProviderProfile** RPC surface
//! (get/lint/import/delete) and the set of profiles RightClaw provisions to
//! the gateway. It is a sibling to `providers.rs` (which owns the **Provider**
//! surface). All RightClaw profile ids are `right-*` prefixed — the prefix is
//! the ownership marker.

use crate::openshell_proto::openshell::sandbox::v1 as sandbox_v1;
use crate::openshell_proto::openshell::v1 as proto_v1;
use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;
use thiserror::Error;
use tonic::transport::Channel;

/// All managed-profile errors — FAIL FAST, never swallowed.
#[derive(Debug, Error)]
pub enum ManagedProfileError {
    #[error("openshell gRPC: {0}")]
    Grpc(String),
    #[error("profile \"{id}\" failed lint: {detail}")]
    LintFailed { id: String, detail: String },
}

/// A profile RightClaw provisions to the gateway.
#[derive(Debug, Clone)]
pub enum ManagedProfile {
    /// Clone the live `github` profile and open all endpoints to full access.
    Github,
    /// A profile authored by Right from scratch (e.g. a generic provider).
    /// Self-contained — has no upstream base to derive from.
    Authored(Box<proto_v1::ProviderProfile>),
}

impl ManagedProfile {
    pub fn id(&self) -> String {
        match self {
            ManagedProfile::Github => "right-github".into(),
            ManagedProfile::Authored(p) => p.id.clone(),
        }
    }

    /// The upstream profile id this profile derives from, if any.
    pub fn base_id(&self) -> Option<&'static str> {
        match self {
            ManagedProfile::Github => Some("github"),
            ManagedProfile::Authored(_) => None,
        }
    }

    /// Produce the desired profile from a fetched base profile. For
    /// `Github`: set `access: "full"` on every endpoint (permits every HTTP
    /// method including git push POSTs). `access` and `rules` are mutually
    /// exclusive — clear rules, set the preset.
    pub fn derive(&self, mut base: proto_v1::ProviderProfile) -> proto_v1::ProviderProfile {
        match self {
            ManagedProfile::Github => {
                base.id = self.id();
                base.display_name = "GitHub".into();
                for ep in &mut base.endpoints {
                    // access and rules are mutually exclusive — full preset permits git push POSTs.
                    ep.rules.clear();
                    ep.access = "full".into();
                }
                base
            }
            ManagedProfile::Authored(p) => (**p).clone(),
        }
    }
}

/// Helper constructor used by tests and the registry.
pub fn github() -> ManagedProfile {
    ManagedProfile::Github
}

/// Author a self-contained OpenShell profile for a generic provider.
pub fn author_generic_profile(
    id: &str,
    upstream_host: &str,
    upstream_path_prefix: Option<&str>,
    header_name: &str,
    env_var: &str,
) -> proto_v1::ProviderProfile {
    let auth_style = if header_name.eq_ignore_ascii_case("authorization") {
        "bearer"
    } else {
        "header"
    };

    proto_v1::ProviderProfile {
        id: id.to_string(),
        display_name: id.to_string(),
        description: "Right-managed generic provider".into(),
        category: proto_v1::ProviderProfileCategory::Other as i32,
        credentials: vec![proto_v1::ProviderProfileCredential {
            name: "api_token".into(),
            description: String::new(),
            env_vars: vec![env_var.to_string()],
            required: true,
            auth_style: auth_style.into(),
            header_name: header_name.to_string(),
            query_param: String::new(),
            refresh: None,
        }],
        endpoints: vec![sandbox_v1::NetworkEndpoint {
            host: upstream_host.to_string(),
            port: 443,
            protocol: "rest".into(),
            enforcement: "enforce".into(),
            access: "full".into(),
            path: upstream_path_prefix.unwrap_or("").to_string(),
            ..Default::default()
        }],
        binaries: vec![sandbox_v1::NetworkBinary {
            path: "**".into(),
            ..Default::default()
        }],
        inference_capable: false,
        discovery: None,
    }
}

/// The set of profiles RightClaw provisions on every `right up`.
///
/// Module-local free-form list — intentionally NOT a cross-crate registry
/// (see ARCHITECTURE.md "promote on demand"). Add a variant + an entry here to
/// ship a new profile (e.g. right-browser-use).
pub fn managed_profiles() -> Vec<ManagedProfile> {
    vec![ManagedProfile::Github]
}

// ────────────────────────────────────────────────────────────────────────────
// Per-endpoint fingerprint + structural diff
// ────────────────────────────────────────────────────────────────────────────

/// One endpoint allow-rule fingerprint: `(method, path, command, operation_type)`.
type RuleFp = (String, String, String, String);
/// One credential fingerprint:
/// `(name, sorted env vars, required, auth_style, header_name, query_param)`.
type CredentialFp = (String, Vec<String>, bool, String, String, String);
/// One endpoint's fingerprint:
/// `(host, port, protocol, tls, enforcement, access, path, sorted rules)`.
type EndpointFp = (
    String,
    u32,
    String,
    String,
    String,
    String,
    String,
    Vec<RuleFp>,
);
/// A whole profile's fingerprint:
/// `(id, display_name, description, category, sorted credentials, sorted endpoints, sorted binaries)`.
type ProfileFp = (
    String,
    String,
    String,
    i32,
    Vec<CredentialFp>,
    Vec<EndpointFp>,
    Vec<String>,
);

/// Per-endpoint fingerprint of the fields RightClaw controls. Includes
/// `access` AND `rules`; the managed profile's drift signal lives in `access`
/// (`full` vs the base `read-only` preset), so a profile still on the old
/// preset is detected as drift. `rules` stays in the fingerprint to catch any
/// base profile that ships explicit rules.
fn endpoint_fp(e: &sandbox_v1::NetworkEndpoint) -> EndpointFp {
    let mut rules: Vec<RuleFp> = e
        .rules
        .iter()
        .map(|r| {
            let a = r.allow.clone().unwrap_or_default();
            (a.method, a.path, a.command, a.operation_type)
        })
        .collect();
    rules.sort();
    (
        e.host.clone(),
        e.port,
        e.protocol.clone(),
        e.tls.clone(),
        e.enforcement.clone(),
        e.access.clone(),
        e.path.clone(),
        rules,
    )
}

/// Stable structural fingerprint of a profile. Compared instead of the whole
/// message so gateway-filled defaults don't force re-imports.
fn fingerprint(p: &proto_v1::ProviderProfile) -> ProfileFp {
    let mut credentials: Vec<_> = p
        .credentials
        .iter()
        .map(|c| {
            let mut env_vars = c.env_vars.clone();
            env_vars.sort();
            (
                c.name.clone(),
                env_vars,
                c.required,
                c.auth_style.clone(),
                c.header_name.clone(),
                c.query_param.clone(),
            )
        })
        .collect();
    credentials.sort();

    let mut eps: Vec<_> = p.endpoints.iter().map(endpoint_fp).collect();
    eps.sort();

    let mut binaries: Vec<_> = p.binaries.iter().map(|b| b.path.clone()).collect();
    binaries.sort();

    (
        p.id.clone(),
        p.display_name.clone(),
        p.description.clone(),
        p.category,
        credentials,
        eps,
        binaries,
    )
}

/// True if `desired` must be (re)imported given the currently `stored` profile.
fn needs_import(
    stored: Option<&proto_v1::ProviderProfile>,
    desired: &proto_v1::ProviderProfile,
) -> bool {
    match stored {
        None => true,
        Some(s) => fingerprint(s) != fingerprint(desired),
    }
}

pub(crate) enum DesiredProfileSource {
    DeriveFromBase(&'static str),
    Authored(Box<proto_v1::ProviderProfile>),
}

pub(crate) fn desired_profile_source(mp: &ManagedProfile) -> DesiredProfileSource {
    match mp {
        ManagedProfile::Github => DesiredProfileSource::DeriveFromBase("github"),
        ManagedProfile::Authored(p) => DesiredProfileSource::Authored(p.clone()),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// gRPC wrappers
// ────────────────────────────────────────────────────────────────────────────

fn grpc_err(s: tonic::Status) -> ManagedProfileError {
    ManagedProfileError::Grpc(format!("{}: {}", s.code(), s.message()))
}

/// Fetch a profile by id. Returns `None` on NotFound.
pub async fn get_profile(
    client: &mut OpenShellClient<Channel>,
    id: &str,
) -> Result<Option<proto_v1::ProviderProfile>, ManagedProfileError> {
    let req = proto_v1::GetProviderProfileRequest { id: id.to_string() };
    match client.get_provider_profile(req).await {
        Ok(resp) => Ok(resp.into_inner().profile),
        Err(s) if s.code() == tonic::Code::NotFound => Ok(None),
        Err(s) => Err(grpc_err(s)),
    }
}

/// Lint then import a single profile. Lint failure is a hard error.
pub async fn lint_and_import(
    client: &mut OpenShellClient<Channel>,
    profile: proto_v1::ProviderProfile,
) -> Result<(), ManagedProfileError> {
    let id = profile.id.clone();
    let item = proto_v1::ProviderProfileImportItem {
        profile: Some(profile),
        source: "right".into(),
    };

    let lint = client
        .lint_provider_profiles(proto_v1::LintProviderProfilesRequest {
            profiles: vec![item.clone()],
        })
        .await
        .map_err(grpc_err)?
        .into_inner();
    if !lint.valid {
        let detail = lint
            .diagnostics
            .iter()
            .map(|d| format!("{}: {}", d.field, d.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ManagedProfileError::LintFailed { id, detail });
    }

    client
        .import_provider_profiles(proto_v1::ImportProviderProfilesRequest {
            profiles: vec![item],
        })
        .await
        .map_err(grpc_err)?;
    Ok(())
}

/// Delete a managed profile (used by tests for cleanup; no auto-GC in prod).
pub async fn delete_profile(
    client: &mut OpenShellClient<Channel>,
    id: &str,
) -> Result<(), ManagedProfileError> {
    client
        .delete_provider_profile(proto_v1::DeleteProviderProfileRequest { id: id.to_string() })
        .await
        .map_err(grpc_err)?;
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Reconcile loop
// ────────────────────────────────────────────────────────────────────────────

/// Outcome of ensuring one managed profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    Imported(String),
    Unchanged(String),
    /// Base profile was absent on the gateway — profile skipped (non-fatal).
    Skipped(String),
}

/// Idempotently provision the given managed profiles to the gateway.
///
/// Derived profiles re-read their base each call (drift-proof). A missing base
/// is non-fatal — the profile is skipped with a warning. Re-imports happen only
/// on real diff.
pub async fn ensure_profiles(
    client: &mut OpenShellClient<Channel>,
    profiles: &[ManagedProfile],
) -> Result<Vec<EnsureOutcome>, ManagedProfileError> {
    let mut outcomes = Vec::with_capacity(profiles.len());
    for mp in profiles {
        let id = mp.id();
        let desired = match desired_profile_source(mp) {
            DesiredProfileSource::DeriveFromBase(base_id) => {
                match get_profile(client, base_id).await? {
                    Some(base) => mp.derive(base),
                    None => {
                        tracing::warn!(
                            profile = id,
                            base = base_id,
                            "base profile missing on gateway — skipping managed profile"
                        );
                        outcomes.push(EnsureOutcome::Skipped(id));
                        continue;
                    }
                }
            }
            DesiredProfileSource::Authored(profile) => *profile,
        };
        let stored = get_profile(client, &id).await?;
        if needs_import(stored.as_ref(), &desired) {
            lint_and_import(client, desired).await?;
            tracing::info!(profile = id, "managed profile drift → imported");
            outcomes.push(EnsureOutcome::Imported(id));
        } else {
            tracing::debug!(profile = id, "managed profile unchanged");
            outcomes.push(EnsureOutcome::Unchanged(id));
        }
    }
    Ok(outcomes)
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openshell_proto::openshell::sandbox::v1 as sandbox_v1;
    use crate::openshell_proto::openshell::v1 as proto_v1;

    fn base_github() -> proto_v1::ProviderProfile {
        let ro = |host: &str, protocol: &str| sandbox_v1::NetworkEndpoint {
            host: host.into(),
            port: 443,
            protocol: protocol.into(),
            access: "read-only".into(),
            enforcement: "enforce".into(),
            rules: vec![],
            ..Default::default()
        };
        proto_v1::ProviderProfile {
            id: "github".into(),
            display_name: "GitHub".into(),
            description: "GitHub API and Git operations".into(),
            category: 4, // SOURCE_CONTROL
            credentials: vec![],
            endpoints: vec![
                ro("api.github.com", "rest"),
                ro("api.github.com", "graphql"),
                ro("github.com", "rest"),
            ],
            binaries: vec![],
            inference_capable: false,
            discovery: None,
        }
    }

    #[test]
    fn derive_github_sets_full_access_and_renames() {
        let derived = github().derive(base_github());
        assert_eq!(derived.id, "right-github");
        assert_eq!(derived.display_name, "GitHub");
        assert_eq!(derived.category, 4, "category preserved from base");
        assert!(!derived.endpoints.is_empty());
        for ep in &derived.endpoints {
            assert_eq!(ep.access, "full", "every endpoint opened to full access");
            assert!(ep.rules.is_empty(), "rules cleared (exclusive with access)");
        }
    }

    #[test]
    fn authored_profile_reports_id_and_no_base() {
        let prof = proto_v1::ProviderProfile {
            id: "right-acme".into(),
            display_name: "acme".into(),
            ..Default::default()
        };
        let mp = ManagedProfile::Authored(Box::new(prof));
        assert_eq!(mp.id(), "right-acme");
        assert_eq!(mp.base_id(), None);
    }

    #[test]
    fn author_generic_profile_sets_endpoint_credential_and_binaries() {
        let p = author_generic_profile(
            "right-acme",
            "api.acme.com",
            Some("/v1"),
            "x-api-key",
            "MY_API_KEY",
        );
        assert_eq!(p.id, "right-acme");
        let ep = &p.endpoints[0];
        assert_eq!(ep.host, "api.acme.com");
        assert_eq!(ep.port, 443);
        assert_eq!(ep.protocol, "rest");
        assert_eq!(ep.access, "full");
        assert_eq!(ep.path, "/v1");
        let cred = &p.credentials[0];
        assert!(cred.env_vars.contains(&"MY_API_KEY".to_string()));
        assert_eq!(cred.header_name.to_lowercase(), "x-api-key");
        assert!(p.binaries.iter().any(|b| b.path == "**"));
    }

    #[test]
    fn desired_profile_for_authored_is_the_authored_body() {
        let authored = proto_v1::ProviderProfile {
            id: "right-acme".into(),
            ..Default::default()
        };
        let mp = ManagedProfile::Authored(Box::new(authored.clone()));
        match desired_profile_source(&mp) {
            DesiredProfileSource::Authored(p) => assert_eq!(p.id, "right-acme"),
            _ => panic!("authored must not require a base fetch"),
        }
    }

    #[test]
    fn desired_profile_for_github_requires_base() {
        match desired_profile_source(&ManagedProfile::Github) {
            DesiredProfileSource::DeriveFromBase(base) => assert_eq!(base, "github"),
            _ => panic!("github derives from base"),
        }
    }

    #[test]
    fn managed_profiles_all_right_prefixed() {
        for mp in managed_profiles() {
            assert!(
                mp.id().starts_with("right-"),
                "managed profile {} must be right-* prefixed",
                mp.id()
            );
        }
    }

    #[test]
    fn ensure_outcome_skipped_variant_exists() {
        // Compile-time guard that the non-fatal Skipped outcome is available.
        let s = EnsureOutcome::Skipped("right-github".into());
        assert!(matches!(s, EnsureOutcome::Skipped(_)));
    }

    #[test]
    fn needs_import_true_when_access_differs() {
        let desired = github().derive(base_github());
        let stored_same = desired.clone();
        let stored_old = base_github(); // still access: read-only

        assert!(
            !needs_import(Some(&stored_same), &desired),
            "identical → no import"
        );
        assert!(
            needs_import(Some(&stored_old), &desired),
            "access drift → import"
        );
        assert!(needs_import(None, &desired), "absent → import");
    }

    #[test]
    fn needs_import_true_when_authored_profile_controlled_fields_differ() {
        let desired = author_generic_profile(
            "right-acme",
            "api.acme.com",
            Some("/v1"),
            "x-api-key",
            "MY_API_KEY",
        );

        let mut stored_old_credential = desired.clone();
        stored_old_credential.credentials[0].env_vars = vec!["OLD_API_KEY".into()];
        assert!(
            needs_import(Some(&stored_old_credential), &desired),
            "credential drift → import"
        );

        let mut stored_missing_binary = desired.clone();
        stored_missing_binary.binaries.clear();
        assert!(
            needs_import(Some(&stored_missing_binary), &desired),
            "binary drift → import"
        );

        let mut stored_old_path = desired.clone();
        stored_old_path.endpoints[0].path = "/v0".into();
        assert!(
            needs_import(Some(&stored_old_path), &desired),
            "endpoint path drift → import"
        );
    }
}
