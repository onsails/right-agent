# Upgrade & Migration Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Codify how codegen changes propagate to deployed RightClaw agents, and apply the model to remove the deprecated `tls: terminate` field from generated OpenShell policies.

**Architecture:** New `codegen::contract` module exposes three write helpers (`write_regenerated`, `write_merged_rmw`, `write_agent_owned`) plus a sandbox-aware async helper (`write_and_apply_sandbox_policy`). Every codegen output becomes a `CodegenFile` entry in a central registry (per-agent + cross-agent). Guard tests assert idempotency, unknown-field preservation, agent-file preservation, and registry coverage. The bot startup gains a filesystem-policy drift check that emits a WARN pointing at `rightclaw agent config`.

**Tech Stack:** Rust edition 2024, `miette` for error reporting, `serde_saphyr` for YAML, `serde_json` for JSON, `sha2` for idempotency hashing in tests, existing `rightclaw::openshell` gRPC helpers, `rightclaw::test_support::TestSandbox` for live-sandbox integration test.

**Spec:** `docs/superpowers/specs/2026-04-24-upgrade-migration-model-design.md`

---

## Phase 1 — Fix `tls: terminate` deprecation (standalone value, no dependencies)

### Task 1: Add deprecated-fields guard test for policy codegen

**Files:**
- Modify: `crates/rightclaw/src/codegen/policy.rs` (extend `#[cfg(test)]` module)

- [ ] **Step 1: Add test scanning both network policies across host_ip variants**

Append this test to the `mod tests` block in `crates/rightclaw/src/codegen/policy.rs`:

```rust
/// Guard against emitting OpenShell-deprecated policy fields.
/// `tls: terminate` and `tls: passthrough` were deprecated in OpenShell PR #544
/// (v0.0.28+). Emitting them produces per-request WARN and will break once
/// OpenShell removes the field entirely.
#[test]
fn policy_has_no_deprecated_openshell_fields() {
    let forbidden = [
        ("tls: terminate", "use `auto` (default) instead"),
        ("tls: passthrough", "use `skip` for raw tunnel"),
    ];
    let ip: std::net::IpAddr = "192.168.65.254".parse().unwrap();
    let cases = [
        ("permissive+no_ip", generate_policy(8100, &NetworkPolicy::Permissive, None)),
        ("permissive+ip", generate_policy(8100, &NetworkPolicy::Permissive, Some(ip))),
        ("restrictive+no_ip", generate_policy(8100, &NetworkPolicy::Restrictive, None)),
        ("restrictive+ip", generate_policy(8100, &NetworkPolicy::Restrictive, Some(ip))),
    ];
    for (label, policy) in &cases {
        for (pattern, guidance) in &forbidden {
            assert!(
                !policy.contains(pattern),
                "[{label}] policy must not emit deprecated `{pattern}` — {guidance}",
            );
        }
    }
}
```

- [ ] **Step 2: Run the test, verify it fails**

```bash
cargo test -p rightclaw policy::tests::policy_has_no_deprecated_openshell_fields
```

Expected: FAIL with message referencing `tls: terminate`.

- [ ] **Step 3: Commit the failing test**

```bash
git add crates/rightclaw/src/codegen/policy.rs
git commit -m "test(policy): guard against deprecated OpenShell tls fields"
```

### Task 2: Remove `tls: terminate` from policy templates

**Files:**
- Modify: `crates/rightclaw/src/codegen/policy.rs:16-30, 48-62, 134-141`

- [ ] **Step 1: Update `restrictive_endpoints()` — drop the `tls: terminate` line**

Replace lines 16-30 (the `restrictive_endpoints` fn body) with:

```rust
fn restrictive_endpoints() -> String {
    RESTRICTIVE_DOMAINS
        .iter()
        .map(|host| {
            format!(
                r#"      - host: "{host}"
        port: 443
        protocol: rest
        access: full"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

- [ ] **Step 2: Update the permissive branch — drop the `tls: terminate` line**

In `generate_policy`, replace the `NetworkPolicy::Permissive` arm (current lines 48-62) with:

```rust
        NetworkPolicy::Permissive => r#"  outbound:
    endpoints:
      - host: "**.*"
        port: 443
        protocol: rest
        access: full
      - host: "**.*"
        port: 80
        protocol: rest
        access: full
    binaries:
      - path: "**""#
            .to_owned(),
```

- [ ] **Step 3: Invert the existing `allows_all_outbound_https_and_http` assertion**

In the test at current line 134-141, change:
```rust
assert!(policy.contains("tls: terminate"));
```
to:
```rust
assert!(
    !policy.contains("tls: terminate"),
    "deprecated OpenShell field must not be emitted",
);
```

- [ ] **Step 4: Run all policy tests, verify they pass**

```bash
cargo test -p rightclaw codegen::policy::tests
```

Expected: all tests pass, including `policy_has_no_deprecated_openshell_fields`.

- [ ] **Step 5: Run workspace build to catch any other callers**

```bash
cargo build --workspace
```

Expected: clean build.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/policy.rs
git commit -m "fix(policy): drop deprecated tls: terminate from generated YAML"
```

---

## Phase 2 — Contract module scaffolding

### Task 3: Create `codegen::contract` module with types

**Files:**
- Create: `crates/rightclaw/src/codegen/contract.rs`
- Modify: `crates/rightclaw/src/codegen/mod.rs` (add `pub mod contract;`)

- [ ] **Step 1: Create `contract.rs` with enums and struct**

Create `crates/rightclaw/src/codegen/contract.rs`:

```rust
//! Codegen output contract.
//!
//! Every file written by codegen belongs to exactly one [`CodegenKind`]. The
//! helpers in this module are the only sanctioned writers for codegen files.
//! Direct `std::fs::write` inside `codegen/*` modules is a review-blocking
//! defect after this module lands.
//!
//! See `docs/superpowers/specs/2026-04-24-upgrade-migration-model-design.md`.

use std::path::{Path, PathBuf};

/// Category of a codegen output. Drives how changes propagate to running agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodegenKind {
    /// Unconditional overwrite on every bot start.
    Regenerated(HotReload),
    /// Read existing, merge codegen fields in, write back. Preserves unknown fields.
    MergedRMW,
    /// Created by init with an initial payload. Never touched by codegen again.
    AgentOwned,
}

/// How a `Regenerated` change reaches a running sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotReload {
    /// Takes effect on next CC invocation. No sandbox RPC needed.
    BotRestart,
    /// Applied via `openshell policy set --wait` after write. Network-only.
    SandboxPolicyApply,
    /// Boot-time-only (landlock, filesystem). Requires sandbox migration.
    SandboxRecreate,
}

/// An entry in the codegen registry.
#[derive(Debug, Clone)]
pub struct CodegenFile {
    pub kind: CodegenKind,
    pub path: PathBuf,
}
```

- [ ] **Step 2: Register module in `mod.rs`**

Edit `crates/rightclaw/src/codegen/mod.rs` to add `pub mod contract;` alongside the other `pub mod ...;` entries.

- [ ] **Step 3: Verify compilation**

```bash
cargo build -p rightclaw
```

Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/codegen/contract.rs crates/rightclaw/src/codegen/mod.rs
git commit -m "feat(codegen): scaffold contract module with CodegenKind types"
```

### Task 4: Implement `write_regenerated`

**Files:**
- Modify: `crates/rightclaw/src/codegen/contract.rs`

- [ ] **Step 1: Add failing test**

Append to `contract.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_regenerated_overwrites_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sub/file.txt");
        write_regenerated(&path, "first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        write_regenerated(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    }

    #[test]
    fn write_regenerated_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a/b/c/file.txt");
        write_regenerated(&path, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }
}
```

- [ ] **Step 2: Run test, verify failure**

```bash
cargo test -p rightclaw codegen::contract::tests::write_regenerated
```

Expected: COMPILE ERROR — `write_regenerated` not defined.

- [ ] **Step 3: Implement `write_regenerated`**

Add to `contract.rs` (above the `#[cfg(test)]` block):

```rust
/// Unconditional write — the sanctioned writer for
/// `Regenerated(BotRestart)` and `Regenerated(SandboxRecreate)` outputs.
///
/// `Regenerated(SandboxPolicyApply)` outputs MUST go through
/// [`write_and_apply_sandbox_policy`] instead — there is no other writer for
/// that category, so callers cannot skip `apply_policy`.
///
/// Creates parent directories if absent.
pub fn write_regenerated(path: &Path, content: &str) -> miette::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            miette::miette!("failed to create parent dir for {}: {e:#}", path.display())
        })?;
    }
    std::fs::write(path, content)
        .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))
}
```

- [ ] **Step 4: Run tests, verify pass**

```bash
cargo test -p rightclaw codegen::contract::tests::write_regenerated
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/contract.rs
git commit -m "feat(codegen/contract): add write_regenerated helper"
```

### Task 5: Implement `write_agent_owned`

**Files:**
- Modify: `crates/rightclaw/src/codegen/contract.rs`

- [ ] **Step 1: Add failing test**

Append inside the existing `mod tests`:

```rust
    #[test]
    fn write_agent_owned_creates_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("TOOLS.md");
        write_agent_owned(&path, "# default").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# default");
    }

    #[test]
    fn write_agent_owned_preserves_when_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("TOOLS.md");
        std::fs::write(&path, "agent-edited content").unwrap();
        write_agent_owned(&path, "# default (should be ignored)").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "agent-edited content");
    }
```

- [ ] **Step 2: Run test, verify failure**

```bash
cargo test -p rightclaw codegen::contract::tests::write_agent_owned
```

Expected: COMPILE ERROR — `write_agent_owned` not defined.

- [ ] **Step 3: Implement `write_agent_owned`**

Add to `contract.rs` alongside `write_regenerated`:

```rust
/// No-op if the file exists. Otherwise writes `initial`, creating parent
/// directories as needed. The sanctioned writer for `AgentOwned` outputs.
pub fn write_agent_owned(path: &Path, initial: &str) -> miette::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            miette::miette!("failed to create parent dir for {}: {e:#}", path.display())
        })?;
    }
    std::fs::write(path, initial)
        .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))
}
```

- [ ] **Step 4: Run tests, verify pass**

```bash
cargo test -p rightclaw codegen::contract::tests::write_agent_owned
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/contract.rs
git commit -m "feat(codegen/contract): add write_agent_owned helper"
```

### Task 6: Implement `write_merged_rmw`

**Files:**
- Modify: `crates/rightclaw/src/codegen/contract.rs`

- [ ] **Step 1: Add failing tests**

Append inside `mod tests`:

```rust
    #[test]
    fn write_merged_rmw_passes_existing_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"a":1}"#).unwrap();
        write_merged_rmw(&path, |existing| {
            let existing = existing.expect("file should exist");
            assert_eq!(existing, r#"{"a":1}"#);
            Ok(format!("{}+merged", existing))
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"a":1}+merged"#);
    }

    #[test]
    fn write_merged_rmw_passes_none_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("new.json");
        write_merged_rmw(&path, |existing| {
            assert!(existing.is_none());
            Ok("{}".to_owned())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    }

    #[test]
    fn write_merged_rmw_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested/new.json");
        write_merged_rmw(&path, |_| Ok("{}".to_owned())).unwrap();
        assert!(path.exists());
    }
```

- [ ] **Step 2: Run tests, verify failure**

```bash
cargo test -p rightclaw codegen::contract::tests::write_merged_rmw
```

Expected: COMPILE ERROR — `write_merged_rmw` not defined.

- [ ] **Step 3: Implement `write_merged_rmw`**

Add to `contract.rs`:

```rust
/// Read-modify-write. `merge_fn` receives `Some(existing)` if the file is
/// present, `None` otherwise, and returns the final content. Merger must
/// preserve unknown fields.
///
/// The sanctioned writer for `MergedRMW` outputs.
pub fn write_merged_rmw<F>(path: &Path, merge_fn: F) -> miette::Result<()>
where
    F: FnOnce(Option<&str>) -> miette::Result<String>,
{
    let existing = if path.exists() {
        Some(std::fs::read_to_string(path).map_err(|e| {
            miette::miette!("failed to read {} for merge: {e:#}", path.display())
        })?)
    } else {
        None
    };
    let content = merge_fn(existing.as_deref())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            miette::miette!("failed to create parent dir for {}: {e:#}", path.display())
        })?;
    }
    std::fs::write(path, content)
        .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))
}
```

- [ ] **Step 4: Run tests, verify pass**

```bash
cargo test -p rightclaw codegen::contract::tests::write_merged_rmw
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/contract.rs
git commit -m "feat(codegen/contract): add write_merged_rmw helper"
```

### Task 7: Implement `write_and_apply_sandbox_policy`

**Files:**
- Modify: `crates/rightclaw/src/codegen/contract.rs`

- [ ] **Step 1: Implement helper**

Add to `contract.rs`:

```rust
/// The ONLY way to update policy for a running sandbox. Writes `content` to
/// `path`, then applies it via `openshell policy set --wait`. Network-only
/// policy changes hot-reload; filesystem changes require sandbox migration
/// (handled separately by `maybe_migrate_sandbox`).
pub async fn write_and_apply_sandbox_policy(
    sandbox: &str,
    path: &Path,
    content: &str,
) -> miette::Result<()> {
    write_regenerated(path, content)?;
    crate::openshell::apply_policy(sandbox, path).await
}
```

No unit test — requires a live sandbox. Covered by the integration test in Task 26 and the existing policy-apply path in `bot/src/lib.rs`.

- [ ] **Step 2: Verify build**

```bash
cargo build -p rightclaw
cargo test -p rightclaw codegen::contract
```

Expected: clean build; all contract unit tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/contract.rs
git commit -m "feat(codegen/contract): add write_and_apply_sandbox_policy"
```

---

## Phase 3 — Central registries

### Task 8: Stub `codegen_registry` and `crossagent_codegen_registry`

**Files:**
- Modify: `crates/rightclaw/src/codegen/contract.rs`

- [ ] **Step 1: Add empty-registry stub with documentation**

Append to `contract.rs` (above `#[cfg(test)]`):

```rust
/// Per-agent codegen outputs. Source of truth for guard tests.
///
/// Every file produced by [`crate::codegen::run_single_agent_codegen`] MUST
/// appear here or in the documented `KNOWN_EXCEPTIONS` inside
/// `registry_covers_all_per_agent_writes`.
pub fn codegen_registry(agent_dir: &Path) -> Vec<CodegenFile> {
    let claude = agent_dir.join(".claude");
    vec![
        CodegenFile {
            kind: CodegenKind::MergedRMW,
            path: agent_dir.join("agent.yaml"),
        },
        CodegenFile {
            kind: CodegenKind::MergedRMW,
            path: agent_dir.join(".claude.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: agent_dir.join("mcp.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::SandboxRecreate),
            path: agent_dir.join("policy.yaml"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("settings.json"),
        },
        CodegenFile {
            kind: CodegenKind::AgentOwned,
            path: claude.join("settings.local.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("reply-schema.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("cron-schema.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("bootstrap-schema.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("system-prompt.md"),
        },
        // Skills are registered as a single tree-rooted entry; the installer
        // manages content-addressed files beneath.
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: claude.join("skills"),
        },
    ]
}

/// Cross-agent codegen outputs under `<home>/run/` and peers.
pub fn crossagent_codegen_registry(home: &Path) -> Vec<CodegenFile> {
    let run = home.join("run");
    vec![
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: run.join("process-compose.yaml"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: run.join("agent-tokens.json"),
        },
        CodegenFile {
            kind: CodegenKind::Regenerated(HotReload::BotRestart),
            path: home.join("scripts").join("cloudflared-start.sh"),
        },
        // cloudflared config path depends on tunnel config; add lazily when
        // the tunnel subsystem registers it.
    ]
}
```

- [ ] **Step 2: Add basic structural test**

Append inside `mod tests`:

```rust
    #[test]
    fn codegen_registry_has_all_expected_categories() {
        let dir = tempdir().unwrap();
        let reg = codegen_registry(dir.path());
        assert!(reg.iter().any(|f| matches!(f.kind, CodegenKind::MergedRMW)));
        assert!(reg.iter().any(|f| matches!(f.kind, CodegenKind::AgentOwned)));
        assert!(reg.iter().any(|f| matches!(f.kind,
            CodegenKind::Regenerated(HotReload::BotRestart))));
        assert!(reg.iter().any(|f| matches!(f.kind,
            CodegenKind::Regenerated(HotReload::SandboxRecreate))));
    }
```

- [ ] **Step 3: Run test + build**

```bash
cargo test -p rightclaw codegen::contract
cargo build -p rightclaw
```

Expected: all contract tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/codegen/contract.rs
git commit -m "feat(codegen/contract): add per-agent and cross-agent registries"
```

---

## Phase 4 — Refactor call sites to use helpers

### Task 9: Refactor `pipeline.rs` static-content writes (schemas + system prompt + settings)

**Files:**
- Modify: `crates/rightclaw/src/codegen/pipeline.rs` (lines 62-137 area)

- [ ] **Step 1: Import helpers at top of file**

Add to the imports at top of `pipeline.rs`:

```rust
use crate::codegen::contract::{write_agent_owned, write_merged_rmw, write_regenerated};
```

- [ ] **Step 2: Replace the four `fs::write` calls for schemas + system-prompt + settings**

In `run_single_agent_codegen`, replace each of these blocks (current line numbers in comments):

```rust
// Was: fs::write(claude_dir.join("reply-schema.json"), REPLY_SCHEMA_JSON) (line 68)
write_regenerated(
    &claude_dir.join("reply-schema.json"),
    crate::codegen::REPLY_SCHEMA_JSON,
)?;

// Was: fs::write(claude_dir.join("cron-schema.json"), CRON_SCHEMA_JSON) (line 80)
write_regenerated(
    &claude_dir.join("cron-schema.json"),
    crate::codegen::CRON_SCHEMA_JSON,
)?;

// Was: fs::write(claude_dir.join("system-prompt.md"), generate_system_prompt(...)) (line 98)
write_regenerated(
    &claude_dir.join("system-prompt.md"),
    &crate::codegen::generate_system_prompt(&agent.name, &agent_sandbox_mode, &home_dir),
)?;

// Was: fs::write(claude_dir.join("bootstrap-schema.json"), BOOTSTRAP_SCHEMA_JSON) (line 110)
write_regenerated(
    &claude_dir.join("bootstrap-schema.json"),
    crate::codegen::BOOTSTRAP_SCHEMA_JSON,
)?;

// Was: fs::write(claude_dir.join("settings.json"), serde_json::to_string_pretty(&settings)?) (line 130)
let settings_json = serde_json::to_string_pretty(&settings).map_err(|e| {
    miette::miette!("failed to serialize settings for '{}': {e:#}", agent.name)
})?;
write_regenerated(&claude_dir.join("settings.json"), &settings_json)?;
```

The existing `create_dir_all(&claude_dir)` stays — it creates the dir even when no writes happen yet, which is still useful for the subsequent `shell-snapshots` subdir.

- [ ] **Step 3: Build + run existing pipeline tests**

```bash
cargo build -p rightclaw
cargo test -p rightclaw codegen::pipeline
```

Expected: clean build; all existing tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/codegen/pipeline.rs
git commit -m "refactor(codegen/pipeline): route static-content writes through write_regenerated"
```

### Task 10: Refactor `pipeline.rs` `settings.local.json` write

**Files:**
- Modify: `crates/rightclaw/src/codegen/pipeline.rs:175-183`

- [ ] **Step 1: Replace the conditional write with `write_agent_owned`**

Current block (line 175-183):

```rust
let settings_local = claude_dir.join("settings.local.json");
if !settings_local.exists() {
    std::fs::write(&settings_local, "{}").map_err(|e| {
        miette::miette!(
            "failed to write settings.local.json for '{}': {e:#}",
            agent.name
        )
    })?;
}
```

Becomes:

```rust
write_agent_owned(&claude_dir.join("settings.local.json"), "{}")?;
```

- [ ] **Step 2: Build + test**

```bash
cargo test -p rightclaw codegen::pipeline
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/pipeline.rs
git commit -m "refactor(codegen/pipeline): route settings.local.json through write_agent_owned"
```

### Task 11: Refactor `ensure_agent_secret` (agent.yaml) to `write_merged_rmw`

**Files:**
- Modify: `crates/rightclaw/src/codegen/pipeline.rs:7-34`

- [ ] **Step 1: Rewrite `ensure_agent_secret`**

Replace the current function body (lines 9-34) with:

```rust
fn ensure_agent_secret(
    agent_path: &Path,
    agent_name: &str,
    existing: Option<&str>,
) -> miette::Result<String> {
    if let Some(secret) = existing {
        return Ok(secret.to_owned());
    }

    let new_secret = crate::mcp::generate_agent_secret();
    let yaml_path = agent_path.join("agent.yaml");

    write_merged_rmw(&yaml_path, |existing| {
        let content = existing.ok_or_else(|| {
            miette::miette!("agent.yaml missing for '{agent_name}'")
        })?;
        let mut doc: serde_json::Map<String, serde_json::Value> =
            serde_saphyr::from_str(content).map_err(|e| {
                miette::miette!("failed to parse agent.yaml for '{agent_name}': {e:#}")
            })?;
        doc.insert(
            "secret".to_owned(),
            serde_json::Value::String(new_secret.clone()),
        );
        serde_saphyr::to_string(&doc).map_err(|e| {
            miette::miette!("failed to serialize agent.yaml for '{agent_name}': {e:#}")
        })
    })?;
    tracing::info!(agent = %agent_name, "generated new agent secret");
    Ok(new_secret)
}
```

- [ ] **Step 2: Build + test**

```bash
cargo test -p rightclaw codegen::pipeline
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/pipeline.rs
git commit -m "refactor(codegen/pipeline): route agent secret injection through write_merged_rmw"
```

### Task 12: Refactor `pipeline.rs` `policy.yaml` seed write

**Files:**
- Modify: `crates/rightclaw/src/codegen/pipeline.rs:196-206`

- [ ] **Step 1: Replace the `fs::write` with `write_regenerated`**

Current block:

```rust
let policy_content = crate::codegen::policy::generate_policy(mcp_port, &network_policy, None);
std::fs::write(agent.path.join("policy.yaml"), &policy_content)
    .map_err(|e| miette::miette!("failed to write policy.yaml for '{}': {e:#}", agent.name))?;
```

Becomes:

```rust
let policy_content = crate::codegen::policy::generate_policy(mcp_port, &network_policy, None);
write_regenerated(&agent.path.join("policy.yaml"), &policy_content)?;
```

- [ ] **Step 2: Build + test**

```bash
cargo test -p rightclaw codegen::pipeline
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/pipeline.rs
git commit -m "refactor(codegen/pipeline): route policy.yaml seed through write_regenerated"
```

### Task 13: Refactor `claude_json.rs` to `write_merged_rmw`

**Files:**
- Modify: `crates/rightclaw/src/codegen/claude_json.rs:13-76`

- [ ] **Step 1: Rewrite `generate_agent_claude_json`**

Replace the function body with:

```rust
pub fn generate_agent_claude_json(agent: &AgentDef) -> miette::Result<()> {
    let claude_json_path = agent.path.join(".claude.json");
    let path_key = agent
        .path
        .canonicalize()
        .unwrap_or_else(|_| agent.path.clone())
        .display()
        .to_string();

    crate::codegen::contract::write_merged_rmw(&claude_json_path, |existing| {
        let mut config: serde_json::Value = match existing {
            Some(content) => serde_json::from_str(content).map_err(|e| {
                miette::miette!("failed to parse {}: {e:#}", claude_json_path.display())
            })?,
            None => serde_json::json!({}),
        };

        let root = config
            .as_object_mut()
            .ok_or_else(|| miette::miette!(".claude.json is not a JSON object"))?;

        root.entry("hasCompletedOnboarding")
            .or_insert(serde_json::Value::Bool(true));

        let projects = root
            .entry("projects")
            .or_insert_with(|| serde_json::json!({}));

        let project = projects
            .as_object_mut()
            .ok_or_else(|| miette::miette!("projects is not a JSON object"))?
            .entry(&path_key)
            .or_insert_with(|| serde_json::json!({}));

        project
            .as_object_mut()
            .ok_or_else(|| miette::miette!("project entry is not a JSON object"))?
            .insert(
                "hasTrustDialogAccepted".to_owned(),
                serde_json::Value::Bool(true),
            );

        projects
            .as_object_mut()
            .ok_or_else(|| miette::miette!("projects is not a JSON object"))?
            .entry("/sandbox")
            .or_insert_with(|| serde_json::json!({"hasTrustDialogAccepted": true}));

        serde_json::to_string_pretty(&config)
            .map_err(|e| miette::miette!("failed to serialize .claude.json: {e:#}"))
    })?;

    tracing::debug!(agent = %agent.name, "wrote .claude.json");
    Ok(())
}
```

- [ ] **Step 2: Build + run the existing claude_json tests**

```bash
cargo test -p rightclaw codegen::claude_json
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/claude_json.rs
git commit -m "refactor(codegen/claude_json): route .claude.json through write_merged_rmw"
```

### Task 14: Refactor `mcp_config.rs` writes

**Files:**
- Modify: `crates/rightclaw/src/codegen/mcp_config.rs:12-65, 73-99`

- [ ] **Step 1: Rewrite `generate_mcp_config` using `write_merged_rmw`**

Replace the function body (lines 12-65) with:

```rust
pub fn generate_mcp_config(
    agent_path: &Path,
    binary: &Path,
    agent_name: &str,
    rightclaw_home: &Path,
) -> miette::Result<()> {
    let mcp_path = agent_path.join("mcp.json");

    crate::codegen::contract::write_merged_rmw(&mcp_path, |existing| {
        let mut root: serde_json::Value = match existing {
            Some(content) => serde_json::from_str(content)
                .map_err(|e| miette::miette!("failed to parse mcp.json: {e:#}"))?,
            None => serde_json::json!({}),
        };

        let obj = root
            .as_object_mut()
            .ok_or_else(|| miette::miette!("mcp.json root is not a JSON object"))?;

        if !obj.contains_key("mcpServers") {
            obj.insert("mcpServers".to_string(), serde_json::json!({}));
        }

        let servers = obj
            .get_mut("mcpServers")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| miette::miette!("mcp.json mcpServers is not a JSON object"))?;

        servers.insert(
            "right".to_string(),
            serde_json::json!({
                "command": binary.to_string_lossy(),
                "args": ["memory-server"],
                "env": {
                    "RC_AGENT_NAME": agent_name,
                    "RC_RIGHTCLAW_HOME": rightclaw_home.to_string_lossy().as_ref()
                }
            }),
        );

        servers.remove("rightmemory");

        serde_json::to_string_pretty(&root)
            .map_err(|e| miette::miette!("failed to serialize mcp.json: {e:#}"))
    })
}
```

- [ ] **Step 2: Rewrite `generate_mcp_config_http` using `write_regenerated`**

Replace the function body (lines 73-99) with:

```rust
pub fn generate_mcp_config_http(
    agent_path: &Path,
    _agent_name: &str,
    right_mcp_url: &str,
    bearer_token: &str,
) -> miette::Result<()> {
    let mcp_path = agent_path.join("mcp.json");

    let root = serde_json::json!({
        "mcpServers": {
            "right": {
                "type": "http",
                "url": right_mcp_url,
                "headers": {
                    "Authorization": format!("Bearer {bearer_token}")
                }
            }
        }
    });

    let output = serde_json::to_string_pretty(&root)
        .map_err(|e| miette::miette!("failed to serialize mcp.json: {e:#}"))?;
    crate::codegen::contract::write_regenerated(&mcp_path, &output)
}
```

- [ ] **Step 3: Build + run existing mcp_config tests**

```bash
cargo test -p rightclaw codegen::mcp_config
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/codegen/mcp_config.rs
git commit -m "refactor(codegen/mcp_config): route mcp.json writes through contract helpers"
```

### Task 15: Refactor `skills.rs` writes

**Files:**
- Modify: `crates/rightclaw/src/codegen/skills.rs:44, 58`

- [ ] **Step 1: Inspect current writes**

```bash
sed -n '40,65p' crates/rightclaw/src/codegen/skills.rs
```

Verify that line 44 writes `installed.json` in the `.claude/skills/` tree and line 58 writes individual skill files. Both are unconditional overwrites — `Regenerated(BotRestart)`.

- [ ] **Step 2: Replace `std::fs::write` calls**

At line 44 (the `installed.json` write):
```rust
// Was: std::fs::write(&installed_json_path, "{}")
crate::codegen::contract::write_regenerated(&installed_json_path, "{}")?;
```

At line 58 (the per-file skill content write):
```rust
// Was: std::fs::write(&dest, file.contents())
crate::codegen::contract::write_regenerated(&dest, std::str::from_utf8(file.contents())
    .map_err(|e| miette::miette!("skill content is not UTF-8: {e:#}"))?)?;
```

If `file.contents()` returns `&[u8]`, use the UTF-8-checked form above (embedded skill sources are Markdown/text, so this should never fail; the check surfaces a real bug if one ever sneaks in).

- [ ] **Step 3: Build + test**

```bash
cargo test -p rightclaw codegen::skills
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/codegen/skills.rs
git commit -m "refactor(codegen/skills): route skill writes through write_regenerated"
```

### Task 16: Refactor cross-agent writes in `pipeline.rs`

**Files:**
- Modify: `crates/rightclaw/src/codegen/pipeline.rs` (lines 265, 317, 332, 358)

- [ ] **Step 1: Identify the four remaining `std::fs::write` calls in cross-agent codegen**

```bash
sed -n '250,360p' crates/rightclaw/src/codegen/pipeline.rs
```

Verify the writes target `agent-tokens.json` (~265), cloudflared config (~317), cloudflared script (~332), and `process-compose.yaml` (~358). All are `Regenerated(BotRestart)`.

- [ ] **Step 2: Replace each with `write_regenerated`**

Pattern — for each of the four blocks:

```rust
// Was:
// std::fs::write(&path, &content).map_err(|e| miette::miette!("failed to write ...: {e:#}"))?;

// Becomes:
crate::codegen::contract::write_regenerated(&path, &content)?;
```

Keep any surrounding `create_dir_all` calls — they're still needed for first-time bootstrap.

- [ ] **Step 3: Register cloudflared config path in the cross-agent registry**

If the cloudflared config path was stubbed out in Task 8, add it now. Edit `codegen/contract.rs` — update `crossagent_codegen_registry`:

```rust
// Replace the "cloudflared config path depends on tunnel config" comment with:
CodegenFile {
    kind: CodegenKind::Regenerated(HotReload::BotRestart),
    path: run.join("cloudflared-config.yml"),
},
```

Use the actual cloudflared config filename from pipeline.rs line ~317 — verify it matches.

- [ ] **Step 4: Build + run tests**

```bash
cargo test -p rightclaw codegen
cargo build --workspace
```

Expected: PASS / clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/pipeline.rs crates/rightclaw/src/codegen/contract.rs
git commit -m "refactor(codegen/pipeline): route cross-agent writes through write_regenerated"
```

### Task 17: Refactor `bot/lib.rs` policy write+apply

**Files:**
- Modify: `crates/bot/src/lib.rs:524-532`

- [ ] **Step 1: Replace the write + apply_policy pair with the contract helper**

Current block:

```rust
let policy_content = rightclaw::codegen::policy::generate_policy(
    rightclaw::runtime::MCP_HTTP_PORT,
    &network_policy,
    host_ip,
);
std::fs::write(&policy_path, &policy_content)
    .map_err(|e| miette::miette!("failed to write policy.yaml: {e:#}"))?;
tracing::info!(agent = %args.agent, "reusing existing sandbox, applying policy with host_ip={:?}", host_ip);
rightclaw::openshell::apply_policy(&sandbox, &policy_path).await?;
```

Becomes:

```rust
let policy_content = rightclaw::codegen::policy::generate_policy(
    rightclaw::runtime::MCP_HTTP_PORT,
    &network_policy,
    host_ip,
);
tracing::info!(agent = %args.agent, "reusing existing sandbox, applying policy with host_ip={:?}", host_ip);
rightclaw::codegen::contract::write_and_apply_sandbox_policy(
    &sandbox,
    &policy_path,
    &policy_content,
).await?;
```

- [ ] **Step 2: Build + run bot tests**

```bash
cargo build --workspace
cargo test -p rightclaw-bot
```

Expected: clean build, all tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "refactor(bot): route policy apply through write_and_apply_sandbox_policy"
```

---

## Phase 5 — Bot startup drift check

### Task 18: Add filesystem-policy drift WARN on bot startup

**Files:**
- Modify: `crates/bot/src/lib.rs` (after the `write_and_apply_sandbox_policy` call added in Task 17)

- [ ] **Step 1: Inspect `filesystem_policy_changed` signature**

```bash
sed -n '1295,1360p' crates/rightclaw/src/openshell.rs
```

Confirm the function signature, argument types, and how to fetch the currently-active policy for a sandbox (typically via `GetSandboxPolicyStatus` + the existing gRPC client `grpc_client`).

- [ ] **Step 2: Add the drift-check call site**

Immediately after `write_and_apply_sandbox_policy(...)` in `bot/src/lib.rs`, add:

```rust
// Drift check: filesystem section changes need sandbox migration, which is
// NOT triggered by a plain bot restart. Surface drift as a visible WARN
// pointing the operator at `rightclaw agent config`.
match rightclaw::openshell::get_active_policy(&mut grpc_client, &sandbox).await {
    Ok(active) if rightclaw::openshell::filesystem_policy_changed(&active, &policy_content) => {
        tracing::warn!(
            agent = %args.agent,
            "Filesystem policy drift detected for '{}'. Landlock rules in the running sandbox do not match policy.yaml. Run `rightclaw agent config {}` (accept defaults) to trigger sandbox migration, or `rightclaw agent backup {} --sandbox-only` first if you want a recovery point.",
            args.agent, args.agent, args.agent,
        );
    }
    Ok(_) => {
        tracing::debug!(agent = %args.agent, "filesystem policy in sync with on-disk policy.yaml");
    }
    Err(e) => {
        tracing::warn!(agent = %args.agent, "could not fetch active policy for drift check: {e:#}");
    }
}
```

If `get_active_policy` does not exist yet (only `filesystem_policy_changed` ships today), this step includes adding that helper. Check first:

```bash
rg -n "fn get_active_policy|GetSandboxPolicyStatusRequest" crates/rightclaw/src/openshell.rs
```

- [ ] **Step 3: If `get_active_policy` does not exist, add it**

Append to `crates/rightclaw/src/openshell.rs` (near `filesystem_policy_changed`):

```rust
/// Fetch the currently-active policy YAML for a sandbox via gRPC.
/// Returns the YAML string that OpenShell is enforcing right now, usable as
/// the `active_policy` argument to [`filesystem_policy_changed`].
pub async fn get_active_policy(
    grpc: &mut OpenshellServiceClient<tonic::transport::Channel>,
    sandbox: &str,
) -> miette::Result<String> {
    use crate::openshell_proto::openshell::v1::GetSandboxPolicyStatusRequest;
    let resp = grpc
        .get_sandbox_policy_status(GetSandboxPolicyStatusRequest {
            sandbox_name: sandbox.to_owned(),
        })
        .await
        .map_err(|e| miette::miette!("GetSandboxPolicyStatus RPC failed: {e:#}"))?
        .into_inner();
    resp.active_policy_yaml
        .ok_or_else(|| miette::miette!("GetSandboxPolicyStatus returned no policy yaml"))
}
```

(Adjust the field name `active_policy_yaml` to match whatever `GetSandboxPolicyStatusResponse` actually exposes — inspect the proto at `crates/rightclaw/proto/openshell/openshell.proto:490` if needed.)

- [ ] **Step 4: Build + run**

```bash
cargo build --workspace
cargo test -p rightclaw-bot
```

Expected: clean build, tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/lib.rs crates/rightclaw/src/openshell.rs
git commit -m "feat(bot): warn on filesystem policy drift at startup"
```

---

## Phase 6 — Guard tests

### Task 19: Add `regenerated_files_are_idempotent` test

**Files:**
- Create: `crates/rightclaw/src/codegen/contract_tests.rs` (if `contract.rs` exceeds 800 LoC, extract; otherwise append to `contract.rs`)

For this task we assume `contract.rs` stays under 800 LoC and the test goes into the existing `#[cfg(test)] mod tests`.

- [ ] **Step 1: Add `sha2` to dev-dependencies**

In `crates/rightclaw/Cargo.toml`, under `[dev-dependencies]`:

```toml
sha2 = "0.10"
```

- [ ] **Step 2: Add a small fixture builder for test agent layouts**

Add to `contract.rs`, inside `#[cfg(test)] mod tests`, above the new tests:

```rust
    use crate::agent::AgentDef;
    use std::path::PathBuf;

    fn minimal_agent_fixture(root: &Path, name: &str) -> AgentDef {
        let agent_path = root.join("agents").join(name);
        std::fs::create_dir_all(agent_path.join(".claude")).unwrap();
        std::fs::write(agent_path.join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(
            agent_path.join("agent.yaml"),
            "name: test\nsandbox:\n  mode: none\n",
        )
        .unwrap();
        crate::agent::discover_single_agent(&agent_path).unwrap()
    }

    fn sha256(path: &Path) -> String {
        use sha2::{Digest, Sha256};
        let bytes = std::fs::read(path).unwrap();
        let hash = Sha256::digest(&bytes);
        format!("{hash:x}")
    }
```

If `discover_single_agent` has a different name, use the closest equivalent — the goal is to produce an `AgentDef` suitable for `run_single_agent_codegen`. Check `crates/rightclaw/src/agent/discovery.rs` for the exact name.

- [ ] **Step 3: Add the idempotency test**

```rust
    #[test]
    fn regenerated_files_are_idempotent() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_owned();
        let agent = minimal_agent_fixture(&home, "t1");

        let self_exe = PathBuf::from("/usr/local/bin/rightclaw");
        crate::codegen::run_single_agent_codegen(&home, &agent, &self_exe, false).unwrap();

        let reg = codegen_registry(&agent.path);
        let first: std::collections::HashMap<_, _> = reg
            .iter()
            .filter(|f| matches!(f.kind, CodegenKind::Regenerated(_)))
            .filter(|f| f.path.is_file())
            .map(|f| (f.path.clone(), sha256(&f.path)))
            .collect();

        crate::codegen::run_single_agent_codegen(&home, &agent, &self_exe, false).unwrap();

        for (path, old_hash) in &first {
            let new_hash = sha256(path);
            assert_eq!(
                &new_hash, old_hash,
                "Regenerated file changed between codegen runs: {}",
                path.display(),
            );
        }
    }
```

- [ ] **Step 4: Run test, expect PASS (or diagnose non-determinism)**

```bash
cargo test -p rightclaw codegen::contract::tests::regenerated_files_are_idempotent
```

Expected: PASS.

If it fails, the failure identifies a non-deterministic `Regenerated` codegen. Common culprits: timestamps, random secrets. Investigate and either make the generator deterministic within a single agent instance OR move the output to `MergedRMW` / `AgentOwned` as appropriate.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/contract.rs crates/rightclaw/Cargo.toml
git commit -m "test(codegen/contract): assert Regenerated outputs are idempotent"
```

### Task 20: Add `agent_owned_files_preserved` test

**Files:**
- Modify: `crates/rightclaw/src/codegen/contract.rs` (tests module)

- [ ] **Step 1: Add the test**

```rust
    #[test]
    fn agent_owned_files_preserved_across_codegen() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_owned();
        let agent = minimal_agent_fixture(&home, "t2");

        // Seed settings.local.json with a marker agent-written value.
        let settings_local = agent.path.join(".claude/settings.local.json");
        std::fs::create_dir_all(settings_local.parent().unwrap()).unwrap();
        std::fs::write(&settings_local, r#"{"__AGENT__":true}"#).unwrap();

        let self_exe = PathBuf::from("/usr/local/bin/rightclaw");
        crate::codegen::run_single_agent_codegen(&home, &agent, &self_exe, false).unwrap();

        assert_eq!(
            std::fs::read_to_string(&settings_local).unwrap(),
            r#"{"__AGENT__":true}"#,
            "AgentOwned file settings.local.json was overwritten by codegen",
        );
    }
```

- [ ] **Step 2: Run + verify**

```bash
cargo test -p rightclaw codegen::contract::tests::agent_owned_files_preserved_across_codegen
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/contract.rs
git commit -m "test(codegen/contract): assert AgentOwned files not overwritten"
```

### Task 21: Add `merged_rmw_preserves_unknown_fields` test

**Files:**
- Modify: `crates/rightclaw/src/codegen/contract.rs` (tests module)

- [ ] **Step 1: Add the test**

```rust
    #[test]
    fn merged_rmw_preserves_unknown_fields_in_claude_json() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_owned();
        let agent = minimal_agent_fixture(&home, "t3");

        // Seed .claude.json with an extra field codegen does not own.
        let claude_json = agent.path.join(".claude.json");
        std::fs::write(
            &claude_json,
            r#"{"customField":"preserve-me","hasCompletedOnboarding":false}"#,
        )
        .unwrap();

        let self_exe = PathBuf::from("/usr/local/bin/rightclaw");
        crate::codegen::run_single_agent_codegen(&home, &agent, &self_exe, false).unwrap();

        let content = std::fs::read_to_string(&claude_json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["customField"], "preserve-me",
            "MergedRMW must preserve unknown fields"
        );
        // hasCompletedOnboarding may have been forced to true by codegen
        // (its .or_insert only triggers when key is absent). Do not assert
        // a specific value — only that the key is present as a boolean.
        assert!(parsed["hasCompletedOnboarding"].is_boolean());
    }
```

- [ ] **Step 2: Run + verify**

```bash
cargo test -p rightclaw codegen::contract::tests::merged_rmw_preserves_unknown_fields_in_claude_json
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/contract.rs
git commit -m "test(codegen/contract): assert MergedRMW preserves unknown fields"
```

### Task 22: Add `registry_covers_all_per_agent_writes` test

**Files:**
- Modify: `crates/rightclaw/src/codegen/contract.rs` (tests module)

- [ ] **Step 1: Add the test with documented `KNOWN_EXCEPTIONS`**

```rust
    /// Files that codegen (or its side effects) may create but that are
    /// intentionally outside the codegen contract. Keep this list tight —
    /// every entry is a gap in the upgrade story.
    const KNOWN_EXCEPTIONS: &[&str] = &[
        ".git",
        "data.db",
        "data.db-shm",
        "data.db-wal",
        "oauth-callback.sock",
        ".claude/shell-snapshots",
        ".claude/.credentials.json", // symlink, target owned by host
        "inbox",
        "outbox",
        "tmp",
    ];

    fn walk_files_rel(root: &Path, base: &Path, out: &mut Vec<PathBuf>) {
        if !root.exists() {
            return;
        }
        for entry in std::fs::read_dir(root).unwrap().flatten() {
            let p = entry.path();
            let rel = p.strip_prefix(base).unwrap().to_owned();
            // Short-circuit at known exception dirs.
            if KNOWN_EXCEPTIONS
                .iter()
                .any(|x| rel.starts_with(x))
            {
                continue;
            }
            if p.is_dir() {
                walk_files_rel(&p, base, out);
            } else if p.is_file() || p.is_symlink() {
                out.push(rel);
            }
        }
    }

    #[test]
    fn registry_covers_all_per_agent_writes() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_owned();
        let agent = minimal_agent_fixture(&home, "t4");

        let self_exe = PathBuf::from("/usr/local/bin/rightclaw");
        crate::codegen::run_single_agent_codegen(&home, &agent, &self_exe, false).unwrap();

        let reg_paths: std::collections::HashSet<PathBuf> = codegen_registry(&agent.path)
            .into_iter()
            .map(|f| {
                f.path.strip_prefix(&agent.path).unwrap().to_owned()
            })
            .collect();

        let mut found = Vec::new();
        walk_files_rel(&agent.path, &agent.path, &mut found);

        // Files under a registered directory (e.g. .claude/skills/...) count as
        // covered by the parent entry — filter them out by "starts_with any
        // registry path".
        let uncovered: Vec<_> = found
            .into_iter()
            .filter(|rel| !reg_paths.iter().any(|r| rel == r || rel.starts_with(r)))
            .collect();

        assert!(
            uncovered.is_empty(),
            "files produced by codegen not in registry or KNOWN_EXCEPTIONS: {:#?}",
            uncovered,
        );
    }
```

- [ ] **Step 2: Run + triage any uncovered files**

```bash
cargo test -p rightclaw codegen::contract::tests::registry_covers_all_per_agent_writes
```

Expected on first run: possibly FAIL with a list of missing files. For each file in the failure output, decide:
- It's a real codegen output → add a `CodegenFile` entry in `codegen_registry()`.
- It's a side-effect that should be ignored → add to `KNOWN_EXCEPTIONS` with a justification comment.

Iterate until the test passes.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/contract.rs
git commit -m "test(codegen/contract): assert registry covers all per-agent writes"
```

---

## Phase 7 — Integration test with live OpenShell

### Task 23: Add `generated_policy_applies_to_live_openshell`

**Files:**
- Create: `crates/rightclaw/tests/policy_apply.rs`

- [ ] **Step 1: Create the test file**

Create `crates/rightclaw/tests/policy_apply.rs`:

```rust
//! Integration test: generated OpenShell policy must apply cleanly to a live
//! sandbox with no deprecation warnings in the agent container.
//!
//! This catches (a) future OpenShell deprecations, and (b) syntax regressions
//! in the codegen template.

use rightclaw::agent::types::NetworkPolicy;
use rightclaw::codegen::policy::generate_policy;
use rightclaw::test_support::{acquire_sandbox_slot, TestSandbox};

#[tokio::test]
async fn generated_policy_applies_to_live_openshell_permissive() {
    let _slot = acquire_sandbox_slot().await;
    let sandbox = TestSandbox::create("policy-apply-permissive").await.unwrap();

    let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);

    // Write policy to a temp file and apply it via the contract helper.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("policy.yaml");
    rightclaw::codegen::contract::write_and_apply_sandbox_policy(
        sandbox.name(),
        &path,
        &policy,
    )
    .await
    .expect("policy must apply cleanly");

    // Exec a trivial command inside the sandbox to force L7 proxy to handle
    // a request; the proxy emits the deprecated-field WARN per request.
    let _ = sandbox.exec(&["true"]).await;

    // Scan the agent container log for deprecation warnings.
    let logs = sandbox.agent_container_logs().await.unwrap_or_default();
    for line in logs.lines() {
        assert!(
            !line.to_ascii_lowercase().contains("deprecated"),
            "deprecated-field WARN in sandbox logs: {line}",
        );
    }
}

#[tokio::test]
async fn generated_policy_applies_to_live_openshell_restrictive() {
    let _slot = acquire_sandbox_slot().await;
    let sandbox = TestSandbox::create("policy-apply-restrictive").await.unwrap();

    let policy = generate_policy(8100, &NetworkPolicy::Restrictive, None);

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("policy.yaml");
    rightclaw::codegen::contract::write_and_apply_sandbox_policy(
        sandbox.name(),
        &path,
        &policy,
    )
    .await
    .expect("restrictive policy must apply cleanly");

    let _ = sandbox.exec(&["true"]).await;

    let logs = sandbox.agent_container_logs().await.unwrap_or_default();
    for line in logs.lines() {
        assert!(
            !line.to_ascii_lowercase().contains("deprecated"),
            "deprecated-field WARN in sandbox logs: {line}",
        );
    }
}
```

If `TestSandbox` does not expose `agent_container_logs`, add that method to `crates/rightclaw/src/test_support.rs`. Use the OpenShell CLI/gRPC path that returns agent-container logs; likely a wrapper over `kubectl logs ... -c agent` executed via the OpenShell host.

- [ ] **Step 2: Ensure `test-support` feature is enabled for the test crate**

In `crates/rightclaw/Cargo.toml`, under `[dev-dependencies]` or `[[test]]`, make sure the test can access `test_support`:

```toml
[dev-dependencies]
# ... existing ...
tempfile = "3"
tokio = { version = "1", features = ["full", "macros"] }

[features]
test-support = []  # already present; this test runs in-crate so it's free
```

(If `test-support` is already wired, no changes needed.)

- [ ] **Step 3: Run the test against a live OpenShell**

```bash
cargo test -p rightclaw --test policy_apply
```

Expected: both tests PASS. On a fresh dev machine this depends on OpenShell running on the host per ARCHITECTURE.md conventions.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/tests/policy_apply.rs crates/rightclaw/src/test_support.rs crates/rightclaw/Cargo.toml
git commit -m "test(integration): generated policy applies cleanly to live sandbox"
```

---

## Phase 8 — Documentation

### Task 24: Insert new `Upgrade & Migration Model` section in ARCHITECTURE.md

**Files:**
- Modify: `ARCHITECTURE.md` (add new section after `## SQLite Rules`)

- [ ] **Step 1: Insert the new top-level section**

Open `ARCHITECTURE.md`. Find the `## SQLite Rules` section. Immediately after its last subsection ends (before `## Integration Tests Using Live Sandboxes`), insert the full appendix copy-paste from the spec:

```markdown
## Upgrade & Migration Model

Every change that touches codegen, sandbox config, or on-disk state must be
deployable to already-running production agents. Manual migration steps,
`rightclaw agent init`, or sandbox recreation are NEVER acceptable as upgrade
paths.

### Codegen categories

Every per-agent codegen output belongs to exactly one category:

| Category | Semantics | Examples |
|---|---|---|
| `Regenerated(BotRestart)` | Unconditional overwrite every bot start. Takes effect on next CC invocation. | settings.json, mcp.json, schemas, system-prompt.md |
| `Regenerated(SandboxPolicyApply)` | Overwrite + `openshell policy set --wait`. Network-only. | policy.yaml (network section) |
| `Regenerated(SandboxRecreate)` | Overwrite + triggers sandbox migration. Filesystem/landlock and other boot-time-only changes. | policy.yaml (filesystem section) |
| `MergedRMW` | Read, merge, write. Preserves unknown fields. | .claude.json, agent.yaml (secret injection) |
| `AgentOwned` | Created by init. Never touched again. | TOOLS.md, AGENTS.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md, settings.local.json |

Cross-agent outputs (process-compose.yaml, agent-tokens.json, cloudflared
config) are all `Regenerated(BotRestart)` — reread on `rightclaw up`.

`policy.yaml` mixes a hot-reloadable network section and a recreate-only
filesystem section. It's registered as the stricter `Regenerated(SandboxRecreate)`;
runtime downgrades to hot-reload when the filesystem section is unchanged.

### Helper API

`crates/rightclaw/src/codegen/contract.rs` provides the only sanctioned writers:

- `write_regenerated(path, content)` — all `Regenerated` outputs except
  `SandboxPolicyApply`.
- `write_merged_rmw(path, merge_fn)` — read-modify-write with unknown-field
  preservation.
- `write_agent_owned(path, initial)` — no-op if file exists.
- `write_and_apply_sandbox_policy(sandbox, path, content).await` — the ONLY
  way to update policy for a running sandbox. Writes + applies atomically.

Direct `std::fs::write` inside codegen modules is a review-blocking defect.

### Rules for adding a new codegen output

1. Pick a category. Add a `CodegenFile` entry to the matching registry
   (`codegen_registry()` or `crossagent_codegen_registry()`).
2. Use the matching helper. No bare `std::fs::write`.
3. Run `cargo test registry_covers_all_per_agent_writes` to verify the
   registry is complete.
4. If `Regenerated(SandboxRecreate)` — exercise the migration path manually
   and update `Sandbox migration` subsection under Data Flow if the trigger
   condition changed.
5. If the new output is policy-related, apply via
   `write_and_apply_sandbox_policy` only. Adding a new network endpoint is
   fine; adding a new filesystem rule requires `SandboxRecreate` treatment.
6. Never require `rightclaw agent init` for existing agents to adopt the
   change. They upgrade via `rightclaw restart <agent>`.

### Upgrade flow for a typical codegen change

1. Code change merged.
2. User runs `rightclaw restart <agent>` (or the bot restarts naturally via
   process-compose `on_failure`).
3. `run_single_agent_codegen` rewrites every `Regenerated` file.
4. Hot-reload machinery applies per category:
   - `BotRestart`: nothing extra — CC picks up the new file on next invocation.
   - `SandboxPolicyApply`: `write_and_apply_sandbox_policy` hot-reloads via
     `openshell policy set --wait`.
   - `SandboxRecreate`: bot startup compares active vs on-disk policy via
     `filesystem_policy_changed`. On drift, logs a WARN telling the operator
     to run `rightclaw agent config <agent>`, which invokes
     `maybe_migrate_sandbox`. No automatic migration — it's disruptive and
     requires operator consent.
5. For `BotRestart` / `SandboxPolicyApply`: zero manual steps.
6. For `SandboxRecreate`: one follow-up command from the operator.

### Non-goals

- Agent-owned content (`AgentOwned` files) — agent property; codegen never
  mutates them.
- OpenShell server upgrades — covered by `OpenShell Integration Conventions`.
- SQLite schema — handled by `rusqlite_migration` (see `SQLite Rules`).

### Cross-references

- `CLAUDE.md` → `Upgrade-friendly design`, `Never delete sandboxes for
  recovery`, `Self-healing platform` — conventions this model implements.
- Data Flow → `Sandbox migration (filesystem policy change)` — the migration
  flow used by `Regenerated(SandboxRecreate)`.
```

- [ ] **Step 2: Update `Configuration Hierarchy` table in ARCHITECTURE.md**

Locate the `### Configuration Hierarchy` subsection under `## Data Flow`. For each row in the table, add a new rightmost `Category` column with the matching kind (e.g. `Regenerated(BotRestart)`, `MergedRMW`, `AgentOwned`). Append a footnote: `See Upgrade & Migration Model for definitions.`

- [ ] **Step 3: Refresh `OpenShell Policy Gotchas`**

In the `## OpenShell Policy Gotchas` section, replace the bullet starting with ``` `tls: terminate` is **required** on all HTTPS endpoints (OpenShell v0.0.23). ...``` with:

```
- `tls: terminate` / `tls: passthrough` are **deprecated** in OpenShell v0.0.28+ (PR #544). TLS termination is now automatic by peeking ClientHello bytes. The codegen no longer emits these fields. Use `tls: skip` only when you explicitly need a raw tunnel (mTLS client-cert, unusual protocols).
```

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs(architecture): add Upgrade & Migration Model section"
```

---

## Phase 9 — Verification

### Task 25: Full workspace build + test + manual verification

**Files:**
- None (verification only)

- [ ] **Step 1: Full build + lint**

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean build, no clippy warnings.

- [ ] **Step 2: Full test suite**

```bash
cargo test --workspace
```

Expected: all tests pass, including the new contract tests, integration policy_apply tests, and existing suites.

- [ ] **Step 3: Manual verification against a live dev environment**

```bash
# Restart one agent and verify no deprecation warnings:
rightclaw restart right

# Wait ~5s for sandbox to pick up the new policy, then check logs:
docker exec openshell-cluster-openshell \
  kubectl -n openshell logs rightclaw-right -c agent --tail=200 \
  | grep -i 'deprecated'
# expect: no matches

# Verify the on-disk policy has no deprecated fields:
grep -c 'tls: terminate' ~/.rightclaw/agents/*/policy.yaml
# expect: 0 for all agents

