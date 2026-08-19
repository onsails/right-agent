//! `providers.db` — Right's own provider credential store.
//!
//! One SQLite database at `~/.right/providers.db`, mode 0600, opened through
//! `right-db` (the only crate that owns driver details). It is deliberately
//! separate from the per-agent `data.db`: credentials are cross-agent state,
//! and revocation must be one delete in one place (ADR 0002).
//!
//! # Row model
//!
//! Two tables, so "a borrowed row carries no credential" is structural rather
//! than advisory:
//!
//! - `providers` — one row per owned record, keyed `(owner_agent, name)`.
//!   Holds the definition and the credential.
//! - `provider_borrows` — one row per borrowed reference, keyed
//!   `(borrower_agent, name)`, pointing at the owning row. A re-share always
//!   points at the *true* owner, never at the intermediary.
//!
//! Ownership is therefore a column, not a derived `shared_from` field in
//! `agent.yaml`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use right_db::Connection;
use right_db::params;

use crate::catalog::{self, BuiltinProvider, GENERIC_SLUG};
use crate::error::StoreError;
use crate::plan::{self, HeldProvider};
use crate::record::{
    Credential, GenericSpec, NewProvider, ProviderKind, ProviderRecord, ProviderStatus,
    check_source_credential_readable,
};
use crate::validate;

/// The database file name under Right's home directory.
pub const PROVIDERS_DB_FILE: &str = "providers.db";

/// `providers.db` is owner-only: it holds plaintext credentials.
#[cfg(unix)]
const DB_FILE_MODE: u32 = 0o600;

/// Current schema version, tracked in `PRAGMA user_version`.
const SCHEMA_VERSION: u32 = 1;

/// `kind` column discriminants.
const KIND_BUILTIN: &str = "builtin";
const KIND_GENERIC: &str = "generic";

/// Prefix of the host environment variable a source-ref secret resolves from.
const SOURCE_ENV_PREFIX: &str = "RIGHT_PROVIDER_";

/// Columns of an owning row, in the order [`owned_row`] decodes them. The
/// has_credential expression embeds the single REDACTION_SENTINEL constant —
/// never a second literal — so a sentinel change cannot split the store's own
/// definition of "credential present".
static OWNED_COLUMNS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "owner_agent, name, kind, builtin_slug, env_var, label, \
         upstream_hosts, upstream_path_prefix, updated_at, \
         (credential IS NOT NULL AND credential <> '' AND credential <> '{}')",
        crate::record::REDACTION_SENTINEL
    )
});

/// The host environment variable a record's credential is resolved from at
/// sandbox spawn.
///
/// Deterministic in the record name so the spawning process and the sandbox
/// spec agree without persisting anything secret.
pub fn source_env_var(name: &str) -> String {
    let mut var = String::with_capacity(SOURCE_ENV_PREFIX.len() + name.len());
    var.push_str(SOURCE_ENV_PREFIX);
    for c in name.chars() {
        var.push(if c.is_ascii_alphanumeric() {
            c.to_ascii_uppercase()
        } else {
            '_'
        });
    }
    var
}

/// A decoded owning row, before the borrower overlay is applied.
struct OwnedRow {
    owner_agent: String,
    name: String,
    kind: ProviderKind,
    /// The `builtin_slug`/`env_var` pair as stored, kept so a record whose
    /// slug has left the catalog still reports the env var it was created
    /// with instead of an empty string.
    stored_env_var: String,
    label: String,
    updated_at: i64,
    has_credential: bool,
}

