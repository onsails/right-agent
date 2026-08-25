//! Provider secret bindings: source-ref secrets over TLS interception.
//!
//! A [`SecretBinding`] carries a redacted, zeroizing credential value alongside
//! its durable source identity. The value is installed into microsandbox's
//! scoped in-process resolver only while create/start/apply executes. Durable
//! config stores only the source identity and placeholder; the guest never
//! receives the credential directly.
//!
//! TLS interception follows ADR-0003: it is a **bypass deny-list**. Adding
//! any secret enables interception for every destination on the intercepted
//! ports except the bypass list. [`TLS_BYPASS_HOSTS`] always carries the
//! Anthropic hosts so the agent's primary path is never intercepted and needs
//! no guest CA configuration.

use std::sync::Arc;

use microsandbox::SecretSource;
use microsandbox::sandbox::SecretBuilder;
use secrecy::{ExposeSecret as _, SecretString};

use crate::error::SandboxError;

/// Hosts that are never TLS-intercepted: the Anthropic API paths Claude Code
/// talks to. Maintained, security-relevant inventory (ADR-0003) — review on
/// change.
pub const TLS_BYPASS_HOSTS: &[&str] = &["api.anthropic.com", "*.anthropic.com"];

/// The full TLS bypass list: Anthropic hosts plus caller-supplied extras,
/// deduplicated, order-preserved.
pub fn tls_bypass_list(extra: &[String]) -> Vec<String> {
    let mut list: Vec<String> = TLS_BYPASS_HOSTS
        .iter()
        .map(|host| (*host).to_owned())
        .collect();
    for host in extra {
        if !list.contains(host) {
            list.push(host.clone());
        }
    }
    list
}

/// The placeholder the guest sees for `env_var` when none is set explicitly.
///
/// Matches the SDK's own default (`$MSB_<ENV_VAR>`) so a binding that never
/// sets a placeholder explicitly still names a stable string.
pub fn default_placeholder(env_var: &str) -> String {
    format!("$MSB_{env_var}")
}

/// A provider credential binding.
///
/// The credential is private, zeroized, and redacted from `Debug`. The SDK
/// persists only `source_env_var`; `resolved_value` is made available through
/// a scoped resolver during create/start/apply and is then dropped.
#[derive(Clone)]
pub struct SecretBinding {
    /// Guest-visible environment variable; holds the placeholder.
    pub env_var: String,

    /// Durable host-side source identity. It identifies the owning record,
    /// not only the provider's display name.
    pub source_env_var: String,

    /// Stable placeholder string the guest sees. Survives rotation.
    pub placeholder: String,

    /// Hosts allowed to receive the substituted value. Exact hosts
    /// (`"api.example.com"`) or suffix wildcards (`"*.example.com"`).
    pub allowed_hosts: Vec<String>,

    /// Opt in to query-parameter substitution (headers and basic-auth are on
    /// by default; body injection is never enabled).
    pub inject_query: bool,

    resolved_value: Arc<SecretString>,
}

impl std::fmt::Debug for SecretBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretBinding")
            .field("env_var", &self.env_var)
            .field("source_env_var", &self.source_env_var)
            .field("placeholder", &self.placeholder)
            .field("allowed_hosts", &self.allowed_hosts)
            .field("inject_query", &self.inject_query)
            .field("resolved_value", &"<redacted>")
            .finish()
    }
}

impl PartialEq for SecretBinding {
    fn eq(&self, other: &Self) -> bool {
        self.env_var == other.env_var
            && self.source_env_var == other.source_env_var
            && self.placeholder == other.placeholder
            && self.allowed_hosts == other.allowed_hosts
            && self.inject_query == other.inject_query
            && self.resolved_value.as_ref().expose_secret()
                == other.resolved_value.as_ref().expose_secret()
    }
}

impl Eq for SecretBinding {}

impl SecretBinding {
    /// A binding with the SDK-default placeholder and no allowed hosts.
    pub fn new(
        env_var: impl Into<String>,
        source_env_var: impl Into<String>,
        resolved_value: SecretString,
    ) -> Self {
        let env_var = env_var.into();
        Self {
            placeholder: default_placeholder(&env_var),
            env_var,
            source_env_var: source_env_var.into(),
            allowed_hosts: Vec::new(),
            resolved_value: Arc::new(resolved_value),
            inject_query: false,
        }
    }

