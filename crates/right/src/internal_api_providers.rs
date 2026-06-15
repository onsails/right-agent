//! Provider management routes — see ARCHITECTURE.md "Providers".

#[derive(Debug, thiserror::Error)]
pub enum ProviderApiError {
    #[error("provider \"{name}\" not found")]
    NotFound { name: String },
    #[error("provider name \"{name}\" already exists")]
    NameCollision { name: String },
    #[error("env var \"{env_var}\" already used by another provider on this agent")]
    EnvVarCollision { env_var: String },
    #[error("invalid name \"{name}\": {reason}")]
    InvalidName { name: String, reason: String },
    #[error("invalid env var \"{env_var}\"")]
    InvalidEnvVar { env_var: String },
    #[error(
        "generic provider \"{name}\" env_var cannot be changed without a new credential; rotate or recreate the provider"
    )]
    GenericEnvVarChangeRequiresCredential { name: String },
    #[error("providers are only available for sandboxed agents (sandbox.mode = openshell)")]
    SandboxModeNone,
    #[error(
        "generic providers require network_policy: permissive — restrictive mode forbids non-Anthropic upstream hosts"
    )]
    NetworkPolicyForbidsGeneric,
    #[error("openshell gateway: {0}")]
    Gateway(String),
    #[error("agent.yaml write failed after gateway change: {0}")]
    AgentYamlWrite(String),
    #[error(
        "provider \"{name}\" references unknown built-in slug \"{slug}\" — the profile catalog no longer recognizes it; config migration required"
    )]
    UnknownBuiltinSlug { name: String, slug: String },
    #[error("not a trusted dashboard user on agent \"{agent}\"")]
    Unauthorized { agent: String },
    #[error("copy conflict: {reason}")]
    CopyConflict { reason: String },
    #[error("internal error: {0}")]
    Internal(String),
}

impl axum::response::IntoResponse for ProviderApiError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        let (status, code) = match &self {
            Self::NotFound { .. } => (StatusCode::NOT_FOUND, "not_found"),
            Self::NameCollision { .. } => (StatusCode::CONFLICT, "name_collision"),
            Self::EnvVarCollision { .. } => (StatusCode::CONFLICT, "env_var_collision"),
            Self::InvalidName { .. } => (StatusCode::BAD_REQUEST, "invalid_name"),
            Self::InvalidEnvVar { .. } => (StatusCode::BAD_REQUEST, "invalid_env_var"),
            Self::GenericEnvVarChangeRequiresCredential { .. } => (
                StatusCode::BAD_REQUEST,
                "generic_env_var_change_requires_credential",
            ),
            Self::SandboxModeNone => (StatusCode::BAD_REQUEST, "sandbox_mode_none"),
            Self::NetworkPolicyForbidsGeneric => {
                (StatusCode::BAD_REQUEST, "network_policy_forbids_generic")
            }
            Self::Gateway(_) => (StatusCode::BAD_GATEWAY, "gateway"),
            Self::AgentYamlWrite(_) => (StatusCode::INTERNAL_SERVER_ERROR, "agent_yaml_write"),
            Self::UnknownBuiltinSlug { .. } => {
                (StatusCode::INTERNAL_SERVER_ERROR, "unknown_builtin_slug")
            }
            Self::Unauthorized { .. } => (StatusCode::FORBIDDEN, "unauthorized"),
            Self::CopyConflict { .. } => (StatusCode::CONFLICT, "copy_conflict"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        (
            status,
            axum::Json(serde_json::json!({"code": code, "message": format!("{self}")})),
        )
            .into_response()
    }
}

/// Open one gRPC client per request. The returned client wraps a
/// tonic::Channel (internally Arc-shared) and is threaded through every
/// provider call in this handler.
type OpenShellClient =
    right_openshell::openshell_proto::openshell::v1::open_shell_client::OpenShellClient<
        tonic::transport::Channel,
    >;

async fn open_openshell_client() -> Result<OpenShellClient, ProviderApiError> {
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("connect: {e:#}")))
}

/// Ensure providers_v2 is enabled before a dashboard mutation that attaches or
/// recomposes a provider. The dashboard is an explicit user action, so a failure
/// here is a hard, surfaced error: the operation cannot work with the flag off.
async fn ensure_v2_for_mutation(client: &mut OpenShellClient) -> Result<(), ProviderApiError> {
    right_openshell::providers::ensure_v2_enabled(client)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))
}

pub fn validate_name(agent: &str, name: &str) -> Result<(), ProviderApiError> {
    let expected_prefix = format!("{agent}-");
    if !name.starts_with(&expected_prefix) {
        return Err(ProviderApiError::InvalidName {
            name: name.into(),
            reason: format!("must start with \"{expected_prefix}\""),
        });
    }
    let slug = &name[expected_prefix.len()..];
    if slug.is_empty() || slug.len() > 32 {
        return Err(ProviderApiError::InvalidName {
            name: name.into(),
            reason: "slug must be 1-32 chars".into(),
        });
    }
    let mut chars = slug.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(ProviderApiError::InvalidName {
            name: name.into(),
            reason: "slug must start with a-z".into(),
        });
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(ProviderApiError::InvalidName {
                name: name.into(),
                reason: "slug allows [a-z0-9-]".into(),
            });
        }
    }
    if name.len() > 64 {
        return Err(ProviderApiError::InvalidName {
            name: name.into(),
            reason: "total length > 64".into(),
        });
    }
    Ok(())
}

pub fn validate_env_var(name: &str) -> Result<(), ProviderApiError> {
    if name.is_empty() || name.len() > 64 {
        return Err(ProviderApiError::InvalidEnvVar {
            env_var: name.into(),
        });
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_uppercase() || first == '_') {
        return Err(ProviderApiError::InvalidEnvVar {
            env_var: name.into(),
        });
    }
    for c in chars {
        if !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
            return Err(ProviderApiError::InvalidEnvVar {
                env_var: name.into(),
            });
        }
    }
    Ok(())
}

/// Validate a free-form text field that will be written verbatim into
/// `agent.yaml` as an unquoted scalar. Rejects YAML metacharacters,
/// control chars, and anything that would shift indentation. The
/// allowed alphabet is intentionally narrow — hostnames, URL path prefixes,
/// and human labels all fit.
fn validate_yaml_scalar(
    value: &str,
    field: &str,
    max_len: usize,
    extra_allowed: &str,
) -> Result<(), ProviderApiError> {
    if value.is_empty() || value.len() > max_len {
        return Err(ProviderApiError::InvalidName {
            name: value.into(),
            reason: format!("{field} must be 1-{max_len} chars"),
        });
    }
    for c in value.chars() {
        let ok = c.is_ascii_alphanumeric() || extra_allowed.contains(c);
        if !ok {
            return Err(ProviderApiError::InvalidName {
                name: value.into(),
                reason: format!("{field} contains disallowed character {c:?}"),
            });
        }
    }
    Ok(())
}

pub fn validate_label(label: &str) -> Result<(), ProviderApiError> {
    validate_yaml_scalar(label, "label", 32, "-_")?;
    // Defense in depth: even though serialize_provider_entry now single-quotes
    // scalars, reject tokens that YAML 1.1 / serde_saphyr would parse as
    // booleans, null, or numbers. A user creating a provider with
    // label: "no" would otherwise write `label: no` into agent.yaml; on
    // next bot restart `Option<String>` deserialization would fail and the
    // agent could not start (self-inflicted DoS via the dashboard).
    if is_yaml_reserved_word(label) || is_pure_numeric(label) {
        return Err(ProviderApiError::InvalidName {
            name: label.into(),
            reason: "label must not be a YAML-reserved word or a pure number".into(),
        });
    }
    Ok(())
}

/// YAML 1.1 reserved booleans + null tokens (case-insensitive match).
/// These would otherwise round-trip through agent.yaml as non-string scalars.
fn is_yaml_reserved_word(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "y" | "yes" | "n" | "no" | "true" | "false" | "on" | "off" | "null" | "~"
    )
}

/// True if `s` parses as a base-10 integer or a float. Catches things like
/// `"123"`, `"-0"`, `"3.14"` that YAML loaders would coerce to numbers.
fn is_pure_numeric(s: &str) -> bool {
    s.parse::<i64>().is_ok() || s.parse::<f64>().is_ok()
}

/// Render a free-form string as a single-quoted YAML scalar. Single quotes
/// inside the value are escaped per the YAML 1.1/1.2 spec by doubling them
/// (`'` -> `''`). Single-quoted scalars do not process backslash escapes,
/// so they are the safest hand-rolled YAML serialization for arbitrary
/// non-control ASCII text — which is exactly what `validate_yaml_scalar`
/// guarantees about every field on `ProviderEntry`.
fn yaml_single_quote(value: &str) -> String {
    let escaped = value.replace('\'', "''");
    format!("'{escaped}'")
}

pub fn validate_upstream_host(host: &str) -> Result<(), ProviderApiError> {
    validate_yaml_scalar(host, "upstream_host", 253, ".-_:")
}

pub fn validate_path_prefix(path: &str) -> Result<(), ProviderApiError> {
    validate_yaml_scalar(path, "upstream_path_prefix", 128, "-_/.~")
}

fn normalize_generic_hosts(
    upstream_host: Option<&str>,
    upstream_hosts: Option<&[String]>,
) -> Vec<String> {
    let mut hosts = Vec::new();
    if let Some(host) = upstream_host {
        let host = host.trim();
        if !host.is_empty() {
            hosts.push(host.to_string());
        }
    }
    if let Some(extra_hosts) = upstream_hosts {
        hosts.extend(extra_hosts.iter().filter_map(|host| {
            let host = host.trim();
            if host.is_empty() {
                None
            } else {
                Some(host.to_string())
            }
        }));
    }

    let mut seen = std::collections::HashSet::new();
    hosts.retain(|host| seen.insert(host.clone()));
    hosts
}

fn validate_generic_request(
    env_var: &str,
    upstream_host: Option<&str>,
    upstream_hosts: Option<&[String]>,
    upstream_path_prefix: Option<&str>,
) -> Result<Vec<String>, ProviderApiError> {
    validate_env_var(env_var)?;
    let hosts = normalize_generic_hosts(upstream_host, upstream_hosts);
    if hosts.is_empty() {
        return Err(ProviderApiError::InvalidName {
            name: String::new(),
            reason: "generic provider requires at least one upstream host".into(),
        });
    }
    for host in &hosts {
        validate_upstream_host(host)?;
    }
    if let Some(prefix) = upstream_path_prefix {
        validate_path_prefix(prefix)?;
    }
    Ok(hosts)
}

fn generic_expected_endpoints(
    upstream_hosts: &[String],
    upstream_path_prefix: Option<&str>,
) -> Vec<(String, String)> {
    let path = upstream_path_prefix.unwrap_or("").to_string();
    upstream_hosts
        .iter()
        .map(|host| (host.clone(), path.clone()))
        .collect()
}

pub fn validate_type_slug(slug: &str) -> Result<(), ProviderApiError> {
    if slug == "claude" {
        return Err(ProviderApiError::InvalidName {
            name: slug.into(),
            reason: "type \"claude\" is reserved for the in-sandbox login flow".into(),
        });
    }
    // The catalog is the single source of truth for valid provider types.
    // Derive the allowlist from it rather than mirroring a hand-maintained
    // parallel array — that mirror silently omitted `right-github`, which made
    // every dashboard "GitHub" create fail with `unknown type`. Catalog-driven
    // validation keeps create in lockstep with new entries automatically.
    let known = right_openshell::providers::profile_catalog()
        .iter()
        .any(|p| p.type_slug == slug);
    if !known {
        return Err(ProviderApiError::InvalidName {
            name: slug.into(),
            reason: format!("unknown type \"{slug}\""),
        });
    }
    Ok(())
}

#[cfg(test)]
mod provider_validation_tests {
    use super::*;

    #[test]
    fn name_must_match_agent_prefix() {
        assert!(validate_name("myagent", "myagent-anthropic").is_ok());
        let err = validate_name("myagent", "other-anthropic").unwrap_err();
        assert!(matches!(err, ProviderApiError::InvalidName { .. }));
    }

    #[test]
    fn slug_pattern_enforced() {
        let err = validate_name("myagent", "myagent-Anthropic").unwrap_err();
        assert!(matches!(err, ProviderApiError::InvalidName { .. }));
        let err2 = validate_name("myagent", "myagent-").unwrap_err();
        assert!(matches!(err2, ProviderApiError::InvalidName { .. }));
    }

    #[test]
    fn env_var_validation() {
        assert!(validate_env_var("MY_TOKEN").is_ok());
        assert!(validate_env_var("X_API_KEY_2").is_ok());
        assert!(matches!(
            validate_env_var("my-token"),
            Err(ProviderApiError::InvalidEnvVar { .. })
        ));
        assert!(matches!(
            validate_env_var("1FOO"),
            Err(ProviderApiError::InvalidEnvVar { .. })
        ));
    }

    #[test]
    fn claude_type_rejected() {
        assert!(matches!(
            validate_type_slug("claude"),
            Err(ProviderApiError::InvalidName { .. })
        ));
        assert!(validate_type_slug("anthropic").is_ok());
        assert!(validate_type_slug("generic").is_ok());
    }

    #[test]
    fn validate_type_slug_accepts_right_github() {
        // Regression: the dashboard offers `right-github` as the GitHub type, so
        // create-validation must accept it. A stale hand-maintained allowlist
        // previously rejected it, breaking the feature end-to-end.
        assert!(validate_type_slug("right-github").is_ok());
    }

    #[test]
    fn validate_type_slug_in_sync_with_catalog() {
        // Every catalog type (except the reserved `claude` login slug, which is
        // never a catalog entry) must be creatable. Guards against the validator
        // and the catalog drifting apart again.
        for p in right_openshell::providers::profile_catalog() {
            assert!(
                validate_type_slug(&p.type_slug).is_ok(),
                "catalog type {} must pass create-validation",
                p.type_slug
            );
        }
    }

    // ── config-update env_var collision tests ────────────────────────────────

    fn make_generic_entry(name: &str, env_var: &str) -> right_agent_config::ProviderEntry {
        right_agent_config::ProviderEntry {
            name: name.to_string(),
            type_: right_agent_config::ProviderType::Generic,
            label: None,
            generic: Some(right_agent_config::GenericProvider {
                env_var: env_var.to_string(),
                upstream_hosts: vec!["api.example.com".to_string()],
                upstream_path_prefix: None,
            }),
        }
    }

    fn make_builtin_entry(name: &str, slug: &str) -> right_agent_config::ProviderEntry {
        right_agent_config::ProviderEntry {
            name: name.to_string(),
            type_: right_agent_config::ProviderType::BuiltIn(slug.to_string()),
            label: None,
            generic: None,
        }
    }

