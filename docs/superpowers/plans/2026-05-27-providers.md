# Providers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Providers feature: a Telegram Mini App dashboard view + internal API + gRPC/CLI wrappers that manage OpenShell provider credentials attached to sandboxed agents.

**Architecture:** Per-agent `providers: [...]` list in `agent.yaml` is the source of truth; OpenShell gateway holds the credential bytes; sandbox sees placeholder env vars. New module `right_openshell::providers` is the sole gRPC/CLI client. New routes in `crates/right/src/internal_api.rs` perform synchronous gateway+yaml mutations. New Vue view + bot dashboard handlers expose the surface. Reference: `docs/superpowers/specs/2026-05-27-providers-design.md`.

**Tech Stack:** Rust 2024, tonic (gRPC), axum (HTTP), Vue 3 + Vite, Telegram Mini App. OpenShell ≥ v0.0.30.

**Cadence (per AGENTS.md):** Run targeted tests after each task. Run `devenv shell -- cargo test --workspace` once at the end of the worktree, not after every task. Live-OpenShell `ci_openshell_provider_*` tests stay `#[ignore]` locally and only run under CI.

---

## Phase 1 — Foundations

### Task 1: Probe OpenShell proto + CLI surface; document findings

**Files:**
- Create: `crates/right-openshell/PROVIDER_NOTES.md` (scratch — DELETED before phase 10)

**Why:** The spec flagged two open questions: (1) is there a gRPC for sandbox provider attach/detach or only CLI? (2) does `UpdateConfig` flip `providers_v2_enabled` via `global=true, setting_key="providers_v2_enabled"`? Verify before writing wrappers.

- [ ] **Step 1: Confirm sandbox provider attach RPC absence**

Run: `grep -n "rpc " crates/right-openshell/proto/openshell/openshell.proto | grep -i "attach\|detach\|UpdateSandbox"`

Expected: no matches for `AttachProvider` / `DetachProvider` / `UpdateSandbox`. This locks in the design choice to shell out to `openshell sandbox provider attach/detach` for runtime ops.

- [ ] **Step 2: Confirm UpdateConfig accepts the v2 flag**

Run: `openshell settings set --global --key providers_v2_enabled --value true --yes`

Expected: command succeeds. Then run: `openshell settings get --global` and confirm `providers_v2_enabled: true` is in the output. (Note: this requires a live OpenShell gateway. If unavailable, document the assumption in PROVIDER_NOTES.md and continue — Task 4 will verify against a live gateway.)

- [ ] **Step 3: Confirm openshell provider profiles list**

Run: `openshell provider list-profiles`

Record the 10 type slugs (`anthropic`, `claude`, `codex`, `copilot`, `github`, `gitlab`, `nvidia`, `openai`, `opencode`, `generic`) and the canonical env-var name each built-in injects. Put them in PROVIDER_NOTES.md as a table — Task 8 hardcodes this.

- [ ] **Step 4: Capture the placeholder string format**

Create a throwaway generic provider + sandbox via CLI (`openshell provider create --name probe --type generic --credential MY_TOKEN=secret`, `openshell sandbox create --name probe-sandbox --provider probe --no-tty` with any minimal policy), wait READY, then `openshell sandbox exec probe-sandbox -- printenv MY_TOKEN`. Confirm output matches `openshell:resolve:env:v<digits>_MY_TOKEN`. Delete both. Record in PROVIDER_NOTES.md.

- [ ] **Step 5: Commit notes file**

```bash
git add crates/right-openshell/PROVIDER_NOTES.md
git commit -m "chore(providers): probe notes — open questions resolved"
```

---

### Task 2: Add agent.yaml schema for providers

**Files:**
- Modify: `crates/right-agent-config/src/lib.rs:255-279` (SandboxConfig)

- [ ] **Step 1: Write failing tests**

Append at the bottom of `crates/right-agent-config/src/lib.rs` inside the existing test module:

```rust
#[test]
fn sandbox_providers_parses_built_in_entry() {
    let yaml = "sandbox:\n  mode: openshell\n  providers:\n    - name: foo-anthropic\n      type: anthropic\n";
    let cfg: AgentConfig = serde_yaml::from_str(yaml).unwrap();
    let sandbox = cfg.sandbox.unwrap();
    assert_eq!(sandbox.providers.len(), 1);
    let entry = &sandbox.providers[0];
    assert_eq!(entry.name, "foo-anthropic");
    assert_eq!(entry.type_, ProviderType::BuiltIn("anthropic".into()));
    assert!(entry.label.is_none());
    assert!(entry.generic.is_none());
}

#[test]
fn sandbox_providers_parses_generic_entry() {
    let yaml = "sandbox:\n  mode: openshell\n  providers:\n    - name: foo-acme\n      type: generic\n      label: acme\n      generic:\n        env_var: ACME_TOKEN\n        header_name: X-Acme-Token\n        upstream_host: api.acme.com\n        upstream_path_prefix: /v1\n";
    let cfg: AgentConfig = serde_yaml::from_str(yaml).unwrap();
    let entry = &cfg.sandbox.unwrap().providers[0];
    assert_eq!(entry.type_, ProviderType::Generic);
    assert_eq!(entry.label.as_deref(), Some("acme"));
    let g = entry.generic.as_ref().unwrap();
    assert_eq!(g.env_var, "ACME_TOKEN");
    assert_eq!(g.header_name, "X-Acme-Token");
    assert_eq!(g.upstream_host, "api.acme.com");
    assert_eq!(g.upstream_path_prefix.as_deref(), Some("/v1"));
}

#[test]
fn sandbox_providers_defaults_to_empty() {
    let yaml = "sandbox: { mode: openshell }";
    let cfg: AgentConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(cfg.sandbox.unwrap().providers.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-agent-config sandbox_providers`
Expected: FAIL — `ProviderType`/`providers` field unknown.

- [ ] **Step 3: Add new types and field**

In `crates/right-agent-config/src/lib.rs`, add above the existing `pub struct SandboxConfig`:

```rust
/// Provider type slug. Built-in slugs are validated against the OpenShell
/// profile catalog at API boundaries; `claude` is rejected by Right (see
/// `crates/right/src/internal_api.rs`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ProviderType {
    /// `"generic"` — custom user-defined provider.
    #[serde(deserialize_with = "deserialize_generic_marker")]
    Generic,
    /// Built-in slug like `"anthropic"`, `"github"`, etc.
    BuiltIn(String),
}

fn deserialize_generic_marker<'de, D: serde::Deserializer<'de>>(d: D) -> Result<(), D::Error> {
    let s: String = serde::Deserialize::deserialize(d)?;
    if s == "generic" { Ok(()) } else {
        Err(serde::de::Error::custom("expected \"generic\""))
    }
}

impl serde::Serialize for ProviderType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ProviderType::Generic => s.serialize_str("generic"),
            ProviderType::BuiltIn(slug) => s.serialize_str(slug),
        }
    }
}

/// Generic-only fields.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct GenericProvider {
    pub env_var: String,
    #[serde(default = "default_header_name")]
    pub header_name: String,
    pub upstream_host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_path_prefix: Option<String>,
}

fn default_header_name() -> String { "Authorization".to_string() }

/// One provider attached to an agent's sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct ProviderEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: ProviderType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic: Option<GenericProvider>,
}
```

Then modify `SandboxConfig` to add the new field (around line 255-269):

```rust
pub struct SandboxConfig {
    #[serde(default)]
    pub mode: SandboxMode,
    pub policy_file: Option<PathBuf>,
    #[serde(default)]
    pub name: Option<String>,
    /// Providers attached to this sandbox. Empty by default. Per-agent source of truth.
    #[serde(default)]
    pub providers: Vec<ProviderEntry>,
}
```

And in `impl Default for SandboxConfig`:
```rust
            providers: Vec::new(),
```

If the existing test module is not yet `pub use` re-exporting `ProviderType`, add a `pub use` in the parent module or `use super::*;` in tests.

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-agent-config sandbox_providers`
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent-config/src/lib.rs
git commit -m "feat(agent-config): add SandboxConfig.providers field + ProviderEntry/ProviderType types"
```

---

### Task 3: Create right_openshell::providers module skeleton

**Files:**
- Create: `crates/right-openshell/src/providers.rs`
- Modify: `crates/right-openshell/src/lib.rs`

- [ ] **Step 1: Add module file with just types and errors**

Create `crates/right-openshell/src/providers.rs`:

```rust
//! OpenShell Provider gRPC + CLI wrappers.
//!
//! This module is the SOLE owner of the OpenShell Provider client.
//! All Provider RPCs and `openshell provider` / `openshell sandbox provider`
//! CLI invocations go through here (see ARCHITECTURE.md).

use std::collections::HashMap;
use std::process::Stdio;

use thiserror::Error;
use tokio::process::Command;

/// All provider operation errors. Each is FAIL FAST — never swallowed.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider gateway unreachable: {0:#}")]
    GatewayUnreachable(#[source] miette::ErrReport),
    #[error("openshell gRPC: {0:#}")]
    Grpc(String),
    #[error("openshell CLI {cmd:?} exited {status}: {stderr}")]
    Cli { cmd: String, status: i32, stderr: String },
    #[error("provider \"{0}\" not found")]
    NotFound(String),
    #[error("providers_v2_enabled is not on; run `right up` to enable")]
    V2NotEnabled,
    #[error("invalid provider: {0}")]
    Invalid(String),
}

/// Input for create/update.
#[derive(Debug, Clone)]
pub struct ProviderSpec {
    pub name: String,
    pub type_: String,                            // raw slug
    pub credentials: HashMap<String, String>,
    pub config: HashMap<String, String>,
}

/// Output of get/list. Credentials field is INTENTIONALLY OMITTED — the
/// gateway returns them, but Right never reads or stores them on host.
#[derive(Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub type_: String,
    pub config: HashMap<String, String>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Profile entry surfaced by `/provider-types` to the dashboard.
#[derive(Debug, Clone)]
pub struct ProviderProfile {
    pub type_slug: String,
    pub env_var: String,
    pub display_name: String,
    pub category: ProviderCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCategory {
    Inference,
    Agent,
    SourceControl,
    Messaging,
    Other,
}
```

- [ ] **Step 2: Register module**

In `crates/right-openshell/src/lib.rs`, add:

```rust
pub mod providers;
```

- [ ] **Step 3: Compile**

Run: `devenv shell -- cargo check -p right-openshell`
Expected: clean compile. No new tests yet.

- [ ] **Step 4: Commit**

```bash
git add crates/right-openshell/src/providers.rs crates/right-openshell/src/lib.rs
git commit -m "feat(openshell): providers module skeleton (errors + DTOs)"
```

---

## Phase 2 — gRPC + CLI wrappers

### Task 4: ensure_v2_enabled (gateway settings)

**Files:**
- Modify: `crates/right-openshell/src/providers.rs`
- Test: `crates/right-openshell/tests/ci_openshell_provider.rs` (NEW)

- [ ] **Step 1: Write failing live test**

Create `crates/right-openshell/tests/ci_openshell_provider.rs`:

```rust
//! Live OpenShell gateway tests. Each test is `#[ignore]` (ci-openshell:)
//! and runs only in CI; see AGENTS.md cadence rules.

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_v2_flip() {
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint()
        .await
        .expect("resolve gateway");
    // Idempotent: returns Ok regardless of prior state.
    let first = right_openshell::providers::ensure_v2_enabled(&endpoint)
        .await
        .expect("ensure_v2_enabled #1");
    let second = right_openshell::providers::ensure_v2_enabled(&endpoint)
        .await
        .expect("ensure_v2_enabled #2");
    // Second call must observe `was_already_on = true`.
    assert!(second.was_already_on);
    let _ = first; // first may be either branch depending on starting state
}
```

- [ ] **Step 2: Verify it fails to compile**

Run: `devenv shell -- cargo test -p right-openshell --tests --no-run`
Expected: FAIL — `ensure_v2_enabled` does not exist.

- [ ] **Step 3: Implement ensure_v2_enabled**

In `providers.rs`, add:

```rust
/// Return value of `ensure_v2_enabled`.
pub struct V2EnableResult {
    /// True when the flag was already `true` before the call.
    pub was_already_on: bool,
}

