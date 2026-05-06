# Crate Split Stage B — Extract `right-core` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carve the stable platform-foundation modules out of `right-agent` into a new `right-core` crate. After this stage, `tonic-prost-build` runs once in `right-core/build.rs` and never re-runs from leaf-crate edits, and `right-agent` shrinks by ~6.5k LoC.

**Architecture:** Create `crates/right-core/` as the bottom-of-stack crate. Move pure-utility modules (`error`, `ui`, `config`, `process_group`, `sandbox_exec`, `test_cleanup`), the openshell stack (proto, generated code, gRPC client, `test_support`), `platform_store`, `stt` (with `WhisperModel`), and the two `IDLE_THRESHOLD_*` constants. Leave compatibility re-exports in `right-agent::*` so external callsites keep compiling. Bot and CLI crates add `right-core` as a direct dep so they can pick `right_core::*` paths over the re-exports — final cleanup of those re-exports happens in Stage F.

**Tech Stack:** Rust 2024, Cargo workspace, `tonic-prost-build` 0.14 (build script), existing workspace deps. Spec at `docs/superpowers/specs/2026-05-06-crate-split-design.md` (commit `16429d54`). All commands run via `devenv shell -- <cmd>` because the project's CLAUDE.md mandates it when `devenv.nix` exists at repo root.

**Pre-existing context:**
- `crates/right-agent/src/lib.rs` declares 22 top-level modules. After this stage, the modules listed in this plan move to `right-core`; the rest stay.
- `crates/right-agent/src/error.rs` (112 LoC) — `display_error_chain` helper + `AgentError` thiserror enum. No internal `crate::*` deps.
- `crates/right-agent/src/process_group.rs` (301 LoC), `sandbox_exec.rs` (58), `test_cleanup.rs` (93) — zero internal `crate::*` deps. Cleanest moves.
- `crates/right-agent/src/config/mod.rs` — also zero internal deps. Standalone YAML config parsing.
- `crates/right-agent/src/ui/` (15 files, ≈3.2k LoC) — only intra-`ui::*` deps. Fully self-contained subtree.
- `crates/right-agent/src/platform_store.rs` (439 LoC) + `platform_store_tests.rs` (171 LoC) — zero internal deps. Two consumers in workspace: `crates/right-agent/src/codegen/pipeline.rs` and `crates/bot/src/sync.rs`.
- `crates/right-agent/proto/openshell/` (3 .proto files), `crates/right-agent/build.rs` (`tonic_prost_build::configure().compile_protos(...)`) — must move together.
- `crates/right-agent/src/openshell.rs` (1701 LoC) + `openshell_tests.rs` (1064 LoC) — only depends on `crate::openshell_proto::*`. Tests use `crate::test_support::TestSandbox` (also moves) and `crate::test_cleanup` (also moves).
- `crates/right-agent/src/stt.rs` (284 LoC) imports `crate::agent::types::WhisperModel`. **Spec deviation**: `WhisperModel` (70 LoC enum, `crates/right-agent/src/agent/types.rs:347-415`) **also moves** to `right_core::stt::WhisperModel`. `right_agent::agent::types` re-exports it. Without this, `stt.rs` cannot move.
- `crates/right-agent/src/test_support.rs` (143 LoC) imports `crate::openshell` and `crate::test_cleanup` — both move. The `feature = "test-support"` declaration in `crates/right-agent/Cargo.toml` migrates to `crates/right-core/Cargo.toml`.
- `crates/right-agent/src/tunnel/health.rs` imports `crate::runtime::pc_client::PcClient`. Since `runtime/*` stays in slim `right-agent` per spec, **`tunnel/` cannot move to `right-core` without creating a cycle**. **Spec deviation**: `tunnel/` stays in `right-agent` for Stage B (90 LoC total, 0 hot-edits per `git log`).
- `crates/right-agent/src/cron_spec.rs:48-51` defines `IDLE_THRESHOLD_SECS: i64 = 180` and `IDLE_THRESHOLD_MIN: i64 = IDLE_THRESHOLD_SECS / 60`. Used by `crates/right-agent/src/codegen/skills.rs:10` (template substitution) and `crates/bot/src/cron_delivery.rs:203,289-296` (engine logic). They move to `right_core::time_constants`; `cron_spec` re-exports them via `pub use` for compat.
- `crates/right-agent/Cargo.toml` already has `[features] test-support = []` and `[build-dependencies] tonic-prost-build = "0.14"`. After Stage B, both move to `right-core`.
- Workspace `Cargo.toml` currently lists 4 members (after Stage A added `right-db`). We add a fifth: `crates/right-core`.

**Verification commands** (run from repo root):
- Build: `devenv shell -- cargo build --workspace`
- Test: `devenv shell -- cargo test --workspace`
- Lint: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
- Single-package check: `devenv shell -- cargo check -p <name>`

---

## Task 1: Create the `right-core` crate skeleton

**Files:**
- Create: `crates/right-core/Cargo.toml`
- Create: `crates/right-core/src/lib.rs`

- [ ] **Step 1: Create `Cargo.toml`**

Create `crates/right-core/Cargo.toml`:

```toml
[package]
name = "right-core"
version.workspace = true
edition.workspace = true

[features]
test-support = []

[dependencies]
miette = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
serde-saphyr = { workspace = true }
chrono = { workspace = true }
dirs = { workspace = true }
tokio = { workspace = true }
reqwest = { workspace = true }
url = { workspace = true }
walkdir = { workspace = true }
sha2 = { workspace = true }
fs4 = { workspace = true }
nix = { workspace = true }
tonic = { workspace = true }
tonic-prost = { workspace = true }
prost = { workspace = true }
prost-types = { workspace = true }
which = { workspace = true }
owo-colors = { workspace = true }
inquire = { workspace = true }
http = { workspace = true }
hyper-util = { workspace = true }

[build-dependencies]
tonic-prost-build = "0.14"

[dev-dependencies]
tempfile = { workspace = true }
right-core = { path = ".", features = ["test-support"] }
```