fn owned_row(row: &right_db::Row<'_>) -> Result<OwnedRow, right_db::DbError> {
    let owner_agent: String = row.get(0)?;
    let name: String = row.get(1)?;
    let kind_tag: String = row.get(2)?;
    let builtin_slug: Option<String> = row.get(3)?;
    let stored_env_var: String = row.get(4)?;
    let label: String = row.get(5)?;
    let upstream_hosts: String = row.get(6)?;
    let upstream_path_prefix: Option<String> = row.get(7)?;
    let updated_at: i64 = row.get(8)?;
    let has_credential: bool = row.get(9)?;

    let kind = match kind_tag.as_str() {
        KIND_BUILTIN => ProviderKind::Builtin(builtin_slug.ok_or_else(|| {
            right_db::DbError::InvalidParameter(format!(
                "provider \"{name}\": kind=builtin with NULL builtin_slug"
            ))
        })?),
        KIND_GENERIC => {
            let hosts: Vec<String> = serde_json::from_str(&upstream_hosts).map_err(|e| {
                right_db::DbError::InvalidParameter(format!(
                    "provider \"{name}\": upstream_hosts is not a JSON array: {e}"
                ))
            })?;
            ProviderKind::Generic(GenericSpec {
                env_var: stored_env_var.clone(),
                upstream_hosts: hosts,
                upstream_path_prefix,
            })
        }
        other => {
            return Err(right_db::DbError::InvalidParameter(format!(
                "provider \"{name}\": unknown kind \"{other}\""
            )));
        }
    };

    Ok(OwnedRow {
        owner_agent,
        name,
        kind,
        stored_env_var,
        label,
        updated_at,
        has_credential,
    })
}

impl OwnedRow {
    /// Build the public record for the agent holding this row.
    ///
    /// A built-in's env var is re-resolved through the catalog so a catalog
    /// correction reaches existing records; a slug that has left the catalog
    /// degrades to [`ProviderStatus::Error`] with the stored env var rather
    /// than failing the whole list.
    fn into_record(self, borrower_agent: Option<String>) -> ProviderRecord {
        let (env_var, resolves) = match self.kind.env_var() {
            Ok(env_var) => (env_var.to_owned(), true),
            Err(_) => (self.stored_env_var.clone(), false),
        };
        let status = if !resolves {
            ProviderStatus::Error
        } else if self.has_credential {
            ProviderStatus::Ready
        } else {
            ProviderStatus::NeedsValue
        };
        ProviderRecord {
            name: self.name,
            owner_agent: self.owner_agent,
            kind: self.kind,
            label: self.label,
            env_var,
            updated_at: self.updated_at,
            borrower_agent,
            status,
        }
    }
}

/// Right's provider credential store.
pub struct ProviderStore {
    conn: Connection,
    db_path: PathBuf,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl std::fmt::Debug for ProviderStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderStore")
            .field("db_path", &self.db_path)
            .finish_non_exhaustive()
    }
}
/// A held per-agent lock. Dropping it releases the critical section.
pub struct AgentGuard(tokio::sync::OwnedMutexGuard<()>);

impl std::fmt::Debug for AgentGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AgentGuard")
    }
}

/// Current wall-clock seconds since the unix epoch.
///
/// A clock fault is a host condition, not client input, so it maps to
/// `Storage` (HTTP 500), never to a 400-class validation variant.
fn now_unix() -> Result<i64, StoreError> {
    let clock_fault = |reason: String| StoreError::Storage {
        path: std::path::PathBuf::from("<system clock>"),
        reason,
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| clock_fault(format!("system clock is before the unix epoch: {e}")))?
        .as_secs();
    i64::try_from(secs)
        .map_err(|_| clock_fault("system clock is beyond the representable range".into()))
}

/// Idempotent schema. Every statement is `IF NOT EXISTS`, so re-running the
/// migration on an already-current database is a no-op.
const V1_SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS providers (
    owner_agent          TEXT    NOT NULL,
    name                 TEXT    NOT NULL,
    kind                 TEXT    NOT NULL,
    builtin_slug         TEXT,
    env_var              TEXT    NOT NULL,
    label                TEXT    NOT NULL,
    upstream_hosts       TEXT    NOT NULL DEFAULT '[]',
    upstream_path_prefix TEXT,
    credential           TEXT    NOT NULL DEFAULT '',
    updated_at           INTEGER NOT NULL,
    PRIMARY KEY (owner_agent, name)
);
CREATE TABLE IF NOT EXISTS provider_borrows (
    borrower_agent TEXT    NOT NULL,
    name           TEXT    NOT NULL,
    owner_agent    TEXT    NOT NULL,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (borrower_agent, name)
);
CREATE INDEX IF NOT EXISTS idx_provider_borrows_owner
    ON provider_borrows (owner_agent, name);
";