    /// Construct a fully-resolved binding received from the authenticated
    /// internal provider-binding endpoint. The credential is immediately
    /// wrapped in the binding's private zeroizing storage.
    pub fn from_resolved_parts(
        env_var: String,
        source_env_var: String,
        placeholder: String,
        allowed_hosts: Vec<String>,
        inject_query: bool,
        resolved_value: SecretString,
    ) -> Self {
        Self {
            env_var,
            source_env_var,
            placeholder,
            allowed_hosts,
            inject_query,
            resolved_value: Arc::new(resolved_value),
        }
    }

    /// Validate the binding. Placeholder rules mirror the SDK's: non-empty,
    /// at most 1024 bytes, no NUL/CR/LF.
    pub(crate) fn validate(&self) -> Result<(), SandboxError> {
        /// SDK placeholder cap (`MAX_SECRET_PLACEHOLDER_BYTES`).
        const MAX_PLACEHOLDER_BYTES: usize = 1024;

        if self.env_var.is_empty() || self.env_var.contains(['=', '\0']) {
            return Err(SandboxError::InvalidSpec {
                field: "secrets.env_var",
                reason: "must be non-empty and contain no '=' or NUL".to_owned(),
            });
        }
        if self.source_env_var.is_empty() {
            return Err(SandboxError::InvalidSpec {
                field: "secrets.source_env_var",
                reason: "must be non-empty".to_owned(),
            });
        }
        if self.placeholder.is_empty()
            || self.placeholder.len() > MAX_PLACEHOLDER_BYTES
            || self.placeholder.contains(['\0', '\r', '\n'])
        {
            return Err(SandboxError::InvalidSpec {
                field: "secrets.placeholder",
                reason: format!(
                    "must be non-empty, at most {MAX_PLACEHOLDER_BYTES} bytes, and contain no NUL/CR/LF"
                ),
            });
        }
        if self.allowed_hosts.is_empty() {
            return Err(SandboxError::InvalidSpec {
                field: "secrets.allowed_hosts",
                reason: "a secret with no allowed hosts can never be substituted".to_owned(),
            });
        }
        for host in &self.allowed_hosts {
            let suffix = host.strip_prefix("*.");
            if host.is_empty() || suffix.is_some_and(str::is_empty) {
                return Err(SandboxError::InvalidSpec {
                    field: "secrets.allowed_hosts",
                    reason: format!("invalid host pattern {host:?}"),
                });
            }
        }
        Ok(())
    }

    /// Build the SDK secret builder. The value is a source reference; nothing
    /// secret is constructed here.
    pub(crate) fn sdk_builder(&self) -> SecretBuilder {
        let mut builder = SecretBuilder::new()
            .env(&self.env_var)
            .source(SecretSource::Env {
                var: self.source_env_var.clone(),
            })
            .placeholder(&self.placeholder);
        for host in &self.allowed_hosts {
            builder = if host.starts_with("*.") {
                builder.allow_host_pattern(host)
            } else {
                builder.allow_host(host)
            };
        }
        if self.inject_query {
            builder = builder.inject_query(true);
        }
        builder
    }

    pub(crate) fn resolver_value(&self) -> (String, zeroize::Zeroizing<String>) {
        (
            self.source_env_var.clone(),
            zeroize::Zeroizing::new(self.resolved_value.as_ref().expose_secret().to_owned()),
        )
    }

    /// Consume this short-lived binding into its transport-safe metadata and
    /// secret value. This is intentionally consumption-only: provider-binding
    /// IPC encodes the value once over the owner-only Unix socket, then drops
    /// the DTO. Debug output remains redacted throughout.
    pub fn into_transport_parts(self) -> (String, String, String, Vec<String>, bool, SecretString) {
        let value = SecretString::from(self.resolved_value.as_ref().expose_secret().to_owned());
        (
            self.env_var,
            self.source_env_var,
            self.placeholder,
            self.allowed_hosts,
            self.inject_query,
            value,
        )
    }
}

