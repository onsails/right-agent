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
    let _guard = provider_lock(&state, &req.agent).await;

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
    let _guard = provider_lock(&state, &req.agent).await;

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
    let _guard = provider_lock(&state, &req.agent).await;

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
    let _guard = provider_lock(&state, &req.dest_agent).await;

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
    let _guard = provider_lock(&state, &req.borrower_agent).await;

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
    remove_provider_from_yaml(&state.agents_dir, &req.borrower_agent, &req.provider)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;

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
    let _guard = provider_lock(&state, &req.agent).await;

    let cfg = load_agent_config(&state.agents_dir, &req.agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    require_openshell_sandbox(&cfg)?;

    // The store enforces owner-only removal (borrowed references are
    // read-only) and re-homes the record to a surviving borrower when the
    // owner deletes it — the credential stays reachable for every agent that
    // still declares it and exactly one authority remains.
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

/// Acquire the per-agent provider mutation lock.
///
/// All provider mutations on a given agent eventually RMW the same
/// `agents/<agent>/agent.yaml`. Keying the lock on `agent` alone (not on
/// `(agent, name)`) serializes those RMWs and prevents a last-write-wins
/// race that would otherwise drop one of two concurrently-created
/// providers from agent.yaml while leaving store state already mutated
/// for it (an orphan).
///
/// Callers MUST invoke `validate_name(agent, name)` (where applicable)
/// before this so the lock map's key space stays bounded to validated
/// agent names — user-supplied agents that fail validation never reach
/// the lock map.
pub(crate) async fn provider_lock(
    state: &crate::internal_api::InternalState,
    agent: &str,
) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut map = state.provider_locks.lock().await;
        map.entry(agent.to_string())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}

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
mod provider_validation_tests {
    use super::*;

    #[test]
    fn name_must_match_agent_prefix() {
        // Legacy {agent}-{slug} form must still validate.
        assert!(validate_name("myagent", "myagent-anthropic").is_ok());
        // Agent-agnostic form (no agent prefix) is now also valid.
        assert!(validate_name("myagent", "other-anthropic").is_ok());
    }

    #[test]
    fn slug_pattern_enforced() {
        let err = validate_name("myagent", "myagent-Anthropic").unwrap_err();
        assert!(matches!(err, ProviderApiError::InvalidName { .. }));
        let err2 = validate_name("myagent", "myagent-").unwrap_err();
        assert!(matches!(err2, ProviderApiError::InvalidName { .. }));
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

    #[test]
    fn validate_type_slug_accepts_right_github() {
        // Regression: the dashboard offers `right-github` as the GitHub type, so
        // create-validation must accept it.
        assert!(validate_type_slug("right-github").is_ok());
    }

    #[test]
    fn validate_type_slug_in_sync_with_catalog() {
        // Every catalog type (except the reserved `claude` login slug, which is
        // never a catalog entry) must be creatable. Guards against the validator
        // and the catalog drifting apart again.
        for p in ProviderStore::catalog() {
            assert!(
                validate_type_slug(p.slug).is_ok(),
                "catalog type {} must pass create-validation",
                p.slug
            );
        }
    }

    // ── label validation against YAML-reserved tokens ─────────────────────

    fn assert_label_rejected(label: &str) {
        let err = validate_label(label)
            .expect_err(&format!("expected validate_label({label:?}) to reject"));
        assert!(
            matches!(err, ProviderApiError::InvalidName { .. }),
            "expected InvalidName for {label:?}, got {err:?}"
        );
    }

    #[test]
    fn validate_label_rejects_yaml_boolean_no() {
        assert_label_rejected("no");
    }

    #[test]
    fn validate_label_rejects_yaml_boolean_true() {
        assert_label_rejected("true");
    }

    #[test]
    fn validate_label_rejects_pure_numeric() {
        assert_label_rejected("123");
    }

    #[test]
    fn validate_label_rejects_tilde_null() {
        // `~` is not ASCII-alphanumeric so the scalar validator already
        // rejects it before the reserved-word check fires, but the field
        // still must be rejected (and classify as InvalidName).
        assert_label_rejected("~");
    }

    #[test]
    fn validate_label_rejects_case_variant_yes() {
        assert_label_rejected("Yes");
    }

    #[test]
    fn validate_label_rejects_all_keyword_case_variants() {
        for tok in [
            "y", "Y", "yes", "YES", "n", "N", "NO", "True", "TRUE", "False", "FALSE", "on", "On",
            "ON", "off", "Off", "OFF", "null", "Null", "NULL",
        ] {
            assert_label_rejected(tok);
        }
    }

    #[test]
    fn validate_label_accepts_hyphenated_keyword_like() {
        validate_label("no-thanks").expect("hyphenated label should be accepted");
    }

    #[test]
    fn validate_label_accepts_number_suffix() {
        validate_label("yes2").expect("'yes2' should be accepted");
    }

    #[test]
    fn validate_name_accepts_legacy_agent_prefixed() {
        validate_name("agent-a", "agent-a-provider").expect("legacy {agent}-{slug} must validate");
    }

    #[test]
    fn validate_name_accepts_agent_agnostic_uuid_form() {
        validate_name("agent-a", "fal-a1b2c3").expect("agent-agnostic name must validate");
    }

    #[test]
    fn validate_name_rejects_bad_agnostic_forms() {
        assert!(validate_name("agent-a", "Fal-a1b2c3").is_err()); // uppercase
        assert!(validate_name("agent-a", "1fal-a1b2c3").is_err()); // leading digit
        assert!(validate_name("agent-a", "").is_err()); // empty
        assert!(validate_name("agent-a", &"f".repeat(41)).is_err()); // over 40-char slug cap
    }

    #[test]
    fn new_record_name_has_type_slug_and_hex_suffix() {
        let n = right_providers::new_record_name("right-fal");
        assert!(n.starts_with("fal-"), "got {n}");
        let suffix = n.rsplit('-').next().unwrap();
        assert_eq!(suffix.len(), 6);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()), "got {n}");
    }

    /// Round-trip guard: serialize a provider entry with a label that YAML 1.1
    /// would otherwise coerce to a boolean, then re-parse the resulting YAML
    /// through `serde_saphyr` and confirm the label survives as a string.
    #[test]
    fn round_trip_quoted_label_survives_saphyr_parse() {
        let entry = right_agent_config::ProviderEntry {
            name: "agent-acme".to_string(),
            type_: right_agent_config::ProviderType::Generic,
            label: Some("acme".to_string()),
            generic: Some(right_agent_config::GenericProvider {
                env_var: "ACME_KEY".to_string(),
                upstream_hosts: vec!["api.acme.com".to_string()],
                upstream_path_prefix: Some("/v1".to_string()),
            }),
            shared_from: None,
        };
        let serialized = serialize_provider_entry(&entry);
        assert!(serialized.contains("name: 'agent-acme'"));
        assert!(serialized.contains("label: 'acme'"));

        let yaml = format!("sandbox:\n  mode: openshell\n  providers:\n{serialized}");
        let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&yaml)
            .expect("serialized provider entry must round-trip through serde_saphyr");
        let entry_back = &cfg.sandbox.unwrap().providers[0];
        assert_eq!(entry_back.name, "agent-acme");
        assert_eq!(entry_back.label.as_deref(), Some("acme"));
        let g = entry_back.generic.as_ref().unwrap();
        assert_eq!(g.env_var, "ACME_KEY");
        assert_eq!(g.upstream_hosts, vec!["api.acme.com"]);
        assert_eq!(g.upstream_path_prefix.as_deref(), Some("/v1"));
    }

    /// If a value that YAML 1.1 would coerce to a non-string ever bypassed
    /// label validation, the on-disk YAML must still re-parse correctly
    /// because all scalars are single-quoted.
    #[test]
    fn round_trip_label_no_parses_as_string_when_quoted() {
        let entry = right_agent_config::ProviderEntry {
            name: "agent-no".to_string(),
            type_: right_agent_config::ProviderType::Generic,
            label: Some("no".to_string()),
            generic: Some(right_agent_config::GenericProvider {
                env_var: "NO_KEY".to_string(),
                upstream_hosts: vec!["api.example.com".to_string()],
                upstream_path_prefix: None,
            }),
            shared_from: None,
        };
        let serialized = serialize_provider_entry(&entry);
        assert!(
            serialized.contains("label: 'no'"),
            "label must be single-quoted; got:\n{serialized}"
        );
        let yaml = format!("sandbox:\n  mode: openshell\n  providers:\n{serialized}");
        let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&yaml)
            .expect("single-quoted reserved-word label must still parse as Option<String>::Some");
        let entry_back = &cfg.sandbox.unwrap().providers[0];
        assert_eq!(entry_back.label.as_deref(), Some("no"));
    }

    #[test]
    fn serialize_provider_entry_never_emits_shared_from() {
        // Ownership is a providers.db column now. Even an entry structurally
        // marked borrowed must not write `shared_from:` to agent.yaml —
        // the field is legacy migration input only (stage 4 removes it).
        let borrowed = right_agent_config::ProviderEntry {
            name: "fal-a1b2c3".into(),
            type_: right_agent_config::ProviderType::BuiltIn("right-fal".into()),
            label: None,
            generic: None,
            shared_from: Some("agent-a".into()),
        };
        let s = serialize_provider_entry(&borrowed);
        assert!(
            !s.contains("shared_from"),
            "shared_from must never be emitted; got: {s}"
        );
    }
}