/// Ensure `providers_v2_enabled=true` on the gateway. Idempotent.
///
/// Uses `openshell settings set --global --key providers_v2_enabled --value true --yes`
/// rather than the raw `UpdateConfig` gRPC, because the CLI also validates the
/// setting key against the gateway's registered-keys table and handles the
/// confirmation prompt for global mutations.
pub async fn ensure_v2_enabled(
    endpoint: &crate::openshell::GatewayEndpoint,
) -> Result<V2EnableResult, ProviderError> {
    let current = get_v2_flag(endpoint).await?;
    if current {
        return Ok(V2EnableResult { was_already_on: true });
    }
    let mut cmd = Command::new("openshell");
    cmd.args([
        "settings", "set", "--global",
        "--key", "providers_v2_enabled",
        "--value", "true",
        "--yes",
    ]);
    endpoint.apply_to_cli(&mut cmd);
    let output = cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| ProviderError::Cli {
            cmd: "openshell settings set".into(),
            status: -1,
            stderr: e.to_string(),
        })?;
    if !output.status.success() {
        return Err(ProviderError::Cli {
            cmd: "openshell settings set".into(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(V2EnableResult { was_already_on: false })
}

async fn get_v2_flag(
    endpoint: &crate::openshell::GatewayEndpoint,
) -> Result<bool, ProviderError> {
    // Implementation note: prefer gRPC GetGatewayConfig if a Rust wrapper
    // exists in openshell.rs; otherwise shell out to `openshell settings get
    // --global --key providers_v2_enabled --output json` and parse.
    let mut cmd = Command::new("openshell");
    cmd.args(["settings", "get", "--global", "--key", "providers_v2_enabled"]);
    endpoint.apply_to_cli(&mut cmd);
    let out = cmd.output().await.map_err(|e| ProviderError::Cli {
        cmd: "openshell settings get".into(),
        status: -1,
        stderr: e.to_string(),
    })?;
    if !out.status.success() {
        // Not-set returns non-zero on some versions; treat as false.
        return Ok(false);
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout.contains("true"))
}
```

If `crate::openshell::GatewayEndpoint` doesn't already exist with `apply_to_cli`, add it. Check `crates/right-openshell/src/openshell.rs` for the endpoint type and grep for `OPENSHELL_GATEWAY_ENDPOINT`. If absent, add:

```rust
// in openshell.rs
#[derive(Debug, Clone)]
pub struct GatewayEndpoint(pub Option<String>);

impl GatewayEndpoint {
    pub fn apply_to_cli(&self, cmd: &mut tokio::process::Command) {
        if let Some(url) = &self.0 {
            cmd.args(["--gateway-endpoint", url]);
        }
    }
}

pub async fn resolve_gateway_endpoint() -> miette::Result<GatewayEndpoint> {
    Ok(GatewayEndpoint(std::env::var("OPENSHELL_GATEWAY_ENDPOINT").ok()))
}
```

- [ ] **Step 4: Compile + run test locally without `--ignored`**

Run: `devenv shell -- cargo test -p right-openshell ci_openshell_provider_v2_flip`
Expected: SKIPPED (ignored). Run with `--ignored` only on a host that has a live gateway.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/providers.rs crates/right-openshell/src/openshell.rs crates/right-openshell/tests/ci_openshell_provider.rs
git commit -m "feat(openshell): ensure_v2_enabled with idempotent get-then-set"
```

---

### Task 5: CRUD provider wrappers via CLI

**Files:**
- Modify: `crates/right-openshell/src/providers.rs`
- Test: `crates/right-openshell/tests/ci_openshell_provider.rs`

- [ ] **Step 1: Write failing live test for create + get**

Append to `ci_openshell_provider.rs`:

```rust
#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_create_get_delete_roundtrip() {
    use right_openshell::providers::*;
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await.unwrap();
    let _ = ensure_v2_enabled(&endpoint).await.unwrap();

    let name = format!("rightprobe-{}-roundtrip", std::process::id());
    let mut creds = std::collections::HashMap::new();
    creds.insert("MY_TOKEN".to_string(), "secret-value".to_string());
    let spec = ProviderSpec {
        name: name.clone(),
        type_: "generic".into(),
        credentials: creds,
        config: Default::default(),
    };
    let created = create_provider(&endpoint, &spec).await.unwrap();
    assert_eq!(created.name, name);
    assert_eq!(created.type_, "generic");

    let got = get_provider(&endpoint, &name).await.unwrap();
    assert_eq!(got.name, name);

    delete_provider(&endpoint, &name).await.unwrap();

    let after = get_provider(&endpoint, &name).await;
    assert!(matches!(after, Err(ProviderError::NotFound(_))));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell --tests --no-run`
Expected: FAIL — `create_provider`, `get_provider`, `delete_provider` not found.

- [ ] **Step 3: Implement CLI-shell CRUD**

Append to `providers.rs`:

```rust
pub async fn create_provider(
    endpoint: &crate::openshell::GatewayEndpoint,
    spec: &ProviderSpec,
) -> Result<Provider, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["provider", "create", "--name", &spec.name, "--type", &spec.type_]);
    for (k, v) in &spec.credentials {
        cmd.arg("--credential").arg(format!("{k}={v}"));
    }
    for (k, v) in &spec.config {
        cmd.arg("--config").arg(format!("{k}={v}"));
    }
    cmd.arg("--output").arg("json");
    endpoint.apply_to_cli(&mut cmd);
    let out = run_cli(cmd, "openshell provider create").await?;
    parse_provider_json(&out)
}

pub async fn get_provider(
    endpoint: &crate::openshell::GatewayEndpoint,
    name: &str,
) -> Result<Provider, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["provider", "get", "--name", name, "--output", "json"]);
    endpoint.apply_to_cli(&mut cmd);
    let out = cmd.stderr(Stdio::piped()).stdout(Stdio::piped()).output().await
        .map_err(|e| ProviderError::Cli {
            cmd: "openshell provider get".into(), status: -1, stderr: e.to_string(),
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("not found") || stderr.contains("NotFound") {
            return Err(ProviderError::NotFound(name.to_string()));
        }
        return Err(ProviderError::Cli {
            cmd: "openshell provider get".into(),
            status: out.status.code().unwrap_or(-1),
            stderr: stderr.into_owned(),
        });
    }
    parse_provider_json(&out.stdout)
}

pub async fn update_provider(
    endpoint: &crate::openshell::GatewayEndpoint,
    spec: &ProviderSpec,
) -> Result<Provider, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["provider", "update", "--name", &spec.name]);
    for (k, v) in &spec.credentials {
        cmd.arg("--credential").arg(format!("{k}={v}"));
    }
    for (k, v) in &spec.config {
        cmd.arg("--config").arg(format!("{k}={v}"));
    }
    cmd.arg("--output").arg("json");
    endpoint.apply_to_cli(&mut cmd);
    let out = run_cli(cmd, "openshell provider update").await?;
    parse_provider_json(&out)
}

pub async fn delete_provider(
    endpoint: &crate::openshell::GatewayEndpoint,
    name: &str,
) -> Result<(), ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["provider", "delete", "--name", name, "--yes"]);
    endpoint.apply_to_cli(&mut cmd);
    let _ = run_cli(cmd, "openshell provider delete").await?;
    Ok(())
}

pub async fn list_providers_by_prefix(
    endpoint: &crate::openshell::GatewayEndpoint,
    prefix: &str,
) -> Result<Vec<Provider>, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["provider", "list", "--output", "json"]);
    endpoint.apply_to_cli(&mut cmd);
    let out = run_cli(cmd, "openshell provider list").await?;
    let arr: Vec<serde_json::Value> = serde_json::from_slice(&out)
        .map_err(|e| ProviderError::Grpc(format!("parse provider list: {e:#}")))?;
    let mut providers = Vec::new();
    for v in arr {
        if let Some(name) = v.get("metadata").and_then(|m| m.get("name")).and_then(|n| n.as_str())
            && name.starts_with(prefix)
        {
            providers.push(provider_from_json(&v)?);
        }
    }
    Ok(providers)
}

async fn run_cli(mut cmd: Command, label: &str) -> Result<Vec<u8>, ProviderError> {
    let out = cmd.stderr(Stdio::piped()).stdout(Stdio::piped()).output().await
        .map_err(|e| ProviderError::Cli {
            cmd: label.into(), status: -1, stderr: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(ProviderError::Cli {
            cmd: label.into(),
            status: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(out.stdout)
}

fn parse_provider_json(bytes: &[u8]) -> Result<Provider, ProviderError> {
    let v: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::Grpc(format!("parse provider: {e:#}")))?;
    provider_from_json(&v)
}

fn provider_from_json(v: &serde_json::Value) -> Result<Provider, ProviderError> {
    let name = v.get("metadata").and_then(|m| m.get("name")).and_then(|n| n.as_str())
        .ok_or_else(|| ProviderError::Grpc("missing metadata.name".into()))?;
    let type_ = v.get("type").and_then(|t| t.as_str())
        .ok_or_else(|| ProviderError::Grpc("missing type".into()))?;
    let mut config = HashMap::new();
    if let Some(obj) = v.get("config").and_then(|c| c.as_object()) {
        for (k, val) in obj {
            if let Some(s) = val.as_str() { config.insert(k.clone(), s.to_string()); }
        }
    }
    let updated_at = v.get("metadata")
        .and_then(|m| m.get("updated_at"))
        .and_then(|u| u.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));
    Ok(Provider { name: name.to_string(), type_: type_.to_string(), config, updated_at })
}
```

If `chrono` is not in `right-openshell` already, add to its `Cargo.toml`: `chrono = { version = "0.4", features = ["serde"] }`.

- [ ] **Step 4: Compile + verify tests build (still ignored)**

Run: `devenv shell -- cargo test -p right-openshell --tests --no-run`
Expected: clean compile.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/providers.rs crates/right-openshell/tests/ci_openshell_provider.rs crates/right-openshell/Cargo.toml
git commit -m "feat(openshell): provider CRUD wrappers via openshell CLI + JSON parse"
```

---

### Task 6: attach_to_sandbox / detach_from_sandbox

**Files:**
- Modify: `crates/right-openshell/src/providers.rs`
- Test: `crates/right-openshell/tests/ci_openshell_provider.rs`

- [ ] **Step 1: Write failing live test**

Append:

```rust
#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_attach_detach() {
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await.unwrap();
    let _ = ensure_v2_enabled(&endpoint).await.unwrap();

    let pid = std::process::id();
    let prov_name = format!("rightprobe-{pid}-attachprov");
    let mut creds = std::collections::HashMap::new();
    creds.insert("RIGHTPROBE_TOKEN".into(), "secret".into());
    create_provider(&endpoint, &ProviderSpec {
        name: prov_name.clone(), type_: "generic".into(),
        credentials: creds, config: Default::default(),
    }).await.unwrap();

    let sandbox = TestSandbox::create("ci-openshell-provider-attach-detach").await.unwrap();
    attach_to_sandbox(&endpoint, sandbox.name(), &prov_name).await.unwrap();
    detach_from_sandbox(&endpoint, sandbox.name(), &prov_name).await.unwrap();

    delete_provider(&endpoint, &prov_name).await.unwrap();
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell --tests --no-run`
Expected: FAIL — `attach_to_sandbox` not found.

- [ ] **Step 3: Implement attach/detach via CLI**

Append to `providers.rs`:

```rust
pub async fn attach_to_sandbox(
    endpoint: &crate::openshell::GatewayEndpoint,
    sandbox_name: &str,
    provider_name: &str,
) -> Result<(), ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["sandbox", "provider", "attach", sandbox_name, provider_name]);
    endpoint.apply_to_cli(&mut cmd);
    let _ = run_cli(cmd, "openshell sandbox provider attach").await?;
    Ok(())
}

pub async fn detach_from_sandbox(
    endpoint: &crate::openshell::GatewayEndpoint,
    sandbox_name: &str,
    provider_name: &str,
) -> Result<(), ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["sandbox", "provider", "detach", sandbox_name, provider_name]);
    endpoint.apply_to_cli(&mut cmd);
    let _ = run_cli(cmd, "openshell sandbox provider detach").await?;
    Ok(())
}

pub async fn list_attached(
    endpoint: &crate::openshell::GatewayEndpoint,
    sandbox_name: &str,
) -> Result<Vec<String>, ProviderError> {
    let mut cmd = Command::new("openshell");
    cmd.args(["sandbox", "provider", "list", sandbox_name, "--output", "json"]);
    endpoint.apply_to_cli(&mut cmd);
    let out = run_cli(cmd, "openshell sandbox provider list").await?;
    let arr: Vec<serde_json::Value> = serde_json::from_slice(&out)
        .map_err(|e| ProviderError::Grpc(format!("parse attached: {e:#}")))?;
    let mut names = Vec::new();
    for v in arr {
        if let Some(n) = v.get("name").and_then(|s| s.as_str()) {
            names.push(n.to_string());
        }
    }
    Ok(names)
}
```

If the CLI returns the attached list with a different JSON shape, parse accordingly — verify with `openshell sandbox provider list <sandbox> --output json` during step 4.

- [ ] **Step 4: Verify CLI JSON shape against live gateway (manual)**

Run (with any existing sandbox): `openshell sandbox provider list <sandbox> --output json`. Confirm the field name is `name`. If different, update `list_attached` accordingly.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/providers.rs crates/right-openshell/tests/ci_openshell_provider.rs
git commit -m "feat(openshell): provider attach/detach/list-attached via sandbox provider CLI"
```

---

### Task 7: get_sandbox_provider_environment

**Files:**
- Modify: `crates/right-openshell/src/providers.rs`

- [ ] **Step 1: Implement diagnostics call**

The proto has `GetSandboxProviderEnvironment` as a real gRPC, not a CLI command — so this is the one wrapper that needs a real tonic client. Look at how existing RPCs are called in `crates/right-openshell/src/openshell.rs` (search for `tonic::transport::Endpoint`). Append to `providers.rs`:

```rust
/// Fetch the env-var map that will be injected into the sandbox.
///
/// SAFETY: the returned values are opaque placeholders (e.g.
/// `openshell:resolve:env:v<digits>_<NAME>`) — never log them. They look
/// secret-shaped to operators and create false alarms in audits.
pub async fn get_sandbox_provider_environment(
    endpoint: &crate::openshell::GatewayEndpoint,
    sandbox_id: &str,
) -> Result<HashMap<String, String>, ProviderError> {
    use crate::proto::openshell::v1::{
        openshell_service_client::OpenshellServiceClient,
        GetSandboxProviderEnvironmentRequest,
    };
    let mut client = crate::openshell::connect_grpc(endpoint).await
        .map_err(ProviderError::GatewayUnreachable)?;
    let resp = client
        .get_sandbox_provider_environment(GetSandboxProviderEnvironmentRequest {
            sandbox_id: sandbox_id.to_string(),
        })
        .await
        .map_err(|s| ProviderError::Grpc(s.to_string()))?
        .into_inner();
    Ok(resp.environment)
}
```

Adjust the import paths after running `cargo check`; existing code in `openshell.rs` already imports from `crate::proto::openshell::v1` — match it.

- [ ] **Step 2: Compile**

Run: `devenv shell -- cargo check -p right-openshell`
Expected: clean. If `connect_grpc` does not exist publicly, extract the existing gRPC connection helper from `openshell.rs` into a `pub(crate) async fn connect_grpc` first.

- [ ] **Step 3: Commit**

```bash
git add crates/right-openshell/src/providers.rs crates/right-openshell/src/openshell.rs
git commit -m "feat(openshell): get_sandbox_provider_environment (diagnostics only)"
```

---

### Task 8: Hardcoded provider profile catalog

**Files:**
- Modify: `crates/right-openshell/src/providers.rs`

- [ ] **Step 1: Write unit test**