impl ProviderStore {
    /// Open (creating if absent) `<home>/providers.db` and bring its schema
    /// up to date.
    pub async fn open(home: &Path) -> Result<Self, StoreError> {
        std::fs::create_dir_all(home)
            .map_err(|e| StoreError::storage(home, format!("create home directory: {e}")))?;
        let db_path = home.join(PROVIDERS_DB_FILE);
        let conn = right_db::open_database_path(&db_path).await?;
        migrate(&conn).await?;
        restrict_permissions(&db_path)?;
        Ok(Self {
            conn,
            db_path,
            locks: Mutex::new(HashMap::new()),
        })
    }

    /// The database file backing this store.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Take the per-agent lock.
    ///
    /// Ported from the internal API's `provider_lock`: a caller that performs
    /// a multi-step flow (validate, mutate the store, rewrite `agent.yaml`)
    /// holds this for the whole flow. Callers validate the record name
    /// *before* taking the lock, and a share locks only the destination
    /// agent.
    ///
    /// The store's own methods never take this lock — their atomicity comes
    /// from immediate transactions — so holding a guard across a store call
    /// cannot deadlock.
    pub async fn agent_lock(&self, agent: &str) -> AgentGuard {
        let mutex = {
            let mut locks = self
                .locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(locks.entry(agent.to_owned()).or_default())
        };
        AgentGuard(mutex.lock_owned().await)
    }

    /// The whole built-in catalog, hidden entries included.
    pub fn catalog() -> &'static [BuiltinProvider] {
        catalog::catalog()
    }

    /// The catalog minus superseded entries — what `/provider-types` offers.
    pub fn offered_catalog() -> Vec<&'static BuiltinProvider> {
        catalog::offered_catalog()
    }

    /// See [`crate::validate::validate_name`].
    pub fn validate_name(agent: &str, name: &str) -> Result<(), StoreError> {
        validate::validate_name(agent, name)
    }

    /// See [`crate::validate::validate_type_slug`].
    pub fn validate_type_slug(slug: &str) -> Result<(), StoreError> {
        validate::validate_type_slug(slug)
    }
}

/// Apply the schema inside one immediate transaction, then commit.
async fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let current: i64 = conn
        .query_one("PRAGMA user_version", (), |row| row.get(0))
        .await?;
    let current = u32::try_from(current).map_err(|_| {
        StoreError::storage(conn.path(), format!("negative user_version {current}"))
    })?;
    if current > SCHEMA_VERSION {
        return Err(StoreError::storage(
            conn.path(),
            format!("schema version {current} is newer than this build ({SCHEMA_VERSION})"),
        ));
    }
    if current == SCHEMA_VERSION {
        return Ok(());
    }

    let tx = conn.transaction().await?;
    let applied = async {
        tx.execute_batch(V1_SCHEMA).await?;
        tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
            .await
    }
    .await;
    match applied {
        Ok(()) => {
            tx.commit().await?;
            Ok(())
        }
        Err(source) => {
            if let Err(rollback) = tx.rollback().await {
                tracing::warn!(
                    path = %conn.path().display(),
                    operation_error = format!("{source:#}"),
                    rollback_error = format!("{rollback:#}"),
                    "providers.db migration rollback failed; returning original error",
                );
            }
            Err(source.into())
        }
    }
}

/// Narrow the database and its WAL sidecars to owner-only.
///
/// The file holds plaintext credentials; anything wider is a disclosure.
fn restrict_permissions(db_path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for suffix in ["", "-wal", "-shm", "-tshm"] {
            let path = if suffix.is_empty() {
                db_path.to_path_buf()
            } else {
                let mut name = db_path.as_os_str().to_owned();
                name.push(suffix);
                PathBuf::from(name)
            };
            match std::fs::metadata(&path) {
                Ok(_) => {
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(DB_FILE_MODE))
                        .map_err(|e| {
                            StoreError::storage(&path, format!("chmod {DB_FILE_MODE:o}: {e}"))
                        })?;
                }
                // A sidecar that does not exist yet needs no narrowing; the
                // next open re-runs this once it does.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(StoreError::storage(&path, format!("stat: {e}"))),
            }
        }
    }
    #[cfg(not(unix))]
    let _ = db_path;
    Ok(())
}