#[cfg(test)]
mod provider_view_tests {
    use super::*;

    fn record(kind: ProviderKind, status: right_providers::ProviderStatus) -> ProviderRecord {
        ProviderRecord {
            name: "fal-a1b2c3".into(),
            owner_agent: "agent-a".into(),
            kind,
            label: String::new(),
            env_var: "FAL_KEY".into(),
            updated_at: 1_755_000_000,
            borrower_agent: None,
            status,
        }
    }

    #[test]
    fn status_serializes_with_kind_tag_and_snake_case() {
        let ready = serde_json::to_value(ProviderStatus::Ready).unwrap();
        assert_eq!(ready, serde_json::json!({"kind": "ready"}));

        let needs = serde_json::to_value(ProviderStatus::NeedsValue).unwrap();
        assert_eq!(needs, serde_json::json!({"kind": "needs_value"}));

        let err = serde_json::to_value(ProviderStatus::Error {
            message: "boom".into(),
        })
        .unwrap();
        assert_eq!(err, serde_json::json!({"kind": "error", "message": "boom"}));
    }

    #[test]
    fn view_has_no_composed_field() {
        // The OpenShell-era `composed: bool|null` is replaced by the tri-state
        // status (design decision "Provider status"); the wire shape must not
        // resurrect it.
        let view = record_view(&record(
            ProviderKind::Builtin("right-fal".into()),
            right_providers::ProviderStatus::Ready,
        ));
        let json = serde_json::to_value(view).unwrap();
        assert!(json.get("composed").is_none(), "got: {json}");
        assert_eq!(json["status"]["kind"], "ready");
    }

    #[test]
    fn view_maps_store_status_and_omits_shared_from_for_owned() {
        let view = record_view(&record(
            ProviderKind::Builtin("right-fal".into()),
            right_providers::ProviderStatus::NeedsValue,
        ));
        let json = serde_json::to_value(view).unwrap();
        assert_eq!(json["status"]["kind"], "needs_value");
        assert!(json.get("shared_from").is_none(), "owned row: {json}");
    }

