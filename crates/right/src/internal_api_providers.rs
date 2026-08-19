//! Provider management routes — see ARCHITECTURE.md "Providers".
//!
//! Stage 3: provider records and credentials live in the Right-owned
//! `right_providers::ProviderStore` (`~/.right/providers.db`). The handlers
//! below keep the internal UDS wire contract (routes, JSON shapes, error
//! codes) compatible with the OpenShell-gateway era, but no handler in this
//! module talks to `right_openshell`: composition confirmation,
//! `wait_for_provider_composed*`, and `ensure_v2_enabled` are gone by design
//! (see `docs/superpowers/specs/2026-08-19-microsandbox-migration-design.md`,
//! decisions "Providers" / "Provider status").

use right_providers::{
    Credential, GenericSpec, NewProvider, ProviderKind, ProviderRecord, ProviderStore, StoreError,
};

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
    #[error(
        "generic provider \"{name}\" env_var cannot be changed without a new credential; rotate or recreate the provider"
    )]
    GenericEnvVarChangeRequiresCredential { name: String },
    #[error("providers are only available for sandboxed agents (sandbox.mode = openshell)")]
    SandboxModeNone,
    #[error(
        "generic providers require network_policy: permissive — restrictive mode forbids non-Anthropic upstream hosts"
    )]
    NetworkPolicyForbidsGeneric,
    #[error("agent.yaml write failed: {0}")]
    AgentYamlWrite(String),
    #[error(
        "provider \"{name}\" references unknown built-in slug \"{slug}\" — the catalog no longer recognizes it; config migration required"
    )]
    UnknownBuiltinSlug { name: String, slug: String },
    #[error("not a trusted dashboard user on agent \"{agent}\"")]
    Unauthorized { agent: String },
    #[error("copy conflict: {reason}")]
    CopyConflict { reason: String },
    #[error(
        "provider \"{name}\" is borrowed (shared from \"{owner}\") and is read-only for this agent; the owner controls rotation/config, and \"unshare\" removes your reference"
    )]
    BorrowedProviderReadOnly { name: String, owner: String },
    #[error(
        "source provider \"{source_provider}\" credential cannot be read back: the store never exposes credential values, so cross-agent copy cannot transfer the key. Add the provider on the destination agent and enter the credential directly."
    )]
    SourceCredentialUnreadable { source_provider: String },
    #[error("internal error: {0}")]
    Internal(String),
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
            Self::GenericEnvVarChangeRequiresCredential { .. } => (
                StatusCode::BAD_REQUEST,
                "generic_env_var_change_requires_credential",
            ),
            Self::SandboxModeNone => (StatusCode::BAD_REQUEST, "sandbox_mode_none"),
            Self::NetworkPolicyForbidsGeneric => {
                (StatusCode::BAD_REQUEST, "network_policy_forbids_generic")
            }
            Self::AgentYamlWrite(_) => (StatusCode::INTERNAL_SERVER_ERROR, "agent_yaml_write"),
            Self::UnknownBuiltinSlug { .. } => {
                (StatusCode::INTERNAL_SERVER_ERROR, "unknown_builtin_slug")
            }
            Self::Unauthorized { .. } => (StatusCode::FORBIDDEN, "unauthorized"),
            Self::CopyConflict { .. } => (StatusCode::CONFLICT, "copy_conflict"),
            Self::BorrowedProviderReadOnly { .. } => (StatusCode::CONFLICT, "borrowed_read_only"),
            Self::SourceCredentialUnreadable { .. } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "source_credential_unreadable",
            ),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        (
            status,
            axum::Json(serde_json::json!({"code": code, "message": format!("{self}")})),
        )
            .into_response()
    }
}

/// Map a store failure onto the HTTP error the dashboard already depends on.
///
/// The variant names are load-bearing — `right_providers` was built so the
/// mapping below reproduces the gateway-era statuses exactly (409
/// `borrowed_read_only`, 422 `source_credential_unreadable`, and so on).
fn store_err(err: StoreError) -> ProviderApiError {
    match err {
        StoreError::NotFound { name } => ProviderApiError::NotFound { name },
        StoreError::NameCollision { name } => ProviderApiError::NameCollision { name },
        StoreError::EnvVarCollision { env_var } => ProviderApiError::EnvVarCollision { env_var },
        StoreError::InvalidName { name, reason } => {
            ProviderApiError::InvalidName { name, reason }
        }
        StoreError::InvalidEnvVar { env_var } => ProviderApiError::InvalidEnvVar { env_var },
        StoreError::UnknownBuiltinSlug { slug } => ProviderApiError::UnknownBuiltinSlug {
            name: String::new(),
            slug,
        },
        StoreError::BorrowedReadOnly { name, owner } => {
            ProviderApiError::BorrowedProviderReadOnly { name, owner }
        }
        StoreError::ShareConflict { reason } => ProviderApiError::CopyConflict { reason },
        StoreError::GenericEnvVarChangeRequiresCredential { name } => {
            ProviderApiError::GenericEnvVarChangeRequiresCredential { name }
        }
        StoreError::SourceCredentialUnreadable { source_provider } => {
            ProviderApiError::SourceCredentialUnreadable { source_provider }
        }
        other => ProviderApiError::Internal(format!("provider store: {other:#}")),
    }
}

