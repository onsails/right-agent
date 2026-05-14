# Right-core Decomposition Phase 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract agent configuration DTOs and host-side STT helpers from `right-core` into `right-agent-config` and `right-stt`.

**Architecture:** `right-agent-config` becomes the public source for `agent.yaml` schema types, including `WhisperModel`. `right-stt` owns model cache paths, ffmpeg detection, model download, and cache warming, and depends on `right-agent-config` for the model enum. `right-core` must not re-export the moved modules.

**Tech Stack:** Rust 2024 Cargo workspace, `serde`, `serde-saphyr`, `miette`, `thiserror`, `reqwest`, `futures`, `tokio`, `which`, `devenv shell -- cargo`.

---

## Scope

This plan implements Phase 2 from `docs/superpowers/specs/2026-05-13-right-core-decomposition-design.md`.

In scope:

- Move `crates/right-core/src/agent_types.rs` to `crates/right-agent-config/src/lib.rs`.
- Move `WhisperModel` from `right_core::stt` into `right-agent-config`.
- Move STT cache/download helpers from `crates/right-core/src/stt.rs` to `crates/right-stt/src/lib.rs`.
- Update direct imports for moved items.
- Remove `right_core::agent_types` and `right_core::stt` module paths.
- Update architecture docs touched by the crate split.
- Verify compile fan-out for config and STT edits.

Out of scope:

- Moving `right-core::ui`, `right-core::process_group`, `right-core::openshell`, `right-core::openshell_proto`, `right-core::sandbox_exec`, `right-core::platform_store`, `right-core::test_cleanup`, or `right-core::test_support`.
- Removing `right-core` entirely. That remains Phase 4.
- Changing STT runtime behavior, model URLs, model cache layout, or ffmpeg warning text except import-path-driven edits.

## Execution Notes

- Work in `/Users/molt/dev/rightclaw/.worktrees/right-core-decomposition-phase-1`.
- Prefix commands with `devenv shell --`.
- The `rust-dev:rust-dev` skill is required by project instructions before writing Rust, but it was unavailable in the previous session. If it is still unavailable, state that and follow `AGENTS.rust.md`.
- Use `apply_patch` for manual edits.
- Do not stage or revert unrelated user changes.
- Before every commit, run `devenv shell -- git diff --cached --name-only` and verify only that task's files are staged.

## File Structure

Create:

- `crates/right-agent-config/Cargo.toml` - manifest for agent configuration DTO crate.
- `crates/right-agent-config/src/lib.rs` - owns `AgentConfig`, `AgentDef`, config enums, `SttConfig`, and `WhisperModel`.
- `crates/right-stt/Cargo.toml` - manifest for host-side STT helper crate.
- `crates/right-stt/src/lib.rs` - owns model cache paths, ffmpeg detection, download, and cache warming.

Modify:

- `Cargo.toml` - add `right-agent-config` and `right-stt` workspace members.
- `crates/right-core/Cargo.toml` - remove STT-only dependencies after STT moves; do not keep a `right-agent-config` dependency after Task 3.
- `crates/right-core/src/lib.rs` - remove `agent_types` and `stt` modules.
- `crates/right-core/src/agent_types.rs` - delete after moving to `right-agent-config`.
- `crates/right-core/src/stt.rs` - delete after moving to `right-stt`.
- `crates/right-agent/Cargo.toml` - add `right-agent-config` and `right-stt`.
- `crates/right-agent/src/agent/types.rs` - re-export config types from `right-agent-config`; keep YAML writer helpers in `right-agent`.
- `crates/right-agent/src/doctor.rs` - import STT helpers from `right-stt`.
- `crates/right-codegen/Cargo.toml` - add `right-agent-config`.
- `crates/right-codegen/src/agent_def.rs` - use `right-agent-config` config types.
- `crates/right-codegen/src/agent_def_tests.rs` - use `right-agent-config` config types.
- `crates/right-codegen/src/claude_json.rs` - use `right-agent-config` config types.
- `crates/right-codegen/src/contract.rs` - use `right-agent-config` config types in tests.
- `crates/right-codegen/src/pipeline.rs` - use `right-agent-config` config types.
- `crates/right-codegen/src/policy.rs` - use `right-agent-config::NetworkPolicy`.
- `crates/right-codegen/src/process_compose.rs` - use `right-agent-config` config types.
- `crates/right-codegen/src/process_compose_tests.rs` - use `right-agent-config` config types.
- `crates/right-codegen/src/skills.rs` - use `right-agent-config::MemoryProvider`.
- `crates/right/Cargo.toml` - add `right-agent-config` and `right-stt`.
- `crates/right/src/main.rs` - use `right-stt` helpers and `right_agent_config::WhisperModel` for cache warming.
- `crates/right/src/wizard.rs` - use `right-stt` helpers and `right_agent_config::WhisperModel` for STT setup.
- `crates/bot/Cargo.toml` - add `right-agent-config` and `right-stt`.
- `crates/bot/src/lib.rs` - use `right-stt` helpers for STT context setup.
- `crates/bot/src/stt/mod.rs` - use `right-stt` helpers and `right_agent_config::WhisperModel` in tests.
- `crates/bot/src/stt/whisper.rs` - use `right-stt` helpers and `right_agent_config::WhisperModel` in tests.
- `ARCHITECTURE.md` - update crate table and `right-core` boundary.
- `docs/architecture/modules.md` - update module map.
- `docs/architecture/lifecycle.md` - add the Phase 2 STT/config owner note near the existing STT lifecycle description.

## Task 1: Scaffold Phase 2 Crates

**Files:**

- Modify: `Cargo.toml`
- Create: `crates/right-agent-config/Cargo.toml`
- Create: `crates/right-agent-config/src/lib.rs`
- Create: `crates/right-stt/Cargo.toml`
- Create: `crates/right-stt/src/lib.rs`
- Modify: `Cargo.lock`

- [ ] **Step 1: Run the failing package check**

Run:

```bash
devenv shell -- cargo test -p right-agent-config -p right-stt
```

Expected: FAIL with `package ID specification` because neither package exists.

- [ ] **Step 2: Add workspace members**

In `Cargo.toml`, add the two Phase 2 members after the existing Phase 1 crates:

```toml
"crates/right-agent-config",
"crates/right-stt",
```

- [ ] **Step 3: Create `right-agent-config` manifest and minimal lib**

In `crates/right-agent-config/Cargo.toml`, use:

```toml
[package]
name = "right-agent-config"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
miette = { workspace = true }
serde = { workspace = true }

[dev-dependencies]
serde-saphyr = { workspace = true }
```

In `crates/right-agent-config/src/lib.rs`, use:

```rust
//! Agent configuration DTOs and filesystem-discovered agent definitions.

#![warn(unreachable_pub)]
```

- [ ] **Step 4: Create `right-stt` manifest and minimal lib**

In `crates/right-stt/Cargo.toml`, use:

```toml
[package]
name = "right-stt"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
futures = { workspace = true }
reqwest = { workspace = true, features = ["stream"] }
right-agent-config = { path = "../right-agent-config", version = "*" }
thiserror = { workspace = true }
tokio = { workspace = true }
which = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

In `crates/right-stt/src/lib.rs`, use:

```rust
//! Host-side speech-to-text cache and download helpers.

#![warn(unreachable_pub)]
```

- [ ] **Step 5: Run scaffold tests**

Run:

```bash
devenv shell -- cargo test -p right-agent-config -p right-stt
```

Expected: PASS for both empty crates and `Cargo.lock` updated with the new local packages.

- [ ] **Step 6: Commit scaffold**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock \
  crates/right-agent-config/Cargo.toml crates/right-agent-config/src/lib.rs \
  crates/right-stt/Cargo.toml crates/right-stt/src/lib.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "chore(workspace): scaffold phase two split crates"
```

Expected: only the six files in this task are staged.

## Task 2: Move Agent Config Types Out Of Right-core

**Files:**