Append to `providers.rs` (inline `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_excludes_claude() {
        let catalog = profile_catalog();
        assert!(!catalog.iter().any(|p| p.type_slug == "claude"));
    }

    #[test]
    fn catalog_has_8_built_in_plus_generic() {
        let catalog = profile_catalog();
        let built_in: Vec<&str> = catalog.iter()
            .filter(|p| p.type_slug != "generic")
            .map(|p| p.type_slug.as_str()).collect();
        assert_eq!(built_in.len(), 8);
        for expected in ["anthropic", "codex", "copilot", "github", "gitlab", "nvidia", "openai", "opencode"] {
            assert!(built_in.contains(&expected), "missing {expected}");
        }
        assert!(catalog.iter().any(|p| p.type_slug == "generic"));
    }

    #[test]
    fn catalog_anthropic_uses_anthropic_api_key() {
        let entry = profile_catalog().into_iter()
            .find(|p| p.type_slug == "anthropic").unwrap();
        assert_eq!(entry.env_var, "ANTHROPIC_API_KEY");
    }
}
```

- [ ] **Step 2: Run failing**

Run: `devenv shell -- cargo test -p right-openshell catalog_`
Expected: FAIL — `profile_catalog` undefined.

- [ ] **Step 3: Implement profile_catalog using values from Task 1 notes**

Append to `providers.rs`:

```rust
/// Static catalog of provider profiles Right exposes. Sourced from
/// `openshell provider list-profiles` (recorded in PROVIDER_NOTES.md during
/// Task 1). The `claude` profile is intentionally omitted — see spec.
pub fn profile_catalog() -> Vec<ProviderProfile> {
    use ProviderCategory::*;
    vec![
        ProviderProfile { type_slug: "anthropic".into(), env_var: "ANTHROPIC_API_KEY".into(), display_name: "Anthropic API".into(), category: Inference },
        ProviderProfile { type_slug: "openai".into(),    env_var: "OPENAI_API_KEY".into(),    display_name: "OpenAI".into(),       category: Inference },
        ProviderProfile { type_slug: "nvidia".into(),    env_var: "NVIDIA_API_KEY".into(),    display_name: "NVIDIA".into(),       category: Inference },
        ProviderProfile { type_slug: "codex".into(),     env_var: "OPENAI_API_KEY".into(),    display_name: "Codex".into(),        category: Agent },
        ProviderProfile { type_slug: "copilot".into(),   env_var: "GITHUB_TOKEN".into(),      display_name: "GitHub Copilot".into(),category: Agent },
        ProviderProfile { type_slug: "opencode".into(),  env_var: "OPENCODE_API_KEY".into(),  display_name: "OpenCode".into(),     category: Agent },
        ProviderProfile { type_slug: "github".into(),    env_var: "GITHUB_TOKEN".into(),      display_name: "GitHub".into(),       category: SourceControl },
        ProviderProfile { type_slug: "gitlab".into(),    env_var: "GITLAB_TOKEN".into(),      display_name: "GitLab".into(),       category: SourceControl },
        ProviderProfile { type_slug: "generic".into(),   env_var: "".into(),                  display_name: "Generic".into(),      category: Other },
    ]
}
```

If Task 1's notes show different canonical env-var names (e.g. `OPENAI_BASE_URL` for `openai`), update accordingly. The `generic` entry uses an empty env_var because users supply their own.

- [ ] **Step 4: Run tests**

Run: `devenv shell -- cargo test -p right-openshell catalog_`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/providers.rs
git commit -m "feat(openshell): hardcoded provider profile catalog (8 built-in + generic)"
```

---

## Phase 3 — Sandbox spawn wiring

### Task 9: spawn_sandbox accepts providers

**Files:**
- Modify: `crates/right-openshell/src/openshell.rs:535-562` (spawn_sandbox)
- Modify: all call sites of `spawn_sandbox` (find via `grep -rn "spawn_sandbox(" crates/`)

- [ ] **Step 1: Update signature + emit --provider flags**

Change `pub fn spawn_sandbox(name, policy_path, upload_dir)` to `pub fn spawn_sandbox(name: &str, policy_path: &Path, upload_dir: Option<&Path>, providers: &[String])`. Inside, after the `--policy` arg block, add:

```rust
for prov in providers {
    cmd.arg("--provider").arg(prov);
}
```

- [ ] **Step 2: Update all call sites**

Run: `grep -rn "spawn_sandbox(" crates/ --include='*.rs'`

For each call site, pass either `&[]` (legacy / migration) or the agent's `sandbox.providers.iter().map(|p| p.name.clone()).collect::<Vec<_>>()`. The agent-init / first-create path passes the empty slice because providers are added later via the dashboard.

- [ ] **Step 3: Compile**

Run: `devenv shell -- cargo check -p right-openshell -p right-agent`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(openshell): spawn_sandbox accepts --provider flags"
```

---

### Task 10: Live test — env var visible to subprocess

**Files:**
- Test: `crates/right-openshell/tests/ci_openshell_provider.rs`

- [ ] **Step 1: Append integration test**

```rust
#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_create_attach_env_visible() {
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await.unwrap();
    let _ = ensure_v2_enabled(&endpoint).await.unwrap();

    let pid = std::process::id();
    let prov = format!("rightprobe-{pid}-envvisible");
    let mut creds = std::collections::HashMap::new();
    creds.insert("RIGHTPROBE_ENVVISIBLE".into(), "secret".into());
    create_provider(&endpoint, &ProviderSpec {
        name: prov.clone(), type_: "generic".into(),
        credentials: creds, config: Default::default(),
    }).await.unwrap();
    let sandbox = TestSandbox::create("ci-openshell-provider-env-visible").await.unwrap();
    attach_to_sandbox(&endpoint, sandbox.name(), &prov).await.unwrap();

    let out = sandbox.exec(&["printenv", "RIGHTPROBE_ENVVISIBLE"]).await.unwrap();
    assert!(
        out.stdout.starts_with(b"openshell:resolve:env:"),
        "expected placeholder, got {}", String::from_utf8_lossy(&out.stdout)
    );

    detach_from_sandbox(&endpoint, sandbox.name(), &prov).await.unwrap();
    delete_provider(&endpoint, &prov).await.unwrap();
}
```

If `TestSandbox::exec` returns a different shape, mirror existing usage in `crates/right-openshell/src/sandbox_exec.rs` callers.

- [ ] **Step 2: Compile**

Run: `devenv shell -- cargo test -p right-openshell --tests --no-run`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/right-openshell/tests/ci_openshell_provider.rs
git commit -m "test(openshell): provider env shim visible to sandbox subprocess"
```

---
### Task 10b: Remaining live OpenShell tests from spec

**Files:**
- Test: `crates/right-openshell/tests/ci_openshell_provider.rs`
- Test: `crates/right-codegen/src/policy_provider_tests.rs`

**Why:** Spec lists seven `ci_openshell_provider_*` tests; tasks 4, 6, 10 cover three. This task adds the remaining four so the inventory matches.

- [ ] **Step 1: rotate-no-restart test**

Add to `ci_openshell_provider.rs`. Mark `#[ignore = "ci-openshell: ..."]`. Pattern:

1. `ensure_v2_enabled(&endpoint)`.
2. `create_provider` with `ROT_TOKEN=first`.
3. `TestSandbox::create("ci-openshell-provider-rotate")`.
4. `attach_to_sandbox(&endpoint, sandbox.name(), &prov)`.
5. Read the placeholder via `sandbox.run(&["printenv", "ROT_TOKEN"])` (use whichever method the existing TestSandbox helper exposes for in-sandbox command runs — check `crates/right-openshell/src/test_support.rs`).
6. `update_provider` with `ROT_TOKEN=second`.
7. Read placeholder again; assert it differs from the first (placeholder version suffix changes when the credential rotates).
8. `detach_from_sandbox` + `delete_provider`.

The intent is to prove rotation works without a sandbox restart. The placeholder strings differ because the gateway assigns a new credential version.

- [ ] **Step 2: policy-hot-apply test**

Mark `#[ignore = "ci-openshell: ..."]`. Pattern:

1. Define `base` policy YAML with a single `api.anthropic.com` REST endpoint.
2. Call `right_codegen::policy::providers_append(base, "ci-myagent-acme", "api.acme.invalid", None)` and assert the resulting string contains both `managed-by: right-providers:ci-myagent-acme` and the new domain.
3. `TestSandbox::create("ci-openshell-provider-policy-hot-apply")`.
4. Write `base` to a tempdir-backed `policy.yaml` and call `right_codegen::contract::write_apply_with_snapshot(sandbox.name(), &policy_path, new)`.
5. Assert the on-disk policy contains `api.acme.invalid`.
6. Call `snap.restore().await` and assert the on-disk policy is restored byte-for-byte to `base`.

- [ ] **Step 3: raw-tunnel-conflict test (pure unit, no live gateway)**

Add to `crates/right-codegen/src/policy_provider_tests.rs`, next to the Task 11 tests:

1. Define a policy YAML containing `- domain: api.example.invalid` with `tls: skip`.
2. Call `providers_append_checked(...)` for that same host.
3. Assert `Err(PolicyConflict::RawTunnel { .. })`.

No `#[ignore]` needed — this is a unit-level test of the string mutator. Included here so the spec's seven-test inventory is complete.

- [ ] **Step 4: destroy-cascade test (wrapper level)**

Mark `#[ignore = "ci-openshell: ..."]`. Pattern:

1. `ensure_v2_enabled`.
2. `create_provider` with `CASCADE_TOKEN=value`.
3. `delete_provider(&endpoint, &prov)`.
4. Call `get_provider(&endpoint, &prov)`; assert `Err(ProviderError::NotFound(_))`.

The full end-to-end destroy path through `right agent destroy` is exercised at task 34; this wrapper-level test isolates the building block.

- [ ] **Step 5: Compile + commit**

```bash
devenv shell -- cargo test -p right-openshell --tests --no-run
devenv shell -- cargo test -p right-codegen provider_policy_raw_tunnel_conflict
git add -A
git commit -m "test(openshell): rotate, policy hot-apply, raw-tunnel, destroy-cascade tests"
```

---


## Phase 4 — Policy reconcile helpers

### Task 11: Policy.yaml loader/saver preserving comments

**Files:**
- Modify or create: `crates/right-codegen/src/policy.rs` (depending on current content)

**Why:** YAML round-trip via `serde_yaml` loses comments. The managed-by tag is a YAML comment. Use `yaml-rust2` (or `serde_yaml::Value` round-trip with documents) for the providers-section edit. Simplest correct approach: use string-level edits scoped by a fenced region.

- [ ] **Step 1: Add unit test for round-trip preservation**

Add `crates/right-codegen/src/policy_provider_tests.rs` (NEW):

```rust
use super::policy::*;

const POLICY_WITHOUT_PROVIDERS: &str = r#"
network:
  endpoints:
    - domain: api.anthropic.com
      protocol: rest
      access: full
"#;

const POLICY_WITH_ONE_PROVIDER: &str = r#"
network:
  endpoints:
    - domain: api.anthropic.com
      protocol: rest
      access: full
    # managed-by: right-providers:myagent-acme
    - domain: api.acme.com
      protocol: rest
      access: full
"#;

#[test]
fn append_provider_endpoint_inserts_tagged_stanza() {
    let after = providers_append(POLICY_WITHOUT_PROVIDERS, "myagent-acme", "api.acme.com", None);
    assert!(after.contains("managed-by: right-providers:myagent-acme"));
    assert!(after.contains("- domain: api.acme.com"));
}

#[test]
fn append_provider_endpoint_existing_rest_is_noop() {
    let already = "network:\n  endpoints:\n    - domain: api.acme.com\n      protocol: rest\n      access: full\n";
    let after = providers_append(already, "myagent-acme", "api.acme.com", None);
    assert_eq!(after, already);
}

#[test]
fn append_provider_endpoint_raw_tunnel_conflict() {
    let raw = "network:\n  endpoints:\n    - allowed_ips: [1.2.3.4/32]\n      tls: skip\n      ports: [443]\n";
    // tls:skip is a raw tunnel; with no domain we can't conflict — that's OK.
    let after = providers_append(raw, "myagent-acme", "api.acme.com", None);
    assert!(after.contains("managed-by: right-providers:myagent-acme"));
}

#[test]
fn append_provider_endpoint_conflicting_domain_raw_tunnel_returns_err() {
    let raw = "network:\n  endpoints:\n    - domain: api.acme.com\n      tls: skip\n";
    let err = providers_append_checked(raw, "myagent-acme", "api.acme.com", None);
    assert!(matches!(err, Err(PolicyConflict::RawTunnel { .. })));
}

#[test]
fn strip_provider_endpoint_removes_tagged() {
    let after = providers_strip(POLICY_WITH_ONE_PROVIDER, "myagent-acme", "api.acme.com");
    assert!(!after.contains("managed-by: right-providers:myagent-acme"));
    assert!(!after.contains("api.acme.com"));
}
```

- [ ] **Step 2: Run failing**

Run: `devenv shell -- cargo test -p right-codegen append_provider_endpoint`
Expected: FAIL — functions undefined.

- [ ] **Step 3: Implement provider policy mutators**

Add to `crates/right-codegen/src/policy.rs` (or create as a sub-module):

```rust
#[derive(Debug, thiserror::Error)]
pub enum PolicyConflict {
    #[error("host {host} is configured as raw tunnel (tls: skip) — cannot terminate for substitution")]
    RawTunnel { host: String },
}

/// Append a TLS-terminated endpoint for `host` tagged with the provider
/// name, unless an entry for the same domain already exists.
pub fn providers_append(
    policy: &str,
    provider_name: &str,
    host: &str,
    path_prefix: Option<&str>,
) -> String {
    providers_append_checked(policy, provider_name, host, path_prefix)
        .unwrap_or_else(|e| panic!("policy conflict: {e:#}"))
}

pub fn providers_append_checked(
    policy: &str,
    provider_name: &str,
    host: &str,
    path_prefix: Option<&str>,
) -> Result<String, PolicyConflict> {
    // Quick scan: look for an `- domain: <host>` line and inspect adjacent
    // `tls: skip` to detect raw-tunnel conflicts.
    let host_marker = format!("- domain: {host}");
    if let Some(idx) = policy.find(&host_marker) {
        // Look ahead within the next ~10 lines for tls: skip.
        let window: &str = &policy[idx..policy[idx..].len().min(400) + idx];
        if window.contains("tls: skip") {
            return Err(PolicyConflict::RawTunnel { host: host.to_string() });
        }
        // Already present (assume rest by default or terminate) — no-op.
        return Ok(policy.to_string());
    }

    // Append a new stanza under `endpoints:` (locate the key).
    let endpoints_idx = match policy.find("endpoints:") {
        Some(i) => i,
        None => return Ok(format!("{policy}\nnetwork:\n  endpoints:\n    # managed-by: right-providers:{provider_name}\n    - domain: {host}\n      protocol: rest\n      access: full\n")),
    };
    // Find the insertion point: end of the endpoints list (next non-indented line).
    let after_endpoints = &policy[endpoints_idx..];
    let list_end = after_endpoints.lines()
        .enumerate()
        .skip(1)
        .find(|(_, l)| !l.is_empty() && !l.starts_with(' '))
        .map(|(i, _)| i)
        .unwrap_or_else(|| after_endpoints.lines().count());
    // Compute byte offset to insert before that line.
    let mut byte_offset = endpoints_idx;
    for (i, line) in after_endpoints.lines().enumerate() {
        if i == list_end { break; }
        byte_offset += line.len() + 1; // +1 for '\n'
    }
    let path_line = path_prefix
        .map(|p| format!("      path: {p}\n"))
        .unwrap_or_default();
    let stanza = format!(
        "    # managed-by: right-providers:{provider_name}\n    - domain: {host}\n      protocol: rest\n      access: full\n{path_line}"
    );
    let mut out = String::with_capacity(policy.len() + stanza.len());
    out.push_str(&policy[..byte_offset]);
    out.push_str(&stanza);
    out.push_str(&policy[byte_offset..]);
    Ok(out)
}

/// Remove a previously-appended tagged stanza. No-op if not present.
pub fn providers_strip(policy: &str, provider_name: &str, host: &str) -> String {
    let tag = format!("# managed-by: right-providers:{provider_name}");
    let Some(tag_idx) = policy.find(&tag) else { return policy.to_string() };
    // Find the beginning of the comment line.
    let line_start = policy[..tag_idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
    // The stanza consists of the comment line + the following `- domain: <host>`
    // block ending at the next sibling `- ` (same indent) or de-indent.
    let mut end_byte = tag_idx + tag.len();
    let after = &policy[end_byte..];
    let _ = host; // currently unused; future: validate end_byte advances past matching domain
    for line in after.lines() {
        end_byte += line.len() + 1;
        // Stop when we encounter the next sibling list item or a de-indented line.
        let next = &policy[end_byte..];
        let first_line = next.lines().next().unwrap_or("");
        if first_line.starts_with("    - ") || (!first_line.is_empty() && !first_line.starts_with(' ')) {
            break;
        }
    }
    let mut out = String::with_capacity(policy.len() - (end_byte - line_start));
    out.push_str(&policy[..line_start]);
    out.push_str(&policy[end_byte..]);
    out
}
```

Wire the module into `policy.rs` either inline or as `mod policy_providers; pub use policy_providers::*;`.

- [ ] **Step 4: Run tests**

Run: `devenv shell -- cargo test -p right-codegen append_provider_endpoint strip_provider_endpoint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-codegen/src/policy.rs crates/right-codegen/src/policy_provider_tests.rs
git commit -m "feat(codegen): policy.yaml provider-endpoint append/strip with managed-by tag"
```

---

### Task 12: Policy hot-apply helper

**Files:**
- Modify: `crates/right-codegen/src/contract.rs` or wherever `write_and_apply_sandbox_policy` lives

- [ ] **Step 1: Check existing helper**

Run: `grep -rn "write_and_apply_sandbox_policy\|policy set --wait" crates/`. The helper exists per ARCHITECTURE.md. Re-use it. If only the file-write variant exists, add the apply variant.

- [ ] **Step 2: Add snapshot-restore wrapper**

In the same file, add:

```rust
/// Write policy + hot-apply with rollback support.
///
/// Returns a `PolicySnapshot` carrying the prior bytes; call `.restore()` to
/// roll back if a follow-on gateway operation fails.
pub async fn write_apply_with_snapshot(
    sandbox_name: &str,
    policy_path: &std::path::Path,
    new_content: String,
) -> miette::Result<PolicySnapshot> {
    let prior = std::fs::read_to_string(policy_path)
        .map_err(|e| miette::miette!("read policy.yaml: {e:#}"))?;
    write_and_apply_sandbox_policy(sandbox_name, policy_path, &new_content).await?;
    Ok(PolicySnapshot { path: policy_path.to_path_buf(), prior, sandbox: sandbox_name.to_string() })
}

pub struct PolicySnapshot {
    path: std::path::PathBuf,
    prior: String,
    sandbox: String,
}

impl PolicySnapshot {
    pub async fn restore(self) -> miette::Result<()> {
        write_and_apply_sandbox_policy(&self.sandbox, &self.path, &self.prior).await
    }
}
```

- [ ] **Step 3: Compile**

Run: `devenv shell -- cargo check -p right-codegen`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/right-codegen/src/contract.rs
git commit -m "feat(codegen): write_apply_with_snapshot for transactional policy hot-apply"
```

---

## Phase 5 — Internal API

### Task 13: ProviderApiError enum + HTTP mapping

**Files:**
- Create: `crates/right/src/internal_api_providers.rs` (or inline in `internal_api.rs` — choose inline if `internal_api.rs` is under 800 LoC; else this new file)
- Modify: `crates/right/src/internal_api.rs`

- [ ] **Step 1: Decide split**

Run: `wc -l crates/right/src/internal_api.rs`. If > 700 lines, create the new file and link via `mod internal_api_providers;`. Otherwise inline.

- [ ] **Step 2: Add error enum + axum response impl**

```rust
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
    #[error("providers are only available for sandboxed agents (sandbox.mode = openshell)")]
    SandboxModeNone,
    #[error("providers_v2_enabled is not on the gateway")]
    V2NotEnabled,
    #[error("policy conflict on host \"{host}\": {kind}")]
    PolicyConflict { host: String, kind: String },
    #[error("openshell gateway: {0}")]
    Gateway(String),
    #[error("agent.yaml write failed after gateway change: {0}")]
    AgentYamlWrite(String),
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
            Self::SandboxModeNone => (StatusCode::BAD_REQUEST, "sandbox_mode_none"),
            Self::V2NotEnabled => (StatusCode::SERVICE_UNAVAILABLE, "v2_not_enabled"),
            Self::PolicyConflict { .. } => (StatusCode::CONFLICT, "policy_conflict"),
            Self::Gateway(_) => (StatusCode::BAD_GATEWAY, "gateway"),
            Self::AgentYamlWrite(_) => (StatusCode::INTERNAL_SERVER_ERROR, "agent_yaml_write"),
        };
        (status, axum::Json(serde_json::json!({"code": code, "message": format!("{self}")}))).into_response()
    }
}
```

- [ ] **Step 3: Compile + commit**

```bash
devenv shell -- cargo check -p right
git add -A
git commit -m "feat(internal-api): ProviderApiError taxonomy + HTTP mapping"
```

---

### Task 14: Validators (name, env var, slug)

**Files:**
- Modify: same file as Task 13

- [ ] **Step 1: Tests**

```rust
#[cfg(test)]
mod provider_validation_tests {
    use super::*;

