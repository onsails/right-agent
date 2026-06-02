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
    #[error("providers are only available for sandboxed agents (sandbox.mode = openshell)")]
    SandboxModeNone,
    #[error(
        "generic providers require network_policy: permissive — restrictive mode forbids non-Anthropic upstream hosts"
    )]
    NetworkPolicyForbidsGeneric,
    #[error("policy conflict on host \"{host}\": {kind}")]
    PolicyConflict { host: String, kind: String },
    #[error("openshell gateway: {0}")]
    Gateway(String),
    #[error("agent.yaml write failed after gateway change: {0}")]
    AgentYamlWrite(String),
    #[error(
        "provider \"{name}\" references unknown built-in slug \"{slug}\" — the profile catalog no longer recognizes it; config migration required"
    )]
    UnknownBuiltinSlug { name: String, slug: String },
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
            Self::SandboxModeNone => (StatusCode::BAD_REQUEST, "sandbox_mode_none"),
            Self::NetworkPolicyForbidsGeneric => {
                (StatusCode::BAD_REQUEST, "network_policy_forbids_generic")
            }
            Self::PolicyConflict { .. } => (StatusCode::CONFLICT, "policy_conflict"),
            Self::Gateway(_) => (StatusCode::BAD_GATEWAY, "gateway"),
            Self::AgentYamlWrite(_) => (StatusCode::INTERNAL_SERVER_ERROR, "agent_yaml_write"),
            Self::UnknownBuiltinSlug { .. } => {
                (StatusCode::INTERNAL_SERVER_ERROR, "unknown_builtin_slug")
            }
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
async fn open_openshell_client() -> Result<
    right_openshell::openshell_proto::openshell::v1::open_shell_client::OpenShellClient<
        tonic::transport::Channel,
    >,
    ProviderApiError,
> {
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("connect: {e:#}")))
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
/// allowed alphabet is intentionally narrow — hostnames, HTTP header
/// names, URL path prefixes, and human labels all fit.
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

pub fn validate_header_name(name: &str) -> Result<(), ProviderApiError> {
    validate_yaml_scalar(name, "header_name", 64, "-_")
}

pub fn validate_path_prefix(path: &str) -> Result<(), ProviderApiError> {
    validate_yaml_scalar(path, "upstream_path_prefix", 128, "-_/.~")
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
                header_name: "Authorization".to_string(),
                upstream_host: "api.example.com".to_string(),
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
                header_name: "Authorization".to_string(),
                upstream_host: "api.acme.com".to_string(),
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
        assert_eq!(g.upstream_host, "api.acme.com");
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
                header_name: "Authorization".to_string(),
                upstream_host: "api.example.com".to_string(),
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
    let mut views = Vec::with_capacity(sandbox.providers.len());
    for entry in &sandbox.providers {
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
        let type_str = match &entry.type_ {
            right_agent_config::ProviderType::Generic => "generic".to_string(),
            right_agent_config::ProviderType::BuiltIn(s) => s.clone(),
        };
        views.push(ProviderView {
            name: entry.name.clone(),
            type_: type_str,
            label: entry.label.clone(),
            env_var,
            generic: entry.generic.clone(),
            updated_at,
            status,
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
    pub header_name: Option<String>,
    pub upstream_host: String,
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
    if let Some(g) = &req.generic {
        validate_upstream_host(&g.upstream_host)?;
        if let Some(h) = &g.header_name {
            validate_header_name(h)?;
        }
        if let Some(p) = &g.upstream_path_prefix {
            validate_path_prefix(p)?;
        }
    }
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
        // Generic providers append a `- host: <upstream_host>` stanza to
        // `network_policies.outbound.endpoints` for HTTPS interception +
        // placeholder substitution. Restrictive mode renders only
        // `network_policies.anthropic.endpoints` (Anthropic/Claude allowlist)
        // and intentionally has no outbound section to extend — so generic
        // providers are incompatible with restrictive mode.
        if matches!(
            cfg.network_policy,
            right_agent_config::NetworkPolicy::Restrictive
        ) {
            return Err(ProviderApiError::NetworkPolicyForbidsGeneric);
        }
        return create_generic_provider(state, req, name, env_var).await;
    }

    // Built-in flow: OpenShell manages endpoints; no policy mutation needed.
    let mut client = open_openshell_client().await?;
    let sandbox_name = sandbox.name.clone().unwrap_or_else(|| req.agent.clone());

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

    let entry = right_agent_config::ProviderEntry {
        name: name.clone(),
        type_: right_agent_config::ProviderType::BuiltIn(req.type_.clone()),
        label: req.label.clone(),
        generic: None,
    };
    if let Err(e) = append_provider_to_yaml(&state.agents_dir, &req.agent, &entry) {
        if let Err(rollback_err) =
            right_openshell::providers::detach_from_sandbox(&mut client, &sandbox_name, &name).await
        {
            tracing::warn!(
                provider = %name,
                original_err = %e,
                "provider rollback failed: could not detach provider after yaml-write failure: {rollback_err:#}"
            );
        }
        if let Err(rollback_err) =
            right_openshell::providers::delete_provider(&mut client, &name).await
        {
            tracing::warn!(
                provider = %name,
                original_err = %e,
                "provider rollback failed: could not delete provider after yaml-write failure: {rollback_err:#}"
            );
        }
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
    }))
}