/// How a provider secret was made effective in the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretApplyDisposition {
    /// An existing binding's value was rotated in the running sandbox.
    RotatedLive,

    /// A missing binding was persisted and made effective by the SDK's
    /// restart-backed apply. The sandbox filesystem is preserved.
    AddedWithRestart,
}

/// How a provider secret removal converged in the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretRemoveDisposition {
    /// The binding was revoked through the running sandbox's live-control path.
    RemovedLive,

    /// The binding was already absent, so removal was a declarative no-op.
    AlreadyAbsent,
}

/// The outcome of removing one provider secret binding.
#[derive(Debug, Clone)]
pub struct SecretRemove {
    /// Whether the binding was removed live or was already absent.
    pub disposition: SecretRemoveDisposition,

    /// Non-fatal planner/apply warnings, rendered as `field: message`.
    pub warnings: Vec<String>,
}

/// The outcome of applying one provider secret binding.
#[derive(Debug, Clone)]
pub struct SecretApply {
    /// Whether the binding rotated live or was added with a restart.
    pub disposition: SecretApplyDisposition,

    /// Non-fatal planner/apply warnings, rendered as `field: message`.
    pub warnings: Vec<String>,
}

/// Classify the SDK's secret change into Right's externally meaningful
/// application path. Kept pure so routing is covered without booting a VM.
pub(crate) fn classify_apply_change(
    change: microsandbox::SecretChangeKind,
) -> Option<SecretApplyDisposition> {
    match change {
        microsandbox::SecretChangeKind::Rotated => Some(SecretApplyDisposition::RotatedLive),
        microsandbox::SecretChangeKind::Added => Some(SecretApplyDisposition::AddedWithRestart),
        _ => None,
    }
}

/// Host sets for failure-safe credential rotation. Removals are applied before
/// rotation; additions are applied only after rotation succeeds.
pub(crate) fn host_rotation_stages(
    current: &[String],
    desired: &[String],
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let retained: Vec<String> = current
        .iter()
        .filter(|host| desired.contains(host))
        .cloned()
        .collect();
    let shrink = (retained.len() != current.len()).then_some(retained);
    let widen = desired
        .iter()
        .any(|host| !current.contains(host))
        .then(|| desired.to_vec());
    (shrink, widen)
}

/// Whether the pinned SDK can add this binding without losing injection policy.
pub(crate) fn addition_supported(binding: &SecretBinding) -> bool {
    !binding.inject_query
}

#[cfg(test)]
mod tests {
    use microsandbox::HostPattern;

    use super::*;

    fn valid_binding() -> SecretBinding {
        let mut binding = SecretBinding::new("KEY", "HOST_KEY", SecretString::from("secret"));
        binding.allowed_hosts = vec!["api.example.com".to_owned()];
        binding
    }

    #[test]
    fn apply_change_routes_rotation_live_and_addition_to_restart() {
        assert_eq!(
            classify_apply_change(microsandbox::SecretChangeKind::Rotated),
            Some(SecretApplyDisposition::RotatedLive)
        );
        assert_eq!(
            classify_apply_change(microsandbox::SecretChangeKind::Added),
            Some(SecretApplyDisposition::AddedWithRestart)
        );
        assert_eq!(
            classify_apply_change(microsandbox::SecretChangeKind::Removed),
            None,
            "removal must never be mistaken for a successful provider apply"
        );
    }

    #[test]
    fn rotation_failure_never_exposes_old_credential_to_added_host() {
        let current = vec!["old.example.com".to_owned(), "keep.example.com".to_owned()];
        let desired = vec!["keep.example.com".to_owned(), "new.example.com".to_owned()];
        let (shrink, widen) = host_rotation_stages(&current, &desired);

        let mut effective = shrink.expect("removal must shrink before rotation");
        let rotation_succeeded = false;
        if rotation_succeeded {
            effective = widen.expect("addition must widen after rotation");
        }

        assert_eq!(effective, ["keep.example.com"]);
        assert!(!effective.contains(&"new.example.com".to_owned()));
    }

