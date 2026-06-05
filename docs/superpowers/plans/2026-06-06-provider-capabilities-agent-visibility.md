# Provider-capabilities visibility for the agent — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give a sandboxed agent a read-only MCP tool (`mcp__right__provider_capabilities`) plus a thin prompt pointer so it learns which binaries can spend which provider credential on which hosts, ending the 401→"bad credential" misdiagnosis.

**Architecture:** A pure correlation function in `right-openshell` joins three live gRPC reads — the effective sandbox policy (the `_provider_<name>` rules that actually govern requests), the sandbox's injected placeholder env vars, and each attached provider's profile — into a `Vec<ProviderCapability>`. A new built-in `RightBackend` MCP tool resolves the agent's own sandbox (no agent-supplied scope) and returns that data as JSON. `OPERATING_INSTRUCTIONS.md` gains a concept + trigger sentence.

**Tech Stack:** Rust (edition 2024), tonic gRPC (`openshell.v1`/`openshell.sandbox.v1`), rmcp MCP server, `anyhow`/`miette`/`thiserror`.

---

## Background facts (read before starting)

- **Root cause confirmed:** the built-in `right-github` profile scopes credential substitution to `binaries` `gh`/`git`. A request to `api.github.com` from any other binary (`curl`, `node` fetch, python) falls through to the permissive `outbound` raw tunnel where no substitution happens — the literal placeholder is sent and GitHub returns `401 Bad credentials`. `gh api user` and `git ls-remote` work; the credential is valid. The fix is **agent knowledge**, not widening the allowlist. Full diagnosis in `docs/superpowers/specs/2026-06-06-provider-capabilities-agent-visibility-design.md`.
- **The `forum_topic_list` tool is the exact precedent** for a read-only, no-argument, server-scoped built-in tool. Copy its shape.
- **Proto types** live at `right_openshell::openshell_proto::openshell::sandbox::v1::{SandboxPolicy, NetworkPolicyRule, NetworkEndpoint, NetworkBinary}`. `SandboxPolicy.network_policies` is `HashMap<String, NetworkPolicyRule>`. `NetworkPolicyRule` has `name: String`, `endpoints: Vec<NetworkEndpoint>`, `binaries: Vec<NetworkBinary>`. `NetworkEndpoint` has `host: String`, `port: u32`, `protocol: String`. `NetworkBinary` has `path: String`.
- **Rule naming:** provider `right-right-github` composes a policy rule keyed `_provider_right_right_github` (prefix `_provider_`, then the provider name with non-alphanumerics → `_`).
- **Provider `type_` IS the profile id:** providers are created with `ProviderSpec.type_ = <profile id>`. `right-right-github` has `type_ = "right-github"`; a generic provider has `type_ = right-provider-<slug>-<hash>`. So `managed_profiles::get_profile(client, &provider.type_)` returns its profile.

### Verification baseline (run once at start)

- [ ] **Baseline build/test** of the two crates this plan touches.

Run:
```bash
devenv shell -- cargo test -p right-openshell --lib
devenv shell -- cargo build -p right
```
Expected: both succeed. Record any pre-existing failures before making changes.

---

## File Structure

- **Create:** `crates/right-openshell/src/provider_capabilities.rs` — the `ProviderCapability` / `ProviderCapabilityInput` types, the pure `correlate_provider_capabilities` function, and the async `provider_capabilities_for_sandbox` gatherer. One focused responsibility: turn live gateway state into agent-facing capability records.
- **Create:** `crates/right-openshell/src/provider_capabilities_tests.rs` — unit tests for the pure function.
- **Modify:** `crates/right-openshell/src/lib.rs` — add `pub mod provider_capabilities;`.
- **Modify:** `crates/right/src/right_backend.rs` — param struct, `tools_list` entry, `tools_call` arm, `call_provider_capabilities` handler.
- **Modify:** `crates/right/src/aggregator.rs` — `with_instructions()` text + its tool-name test.
- **Modify:** `crates/right/src/memory_server.rs` — `with_instructions()` text (stdio caveat) + its tool-name test.
- **Modify:** `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` — extend the existing "## Credentials & API Keys" section.
- **Modify:** `crates/right-openshell/tests/ci_openshell_generic_provider.rs` — one live `ci_openshell_` integration test reusing the existing harness.
- **Modify:** `PROMPT_SYSTEM.md` — sync the prompt/tool description.