- Modify: `crates/right-agent-config/src/lib.rs`
- Modify: `crates/right-agent/Cargo.toml`
- Modify: `crates/right-agent/src/agent/types.rs`
- Modify: `crates/right-codegen/Cargo.toml`
- Modify: `crates/right-codegen/src/agent_def.rs`
- Modify: `crates/right-codegen/src/agent_def_tests.rs`
- Modify: `crates/right-codegen/src/claude_json.rs`
- Modify: `crates/right-codegen/src/contract.rs`
- Modify: `crates/right-codegen/src/pipeline.rs`
- Modify: `crates/right-codegen/src/policy.rs`
- Modify: `crates/right-codegen/src/process_compose.rs`
- Modify: `crates/right-codegen/src/process_compose_tests.rs`
- Modify: `crates/right-codegen/src/skills.rs`
- Modify: `crates/right-core/Cargo.toml`
- Modify: `crates/right-core/src/lib.rs`
- Modify: `crates/right-core/src/stt.rs`
- Delete: `crates/right-core/src/agent_types.rs`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write the failing import change**

In `crates/right-agent/src/agent/types.rs`, change the first line to use the new owner crate:

```rust
pub use right_agent_config::*;
```

In the same file, replace the old core-path test with this config-path test:

```rust
#[test]
fn shared_agent_types_are_available_from_config_and_agent_paths() {
    let config: right_agent_config::AgentConfig = AgentConfig::default();
    let _: AgentConfig = config;
    let _: right_agent_config::AgentDef = AgentDef {
        name: "demo".to_owned(),
        path: std::path::PathBuf::from("/agents/demo"),
        identity_path: std::path::PathBuf::from("/agents/demo/IDENTITY.md"),
        config: None,
        soul_path: None,
        user_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    };
    let _: right_agent_config::WhisperModel = WhisperModel::Small;
}
```