/// Fetch the owning row for `(owner, name)`, if it exists.
async fn owned_row_for(
    conn: &Connection,
    owner: &str,
    name: &str,
) -> Result<Option<OwnedRow>, StoreError> {
    let sql = format!(
        "SELECT {} FROM providers WHERE owner_agent = ?1 AND name = ?2",
        OWNED_COLUMNS.as_str()
    );
    let mut rows = conn
        .query_all(&sql, params![owner, name], owned_row)
        .await?;
    Ok(rows.pop())
}

/// The owner a borrowed reference points at, if `agent` borrows `name`.
async fn borrow_owner(
    conn: &Connection,
    borrower: &str,
    name: &str,
) -> Result<Option<String>, StoreError> {
    let mut rows = conn
        .query_all(
            "SELECT owner_agent FROM provider_borrows WHERE borrower_agent = ?1 AND name = ?2",
            params![borrower, name],
            |row| row.get(0),
        )
        .await?;
    Ok(rows.pop())
}

/// An owning row plus the borrower overlay for the agent that asked.
struct Resolved {
    row: OwnedRow,
    borrower: Option<String>,
}

/// Resolve `name` as seen by `agent`, following a borrow to the owning row.
async fn resolve(conn: &Connection, agent: &str, name: &str) -> Result<Resolved, StoreError> {
    if let Some(row) = owned_row_for(conn, agent, name).await? {
        return Ok(Resolved {
            row,
            borrower: None,
        });
    }
    let Some(owner) = borrow_owner(conn, agent, name).await? else {
        return Err(StoreError::NotFound { name: name.into() });
    };
    // A borrow row without its owning row is corruption, not a miss: the
    // borrower would otherwise silently lose a credential it still declares.
    let row = owned_row_for(conn, &owner, name).await?.ok_or_else(|| {
        StoreError::storage(
            conn.path(),
            format!(
                "agent \"{agent}\" borrows \"{name}\" from \"{owner}\", which has no such record"
            ),
        )
    })?;
    Ok(Resolved {
        row,
        borrower: Some(agent.to_owned()),
    })
}

/// Every record `agent` declares, owned and borrowed, name + true owner only.
async fn held_providers(conn: &Connection, agent: &str) -> Result<Vec<HeldProvider>, StoreError> {
    let mut held = conn
        .query_all(
            "SELECT name, owner_agent FROM providers WHERE owner_agent = ?1
             UNION ALL
             SELECT name, owner_agent FROM provider_borrows WHERE borrower_agent = ?1
             ORDER BY 1",
            params![agent],
            |row| {
                Ok(HeldProvider::new(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                ))
            },
        )
        .await?;
    held.dedup_by(|a, b| a.name == b.name);
    Ok(held)
}

impl ProviderStore {
    /// One record as `agent` sees it. Never carries a credential value.
    pub async fn get(&self, agent: &str, name: &str) -> Result<ProviderRecord, StoreError> {
        let resolved = resolve(&self.conn, agent, name).await?;
        Ok(resolved.row.into_record(resolved.borrower))
    }

    /// Every record `agent` declares, owned first then borrowed, each sorted
    /// by name. Never carries a credential value.
    pub async fn list(&self, agent: &str) -> Result<Vec<ProviderRecord>, StoreError> {
        let owned_sql = format!(
            "SELECT {} FROM providers WHERE owner_agent = ?1 ORDER BY name",
            OWNED_COLUMNS.as_str()
        );
        let mut records: Vec<ProviderRecord> = self
            .conn
            .query_all(&owned_sql, params![agent], owned_row)
            .await?
            .into_iter()
            .map(|row| row.into_record(None))
            .collect();

        // Correlated EXISTS rather than a join so every column in
        // `OWNED_COLUMNS` stays unambiguously a `providers` column.
        let borrowed_sql = format!(
            "SELECT {} FROM providers
             WHERE EXISTS (
                 SELECT 1 FROM provider_borrows
                 WHERE provider_borrows.borrower_agent = ?1
                   AND provider_borrows.owner_agent = providers.owner_agent
                   AND provider_borrows.name = providers.name
             )
             ORDER BY providers.name",
            OWNED_COLUMNS.as_str()
        );
        records.extend(
            self.conn
                .query_all(&borrowed_sql, params![agent], owned_row)
                .await?
                .into_iter()
                .map(|row| row.into_record(Some(agent.to_owned()))),
        );
        Ok(records)
    }
}