---

## Task 1: Pure correlation logic + types in `right-openshell`

**Files:**
- Create: `crates/right-openshell/src/provider_capabilities.rs`
- Create: `crates/right-openshell/src/provider_capabilities_tests.rs`
- Modify: `crates/right-openshell/src/lib.rs`

- [ ] **Step 1: Register the module**

In `crates/right-openshell/src/lib.rs`, add alongside the other `pub mod` declarations (e.g. near `pub mod providers;`):

```rust
pub mod provider_capabilities;
```

- [ ] **Step 2: Write the module with types + pure function (test wiring at bottom)**

Create `crates/right-openshell/src/provider_capabilities.rs`:

```rust
//! Agent-facing provider capability records.
//!
//! Joins the live effective sandbox policy, the sandbox's injected placeholder
//! env vars, and each attached provider's profile into a description the agent
//! can read to learn *which binary* can spend *which credential* on *which
//! host*. Read-only; never returns credential or placeholder values.

use std::collections::HashSet;

use crate::openshell_proto::openshell::sandbox::v1::SandboxPolicy;

/// One attached provider's identity + candidate env vars, gathered from the
/// gateway before correlation. Pure-function input so the join logic is
/// testable without gRPC.
#[derive(Debug, Clone)]
pub struct ProviderCapabilityInput {
    /// Gateway provider name, e.g. `right-right-github`.
    pub name: String,
    /// User-friendly name from the profile, e.g. `GitHub`.
    pub display_name: String,
    /// Env vars the profile declares for this credential (candidates).
    pub candidate_env_vars: Vec<String>,
}

/// Agent-facing capability record for one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapability {
    pub display_name: String,
    /// Env vars actually injected into this sandbox for the provider (the
    /// intersection of profile candidates and the sandbox's real placeholder
    /// env). Names only — never values.
    pub env_vars: Vec<String>,
    /// Binary paths allowed to use the credential (from the effective policy).
    pub allowed_binaries: Vec<String>,
    /// Hosts the credential is valid for (from the effective policy).
    pub endpoint_hosts: Vec<String>,
    /// One-line, agent-readable usage guidance.
    pub usage_hint: String,
}

/// `_provider_` prefix OpenShell uses for composed provider rules.
const PROVIDER_RULE_PREFIX: &str = "_provider_";

/// Lowercase + map every non-alphanumeric byte to `_`, matching how OpenShell
/// derives a `_provider_<sanitized-name>` rule key from a provider name.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Find the composed `_provider_<name>` rule for a provider, tolerant of
/// dash/underscore differences in the gateway's key derivation.
fn rule_for_provider<'a>(
    policy: &'a SandboxPolicy,
    provider_name: &str,
) -> Option<&'a crate::openshell_proto::openshell::sandbox::v1::NetworkPolicyRule> {
    let want = sanitize(provider_name);
    policy.network_policies.iter().find_map(|(key, rule)| {
        let stripped = key.strip_prefix(PROVIDER_RULE_PREFIX)?;
        (sanitize(stripped) == want).then_some(rule)
    })
}

/// Strip a binary path to its basename for the usage hint (`/usr/bin/gh` → `gh`).
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn build_usage_hint(allowed_binaries: &[String], hosts: &[String], active: bool) -> String {
    if !active {
        return "Attached but not currently active in the sandbox policy \
                (composition may be reloading). While inactive the credential is \
                not injected; requests will 401."
            .to_string();
    }
    let hosts_list = hosts.join(", ");
    if allowed_binaries.iter().any(|b| b == "**") {
        return format!(
            "Any binary may use this credential on {hosts_list}; the gateway \
             injects it automatically on those requests. Do not paste the \
             placeholder env var elsewhere."
        );
    }
    let mut bins: Vec<&str> = allowed_binaries.iter().map(|b| basename(b)).collect();
    bins.sort_unstable();
    bins.dedup();
    format!(
        "Reach {hosts_list} only via {}; the gateway injects the credential \
         automatically on those requests. Other clients (curl/fetch/python) get \
         no substitution and will 401 — do not paste the placeholder env var into them.",
        bins.join(", ")
    )
}

/// Join provider inputs with the effective policy and the sandbox's injected
/// env keys. Pure — no IO. Sorted deterministically for stable output/tests.
pub fn correlate_provider_capabilities(
    inputs: &[ProviderCapabilityInput],
    policy: &SandboxPolicy,
    sandbox_env_keys: &HashSet<String>,
) -> Vec<ProviderCapability> {
    let mut out: Vec<ProviderCapability> = inputs
        .iter()
        .map(|input| {
            let rule = rule_for_provider(policy, &input.name);
            let active = rule.is_some();

            let mut allowed_binaries: Vec<String> = rule
                .map(|r| r.binaries.iter().map(|b| b.path.clone()).collect())
                .unwrap_or_default();
            allowed_binaries.sort_unstable();
            allowed_binaries.dedup();

            let mut endpoint_hosts: Vec<String> = rule
                .map(|r| r.endpoints.iter().map(|e| e.host.clone()).collect())
                .unwrap_or_default();
            endpoint_hosts.sort_unstable();
            endpoint_hosts.dedup();

            let mut env_vars: Vec<String> = input
                .candidate_env_vars
                .iter()
                .filter(|v| sandbox_env_keys.contains(*v))
                .cloned()
                .collect();
            env_vars.sort_unstable();
            env_vars.dedup();

            let usage_hint = build_usage_hint(&allowed_binaries, &endpoint_hosts, active);

            ProviderCapability {
                display_name: input.display_name.clone(),
                env_vars,
                allowed_binaries,
                endpoint_hosts,
                usage_hint,
            }
        })
        .collect();
    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    out
}

#[cfg(test)]
#[path = "provider_capabilities_tests.rs"]
mod tests;
```

