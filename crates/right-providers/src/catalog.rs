//! The built-in provider catalog, as compile-time constants.
//!
//! This is the port of `right_openshell::providers::profile_catalog()` plus
//! the hidden-set derivation that used to live in
//! `right_openshell::managed_profiles::ManagedProfile::base_id()`. Both are
//! gone in the microsandbox world; the catalog is now data, not a gateway
//! round-trip.
//!
//! Byte-compat: the `category` string surfaced by `/provider-types` is
//! `ProviderCategory::as_str`, which reproduces the old
//! `format!("{:?}", category).to_lowercase()` rendering exactly.

/// Built-in provider category. Surfaced lowercase in the `/provider-types`
/// response.
///
/// The variant set mirrors the retired `right_openshell::providers::
/// ProviderCategory` because the rendered strings are a dashboard contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCategory {
    Inference,
    Agent,
    SourceControl,
    Messaging,
    Other,
}

impl ProviderCategory {
    /// The wire string. Identical to the old
    /// `format!("{:?}", category).to_lowercase()`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inference => "inference",
            Self::Agent => "agent",
            Self::SourceControl => "sourcecontrol",
            Self::Messaging => "messaging",
            Self::Other => "other",
        }
    }
}

impl std::fmt::Display for ProviderCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A built-in catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinProvider {
    /// Type slug as it appears in `agent.yaml` and on the wire.
    pub slug: &'static str,
    /// Environment variable the credential is bound to inside the sandbox.
    /// Empty only for the `generic` escape hatch, whose env var is per-record.
    pub env_var: &'static str,
    pub display_name: &'static str,
    pub category: ProviderCategory,
    /// Superseded built-ins stay in the catalog so existing records still
    /// resolve their env var, but are not offered as a new provider type.
    pub hidden: bool,
    /// Hosts allowed to receive the substituted credential. Non-empty for
    /// every real provider: `right_sandbox::SecretBinding` rejects a binding
    /// with no allowed hosts, because it could never substitute.
    pub allowed_hosts: &'static [&'static str],
    /// Opt in to query-parameter injection. Headers and basic-auth are on by
    /// default; body injection is never enabled (design decision "Injection").
    pub query_injection: bool,
}

/// The type slug reserved for the in-sandbox Claude Code login flow. Never in
/// the catalog, always rejected by [`crate::validate::validate_type_slug`].
pub const RESERVED_TYPE_SLUG: &str = "claude";

/// The escape-hatch slug whose endpoints come from the record, not the catalog.
pub const GENERIC_SLUG: &str = "generic";

/// The built-in catalog.
///
/// Ported one-for-one from `profile_catalog()` — same slugs, env vars, display
/// names, and categories, in the same order. `github` is hidden because
/// `right-github` supersedes it (the old `ManagedProfile::Github::base_id()`
/// derivation); nothing else is hidden. `claude` is absent by design.
pub const BUILTIN_CATALOG: &[BuiltinProvider] = &[
    BuiltinProvider {
        slug: "anthropic",
        env_var: "ANTHROPIC_API_KEY",
        display_name: "Anthropic API",
        category: ProviderCategory::Inference,
        hidden: false,
        allowed_hosts: &["api.anthropic.com"],
        query_injection: false,
    },
    BuiltinProvider {
        slug: "openai",
        env_var: "OPENAI_API_KEY",
        display_name: "OpenAI",
        category: ProviderCategory::Inference,
        hidden: false,
        allowed_hosts: &["api.openai.com"],
        query_injection: false,
    },
    BuiltinProvider {
        slug: "nvidia",
        env_var: "NVIDIA_API_KEY",
        display_name: "NVIDIA",
        category: ProviderCategory::Inference,
        hidden: false,
        allowed_hosts: &["integrate.api.nvidia.com"],
        query_injection: false,
    },
    BuiltinProvider {
        slug: "codex",
        env_var: "OPENAI_API_KEY",
        display_name: "Codex",
        category: ProviderCategory::Agent,
        hidden: false,
        allowed_hosts: &["api.openai.com", "chatgpt.com"],
        query_injection: false,
    },
    BuiltinProvider {
        slug: "copilot",
        env_var: "COPILOT_GITHUB_TOKEN",
        display_name: "GitHub Copilot",
        category: ProviderCategory::Agent,
        hidden: false,
        allowed_hosts: &["api.github.com", "api.githubcopilot.com"],
        query_injection: false,
    },
    BuiltinProvider {
        slug: "opencode",
        env_var: "OPENCODE_API_KEY",
        display_name: "OpenCode",
        category: ProviderCategory::Agent,
        hidden: false,
        allowed_hosts: &["opencode.ai", "api.opencode.ai"],
        query_injection: false,
    },
    BuiltinProvider {
        slug: "github",
        env_var: "GITHUB_TOKEN",
        display_name: "GitHub",
        category: ProviderCategory::SourceControl,
        // Superseded by `right-github`, which opens every HTTP method (the old
        // `access: "full"` derivation). Kept so existing records resolve.
        hidden: true,
        allowed_hosts: &["api.github.com", "github.com"],
        query_injection: false,
    },
    BuiltinProvider {
        slug: "right-github",
        env_var: "GITHUB_TOKEN",
        display_name: "GitHub",
        category: ProviderCategory::SourceControl,
        hidden: false,
        allowed_hosts: &["api.github.com", "github.com"],
        query_injection: false,
    },
    BuiltinProvider {
        slug: "right-fal",
        env_var: "FAL_KEY",
        display_name: "fal.ai",
        category: ProviderCategory::Other,
        hidden: false,
        // Authenticated API/control-plane hosts only. Media/CDN and
        // upload-target hosts are intentionally excluded from credential
        // injection (ported from the retired `fal_profile()`).
        allowed_hosts: &["fal.run", "queue.fal.run", "rest.fal.ai"],
        query_injection: false,
    },
    BuiltinProvider {
        slug: "gitlab",
        env_var: "GITLAB_TOKEN",
        display_name: "GitLab",
        category: ProviderCategory::SourceControl,
        hidden: false,
        allowed_hosts: &["gitlab.com"],
        query_injection: false,
    },
    BuiltinProvider {
        slug: GENERIC_SLUG,
        env_var: "",
        display_name: "Generic",
        category: ProviderCategory::Other,
        hidden: false,
        // Endpoints come from the record's `GenericSpec`, never from here.
        allowed_hosts: &[],
        query_injection: false,
    },
];

/// The whole catalog, hidden entries included.
pub fn catalog() -> &'static [BuiltinProvider] {
    BUILTIN_CATALOG
}

/// The catalog minus superseded entries — what `/provider-types` offers.
pub fn offered_catalog() -> Vec<&'static BuiltinProvider> {
    BUILTIN_CATALOG.iter().filter(|p| !p.hidden).collect()
}

/// Look up a built-in by slug.
pub fn builtin(slug: &str) -> Option<&'static BuiltinProvider> {
    BUILTIN_CATALOG.iter().find(|p| p.slug == slug)
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
