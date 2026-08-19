//! Provider secret bindings: source-ref secrets over TLS interception.
//!
//! A [`SecretBinding`] never carries a credential value. It names the
//! guest-visible environment variable (which holds the *placeholder*), the
//! host-side environment variable the value is resolved from at spawn/apply
//! time, and the hosts allowed to receive the real value. The SDK persists
//! only the placeholder and the source reference — no secret material at
//! rest.
//!
//! TLS interception follows ADR-0003: it is a **bypass deny-list**. Adding
//! any secret enables interception for every destination on the intercepted
//! ports except the bypass list. [`TLS_BYPASS_HOSTS`] always carries the
//! Anthropic hosts so the agent's primary path is never intercepted and needs
//! no guest CA configuration.

use microsandbox::SecretSource;
use microsandbox::sandbox::SecretBuilder;

use crate::error::SandboxError;

/// Hosts that are never TLS-intercepted: the Anthropic API paths Claude Code
/// talks to. Maintained, security-relevant inventory (ADR-0003) — review on
/// change.
pub const TLS_BYPASS_HOSTS: &[&str] = &["api.anthropic.com", "*.anthropic.com"];

/// The full TLS bypass list: Anthropic hosts plus caller-supplied extras,
/// deduplicated, order-preserved.
pub fn tls_bypass_list(extra: &[String]) -> Vec<String> {
    let mut list: Vec<String> = TLS_BYPASS_HOSTS.iter().map(|host| (*host).to_owned()).collect();
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
/// Carries *references* only. The value is resolved from the host
/// environment variable named by `source_env_var` at sandbox spawn and at
/// rotation apply; it never enters this struct, the sandbox's durable config,
/// or the guest (the guest sees `placeholder`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretBinding {
    /// Guest-visible environment variable; holds the placeholder.
    pub env_var: String,

    /// Host environment variable the real value is resolved from.
    pub source_env_var: String,

    /// Stable placeholder string the guest sees. Survives rotation.
    pub placeholder: String,

    /// Hosts allowed to receive the substituted value. Exact hosts
    /// (`"api.example.com"`) or suffix wildcards (`"*.example.com"`).
    pub allowed_hosts: Vec<String>,

    /// Opt in to query-parameter substitution (headers and basic-auth are on
    /// by default; body injection is never enabled).
    pub inject_query: bool,
}

impl SecretBinding {
    /// A binding with the SDK-default placeholder and no allowed hosts.
    pub fn new(env_var: impl Into<String>, source_env_var: impl Into<String>) -> Self {
        let env_var = env_var.into();
        Self {
            placeholder: default_placeholder(&env_var),
            env_var,
            source_env_var: source_env_var.into(),
            allowed_hosts: Vec::new(),
            inject_query: false,
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

    /// Validate only the identity half of the binding (rotation needs no
    /// hosts or placeholder — those are deliberately left untouched so the
    /// placeholder stays stable and the change classifies as `Rotated`).
    pub(crate) fn validate_ref(&self) -> Result<(), SandboxError> {
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
}

/// How a secret rotation takes effect, mirroring the SDK's modification
/// disposition for the secret change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationDisposition {
    /// Applied to the running sandbox without a restart (the runtime
    /// advertised the `secrets_update` control capability).
    Live,

    /// Persisted; takes effect the next time the sandbox starts.
    NextStart,

    /// The change requires a restart, which `apply()` performs.
    RequiresRestart,
}

/// The outcome of a secret rotation on a sandbox.
#[derive(Debug, Clone)]
pub struct SecretRotation {
    /// How the rotation took effect.
    pub disposition: RotationDisposition,

    /// Non-fatal planner/apply warnings, rendered as `field: message`.
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use microsandbox::HostPattern;

    use super::*;

    fn valid_binding() -> SecretBinding {
        let mut binding = SecretBinding::new("KEY", "HOST_KEY");
        binding.allowed_hosts = vec!["api.example.com".to_owned()];
        binding
    }

    #[test]
    fn anthropic_hosts_are_always_bypassed() {
        let list = tls_bypass_list(&[]);
        assert!(list.contains(&"api.anthropic.com".to_owned()));
        assert!(list.contains(&"*.anthropic.com".to_owned()));
    }

    #[test]
    fn bypass_list_appends_extras_without_duplicates() {
        let list = tls_bypass_list(&[
            "internal.corp".to_owned(),
            "api.anthropic.com".to_owned(),
        ]);
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
        let mut binding = SecretBinding::new("RIGHT_PROVIDER_KEY", "HOST_KEY_VAR");
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

    #[test]
    fn rotation_refs_need_only_the_identity_half() {
        let minimal = SecretBinding::new("KEY", "HOST_KEY");
        assert!(minimal.validate_ref().is_ok());
        assert!(minimal.validate().is_err(), "no hosts: not creatable");
    }
}