> Note: the async `provider_capabilities_for_sandbox` gatherer is added in Task 2 — this step is the pure logic only so it can be unit-tested in isolation.

- [ ] **Step 3: Write the failing unit tests**

Create `crates/right-openshell/src/provider_capabilities_tests.rs`:

```rust
use std::collections::HashSet;

use super::*;
use crate::openshell_proto::openshell::sandbox::v1::{
    NetworkBinary, NetworkEndpoint, NetworkPolicyRule, SandboxPolicy,
};

fn endpoint(host: &str) -> NetworkEndpoint {
    NetworkEndpoint {
        host: host.into(),
        port: 443,
        protocol: "rest".into(),
        ..Default::default()
    }
}

fn binary(path: &str) -> NetworkBinary {
    NetworkBinary {
        path: path.into(),
        ..Default::default()
    }
}

fn policy_with(rule_key: &str, bins: &[&str], hosts: &[&str]) -> SandboxPolicy {
    let rule = NetworkPolicyRule {
        name: rule_key.into(),
        endpoints: hosts.iter().map(|h| endpoint(h)).collect(),
        binaries: bins.iter().map(|b| binary(b)).collect(),
    };
    let mut policy = SandboxPolicy::default();
    policy.network_policies.insert(rule_key.into(), rule);
    policy
}

fn env_keys(keys: &[&str]) -> HashSet<String> {
    keys.iter().map(|s| s.to_string()).collect()
}

#[test]
fn matched_provider_reports_binaries_hosts_and_present_env() {
    let inputs = vec![ProviderCapabilityInput {
        name: "right-right-github".into(),
        display_name: "GitHub".into(),
        candidate_env_vars: vec!["GITHUB_TOKEN".into(), "GH_TOKEN".into()],
    }];
    let policy = policy_with(
        "_provider_right_right_github",
        &["/usr/bin/gh", "/usr/local/bin/gh", "/usr/bin/git"],
        &["api.github.com", "github.com", "api.github.com"],
    );
    // Only GITHUB_TOKEN is actually injected.
    let caps = correlate_provider_capabilities(&inputs, &policy, &env_keys(&["GITHUB_TOKEN"]));

    assert_eq!(caps.len(), 1);
    let c = &caps[0];
    assert_eq!(c.display_name, "GitHub");
    assert_eq!(c.env_vars, vec!["GITHUB_TOKEN".to_string()]); // GH_TOKEN absent → excluded
    assert_eq!(c.endpoint_hosts, vec!["api.github.com", "github.com"]); // deduped+sorted
    assert!(c.allowed_binaries.contains(&"/usr/bin/gh".to_string()));
    assert!(c.usage_hint.contains("gh"));
    assert!(c.usage_hint.contains("git"));
    assert!(c.usage_hint.contains("api.github.com"));
}

#[test]
fn wildcard_binary_yields_any_binary_hint() {
    let inputs = vec![ProviderCapabilityInput {
        name: "right-typefully".into(),
        display_name: "Typefully".into(),
        candidate_env_vars: vec!["TYPEFULLY_API_KEY".into()],
    }];
    let policy = policy_with("_provider_right_typefully", &["**"], &["api.typefully.com"]);
    let caps =
        correlate_provider_capabilities(&inputs, &policy, &env_keys(&["TYPEFULLY_API_KEY"]));

    assert_eq!(caps[0].env_vars, vec!["TYPEFULLY_API_KEY".to_string()]);
    assert_eq!(caps[0].allowed_binaries, vec!["**".to_string()]);
    assert!(caps[0].usage_hint.contains("Any binary"));
}

#[test]
fn attached_but_uncomposed_provider_is_inactive() {
    let inputs = vec![ProviderCapabilityInput {
        name: "right-orphan".into(),
        display_name: "Orphan".into(),
        candidate_env_vars: vec!["ORPHAN_KEY".into()],
    }];
    // Policy has no _provider_right_orphan rule.
    let policy = policy_with("_provider_something_else", &["**"], &["example.com"]);
    let caps = correlate_provider_capabilities(&inputs, &policy, &env_keys(&["ORPHAN_KEY"]));

    assert!(caps[0].allowed_binaries.is_empty());
    assert!(caps[0].endpoint_hosts.is_empty());
    assert!(caps[0].usage_hint.contains("not currently active"));
}
```

