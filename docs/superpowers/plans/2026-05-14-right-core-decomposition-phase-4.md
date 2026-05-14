# Right-core Decomposition Phase 4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove `right-core` from the workspace by moving shared config to `right-config` and localizing the remaining error helpers.

**Architecture:** `right-config` becomes the public owner for `RIGHT_HOME`, global config, agent/backups directory helpers, and config YAML IO. `AgentError` moves to `right-agent` because it is agent-discovery specific; `display_error_chain` moves to `right-bot` because it has a single bot attachment caller. After this phase, no crate depends on `right-core`, and `crates/right-core/` is deleted.

**Tech Stack:** Rust 2024 Cargo workspace, `miette`, `serde`, `serde_json`, `serde-saphyr`, `dirs`, `tracing`, `thiserror`, `tempfile`, `devenv shell -- cargo`.

---

## Scope

This plan implements Phase 4 from `docs/superpowers/specs/2026-05-13-right-core-decomposition-design.md`.

In scope:

- Create `right-config`.
- Move `crates/right-core/src/config/mod.rs` to `crates/right-config/src/lib.rs`.
- Update all `right_core::config` imports to `right_config`.
- Move `AgentError` into `right-agent`.
- Move `display_error_chain` into `right-bot` near its only caller.
- Remove every `right-core` dependency from crate manifests.
- Remove `right-core` from workspace members and delete `crates/right-core/`.
- Update docs and verify compile fan-out.

Out of scope:

- Behavioral changes to config parsing, `~/.rightclaw` migration, tunnel config writing, or error messages.
- Creating a broad `right-error` crate. Current usage does not justify one.
- Renaming existing public functions such as `resolve_home`, `agents_dir`, `backups_dir`, `read_global_config`, or `write_global_config`.

## Current State

After Phase 3, `right-core` contains only:

```text
crates/right-core/src/lib.rs
crates/right-core/src/config/mod.rs
crates/right-core/src/error.rs
crates/right-core/Cargo.toml
```

`crates/right-core/src/lib.rs` exports only:

```rust
pub mod config;
pub mod error;
```

The only remaining `right-core` code users are `right_core::config` and `right_core::error`.

## File Structure

Create:

- `crates/right-config/Cargo.toml`
- `crates/right-config/src/lib.rs`
- `crates/right-agent/src/agent/error.rs`

Modify:

- `Cargo.toml`
- `Cargo.lock`
- `crates/right-codegen/Cargo.toml`
- `crates/right-codegen/src/pipeline.rs`
- `crates/right-agent/Cargo.toml`
- `crates/right-agent/src/agent/mod.rs`
- `crates/right-agent/src/agent/discovery.rs`
- `crates/right-agent/src/agent/discovery_tests.rs`
- `crates/right-agent/src/doctor.rs`
- `crates/right-agent/src/doctor_tests.rs`
- `crates/right-agent/src/init.rs`
- `crates/right-agent/src/rebootstrap.rs`
- `crates/right-agent/src/tunnel/health.rs`
- `crates/right-agent/src/agent/destroy.rs`
- `crates/right-agent/src/agent/register.rs`
- `crates/right/Cargo.toml`
- `crates/right/src/main.rs`
- `crates/right/src/wizard.rs`
- `crates/bot/Cargo.toml`
- `crates/bot/src/lib.rs`
- `crates/bot/src/telegram/handler.rs`
- `crates/bot/src/telegram/attachments.rs`
- `crates/right-mcp/Cargo.toml`
- `ARCHITECTURE.md`
- `docs/architecture/modules.md`
- `docs/architecture/lifecycle.md`

Delete:

- `crates/right-core/Cargo.toml`
- `crates/right-core/src/lib.rs`
- `crates/right-core/src/config/mod.rs`
- `crates/right-core/src/error.rs`
- `crates/right-core/`

## Task 1: Scaffold `right-config`

**Files:**