/// Returns `Ok(true)` if renaming `excluding_name`'s env_var to `new_env_var`
/// would collide with another provider in `providers`.
///
/// Propagates `ProviderApiError::UnknownBuiltinSlug` if any built-in entry
/// in `providers` references a slug that is no longer in `profile_catalog()`
/// — silently treating it as "no env var" would let the rename succeed while
/// the stale entry continues to produce broken placeholder substitution.
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
        out.push_str(&format!(
            "        header_name: {}\n",
            yaml_single_quote(&g.header_name)
        ));
        out.push_str(&format!(
            "        upstream_host: {}\n",
            yaml_single_quote(&g.upstream_host)
        ));
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
        if let Some(c) = ch {
            if c != ' ' && c != '\t' && c != '\n' && c != '\r' && c != '#' {
                sandbox_end = i;
                break;
            }
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

async fn create_generic_provider(
    state: crate::internal_api::InternalState,
    req: ProviderCreateReq,
    name: String,
    env_var: String,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    use secrecy::ExposeSecret;
    // Lock is already held by the caller (handle_provider_create acquires it
    // before dispatching here), so no second acquisition needed.
    let g = req
        .generic
        .clone()
        .ok_or_else(|| ProviderApiError::InvalidEnvVar { env_var: "".into() })?;
    let header_name = g
        .header_name
        .clone()
        .unwrap_or_else(|| "Authorization".into());

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

    let prior = std::fs::read_to_string(&policy_path)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("read policy: {e:#}")))?;
    let new_policy = right_codegen::policy::providers_append_checked(
        &prior,
        &name,
        &g.upstream_host,
        g.upstream_path_prefix.as_deref(),
    )
    .map_err(|e| match e {
        right_codegen::policy::PolicyConflict::RawTunnel { host } => {
            ProviderApiError::PolicyConflict {
                host,
                kind: "raw-tunnel".into(),
            }
        }
    })?;
    let snapshot =
        right_codegen::contract::write_apply_with_snapshot(&sandbox_name, &policy_path, new_policy)
            .await
            .map_err(|e| ProviderApiError::Gateway(format!("policy apply: {e:#}")))?;

    let mut client = open_openshell_client().await?;
    let mut creds = std::collections::HashMap::new();
    creds.insert(env_var.clone(), req.credential.expose_secret().to_string());
    let mut config = std::collections::HashMap::new();
    config.insert("header_name".into(), header_name.clone());
    if let Some(prefix) = &g.upstream_path_prefix {
        config.insert("upstream_path_prefix".into(), prefix.clone());
    }
    config.insert("upstream_host".into(), g.upstream_host.clone());
    let spec = right_openshell::providers::ProviderSpec {
        name: name.clone(),
        type_: "generic".into(),
        credentials: creds,
        config,
    };
    if let Err(e) = right_openshell::providers::create_provider(&mut client, &spec).await {
        if let Err(rollback_err) = snapshot.restore().await {
            tracing::warn!(
                provider = %name,
                original_err = %e,
                "provider rollback failed: could not restore policy snapshot after create_provider failure: {rollback_err:#}"
            );
        }
        return Err(ProviderApiError::Gateway(format!("{e:#}")));
    }

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
        if let Err(rollback_err) = snapshot.restore().await {
            tracing::warn!(
                provider = %name,
                original_err = %attach_err,
                "provider rollback failed: could not restore policy snapshot after attach failure: {rollback_err:#}"
            );
        }
        return Err(ProviderApiError::Gateway(format!("{attach_err:#}")));
    }

    let generic_entry = right_agent_config::GenericProvider {
        env_var: env_var.clone(),
        header_name: header_name.clone(),
        upstream_host: g.upstream_host.clone(),
        upstream_path_prefix: g.upstream_path_prefix.clone(),
    };
    let entry = right_agent_config::ProviderEntry {
        name: name.clone(),
        type_: right_agent_config::ProviderType::Generic,
        label: req.label.clone(),
        generic: Some(generic_entry.clone()),
    };
    if let Err(e) = append_provider_to_yaml(&state.agents_dir, &req.agent, &entry) {
        if let Err(rollback_err) =
            right_openshell::providers::detach_from_sandbox(&mut client, &sandbox_name, &name).await
        {
            tracing::warn!(
                provider = %name,
                original_err = %e,
                "provider rollback failed: could not detach provider after yaml-write failure: {rollback_err:#}"
            );
        }
        if let Err(rollback_err) =
            right_openshell::providers::delete_provider(&mut client, &name).await
        {
            tracing::warn!(
                provider = %name,
                original_err = %e,
                "provider rollback failed: could not delete provider after yaml-write failure: {rollback_err:#}"
            );
        }
        if let Err(rollback_err) = snapshot.restore().await {
            tracing::warn!(
                provider = %name,
                original_err = %e,
                "provider rollback failed: could not restore policy snapshot after yaml-write failure: {rollback_err:#}"
            );
        }
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
    }))
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
/// providers from agent.yaml while leaving the gateway and policy.yaml
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
    let type_str = match &entry.type_ {
        right_agent_config::ProviderType::Generic => "generic".to_string(),
        right_agent_config::ProviderType::BuiltIn(s) => s.clone(),
    };
    let spec = right_openshell::providers::ProviderSpec {
        name: req.name.clone(),
        type_: type_str.clone(),
        credentials: creds,
        config: Default::default(),
    };
    right_openshell::providers::update_provider(&mut client, &spec)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;

    Ok(axum::Json(ProviderView {
        name: req.name,
        type_: type_str,
        label: entry.label.clone(),
        env_var,
        generic: entry.generic.clone(),
        updated_at: Some(chrono::Utc::now()),
        status: ProviderStatus::Healthy,
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
    pub header_name: Option<String>,
    pub upstream_host: Option<String>,
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
    // Same rationale as `handle_provider_create`: generic providers cannot
    // be edited under restrictive policy because the outbound endpoints
    // section needed for placeholder substitution is not rendered.
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
    let new_header = req
        .generic
        .header_name
        .clone()
        .unwrap_or(current.header_name.clone());
    let new_host = req
        .generic
        .upstream_host
        .clone()
        .unwrap_or(current.upstream_host.clone());
    let new_path = match req.generic.upstream_path_prefix.clone() {
        None => current.upstream_path_prefix.clone(),
        Some(v) => v,
    };
    validate_env_var(&new_env_var)?;
    if new_env_var != current.env_var && would_collide(&sandbox.providers, &req.name, &new_env_var)?
    {
        return Err(ProviderApiError::EnvVarCollision {
            env_var: new_env_var,
        });
    }
    validate_header_name(&new_header)?;
    validate_upstream_host(&new_host)?;
    if let Some(p) = &new_path {
        validate_path_prefix(p)?;
    }

    let mut client = open_openshell_client().await?;
    let sandbox_name = sandbox.name.clone().unwrap_or_else(|| req.agent.clone());
    let policy_path = state.agents_dir.join(&req.agent).join(
        sandbox
            .policy_file
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("policy.yaml")),
    );

    // Capture the CURRENT gateway state (config map only — credentials are
    // intentionally not returned by GetProvider and never persisted on host)
    // BEFORE any mutation, so we can roll the gateway back if a later step
    // (policy apply, update_provider, or replace_provider_in_yaml) fails.
    //
    // Without this snapshot, a yaml-write failure after a successful
    // `update_provider` leaves the gateway holding new header_name /
    // upstream_path_prefix while agent.yaml still holds the old values —
    // silent drift the operator only notices when placeholder substitution
    // breaks (see review note Iteration 2 #91).
    let gateway_snapshot = right_openshell::providers::get_provider(&mut client, &req.name)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("get_provider snapshot: {e:#}")))?;

    let mut snapshot: Option<right_codegen::contract::PolicySnapshot> = None;
    if new_host != current.upstream_host {
        let prior = std::fs::read_to_string(&policy_path)
            .map_err(|e| ProviderApiError::AgentYamlWrite(format!("read policy: {e:#}")))?;
        let stripped =
            right_codegen::policy::providers_strip(&prior, &req.name, &current.upstream_host);
        let new_policy = right_codegen::policy::providers_append_checked(
            &stripped,
            &req.name,
            &new_host,
            new_path.as_deref(),
        )
        .map_err(|e| match e {
            right_codegen::policy::PolicyConflict::RawTunnel { host } => {
                ProviderApiError::PolicyConflict {
                    host,
                    kind: "raw-tunnel".into(),
                }
            }
        })?;
        snapshot = Some(
            right_codegen::contract::write_apply_with_snapshot(
                &sandbox_name,
                &policy_path,
                new_policy,
            )
            .await
            .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?,
        );
    }

    let mut config = std::collections::HashMap::new();
    config.insert("header_name".into(), new_header.clone());
    config.insert("upstream_host".into(), new_host.clone());
    if let Some(p) = &new_path {
        config.insert("upstream_path_prefix".into(), p.clone());
    }
    let spec = right_openshell::providers::ProviderSpec {
        name: req.name.clone(),
        type_: "generic".into(),
        credentials: Default::default(),
        config,
    };
    if let Err(e) = right_openshell::providers::update_provider(&mut client, &spec).await {
        // The gateway never accepted the new config, so nothing to roll back
        // there. Only the policy snapshot (if taken) needs restoring.
        if let Some(s) = snapshot {
            if let Err(rollback_err) = s.restore().await {
                tracing::warn!(
                    provider = %req.name,
                    original_err = %e,
                    "provider rollback failed: could not restore policy snapshot after update_provider failure: {rollback_err:#}"
                );
            }
        }
        return Err(ProviderApiError::Gateway(format!("{e:#}")));
    }

    let updated = right_agent_config::ProviderEntry {
        name: req.name.clone(),
        type_: right_agent_config::ProviderType::Generic,
        label: entry.label.clone(),
        generic: Some(right_agent_config::GenericProvider {
            env_var: new_env_var.clone(),
            header_name: new_header.clone(),
            upstream_host: new_host.clone(),
            upstream_path_prefix: new_path.clone(),
        }),
    };
    if let Err(e) = replace_provider_in_yaml(&state.agents_dir, &req.agent, &updated) {
        // Gateway accepted the new config but agent.yaml is now stale.
        // Roll BOTH the gateway and the policy snapshot back so we don't
        // leave the system in a divergent state where the proxy uses the
        // new header/path but agent.yaml still describes the old values.
        let mut rollback_errors: Vec<String> = Vec::new();
        let rollback_spec = build_gateway_rollback_spec(&gateway_snapshot);
        if let Err(rollback_err) =
            right_openshell::providers::update_provider(&mut client, &rollback_spec).await
        {
            tracing::error!(
                provider = %req.name,
                original_err = %e,
                "provider rollback failed: could not restore gateway state after yaml-write failure: {rollback_err:#}"
            );
            rollback_errors.push(format!("gateway: {rollback_err:#}"));
        }
        if let Some(s) = snapshot {
            if let Err(rollback_err) = s.restore().await {
                tracing::error!(
                    provider = %req.name,
                    original_err = %e,
                    "provider rollback failed: could not restore policy snapshot after yaml-write failure: {rollback_err:#}"
                );
                rollback_errors.push(format!("policy: {rollback_err:#}"));
            }
        }
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
    }))
}