- [ ] **Step 4: Run the tests to verify they fail, then pass**

Run:
```bash
devenv shell -- cargo test -p right-openshell provider_capabilities
```
Expected: compiles, all three tests PASS. (If `SandboxPolicy::default()` / field names mismatch the generated proto, fix the test helpers to match the generated struct — do not change the proto.)

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/provider_capabilities.rs \
        crates/right-openshell/src/provider_capabilities_tests.rs \
        crates/right-openshell/src/lib.rs
git commit -m "feat(openshell): correlate provider capabilities from policy + env"
```

---

## Task 2: Async gatherer `provider_capabilities_for_sandbox`

**Files:**
- Modify: `crates/right-openshell/src/provider_capabilities.rs`

This is gRPC plumbing only — all logic lives in the pure function from Task 1, so it is exercised by the live test in Task 6 rather than a unit test.

- [ ] **Step 1: Add the gatherer to `provider_capabilities.rs`**

Append to `crates/right-openshell/src/provider_capabilities.rs` (above the `#[cfg(test)]` block):

```rust
use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;
use tonic::transport::Channel;

/// Error gathering provider capabilities from the gateway.
#[derive(Debug, thiserror::Error)]
pub enum CapabilitiesError {
    #[error("provider gRPC: {0}")]
    Provider(#[from] crate::providers::ProviderError),
    #[error("profile gRPC: {0}")]
    Profile(#[from] crate::managed_profiles::ManagedProfileError),
    #[error("policy read: {0}")]
    Policy(String),
}

/// Gather agent-facing provider capabilities for one sandbox by joining live
/// gateway reads. The effective policy is the source of truth for binaries and
/// hosts (catches composition drift); the profile supplies display name and
/// candidate env vars; the sandbox env supplies the actually-injected keys.
pub async fn provider_capabilities_for_sandbox(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
) -> Result<Vec<ProviderCapability>, CapabilitiesError> {
    let attached = crate::providers::list_attached(client, sandbox_name).await?;

    let mut inputs = Vec::with_capacity(attached.len());
    for name in &attached {
        let provider = crate::providers::get_provider(client, name).await?;
        // provider.type_ is the profile id used at creation.
        let (display_name, candidate_env_vars) =
            match crate::managed_profiles::get_profile(client, &provider.type_).await? {
                Some(p) => {
                    let envs = p
                        .credentials
                        .iter()
                        .flat_map(|c| c.env_vars.clone())
                        .collect();
                    let display = if p.display_name.is_empty() {
                        provider.type_.clone()
                    } else {
                        p.display_name
                    };
                    (display, envs)
                }
                None => (provider.type_.clone(), Vec::new()),
            };
        inputs.push(ProviderCapabilityInput {
            name: name.clone(),
            display_name,
            candidate_env_vars,
        });
    }

    let policy = crate::openshell::get_active_policy(client, sandbox_name)
        .await
        .map_err(|e| CapabilitiesError::Policy(format!("{e:#}")))?
        .unwrap_or_default();

    let sandbox_id = crate::openshell::resolve_sandbox_id(client, sandbox_name)
        .await
        .map_err(|e| CapabilitiesError::Policy(format!("{e:#}")))?;
    let env_map =
        crate::providers::get_sandbox_provider_environment(client, &sandbox_id).await?;
    let env_keys: std::collections::HashSet<String> = env_map.into_keys().collect();

    Ok(correlate_provider_capabilities(&inputs, &policy, &env_keys))
}
```