    #[test]
    fn would_collide_detects_env_var_clash_with_other_provider() {
        let providers = vec![
            make_generic_entry("agent-openai", "OPENAI_API_KEY"),
            make_builtin_entry("agent-anthropic", "anthropic"),
        ];
        // Renaming "agent-openai" to a new env var that doesn't collide → false.
        assert!(!would_collide(&providers, "agent-openai", "MY_CUSTOM_KEY").unwrap());
        // Renaming "agent-openai" to the same var it already has → false (no-op, caller skips).
        assert!(!would_collide(&providers, "agent-openai", "OPENAI_API_KEY").unwrap());
        // Renaming some other provider to OPENAI_API_KEY → collides with agent-openai.
        assert!(would_collide(&providers, "agent-other", "OPENAI_API_KEY").unwrap());
    }

    #[test]
    fn would_collide_rename_to_builtin_env_var_detected() {
        // Built-in "anthropic" provider uses ANTHROPIC_API_KEY (from catalog).
        // A generic provider renaming its env_var to match the builtin's should collide.
        // We can't call profile_catalog() in a unit test since it may need OpenShell,
        // so we model the builtin env_var explicitly as a generic for the collision check.
        let providers = vec![
            make_generic_entry("agent-custom", "MY_KEY"),
            make_generic_entry("agent-anthropic", "ANTHROPIC_API_KEY"),
        ];
        // Renaming agent-custom to ANTHROPIC_API_KEY → collision detected.
        assert!(would_collide(&providers, "agent-custom", "ANTHROPIC_API_KEY",).unwrap());
        // Renaming agent-custom to something else → no collision.
        assert!(!would_collide(&providers, "agent-custom", "OPENAI_API_KEY").unwrap());
    }

    #[test]
    fn would_collide_single_provider_never_collides_with_itself() {
        let providers = vec![make_generic_entry("agent-foo", "FOO_API_KEY")];
        // The only provider is the one being updated — should never collide with itself.
        assert!(!would_collide(&providers, "agent-foo", "FOO_API_KEY").unwrap());
        assert!(!would_collide(&providers, "agent-foo", "NEW_KEY").unwrap());
    }

    #[test]
    fn would_collide_propagates_unknown_builtin_slug() {
        // An entry that references a slug no longer in the catalog must cause
        // would_collide to bubble up — silently treating it as no-env-var
        // would let a rename collide with a stale entry undetected.
        let providers = vec![
            make_generic_entry("agent-foo", "FOO_KEY"),
            make_builtin_entry("agent-stale", "definitely-not-a-real-slug"),
        ];
        let err = would_collide(&providers, "agent-foo", "NEW_KEY").unwrap_err();
        assert!(
            matches!(err, ProviderApiError::UnknownBuiltinSlug { .. }),
            "expected UnknownBuiltinSlug, got {err:?}"
        );
    }

    #[test]
    fn extract_env_var_returns_err_for_unknown_builtin_slug() {
        let entry = make_builtin_entry("agent-mystery", "definitely-not-a-real-slug");
        let err = extract_env_var(&entry).unwrap_err();
        match err {
            ProviderApiError::UnknownBuiltinSlug { name, slug } => {
                assert_eq!(name, "agent-mystery");
                assert_eq!(slug, "definitely-not-a-real-slug");
            }
            other => panic!("expected UnknownBuiltinSlug, got {other:?}"),
        }
    }

    #[test]
    fn extract_env_var_returns_ok_for_known_builtin_slug() {
        // Sanity: a slug that IS in the catalog ("anthropic") must succeed.
        let entry = make_builtin_entry("agent-anthropic", "anthropic");
        let env_var = extract_env_var(&entry).expect("anthropic is in profile_catalog()");
        assert_eq!(env_var, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn extract_env_var_returns_ok_for_generic_entry() {
        let entry = make_generic_entry("agent-foo", "FOO_KEY");
        let env_var = extract_env_var(&entry).expect("generic entry with env_var must succeed");
        assert_eq!(env_var, "FOO_KEY");
    }

    // ── label validation against YAML-reserved tokens ─────────────────────

    fn assert_label_rejected(label: &str) {
        let err = validate_label(label)
            .expect_err(&format!("expected validate_label({label:?}) to reject"));
        assert!(
            matches!(err, ProviderApiError::InvalidName { .. }),
            "expected InvalidName for {label:?}, got {err:?}"
        );
    }

    #[test]
    fn validate_label_rejects_yaml_boolean_no() {
        assert_label_rejected("no");
    }

    #[test]
    fn validate_label_rejects_yaml_boolean_true() {
        assert_label_rejected("true");
    }

    #[test]
    fn validate_label_rejects_pure_numeric() {
        assert_label_rejected("123");
    }

    #[test]
    fn validate_label_rejects_tilde_null() {
        // `~` is not ASCII-alphanumeric so `validate_yaml_scalar` already
        // rejects it before the reserved-word check fires, but the field
        // still must be rejected (and the error must classify as InvalidName).
        assert_label_rejected("~");
    }

    #[test]
    fn validate_label_rejects_case_variant_yes() {
        assert_label_rejected("Yes");
    }

    #[test]
    fn validate_label_rejects_all_keyword_case_variants() {
        for tok in [
            "y", "Y", "yes", "YES", "n", "N", "NO", "True", "TRUE", "False", "FALSE", "on", "On",
            "ON", "off", "Off", "OFF", "null", "Null", "NULL",
        ] {
            assert_label_rejected(tok);
        }
    }

    #[test]
    fn validate_label_accepts_hyphenated_keyword_like() {
        // "no-thanks" is not a reserved word and not purely numeric, so OK.
        validate_label("no-thanks").expect("hyphenated label should be accepted");
    }

    #[test]
    fn validate_label_accepts_number_suffix() {
        // "yes2" is not a reserved word (case-insensitive match is exact)
        // and not purely numeric (parse::<i64>() fails). Should be accepted.
        validate_label("yes2").expect("'yes2' should be accepted");
    }

    /// Round-trip guard: serialize a provider entry with a label that YAML 1.1
    /// would otherwise coerce to a boolean, then re-parse the resulting YAML
    /// through `serde_saphyr` and confirm the label survives as a string.
    /// Together with `validate_label`'s rejection of the same tokens, this
    /// closes both halves of the DoS path (defense in depth).
    #[test]
    fn round_trip_quoted_label_survives_saphyr_parse() {
        // Build a YAML doc that simulates an agent.yaml created via our
        // own serializer: indented under `sandbox.providers:`.
        let entry = right_agent_config::ProviderEntry {
            name: "agent-acme".to_string(),
            type_: right_agent_config::ProviderType::Generic,
            label: Some("acme".to_string()),
            generic: Some(right_agent_config::GenericProvider {
                env_var: "ACME_KEY".to_string(),
                upstream_hosts: vec!["api.acme.com".to_string()],
                upstream_path_prefix: Some("/v1".to_string()),
            }),
        };
        let serialized = serialize_provider_entry(&entry);
        // Sanity: the serializer single-quotes string scalars now.
        assert!(serialized.contains("name: 'agent-acme'"));
        assert!(serialized.contains("label: 'acme'"));

        let yaml = format!("sandbox:\n  mode: openshell\n  providers:\n{serialized}");
        let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&yaml)
            .expect("serialized provider entry must round-trip through serde_saphyr");
        let entry_back = &cfg.sandbox.unwrap().providers[0];
        assert_eq!(entry_back.name, "agent-acme");
        assert_eq!(entry_back.label.as_deref(), Some("acme"));
        let g = entry_back.generic.as_ref().unwrap();
        assert_eq!(g.env_var, "ACME_KEY");
        assert_eq!(g.upstream_hosts, vec!["api.acme.com"]);
        assert_eq!(g.upstream_path_prefix.as_deref(), Some("/v1"));
    }

    /// If a value that YAML 1.1 would coerce to a non-string ever bypassed
    /// `validate_label` (e.g. via a future code path), the on-disk YAML
    /// must still re-parse correctly because all scalars are now
    /// single-quoted. Verifies the defense-in-depth half independently.
    #[test]
    fn round_trip_label_no_parses_as_string_when_quoted() {
        let entry = right_agent_config::ProviderEntry {
            name: "agent-no".to_string(),
            // Built-in type slugs are never YAML-reserved, but a hypothetical
            // generic label "no" exercises the dangerous case.
            type_: right_agent_config::ProviderType::Generic,
            label: Some("no".to_string()),
            generic: Some(right_agent_config::GenericProvider {
                env_var: "NO_KEY".to_string(),
                upstream_hosts: vec!["api.example.com".to_string()],
                upstream_path_prefix: None,
            }),
        };
        let serialized = serialize_provider_entry(&entry);
        assert!(
            serialized.contains("label: 'no'"),
            "label must be single-quoted; got:\n{serialized}"
        );
        let yaml = format!("sandbox:\n  mode: openshell\n  providers:\n{serialized}");
        let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&yaml)
            .expect("single-quoted reserved-word label must still parse as Option<String>::Some");
        let entry_back = &cfg.sandbox.unwrap().providers[0];
        assert_eq!(entry_back.label.as_deref(), Some("no"));
    }
}