(The dependency list mirrors the union of what the moved modules need. We can prune dead deps in Stage F when the dust settles.)

- [ ] **Step 2: Create `lib.rs` stub**

Create `crates/right-core/src/lib.rs`:

```rust
//! Stable platform-foundation modules for `right`.
//!
//! Bottom-of-stack crate. Other crates depend on it; it depends on
//! nothing in this workspace. Modules here change rarely — incremental
//! edits to `right-codegen`, `right-memory`, `right-mcp`, or
//! `right-cc` should not invalidate this crate's build cache.
```

(Modules are added one per task in subsequent steps; we start empty so each addition compiles in isolation.)

- [ ] **Step 3: Verify the empty crate compiles standalone**

Run: `devenv shell -- cargo check --manifest-path crates/right-core/Cargo.toml`

Expected: fails because `right-core` is not yet in the workspace. That's fine — Task 2 wires it in.

- [ ] **Step 4: Commit**

```bash
git add crates/right-core/
git commit -m "feat(right-core): scaffold new platform-foundation crate"
```

---

## Task 2: Add `right-core` to the workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add `right-core` to members**

In repo-root `Cargo.toml`, edit `[workspace] members = [...]` to insert `"crates/right-core"` so the line reads (after Stage A this list already contains `right-db`):

```toml
[workspace]
members = ["crates/right-agent", "crates/right-core", "crates/right-db", "crates/right", "crates/bot"]
resolver = "3"
```

- [ ] **Step 2: Verify the empty crate builds**

Run: `devenv shell -- cargo build -p right-core`
Expected: succeeds (empty `lib.rs`).

- [ ] **Step 3: Verify the workspace as a whole still builds**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat(workspace): register right-core crate"
```

---

## Task 3: Wire `right-agent`, `right-bot`, and `right` to depend on `right-core`

**Files:**
- Modify: `crates/right-agent/Cargo.toml`
- Modify: `crates/bot/Cargo.toml`
- Modify: `crates/right/Cargo.toml`

- [ ] **Step 1: Add `right-core` to `right-agent` deps and dev-deps**

In `crates/right-agent/Cargo.toml`, in the `[dependencies]` section, add (alphabetically — between `rand` and `reqwest`):

```toml
right-core = { path = "../right-core" }
```

In the `[dev-dependencies]` section, replace the existing line:

```toml
right-agent = { path = ".", features = ["test-support"] }
```

with:

```toml
right-agent = { path = ".", features = ["test-support"] }
right-core = { path = "../right-core", features = ["test-support"] }
```

(We keep the `right-agent` self-dep until Task 17 removes the `test-support` feature from `right-agent`.)

- [ ] **Step 2: Add `right-core` to `right-bot` deps and dev-deps**

In `crates/bot/Cargo.toml`, in `[dependencies]`, add (alphabetically):

```toml
right-core = { path = "../right-core" }
```

In `[dev-dependencies]`, if there is an entry like `right-agent = { path = "...", features = ["test-support"] }`, add an additional line:

```toml
right-core = { path = "../right-core", features = ["test-support"] }
```

If `[dev-dependencies]` does not enable `test-support` for any crate, only add the plain `right-core = { path = "../right-core" }` line in `[dev-dependencies]` (some bot tests use `right_core` types directly).

- [ ] **Step 3: Add `right-core` to `right` CLI deps**

In `crates/right/Cargo.toml`, in `[dependencies]`, add (alphabetically):

```toml
right-core = { path = "../right-core" }
```

In `[dev-dependencies]`, add `right-core = { path = "../right-core", features = ["test-support"] }` if the CLI crate has integration tests that call `TestSandbox` (else the plain `right-core = { path = "../right-core" }`).

- [ ] **Step 4: Verify the workspace still builds (right-core is empty so this is just a dep-graph update)**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/Cargo.toml crates/bot/Cargo.toml crates/right/Cargo.toml Cargo.lock
git commit -m "deps: wire right-core path dep into agent, bot, cli"
```

---

## Task 4: Move `error.rs` to `right-core`

**Files:**
- Move: `crates/right-agent/src/error.rs` → `crates/right-core/src/error.rs`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Move the file**

Run:

```bash
git mv crates/right-agent/src/error.rs crates/right-core/src/error.rs
```

- [ ] **Step 2: Declare the module in `right-core`**

Open `crates/right-core/src/lib.rs` and append:

```rust
pub mod error;
```

- [ ] **Step 3: Replace the `pub mod error;` line in `right-agent` with a re-export**

Open `crates/right-agent/src/lib.rs`. Replace the line:

```rust
pub mod error;
```

with:

```rust
pub use right_core::error;
```

- [ ] **Step 4: Verify `right-core` builds**

Run: `devenv shell -- cargo build -p right-core`
Expected: succeeds.

- [ ] **Step 5: Verify `right-agent` builds**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds. Internal callers of `crate::error::*` still resolve via `crate::error` because the re-export creates `right_agent::error` as a path alias to `right_core::error`.

