//! Record types. Nothing here carries a credential value across a read API.

use secrecy::SecretString;

use crate::catalog::{self, BuiltinProvider};
use crate::error::StoreError;

/// The sentinel a redacted credential reads back as.
///
/// Preserved from the OpenShell era: any host-side path that ever obtains a
/// credential string must reject this value (and the empty string) rather than
/// write it anywhere. [`crate::ProviderStore::source_ref_binding`] is the only
/// reader, and it enforces exactly that.
pub const REDACTION_SENTINEL: &str = "REDACTED";

/// Reject a credential that is empty or already redacted.
///
/// Defense in depth for the one code path that reads a stored credential.
pub(crate) fn check_source_credential_readable(
    value: &str,
    source_provider: &str,
) -> Result<(), StoreError> {
    if value.is_empty() || value == REDACTION_SENTINEL {
        return Err(StoreError::SourceCredentialUnreadable {
            source_provider: source_provider.to_string(),
        });
    }
    Ok(())
}

/// Generic-provider endpoints — the escape hatch for anything not in the
/// built-in catalog.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GenericSpec {
    pub env_var: String,
    /// Normalized and deduplicated by [`crate::validate::normalize_generic_hosts`].
    pub upstream_hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_path_prefix: Option<String>,
}

/// A provider's type: a built-in catalog slug, or a generic definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    Builtin(String),
    Generic(GenericSpec),
}

impl ProviderKind {
    /// The type slug as it appears on the wire (`"generic"` for generics).
    pub fn slug(&self) -> &str {
        match self {
            Self::Builtin(slug) => slug,
            Self::Generic(_) => catalog::GENERIC_SLUG,
        }
    }

    pub fn generic(&self) -> Option<&GenericSpec> {
        match self {
            Self::Builtin(_) => None,
            Self::Generic(spec) => Some(spec),
        }
    }

    /// The catalog entry backing a built-in kind.
    pub(crate) fn builtin_entry(&self) -> Option<&'static BuiltinProvider> {
        match self {
            Self::Builtin(slug) => catalog::builtin(slug),
            Self::Generic(_) => None,
        }
    }

    /// Resolve the environment variable this kind binds to.
    ///
    /// Fails loudly on a built-in slug that is no longer in the catalog: an
    /// empty key would silently break rotation and secret substitution.
    pub fn env_var(&self) -> Result<&str, StoreError> {
        match self {
            Self::Builtin(slug) => catalog::builtin(slug)
                .map(|p| p.env_var)
                .ok_or_else(|| StoreError::UnknownBuiltinSlug { slug: slug.clone() }),
            Self::Generic(spec) => Ok(&spec.env_var),
        }
    }

    /// Hosts allowed to receive the substituted credential.
    pub fn allowed_hosts(&self) -> Result<Vec<String>, StoreError> {
        match self {
            Self::Builtin(slug) => catalog::builtin(slug)
                .map(|p| p.allowed_hosts.iter().map(|h| (*h).to_owned()).collect())
                .ok_or_else(|| StoreError::UnknownBuiltinSlug { slug: slug.clone() }),
            Self::Generic(spec) => Ok(spec.upstream_hosts.clone()),
        }
    }
}

/// Tri-state provider health. Replaces the OpenShell-era `composed` flag and
/// the four-variant gateway status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderStatus {
    /// A credential is stored and the type resolves.
    Ready,
    /// The record exists but carries no usable credential yet.
    NeedsValue,
    /// The record cannot be used: its built-in slug is no longer in the
    /// catalog, or its definition is otherwise unresolvable.
    Error,
}

/// A stored provider record, as returned by every read API.
///
/// There is deliberately no credential field: the value never leaves the
/// database except through [`crate::ProviderStore::source_ref_binding`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRecord {
    /// Record name, unique per holding agent.
    pub name: String,
    /// The agent that owns the credential.
    pub owner_agent: String,
    pub kind: ProviderKind,
    pub label: String,
    /// Environment variable the credential binds to inside the sandbox.
    pub env_var: String,
    /// Unix seconds.
    pub updated_at: i64,
    /// The borrowing agent when this row is a borrowed reference; `None` on
    /// the owning row.
    pub borrower_agent: Option<String>,
    pub status: ProviderStatus,
}

impl ProviderRecord {
    /// The agent that declares this record — the borrower if borrowed, else
    /// the owner.
    pub fn holder_agent(&self) -> &str {
        self.borrower_agent.as_deref().unwrap_or(&self.owner_agent)
    }

    pub fn is_borrowed(&self) -> bool {
        self.borrower_agent.is_some()
    }

    pub fn is_owned(&self) -> bool {
        self.borrower_agent.is_none()
    }
}

/// Input for [`crate::ProviderStore::create`].
#[derive(Debug, Clone)]
pub struct NewProvider {
    pub owner_agent: String,
    pub name: String,
    pub kind: ProviderKind,
    pub label: String,
}

/// A credential value in transport. Write-only: there is no accessor that
/// hands the value to a caller, and `Debug` never renders it.
#[derive(Clone)]
pub struct Credential(SecretString);

impl Credential {
    pub fn new(value: SecretString) -> Self {
        Self(value)
    }

    /// The value, for the single call site that writes it into the database.
    pub(crate) fn expose(&self) -> &str {
        use secrecy::ExposeSecret as _;
        self.0.expose_secret()
    }
}

impl From<SecretString> for Credential {
    fn from(value: SecretString) -> Self {
        Self::new(value)
    }
}

impl From<String> for Credential {
    fn from(value: String) -> Self {
        Self::new(SecretString::from(value))
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credential(<redacted>)")
    }
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