/// Validate a provider record name (see `right_providers::validate`).
fn validate_name(agent: &str, name: &str) -> Result<(), ProviderApiError> {
    ProviderStore::validate_name(agent, name).map_err(store_err)
}

/// Validate a provider type slug against the store catalog (rejects the
/// reserved `claude` slug before the catalog lookup).
fn validate_type_slug(slug: &str) -> Result<(), ProviderApiError> {
    ProviderStore::validate_type_slug(slug).map_err(store_err)
}

/// Validate a human label (see `right_providers::validate::validate_label`).
fn validate_label(label: &str) -> Result<(), ProviderApiError> {
    right_providers::validate::validate_label(label).map_err(store_err)
}

/// Validate a generic-provider definition and return its normalized hosts
/// (see `right_providers::validate::validate_generic_request`).
fn validate_generic_request(
    env_var: &str,
    upstream_host: Option<&str>,
    upstream_hosts: Option<&[String]>,
    upstream_path_prefix: Option<&str>,
) -> Result<Vec<String>, ProviderApiError> {
    right_providers::validate::validate_generic_request(
        env_var,
        upstream_host,
        upstream_hosts,
        upstream_path_prefix,
    )
    .map_err(store_err)
}

/// Validate a generic config-update env var against the current one:
/// rebinding a credential to a new variable is a create, not a config edit.
fn validate_generic_env_var_unchanged(
    provider_name: &str,
    current_env_var: &str,
    new_env_var: &str,
) -> Result<(), ProviderApiError> {
    if new_env_var != current_env_var {
        return Err(ProviderApiError::GenericEnvVarChangeRequiresCredential {
            name: provider_name.to_string(),
        });
    }
    Ok(())
}

/// The type slug as it appears on the wire (`"generic"` for generics).
fn record_view_type(record: &ProviderRecord) -> String {
    record.kind.slug().to_string()
}

/// The `GenericProvider` block of a record, for views and yaml entries.
fn record_generic(record: &ProviderRecord) -> Option<right_agent_config::GenericProvider> {
    record.kind.generic().map(|spec| {
        right_agent_config::GenericProvider {
            env_var: spec.env_var.clone(),
            upstream_hosts: spec.upstream_hosts.clone(),
            upstream_path_prefix: spec.upstream_path_prefix.clone(),
        }
    })
}

/// Whether an agent is a sandboxed agent that may carry providers.
///
/// Absent `sandbox` section or `mode: none` rejects with `sandbox_mode_none`
/// (mode:none is transitional; stage 4 removes it entirely).
fn require_openshell_sandbox(
    cfg: &right_agent_config::AgentConfig,
) -> Result<&right_agent_config::SandboxConfig, ProviderApiError> {
    let sandbox = cfg
        .sandbox
        .as_ref()
        .ok_or(ProviderApiError::SandboxModeNone)?;
    if sandbox.mode != right_agent_config::SandboxMode::Openshell {
        return Err(ProviderApiError::SandboxModeNone);
    }
    Ok(sandbox)
}


// ── /provider-list ──────────────────────────────────────────────────────────

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
    /// Present ⇒ this provider is a *borrowed* reference shared from the named
    /// owner agent (read-only for this agent). Absent ⇒ owned. Ownership is
    /// derived from `providers.db`, never from `agent.yaml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_from: Option<String>,
}

/// Tri-state provider health. Replaces the OpenShell-era `composed` flag and
/// the four-variant gateway status (design decision "Provider status"):
/// `ready` / `needs-value` / `error`. `error` carries a message so a row
/// whose built-in slug has left the catalog (or whose credential was wiped)
/// still tells the operator what is wrong.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderStatus {
    Ready,
    NeedsValue,
    Error { message: String },
}

/// Project a store record's tri-state status onto the wire shape.
fn record_status(record: &ProviderRecord) -> ProviderStatus {
    match record.status {
        right_providers::ProviderStatus::Ready => ProviderStatus::Ready,
        right_providers::ProviderStatus::NeedsValue => ProviderStatus::NeedsValue,
        right_providers::ProviderStatus::Error => ProviderStatus::Error {
            message: record_status_error_message(record),
        },
    }
}

