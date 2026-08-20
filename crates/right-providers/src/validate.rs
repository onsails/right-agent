//! Pure validation helpers.
//!
//! Ported byte-for-byte from `right::internal_api_providers` so the dashboard
//! keeps seeing the same rejections with the same reason strings. Every
//! function here is side-effect free and callable without a database — the
//! config loader uses them too.

use crate::catalog;
use crate::error::StoreError;

/// Max length of the slug that remains after stripping an optional
/// `{agent}-` prefix.
const MAX_SLUG_LEN: usize = 40;
/// Max length of the full record name including any `{agent}-` prefix.
const MAX_NAME_LEN: usize = 64;
/// Max length of an environment variable name.
const MAX_ENV_VAR_LEN: usize = 64;
/// Max length of a human label.
const MAX_LABEL_LEN: usize = 32;
/// Max length of an upstream host (DNS name limit).
const MAX_HOST_LEN: usize = 253;
/// Max length of an upstream path prefix.
const MAX_PATH_PREFIX_LEN: usize = 128;
/// Hex characters of entropy in a generated record name.
const RECORD_NAME_HEX_LEN: usize = 6;

/// Validate a provider record name.
///
/// Accepts either the legacy `{agent}-{slug}` shape or the agent-agnostic
/// `{type-slug}-{hex}` shape: the agent prefix is stripped when present and
/// the remainder is length-checked, then the full name is checked as a whole.
pub fn validate_name(agent: &str, name: &str) -> Result<(), StoreError> {
    let slug = name.strip_prefix(&format!("{agent}-")).unwrap_or(name);
    if slug.is_empty() || slug.len() > MAX_SLUG_LEN {
        return Err(StoreError::InvalidName {
            name: name.into(),
            reason: format!("1-{MAX_SLUG_LEN} chars after optional agent prefix"),
        });
    }
    if name.len() > MAX_NAME_LEN {
        return Err(StoreError::InvalidName {
            name: name.into(),
            reason: format!("name too long (max {MAX_NAME_LEN})"),
        });
    }
    let first_ok = name.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    let rest_ok = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !first_ok || !rest_ok {
        return Err(StoreError::InvalidName {
            name: name.into(),
            reason: "lowercase a-z/0-9/'-', must start a-z".into(),
        });
    }
    Ok(())
}

/// Mint an agent-agnostic record id: `{type-slug}-{6 hex}`.
///
/// The `right-` ownership prefix is stripped (`right-fal` → `fal-a1b2c3`) and
/// every generic shape collapses to `generic-`.
pub fn new_record_name(type_slug: &str) -> String {
    let base = type_slug.strip_prefix("right-").unwrap_or(type_slug);
    let base = if base.is_empty() || base.starts_with(catalog::GENERIC_SLUG) {
        catalog::GENERIC_SLUG
    } else {
        base
    };
    let hex: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(RECORD_NAME_HEX_LEN)
        .collect();
    format!("{base}-{hex}")
}

/// Validate a provider type slug against the catalog.
///
/// `claude` is reserved for the in-sandbox login flow and is rejected before
/// the catalog lookup so the reason string stays specific.
pub fn validate_type_slug(slug: &str) -> Result<(), StoreError> {
    if slug == catalog::RESERVED_TYPE_SLUG {
        return Err(StoreError::InvalidName {
            name: slug.into(),
            reason: format!(
                "type \"{}\" is reserved for the in-sandbox login flow",
                catalog::RESERVED_TYPE_SLUG
            ),
        });
    }
    if catalog::builtin(slug).is_none() {
        return Err(StoreError::InvalidName {
            name: slug.into(),
            reason: format!("unknown type \"{slug}\""),
        });
    }
    Ok(())
}

/// Validate an environment variable name: `[A-Z_][A-Z0-9_]*`, 1-64 chars.
pub fn validate_env_var(name: &str) -> Result<(), StoreError> {
    let invalid = || StoreError::InvalidEnvVar {
        env_var: name.into(),
    };
    if name.is_empty() || name.len() > MAX_ENV_VAR_LEN {
        return Err(invalid());
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(invalid());
    };
    if !(first.is_ascii_uppercase() || first == '_') {
        return Err(invalid());
    }
    for c in chars {
        if !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
            return Err(invalid());
        }
    }
    Ok(())
}

/// Validate a free-form scalar that ends up verbatim in `agent.yaml`.
///
/// The alphabet is deliberately narrow — hostnames, URL path prefixes, and
/// human labels all fit, and nothing that could shift YAML indentation does.
fn validate_yaml_scalar(
    value: &str,
    field: &str,
    max_len: usize,
    extra_allowed: &str,
) -> Result<(), StoreError> {
    if value.is_empty() || value.len() > max_len {
        return Err(StoreError::InvalidName {
            name: value.into(),
            reason: format!("{field} must be 1-{max_len} chars"),
        });
    }
    for c in value.chars() {
        if !(c.is_ascii_alphanumeric() || extra_allowed.contains(c)) {
            return Err(StoreError::InvalidName {
                name: value.into(),
                reason: format!("{field} contains disallowed character {c:?}"),
            });
        }
    }
    Ok(())
}

/// Validate a human label.
///
/// Beyond the scalar alphabet, tokens that YAML 1.1 would parse as a boolean,
/// null, or number are rejected: `label: no` would fail `Option<String>`
/// deserialization on the next bot start — a self-inflicted DoS reachable
/// from the dashboard.
pub fn validate_label(label: &str) -> Result<(), StoreError> {
    validate_yaml_scalar(label, "label", MAX_LABEL_LEN, "-_")?;
    if is_yaml_reserved_word(label) || is_pure_numeric(label) {
        return Err(StoreError::InvalidName {
            name: label.into(),
            reason: "label must not be a YAML-reserved word or a pure number".into(),
        });
    }
    Ok(())
}

/// YAML 1.1 reserved boolean and null tokens, matched case-insensitively.
fn is_yaml_reserved_word(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "y" | "yes" | "n" | "no" | "true" | "false" | "on" | "off" | "null" | "~"
    )
}

/// True when the string would be coerced to a number by a YAML loader.
fn is_pure_numeric(s: &str) -> bool {
    s.parse::<i64>().is_ok() || s.parse::<f64>().is_ok()
}

pub fn validate_upstream_host(host: &str) -> Result<(), StoreError> {
    validate_yaml_scalar(host, "upstream_host", MAX_HOST_LEN, ".-_:")
}

pub fn validate_path_prefix(path: &str) -> Result<(), StoreError> {
    validate_yaml_scalar(path, "upstream_path_prefix", MAX_PATH_PREFIX_LEN, "-_/.~")
}

/// Merge the singular `upstream_host` and the plural `upstream_hosts` inputs
/// into one trimmed, order-preserved, deduplicated list.
pub fn normalize_generic_hosts(
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
    if let Some(extra) = upstream_hosts {
        hosts.extend(extra.iter().filter_map(|host| {
            let host = host.trim();
            (!host.is_empty()).then(|| host.to_string())
        }));
    }
    let mut seen = std::collections::HashSet::new();
    hosts.retain(|host| seen.insert(host.clone()));
    hosts
}

/// Validate a generic-provider definition and return its normalized hosts.
pub fn validate_generic_request(
    env_var: &str,
    upstream_host: Option<&str>,
    upstream_hosts: Option<&[String]>,
    upstream_path_prefix: Option<&str>,
) -> Result<Vec<String>, StoreError> {
    validate_env_var(env_var)?;
    let hosts = normalize_generic_hosts(upstream_host, upstream_hosts);
    if hosts.is_empty() {
        return Err(StoreError::InvalidName {
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

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