    #[test]
    fn missing_query_injected_binding_is_not_supported_by_sdk_modify() {
        let mut binding = valid_binding();
        assert!(addition_supported(&binding));
        binding.inject_query = true;
        assert!(
            !addition_supported(&binding),
            "addition must fail rather than silently drop query injection"
        );
    }

    #[test]
    fn anthropic_hosts_are_always_bypassed() {
        let list = tls_bypass_list(&[]);
        assert!(list.contains(&"api.anthropic.com".to_owned()));
        assert!(list.contains(&"*.anthropic.com".to_owned()));
    }

    #[test]
    fn bypass_list_appends_extras_without_duplicates() {
        let list = tls_bypass_list(&["internal.corp".to_owned(), "api.anthropic.com".to_owned()]);
        assert_eq!(
            list,
            ["api.anthropic.com", "*.anthropic.com", "internal.corp"],
            "base hosts first, extras appended, duplicates dropped: {list:?}"
        );
    }

    #[test]
    fn default_placeholder_matches_the_sdk_shape() {
        assert_eq!(
            default_placeholder("RIGHT_PROVIDER_KEY"),
            "$MSB_RIGHT_PROVIDER_KEY"
        );
    }

    #[test]
    fn binding_translation_carries_only_references() {
        let mut binding = SecretBinding::new(
            "RIGHT_PROVIDER_KEY",
            "HOST_KEY_VAR",
            SecretString::from("secret"),
        );
        binding.allowed_hosts = vec!["api.example.com".to_owned(), "*.example.org".to_owned()];
        let entry = binding.sdk_builder().build();

        assert_eq!(entry.env_var, "RIGHT_PROVIDER_KEY");
        assert!(
            entry.value.is_empty(),
            "a source-ref binding must never carry a value"
        );
        assert_eq!(
            entry.source,
            Some(SecretSource::Env {
                var: "HOST_KEY_VAR".to_owned()
            })
        );
        assert_eq!(entry.placeholder, "$MSB_RIGHT_PROVIDER_KEY");
        assert_eq!(
            entry.allowed_hosts,
            [
                HostPattern::Exact("api.example.com".to_owned()),
                HostPattern::Wildcard("*.example.org".to_owned()),
            ]
        );
        assert!(entry.injection.headers, "headers inject by default");
        assert!(!entry.injection.query_params, "query params are opt-in");
        assert!(!entry.injection.body, "body injection is never enabled");
    }

    #[test]
    fn explicit_placeholder_overrides_the_default() {
        let mut binding = valid_binding();
        binding.placeholder = "stable-placeholder".to_owned();
        let entry = binding.sdk_builder().build();
        assert_eq!(entry.placeholder, "stable-placeholder");
    }

    #[test]
    fn query_injection_is_opt_in() {
        let mut binding = valid_binding();
        binding.inject_query = true;
        assert!(binding.sdk_builder().build().injection.query_params);
    }

    #[test]
    fn invalid_bindings_are_rejected() {
        let binding = valid_binding();
        assert!(binding.validate().is_ok(), "a complete binding validates");

        let mut empty_hosts = binding.clone();
        empty_hosts.allowed_hosts = Vec::new();
        assert!(empty_hosts.validate().is_err(), "no allowed hosts");

        let mut empty_env = binding.clone();
        empty_env.env_var = String::new();
        assert!(empty_env.validate().is_err(), "empty env var");

        let mut eq_env = binding.clone();
        eq_env.env_var = "A=B".to_owned();
        assert!(eq_env.validate().is_err(), "'=' in env var");

        let mut empty_source = binding.clone();
        empty_source.source_env_var = String::new();
        assert!(empty_source.validate().is_err(), "empty source env var");

        for placeholder in ["", "with\nnewline", "with\rcr", "with\0nul"] {
            let mut bad = binding.clone();
            bad.placeholder = placeholder.to_owned();
            assert!(bad.validate().is_err(), "placeholder {placeholder:?}");
        }

        let mut long = binding.clone();
        long.placeholder = "x".repeat(1025);
        assert!(long.validate().is_err(), "placeholder over 1024 bytes");

        let mut bad_host = binding.clone();
        bad_host.allowed_hosts = vec!["*.".to_owned()];
        assert!(bad_host.validate().is_err(), "wildcard with empty suffix");
    }
}