- [ ] **Step 6: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/error.rs crates/right-agent/src/lib.rs crates/right-agent/src/error.rs
git commit -m "refactor(right-core): move error module from right-agent"
```

---

## Task 5: Move `process_group.rs` to `right-core`

**Files:**
- Move: `crates/right-agent/src/process_group.rs` → `crates/right-core/src/process_group.rs`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Move the file**

```bash
git mv crates/right-agent/src/process_group.rs crates/right-core/src/process_group.rs
```

- [ ] **Step 2: Declare in `right-core/src/lib.rs`**

Append:

```rust
#[cfg(unix)]
pub mod process_group;
```

- [ ] **Step 3: Replace `pub mod process_group;` in `right-agent/src/lib.rs` with a re-export**

Replace the existing block:

```rust
#[cfg(unix)]
pub mod process_group;
```

with:

```rust
#[cfg(unix)]
pub use right_core::process_group;
```

- [ ] **Step 4: Verify both crates build**

Run: `devenv shell -- cargo build -p right-core && devenv shell -- cargo build -p right-agent`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/process_group.rs crates/right-agent/src/lib.rs crates/right-agent/src/process_group.rs
git commit -m "refactor(right-core): move process_group module"
```

---

## Task 6: Move `sandbox_exec.rs` to `right-core`

**Files:**
- Move: `crates/right-agent/src/sandbox_exec.rs` → `crates/right-core/src/sandbox_exec.rs`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Move the file**

```bash
git mv crates/right-agent/src/sandbox_exec.rs crates/right-core/src/sandbox_exec.rs
```

- [ ] **Step 2: Declare in `right-core/src/lib.rs`**

Append:

```rust
pub mod sandbox_exec;
```

- [ ] **Step 3: Replace `pub mod sandbox_exec;` in `right-agent/src/lib.rs` with a re-export**

Replace `pub mod sandbox_exec;` with:

```rust
pub use right_core::sandbox_exec;
```

- [ ] **Step 4: Verify both crates build**

Run: `devenv shell -- cargo build -p right-core && devenv shell -- cargo build -p right-agent`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/sandbox_exec.rs crates/right-agent/src/lib.rs crates/right-agent/src/sandbox_exec.rs
git commit -m "refactor(right-core): move sandbox_exec module"
```

---

## Task 7: Move `test_cleanup.rs` to `right-core`

**Files:**
- Move: `crates/right-agent/src/test_cleanup.rs` → `crates/right-core/src/test_cleanup.rs`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Move the file**

```bash
git mv crates/right-agent/src/test_cleanup.rs crates/right-core/src/test_cleanup.rs
```

- [ ] **Step 2: Declare in `right-core/src/lib.rs`**

Append:

```rust
#[cfg(unix)]
pub mod test_cleanup;
```

- [ ] **Step 3: Replace in `right-agent/src/lib.rs`**

Replace:

```rust
#[cfg(unix)]
pub mod test_cleanup;
```

with:

```rust
#[cfg(unix)]
pub use right_core::test_cleanup;
```

- [ ] **Step 4: Verify both crates build**

Run: `devenv shell -- cargo build -p right-core && devenv shell -- cargo build -p right-agent`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/test_cleanup.rs crates/right-agent/src/lib.rs crates/right-agent/src/test_cleanup.rs
git commit -m "refactor(right-core): move test_cleanup module"
```

---

## Task 8: Move `config/` subdir to `right-core`

**Files:**
- Move: `crates/right-agent/src/config/` → `crates/right-core/src/config/`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Move the subdirectory**

```bash
git mv crates/right-agent/src/config crates/right-core/src/config
```

- [ ] **Step 2: Declare in `right-core/src/lib.rs`**

Append:

```rust
pub mod config;
```

- [ ] **Step 3: Replace in `right-agent/src/lib.rs`**

Replace `pub mod config;` with:

```rust
pub use right_core::config;
```

- [ ] **Step 4: Verify both crates build**

Run: `devenv shell -- cargo build -p right-core && devenv shell -- cargo build -p right-agent`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/config crates/right-agent/src/lib.rs crates/right-agent/src/config
git commit -m "refactor(right-core): move config module"
```

---

## Task 9: Move `ui/` subdir to `right-core`

**Files:**
- Move: `crates/right-agent/src/ui/` (15 files) → `crates/right-core/src/ui/`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Move the subdirectory**

```bash
git mv crates/right-agent/src/ui crates/right-core/src/ui
```

- [ ] **Step 2: Declare in `right-core/src/lib.rs`**

Append:

```rust
pub mod ui;
```

- [ ] **Step 3: Replace in `right-agent/src/lib.rs`**

Replace `pub mod ui;` with:

```rust
pub use right_core::ui;
```

- [ ] **Step 4: Verify the ui tests run from `right-core`**

Run: `devenv shell -- cargo test -p right-core --lib ui`
Expected: passes (atoms_tests, line_tests, recap_tests, splash_tests, theme_tests).

- [ ] **Step 5: Verify `right-agent` still builds**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds. Internal `crate::ui::*` callsites resolve through the re-export.

- [ ] **Step 6: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/ui crates/right-agent/src/lib.rs crates/right-agent/src/ui
git commit -m "refactor(right-core): move ui module"
```

---

## Task 10: Move `platform_store.rs` (+ tests) to `right-core`

**Files:**
- Move: `crates/right-agent/src/platform_store.rs` → `crates/right-core/src/platform_store.rs`
- Move: `crates/right-agent/src/platform_store_tests.rs` → `crates/right-core/src/platform_store_tests.rs`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Move both files**

```bash
git mv crates/right-agent/src/platform_store.rs crates/right-core/src/platform_store.rs
git mv crates/right-agent/src/platform_store_tests.rs crates/right-core/src/platform_store_tests.rs
```

- [ ] **Step 2: Declare in `right-core/src/lib.rs`**

Append:

```rust
pub mod platform_store;
```

- [ ] **Step 3: Replace in `right-agent/src/lib.rs`**

Replace `pub mod platform_store;` with:

```rust
pub use right_core::platform_store;
```

- [ ] **Step 4: Verify the moved tests run**