    #[test]
    fn name_must_match_agent_prefix() {
        assert!(validate_name("myagent", "myagent-anthropic").is_ok());
        let err = validate_name("myagent", "other-anthropic").unwrap_err();
        assert!(matches!(err, ProviderApiError::InvalidName { .. }));
    }

    #[test]
    fn slug_pattern_enforced() {
        let err = validate_name("myagent", "myagent-Anthropic").unwrap_err();
        assert!(matches!(err, ProviderApiError::InvalidName { .. }));
        let err2 = validate_name("myagent", "myagent-").unwrap_err();
        assert!(matches!(err2, ProviderApiError::InvalidName { .. }));
    }

    #[test]
    fn env_var_validation() {
        assert!(validate_env_var("MY_TOKEN").is_ok());
        assert!(validate_env_var("X_API_KEY_2").is_ok());
        assert!(matches!(validate_env_var("my-token"), Err(ProviderApiError::InvalidEnvVar { .. })));
        assert!(matches!(validate_env_var("1FOO"), Err(ProviderApiError::InvalidEnvVar { .. })));
    }

    #[test]
    fn claude_type_rejected() {
        assert!(matches!(validate_type_slug("claude"), Err(ProviderApiError::InvalidName { .. })));
        assert!(validate_type_slug("anthropic").is_ok());
        assert!(validate_type_slug("generic").is_ok());
    }
}
```

- [ ] **Step 2: Implement validators**

```rust
pub fn validate_name(agent: &str, name: &str) -> Result<(), ProviderApiError> {
    let expected_prefix = format!("{agent}-");
    if !name.starts_with(&expected_prefix) {
        return Err(ProviderApiError::InvalidName { name: name.into(), reason: format!("must start with \"{expected_prefix}\"") });
    }
    let slug = &name[expected_prefix.len()..];
    if slug.is_empty() || slug.len() > 32 {
        return Err(ProviderApiError::InvalidName { name: name.into(), reason: "slug must be 1-32 chars".into() });
    }
    let mut chars = slug.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(ProviderApiError::InvalidName { name: name.into(), reason: "slug must start with a-z".into() });
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(ProviderApiError::InvalidName { name: name.into(), reason: "slug allows [a-z0-9-]".into() });
        }
    }
    if name.len() > 64 {
        return Err(ProviderApiError::InvalidName { name: name.into(), reason: "total length > 64".into() });
    }
    Ok(())
}

pub fn validate_env_var(name: &str) -> Result<(), ProviderApiError> {
    if name.is_empty() || name.len() > 64 {
        return Err(ProviderApiError::InvalidEnvVar { env_var: name.into() });
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_uppercase() || first == '_') {
        return Err(ProviderApiError::InvalidEnvVar { env_var: name.into() });
    }
    for c in chars {
        if !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
            return Err(ProviderApiError::InvalidEnvVar { env_var: name.into() });
        }
    }
    Ok(())
}

pub fn validate_type_slug(slug: &str) -> Result<(), ProviderApiError> {
    if slug == "claude" {
        return Err(ProviderApiError::InvalidName { name: slug.into(), reason: "type \"claude\" is reserved for the in-sandbox login flow".into() });
    }
    let known = ["anthropic", "codex", "copilot", "github", "gitlab", "nvidia", "openai", "opencode", "generic"];
    if !known.contains(&slug) {
        return Err(ProviderApiError::InvalidName { name: slug.into(), reason: format!("unknown type \"{slug}\"") });
    }
    Ok(())
}
```

- [ ] **Step 3: Run tests + commit**

```bash
devenv shell -- cargo test -p right provider_validation_tests
git add -A
git commit -m "feat(internal-api): provider name/env-var/type validators"
```

---

### Task 15: Request/response DTOs + provider-list route

**Files:** same file.

- [ ] **Step 1: DTOs**

```rust
#[derive(Debug, serde::Deserialize)]
pub struct ProviderListReq { pub agent: String }

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
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderStatus {
    Healthy,
    Missing,
    GatewayError { message: String },
}
```

- [ ] **Step 2: Handler**

```rust
async fn handle_provider_list(
    State(state): State<InternalApiState>,
    axum::Json(req): axum::Json<ProviderListReq>,
) -> Result<axum::Json<Vec<ProviderView>>, ProviderApiError> {
    let cfg = load_agent_config(&state.home, &req.agent)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;
    let sandbox = cfg.sandbox.as_ref().ok_or(ProviderApiError::SandboxModeNone)?;
    if sandbox.mode != right_agent_config::SandboxMode::Openshell {
        return Err(ProviderApiError::SandboxModeNone);
    }
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    let mut views = Vec::with_capacity(sandbox.providers.len());
    for entry in &sandbox.providers {
        let status = match right_openshell::providers::get_provider(&endpoint, &entry.name).await {
            Ok(_) => ProviderStatus::Healthy,
            Err(right_openshell::providers::ProviderError::NotFound(_)) => ProviderStatus::Missing,
            Err(e) => ProviderStatus::GatewayError { message: format!("{e:#}") },
        };
        let env_var = match &entry.type_ {
            right_agent_config::ProviderType::Generic => entry.generic.as_ref().map(|g| g.env_var.clone()).unwrap_or_default(),
            right_agent_config::ProviderType::BuiltIn(slug) => right_openshell::providers::profile_catalog()
                .into_iter().find(|p| &p.type_slug == slug).map(|p| p.env_var).unwrap_or_default(),
        };
        let type_str = match &entry.type_ {
            right_agent_config::ProviderType::Generic => "generic".to_string(),
            right_agent_config::ProviderType::BuiltIn(s) => s.clone(),
        };
        views.push(ProviderView {
            name: entry.name.clone(),
            type_: type_str,
            label: entry.label.clone(),
            env_var,
            generic: entry.generic.clone(),
            updated_at: None, // backfilled later if we add GetProvider metadata
            status,
        });
    }
    Ok(axum::Json(views))
}
```

`load_agent_config` should already exist (used by `mcp_list`); find via `grep -rn "fn load_agent_config" crates/right/src/`. If not, add a simple `serde_yaml::from_reader` over `agent.yaml`.

- [ ] **Step 3: Register route**

Add to the existing router builder in `internal_api.rs`: `.route("/provider-list", post(handle_provider_list))`.

- [ ] **Step 4: Compile + commit**

```bash
devenv shell -- cargo check -p right
git add -A
git commit -m "feat(internal-api): /provider-list route + ProviderView DTO"
```

---

### Task 16: /provider-types route

**Files:** same.

- [ ] **Step 1: Handler**

```rust
#[derive(Debug, serde::Serialize)]
pub struct ProviderProfileView {
    #[serde(rename = "type")]
    pub type_slug: String,
    pub env_var: String,
    pub display_name: String,
    pub category: String,
}