# Verify MCP is still reachable from sandbox:
tail -n 20 ~/.rightclaw/logs/streams/*.ndjson | grep mcp_servers
# expect: mcp_servers with status "connected"
```

- [ ] **Step 4: Final commit (only if verification reveals any last docs/log tweaks)**

If the verification surfaced any small fixes (typo in WARN message, missing registry entry, etc.), amend those into small focused commits. Otherwise, no action.

- [ ] **Step 5: Push PR-ready branch**

```bash
git log --oneline -30
```

Review the commit chain makes sense top-to-bottom. Optionally squash trivial fixups via `git rebase -i` (only if asked by the user).

---

## Self-Review Checklist

**Spec coverage:**
- `tls: terminate` removal — Tasks 1-2 ✓
- `codegen::contract` module with helpers — Tasks 3-7 ✓
- Per-agent + cross-agent registries — Tasks 8, 16 ✓
- Call-site refactor across pipeline / claude_json / mcp_config / skills — Tasks 9-16 ✓
- Bot-side `write_and_apply_sandbox_policy` — Task 17 ✓
- Bot startup drift check — Task 18 ✓
- Guard tests (idempotent, agent-owned, merged-rmw, registry coverage) — Tasks 19-22 ✓
- Integration test against live sandbox — Task 23 ✓
- ARCHITECTURE.md section + related edits — Task 24 ✓
- Full verification — Task 25 ✓

**Type consistency:**
- `write_regenerated(path, content)` — used consistently (Tasks 4, 9-16).
- `write_merged_rmw(path, fn)` with `FnOnce(Option<&str>) -> Result<String>` — consistent (Tasks 6, 11, 13, 14).
- `write_agent_owned(path, initial)` — consistent (Tasks 5, 10).
- `write_and_apply_sandbox_policy(sandbox, path, content).await` — consistent (Tasks 7, 17, 23).
- `CodegenKind::{Regenerated(HotReload), MergedRMW, AgentOwned}` — consistent.
- `HotReload::{BotRestart, SandboxPolicyApply, SandboxRecreate}` — consistent.

**Placeholder scan:** No `TBD` / `TODO` / `implement later` / `add appropriate handling` / `similar to Task N without code` remain.

**Known assumptions to validate during execution:**
- Exact field name on `GetSandboxPolicyStatusResponse` for the active-policy YAML (Task 18 Step 3) — check proto.
- Exact function name for agent discovery used in fixture (`discover_single_agent` vs alternative) — check `crates/rightclaw/src/agent/discovery.rs`.
- Skills embedded-file API (`file.contents()` return type) in `skills.rs` — may need adjustment if the crate uses raw bytes.
- `TestSandbox::agent_container_logs` may not exist; add it to `test_support.rs` if missing.