Run: `devenv shell -- cargo test -p right-core --lib platform_store`
Expected: passes. (`platform_store_tests.rs` is wired via `#[cfg(test)] #[path = "platform_store_tests.rs"] mod tests;` inside `platform_store.rs` — that wiring needs no change because both files moved together and the relative path holds.)

- [ ] **Step 5: Verify `right-agent` builds, including `codegen::pipeline` which calls `crate::platform_store::*`**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds.

- [ ] **Step 6: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/platform_store.rs crates/right-core/src/platform_store_tests.rs crates/right-agent/src/lib.rs crates/right-agent/src/platform_store.rs crates/right-agent/src/platform_store_tests.rs
git commit -m "refactor(right-core): move platform_store module"
```

---

## Task 11: Move openshell proto + build script + generated module to `right-core`

**Files:**
- Move: `crates/right-agent/proto/` → `crates/right-core/proto/`
- Move: `crates/right-agent/build.rs` → `crates/right-core/build.rs`
- Modify: `crates/right-core/src/lib.rs` (add `openshell_proto` module)
- Modify: `crates/right-agent/src/lib.rs` (replace `openshell_proto` definition with re-export)
- Modify: `crates/right-agent/Cargo.toml` (drop `[build-dependencies] tonic-prost-build`)

- [ ] **Step 1: Move proto files and build script**

```bash
git mv crates/right-agent/proto crates/right-core/proto
git mv crates/right-agent/build.rs crates/right-core/build.rs
```

- [ ] **Step 2: Add `openshell_proto` module declaration in `right-core/src/lib.rs`**

Append to `crates/right-core/src/lib.rs`:

```rust
/// Generated protobuf types for the OpenShell gRPC API.
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
```

- [ ] **Step 3: Replace the `openshell_proto` block in `right-agent/src/lib.rs` with a re-export**

Replace the existing block (the entire `pub mod openshell_proto { ... }` region — currently `crates/right-agent/src/lib.rs:26-44`) with:

```rust
pub use right_core::openshell_proto;
```

- [ ] **Step 4: Drop `tonic-prost-build` from `right-agent`**

In `crates/right-agent/Cargo.toml`, delete the entire `[build-dependencies]` section (it only contained `tonic-prost-build = "0.14"`).

- [ ] **Step 5: Verify `right-core` builds — its `build.rs` regenerates protos**

Run: `devenv shell -- cargo build -p right-core`
Expected: succeeds. `OUT_DIR` for `right-core` now contains the generated proto code.

- [ ] **Step 6: Verify `right-agent` builds — its `crate::openshell_proto::*` paths still resolve via re-export**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds. The 8+ callsites in `openshell.rs`, `openshell_tests.rs`, and `doctor.rs` (using `crate::openshell_proto::*`) all reach the re-exported alias.

- [ ] **Step 7: Commit**

```bash
git add crates/right-core/proto crates/right-core/build.rs crates/right-core/src/lib.rs crates/right-agent/src/lib.rs crates/right-agent/build.rs crates/right-agent/Cargo.toml crates/right-agent/proto Cargo.lock
git commit -m "refactor(right-core): move openshell proto + build.rs"
```

---

## Task 12: Move `openshell.rs` (+ tests) to `right-core`

**Files:**
- Move: `crates/right-agent/src/openshell.rs` → `crates/right-core/src/openshell.rs`
- Move: `crates/right-agent/src/openshell_tests.rs` → `crates/right-core/src/openshell_tests.rs`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Move both files**

```bash
git mv crates/right-agent/src/openshell.rs crates/right-core/src/openshell.rs
git mv crates/right-agent/src/openshell_tests.rs crates/right-core/src/openshell_tests.rs
```

- [ ] **Step 2: Declare in `right-core/src/lib.rs`**

Append:

```rust
pub mod openshell;
```

- [ ] **Step 3: Replace in `right-agent/src/lib.rs`**

Replace `pub mod openshell;` with:

```rust
pub use right_core::openshell;
```

- [ ] **Step 4: Verify `right-core` compiles — `openshell.rs` references `crate::openshell_proto`, which is in the same crate, so paths resolve**

Run: `devenv shell -- cargo check -p right-core`
Expected: succeeds.

- [ ] **Step 5: Run the openshell test suite from `right-core`**

Run: `devenv shell -- cargo test -p right-core --lib openshell`
Expected: passes. (The tests use `crate::test_support::TestSandbox` — but `test_support` hasn't moved yet. If this errors with "cannot find module test_support", proceed; Task 14 unblocks. To verify the build is clean, fall back to `cargo check -p right-core --features test-support` for now.)

If the openshell tests block on `test_support` not yet existing in `right-core`:

```rust
// inside crates/right-core/src/openshell_tests.rs near top
#[cfg(test)]
use right_agent::test_support::TestSandbox;
```

is **forbidden** because that creates a cycle. Instead, defer the test-running verification to Task 14.

- [ ] **Step 6: Verify `right-agent` builds — internal `crate::openshell::*` callers resolve via re-export**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds. `doctor.rs` and other right-agent modules calling `crate::openshell::*` see the re-exported alias.

- [ ] **Step 7: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/openshell.rs crates/right-core/src/openshell_tests.rs crates/right-agent/src/lib.rs crates/right-agent/src/openshell.rs crates/right-agent/src/openshell_tests.rs
git commit -m "refactor(right-core): move openshell gRPC client + tests"
```

---

## Task 13: Move `WhisperModel` enum to `right-core::stt`

**Files:**
- Modify: `crates/right-agent/src/agent/types.rs` (move `WhisperModel` definition + remove its impls)
- Create (will be filled in Task 14): `crates/right-core/src/stt.rs`
- Add: a stub `right-core/src/stt.rs` ahead of Task 14

- [ ] **Step 1: Create the `stt` module in `right-core` with just `WhisperModel`**

