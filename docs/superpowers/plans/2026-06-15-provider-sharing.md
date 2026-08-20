# Provider Sharing (Cross-Agent Import / Export) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a dashboard operator who is trusted on two agents copy a provider — credential included — from one agent to another (import = pull, export = push), with an overwrite mode that re-syncs a rotated key in place.

**Architecture:** One internal-API primitive `provider_copy(actor, source_agent, source_provider, dest_agent, label?, overwrite)` plus a read-only `provider_peers` discovery, both keyed off the requesting Telegram user id. The dashboard pins one side to its own agent (import → dest = current; export → source = current) and forwards the authenticated user id; the internal API re-checks the *other* agent's `allowlist.yaml`. Copy reuses the existing create / rotate / config-update handlers; the source credential is read back from the gateway via a new `get_provider_credentials`. Match key for overwrite is `env_var` (unique per agent). It is always a copy — never a shared gateway provider.

**Tech Stack:** Rust (axum internal API, `right-openshell` gRPC, `secrecy`), Vue 3 + TypeScript dashboard (vitest), OpenShell gateway.

**Spec:** `docs/superpowers/specs/2026-06-15-provider-sharing-design.md`

---

## Before you start

- Work in a dedicated worktree under the repo's `.worktrees/` (this shared
  checkout otherwise churns `master`). Land via fast-forward to
  `origin/master` at the end.
- Baseline (record any pre-existing failures, do not fix unrelated ones):

  ```bash
  devenv shell -- cargo nextest run -p right-openshell -p right -p bot
  devenv shell -- pnpm -C crates/right-dashboard/frontend exec vitest run
  ```

- Reference reading: spec above; `ARCHITECTURE.md` → "Providers";
  `docs/architecture/providers.md`.

---

## Task 1: `get_provider_credentials` (credential read-back)

The single sanctioned read of a stored secret back from the gateway. The
raw `GetProvider` proto carries credentials (the public `get_provider`
deliberately drops them). Wrap each value in `SecretString`.

**Files:**
- Modify: `crates/right-openshell/Cargo.toml`
- Modify: `crates/right-openshell/src/providers.rs` (add helpers near
  `get_provider`, ~line 254; tests in the existing `#[cfg(test)]` module
  near line 834)

- [ ] **Step 1: Add the `secrecy` dependency**

In `crates/right-openshell/Cargo.toml`, under `[dependencies]`, add:

```toml
secrecy = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in
`crates/right-openshell/src/providers.rs`:

```rust
#[test]
fn credentials_from_proto_wraps_each_value_as_secret() {
    use secrecy::ExposeSecret;
    let mut credentials = HashMap::new();
    credentials.insert("FAL_KEY".to_string(), "secret-abc".to_string());
    let p = datamodel::Provider {
        metadata: Some(datamodel::ObjectMeta {
            name: "agent-fal".into(),
            ..Default::default()
        }),
        r#type: "right-fal".into(),
        credentials,
        config: Default::default(),
        credential_expires_at_ms: Default::default(),
    };

    let out = credentials_from_proto(&p);

    assert_eq!(out.len(), 1);
    assert_eq!(out.get("FAL_KEY").unwrap().expose_secret(), "secret-abc");
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `devenv shell -- cargo nextest run -p right-openshell credentials_from_proto`
Expected: FAIL to compile — `credentials_from_proto` not found.

- [ ] **Step 4: Implement the helpers**

Add to `crates/right-openshell/src/providers.rs` right after `get_provider`
(the function ending near line 261):

```rust
/// Wrap each gateway-stored credential value in `SecretString`.
fn credentials_from_proto(
    p: &datamodel::Provider,
) -> HashMap<String, secrecy::SecretString> {
    p.credentials
        .iter()
        .map(|(k, v)| (k.clone(), secrecy::SecretString::from(v.clone())))
        .collect()
}

/// Read a provider's stored credentials back from the gateway.
///
/// Unlike [`get_provider`], this exposes the credential bytes — the single
/// sanctioned read-back, used only to copy a provider between agents on the
/// host. Values are `SecretString`; never log them, never persist them to
/// `agent.yaml`, backups, or list/detail responses.
pub async fn get_provider_credentials(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> Result<HashMap<String, secrecy::SecretString>, ProviderError> {
    let proto = get_provider_proto(client, name).await?;
    Ok(credentials_from_proto(&proto))
}
```

- [ ] **Step 5: Run it to verify it passes**

Run: `devenv shell -- cargo nextest run -p right-openshell credentials_from_proto`
Expected: PASS.

- [ ] **Step 6: Add a live gateway test**

Append to `crates/right-openshell/tests/ci_openshell_provider.rs` (this uses
the exact client construction and naming style of the existing
`ci_openshell_provider_create_get_delete_roundtrip` in that file):

```rust
#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_get_provider_credentials_returns_stored_secret() {
    use right_openshell::providers::*;
    use secrecy::ExposeSecret;
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();

    let name = format!("rightprobe-{}-getcreds", std::process::id());
    let mut creds = std::collections::HashMap::new();
    creds.insert("FAL_KEY".to_string(), "live-secret-xyz".to_string());
    let spec = ProviderSpec {
        name: name.clone(),
        type_: "generic".into(),
        credentials: creds,
        config: Default::default(),
    };
    create_provider(&mut client, &spec).await.unwrap();

    let out = get_provider_credentials(&mut client, &name).await.unwrap();
    assert_eq!(out.get("FAL_KEY").unwrap().expose_secret(), "live-secret-xyz");

    delete_provider(&mut client, &name).await.unwrap();
}
```

- [ ] **Step 7: Commit**

```bash
git add crates/right-openshell/Cargo.toml crates/right-openshell/src/providers.rs crates/right-openshell/tests/ci_openshell_provider.rs
git commit -m "feat(openshell): add get_provider_credentials for cross-agent copy"
```

---

## Task 2: New `ProviderApiError` variants

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs` (enum at line 3, the
  `into_response` match at line 38, tests in a `#[cfg(test)]` block)

- [ ] **Step 1: Write the failing test**

Add a new test module near the top tests in
`crates/right/src/internal_api_providers.rs`:

```rust
#[cfg(test)]
mod copy_error_status_tests {
    use super::*;
    use axum::response::IntoResponse;
    use axum::http::StatusCode;

    #[test]
    fn new_error_variants_map_to_expected_status() {
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
            ProviderApiError::Internal("boom".into())
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `devenv shell -- cargo nextest run -p right new_error_variants_map_to_expected_status`
Expected: FAIL to compile — variants not defined.

- [ ] **Step 3: Add the variants**

In the `pub enum ProviderApiError` block (line 3), add:

```rust
    #[error("not a trusted dashboard user on agent \"{agent}\"")]
    Unauthorized { agent: String },
    #[error("copy conflict: {reason}")]
    CopyConflict { reason: String },
    #[error("internal error: {0}")]
    Internal(String),
```

In the `into_response` match (line 38), add arms before the closing `}`:

```rust
            Self::Unauthorized { .. } => (StatusCode::FORBIDDEN, "unauthorized"),
            Self::CopyConflict { .. } => (StatusCode::CONFLICT, "copy_conflict"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
```

- [ ] **Step 4: Run it to verify it passes**

Run: `devenv shell -- cargo nextest run -p right new_error_variants_map_to_expected_status`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(providers): add error variants for cross-agent copy"
```

---

## Task 3: Pure `plan_copy` decision logic

The create-vs-overwrite decision, type-compatibility, and label defaulting
— pure, fully unit-tested, no fs/gateway.

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs`

- [ ] **Step 1: Write the failing tests**

Add a test module:

```rust
#[cfg(test)]
mod plan_copy_tests {
    use super::*;

    fn generic(name: &str, env: &str, hosts: &[&str], path: Option<&str>)
        -> right_agent_config::ProviderEntry {
        right_agent_config::ProviderEntry {
            name: name.into(),
            type_: right_agent_config::ProviderType::Generic,
            label: None,
            generic: Some(right_agent_config::GenericProvider {
                env_var: env.into(),
                upstream_hosts: hosts.iter().map(|h| h.to_string()).collect(),
                upstream_path_prefix: path.map(|p| p.to_string()),
            }),
        }
    }
    fn builtin(name: &str, slug: &str) -> right_agent_config::ProviderEntry {
        right_agent_config::ProviderEntry {
            name: name.into(),
            type_: right_agent_config::ProviderType::BuiltIn(slug.into()),
            label: None,
            generic: None,
        }
    }

    #[test]
    fn create_when_no_env_var_match() {
        let src = builtin("riskoff-fal", "right-fal");
        let plan = plan_copy("riskoff", &src, "FAL_KEY", &[], false, None).unwrap();
        match plan {
            CopyPlan::Create { type_, label, generic } => {
                assert_eq!(type_, "right-fal");
                assert_eq!(label.as_deref(), Some("fal"));
                assert!(generic.is_none());
            }
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn create_label_override_wins() {
        let src = builtin("riskoff-fal", "right-fal");
        let plan = plan_copy("riskoff", &src, "FAL_KEY", &[], false, Some("media")).unwrap();
        match plan {
            CopyPlan::Create { label, .. } => assert_eq!(label.as_deref(), Some("media")),
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn create_collision_without_overwrite_errors() {
        let src = builtin("riskoff-fal", "right-fal");
        let dest = vec![builtin("other-fal", "right-fal")];
        let err = plan_copy("riskoff", &src, "FAL_KEY", &dest, false, None).unwrap_err();
        assert!(matches!(err, ProviderApiError::EnvVarCollision { .. }));
    }

    #[test]
    fn overwrite_without_match_errors() {
        let src = builtin("riskoff-fal", "right-fal");
        let err = plan_copy("riskoff", &src, "FAL_KEY", &[], true, None).unwrap_err();
        assert!(matches!(err, ProviderApiError::CopyConflict { .. }));
    }

    #[test]
    fn overwrite_type_mismatch_errors() {
        let src = generic("riskoff-fal", "FAL_KEY", &["fal.run"], Some("/v1"));
        let dest = vec![builtin("other-fal", "right-fal")];
        let err = plan_copy("riskoff", &src, "FAL_KEY", &dest, true, None).unwrap_err();
        assert!(matches!(err, ProviderApiError::CopyConflict { .. }));
    }

    #[test]
    fn overwrite_builtin_is_credential_only() {
        let src = builtin("riskoff-fal", "right-fal");
        let dest = vec![builtin("other-fal", "right-fal")];
        let plan = plan_copy("riskoff", &src, "FAL_KEY", &dest, true, None).unwrap();
        match plan {
            CopyPlan::Overwrite { dest_name, resync_generic } => {
                assert_eq!(dest_name, "other-fal");
                assert!(resync_generic.is_none());
            }
            _ => panic!("expected Overwrite"),
        }
    }

    #[test]
    fn overwrite_generic_resyncs_only_when_config_differs() {
        let src = generic("riskoff-fal", "FAL_KEY", &["fal.run", "queue.fal.run"], Some("/v1"));
        // identical config → no resync
        let same = vec![generic("other-fal", "FAL_KEY", &["fal.run", "queue.fal.run"], Some("/v1"))];
        match plan_copy("riskoff", &src, "FAL_KEY", &same, true, None).unwrap() {
            CopyPlan::Overwrite { resync_generic, .. } => assert!(resync_generic.is_none()),
            _ => panic!("expected Overwrite"),
        }
        // different hosts → resync
        let diff = vec![generic("other-fal", "FAL_KEY", &["fal.run"], Some("/v1"))];
        match plan_copy("riskoff", &src, "FAL_KEY", &diff, true, None).unwrap() {
            CopyPlan::Overwrite { resync_generic, .. } => {
                let g = resync_generic.expect("resync");
                assert_eq!(g.upstream_hosts, vec!["fal.run", "queue.fal.run"]);
            }
            _ => panic!("expected Overwrite"),
        }
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `devenv shell -- cargo nextest run -p right plan_copy_tests`
Expected: FAIL to compile — `CopyPlan` / `plan_copy` not defined.

- [ ] **Step 3: Implement `CopyPlan` + `plan_copy`**

Add to `crates/right/src/internal_api_providers.rs` (near the other helper
fns, e.g. after `extract_env_var`):

```rust
/// What a copy resolves to once the source provider and the destination's
/// existing providers are known. Carries everything except the credential
/// (read separately from the gateway).
pub(crate) enum CopyPlan {
    Create {
        type_: String,
        label: Option<String>,
        generic: Option<ProviderCreateGeneric>,
    },
    Overwrite {
        dest_name: String,
        /// `Some` → re-sync the destination generic config to this; `None`
        /// → credential-only rotate.
        resync_generic: Option<right_agent_config::GenericProvider>,
    },
}

/// Decide how copying `source_entry` (using `env_var`) into a destination
/// whose current providers are `dest_providers` should proceed. Pure.
pub(crate) fn plan_copy(
    source_agent: &str,
    source_entry: &right_agent_config::ProviderEntry,
    env_var: &str,
    dest_providers: &[right_agent_config::ProviderEntry],
    overwrite: bool,
    label_override: Option<&str>,
) -> Result<CopyPlan, ProviderApiError> {
    let existing = dest_providers
        .iter()
        .find(|p| extract_env_var(p).map(|e| e == env_var).unwrap_or(false));

    if overwrite {
        let dest_entry = existing.ok_or_else(|| ProviderApiError::CopyConflict {
            reason: format!("nothing to overwrite: no provider uses env var \"{env_var}\""),
        })?;
        let compatible = matches!(
            (&source_entry.type_, &dest_entry.type_),
            (
                right_agent_config::ProviderType::Generic,
                right_agent_config::ProviderType::Generic
            ) | (
                right_agent_config::ProviderType::BuiltIn(_),
                right_agent_config::ProviderType::BuiltIn(_)
            )
        );
        if !compatible {
            return Err(ProviderApiError::CopyConflict {
                reason: format!(
                    "existing provider \"{}\" has an incompatible type; remove it first",
                    dest_entry.name
                ),
            });
        }
        let resync_generic = match (&source_entry.type_, &source_entry.generic) {
            (right_agent_config::ProviderType::Generic, Some(src_g)) => {
                let differs = dest_entry
                    .generic
                    .as_ref()
                    .map(|d| {
                        d.upstream_hosts != src_g.upstream_hosts
                            || d.upstream_path_prefix != src_g.upstream_path_prefix
                    })
                    .unwrap_or(true);
                if differs { Some(src_g.clone()) } else { None }
            }
            _ => None,
        };
        Ok(CopyPlan::Overwrite {
            dest_name: dest_entry.name.clone(),
            resync_generic,
        })
    } else {
        if existing.is_some() {
            return Err(ProviderApiError::EnvVarCollision {
                env_var: env_var.to_string(),
            });
        }
        let label = label_override.map(|s| s.to_string()).or_else(|| {
            source_entry
                .name
                .strip_prefix(&format!("{source_agent}-"))
                .map(|s| s.to_string())
        });
        let (type_, generic) = match &source_entry.type_ {
            right_agent_config::ProviderType::Generic => {
                let g = source_entry.generic.clone().ok_or_else(|| {
                    ProviderApiError::InvalidName {
                        name: source_entry.name.clone(),
                        reason: "generic source provider missing 'generic:' block".into(),
                    }
                })?;
                (
                    "generic".to_string(),
                    Some(ProviderCreateGeneric {
                        env_var: g.env_var,
                        upstream_host: None,
                        upstream_hosts: Some(g.upstream_hosts),
                        upstream_path_prefix: g.upstream_path_prefix,
                    }),
                )
            }
            right_agent_config::ProviderType::BuiltIn(slug) => (slug.clone(), None),
        };
        Ok(CopyPlan::Create { type_, label, generic })
    }
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `devenv shell -- cargo nextest run -p right plan_copy_tests`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(providers): pure plan_copy decision logic for cross-agent copy"
```

---

## Task 4: `require_trusted` + `build_peers` discovery

Both are fs-only (allowlist + agent.yaml) — no gateway. Fully unit-testable
with a temp `agents_dir`.

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod peers_tests {
    use super::*;

    fn write_agent(dir: &std::path::Path, name: &str, allow_ids: &[i64], providers_yaml: &str) {
        let agent_dir = dir.join(name);
        std::fs::create_dir_all(&agent_dir).unwrap();
        let users = allow_ids
            .iter()
            .map(|id| format!("  - id: {id}\n    opened_at: 2026-01-01T00:00:00Z"))
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

    #[test]
    fn require_trusted_accepts_member_rejects_others() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "riskoff", &[7], "  providers: []\n");
        assert!(require_trusted(tmp.path(), "riskoff", 7).is_ok());
        let err = require_trusted(tmp.path(), "riskoff", 99).unwrap_err();
        assert!(matches!(err, ProviderApiError::Unauthorized { .. }));
    }

    #[test]
    fn build_peers_excludes_self_and_untrusted_and_reports_providers() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        write_agent(
            tmp.path(),
            "riskoff",
            &[7],
            "  providers:\n    - name: riskoff-fal\n      type: right-fal\n",
        );
        write_agent(tmp.path(), "secret", &[42], "  providers: []\n");

        let peers = build_peers(tmp.path(), 7, "current").unwrap();
        let names: Vec<&str> = peers.iter().map(|p| p.agent.as_str()).collect();
        assert_eq!(names, vec!["riskoff"]); // self + untrusted filtered
        assert_eq!(peers[0].providers.len(), 1);
        assert_eq!(peers[0].providers[0].name, "riskoff-fal");
        assert_eq!(peers[0].providers[0].env_var, "FAL_KEY");
        assert_eq!(peers[0].network_policy, "permissive");
    }
}
```

> `tempfile` is already a dev-dependency of `right` (used by existing
> internal_api tests near line 1200). If a compile error says otherwise,
> add `tempfile = { workspace = true }` under `[dev-dependencies]`.

- [ ] **Step 2: Run it to verify it fails**

Run: `devenv shell -- cargo nextest run -p right peers_tests`
Expected: FAIL to compile — `require_trusted` / `build_peers` not defined.

- [ ] **Step 3: Implement the request/response types + helpers**

Add to `crates/right/src/internal_api_providers.rs`:

```rust
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

/// Require that `actor_user_id` is in `agent`'s allowlist. Missing/empty
/// allowlist = not trusted (secure default).
fn require_trusted(
    agents_dir: &std::path::Path,
    agent: &str,
    actor_user_id: i64,
) -> Result<(), ProviderApiError> {
    let agent_dir = agents_dir.join(agent);
    let file =
        right_agent::agent::allowlist::read_file(&agent_dir).map_err(ProviderApiError::Internal)?;
    let trusted = file
        .map(|f| f.users.iter().any(|u| u.id == actor_user_id))
        .unwrap_or(false);
    if trusted {
        Ok(())
    } else {
        Err(ProviderApiError::Unauthorized {
            agent: agent.to_string(),
        })
    }
}

/// Enumerate host-local agents (except `for_agent`) where `actor_user_id`
/// is trusted and the sandbox mode is openshell. Returns each peer's
/// providers (no credentials). Tolerant: a peer with an unreadable
/// `agent.yaml` is skipped, not fatal.
pub(crate) fn build_peers(
    agents_dir: &std::path::Path,
    actor_user_id: i64,
    for_agent: &str,
) -> Result<Vec<ProviderPeer>, ProviderApiError> {
    let mut names: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(agents_dir)
        .map_err(|e| ProviderApiError::Internal(format!("read agents dir: {e}")))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| ProviderApiError::Internal(format!("read agents dir entry: {e}")))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()) else {
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
        let trusted = right_agent::agent::allowlist::read_file(&agent_dir)
            .map_err(ProviderApiError::Internal)?
            .map(|f| f.users.iter().any(|u| u.id == actor_user_id))
            .unwrap_or(false);
        if !trusted {
            continue;
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
        let mut providers = Vec::new();
        for entry in &sandbox.providers {
            let env_var = match extract_env_var(entry) {
                Ok(v) => v,
                Err(_) => continue,
            };
            providers.push(PeerProvider {
                name: entry.name.clone(),
                type_: provider_view_type(entry),
                env_var,
                label: entry.label.clone(),
                generic: entry.generic.clone(),
            });
        }
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
    build_peers(&state.agents_dir, req.actor_user_id, &req.for_agent).map(axum::Json)
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `devenv shell -- cargo nextest run -p right peers_tests`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(providers): provider_peers discovery + require_trusted"
```

---

## Task 5: `handle_provider_copy` (delegating executor)

Wires authorization + source read + `plan_copy` + credential read-back +
delegation to the existing create / rotate / config-update handlers.

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs`

- [ ] **Step 1: Implement the request type + handler**

Add to `crates/right/src/internal_api_providers.rs`:

```rust
#[derive(Debug, serde::Deserialize)]
pub struct ProviderCopyReq {
    pub actor_user_id: i64,
    pub source_agent: String,
    pub source_provider: String,
    pub dest_agent: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
}

pub(crate) async fn handle_provider_copy(
    axum::extract::State(state): axum::extract::State<crate::internal_api::InternalState>,
    axum::Json(req): axum::Json<ProviderCopyReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    // Actor must be trusted on BOTH sides.
    require_trusted(&state.agents_dir, &req.source_agent, req.actor_user_id)?;
    require_trusted(&state.agents_dir, &req.dest_agent, req.actor_user_id)?;

    // Resolve the source provider entry + its env var.
    let source_cfg = load_agent_config(&state.agents_dir, &req.source_agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    let source_sandbox = source_cfg
        .sandbox
        .as_ref()
        .ok_or(ProviderApiError::SandboxModeNone)?;
    let source_entry = source_sandbox
        .providers
        .iter()
        .find(|p| p.name == req.source_provider)
        .cloned()
        .ok_or_else(|| ProviderApiError::NotFound {
            name: req.source_provider.clone(),
        })?;
    let env_var = extract_env_var(&source_entry)?;

    // Resolve the destination's current providers, then plan.
    let dest_cfg = load_agent_config(&state.agents_dir, &req.dest_agent)
        .map_err(ProviderApiError::AgentYamlWrite)?;
    let dest_sandbox = dest_cfg
        .sandbox
        .as_ref()
        .ok_or(ProviderApiError::SandboxModeNone)?;
    let plan = plan_copy(
        &req.source_agent,
        &source_entry,
        &env_var,
        &dest_sandbox.providers,
        req.overwrite,
        req.label.as_deref(),
    )?;

    // Read the source credential from the gateway (the one deliberate read-back).
    let mut client = open_openshell_client().await?;
    let mut creds =
        right_openshell::providers::get_provider_credentials(&mut client, &req.source_provider)
            .await
            .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    let credential = creds.remove(&env_var).ok_or_else(|| {
        ProviderApiError::Gateway(format!(
            "source provider \"{}\" has no stored credential for env var \"{env_var}\"",
            req.source_provider
        ))
    })?;

    match plan {
        CopyPlan::Create {
            type_,
            label,
            generic,
        } => {
            let create_req = ProviderCreateReq {
                agent: req.dest_agent.clone(),
                type_,
                label,
                credential,
                generic,
            };
            handle_provider_create(axum::extract::State(state), axum::Json(create_req)).await
        }
        CopyPlan::Overwrite {
            dest_name,
            resync_generic,
        } => {
            let rotate_req = ProviderRotateReq {
                agent: req.dest_agent.clone(),
                name: dest_name.clone(),
                credential,
            };
            let view = handle_provider_rotate(
                axum::extract::State(state.clone()),
                axum::Json(rotate_req),
            )
            .await?;
            if let Some(g) = resync_generic {
                let cfg_req = ProviderConfigUpdateReq {
                    agent: req.dest_agent.clone(),
                    name: dest_name,
                    generic: ProviderConfigUpdateGeneric {
                        env_var: Some(g.env_var.clone()),
                        upstream_host: None,
                        upstream_hosts: Some(g.upstream_hosts.clone()),
                        upstream_path_prefix: Some(g.upstream_path_prefix.clone()),
                    },
                };
                return handle_provider_config_update(
                    axum::extract::State(state),
                    axum::Json(cfg_req),
                )
                .await;
            }
            Ok(view)
        }
    }
}
```

> Note: `handle_provider_rotate` and `handle_provider_config_update` both
> return `axum::Json<ProviderView>`, so `view` is already the right type;
> the credential-only overwrite returns the rotate result directly.

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p right`
Expected: PASS (no errors).

- [ ] **Step 3: Commit**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(providers): handle_provider_copy executor (import/export core)"
```

---

## Task 6: Register `/provider-peers` and `/provider-copy` routes

**Files:**
- Modify: `crates/right/src/internal_api.rs:242` (router builder, after the
  `/provider-remove` route)

- [ ] **Step 1: Add the routes**

After the `/provider-remove` route block (line 242-245), before
`.with_state(state)`:

```rust
        .route(
            "/provider-peers",
            post(crate::internal_api_providers::handle_provider_peers),
        )
        .route(
            "/provider-copy",
            post(crate::internal_api_providers::handle_provider_copy),
        )
```

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p right`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/right/src/internal_api.rs
git commit -m "feat(providers): register provider-peers and provider-copy routes"
```

---

## Task 7: `InternalClient` methods + request types

**Files:**
- Modify: `crates/right-mcp/src/internal_client.rs` (methods near line 347;
  types near line 408; tests in the file's test module)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block (or create one) in
`crates/right-mcp/src/internal_client.rs`:

```rust
#[test]
fn provider_copy_request_serializes_without_credential_field() {
    let req = ProviderCopyRequest {
        actor_user_id: 7,
        source_agent: "riskoff",
        source_provider: "riskoff-fal",
        dest_agent: "other",
        label: Some("fal"),
        overwrite: true,
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["actor_user_id"], 7);
    assert_eq!(v["source_agent"], "riskoff");
    assert_eq!(v["source_provider"], "riskoff-fal");
    assert_eq!(v["dest_agent"], "other");
    assert_eq!(v["label"], "fal");
    assert_eq!(v["overwrite"], true);
    assert!(v.get("credential").is_none(), "copy carries no secret");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `devenv shell -- cargo nextest run -p right-mcp provider_copy_request_serializes`
Expected: FAIL to compile — `ProviderCopyRequest` not defined.

- [ ] **Step 3: Add the request types**

After `ProviderRemoveRequest` (line 408-412) in
`crates/right-mcp/src/internal_client.rs`:

```rust
#[derive(Debug, serde::Serialize)]
pub struct ProviderPeersRequest<'a> {
    pub actor_user_id: i64,
    pub for_agent: &'a str,
}

#[derive(Debug, serde::Serialize)]
pub struct ProviderCopyRequest<'a> {
    pub actor_user_id: i64,
    pub source_agent: &'a str,
    pub source_provider: &'a str,
    pub dest_agent: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<&'a str>,
    pub overwrite: bool,
}
```

- [ ] **Step 4: Add the client methods**

Inside the `impl InternalClient` block, after `provider_config_update`
(line 347):

```rust
    /// List host-local peer agents (where the actor is trusted) and their providers.
    pub async fn provider_peers(
        &self,
        actor_user_id: i64,
        for_agent: &str,
    ) -> Result<Vec<serde_json::Value>, InternalClientError> {
        self.post(
            "/provider-peers",
            &ProviderPeersRequest {
                actor_user_id,
                for_agent,
            },
        )
        .await
    }

    /// Copy a provider (credential included) between two host-local agents.
    pub async fn provider_copy(
        &self,
        req: &ProviderCopyRequest<'_>,
    ) -> Result<serde_json::Value, InternalClientError> {
        self.post("/provider-copy", req).await
    }
```

- [ ] **Step 5: Run it to verify it passes**

Run: `devenv shell -- cargo nextest run -p right-mcp provider_copy_request_serializes`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-mcp/src/internal_client.rs
git commit -m "feat(providers): InternalClient provider_peers + provider_copy"
```

---

## Task 8: Dashboard handlers (peers / import / export)

**Files:**
- Modify: `crates/bot/src/telegram/dashboard/providers.rs`

- [ ] **Step 1: Write the failing test (body deserialization)**

Add to the `#[cfg(test)] mod tests` block at the bottom of
`crates/bot/src/telegram/dashboard/providers.rs`:

```rust
#[test]
fn import_body_defaults_overwrite_false() {
    let b: ProviderImportBody = serde_json::from_value(serde_json::json!({
        "source_agent": "riskoff",
        "source_provider": "riskoff-fal"
    }))
    .unwrap();
    assert_eq!(b.source_agent, "riskoff");
    assert_eq!(b.source_provider, "riskoff-fal");
    assert!(b.label.is_none());
    assert!(!b.overwrite);
}

#[test]
fn export_body_parses_overwrite() {
    let b: ProviderExportBody = serde_json::from_value(serde_json::json!({
        "provider": "current-fal",
        "dest_agent": "riskoff",
        "overwrite": true
    }))
    .unwrap();
    assert_eq!(b.provider, "current-fal");
    assert_eq!(b.dest_agent, "riskoff");
    assert!(b.overwrite);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `devenv shell -- cargo nextest run -p bot import_body_defaults_overwrite_false`
Expected: FAIL to compile — types not defined.

- [ ] **Step 3: Implement bodies + handlers**

Add to `crates/bot/src/telegram/dashboard/providers.rs` (after
`ProviderRotateBody`, line 54):

```rust
#[derive(Debug, Deserialize)]
pub(crate) struct ProviderImportBody {
    pub source_agent: String,
    pub source_provider: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderExportBody {
    pub provider: String,
    pub dest_agent: String,
    #[serde(default)]
    pub overwrite: bool,
}
```

Add the handlers (after `handle_config_update`, before the test module):

```rust
pub(crate) async fn handle_peers(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    let user = match authenticate_api(&state, &agent, &headers) {
        Ok(u) => u,
        Err(error) => return error.into_response(),
    };
    match state
        .internal_client
        .provider_peers(user.id, &state.agent_name)
        .await
    {
        Ok(peers) => Json(serde_json::json!({ "peers": peers })).into_response(),
        Err(error) => internal_api_error_response(error, "provider_peers_failed"),
    }
}

pub(crate) async fn handle_import(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user = match authenticate_api(&state, &agent, &headers) {
        Ok(u) => u,
        Err(error) => return error.into_response(),
    };
    let body: ProviderImportBody = match parse_json_body(&body) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let req = right_mcp::internal_client::ProviderCopyRequest {
        actor_user_id: user.id,
        source_agent: &body.source_agent,
        source_provider: &body.source_provider,
        dest_agent: &state.agent_name,
        label: body.label.as_deref(),
        overwrite: body.overwrite,
    };
    match state.internal_client.provider_copy(&req).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_import_failed"),
    }
}

pub(crate) async fn handle_export(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user = match authenticate_api(&state, &agent, &headers) {
        Ok(u) => u,
        Err(error) => return error.into_response(),
    };
    let body: ProviderExportBody = match parse_json_body(&body) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let req = right_mcp::internal_client::ProviderCopyRequest {
        actor_user_id: user.id,
        source_agent: &state.agent_name,
        source_provider: &body.provider,
        dest_agent: &body.dest_agent,
        label: None,
        overwrite: body.overwrite,
    };
    match state.internal_client.provider_copy(&req).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_export_failed"),
    }
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `devenv shell -- cargo nextest run -p bot import_body_defaults_overwrite_false export_body_parses_overwrite`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/dashboard/providers.rs
git commit -m "feat(dashboard): provider peers/import/export handlers"
```

---

## Task 9: Register the dashboard routes

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs:267` (after the
  `/providers/types` route block)

- [ ] **Step 1: Add the routes**

After the `/providers/types` route (line 266-269):

```rust
        .route(
            "/dashboard/{agent}/api/v1/providers/peers",
            get(providers::handle_peers),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers/import",
            post(providers::handle_import),
        )
        .route(
            "/dashboard/{agent}/api/v1/providers/export",
            post(providers::handle_export),
        )
```

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p bot`
Expected: PASS. (`get`/`post` are already imported in this file.)

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/dashboard.rs
git commit -m "feat(dashboard): register provider peers/import/export routes"
```

---

## Task 10: Frontend types + API functions

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts:588` (after
  `ProviderGenericBody`)
- Modify: `crates/right-dashboard/frontend/src/api.ts` (imports + after
  `providerRemove`, line 200)
- Test: `crates/right-dashboard/frontend/src/api.test.ts`

- [ ] **Step 1: Add the types**

After `ProviderCreateBody` in `types.ts` (line 595):

```ts
export interface PeerProvider {
  name: string
  type: string
  env_var: string
  label: string | null
  generic: ProviderGenericBody | null
}

export interface ProviderPeer {
  agent: string
  network_policy: 'permissive' | 'restrictive'
  providers: PeerProvider[]
}

export interface ProviderImportBody {
  source_agent: string
  source_provider: string
  label?: string
  overwrite: boolean
}

export interface ProviderExportBody {
  provider: string
  dest_agent: string
  overwrite: boolean
}
```

- [ ] **Step 2: Write the failing API test**

Add to `crates/right-dashboard/frontend/src/api.test.ts`:

```ts
import { providerPeers, providerImport, providerExport } from './api'

describe('provider sharing api', () => {
  afterEach(() => vi.restoreAllMocks())

  it('providerPeers GETs the peers endpoint', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ peers: [] }), { status: 200 }),
    )
    const res = await providerPeers()
    expect(res.peers).toEqual([])
    const [path, init] = fetchMock.mock.calls[0]
    expect(path).toBe('api/v1/providers/peers')
    expect(init?.method ?? 'GET').toBe('GET')
  })

  it('providerImport POSTs the import body', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ name: 'cur-fal' }), { status: 200 }),
    )
    await providerImport({ source_agent: 'riskoff', source_provider: 'riskoff-fal', overwrite: false })
    const [path, init] = fetchMock.mock.calls[0]
    expect(path).toBe('api/v1/providers/import')
    expect(init?.method).toBe('POST')
    expect(JSON.parse(init?.body as string).source_agent).toBe('riskoff')
  })

  it('providerExport POSTs the export body', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ name: 'riskoff-fal' }), { status: 200 }),
    )
    await providerExport({ provider: 'cur-fal', dest_agent: 'riskoff', overwrite: true })
    const [path, init] = fetchMock.mock.calls[0]
    expect(path).toBe('api/v1/providers/export')
    expect(init?.method).toBe('POST')
    expect(JSON.parse(init?.body as string).overwrite).toBe(true)
  })
})
```

- [ ] **Step 3: Run it to verify it fails**

Run: `devenv shell -- pnpm -C crates/right-dashboard/frontend exec vitest run src/api.test.ts`
Expected: FAIL — `providerPeers` etc. not exported.

- [ ] **Step 4: Add the API functions**

In `api.ts`, extend the type import block (lines 1-30) to include
`PeerProvider`, `ProviderPeer`, `ProviderImportBody`, `ProviderExportBody`
(add them alphabetically among the existing `Provider*` imports), then add
after `providerRemove` (line 200):

```ts
export function providerPeers(): Promise<{ peers: ProviderPeer[] }> {
  return requestJson<{ peers: ProviderPeer[] }>('api/v1/providers/peers')
}

