# Provider Policy Durability + Hot-Reload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make generic-provider network-policy stanzas survive every policy regeneration, and make a `sandbox.providers`-only change to `agent.yaml` hot-reconcile without a full bot restart.

**Architecture:** Two coupled fixes. **#2 (foundation):** every full `policy.yaml` regeneration folds the agent's generic providers back in, so any regen trigger (bot start, `right restart`, host reboot, the supervisor recovery loop, or a config-watcher restart) reconstructs the TLS-terminating host stanzas from `agent.yaml`. **#1 (hot-reload):** `config_watcher` classifies a providers-only delta as a new `ProvidersReload` kind, applies model/debug in-memory as today, and signals an async task to re-apply the (now provider-aware) policy + reconcile gateway attach/detach — no `token.cancel()`, no process bounce. #1 depends on #2's `apply_provider_stanzas` helper.

**Tech Stack:** Rust 2024, tokio, tonic/gRPC (OpenShell), `notify_debouncer_mini`, `serde_saphyr` (YAML), `thiserror`/`miette`. Crates: `right-codegen` (policy render), `right-agent-config` (`AgentConfig`/`ProviderEntry`), `bot` (`config_watcher`, `sandbox_supervisor`, `lib`), `right` (`main`).

**Root cause being fixed:** `right_codegen::policy::generate_policy` emits only a bare `# right-providers: insert-above` anchor and never reads `sandbox.providers`. The on-add dashboard handler patches the stanza in via `providers_append_checked` and hot-applies it, but writing `sandbox.providers` to `agent.yaml` trips `config_watcher::diff_classify` → `RestartRequired` → graceful restart → provider-blind `generate_policy` overwrites `policy.yaml` without the stanza. The OpenShell proxy then can't substitute the `openshell:resolve:env:..._<NAME>` placeholder on the raw `tls: skip` tunnel, so the literal placeholder reaches the upstream and it returns 401.

**Branch:** Work directly on `master` (per user instruction). No worktree. Commit each task to `master`; push only when the user asks.

**Non-goals (do NOT do in this plan):**
- The on-add handler (`internal_api_providers.rs::create_generic_provider`) is already correct; do not change it.
- The docs/proxy discrepancy "placeholder over `tls: skip` should be HTTP 500 but observed 401" is an OpenShell-side concern; out of scope.
- No CLI flag for providers (dashboard-managed per project conventions).

---

## Ground-truth types (verified verbatim — use exactly these)

From `crates/right-agent-config/src/lib.rs`:
```rust
pub enum NetworkPolicy { Restrictive, Permissive }      // unit variants; Default = Permissive

pub enum ProviderType {                                  // NOT a String
    Generic,                                             // yaml "generic"
    BuiltIn(String),                                     // e.g. "anthropic"
}

pub struct GenericProvider {                             // struct name is GenericProvider
    pub env_var: String,
    pub header_name: String,                             // default "Authorization"
    pub upstream_host: String,
    pub upstream_path_prefix: Option<String>,
}

pub struct ProviderEntry {
    pub name: String,
    pub type_: ProviderType,                             // serde rename "type"
    pub label: Option<String>,                           // Option!
    pub generic: Option<GenericProvider>,                // Option!
}

pub struct SandboxConfig { pub mode, pub policy_file, pub name, pub providers: Vec<ProviderEntry> }
pub struct AgentConfig  { ..., pub network_policy: NetworkPolicy, pub model: Option<String>,
                          pub debug: Option<bool>, pub sandbox: Option<SandboxConfig>, ... }
// AgentConfig + SandboxConfig + ProviderEntry all derive PartialEq.
```

From `crates/right-codegen/src/policy.rs` (already `use right_agent_config::NetworkPolicy;`):
```rust
pub enum HostMcpAccess { BootstrapUnresolved, Resolved(Vec<std::net::IpAddr>) }   // NOT "Bootstrap"; Resolved takes IpAddr
pub fn generate_policy(right_mcp_port: u16, network_policy: &NetworkPolicy, host_mcp_access: HostMcpAccess) -> String
pub fn providers_append(policy, provider_name, host, path_prefix: Option<&str>) -> String          // panics on conflict
pub fn providers_append_checked(policy, provider_name, host, path_prefix: Option<&str>) -> Result<String, PolicyConflict>
pub enum PolicyConflict { RawTunnel { host: String } }   // thiserror
// Permissive policy renders the anchor; Restrictive renders NO anchor (append is a no-op there).
// Tests live in crates/right-codegen/src/policy_provider_tests.rs, declared `mod policy_provider_tests;` in lib.rs:31, opening `use super::policy::*;`.
```

