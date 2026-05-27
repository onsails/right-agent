//! Provider management routes — see ARCHITECTURE.md "Providers".
// Callers arrive in Tasks 15+; suppress dead_code until then.
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