/// Build the `ProviderSpec` used to roll the gateway back to a captured
/// snapshot. `credentials` is left empty on purpose: `get_provider` never
/// returns credential bytes, and the OpenShell `UpdateProvider` RPC treats
/// the `credentials` map as a sparse merge — an empty map preserves
/// existing credentials rather than wiping them.
///
/// Extracted so the rollback shape can be unit-tested without spinning up
/// a real gateway (see `provider_config_update_rollback_spec_*` tests).
fn build_gateway_rollback_spec(
    snapshot: &right_openshell::providers::Provider,
) -> right_openshell::providers::ProviderSpec {
    right_openshell::providers::ProviderSpec {
        name: snapshot.name.clone(),
        type_: snapshot.type_.clone(),
        credentials: Default::default(),
        config: snapshot.config.clone(),
    }
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
    // data — remove it first, then strip the policy stanza. If the
    // policy strip fails the user sees a degraded-state error, but
    // they don't end up with a permanently stranded agent.yaml row.
    remove_provider_from_yaml(&state.agents_dir, &req.agent, &req.name)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;

    if let Some(g) = &entry.generic {
        let policy_path = state.agents_dir.join(&req.agent).join(
            sandbox
                .policy_file
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("policy.yaml")),
        );
        let used_by_other = sandbox.providers.iter().any(|p| {
            p.name != req.name
                && p.generic
                    .as_ref()
                    .map(|gp| gp.upstream_host == g.upstream_host)
                    .unwrap_or(false)
        });
        if !used_by_other {
            let prior = std::fs::read_to_string(&policy_path)
                .map_err(|e| ProviderApiError::AgentYamlWrite(format!("read policy: {e:#}")))?;
            let stripped =
                right_codegen::policy::providers_strip(&prior, &req.name, &g.upstream_host);
            right_codegen::contract::write_and_apply_sandbox_policy(
                &sandbox_name,
                &policy_path,
                &stripped,
            )
            .await
            .map_err(|e| ProviderApiError::Gateway(format!("policy apply: {e:#}")))?;
        }
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
        // Restrictive network policy renders only the Anthropic allowlist
        // sub-section — there is no outbound endpoints list to extend with
        // generic provider stanzas, so creation must be refused up-front.
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

    /// `handle_provider_config_update` must capture the gateway's
    /// pre-mutation config so it can roll the gateway back when a later
    /// step (notably `replace_provider_in_yaml`) fails. The rollback spec
    /// must round-trip the original `name`, `type_`, and `config`, and
    /// must NOT carry credentials — `get_provider` does not return them,
    /// and OpenShell's `UpdateProvider` treats `credentials` as a sparse
    /// merge so an empty map preserves existing credentials.
    ///
    /// Live-gateway end-to-end coverage of the failure path requires a
    /// real OpenShell gateway plus a yaml-write fault, which the unit
    /// suite can't provide without major plumbing. This test pins the
    /// rollback shape (the load-bearing invariant) directly via the
    /// extracted helper — the same shape the handler sends when yaml
    /// write fails.
    #[test]
    fn config_update_rollback_on_yaml_failure_restores_gateway() {
        use std::collections::HashMap;

        let mut config = HashMap::new();
        config.insert("header_name".into(), "x-api-key".into());
        config.insert("upstream_host".into(), "api.acme.invalid".into());
        config.insert("upstream_path_prefix".into(), "/v1".into());
        let snapshot = right_openshell::providers::Provider {
            name: "hostagent-acme".into(),
            type_: "generic".into(),
            config: config.clone(),
            updated_at: None,
        };

        let spec = super::build_gateway_rollback_spec(&snapshot);

        assert_eq!(spec.name, "hostagent-acme");
        assert_eq!(spec.type_, "generic");
        assert_eq!(
            spec.config, config,
            "rollback must restore every captured config field"
        );
        assert!(
            spec.credentials.is_empty(),
            "rollback must NOT carry credentials — an empty map is a sparse merge that \
             preserves existing credentials; sending a populated map would risk wiping them"
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
                    header_name: "Authorization".to_string(),
                    upstream_host: format!("api{i:02}.example.com"),
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
        .filter(|p| !hidden.iter().any(|h| *h == p.type_slug.as_str()))
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