From `crates/bot/src/sandbox_supervisor.rs`:
```rust
pub(crate) struct BringUpCtx<'a> { pub agent:&'a str, pub home:&'a Path, pub agent_dir:&'a Path, pub resolved_sandbox:&'a str, pub config:&'a AgentConfig }
pub(crate) async fn bring_up_sandbox(ctx:&BringUpCtx<'_>) -> miette::Result<Result<SandboxBringUp, GatewayDiagnosis>>
// bring_up does heavy work incl. a filesystem-drift check that can hard-Err — do NOT reuse it for the hot path.
// Inside bring_up: `let config = ctx.config;` then generate_policy(right_runtime_state::MCP_HTTP_PORT, &config.network_policy, HostMcpAccess::Resolved(host_ips.clone()))
// Provider reconcile: connect_grpc(default_mtls_dir()) -> reconcile_for_sandbox(&mut client, &sandbox, agent /*=prefix*/, &declared)
```

From `crates/right-openshell/src/{openshell,providers}.rs`:
```rust
pub fn default_mtls_dir() -> PathBuf
pub async fn connect_grpc(mtls_dir:&Path) -> miette::Result<OpenShellClient<Channel>>
pub async fn resolve_sandbox_id(client:&mut OpenShellClient<Channel>, name:&str) -> miette::Result<String>
pub async fn resolve_host_ips(client:&mut OpenShellClient<Channel>, sandbox_id:&str) -> miette::Result<Vec<std::net::IpAddr>>
pub async fn reconcile_for_sandbox(client:&mut OpenShellClient<Channel>, sandbox_name:&str, agent_prefix:&str, declared:&[String]) -> Result<ReconcileReport, ProviderError>
```

From `crates/bot/src/config_watcher.rs`: watcher runs on a bare `std::thread` (no tokio context); `diff_classify` `.take()`s `model`/`debug`, `normalize_for_reload_diff` nulls model/debug/learning, then `old == new` ⇒ `HotReloadable` else `RestartRequired`. Tests are inline `#[cfg(test)] mod tests` using `#[tokio::test] async fn` + `fn classify(old,new)`.

From `crates/bot/src/lib.rs` (`run_async`): at the watcher spawn (~L515-537) these locals exist: `config: AgentConfig`, `args.agent`, `agent_dir: PathBuf`, `home: PathBuf`, `shutdown: CancellationToken`, `model_arc: Arc<ArcSwap<Option<String>>>`, `debug_flag: Arc<AtomicBool>`, `config_changed`, `agent_yaml_path`, `args.debug`. `resolved_sandbox: Option<String>` is computed slightly later (~L607). No `OpenShellClient`/`SandboxExec` exists until bring-up (~L680).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/right-codegen/src/policy.rs` | New `apply_provider_stanzas` helper folding `ProviderEntry` list onto a rendered policy. | Modify (add fn) |
| `crates/right-codegen/src/policy_provider_tests.rs` | Unit tests for `apply_provider_stanzas`. | Modify (add tests) |
| `crates/bot/src/sandbox_supervisor.rs` | Provider-aware regen in `bring_up_sandbox`; new `hot_reconcile_providers` helper. | Modify |
| `crates/right-codegen/src/pipeline.rs` | Provider-aware regen in `run_single_agent_codegen`. | Modify |
| `crates/right/src/main.rs` | Provider-aware regen in the init policy helpers. | Modify |
| `crates/bot/src/config_watcher.rs` | `ProvidersReload` kind; two-stage `diff_classify`; loop arm; new mpsc sender param. | Modify |
| `crates/bot/src/lib.rs` | Create providers channel, pass sender to watcher, spawn async reconcile consumer. | Modify |
| `ARCHITECTURE.md` + `docs/architecture/providers.md` | Hot-reloadable-fields + provider-aware-regen invariant. | Modify |

**Verification cadence (AGENTS.md):** targeted package tests after each TDD slice; one final `devenv shell -- cargo test --workspace`. Do NOT run the full suite between every task.

---

## Task 0: Baseline + anchor confirmation

- [ ] **Step 1:** `devenv shell -- cargo test -p right-codegen -p bot --no-run` — record pre-existing failures.
- [ ] **Step 2:** Confirm anchors (line numbers drift):
```bash
rg -n "fn generate_policy|fn providers_append\b|fn providers_append_checked|enum PolicyConflict|enum HostMcpAccess" crates/right-codegen/src/policy.rs
rg -n "generate_policy\(" crates/right-codegen/src/pipeline.rs crates/right/src/main.rs crates/bot/src/sandbox_supervisor.rs
rg -n "fn bring_up_sandbox|reconcile_for_sandbox\(|connect_grpc|resolve_host_ips|resolve_sandbox_id" crates/bot/src/sandbox_supervisor.rs
rg -n "spawn_config_watcher|let resolved_sandbox" crates/bot/src/lib.rs
rg -n "fn write_bootstrap_right_mcp_policy|fn apply_exact_right_mcp_policy_for_sandbox" crates/right/src/main.rs
```

---

## Task 1: `apply_provider_stanzas` helper (#2 core primitive)

**Files:** Modify `crates/right-codegen/src/policy.rs`; Test `crates/right-codegen/src/policy_provider_tests.rs`.

Returns `Result<String, PolicyConflict>` (FAIL-FAST: a provider host configured as a raw tunnel surfaces, never silently dropped). Reuses `providers_append_checked` (idempotent by managed-by tag; no-op when the anchor is absent, e.g. restrictive mode).

- [ ] **Step 1: Write failing tests** — append to `policy_provider_tests.rs`:
```rust
use right_agent_config::{GenericProvider, NetworkPolicy, ProviderEntry, ProviderType};

