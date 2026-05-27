//! Provider management routes — see ARCHITECTURE.md "Providers".
// Handlers below use validators and helpers added in Tasks 13-14.
// The `#[allow(dead_code)]` is narrowed here: Tasks 17+ will consume the
// validators, but they are not dead — suppressed until then.
#![allow(dead_code)]

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
    #[error("providers_v2_enabled is not on the gateway")]
    V2NotEnabled,
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
            Self::V2NotEnabled => (StatusCode::SERVICE_UNAVAILABLE, "v2_not_enabled"),
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
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint()
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    let catalog = right_openshell::providers::profile_catalog();
    let mut views = Vec::with_capacity(sandbox.providers.len());
    for entry in &sandbox.providers {
        let status = match right_openshell::providers::get_provider(&endpoint, &entry.name).await {
            Ok(_) => ProviderStatus::Healthy,
            Err(right_openshell::providers::ProviderError::NotFound(_)) => ProviderStatus::Missing,
            Err(e) => ProviderStatus::GatewayError {
                message: format!("{e:#}"),
            },
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
            updated_at: None,
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
    let label_slug = req.label.clone().unwrap_or_else(|| req.type_.clone());
    let name = format!("{}-{}", req.agent, label_slug);
    validate_name(&req.agent, &name)?;

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
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint()
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    let sandbox_name = sandbox.name.clone().unwrap_or_else(|| req.agent.clone());

    let mut creds = std::collections::HashMap::new();
    creds.insert(env_var.clone(), req.credential.expose_secret().to_string());
    let spec = right_openshell::providers::ProviderSpec {
        name: name.clone(),
        type_: req.type_.clone(),
        credentials: creds,
        config: Default::default(),
    };
    right_openshell::providers::create_provider(&endpoint, &spec)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    if let Err(attach_err) =
        right_openshell::providers::attach_to_sandbox(&endpoint, &sandbox_name, &name).await
    {
        let _ = right_openshell::providers::delete_provider(&endpoint, &name).await;
        return Err(ProviderApiError::Gateway(format!("{attach_err:#}")));
    }

    let entry = right_agent_config::ProviderEntry {
        name: name.clone(),
        type_: right_agent_config::ProviderType::BuiltIn(req.type_.clone()),
        label: req.label.clone(),
        generic: None,
    };
    if let Err(e) = append_provider_to_yaml(&state.agents_dir, &req.agent, &entry) {
        let _ =
            right_openshell::providers::detach_from_sandbox(&endpoint, &sandbox_name, &name).await;
        let _ = right_openshell::providers::delete_provider(&endpoint, &name).await;
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
    let sandbox = cfg.sandbox.as_ref().unwrap();
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

    let endpoint = right_openshell::openshell::resolve_gateway_endpoint()
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
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
    if let Err(e) = right_openshell::providers::create_provider(&endpoint, &spec).await {
        let _ = snapshot.restore().await;
        return Err(ProviderApiError::Gateway(format!("{e:#}")));
    }

    if let Err(attach_err) =
        right_openshell::providers::attach_to_sandbox(&endpoint, &sandbox_name, &name).await
    {
        let _ = right_openshell::providers::delete_provider(&endpoint, &name).await;
        let _ = snapshot.restore().await;
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
        let _ =
            right_openshell::providers::detach_from_sandbox(&endpoint, &sandbox_name, &name).await;
        let _ = right_openshell::providers::delete_provider(&endpoint, &name).await;
        let _ = snapshot.restore().await;
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
