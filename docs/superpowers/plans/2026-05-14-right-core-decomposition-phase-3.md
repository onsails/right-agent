# Right-core Decomposition Phase 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract CLI UI, process-group handling, OpenShell/proto/test support, and platform-store deployment from `right-core`.

**Architecture:** Add four owner crates: `right-ui`, `right-process`, `right-openshell`, and `right-platform-store`. Consumers import owner crates directly; `right-core` does not re-export moved modules. After this phase, `right-core` should only own `config` and `error`, leaving Phase 4 to decide whether that crate is still needed.

**Tech Stack:** Rust 2024 Cargo workspace, `miette`, `thiserror`, `serde`, `serde_json`, `serde-saphyr`, `tokio`, `tonic`, `tonic-prost`, `prost`, `fs4`, `futures`, `nix`, `owo-colors`, `inquire`, `devenv shell -- cargo`.

---

## Scope

This plan implements Phase 3 from `docs/superpowers/specs/2026-05-13-right-core-decomposition-design.md`.

In scope:

- Move `crates/right-core/src/ui/` to `crates/right-ui/src/`.
- Move `crates/right-core/src/process_group.rs` to `crates/right-process/src/lib.rs`.
- Move `openshell`, generated proto module ownership, `sandbox_exec`, `test_cleanup`, and `test_support` to `right-openshell`.
- Move `platform_store` to `right-platform-store`.
- Move OpenShell proto files and build script from `right-core` to `right-openshell`.
- Update direct imports and remove moved modules from `right-core`.
- Update architecture docs and compile fan-out probes.

Out of scope:

- Moving `right-core::config` or `right-core::error`.
- Removing `right-core` from the workspace. That remains Phase 4.
- Behavioral changes to OpenShell, process cleanup, platform-store deployment, live sandbox tests, or UI rendering.
- Adding compatibility re-exports from `right-core`.

## Execution Notes

- Work in `/Users/molt/dev/rightclaw/.worktrees/right-core-decomposition-phase-1`.
- Prefix commands with `devenv shell --`.
- Before writing Rust implementation code, use `rust-dev:rust-dev` if available. If unavailable, state that and follow `AGENTS.rust.md`.
- Use `apply_patch` for manual edits.
- Do not stage or revert unrelated user changes.
- Before every commit, run `devenv shell -- git diff --cached --name-only` and verify only the task files are staged.

## File Structure

Create:

- `crates/right-ui/Cargo.toml`
- `crates/right-ui/src/lib.rs`
- `crates/right-ui/src/*.rs`
- `crates/right-process/Cargo.toml`
- `crates/right-process/src/lib.rs`
- `crates/right-openshell/Cargo.toml`
- `crates/right-openshell/build.rs`
- `crates/right-openshell/proto/openshell/*.proto`
- `crates/right-openshell/src/lib.rs`
- `crates/right-openshell/src/openshell.rs`
- `crates/right-openshell/src/sandbox_exec.rs`
- `crates/right-openshell/src/test_cleanup.rs`
- `crates/right-openshell/src/test_support.rs`
- `crates/right-platform-store/Cargo.toml`
- `crates/right-platform-store/src/lib.rs`

Modify:

- `Cargo.toml`
- `crates/right-core/Cargo.toml`
- `crates/right-core/src/lib.rs`
- `crates/right-agent/Cargo.toml`
- `crates/right-agent/src/doctor.rs`
- `crates/right-agent/src/doctor_tests.rs`
- `crates/right-agent/src/agent/destroy.rs`
- `crates/right-agent/src/rebootstrap.rs`
- `crates/right-agent/tests/*.rs` that import moved OpenShell test support.
- `crates/right-codegen/Cargo.toml`
- `crates/right-codegen/src/contract.rs`
- `crates/right/Cargo.toml`
- `crates/right/src/main.rs`
- `crates/right/src/wizard.rs`
- `crates/right/src/internal_api.rs`
- `crates/right/src/right_backend.rs`
- `crates/right/src/right_backend_tests.rs`
- `crates/right/tests/*.rs` that import moved OpenShell test support.
- `crates/bot/Cargo.toml`
- `crates/bot/src/**/*.rs` that import moved UI/process/OpenShell/platform-store APIs.
- `crates/bot/tests/*.rs` that import moved OpenShell test support.
- `ARCHITECTURE.md`
- `docs/architecture/modules.md`
- `docs/architecture/lifecycle.md`