fn generic_entry(name: &str, host: &str, path: Option<&str>) -> ProviderEntry {
    ProviderEntry {
        name: name.to_string(),
        type_: ProviderType::Generic,
        label: Some("lbl".to_string()),
        generic: Some(GenericProvider {
            env_var: "API_KEY".to_string(),
            header_name: "Authorization".to_string(),
            upstream_host: host.to_string(),
            upstream_path_prefix: path.map(str::to_string),
        }),
    }
}

#[test]
fn apply_provider_stanzas_folds_generic_above_anchor() {
    let base = generate_policy(8100, &NetworkPolicy::Permissive, HostMcpAccess::BootstrapUnresolved);
    let out = apply_provider_stanzas(&base, &[generic_entry("right-typefully", "api.typefully.com", None)]).unwrap();
    assert!(out.contains("# managed-by: right-providers:right-typefully"));
    assert!(out.contains("- host: api.typefully.com"));
    assert!(out.contains("protocol: rest"));
    assert!(out.find("api.typefully.com").unwrap() < out.find("# right-providers: insert-above").unwrap());
}

#[test]
fn apply_provider_stanzas_is_idempotent_and_keeps_path() {
    let base = generate_policy(8100, &NetworkPolicy::Permissive, HostMcpAccess::BootstrapUnresolved);
    let providers = [generic_entry("right-acme", "api.acme.com", Some("/v1"))];
    let once = apply_provider_stanzas(&base, &providers).unwrap();
    let twice = apply_provider_stanzas(&once, &providers).unwrap();
    assert_eq!(once, twice);
    assert!(once.contains("path: /v1"));
}

#[test]
fn apply_provider_stanzas_skips_builtin_and_none() {
    let base = generate_policy(8100, &NetworkPolicy::Permissive, HostMcpAccess::BootstrapUnresolved);
    let providers = [ProviderEntry {
        name: "right-anthropic".to_string(),
        type_: ProviderType::BuiltIn("anthropic".to_string()),
        label: None,
        generic: None,
    }];
    assert_eq!(apply_provider_stanzas(&base, &providers).unwrap(), base);
}