/// Validate a kind and return the environment variable it binds to plus the
/// column values a row needs.
struct KindColumns {
    tag: &'static str,
    builtin_slug: Option<String>,
    env_var: String,
    upstream_hosts: String,
    upstream_path_prefix: Option<String>,
}

fn kind_columns(kind: &ProviderKind) -> Result<KindColumns, StoreError> {
    match kind {
        ProviderKind::Builtin(slug) => {
            validate::validate_type_slug(slug)?;
            if slug == GENERIC_SLUG {
                return Err(StoreError::InvalidName {
                    name: slug.clone(),
                    reason: "type \"generic\" requires a generic definition".into(),
                });
            }
            let env_var = kind.env_var()?.to_owned();
            Ok(KindColumns {
                tag: KIND_BUILTIN,
                builtin_slug: Some(slug.clone()),
                env_var,
                upstream_hosts: "[]".into(),
                upstream_path_prefix: None,
            })
        }
        ProviderKind::Generic(spec) => {
            let hosts = validate::validate_generic_request(
                &spec.env_var,
                None,
                Some(&spec.upstream_hosts),
                spec.upstream_path_prefix.as_deref(),
            )?;
            let upstream_hosts =
                serde_json::to_string(&hosts).map_err(|e| StoreError::InvalidName {
                    name: spec.env_var.clone(),
                    reason: format!("upstream_hosts are not serializable: {e}"),
                })?;
            Ok(KindColumns {
                tag: KIND_GENERIC,
                builtin_slug: None,
                env_var: spec.env_var.clone(),
                upstream_hosts,
                upstream_path_prefix: spec.upstream_path_prefix.clone(),
            })
        }
    }
}

/// The environment variables `agent` already binds, so a create can reject a
/// collision. Built-ins resolve through the catalog, exactly as reads do.
async fn bound_env_vars(conn: &Connection, agent: &str) -> Result<Vec<String>, StoreError> {
    let mut env_vars = Vec::new();
    for held in held_providers(conn, agent).await? {
        // A borrow row without its owning row is corruption (a dangling
        // reference), not a miss — resolve() fails loud on the same condition,
        // and a create's env-var collision check must not silently pass
        // against an env var the missing row already bound.
        let row = owned_row_for(conn, &held.owner_agent, &held.name)
            .await?
            .ok_or_else(|| {
                StoreError::storage(
                    conn.path(),
                    format!(
                        "agent \"{agent}\" holds \"{}\" from \"{}\", which has no such record",
                        held.name, held.owner_agent
                    ),
                )
            })?;
        let record = row.into_record(None);
        env_vars.push(record.env_var);
    }
    Ok(env_vars)
}

impl ProviderStore {
    /// Create an owned record and store its credential.
    ///
    /// The name is validated under the `{agent}-` prefix rules; mint one with
    /// [`crate::validate::new_record_name`]. Name and environment-variable
    /// collisions are checked against everything the agent already declares,
    /// borrowed records included, inside the same transaction as the insert.
    pub async fn create(
        &self,
        rec: NewProvider,
        cred: Credential,
    ) -> Result<ProviderRecord, StoreError> {
        validate::validate_name(&rec.owner_agent, &rec.name)?;
        // `""` is the no-label marker, not a label: the wire shape carries
        // `label: null` and the row stores `""`, so an absent dashboard label
        // must not trip the 1-32-char label validator.
        if !rec.label.is_empty() {
            validate::validate_label(&rec.label)?;
        }
        let columns = kind_columns(&rec.kind)?;
        let updated_at = now_unix()?;

        let tx = self.conn.transaction().await?;
        let outcome = async {
            let held = held_providers(&tx, &rec.owner_agent).await?;
            if held.iter().any(|p| p.name == rec.name) {
                return Err(StoreError::NameCollision {
                    name: rec.name.clone(),
                });
            }
            if bound_env_vars(&tx, &rec.owner_agent)
                .await?
                .iter()
                .any(|env_var| env_var == &columns.env_var)
            {
                return Err(StoreError::EnvVarCollision {
                    env_var: columns.env_var.clone(),
                });
            }
            tx.execute(
                "INSERT INTO providers
                     (owner_agent, name, kind, builtin_slug, env_var, label,
                      upstream_hosts, upstream_path_prefix, credential, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    &rec.owner_agent,
                    &rec.name,
                    columns.tag,
                    &columns.builtin_slug,
                    &columns.env_var,
                    &rec.label,
                    &columns.upstream_hosts,
                    &columns.upstream_path_prefix,
                    cred.expose(),
                    updated_at,
                ],
            )
            .await?;
            Ok(())
        }
        .await;
        commit_or_rollback(tx, outcome, "create").await?;

        self.get(&rec.owner_agent, &rec.name).await
    }