Delete:

- `crates/right-core/build.rs`
- `crates/right-core/proto/openshell/*.proto`
- `crates/right-core/src/ui/`
- `crates/right-core/src/process_group.rs`
- `crates/right-core/src/openshell.rs`
- `crates/right-core/src/openshell_tests.rs`
- `crates/right-core/src/sandbox_exec.rs`
- `crates/right-core/src/test_cleanup.rs`
- `crates/right-core/src/test_support.rs`
- `crates/right-core/src/platform_store.rs`
- `crates/right-core/src/platform_store_tests.rs`

## Task 1: Scaffold Phase 3 Crates

**Files:**

- Modify: `Cargo.toml`
- Create: `crates/right-ui/Cargo.toml`
- Create: `crates/right-ui/src/lib.rs`
- Create: `crates/right-process/Cargo.toml`
- Create: `crates/right-process/src/lib.rs`
- Create: `crates/right-openshell/Cargo.toml`
- Create: `crates/right-openshell/src/lib.rs`
- Create: `crates/right-platform-store/Cargo.toml`
- Create: `crates/right-platform-store/src/lib.rs`
- Modify: `Cargo.lock`

- [ ] **Step 1: Run the failing package check**

Run:

```bash
devenv shell -- cargo test -p right-ui -p right-process -p right-openshell -p right-platform-store
```

Expected: FAIL with `package ID specification` because the four packages do not exist.

- [ ] **Step 2: Add workspace members**

In root `Cargo.toml`, add these workspace members after the Phase 2 crates:

```toml
"crates/right-ui",
"crates/right-process",
"crates/right-openshell",
"crates/right-platform-store",
```

- [ ] **Step 3: Create manifests**

Create `crates/right-ui/Cargo.toml`:

```toml
[package]
name = "right-ui"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
inquire = { workspace = true }
miette = { workspace = true }
owo-colors = { workspace = true }
```

Create `crates/right-process/Cargo.toml`:

```toml
[package]
name = "right-process"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
nix = { workspace = true }
tokio = { workspace = true }
```

Create `crates/right-openshell/Cargo.toml`:

```toml
[package]
name = "right-openshell"
version.workspace = true
edition.workspace = true
publish = false

[features]
test-support = []

[dependencies]
dirs = { workspace = true }
fs4 = { workspace = true }
futures = { workspace = true }
http = { workspace = true }
hyper-util = { workspace = true }
miette = { workspace = true }
prost = { workspace = true }
prost-types = { workspace = true }
right-process = { path = "../right-process", version = "*" }
serde_json = { workspace = true }
serde-saphyr = { workspace = true }
tempfile = { workspace = true }
tokio = { workspace = true }
tonic = { workspace = true }
tonic-prost = { workspace = true }
tracing = { workspace = true }
walkdir = { workspace = true }
which = { workspace = true }

[build-dependencies]
tonic-prost-build = "0.14"

[dev-dependencies]
tokio-stream = { version = "0.1", features = ["net"] }
```

Create `crates/right-platform-store/Cargo.toml`:

```toml
[package]
name = "right-platform-store"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
futures = { workspace = true }
miette = { workspace = true }
right-openshell = { path = "../right-openshell", version = "*" }
sha2 = { workspace = true }
tempfile = { workspace = true }
walkdir = { workspace = true }
```

- [ ] **Step 4: Create minimal libs**

Create the four `src/lib.rs` files with crate-level docs and `#![warn(unreachable_pub)]`. Use these one-line module docs:

```rust
//! Brand-conformant CLI presentation primitives.
//! Process-group subprocess handling.
//! OpenShell gRPC, CLI wrappers, sandbox exec, and live-test support.
//! Content-addressed platform-managed file deployment into sandboxes.
```

Each file gets only the matching doc line, not all four lines.

- [ ] **Step 5: Run scaffold tests**

Run:

```bash
devenv shell -- cargo test -p right-ui -p right-process -p right-openshell -p right-platform-store
```

Expected: PASS for the four empty crates and `Cargo.lock` updated.