#[test]
fn apply_provider_stanzas_noop_restrictive() {
    let base = generate_policy(8100, &NetworkPolicy::Restrictive, HostMcpAccess::BootstrapUnresolved);
    let out = apply_provider_stanzas(&base, &[generic_entry("right-acme", "api.acme.com", None)]).unwrap();
    assert_eq!(out, base, "restrictive policy has no anchor; fold is a no-op");
}
```

- [ ] **Step 2:** `devenv shell -- cargo test -p right-codegen apply_provider_stanzas` → FAIL (`cannot find function`).

- [ ] **Step 3: Implement** — in `policy.rs`, immediately after `providers_append`:
```rust
/// Fold an agent's generic-provider host stanzas onto a rendered policy.
///
/// For each `ProviderType::Generic` entry with a `generic` config, inserts a
/// TLS-terminating REST endpoint above the `# right-providers: insert-above`
/// anchor via [`providers_append_checked`]. Idempotent; a no-op when the policy
/// has no anchor (restrictive mode) or the list is empty. This is what makes
/// every full policy regeneration provider-aware so the network policy is
/// reconstructable from `agent.yaml` on every regen.
pub fn apply_provider_stanzas(
    policy: &str,
    providers: &[right_agent_config::ProviderEntry],
) -> Result<String, PolicyConflict> {
    let mut out = policy.to_string();
    for entry in providers {
        if !matches!(entry.type_, right_agent_config::ProviderType::Generic) {
            continue;
        }
        let Some(g) = entry.generic.as_ref() else {
            continue;
        };
        out = providers_append_checked(&out, &entry.name, &g.upstream_host, g.upstream_path_prefix.as_deref())?;
    }
    Ok(out)
}
```

- [ ] **Step 4:** `devenv shell -- cargo test -p right-codegen apply_provider_stanzas` → PASS (4).

- [ ] **Step 5: Commit**
```bash
git add crates/right-codegen/src/policy.rs crates/right-codegen/src/policy_provider_tests.rs
git commit -m "feat(codegen): provider-aware policy fold via apply_provider_stanzas"
```

---

## Task 2: Make every full regen provider-aware (#2 wiring)

`apply_provider_stanzas` returns `Result`, so each callsite must handle the error per its context (`?` in `miette`/`anyhow` fns). `config.sandbox` is `Option`, so extract the slice with a fallback to `&[]`.

**2a — `crates/bot/src/sandbox_supervisor.rs` (primary; fixes the bug)**

- [ ] **Step 1:** Find inside `bring_up_sandbox` (Task 0 anchor):
```rust
    let network_policy = config.network_policy;
    let policy_content = right_codegen::policy::generate_policy(
        right_runtime_state::MCP_HTTP_PORT,
        &network_policy,
        right_codegen::policy::HostMcpAccess::Resolved(host_ips.clone()),
    );
```
Replace with:
```rust
    let network_policy = config.network_policy;
    let providers = config
        .sandbox
        .as_ref()
        .map(|s| s.providers.as_slice())
        .unwrap_or(&[]);
    let policy_content = right_codegen::policy::apply_provider_stanzas(
        &right_codegen::policy::generate_policy(
            right_runtime_state::MCP_HTTP_PORT,
            &network_policy,
            right_codegen::policy::HostMcpAccess::Resolved(host_ips.clone()),
        ),
        providers,
    )
    .map_err(|e| miette::miette!("provider policy fold failed: {e:#}"))?;
```

**2b — `crates/right-codegen/src/pipeline.rs` (`run_single_agent_codegen`)**

- [ ] **Step 2:** Find:
```rust
    let policy_content = crate::policy::generate_policy(
        mcp_port,
        &network_policy,
        crate::policy::HostMcpAccess::BootstrapUnresolved,
    );
```
Replace with:
```rust
    let providers = agent
        .config
        .as_ref()
        .and_then(|c| c.sandbox.as_ref())
        .map(|s| s.providers.as_slice())
        .unwrap_or(&[]);
    let policy_content = crate::policy::apply_provider_stanzas(
        &crate::policy::generate_policy(
            mcp_port,
            &network_policy,
            crate::policy::HostMcpAccess::BootstrapUnresolved,
        ),
        providers,
    )
    .map_err(|e| /* match this fn's error type */ e)?;
```
Adjust the `map_err` to this function's return type (Task 0: read `run_single_agent_codegen`'s signature). If it returns `miette::Result`, use `.map_err(|e| miette::miette!("provider policy fold failed: {e:#}"))?`; if `anyhow::Result`, use `.map_err(|e| anyhow::anyhow!("provider policy fold failed: {e:#}"))?`.

**2c — `crates/right/src/main.rs` (init helpers; completeness, not the bug path)**

- [ ] **Step 3:** Add a `providers: &[right_agent_config::ProviderEntry]` parameter to both `write_bootstrap_right_mcp_policy` and `apply_exact_right_mcp_policy_for_sandbox` (+ the `_sync` wrapper), folding inside before the existing `write_*`:
```rust
    let policy_content = right_codegen::policy::apply_provider_stanzas(
        &right_codegen::policy::generate_policy(
            right_runtime_state::MCP_HTTP_PORT,
            &network_policy,
            /* keep this callsite's existing HostMcpAccess arg */
        ),
        providers,
    )
    .map_err(|e| miette::miette!("provider policy fold failed: {e:#}"))?;