async fn handle_provider_types() -> axum::Json<Vec<ProviderProfileView>> {
    let catalog = right_openshell::providers::profile_catalog();
    let views: Vec<_> = catalog.into_iter().map(|p| ProviderProfileView {
        type_slug: p.type_slug, env_var: p.env_var,
        display_name: p.display_name,
        category: format!("{:?}", p.category).to_lowercase(),
    }).collect();
    axum::Json(views)
}
```

Register `.route("/provider-types", post(handle_provider_types))`.

- [ ] **Step 2: Commit**

```bash
git add -A
git commit -m "feat(internal-api): /provider-types route"
```

---

### Task 17: /provider-create (built-in)

**Files:** same.

- [ ] **Step 1: Request DTO**

```rust
#[derive(Debug, serde::Deserialize)]
pub struct ProviderCreateReq {
    pub agent: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub label: Option<String>,
    pub credential: secrecy::SecretString,
    pub generic: Option<ProviderCreateGeneric>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ProviderCreateGeneric {
    pub env_var: String,
    pub header_name: Option<String>,
    pub upstream_host: String,
    pub upstream_path_prefix: Option<String>,
}
```

Add `secrecy = "0.10"` to `crates/right/Cargo.toml` if not present. The `SecretString` ensures the `Debug` impl shows `[REDACTED]`.

- [ ] **Step 2: Built-in branch handler** (generic branch in Task 18)

```rust
async fn handle_provider_create(
    State(state): State<InternalApiState>,
    axum::Json(req): axum::Json<ProviderCreateReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    use secrecy::ExposeSecret;
    validate_type_slug(&req.type_)?;
    let label_slug = req.label.clone().unwrap_or_else(|| req.type_.clone());
    let name = format!("{}-{}", req.agent, label_slug);
    validate_name(&req.agent, &name)?;

    let cfg = load_agent_config(&state.home, &req.agent)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;
    let sandbox = cfg.sandbox.as_ref().ok_or(ProviderApiError::SandboxModeNone)?;
    if sandbox.mode != right_agent_config::SandboxMode::Openshell {
        return Err(ProviderApiError::SandboxModeNone);
    }
    if sandbox.providers.iter().any(|p| p.name == name) {
        return Err(ProviderApiError::NameCollision { name });
    }
    let env_var = if req.type_ == "generic" {
        req.generic.as_ref().map(|g| g.env_var.clone()).ok_or_else(|| ProviderApiError::InvalidEnvVar { env_var: "".into() })?
    } else {
        right_openshell::providers::profile_catalog()
            .into_iter().find(|p| p.type_slug == req.type_).map(|p| p.env_var).unwrap_or_default()
    };
    validate_env_var(&env_var)?;
    if sandbox.providers.iter().any(|p| extract_env_var(p) == env_var) {
        return Err(ProviderApiError::EnvVarCollision { env_var });
    }

    if req.type_ == "generic" {
        return create_generic_provider(state, req, name, env_var).await;
    }

    // Built-in flow: skip policy mutation; OpenShell contributes endpoints.
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    let sandbox_name = sandbox.name.as_deref().unwrap_or(&req.agent);

    let mut creds = std::collections::HashMap::new();
    creds.insert(env_var.clone(), req.credential.expose_secret().to_string());
    let spec = right_openshell::providers::ProviderSpec {
        name: name.clone(), type_: req.type_.clone(),
        credentials: creds, config: Default::default(),
    };
    right_openshell::providers::create_provider(&endpoint, &spec).await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    if let Err(attach_err) = right_openshell::providers::attach_to_sandbox(&endpoint, sandbox_name, &name).await {
        // Rollback created provider.
        let _ = right_openshell::providers::delete_provider(&endpoint, &name).await;
        return Err(ProviderApiError::Gateway(format!("{attach_err:#}")));
    }

    // agent.yaml write.
    let entry = right_agent_config::ProviderEntry {
        name: name.clone(),
        type_: right_agent_config::ProviderType::BuiltIn(req.type_.clone()),
        label: req.label.clone(),
        generic: None,
    };
    if let Err(e) = append_provider_to_yaml(&state.home, &req.agent, &entry).await {
        // Best-effort rollback.
        let _ = right_openshell::providers::detach_from_sandbox(&endpoint, sandbox_name, &name).await;
        let _ = right_openshell::providers::delete_provider(&endpoint, &name).await;
        return Err(ProviderApiError::AgentYamlWrite(format!("{e:#}")));
    }

    Ok(axum::Json(ProviderView {
        name, type_: req.type_, label: req.label, env_var,
        generic: None, updated_at: None, status: ProviderStatus::Healthy,
    }))
}

fn extract_env_var(entry: &right_agent_config::ProviderEntry) -> String {
    match &entry.type_ {
        right_agent_config::ProviderType::Generic => entry.generic.as_ref().map(|g| g.env_var.clone()).unwrap_or_default(),
        right_agent_config::ProviderType::BuiltIn(slug) => right_openshell::providers::profile_catalog()
            .into_iter().find(|p| &p.type_slug == slug).map(|p| p.env_var).unwrap_or_default(),
    }
}

async fn append_provider_to_yaml(
    home: &std::path::Path,
    agent: &str,
    entry: &right_agent_config::ProviderEntry,
) -> miette::Result<()> {
    // Read-modify-write per MergedRMW category. Look at existing
    // `crates/right-codegen/src/contract.rs::write_merged_rmw` and call it.
    let path = home.join("agents").join(agent).join("agent.yaml");
    right_codegen::contract::write_merged_rmw(&path, |doc: &mut serde_yaml::Value| {
        let sandbox = doc.get_mut("sandbox").and_then(|v| v.as_mapping_mut())
            .ok_or_else(|| miette::miette!("sandbox section missing"))?;
        let providers = sandbox.entry("providers".into()).or_insert(serde_yaml::Value::Sequence(vec![]));
        let seq = providers.as_sequence_mut().ok_or_else(|| miette::miette!("providers not a sequence"))?;
        seq.push(serde_yaml::to_value(entry).unwrap());
        Ok(())
    }).await
}
```

`write_merged_rmw` is per ARCHITECTURE.md; if its signature differs, adapt the call. The helper must preserve unknown fields.

- [ ] **Step 3: Register route**

```rust
.route("/provider-create", axum::routing::post(handle_provider_create))
```

- [ ] **Step 4: Compile + commit**

```bash
devenv shell -- cargo check -p right
git add -A
git commit -m "feat(internal-api): /provider-create (built-in) with rollback chain"
```

---

### Task 18: /provider-create (generic) policy reconcile branch

**Files:** same.

- [ ] **Step 1: Implement generic branch**

```rust
async fn create_generic_provider(
    state: InternalApiState,
    req: ProviderCreateReq,
    name: String,
    env_var: String,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    use secrecy::ExposeSecret;
    let g = req.generic.clone().ok_or_else(|| ProviderApiError::InvalidEnvVar { env_var: "".into() })?;
    let header_name = g.header_name.clone().unwrap_or_else(|| "Authorization".into());

    let cfg = load_agent_config(&state.home, &req.agent)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;
    let sandbox = cfg.sandbox.as_ref().unwrap();
    let sandbox_name = sandbox.name.as_deref().unwrap_or(&req.agent);
    let policy_path = state.home.join("agents").join(&req.agent).join(
        sandbox.policy_file.clone().unwrap_or_else(|| std::path::PathBuf::from("policy.yaml"))
    );

    // Step 1: write policy.yaml + hot-apply.
    let prior = std::fs::read_to_string(&policy_path)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("read policy: {e:#}")))?;
    let new_policy = right_codegen::policy::providers_append_checked(
        &prior, &name, &g.upstream_host, g.upstream_path_prefix.as_deref()
    ).map_err(|e| match e {
        right_codegen::policy::PolicyConflict::RawTunnel { host } =>
            ProviderApiError::PolicyConflict { host, kind: "raw-tunnel".into() },
    })?;
    let snapshot = right_codegen::contract::write_apply_with_snapshot(
        sandbox_name, &policy_path, new_policy
    ).await.map_err(|e| ProviderApiError::Gateway(format!("policy apply: {e:#}")))?;

    // Step 2: CreateProvider.
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    let mut creds = std::collections::HashMap::new();
    creds.insert(env_var.clone(), req.credential.expose_secret().to_string());
    let mut config = std::collections::HashMap::new();
    config.insert("header_name".into(), header_name.clone());
    if let Some(prefix) = &g.upstream_path_prefix {
        config.insert("upstream_path_prefix".into(), prefix.clone());
    }
    config.insert("upstream_host".into(), g.upstream_host.clone());
    let spec = right_openshell::providers::ProviderSpec {
        name: name.clone(), type_: "generic".into(),
        credentials: creds, config,
    };
    if let Err(e) = right_openshell::providers::create_provider(&endpoint, &spec).await {
        let _ = snapshot.restore().await;
        return Err(ProviderApiError::Gateway(format!("{e:#}")));
    }

    // Step 3: attach.
    if let Err(attach_err) = right_openshell::providers::attach_to_sandbox(&endpoint, sandbox_name, &name).await {
        let _ = right_openshell::providers::delete_provider(&endpoint, &name).await;
        let _ = snapshot.restore().await;
        return Err(ProviderApiError::Gateway(format!("{attach_err:#}")));
    }

    // Step 4: agent.yaml write.
    let entry = right_agent_config::ProviderEntry {
        name: name.clone(),
        type_: right_agent_config::ProviderType::Generic,
        label: req.label.clone(),
        generic: Some(right_agent_config::GenericProvider {
            env_var: env_var.clone(),
            header_name: header_name.clone(),
            upstream_host: g.upstream_host.clone(),
            upstream_path_prefix: g.upstream_path_prefix.clone(),
        }),
    };
    if let Err(e) = append_provider_to_yaml(&state.home, &req.agent, &entry).await {
        let _ = right_openshell::providers::detach_from_sandbox(&endpoint, sandbox_name, &name).await;
        let _ = right_openshell::providers::delete_provider(&endpoint, &name).await;
        let _ = snapshot.restore().await;
        return Err(ProviderApiError::AgentYamlWrite(format!("{e:#}")));
    }

    Ok(axum::Json(ProviderView {
        name, type_: "generic".to_string(), label: req.label,
        env_var, generic: entry.generic, updated_at: None, status: ProviderStatus::Healthy,
    }))
}
```

- [ ] **Step 2: Compile + commit**

```bash
devenv shell -- cargo check -p right
git add -A
git commit -m "feat(internal-api): /provider-create generic branch with ordered rollback"
```

---

### Task 19: /provider-rotate

**Files:** same.

- [ ] **Step 1: Handler**

```rust
#[derive(Debug, serde::Deserialize)]
pub struct ProviderRotateReq {
    pub agent: String,
    pub name: String,
    pub credential: secrecy::SecretString,
}

async fn handle_provider_rotate(
    State(state): State<InternalApiState>,
    axum::Json(req): axum::Json<ProviderRotateReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    use secrecy::ExposeSecret;
    validate_name(&req.agent, &req.name)?;
    let cfg = load_agent_config(&state.home, &req.agent)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;
    let sandbox = cfg.sandbox.as_ref().ok_or(ProviderApiError::SandboxModeNone)?;
    let entry = sandbox.providers.iter().find(|p| p.name == req.name)
        .ok_or_else(|| ProviderApiError::NotFound { name: req.name.clone() })?;

    let env_var = extract_env_var(entry);
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    let mut creds = std::collections::HashMap::new();
    creds.insert(env_var.clone(), req.credential.expose_secret().to_string());
    let spec = right_openshell::providers::ProviderSpec {
        name: req.name.clone(),
        type_: match &entry.type_ {
            right_agent_config::ProviderType::Generic => "generic".into(),
            right_agent_config::ProviderType::BuiltIn(s) => s.clone(),
        },
        credentials: creds, config: Default::default(),
    };
    right_openshell::providers::update_provider(&endpoint, &spec).await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;

    Ok(axum::Json(ProviderView {
        name: req.name, type_: spec.type_, label: entry.label.clone(),
        env_var, generic: entry.generic.clone(),
        updated_at: Some(chrono::Utc::now()), status: ProviderStatus::Healthy,
    }))
}
```

Register: `.route("/provider-rotate", post(handle_provider_rotate))`.

- [ ] **Step 2: Commit**

```bash
devenv shell -- cargo check -p right
git add -A
git commit -m "feat(internal-api): /provider-rotate"
```

---

### Task 20: /provider-config-update (generic only)

**Files:** same.

- [ ] **Step 1: Handler**

```rust
#[derive(Debug, serde::Deserialize)]
pub struct ProviderConfigUpdateReq {
    pub agent: String,
    pub name: String,
    pub generic: ProviderConfigUpdateGeneric,
}

#[derive(Debug, serde::Deserialize)]
pub struct ProviderConfigUpdateGeneric {
    pub env_var: Option<String>,
    pub header_name: Option<String>,
    pub upstream_host: Option<String>,
    pub upstream_path_prefix: Option<Option<String>>, // None=no-change, Some(None)=clear
}

async fn handle_provider_config_update(
    State(state): State<InternalApiState>,
    axum::Json(req): axum::Json<ProviderConfigUpdateReq>,
) -> Result<axum::Json<ProviderView>, ProviderApiError> {
    let cfg = load_agent_config(&state.home, &req.agent)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;
    let sandbox = cfg.sandbox.as_ref().ok_or(ProviderApiError::SandboxModeNone)?;
    let entry = sandbox.providers.iter().find(|p| p.name == req.name)
        .ok_or_else(|| ProviderApiError::NotFound { name: req.name.clone() })?;
    if !matches!(entry.type_, right_agent_config::ProviderType::Generic) {
        return Err(ProviderApiError::InvalidName { name: req.name.clone(), reason: "config-update only valid on generic providers".into() });
    }
    let current = entry.generic.clone().unwrap();
    let new_env_var = req.generic.env_var.clone().unwrap_or(current.env_var);
    let new_header = req.generic.header_name.clone().unwrap_or(current.header_name);
    let new_host = req.generic.upstream_host.clone().unwrap_or(current.upstream_host.clone());
    let new_path = match req.generic.upstream_path_prefix.clone() {
        None => current.upstream_path_prefix,
        Some(v) => v,
    };
    validate_env_var(&new_env_var)?;

    let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    let sandbox_name = sandbox.name.as_deref().unwrap_or(&req.agent);
    let policy_path = state.home.join("agents").join(&req.agent).join(
        sandbox.policy_file.clone().unwrap_or_else(|| std::path::PathBuf::from("policy.yaml"))
    );

    let mut snapshot: Option<right_codegen::contract::PolicySnapshot> = None;
    if new_host != current.upstream_host {
        let prior = std::fs::read_to_string(&policy_path)
            .map_err(|e| ProviderApiError::AgentYamlWrite(format!("read policy: {e:#}")))?;
        let stripped = right_codegen::policy::providers_strip(&prior, &req.name, &current.upstream_host);
        let new_policy = right_codegen::policy::providers_append_checked(
            &stripped, &req.name, &new_host, new_path.as_deref()
        ).map_err(|e| match e {
            right_codegen::policy::PolicyConflict::RawTunnel { host } =>
                ProviderApiError::PolicyConflict { host, kind: "raw-tunnel".into() },
        })?;
        snapshot = Some(right_codegen::contract::write_apply_with_snapshot(
            sandbox_name, &policy_path, new_policy
        ).await.map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?);
    }

    let mut config = std::collections::HashMap::new();
    config.insert("header_name".into(), new_header.clone());
    config.insert("upstream_host".into(), new_host.clone());
    if let Some(p) = &new_path { config.insert("upstream_path_prefix".into(), p.clone()); }
    let spec = right_openshell::providers::ProviderSpec {
        name: req.name.clone(), type_: "generic".into(),
        credentials: Default::default(), config,
    };
    if let Err(e) = right_openshell::providers::update_provider(&endpoint, &spec).await {
        if let Some(s) = snapshot { let _ = s.restore().await; }
        return Err(ProviderApiError::Gateway(format!("{e:#}")));
    }

    // agent.yaml write.
    let updated = right_agent_config::ProviderEntry {
        name: req.name.clone(),
        type_: right_agent_config::ProviderType::Generic,
        label: entry.label.clone(),
        generic: Some(right_agent_config::GenericProvider {
            env_var: new_env_var.clone(), header_name: new_header.clone(),
            upstream_host: new_host.clone(), upstream_path_prefix: new_path.clone(),
        }),
    };
    if let Err(e) = replace_provider_in_yaml(&state.home, &req.agent, &updated).await {
        if let Some(s) = snapshot { let _ = s.restore().await; }
        return Err(ProviderApiError::AgentYamlWrite(format!("{e:#}")));
    }

    Ok(axum::Json(ProviderView {
        name: req.name, type_: "generic".into(),
        label: entry.label.clone(), env_var: new_env_var,
        generic: updated.generic, updated_at: Some(chrono::Utc::now()),
        status: ProviderStatus::Healthy,
    }))
}