fn record_status_error_message(record: &ProviderRecord) -> String {
    match &record.kind {
        ProviderKind::Builtin(slug) => {
            format!("unknown built-in slug \"{slug}\" — config migration required")
        }
        ProviderKind::Generic(_) => "provider definition is unresolvable".into(),
    }
}

/// Optional label: the store persists `""` for "no label"; the wire shape and
/// agent.yaml both use absent/null.
fn record_label(record: &ProviderRecord) -> Option<String> {
    (!record.label.is_empty()).then(|| record.label.clone())
}

/// Unix seconds → RFC 3339. A pre-epoch or unrepresentable timestamp renders
/// as `None` rather than aborting the whole list view.
fn record_updated_at(updated_at: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp(updated_at, 0)
}

/// The owner a borrowed reference points at (`shared_from` on the wire).
fn record_shared_from(record: &ProviderRecord) -> Option<String> {
    record.is_borrowed().then(|| record.owner_agent.clone())
}

/// Build the wire view of one store record.
fn record_view(record: &ProviderRecord) -> ProviderView {
    ProviderView {
        name: record.name.clone(),
        type_: record_view_type(record),
        label: record_label(record),
        env_var: record.env_var.clone(),
        generic: record_generic(record),
        updated_at: record_updated_at(record.updated_at),
        status: record_status(record),
        shared_from: record_shared_from(record),
    }
}

/// The `agent.yaml` entry a record corresponds to.
///
/// `shared_from` is NOT emitted: ownership is a providers.db column now and
/// the yaml field is legacy migration input only (stage 4 removes it from
/// `right_agent_config::ProviderEntry` together with its remaining readers in
/// `destroy.rs` / `sandbox_supervisor.rs`).
fn record_yaml_entry(record: &ProviderRecord) -> right_agent_config::ProviderEntry {
    right_agent_config::ProviderEntry {
        name: record.name.clone(),
        type_: match &record.kind {
            ProviderKind::Builtin(slug) => {
                right_agent_config::ProviderType::BuiltIn(slug.clone())
            }
            ProviderKind::Generic(_) => right_agent_config::ProviderType::Generic,
        },
        label: record_label(record),
        generic: record_generic(record),
        shared_from: None,
    }
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
    require_openshell_sandbox(&cfg)?;
    let records = state.providers.list(&req.agent).await.map_err(store_err)?;
    let views = records.iter().map(record_view).collect();
    Ok(axum::Json(views))
}


// ── /provider-create ─────────────────────────────────────────────────────────

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
    #[serde(default)]
    pub upstream_host: Option<String>,
    #[serde(default)]
    pub upstream_hosts: Option<Vec<String>>,
    pub upstream_path_prefix: Option<String>,
}

pub(crate) async fn handle_provider_create(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderCreateReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    validate_type_slug(&req.type_)?;
    if let Some(label) = &req.label {
        validate_label(label)?;
    }
    let is_generic = req.type_ == right_providers::GENERIC_SLUG;
    let generic_spec = if is_generic {
        let g = req
            .generic
            .as_ref()
            .ok_or_else(|| ProviderApiError::InvalidEnvVar { env_var: "".into() })?;
        let hosts = validate_generic_request(
            &g.env_var,
            g.upstream_host.as_deref(),
            g.upstream_hosts.as_deref(),
            g.upstream_path_prefix.as_deref(),
        )?;
        Some(GenericSpec {
            env_var: g.env_var.clone(),
            upstream_hosts: hosts,
            upstream_path_prefix: g.upstream_path_prefix.clone(),
        })
    } else {
        None
    };
    let name = right_providers::new_record_name(&req.type_);
    validate_name(&req.agent, &name)?;
    let _guard = state.providers.agent_lock(&req.agent).await;

    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    require_openshell_sandbox(&cfg)?;
    if is_generic
        && matches!(
            cfg.network_policy,
            right_agent_config::NetworkPolicy::Restrictive
        )
    {
        // Generic providers substitute credentials against arbitrary upstream
        // hosts; restrictive mode has not been validated for those endpoints,
        // so generic providers stay permissive-only.
        return Err(ProviderApiError::NetworkPolicyForbidsGeneric);
    }

    let kind = match generic_spec {
        Some(spec) => ProviderKind::Generic(spec),
        None => ProviderKind::Builtin(req.type_.clone()),
    };
    let record = state
        .providers
        .create(
            NewProvider {
                owner_agent: req.agent.clone(),
                name,
                kind,
                label: req.label.clone().unwrap_or_default(),
            },
            Credential::from(req.credential.clone()),
        )
        .await
        .map_err(store_err)?;

    // The store is the authority; agent.yaml records the definition so the
    // sandbox spec can declare the provider. On yaml failure the record is
    // rolled back so the two never disagree (FAIL FAST, AGENTS.rust.md §2).
    let entry = record_yaml_entry(&record);
    if let Err(e) = append_provider_to_yaml(&state.agents_dir, &req.agent, &entry) {
        state
            .providers
            .remove(&req.agent, &record.name)
            .await
            .map_err(|remove_err| {
                ProviderApiError::Internal(format!(
                    "agent.yaml write failed ({e:#}) AND store rollback failed: {remove_err:#}"
                ))
            })?;
        return Err(ProviderApiError::AgentYamlWrite(format!("{e:#}")));
    }

    Ok(axum::Json(record_view(&record)))
}