```
Update all callers (Task 0 `rg`) to pass the agent's providers. In `cmd_agent_init`, source them from the parsed config (`cfg.sandbox.as_ref().map(|s| s.providers.as_slice()).unwrap_or(&[])`); if `cfg` is parsed *after* the apply call, move the parse above the call. Where no config is available (pure bootstrap before any provider can exist), pass `&[]`.

- [ ] **Step 4: Build**
```bash
devenv shell -- cargo build -p right-codegen -p bot -p right
```
Expected: compiles. Fix any error-type/`map_err` mismatch per the enclosing fn.

- [ ] **Step 5: Regression test (the wipe)** — append to `policy_provider_tests.rs`:
```rust
#[test]
fn full_regen_then_fold_reconstructs_host_stanza() {
    let providers = [generic_entry("right-typefully", "api.typefully.com", None)];
    let regen = generate_policy(
        8100,
        &NetworkPolicy::Permissive,
        HostMcpAccess::Resolved(vec!["10.0.0.5".parse().unwrap()]),
    );
    assert!(!regen.contains("api.typefully.com"), "bare regen must be stanza-less");
    let folded = apply_provider_stanzas(&regen, &providers).unwrap();
    assert!(folded.contains("- host: api.typefully.com"));
    assert!(folded.contains("protocol: rest"));
}
```

- [ ] **Step 6:** `devenv shell -- cargo test -p right-codegen full_regen_then_fold` → PASS.

- [ ] **Step 7: Commit**
```bash
git add crates/bot/src/sandbox_supervisor.rs crates/right-codegen/src/pipeline.rs crates/right/src/main.rs crates/right-codegen/src/policy_provider_tests.rs
git commit -m "fix(providers): fold provider stanzas into every policy regen (durable across restarts)"
```

> **After Task 2 the 401 is fixed:** every regen trigger now reconstructs the stanza from `agent.yaml`. Tasks 3-6 remove the gratuitous restart on provider changes.

---

## Task 3: `hot_reconcile_providers` helper (#1 — the live re-apply)

**Files:** Modify `crates/bot/src/sandbox_supervisor.rs`.

A network-only re-apply (no filesystem-drift check, unlike `bring_up_sandbox`): regenerate the provider-aware policy with resolved host IPs, hot-apply it, then reconcile gateway attach/detach. Idempotent and safe to call repeatedly.

- [ ] **Step 1: Add the helper** (near `bring_up_sandbox`):
```rust
/// Hot-apply a `sandbox.providers` change to a live sandbox without a restart.
///
/// Re-renders the provider-aware policy (network-only, via
/// `openshell policy set --wait`) and reconciles gateway attach/detach. Used by
/// the config-watcher providers hot path. The supervisor recovery loop is the
/// fallback if this fails.
pub(crate) async fn hot_reconcile_providers(
    agent: &str,
    agent_dir: &std::path::Path,
    resolved_sandbox: &str,
    config: &AgentConfig,
) -> miette::Result<()> {
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir).await?;
    let sandbox_id =
        right_openshell::openshell::resolve_sandbox_id(&mut client, resolved_sandbox).await?;
    let host_ips =
        right_openshell::openshell::resolve_host_ips(&mut client, &sandbox_id).await?;

    let providers = config
        .sandbox
        .as_ref()
        .map(|s| s.providers.as_slice())
        .unwrap_or(&[]);
    let policy_content = right_codegen::policy::apply_provider_stanzas(
        &right_codegen::policy::generate_policy(
            right_runtime_state::MCP_HTTP_PORT,
            &config.network_policy,
            right_codegen::policy::HostMcpAccess::Resolved(host_ips),
        ),
        providers,
    )
    .map_err(|e| miette::miette!("provider policy fold failed: {e:#}"))?;

    let policy_path = agent_dir.join("policy.yaml");
    right_codegen::contract::write_and_apply_sandbox_policy(
        resolved_sandbox,
        &policy_path,
        &policy_content,
    )
    .await?;

    let declared: Vec<String> = providers.iter().map(|p| p.name.clone()).collect();
    let report = right_openshell::providers::reconcile_for_sandbox(
        &mut client,
        resolved_sandbox,
        agent,
        &declared,
    )
    .await
    .map_err(|e| miette::miette!("provider reconcile failed: {e:#}"))?;
    tracing::info!(
        agent = %agent,
        attached = ?report.attached,
        detached = ?report.detached,
        "providers hot-reconcile complete"
    );
    Ok(())
}
```
Confirm `policy.yaml` is the correct on-disk path: in `bring_up_sandbox` it is `let policy_path = ...` (Task 0 — match how bring_up derives it; if it uses a helper like `generated_policy_path`, use the same, otherwise `agent_dir.join("policy.yaml")` matches the agent-dir layout).

- [ ] **Step 2: Build**
```bash
devenv shell -- cargo build -p bot
```
Expected: compiles.

- [ ] **Step 3: Commit**
```bash
git add crates/bot/src/sandbox_supervisor.rs
git commit -m "feat(supervisor): hot_reconcile_providers — live provider policy re-apply"
```

---

## Task 4: `ProvidersReload` classification (#1 core)

**Files:** Modify `crates/bot/src/config_watcher.rs`; tests inline.

- [ ] **Step 1: Failing tests** — add inside the existing `#[cfg(test)] mod tests`:
```rust
    #[tokio::test]
    async fn diff_providers_only_is_providers_reload() {
        let old = "restart: never\nmax_restarts: 5\nsandbox:\n  mode: openshell\n";
        let new = "restart: never\nmax_restarts: 5\nsandbox:\n  mode: openshell\n  providers:\n    - name: right-typefully\n      type: generic\n      generic:\n        env_var: TYPEFULLY_API_KEY\n        upstream_host: api.typefully.com\n";
        match classify(old, new) {
            ChangeKind::ProvidersReload { new_config, .. } => {
                let provs = new_config.sandbox.as_ref().expect("sandbox").providers.clone();
                assert_eq!(provs.len(), 1);
                assert_eq!(provs[0].name, "right-typefully");
            }
            other => panic!("expected ProvidersReload, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn diff_providers_plus_other_field_is_restart_required() {
        let old = "restart: never\nmax_restarts: 5\nsandbox:\n  mode: openshell\n";
        let new = "restart: always\nmax_restarts: 5\nsandbox:\n  mode: openshell\n  providers:\n    - name: right-typefully\n      type: generic\n      generic:\n        env_var: TYPEFULLY_API_KEY\n        upstream_host: api.typefully.com\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[tokio::test]
    async fn diff_model_only_still_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\nmodel: opus\n";
        let new = "restart: never\nmax_restarts: 5\nmodel: sonnet\n";
        assert!(matches!(classify(old, new), ChangeKind::HotReloadable { .. }));
    }
```