Create `crates/right-core/src/stt.rs`:

```rust
//! Whisper model identification + cache-path helpers.
//!
//! Inference (whisper-rs) is NOT here — it lives in `right-bot::stt`.
//! This module exposes only the model enum, default download URLs,
//! approximate sizes, and `model_cache_path` / `download_model`.

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperModel {
    Tiny,
    Base,
    #[default]
    Small,
    Medium,
    #[serde(rename = "large-v3")]
    LargeV3,
}

impl WhisperModel {
    pub fn filename(&self) -> &'static str {
        match self {
            Self::Tiny => "ggml-tiny.bin",
            Self::Base => "ggml-base.bin",
            Self::Small => "ggml-small.bin",
            Self::Medium => "ggml-medium.bin",
            Self::LargeV3 => "ggml-large-v3.bin",
        }
    }

    pub fn download_url(&self) -> &'static str {
        match self {
            Self::Tiny => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            Self::Base => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            Self::Small => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
            }
            Self::Medium => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
            }
            Self::LargeV3 => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"
            }
        }
    }

    pub fn approx_size_mb(&self) -> u64 {
        match self {
            Self::Tiny => 75,
            Self::Base => 150,
            Self::Small => 470,
            Self::Medium => 1500,
            Self::LargeV3 => 3100,
        }
    }

    /// Kebab-case YAML string for this model — mirrors serde's rename_all output.
    pub fn yaml_str(self) -> &'static str {
        match self {
            Self::Tiny => "tiny",
            Self::Base => "base",
            Self::Small => "small",
            Self::Medium => "medium",
            Self::LargeV3 => "large-v3",
        }
    }
}
```

(Copy verbatim — keep all impls. Verify against the source file `crates/right-agent/src/agent/types.rs:347-415` to ensure parity. If `WhisperModel` has more impls than shown above in your local checkout, copy them all.)

- [ ] **Step 2: Declare the new module in `right-core/src/lib.rs`**

Append:

```rust
pub mod stt;
```

- [ ] **Step 3: Verify `right-core` still builds**

Run: `devenv shell -- cargo build -p right-core`
Expected: succeeds.

- [ ] **Step 4: Replace the `WhisperModel` definition in `right-agent::agent::types` with a re-export**

Open `crates/right-agent/src/agent/types.rs`. Locate the `WhisperModel` definition (the `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, ...)]` block beginning at the line before `pub enum WhisperModel { ... }` and including all `impl WhisperModel` blocks). Delete that entire region.

In its place insert:

```rust
pub use right_core::stt::WhisperModel;
```

- [ ] **Step 5: Verify `right-agent` builds**

Run: `devenv shell -- cargo build -p right-agent`
Expected: succeeds. Existing callers `crate::agent::types::WhisperModel` and external `right_agent::agent::types::WhisperModel` callsites both resolve through the re-export.

- [ ] **Step 6: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/stt.rs crates/right-agent/src/agent/types.rs
git commit -m "refactor(right-core): host WhisperModel enum"
```

---

## Task 14: Move `stt.rs` to `right-core::stt` (merge into existing stub)

**Files:**
- Modify: `crates/right-core/src/stt.rs` (extend with content from `right-agent::stt`)
- Delete: `crates/right-agent/src/stt.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Append the contents of `right-agent::stt` into `right-core::stt`**

Open `crates/right-agent/src/stt.rs`. Take everything below the imports section (the `model_cache_path`, `download_model`, etc. function bodies) and append it to `crates/right-core/src/stt.rs` below the `WhisperModel` impl block.

Adjust imports inside the appended content:
- `use crate::agent::types::WhisperModel;` → delete (already in scope as `WhisperModel` is in the same module).

If the file has additional `use crate::*` lines, replace `crate::*` with the appropriate `right_core::*` path or absolute path.

- [ ] **Step 2: Delete the original `crates/right-agent/src/stt.rs`**

```bash
git rm crates/right-agent/src/stt.rs
```

- [ ] **Step 3: Replace the module declaration in `right-agent/src/lib.rs`**

Replace `pub mod stt;` with:

```rust
pub use right_core::stt;
```

- [ ] **Step 4: Verify both crates build**

Run: `devenv shell -- cargo build -p right-core && devenv shell -- cargo build -p right-agent`
Expected: succeeds.

- [ ] **Step 5: Verify external callers (bot, CLI) still compile against `right_agent::stt::*`**

Run: `devenv shell -- cargo build -p right-bot && devenv shell -- cargo build -p right`
Expected: succeeds. `crates/bot/src/stt/mod.rs:149` calls `right_agent::stt::download_model` — resolves via re-export.

- [ ] **Step 6: Commit**

```bash
git add crates/right-core/src/stt.rs crates/right-agent/src/lib.rs crates/right-agent/src/stt.rs
git commit -m "refactor(right-core): move stt module from right-agent"
```

---

## Task 15: Move `test_support.rs` and the `test-support` feature