> Verify exact public names while implementing: `providers::ProviderError`, `managed_profiles::ManagedProfileError`, and `proto_v1::ProviderProfile.credentials[].env_vars`. If `get_active_policy` returns `Ok(None)` (no policy yet), `unwrap_or_default()` yields an empty policy → all providers reported inactive, which is the correct truthful signal. Never log `env_map` values (placeholders).

- [ ] **Step 2: Confirm the crate still builds**

Run:
```bash
devenv shell -- cargo build -p right-openshell
devenv shell -- cargo test -p right-openshell provider_capabilities
```
Expected: builds clean; Task 1 unit tests still PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/right-openshell/src/provider_capabilities.rs
git commit -m "feat(openshell): gather provider capabilities over gRPC"
```

---

## Task 3: MCP tool `provider_capabilities` in `RightBackend`

**Files:**
- Modify: `crates/right/src/right_backend.rs`

- [ ] **Step 1: Add the empty param struct**

In `crates/right/src/right_backend.rs`, next to `ForumTopicListParams` (around line 96-98), add:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderCapabilitiesParams {}
```

- [ ] **Step 2: Register the tool in `tools_list`**

In the `tools_list` `vec![...]` (after the `bootstrap_done` entry, before the closing `]).clone()` at ~line 241), add:

```rust
            // Provider capabilities (read-only; sandbox scope server-enforced)
            Tool::new(
                "provider_capabilities",
                "List the providers attached to your own sandbox and, per provider, which env-var placeholders are injected, which binaries may use the credential, and which hosts it's valid for. Scope is server-enforced (your sandbox only; no arguments). Call this when a provider/API request returns 401/403 before concluding the credential is invalid — the credential may simply require a specific binary (e.g. gh/git) on a specific host.",
                schema_for_type::<ProviderCapabilitiesParams>(),
            ),
```

- [ ] **Step 3: Add the dispatch arm in `tools_call`**

In the `match tool_name { ... }` (after the `"bootstrap_done"` arm, ~line 312), add:

```rust
            "provider_capabilities" => self.call_provider_capabilities(agent_name).await,
```