export function providerImport(body: ProviderImportBody): Promise<ProviderView> {
  return requestJson<ProviderView>('api/v1/providers/import', {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export function providerExport(body: ProviderExportBody): Promise<ProviderView> {
  return requestJson<ProviderView>('api/v1/providers/export', {
    method: 'POST',
    body: JSON.stringify(body),
  })
}
```

- [ ] **Step 5: Run it to verify it passes**

Run: `devenv shell -- pnpm -C crates/right-dashboard/frontend exec vitest run src/api.test.ts`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/api.ts crates/right-dashboard/frontend/src/api.test.ts
git commit -m "feat(dashboard): provider sharing types + api functions"
```

---

## Task 11: Frontend view-model (collision / mode logic)

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/providersViewModel.ts`
- Test: `crates/right-dashboard/frontend/src/views/providersViewModel.test.ts`

- [ ] **Step 1: Write the failing tests**

Add to `providersViewModel.test.ts` (extend the existing imports to add
`copyTargetMode, exportTargetState`):

```ts
import { copyTargetMode, exportTargetState } from './providersViewModel'
import type { ProviderPeer } from '../types'

describe('copyTargetMode', () => {
  it('overwrite when an env var already exists locally, else create', () => {
    const local = [providerView({ env_var: 'FAL_KEY' })]
    expect(copyTargetMode(local, 'FAL_KEY')).toBe('overwrite')
    expect(copyTargetMode(local, 'OPENAI_API_KEY')).toBe('create')
  })
})

describe('exportTargetState', () => {
  function peer(overrides: Partial<ProviderPeer> = {}): ProviderPeer {
    return { agent: 'riskoff', network_policy: 'permissive', providers: [], ...overrides }
  }

  it('blocks a generic provider when the peer is restrictive', () => {
    const p = providerView({ generic: { env_var: 'FAL_KEY', upstream_hosts: ['fal.run'] } })
    const state = exportTargetState(peer({ network_policy: 'restrictive' }), p)
    expect(state.blocked).not.toBeNull()
  })

  it('marks overwrite when the peer already has the env var', () => {
    const p = providerView({ env_var: 'FAL_KEY', generic: null })
    const target = peer({
      providers: [{ name: 'riskoff-fal', type: 'right-fal', env_var: 'FAL_KEY', label: null, generic: null }],
    })
    const state = exportTargetState(target, p)
    expect(state.blocked).toBeNull()
    expect(state.mode).toBe('overwrite')
  })

  it('marks create when the peer lacks the env var', () => {
    const p = providerView({ env_var: 'FAL_KEY', generic: null })
    const state = exportTargetState(peer(), p)
    expect(state.mode).toBe('create')
  })
})
```

- [ ] **Step 2: Run it to verify it fails**

Run: `devenv shell -- pnpm -C crates/right-dashboard/frontend exec vitest run src/views/providersViewModel.test.ts`
Expected: FAIL — functions not exported.

- [ ] **Step 3: Implement the functions**

Add to `providersViewModel.ts` (extend the top import to add
`ProviderPeer`):

```ts
import type { ProviderPeer, ProviderView } from '../types'

/** Copying a provider with `envVar` into an agent whose providers are
 *  `localProviders`: overwrite if that env var is already used, else create. */
export function copyTargetMode(
  localProviders: ProviderView[],
  envVar: string,
): 'create' | 'overwrite' {
  return localProviders.some((p) => p.env_var === envVar) ? 'overwrite' : 'create'
}

/** For exporting `provider` to `peer`: whether the peer can accept it and
 *  whether it would create or overwrite. */
export function exportTargetState(
  peer: ProviderPeer,
  provider: ProviderView,
): { mode: 'create' | 'overwrite'; blocked: string | null } {
  const isGeneric = provider.generic !== null
  if (isGeneric && peer.network_policy === 'restrictive') {
    return { mode: 'create', blocked: 'restrictive policy cannot accept generic providers' }
  }
  const mode = peer.providers.some((p) => p.env_var === provider.env_var)
    ? 'overwrite'
    : 'create'
  return { mode, blocked: null }
}
```

> The existing `import type { ProviderView } from '../types'` on line 1 is
> replaced by the combined import above.

- [ ] **Step 4: Run it to verify it passes**

Run: `devenv shell -- pnpm -C crates/right-dashboard/frontend exec vitest run src/views/providersViewModel.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/providersViewModel.ts crates/right-dashboard/frontend/src/views/providersViewModel.test.ts
git commit -m "feat(dashboard): copy/export mode view-model helpers"
```

---

## Task 12: Wire import + export into `ProvidersView.vue`

Add an "Import from another agent" path to the Add modal and an "Export"
action per provider row. Both load peers via `providerPeers()`.

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/ProvidersView.vue`
- Test: `crates/right-dashboard/frontend/src/views/ProvidersView.test.ts`

- [ ] **Step 1: Add script state + handlers**

In the `<script setup>` block of `ProvidersView.vue`, extend the api import
(lines 4-11) to also import `providerPeers, providerImport, providerExport`,
extend the type import (lines 12-15) to add `ProviderPeer, PeerProvider`,
and the view-model import (lines 18-27) to add `copyTargetMode,
exportTargetState`. Then add after the existing refs (around line 76):

```ts
// Peer (other-agent) state for import/export
const peers = ref<ProviderPeer[]>([])

// Import flow
const importOpen = ref(false)
const importBusy = ref<string | null>(null)
const importError = ref<string | null>(null)

// Export flow
const exportOpen = ref(false)
const exportProvider = ref<ProviderView | null>(null)
const exportBusy = ref<string | null>(null)
const exportError = ref<string | null>(null)

function openImport(): void {
  importError.value = null
  importOpen.value = true
}
function closeImport(): void {
  importOpen.value = false
  importError.value = null
}

interface ImportCandidate {
  agent: string
  provider: PeerProvider
  mode: 'create' | 'overwrite'
}
function importCandidates(): ImportCandidate[] {
  const out: ImportCandidate[] = []
  for (const peer of peers.value) {
    for (const p of peer.providers) {
      out.push({ agent: peer.agent, provider: p, mode: copyTargetMode(providers.value, p.env_var) })
    }
  }
  return out
}

async function runImport(c: ImportCandidate): Promise<void> {
  importBusy.value = `${c.agent}/${c.provider.name}`
  importError.value = null
  try {
    await providerImport({
      source_agent: c.agent,
      source_provider: c.provider.name,
      overwrite: c.mode === 'overwrite',
    })
    closeImport()
    await refresh()
  } catch (err) {
    importError.value = err instanceof Error ? err.message : 'Import failed'
  } finally {
    importBusy.value = null
  }
}

function openExport(provider: ProviderView): void {
  exportProvider.value = provider
  exportError.value = null
  exportOpen.value = true
}
function closeExport(): void {
  exportOpen.value = false
  exportProvider.value = null
  exportError.value = null
}

interface ExportTarget {
  agent: string
  mode: 'create' | 'overwrite'
  blocked: string | null
}
function exportTargets(): ExportTarget[] {
  const p = exportProvider.value
  if (!p) return []
  return peers.value.map((peer) => {
    const s = exportTargetState(peer, p)
    return { agent: peer.agent, mode: s.mode, blocked: s.blocked }
  })
}

async function runExport(t: ExportTarget): Promise<void> {
  const p = exportProvider.value
  if (!p || t.blocked) return
  exportBusy.value = t.agent
  exportError.value = null
  try {
    await providerExport({ provider: p.name, dest_agent: t.agent, overwrite: t.mode === 'overwrite' })
  } catch (err) {
    exportError.value = err instanceof Error ? err.message : 'Export failed'
  } finally {
    exportBusy.value = null
  }
}
```

Then extend `refresh()` (line 137) to also load peers — change the
`Promise.all` to include peers and assign:

```ts
    const [listRes, typesRes, peersRes] = await Promise.all([
      providerList(),
      providerTypes(),
      providerPeers(),
    ])
    if (disposed) return
    providers.value = listRes.providers
    types.value = typesRes.types
    peers.value = peersRes.peers
```

- [ ] **Step 2: Add template UI**

In the `<template>`, add an Import button in the panel header next to
`+ Add` (after the `+ Add` button, line 408):

```html
      <button class="tool-button" type="button" @click="importOpen ? closeImport() : openImport()">
        {{ importOpen ? 'Close' : 'Import' }}
      </button>
```

Add the import modal after the existing Add modal `</section>` (line 495):

```html
    <!-- Import modal -->
    <section v-if="importOpen" class="providers-section">
      <p class="muted-line">Import a provider from another agent you manage:</p>
      <p v-if="importCandidates().length === 0" class="muted-line">No importable providers</p>
      <article
        v-for="c in importCandidates()"
        :key="`${c.agent}/${c.provider.name}`"
        class="data-row providers-row static"
      >
        <div class="row-main providers-row-main">
          <strong>{{ c.provider.label ?? c.provider.name }}</strong>
          <small>from {{ c.agent }}</small>
          <small>{{ c.provider.env_var }}</small>
        </div>
        <div class="button-row row-actions">
          <button
            class="tool-button"
            type="button"
            :disabled="importBusy === `${c.agent}/${c.provider.name}`"
            @click="runImport(c)"
          >
            {{ importBusy === `${c.agent}/${c.provider.name}` ? 'Working' : (c.mode === 'overwrite' ? 'Update' : 'Import') }}
          </button>
        </div>
      </article>
      <p v-if="importError" class="notice inline">{{ importError }}</p>
    </section>

    <!-- Export modal -->
    <section v-if="exportOpen && exportProvider" class="providers-section">
      <p class="muted-line">Export <strong>{{ exportProvider.name }}</strong> to:</p>
      <p v-if="exportTargets().length === 0" class="muted-line">No eligible agents</p>
      <article
        v-for="t in exportTargets()"
        :key="t.agent"
        class="data-row providers-row static"
      >
        <div class="row-main providers-row-main">
          <strong>{{ t.agent }}</strong>
          <small v-if="t.blocked">{{ t.blocked }}</small>
        </div>
        <div class="button-row row-actions">
          <button
            class="tool-button"
            type="button"
            :disabled="t.blocked !== null || exportBusy === t.agent"
            @click="runExport(t)"
          >
            {{ exportBusy === t.agent ? 'Working' : (t.mode === 'overwrite' ? 'Update' : 'Export') }}
          </button>
        </div>
      </article>
      <p v-if="exportError" class="notice inline">{{ exportError }}</p>
      <div class="button-row">
        <button class="tool-button" type="button" @click="closeExport">Close</button>
      </div>
    </section>
```

Add an Export button to each provider row's action group (after the Remove
button, line 621), for non-ghost rows:

```html
          <button
            v-if="!isGhost(provider)"
            class="tool-button"
            type="button"
            @click="openExport(provider)"
          >
            Export
          </button>
```

- [ ] **Step 3: Extend the test harness mocks**

In `ProvidersView.test.ts`, add the three new api functions to the
`vi.hoisted` `apiMocks` object (the block at lines 10-17):

```ts
const apiMocks = vi.hoisted(() => ({
  providerList: vi.fn(),
  providerTypes: vi.fn(),
  providerCreate: vi.fn(),
  providerRotate: vi.fn(),
  providerConfigUpdate: vi.fn(),
  providerRemove: vi.fn(),
  providerPeers: vi.fn(),
  providerImport: vi.fn(),
  providerExport: vi.fn(),
}))
```

And add a default resolution in the `beforeEach` block (lines 107-114):

```ts
  apiMocks.providerPeers.mockResolvedValue({ peers: [] })
  apiMocks.providerImport.mockResolvedValue({})
  apiMocks.providerExport.mockResolvedValue({})
```

- [ ] **Step 4: Write the failing SSR test**

Add inside the `describe('ProvidersView', ...)` block (the `provider()`
helper at lines 41-58 and `mountProvidersView` / `flushAsync` /
`buttonsByText` helpers already exist in this file):

```ts
it('renders Import and per-row Export entry points', async () => {
  apiMocks.providerList.mockResolvedValue({ providers: [provider()] })
  const { app, root } = mountProvidersView()
  await flushAsync()

  // Header-level Import button.
  expect(buttonsByText(root, 'Import').length).toBeGreaterThan(0)
  // Per-row Export button (one provider row).
  expect(buttonsByText(root, 'Export').length).toBe(1)

  app.unmount()
})
```

- [ ] **Step 5: Run the view tests + typecheck**

Run:
```bash
devenv shell -- pnpm -C crates/right-dashboard/frontend exec vitest run src/views/ProvidersView.test.ts
devenv shell -- pnpm -C crates/right-dashboard/frontend exec vue-tsc --noEmit
```
Expected: PASS, no type errors.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/ProvidersView.vue crates/right-dashboard/frontend/src/views/ProvidersView.test.ts
git commit -m "feat(dashboard): import/export UI in ProvidersView"
```

---

## Task 13: Documentation

**Files:**
- Modify: `ARCHITECTURE.md` (Providers section)
- Modify: `docs/architecture/providers.md`
- Modify: `ARCHITECTURE.md` (Security Model section)

- [ ] **Step 1: ARCHITECTURE.md — Providers invariants**

In `ARCHITECTURE.md`, in the `### Providers` section, append a short
prescriptive paragraph (keep the file under 40k chars — verify with
`wc -c ARCHITECTURE.md` afterward):

```markdown
Cross-agent provider sharing (dashboard import/export) is **copy-only**:
`provider_copy` reads the source credential via
`right_openshell::providers::get_provider_credentials` and writes an
independent provider owned by the destination agent — never a gateway
provider attached to two sandboxes. The actor (Telegram user id) MUST be in
the allowlist of both agents; the internal API re-checks the non-dashboard
side from `allowlist.yaml`. Overwrite matches the destination provider by
`env_var`; type mismatch is rejected. `get_provider_credentials` is the only
sanctioned credential read-back — `SecretString`, never logged or persisted
to `agent.yaml`/backups/list responses.
```

- [ ] **Step 2: docs/architecture/providers.md — walkthrough**

Append a `## Cross-agent sharing (import / export)` section describing: the
single `provider_copy` primitive and the two dashboard perspectives; the
`provider_peers` discovery (trusted + openshell peers, no credentials); the
`env_var` overwrite match key with credential rotate (+ generic config
re-sync only when it differs); the both-allowlists authorization; and the
single credential read-back. Reference the handlers
(`internal_api_providers::handle_provider_copy`,
`handle_provider_peers`) and the dashboard routes
(`/providers/peers|import|export`).

- [ ] **Step 3: ARCHITECTURE.md — Security Model**

In the `## Security Model` list, add two bullets:

```markdown
- **Cross-agent provider copy authorization**: import/export require the
  actor trusted in BOTH agents' allowlists; the internal API verifies the
  non-dashboard side from disk. The internal socket is host-only.
- **Credential read-back**: `get_provider_credentials` is the sole path
  that reads a stored credential back; used only to copy between agents,
  kept in `SecretString`, never logged or written to `agent.yaml`/backups.
```

- [ ] **Step 4: Verify budget + commit**

Run: `wc -c ARCHITECTURE.md` — confirm under 40000. If over, move the
Providers paragraph detail into `docs/architecture/providers.md` and keep a
one-line summary.

```bash
git add ARCHITECTURE.md docs/architecture/providers.md
git commit -m "docs(providers): document cross-agent import/export"
```

---

## Task 14: Final verification

- [ ] **Step 1: Full Rust workspace tests**

Run:
```bash
devenv shell -- cargo nextest run --workspace
devenv shell -- cargo test --doc --workspace
```
Expected: PASS (excluding any pre-existing failures recorded at baseline,
and the `#[ignore]` ci-openshell tests which run in CI).

- [ ] **Step 2: Clippy + build**

Run:
```bash
devenv shell -- cargo clippy --workspace --all-targets
devenv shell -- cargo build --workspace
```
Expected: no warnings/errors introduced by these changes.

- [ ] **Step 3: Full frontend suite + build**

Run:
```bash
devenv shell -- pnpm -C crates/right-dashboard/frontend exec vitest run
devenv shell -- pnpm -C crates/right-dashboard/frontend run build
```
Expected: PASS.

- [ ] **Step 4: Manual end-to-end (live gateway, two agents)**

> The spec listed automated `ci_openshell_` copy-create/overwrite/generic
> tests. Those need two agents + sandboxes + the internal API wired
> together — a heavy harness. The copy *decision* logic is fully covered by
> `plan_copy_tests`, discovery/auth by `peers_tests`, and the credential
> read-back by the `ci_openshell_get_provider_credentials` live test
> (Task 1). The full copy flow is verified manually here; promote to an
> automated `ci_openshell_` harness later if it regresses.

With two `up` openshell agents where you are a trusted dashboard user on
both:
1. Open agent B's dashboard → Providers → **Import** → pick agent A's
   `fal` provider → confirm it creates `B-fal`, status Healthy + Composed.
2. Rotate the key on A (Rotate), then re-run Import on B → it shows
   **Update** and re-syncs without delete.
3. From agent A's dashboard → provider row → **Export** → pick agent B
   (shows **Update**) and a third agent C (shows **Export**) → confirm
   both succeed.
4. Negative: a generic provider export to a `restrictive` agent is shown
   blocked; an agent where you are not trusted does not appear as a peer.

- [ ] **Step 5: Land**

Fast-forward push the worktree branch to `origin/master` (per the repo's
shared-checkout workflow). Do not force-push.
```