    #[test]
    fn view_sets_shared_from_to_true_owner_for_borrowed() {
        let mut rec = record(
            ProviderKind::Builtin("right-fal".into()),
            right_providers::ProviderStatus::Ready,
        );
        rec.borrower_agent = Some("right".into());
        let json = serde_json::to_value(record_view(&rec)).unwrap();
        assert_eq!(json["shared_from"], "agent-a");
    }

    #[test]
    fn view_error_status_names_unknown_builtin_slug() {
        let rec = record(
            ProviderKind::Builtin("not-a-real-slug".into()),
            right_providers::ProviderStatus::Error,
        );
        let json = serde_json::to_value(record_view(&rec)).unwrap();
        assert_eq!(json["status"]["kind"], "error");
        let msg = json["status"]["message"].as_str().unwrap();
        assert!(msg.contains("not-a-real-slug"), "got: {msg}");
    }

    #[test]
    fn view_generic_record_carries_generic_block() {
        let rec = record(
            ProviderKind::Generic(GenericSpec {
                env_var: "ACME_KEY".into(),
                upstream_hosts: vec!["api.acme.com".into()],
                upstream_path_prefix: Some("/v1".into()),
            }),
            right_providers::ProviderStatus::Ready,
        );
        let json = serde_json::to_value(record_view(&rec)).unwrap();
        assert_eq!(json["type"], "generic");
        assert_eq!(json["generic"]["env_var"], "ACME_KEY");
        assert_eq!(json["generic"]["upstream_hosts"][0], "api.acme.com");
        assert_eq!(json["generic"]["upstream_path_prefix"], "/v1");
    }

    #[test]
    fn view_updated_at_is_rfc3339_when_set() {
        let view = record_view(&record(
            ProviderKind::Builtin("right-fal".into()),
            right_providers::ProviderStatus::Ready,
        ));
        let json = serde_json::to_value(view).unwrap();
        assert!(
            json["updated_at"].as_str().unwrap().contains('T'),
            "updated_at must serialize RFC 3339: {json}"
        );
    }

    #[test]
    fn yaml_entry_from_record_never_borrowed() {
        let mut rec = record(
            ProviderKind::Builtin("right-fal".into()),
            right_providers::ProviderStatus::Ready,
        );
        rec.borrower_agent = Some("right".into());
        let entry = record_yaml_entry(&rec);
        assert!(entry.shared_from.is_none());
        assert!(matches!(
            entry.type_,
            right_agent_config::ProviderType::BuiltIn(ref s) if s == "right-fal"
        ));
    }
}