- [ ] **Step 4: Add the handler**

Add a method on `impl RightBackend` (place it right after `call_bootstrap_done`, before the closing `}` of the impl at ~line 1543):

```rust
    async fn call_provider_capabilities(
        &self,
        agent_name: &str,
    ) -> Result<CallToolResult, anyhow::Error> {
        let Some(mtls_dir) = &self.mtls_dir else {
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::json!({
                    "providers": [],
                    "note": "This agent has no sandbox; gateway providers are not available."
                })
                .to_string(),
            )]));
        };

        let agent_dir = self.agents_dir.join(agent_name);
        let sandbox_name = match right_agent::agent::parse_agent_config(&agent_dir) {
            Ok(Some(config)) => right_openshell::openshell::resolve_sandbox_name(
                agent_name,
                config.sandbox.as_ref().and_then(|s| s.name.as_deref()),
            ),
            _ => right_openshell::openshell::resolve_sandbox_name(agent_name, None),
        };

        let mut client = right_openshell::openshell::connect_grpc(mtls_dir)
            .await
            .map_err(|e| anyhow::anyhow!("{e:#}"))
            .context("provider_capabilities: failed to connect to OpenShell gRPC")?;

        let caps = match right_openshell::provider_capabilities::provider_capabilities_for_sandbox(
            &mut client,
            &sandbox_name,
        )
        .await
        {
            Ok(caps) => caps,
            Err(e) => {
                return Ok(tool_error(
                    "provider_capabilities_failed",
                    format!("could not read provider capabilities: {e:#}"),
                    None,
                ));
            }
        };

        let providers: Vec<serde_json::Value> = caps
            .into_iter()
            .map(|c| {
                serde_json::json!({
                    "display_name": c.display_name,
                    "env_vars": c.env_vars,
                    "allowed_binaries": c.allowed_binaries,
                    "endpoint_hosts": c.endpoint_hosts,
                    "usage_hint": c.usage_hint,
                })
            })
            .collect();

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({ "providers": providers }).to_string(),
        )]))
    }
```

- [ ] **Step 5: Build and run the existing backend tests**

Run:
```bash
devenv shell -- cargo build -p right
devenv shell -- cargo test -p right --lib right_backend
```
Expected: builds; existing `RightBackend`/aggregator schema tests (e.g. `all_tools_have_valid_input_schema`) still PASS — the empty schema is valid `{"type":"object"}`.

- [ ] **Step 6: Commit**

```bash
git add crates/right/src/right_backend.rs
git commit -m "feat(mcp): add provider_capabilities built-in tool"
```

---

## Task 4: Advertise the tool in both `with_instructions()` + tests

**Files:**
- Modify: `crates/right/src/aggregator.rs`
- Modify: `crates/right/src/memory_server.rs`

- [ ] **Step 1: Add a section to the aggregator instructions**

In `crates/right/src/aggregator.rs`, inside the `with_instructions(...)` string (after the `## Forum Topics` block ending at ~line 607, before `## Learning`), insert:

```
                 ## Providers\n\
                 - mcp__right__provider_capabilities: List your sandbox's attached providers — per provider: injected env-var placeholders, which binaries may use the credential, and valid hosts. Scope is server-enforced (your sandbox; no args). On a provider 401/403, call this before concluding the credential is invalid; the gateway substitutes the secret only for the listed binaries on the listed hosts.\n\n\
```

- [ ] **Step 2: Add the same to the stdio instructions (with stdio caveat)**

In `crates/right/src/memory_server.rs`, inside its `with_instructions(...)` string (after the `## Forum Topics` block ending ~line 602, before `## Learning`), insert:

```
                 ## Providers\n\
                 - mcp__right__provider_capabilities: List your sandbox's attached providers — per provider: injected env-var placeholders, allowed binaries, and valid hosts. On a provider 401/403, call this before concluding the credential is invalid. DO NOT call in stdio mode — provider capabilities require the HTTP aggregator + sandbox gateway.\n\n\
```

- [ ] **Step 3: Extend the instruction-content tests**