**Files:**
- Move: `crates/right-agent/src/test_support.rs` → `crates/right-core/src/test_support.rs`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-agent/src/lib.rs`
- Modify: `crates/right-agent/Cargo.toml` (remove `[features] test-support = []`)

- [ ] **Step 1: Move the file**

```bash
git mv crates/right-agent/src/test_support.rs crates/right-core/src/test_support.rs
```

- [ ] **Step 2: Update imports inside the moved file**

Open `crates/right-core/src/test_support.rs`. The two `use crate::*` lines at the top are:

```rust
use crate::openshell;
use crate::test_cleanup;
```

Both modules are now in `right-core`, so the paths still work — no edit needed. (Sanity-check by reading the file; if any other `crate::*` import appears, it must resolve to a `right-core` module.)

- [ ] **Step 3: Declare in `right-core/src/lib.rs`**

Append:

```rust
#[cfg(all(unix, any(test, feature = "test-support")))]
pub mod test_support;
```

- [ ] **Step 4: Replace in `right-agent/src/lib.rs`**

Replace:

```rust
#[cfg(all(unix, any(test, feature = "test-support")))]
pub mod test_support;
```

with:

```rust
#[cfg(all(unix, any(test, feature = "test-support")))]
pub use right_core::test_support;
```

- [ ] **Step 5: Remove `test-support` feature from `right-agent/Cargo.toml`**

Open `crates/right-agent/Cargo.toml`. Delete the entire `[features]` section (it only contained `test-support = []`).

In `[dev-dependencies]`, remove the line:

```toml
right-agent = { path = ".", features = ["test-support"] }
```

The feature now lives only in `right-core` (added in Task 1, enabled by `right-agent`'s dev-deps via `right-core = { path = "...", features = ["test-support"] }` — already added in Task 3).

- [ ] **Step 6: Update the `right-agent::lib.rs` `#[cfg]` so the re-export aligns with the dev-dep feature**

Because the `feature = "test-support"` no longer exists on `right-agent`, the gate must check `right-core`'s feature instead. The cleanest form is to gate via `cfg(any(test, feature = "test-support"))` on the **`right-core` feature** through a transitive feature flag declared on `right-agent` — but that re-introduces the feature on `right-agent`. Simpler: keep the re-export gated only by `cfg(test)` so that `right-agent`'s own tests (none currently use `TestSandbox` directly) still see it, and rely on external users importing `right_core::test_support::TestSandbox` directly going forward.

Concretely, replace the line you just inserted in Step 4 with:

```rust
#[cfg(all(unix, test))]
pub use right_core::test_support;
```

- [ ] **Step 7: Update external callers that use `right_agent::test_support::*` directly to switch to `right_core::test_support::*`**

Inventory:

```bash
devenv shell -- rg -n 'right_agent::test_support' crates 2>/dev/null
```

Expected callsites (today):
- `crates/right-agent/tests/control_master.rs`
- `crates/right-agent/tests/rebootstrap_sandbox.rs`
- `crates/bot/tests/sandbox_upgrade.rs`

For each file, replace `right_agent::test_support::` with `right_core::test_support::`:

```bash
devenv shell -- rg -l 'right_agent::test_support' crates \
  | xargs sed -i.bak 's|right_agent::test_support|right_core::test_support|g'
devenv shell -- find crates -name '*.bak' -delete
```

- [ ] **Step 8: Verify all crates build, tests compile**

Run:

```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo test --workspace --no-run
```

Expected: succeeds.

- [ ] **Step 9: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/test_support.rs crates/right-agent/src/lib.rs crates/right-agent/src/test_support.rs crates/right-agent/Cargo.toml crates/right-agent/tests crates/bot/tests Cargo.lock
git commit -m "refactor(right-core): move test_support + relocate feature flag"
```

---

## Task 16: Move `IDLE_THRESHOLD_*` constants to `right-core::time_constants`

**Files:**
- Create: `crates/right-core/src/time_constants.rs`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-agent/src/cron_spec.rs`

- [ ] **Step 1: Create `crates/right-core/src/time_constants.rs`**

```rust
//! Time-related constants that cross crate boundaries.
//!
//! `IDLE_THRESHOLD_SECS` is user-meaningful: it answers "why didn't the cron
//! notification arrive yet?" — pending notifications are held until the chat
//! has been idle for this long (within CC's 5-min prompt cache TTL).

/// Idle threshold (seconds) before pending cron notifications are delivered.
pub const IDLE_THRESHOLD_SECS: i64 = 180;

/// Human-readable form for prose ("3 min" reads better than "180 s").
pub const IDLE_THRESHOLD_MIN: i64 = IDLE_THRESHOLD_SECS / 60;
```

- [ ] **Step 2: Declare in `right-core/src/lib.rs`**

Append:

```rust
pub mod time_constants;
```

- [ ] **Step 3: Replace the constants in `right-agent::cron_spec` with re-exports**

Open `crates/right-agent/src/cron_spec.rs`. Locate lines 45-51 (the doc-comment block plus the two `pub const` lines). Replace them with:

```rust
pub use right_core::time_constants::{IDLE_THRESHOLD_MIN, IDLE_THRESHOLD_SECS};
```

- [ ] **Step 4: Verify `cron_spec.rs:55-63` (the `TRIGGER_TOOL_DESC` const_format usage) still compiles**

`TRIGGER_TOOL_DESC` uses `const_format::formatcp!` to embed `IDLE_THRESHOLD_MIN`. Re-exports work in `const` contexts as long as the source const is itself `pub const` — verify. Run:

```bash
devenv shell -- cargo build -p right-agent
```

Expected: succeeds. If `const_format` complains, the workaround is to redefine the constant locally (as a `pub use` re-export *should* work in const context per Rust semantics, but if any toolchain edge case triggers, fall back to: keep `IDLE_THRESHOLD_SECS`/`IDLE_THRESHOLD_MIN` defined locally in `cron_spec.rs` AND in `right-core::time_constants`, with a debug_assert at startup that they agree).

- [ ] **Step 5: Update external callers in `bot::cron_delivery`**

`crates/bot/src/cron_delivery.rs:203` currently has:

```rust
use right_agent::cron_spec::IDLE_THRESHOLD_SECS;
```

This still works via the re-export. No edit required for Stage B. (Final cleanup at Stage F may switch this to `right_core::time_constants::IDLE_THRESHOLD_SECS`.)

- [ ] **Step 6: Commit**