#[cfg(test)]
mod store_err_mapping_tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    fn status_of(err: StoreError) -> StatusCode {
        store_err(err).into_response().status()
    }

    #[test]
    fn store_errors_map_to_dashboard_statuses() {
        assert_eq!(
            status_of(StoreError::NotFound { name: "x".into() }),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(StoreError::NameCollision { name: "x".into() }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(StoreError::EnvVarCollision {
                env_var: "X".into()
            })
            ,
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(StoreError::InvalidName {
                name: "x".into(),
                reason: "y".into()
            })
            ,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(StoreError::InvalidEnvVar {
                env_var: "x".into()
            })
            ,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(StoreError::UnknownBuiltinSlug { slug: "x".into() }),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(StoreError::BorrowedReadOnly {
                name: "x".into(),
                owner: "y".into()
            })
            ,
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(StoreError::ShareConflict { reason: "x".into() }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(StoreError::GenericEnvVarChangeRequiresCredential { name: "x".into() }),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(StoreError::SourceCredentialUnreadable {
                source_provider: "x".into()
            })
            ,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn copy_error_status_variants_map_to_expected_status() {
        assert_eq!(
            ProviderApiError::Unauthorized { agent: "a".into() }
                .into_response()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            ProviderApiError::CopyConflict { reason: "x".into() }
                .into_response()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            ProviderApiError::BorrowedProviderReadOnly {
                name: "n".into(),
                owner: "o".into()
            }
            .into_response()
            .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            ProviderApiError::Internal("boom".into())
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}


#[cfg(test)]
mod plan_share_tests {
    use right_providers::{HeldProvider, plan_share, plan_unshare};

    use super::*;

    #[test]
    fn plan_share_rejects_self() {
        let e = plan_share("right", "right", "fal-a1b2c3", &[]).unwrap_err();
        assert!(matches!(e, StoreError::ShareConflict { .. }));
        // …and surfaces as 409 copy_conflict through the API mapping.
        assert!(matches!(
            store_err(e),
            ProviderApiError::CopyConflict { .. }
        ));
    }

    #[test]
    fn plan_share_rejects_dup_when_dest_already_has_record() {
        let existing = vec![HeldProvider::new("fal-a1b2c3", "agent-a")];
        let e = plan_share("agent-a", "right", "fal-a1b2c3", &existing).unwrap_err();
        assert!(matches!(e, StoreError::ShareConflict { .. }));
    }

    #[test]
    fn plan_share_accepts_new_record() {
        plan_share("agent-a", "right", "fal-a1b2c3", &[]).expect("share into a fresh dest is ok");
    }

    #[test]
    fn plan_share_rejects_dup_even_when_dest_borrows_same_name_from_elsewhere() {
        // Name uniqueness is per-holding-agent regardless of owner.
        let existing = vec![HeldProvider::new("fal-a1b2c3", "someone-else")];
        assert!(plan_share("agent-a", "right", "fal-a1b2c3", &existing).is_err());
    }

    #[test]
    fn plan_unshare_rejects_owned_entry() {
        let owned = HeldProvider::new("fal-a1b2c3", "right");
        assert!(matches!(
            plan_unshare("right", &owned).unwrap_err(),
            StoreError::ShareConflict { .. }
        ));
    }

    #[test]
    fn plan_unshare_accepts_borrowed_entry() {
        let borrowed = HeldProvider::new("fal-a1b2c3", "agent-a");
        plan_unshare("right", &borrowed).expect("borrowed entry can be unshared");
    }

    #[test]
    fn true_owner_points_past_the_intermediary() {
        let borrowed = HeldProvider::new("fal-a1b2c3", "agent-a");
        assert_eq!(right_providers::plan::true_owner(&borrowed), "agent-a");
    }
}

#[cfg(test)]
mod insert_tests {
    use super::*;

    #[test]
    fn insert_into_empty_sandbox() {
        let original = "name: foo\nsandbox:\n  mode: openshell\n";
        let entry = "    - name: foo-bar\n      type: anthropic\n";
        let out = insert_provider_entry(original, entry).unwrap();
        assert!(
            out.contains("providers:\n    - name: foo-bar"),
            "expected providers key followed by entry, got:\n{out}"
        );
    }

    #[test]
    fn insert_into_existing_providers() {
        let original = "sandbox:\n  mode: openshell\n  providers:\n    - name: x\n      type: y\n";
        let entry = "    - name: foo-bar\n      type: anthropic\n";
        let out = insert_provider_entry(original, entry).unwrap();
        assert!(
            out.contains("- name: foo-bar"),
            "new entry missing from:\n{out}"
        );
        assert!(
            out.contains("- name: x"),
            "existing entry missing from:\n{out}"
        );
    }

    #[test]
    fn replace_existing_provider_swaps_in_place() {
        let original = "sandbox:\n  providers:\n    - name: foo-x\n      type: anthropic\n    - name: foo-y\n      type: github\n";
        let new_entry = "    - name: foo-x\n      type: openai\n";
        let out = replace_provider_entry(original, "foo-x", new_entry).unwrap();
        assert!(
            out.contains("- name: foo-x\n      type: openai"),
            "replaced entry not found in:\n{out}"
        );
        assert!(
            out.contains("- name: foo-y\n      type: github"),
            "sibling entry missing from:\n{out}"
        );
        assert!(
            !out.contains("type: anthropic"),
            "old entry still present in:\n{out}"
        );
    }

    #[test]
    fn remove_provider_drops_entry_only() {
        let original = "sandbox:\n  providers:\n    - name: foo-x\n      type: anthropic\n    - name: foo-y\n      type: github\n";
        let out = remove_provider_entry(original, "foo-x");
        assert!(!out.contains("foo-x"), "foo-x still present in:\n{out}");
        assert!(
            out.contains("- name: foo-y"),
            "sibling entry missing from:\n{out}"
        );
    }

    /// New writes single-quote names, so subsequent remove/replace operations
    /// must locate quoted entries too — not just legacy unquoted ones.
    #[test]
    fn remove_provider_handles_quoted_name() {
        let original = "sandbox:\n  providers:\n    - name: 'foo-x'\n      type: 'anthropic'\n    - name: 'foo-y'\n      type: 'github'\n";
        let out = remove_provider_entry(original, "foo-x");
        assert!(!out.contains("foo-x"), "foo-x still present in:\n{out}");
        assert!(
            out.contains("- name: 'foo-y'"),
            "sibling entry missing from:\n{out}"
        );
    }

    #[test]
    fn replace_provider_handles_quoted_name() {
        let original = "sandbox:\n  providers:\n    - name: 'foo-x'\n      type: 'anthropic'\n    - name: 'foo-y'\n      type: 'github'\n";
        let new_entry = "    - name: 'foo-x'\n      type: 'openai'\n";
        let out = replace_provider_entry(original, "foo-x", new_entry).unwrap();
        assert!(
            out.contains("- name: 'foo-x'\n      type: 'openai'"),
            "replaced entry not found in:\n{out}"
        );
        assert!(
            !out.contains("type: 'anthropic'"),
            "old entry still present in:\n{out}"
        );
    }

    /// Searching for an unquoted legacy entry whose name is a prefix of a
    /// longer name must not match the longer entry.
    #[test]
    fn find_provider_name_marker_unquoted_does_not_match_prefix() {
        let haystack =
            "sandbox:\n  providers:\n    - name: myagent-foo-bar\n      type: anthropic\n";
        assert_eq!(find_provider_name_marker(haystack, "myagent-foo"), None);
    }

    /// With both an exact unquoted entry and a longer-name entry present, the
    /// search must return the offset of the exact match.
    #[test]
    fn find_provider_name_marker_unquoted_matches_only_exact_followed_by_newline() {
        let haystack = "sandbox:\n  providers:\n    - name: myagent-foo\n      type: anthropic\n    - name: myagent-foo-bar\n      type: github\n";
        let foo_idx = find_provider_name_marker(haystack, "myagent-foo")
            .expect("exact unquoted match should resolve");
        let foo_bar_idx = find_provider_name_marker(haystack, "myagent-foo-bar")
            .expect("longer unquoted match should resolve");
        assert!(
            foo_idx < foo_bar_idx,
            "exact match must come before longer-name match: foo={foo_idx} foo-bar={foo_bar_idx}"
        );
        assert_eq!(
            &haystack[foo_idx..foo_idx + "    - name: myagent-foo".len()],
            "    - name: myagent-foo"
        );
        assert_eq!(
            &haystack[foo_bar_idx..foo_bar_idx + "    - name: myagent-foo-bar".len()],
            "    - name: myagent-foo-bar"
        );
    }

    /// Quoted form is bounded by the closing quote, so prefix collisions are
    /// impossible.
    #[test]
    fn find_provider_name_marker_quoted_matches() {
        let haystack =
            "sandbox:\n  providers:\n    - name: 'myagent-foo'\n      type: 'anthropic'\n";
        let idx = find_provider_name_marker(haystack, "myagent-foo")
            .expect("quoted match should resolve");
        assert_eq!(
            &haystack[idx..idx + "    - name: 'myagent-foo'".len()],
            "    - name: 'myagent-foo'"
        );
    }

    /// Removing the shorter name must drop the shorter row and leave the
    /// longer-name sibling untouched.
    #[test]
    fn remove_provider_entry_unquoted_does_not_remove_wrong_row() {
        let original = "sandbox:\n  providers:\n    - name: myagent-foo\n      type: anthropic\n    - name: myagent-foo-bar\n      type: github\n";
        let out = remove_provider_entry(original, "myagent-foo");
        assert!(
            out.contains("- name: myagent-foo-bar\n      type: github"),
            "longer-name sibling must be preserved in:\n{out}"
        );
        assert!(
            !out.contains("- name: myagent-foo\n      type: anthropic"),
            "exact entry should have been removed from:\n{out}"
        );
    }
}


#[cfg(test)]
mod handler_tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use tower::ServiceExt;

    use super::{
        Credential, GenericSpec, NewProvider, ProviderApiError, ProviderConfigUpdateGeneric,
        ProviderConfigUpdateReq, ProviderKind, ProviderRemoveReq, ProviderRotateReq,
        handle_provider_config_update, handle_provider_remove, handle_provider_rotate,
    };

    /// Build a minimal internal router pointed at `agents_dir`, backed by a
    /// real `ProviderStore` in the temp home. Mirrors `make_test_router` from
    /// internal_api.rs.
    async fn make_provider_test_router(tmp: &std::path::Path) -> axum::Router {
        use crate::aggregator::{AgentInfo, BackendRegistry};
        use crate::right_backend::RightBackend;
        use dashmap::DashMap;
        use std::collections::HashMap;
        use std::sync::Arc;

        let agents_dir = tmp.join("agents");
        // Ensure the dispatcher has a known agent so token auth passes,
        // but the test agent ("hostagent") only needs the agent.yaml on disk.
        let placeholder_dir = agents_dir.join("hostagent");
        std::fs::create_dir_all(&placeholder_dir).unwrap();
        right_db::open_db(&placeholder_dir, true).await.unwrap();

        let right = RightBackend::new(agents_dir.clone(), None);
        let registry = BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            agent_dir: placeholder_dir.clone(),
            hindsight: None,
        };
        let agents = DashMap::new();
        agents.insert("hostagent".into(), registry);
        let dispatcher = Arc::new(crate::aggregator::ToolDispatcher { agents });

        let refresh_senders: crate::aggregator::RefreshSenders =
            Arc::new(std::collections::HashMap::new());
        let reconnect_managers: crate::aggregator::ReconnectManagers =
            Arc::new(std::collections::HashMap::new());

        let token_map_path = tmp.join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"hostagent": "tok-test"}).to_string(),
        )
        .unwrap();
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "tok-test".into(),
                AgentInfo {
                    name: "hostagent".into(),
                    dir: placeholder_dir,
                },
            );
            Arc::new(tokio::sync::RwLock::new(map))
        };

        let providers = right_providers::ProviderStore::open(tmp)
            .await
            .expect("temp provider store");
        crate::internal_api::internal_router(
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            agents_dir,
            providers,
        )
    }

    /// Build a minimal `InternalState` rooted at `tmp/agents` with a store in
    /// `tmp`, so unit tests can exercise `provider_lock` and the agent.yaml
    /// RMW writer directly without going through the axum router.
    async fn make_provider_test_state(tmp: &std::path::Path) -> crate::internal_api::InternalState {
        use crate::aggregator::{AgentInfo, BackendRegistry};
        use crate::right_backend::RightBackend;
        use dashmap::DashMap;
        use std::collections::HashMap;
        use std::sync::Arc;

        let agents_dir = tmp.join("agents");
        let placeholder_dir = agents_dir.join("hostagent");
        std::fs::create_dir_all(&placeholder_dir).unwrap();
        right_db::open_db(&placeholder_dir, true).await.unwrap();

        let right = RightBackend::new(agents_dir.clone(), None);
        let registry = BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            agent_dir: placeholder_dir.clone(),
            hindsight: None,
        };
        let agents = DashMap::new();
        agents.insert("hostagent".into(), registry);
        let dispatcher = Arc::new(crate::aggregator::ToolDispatcher { agents });

        let refresh_senders: crate::aggregator::RefreshSenders =
            Arc::new(std::collections::HashMap::new());
        let reconnect_managers: crate::aggregator::ReconnectManagers =
            Arc::new(std::collections::HashMap::new());

        let token_map_path = tmp.join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"hostagent": "tok-test"}).to_string(),
        )
        .unwrap();
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "tok-test".into(),
                AgentInfo {
                    name: "hostagent".into(),
                    dir: placeholder_dir,
                },
            );
            Arc::new(tokio::sync::RwLock::new(map))
        };

        let providers = right_providers::ProviderStore::open(tmp)
            .await
            .expect("temp provider store");
        crate::internal_api::InternalState::new_for_test(
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            agents_dir,
            providers,
        )
    }

    #[tokio::test]
    async fn provider_create_generic_rejected_in_restrictive_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // Generic providers are only supported under the permissive network
        // policy, so creation must be refused up-front.
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "network_policy: restrictive\n\
             sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;

        let req = Request::builder()
            .method("POST")
            .uri("/provider-create")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "type": "generic",
                    "label": "acme",
                    "credential": "secret-value",
                    "generic": {
                        "env_var": "ACME_API_KEY",
                        "upstream_host": "api.acme.invalid",
                    },
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "network_policy_forbids_generic",
            "expected code=network_policy_forbids_generic, got: {json}"
        );
    }

    #[tokio::test]
    async fn provider_rotate_fails_fast_on_unknown_builtin() {
        // If an agent.yaml has a BuiltIn(slug) that the catalog no longer
        // knows about (e.g. catalog renamed/dropped), rotating that provider
        // MUST fail with HTTP 500 + code=unknown_builtin_slug rather than
        // silently inserting "" as the credential key (AGENTS.rust.md §2
        // FAIL-FAST). The gateway-era code surfaced this from
        // `extract_env_var`; the store era surfaces it when the stored row's
        // slug fails to resolve.
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        // Seed the stale row directly into the store (the create path would
        // reject an unknown slug, so simulate a catalog drift).
        let store = right_providers::ProviderStore::open(tmp.path())
            .await
            .expect("temp provider store");
        seed_stale_builtin(&store, "hostagent", "hostagent-stale", "definitely-not-a-real-slug")
            .await;

        let app = make_provider_test_router(tmp.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/provider-rotate")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "name": "hostagent-stale",
                    "credential": "new-secret",
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "unknown built-in slug must surface as 500, not silent success"
        );
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "unknown_builtin_slug",
            "expected code=unknown_builtin_slug, got: {json}"
        );
    }

    /// Insert a builtin row whose slug is NOT in the catalog, bypassing
    /// `ProviderStore::create`'s catalog validation, to model a record that
    /// drifted out of the catalog after creation.
    async fn seed_stale_builtin(
        store: &right_providers::ProviderStore,
        agent: &str,
        name: &str,
        slug: &str,
    ) {
        store
            .seed_builtin_unchecked(agent, name, slug, "STALE_KEY")
            .await
            .expect("seed stale builtin row");
    }

    #[tokio::test]
    async fn provider_list_marks_unknown_builtin_as_status_error() {
        // List view must NOT abort when a single entry references an unknown
        // slug: the bad row is marked status.kind=error so the operator sees
        // it; rotation on that same row still fails fast (see above).
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        let store = right_providers::ProviderStore::open(tmp.path())
            .await
            .expect("temp provider store");
        seed_stale_builtin(&store, "hostagent", "hostagent-stale", "definitely-not-a-real-slug")
            .await;

        let app = make_provider_test_router(tmp.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/provider-list")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({"agent": "hostagent"})).unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let arr: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let entries = arr.as_array().expect("list returns an array");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "hostagent-stale");
        assert_eq!(
            entries[0]["status"]["kind"], "error",
            "stale row must be marked, not silently emptied: {entries:?}"
        );
        let msg = entries[0]["status"]["message"].as_str().unwrap();
        assert!(
            msg.contains("definitely-not-a-real-slug"),
            "message must name the slug: {msg}"
        );
        // The stored env var ("") is what the row reports; the record keeps
        // whatever was stored at creation rather than resolving to nothing.
        assert_eq!(entries[0]["env_var"], "STALE_KEY");
    }

    #[tokio::test]
    async fn provider_create_tolerates_unknown_builtin_row() {
        // If providers.db carries a stale row whose slug is no longer in the
        // catalog, the env-var collision check inside create must skip it
        // rather than locking the operator out of adding ANY new provider.
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        let store = right_providers::ProviderStore::open(tmp.path())
            .await
            .expect("temp provider store");
        seed_stale_builtin(&store, "hostagent", "hostagent-stale", "definitely-not-a-real-slug")
            .await;

        let app = make_provider_test_router(tmp.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/provider-create")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "type": "gitlab",
                    "credential": "glpat-test",
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "stale unknown_builtin row must not block creating a different provider; body={json}"
        );
    }

    #[tokio::test]
    async fn provider_config_update_rejects_invalid_name() {
        // /provider-config-update must validate `name` before acquiring the
        // per-agent lock or touching agent.yaml.
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/provider-config-update")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "name": "../bad",
                    "generic": {
                        "upstream_host": "api.acme.invalid",
                    },
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "invalid_name",
            "expected code=invalid_name, got: {json}"
        );
    }

    /// Many concurrent provider mutations for DISTINCT providers on the SAME
    /// agent must all end up in agent.yaml. Keying `provider_lock` on
    /// `(agent, name)` would let different names take different locks, and
    /// last-write-wins RMW would silently drop entries (store already
    /// mutated, agent.yaml an orphan). The per-agent lock serializes the
    /// whole read+write window.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_provider_create_serializes_on_same_agent() {
        use super::{load_agent_config, provider_lock, serialize_provider_entry};

        let tmp = tempfile::tempdir().unwrap();
        let state = make_provider_test_state(tmp.path()).await;
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        // N distinct provider entries for the SAME agent.
        const N: usize = 5;
        let entries: Vec<_> = (0..N)
            .map(|i| right_agent_config::ProviderEntry {
                name: format!("hostagent-p{i:02}"),
                type_: right_agent_config::ProviderType::Generic,
                label: Some(format!("p{i:02}")),
                generic: Some(right_agent_config::GenericProvider {
                    env_var: format!("KEY_{i:02}"),
                    upstream_hosts: vec![format!("api{i:02}.example.com")],
                    upstream_path_prefix: None,
                }),
                shared_from: None,
            })
            .collect();

        // Spawn N tasks each performing a guarded RMW with a deliberate
        // sleep between read and write, widening the race window.
        let agent_yaml = state.agents_dir.join("hostagent").join("agent.yaml");
        let mut tasks = Vec::with_capacity(N);
        for entry in entries.iter().cloned() {
            let state = state.clone();
            let agent_yaml = agent_yaml.clone();
            tasks.push(tokio::spawn(async move {
                let _guard = provider_lock(&state, "hostagent").await;
                let existing = tokio::fs::read_to_string(&agent_yaml).await.unwrap();
                // Hold open the RMW window: under a per-name lock every task
                // reaches this sleep concurrently; under the per-agent lock
                // the next task is still blocked on `provider_lock`.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let updated =
                    super::insert_provider_entry(&existing, &serialize_provider_entry(&entry))
                        .unwrap();
                tokio::fs::write(&agent_yaml, updated).await.unwrap();
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }

        let cfg = load_agent_config(&state.agents_dir, "hostagent")
            .expect("agent.yaml must parse after concurrent appends");
        let names: std::collections::BTreeSet<_> = cfg
            .sandbox
            .as_ref()
            .expect("sandbox section present")
            .providers
            .iter()
            .map(|p| p.name.clone())
            .collect();
        for entry in &entries {
            assert!(
                names.contains(&entry.name),
                "{} dropped from agent.yaml (RMW race): {names:?}",
                entry.name
            );
        }
        assert_eq!(
            names.len(),
            N,
            "expected exactly {N} providers after concurrent create, got: {names:?}"
        );
    }

    #[tokio::test]
    async fn provider_remove_rejects_borrowed_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = make_provider_test_state(tmp.path()).await;
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        // Seed an owned record on the true owner, then borrow it here.
        state
            .providers
            .create(
                NewProvider {
                    owner_agent: "owner-agent".into(),
                    name: "shared-key".into(),
                    kind: ProviderKind::Builtin("right-fal".into()),
                    label: "seeded".into(),
                },
                Credential::from("owner-secret".to_string()),
            )
            .await
            .expect("seed owner record");
        state
            .providers
            .share("owner-agent", "shared-key", "hostagent")
            .await
            .expect("seed borrow");

        let req = ProviderRemoveReq {
            agent: "hostagent".into(),
            name: "shared-key".into(),
        };
        let result = handle_provider_remove(axum::extract::State(state), axum::Json(req)).await;
        assert!(
            matches!(
                result,
                Err(ProviderApiError::BorrowedProviderReadOnly { .. })
            ),
            "borrowed entry must be rejected: {result:?}"
        );
    }

    #[tokio::test]
    async fn provider_rotate_rejects_borrowed_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = make_provider_test_state(tmp.path()).await;
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        state
            .providers
            .create(
                NewProvider {
                    owner_agent: "owner-agent".into(),
                    name: "shared-key".into(),
                    kind: ProviderKind::Builtin("right-fal".into()),
                    label: "seeded".into(),
                },
                Credential::from("owner-secret".to_string()),
            )
            .await
            .expect("seed owner record");
        state
            .providers
            .share("owner-agent", "shared-key", "hostagent")
            .await
            .expect("seed borrow");

        let req = ProviderRotateReq {
            agent: "hostagent".into(),
            name: "shared-key".into(),
            credential: secrecy::SecretString::from("secret"),
        };
        let result = handle_provider_rotate(axum::extract::State(state), axum::Json(req)).await;
        assert!(
            matches!(
                result,
                Err(ProviderApiError::BorrowedProviderReadOnly { .. })
            ),
            "borrowed entry must be rejected: {result:?}"
        );
    }

    #[tokio::test]
    async fn provider_config_update_rejects_borrowed_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = make_provider_test_state(tmp.path()).await;
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        state
            .providers
            .create(
                NewProvider {
                    owner_agent: "owner-agent".into(),
                    name: "shared-key".into(),
                    kind: ProviderKind::Generic(GenericSpec {
                        env_var: "SHARED_KEY".into(),
                        upstream_hosts: vec!["api.example.com".into()],
                        upstream_path_prefix: None,
                    }),
                    label: "seeded".into(),
                },
                Credential::from("owner-secret".to_string()),
            )
            .await
            .expect("seed owner record");
        state
            .providers
            .share("owner-agent", "shared-key", "hostagent")
            .await
            .expect("seed borrow");

        let req = ProviderConfigUpdateReq {
            agent: "hostagent".into(),
            name: "shared-key".into(),
            generic: ProviderConfigUpdateGeneric {
                env_var: None,
                upstream_host: None,
                upstream_hosts: None,
                upstream_path_prefix: None,
            },
        };
        let result =
            handle_provider_config_update(axum::extract::State(state), axum::Json(req)).await;
        assert!(
            matches!(
                result,
                Err(ProviderApiError::BorrowedProviderReadOnly { .. })
            ),
            "borrowed entry must be rejected: {result:?}"
        );
    }

    #[tokio::test]
    async fn provider_list_rejects_sandbox_mode_none() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // Write agent.yaml with mode: none — sandbox-mode guard should fire.
        std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();

        let app = make_provider_test_router(tmp.path()).await;

        let req = Request::builder()
            .method("POST")
            .uri("/provider-list")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({"agent": "hostagent"})).unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "sandbox_mode_none",
            "expected code=sandbox_mode_none, got: {json}"
        );
    }

    #[tokio::test]
    async fn provider_list_empty_for_fresh_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  name: hostagent\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/provider-list")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({"agent": "hostagent"})).unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let arr: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 0);
    }
}