- Modify: `Cargo.toml`
- Create: `crates/right-config/Cargo.toml`
- Create: `crates/right-config/src/lib.rs`
- Modify: `Cargo.lock`

- [ ] **Step 1: Run the failing package check**

Run:

```bash
devenv shell -- cargo test -p right-config
```

Expected: FAIL with `package ID specification` because `right-config` does not exist.

- [ ] **Step 2: Add workspace member**

In root `Cargo.toml`, add the new member near the other foundational crates:

```toml
"crates/right-config",
```

Keep `crates/right-core` in the workspace until Task 4 deletes it.

- [ ] **Step 3: Create manifest**

Create `crates/right-config/Cargo.toml`:

```toml
[package]
name = "right-config"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
dirs = { workspace = true }
miette = { workspace = true }
serde = { workspace = true }
serde-saphyr = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 4: Create minimal lib**

Create `crates/right-config/src/lib.rs`:

```rust
//! Global Right Agent configuration and RIGHT_HOME path helpers.

#![warn(unreachable_pub)]
```

- [ ] **Step 5: Run scaffold test**

Run:

```bash
devenv shell -- cargo test -p right-config
```

Expected: PASS for the empty crate and `Cargo.lock` updated.

- [ ] **Step 6: Commit scaffold**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock crates/right-config/Cargo.toml crates/right-config/src/lib.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "chore(workspace): scaffold right-config crate"
```

Expected staged files:

```text
Cargo.lock
Cargo.toml
crates/right-config/Cargo.toml
crates/right-config/src/lib.rs
```

## Task 2: Move Global Config Out Of `right-core`

**Files:**

- Modify: `crates/right-config/src/lib.rs`
- Modify: `crates/right-core/src/lib.rs`
- Delete: `crates/right-core/src/config/mod.rs`
- Modify: `crates/right-codegen/Cargo.toml`
- Modify: `crates/right-codegen/src/pipeline.rs`
- Modify: `crates/right-agent/Cargo.toml`
- Modify: `crates/right-agent/src/doctor.rs`
- Modify: `crates/right-agent/src/doctor_tests.rs`
- Modify: `crates/right-agent/src/init.rs`
- Modify: `crates/right-agent/src/rebootstrap.rs`
- Modify: `crates/right-agent/src/tunnel/health.rs`
- Modify: `crates/right-agent/src/agent/destroy.rs`
- Modify: `crates/right-agent/src/agent/register.rs`
- Modify: `crates/right/Cargo.toml`
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/wizard.rs`
- Modify: `crates/bot/Cargo.toml`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/right-mcp/Cargo.toml`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing import change**

In `crates/right-codegen/src/pipeline.rs`, replace:

```rust
let global_cfg = right_core::config::read_global_config(home)?;
```

with:

```rust
let global_cfg = right_config::read_global_config(home)?;
```