async fn replace_provider_in_yaml(
    home: &std::path::Path, agent: &str,
    updated: &right_agent_config::ProviderEntry,
) -> miette::Result<()> {
    let path = home.join("agents").join(agent).join("agent.yaml");
    right_codegen::contract::write_merged_rmw(&path, |doc: &mut serde_yaml::Value| {
        let sandbox = doc.get_mut("sandbox").and_then(|v| v.as_mapping_mut())
            .ok_or_else(|| miette::miette!("sandbox section missing"))?;
        let providers = sandbox.get_mut("providers").and_then(|v| v.as_sequence_mut())
            .ok_or_else(|| miette::miette!("providers list missing"))?;
        let idx = providers.iter().position(|v| v.get("name").and_then(|n| n.as_str()) == Some(&updated.name))
            .ok_or_else(|| miette::miette!("provider not in list"))?;
        providers[idx] = serde_yaml::to_value(updated).unwrap();
        Ok(())
    }).await
}
```

Register: `.route("/provider-config-update", post(handle_provider_config_update))`.

- [ ] **Step 2: Commit**

```bash
devenv shell -- cargo check -p right
git add -A
git commit -m "feat(internal-api): /provider-config-update (generic only)"
```

---

### Task 21: /provider-remove + policy strip

**Files:** same.

- [ ] **Step 1: Handler**

```rust
#[derive(Debug, serde::Deserialize)]
pub struct ProviderRemoveReq { pub agent: String, pub name: String }

#[derive(Debug, serde::Serialize)]
pub struct ProviderRemoveResp { pub removed: bool }

async fn handle_provider_remove(
    State(state): State<InternalApiState>,
    axum::Json(req): axum::Json<ProviderRemoveReq>,
) -> Result<axum::Json<ProviderRemoveResp>, ProviderApiError> {
    validate_name(&req.agent, &req.name)?;
    let cfg = load_agent_config(&state.home, &req.agent)
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;
    let sandbox = cfg.sandbox.as_ref().ok_or(ProviderApiError::SandboxModeNone)?;
    let entry = sandbox.providers.iter().find(|p| p.name == req.name)
        .ok_or_else(|| ProviderApiError::NotFound { name: req.name.clone() })?
        .clone();
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    let sandbox_name = sandbox.name.as_deref().unwrap_or(&req.agent);

    right_openshell::providers::detach_from_sandbox(&endpoint, sandbox_name, &req.name).await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;
    right_openshell::providers::delete_provider(&endpoint, &req.name).await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))?;

    // Policy strip for generic.
    if let Some(g) = &entry.generic {
        let policy_path = state.home.join("agents").join(&req.agent).join(
            sandbox.policy_file.clone().unwrap_or_else(|| std::path::PathBuf::from("policy.yaml"))
        );
        let used_by_other = sandbox.providers.iter()
            .any(|p| p.name != req.name && p.generic.as_ref().map(|gp| gp.upstream_host == g.upstream_host).unwrap_or(false));
        if !used_by_other {
            let prior = std::fs::read_to_string(&policy_path)
                .map_err(|e| ProviderApiError::AgentYamlWrite(format!("read policy: {e:#}")))?;
            let stripped = right_codegen::policy::providers_strip(&prior, &req.name, &g.upstream_host);
            right_codegen::contract::write_and_apply_sandbox_policy(sandbox_name, &policy_path, &stripped).await
                .map_err(|e| ProviderApiError::Gateway(format!("policy apply: {e:#}")))?;
        }
    }

    remove_provider_from_yaml(&state.home, &req.agent, &req.name).await
        .map_err(|e| ProviderApiError::AgentYamlWrite(format!("{e:#}")))?;
    Ok(axum::Json(ProviderRemoveResp { removed: true }))
}

async fn remove_provider_from_yaml(
    home: &std::path::Path, agent: &str, name: &str,
) -> miette::Result<()> {
    let path = home.join("agents").join(agent).join("agent.yaml");
    right_codegen::contract::write_merged_rmw(&path, |doc: &mut serde_yaml::Value| {
        let sandbox = doc.get_mut("sandbox").and_then(|v| v.as_mapping_mut())
            .ok_or_else(|| miette::miette!("sandbox section missing"))?;
        if let Some(providers) = sandbox.get_mut("providers").and_then(|v| v.as_sequence_mut()) {
            providers.retain(|v| v.get("name").and_then(|n| n.as_str()) != Some(name));
        }
        Ok(())
    }).await
}
```

Register: `.route("/provider-remove", delete(handle_provider_remove))`.

- [ ] **Step 2: Commit**

```bash
devenv shell -- cargo check -p right
git add -A
git commit -m "feat(internal-api): /provider-remove + policy strip when last reference goes away"
```

---

### Task 22: Per-(agent, name) mutex

**Files:** same file, plus `InternalApiState` definition.

- [ ] **Step 1: Add mutex map to state**

In the `InternalApiState` struct definition (search `pub struct InternalApiState`):

```rust
pub provider_locks: std::sync::Arc<tokio::sync::Mutex<
    std::collections::HashMap<(String, String), std::sync::Arc<tokio::sync::Mutex<()>>>
>>,
```

Initialize in the constructor with `Default::default()`.

- [ ] **Step 2: Wrap mutation handlers**

Add helper:

```rust
async fn provider_lock(
    state: &InternalApiState, agent: &str, name: &str,
) -> tokio::sync::OwnedMutexGuard<()> {
    let key = (agent.to_string(), name.to_string());
    let lock = {
        let mut map = state.provider_locks.lock().await;
        map.entry(key).or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(()))).clone()
    };
    lock.lock_owned().await
}
```

In each of `handle_provider_create` / `_rotate` / `_config_update` / `_remove`, immediately after parsing the request, derive the name and:

```rust
let _guard = provider_lock(&state, &req.agent, &name).await;
```

Hold the guard for the entire handler body.

- [ ] **Step 3: Compile + commit**

```bash
devenv shell -- cargo check -p right
git add -A
git commit -m "feat(internal-api): per-(agent,name) mutex for provider mutations"
```

---

## Phase 6 — InternalClient binding

### Task 23: Add provider_*() methods to InternalClient

**Files:**
- Modify: `crates/right-mcp/src/internal_client.rs`

- [ ] **Step 1: Add request/response types**

```rust
#[derive(Debug, serde::Serialize)]
pub struct ProviderListRequest<'a> { pub agent: &'a str }

#[derive(Debug, serde::Deserialize)]
pub struct ProviderListResponse { pub providers: Vec<serde_json::Value> } // pass-through

#[derive(Debug, serde::Serialize)]
pub struct ProviderCreateRequest<'a> {
    pub agent: &'a str,
    #[serde(rename = "type")]
    pub type_: &'a str,
    pub label: Option<&'a str>,
    pub credential: &'a str,
    pub generic: Option<ProviderCreateGenericArg<'a>>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProviderCreateGenericArg<'a> {
    pub env_var: &'a str,
    pub header_name: Option<&'a str>,
    pub upstream_host: &'a str,
    pub upstream_path_prefix: Option<&'a str>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProviderRotateRequest<'a> {
    pub agent: &'a str, pub name: &'a str, pub credential: &'a str,
}

#[derive(Debug, serde::Serialize)]
pub struct ProviderRemoveRequest<'a> { pub agent: &'a str, pub name: &'a str }
```

- [ ] **Step 2: Methods**

```rust
impl InternalClient {
    pub async fn provider_list(&self, agent: &str) -> Result<Vec<serde_json::Value>, InternalClientError> {
        let resp: ProviderListResponse = self.post_json("/provider-list", &ProviderListRequest { agent }).await?;
        Ok(resp.providers)
    }
    pub async fn provider_types(&self) -> Result<Vec<serde_json::Value>, InternalClientError> {
        self.post_json("/provider-types", &serde_json::json!({})).await
    }
    pub async fn provider_create(&self, req: &ProviderCreateRequest<'_>) -> Result<serde_json::Value, InternalClientError> {
        self.post_json("/provider-create", req).await
    }
    pub async fn provider_rotate(&self, req: &ProviderRotateRequest<'_>) -> Result<serde_json::Value, InternalClientError> {
        self.post_json("/provider-rotate", req).await
    }
    pub async fn provider_remove(&self, req: &ProviderRemoveRequest<'_>) -> Result<serde_json::Value, InternalClientError> {
        self.delete_json("/provider-remove", req).await
    }
    pub async fn provider_config_update(&self, body: &serde_json::Value) -> Result<serde_json::Value, InternalClientError> {
        self.post_json("/provider-config-update", body).await
    }
}
```

The `post_json` / `delete_json` helpers must already exist (they're used by `mcp_add`). If `delete_json` doesn't exist, add it (mirror `post_json` with `Method::DELETE`).

- [ ] **Step 3: Compile + commit**

```bash
devenv shell -- cargo check -p right-mcp
git add -A
git commit -m "feat(mcp-client): provider_* methods on InternalClient"
```

---

## Phase 7 — Bot dashboard handlers

### Task 24: providers.rs handler module + register routes

**Files:**
- Create: `crates/bot/src/telegram/dashboard/providers.rs`
- Modify: `crates/bot/src/telegram/dashboard.rs` (register `mod providers;` + routes)

- [ ] **Step 1: Add `mod providers;` and routes**

In `dashboard.rs`, near `pub(crate) mod mcp;` add `pub(crate) mod providers;`. In `build_dashboard_router` add (after the MCP routes):

```rust
.route(
    "/dashboard/{agent}/api/v1/providers",
    get(providers::handle_list).post(providers::handle_create),
)
.route(
    "/dashboard/{agent}/api/v1/providers/types",
    get(providers::handle_types),
)
.route(
    "/dashboard/{agent}/api/v1/providers/{provider_name}",
    delete(providers::handle_remove),
)
.route(
    "/dashboard/{agent}/api/v1/providers/{provider_name}/rotate",
    axum::routing::post(providers::handle_rotate),
)
.route(
    "/dashboard/{agent}/api/v1/providers/{provider_name}/config",
    axum::routing::patch(providers::handle_config_update),
)
```

- [ ] **Step 2: Create handler file**

`crates/bot/src/telegram/dashboard/providers.rs`:

```rust
use axum::{
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use super::DashboardState;
use super::mcp::{authenticate_api, parse_json_body, json_error, internal_api_error_response};

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderCreateBody {
    #[serde(rename = "type")]
    pub type_: String,
    pub label: Option<String>,
    pub credential: String,
    pub generic: Option<ProviderCreateGenericBody>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderCreateGenericBody {
    pub env_var: String,
    pub header_name: Option<String>,
    pub upstream_host: String,
    pub upstream_path_prefix: Option<String>,
}

pub(crate) async fn handle_list(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) { return error.into_response(); }
    match state.internal_client.provider_list(&state.agent_name).await {
        Ok(list) => Json(serde_json::json!({"providers": list})).into_response(),
        Err(error) => internal_api_error_response(error, "provider_list_failed"),
    }
}

pub(crate) async fn handle_types(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) { return error.into_response(); }
    match state.internal_client.provider_types().await {
        Ok(list) => Json(serde_json::json!({"types": list})).into_response(),
        Err(error) => internal_api_error_response(error, "provider_types_failed"),
    }
}

pub(crate) async fn handle_create(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) { return error.into_response(); }
    let body: ProviderCreateBody = match parse_json_body(&body) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let generic = body.generic.as_ref().map(|g| right_mcp::internal_client::ProviderCreateGenericArg {
        env_var: &g.env_var,
        header_name: g.header_name.as_deref(),
        upstream_host: &g.upstream_host,
        upstream_path_prefix: g.upstream_path_prefix.as_deref(),
    });
    let req = right_mcp::internal_client::ProviderCreateRequest {
        agent: &state.agent_name,
        type_: &body.type_,
        label: body.label.as_deref(),
        credential: &body.credential,
        generic,
    };
    match state.internal_client.provider_create(&req).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_create_failed"),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderRotateBody { pub credential: String }

pub(crate) async fn handle_rotate(
    AxumPath((agent, provider_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) { return error.into_response(); }
    let body: ProviderRotateBody = match parse_json_body(&body) {
        Ok(b) => b, Err(resp) => return resp,
    };
    let req = right_mcp::internal_client::ProviderRotateRequest {
        agent: &state.agent_name, name: &provider_name, credential: &body.credential,
    };
    match state.internal_client.provider_rotate(&req).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_rotate_failed"),
    }
}

pub(crate) async fn handle_remove(
    AxumPath((agent, provider_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) { return error.into_response(); }
    let req = right_mcp::internal_client::ProviderRemoveRequest {
        agent: &state.agent_name, name: &provider_name,
    };
    match state.internal_client.provider_remove(&req).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_remove_failed"),
    }
}

pub(crate) async fn handle_config_update(
    AxumPath((agent, provider_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) { return error.into_response(); }
    let raw: serde_json::Value = match parse_json_body::<serde_json::Value>(&body) {
        Ok(v) => v, Err(resp) => return resp,
    };
    let mut full = raw;
    full.as_object_mut().unwrap().insert("agent".into(), serde_json::Value::String(state.agent_name.clone()));
    full.as_object_mut().unwrap().insert("name".into(), serde_json::Value::String(provider_name));
    match state.internal_client.provider_config_update(&full).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_config_update_failed"),
    }
}
```

The helpers `authenticate_api`, `parse_json_body`, `json_error`, `internal_api_error_response` already live in `mcp.rs`; either re-export them from `mcp` (`pub(crate)` already) or move to a shared `dashboard/common.rs`.

- [ ] **Step 3: Compile + commit**

```bash
devenv shell -- cargo check -p bot
git add -A
git commit -m "feat(bot): dashboard provider routes + handlers"
```

---

### Task 25: `/providers` Telegram bot command

**Files:**
- Modify: wherever `/mcp` is registered (grep for `"/mcp"` in `crates/bot/src/`)

- [ ] **Step 1: Register command**

Find the command enum / dispatcher. Add a `Providers` variant alongside `Mcp`. Behavior identical to `/mcp` (open Mini App with `/dashboard/{agent}/providers` initial tab).

- [ ] **Step 2: Sandbox-mode guard**

In the handler:

```rust
let cfg = right_agent_config::load_for_agent(&home, &agent_name)?;
if cfg.sandbox_mode() != &right_agent_config::SandboxMode::Openshell {
    send_text(&bot, msg.chat.id, "Providers are only available for sandboxed agents. This agent runs in host mode.").await?;
    return Ok(());
}
```

- [ ] **Step 3: Compile + commit**

```bash
devenv shell -- cargo check -p bot
git add -A
git commit -m "feat(bot): /providers command opens Mini App with sandbox-mode guard"
```

---

### Task 26: SandboxModeNone rejection test

**Files:**
- Modify: `crates/right/src/internal_api.rs` (or its tests file)

- [ ] **Step 1: Test**

```rust
#[tokio::test]
async fn provider_list_rejects_sandbox_mode_none() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    // Seed an agent with sandbox.mode = none.
    let agent_dir = home.join("agents").join("hostagent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("agent.yaml"),
        "sandbox:\n  mode: none\n").unwrap();
    let state = InternalApiState::for_test(home.into());
    let app = axum::Router::new()
        .route("/provider-list", axum::routing::post(handle_provider_list))
        .with_state(state);
    let response = run_request(app, "POST", "/provider-list",
        serde_json::json!({"agent": "hostagent"})).await;
    assert_eq!(response.status, 400);
    let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(body["code"], "sandbox_mode_none");
}
```

Adapt `InternalApiState::for_test` and `run_request` to match the existing mcp_list test pattern in the same file.

- [ ] **Step 2: Run + commit**

```bash
devenv shell -- cargo test -p right provider_list_rejects_sandbox_mode_none
git add -A
git commit -m "test(internal-api): sandbox_mode_none rejection for /provider-list"
```

---

## Phase 8 — Frontend (Vue + Vite)

### Task 27: Register 'providers' tab

**Files:**
- Modify: `crates/right-dashboard/frontend/src/dashboardTabs.ts`

- [ ] **Step 1: Add to tabs list**

Edit:

```ts
export const dashboardTabs = ['overview', 'activity', 'knowledge', 'usage', 'identity', 'health', 'mcp', 'providers'] as const