Find the test in `crates/right/src/aggregator.rs` (~lines 907-957) that asserts tool names appear in the instructions. Add an assertion mirroring the existing style, e.g.:

```rust
        assert!(instructions.contains("mcp__right__provider_capabilities"));
```

Do the same in the `crates/right/src/memory_server.rs` instruction test (~lines 771-801).

- [ ] **Step 4: Run the instruction tests**

Run:
```bash
devenv shell -- cargo test -p right with_instructions
devenv shell -- cargo test -p right get_info
```
Expected: PASS, including the new assertions. (If the test fn names differ, run `devenv shell -- cargo test -p right instructions` to locate them.)

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/aggregator.rs crates/right/src/memory_server.rs
git commit -m "docs(mcp): advertise provider_capabilities in server instructions"
```

---

## Task 5: Prompt — concept + trigger in `OPERATING_INSTRUCTIONS.md`

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Extend the existing "## Credentials & API Keys" section**

In `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`, the section at lines 56-58 already exists. Append these two sentences to the end of that paragraph (keep it declarative, within the prompt-tier brevity budget — do NOT add a new heading):

```
Provider credentials appear in env as opaque placeholders; the gateway substitutes the real secret only for specific binaries on specific hosts, so a placeholder pasted into an arbitrary HTTP client (curl/fetch) is sent unsubstituted and 401s. On a provider 401/403, call `mcp__right__provider_capabilities` to see which binary can use the credential and on which hosts before concluding it is invalid.
```

- [ ] **Step 2: Sync `PROMPT_SYSTEM.md`**

In `PROMPT_SYSTEM.md`, find where the built-in MCP tool set and/or `OPERATING_INSTRUCTIONS` Credentials guidance is documented (search for `forum_topic_list` or `Credentials`). Add a one-line entry describing `mcp__right__provider_capabilities` and the 401-trigger guidance, matching the surrounding style.

Run:
```bash
rg -n "forum_topic_list|Credentials & API Keys|provider" PROMPT_SYSTEM.md
```
Expected: locate the right spot; add the matching line.

- [ ] **Step 3: Verify codegen tests still pass**

Run:
```bash
devenv shell -- cargo test -p right-codegen
```
Expected: PASS (prompt assembly tests unaffected by added prose).

- [ ] **Step 4: Commit**

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md PROMPT_SYSTEM.md
git commit -m "docs(prompt): teach agent provider placeholder/binary model + 401 trigger"
```

---

## Task 6: Live `ci_openshell_` integration test

**Files:**
- Modify: `crates/right-openshell/tests/ci_openshell_generic_provider.rs`

Reuses the existing harness (`author_generic_profile`, `ensure_generic_profile`, `create_provider`, `TestSandbox::create_with_policy`, `attach_to_sandbox`, `ensure_provider_policy_loaded`, `wait_for_provider_placeholder`, `cleanup_generic_resources`, `with_generic_cleanup`). The test reads capabilities from the gateway only — no external HTTP — so it cannot flake on upstream uptime.

- [ ] **Step 1: Add the test**

Append to `crates/right-openshell/tests/ci_openshell_generic_provider.rs`:

```rust
#[tokio::test]
#[ignore = "ci-openshell: live sandbox + gateway"]
async fn ci_openshell_provider_capabilities_reports_attached_provider() {
    let profile_id = unique_profile_id("generic-caps");
    let provider_name = unique_name("generic-caps");
    let sandbox_name = Arc::new(Mutex::new(None));
    cleanup_generic_resources(&provider_name, &profile_id, None).await;

    with_generic_cleanup(&provider_name, &profile_id, sandbox_name.clone(), async {
        let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();

        ensure_generic_profile(&mut client, &profile_id, true).await;
        right_openshell::test_cleanup::register_test_provider(&provider_name, Some(&profile_id));
        create_provider(&mut client, &fake_provider_spec(&provider_name, &profile_id))
            .await
            .expect("create provider");

        let (_policy_tmp, policy_path) = raw_tunnel_policy_file();
        let sandbox =
            TestSandbox::create_with_policy("ci-openshell-generic-caps", RAW_TUNNEL_BASE_POLICY)
                .await;
        *sandbox_name.lock().expect("sandbox name lock") = Some(sandbox.name().to_string());
        attach_to_sandbox(&mut client, sandbox.name(), &provider_name)
            .await
            .expect("attach provider");
        right_openshell::test_cleanup::register_test_provider_attachment(
            &provider_name,
            sandbox.name(),
        );
        ensure_provider_policy_loaded(sandbox.name(), &policy_path)
            .await
            .expect("provider policy loaded");
        wait_for_provider_placeholder(&sandbox).await;

        let caps =
            right_openshell::provider_capabilities::provider_capabilities_for_sandbox(
                &mut client,
                sandbox.name(),
            )
            .await
            .expect("gather provider capabilities");

        let cap = caps
            .iter()
            .find(|c| c.env_vars.iter().any(|v| v == ENV_VAR))
            .unwrap_or_else(|| panic!("capabilities must include {ENV_VAR}; got {caps:?}"));
        assert!(
            cap.allowed_binaries.iter().any(|b| b == "**"),
            "generic profile uses binaries ** ; got {:?}",
            cap.allowed_binaries
        );
        assert!(
            cap.endpoint_hosts.iter().any(|h| h == UPSTREAM_HOST),
            "endpoint host must include {UPSTREAM_HOST}; got {:?}",
            cap.endpoint_hosts
        );
    })
    .await;
}
```

- [ ] **Step 2: Run the live test (dev machine has OpenShell)**

Run:
```bash
devenv shell -- cargo test -p right-openshell --test ci_openshell_generic_provider \
  ci_openshell_provider_capabilities_reports_attached_provider -- --ignored --exact
```
Expected: PASS (creates a throwaway sandbox + provider, asserts capabilities, auto-cleans). If `ENV_VAR`/`UPSTREAM_HOST`/`ProviderCapability` debug formatting differ, align with the actual constants/struct.

- [ ] **Step 3: Commit**

```bash
git add crates/right-openshell/tests/ci_openshell_generic_provider.rs
git commit -m "test(ci-openshell): provider_capabilities reports attached provider"
```

---

## Task 7: Final verification

- [ ] **Step 1: Full workspace test (mandatory)**

Run:
```bash
devenv shell -- cargo test --workspace
```
Expected: PASS. Live `ci_openshell_`-prefixed tests are `#[ignore]` in the default run; the new one runs only under the explicit `--ignored` invocation (Task 6) and in CI's ignored-tests job. Note any pre-existing flakes (see memory: cc/invocation pid race, dashboard warn-count) and re-run isolated before attributing to this change.

- [ ] **Step 2: Clippy on touched crates**

Run:
```bash
devenv shell -- cargo clippy -p right-openshell -p right
```
Expected: no new warnings.

- [ ] **Step 3: Confirm `git status` is clean and the spec/plan are committed**

```bash
git status
```
Expected: working tree clean; the design spec and this plan are tracked.

---

## Self-review checklist (completed during authoring)

- **Spec coverage:** tool (Task 3), D2 effective-policy source (Tasks 1-2), scope server-enforced/no-args (Task 3 empty `deny_unknown_fields` param + agent-derived sandbox), prompt concept+trigger (Task 5), with_instructions both files (Task 4), unit + live tests (Tasks 1, 6), no credential values returned (env keys only, never `env_map` values). All covered.
- **Placeholder scan:** no TBD/TODO; every code step shows complete code.
- **Type consistency:** `ProviderCapability`/`ProviderCapabilityInput`/`correlate_provider_capabilities`/`provider_capabilities_for_sandbox` names are identical across Tasks 1, 2, 3, 6. Tool name `provider_capabilities` consistent across `tools_list`, `tools_call`, both `with_instructions`, and the prompt.
- **Out of scope (unchanged):** `right-github` binary allowlist, broad policy introspection, gateway error shaping, live credential validation.