// ── Task 15: /provider-list ───────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ProviderListReq {
    pub agent: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ProviderView {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub label: Option<String>,
    pub env_var: String,
    pub generic: Option<right_agent_config::GenericProvider>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub status: ProviderStatus,
    /// Whether this provider's endpoints are composed into the sandbox's active
    /// policy. `None` means the active policy could not be read.
    #[serde(default)]
    pub composed: Option<bool>,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderStatus {
    Healthy,
    Missing,
    GatewayError {
        message: String,
    },
    /// The agent's stored built-in slug no longer maps to any profile in
    /// `profile_catalog()`. Surfaced in the list view so the operator can
    /// see the bad row without aborting the entire list; rotation and
    /// config-update on the same entry will fail fast with HTTP 500.
    UnknownBuiltin {
        slug: String,
    },
}

#[cfg(test)]
mod provider_view_tests {
    use super::*;

    #[test]
    fn provider_view_serializes_composed_field() {
        for composed in [false, true] {
            let view = ProviderView {
                name: "hostagent-acme".into(),
                type_: "generic".into(),
                label: Some("acme".into()),
                env_var: "ACME_API_KEY".into(),
                generic: None,
                updated_at: None,
                status: ProviderStatus::Healthy,
                composed: Some(composed),
            };

            let json = serde_json::to_value(view).unwrap();

            assert_eq!(json["composed"], composed);
        }
    }

    #[test]
    fn provider_view_serializes_unknown_composed_as_null() {
        let view = ProviderView {
            name: "hostagent-acme".into(),
            type_: "generic".into(),
            label: Some("acme".into()),
            env_var: "ACME_API_KEY".into(),
            generic: None,
            updated_at: None,
            status: ProviderStatus::Healthy,
            composed: None,
        };

        let json = serde_json::to_value(view).unwrap();

        assert!(json["composed"].is_null());
    }

    fn policy_with_provider_endpoints(
        provider_name: &str,
        endpoints: &[(&str, &str)],
    ) -> right_openshell::openshell_proto::openshell::sandbox::v1::SandboxPolicy {
        use right_openshell::openshell_proto::openshell::sandbox::v1::{
            NetworkEndpoint, NetworkPolicyRule,
        };

        let rule_key = format!("_provider_{}", provider_name.replace('-', "_"));
        let rule = NetworkPolicyRule {
            name: rule_key.clone(),
            endpoints: endpoints
                .iter()
                .map(|(host, path)| NetworkEndpoint {
                    host: (*host).into(),
                    path: (*path).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let mut policy =
            right_openshell::openshell_proto::openshell::sandbox::v1::SandboxPolicy::default();
        policy.network_policies.insert(rule_key, rule);
        policy
    }

    fn policy_with_provider_endpoint(
        provider_name: &str,
        host: &str,
        path: &str,
    ) -> right_openshell::openshell_proto::openshell::sandbox::v1::SandboxPolicy {
        policy_with_provider_endpoints(provider_name, &[(host, path)])
    }

    #[test]
    fn provider_entry_is_composed_uses_rule_presence_for_builtins() {
        let entry = right_agent_config::ProviderEntry {
            name: "hostagent-gh".into(),
            type_: right_agent_config::ProviderType::BuiltIn("right-github".into()),
            label: None,
            generic: None,
        };
        let policy = policy_with_provider_endpoint("hostagent-gh", "api.github.com", "");

        assert!(provider_entry_is_composed(&policy, &entry));
    }

    #[test]
    fn provider_entry_is_composed_rejects_stale_generic_endpoint() {
        let entry = right_agent_config::ProviderEntry {
            name: "hostagent-acme".into(),
            type_: right_agent_config::ProviderType::Generic,
            label: None,
            generic: Some(right_agent_config::GenericProvider {
                env_var: "ACME_TOKEN".into(),
                upstream_hosts: vec!["api.acme.test".into()],
                upstream_path_prefix: Some("/v1".into()),
            }),
        };
        let policy = policy_with_provider_endpoint("hostagent-acme", "old.acme.test", "/v1");

        assert!(
            !provider_entry_is_composed(&policy, &entry),
            "generic list status must not accept a stale pre-update provider rule"
        );
    }

    #[test]
    fn provider_entry_is_composed_rejects_missing_one_generic_host() {
        let entry = right_agent_config::ProviderEntry {
            name: "hostagent-fal".into(),
            type_: right_agent_config::ProviderType::Generic,
            label: None,
            generic: Some(right_agent_config::GenericProvider {
                env_var: "FAL_KEY".into(),
                upstream_hosts: vec!["fal.run".into(), "queue.fal.run".into()],
                upstream_path_prefix: Some("/v1".into()),
            }),
        };
        let policy = policy_with_provider_endpoint("hostagent-fal", "fal.run", "/v1");

        assert!(
            !provider_entry_is_composed(&policy, &entry),
            "multi-host generic providers must require every host to be composed"
        );
    }

    #[test]
    fn provider_entry_is_composed_rejects_stale_extra_generic_host() {
        let entry = right_agent_config::ProviderEntry {
            name: "hostagent-fal".into(),
            type_: right_agent_config::ProviderType::Generic,
            label: None,
            generic: Some(right_agent_config::GenericProvider {
                env_var: "FAL_KEY".into(),
                upstream_hosts: vec!["fal.run".into()],
                upstream_path_prefix: Some("/v1".into()),
            }),
        };
        let policy = policy_with_provider_endpoints(
            "hostagent-fal",
            &[("fal.run", "/v1"), ("queue.fal.run", "/v1")],
        );

        assert!(
            !provider_entry_is_composed(&policy, &entry),
            "generic list status must reject stale active endpoints removed from agent.yaml"
        );
    }
}

/// Load and parse `agent.yaml` for the given agent name from `agents_dir`.
///
/// Returns `Err(String)` (mapped to `ProviderApiError::AgentYamlWrite`) on
/// IO or parse failure, or if the agent has no `agent.yaml`.
fn load_agent_config(
    agents_dir: &std::path::Path,
    agent: &str,
) -> Result<right_agent_config::AgentConfig, String> {
    let agent_dir = agents_dir.join(agent);
    right_agent::agent::discovery::parse_agent_config(&agent_dir)
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("agent.yaml not found for agent '{agent}'"))
}

pub(crate) async fn handle_provider_list(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderListReq>,
) -> Result<axum::Json<Vec<ProviderView>>, ProviderApiError> {
    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    let sandbox = cfg
        .sandbox
        .as_ref()
        .ok_or(ProviderApiError::SandboxModeNone)?;
    if sandbox.mode != right_agent_config::SandboxMode::Openshell {
        return Err(ProviderApiError::SandboxModeNone);
    }
    let mut client = open_openshell_client().await?;
    let sandbox_name = sandbox.name.clone().unwrap_or_else(|| req.agent.clone());
    let active_policy =
        match right_openshell::openshell::get_effective_policy(&mut client, &sandbox_name).await {
            Ok(Some(policy)) => Some(policy),
            Ok(None) => {
                tracing::warn!(
                    agent = %req.agent,
                    sandbox = %sandbox_name,
                    "provider composition state unavailable: active policy response omitted payload"
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    agent = %req.agent,
                    sandbox = %sandbox_name,
                    error = %format_args!("{e:#}"),
                    "provider composition state unavailable: active policy read failed"
                );
                None
            }
        };
    let mut views = Vec::with_capacity(sandbox.providers.len());
    for entry in &sandbox.providers {
        let composed = active_policy
            .as_ref()
            .map(|policy| provider_entry_is_composed(policy, entry));
        let (mut status, updated_at) =
            match right_openshell::providers::get_provider(&mut client, &entry.name).await {
                Ok(p) => (ProviderStatus::Healthy, p.updated_at),
                Err(right_openshell::providers::ProviderError::NotFound(_)) => {
                    (ProviderStatus::Missing, None)
                }
                Err(e) => (
                    ProviderStatus::GatewayError {
                        message: format!("{e:#}"),
                    },
                    None,
                ),
            };
        // Resolve the env var via the FAIL-FAST helper. An unknown built-in
        // slug downgrades the row's status to `UnknownBuiltin` so the
        // operator sees the bad entry instead of having the whole list
        // abort. Per AGENTS.rust.md §2, the error is preserved on the row,
        // not silently swallowed.
        let env_var = match extract_env_var(entry) {
            Ok(v) => v,
            Err(ProviderApiError::UnknownBuiltinSlug { slug, .. }) => {
                tracing::warn!(
                    provider = %entry.name,
                    slug = %slug,
                    "provider entry references unknown built-in slug; marking row as unknown_builtin"
                );
                status = ProviderStatus::UnknownBuiltin { slug };
                String::new()
            }
            Err(e) => return Err(e),
        };
        views.push(ProviderView {
            name: entry.name.clone(),
            type_: provider_view_type(entry),
            label: entry.label.clone(),
            env_var,
            generic: entry.generic.clone(),
            updated_at,
            status,
            composed,
        });
    }
    Ok(axum::Json(views))
}

// ── Tasks 17 + 18: /provider-create ──────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ProviderCreateReq {
    pub agent: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub label: Option<String>,
    pub credential: secrecy::SecretString,
    pub generic: Option<ProviderCreateGeneric>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProviderCreateGeneric {
    pub env_var: String,
    #[serde(default)]
    pub upstream_host: Option<String>,
    #[serde(default)]
    pub upstream_hosts: Option<Vec<String>>,
    pub upstream_path_prefix: Option<String>,
}

pub(crate) async fn handle_provider_create(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderCreateReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    use secrecy::ExposeSecret;
    validate_type_slug(&req.type_)?;
    if let Some(label) = &req.label {
        validate_label(label)?;
    }
    let generic_hosts = if req.type_ == "generic" {
        let g = req
            .generic
            .as_ref()
            .ok_or_else(|| ProviderApiError::InvalidEnvVar { env_var: "".into() })?;
        Some(validate_generic_request(
            &g.env_var,
            g.upstream_host.as_deref(),
            g.upstream_hosts.as_deref(),
            g.upstream_path_prefix.as_deref(),
        )?)
    } else {
        None
    };
    let label_slug = req.label.clone().unwrap_or_else(|| req.type_.clone());
    let name = format!("{}-{}", req.agent, label_slug);
    validate_name(&req.agent, &name)?;
    let _guard = provider_lock(&state, &req.agent).await;

    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    let sandbox = cfg
        .sandbox
        .as_ref()
        .ok_or(ProviderApiError::SandboxModeNone)?;
    if sandbox.mode != right_agent_config::SandboxMode::Openshell {
        return Err(ProviderApiError::SandboxModeNone);
    }
    if sandbox.providers.iter().any(|p| p.name == name) {
        return Err(ProviderApiError::NameCollision { name });
    }
    let env_var = if req.type_ == "generic" {
        req.generic
            .as_ref()
            .map(|g| g.env_var.clone())
            .ok_or_else(|| ProviderApiError::InvalidEnvVar { env_var: "".into() })?
    } else {
        right_openshell::providers::profile_catalog()
            .into_iter()
            .find(|p| p.type_slug == req.type_)
            .map(|p| p.env_var)
            .unwrap_or_default()
    };
    validate_env_var(&env_var)?;
    for existing in &sandbox.providers {
        // Tolerate stale `unknown_builtin` rows here so the operator can add a
        // NEW provider even when an existing entry references a slug that is
        // no longer in `profile_catalog()`. The bad row is already surfaced by
        // `handle_provider_list` as `status: unknown_builtin`; failing the
        // create flow would leave the user with no recovery path inside the
        // dashboard. Other `extract_env_var` errors (e.g. malformed Generic)
        // still propagate.
        let existing_env = match extract_env_var(existing) {
            Ok(v) => v,
            Err(ProviderApiError::UnknownBuiltinSlug { name, slug }) => {
                tracing::warn!(
                    provider = %name,
                    slug = %slug,
                    "skipping unknown_builtin entry in collision check"
                );
                continue;
            }
            Err(e) => return Err(e),
        };
        if existing_env == env_var {
            return Err(ProviderApiError::EnvVarCollision { env_var });
        }
    }

    if req.type_ == "generic" {
        // Generic providers use authored OpenShell profiles for HTTPS
        // interception + placeholder substitution. Restrictive mode has not
        // been validated for those composed outbound endpoints, so generic
        // providers stay permissive-only.
        if matches!(
            cfg.network_policy,
            right_agent_config::NetworkPolicy::Restrictive
        ) {
            return Err(ProviderApiError::NetworkPolicyForbidsGeneric);
        }
        let upstream_hosts = generic_hosts.ok_or_else(|| ProviderApiError::InvalidName {
            name: name.clone(),
            reason: "generic provider requires at least one upstream host".into(),
        })?;
        return create_generic_provider(state, req, name, env_var, upstream_hosts).await;
    }

    // Built-in flow: OpenShell owns the profile endpoints, but Right must still
    // trigger and confirm provider-profile composition after attachment.
    let mut client = open_openshell_client().await?;
    ensure_v2_for_mutation(&mut client).await?;
    let sandbox_name = sandbox.name.clone().unwrap_or_else(|| req.agent.clone());
    let policy_path = state.agents_dir.join(&req.agent).join(
        sandbox
            .policy_file
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("policy.yaml")),
    );

    let mut creds = std::collections::HashMap::new();
    creds.insert(env_var.clone(), req.credential.expose_secret().to_string());
    let spec = right_openshell::providers::ProviderSpec {
        name: name.clone(),
        type_: req.type_.clone(),
        credentials: creds,
        config: Default::default(),
    };
    right_openshell::providers::create_provider(&mut client, &spec)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    if let Err(attach_err) =
        right_openshell::providers::attach_to_sandbox(&mut client, &sandbox_name, &name).await
    {
        if let Err(rollback_err) =
            right_openshell::providers::delete_provider(&mut client, &name).await
        {
            tracing::warn!(
                provider = %name,
                original_err = %attach_err,
                "provider rollback failed: could not delete provider after attach failure: {rollback_err:#}"
            );
        }
        return Err(ProviderApiError::Gateway(format!("{attach_err:#}")));
    }

    if let Err(ensure_err) =
        right_openshell::openshell::ensure_provider_policy_loaded(&sandbox_name, &policy_path).await
    {
        rollback_attached_provider(
            &mut client,
            &name,
            &sandbox_name,
            &policy_path,
            &format!("{ensure_err:#}"),
            "policy-load failure",
        )
        .await;
        return Err(ProviderApiError::Gateway(format!(
            "policy load: {ensure_err:#}"
        )));
    }

    if let Err(compose_err) =
        right_openshell::openshell::wait_for_provider_composed(&mut client, &sandbox_name, &name)
            .await
    {
        rollback_attached_provider(
            &mut client,
            &name,
            &sandbox_name,
            &policy_path,
            &format!("{compose_err:#}"),
            "composition not confirmed",
        )
        .await;
        return Err(ProviderApiError::Gateway(format!("{compose_err:#}")));
    }

    let entry = right_agent_config::ProviderEntry {
        name: name.clone(),
        type_: right_agent_config::ProviderType::BuiltIn(req.type_.clone()),
        label: req.label.clone(),
        generic: None,
    };
    if let Err(e) = append_provider_to_yaml(&state.agents_dir, &req.agent, &entry) {
        rollback_attached_provider(
            &mut client,
            &name,
            &sandbox_name,
            &policy_path,
            &format!("{e:#}"),
            "yaml-write failure",
        )
        .await;
        return Err(ProviderApiError::AgentYamlWrite(format!("{e:#}")));
    }

    Ok(axum::Json(ProviderView {
        name,
        type_: req.type_,
        label: req.label,
        env_var,
        generic: None,
        updated_at: None,
        status: ProviderStatus::Healthy,
        composed: Some(true),
    }))
}

/// Returns `Ok(true)` if renaming `excluding_name`'s env_var to `new_env_var`
/// would collide with another provider in `providers`.
///
/// Propagates `ProviderApiError::UnknownBuiltinSlug` if any built-in entry
/// in `providers` references a slug that is no longer in `profile_catalog()`
/// — silently treating it as "no env var" would let the rename succeed while
/// the stale entry continues to produce broken placeholder substitution.
#[cfg(test)]
fn would_collide(
    providers: &[right_agent_config::ProviderEntry],
    excluding_name: &str,
    new_env_var: &str,
) -> Result<bool, ProviderApiError> {
    for p in providers {
        if p.name == excluding_name {
            continue;
        }
        if extract_env_var(p)? == new_env_var {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Resolve the env var name that the gateway uses as the credential key for
/// this provider entry.
///
/// For `BuiltIn(slug)` this looks the slug up in `profile_catalog()`. If the
/// slug is unknown, this returns `UnknownBuiltinSlug` instead of an empty
/// string — an empty key would silently break credential rotation and
/// placeholder substitution (FAIL-FAST per AGENTS.rust.md §2).
fn extract_env_var(entry: &right_agent_config::ProviderEntry) -> Result<String, ProviderApiError> {
    match &entry.type_ {
        right_agent_config::ProviderType::Generic => entry
            .generic
            .as_ref()
            .map(|g| g.env_var.clone())
            .ok_or_else(|| ProviderApiError::InvalidEnvVar { env_var: "".into() }),
        right_agent_config::ProviderType::BuiltIn(slug) => {
            right_openshell::providers::profile_catalog()
                .into_iter()
                .find(|p| &p.type_slug == slug)
                .map(|p| p.env_var)
                .ok_or_else(|| ProviderApiError::UnknownBuiltinSlug {
                    name: entry.name.clone(),
                    slug: slug.clone(),
                })
        }
    }
}

/// What a copy resolves to once the source provider and the destination's
/// existing providers are known. Carries everything except the credential
/// (read separately from the gateway).
#[derive(Debug)]
pub(crate) enum CopyPlan {
    Create {
        type_: String,
        label: Option<String>,
        generic: Option<ProviderCreateGeneric>,
    },
    Overwrite {
        dest_name: String,
        /// `Some` → re-sync the destination generic config to this; `None`
        /// → credential-only rotate.
        resync_generic: Option<right_agent_config::GenericProvider>,
    },
}

/// Decide how copying `source_entry` (using `env_var`) into a destination
/// whose current providers are `dest_providers` should proceed. Pure.
pub(crate) fn plan_copy(
    source_agent: &str,
    source_entry: &right_agent_config::ProviderEntry,
    env_var: &str,
    dest_providers: &[right_agent_config::ProviderEntry],
    overwrite: bool,
    label_override: Option<&str>,
) -> Result<CopyPlan, ProviderApiError> {
    let existing = dest_providers
        .iter()
        .find_map(|p| match extract_env_var(p) {
            Ok(e) if e == env_var => Some(Ok(p)),
            Ok(_) => None,
            Err(e) => Some(Err(e)),
        })
        .transpose()?;

    if overwrite {
        let dest_entry = existing.ok_or_else(|| ProviderApiError::CopyConflict {
            reason: format!("nothing to overwrite: no provider uses env var \"{env_var}\""),
        })?;
        // Only checks the Generic-vs-BuiltIn category, NOT slug-level
        // compatibility (e.g. right-github→right-fal passes here); slug
        // compat, if any, is the caller's concern.
        let compatible = matches!(
            (&source_entry.type_, &dest_entry.type_),
            (
                right_agent_config::ProviderType::Generic,
                right_agent_config::ProviderType::Generic
            ) | (
                right_agent_config::ProviderType::BuiltIn(_),
                right_agent_config::ProviderType::BuiltIn(_)
            )
        );
        if !compatible {
            return Err(ProviderApiError::CopyConflict {
                reason: format!(
                    "existing provider \"{}\" has an incompatible type; remove it first",
                    dest_entry.name
                ),
            });
        }
        let resync_generic = match (&source_entry.type_, &source_entry.generic) {
            (right_agent_config::ProviderType::Generic, Some(src_g)) => {
                let differs = dest_entry
                    .generic
                    .as_ref()
                    .map(|d| {
                        d.upstream_hosts != src_g.upstream_hosts
                            || d.upstream_path_prefix != src_g.upstream_path_prefix
                    })
                    .unwrap_or(true);
                if differs { Some(src_g.clone()) } else { None }
            }
            _ => None,
        };
        Ok(CopyPlan::Overwrite {
            dest_name: dest_entry.name.clone(),
            resync_generic,
        })
    } else {
        if existing.is_some() {
            return Err(ProviderApiError::EnvVarCollision {
                env_var: env_var.to_string(),
            });
        }
        let label = label_override.map(|s| s.to_string()).or_else(|| {
            source_entry
                .name
                .strip_prefix(&format!("{source_agent}-"))
                .map(|s| s.to_string())
        });
        let (type_, generic) =
            match &source_entry.type_ {
                right_agent_config::ProviderType::Generic => {
                    let g = source_entry.generic.clone().ok_or_else(|| {
                        ProviderApiError::InvalidName {
                            name: source_entry.name.clone(),
                            reason: "generic source provider missing 'generic:' block".into(),
                        }
                    })?;
                    (
                        "generic".to_string(),
                        Some(ProviderCreateGeneric {
                            env_var: g.env_var,
                            upstream_host: None,
                            upstream_hosts: Some(g.upstream_hosts),
                            upstream_path_prefix: g.upstream_path_prefix,
                        }),
                    )
                }
                right_agent_config::ProviderType::BuiltIn(slug) => (slug.clone(), None),
            };
        Ok(CopyPlan::Create {
            type_,
            label,
            generic,
        })
    }
}

#[cfg(test)]
mod plan_copy_tests {
    use super::*;

    fn generic(
        name: &str,
        env: &str,
        hosts: &[&str],
        path: Option<&str>,
    ) -> right_agent_config::ProviderEntry {
        right_agent_config::ProviderEntry {
            name: name.into(),
            type_: right_agent_config::ProviderType::Generic,
            label: None,
            generic: Some(right_agent_config::GenericProvider {
                env_var: env.into(),
                upstream_hosts: hosts.iter().map(|h| h.to_string()).collect(),
                upstream_path_prefix: path.map(|p| p.to_string()),
            }),
        }
    }
    fn builtin(name: &str, slug: &str) -> right_agent_config::ProviderEntry {
        right_agent_config::ProviderEntry {
            name: name.into(),
            type_: right_agent_config::ProviderType::BuiltIn(slug.into()),
            label: None,
            generic: None,
        }
    }

    #[test]
    fn create_when_no_env_var_match() {
        let src = builtin("agent-a-provider", "right-fal");
        let plan = plan_copy("agent-a", &src, "FAL_KEY", &[], false, None).unwrap();
        match plan {
            CopyPlan::Create {
                type_,
                label,
                generic,
            } => {
                assert_eq!(type_, "right-fal");
                assert_eq!(label.as_deref(), Some("fal"));
                assert!(generic.is_none());
            }
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn create_label_override_wins() {
        let src = builtin("agent-a-provider", "right-fal");
        let plan = plan_copy("agent-a", &src, "FAL_KEY", &[], false, Some("media")).unwrap();
        match plan {
            CopyPlan::Create { label, .. } => assert_eq!(label.as_deref(), Some("media")),
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn create_collision_without_overwrite_errors() {
        let src = builtin("agent-a-provider", "right-fal");
        let dest = vec![builtin("other-fal", "right-fal")];
        let err = plan_copy("agent-a", &src, "FAL_KEY", &dest, false, None).unwrap_err();
        assert!(matches!(err, ProviderApiError::EnvVarCollision { .. }));
    }

    #[test]
    fn overwrite_without_match_errors() {
        let src = builtin("agent-a-provider", "right-fal");
        let err = plan_copy("agent-a", &src, "FAL_KEY", &[], true, None).unwrap_err();
        assert!(matches!(err, ProviderApiError::CopyConflict { .. }));
    }

    #[test]
    fn overwrite_type_mismatch_errors() {
        let src = generic("agent-a-provider", "FAL_KEY", &["fal.run"], Some("/v1"));
        let dest = vec![builtin("other-fal", "right-fal")];
        let err = plan_copy("agent-a", &src, "FAL_KEY", &dest, true, None).unwrap_err();
        assert!(matches!(err, ProviderApiError::CopyConflict { .. }));
    }

    #[test]
    fn overwrite_builtin_is_credential_only() {
        let src = builtin("agent-a-provider", "right-fal");
        let dest = vec![builtin("other-fal", "right-fal")];
        let plan = plan_copy("agent-a", &src, "FAL_KEY", &dest, true, None).unwrap();
        match plan {
            CopyPlan::Overwrite {
                dest_name,
                resync_generic,
            } => {
                assert_eq!(dest_name, "other-fal");
                assert!(resync_generic.is_none());
            }
            _ => panic!("expected Overwrite"),
        }
    }

    #[test]
    fn overwrite_generic_resyncs_only_when_config_differs() {
        let src = generic(
            "agent-a-provider",
            "FAL_KEY",
            &["fal.run", "queue.fal.run"],
            Some("/v1"),
        );
        let same = vec![generic(
            "other-fal",
            "FAL_KEY",
            &["fal.run", "queue.fal.run"],
            Some("/v1"),
        )];
        match plan_copy("agent-a", &src, "FAL_KEY", &same, true, None).unwrap() {
            CopyPlan::Overwrite { resync_generic, .. } => assert!(resync_generic.is_none()),
            _ => panic!("expected Overwrite"),
        }
        let diff = vec![generic("other-fal", "FAL_KEY", &["fal.run"], Some("/v1"))];
        match plan_copy("agent-a", &src, "FAL_KEY", &diff, true, None).unwrap() {
            CopyPlan::Overwrite { resync_generic, .. } => {
                let g = resync_generic.expect("resync");
                assert_eq!(g.upstream_hosts, vec!["fal.run", "queue.fal.run"]);
            }
            _ => panic!("expected Overwrite"),
        }
    }

    #[test]
    fn create_generic_source_maps_config() {
        let src = generic(
            "agent-a-provider",
            "FAL_KEY",
            &["fal.run", "queue.fal.run"],
            Some("/v1"),
        );
        let plan = plan_copy("agent-a", &src, "FAL_KEY", &[], false, None).unwrap();
        match plan {
            CopyPlan::Create {
                type_,
                label,
                generic,
            } => {
                assert_eq!(type_, "generic");
                assert_eq!(label.as_deref(), Some("fal"));
                let g = generic.expect("generic create body");
                assert_eq!(g.env_var, "FAL_KEY");
                assert_eq!(
                    g.upstream_hosts,
                    Some(vec!["fal.run".to_string(), "queue.fal.run".to_string()])
                );
                assert_eq!(g.upstream_path_prefix, Some("/v1".to_string()));
                assert!(g.upstream_host.is_none());
            }
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn create_label_none_when_name_lacks_agent_prefix() {
        let src = builtin("alice-fal", "right-fal");
        let plan = plan_copy("bob", &src, "FAL_KEY", &[], false, None).unwrap();
        match plan {
            CopyPlan::Create { label, .. } => assert!(label.is_none()),
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn create_generic_source_missing_block_errors() {
        let src = right_agent_config::ProviderEntry {
            name: "agent-a-provider".into(),
            type_: right_agent_config::ProviderType::Generic,
            label: None,
            generic: None,
        };
        let err = plan_copy("agent-a", &src, "FAL_KEY", &[], false, None).unwrap_err();
        assert!(matches!(err, ProviderApiError::InvalidName { .. }));
    }
}

fn provider_gateway_type(entry: &right_agent_config::ProviderEntry) -> String {
    match &entry.type_ {
        right_agent_config::ProviderType::Generic => {
            right_openshell::managed_profiles::generic_provider_profile_id(&entry.name)
        }
        right_agent_config::ProviderType::BuiltIn(slug) => slug.clone(),
    }
}

fn provider_view_type(entry: &right_agent_config::ProviderEntry) -> String {
    match &entry.type_ {
        right_agent_config::ProviderType::Generic => "generic".to_string(),
        right_agent_config::ProviderType::BuiltIn(slug) => slug.clone(),
    }
}

fn provider_entry_is_composed(
    policy: &right_openshell::openshell_proto::openshell::sandbox::v1::SandboxPolicy,
    entry: &right_agent_config::ProviderEntry,
) -> bool {
    match &entry.type_ {
        right_agent_config::ProviderType::BuiltIn(_) => {
            right_openshell::provider_capabilities::provider_is_composed(policy, &entry.name)
        }
        right_agent_config::ProviderType::Generic => {
            entry.generic.as_ref().is_some_and(|generic| {
                let expected = generic_expected_endpoints(
                    &generic.upstream_hosts,
                    generic.upstream_path_prefix.as_deref(),
                );
                right_openshell::provider_capabilities::provider_is_composed_with_exact_endpoints(
                    policy,
                    &entry.name,
                    &expected,
                )
            })
        }
    }
}

fn validate_generic_env_var_unchanged(
    provider_name: &str,
    current_env_var: &str,
    new_env_var: &str,
) -> Result<(), ProviderApiError> {
    if new_env_var != current_env_var {
        return Err(ProviderApiError::GenericEnvVarChangeRequiresCredential {
            name: provider_name.to_string(),
        });
    }
    Ok(())
}

/// Append a provider entry to `sandbox.providers:` in agent.yaml,
/// preserving comments and unknown fields via line-walking on the raw YAML.
fn append_provider_to_yaml(
    agents_dir: &std::path::Path,
    agent: &str,
    entry: &right_agent_config::ProviderEntry,
) -> miette::Result<()> {
    let path = agents_dir.join(agent).join("agent.yaml");
    let serialized_entry = serialize_provider_entry(entry);
    right_codegen::contract::write_merged_rmw(&path, |existing| {
        let original = existing.unwrap_or("");
        let updated = insert_provider_entry(original, &serialized_entry)?;
        Ok(updated)
    })
}

/// Render the provider entry as YAML lines at 4-space indentation (nested
/// under `sandbox.providers:` which is at column 2).
///
/// All free-form string scalars are single-quoted (`yaml_single_quote`)
/// so that values like `label: "no"` or `label: "123"` round-trip as
/// strings instead of getting reinterpreted by YAML 1.1 loaders as
/// booleans or numbers — which would break `Option<String>` /
/// `String` deserialization on the next bot restart.
fn serialize_provider_entry(entry: &right_agent_config::ProviderEntry) -> String {
    let mut out = String::new();
    out.push_str(&format!("    - name: {}\n", yaml_single_quote(&entry.name)));
    let type_str = match &entry.type_ {
        right_agent_config::ProviderType::Generic => "generic".to_string(),
        right_agent_config::ProviderType::BuiltIn(s) => s.clone(),
    };
    out.push_str(&format!("      type: {}\n", yaml_single_quote(&type_str)));
    if let Some(label) = &entry.label {
        out.push_str(&format!("      label: {}\n", yaml_single_quote(label)));
    }
    if let Some(g) = &entry.generic {
        out.push_str("      generic:\n");
        out.push_str(&format!(
            "        env_var: {}\n",
            yaml_single_quote(&g.env_var)
        ));
        out.push_str("        upstream_hosts:\n");
        for host in &g.upstream_hosts {
            out.push_str(&format!("          - {}\n", yaml_single_quote(host)));
        }
        if let Some(prefix) = &g.upstream_path_prefix {
            out.push_str(&format!(
                "        upstream_path_prefix: {}\n",
                yaml_single_quote(prefix)
            ));
        }
    }
    out
}

/// Locate (or create) `sandbox.providers:` and insert the serialized entry
/// at the end of that list. Comments and unknown fields are preserved.
fn insert_provider_entry(original: &str, entry_yaml: &str) -> miette::Result<String> {
    let lines: Vec<&str> = original.split_inclusive('\n').collect();

    // Find `sandbox:` at column 0.
    let sandbox_start = lines
        .iter()
        .position(|l| {
            l.trim_end() == "sandbox:" || (l.starts_with("sandbox:") && !l.starts_with("sandbox: "))
        })
        .ok_or_else(|| miette::miette!("agent.yaml: sandbox: section missing"))?;

    // Find the end of the sandbox block (next top-level non-blank key OR end of file).
    let mut sandbox_end = lines.len();
    for (i, line) in lines.iter().enumerate().skip(sandbox_start + 1) {
        let ch = line.chars().next();
        if let Some(c) = ch
            && c != ' '
            && c != '\t'
            && c != '\n'
            && c != '\r'
            && c != '#'
        {
            sandbox_end = i;
            break;
        }
    }

    // Within the sandbox block, find `  providers:` at exactly column 2.
    let providers_idx = lines
        .iter()
        .enumerate()
        .skip(sandbox_start + 1)
        .take(sandbox_end - sandbox_start - 1)
        .find(|(_, l)| {
            l.starts_with("  ")
                && !l.starts_with("   ")
                && l.trim_start_matches(' ').starts_with("providers:")
        })
        .map(|(i, _)| i);

    let mut out = String::with_capacity(original.len() + entry_yaml.len() + 32);

    if let Some(p_idx) = providers_idx {
        // Find end of the providers list — next column-2 non-blank key OR end of sandbox block.
        let mut list_end = sandbox_end;
        for (i, line) in lines
            .iter()
            .enumerate()
            .skip(p_idx + 1)
            .take(sandbox_end - p_idx - 1)
        {
            let is_col2_key =
                line.starts_with("  ") && !line.starts_with("   ") && !line.trim().is_empty();
            if is_col2_key {
                list_end = i;
                break;
            }
        }
        // Insert entry BEFORE list_end.
        for line in &lines[..list_end] {
            out.push_str(line);
        }
        out.push_str(entry_yaml);
        for line in &lines[list_end..] {
            out.push_str(line);
        }
    } else {
        // No `providers:` key yet — insert at end of sandbox block.
        for line in &lines[..sandbox_end] {
            out.push_str(line);
        }
        // Ensure the last sandbox line ends with newline before inserting.
        if sandbox_end > sandbox_start && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("  providers:\n");
        out.push_str(entry_yaml);
        for line in &lines[sandbox_end..] {
            out.push_str(line);
        }
    }
    Ok(out)
}

fn generic_provider_profile_and_spec(
    provider_name: &str,
    env_var: &str,
    credential: &str,
) -> (String, right_openshell::providers::ProviderSpec) {
    let profile_id = right_openshell::managed_profiles::generic_provider_profile_id(provider_name);
    let mut credentials = std::collections::HashMap::new();
    credentials.insert(env_var.to_string(), credential.to_string());
    let spec = right_openshell::providers::ProviderSpec {
        name: provider_name.to_string(),
        type_: profile_id.clone(),
        credentials,
        config: Default::default(),
    };
    (profile_id, spec)
}

fn generic_provider_update_profile(
    provider_name: &str,
    generic: &right_agent_config::GenericProvider,
) -> (
    String,
    right_openshell::openshell_proto::openshell::v1::ProviderProfile,
) {
    let profile_id = right_openshell::managed_profiles::generic_provider_profile_id(provider_name);
    let profile = right_openshell::managed_profiles::author_generic_profile(
        &profile_id,
        &generic.upstream_hosts,
        generic.upstream_path_prefix.as_deref(),
        &generic.env_var,
    );
    (profile_id, profile)
}

async fn ensure_provider_policy_loaded_after_rollback(
    provider_name: &str,
    sandbox_name: &str,
    policy_path: &std::path::Path,
    original_err: String,
    rollback_reason: &str,
) {
    if let Err(rollback_err) =
        right_openshell::openshell::ensure_provider_policy_loaded(sandbox_name, policy_path).await
    {
        tracing::warn!(
            provider = %provider_name,
            original_err = %original_err,
            rollback_reason,
            "provider rollback failed: could not ensure provider policy was loaded after rollback: {rollback_err:#}"
        );
    }
}

/// Best-effort rollback of a provider that was already attached to the sandbox
/// before a later create step failed: detach, delete, then reload composition so
/// the active policy no longer references it. Each step is best-effort and
/// logged; the caller still returns the original error. The authored generic
/// profile is intentionally left for reconcile to refresh (see
/// `create_generic_provider`).
async fn rollback_attached_provider(
    client: &mut OpenShellClient,
    provider_name: &str,
    sandbox_name: &str,
    policy_path: &std::path::Path,
    original_err: &str,
    rollback_reason: &str,
) {
    if let Err(rollback_err) =
        right_openshell::providers::detach_from_sandbox(client, sandbox_name, provider_name).await
    {
        tracing::warn!(
            provider = %provider_name,
            original_err,
            rollback_reason,
            "provider rollback failed: could not detach provider: {rollback_err:#}"
        );
    }
    if let Err(rollback_err) =
        right_openshell::providers::delete_provider(client, provider_name).await
    {
        tracing::warn!(
            provider = %provider_name,
            original_err,
            rollback_reason,
            "provider rollback failed: could not delete provider: {rollback_err:#}"
        );
    }
    ensure_provider_policy_loaded_after_rollback(
        provider_name,
        sandbox_name,
        policy_path,
        original_err.to_string(),
        rollback_reason,
    )
    .await;
}

async fn reensure_generic_profile_after_rollback(
    client: &mut OpenShellClient,
    sandbox_name: &str,
    provider_name: &str,
    generic: &right_agent_config::GenericProvider,
    original_err: String,
    rollback_reason: &str,
) -> Vec<String> {
    // Restoring the prior profile means replacing an id that the gateway already
    // holds — `ensure_profiles` cannot do that (it reports drift and skips), so
    // the restore must go through the same detach-dance the forward update uses.
    let (_, profile) = generic_provider_update_profile(provider_name, generic);
    let attachments = vec![right_openshell::providers::ProfileAttachment {
        sandbox_name: sandbox_name.to_string(),
        provider_name: provider_name.to_string(),
    }];
    match right_openshell::providers::update_referenced_profile(client, &attachments, profile).await
    {
        Ok(()) => Vec::new(),
        Err(rollback_err) => {
            tracing::warn!(
                provider = %provider_name,
                original_err = %original_err,
                rollback_reason,
                "provider rollback failed: could not restore generic provider profile after rollback: {rollback_err:#}"
            );
            vec![format!("profile: {rollback_err:#}")]
        }
    }
}

async fn create_generic_provider(
    state: crate::internal_api::InternalState,
    req: ProviderCreateReq,
    name: String,
    env_var: String,
    upstream_hosts: Vec<String>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    use secrecy::ExposeSecret;
    // Lock is already held by the caller (handle_provider_create acquires it
    // before dispatching here), so no second acquisition needed.
    let g = req
        .generic
        .clone()
        .ok_or_else(|| ProviderApiError::InvalidEnvVar { env_var: "".into() })?;

    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    let sandbox = cfg
        .sandbox
        .as_ref()
        .ok_or(ProviderApiError::SandboxModeNone)?;
    let sandbox_name = sandbox.name.clone().unwrap_or_else(|| req.agent.clone());
    let policy_path = state.agents_dir.join(&req.agent).join(
        sandbox
            .policy_file
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("policy.yaml")),
    );

    let mut client = open_openshell_client().await?;
    ensure_v2_for_mutation(&mut client).await?;
    let (profile_id, spec) =
        generic_provider_profile_and_spec(&name, &env_var, req.credential.expose_secret());
    let profile = right_openshell::managed_profiles::author_generic_profile(
        &profile_id,
        &upstream_hosts,
        g.upstream_path_prefix.as_deref(),
        &env_var,
    );
    let managed_profile =
        right_openshell::managed_profiles::ManagedProfile::Authored(Box::new(profile));
    right_openshell::managed_profiles::ensure_profiles(&mut client, &[managed_profile])
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("profile import: {e:#}")))?;

    // Right-managed authored profiles are non-secret gateway cache. If provider
    // creation fails, startup reconciliation can leave or refresh this profile.
    right_openshell::providers::create_provider(&mut client, &spec)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;

    if let Err(attach_err) =
        right_openshell::providers::attach_to_sandbox(&mut client, &sandbox_name, &name).await
    {
        if let Err(rollback_err) =
            right_openshell::providers::delete_provider(&mut client, &name).await
        {
            tracing::warn!(
                provider = %name,
                original_err = %attach_err,
                "provider rollback failed: could not delete provider after attach failure: {rollback_err:#}"
            );
        }
        return Err(ProviderApiError::Gateway(format!("{attach_err:#}")));
    }

    if let Err(ensure_err) =
        right_openshell::openshell::ensure_provider_policy_loaded(&sandbox_name, &policy_path).await
    {
        rollback_attached_provider(
            &mut client,
            &name,
            &sandbox_name,
            &policy_path,
            &format!("{ensure_err:#}"),
            "policy-load failure",
        )
        .await;
        return Err(ProviderApiError::Gateway(format!(
            "policy load: {ensure_err:#}"
        )));
    }

    let expected_endpoints =
        generic_expected_endpoints(&upstream_hosts, g.upstream_path_prefix.as_deref());
    if let Err(compose_err) =
        right_openshell::openshell::wait_for_provider_composed_with_exact_endpoints(
            &mut client,
            &sandbox_name,
            &name,
            expected_endpoints,
        )
        .await
    {
        rollback_attached_provider(
            &mut client,
            &name,
            &sandbox_name,
            &policy_path,
            &format!("{compose_err:#}"),
            "composition not confirmed",
        )
        .await;
        return Err(ProviderApiError::Gateway(format!("{compose_err:#}")));
    }

    let generic_entry = right_agent_config::GenericProvider {
        env_var: env_var.clone(),
        upstream_hosts,
        upstream_path_prefix: g.upstream_path_prefix.clone(),
    };
    let entry = right_agent_config::ProviderEntry {
        name: name.clone(),
        type_: right_agent_config::ProviderType::Generic,
        label: req.label.clone(),
        generic: Some(generic_entry.clone()),
    };
    if let Err(e) = append_provider_to_yaml(&state.agents_dir, &req.agent, &entry) {
        rollback_attached_provider(
            &mut client,
            &name,
            &sandbox_name,
            &policy_path,
            &format!("{e:#}"),
            "yaml-write failure",
        )
        .await;
        return Err(ProviderApiError::AgentYamlWrite(format!("{e:#}")));
    }

    Ok(axum::Json(ProviderView {
        name,
        type_: "generic".to_string(),
        label: req.label,
        env_var,
        generic: Some(generic_entry),
        updated_at: None,
        status: ProviderStatus::Healthy,
        composed: Some(true),
    }))
}

#[cfg(test)]
mod generic_provider_spec_tests {
    use super::*;

    #[test]
    fn create_generic_request_accepts_multi_host_request() {
        let g: ProviderCreateGeneric = serde_json::from_value(serde_json::json!({
            "env_var": "FAL_KEY",
            "upstream_hosts": ["fal.run", "queue.fal.run"],
            "upstream_path_prefix": "/v1",
        }))
        .expect("multi-host generic request must deserialize");

        let hosts = validate_generic_request(
            &g.env_var,
            g.upstream_host.as_deref(),
            g.upstream_hosts.as_deref(),
            g.upstream_path_prefix.as_deref(),
        )
        .expect("multi-host generic request must validate");

        assert_eq!(hosts, vec!["fal.run", "queue.fal.run"]);
    }

    #[test]
    fn create_generic_request_rejects_empty_hosts() {
        let g: ProviderCreateGeneric = serde_json::from_value(serde_json::json!({
            "env_var": "FAL_KEY",
            "upstream_host": "  ",
            "upstream_hosts": ["", "   "],
        }))
        .expect("empty-host generic request must deserialize for validation");

        let err = validate_generic_request(
            &g.env_var,
            g.upstream_host.as_deref(),
            g.upstream_hosts.as_deref(),
            g.upstream_path_prefix.as_deref(),
        )
        .expect_err("generic request with no normalized hosts must fail validation");

        assert!(
            matches!(err, ProviderApiError::InvalidName { .. }),
            "empty hosts must be rejected as invalid input, got {err:?}"
        );
    }

    #[test]
    fn create_generic_request_merges_legacy_and_new_hosts() {
        let g: ProviderCreateGeneric = serde_json::from_value(serde_json::json!({
            "env_var": "FAL_KEY",
            "header_name": "This-Legacy-Field-Is-Ignored",
            "upstream_host": " fal.run ",
            "upstream_hosts": ["queue.fal.run ", "fal.run", "", "  "],
        }))
        .expect("legacy plus new generic request must deserialize");

        let hosts = validate_generic_request(
            &g.env_var,
            g.upstream_host.as_deref(),
            g.upstream_hosts.as_deref(),
            g.upstream_path_prefix.as_deref(),
        )
        .expect("legacy plus new generic request must validate");

        assert_eq!(hosts, vec!["fal.run", "queue.fal.run"]);
    }

    #[test]
    fn serialize_generic_provider_writes_upstream_hosts_only() {
        let entry = right_agent_config::ProviderEntry {
            name: "hostagent-fal".into(),
            type_: right_agent_config::ProviderType::Generic,
            label: Some("fal".into()),
            generic: Some(right_agent_config::GenericProvider {
                env_var: "FAL_KEY".into(),
                upstream_hosts: vec!["fal.run".into(), "queue.fal.run".into()],
                upstream_path_prefix: Some("/v1".into()),
            }),
        };

        let serialized = serialize_provider_entry(&entry);

        assert!(
            serialized.contains(
                "        upstream_hosts:\n          - 'fal.run'\n          - 'queue.fal.run'\n"
            ),
            "generic provider must serialize all hosts as a YAML list:\n{serialized}"
        );
        assert!(
            !serialized.contains("header_name"),
            "header_name must not be written for generic providers:\n{serialized}"
        );
        assert!(
            !serialized.contains("upstream_host:"),
            "legacy single upstream_host must not be written:\n{serialized}"
        );
    }

    #[test]
    fn generic_provider_spec_uses_profile_id() {
        let expected_profile_id =
            right_openshell::managed_profiles::generic_provider_profile_id("right-acme");
        let (profile_id, spec) =
            generic_provider_profile_and_spec("right-acme", "MY_API_KEY", "secret-value");

        assert_eq!(profile_id, expected_profile_id);
        assert_eq!(spec.name, "right-acme");
        assert_eq!(spec.type_, expected_profile_id);
        assert_eq!(
            spec.credentials.get("MY_API_KEY").map(String::as_str),
            Some("secret-value")
        );
        assert!(
            spec.config.is_empty(),
            "generic provider record should not carry profile config"
        );

        let expected_profile_id =
            right_openshell::managed_profiles::generic_provider_profile_id("acme");
        let (profile_id, spec) =
            generic_provider_profile_and_spec("acme", "MY_API_KEY", "secret-value");
        assert_eq!(profile_id, expected_profile_id);
        assert_eq!(spec.name, "acme");
        assert_eq!(spec.type_, expected_profile_id);
    }

    #[test]
    fn generic_provider_update_spec_uses_profile_id_but_view_type_stays_generic() {
        let entry = right_agent_config::ProviderEntry {
            name: "hostagent-acme".into(),
            type_: right_agent_config::ProviderType::Generic,
            label: None,
            generic: Some(right_agent_config::GenericProvider {
                env_var: "ACME_API_KEY".into(),
                upstream_hosts: vec!["api.acme.invalid".into()],
                upstream_path_prefix: Some("/v1".into()),
            }),
        };

        let expected_profile_id =
            right_openshell::managed_profiles::generic_provider_profile_id("hostagent-acme");

        assert_eq!(provider_gateway_type(&entry), expected_profile_id);
        assert_eq!(provider_view_type(&entry), "generic");
    }

    #[test]
    fn generic_provider_config_update_authors_profile_only() {
        let generic = right_agent_config::GenericProvider {
            env_var: "ACME_API_KEY".into(),
            upstream_hosts: vec!["api.acme.invalid".into(), "queue.acme.invalid".into()],
            upstream_path_prefix: Some("/v2".into()),
        };

        let (profile_id, profile) = generic_provider_update_profile("hostagent-acme", &generic);

        assert_eq!(
            profile_id,
            right_openshell::managed_profiles::generic_provider_profile_id("hostagent-acme")
        );
        assert_eq!(profile.id, profile_id);
        assert_eq!(profile.endpoints[0].host, "api.acme.invalid");
        assert_eq!(profile.endpoints[1].host, "queue.acme.invalid");
        assert_eq!(profile.endpoints[0].path, "/v2");
        assert_eq!(profile.endpoints[1].path, "/v2");
        assert_eq!(profile.credentials[0].env_vars, vec!["ACME_API_KEY"]);
    }

    #[test]
    fn generic_config_update_rejects_env_var_change_without_credential() {
        let err =
            validate_generic_env_var_unchanged("hostagent-acme", "ACME_API_KEY", "NEW_API_KEY")
                .expect_err("env_var change must require a credential-bearing flow");

        assert!(
            matches!(
                err,
                ProviderApiError::GenericEnvVarChangeRequiresCredential { .. }
            ),
            "expected dedicated env-var change error, got {err:?}"
        );
    }
}

#[cfg(test)]
mod insert_tests {
    use super::*;

    #[test]
    fn insert_into_empty_sandbox() {
        let original = "name: foo\nsandbox:\n  mode: openshell\n";
        let entry = "    - name: foo-bar\n      type: anthropic\n";
        let out = insert_provider_entry(original, entry).unwrap();
        assert!(
            out.contains("providers:\n    - name: foo-bar"),
            "expected providers key followed by entry, got:\n{out}"
        );
    }

    #[test]
    fn insert_into_existing_providers() {
        let original = "sandbox:\n  mode: openshell\n  providers:\n    - name: x\n      type: y\n";
        let entry = "    - name: foo-bar\n      type: anthropic\n";
        let out = insert_provider_entry(original, entry).unwrap();
        assert!(
            out.contains("- name: foo-bar"),
            "new entry missing from:\n{out}"
        );
        assert!(
            out.contains("- name: x"),
            "existing entry missing from:\n{out}"
        );
    }

    #[test]
    fn replace_existing_provider_swaps_in_place() {
        let original = "sandbox:\n  providers:\n    - name: foo-x\n      type: anthropic\n    - name: foo-y\n      type: github\n";
        let new_entry = "    - name: foo-x\n      type: openai\n";
        let out = replace_provider_entry(original, "foo-x", new_entry).unwrap();
        assert!(
            out.contains("- name: foo-x\n      type: openai"),
            "replaced entry not found in:\n{out}"
        );
        assert!(
            out.contains("- name: foo-y\n      type: github"),
            "sibling entry missing from:\n{out}"
        );
        assert!(
            !out.contains("type: anthropic"),
            "old entry still present in:\n{out}"
        );
    }

    #[test]
    fn remove_provider_drops_entry_only() {
        let original = "sandbox:\n  providers:\n    - name: foo-x\n      type: anthropic\n    - name: foo-y\n      type: github\n";
        let out = remove_provider_entry(original, "foo-x");
        assert!(!out.contains("foo-x"), "foo-x still present in:\n{out}");
        assert!(
            out.contains("- name: foo-y"),
            "sibling entry missing from:\n{out}"
        );
    }

    /// New writes single-quote names, so subsequent remove/replace operations
    /// must locate quoted entries too — not just legacy unquoted ones.
    #[test]
    fn remove_provider_handles_quoted_name() {
        let original = "sandbox:\n  providers:\n    - name: 'foo-x'\n      type: 'anthropic'\n    - name: 'foo-y'\n      type: 'github'\n";
        let out = remove_provider_entry(original, "foo-x");
        assert!(!out.contains("foo-x"), "foo-x still present in:\n{out}");
        assert!(
            out.contains("- name: 'foo-y'"),
            "sibling entry missing from:\n{out}"
        );
    }

    #[test]
    fn replace_provider_handles_quoted_name() {
        let original = "sandbox:\n  providers:\n    - name: 'foo-x'\n      type: 'anthropic'\n    - name: 'foo-y'\n      type: 'github'\n";
        let new_entry = "    - name: 'foo-x'\n      type: 'openai'\n";
        let out = replace_provider_entry(original, "foo-x", new_entry).unwrap();
        assert!(
            out.contains("- name: 'foo-x'\n      type: 'openai'"),
            "replaced entry not found in:\n{out}"
        );
        assert!(
            !out.contains("type: 'anthropic'"),
            "old entry still present in:\n{out}"
        );
    }

    /// Searching for an unquoted legacy entry whose name is a prefix of a
    /// longer name must not match the longer entry. Both `myagent-foo` and
    /// `myagent-foo-bar` are valid slugs, so this is reachable with
    /// legitimate user input on legacy unquoted agent.yaml files.
    #[test]
    fn find_provider_name_marker_unquoted_does_not_match_prefix() {
        let haystack =
            "sandbox:\n  providers:\n    - name: myagent-foo-bar\n      type: anthropic\n";
        assert_eq!(find_provider_name_marker(haystack, "myagent-foo"), None);
    }

    /// With both an exact unquoted entry and a longer-name entry present, the
    /// search must return the offset of the exact match (not the longer one),
    /// and the longer-name search must return the longer entry.
    #[test]
    fn find_provider_name_marker_unquoted_matches_only_exact_followed_by_newline() {
        let haystack = "sandbox:\n  providers:\n    - name: myagent-foo\n      type: anthropic\n    - name: myagent-foo-bar\n      type: github\n";
        let foo_idx = find_provider_name_marker(haystack, "myagent-foo")
            .expect("exact unquoted match should resolve");
        let foo_bar_idx = find_provider_name_marker(haystack, "myagent-foo-bar")
            .expect("longer unquoted match should resolve");
        assert!(
            foo_idx < foo_bar_idx,
            "exact match must come before longer-name match: foo={foo_idx} foo-bar={foo_bar_idx}"
        );
        assert_eq!(
            &haystack[foo_idx..foo_idx + "    - name: myagent-foo".len()],
            "    - name: myagent-foo"
        );
        assert_eq!(
            &haystack[foo_bar_idx..foo_bar_idx + "    - name: myagent-foo-bar".len()],
            "    - name: myagent-foo-bar"
        );
    }

    /// Quoted form is bounded by the closing quote, so prefix collisions are
    /// impossible. Sanity-check that the quoted path still resolves the
    /// expected offset.
    #[test]
    fn find_provider_name_marker_quoted_matches() {
        let haystack =
            "sandbox:\n  providers:\n    - name: 'myagent-foo'\n      type: 'anthropic'\n";
        let idx = find_provider_name_marker(haystack, "myagent-foo")
            .expect("quoted match should resolve");
        assert_eq!(
            &haystack[idx..idx + "    - name: 'myagent-foo'".len()],
            "    - name: 'myagent-foo'"
        );
    }

    /// Integration test: with both an exact unquoted entry and a
    /// longer-name unquoted entry, removing the shorter name must drop the
    /// shorter row and leave the longer-name sibling untouched.
    #[test]
    fn remove_provider_entry_unquoted_does_not_remove_wrong_row() {
        let original = "sandbox:\n  providers:\n    - name: myagent-foo\n      type: anthropic\n    - name: myagent-foo-bar\n      type: github\n";
        let out = remove_provider_entry(original, "myagent-foo");
        assert!(
            out.contains("- name: myagent-foo-bar\n      type: github"),
            "longer-name sibling must be preserved in:\n{out}"
        );
        assert!(
            !out.contains("- name: myagent-foo\n      type: anthropic"),
            "exact entry should have been removed from:\n{out}"
        );
    }
}

// ── Tasks 19–22: /provider-rotate, /provider-config-update, /provider-remove ─

// ── Task 22 (mutex helper) ───────────────────────────────────────────────────

/// Acquire the per-agent provider mutation lock.
///
/// All provider mutations on a given agent eventually RMW the same
/// `agents/<agent>/agent.yaml`. Keying the lock on `agent` alone (not on
/// `(agent, name)`) serializes those RMWs and prevents a last-write-wins
/// race that would otherwise drop one of two concurrently-created
/// providers from agent.yaml while leaving gateway provider/profile state
/// already mutated for it (an orphan).
///
/// Callers MUST invoke `validate_name(agent, name)` (where applicable)
/// before this so the lock map's key space stays bounded to validated
/// agent names — user-supplied agents that fail validation never reach
/// the lock map.
pub(crate) async fn provider_lock(
    state: &crate::internal_api::InternalState,
    agent: &str,
) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut map = state.provider_locks.lock().await;
        map.entry(agent.to_string())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}

// ── Task 19: /provider-rotate ────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ProviderRotateReq {
    pub agent: String,
    pub name: String,
    pub credential: secrecy::SecretString,
}

pub(crate) async fn handle_provider_rotate(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderRotateReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    use secrecy::ExposeSecret;
    validate_name(&req.agent, &req.name)?;
    let _guard = provider_lock(&state, &req.agent).await;

    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    let sandbox = cfg
        .sandbox
        .as_ref()
        .ok_or(ProviderApiError::SandboxModeNone)?;
    let entry = sandbox
        .providers
        .iter()
        .find(|p| p.name == req.name)
        .ok_or_else(|| ProviderApiError::NotFound {
            name: req.name.clone(),
        })?;

    let env_var = extract_env_var(entry)?;
    let mut client = open_openshell_client().await?;
    let mut creds = std::collections::HashMap::new();
    creds.insert(env_var.clone(), req.credential.expose_secret().to_string());
    let gateway_type = provider_gateway_type(entry);
    let view_type = provider_view_type(entry);
    let spec = right_openshell::providers::ProviderSpec {
        name: req.name.clone(),
        type_: gateway_type,
        credentials: creds,
        config: Default::default(),
    };
    right_openshell::providers::update_provider(&mut client, &spec)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;

    // Rotation changes only the credential, not composition. Report the live
    // composed state so the dashboard doesn't flash "Unknown" after a rotate;
    // degrade to None only when the active policy can't be read.
    let sandbox_name = sandbox.name.clone().unwrap_or_else(|| req.agent.clone());
    let composed =
        match right_openshell::openshell::get_effective_policy(&mut client, &sandbox_name).await {
            Ok(policy) => policy
                .as_ref()
                .map(|policy| provider_entry_is_composed(policy, entry)),
            Err(e) => {
                tracing::warn!(
                    agent = %req.agent,
                    sandbox = %sandbox_name,
                    error = %format_args!("{e:#}"),
                    "provider composition state unavailable after rotate: active policy read failed"
                );
                None
            }
        };

    Ok(axum::Json(ProviderView {
        name: req.name,
        type_: view_type,
        label: entry.label.clone(),
        env_var,
        generic: entry.generic.clone(),
        updated_at: Some(chrono::Utc::now()),
        status: ProviderStatus::Healthy,
        composed,
    }))
}

// ── Task 20: /provider-config-update ─────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ProviderConfigUpdateReq {
    pub agent: String,
    pub name: String,
    pub generic: ProviderConfigUpdateGeneric,
}

#[derive(Debug, serde::Deserialize)]
pub struct ProviderConfigUpdateGeneric {
    pub env_var: Option<String>,
    #[serde(default)]
    pub upstream_host: Option<String>,
    #[serde(default)]
    pub upstream_hosts: Option<Vec<String>>,
    pub upstream_path_prefix: Option<Option<String>>,
}

pub(crate) async fn handle_provider_config_update(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderConfigUpdateReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    validate_name(&req.agent, &req.name)?;
    let _guard = provider_lock(&state, &req.agent).await;

    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    let sandbox = cfg
        .sandbox
        .as_ref()
        .ok_or(ProviderApiError::SandboxModeNone)?;
    let entry = sandbox
        .providers
        .iter()
        .find(|p| p.name == req.name)
        .ok_or_else(|| ProviderApiError::NotFound {
            name: req.name.clone(),
        })?;
    if !matches!(entry.type_, right_agent_config::ProviderType::Generic) {
        return Err(ProviderApiError::InvalidName {
            name: req.name.clone(),
            reason: "config-update only valid on generic providers".into(),
        });
    }
    // Same rationale as `handle_provider_create`: generic provider profiles
    // are only supported under the permissive network policy.
    if matches!(
        cfg.network_policy,
        right_agent_config::NetworkPolicy::Restrictive
    ) {
        return Err(ProviderApiError::NetworkPolicyForbidsGeneric);
    }
    let current = entry
        .generic
        .clone()
        .ok_or_else(|| ProviderApiError::InvalidName {
            name: req.name.clone(),
            reason: "generic provider entry missing 'generic:' block in agent.yaml".into(),
        })?;
    let new_env_var = req
        .generic
        .env_var
        .clone()
        .unwrap_or(current.env_var.clone());
    let new_path = match req.generic.upstream_path_prefix.clone() {
        None => current.upstream_path_prefix.clone(),
        Some(v) => v,
    };
    let fallback_hosts =
        if req.generic.upstream_host.is_none() && req.generic.upstream_hosts.is_none() {
            Some(current.upstream_hosts.clone())
        } else {
            None
        };
    let new_hosts = validate_generic_request(
        &new_env_var,
        req.generic.upstream_host.as_deref(),
        req.generic
            .upstream_hosts
            .as_deref()
            .or(fallback_hosts.as_deref()),
        new_path.as_deref(),
    )?;
    validate_generic_env_var_unchanged(&req.name, &current.env_var, &new_env_var)?;

    let mut client = open_openshell_client().await?;
    ensure_v2_for_mutation(&mut client).await?;
    let sandbox_name = sandbox.name.clone().unwrap_or_else(|| req.agent.clone());
    let policy_path = state.agents_dir.join(&req.agent).join(
        sandbox
            .policy_file
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("policy.yaml")),
    );

    let updated_generic = right_agent_config::GenericProvider {
        env_var: new_env_var.clone(),
        upstream_hosts: new_hosts.clone(),
        upstream_path_prefix: new_path.clone(),
    };
    let (_, profile) = generic_provider_update_profile(&req.name, &updated_generic);
    let attachments = vec![right_openshell::providers::ProfileAttachment {
        sandbox_name: sandbox_name.clone(),
        provider_name: req.name.clone(),
    }];
    if let Err(e) =
        right_openshell::providers::update_referenced_profile(&mut client, &attachments, profile)
            .await
    {
        let rollback_errors = reensure_generic_profile_after_rollback(
            &mut client,
            &sandbox_name,
            &req.name,
            &current,
            format!("{e:#}"),
            "profile update failure",
        )
        .await;
        let mut msg = format!("profile update: {e:#}");
        if !rollback_errors.is_empty() {
            msg.push_str(" (rollback also failed: ");
            msg.push_str(&rollback_errors.join("; "));
            msg.push(')');
        }
        return Err(ProviderApiError::Gateway(msg));
    }

    if let Err(e) =
        right_openshell::openshell::ensure_provider_policy_loaded(&sandbox_name, &policy_path).await
    {
        let rollback_errors = reensure_generic_profile_after_rollback(
            &mut client,
            &sandbox_name,
            &req.name,
            &current,
            format!("{e:#}"),
            "policy-load failure",
        )
        .await;
        ensure_provider_policy_loaded_after_rollback(
            &req.name,
            &sandbox_name,
            &policy_path,
            format!("{e:#}"),
            "policy-load failure",
        )
        .await;
        let mut msg = format!("policy load: {e:#}");
        if !rollback_errors.is_empty() {
            msg.push_str(" (rollback also failed: ");
            msg.push_str(&rollback_errors.join("; "));
            msg.push(')');
        }
        return Err(ProviderApiError::Gateway(msg));
    }

    let expected_endpoints = generic_expected_endpoints(&new_hosts, new_path.as_deref());
    if let Err(e) = right_openshell::openshell::wait_for_provider_composed_with_exact_endpoints(
        &mut client,
        &sandbox_name,
        &req.name,
        expected_endpoints,
    )
    .await
    {
        let rollback_errors = reensure_generic_profile_after_rollback(
            &mut client,
            &sandbox_name,
            &req.name,
            &current,
            format!("{e:#}"),
            "composition failure",
        )
        .await;
        ensure_provider_policy_loaded_after_rollback(
            &req.name,
            &sandbox_name,
            &policy_path,
            format!("{e:#}"),
            "composition not confirmed",
        )
        .await;
        let mut msg = format!("{e:#}");
        if !rollback_errors.is_empty() {
            msg.push_str(" (rollback also failed: ");
            msg.push_str(&rollback_errors.join("; "));
            msg.push(')');
        }
        return Err(ProviderApiError::Gateway(msg));
    }

    let updated = right_agent_config::ProviderEntry {
        name: req.name.clone(),
        type_: right_agent_config::ProviderType::Generic,
        label: entry.label.clone(),
        generic: Some(updated_generic.clone()),
    };
    if let Err(e) = replace_provider_in_yaml(&state.agents_dir, &req.agent, &updated) {
        // Gateway profile accepted the new config but agent.yaml is now stale.
        // Roll the profile back so the proxy matches the file.
        let rollback_errors = reensure_generic_profile_after_rollback(
            &mut client,
            &sandbox_name,
            &req.name,
            &current,
            format!("{e:#}"),
            "yaml-write failure",
        )
        .await;
        ensure_provider_policy_loaded_after_rollback(
            &req.name,
            &sandbox_name,
            &policy_path,
            format!("{e:#}"),
            "yaml-write failure",
        )
        .await;
        let mut msg = format!("{e:#}");
        if !rollback_errors.is_empty() {
            msg.push_str(" (rollback also failed: ");
            msg.push_str(&rollback_errors.join("; "));
            msg.push(')');
        }
        return Err(ProviderApiError::AgentYamlWrite(msg));
    }

    Ok(axum::Json(ProviderView {
        name: req.name,
        type_: "generic".into(),
        label: entry.label.clone(),
        env_var: new_env_var,
        generic: updated.generic,
        updated_at: Some(chrono::Utc::now()),
        status: ProviderStatus::Healthy,
        composed: Some(true),
    }))
}

/// Replace an existing provider entry by name. Line-walking YAML mutator.
fn replace_provider_in_yaml(
    agents_dir: &std::path::Path,
    agent: &str,
    updated: &right_agent_config::ProviderEntry,
) -> miette::Result<()> {
    let path = agents_dir.join(agent).join("agent.yaml");
    let new_entry = serialize_provider_entry(updated);
    let name = updated.name.clone();
    right_codegen::contract::write_merged_rmw(&path, |existing| {
        let original = existing.unwrap_or("");
        replace_provider_entry(original, &name, &new_entry)
    })
}

/// Locate the byte offset of the first `    - name: <name>` line in
/// `original`. Tolerates both legacy unquoted and new single-quoted forms
/// (`    - name: foo` and `    - name: 'foo'`) so that on-disk entries
/// written before the YAML-quoting fix still resolve. `name` itself is
/// always the unquoted, validated provider name from `ProviderEntry::name`.
fn find_provider_name_marker(original: &str, name: &str) -> Option<usize> {
    // Quoted form: `    - name: 'name'` is bounded by the closing quote, so a
    // plain substring search cannot collide with a longer name. New writes are
    // quoted, so the common path stays a single substring search.
    let quoted = format!("    - name: '{name}'");
    if let Some(idx) = original.find(&quoted) {
        return Some(idx);
    }
    // Unquoted (legacy) form needs an explicit trailing delimiter (\n, \r, or
    // EOF). Without it, searching for `myagent-foo` would match a line for
    // `myagent-foo-bar` and mutate the wrong row.
    let plain = format!("    - name: {name}");
    for (idx, _) in original.match_indices(&plain) {
        match original.as_bytes().get(idx + plain.len()) {
            None | Some(b'\n') | Some(b'\r') => return Some(idx),
            _ => continue,
        }
    }
    None
}

/// Replace the entry whose `    - name: <name>` matches. Returns Err if not found.
fn replace_provider_entry(original: &str, name: &str, new_entry: &str) -> miette::Result<String> {
    let Some(start_byte) = find_provider_name_marker(original, name) else {
        return Err(miette::miette!("provider '{}' not in agent.yaml", name));
    };
    // Find start of the line containing the marker.
    let line_start = original[..start_byte]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    // Find end: the next `    - ` sibling OR a column-2 key OR EOF.
    let first_line_end = line_start
        + original[line_start..]
            .find('\n')
            .map(|i| i + 1)
            .unwrap_or(original.len() - line_start);
    let mut end_byte = first_line_end;
    let rest = &original[first_line_end..];
    for line in rest.split_inclusive('\n') {
        if line.starts_with("    - ") {
            break;
        }
        if line.starts_with("  ") && !line.starts_with("    ") && !line.trim().is_empty() {
            break;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') && !line.trim().is_empty() {
            break;
        }
        end_byte += line.len();
    }
    let mut out = String::with_capacity(original.len());
    out.push_str(&original[..line_start]);
    out.push_str(new_entry);
    out.push_str(&original[end_byte..]);
    Ok(out)
}

// ── Task 21: /provider-remove ─────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ProviderRemoveReq {
    pub agent: String,
    pub name: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ProviderRemoveResp {
    pub removed: bool,
}

pub(crate) async fn handle_provider_remove(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderRemoveReq>,
) -> Result<axum::Json<ProviderRemoveResp>, ProviderApiError> {
    validate_name(&req.agent, &req.name)?;
    let _guard = provider_lock(&state, &req.agent).await;

    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    let sandbox = cfg
        .sandbox
        .as_ref()
        .ok_or(ProviderApiError::SandboxModeNone)?;
    let entry = sandbox
        .providers
        .iter()
        .find(|p| p.name == req.name)
        .ok_or_else(|| ProviderApiError::NotFound {
            name: req.name.clone(),
        })?
        .clone();
    let mut client = open_openshell_client().await?;
    let sandbox_name = sandbox.name.clone().unwrap_or_else(|| req.agent.clone());
    let policy_path = state.agents_dir.join(&req.agent).join(
        sandbox
            .policy_file
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("policy.yaml")),
    );

    match right_openshell::providers::detach_from_sandbox(&mut client, &sandbox_name, &req.name)
        .await
    {
        Ok(()) => {}
        Err(right_openshell::providers::ProviderError::NotFound(_)) => {
            tracing::info!(provider = %req.name, "detach: provider already not attached");
        }
        Err(e) => return Err(ProviderApiError::Gateway(format!("{e:#}"))),
    }
    match right_openshell::providers::delete_provider(&mut client, &req.name).await {
        Ok(()) => {}
        Err(right_openshell::providers::ProviderError::NotFound(_)) => {
            tracing::info!(provider = %req.name, "delete: provider already absent");
        }
        Err(e) => return Err(ProviderApiError::Gateway(format!("{e:#}"))),
    }

    // Gateway state is gone. The agent.yaml entry would now be ghost
    // data — remove it first, then reload provider-profile composition.
    // Generic providers additionally run legacy folded-policy cleanup; new
    // composition-based policies have no stanza, so that strip is a no-op.
    remove_provider_from_yaml(&state.agents_dir, &req.agent, &req.name)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;

    let mut composition_reloaded = false;
    if let Some(g) = &entry.generic {
        // `providers_strip` removes the whole `_provider_<name>` stanza by
        // provider name (its host arg is advisory), so one strip closes every
        // host this provider opened. Only strip when at least one of the removed
        // provider's hosts is not still required by another provider's own
        // stanza; shared hosts stay reachable through those other stanzas.
        let any_host_exclusive = g.upstream_hosts.iter().any(|host| {
            !sandbox.providers.iter().any(|p| {
                p.name != req.name
                    && p.generic
                        .as_ref()
                        .is_some_and(|gp| gp.upstream_hosts.iter().any(|other| other == host))
            })
        });
        if any_host_exclusive {
            let prior = std::fs::read_to_string(&policy_path)
                .map_err(|e| ProviderApiError::AgentYamlWrite(format!("read policy: {e:#}")))?;
            let stripped = right_codegen::policy::providers_strip(&prior, &req.name, "");
            right_codegen::contract::write_and_apply_sandbox_policy(
                &sandbox_name,
                &policy_path,
                &stripped,
            )
            .await
            .map_err(|e| ProviderApiError::Gateway(format!("policy apply: {e:#}")))?;
            composition_reloaded = true;
        }
    }
    if !composition_reloaded {
        right_openshell::openshell::ensure_provider_policy_loaded(&sandbox_name, &policy_path)
            .await
            .map_err(|e| ProviderApiError::Gateway(format!("policy apply: {e:#}")))?;
    }

    Ok(axum::Json(ProviderRemoveResp { removed: true }))
}

fn remove_provider_from_yaml(
    agents_dir: &std::path::Path,
    agent: &str,
    name: &str,
) -> miette::Result<()> {
    let path = agents_dir.join(agent).join("agent.yaml");
    let name = name.to_string();
    right_codegen::contract::write_merged_rmw(&path, |existing| {
        let original = existing.unwrap_or("");
        Ok(remove_provider_entry(original, &name))
    })
}

/// Remove the entry whose `    - name: <name>` matches. No-op if not present.
fn remove_provider_entry(original: &str, name: &str) -> String {
    let Some(start_byte) = find_provider_name_marker(original, name) else {
        return original.to_string();
    };
    let line_start = original[..start_byte]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let first_line_end = line_start
        + original[line_start..]
            .find('\n')
            .map(|i| i + 1)
            .unwrap_or(original.len() - line_start);
    let mut end_byte = first_line_end;
    let rest = &original[first_line_end..];
    for line in rest.split_inclusive('\n') {
        if line.starts_with("    - ") {
            break;
        }
        if line.starts_with("  ") && !line.starts_with("    ") && !line.trim().is_empty() {
            break;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') && !line.trim().is_empty() {
            break;
        }
        end_byte += line.len();
    }
    let mut out = String::with_capacity(original.len() - (end_byte - line_start));
    out.push_str(&original[..line_start]);
    out.push_str(&original[end_byte..]);
    out
}

// ── Task 26: sandbox_mode_none rejection test ─────────────────────────────────

#[cfg(test)]
mod sandbox_mode_tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use tower::ServiceExt;

    /// Build a minimal internal router pointed at `agents_dir`, enough to
    /// exercise /provider-list. Mirrors `make_test_router` from internal_api.rs.
    async fn make_provider_test_router(tmp: &std::path::Path) -> axum::Router {
        use crate::aggregator::{AgentInfo, BackendRegistry};
        use crate::right_backend::RightBackend;
        use dashmap::DashMap;
        use std::collections::HashMap;
        use std::sync::Arc;

        let agents_dir = tmp.join("agents");
        // Ensure the dispatcher has a known agent so token auth passes,
        // but the test agent ("hostagent") only needs the agent.yaml on disk.
        let placeholder_dir = agents_dir.join("hostagent");
        std::fs::create_dir_all(&placeholder_dir).unwrap();
        right_db::open_db(&placeholder_dir, true).await.unwrap();

        let right = RightBackend::new(agents_dir.clone(), None);
        let registry = BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            agent_dir: placeholder_dir.clone(),
            hindsight: None,
        };
        let agents = DashMap::new();
        agents.insert("hostagent".into(), registry);
        let dispatcher = Arc::new(crate::aggregator::ToolDispatcher { agents });

        let refresh_senders: crate::aggregator::RefreshSenders =
            Arc::new(std::collections::HashMap::new());
        let reconnect_managers: crate::aggregator::ReconnectManagers =
            Arc::new(std::collections::HashMap::new());

        let token_map_path = tmp.join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"hostagent": "tok-test"}).to_string(),
        )
        .unwrap();
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "tok-test".into(),
                AgentInfo {
                    name: "hostagent".into(),
                    dir: placeholder_dir,
                },
            );
            Arc::new(tokio::sync::RwLock::new(map))
        };

        crate::internal_api::internal_router(
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            agents_dir,
        )
    }

    #[tokio::test]
    async fn provider_create_generic_rejected_in_restrictive_mode() {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // Generic provider profile composition is only supported under the
        // permissive network policy, so creation must be refused up-front.
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "network_policy: restrictive\n\
             sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;

        let req = Request::builder()
            .method("POST")
            .uri("/provider-create")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "type": "generic",
                    "label": "acme",
                    "credential": "secret-value",
                    "generic": {
                        "env_var": "ACME_API_KEY",
                        "upstream_host": "api.acme.invalid",
                    },
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "network_policy_forbids_generic",
            "expected code=network_policy_forbids_generic, got: {json}"
        );
    }

    #[tokio::test]
    async fn provider_rotate_fails_fast_on_unknown_builtin() {
        // If an agent.yaml has a BuiltIn(slug) that profile_catalog() no
        // longer knows about (e.g. catalog renamed/dropped), rotating that
        // provider MUST fail with HTTP 500 + code=unknown_builtin_slug
        // BEFORE any gateway call. Silently inserting "" as the credential
        // key would let rotation "succeed" but break placeholder
        // substitution downstream (AGENTS.rust.md §2 FAIL-FAST).
        let _ = rustls::crypto::ring::default_provider().install_default();

        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n  providers:\n    \
             - name: 'hostagent-stale'\n      type: 'definitely-not-a-real-slug'\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;

        let req = Request::builder()
            .method("POST")
            .uri("/provider-rotate")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "name": "hostagent-stale",
                    "credential": "new-secret",
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "unknown built-in slug must surface as 500, not silent success"
        );

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "unknown_builtin_slug",
            "expected code=unknown_builtin_slug, got: {json}"
        );
    }

    #[tokio::test]
    async fn provider_list_marks_unknown_builtin_as_status() {
        // List view must NOT abort when a single entry references an unknown
        // slug. The bad row is marked status.kind=unknown_builtin so the
        // operator sees it; rotation/config-update on that same row still
        // fails fast (see provider_rotate_fails_fast_on_unknown_builtin).
        let _ = rustls::crypto::ring::default_provider().install_default();

        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n  providers:\n    \
             - name: 'hostagent-stale'\n      type: 'definitely-not-a-real-slug'\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;

        let req = Request::builder()
            .method("POST")
            .uri("/provider-list")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({"agent": "hostagent"})).unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // List talks to the gateway via get_provider; if the gateway isn't
        // available the call returns 502 (BAD_GATEWAY) before we reach the
        // env_var resolution. In that environment we still get a useful
        // signal: either 200 with status.kind=unknown_builtin, or 502 (no
        // gateway reachable). Accept both; the env_var resolution is what
        // the unit test covers exhaustively.
        let status = resp.status();
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        if status == StatusCode::OK {
            let arr: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
            let entries = arr.as_array().expect("list returns an array");
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0]["name"], "hostagent-stale");
            assert_eq!(
                entries[0]["status"]["kind"], "unknown_builtin",
                "stale row must be marked, not silently emptied: {entries:?}"
            );
            assert_eq!(entries[0]["env_var"], "");
        } else {
            assert_eq!(
                status,
                StatusCode::BAD_GATEWAY,
                "without a gateway, expected 502; got {status} body={body_bytes:?}"
            );
        }
    }

    #[tokio::test]
    async fn provider_create_succeeds_when_existing_entry_has_unknown_builtin_slug() {
        // If agent.yaml carries a stale `BuiltIn(slug)` row whose slug is no
        // longer in `profile_catalog()`, the env-var collision loop in
        // /provider-create used to call `extract_env_var(existing)?` and
        // propagate `UnknownBuiltinSlug` — locking the operator out of adding
        // ANY new provider until they manually pruned the bad row. The bad
        // row is already surfaced as `status: unknown_builtin` by
        // /provider-list; the create path must mirror that tolerance and skip
        // the bad row in collision checks instead of aborting.
        //
        // Without the fix: HTTP 500 with `code=unknown_builtin_slug`.
        // With the fix:    no early 500 — the request passes the collision
        // check and either reaches the gateway (no live gateway in tests →
        // 502 BAD_GATEWAY) or some later validation. Asserting "not the bad
        // 500" is the load-bearing signal.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n  providers:\n    \
             - name: 'hostagent-stale'\n      type: 'definitely-not-a-real-slug'\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;

        // New provider: a DIFFERENT name/type (built-in `anthropic`,
        // env_var `ANTHROPIC_API_KEY`) that does not collide with anything
        // the stale row would have resolved to.
        let req = Request::builder()
            .method("POST")
            .uri("/provider-create")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "type": "anthropic",
                    "credential": "sk-ant-test",
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();

        assert_ne!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "stale unknown_builtin row must not block creating a different provider; \
             got 500 body={json}"
        );
        assert_ne!(
            json["code"], "unknown_builtin_slug",
            "collision loop must skip unknown_builtin entries, not propagate them; \
             body={json}"
        );
        // Without a live OpenShell gateway the post-collision path lands on
        // a gateway-connect failure (502). That's the expected "we got past
        // the collision check" outcome in this test environment.
        assert_eq!(
            status,
            StatusCode::BAD_GATEWAY,
            "expected 502 once collision check is tolerant; got {status} body={json}"
        );
    }

    fn assert_markers_in_order(src: &str, markers: &[&str]) {
        let mut offset = 0;
        for marker in markers {
            let rel = src[offset..]
                .find(*marker)
                .unwrap_or_else(|| panic!("marker {marker:?} missing or out of order"));
            offset += rel + (*marker).len();
        }
    }

    #[test]
    fn built_in_create_confirms_composition_before_yaml_write() {
        // The built-in dashboard create path opens the real gateway client and
        // shell policy loader directly. Pin the ordering so a built-in add
        // cannot update agent.yaml until the gateway's active policy includes
        // the composed provider rule.
        let src = include_str!("internal_api_providers.rs");
        let start = src
            .find("// Built-in flow:")
            .expect("built-in provider create flow comment must exist");
        let end = src[start..]
            .find("Ok(axum::Json(ProviderView {")
            .expect("built-in provider create response must exist")
            + start;
        assert_markers_in_order(
            &src[start..end],
            &[
                "right_openshell::providers::create_provider(&mut client, &spec)",
                "right_openshell::providers::attach_to_sandbox(&mut client, &sandbox_name, &name)",
                "right_openshell::openshell::ensure_provider_policy_loaded(&sandbox_name, &policy_path)",
                "right_openshell::openshell::wait_for_provider_composed(",
                "&name,",
                "append_provider_to_yaml(&state.agents_dir, &req.agent, &entry)",
            ],
        );
    }

    #[test]
    fn generic_create_confirms_endpoint_composition_before_yaml_write() {
        // Generic create must not just see a provider rule; it must see the
        // endpoint content from the authored generic profile before agent.yaml
        // becomes the source of truth.
        let src = include_str!("internal_api_providers.rs");
        let start = src
            .find("async fn create_generic_provider")
            .expect("generic provider create handler must exist");
        let end = src[start..]
            .find("Ok(axum::Json(ProviderView {")
            .expect("generic provider create response must exist")
            + start;
        assert_markers_in_order(
            &src[start..end],
            &[
                "right_openshell::providers::create_provider(&mut client, &spec)",
                "right_openshell::providers::attach_to_sandbox(&mut client, &sandbox_name, &name)",
                "right_openshell::openshell::ensure_provider_policy_loaded(&sandbox_name, &policy_path)",
                "right_openshell::openshell::wait_for_provider_composed_with_exact_endpoints(",
                "expected_endpoints,",
                "append_provider_to_yaml(&state.agents_dir, &req.agent, &entry)",
            ],
        );
    }

    #[test]
    fn config_update_confirms_composition_before_yaml_write() {
        // Like create, config-update talks to the real gateway and shell policy
        // loader directly. Pin the ordering so a generic profile/config change
        // cannot update agent.yaml until the gateway's active policy includes
        // the recomposed provider rule.
        let src = include_str!("internal_api_providers.rs");
        let start = src
            .find("pub(crate) async fn handle_provider_config_update")
            .expect("provider config update handler must exist");
        let end = src[start..]
            .find("Ok(axum::Json(ProviderView {")
            .expect("provider config update response must exist")
            + start;
        assert_markers_in_order(
            &src[start..end],
            &[
                "right_openshell::providers::update_referenced_profile(&mut client, &attachments, profile)",
                "right_openshell::openshell::ensure_provider_policy_loaded(&sandbox_name, &policy_path)",
                "right_openshell::openshell::wait_for_provider_composed_with_exact_endpoints(",
                "expected_endpoints,",
                "replace_provider_in_yaml(&state.agents_dir, &req.agent, &updated)",
            ],
        );
    }

    #[test]
    fn config_update_does_not_update_provider_without_fresh_credential() {
        let src = include_str!("internal_api_providers.rs");
        let start = src
            .find("pub(crate) async fn handle_provider_config_update")
            .expect("provider config update handler must exist");
        let end = src[start..]
            .find("Ok(axum::Json(ProviderView {")
            .expect("provider config update response must exist")
            + start;
        assert!(
            !src[start..end].contains("right_openshell::providers::update_provider"),
            "generic config update is profile-only; provider record mutation is unnecessary"
        );
    }

    #[tokio::test]
    async fn provider_config_update_rejects_invalid_name() {
        // /provider-config-update must validate `name` before acquiring the
        // per-(agent,name) lock or touching agent.yaml. Without the up-front
        // `validate_name(...)?`, the lock map would accumulate entries for
        // arbitrary user-supplied tuples (unbounded growth from user input)
        // and any future code path using `req.name` between lock acquisition
        // and the eventual NotFound would inherit the unvalidated value.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;

        // "../bad" lacks the required "{agent}-" prefix and contains
        // disallowed characters — validate_name must reject it before any
        // gateway / filesystem work.
        let req = Request::builder()
            .method("POST")
            .uri("/provider-config-update")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "name": "../bad",
                    "generic": {
                        "upstream_host": "api.acme.invalid",
                    },
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "invalid_name",
            "expected code=invalid_name, got: {json}"
        );
    }

    /// Build a minimal `InternalState` rooted at `tmp/agents` so unit tests
    /// can exercise `provider_lock` and the agent.yaml RMW writer directly
    /// without going through the axum router (which would also require a
    /// live OpenShell gateway for the create path).
    async fn make_provider_test_state(tmp: &std::path::Path) -> crate::internal_api::InternalState {
        use crate::aggregator::{AgentInfo, BackendRegistry};
        use crate::right_backend::RightBackend;
        use dashmap::DashMap;
        use std::collections::HashMap;
        use std::sync::Arc;

        let agents_dir = tmp.join("agents");
        let placeholder_dir = agents_dir.join("hostagent");
        std::fs::create_dir_all(&placeholder_dir).unwrap();
        right_db::open_db(&placeholder_dir, true).await.unwrap();

        let right = RightBackend::new(agents_dir.clone(), None);
        let registry = BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            agent_dir: placeholder_dir.clone(),
            hindsight: None,
        };
        let agents = DashMap::new();
        agents.insert("hostagent".into(), registry);
        let dispatcher = Arc::new(crate::aggregator::ToolDispatcher { agents });

        let refresh_senders: crate::aggregator::RefreshSenders =
            Arc::new(std::collections::HashMap::new());
        let reconnect_managers: crate::aggregator::ReconnectManagers =
            Arc::new(std::collections::HashMap::new());

        let token_map_path = tmp.join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"hostagent": "tok-test"}).to_string(),
        )
        .unwrap();
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "tok-test".into(),
                AgentInfo {
                    name: "hostagent".into(),
                    dir: placeholder_dir,
                },
            );
            Arc::new(tokio::sync::RwLock::new(map))
        };

        crate::internal_api::InternalState::new_for_test(
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            agents_dir,
        )
    }

    /// Many concurrent `provider-create` calls for DISTINCT providers on
    /// the SAME agent must all end up in agent.yaml. Before the fix
    /// `provider_lock` keyed on `(agent, name)` — so different names took
    /// different locks, multiple writers performed RMW on the same
    /// agent.yaml in parallel, and last-write-wins dropped entries silently
    /// (gateway/policy already mutated, agent.yaml left as an orphan).
    ///
    /// To make the race deterministic regardless of platform scheduling,
    /// we inline the read+sleep+write that `write_merged_rmw` performs
    /// internally, with a 50ms sleep between read and write. With the
    /// buggy per-name lock, the N writers all read the same starting
    /// content and only the last write survives. With the correct
    /// per-agent lock, the second writer can't even start until the first
    /// has released its guard (and thus completed the write).
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_provider_create_serializes_on_same_agent() {
        use super::{load_agent_config, provider_lock, serialize_provider_entry};

        let _ = rustls::crypto::ring::default_provider().install_default();

        let tmp = tempfile::tempdir().unwrap();
        let state = make_provider_test_state(tmp.path()).await;
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        // N distinct provider entries for the SAME agent.
        const N: usize = 5;
        let entries: Vec<_> = (0..N)
            .map(|i| right_agent_config::ProviderEntry {
                name: format!("hostagent-p{i:02}"),
                type_: right_agent_config::ProviderType::Generic,
                label: Some(format!("p{i:02}")),
                generic: Some(right_agent_config::GenericProvider {
                    env_var: format!("KEY_{i:02}"),
                    upstream_hosts: vec![format!("api{i:02}.example.com")],
                    upstream_path_prefix: None,
                }),
            })
            .collect();

        // Spawn N tasks each performing a guarded RMW with a deliberate
        // sleep between read and write. The sleep widens the race window
        // enough that, under the buggy per-name lock, multiple tasks will
        // observe the same prior content and one or more entries will be
        // overwritten. The correct per-agent lock prevents this.
        let agent_yaml = state.agents_dir.join("hostagent").join("agent.yaml");
        let mut tasks = Vec::with_capacity(N);
        for entry in entries.iter().cloned() {
            let state = state.clone();
            let agent_yaml = agent_yaml.clone();
            tasks.push(tokio::spawn(async move {
                let _guard = provider_lock(&state, "hostagent").await;
                let existing = tokio::fs::read_to_string(&agent_yaml).await.unwrap();
                // Hold open the RMW window. Under a per-name lock every
                // task reaches this sleep concurrently; under the
                // per-agent lock the next task is still blocked on
                // provider_lock.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let updated =
                    super::insert_provider_entry(&existing, &serialize_provider_entry(&entry))
                        .unwrap();
                tokio::fs::write(&agent_yaml, updated).await.unwrap();
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }

        let cfg = load_agent_config(&state.agents_dir, "hostagent")
            .expect("agent.yaml must parse after concurrent appends");
        let names: std::collections::BTreeSet<_> = cfg
            .sandbox
            .as_ref()
            .expect("sandbox section present")
            .providers
            .iter()
            .map(|p| p.name.clone())
            .collect();
        for entry in &entries {
            assert!(
                names.contains(&entry.name),
                "{} dropped from agent.yaml (RMW race): {names:?}",
                entry.name
            );
        }
        assert_eq!(
            names.len(),
            N,
            "expected exactly {N} providers after concurrent create, got: {names:?}"
        );
    }

    #[tokio::test]
    async fn provider_list_rejects_sandbox_mode_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // Write agent.yaml with mode: none — sandbox-mode guard should fire.
        std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();

        let app = make_provider_test_router(tmp.path()).await;

        let req = Request::builder()
            .method("POST")
            .uri("/provider-list")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({"agent": "hostagent"})).unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "sandbox_mode_none",
            "expected code=sandbox_mode_none, got: {json}"
        );
    }
}