// ── /provider-rotate ────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ProviderRotateReq {
    pub agent: String,
    pub name: String,
    pub credential: secrecy::SecretString,
}

pub(crate) async fn handle_provider_rotate(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderRotateReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    validate_name(&req.agent, &req.name)?;
    let _guard = state.providers.agent_lock(&req.agent).await;

    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    require_openshell_sandbox(&cfg)?;

    // Borrowed records are read-only for the borrower; the store enforces it
    // (resolve + reject_if_borrowed) inside `rotate`.
    state
        .providers
        .rotate(&req.agent, &req.name, Credential::from(req.credential.clone()))
        .await
        .map_err(store_err)?;

    let record = state
        .providers
        .get(&req.agent, &req.name)
        .await
        .map_err(store_err)?;
    Ok(axum::Json(record_view(&record)))
}

// ── /provider-config-update ──────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ProviderConfigUpdateReq {
    pub agent: String,
    pub name: String,
    pub generic: ProviderConfigUpdateGeneric,
}

#[derive(Debug, serde::Deserialize)]
pub struct ProviderConfigUpdateGeneric {
    pub env_var: Option<String>,
    #[serde(default)]
    pub upstream_host: Option<String>,
    #[serde(default)]
    pub upstream_hosts: Option<Vec<String>>,
    pub upstream_path_prefix: Option<Option<String>>,
}

pub(crate) async fn handle_provider_config_update(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderConfigUpdateReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    validate_name(&req.agent, &req.name)?;
    let _guard = state.providers.agent_lock(&req.agent).await;

    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    require_openshell_sandbox(&cfg)?;

    let record = state
        .providers
        .get(&req.agent, &req.name)
        .await
        .map_err(store_err)?;
    if record.is_borrowed() {
        return Err(ProviderApiError::BorrowedProviderReadOnly {
            name: record.name.clone(),
            owner: record.owner_agent.clone(),
        });
    }
    let ProviderKind::Generic(current) = &record.kind else {
        return Err(ProviderApiError::InvalidName {
            name: req.name.clone(),
            reason: "config-update only valid on generic providers".into(),
        });
    };
    // Same rationale as `handle_provider_create`: generic providers are only
    // supported under the permissive network policy.
    if matches!(
        cfg.network_policy,
        right_agent_config::NetworkPolicy::Restrictive
    ) {
        return Err(ProviderApiError::NetworkPolicyForbidsGeneric);
    }

    let new_env_var = req
        .generic
        .env_var
        .clone()
        .unwrap_or_else(|| current.env_var.clone());
    // `upstream_path_prefix` is Option<Option<String>>: absent keeps the
    // current value, explicit null CLEARS it (byte-compat contract).
    let new_path = match req.generic.upstream_path_prefix.clone() {
        None => current.upstream_path_prefix.clone(),
        Some(v) => v,
    };
    let fallback_hosts =
        if req.generic.upstream_host.is_none() && req.generic.upstream_hosts.is_none() {
            Some(current.upstream_hosts.clone())
        } else {
            None
        };
    let new_hosts = validate_generic_request(
        &new_env_var,
        req.generic.upstream_host.as_deref(),
        req.generic
            .upstream_hosts
            .as_deref()
            .or(fallback_hosts.as_deref()),
        new_path.as_deref(),
    )?;
    validate_generic_env_var_unchanged(&req.name, &current.env_var, &new_env_var)?;

    state
        .providers
        .update_generic(
            &req.agent,
            &req.name,
            GenericSpec {
                env_var: new_env_var,
                upstream_hosts: new_hosts,
                upstream_path_prefix: new_path,
            },
        )
        .await
        .map_err(store_err)?;

    let updated = state
        .providers
        .get(&req.agent, &req.name)
        .await
        .map_err(store_err)?;
    // Keep agent.yaml in lockstep with the store. On yaml failure the store
    // update is rolled back to the previous definition so the two never
    // disagree (FAIL FAST, AGENTS.rust.md §2).
    let entry = record_yaml_entry(&updated);
    if let Err(e) = replace_provider_in_yaml(&state.agents_dir, &req.agent, &entry) {
        state
            .providers
            .update_generic(&req.agent, &req.name, current.clone())
            .await
            .map_err(|rollback_err| {
                ProviderApiError::Internal(format!(
                    "agent.yaml write failed ({e:#}) AND store rollback failed: {rollback_err:#}"
                ))
            })?;
        return Err(ProviderApiError::AgentYamlWrite(format!("{e:#}")));
    }

    Ok(axum::Json(record_view(&updated)))
}