// in dashboardTabItems:
    { key: 'providers', label: 'Providers', enabled: features?.providers ?? true },
```

Update `DashboardFeatures` type in `types.ts` to add `providers?: boolean`.

- [ ] **Step 2: Vitest expectations**

If `App.test.ts` enumerates tabs, update its expected list.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(dashboard-fe): register Providers tab"
```

---

### Task 28: ProvidersView.vue + viewmodel

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/ProvidersView.vue`
- Create: `crates/right-dashboard/frontend/src/views/providersViewModel.ts`
- Modify: `crates/right-dashboard/frontend/src/App.vue` (route the new tab)

- [ ] **Step 1: API binding**

In `crates/right-dashboard/frontend/src/api.ts` add:

```ts
export async function providerList(agent: string): Promise<ProviderView[]> {
    const resp = await apiFetch(`/api/v1/providers`, agent, { method: 'GET' })
    return (resp.providers as ProviderView[]) ?? []
}
export async function providerTypes(agent: string): Promise<ProviderProfileView[]> {
    const resp = await apiFetch(`/api/v1/providers/types`, agent, { method: 'GET' })
    return (resp.types as ProviderProfileView[]) ?? []
}
export async function providerCreate(agent: string, body: ProviderCreateBody): Promise<ProviderView> {
    return apiFetch(`/api/v1/providers`, agent, { method: 'POST', body: JSON.stringify(body) })
}
export async function providerRotate(agent: string, name: string, credential: string): Promise<ProviderView> {
    return apiFetch(`/api/v1/providers/${encodeURIComponent(name)}/rotate`, agent, {
        method: 'POST', body: JSON.stringify({ credential }),
    })
}
export async function providerConfigUpdate(agent: string, name: string, body: Partial<ProviderGenericBody>): Promise<ProviderView> {
    return apiFetch(`/api/v1/providers/${encodeURIComponent(name)}/config`, agent, {
        method: 'PATCH', body: JSON.stringify({ generic: body }),
    })
}
export async function providerRemove(agent: string, name: string): Promise<void> {
    await apiFetch(`/api/v1/providers/${encodeURIComponent(name)}`, agent, { method: 'DELETE' })
}
```

Add corresponding types to `types.ts`:

```ts
export interface ProviderView {
    name: string; type: string; label: string | null; env_var: string;
    generic: ProviderGenericBody | null; updated_at: string | null;
    status: { kind: 'healthy' } | { kind: 'missing' } | { kind: 'gateway_error', message: string };
}
export interface ProviderProfileView { type: string; env_var: string; display_name: string; category: string }
export interface ProviderGenericBody {
    env_var: string; header_name?: string; upstream_host: string; upstream_path_prefix?: string;
}
export interface ProviderCreateBody {
    type: string; label?: string; credential: string; generic?: ProviderGenericBody;
}
```

- [ ] **Step 2: ProvidersView.vue skeleton**

```vue
<script setup lang="ts">
import { onMounted, ref, computed } from 'vue'
import { providerList, providerTypes, providerCreate, providerRotate, providerRemove, providerConfigUpdate } from '../api'
import type { ProviderView, ProviderProfileView, ProviderCreateBody, ProviderGenericBody } from '../types'
import SecretInput from '../components/SecretInput.vue'

const props = defineProps<{ agent: string }>()
const providers = ref<ProviderView[]>([])
const types = ref<ProviderProfileView[]>([])
const loading = ref(false)
const error = ref<string | null>(null)
const addOpen = ref(false)

async function refresh() {
    loading.value = true; error.value = null
    try {
        const [list, profiles] = await Promise.all([
            providerList(props.agent), providerTypes(props.agent),
        ])
        providers.value = list; types.value = profiles
    } catch (e: any) {
        error.value = String(e?.message ?? e)
    } finally { loading.value = false }
}
onMounted(refresh)
</script>

<template>
  <section class="providers-view">
    <header>
      <h2>Providers</h2>
      <button @click="addOpen = true">+ Add</button>
    </header>
    <p v-if="error" class="banner banner-error">{{ error }}</p>
    <p v-if="loading">Loading…</p>
    <ul v-else>
      <li v-for="p in providers" :key="p.name" :class="p.status.kind">
        <span class="type">{{ p.type }}</span>
        <span class="name">{{ p.name }}</span>
        <span class="env">{{ p.env_var }}</span>
        <span class="status" v-if="p.status.kind !== 'healthy'">{{ p.status.kind }}</span>
        <button @click="rotate(p)">Rotate</button>
        <button @click="remove(p)">Delete</button>
      </li>
    </ul>
    <!-- Add modal goes here (Task 29) -->
  </section>
</template>
```

Wire `rotate` and `remove` to API calls + `refresh()`. Use existing component patterns from `McpView.vue`.

- [ ] **Step 3: Register in App.vue**

Mirror the existing `<McpView />` conditional rendering: `<ProvidersView v-if="activeTab === 'providers'" :agent="agent" />`.

- [ ] **Step 4: Compile + commit**

```bash
cd crates/right-dashboard/frontend && npm run typecheck && cd ../../..
git add -A
git commit -m "feat(dashboard-fe): ProvidersView + API bindings"
```

---

### Task 29: Add flow — built-in and generic

**Files:**
- Modify: `ProvidersView.vue` (extend the modal section)
- Modify: `providersViewModel.ts` (add validation helpers)

- [ ] **Step 1: View-model validators**

```ts
export function validateSlug(s: string): string | null {
    if (!s) return 'required'
    if (s.length > 32) return 'too long'
    if (!/^[a-z][a-z0-9-]*$/.test(s)) return 'must match [a-z][a-z0-9-]*'
    return null
}
export function validateEnvVar(s: string): string | null {
    if (!s) return 'required'
    if (s.length > 64) return 'too long'
    if (!/^[A-Z_][A-Z0-9_]*$/.test(s)) return 'must match [A-Z_][A-Z0-9_]*'
    return null
}
```

- [ ] **Step 2: Modal markup**

In `ProvidersView.vue`, add:

```vue
<dialog v-if="addOpen" open>
  <div v-if="!selectedType">
    <h3>Choose provider type</h3>
    <button v-for="t in types" :key="t.type" @click="selectedType = t.type">
      <strong>{{ t.display_name }}</strong>
      <small>{{ t.env_var || '— custom env var —' }}</small>
    </button>
  </div>
  <form v-else @submit.prevent="submitAdd">
    <h3>{{ typeDisplayName(selectedType) }}</h3>
    <label v-if="selectedType === 'generic'">
      Label
      <input v-model="form.label" required />
    </label>
    <label v-else>
      Label (optional)
      <input v-model="form.label" />
    </label>
    <label v-if="selectedType === 'generic'">
      Env var
      <input v-model="form.envVar" required />
    </label>
    <label>
      Credential
      <SecretInput v-model="form.credential" />
    </label>
    <template v-if="selectedType === 'generic'">
      <label>Upstream host <input v-model="form.upstreamHost" required /></label>
      <label>Header name <input v-model="form.headerName" placeholder="Authorization" /></label>
      <label>Upstream path prefix <input v-model="form.upstreamPathPrefix" /></label>
    </template>
    <button type="submit">Create</button>
    <button type="button" @click="cancelAdd">Cancel</button>
  </form>
</dialog>
```

- [ ] **Step 3: Submit handler**

```ts
async function submitAdd() {
  const body: ProviderCreateBody = { type: selectedType.value!, credential: form.value.credential, label: form.value.label || undefined }
  if (selectedType.value === 'generic') {
    body.generic = {
      env_var: form.value.envVar,
      header_name: form.value.headerName || undefined,
      upstream_host: form.value.upstreamHost,
      upstream_path_prefix: form.value.upstreamPathPrefix || undefined,
    }
  }
  await providerCreate(props.agent, body)
  cancelAdd()
  await refresh()
}
```

- [ ] **Step 4: Compile + commit**

```bash
cd crates/right-dashboard/frontend && npm run typecheck && cd ../../..
git add -A
git commit -m "feat(dashboard-fe): provider Add flow (built-in + generic)"
```

---

### Task 30: Rotate / Edit config / Delete actions

**Files:**
- Modify: `ProvidersView.vue`

- [ ] **Step 1: Rotate modal**

Single-field password input + Save button. Submits via `providerRotate(agent, name, value)`.

- [ ] **Step 2: Edit-config modal (generic only)**

Mirror the generic Add form but populate from current entry; submit via `providerConfigUpdate`.

- [ ] **Step 3: Delete confirmation**

Native `confirm()` is acceptable for v1. Submit via `providerRemove`.

- [ ] **Step 4: Compile + commit**

```bash
cd crates/right-dashboard/frontend && npm run typecheck && cd ../../..
git add -A
git commit -m "feat(dashboard-fe): provider rotate/edit/delete actions"
```

---

### Task 31: Ghost-provider Resolve UX

**Files:**
- Modify: `ProvidersView.vue`

- [ ] **Step 1: Conditional rendering**

For rows where `p.status.kind === 'missing'`, style the row muted and offer two buttons: *Re-create* (opens the Add modal pre-filled with the existing label/type/generic fields, asks for a new credential, calls `providerCreate` — backend will collide on name unless we change it to use `providerRemove` then `providerCreate`; for simplicity, do both: `providerRemove` first, then `providerCreate`), and *Remove from agent.yaml* which calls `providerRemove` only.

- [ ] **Step 2: Commit**

```bash
cd crates/right-dashboard/frontend && npm run typecheck && cd ../../..
git add -A
git commit -m "feat(dashboard-fe): ghost-provider Resolve action"
```

---

## Phase 9 — Lifecycle wiring

### Task 32: right up calls ensure_v2_enabled

**Files:**
- Modify: `crates/right/src/main.rs` cmd_up (find via `grep -n "fn cmd_up" crates/right/src/main.rs`)

- [ ] **Step 1: After PC starts, before agents launch**

Insert:

```rust
let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await
    .map_err(|e| miette::miette!("resolve openshell gateway: {e:#}"))?;
let any_provider = agents.iter().any(|a| {
    a.config.sandbox.as_ref()
        .map(|s| !s.providers.is_empty()).unwrap_or(false)
});
match right_openshell::providers::ensure_v2_enabled(&endpoint).await {
    Ok(r) => tracing::info!(?r, "providers_v2_enabled OK"),
    Err(e) if any_provider => {
        miette::bail!("providers_v2_enabled could not be set: {e:#}. At least one agent uses providers; refusing to start.");
    }
    Err(e) => {
        tracing::warn!("providers_v2_enabled could not be set, but no agent uses providers — continuing: {e:#}");
    }
}
```

Adapt to the actual `agents` collection shape in `cmd_up`.

- [ ] **Step 2: Compile + commit**

```bash
devenv shell -- cargo check -p right
git add -A
git commit -m "feat(right-up): ensure_v2_enabled at startup with conditional fatal"
```

---

### Task 33: Startup reconciler per agent

**Files:**
- Modify: bot startup path or wherever per-agent post-sandbox-ready actions live

- [ ] **Step 1: Find the right hook**

Run: `grep -rn "wait_for_ready\|sandbox.*ready\|initial_sync" crates/bot/src/ crates/right-agent/src/ | head`. The reconciler runs **after** `wait_for_ready` and **before** the bot serves messages. Likely in `crates/bot/src/main.rs` or `crates/right-agent/src/init.rs`.

- [ ] **Step 2: Reconcile function**

In `crates/right-openshell/src/providers.rs`:

```rust
pub struct ReconcileReport {
    pub attached: Vec<String>,
    pub detached: Vec<String>,
    pub missing: Vec<String>,
}