// ── Task 16: /provider-types ──────────────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
pub struct ProviderProfileView {
    #[serde(rename = "type")]
    pub type_slug: String,
    pub env_var: String,
    pub display_name: String,
    pub category: String,
}

pub(crate) async fn handle_provider_types() -> axum::Json<Vec<ProviderProfileView>> {
    // A built-in profile that a right-* managed profile supersedes stays in the
    // catalog (so existing providers still resolve their env var) but is not
    // offered as a new provider type. The supersession relationship lives in
    // exactly one place — `ManagedProfile::base_id()` — so deriving the hidden
    // set from it keeps the offered list correct automatically when a managed
    // profile is added, with no parallel denylist to forget to update.
    let hidden: Vec<&'static str> = right_openshell::managed_profiles::managed_profiles()
        .iter()
        .filter_map(|p| p.base_id())
        .collect();
    let catalog = right_openshell::providers::profile_catalog();
    let views: Vec<_> = catalog
        .into_iter()
        .filter(|p| !hidden.contains(&p.type_slug.as_str()))
        .map(|p| ProviderProfileView {
            type_slug: p.type_slug,
            env_var: p.env_var,
            display_name: p.display_name,
            category: format!("{:?}", p.category).to_lowercase(),
        })
        .collect();
    axum::Json(views)
}