    /// Replace the credential of an owned record. Borrowed records are
    /// read-only: only the owner rotates.
    ///
    /// FAIL FAST on a record whose kind no longer resolves (e.g. a built-in
    /// slug that has left the catalog): the gateway-era handler surfaced this
    /// as `unknown_builtin_slug` before touching any state, and writing a new
    /// credential under an unresolvable definition would only hide the drift.
    pub async fn rotate(
        &self,
        agent: &str,
        name: &str,
        cred: Credential,
    ) -> Result<(), StoreError> {
        let resolved = resolve(&self.conn, agent, name).await?;
        reject_if_borrowed(&resolved)?;
        // Force resolution of the kind's env var so an unknown built-in slug
        // aborts here instead of after the credential write.
        resolved.row.kind.env_var()?;
        let updated_at = now_unix()?;
        let changed = self
            .conn
            .execute(
                "UPDATE providers SET credential = ?3, updated_at = ?4
                 WHERE owner_agent = ?1 AND name = ?2",
                params![agent, name, cred.expose(), updated_at],
            )
            .await?;
        if changed == 0 {
            return Err(StoreError::NotFound { name: name.into() });
        }
        Ok(())
    }

    /// Update a generic record's endpoints. Owner only, generic only, and the
    /// environment variable may not change — rebinding a credential to a new
    /// variable is a create, not a config edit.
    pub async fn update_generic(
        &self,
        agent: &str,
        name: &str,
        spec: GenericSpec,
    ) -> Result<(), StoreError> {
        let resolved = resolve(&self.conn, agent, name).await?;
        reject_if_borrowed(&resolved)?;
        let current = resolved
            .row
            .kind
            .generic()
            .ok_or_else(|| StoreError::InvalidName {
                name: name.into(),
                reason: "config-update only valid on generic providers".into(),
            })?;
        if spec.env_var != current.env_var {
            return Err(StoreError::GenericEnvVarChangeRequiresCredential { name: name.into() });
        }
        let columns = kind_columns(&ProviderKind::Generic(spec))?;
        let updated_at = now_unix()?;
        let changed = self
            .conn
            .execute(
                "UPDATE providers
                 SET upstream_hosts = ?3, upstream_path_prefix = ?4, updated_at = ?5
                 WHERE owner_agent = ?1 AND name = ?2",
                params![
                    agent,
                    name,
                    &columns.upstream_hosts,
                    &columns.upstream_path_prefix,
                    updated_at
                ],
            )
            .await?;
        if changed == 0 {
            return Err(StoreError::NotFound { name: name.into() });
        }
        Ok(())
    }
}

/// Reject any mutation aimed at a borrowed reference. The single chokepoint
/// that keeps a borrower from clobbering the owner's record.
fn reject_if_borrowed(resolved: &Resolved) -> Result<(), StoreError> {
    if resolved.borrower.is_some() {
        return Err(StoreError::BorrowedReadOnly {
            name: resolved.row.name.clone(),
            owner: resolved.row.owner_agent.clone(),
        });
    }
    Ok(())
}

/// Commit on success, roll back on failure, and always return the original
/// operation error — a failed rollback must never mask what actually broke.
async fn commit_or_rollback(
    tx: right_db::Transaction<'_>,
    outcome: Result<(), StoreError>,
    operation: &'static str,
) -> Result<(), StoreError> {
    match outcome {
        Ok(()) => {
            tx.commit().await?;
            Ok(())
        }
        Err(source) => {
            if let Err(rollback) = tx.rollback().await {
                tracing::warn!(
                    operation,
                    operation_error = format!("{source:#}"),
                    rollback_error = format!("{rollback:#}"),
                    "providers.db transaction rollback failed; returning original error",
                );
            }
            Err(source)
        }
    }
}

