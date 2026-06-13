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

pub type OpenShellGrpcClient = OpenShellClient<Channel>;

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

pub fn generic_provider_profile_id(provider_name: &str) -> String {
    const PREFIX: &str = "right-provider-";
    const HASH_HEX_LEN: usize = 16;
    const MAX_ID_LEN: usize = 64;
    let max_slug_len = MAX_ID_LEN - PREFIX.len() - 1 - HASH_HEX_LEN;

    let mut slug = String::with_capacity(provider_name.len().min(max_slug_len));
    let mut last_was_dash = false;
    for byte in provider_name.bytes() {
        let ch = match byte {
            b'a'..=b'z' | b'0'..=b'9' => byte as char,
            b'A'..=b'Z' => (byte + (b'a' - b'A')) as char,
            b'-' => '-',
            _ => '-',
        };
        if ch == '-' {
            if !slug.is_empty() && !last_was_dash {
                slug.push('-');
            }
            last_was_dash = true;
        } else {
            slug.push(ch);
            last_was_dash = false;
        }
        if slug.len() >= max_slug_len {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("provider");
    }

    format!(
        "{PREFIX}{slug}-{hash:016x}",
        hash = fnv1a64(provider_name.as_bytes())
    )
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Built-in fal.ai OpenShell profile authored by RightClaw.
pub fn fal_profile() -> proto_v1::ProviderProfile {
    // This profile covers only authenticated API/control-plane hosts. Media/CDN
    // and upload-target hosts are intentionally excluded from this credential-injection profile.
    let hosts = ["fal.run", "queue.fal.run", "rest.fal.ai"];

    proto_v1::ProviderProfile {
        id: "right-fal".into(),
        display_name: "fal.ai".into(),
        description: "RightClaw-managed fal.ai provider".into(),
        category: proto_v1::ProviderProfileCategory::Other as i32,
        credentials: vec![proto_v1::ProviderProfileCredential {
            name: "api_token".into(),
            description: String::new(),
            env_vars: vec!["FAL_KEY".into()],
            required: true,
            auth_style: "bearer".into(),
            header_name: "Authorization".into(),
            query_param: String::new(),
            refresh: None,
            path_template: String::new(),
            token_grant: None,
        }],
        endpoints: hosts
            .iter()
            .map(|host| sandbox_v1::NetworkEndpoint {
                host: (*host).into(),
                port: 443,
                protocol: "rest".into(),
                enforcement: "enforce".into(),
                access: "full".into(),
                path: String::new(),
                ..Default::default()
            })
            .collect(),
        binaries: vec![sandbox_v1::NetworkBinary {
            path: "**".into(),
            ..Default::default()
        }],
        inference_capable: false,
        discovery: None,
    }
}

/// Author a self-contained OpenShell profile for a generic provider.
pub fn author_generic_profile(
    id: &str,
    upstream_hosts: &[String],
    upstream_path_prefix: Option<&str>,
    env_var: &str,
) -> proto_v1::ProviderProfile {
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
            // Fixed placement is canonical-valid and inert for OpenShell static-key
            // substitution; the agent writes the real auth header.
            auth_style: "bearer".into(),
            header_name: "Authorization".into(),
            query_param: String::new(),
            refresh: None,
            path_template: String::new(),
            token_grant: None,
        }],
        endpoints: upstream_hosts
            .iter()
            .map(|host| sandbox_v1::NetworkEndpoint {
                host: host.clone(),
                port: 443,
                protocol: "rest".into(),
                enforcement: "enforce".into(),
                access: "full".into(),
                path: upstream_path_prefix.unwrap_or("").to_string(),
                ..Default::default()
            })
            .collect(),
        binaries: vec![sandbox_v1::NetworkBinary {
            path: "**".into(),
            ..Default::default()
        }],
        inference_capable: false,
        discovery: None,
    }
}

/// Config-free generic provider data used to author a managed OpenShell profile.
pub struct GenericProviderProfileInput<'a> {
    pub name: &'a str,
    pub upstream_hosts: &'a [String],
    pub upstream_path_prefix: Option<&'a str>,
    pub env_var: &'a str,
}