- [ ] **Step 2:** `devenv shell -- cargo test -p bot diff_providers` → FAIL (`no variant ProvidersReload`).

- [ ] **Step 3: Add the variant** to `ChangeKind`:
```rust
    /// Anything else — graceful restart.
    RestartRequired,
    /// Only `sandbox.providers` (optionally with model/debug) changed — apply
    /// model/debug in-memory and hot-reconcile providers without a restart.
    /// Carries the freshly parsed config so the reconcile reads new providers.
    ProvidersReload {
        new_model: Option<String>,
        new_debug: Option<bool>,
        new_config: Box<AgentConfig>,
    },
```

- [ ] **Step 4: Rewrite `diff_classify` to two-stage** (sandbox is `Option`, so clear providers via `as_mut`):
```rust
pub(crate) fn diff_classify(old_yaml: &str, new_yaml: &str) -> ChangeKind {
    if old_yaml == new_yaml {
        return ChangeKind::NoChange;
    }
    let old: AgentConfig = match serde_saphyr::from_str(old_yaml) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"),
                "config_watcher: failed to parse old agent.yaml — restart required");
            return ChangeKind::RestartRequired;
        }
    };
    let new: AgentConfig = match serde_saphyr::from_str(new_yaml) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"),
                "config_watcher: failed to parse new agent.yaml — restart required");
            return ChangeKind::RestartRequired;
        }
    };
    let new_model = new.model.clone();
    let new_debug = new.debug.clone();

    // Stage A: only model/debug/learning differ → in-memory hot reload.
    let mut old_a = old.clone();
    let mut new_a = new.clone();
    normalize_for_reload_diff(&mut old_a);
    normalize_for_reload_diff(&mut new_a);
    if old_a == new_a {
        return ChangeKind::HotReloadable { new_model, new_debug };
    }

    // Stage B: additionally ignore sandbox.providers → providers hot-reconcile.
    if let Some(s) = old_a.sandbox.as_mut() {
        s.providers.clear();
    }
    if let Some(s) = new_a.sandbox.as_mut() {
        s.providers.clear();
    }
    if old_a == new_a {
        return ChangeKind::ProvidersReload {
            new_model,
            new_debug,
            new_config: Box::new(new),
        };
    }

    ChangeKind::RestartRequired
}
```
Note the edge case (acceptable): adding the *first* provider when the agent has no `sandbox:` section at all yields `old.sandbox == None` vs `new.sandbox == Some{providers:[] after clear}` → not equal → `RestartRequired`. Real agents always have a `sandbox:` section (openshell mode), and the dashboard appends under it, so this path is not hit in practice.

- [ ] **Step 5:** `devenv shell -- cargo test -p bot diff_` → PASS (new + existing). The watcher loop `match` is now non-exhaustive for the non-test build; Task 5 adds the arm.