- [ ] **Step 2: Run targeted test and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-codegen pipeline
```

Expected: FAIL because `right-codegen` does not depend on `right-config`, and `right-config` does not yet export `read_global_config`.

- [ ] **Step 3: Move config implementation**

Move the full contents of `crates/right-core/src/config/mod.rs` into `crates/right-config/src/lib.rs`, replacing the minimal lib. Keep all public APIs and tests unchanged:

```rust
pub fn resolve_home(cli_home: Option<&str>, env_home: Option<&str>) -> miette::Result<PathBuf>
pub struct GlobalConfig
pub struct AggregatorConfig
pub struct TunnelConfig
pub fn agents_dir(home: &Path) -> PathBuf
pub fn backups_dir(home: &Path, agent_name: &str) -> PathBuf
pub fn read_global_config(home: &Path) -> miette::Result<GlobalConfig>
pub fn write_global_config(home: &Path, config: &GlobalConfig) -> miette::Result<()>
```

In `crates/right-core/src/lib.rs`, delete:

```rust
pub mod config;
```

Delete `crates/right-core/src/config/mod.rs`.

- [ ] **Step 4: Add `right-config` dependencies**

Add this normal dependency to `crates/right-codegen/Cargo.toml`, `crates/right-agent/Cargo.toml`, `crates/right/Cargo.toml`, and `crates/bot/Cargo.toml`:

```toml
right-config = { path = "../right-config", version = "*" }
```

Remove the unused `right-core` dependency from `crates/right-mcp/Cargo.toml`; `right-mcp` has no `right_core::...` call sites.

- [ ] **Step 5: Update config call sites**

Replace all remaining config paths:

```text
right_core::config -> right_config
```

in these files:

```text
crates/right-codegen/src/pipeline.rs
crates/right-agent/src/doctor.rs
crates/right-agent/src/doctor_tests.rs
crates/right-agent/src/init.rs
crates/right-agent/src/rebootstrap.rs
crates/right-agent/src/tunnel/health.rs
crates/right-agent/src/agent/destroy.rs
crates/right-agent/src/agent/register.rs
crates/right/src/main.rs
crates/right/src/wizard.rs
crates/bot/src/lib.rs
crates/bot/src/telegram/handler.rs
```

In `crates/right/src/wizard.rs`, replace the top-level import:

```rust
use right_core::config::{TunnelConfig, read_global_config, write_global_config};
```

with:

```rust
use right_config::{TunnelConfig, read_global_config, write_global_config};
```

In `crates/right-agent/src/tunnel/health.rs`, replace:

```rust
use right_core::config::read_global_config;
```

with:

```rust
use right_config::read_global_config;
```

- [ ] **Step 6: Verify config move**

Run:

```bash
devenv shell -- cargo test -p right-config
devenv shell -- cargo test -p right-codegen pipeline
devenv shell -- cargo test -p right-agent doctor
devenv shell -- cargo test -p right
devenv shell -- cargo test -p right-bot
```

Expected: all commands PASS.

- [ ] **Step 7: Search stale config paths**

Run:

```bash
devenv shell -- rg -n "right_core::config|pub mod config|crates/right-core/src/config" crates ARCHITECTURE.md docs/architecture
```

Expected: no code matches. Doc matches are allowed until the docs task.

- [ ] **Step 8: Commit config move**

Run:

```bash
devenv shell -- git add Cargo.lock crates/right-config/src/lib.rs \
  crates/right-core/src/lib.rs crates/right-core/src/config/mod.rs \
  crates/right-codegen/Cargo.toml crates/right-codegen/src/pipeline.rs \
  crates/right-agent/Cargo.toml crates/right-agent/src/doctor.rs crates/right-agent/src/doctor_tests.rs \
  crates/right-agent/src/init.rs crates/right-agent/src/rebootstrap.rs crates/right-agent/src/tunnel/health.rs \
  crates/right-agent/src/agent/destroy.rs crates/right-agent/src/agent/register.rs \
  crates/right/Cargo.toml crates/right/src/main.rs crates/right/src/wizard.rs \
  crates/bot/Cargo.toml crates/bot/src/lib.rs crates/bot/src/telegram/handler.rs \
  crates/right-mcp/Cargo.toml
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(config): move global config out of right-core"
```

Expected: staged files are only this task's files plus `Cargo.lock`.

## Task 3: Localize Error Helpers

**Files:**

- Create: `crates/right-agent/src/agent/error.rs`
- Modify: `crates/right-agent/src/agent/mod.rs`
- Modify: `crates/right-agent/src/agent/discovery.rs`
- Modify: `crates/right-agent/src/agent/discovery_tests.rs`
- Modify: `crates/bot/src/telegram/attachments.rs`
- Modify: `crates/right-core/src/lib.rs`
- Delete: `crates/right-core/src/error.rs`
- Modify: `crates/right-codegen/Cargo.toml`
- Modify: `crates/right-agent/Cargo.toml`
- Modify: `crates/right/Cargo.toml`
- Modify: `crates/bot/Cargo.toml`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing agent-error import change**

In `crates/right-agent/src/agent/discovery.rs`, replace:

```rust
use right_core::error::AgentError;
```

with:

```rust
use crate::agent::error::AgentError;
```

In `crates/right-agent/src/agent/mod.rs`, add:

```rust
pub mod error;
```

- [ ] **Step 2: Run targeted test and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-agent validate_rejects_name_with_spaces
```