- [ ] **Step 2: Run the targeted test and verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-agent shared_agent_types_are_available_from_config_and_agent_paths
```

Expected: FAIL because `right-agent` does not yet depend on `right-agent-config`, and `right-agent-config` does not yet export the moved types.

- [ ] **Step 3: Move `agent_types` into `right-agent-config`**

Move the body of `crates/right-core/src/agent_types.rs` into `crates/right-agent-config/src/lib.rs`.

At the top of `crates/right-agent-config/src/lib.rs`, remove the old core-local re-export:

```rust
pub use crate::stt::WhisperModel;
```

Replace it with the complete `WhisperModel` enum and impl currently in `crates/right-core/src/stt.rs`. Place it before `SttConfig` in `crates/right-agent-config/src/lib.rs`. The moved impl must keep these public methods unchanged: `filename`, `download_url`, `approx_size_mb`, and `yaml_str`.

In `crates/right-core/src/stt.rs`, remove the `WhisperModel` enum and impl. Add this import near the other imports:

```rust
use right_agent_config::WhisperModel;
```

This keeps `right_core::stt` compiling until Task 3 moves the STT helpers out of `right-core`.

- [ ] **Step 4: Update manifests for config consumers**

In `crates/right-agent/Cargo.toml`, add:

```toml
right-agent-config = { path = "../right-agent-config", version = "*" }
```

In `crates/right-codegen/Cargo.toml`, add:

```toml
right-agent-config = { path = "../right-agent-config", version = "*" }
```

In `crates/right-core/Cargo.toml`, add this temporary dependency because `right-core::stt` uses `WhisperModel` until Task 3:

```toml
right-agent-config = { path = "../right-agent-config", version = "*" }
```

- [ ] **Step 5: Remove `agent_types` from `right-core`**

In `crates/right-core/src/lib.rs`, delete this line:

```rust
pub mod agent_types;
```

Delete `crates/right-core/src/agent_types.rs`.

- [ ] **Step 6: Update `right-codegen` imports**

Replace `right_core::agent_types` paths with `right_agent_config` in these files:

```text
crates/right-codegen/src/agent_def.rs
crates/right-codegen/src/agent_def_tests.rs
crates/right-codegen/src/claude_json.rs
crates/right-codegen/src/contract.rs
crates/right-codegen/src/pipeline.rs
crates/right-codegen/src/policy.rs
crates/right-codegen/src/process_compose.rs
crates/right-codegen/src/process_compose_tests.rs
crates/right-codegen/src/skills.rs
```

For example, in `crates/right-codegen/src/process_compose.rs`, use:

```rust
use right_agent_config::{AgentDef, RestartPolicy, SandboxMode};
```

In `crates/right-codegen/src/agent_def.rs`, make the signature and matches use the owning crate:

```rust
fn sandbox_mode_description(sandbox_mode: &right_agent_config::SandboxMode) -> &'static str {
    match sandbox_mode {
        right_agent_config::SandboxMode::Openshell => {
            "OpenShell sandbox (restricted host access)"
        }
        right_agent_config::SandboxMode::None => "no sandbox (direct host access)",
    }
}
```

In `crates/right-codegen/src/policy.rs`, use:

```rust
use right_agent_config::NetworkPolicy;
```

In `crates/right-codegen/src/skills.rs`, use:

```rust
use right_agent_config::MemoryProvider;
```

- [ ] **Step 7: Verify config move tests**

Run:

```bash
devenv shell -- cargo test -p right-agent shared_agent_types_are_available_from_config_and_agent_paths
devenv shell -- cargo test -p right-agent stt_config
devenv shell -- cargo test -p right-agent agent_config
devenv shell -- cargo test -p right-codegen
```

Expected: all commands PASS.

- [ ] **Step 8: Search for stale config paths**

Run:

```bash
devenv shell -- rg -n "right_core::agent_types|pub mod agent_types|crates/right-core/src/agent_types.rs" crates docs ARCHITECTURE.md
```

Expected: no matches.

- [ ] **Step 9: Commit config move**

Run:

```bash
devenv shell -- git add Cargo.toml Cargo.lock \
  crates/right-agent-config/src/lib.rs \
  crates/right-agent/Cargo.toml crates/right-agent/src/agent/types.rs \
  crates/right-codegen/Cargo.toml \
  crates/right-codegen/src/agent_def.rs \
  crates/right-codegen/src/agent_def_tests.rs \
  crates/right-codegen/src/claude_json.rs \
  crates/right-codegen/src/contract.rs \
  crates/right-codegen/src/pipeline.rs \
  crates/right-codegen/src/policy.rs \
  crates/right-codegen/src/process_compose.rs \
  crates/right-codegen/src/process_compose_tests.rs \
  crates/right-codegen/src/skills.rs \
  crates/right-core/Cargo.toml crates/right-core/src/lib.rs crates/right-core/src/stt.rs \
  crates/right-core/src/agent_types.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(config): move agent config out of right-core"
```

Expected: staged files are only this task's files plus `Cargo.lock`.

## Task 3: Move STT Helpers Out Of Right-core

**Files:**

- Modify: `crates/right-stt/src/lib.rs`
- Modify: `crates/right-core/Cargo.toml`
- Modify: `crates/right-core/src/lib.rs`
- Delete: `crates/right-core/src/stt.rs`
- Modify: `crates/right-agent/Cargo.toml`
- Modify: `crates/right-agent/src/doctor.rs`
- Modify: `crates/right/Cargo.toml`
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/wizard.rs`
- Modify: `crates/bot/Cargo.toml`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/stt/mod.rs`
- Modify: `crates/bot/src/stt/whisper.rs`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write the failing import change**

In `crates/bot/src/stt/whisper.rs`, replace the test import:

```rust
use right_core::stt::{download_model, model_cache_path, WhisperModel};
```

with:

```rust
use right_agent_config::WhisperModel;
use right_stt::{download_model, model_cache_path};
```

- [ ] **Step 2: Run the targeted test and verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-bot whisper::tests::tiny_fixture_downloads_if_missing
```

Expected: FAIL because `right-bot` does not yet depend on `right-stt`, and `right-stt` does not yet export STT helpers.

- [ ] **Step 3: Move STT implementation into `right-stt`**

Move the body of `crates/right-core/src/stt.rs` into `crates/right-stt/src/lib.rs`.