impl ProviderStore {
    /// Delete an owned record, cascading to its borrowers.
    ///
    /// With no borrowers the row is deleted outright. With borrowers the
    /// record is re-homed to the first survivor (deterministic by agent name)
    /// and that survivor's borrow row is dropped, so the credential stays
    /// reachable for everyone who still declares it and exactly one authority
    /// remains. A borrower cannot reach this path at all — see
    /// [`ProviderStore::unshare`].
    pub async fn remove(&self, agent: &str, name: &str) -> Result<(), StoreError> {
        let resolved = resolve(&self.conn, agent, name).await?;
        reject_if_borrowed(&resolved)?;

        let tx = self.conn.transaction().await?;
        let outcome = async {
            let borrowers: Vec<String> = tx
                .query_all(
                    "SELECT borrower_agent FROM provider_borrows
                     WHERE owner_agent = ?1 AND name = ?2 ORDER BY borrower_agent",
                    params![agent, name],
                    |row| row.get(0),
                )
                .await?;

            let Some(new_owner) = borrowers.first() else {
                let changed = tx
                    .execute(
                        "DELETE FROM providers WHERE owner_agent = ?1 AND name = ?2",
                        params![agent, name],
                    )
                    .await?;
                if changed == 0 {
                    return Err(StoreError::NotFound { name: name.into() });
                }
                return Ok(());
            };

            tx.execute(
                "DELETE FROM provider_borrows WHERE borrower_agent = ?1 AND name = ?2",
                params![new_owner, name],
            )
            .await?;
            let changed = tx
                .execute(
                    "UPDATE providers SET owner_agent = ?3
                     WHERE owner_agent = ?1 AND name = ?2",
                    params![agent, name, new_owner],
                )
                .await?;
            if changed == 0 {
                return Err(StoreError::NotFound { name: name.into() });
            }
            tx.execute(
                "UPDATE provider_borrows SET owner_agent = ?3
                 WHERE owner_agent = ?1 AND name = ?2",
                params![agent, name, new_owner],
            )
            .await?;
            Ok(())
        }
        .await;
        commit_or_rollback(tx, outcome, "remove").await
    }

    /// Attach an existing record to `dest_agent` as a borrowed reference.
    ///
    /// No credential is copied: the destination gets a pointer to the owning
    /// row. Re-sharing a borrowed record points the new borrower at the true
    /// owner, so rotation rights and the destroy cascade always resolve to one
    /// authority. Trust checks belong to the caller.
    pub async fn share(
        &self,
        owner_agent: &str,
        name: &str,
        dest_agent: &str,
    ) -> Result<ProviderRecord, StoreError> {
        validate::validate_name(dest_agent, name)?;
        let created_at = now_unix()?;

        let tx = self.conn.transaction().await?;
        let outcome = async {
            let resolved = resolve(&tx, owner_agent, name).await?;
            let true_owner = resolved.row.owner_agent.clone();
            let dest_held = held_providers(&tx, dest_agent).await?;
            plan::plan_share(&true_owner, dest_agent, name, &dest_held)?;
            tx.execute(
                "INSERT INTO provider_borrows (borrower_agent, name, owner_agent, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![dest_agent, name, &true_owner, created_at],
            )
            .await?;
            Ok(())
        }
        .await;
        commit_or_rollback(tx, outcome, "share").await?;

        self.get(dest_agent, name).await
    }

    /// Drop a borrowed reference. The record itself is never deleted here —
    /// an owned record must go through [`ProviderStore::remove`].
    pub async fn unshare(&self, borrower_agent: &str, name: &str) -> Result<(), StoreError> {
        let resolved = resolve(&self.conn, borrower_agent, name).await?;
        let held = HeldProvider::new(name, resolved.row.owner_agent.clone());
        plan::plan_unshare(borrower_agent, &held)?;
        let changed = self
            .conn
            .execute(
                "DELETE FROM provider_borrows WHERE borrower_agent = ?1 AND name = ?2",
                params![borrower_agent, name],
            )
            .await?;
        if changed == 0 {
            return Err(StoreError::NotFound { name: name.into() });
        }
        Ok(())
    }
}