#[cfg(test)]
mod provider_types_tests {
    use super::*;

    #[tokio::test]
    async fn provider_types_hides_builtin_github_and_shows_right_github() {
        let axum::Json(types) = handle_provider_types().await;
        assert!(
            types.iter().all(|t| t.type_slug != "github"),
            "built-in read-only github is hidden from the dashboard"
        );
        assert!(
            types
                .iter()
                .any(|t| t.type_slug == "right-github" && t.display_name == "GitHub"),
            "right-github offered as GitHub"
        );
        assert!(
            types.iter().any(|t| t.type_slug == "gitlab"),
            "filter is narrow — other built-ins (gitlab) still offered"
        );
    }

    #[tokio::test]
    async fn every_hidden_catalog_entry_stays_offered_only_to_existing_records() {
        // Invariant: hidden catalog entries resolve (so existing records keep
        // working) but are never offered as new provider types.
        let axum::Json(types) = handle_provider_types().await;
        for p in ProviderStore::catalog() {
            if p.hidden {
                assert!(
                    types.iter().all(|t| t.type_slug != p.slug),
                    "hidden catalog entry {} must not be offered",
                    p.slug
                );
            } else {
                assert!(
                    types.iter().any(|t| t.type_slug == p.slug),
                    "visible catalog entry {} must be offered",
                    p.slug
                );
            }
        }
    }