In `crates/right-stt/src/lib.rs`, remove any local `WhisperModel` definition. Add this import near the top:

```rust
use right_agent_config::WhisperModel;
```

Keep these public functions and types in `crates/right-stt/src/lib.rs`:

```rust
pub fn model_cache_path(home: &Path, model: WhisperModel) -> PathBuf
pub fn ffmpeg_available() -> bool
pub fn is_model_cached(dest: &Path) -> bool
pub enum DownloadError
pub async fn download_model(model: WhisperModel, dest: &Path) -> Result<(), DownloadError>
pub async fn ensure_models_cached(
    home: &Path,
    models: &HashSet<WhisperModel>,
) -> Result<usize, DownloadError>
```

Keep `ensure_models_cached_inner`, `partial_path_for`, `download_url_to_path`, and `write_then_rename` private or crate-private exactly as they are today. Do not add a public `WhisperModel` re-export from `right-stt`; the public source of that enum is `right-agent-config`.

- [ ] **Step 4: Update manifests for STT consumers**

In `crates/right-agent/Cargo.toml`, add:

```toml
right-stt = { path = "../right-stt", version = "*" }
```

In `crates/right/Cargo.toml`, add:

```toml
right-agent-config = { path = "../right-agent-config", version = "*" }
right-stt = { path = "../right-stt", version = "*" }
```

In `crates/bot/Cargo.toml`, add:

```toml
right-agent-config = { path = "../right-agent-config", version = "*" }
right-stt = { path = "../right-stt", version = "*" }
```

In `crates/right-core/Cargo.toml`, remove the temporary dependency added in Task 2:

```toml
right-agent-config = { path = "../right-agent-config", version = "*" }
```

Also remove this STT-only dependency from `crates/right-core/Cargo.toml`:

```toml
reqwest = { workspace = true, features = ["stream"] }
```

Keep `futures`, `thiserror`, `tokio`, and `which` in `right-core`; they are still used by OpenShell, platform-store, error handling, and config code.

- [ ] **Step 5: Remove `stt` from `right-core`**

In `crates/right-core/src/lib.rs`, delete this line:

```rust
pub mod stt;
```

Delete `crates/right-core/src/stt.rs`.

- [ ] **Step 6: Update STT call sites**

In `crates/right-agent/src/doctor.rs`, replace:

```rust
right_core::stt::ffmpeg_available()
right_core::stt::model_cache_path(home, stt.model)
```

with:

```rust
right_stt::ffmpeg_available()
right_stt::model_cache_path(home, stt.model)
```

In `crates/bot/src/lib.rs`, replace:

```rust
let model_path = right_core::stt::model_cache_path(&home, config.stt.model);
let ffmpeg_available = right_core::stt::ffmpeg_available();
```

with:

```rust
let model_path = right_stt::model_cache_path(&home, config.stt.model);
let ffmpeg_available = right_stt::ffmpeg_available();
```

In `crates/right/src/main.rs`, replace the cache-warming import block:

```rust
use right_core::stt::WhisperModel;
use std::collections::HashSet;
```

with:

```rust
use right_agent_config::WhisperModel;
use std::collections::HashSet;
```

and replace:

```rust
right_core::stt::ensure_models_cached(home, &models).await
```

with:

```rust
right_stt::ensure_models_cached(home, &models).await
```

In `crates/right/src/main.rs`, replace:

```rust
let ffmpeg_ok = right_core::stt::ffmpeg_available();
model: right_core::stt::WhisperModel::Small,
```

with:

```rust
let ffmpeg_ok = right_stt::ffmpeg_available();
model: right_agent_config::WhisperModel::Small,
```

In `crates/right/src/wizard.rs`, replace:

```rust
right_core::stt::ffmpeg_available()
```

with:

```rust
right_stt::ffmpeg_available()
```

and change the STT setup signature:

```rust
pub fn stt_setup() -> miette::Result<Option<(bool, right_agent_config::WhisperModel)>> {
    use right_agent_config::WhisperModel;
```

In `crates/right/src/wizard.rs` tests, replace:

```rust
use right_core::stt::WhisperModel;
```