#[cfg(test)]
mod provider_types_tests {
    use super::*;

    #[tokio::test]
    async fn provider_types_hides_builtin_github_and_shows_right_github() {
        let axum::Json(types) = handle_provider_types().await;
        assert!(
            types.iter().all(|t| t.type_slug != "github"),
            "built-in read-only github is hidden from the dashboard"
        );
        assert!(
            types
                .iter()
                .any(|t| t.type_slug == "right-github" && t.display_name == "GitHub"),
            "right-github offered as GitHub"
        );
        assert!(
            types.iter().any(|t| t.type_slug == "gitlab"),
            "filter is narrow — other built-ins (gitlab) still offered"
        );
    }

    #[tokio::test]
    async fn every_managed_profile_base_is_hidden() {
        // Invariant: for every managed profile that supersedes a base, the base
        // slug must be absent from the offered list. Enforces the derivation so a
        // future right-* profile can't accidentally show its base alongside it.
        let axum::Json(types) = handle_provider_types().await;
        for mp in right_openshell::managed_profiles::managed_profiles() {
            if let Some(base) = mp.base_id() {
                assert!(
                    types.iter().all(|t| t.type_slug != base),
                    "managed profile {} supersedes base {} — base must be hidden",
                    mp.id(),
                    base
                );
            }
        }
    }
}

#[cfg(test)]
mod copy_error_status_tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[test]
    fn new_error_variants_map_to_expected_status() {
        assert_eq!(
            ProviderApiError::Unauthorized { agent: "a".into() }
                .into_response()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            ProviderApiError::CopyConflict { reason: "x".into() }
                .into_response()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            ProviderApiError::Internal("boom".into())
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
