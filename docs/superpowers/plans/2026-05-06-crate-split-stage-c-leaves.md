# Crate Split Stage C — Extract Leaves (`right-mcp`, `right-codegen`, `right-memory`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the three hot-edit domains (`mcp/`, `codegen/`, `memory/`) from `right-agent` into independent leaf crates so that an edit to any one of them only rebuilds that crate plus thin orchestrators downstream — not the 30k-LoC `right-agent` god crate.

**Architecture:** Three new crates: `right-mcp` (mcp/* + auth_token helpers parked there at Stage A), `right-codegen` (codegen/* — depends on right-mcp), `right-memory` (memory/* sans the SQL plumbing already in right-db). Each receives the contents of the corresponding subdir from `right-agent::src/`; `right-agent::{mcp, codegen, memory}` becomes thin re-export modules so external `right_agent::*` callsites and internal `crate::*` paths keep working. Bot, CLI, and tests get bulk-rewritten to call the new crates directly when convenient — final cleanup happens at Stage F.

**Tech Stack:** Rust 2024, Cargo workspace, existing deps (`rmcp`, `hyper`, `minijinja`, `include_dir`, `reqwest`). Spec at `docs/superpowers/specs/2026-05-06-crate-split-design.md` (commit `16429d54`). All commands run via `devenv shell -- <cmd>` because the project's CLAUDE.md mandates it when `devenv.nix` exists at repo root.

**Pre-existing context:**
- This plan **assumes Stages A and B are merged**. `right-db` owns SQLite plumbing (open_connection, MIGRATIONS); `right-core` owns error/ui/config/openshell/proto/platform_store/stt/test_support and `IDLE_THRESHOLD_{SECS,MIN}` in `right_core::time_constants`.
- After Stage A, `right-agent::memory::mod.rs` is a slim shell: `pub mod {circuit, classify, error, guard, hindsight, prefetch, resilient, retain_queue, status}`, an `alert_types` module, `pub use` re-exports for `ErrorKind` / `MemoryError` / `ResilientError` / `ResilientHindsight` / `MemoryStatus`, and `pub use right_db::{open_connection, open_db};`. The `migrations` module and `store.rs` are gone.
- After Stage A, `right-agent::mcp::credentials` hosts `save_auth_token`, `get_auth_token`, `delete_auth_token` (moved out of `memory::store`). They still depend on the `auth_tokens` table whose migration is in `right-db`.
- `right-agent::mcp::mod.rs` declares 7 submodules (`credentials`, `internal_client`, `oauth`, `proxy`, `reconnect`, `refresh`, `tool_error`), the `PROTECTED_MCP_SERVER` constant, and the `generate_agent_secret` / `derive_token` functions. `crates/right-agent/src/codegen/{pipeline,mcp_instructions}.rs` imports those last two functions and `credentials::McpServerEntry` (4 references total) — this is the cross-edge `right-codegen → right-mcp` the spec keeps.
- `right-agent::codegen::skills.rs:10` currently imports `crate::cron_spec::{IDLE_THRESHOLD_MIN, IDLE_THRESHOLD_SECS}`. After Stage B those constants live in `right_core::time_constants`; after Stage C, codegen lives in a separate crate, so this import switches to `right_core::time_constants::{...}` directly.
- `right-agent::codegen::pipeline.rs` calls `crate::platform_store::*`. After Stage B, `platform_store` lives in `right-core` (re-exported from `right-agent`). After Stage C, codegen is its own crate, so the import switches to `right_core::platform_store::*`.
- Module sizes (post-Stage-A approximations; subtract 31 LoC for `memory::store.rs` and ~1000 for `memory::migrations.rs` already moved):
  - `mcp/`: ≈4.1k LoC across 7 files (`credentials` 871, `internal_client` 314, `oauth` 1298, `proxy` 598, `reconnect` 538, `refresh` 319, `tool_error` 144 + a small `mod.rs`).
  - `codegen/`: ≈4.5k LoC across 17 files (`pipeline`, `contract`, `policy`, `skills`, `mcp_config`, `mcp_instructions`, etc., plus tests).
  - `memory/` (post Stage A): ≈2.5k LoC across 9 files (`circuit`, `classify`, `error`, `guard`, `hindsight`, `prefetch`, `resilient`, `retain_queue`, `status`) + `mod.rs`.
- Workspace `Cargo.toml` lists 5 members after Stage B (`right-agent`, `right-core`, `right-db`, `right`, `bot`). Stage C adds three more, ending at 8.

**Verification commands** (run from repo root):
- Build: `devenv shell -- cargo build --workspace`
- Test: `devenv shell -- cargo test --workspace`
- Lint: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
- Single-package check: `devenv shell -- cargo check -p <name>`

---

## Sub-stage C1: extract `right-mcp`

### Task 1: Scaffold the `right-mcp` crate

**Files:**
- Create: `crates/right-mcp/Cargo.toml`
- Create: `crates/right-mcp/src/lib.rs`

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "right-mcp"
version.workspace = true
edition.workspace = true

[dependencies]
right-core = { path = "../right-core" }
right-db = { path = "../right-db" }

# Re-used from the workspace; mirrors what right-agent's `mcp/` currently uses.
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
miette = { workspace = true }
tracing = { workspace = true }
tokio = { workspace = true }
tokio-util = { workspace = true }
reqwest = { workspace = true }
url = { workspace = true }
rusqlite = { workspace = true }
chrono = { workspace = true }
sha2 = { workspace = true }
hmac = { workspace = true }
rand = { workspace = true }
base64 = { workspace = true }
subtle = { workspace = true }
rmcp = { workspace = true }
hyper = { workspace = true }
hyper-util = { workspace = true }
http = { workspace = true }
http-body-util = { workspace = true }
sse-stream = { workspace = true }
futures = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
right-core = { path = "../right-core", features = ["test-support"] }
```

(Pruning of unused deps happens at Stage F. Mirror what `right-agent` currently pulls in for the moving subset.)

- [ ] **Step 2: Create stub `lib.rs`**

```rust
//! MCP (Model Context Protocol) primitives for `right`.
//!
//! Aggregator backend types, proxy, reconnect and refresh logic,
//! credentials (server entries + OAuth + per-agent auth tokens),
//! token derivation (`generate_agent_secret`, `derive_token`).

pub mod credentials;
pub mod internal_client;
pub mod oauth;
pub mod proxy;
pub mod reconnect;
pub mod refresh;
pub mod tool_error;

/// Name of the built-in MCP server that right-agent manages.
/// Protected from `/mcp remove` — required for core functionality.
pub const PROTECTED_MCP_SERVER: &str = "right";

pub use mod_helpers::{derive_token, generate_agent_secret};

mod mod_helpers; // private file populated in Task 3 with the moved fn bodies.
```

(We split off the inline `generate_agent_secret` / `derive_token` into a private `mod_helpers.rs` to keep `lib.rs` tidy. Existing tests for those functions move to `mod_helpers.rs` too.)

- [ ] **Step 3: Verify the crate is wired**

Run: `devenv shell -- cargo check --manifest-path crates/right-mcp/Cargo.toml`
Expected: fails because the workspace doesn't yet know the crate. That's fine — Task 2 wires it in. The submodule files don't yet exist either; Task 3 fills them.

- [ ] **Step 4: Commit**

```bash
git add crates/right-mcp/
git commit -m "feat(right-mcp): scaffold new MCP primitives crate"
```

### Task 2: Add `right-mcp` to workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add the member**

Edit the `[workspace] members = [...]` line to insert `"crates/right-mcp"`:

```toml
[workspace]
members = ["crates/right-agent", "crates/right-core", "crates/right-db", "crates/right-mcp", "crates/right", "crates/bot"]
resolver = "3"
```

- [ ] **Step 2: Verify the workspace recognises the empty crate**

Run: `devenv shell -- cargo build -p right-mcp`
Expected: fails — `mod_helpers` and submodule files don't exist yet. That's expected and fixed by Task 3.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat(workspace): register right-mcp crate"
```

### Task 3: Move `mcp/` contents to `right-mcp`

**Files:**
- Move: `crates/right-agent/src/mcp/{credentials,internal_client,oauth,proxy,reconnect,refresh,tool_error}.rs` → `crates/right-mcp/src/`
- Create: `crates/right-mcp/src/mod_helpers.rs` (extracted from `right-agent::mcp::mod.rs`)
- Modify: `crates/right-agent/src/mcp/mod.rs` (becomes a thin shim)
- Modify: `crates/right-mcp/src/credentials.rs` (auth-token helpers stay; `crate::mcp::credentials` paths inside the file resolve naturally because the file is now at crate root)

- [ ] **Step 1: Move the seven submodule files**

```bash
git mv crates/right-agent/src/mcp/credentials.rs crates/right-mcp/src/credentials.rs
git mv crates/right-agent/src/mcp/internal_client.rs crates/right-mcp/src/internal_client.rs
git mv crates/right-agent/src/mcp/oauth.rs crates/right-mcp/src/oauth.rs
git mv crates/right-agent/src/mcp/proxy.rs crates/right-mcp/src/proxy.rs
git mv crates/right-agent/src/mcp/reconnect.rs crates/right-mcp/src/reconnect.rs
git mv crates/right-agent/src/mcp/refresh.rs crates/right-mcp/src/refresh.rs
git mv crates/right-agent/src/mcp/tool_error.rs crates/right-mcp/src/tool_error.rs
```

If any of those files contains `#[path = "credentials_auth_token_tests.rs"] mod auth_token_tests;` (created at Stage A) and the test file lives next to it, also move the tests file:

```bash
git mv crates/right-agent/src/mcp/credentials_auth_token_tests.rs crates/right-mcp/src/credentials_auth_token_tests.rs 2>/dev/null || true
```

- [ ] **Step 2: Extract `generate_agent_secret` / `derive_token` and the `tests` block from the old `mcp/mod.rs` into `right-mcp/src/mod_helpers.rs`**

Open `crates/right-agent/src/mcp/mod.rs`. Copy lines 13-108 (everything except the `pub mod` declarations and the `PROTECTED_MCP_SERVER` constant) into `crates/right-mcp/src/mod_helpers.rs`. The body is:

```rust
/// Generate a random 32-byte agent secret, base64url-encoded (no padding).
pub fn generate_agent_secret() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Derive a Bearer token from an agent secret using HMAC-SHA256.
pub fn derive_token(secret_b64: &str, label: &str) -> miette::Result<String> {
    use base64::Engine as _;
    use hmac::{Hmac, KeyInit as _, Mac as _};
    use sha2::Sha256;

    let secret_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(secret_b64)
        .map_err(|e| miette::miette!("invalid agent secret (bad base64url): {e:#}"))?;

    let mut mac = Hmac::<Sha256>::new_from_slice(&secret_bytes)
        .map_err(|e| miette::miette!("HMAC init failed: {e:#}"))?;
    mac.update(label.as_bytes());
    let result = mac.finalize().into_bytes();

    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_agent_secret_is_43_chars() {
        let secret = generate_agent_secret();
        assert_eq!(secret.len(), 43);
    }

    #[test]
    fn generate_agent_secret_unique() {
        let a = generate_agent_secret();
        let b = generate_agent_secret();
        assert_ne!(a, b);
    }

    #[test]
    fn derive_token_deterministic() {
        let secret = generate_agent_secret();
        let t1 = derive_token(&secret, "right-mcp").unwrap();
        let t2 = derive_token(&secret, "right-mcp").unwrap();
        assert_eq!(t1, t2);
    }

    #[test]
    fn derive_token_different_labels_differ() {
        let secret = generate_agent_secret();
        let t1 = derive_token(&secret, "right-mcp").unwrap();
        let t2 = derive_token(&secret, "right-cron").unwrap();
        assert_ne!(t1, t2);
    }

    #[test]
    fn derive_token_is_43_chars() {
        let secret = generate_agent_secret();
        let token = derive_token(&secret, "right-mcp").unwrap();
        assert_eq!(token.len(), 43);
    }

    #[test]
    fn derive_token_rejects_invalid_base64() {
        let result = derive_token("not!valid!base64", "right-mcp");
        assert!(result.is_err());
    }

    #[test]
    fn derive_token_for_tg_webhook_matches_telegram_secret_format() {
        let secret = generate_agent_secret();
        let webhook_secret = derive_token(&secret, "tg-webhook").unwrap();
        assert!(
            !webhook_secret.is_empty() && webhook_secret.len() <= 256,
            "len out of Telegram's 1-256 range: {}",
            webhook_secret.len()
        );
        assert!(
            webhook_secret
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "char outside Telegram's [A-Za-z0-9_-]: {webhook_secret}"
        );
    }
}
```

- [ ] **Step 3: Convert `right-agent/src/mcp/mod.rs` into a re-export shim**

Replace the entire contents of `crates/right-agent/src/mcp/mod.rs` with:

```rust
//! Re-export shim. Real definitions live in the `right-mcp` crate.
//! Removed in Stage F.

pub use right_mcp::*;
```

- [ ] **Step 4: Update internal `crate::*` paths inside the moved files**

The moved files contain `use crate::mcp::*` and `use crate::*` patterns that, before the move, referenced `right-agent`'s own modules. After the move, those paths must resolve from inside `right-mcp`:

For the moved `crates/right-mcp/src/*.rs` files:

```bash
# crate::mcp::X → crate::X (since they now live at crate root of right-mcp)
devenv shell -- rg -l 'crate::mcp::' crates/right-mcp/src \
  | xargs sed -i.bak 's|crate::mcp::|crate::|g'

# crate::error::display_error_chain → right_core::error::display_error_chain
devenv shell -- rg -l 'crate::error::' crates/right-mcp/src \
  | xargs sed -i.bak 's|crate::error::|right_core::error::|g' 2>/dev/null

# crate::config:: → right_core::config::
devenv shell -- rg -l 'crate::config::' crates/right-mcp/src \
  | xargs sed -i.bak 's|crate::config::|right_core::config::|g' 2>/dev/null

# Anything else that used to reach across into right-agent is a problem; flag and fix manually.
devenv shell -- rg -l 'crate::ui::|crate::openshell\b|crate::test_support|crate::test_cleanup' crates/right-mcp/src \
  | xargs sed -i.bak 's|crate::ui::|right_core::ui::|g; s|crate::openshell|right_core::openshell|g; s|crate::test_support|right_core::test_support|g; s|crate::test_cleanup|right_core::test_cleanup|g' 2>/dev/null

# Migrations registry: crate::memory::migrations::MIGRATIONS → right_db::MIGRATIONS
devenv shell -- rg -l 'crate::memory::migrations::MIGRATIONS' crates/right-mcp/src \
  | xargs sed -i.bak 's|crate::memory::migrations::MIGRATIONS|right_db::MIGRATIONS|g' 2>/dev/null

# right_db open helpers
devenv shell -- rg -l 'crate::memory::open_connection|crate::memory::open_db' crates/right-mcp/src \
  | xargs sed -i.bak 's|crate::memory::open_connection|right_db::open_connection|g; s|crate::memory::open_db|right_db::open_db|g' 2>/dev/null

devenv shell -- find crates/right-mcp/src -name '*.bak' -delete
```

- [ ] **Step 5: Verify `right-mcp` builds**

Run: `devenv shell -- cargo build -p right-mcp`
Expected: succeeds. If any `crate::*` path remains unresolved, grep and fix manually:

```bash
devenv shell -- cargo check -p right-mcp 2>&1 | rg 'cannot find|unresolved' | head -10
```

- [ ] **Step 6: Verify `right-agent` still builds via its re-export shim**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds. Internal `crate::mcp::*` callers (e.g. `agent/destroy.rs`, `cron_spec.rs`, `usage/insert.rs`) reach `right_mcp::*` through the shim.

- [ ] **Step 7: Run the moved tests**

Run: `devenv shell -- cargo test -p right-mcp`
Expected: passes. Tests for `derive_token`, `generate_agent_secret`, plus the auth-token tests if `credentials_auth_token_tests.rs` was moved.

- [ ] **Step 8: Commit**

```bash
git add crates/right-mcp crates/right-agent/src/mcp Cargo.lock
git commit -m "refactor(right-mcp): extract mcp subsystem from right-agent"
```

---

## Sub-stage C2: extract `right-codegen`

### Task 4: Scaffold the `right-codegen` crate

**Files:**
- Create: `crates/right-codegen/Cargo.toml`
- Create: `crates/right-codegen/src/lib.rs`

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "right-codegen"
version.workspace = true
edition.workspace = true

[dependencies]
right-core = { path = "../right-core" }
right-db = { path = "../right-db" }
right-mcp = { path = "../right-mcp" }

serde = { workspace = true }
serde_json = { workspace = true }
serde-saphyr = { workspace = true }
thiserror = { workspace = true }
miette = { workspace = true }
tracing = { workspace = true }
minijinja = { workspace = true }
include_dir = { workspace = true }
walkdir = { workspace = true }
sha2 = { workspace = true }
chrono = { workspace = true }
url = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
right-core = { path = "../right-core", features = ["test-support"] }
```

- [ ] **Step 2: Create stub `lib.rs`**

```rust
//! Per-agent codegen for `right`.
//!
//! Generates `.claude/settings.json`, `.mcp.json`, system-prompt files,
//! `process-compose.yaml`, cloudflared config, sandbox policy.yaml,
//! and skill bundles. Single source of truth for what gets deployed
//! to a sandbox by `right up` / `right restart`.

pub mod agent_def;
pub mod claude_json;
pub mod cloudflared;
pub mod contract;
pub mod mcp_config;
pub mod mcp_instructions;
pub mod pipeline;
pub mod plugin;
pub mod policy;
pub mod process_compose;
pub mod settings;
pub mod skills;
pub mod telegram;
```

- [ ] **Step 3: Verify wiring**

Run: `devenv shell -- cargo check --manifest-path crates/right-codegen/Cargo.toml`
Expected: fails — workspace doesn't know it yet, and submodule files don't exist. Fixed by Task 5 + 6.

- [ ] **Step 4: Commit**

```bash
git add crates/right-codegen/
git commit -m "feat(right-codegen): scaffold new codegen crate"
```

### Task 5: Add `right-codegen` to workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add member**

```toml
[workspace]
members = ["crates/right-agent", "crates/right-codegen", "crates/right-core", "crates/right-db", "crates/right-mcp", "crates/right", "crates/bot"]
resolver = "3"
```

- [ ] **Step 2: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat(workspace): register right-codegen crate"
```

### Task 6: Move `codegen/` contents to `right-codegen`

**Files:**
- Move: `crates/right-agent/src/codegen/{agent_def,agent_def_tests,claude_json,cloudflared,cloudflared_tests,contract,mcp_config,mcp_instructions,pipeline,plugin,policy,process_compose,process_compose_tests,settings,settings_tests,skills,telegram}.rs` → `crates/right-codegen/src/`
- Modify: `crates/right-agent/src/codegen/mod.rs` (becomes shim)
- Modify: each moved file (rewrite imports)

- [ ] **Step 1: Move all 17 codegen files**

```bash
for f in agent_def agent_def_tests claude_json cloudflared cloudflared_tests contract \
         mcp_config mcp_instructions pipeline plugin policy process_compose \
         process_compose_tests settings settings_tests skills telegram; do
  git mv crates/right-agent/src/codegen/${f}.rs crates/right-codegen/src/${f}.rs
done
```

- [ ] **Step 2: If `crates/right-agent/src/codegen/` contains a templates subdir or assets used via `include_str!` / `include_dir!`, move them too**

Run:

```bash
ls crates/right-agent/src/codegen
```

If there are non-`.rs` files (e.g. `templates/`), move them with `git mv` to `crates/right-codegen/src/`. Verify after move that any `include_str!("templates/...")` calls resolve relative to the new location.

- [ ] **Step 3: Convert `right-agent/src/codegen/mod.rs` into a re-export shim**

Replace the entire contents of `crates/right-agent/src/codegen/mod.rs` with:

```rust
//! Re-export shim. Real definitions live in the `right-codegen` crate.
//! Removed in Stage F.

pub use right_codegen::*;
```

- [ ] **Step 4: Rewrite imports inside moved codegen files**

Use the same bulk-sed approach as Task 3:

```bash
# crate::codegen::X → crate::X (codegen modules are now at crate root)
devenv shell -- rg -l 'crate::codegen::' crates/right-codegen/src \
  | xargs sed -i.bak 's|crate::codegen::|crate::|g'

# crate::mcp::X → right_mcp::X
devenv shell -- rg -l 'crate::mcp::' crates/right-codegen/src \
  | xargs sed -i.bak 's|crate::mcp::|right_mcp::|g'

# crate::platform_store → right_core::platform_store
devenv shell -- rg -l 'crate::platform_store' crates/right-codegen/src \
  | xargs sed -i.bak 's|crate::platform_store|right_core::platform_store|g'

# crate::error::* → right_core::error::*
devenv shell -- rg -l 'crate::error::' crates/right-codegen/src \
  | xargs sed -i.bak 's|crate::error::|right_core::error::|g'

# crate::config:: → right_core::config::
devenv shell -- rg -l 'crate::config::' crates/right-codegen/src \
  | xargs sed -i.bak 's|crate::config::|right_core::config::|g'

# crate::ui:: → right_core::ui::
devenv shell -- rg -l 'crate::ui::' crates/right-codegen/src \
  | xargs sed -i.bak 's|crate::ui::|right_core::ui::|g'

# crate::openshell:: → right_core::openshell:: (and the openshell_proto one)
devenv shell -- rg -l 'crate::openshell\b|crate::openshell_proto' crates/right-codegen/src \
  | xargs sed -i.bak 's|crate::openshell_proto|right_core::openshell_proto|g; s|crate::openshell|right_core::openshell|g'

# crate::cron_spec::IDLE_THRESHOLD_* → right_core::time_constants::IDLE_THRESHOLD_*
devenv shell -- rg -l 'crate::cron_spec::IDLE_THRESHOLD' crates/right-codegen/src \
  | xargs sed -i.bak 's|crate::cron_spec::IDLE_THRESHOLD|right_core::time_constants::IDLE_THRESHOLD|g'

# Remaining crate::cron_spec::* — codegen should not need other cron_spec items, but flag and fix manually.
devenv shell -- rg 'crate::cron_spec::' crates/right-codegen/src

# Migrations + open helpers
devenv shell -- rg -l 'crate::memory::migrations::MIGRATIONS|crate::memory::open_connection|crate::memory::open_db' crates/right-codegen/src \
  | xargs sed -i.bak 's|crate::memory::migrations::MIGRATIONS|right_db::MIGRATIONS|g; s|crate::memory::open_connection|right_db::open_connection|g; s|crate::memory::open_db|right_db::open_db|g' 2>/dev/null

devenv shell -- find crates/right-codegen/src -name '*.bak' -delete
```

- [ ] **Step 5: Verify `right-codegen` builds**

Run: `devenv shell -- cargo build -p right-codegen`
Expected: succeeds. If anything fails, grep for unresolved paths:

```bash
devenv shell -- cargo check -p right-codegen 2>&1 | rg 'cannot find|unresolved|use of undeclared' | head -20
```

Common fixes:
- Missing `use right_core::ui::*;` at top of a file that calls `ui::Glyph`.
- A `crate::` path referencing `agent::types::*` (agent stays in right-agent — codegen should not depend on it; if a real reference exists, the `crate::*` becomes `right_agent::*`, which would create a cycle. In that case, the type must move down to `right-core::types` or be inlined; flag for follow-up and use a local `pub use right_agent::agent::types::*;` workaround temporarily).

- [ ] **Step 6: Verify `right-agent` builds via its re-export shim**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds. Internal `crate::codegen::*` callers (e.g. `agent/destroy.rs`, `init.rs`, `rebootstrap.rs`, `runtime/*`) reach `right_codegen::*` via the shim.

If `right-agent` build fails because of a cycle (e.g. `right-codegen` ended up needing `right_agent::*`), revisit Step 5's last note — break the cycle by inlining the offending type into `right-core` or by introducing a feature gate. Document the deviation in the task report.

- [ ] **Step 7: Run codegen tests**

Run: `devenv shell -- cargo test -p right-codegen`
Expected: passes (registry tests, process-compose tests, settings tests, etc.).

- [ ] **Step 8: Commit**

```bash
git add crates/right-codegen crates/right-agent/src/codegen Cargo.lock
git commit -m "refactor(right-codegen): extract codegen subsystem from right-agent"
```

---

## Sub-stage C3: extract `right-memory`

### Task 7: Scaffold the `right-memory` crate

**Files:**
- Create: `crates/right-memory/Cargo.toml`
- Create: `crates/right-memory/src/lib.rs`

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "right-memory"
version.workspace = true
edition.workspace = true

[dependencies]
right-core = { path = "../right-core" }
right-db = { path = "../right-db" }

serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
tokio = { workspace = true }
reqwest = { workspace = true }
chrono = { workspace = true }
url = { workspace = true }
rusqlite = { workspace = true }
uuid = { workspace = true }
fastrand = "2"

[dev-dependencies]
tempfile = { workspace = true }
right-core = { path = "../right-core", features = ["test-support"] }
```

- [ ] **Step 2: Create `lib.rs` stub**

```rust
//! Hindsight-resilience layer + retain queue for `right`.
//!
//! Pure HTTP-driven semantic memory (Hindsight Cloud API) plus a
//! SQLite-backed pending-retain queue. Schema lives in `right-db`.

pub mod circuit;
pub mod classify;
pub mod error;
pub mod guard;
pub mod hindsight;
pub mod prefetch;
pub mod resilient;
pub mod retain_queue;
pub mod status;

pub use classify::ErrorKind;
pub use error::MemoryError;
pub use resilient::{ResilientError, ResilientHindsight};
pub use status::MemoryStatus;

/// Dedup keys for rows in the `memory_alerts` table.
pub mod alert_types {
    pub const AUTH_FAILED: &str = "auth_failed";
    pub const CLIENT_FLOOD: &str = "client_flood";
}
```

- [ ] **Step 3: Verify wiring**

Run: `devenv shell -- cargo check --manifest-path crates/right-memory/Cargo.toml`
Expected: fails — workspace not aware, submodule files don't exist. Fixed by Task 8 + 9.

- [ ] **Step 4: Commit**

```bash
git add crates/right-memory/
git commit -m "feat(right-memory): scaffold new memory crate"
```

### Task 8: Add `right-memory` to workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add member**

```toml
[workspace]
members = ["crates/right-agent", "crates/right-codegen", "crates/right-core", "crates/right-db", "crates/right-mcp", "crates/right-memory", "crates/right", "crates/bot"]
resolver = "3"
```

- [ ] **Step 2: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat(workspace): register right-memory crate"
```

### Task 9: Move `memory/` contents to `right-memory`

**Files:**
- Move: `crates/right-agent/src/memory/{circuit,classify,error,guard,hindsight,prefetch,resilient,retain_queue,status}.rs` → `crates/right-memory/src/`
- Modify: `crates/right-agent/src/memory/mod.rs` (becomes a shim)

- [ ] **Step 1: Move the nine submodule files**

```bash
for f in circuit classify error guard hindsight prefetch resilient retain_queue status; do
  git mv crates/right-agent/src/memory/${f}.rs crates/right-memory/src/${f}.rs
done
```

(`store.rs` and `migrations.rs` were already removed at Stage A; nothing to move there.)

- [ ] **Step 2: Convert `right-agent/src/memory/mod.rs` to a re-export shim**

Replace the entire contents with:

```rust
//! Re-export shim. Real definitions live in the `right-memory` crate.
//! `open_db` / `open_connection` are sourced from `right-db`. Removed in Stage F.

pub use right_db::{open_connection, open_db};
pub use right_memory::*;
```

- [ ] **Step 3: Rewrite imports inside moved memory files**

```bash
# crate::memory::X → crate::X (memory modules are at crate root of right-memory)
devenv shell -- rg -l 'crate::memory::' crates/right-memory/src \
  | xargs sed -i.bak 's|crate::memory::|crate::|g'

# crate::error::* → right_core::error::*
devenv shell -- rg -l 'crate::error::display_error_chain|crate::error::AgentError' crates/right-memory/src \
  | xargs sed -i.bak 's|crate::error::|right_core::error::|g' 2>/dev/null

# crate::config:: → right_core::config::
devenv shell -- rg -l 'crate::config::' crates/right-memory/src \
  | xargs sed -i.bak 's|crate::config::|right_core::config::|g' 2>/dev/null

# Migration registry + open helpers
devenv shell -- rg -l 'crate::memory::migrations::MIGRATIONS' crates/right-memory/src \
  | xargs sed -i.bak 's|crate::memory::migrations::MIGRATIONS|right_db::MIGRATIONS|g' 2>/dev/null

devenv shell -- rg -l 'crate::memory::open_connection|crate::memory::open_db' crates/right-memory/src \
  | xargs sed -i.bak 's|crate::memory::open_connection|right_db::open_connection|g; s|crate::memory::open_db|right_db::open_db|g' 2>/dev/null

# alert_types references — `crate::memory::alert_types` → `crate::alert_types`
devenv shell -- rg -l 'crate::memory::alert_types' crates/right-memory/src \
  | xargs sed -i.bak 's|crate::memory::alert_types|crate::alert_types|g' 2>/dev/null

devenv shell -- find crates/right-memory/src -name '*.bak' -delete
```

- [ ] **Step 4: Verify `right-memory` builds**

Run: `devenv shell -- cargo build -p right-memory`
Expected: succeeds.

If unresolved paths appear:

```bash
devenv shell -- cargo check -p right-memory 2>&1 | rg 'cannot find|unresolved' | head -10
```

- [ ] **Step 5: Verify `right-agent` builds via its memory shim**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds.

- [ ] **Step 6: Add `right-memory` as a direct dep on `right-agent`**

In `crates/right-agent/Cargo.toml`, in `[dependencies]`, add (alphabetically):

```toml
right-memory = { path = "../right-memory" }
```

Without this, `right_memory::*` re-exported through `crate::memory::*` would be unresolvable from inside `right-agent`'s own callers because cargo doesn't auto-pull workspace deps.

- [ ] **Step 7: Run memory tests**

Run: `devenv shell -- cargo test -p right-memory`
Expected: passes (hindsight tests, classify tests, retain_queue tests, etc.).

- [ ] **Step 8: Commit**

```bash
git add crates/right-memory crates/right-agent/src/memory crates/right-agent/Cargo.toml Cargo.lock
git commit -m "refactor(right-memory): extract memory subsystem from right-agent"
```

---

## Cross-cutting integration

### Task 10: Add `right-codegen` and `right-mcp` as direct deps to `right-agent`

**Files:**
- Modify: `crates/right-agent/Cargo.toml`

- [ ] **Step 1: Add the deps**

In `[dependencies]`:

```toml
right-codegen = { path = "../right-codegen" }
right-mcp = { path = "../right-mcp" }
```

(Already done for `right-memory` in Task 9 Step 6.)

- [ ] **Step 2: Verify**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds. The shim files `mcp/mod.rs`, `codegen/mod.rs`, `memory/mod.rs` now resolve `right_mcp::*`, `right_codegen::*`, `right_memory::*`.

- [ ] **Step 3: Commit**

```bash
git add crates/right-agent/Cargo.toml Cargo.lock
git commit -m "deps(right-agent): wire direct paths to extracted leaf crates"
```

### Task 11: Update `right-bot` and `right` CLI to depend directly on `right-mcp`, `right-codegen`, `right-memory`

**Files:**
- Modify: `crates/bot/Cargo.toml`
- Modify: `crates/right/Cargo.toml`

- [ ] **Step 1: Inventory which leaf crates each consumer needs**

```bash
echo "=== bot needs ==="
devenv shell -- rg -l 'right_agent::mcp\b|right_agent::codegen\b|right_agent::memory\b' crates/bot/src crates/bot/tests 2>/dev/null
echo "=== CLI needs ==="
devenv shell -- rg -l 'right_agent::mcp\b|right_agent::codegen\b|right_agent::memory\b' crates/right/src crates/right/tests 2>/dev/null
```

- [ ] **Step 2: Add deps**

In `crates/bot/Cargo.toml` `[dependencies]`:

```toml
right-codegen = { path = "../right-codegen" }
right-mcp = { path = "../right-mcp" }
right-memory = { path = "../right-memory" }
```

(Skip any that the bot does not actually call — based on Step 1.)

In `crates/right/Cargo.toml` `[dependencies]`, add the same three (or only those the CLI calls).

- [ ] **Step 3: Verify**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/Cargo.toml crates/right/Cargo.toml Cargo.lock
git commit -m "deps: bot and CLI depend directly on extracted leaves"
```

### Task 12: Bulk-rewrite `right_agent::{mcp,codegen,memory}::*` → `right_{mcp,codegen,memory}::*` in bot and CLI sources

**Files:**
- Modify: files under `crates/bot/src/`, `crates/bot/tests/`, `crates/right/src/`, `crates/right/tests/`, `crates/right-agent/tests/`

- [ ] **Step 1: Bulk replace in bot src + tests**

```bash
for prefix in mcp codegen memory; do
  for tree in crates/bot/src crates/bot/tests; do
    devenv shell -- rg -l "right_agent::${prefix}\b" "$tree" 2>/dev/null \
      | xargs sed -i.bak "s|right_agent::${prefix}|right_${prefix}|g" 2>/dev/null
  done
done
devenv shell -- find crates/bot -name '*.bak' -delete
```

- [ ] **Step 2: Bulk replace in CLI src + tests**

```bash
for prefix in mcp codegen memory; do
  for tree in crates/right/src crates/right/tests; do
    devenv shell -- rg -l "right_agent::${prefix}\b" "$tree" 2>/dev/null \
      | xargs sed -i.bak "s|right_agent::${prefix}|right_${prefix}|g" 2>/dev/null
  done
done
devenv shell -- find crates/right -name '*.bak' -delete
```

- [ ] **Step 3: Bulk replace in right-agent integration tests**

```bash
for prefix in mcp codegen memory; do
  devenv shell -- rg -l "right_agent::${prefix}\b" crates/right-agent/tests 2>/dev/null \
    | xargs sed -i.bak "s|right_agent::${prefix}|right_${prefix}|g" 2>/dev/null
done
devenv shell -- find crates/right-agent/tests -name '*.bak' -delete
```

- [ ] **Step 4: Build & test**

```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo test --workspace --no-run
```

Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/bot crates/right crates/right-agent/tests
git commit -m "refactor: bulk-switch right_agent::{mcp,codegen,memory} callsites to extracted crates"
```

---

## Task 13: Whole-workspace build, test, lint pass

**Files:** none (verification only)

- [ ] **Step 1: Whole-workspace build (debug)**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds with zero warnings.

- [ ] **Step 2: Whole-workspace build (release)**

Run: `devenv shell -- cargo build --workspace --release`
Expected: succeeds.

- [ ] **Step 3: Whole-workspace test**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 4: Whole-workspace clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 5: Build-time benchmark**

```bash
devenv shell -- cargo clean
devenv shell -- cargo build --workspace --timings
```

Save wall-clock and timing-html to `~/Desktop/stage-c-timing.html` (or any path outside the repo). Expected outcome: editing a file under `crates/right-codegen/src/` rebuilds only `right-codegen` + `right-agent` + `right-bot` + `right`, not `right-memory` or `right-mcp`. Same for the other two leaves. Document the hot-edit rebuild times in the commit message.

- [ ] **Step 6: Fix in-place if anything fails**

Common issues:
- A test file under `crates/right-agent/tests/` that imports `right_agent::memory::store::*` — that path no longer exists post-Stage-A; switch to `right_agent::mcp::credentials::*` (auth tokens) or `right_memory::*` (semantic memory).
- A bot module that depends on `crate::memory::*` — that's a `right-bot` internal path and must stay; only `right_agent::memory::*` rewrites apply.
- An integration test that depended on a struct exported from a now-private path inside the moved crates — re-export it from the new crate's `lib.rs` if external consumers need it.

```bash
git add <fixed files>
git commit -m "fix(stage-c): resolve dangling references after leaf extraction"
```

---

## Task 14: Run `rust-dev:review-rust-code` agent

**Files:** none (review only)

- [ ] **Step 1: Dispatch**

> Review changes on the current branch since `<sha-of-stage-c-start>`. Focus on:
> 1. Cross-crate edges. Did `right-codegen` accidentally depend on `right-agent`? (Would create a cycle since slim `right-agent` depends on `right-codegen`.)
> 2. The `right-codegen → right-mcp` edge — confirm only the four documented references (`McpServerEntry`, `generate_agent_secret`, `derive_token` × 2) are still alive, no new edges introduced.
> 3. Re-export shims (`right-agent/src/{mcp,codegen,memory}/mod.rs`). Are they minimal? Do they over-export private items?
> 4. `MemoryError` and `DbError` boundary — after Stage A and Stage C, the wrapper logic should still compile cleanly; verify `From` impls don't double up.
> 5. Any newly-pub items in the leaf crates that are only used via the re-export shim — those should be `pub(crate)` or hidden until Stage F.
>
> Don't fix; report. Output as TODO list with file:line references.

- [ ] **Step 2: Triage findings**

Process as in earlier stages: bugs → fix one at a time with separate commits; nitpicks → defer file `docs/superpowers/plans/2026-05-06-stage-c-followups.md`; misunderstandings → ignore.

- [ ] **Step 3: Confirm tests pass**

Run: `devenv shell -- cargo test --workspace`

- [ ] **Step 4: Commit any fixes / followup file**

```bash
git add <files>
git commit -m "fix(stage-c): address review-rust-code findings"
```

---

## Task 15: Update `ARCHITECTURE.md`

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Extend the Workspace table**

Replace the table with:

```markdown
| Crate | Path | Role |
|-------|------|------|
| **right-core** | `crates/right-core/` | Stable platform-foundation — error/ui/config/openshell/proto/platform_store/stt/test_support, time constants |
| **right-db** | `crates/right-db/` | Per-agent SQLite plumbing — `open_connection`, central migration registry |
| **right-mcp** | `crates/right-mcp/` | MCP aggregator backend, proxy, reconnect, credentials, token derivation, auth tokens |
| **right-codegen** | `crates/right-codegen/` | Per-agent codegen — settings.json, .mcp.json, system prompts, process-compose, cloudflared, sandbox policy |
| **right-memory** | `crates/right-memory/` | Hindsight-resilience layer + retain queue (HTTP-driven semantic memory) |
| **right-agent** | `crates/right-agent/` | Slim orchestrator — agent CRUD, runtime, init, doctor, rebootstrap, cron_spec, usage, tunnel |
| **right** | `crates/right/` | CLI binary (`right`) + MCP Aggregator (HTTP) |
| **right-bot** | `crates/bot/` | Telegram bot runtime + cron engine + login flow |
```

- [ ] **Step 2: Update "Module Map" reference**

If `## Module Map` section says "see `docs/architecture/modules.md`", refresh that satellite file (`docs/architecture/modules.md`) to mirror the new layout. Cite-on-touch per CLAUDE.md.

- [ ] **Step 3: Refresh stale path references**

```bash
devenv shell -- rg -n 'right_agent::(mcp|codegen|memory)::' ARCHITECTURE.md docs/architecture
```

For each hit, decide: keep (documenting the intentional re-export path) or update to `right_{mcp,codegen,memory}::*`.

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md docs/architecture
git commit -m "docs(arch): add right-{mcp,codegen,memory} to workspace map"
```

---

## Task 16: Final verification

**Files:** none (verification + an optional summary commit)

- [ ] **Step 1: Re-run the full check suite**

```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo build --workspace --release
devenv shell -- cargo test --workspace
devenv shell -- cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all pass.

- [ ] **Step 2: Inventory check — leaves are leaves**

Each leaf crate must NOT depend on `right-agent`. Verify:

```bash
for crate in right-mcp right-codegen right-memory; do
  echo "=== ${crate} dep tree ==="
  devenv shell -- cargo tree -p ${crate} --depth 1 -e normal | rg right- | rg -v "${crate}"
done
```

Expected output for each: only `right-core`, `right-db`, and (for `right-codegen`) `right-mcp`. Any line containing `right-agent` is a cycle bug — fix before merging.

- [ ] **Step 3: Optional summary commit**

```bash
git commit --allow-empty -m "chore(stage-c): leaf extraction complete"
```

- [ ] **Step 4: Open PR (if on a branch)**

Title: `Stage C: extract right-{mcp,codegen,memory}`. Body references spec + this plan + the build-timing numbers from Task 13 Step 5.