with:

```rust
use right_agent_config::WhisperModel;
```

In `crates/bot/src/stt/mod.rs` tests, replace:

```rust
use right_core::stt::{WhisperModel, model_cache_path};
```

with:

```rust
use right_agent_config::WhisperModel;
use right_stt::model_cache_path;
```

and replace:

```rust
right_core::stt::download_model(WhisperModel::Tiny, &p)
```

with:

```rust
right_stt::download_model(WhisperModel::Tiny, &p)
```

In `crates/bot/src/stt/whisper.rs` tests, use:

```rust
use right_agent_config::WhisperModel;
use right_stt::{download_model, model_cache_path};
```

- [ ] **Step 7: Verify STT move tests**

Run:

```bash
devenv shell -- cargo test -p right-stt
devenv shell -- cargo test -p right-agent stt_doctor
devenv shell -- cargo test -p right wizard::stt_yaml_tests
devenv shell -- cargo test -p right-bot stt
```

Expected: all commands PASS. Tests that depend on external network may print a skip message for network unavailability; they must not fail.

- [ ] **Step 8: Search for stale STT paths**

Run:

```bash
devenv shell -- rg -n "right_core::stt|pub mod stt|crates/right-core/src/stt.rs" crates docs ARCHITECTURE.md
```

Expected: no matches.

- [ ] **Step 9: Commit STT move**

Run:

```bash
devenv shell -- git add Cargo.lock \
  crates/right-stt/src/lib.rs \
  crates/right-core/Cargo.toml crates/right-core/src/lib.rs crates/right-core/src/stt.rs \
  crates/right-agent/Cargo.toml crates/right-agent/src/doctor.rs \
  crates/right/Cargo.toml crates/right/src/main.rs crates/right/src/wizard.rs \
  crates/bot/Cargo.toml crates/bot/src/lib.rs crates/bot/src/stt/mod.rs crates/bot/src/stt/whisper.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(stt): move stt helpers out of right-core"
```

Expected: staged files are only this task's files plus `Cargo.lock`.

## Task 4: Update Architecture Docs For Phase 2

**Files:**

- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/modules.md`
- Modify: `docs/architecture/lifecycle.md`

- [ ] **Step 1: Read the relevant docs**

Run:

```bash
devenv shell -- sed -n '1,90p' ARCHITECTURE.md
devenv shell -- sed -n '1,120p' docs/architecture/modules.md
devenv shell -- sed -n '110,145p' docs/architecture/lifecycle.md
```

Expected: the docs still mention `agent_types.rs`, `stt.rs`, or STT under `right-core`.

- [ ] **Step 2: Update the crate table and boundary text**

In `ARCHITECTURE.md`, update the crate table rows so Phase 2 crates appear as independent owners:

```markdown
| **right-core** | `crates/right-core/` | Stable platform foundation: error/ui/config/OpenShell/proto/platform_store/test_support |
| **right-agent-config** | `crates/right-agent-config/` | Agent configuration DTOs, discovery DTOs, sandbox/memory/STT schema types |
| **right-stt** | `crates/right-stt/` | Host-side STT model cache paths, ffmpeg detection, model download, cache warming |
```

In the `right-core` boundary paragraph, remove `agent config DTOs` and `STT model-download helpers with WhisperModel`. Replace them with:

```markdown
Agent configuration DTOs live in `right-agent-config`; host-side STT cache and
download helpers live in `right-stt`. `right-core` must not re-export those
modules because that would preserve the old rebuild edge.
```

- [ ] **Step 3: Update module map docs**

In `docs/architecture/modules.md`, replace the old `right-core` module bullets:

```markdown
- `agent_types.rs` - shared agent configuration and discovery DTOs (`AgentConfig`, `AgentDef`, sandbox/memory/STT config types).
- `stt.rs` - `WhisperModel`, whisper model cache paths, ffmpeg detection, and model download.
```

with:

```markdown
### right-agent-config

- `src/lib.rs` - shared agent configuration and discovery DTOs (`AgentConfig`, `AgentDef`, sandbox/memory/STT config types, `WhisperModel`).