```bash
git add crates/right-core/src/lib.rs crates/right-core/src/time_constants.rs crates/right-agent/src/cron_spec.rs
git commit -m "refactor(right-core): move IDLE_THRESHOLD constants"
```

---

## Task 17: Update `right-bot` to depend directly on `right-core` for `platform_store` callsite

**Files:**
- Modify: `crates/bot/src/sync.rs`

(The dep was already added in Task 3 Step 2; this task switches the source code path.)

- [ ] **Step 1: Inventory bot's `right_agent::*` callsites that should use `right_core::*`**

Run:

```bash
devenv shell -- rg -n 'right_agent::platform_store|right_agent::ui|right_agent::config|right_agent::error|right_agent::process_group|right_agent::sandbox_exec|right_agent::stt|right_agent::openshell\b|right_agent::openshell_proto|right_agent::test_cleanup' crates/bot/src 2>/dev/null
```

Expected: a small list (chiefly `crates/bot/src/sync.rs:61,64` calling `right_agent::platform_store::*`, plus possibly a handful of `right_agent::ui::*` lines).

- [ ] **Step 2: Replace `right_agent::<moved-module>::` with `right_core::<moved-module>::` in `crates/bot/src/`**

Use one bulk sed pass per module name:

```bash
for mod in platform_store ui config error process_group sandbox_exec stt openshell openshell_proto test_cleanup; do
  devenv shell -- rg -l "right_agent::${mod}\b" crates/bot/src \
    | xargs sed -i.bak "s|right_agent::${mod}|right_core::${mod}|g" 2>/dev/null
done
devenv shell -- find crates/bot/src -name '*.bak' -delete
```

- [ ] **Step 3: Build the bot**

Run: `devenv shell -- cargo build -p right-bot`
Expected: succeeds.

- [ ] **Step 4: Run bot lib tests**

Run: `devenv shell -- cargo test -p right-bot --lib`
Expected: passes.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src
git commit -m "refactor(bot): switch to right-core for moved modules"
```

---

## Task 18: Update `right` CLI to use `right_core::*` paths

**Files:**
- Modify: files under `crates/right/src/` that reference moved modules.

- [ ] **Step 1: Inventory CLI callsites**

Run:

```bash
devenv shell -- rg -n 'right_agent::platform_store|right_agent::ui|right_agent::config|right_agent::error|right_agent::process_group|right_agent::sandbox_exec|right_agent::stt|right_agent::openshell\b|right_agent::openshell_proto|right_agent::test_cleanup' crates/right/src 2>/dev/null
```

Expected: many hits, especially in `main.rs` (handler paths), `wizard.rs` (UI atoms), `aggregator.rs`.

- [ ] **Step 2: Bulk replace**

```bash
for mod in platform_store ui config error process_group sandbox_exec stt openshell openshell_proto test_cleanup; do
  devenv shell -- rg -l "right_agent::${mod}\b" crates/right/src \
    | xargs sed -i.bak "s|right_agent::${mod}|right_core::${mod}|g" 2>/dev/null
done
devenv shell -- find crates/right/src -name '*.bak' -delete
```

- [ ] **Step 3: Build the CLI**

Run: `devenv shell -- cargo build -p right`
Expected: succeeds.

- [ ] **Step 4: Run CLI lib + bin tests**

Run: `devenv shell -- cargo test -p right --lib --bins`
Expected: passes.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src
git commit -m "refactor(right): switch CLI to right-core for moved modules"
```

---

## Task 19: Update `right-agent` integration tests (`crates/right-agent/tests/*`) to use `right_core::*`

**Files:**
- Modify: `crates/right-agent/tests/*.rs`

- [ ] **Step 1: Inventory**

Run:

```bash
devenv shell -- rg -n 'right_agent::platform_store|right_agent::ui|right_agent::config|right_agent::error|right_agent::process_group|right_agent::sandbox_exec|right_agent::stt|right_agent::openshell\b|right_agent::openshell_proto|right_agent::test_cleanup' crates/right-agent/tests 2>/dev/null
```

- [ ] **Step 2: Bulk replace**

```bash
for mod in platform_store ui config error process_group sandbox_exec stt openshell openshell_proto test_cleanup; do
  devenv shell -- rg -l "right_agent::${mod}\b" crates/right-agent/tests \
    | xargs sed -i.bak "s|right_agent::${mod}|right_core::${mod}|g" 2>/dev/null
done
devenv shell -- find crates/right-agent/tests -name '*.bak' -delete
```

- [ ] **Step 3: Run integration tests**