pub async fn reconcile_for_sandbox(
    endpoint: &crate::openshell::GatewayEndpoint,
    sandbox_name: &str,
    agent_prefix: &str,
    declared: &[String],
) -> Result<ReconcileReport, ProviderError> {
    let attached = list_attached(endpoint, sandbox_name).await?;
    let declared_set: std::collections::HashSet<_> = declared.iter().collect();
    let attached_set: std::collections::HashSet<_> = attached.iter().collect();
    let mut report = ReconcileReport { attached: vec![], detached: vec![], missing: vec![] };
    for name in declared {
        match get_provider(endpoint, name).await {
            Ok(_) => {
                if !attached_set.contains(name) {
                    attach_to_sandbox(endpoint, sandbox_name, name).await?;
                    report.attached.push(name.clone());
                }
            }
            Err(ProviderError::NotFound(_)) => report.missing.push(name.clone()),
            Err(e) => return Err(e),
        }
    }
    for name in &attached {
        if name.starts_with(&format!("{agent_prefix}-")) && !declared_set.contains(name) {
            detach_from_sandbox(endpoint, sandbox_name, name).await?;
            report.detached.push(name.clone());
        }
    }
    Ok(report)
}
```

- [ ] **Step 3: Call from agent startup**

At the chosen call site:

```rust
let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await?;
let declared: Vec<String> = sandbox.providers.iter().map(|p| p.name.clone()).collect();
let report = right_openshell::providers::reconcile_for_sandbox(
    &endpoint, sandbox_name, &agent_name, &declared
).await?;
tracing::info!(?report, "provider reconcile complete");
```

- [ ] **Step 4: Commit**

```bash
devenv shell -- cargo check
git add -A
git commit -m "feat(providers): startup reconciler per agent"
```

---

### Task 34: right agent destroy cascade

**Files:**
- Modify: `crates/right-agent/src/agent/destroy.rs` (or wherever destroy logic lives)

- [ ] **Step 1: Cascade delete**

Before tearing down the sandbox, in the destroy flow:

```rust
if let Some(sandbox) = &cfg.sandbox
    && sandbox.mode == right_agent_config::SandboxMode::Openshell
{
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await?;
    for entry in &sandbox.providers {
        if let Err(e) = right_openshell::providers::delete_provider(&endpoint, &entry.name).await {
            tracing::warn!(name = %entry.name, error = ?e, "failed to delete provider during destroy; continuing");
        }
    }
}
```

Failure to delete is logged but not fatal — destroy should still tear down the sandbox. The reconciler on a future `right up` would normally clean up, but here the agent is being destroyed entirely.

- [ ] **Step 2: Compile + commit**

```bash
devenv shell -- cargo check -p right-agent
git add -A
git commit -m "feat(agent-destroy): cascade-delete provider entries from gateway"
```

---

### Task 35: Backup/restore field travel test

**Files:**
- Modify: existing backup/restore test file (`grep -rn "fn.*backup\|test_backup" crates/right-agent/src/`)

- [ ] **Step 1: Add test**

A non-OpenShell unit test asserting that an `agent.yaml` containing `sandbox.providers: [...]` survives a round-trip through `right agent backup` -> `right agent init --from-backup` (or whatever the in-process equivalent is) with the providers list intact. Don't touch the gateway side — that's tested by reconciler tests separately.

- [ ] **Step 2: Run + commit**

```bash
devenv shell -- cargo test -p right-agent backup
git add -A
git commit -m "test(agent-backup): providers field survives backup/restore"
```

---

## Phase 10 — Docs

### Task 36: ARCHITECTURE.md updates

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Three additions**

Insert under "OpenShell Integration Conventions":

> All Provider operations (Create/Get/Update/Delete/ListProviders, sandbox attach/detach, GetSandboxProviderEnvironment, ensure_v2_enabled) MUST go through `right_openshell::providers`. Direct gRPC or `openshell provider` CLI invocations from other crates are a review-blocking defect.

Insert under "Conventions" → "Bot-first management":

> Provider management goes through the Telegram Mini App dashboard opened by `/providers`. Never create or edit gateway providers via host CLI or agent.yaml directly — the bot/dashboard is the control plane.

Insert under "Security Model":

> Provider credential values and gateway placeholder values (`openshell:resolve:env:v…_<NAME>`) are never logged. Use `secrecy::SecretString` for in-memory transport; do not pass credential fields to tracing macros.

Add reference under "Data Flow" or near "Configuration Hierarchy":

> See `docs/architecture/providers.md` for the placeholder mechanism, the substitution flow, and the reconciler walkthrough.

- [ ] **Step 2: Verify the 40k character budget**

Run: `wc -c ARCHITECTURE.md`. Expected: still well under 40000. If close, move some non-prescriptive content out.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs(arch): provider gRPC ownership, mgmt surface, credential logging rules"
```

---

### Task 37: docs/architecture/providers.md satellite

**Files:**
- Create: `docs/architecture/providers.md`

- [ ] **Step 1: Write descriptive doc**

Create `docs/architecture/providers.md` with this exact content:

```markdown
# Providers

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Overview

Providers are typed credential bundles stored on the NVIDIA OpenShell
gateway and attached to sandboxed agents. Each provider has a
gateway-unique name, a type slug (`anthropic`, `openai`, `github`,
`gitlab`, `nvidia`, `codex`, `copilot`, `opencode`, or `generic`), a
credentials map, and an optional non-secret config map. Right Agent
exposes provider management exclusively through the Telegram Mini App
dashboard route `/providers`; credentials never enter `agent.yaml`,
backups, or logs on the host.

The feature is sandbox-only. `sandbox.mode = none` agents cannot
receive provider env vars; the bot rejects `/providers` for them.

## Placeholder substitution

At sandbox boot, the OpenShell supervisor calls
`GetSandboxProviderEnvironment` and injects the result as environment
variables on the sandbox supervisor process. The values are opaque
placeholders shaped like `openshell:resolve:env:v<digits>_<NAME>`. Every
process spawned inside the sandbox — including `claude -p` over gRPC
exec and SSH — inherits these env vars at the kernel level.

When the agent makes an HTTPS request through the gateway proxy
(`HTTPS_PROXY=10.200.0.1:3128`, injected at sandbox boot), the proxy
substitutes the placeholder with the real credential before forwarding
upstream. Substitution happens after TLS termination, so the policy
endpoint must use `protocol: rest` (auto-TLS-terminate) or explicit
`tls: terminate`. If the placeholder is sent to a raw-tunnel endpoint
(`tls: skip`), the proxy cannot resolve it and rejects the request with
HTTP 500 — never forwarding the raw placeholder.

## State of truth split

Two stores, both authoritative for different things:

| What                              | Where                                      |
| --------------------------------- | ------------------------------------------ |
| Per-agent list of attached names  | `agent.yaml::sandbox::providers: [...]`    |
| Credential bytes                  | OpenShell gateway (write-once via Right)   |
| Non-secret provider config        | OpenShell gateway                          |
| Sandbox attachment state          | OpenShell gateway (`Sandbox.providers`)    |

`agent.yaml` wins on drift: the reconciler attaches anything in the
file that isn't currently attached, and detaches any extra
`<agent>-*` providers attached to the sandbox but missing from the
file.

## Reconciler walkthrough

Runs at `right up`, after the sandbox is READY and before the bot
starts serving messages.

For each entry in `agent.yaml::sandbox::providers`:

1. `GetProvider` against the gateway.
   - **Ok** → continue.
   - **NotFound** → mark the entry as `Status::Missing` (a "ghost"
     provider). Do not auto-heal: Right does not have the credential
     bytes. The dashboard surfaces these with a *Resolve* action.
2. If not currently attached to the sandbox, call
   `Sandbox.provider.attach`.

Then for each provider currently attached to the sandbox whose name
starts with `<agent>-` but is absent from `agent.yaml`: call
`Sandbox.provider.detach`.

The reconciler returns a `ReconcileReport { attached, detached,
missing }` per agent which is surfaced to the dashboard.

## Policy interaction

Two distinct paths into `policy.yaml`:

**Path A — built-in providers.** Right does not mutate `policy.yaml`.
With `providers_v2_enabled=true`, the OpenShell gateway contributes the
profile's endpoints to the effective sandbox policy automatically when
a provider is attached. Right's `policy.yaml` stays unchanged.

**Path B — generic providers.** Right owns the `policy.yaml` mutation.
On create or upstream-host change:

1. Load current `policy.yaml`.
2. Look for an existing `endpoints[]` entry matching `upstream_host`.
   - Absent → append a new stanza: `domain: <host>`, `protocol: rest`,
     `access: full`, optional `path: <prefix>`. Tag with a YAML comment
     `# managed-by: right-providers:<provider-name>` so future strip
     operations can find it.
   - Present with `protocol: rest` → no-op.
   - Present with `tls: skip` → refuse the operation with
     `PolicyConflict { kind: "raw-tunnel" }`. Right does not
     auto-rewrite; the user must resolve the conflict.
   - Present with `tls: terminate` (deprecated but functional) →
     no-op.
3. Write `policy.yaml`.
4. Hot-apply via `openshell policy set --wait`. This is the
   `Regenerated(SandboxPolicyApply)` codegen category — **never**
   `SandboxRecreate`. New endpoints are hot-reloadable.

On remove: if no other generic provider on the same agent uses the
same `upstream_host`, strip the tagged stanza and hot-apply. The strip
is idempotent — if the tag is absent, the policy is returned
unchanged.

## Lifecycle

**Create.** Generic providers run: write policy.yaml (with snapshot)
→ hot-apply → `CreateProvider` → `Sandbox.provider.attach` → write
`agent.yaml`. Built-in providers skip the policy steps. Any failure
triggers ordered rollback: a failed `attach` removes the freshly
created provider; a failed `agent.yaml` write triggers best-effort
detach + delete; a failed policy hot-apply restores the snapshotted
policy.

**Rotate.** `UpdateProvider` only. No sandbox restart. The gateway
issues a new placeholder version; the next outbound request from the
sandbox carries the new placeholder and resolves to the new
credential.

**Edit non-secret config.** Generic providers only. If
`upstream_host` changed, strip the old stanza and append the new one
(with snapshot). Then `UpdateProvider`. Then write `agent.yaml`.

**Remove.** `Sandbox.provider.detach` → `DeleteProvider` → if generic
and no other provider on the agent uses the same host, strip the
policy stanza and hot-apply. Then write `agent.yaml`.

**Ghost (post-restore).** When `agent.yaml` lists a provider that the
gateway doesn't have (typical after backup/restore to a new host),
the reconciler marks the row `Status::Missing`. The dashboard's
*Resolve* action either re-creates the provider with a fresh
credential or strips the entry from `agent.yaml`.

**Cascade on `right agent destroy`.** Before tearing down the sandbox,
Right iterates `agent.yaml::sandbox::providers` and calls
`DeleteProvider` on each. Failures are logged and skipped so destroy
proceeds; orphans clean up on the next `right up`.
```

Keep this strictly descriptive — rules and review-blocking
prescriptions belong in ARCHITECTURE.md (Task 36).

- [ ] **Step 2: Commit**

```bash
git add docs/architecture/providers.md
git commit -m "docs(arch): satellite providers.md (placeholder mechanism, lifecycle, policy)"
```

---

### Task 38: Cite-on-touch updates

**Files:**
- Modify: `docs/architecture/sandbox.md` (Right MCP host-access section already mentions policy; add a line about provider-managed endpoints)
- Modify: `docs/architecture/mcp.md` if dashboard MCP routing section needs cross-reference

- [ ] **Step 1: sandbox.md addendum**

After the OpenShell sandbox architecture diagram, add a paragraph:

> Sandbox supervisors inject provider env-var placeholders at boot from the gateway's
> attached-providers list (see `docs/architecture/providers.md`). The gateway proxy
> substitutes the real credential on outbound HTTPS for TLS-terminated endpoints; raw
> tunnels (tls: skip) cannot substitute and Right refuses to attach generic providers
> against those hosts.

- [ ] **Step 2: Remove PROVIDER_NOTES.md scratch file**

```bash
git rm crates/right-openshell/PROVIDER_NOTES.md
```

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/sandbox.md
git commit -m "docs(arch): cite-on-touch — sandbox provider env interaction"
```

---

## Phase 11 — Final verification

### Task 39: Full workspace test

- [ ] **Step 1: Run the suite**

Run: `devenv shell -- cargo test --workspace`
Expected: all non-ignored tests pass. Record any pre-existing failures separately from anything this plan introduced.

- [ ] **Step 2: Run live OpenShell tests (CI gate)**

Run: `devenv shell -- cargo test --workspace -- --ignored ci_openshell_provider_`
Expected: all 7 `ci_openshell_provider_*` tests pass against a live gateway. If running locally, skip this step — the CI workflow in `.github/workflows/tests.yml` covers it.

- [ ] **Step 3: Run clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets`
Expected: no new warnings introduced by this plan.

- [ ] **Step 4: Final review of ARCHITECTURE.md size**

Run: `wc -c ARCHITECTURE.md`
Expected: < 40000 bytes (hard budget per AGENTS.md).

- [ ] **Step 5: Final commit (if any merging cleanup needed)**

```bash
git status
# If clean, no further commits needed.
```

---

## Acceptance checklist

- [ ] Built-in provider can be created, rotated, deleted from the dashboard, and the env var becomes visible inside the sandbox.
- [ ] Generic provider creation appends a TLS-terminated endpoint to policy.yaml and hot-applies without recreating the sandbox.
- [ ] Generic provider creation against a raw-tunnel host is refused with `PolicyConflict`.
- [ ] `claude` as a type slug is rejected at the API boundary.
- [ ] `sandbox.mode = none` agents see *"Providers are only available for sandboxed agents"* and the dashboard route is not exposed.
- [ ] `providers_v2_enabled` is set at `right up` (idempotently).
- [ ] Existing agents without `providers` in agent.yaml continue to work.
- [ ] `right agent destroy` removes all `<agent>-*` providers from the gateway.
- [ ] Live OpenShell CI tests prefixed `ci_openshell_provider_` are green.
- [ ] `cargo test --workspace` passes.
- [ ] `ARCHITECTURE.md` stays under 40000 chars.