### right-stt

- `src/lib.rs` - host-side whisper model cache paths, ffmpeg detection, model download, and cache warming.
```

- [ ] **Step 4: Update lifecycle docs if STT owner is mentioned**

In `docs/architecture/lifecycle.md`, update STT prose around attachment/transcription and model cache setup so it references `right-stt` for helper ownership and `right-agent-config::SttConfig`/`WhisperModel` for schema ownership.

Use this wording where a concise owner note is needed:

```markdown
The `agent.yaml` STT schema is owned by `right-agent-config`; host-side model
cache and ffmpeg helpers are owned by `right-stt`.
```

- [ ] **Step 5: Search docs for stale paths**

Run:

```bash
devenv shell -- rg -n "right_core::agent_types|right_core::stt|agent_types\\.rs|stt\\.rs|platform foundation: .*stt|WhisperModel.*right-core" ARCHITECTURE.md docs/architecture
```

Expected: no stale Phase 2 ownership references.

- [ ] **Step 6: Commit docs**

Run:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/modules.md docs/architecture/lifecycle.md
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "docs(architecture): document phase two core split"
```

Expected: only the three docs files are staged.

## Task 5: Full Verification And Compile Fan-out Probes

**Files:**

- No committed source edits.
- Temporary probe edits to `crates/right-agent-config/src/lib.rs` and `crates/right-stt/src/lib.rs`; revert them before committing or finishing.

- [ ] **Step 1: Run full build**

Run:

```bash
devenv shell -- cargo build --workspace
```

- [ ] **Step 2: Run full test suite**

Run:

```bash
devenv shell -- cargo test --workspace
```

- [ ] **Step 3: Verify stale code paths are gone**

Run:

```bash
devenv shell -- rg -n "right_core::agent_types|right_core::stt|pub mod agent_types|pub mod stt|crates/right-core/src/agent_types.rs|crates/right-core/src/stt.rs" crates ARCHITECTURE.md docs/architecture
```

Expected: no matches.

- [ ] **Step 4: Probe config compile fan-out**

Temporarily change `WhisperModel::Tiny` approximate size in `crates/right-agent-config/src/lib.rs`:

```rust
Self::Tiny => 76,
```

Run:

```bash
devenv shell -- cargo build --workspace -vv
```

Expected rebuilds include crates that use config types, such as `right-agent-config`, `right-codegen`, `right-agent`, `right`, and `right-bot`. Expected rebuilds must not include recompiling `right-core` because `right-core` no longer owns or imports config DTOs.

Revert only the temporary probe edit in `crates/right-agent-config/src/lib.rs` so the match arm is restored:

```rust
Self::Tiny => 75,
```

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS after revert.

- [ ] **Step 5: Probe STT compile fan-out**

Temporarily edit the doc comment above `model_cache_path` in `crates/right-stt/src/lib.rs`:

```rust
/// Returns the cache path for a whisper model under the given RIGHT_HOME directory.
```

Run:

```bash
devenv shell -- cargo build --workspace -vv
```

Expected rebuilds include `right-stt`, `right-agent`, `right`, and `right-bot`. Expected rebuilds must not include recompiling `right-core`, `right-codegen`, `right-db`, `right-mcp`, or `right-memory`.

Revert only the temporary probe edit in `crates/right-stt/src/lib.rs` so the doc comment is restored:

```rust
/// Returns the cache path for a whisper model under the given RIGHT_HOME.
```

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS after revert.

- [ ] **Step 6: Confirm clean worktree**

Run:

```bash
devenv shell -- git status --short
```

Expected: no uncommitted changes from Phase 2. If unrelated pre-existing files are shown, confirm they were not touched by this plan.

- [ ] **Step 7: Record verification in handoff**

Include these exact results in the implementation handoff:

```text
cargo build --workspace: PASS
cargo test --workspace: PASS
stale right_core::agent_types/right_core::stt search: no matches
config fan-out probe: right-core did not rebuild
stt fan-out probe: right-core/right-codegen/right-db/right-mcp/right-memory did not rebuild
```