- [ ] **Step 6: Commit**
```bash
git add crates/bot/src/config_watcher.rs
git commit -m "feat(config-watcher): classify providers-only change as ProvidersReload"
```

---

## Task 5: Watcher loop arm + mpsc sender (no restart)

**Files:** Modify `crates/bot/src/config_watcher.rs`.

Bridge the sync watcher thread to async via `tokio::sync::mpsc::UnboundedSender<Box<AgentConfig>>` (its `send` is sync, callable off-runtime). On `ProvidersReload`: apply model/debug like `HotReloadable`, send the fresh config, update `last_yaml`, **continue** (no `token.cancel()`).

- [ ] **Step 1: Add the sender parameter** to `spawn_config_watcher`:
```rust
pub(crate) fn spawn_config_watcher(
    agent_yaml: &Path,
    token: CancellationToken,
    config_changed: Arc<AtomicBool>,
    model_swap: Arc<ArcSwap<Option<String>>>,
    debug_flag: Arc<AtomicBool>,
    initial_debug: bool,
    providers_tx: tokio::sync::mpsc::UnboundedSender<Box<AgentConfig>>,
) -> miette::Result<()> {
```
Ensure `providers_tx` is captured by the `std::thread::spawn(move || ...)` closure (it is `Send`).

- [ ] **Step 2: Add the loop arm** after the `RestartRequired` arm in the `match diff_classify(...)`:
```rust
                        ChangeKind::ProvidersReload {
                            new_model,
                            new_debug,
                            new_config,
                        } => {
                            tracing::info!(
                                providers = new_config.sandbox.as_ref().map(|s| s.providers.len()).unwrap_or(0),
                                "agent.yaml: providers-only change — hot reconcile without restart"
                            );
                            model_swap.store(Arc::new(new_model));
                            debug_flag.store(new_debug.unwrap_or(initial_debug), Ordering::Release);
                            if let Err(e) = providers_tx.send(new_config) {
                                tracing::warn!(error = %format!("{e}"),
                                    "providers reconcile channel closed — recovery loop will retry");
                            }
                            last_yaml = new_yaml;
                        }
```

- [ ] **Step 3: Build** — `devenv shell -- cargo build -p bot` → FAIL at the `lib.rs` callsite (missing arg). Fixed in Task 6.

- [ ] **Step 4: Commit**
```bash
git add crates/bot/src/config_watcher.rs
git commit -m "feat(config-watcher): signal providers reconcile over mpsc instead of restart"
```

---

## Task 6: Wire the channel + consumer in `lib.rs`

**Files:** Modify `crates/bot/src/lib.rs`.

Create the channel at the watcher spawn (where `config`/`args`/`agent_dir` exist). Pass the sender to the watcher. Spawn the consumer **after** `resolved_sandbox` is computed (~L607), so it has the sandbox name; call `hot_reconcile_providers` with the FRESH config from the channel.

- [ ] **Step 1: Edit the watcher spawn** (Task 0 anchor). Add the channel and pass the sender:
```rust
    let (providers_tx, providers_rx) =
        tokio::sync::mpsc::unbounded_channel::<Box<right_agent::agent::types::AgentConfig>>();
    config_watcher::spawn_config_watcher(
        &agent_yaml_path,
        shutdown.clone(),
        Arc::clone(&config_changed),
        Arc::clone(&model_arc),
        Arc::clone(&debug_flag),
        args.debug,
        providers_tx,
    )?;
```

- [ ] **Step 2: Spawn the consumer** after `resolved_sandbox` is computed (find `let resolved_sandbox` via Task 0):
```rust
    // Drain providers-reconcile signals from the config watcher (no restart path).
    {
        let mut providers_rx = providers_rx;
        let agent = args.agent.clone();
        let agent_dir = agent_dir.clone();
        let resolved_sandbox = resolved_sandbox.clone();
        tokio::spawn(async move {
            while let Some(new_cfg) = providers_rx.recv().await {
                let Some(sandbox) = resolved_sandbox.as_deref() else {
                    continue; // mode: none — no sandbox to reconcile
                };
                tracing::info!("hot-reconciling providers from agent.yaml change");
                if let Err(e) = sandbox_supervisor::hot_reconcile_providers(
                    &agent, &agent_dir, sandbox, &new_cfg,
                ).await {
                    tracing::warn!(error = %format!("{e:#}"),
                        "providers hot-reconcile failed; supervisor recovery loop will retry");
                }
            }
        });
    }
```
Confirm the variable names against the file (`args.agent`, `agent_dir`, `resolved_sandbox: Option<String>`). `providers_rx` must be moved into exactly one consumer; if the watcher spawn and this consumer are far apart, keep `providers_rx` un-moved at creation and move it here.