- [ ] **Step 6: Commit scaffold**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock \
  crates/right-ui/Cargo.toml crates/right-ui/src/lib.rs \
  crates/right-process/Cargo.toml crates/right-process/src/lib.rs \
  crates/right-openshell/Cargo.toml crates/right-openshell/src/lib.rs \
  crates/right-platform-store/Cargo.toml crates/right-platform-store/src/lib.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "chore(workspace): scaffold phase three split crates"
```


## Task 2: Move UI And Process Helpers

**Files:**

- Modify: `crates/right-ui/src/lib.rs`
- Create: `crates/right-ui/src/atoms.rs`
- Create: `crates/right-ui/src/error.rs`
- Create: `crates/right-ui/src/header.rs`
- Create: `crates/right-ui/src/line.rs`
- Create: `crates/right-ui/src/prompts.rs`
- Create: `crates/right-ui/src/recap.rs`
- Create: `crates/right-ui/src/splash.rs`
- Create: `crates/right-ui/src/theme.rs`
- Create: `crates/right-ui/src/writer.rs`
- Create: `crates/right-ui/src/*_tests.rs`
- Modify: `crates/right-process/src/lib.rs`
- Modify: `crates/right-core/src/lib.rs`
- Delete: `crates/right-core/src/ui/`
- Delete: `crates/right-core/src/process_group.rs`
- Modify: `crates/right-agent/Cargo.toml`
- Modify: `crates/right-agent/src/doctor.rs`
- Modify: `crates/right-agent/src/doctor_tests.rs`
- Modify: `crates/right/Cargo.toml`
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/wizard.rs`
- Modify: `crates/bot/Cargo.toml`
- Modify: process-group call sites under `crates/bot/src/`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing imports**

In `crates/right-agent/src/doctor.rs`, replace:

```rust
use right_core::ui::{self, Glyph};
```

with:

```rust
use right_ui::{self as ui, Glyph};
```

In one bot process call site, for example `crates/bot/src/keepalive.rs`, replace:

```rust
right_core::process_group::ProcessGroupChild::spawn(cmd)
```

with:

```rust
right_process::ProcessGroupChild::spawn(cmd)
```

- [ ] **Step 2: Run targeted tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-agent doctor
devenv shell -- cargo test -p right-bot keepalive
```

Expected: FAIL because `right-agent` lacks `right-ui`, `right-bot` lacks `right-process`, and the owner crates do not export the moved APIs yet.

- [ ] **Step 3: Move UI implementation**

Move all files from `crates/right-core/src/ui/` into `crates/right-ui/src/`.

Use the old `crates/right-core/src/ui/mod.rs` contents as `crates/right-ui/src/lib.rs`, keeping all public exports:

```rust
pub use atoms::{Glyph, Rail};
pub use error::BlockAlreadyRendered;
pub use header::section;
pub use line::{Block, Line, status};
pub use prompts::install_global as install_prompt_render_config;
pub use recap::Recap;
pub use splash::splash;
pub use theme::{Theme, detect};
pub use writer::{stderr, stdout};
```

In moved UI files, replace internal paths:

```text
crate::ui::theme::Theme -> crate::theme::Theme
crate::ui::atoms -> crate::atoms
crate::ui::line -> crate::line
crate::ui::header -> crate::header
crate::ui::Glyph -> crate::Glyph
crate::ui::detect() -> crate::detect()
```

- [ ] **Step 4: Move process helper implementation**

Move the body of `crates/right-core/src/process_group.rs` into `crates/right-process/src/lib.rs`, preserving the public `ProcessGroupChild` API.

- [ ] **Step 5: Remove moved modules from `right-core`**

In `crates/right-core/src/lib.rs`, delete:

```rust
#[cfg(unix)]
pub mod process_group;
pub mod ui;
```

Delete `crates/right-core/src/process_group.rs` and `crates/right-core/src/ui/`.

- [ ] **Step 6: Update consumer manifests and imports**

Add `right-ui` to normal dependencies of `crates/right-agent/Cargo.toml`, `crates/right/Cargo.toml`, and `crates/bot/Cargo.toml`.

Add `right-process` to normal dependencies of `crates/bot/Cargo.toml`.

Replace UI paths:

```text
right_core::ui -> right_ui
```

in:

```text
crates/right-agent/src/doctor.rs
crates/right-agent/src/doctor_tests.rs
crates/right/src/main.rs
crates/right/src/wizard.rs
crates/bot/src/telegram/handler.rs
```

Replace process paths:

```text
right_core::process_group::ProcessGroupChild -> right_process::ProcessGroupChild
```

under `crates/bot/src/`.

- [ ] **Step 7: Verify UI/process move**

Run:

```bash
devenv shell -- cargo test -p right-ui
devenv shell -- cargo test -p right-process
devenv shell -- cargo test -p right-agent doctor
devenv shell -- cargo test -p right-bot keepalive
devenv shell -- cargo test -p right wizard
```

Expected: all commands PASS.

- [ ] **Step 8: Search stale UI/process paths**

Run:

```bash
devenv shell -- rg -n "right_core::ui|right_core::process_group|pub mod ui|pub mod process_group|crates/right-core/src/ui|crates/right-core/src/process_group.rs" crates ARCHITECTURE.md docs/architecture
```

Expected: no code matches. Doc matches are allowed only until the docs task.

- [ ] **Step 9: Commit UI/process move**

Run:

```bash
devenv shell -- git add Cargo.lock \
  crates/right-ui crates/right-process \
  crates/right-core/src/lib.rs crates/right-core/src/ui crates/right-core/src/process_group.rs \
  crates/right-agent/Cargo.toml crates/right-agent/src/doctor.rs crates/right-agent/src/doctor_tests.rs \
  crates/right/Cargo.toml crates/right/src/main.rs crates/right/src/wizard.rs \
  crates/bot/Cargo.toml crates/bot/src
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(ui): move ui and process helpers out of right-core"
```


## Task 3: Move OpenShell Stack

**Files:**

- Modify: `crates/right-openshell/Cargo.toml`
- Modify: `crates/right-openshell/src/lib.rs`
- Create: `crates/right-openshell/build.rs`
- Create: `crates/right-openshell/proto/openshell/*.proto`
- Create: `crates/right-openshell/src/openshell.rs`
- Create: `crates/right-openshell/src/openshell_tests.rs`
- Create: `crates/right-openshell/src/sandbox_exec.rs`
- Create: `crates/right-openshell/src/test_cleanup.rs`
- Create: `crates/right-openshell/src/test_support.rs`
- Modify: `crates/right-core/Cargo.toml`
- Modify: `crates/right-core/src/lib.rs`
- Delete: `crates/right-core/build.rs`
- Delete: `crates/right-core/proto/openshell/*.proto`
- Delete: `crates/right-core/src/openshell.rs`
- Delete: `crates/right-core/src/openshell_tests.rs`
- Delete: `crates/right-core/src/sandbox_exec.rs`
- Delete: `crates/right-core/src/test_cleanup.rs`
- Delete: `crates/right-core/src/test_support.rs`
- Modify: manifests and call sites in `crates/right-codegen`, `crates/right-agent`, `crates/right`, and `crates/bot`.
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing OpenShell imports**

In `crates/right-codegen/src/contract.rs`, replace:

```rust
right_core::openshell::apply_policy(sandbox, path).await
```

with:

```rust
right_openshell::openshell::apply_policy(sandbox, path).await
```

In `crates/right-agent/src/rebootstrap.rs`, replace:

```rust
use right_core::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;
```

with:

```rust
use right_openshell::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;
```

- [ ] **Step 2: Run targeted tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-codegen contract
devenv shell -- cargo test -p right-agent rebootstrap
```

Expected: FAIL because consumers do not yet depend on `right-openshell`, and `right-openshell` does not yet export the moved modules.

- [ ] **Step 3: Move OpenShell source and proto ownership**

Move these files and directories:

```text
crates/right-core/build.rs -> crates/right-openshell/build.rs
crates/right-core/proto/openshell/ -> crates/right-openshell/proto/openshell/
crates/right-core/src/openshell.rs -> crates/right-openshell/src/openshell.rs
crates/right-core/src/openshell_tests.rs -> crates/right-openshell/src/openshell_tests.rs
crates/right-core/src/sandbox_exec.rs -> crates/right-openshell/src/sandbox_exec.rs
crates/right-core/src/test_cleanup.rs -> crates/right-openshell/src/test_cleanup.rs
crates/right-core/src/test_support.rs -> crates/right-openshell/src/test_support.rs
```

In `crates/right-openshell/src/lib.rs`, define the moved module surface:

```rust
//! OpenShell gRPC, CLI wrappers, sandbox exec, and live-test support.

#![warn(unreachable_pub)]

pub mod openshell;
#[allow(clippy::large_enum_variant)]
pub mod openshell_proto {
    pub mod openshell {
        pub mod v1 {
            tonic::include_proto!("openshell.v1");
        }
        pub mod datamodel {
            pub mod v1 {
                tonic::include_proto!("openshell.datamodel.v1");
            }
        }
        pub mod sandbox {
            pub mod v1 {
                tonic::include_proto!("openshell.sandbox.v1");
            }
        }
    }
}
pub mod sandbox_exec;
#[cfg(unix)]
pub mod test_cleanup;
#[cfg(all(unix, any(test, feature = "test-support")))]
pub mod test_support;
```

- [ ] **Step 4: Fix internal OpenShell crate paths**

Inside moved `crates/right-openshell/src/openshell.rs`, replace:

```text
crate::process_group::ProcessGroupChild -> right_process::ProcessGroupChild
```

Keep `crate::openshell_proto` paths as crate-local paths.

Inside moved `crates/right-openshell/src/sandbox_exec.rs`, keep `crate::openshell` paths as crate-local paths.

Inside moved `crates/right-openshell/src/test_support.rs`, keep `crate::openshell` and `crate::test_cleanup` paths as crate-local paths.

- [ ] **Step 5: Remove OpenShell modules from `right-core`**

In `crates/right-core/src/lib.rs`, delete:

```rust
pub mod openshell;
pub mod openshell_proto { ... }
pub mod sandbox_exec;
#[cfg(unix)]
pub mod test_cleanup;
#[cfg(all(unix, any(test, feature = "test-support")))]
pub mod test_support;
```

Delete the moved source files, build script, and proto directory from `crates/right-core/`.

In `crates/right-core/Cargo.toml`, remove the `test-support` feature, `tonic-prost-build`, OpenShell/proto dependencies, and OpenShell test-only dev dependencies. Keep only dependencies used by `config` and `error`.

- [ ] **Step 6: Update consumer manifests**

Add normal dependency:

```toml
right-openshell = { path = "../right-openshell", version = "*" }
```

to `crates/right-codegen/Cargo.toml`, `crates/right-agent/Cargo.toml`, `crates/right/Cargo.toml`, and `crates/bot/Cargo.toml`.

Add dev dependency with live-test support:

```toml
right-openshell = { path = "../right-openshell", version = "*", features = ["test-support"] }
```

to `crates/right-agent/Cargo.toml`, `crates/right/Cargo.toml`, and `crates/bot/Cargo.toml`.

Remove `features = ["test-support"]` from `right-core` dev-dependency entries in those manifests.

- [ ] **Step 7: Update OpenShell call sites**

Replace these paths across `crates/right-codegen`, `crates/right-agent`, `crates/right`, and `crates/bot`:

```text
right_core::openshell -> right_openshell::openshell
right_core::openshell_proto -> right_openshell::openshell_proto
right_core::sandbox_exec -> right_openshell::sandbox_exec
right_core::test_cleanup -> right_openshell::test_cleanup
right_core::test_support -> right_openshell::test_support
```

Update comments in tests that name `right_core::test_support` to say `right_openshell::test_support`.

- [ ] **Step 8: Verify OpenShell move**

Run:

```bash
devenv shell -- cargo test -p right-openshell
devenv shell -- cargo test -p right-codegen contract
devenv shell -- cargo test -p right-agent doctor
devenv shell -- cargo test -p right-agent rebootstrap
devenv shell -- cargo test -p right-bot cc_debug
devenv shell -- cargo test -p right cli_integration
```

Expected: all commands PASS. Live sandbox tests must continue using OpenShell; do not mark them ignored.

- [ ] **Step 9: Search stale OpenShell paths**

Run:

```bash
devenv shell -- rg -n "right_core::openshell|right_core::openshell_proto|right_core::sandbox_exec|right_core::test_cleanup|right_core::test_support|pub mod openshell|pub mod openshell_proto|pub mod sandbox_exec|pub mod test_cleanup|pub mod test_support" crates ARCHITECTURE.md docs/architecture
```

Expected: no code matches. Doc matches are allowed only until the docs task.

- [ ] **Step 10: Commit OpenShell move**

Run:

```bash
devenv shell -- git add Cargo.lock crates/right-openshell \
  crates/right-core/Cargo.toml crates/right-core/src/lib.rs crates/right-core/build.rs crates/right-core/proto \
  crates/right-core/src/openshell.rs crates/right-core/src/openshell_tests.rs \
  crates/right-core/src/sandbox_exec.rs crates/right-core/src/test_cleanup.rs crates/right-core/src/test_support.rs \
  crates/right-codegen crates/right-agent crates/right crates/bot
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(openshell): move openshell stack out of right-core"
```


## Task 4: Move Platform Store

**Files:**

- Modify: `crates/right-platform-store/src/lib.rs`
- Create: `crates/right-platform-store/src/platform_store_tests.rs`
- Modify: `crates/right-core/Cargo.toml`
- Modify: `crates/right-core/src/lib.rs`
- Delete: `crates/right-core/src/platform_store.rs`
- Delete: `crates/right-core/src/platform_store_tests.rs`
- Modify: `crates/bot/Cargo.toml`
- Modify: `crates/bot/src/sync.rs`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing platform-store import**

In `crates/bot/src/sync.rs`, replace:

```rust
let manifest = right_core::platform_store::build_manifest(agent_dir)?;
right_core::platform_store::deploy_manifest(sbox, &manifest).await?;
```

with:

```rust
let manifest = right_platform_store::build_manifest(agent_dir)?;
right_platform_store::deploy_manifest(sbox, &manifest).await?;
```

- [ ] **Step 2: Run targeted test and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-bot sync
```

Expected: FAIL because `right-bot` does not yet depend on `right-platform-store`, and the owner crate does not yet export the moved API.

- [ ] **Step 3: Move platform-store implementation**

Move:

```text
crates/right-core/src/platform_store.rs -> crates/right-platform-store/src/lib.rs
crates/right-core/src/platform_store_tests.rs -> crates/right-platform-store/src/platform_store_tests.rs
```

In `crates/right-platform-store/src/lib.rs`, replace:

```text
crate::sandbox_exec::SandboxExec -> right_openshell::sandbox_exec::SandboxExec
crate::openshell::upload_file -> right_openshell::openshell::upload_file
```

Keep public functions and constants unchanged: `content_hash`, `directory_hash`, `platform_path`, `PLATFORM_DIR`, `build_manifest`, and `deploy_manifest`.

- [ ] **Step 4: Remove platform-store from `right-core`**

In `crates/right-core/src/lib.rs`, delete:

```rust
pub mod platform_store;
```

Delete `crates/right-core/src/platform_store.rs` and `crates/right-core/src/platform_store_tests.rs`.

Remove platform-store-only dependencies from `crates/right-core/Cargo.toml` if no longer used by `config` or `error`: `futures`, `sha2`, and `walkdir`.

- [ ] **Step 5: Update bot manifest**

Add to `crates/bot/Cargo.toml`:

```toml
right-platform-store = { path = "../right-platform-store", version = "*" }
```

- [ ] **Step 6: Verify platform-store move**

Run:

```bash
devenv shell -- cargo test -p right-platform-store
devenv shell -- cargo test -p right-bot sync
devenv shell -- cargo test -p right-bot
```

Expected: all commands PASS.

- [ ] **Step 7: Search stale platform-store paths**

Run:

```bash
devenv shell -- rg -n "right_core::platform_store|pub mod platform_store|crates/right-core/src/platform_store" crates ARCHITECTURE.md docs/architecture
```

Expected: no code matches. Doc matches are allowed only until the docs task.

- [ ] **Step 8: Commit platform-store move**

Run:

```bash
devenv shell -- git add Cargo.lock crates/right-platform-store \
  crates/right-core/Cargo.toml crates/right-core/src/lib.rs \
  crates/right-core/src/platform_store.rs crates/right-core/src/platform_store_tests.rs \
  crates/bot/Cargo.toml crates/bot/src/sync.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(platform): move platform store out of right-core"
```


## Task 5: Docs And Verification

**Files:**

- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/modules.md`
- Modify: `docs/architecture/lifecycle.md`
- Temporary probe edits only; revert them before finishing.

- [ ] **Step 1: Update architecture docs**

In `ARCHITECTURE.md`, update the workspace count and table for the four new crates. `right-core` role becomes:

```markdown
| **right-core** | `crates/right-core/` | Shared config and error primitives pending Phase 4 |
```

Add rows:

```markdown
| **right-ui** | `crates/right-ui/` | Brand-conformant CLI atoms, blocks, recaps, prompts, and theme detection |
| **right-process** | `crates/right-process/` | Cancel-safe process-group child handling |
| **right-openshell** | `crates/right-openshell/` | OpenShell gRPC/proto, CLI wrappers, sandbox exec, and live-test support |
| **right-platform-store** | `crates/right-platform-store/` | Content-addressed platform-managed sandbox file deployment |
```

Update the live-sandbox test section to use `right_openshell::test_support::TestSandbox` and the dev-dependency example to use `right-openshell` with `features = ["test-support"]`.

Update the brand UI rule to use `right_ui::*` and `crates/right-ui/src/`.

- [ ] **Step 2: Update module docs**

In `docs/architecture/modules.md`, remove UI/OpenShell/platform-store/process bullets from `right-core`. Add sections for `right-ui`, `right-process`, `right-openshell`, and `right-platform-store` matching the crate responsibilities above.

In `docs/architecture/lifecycle.md`, update prose that names platform-store or OpenShell helper ownership so it references `right-platform-store` and `right-openshell`.

- [ ] **Step 3: Run full build and tests**

Run:

```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo test --workspace
```

Expected: both commands PASS.

- [ ] **Step 4: Verify stale moved paths are gone**

Run:

```bash
devenv shell -- rg -n "right_core::ui|right_core::process_group|right_core::openshell|right_core::openshell_proto|right_core::sandbox_exec|right_core::test_cleanup|right_core::test_support|right_core::platform_store|pub mod ui|pub mod process_group|pub mod openshell|pub mod openshell_proto|pub mod sandbox_exec|pub mod test_cleanup|pub mod test_support|pub mod platform_store" crates ARCHITECTURE.md docs/architecture
```

Expected: no stale Phase 3 ownership paths.

- [ ] **Step 5: Verify `right-core` is reduced to config/error**

Run:

```bash
devenv shell -- sed -n '1,120p' crates/right-core/src/lib.rs
devenv shell -- rg --files crates/right-core/src
```

Expected `crates/right-core/src/lib.rs` only exports:

```rust
pub mod config;
pub mod error;
```

Expected `crates/right-core/src` contains only `lib.rs`, `error.rs`, and `config/mod.rs`.

- [ ] **Step 6: Run compile fan-out probes**

Temporarily edit one doc comment in each owner crate and run `devenv shell -- cargo build --workspace -vv` after each edit:

```text
crates/right-ui/src/theme.rs
crates/right-process/src/lib.rs
crates/right-openshell/src/openshell.rs
crates/right-platform-store/src/lib.rs
```

Expected:

- UI edit rebuilds `right-ui` and UI consumers, not `right-core`, `right-db`, `right-mcp`, `right-memory`, or `right-codegen`.
- Process edit rebuilds `right-process`, `right-openshell`, and direct process consumers, not `right-core`.
- OpenShell edit rebuilds `right-openshell` and real OpenShell consumers, not `right-core`, `right-db`, `right-mcp`, or `right-memory`.
- Platform-store edit rebuilds `right-platform-store` and `right-bot`, not `right-core`, `right-codegen`, `right-agent`, `right-db`, `right-mcp`, or `right-memory`.

Revert only the temporary probe edits with `apply_patch`, then rerun:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 7: Commit docs**

Run:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/modules.md docs/architecture/lifecycle.md
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "docs(architecture): document phase three core split"
```

Expected: only docs files are staged.

- [ ] **Step 8: Confirm clean worktree**

Run:

```bash
devenv shell -- git status --short
```

Expected: no uncommitted changes from Phase 3.
