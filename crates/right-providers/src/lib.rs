//! Right's provider credential store.
//!
//! Replaces the OpenShell provider gateway. Credentials live in
//! `~/.right/providers.db` (SQLite, mode 0600) and reach a sandbox through a
//! redacted binding and the SDK's scoped in-process resolver. Durable sandbox
//! config stores only an owner-scoped source identity and placeholder.
//!
//! Three rules hold everywhere in this crate:
//!
//! - **No credential value crosses a read API.** [`ProviderRecord`] has no
//!   credential field, [`Credential`] has no public accessor, and its `Debug`
//!   redacts. [`ProviderStore::source_ref_binding`] is the single reader.
//! - **Ownership is a column.** `owner_agent` plus `borrower_agent` replace the
//!   derived `shared_from` field in `agent.yaml`. A re-share points at the true
//!   owner; a borrowed record is read-only for its borrower.
//! - **Fail fast.** Every fallible step returns [`StoreError`]; nothing is
//!   logged and swallowed.

#![warn(unreachable_pub)]

pub mod catalog;
mod error;
pub mod plan;
mod record;
mod store;
pub mod validate;

pub use catalog::{
    BUILTIN_CATALOG, BuiltinProvider, GENERIC_SLUG, ProviderCategory, RESERVED_TYPE_SLUG,
};
pub use error::StoreError;
pub use plan::{DestroyProviderPlan, HeldProvider, plan_destroy_provider_cascade};
pub use record::{
    Credential, GenericSpec, NewProvider, ProviderHolder, ProviderKind, ProviderRecord,
    ProviderStatus, REDACTION_SENTINEL,
};
pub use store::{AgentGuard, PROVIDERS_DB_FILE, ProviderStore, is_source_identity, source_env_var};
pub use validate::{new_record_name, validate_name, validate_type_slug};
