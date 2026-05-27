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
    #[error("policy conflict on host \"{host}\": {kind}")]
    PolicyConflict { host: String, kind: String },
    #[error("openshell gateway: {0}")]
    Gateway(String),
    #[error("agent.yaml write failed after gateway change: {0}")]
    AgentYamlWrite(String),
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
            Self::PolicyConflict { .. } => (StatusCode::CONFLICT, "policy_conflict"),
            Self::Gateway(_) => (StatusCode::BAD_GATEWAY, "gateway"),
            Self::AgentYamlWrite(_) => (StatusCode::INTERNAL_SERVER_ERROR, "agent_yaml_write"),
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
    validate_yaml_scalar(label, "label", 32, "-_")
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
    let known = [
        "anthropic",
        "codex",
        "copilot",
        "github",
        "gitlab",
        "nvidia",
        "openai",
        "opencode",
        "generic",
    ];
    if !known.contains(&slug) {
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
        assert!(!would_collide(&providers, "agent-openai", "MY_CUSTOM_KEY"));
        // Renaming "agent-openai" to the same var it already has → false (no-op, caller skips).
        assert!(!would_collide(&providers, "agent-openai", "OPENAI_API_KEY"));
        // Renaming some other provider to OPENAI_API_KEY → collides with agent-openai.
        assert!(would_collide(&providers, "agent-other", "OPENAI_API_KEY"));
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
        assert!(would_collide(
            &providers,
            "agent-custom",
            "ANTHROPIC_API_KEY"
        ));
        // Renaming agent-custom to something else → no collision.
        assert!(!would_collide(&providers, "agent-custom", "OPENAI_API_KEY"));
    }

    #[test]
    fn would_collide_single_provider_never_collides_with_itself() {
        let providers = vec![make_generic_entry("agent-foo", "FOO_API_KEY")];
        // The only provider is the one being updated — should never collide with itself.
        assert!(!would_collide(&providers, "agent-foo", "FOO_API_KEY"));
        assert!(!would_collide(&providers, "agent-foo", "NEW_KEY"));
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
    GatewayError { message: String },
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
    let catalog = right_openshell::providers::profile_catalog();
    let mut views = Vec::with_capacity(sandbox.providers.len());
    for entry in &sandbox.providers {
        let (status, updated_at) =
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
        let env_var = match &entry.type_ {
            right_agent_config::ProviderType::Generic => entry
                .generic
                .as_ref()
                .map(|g| g.env_var.clone())
                .unwrap_or_default(),
            right_agent_config::ProviderType::BuiltIn(slug) => catalog
                .iter()
                .find(|p| &p.type_slug == slug)
                .map(|p| p.env_var.clone())
                .unwrap_or_default(),
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
    let _guard = provider_lock(&state, &req.agent, &name).await;

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
    if sandbox
        .providers
        .iter()
        .any(|p| extract_env_var(p) == env_var)
    {
        return Err(ProviderApiError::EnvVarCollision { env_var });
    }

    if req.type_ == "generic" {
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

/// Returns `true` if renaming `excluding_name`'s env_var to `new_env_var`
/// would collide with another provider in `providers`.
fn would_collide(
    providers: &[right_agent_config::ProviderEntry],
    excluding_name: &str,
    new_env_var: &str,
) -> bool {
    providers
        .iter()
        .any(|p| p.name != excluding_name && extract_env_var(p) == new_env_var)
}

fn extract_env_var(entry: &right_agent_config::ProviderEntry) -> String {
    match &entry.type_ {
        right_agent_config::ProviderType::Generic => entry
            .generic
            .as_ref()
            .map(|g| g.env_var.clone())
            .unwrap_or_default(),
        right_agent_config::ProviderType::BuiltIn(slug) => {
            right_openshell::providers::profile_catalog()
                .into_iter()
                .find(|p| &p.type_slug == slug)
                .map(|p| p.env_var)
                .unwrap_or_default()
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
fn serialize_provider_entry(entry: &right_agent_config::ProviderEntry) -> String {
    let mut out = String::new();
    out.push_str(&format!("    - name: {}\n", entry.name));
    let type_str = match &entry.type_ {
        right_agent_config::ProviderType::Generic => "generic".to_string(),
        right_agent_config::ProviderType::BuiltIn(s) => s.clone(),
    };
    out.push_str(&format!("      type: {type_str}\n"));
    if let Some(label) = &entry.label {
        out.push_str(&format!("      label: {label}\n"));
    }
    if let Some(g) = &entry.generic {
        out.push_str("      generic:\n");
        out.push_str(&format!("        env_var: {}\n", g.env_var));
        out.push_str(&format!("        header_name: {}\n", g.header_name));
        out.push_str(&format!("        upstream_host: {}\n", g.upstream_host));
        if let Some(prefix) = &g.upstream_path_prefix {
            out.push_str(&format!("        upstream_path_prefix: {prefix}\n"));
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
}

// ── Tasks 19–22: /provider-rotate, /provider-config-update, /provider-remove ─

// ── Task 22 (mutex helper) ───────────────────────────────────────────────────

pub(crate) async fn provider_lock(
    state: &crate::internal_api::InternalState,
    agent: &str,
    name: &str,
) -> tokio::sync::OwnedMutexGuard<()> {
    let key = (agent.to_string(), name.to_string());
    let lock = {
        let mut map = state.provider_locks.lock().await;
        map.entry(key)
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
    let _guard = provider_lock(&state, &req.agent, &req.name).await;

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

    let env_var = extract_env_var(entry);
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
    let _guard = provider_lock(&state, &req.agent, &req.name).await;

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
    if new_env_var != current.env_var && would_collide(&sandbox.providers, &req.name, &new_env_var)
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
        if let Some(s) = snapshot {
            if let Err(rollback_err) = s.restore().await {
                tracing::warn!(
                    provider = %req.name,
                    original_err = %e,
                    "provider rollback failed: could not restore policy snapshot after yaml-write failure: {rollback_err:#}"
                );
            }
        }
        return Err(ProviderApiError::AgentYamlWrite(format!("{e:#}")));
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

/// Replace the entry whose `    - name: <name>` matches. Returns Err if not found.
fn replace_provider_entry(original: &str, name: &str, new_entry: &str) -> miette::Result<String> {
    let name_marker = format!("    - name: {name}");
    let Some(start_byte) = original.find(&name_marker) else {
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
    let _guard = provider_lock(&state, &req.agent, &req.name).await;

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
    let name_marker = format!("    - name: {name}");
    let Some(start_byte) = original.find(&name_marker) else {
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
    let catalog = right_openshell::providers::profile_catalog();
    let views: Vec<_> = catalog
        .into_iter()
        .map(|p| ProviderProfileView {
            type_slug: p.type_slug,
            env_var: p.env_var,
            display_name: p.display_name,
            category: format!("{:?}", p.category).to_lowercase(),
        })
        .collect();
    axum::Json(views)
}