Run: `devenv shell -- cargo test -p right-agent --tests`
Expected: passes (live-sandbox tests run against the dev machine's OpenShell per CLAUDE.md).

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/tests
git commit -m "test(right-agent): switch integration tests to right-core paths"
```

---

## Task 20: Whole-workspace build, test, lint pass

**Files:** none (verification only)

- [ ] **Step 1: Whole-workspace build (debug)**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds with zero warnings.

- [ ] **Step 2: Whole-workspace build (release)**

Run: `devenv shell -- cargo build --workspace --release`
Expected: succeeds.

- [ ] **Step 3: Whole-workspace test**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass, including `TestSandbox`-using integration tests.

- [ ] **Step 4: Whole-workspace clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 5: Build-time benchmark**

Run:

```bash
devenv shell -- cargo clean
devenv shell -- cargo build --workspace --timings
```

Save the wall-clock output and the resulting `target/cargo-timings/cargo-timing-*.html` to `~/Desktop/stage-b-timing.html` (or any external path) for comparison against the Stage A baseline. The expected outcome is a measurable reduction in incremental build time when editing `crates/right-agent/src/codegen/*` (since `tonic-prost-build` no longer re-runs from such edits).

Document the wall-clock numbers in the commit message.

- [ ] **Step 6: If any of the above fails, fix in-place**

Common failure modes:
- Dangling `crate::error::*` import in `right-agent` that bypassed Step 4's re-export — locate via `rg 'crate::error' crates/right-agent/src`.
- A `right_agent::test_support::TestSandbox` import not caught by sed because the path was multi-line — locate via `rg -A1 'right_agent::test_support' crates`.
- `WhisperModel` impls drift — diff the moved file against the original to confirm parity.

Commit any fixes:

```bash
git add <fixed files>
git commit -m "fix(stage-b): resolve dangling references after right-core extraction"
```

---

## Task 21: Run `rust-dev:review-rust-code` agent

**Files:** none (review only)

- [ ] **Step 1: Dispatch the review agent**

Use the `rust-dev:review-rust-code` agent with this prompt:

> Review changes on the current branch since `<sha-of-stage-b-start>`. Focus on:
> 1. Correctness of the `right-core` extraction. Did anything important stay behind that should be in core, or move that shouldn't have?
> 2. The transitional re-exports in `right-agent::lib.rs` (`pub use right_core::*`). Are the cfg attributes consistent (e.g. `#[cfg(unix)]` matches between core and the re-export)?
> 3. The `WhisperModel` move — did all impls migrate? Any divergence from the original?
> 4. The `IDLE_THRESHOLD_*` re-export — does `const_format::formatcp!` still successfully embed the value at compile time after the re-export?
> 5. Any place where `crate::*` paths in the moved modules accidentally pointed back into `right-agent` (would create a cycle and break compilation; should be caught by build, but flag for awareness).
>
> Don't fix; report. Output as a TODO list with file:line references.

- [ ] **Step 2: Triage findings**

For each finding:
- Clear bug → add to `docs/superpowers/plans/2026-05-06-stage-b-followups.md` and fix one at a time, each in its own commit.
- Style nitpick → add to the same TODO file but defer.
- Misunderstanding of plan or spec → ignore.

- [ ] **Step 3: Confirm tests still pass after fixes**

Run: `devenv shell -- cargo test --workspace`
Expected: passes.

- [ ] **Step 4: Commit the followup TODO file (if any)**

```bash
git add docs/superpowers/plans/2026-05-06-stage-b-followups.md
git commit -m "docs(stage-b): record review-rust-code followups"
```

---

## Task 22: Update `ARCHITECTURE.md`

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Add `right-core` to the Workspace table**

Open `ARCHITECTURE.md`. In the `## Workspace` section's table, insert a new row at the top (so the dependency-stack order reads top-down):

```markdown
| Crate | Path | Role |
|-------|------|------|
| **right-core** | `crates/right-core/` | Stable platform-foundation — error/ui/config/openshell/proto/platform_store/stt/test_support, time constants |
| **right-db** | `crates/right-db/` | Per-agent SQLite plumbing — `open_connection`, central migration registry |
| **right-agent** | `crates/right-agent/` | Core library — agent discovery, codegen, config, memory, runtime, MCP, tunnel |
| **right** | `crates/right/` | CLI binary (`right`) + MCP Aggregator (HTTP) |
| **right-bot** | `crates/bot/` | Telegram bot runtime + cron engine + login flow |
```

- [ ] **Step 2: Add a brief note about `right-core` ownership**

Below the table, add a paragraph:

```markdown
The `right-core` crate hosts stable platform primitives — error rendering,
brand-conformant UI atoms, OpenShell gRPC client and generated proto types,
process-group / sandbox-exec helpers, configuration parsing, the
`platform_store` content-hashed deployment helper, the `stt` model-download
helper (with `WhisperModel`), and `test_support::TestSandbox`. Modules here
change rarely; edits to leaf crates do not invalidate the build cache here.
The `tonic-prost-build` build script lives in `crates/right-core/build.rs`
and only re-runs when the `.proto` files change.
```

- [ ] **Step 3: Skim for stale `right_agent::<moved>::` references**

Run:

```bash
devenv shell -- rg -n 'right_agent::(error|ui|config|process_group|sandbox_exec|stt|openshell|openshell_proto|platform_store|test_cleanup|test_support)' ARCHITECTURE.md docs/architecture
```

For each hit, decide: keep (if it's documenting a re-export path that's intentionally still working) or update to `right_core::*`. Default: update.

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md docs/architecture
git commit -m "docs(arch): add right-core to workspace map"
```

---

## Task 23: Final verification + summary

**Files:** none (verification + an optional summary commit on top)

- [ ] **Step 1: Re-run the full check suite**

```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo build --workspace --release
devenv shell -- cargo test --workspace
devenv shell -- cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all four pass.

- [ ] **Step 2: Inventory check — no leftover internal references to moved modules**

Run:

```bash
devenv shell -- rg -n 'pub mod (error|ui|config|process_group|sandbox_exec|stt|openshell|platform_store|test_cleanup|test_support);' crates/right-agent/src/lib.rs
```

Expected: empty (every `pub mod <moved>` line was replaced with `pub use right_core::<moved>;`).

Also:

```bash
devenv shell -- rg -n 'use crate::(error|ui|config|process_group|sandbox_exec|stt|openshell|platform_store|test_cleanup|test_support)::' crates/right-agent/src
```

Expected: hits remain — they're internal callers using `crate::*` — and resolve via the re-exports. That is fine; final cleanup at Stage F removes the re-exports and updates these to `right_core::*`.

- [ ] **Step 3: Optional summary commit**

```bash
git commit --allow-empty -m "chore(stage-b): right-core extraction complete"
```

- [ ] **Step 4: Open a PR (if working on a branch)**

Title: `Stage B: extract right-core crate`. Body references the spec at `docs/superpowers/specs/2026-05-06-crate-split-design.md` and this plan. Include the build-timing numbers from Task 20 Step 5.