/// Serializes every `set_var` this crate performs. `std::env` is process
/// global; this is the only writer in Right.
static ENV_PUBLISH: Mutex<()> = Mutex::new(());

/// Publish `value` under `var` in the current process environment.
///
/// # Why this exists
///
/// A microsandbox source-ref secret names a *host* environment variable and
/// the runtime reads it at spawn and at rotation apply. That is the only
/// mechanism upstream exposes, so the spawning process must carry the value —
/// there is no API that accepts it directly. Nothing is persisted: the
/// sandbox config stores the variable name and a placeholder, never the
/// secret.
///
/// # Safety
///
/// `std::env::set_var` is unsound if another thread reads the environment
/// concurrently. Writes are serialized here, and Right performs no other
/// environment mutation at runtime (`right::main` only sets `NO_COLOR`,
/// before any provider work).
fn publish_source_value(var: &str, value: &str) {
    let _guard = ENV_PUBLISH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: serialized against every other writer in this process; see the
    // function docs for why an environment write is unavoidable here.
    unsafe { std::env::set_var(var, value) };
}

/// Remove a published source variable. Test-only: lets tests of the
/// source-ref publish path leave the process environment as they found it, so
/// a later test never observes a value an earlier test set.
#[cfg(test)]
pub(crate) fn remove_source_value(var: &str) {
    let _guard = ENV_PUBLISH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: serialized against every other writer in this process.
    unsafe { std::env::remove_var(var) };
}

impl ProviderStore {
    /// Resolve a record into a microsandbox source-ref secret binding.
    ///
    /// This is the only code path that reads a stored credential. The value
    /// is published into this process's environment under the binding's
    /// `source_env_var` and is never returned: the caller receives names, a
    /// placeholder, and the hosts allowed to receive the substitution.
    ///
    /// An empty or already-redacted credential is a hard error rather than a
    /// binding that would silently substitute nothing.
    pub async fn source_ref_binding(
        &self,
        agent: &str,
        name: &str,
    ) -> Result<right_sandbox::SecretBinding, StoreError> {
        let resolved = resolve(&self.conn, agent, name).await?;
        let owner = resolved.row.owner_agent.clone();
        let kind = resolved.row.kind.clone();
        let env_var = kind.env_var()?.to_owned();
        let allowed_hosts = kind.allowed_hosts()?;
        if allowed_hosts.is_empty() {
            return Err(StoreError::InvalidName {
                name: name.into(),
                reason: "provider has no allowed hosts, so its credential could never be used"
                    .into(),
            });
        }
        let query_injection = kind
            .builtin_entry()
            .is_some_and(|entry| entry.query_injection);

        let credential: String = self
            .conn
            .query_one(
                "SELECT credential FROM providers WHERE owner_agent = ?1 AND name = ?2",
                params![&owner, name],
                |row| row.get(0),
            )
            .await?;
        check_source_credential_readable(&credential, name)?;

        let source_env_var = source_env_var(name);
        publish_source_value(&source_env_var, &credential);

        let mut binding = right_sandbox::SecretBinding::new(env_var, source_env_var);
        binding.allowed_hosts = allowed_hosts;
        binding.inject_query = query_injection;
        Ok(binding)
    }

    /// Seed a built-in row bypassing catalog validation.
    ///
    /// Test-only: models a record whose slug drifted out of the catalog after
    /// creation, which `create` can never produce (it validates the slug).
    /// The handler tests use it to prove list/rotate fail loud on such rows.
    /// `env_var` is the value the row stored at creation time, so reads can
    /// still report it after the catalog lookup starts failing.
    #[cfg(feature = "test-support")]
    pub async fn seed_builtin_unchecked(
        &self,
        agent: &str,
        name: &str,
        slug: &str,
        env_var: &str,
    ) -> Result<(), StoreError> {
        self.conn
            .execute(
                "INSERT INTO providers
                     (owner_agent, name, kind, builtin_slug, env_var, label,
                      upstream_hosts, upstream_path_prefix, credential, updated_at)
                 VALUES (?1, ?2, 'builtin', ?3, ?4, '', '[]', NULL, 'x', 0)",
                params![agent, name, slug, env_var],
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