- [ ] **Step 3: Build** — `devenv shell -- cargo build -p bot` → compiles.

- [ ] **Step 4: Targeted test** — `devenv shell -- cargo test -p bot config_watcher` → PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/bot/src/lib.rs
git commit -m "feat(bot): hot-reconcile providers on agent.yaml change without restart"
```

---

## Task 7: Documentation (cite-on-touch — mandatory)

- [ ] **Step 1: `ARCHITECTURE.md`** — in "Hot-reloadable fields in `agent.yaml`", record that a `sandbox.providers`-only change is also hot-applied: classified `ProvidersReload`, applies model/debug in-memory, and signals an async `hot_reconcile_providers` (policy re-apply + gateway reconcile) instead of restarting. End with: "Adding more hot-reloadable fields requires extending the two-stage diff in `crates/bot/src/config_watcher.rs::diff_classify`." Keep ≤3 sentences (40k budget).

- [ ] **Step 2: `docs/architecture/providers.md`** — add the invariant: every full `policy.yaml` regeneration MUST fold generic-provider stanzas via `right_codegen::policy::apply_provider_stanzas(&generate_policy(...), providers)`; callsites are `sandbox_supervisor::bring_up_sandbox`, `right_codegen::pipeline::run_single_agent_codegen`, and the `right/src/main.rs` init helpers. A `sandbox.providers` change hot-reconciles via `config_watcher` (no restart) and self-heals within the supervisor recovery loop as fallback.

- [ ] **Step 3: Commit**
```bash
git add ARCHITECTURE.md docs/architecture/providers.md
git commit -m "docs: provider-aware policy regen invariant + providers hot-reload"
```

---

## Task 8: Final verification

- [ ] **Step 1:** `devenv shell -- cargo test --workspace` → PASS (mandatory; investigate any new failure).
- [ ] **Step 2:** `devenv shell -- cargo build --workspace` → clean.
- [ ] **Step 3:** `devenv shell -- cargo clippy -p right-codegen -p bot -p right -- -D warnings` → no warnings.
- [ ] **Step 4 (manual E2E, recommended):** With a running bot, add a provider via `/providers` for a sandboxed agent, then:
```bash
curl -s "http://localhost:18927/process/logs/right-bot/0/80"
```
Expected: log "providers-only change — hot reconcile without restart"; **no** process restart; `~/.right/agents/<name>/policy.yaml` contains `- host: <upstream_host>` above `# right-providers: insert-above`; next agent turn using that provider authenticates (no 401).

---

## Self-Review Notes

- **Spec coverage:** #2 = Tasks 1-2 (every regen folds providers, durable across all restart/recovery triggers). #1 = Tasks 3-6 (providers-only change → live re-apply + reconcile, no restart). Docs = Task 7. Verification = Tasks 0, 8.
- **Type consistency (verified against verbatim source):** `ProviderType::Generic` (enum, not string); `ProviderEntry.label: Option`, `.generic: Option<GenericProvider>`; `AgentConfig.sandbox: Option<SandboxConfig>` (always `as_ref()/as_mut()`); `HostMcpAccess::BootstrapUnresolved` / `Resolved(Vec<IpAddr>)`; `NetworkPolicy::{Permissive,Restrictive}` unit; `apply_provider_stanzas(&str, &[ProviderEntry]) -> Result<String, PolicyConflict>` used identically in Tasks 1/2/3/regen-test; `ProvidersReload { new_model, new_debug, new_config: Box<AgentConfig> }` defined Task 4, consumed Task 5, produced over `UnboundedSender<Box<AgentConfig>>` Tasks 5↔6; `hot_reconcile_providers(agent, agent_dir, resolved_sandbox, config)` defined Task 3, called Task 6.
- **Watcher async-bridge:** the watcher thread has no tokio context; it only `send`s over the unbounded channel (sync) — the async work runs in the `lib.rs` consumer task. This avoids `block_on` inside the watcher thread.
- **#1 depends on #2:** `hot_reconcile_providers` and `bring_up_sandbox` both call `apply_provider_stanzas`; do Tasks 1-2 before 3-6.
- **Soft spots (confirm via Task 0 `rg`, not placeholders):** exact line numbers and a few enclosing variable names in `lib.rs`/`main.rs`/`pipeline.rs`, the `policy.yaml` path helper in `bring_up_sandbox`, and each callsite's error type for the `map_err` form. All new code is exact; only the match-and-replace anchors need confirmation.