// ── Provider sharing (borrow references) ────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ProviderShareReq {
    pub actor_user_id: i64,
    pub owner_agent: String,
    pub provider: String,
    pub dest_agent: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ProviderUnshareReq {
    pub actor_user_id: i64,
    pub borrower_agent: String,
    pub provider: String,
}

/// Share the owner's record with `dest_agent` as a *borrowed* reference: a
/// row in `provider_borrows` pointing at the true owner, plus a definition
/// entry in the destination's agent.yaml. No credential is copied or read
/// back — the value stays in `providers.db`, owned by `owner_agent`.
pub(crate) async fn handle_provider_share(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderShareReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    // Actor must be trusted on BOTH sides.
    require_trusted(&state.agents_dir, &req.owner_agent, req.actor_user_id)?;
    require_trusted(&state.agents_dir, &req.dest_agent, req.actor_user_id)?;

    // Both sides must be sandboxed agents (mode:none is transitional).
    let owner_cfg = load_agent_config(&state.agents_dir, &req.owner_agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    require_openshell_sandbox(&owner_cfg)?;
    let dest_cfg = load_agent_config(&state.agents_dir, &req.dest_agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    require_openshell_sandbox(&dest_cfg)?;

    validate_name(&req.dest_agent, &req.provider)?;
    // Share locks the DEST agent only: the dest agent.yaml RMW is the only
    // mutation on that side (byte-compat contract).
    let _guard = state.providers.agent_lock(&req.dest_agent).await;

    // The store resolves the true owner for a re-share and enforces the
    // share plan (no share into self, no name collision at the destination)
    // inside one transaction.
    let record = state
        .providers
        .share(&req.owner_agent, &req.provider, &req.dest_agent)
        .await
        .map_err(store_err)?;

    // Write the borrowed definition entry to the destination's agent.yaml.
    // On failure, unshare so the store never references a provider the yaml
    // does not declare (FAIL FAST).
    let entry = record_yaml_entry(&record);
    if let Err(e) = append_provider_to_yaml(&state.agents_dir, &req.dest_agent, &entry) {
        state
            .providers
            .unshare(&req.dest_agent, &req.provider)
            .await
            .map_err(|rollback_err| {
                ProviderApiError::Internal(format!(
                    "agent.yaml write failed ({e:#}) AND share rollback failed: {rollback_err:#}"
                ))
            })?;
        return Err(ProviderApiError::AgentYamlWrite(format!("{e:#}")));
    }

    Ok(axum::Json(record_view(&record)))
}

/// Unshare a *borrowed* provider from `borrower_agent`: drop the borrow row
/// and the borrowed agent.yaml entry. The owned record is never touched —
/// removing an owned record goes through `/provider-remove`.
pub(crate) async fn handle_provider_unshare(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderUnshareReq>,
) -> Result<axum::Json<ProviderRemoveResp>, ProviderApiError> {
    require_trusted(&state.agents_dir, &req.borrower_agent, req.actor_user_id)?;
    validate_name(&req.borrower_agent, &req.provider)?;
    let _guard = state.providers.agent_lock(&req.borrower_agent).await;

    let cfg = load_agent_config(&state.agents_dir, &req.borrower_agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    require_openshell_sandbox(&cfg)?;

    // Reject unsharing an OWNED record with copy_conflict (the store's
    // plan_unshare), and reject unsharing a provider that is not borrowed
    // here at all with not_found.
    let record = state
        .providers
        .get(&req.borrower_agent, &req.provider)
        .await
        .map_err(store_err)?;
    if record.is_owned() {
        return Err(ProviderApiError::CopyConflict {
            reason: format!(
                "provider \"{}\" is owned by this agent, not borrowed; use remove, not unshare",
                req.provider
            ),
        });
    }

    state
        .providers
        .unshare(&req.borrower_agent, &req.provider)
        .await
        .map_err(store_err)?;
    if let Err(e) =
        remove_provider_from_yaml(&state.agents_dir, &req.borrower_agent, &req.provider)
    {
        // Compensate: re-attach the borrow so the store never drops a
        // reference the yaml still declares (mirrors the share handler's
        // rollback). `record` was fetched before the unshare and still holds
        // the true owner.
        state
            .providers
            .share(&record.owner_agent, &req.provider, &req.borrower_agent)
            .await
            .map_err(|rollback_err| {
                ProviderApiError::Internal(format!(
                    "agent.yaml removal failed ({e:#}) AND unshare rollback failed: {rollback_err:#}"
                ))
            })?;
        return Err(ProviderApiError::AgentYamlWrite(format!("{e:#}")));
    }

    Ok(axum::Json(ProviderRemoveResp { removed: true }))
}

// ── /provider-remove ─────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ProviderRemoveReq {
    pub agent: String,
    pub name: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ProviderRemoveResp {
    pub removed: bool,
}

pub(crate) async fn handle_provider_remove(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderRemoveReq>,
) -> Result<axum::Json<ProviderRemoveResp>, ProviderApiError> {
    validate_name(&req.agent, &req.name)?;
    let _guard = state.providers.agent_lock(&req.agent).await;

    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    require_openshell_sandbox(&cfg)?;

    // The store enforces owner-only removal (borrowed references are
    // read-only) and re-homes the record to a surviving borrower when the
    // owner deletes it — the credential stays reachable for every agent that
    // still declares it and exactly one authority remains.
    //
    // Accepted divergence: once the credential is destroyed the store
    // mutation cannot be compensated, so a subsequent agent.yaml failure
    // leaves the yaml declaring a provider the store no longer has. That
    // diverges fail-loud (the next spawn's source_ref_binding returns
    // NotFound) rather than silently. Matches the old gateway behavior.
    state
        .providers
        .remove(&req.agent, &req.name)
        .await
        .map_err(store_err)?;
    remove_provider_from_yaml(&state.agents_dir, &req.agent, &req.name)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;

    Ok(axum::Json(ProviderRemoveResp { removed: true }))
}

// ── /provider-types ──────────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
pub struct ProviderProfileView {
    #[serde(rename = "type")]
    pub type_slug: String,
    pub env_var: String,
    pub display_name: String,
    pub category: String,
}

pub(crate) async fn handle_provider_types() -> axum::Json<Vec<ProviderProfileView>> {
    // The offered catalog is the compile-time `right_providers` catalog minus
    // superseded (hidden) entries — `github` stays hidden behind
    // `right-github` exactly as before. Category strings reproduce the old
    // `format!("{:?}", category).to_lowercase()` rendering byte-for-byte.
    let views: Vec<_> = ProviderStore::offered_catalog()
        .into_iter()
        .map(|p| ProviderProfileView {
            type_slug: p.slug.to_string(),
            env_var: p.env_var.to_string(),
            display_name: p.display_name.to_string(),
            category: p.category.as_str().to_string(),
        })
        .collect();
    axum::Json(views)
}


// ── /provider-peers discovery ────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ProviderPeersReq {
    pub actor_user_id: i64,
    pub for_agent: String,
}

#[derive(Debug, serde::Serialize)]
pub struct PeerProvider {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub env_var: String,
    pub label: Option<String>,
    pub generic: Option<right_agent_config::GenericProvider>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProviderPeer {
    pub agent: String,
    pub network_policy: String,
    pub providers: Vec<PeerProvider>,
}

/// Whether `actor_user_id` is in `agent`'s allowlist. Missing/empty allowlist
/// = not trusted (secure default).
fn is_trusted(
    agents_dir: &std::path::Path,
    agent: &str,
    actor_user_id: i64,
) -> Result<bool, ProviderApiError> {
    let agent_dir = agents_dir.join(agent);
    let file =
        right_agent::agent::allowlist::read_file(&agent_dir).map_err(ProviderApiError::Internal)?;
    Ok(file
        .map(|f| f.users.iter().any(|u| u.id == actor_user_id))
        .unwrap_or(false))
}

/// Require that `actor_user_id` is in `agent`'s allowlist. Missing/empty
/// allowlist = not trusted (secure default).
fn require_trusted(
    agents_dir: &std::path::Path,
    agent: &str,
    actor_user_id: i64,
) -> Result<(), ProviderApiError> {
    if is_trusted(agents_dir, agent, actor_user_id)? {
        Ok(())
    } else {
        Err(ProviderApiError::Unauthorized {
            agent: agent.to_string(),
        })
    }
}

/// Enumerate host-local agents (except `for_agent`) where `actor_user_id`
/// is trusted and the sandbox mode is openshell. Returns each peer's
/// providers (never credentials — the store read APIs don't carry them).
/// Tolerant: a peer with an unreadable `agent.yaml` or store row is
/// skipped, not fatal.
async fn build_peers(
    store: &ProviderStore,
    agents_dir: &std::path::Path,
    actor_user_id: i64,
    for_agent: &str,
) -> Result<Vec<ProviderPeer>, ProviderApiError> {
    let mut names: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(agents_dir)
        .map_err(|e| ProviderApiError::Internal(format!("read agents dir: {e:#}")))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| ProviderApiError::Internal(format!("read agents dir entry: {e:#}")))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        if name == for_agent || !path.join("agent.yaml").exists() {
            continue;
        }
        names.push(name);
    }
    names.sort();

    let mut peers = Vec::new();
    for name in names {
        let agent_dir = agents_dir.join(&name);
        // Discovery is tolerant: a peer whose allowlist can't be read can't be
        // authorized, so it is excluded rather than failing the whole listing.
        // (Trust for the *current* agent is enforced separately, fail-closed,
        // by `require_trusted` in the handler.)
        match is_trusted(agents_dir, &name, actor_user_id) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                tracing::warn!(
                    agent = %name,
                    "skipping peer with unreadable allowlist.yaml: {e:#}"
                );
                continue;
            }
        }
        let cfg = match right_agent::agent::discovery::parse_agent_config(&agent_dir) {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(agent = %name, "skipping peer with unreadable agent.yaml: {e:#}");
                continue;
            }
        };
        let Some(sandbox) = cfg.sandbox.as_ref() else {
            continue;
        };
        if sandbox.mode != right_agent_config::SandboxMode::Openshell {
            continue;
        }
        let network_policy = match cfg.network_policy {
            right_agent_config::NetworkPolicy::Permissive => "permissive",
            right_agent_config::NetworkPolicy::Restrictive => "restrictive",
        }
        .to_string();
        let records = match store.list(&name).await {
            Ok(records) => records,
            Err(e) => {
                tracing::warn!(
                    agent = %name,
                    "skipping peer with unreadable providers.db rows: {e:#}"
                );
                continue;
            }
        };
        let providers = records
            .iter()
            .map(|record| PeerProvider {
                name: record.name.clone(),
                type_: record_view_type(record),
                env_var: record.env_var.clone(),
                label: record_label(record),
                generic: record_generic(record),
            })
            .collect();
        peers.push(ProviderPeer {
            agent: name,
            network_policy,
            providers,
        });
    }
    Ok(peers)
}