    #[tokio::test]
    async fn provider_types_categories_render_lowercase() {
        let axum::Json(types) = handle_provider_types().await;
        for t in &types {
            assert_eq!(
                t.category,
                t.category.to_lowercase(),
                "category must render lowercase: {t:?}"
            );
            assert!(!t.category.is_empty());
        }
    }
}

#[cfg(test)]
mod peers_tests {
    use super::*;

    async fn open_store(dir: &std::path::Path) -> ProviderStore {
        ProviderStore::open(dir).await.expect("temp provider store")
    }

    fn write_agent(dir: &std::path::Path, name: &str, allow_ids: &[i64], providers_yaml: &str) {
        let agent_dir = dir.join(name);
        std::fs::create_dir_all(&agent_dir).unwrap();
        let users = allow_ids
            .iter()
            .map(|id| format!("  - id: {id}\n    added_at: 2026-01-01T00:00:00Z"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            agent_dir.join("allowlist.yaml"),
            format!("version: 2\nusers:\n{users}\n"),
        )
        .unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            format!("sandbox:\n  mode: openshell\n{providers_yaml}"),
        )
        .unwrap();
    }

    async fn create_builtin(store: &ProviderStore, agent: &str, name: &str, slug: &str) {
        store
            .create(
                NewProvider {
                    owner_agent: agent.into(),
                    name: name.into(),
                    kind: ProviderKind::Builtin(slug.into()),
                    label: "seeded".into(),
                },
                Credential::from("peer-secret".to_string()),
            )
            .await
            .expect("seed peer provider");
    }