Expected: FAIL because `crate::agent::error::AgentError` does not exist yet.

- [ ] **Step 3: Add `AgentError` to `right-agent`**

Create `crates/right-agent/src/agent/error.rs`:

```rust
use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum AgentError {
    #[error("Agent '{name}' is missing required file: {file}")]
    #[diagnostic(code(right_agent::agent::missing_file))]
    MissingRequiredFile { name: String, file: String },

    #[error("Failed to parse agent.yaml for '{name}': {reason}")]
    #[diagnostic(code(right_agent::agent::invalid_config))]
    InvalidConfig { name: String, reason: String },

    #[error(
        "Invalid agent directory name '{name}': must contain only alphanumeric characters, hyphens, or underscores"
    )]
    #[diagnostic(code(right_agent::agent::invalid_name))]
    InvalidName { name: String },

    #[error("Failed to read agents directory: {path}")]
    #[diagnostic(code(right_agent::agent::io_error))]
    IoError {
        path: String,
        #[source]
        source: std::io::Error,
    },
}
```

In `crates/right-agent/src/agent/discovery_tests.rs`, keep `use super::*`; the tests should continue to see `AgentError` through `discovery.rs`'s import.

- [ ] **Step 4: Localize `display_error_chain` in bot**

In `crates/bot/src/telegram/attachments.rs`, add this private helper near `SendError`:

```rust
fn display_error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        use std::fmt::Write as _;
        let _ = write!(out, ": {cause}");
        source = cause.source();
    }
    out
}
```

Replace:

```rust
right_core::error::display_error_chain(&e)
```

with:

```rust
display_error_chain(&e)
```

- [ ] **Step 5: Remove error from `right-core` and manifests**

In `crates/right-core/src/lib.rs`, delete:

```rust
pub mod error;
```

Delete `crates/right-core/src/error.rs`.

Remove `right-core` normal and dev dependencies from:

```text
crates/right-codegen/Cargo.toml
crates/right-agent/Cargo.toml
crates/right/Cargo.toml
crates/bot/Cargo.toml
```

- [ ] **Step 6: Verify localized errors**

Run:

```bash
devenv shell -- cargo test -p right-agent validate_rejects_name_with_spaces
devenv shell -- cargo test -p right-agent discovery
devenv shell -- cargo test -p right-bot attachments
devenv shell -- rg -n "right_core::error|crates/right-core/src/error.rs" crates
```

Expected: tests PASS and search has no matches.

- [ ] **Step 7: Commit error localization**

Run:

```bash
devenv shell -- git add Cargo.lock \
  crates/right-agent/Cargo.toml crates/right-agent/src/agent/mod.rs crates/right-agent/src/agent/error.rs \
  crates/right-agent/src/agent/discovery.rs crates/right-agent/src/agent/discovery_tests.rs \
  crates/bot/Cargo.toml crates/bot/src/telegram/attachments.rs \
  crates/right-codegen/Cargo.toml crates/right/Cargo.toml \
  crates/right-core/src/lib.rs crates/right-core/src/error.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(errors): localize remaining right-core errors"
```

Expected: staged files are only this task's files plus `Cargo.lock`.

## Task 4: Delete `right-core`

**Files:**

- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Delete: `crates/right-core/`

- [ ] **Step 1: Verify `right-core` is empty before deletion**

Run:

```bash
devenv shell -- sed -n '1,80p' crates/right-core/src/lib.rs
devenv shell -- rg --files crates/right-core
devenv shell -- rg -n "right-core =|right_core::" crates Cargo.toml
```

Expected:

- `crates/right-core/src/lib.rs` has no `pub mod` lines.
- `crates/right-core` contains only `Cargo.toml` and `src/lib.rs`.
- No `right-core =` or `right_core::` matches in code/manifests.

- [ ] **Step 2: Remove workspace member and crate directory**

In root `Cargo.toml`, remove:

```toml
"crates/right-core",
```

Delete:

```text
crates/right-core/Cargo.toml
crates/right-core/src/lib.rs
crates/right-core/
```

- [ ] **Step 3: Update lockfile through Cargo**

Run:

```bash
devenv shell -- cargo test -p right-config
```

Expected: PASS and `Cargo.lock` no longer contains the `right-core` package.

- [ ] **Step 4: Verify `right-core` is gone**

Run:

```bash
devenv shell -- rg -n 'name = "right-core"' Cargo.lock
devenv shell -- rg -n "right-core|right_core::|crates/right-core" Cargo.toml crates ARCHITECTURE.md docs/architecture
```

Expected: first command has no matches. Second command may have docs matches until Task 5, but must have no code or manifest matches.

- [ ] **Step 5: Commit deletion**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock crates/right-core
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(core): remove right-core crate"
```

Expected: staged files are only root `Cargo.toml`, `Cargo.lock`, and deleted `crates/right-core` files.

## Task 5: Docs And Final Verification

**Files:**

- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/modules.md`
- Modify: `docs/architecture/lifecycle.md`
- Temporary probe edits only; revert before finishing.

- [ ] **Step 1: Update architecture docs**

In `ARCHITECTURE.md`:

- Keep the workspace count at 17 if only `right-core` was replaced by `right-config`.
- Remove the `right-core` table row.
- Add:

```markdown
| **right-config** | `crates/right-config/` | RIGHT_HOME resolution, global config YAML, agents/backups directory helpers |
```

- Remove text that describes `right-core` as a boundary.
- State that `right-core` was removed in Phase 4 and new shared code must go to the most-specific owner crate.

In `docs/architecture/modules.md`, replace the `right-core` section with:

```markdown
### right-config

- `src/lib.rs` - `GlobalConfig`, `TunnelConfig`, `AggregatorConfig`, `RIGHT_HOME` resolution, global config YAML IO, and agents/backups path helpers.
```

In `docs/architecture/lifecycle.md`, update any config owner prose to reference `right-config`.

- [ ] **Step 2: Verify stale docs/code paths are gone**

Run:

```bash
devenv shell -- rg -n "right-core|right_core::|crates/right-core" Cargo.toml crates ARCHITECTURE.md docs/architecture
```

Expected: no matches except historical references in committed `docs/superpowers/specs/` or `docs/superpowers/plans/`, which are intentionally outside this search.

- [ ] **Step 3: Run full build and tests**

Run:

```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo test --workspace
```

Expected: both commands PASS.

- [ ] **Step 4: Probe config compile fan-out**

Temporarily edit a doc comment in `crates/right-config/src/lib.rs`, then run:

```bash
devenv shell -- cargo build --workspace -vv
```

Expected rebuilds include `right-config` and direct config consumers such as `right-codegen`, `right-agent`, `right`, and `right-bot`. Expected rebuilds must not include `right-db`, `right-memory`, `right-mcp`, `right-ui`, `right-openshell`, `right-platform-store`, `right-stt`, `right-process`, `right-prompt-safety`, `right-platform-knobs`, or `right-runtime-state`.

Revert only the temporary doc-comment edit with `apply_patch`, then run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 5: Commit docs**

Run:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/modules.md docs/architecture/lifecycle.md
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "docs(architecture): document right-core removal"
```

Expected staged files:

```text
ARCHITECTURE.md
docs/architecture/lifecycle.md
docs/architecture/modules.md
```

- [ ] **Step 6: Confirm clean worktree**

Run:

```bash
devenv shell -- git status --short
```

Expected: no uncommitted changes from Phase 4.