/// Author one Right-managed profile per generic provider, deduped by profile id.
pub fn generic_provider_profiles<'a, I>(providers: I) -> Vec<ManagedProfile>
where
    I: IntoIterator<Item = GenericProviderProfileInput<'a>>,
{
    let mut seen = std::collections::HashSet::new();
    let mut profiles = Vec::new();

    for provider in providers {
        let id = generic_provider_profile_id(provider.name);
        if !seen.insert(id.clone()) {
            continue;
        }
        profiles.push(ManagedProfile::Authored(Box::new(author_generic_profile(
            &id,
            provider.upstream_hosts,
            provider.upstream_path_prefix,
            provider.env_var,
        ))));
    }

    profiles
}

/// The set of profiles RightClaw provisions on every `right up`.
///
/// Module-local free-form list — intentionally NOT a cross-crate registry
/// (see ARCHITECTURE.md "promote on demand"). Add a variant + an entry here to
/// ship a new profile (e.g. right-browser-use).
pub fn managed_profiles() -> Vec<ManagedProfile> {
    vec![
        ManagedProfile::Github,
        ManagedProfile::Authored(Box::new(fal_profile())),
    ]
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
/// `(host, effective ports, protocol, tls, enforcement, access, sorted allowed IPs, path, sorted rules)`.
type EndpointFp = (
    String,
    Vec<u32>,
    String,
    String,
    String,
    String,
    Vec<String>,
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

    let mut effective_ports = if e.ports.is_empty() {
        if e.port == 0 {
            Vec::new()
        } else {
            vec![e.port]
        }
    } else {
        e.ports.clone()
    };
    effective_ports.sort();

    let mut allowed_ips = e.allowed_ips.clone();
    allowed_ips.sort();

    (
        e.host.clone(),
        effective_ports,
        e.protocol.clone(),
        e.tls.clone(),
        e.enforcement.clone(),
        e.access.clone(),
        allowed_ips,
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
    fn author_generic_profile_emits_one_endpoint_per_host() {
        let hosts = vec!["fal.run".to_string(), "queue.fal.run".to_string()];
        let p = author_generic_profile("right-provider-x", &hosts, Some("/v1"), "FAL_KEY");
        let endpoint_hosts: Vec<&str> = p.endpoints.iter().map(|e| e.host.as_str()).collect();
        assert_eq!(endpoint_hosts, vec!["fal.run", "queue.fal.run"]);
        for e in &p.endpoints {
            assert_eq!(e.protocol, "rest");
            assert_eq!(e.access, "full");
            assert_eq!(e.path, "/v1");
            assert_eq!(e.port, 443);
        }
    }

    #[test]
    fn author_generic_profile_credential_is_fixed_inert_placement() {
        let p = author_generic_profile(
            "right-provider-x",
            &["api.acme.com".to_string()],
            None,
            "ACME_TOKEN",
        );
        let cred = &p.credentials[0];
        assert_eq!(cred.env_vars, vec!["ACME_TOKEN".to_string()]);
        // Fixed canonical-valid placement; inert for static keys.
        assert_eq!(cred.header_name, "Authorization");
        assert_eq!(cred.auth_style, "bearer");
    }

    #[test]
    fn fal_profile_id_and_hosts() {
        let p = fal_profile();
        assert_eq!(p.id, "right-fal");
        assert_eq!(p.display_name, "fal.ai");
        let hosts: Vec<&str> = p.endpoints.iter().map(|e| e.host.as_str()).collect();
        assert_eq!(hosts, vec!["fal.run", "queue.fal.run", "rest.fal.ai"]);
        for endpoint in &p.endpoints {
            assert_eq!(endpoint.port, 443);
            assert_eq!(endpoint.protocol, "rest");
            assert_eq!(endpoint.enforcement, "enforce");
            assert_eq!(endpoint.access, "full");
            assert_eq!(endpoint.path, "");
        }
        assert_eq!(p.credentials[0].env_vars, vec!["FAL_KEY".to_string()]);
    }

    #[test]
    fn generic_provider_profile_id_is_valid_bounded_and_collision_resistant() {
        let names = [
            "foo-bar",
            "right-foo-bar",
            "agent_01-acme",
            "Agent_01-acme",
            "RIGHT__ODD Provider",
        ];
        let ids: Vec<_> = names
            .iter()
            .map(|name| generic_provider_profile_id(name))
            .collect();
        let unique: std::collections::HashSet<_> = ids.iter().cloned().collect();

        assert_eq!(unique.len(), ids.len(), "ids must include a raw-name hash");
        for (name, id) in names.iter().zip(ids.iter()) {
            assert_eq!(
                generic_provider_profile_id(name),
                *id,
                "profile id must be deterministic"
            );
            assert!(
                id.len() <= 64,
                "profile id {id:?} for {name:?} exceeds the length bound"
            );
            assert!(id.starts_with("right-provider-"));
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "profile id {id:?} contains invalid characters"
            );
            let hash = id.rsplit('-').next().expect("hash suffix");
            assert_eq!(hash.len(), 16, "profile id {id:?} must end in u64 hex");
            assert!(
                hash.chars().all(|c| c.is_ascii_hexdigit()),
                "profile id {id:?} must end in hex"
            );
        }
    }

    #[test]
    fn generic_provider_profiles_uses_collision_resistant_profile_ids() {
        let hosts = vec!["api.acme.com".to_string()];
        let profiles = generic_provider_profiles([
            GenericProviderProfileInput {
                name: "right-acme",
                upstream_hosts: &hosts,
                upstream_path_prefix: None,
                env_var: "ACME_API_KEY",
            },
            GenericProviderProfileInput {
                name: "acme",
                upstream_hosts: &hosts,
                upstream_path_prefix: None,
                env_var: "ACME_API_KEY",
            },
        ]);

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].id(), generic_provider_profile_id("right-acme"));
        assert_eq!(profiles[1].id(), generic_provider_profile_id("acme"));
    }

    #[test]
    fn generic_provider_profiles_dedupes_duplicate_provider_names() {
        let hosts = vec!["api.acme.com".to_string()];
        let profiles = generic_provider_profiles([
            GenericProviderProfileInput {
                name: "right-acme",
                upstream_hosts: &hosts,
                upstream_path_prefix: None,
                env_var: "ACME_API_KEY",
            },
            GenericProviderProfileInput {
                name: "right-acme",
                upstream_hosts: &hosts,
                upstream_path_prefix: None,
                env_var: "ACME_API_KEY",
            },
        ]);

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id(), generic_provider_profile_id("right-acme"));
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
    fn managed_profiles_registry_includes_fal() {
        let ids: Vec<String> = managed_profiles().iter().map(|p| p.id()).collect();
        assert!(ids.contains(&"right-fal".to_string()));
        assert!(ids.contains(&"right-github".to_string()));
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
            &["api.acme.com".to_string()],
            Some("/v1"),
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

    #[test]
    fn needs_import_true_when_authored_endpoint_port_allowlist_drift() {
        let desired = author_generic_profile(
            "right-acme",
            &["api.acme.com".to_string()],
            Some("/v1"),
            "MY_API_KEY",
        );

        let mut stored_old_ports = desired.clone();
        stored_old_ports.endpoints[0].ports = vec![80];
        assert!(
            needs_import(Some(&stored_old_ports), &desired),
            "ports drift → import"
        );

        let mut stored_old_allowed_ips = desired.clone();
        stored_old_allowed_ips.endpoints[0].allowed_ips = vec!["203.0.113.10".into()];
        assert!(
            needs_import(Some(&stored_old_allowed_ips), &desired),
            "allowed IP drift → import"
        );
    }

    #[test]
    fn needs_import_false_when_gateway_normalizes_port_into_ports() {
        let desired = author_generic_profile(
            "right-acme",
            &["api.acme.com".to_string()],
            Some("/v1"),
            "MY_API_KEY",
        );
        assert_eq!(desired.endpoints[0].port, 443);
        assert!(desired.endpoints[0].ports.is_empty());

        let mut stored_normalized = desired.clone();
        stored_normalized.endpoints[0].ports = vec![443];

        assert!(
            !needs_import(Some(&stored_normalized), &desired),
            "gateway port normalization must not force re-import"
        );
    }
}