    #[test]
    fn require_trusted_accepts_member_rejects_others() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "agent-a", &[7], "  providers: []\n");
        assert!(require_trusted(tmp.path(), "agent-a", 7).is_ok());
        let err = require_trusted(tmp.path(), "agent-a", 99).unwrap_err();
        assert!(matches!(err, ProviderApiError::Unauthorized { .. }));
    }

    #[test]
    fn require_trusted_rejects_when_no_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("nolst");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  providers: []\n",
        )
        .unwrap();
        // Missing allowlist = secure default: deny all
        let err = require_trusted(tmp.path(), "nolst", 7).unwrap_err();
        assert!(matches!(err, ProviderApiError::Unauthorized { .. }));
    }

    #[tokio::test]
    async fn build_peers_excludes_non_openshell_agent() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        // trusted peer, but host-mode sandbox → must be excluded
        let agent_dir = tmp.path().join("hostmode");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("allowlist.yaml"),
            "version: 2\nusers:\n  - id: 7\n    added_at: 2026-01-01T00:00:00Z\n",
        )
        .unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: none\n  providers: []\n",
        )
        .unwrap();

        let store = open_store(tmp.path()).await;
        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        assert!(peers.iter().all(|p| p.agent != "hostmode"));
    }

    #[tokio::test]
    async fn build_peers_excludes_agent_with_no_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        // Peer with no allowlist.yaml at all
        let no_allow_dir = tmp.path().join("nolst");
        std::fs::create_dir_all(&no_allow_dir).unwrap();
        std::fs::write(
            no_allow_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  providers:\n    - name: nolst-fal\n      type: right-fal\n",
        )
        .unwrap();

        let store = open_store(tmp.path()).await;
        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        assert!(peers.is_empty(), "peer with no allowlist must be excluded");
    }

    #[tokio::test]
    async fn build_peers_skips_peer_with_corrupt_allowlist_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        // A healthy, trusted peer that must still be returned.
        write_agent(tmp.path(), "healthy", &[7], "  providers: []\n");
        // A peer whose allowlist.yaml is corrupt (users is not a sequence):
        // it must be skipped, never abort the whole listing.
        let bad_dir = tmp.path().join("corrupt");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(
            bad_dir.join("allowlist.yaml"),
            "version: 2\nusers: not-a-list\n",
        )
        .unwrap();
        std::fs::write(
            bad_dir.join("agent.yaml"),
            "sandbox:\n  mode: openshell\n  providers: []\n",
        )
        .unwrap();

        let store = open_store(tmp.path()).await;
        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        assert_eq!(
            peers.len(),
            1,
            "corrupt-allowlist peer skipped, healthy kept"
        );
        assert_eq!(peers[0].agent, "healthy");
    }

    #[tokio::test]
    async fn build_peers_excludes_self_and_untrusted_and_reports_providers() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        write_agent(tmp.path(), "agent-a", &[7], "  providers: []\n");
        write_agent(tmp.path(), "secret", &[42], "  providers: []\n");

        let store = open_store(tmp.path()).await;
        create_builtin(&store, "agent-a", "agent-a-provider", "right-fal").await;

        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        let names: Vec<&str> = peers.iter().map(|p| p.agent.as_str()).collect();
        assert_eq!(names, vec!["agent-a"]); // self + untrusted filtered
        assert_eq!(peers[0].providers.len(), 1);
        assert_eq!(peers[0].providers[0].name, "agent-a-provider");
        assert_eq!(peers[0].providers[0].env_var, "FAL_KEY");
        assert_eq!(peers[0].network_policy, "permissive");
    }

    #[tokio::test]
    async fn build_peers_reads_borrowed_rows_without_credentials() {
        // A peer whose provider is BORROWED still lists it (name/env/type
        // only); the credential value never crosses the read path.
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        write_agent(tmp.path(), "agent-a", &[7], "  providers: []\n");
        write_agent(tmp.path(), "borrower", &[7], "  providers: []\n");

        let store = open_store(tmp.path()).await;
        create_builtin(&store, "agent-a", "agent-a-provider", "right-fal").await;
        store
            .share("agent-a", "agent-a-provider", "borrower")
            .await
            .expect("share to borrower");

        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        let borrower = peers.iter().find(|p| p.agent == "borrower").unwrap();
        assert_eq!(borrower.providers.len(), 1);
        assert_eq!(borrower.providers[0].name, "agent-a-provider");
        assert_eq!(borrower.providers[0].env_var, "FAL_KEY");
        // PeerProvider has no credential field at all — structural redaction.
    }
}