pub(crate) async fn handle_provider_peers(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderPeersReq>,
) -> Result<axum::Json<Vec<ProviderPeer>>, ProviderApiError> {
    require_trusted(&state.agents_dir, &req.for_agent, req.actor_user_id)?;
    build_peers(&state.providers, &state.agents_dir, req.actor_user_id, &req.for_agent)
        .await
        .map(axum::Json)
}


// ── agent.yaml writers (write_merged_rmw) ────────────────────────────────────


/// Render a free-form string as a single-quoted YAML scalar. Single quotes
/// inside the value are escaped per the YAML 1.1/1.2 spec by doubling them
/// (`'` -> `''`). Single-quoted scalars do not process backslash escapes,
/// so they are the safest hand-rolled YAML serialization for arbitrary
/// non-control ASCII text — which is exactly what the store validators
/// guarantee about every field on `ProviderEntry`.
fn yaml_single_quote(value: &str) -> String {
    let escaped = value.replace('\'', "''");
    format!("'{escaped}'")
}

/// Render the provider entry as YAML lines at 4-space indentation (nested
/// under `sandbox.providers:` which is at column 2).
///
/// All free-form string scalars are single-quoted (`yaml_single_quote`)
/// so that values like `label: "no"` or `label: "123"` round-trip as
/// strings instead of getting reinterpreted by YAML 1.1 loaders as
/// booleans or numbers — which would break `Option<String>` /
/// `String` deserialization on the next bot restart.
///
/// `shared_from` is deliberately never emitted: ownership is a providers.db
/// column now, and the yaml field is legacy migration input only (stage 4
/// removes it from `right_agent_config::ProviderEntry`).
fn serialize_provider_entry(entry: &right_agent_config::ProviderEntry) -> String {
    let mut out = String::new();
    out.push_str(&format!("    - name: {}\n", yaml_single_quote(&entry.name)));
    let type_str = match &entry.type_ {
        right_agent_config::ProviderType::Generic => "generic".to_string(),
        right_agent_config::ProviderType::BuiltIn(s) => s.clone(),
    };
    out.push_str(&format!("      type: {}\n", yaml_single_quote(&type_str)));
    if let Some(label) = &entry.label {
        out.push_str(&format!("      label: {}\n", yaml_single_quote(label)));
    }
    if let Some(g) = &entry.generic {
        out.push_str("      generic:\n");
        out.push_str(&format!(
            "        env_var: {}\n",
            yaml_single_quote(&g.env_var)
        ));
        out.push_str("        upstream_hosts:\n");
        for host in &g.upstream_hosts {
            out.push_str(&format!("          - {}\n", yaml_single_quote(host)));
        }
        if let Some(prefix) = &g.upstream_path_prefix {
            out.push_str(&format!(
                "        upstream_path_prefix: {}\n",
                yaml_single_quote(prefix)
            ));
        }
    }
    out
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
        if let Some(c) = ch
            && c != ' '
            && c != '\t'
            && c != '\n'
            && c != '\r'
            && c != '#'
        {
            sandbox_end = i;
            break;
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

/// Replace an existing provider entry by name. Line-walking YAML mutator.
fn replace_provider_in_yaml(
    agents_dir: &std::path::Path,
    agent: &str,
    updated: &right_agent_config::ProviderEntry,
) -> miette::Result<()> {
    let path = agents_dir.join(agent).join("agent.yaml");
    let new_entry = serialize_provider_entry(updated);
    let name = updated.name.clone();
    right_codegen::contract::write_merged_rmw(&path, |existing| {
        let original = existing.unwrap_or("");
        replace_provider_entry(original, &name, &new_entry)
    })
}

/// Locate the byte offset of the first `    - name: <name>` line in
/// `original`. Tolerates both legacy unquoted and new single-quoted forms
/// (`    - name: foo` and `    - name: 'foo'`) so that on-disk entries
/// written before the YAML-quoting fix still resolve. `name` itself is
/// always the unquoted, validated provider name from `ProviderEntry::name`.
fn find_provider_name_marker(original: &str, name: &str) -> Option<usize> {
    // Quoted form: `    - name: 'name'` is bounded by the closing quote, so a
    // plain substring search cannot collide with a longer name. New writes are
    // quoted, so the common path stays a single substring search.
    let quoted = format!("    - name: '{name}'");
    if let Some(idx) = original.find(&quoted) {
        return Some(idx);
    }
    // Unquoted (legacy) form needs an explicit trailing delimiter (\n, \r, or
    // EOF). Without it, searching for `myagent-foo` would match a line for
    // `myagent-foo-bar` and mutate the wrong row.
    let plain = format!("    - name: {name}");
    for (idx, _) in original.match_indices(&plain) {
        match original.as_bytes().get(idx + plain.len()) {
            None | Some(b'\n') | Some(b'\r') => return Some(idx),
            _ => continue,
        }
    }
    None
}

/// Replace the entry whose `    - name: <name>` matches. Returns Err if not found.
fn replace_provider_entry(original: &str, name: &str, new_entry: &str) -> miette::Result<String> {
    let Some(start_byte) = find_provider_name_marker(original, name) else {
        return Err(miette::miette!("provider '{}' not in agent.yaml", name));
    };
    // Find start of the line containing the marker.
    let line_start = original[..start_byte]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    // Find end: the next `    - ` sibling OR a column-2 key OR EOF.
    let first_line_end = line_start
        + original[line_start..]
            .find('\n')
            .map(|i| i + 1)
            .unwrap_or(original.len() - line_start);
    let mut end_byte = first_line_end;
    let rest = &original[first_line_end..];
    for line in rest.split_inclusive('\n') {
        if line.starts_with("    - ") {
            break;
        }
        if line.starts_with("  ") && !line.starts_with("    ") && !line.trim().is_empty() {
            break;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') && !line.trim().is_empty() {
            break;
        }
        end_byte += line.len();
    }
    let mut out = String::with_capacity(original.len());
    out.push_str(&original[..line_start]);
    out.push_str(new_entry);
    out.push_str(&original[end_byte..]);
    Ok(out)
}

fn remove_provider_from_yaml(
    agents_dir: &std::path::Path,
    agent: &str,
    name: &str,
) -> miette::Result<()> {
    let path = agents_dir.join(agent).join("agent.yaml");
    let name = name.to_string();
    right_codegen::contract::write_merged_rmw(&path, |existing| {
        let original = existing.unwrap_or("");
        Ok(remove_provider_entry(original, &name))
    })
}

/// Remove the entry whose `    - name: <name>` matches. No-op if not present.
fn remove_provider_entry(original: &str, name: &str) -> String {
    let Some(start_byte) = find_provider_name_marker(original, name) else {
        return original.to_string();
    };
    let line_start = original[..start_byte]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let first_line_end = line_start
        + original[line_start..]
            .find('\n')
            .map(|i| i + 1)
            .unwrap_or(original.len() - line_start);
    let mut end_byte = first_line_end;
    let rest = &original[first_line_end..];
    for line in rest.split_inclusive('\n') {
        if line.starts_with("    - ") {
            break;
        }
        if line.starts_with("  ") && !line.starts_with("    ") && !line.trim().is_empty() {
            break;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') && !line.trim().is_empty() {
            break;
        }
        end_byte += line.len();
    }
    let mut out = String::with_capacity(original.len() - (end_byte - line_start));
    out.push_str(&original[..line_start]);
    out.push_str(&original[end_byte..]);
    out
}


// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "internal_api_providers_tests.rs"]
mod tests;
