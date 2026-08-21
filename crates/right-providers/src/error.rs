//! The store error taxonomy. Every variant is terminal for the operation that
//! produced it — nothing in this crate swallows an error.

// `thiserror`'s `backtrace` field needs the unstable
// `error_generic_member_access` feature, so error chains are preserved by
// `#[source]` and rendered with `format!("{:#}", e)` instead.

/// Every failure mode of [`crate::ProviderStore`].
///
/// The variant names are load-bearing: the internal API maps them onto the
/// HTTP codes the dashboard already depends on (409 `borrowed_read_only`,
/// 422 `source_credential_unreadable`, and so on).
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("provider \"{name}\" not found")]
    NotFound { name: String },

    #[error("provider \"{name}\" already exists for this agent")]
    NameCollision { name: String },

    #[error("environment variable \"{env_var}\" is already used by another provider")]
    EnvVarCollision { env_var: String },

    #[error("invalid provider name \"{name}\": {reason}")]
    InvalidName { name: String, reason: String },

    #[error("invalid environment variable name \"{env_var}\"")]
    InvalidEnvVar { env_var: String },

    #[error("unknown built-in provider type \"{slug}\"")]
    UnknownBuiltinSlug { slug: String },

    #[error("provider \"{name}\" is borrowed from agent \"{owner}\" and is read-only here")]
    BorrowedReadOnly { name: String, owner: String },

    /// A share/unshare request that cannot be satisfied as stated (sharing
    /// into self, destination already declares the record, unsharing an owned
    /// record). Maps to 409 `copy_conflict`.
    #[error("{reason}")]
    ShareConflict { reason: String },

    /// Changing a generic provider's env var is a credential-rebinding
    /// operation, not a config edit. Maps to 400
    /// `generic_env_var_change_requires_credential`.
    #[error("changing the environment variable of \"{name}\" requires a new credential")]
    GenericEnvVarChangeRequiresCredential { name: String },

    /// The stored credential is empty or the redaction sentinel, so it can
    /// never be resolved into a source-ref binding. Maps to 422.
    #[error("credential for provider \"{source_provider}\" is unreadable")]
    SourceCredentialUnreadable { source_provider: String },

    #[error("providers.db: {source}")]
    Db {
        #[from]
        source: right_db::DbError,
    },

    #[error("providers.db at {}: {reason}", path.display())]
    Storage {
        path: std::path::PathBuf,
        reason: String,
    },
}

impl StoreError {
    pub(crate) fn storage(path: impl Into<std::path::PathBuf>, reason: impl Into<String>) -> Self {
        Self::Storage {
            path: path.into(),
            reason: reason.into(),
        }
    }
}
