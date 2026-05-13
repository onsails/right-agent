# Right-core Decomposition Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract Phase 1 compile-isolation units from `right-core`: platform knobs, prompt safety, and runtime state.

**Architecture:** Add three small crates and move each ownership unit out of `right-core` with direct imports from consumers. Do not re-export moved modules from `right-core`; breaking old internal `right_core::...` paths is intentional because the compile win depends on removing those dependency edges.

**Tech Stack:** Rust 2024 Cargo workspace, `miette`, `serde`, `serde_json`, `base64`, `rand`, `ironclaw_safety`, `devenv shell -- cargo`.

---

## Scope

This plan implements only Phase 1 from [2026-05-13-right-core-decomposition-design.md](/Users/molt/dev/rightclaw/docs/superpowers/specs/2026-05-13-right-core-decomposition-design.md).

Out of scope for this plan:

- `right-agent-config`
- `right-stt`
- `right-ui`
- `right-process`
- `right-openshell`
- `right-platform-store`

Those need separate plans after Phase 1 is verified.

## Execution Notes

- Run every command from `/Users/molt/dev/rightclaw`.
- Prefix commands with `devenv shell --`.
- Before writing Rust implementation code, use `rust-dev:rust-dev` if that skill is available in the execution session. If unavailable, state that explicitly and follow `AGENTS.rust.md`.
- Existing unrelated modified files may be present. Do not stage or revert them.
- Before every commit, run `devenv shell -- git diff --cached --name-only` and confirm only the files listed in that task are staged.

## File Structure

Create:

- `crates/right-platform-knobs/Cargo.toml` — manifest for UX/prose constants crate.
- `crates/right-platform-knobs/src/lib.rs` — owns `IDLE_THRESHOLD_SECS` and `IDLE_THRESHOLD_MIN`.
- `crates/right-prompt-safety/Cargo.toml` — manifest for prompt-safety crate.
- `crates/right-prompt-safety/src/lib.rs` — owns memory prompt-injection sanitizing/wrapping facade.
- `crates/right-runtime-state/Cargo.toml` — manifest for runtime-state crate.
- `crates/right-runtime-state/src/lib.rs` — owns process-compose ports, `RuntimeState`, and PC API token generation.

Modify:

- `Cargo.toml` — add three workspace members.
- `crates/right-core/Cargo.toml` — remove dependencies no longer used by `right-core` after moves.
- `crates/right-core/src/lib.rs` — remove `injection_guard`, `runtime_state`, and `time_constants` modules.
- `crates/right-core/src/time_constants.rs` — delete after moving to `right-platform-knobs`.
- `crates/right-core/src/injection_guard.rs` — delete after moving to `right-prompt-safety`.
- `crates/right-core/src/runtime_state.rs` — delete after moving to `right-runtime-state`.
- `crates/right-codegen/Cargo.toml` — add `right-platform-knobs`, `right-runtime-state`.
- `crates/right-codegen/src/skills.rs` — import idle constants from `right-platform-knobs`.
- `crates/right-codegen/src/agent_def_tests.rs` — import idle constant from `right-platform-knobs`.
- `crates/right-codegen/src/process_compose.rs` — import ports from `right-runtime-state`.
- `crates/right-codegen/src/pipeline.rs` — import runtime state from `right-runtime-state`.
- `crates/right-agent/Cargo.toml` — add `right-platform-knobs`, `right-runtime-state`.
- `crates/right-agent/src/cron_spec.rs` — import idle constants from `right-platform-knobs`.
- `crates/right-agent/src/runtime/mod.rs` — stop re-exporting runtime state.
- `crates/right-agent/src/runtime/state.rs` — delete obsolete re-export module.
- `crates/right-agent/src/runtime/pc_client.rs` — import `read_state` from `right-runtime-state`.
- `crates/right-agent/src/init.rs` — import `MCP_HTTP_PORT` from `right-runtime-state`.
- `crates/right-agent/src/runtime/pc_client_tests.rs` — import runtime-state test types from `right-runtime-state`.
- `crates/right-memory/Cargo.toml` — replace normal `right-core` dependency with `right-prompt-safety`.
- `crates/right-memory/src/resilient.rs` — call `right_prompt_safety`.
- `crates/bot/Cargo.toml` — add `right-platform-knobs`, `right-prompt-safety`.
- `crates/bot/src/cc/prompt.rs` — call `right_prompt_safety`.
- `crates/bot/src/cron_delivery.rs` — import idle threshold from `right-platform-knobs`.
- `crates/right/Cargo.toml` — add `right-runtime-state`.
- `crates/right/src/main.rs` — import runtime state from `right-runtime-state` instead of `right_agent::runtime`.
- `crates/right/tests/cli_integration.rs` — import `MCP_HTTP_PORT` from `right-runtime-state`.
- `ARCHITECTURE.md` — update workspace table and `right-core` boundary text.
- `docs/architecture/modules.md` — update module map.
- `docs/architecture/memory.md` — update prompt-safety crate path.
- `docs/architecture/sessions.md` — document `IDLE_THRESHOLD_SECS` ownership move.

## Task 1: Scaffold Phase 1 crates

**Files:**

- Modify: `Cargo.toml`
- Create: `crates/right-platform-knobs/Cargo.toml`
- Create: `crates/right-platform-knobs/src/lib.rs`
- Create: `crates/right-prompt-safety/Cargo.toml`
- Create: `crates/right-prompt-safety/src/lib.rs`
- Create: `crates/right-runtime-state/Cargo.toml`
- Create: `crates/right-runtime-state/src/lib.rs`

- [ ] **Step 1: Run the failing package check**

Run:

```bash
devenv shell -- cargo test -p right-platform-knobs -p right-prompt-safety -p right-runtime-state
```

Expected: FAIL because the three packages do not exist yet. The failure should include `package ID specification`.

- [ ] **Step 2: Add workspace members**

Edit the root `Cargo.toml` `[workspace]` block so `members` is:

```toml
members = [
    "crates/right-agent",
    "crates/right-core",
    "crates/right-db",
    "crates/right-memory",
    "crates/right-mcp",
    "crates/right-codegen",
    "crates/right",
    "crates/bot",
    "crates/right-platform-knobs",
    "crates/right-prompt-safety",
    "crates/right-runtime-state",
]
resolver = "3"
```

- [ ] **Step 3: Create `right-platform-knobs` manifest and minimal lib**

Create `crates/right-platform-knobs/Cargo.toml`:

```toml
[package]
name = "right-platform-knobs"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
```

Create `crates/right-platform-knobs/src/lib.rs`:

```rust
//! Volatile platform knobs with agent-facing or UX-facing effects.

#![warn(unreachable_pub)]
```

- [ ] **Step 4: Create `right-prompt-safety` manifest and minimal lib**

Create `crates/right-prompt-safety/Cargo.toml`:

```toml
[package]
name = "right-prompt-safety"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
ironclaw_safety = "0.2"
```

Create `crates/right-prompt-safety/src/lib.rs`:

```rust
//! Prompt-injection safety wrappers for untrusted external content.

#![warn(unreachable_pub)]
```

- [ ] **Step 5: Create `right-runtime-state` manifest and minimal lib**

Create `crates/right-runtime-state/Cargo.toml`:

```toml
[package]
name = "right-runtime-state"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
base64 = { workspace = true }
miette = { workspace = true }
rand = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

Create `crates/right-runtime-state/src/lib.rs`:

```rust
//! Runtime state shared by process-compose producers and consumers.

#![warn(unreachable_pub)]
```

- [ ] **Step 6: Run scaffold tests**

Run:

```bash
devenv shell -- cargo test -p right-platform-knobs -p right-prompt-safety -p right-runtime-state
```

Expected: PASS for all three empty crates.

- [ ] **Step 7: Commit scaffold**

Stage only:

```bash
devenv shell -- git add Cargo.toml \
  crates/right-platform-knobs/Cargo.toml crates/right-platform-knobs/src/lib.rs \
  crates/right-prompt-safety/Cargo.toml crates/right-prompt-safety/src/lib.rs \
  crates/right-runtime-state/Cargo.toml crates/right-runtime-state/src/lib.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "chore(workspace): scaffold phase one core split crates"
```

Expected staged files: only the seven files listed above.

## Task 2: Move platform knobs out of right-core

**Files:**

- Modify: `crates/right-platform-knobs/src/lib.rs`
- Modify: `crates/right-core/src/lib.rs`
- Delete: `crates/right-core/src/time_constants.rs`
- Modify: `crates/right-codegen/Cargo.toml`
- Modify: `crates/right-codegen/src/skills.rs`
- Modify: `crates/right-codegen/src/agent_def_tests.rs`
- Modify: `crates/right-agent/Cargo.toml`
- Modify: `crates/right-agent/src/cron_spec.rs`
- Modify: `crates/bot/Cargo.toml`
- Modify: `crates/bot/src/cron_delivery.rs`

- [ ] **Step 1: Write the failing import change**

In `crates/right-codegen/src/agent_def_tests.rs`, change the idle-threshold test to use `right_platform_knobs::IDLE_THRESHOLD_MIN` before the crate exports it:

```rust
#[test]
fn operating_instructions_cron_idle_threshold_matches_const() {
    let needle = format!(
        "idle for **{} minutes**",
        right_platform_knobs::IDLE_THRESHOLD_MIN
    );
    assert!(
        crate::OPERATING_INSTRUCTIONS.contains(&needle),
        "OPERATING_INSTRUCTIONS must mention `idle for **{} minutes**` to match \
         right_platform_knobs::IDLE_THRESHOLD_MIN",
        right_platform_knobs::IDLE_THRESHOLD_MIN
    );
    let promise_needle = format!(
        "sooner than {} minutes",
        right_platform_knobs::IDLE_THRESHOLD_MIN
    );
    assert!(
        crate::OPERATING_INSTRUCTIONS.contains(&promise_needle),
        "OPERATING_INSTRUCTIONS must spell out the \"never promise sooner than {} minutes\" rule",
        right_platform_knobs::IDLE_THRESHOLD_MIN
    );
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
devenv shell -- cargo test -p right-codegen operating_instructions_cron_idle_threshold_matches_const
```

Expected: FAIL with unresolved crate or missing item for `right_platform_knobs`.

- [ ] **Step 3: Move the constants into `right-platform-knobs`**

Replace `crates/right-platform-knobs/src/lib.rs` with:

```rust
//! Volatile platform knobs with agent-facing or UX-facing effects.
//!
//! `IDLE_THRESHOLD_SECS` is the UX-politeness gate on cron notification
//! delivery: pending notifications are held until the chat has been idle
//! for this long, so a cron result never interrupts an active conversation.
//! Correctness against `--resume` races is handled separately by the
//! per-session mutex (see `docs/architecture/sessions.md`); this constant
//! is purely about UX.
//!
//! Implication for the agent: any delivery the user is expecting (e.g. a
//! "remind me in N minutes" reminder) cannot arrive sooner than
//! `IDLE_THRESHOLD_SECS` of chat idle, regardless of `run_at`. The agent
//! must not promise faster delivery — see `OPERATING_INSTRUCTIONS.md` and
//! the `/rightcron` skill.

#![warn(unreachable_pub)]

/// Idle threshold in seconds before pending cron notifications are delivered.
pub const IDLE_THRESHOLD_SECS: i64 = 120;

/// Human-readable form for prose ("2 min" reads better than "120 s").
pub const IDLE_THRESHOLD_MIN: i64 = IDLE_THRESHOLD_SECS / 60;
```

Then delete `crates/right-core/src/time_constants.rs`.

In `crates/right-core/src/lib.rs`, remove:

```rust
pub mod time_constants;
```

- [ ] **Step 4: Add crate dependencies for direct consumers**

In `crates/right-codegen/Cargo.toml`, add to `[dependencies]`:

```toml
right-platform-knobs = { path = "../right-platform-knobs", version = "*" }
```

In `crates/right-agent/Cargo.toml`, add to `[dependencies]`:

```toml
right-platform-knobs = { path = "../right-platform-knobs", version = "*" }
```

In `crates/bot/Cargo.toml`, add to `[dependencies]`:

```toml
right-platform-knobs = { path = "../right-platform-knobs", version = "*" }
```

- [ ] **Step 5: Update idle-threshold imports**

In `crates/right-codegen/src/skills.rs`, replace:

```rust
use right_core::time_constants::{IDLE_THRESHOLD_MIN, IDLE_THRESHOLD_SECS};
```

with:

```rust
use right_platform_knobs::{IDLE_THRESHOLD_MIN, IDLE_THRESHOLD_SECS};
```

In the same file, replace this doc comment sentence:

```rust
/// `cron_spec`. Files without `{{ }}` syntax pass through unchanged.
```

with:

```rust
/// `right-platform-knobs`. Files without `{{ }}` syntax pass through unchanged.
```

In `crates/right-agent/src/cron_spec.rs`, replace:

```rust
pub use right_core::time_constants::{IDLE_THRESHOLD_MIN, IDLE_THRESHOLD_SECS};
```

with:

```rust
use right_platform_knobs::IDLE_THRESHOLD_MIN;
```

In `crates/bot/src/cron_delivery.rs`, replace:

```rust
use right_agent::cron_spec::IDLE_THRESHOLD_SECS;
```

with:

```rust
use right_platform_knobs::IDLE_THRESHOLD_SECS;
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
devenv shell -- cargo test -p right-platform-knobs
devenv shell -- cargo test -p right-codegen operating_instructions_cron_idle_threshold_matches_const
devenv shell -- cargo test -p right-codegen rightcron_skill_interpolates_idle_threshold
devenv shell -- cargo test -p right cron_trigger_description_matches_const
```

Expected: all PASS.

- [ ] **Step 7: Prove old `right_core::time_constants` path is gone**

Run:

```bash
devenv shell -- rg -n "right_core::time_constants|pub mod time_constants|time_constants.rs" crates
```

Expected: no matches.

- [ ] **Step 8: Commit platform knobs move**

Stage only:

```bash
devenv shell -- git add crates/right-platform-knobs/src/lib.rs \
  crates/right-core/src/lib.rs crates/right-core/src/time_constants.rs \
  crates/right-codegen/Cargo.toml crates/right-codegen/src/skills.rs crates/right-codegen/src/agent_def_tests.rs \
  crates/right-agent/Cargo.toml crates/right-agent/src/cron_spec.rs \
  crates/bot/Cargo.toml crates/bot/src/cron_delivery.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(workspace): move platform knobs out of right-core"
```

Expected staged files: only the files listed in this task.

## Task 3: Move prompt safety out of right-core

**Files:**

- Modify: `crates/right-prompt-safety/src/lib.rs`
- Modify: `crates/right-core/Cargo.toml`
- Modify: `crates/right-core/src/lib.rs`
- Delete: `crates/right-core/src/injection_guard.rs`
- Modify: `crates/right-memory/Cargo.toml`
- Modify: `crates/right-memory/src/resilient.rs`
- Modify: `crates/bot/Cargo.toml`
- Modify: `crates/bot/src/cc/prompt.rs`

- [ ] **Step 1: Write the failing import change**

In `crates/right-memory/src/resilient.rs`, replace:

```rust
let sanitized = right_core::injection_guard::sanitize_memory_content(content);
```

with:

```rust
let sanitized = right_prompt_safety::sanitize_memory_content(content);
```

- [ ] **Step 2: Run the failing memory test build**

Run:

```bash
devenv shell -- cargo test -p right-memory
```

Expected: FAIL because `right_prompt_safety` is not yet a dependency/exporting the function.

- [ ] **Step 3: Move prompt-safety implementation**

Move the entire current content of `crates/right-core/src/injection_guard.rs` into `crates/right-prompt-safety/src/lib.rs`.

Keep the existing module documentation and tests. Add this crate attribute immediately after the top module docs:

```rust
#![warn(unreachable_pub)]
```

Then delete `crates/right-core/src/injection_guard.rs`.

In `crates/right-core/src/lib.rs`, remove:

```rust
pub mod injection_guard;
```

In `crates/right-core/Cargo.toml`, remove:

```toml
ironclaw_safety = "0.2"
```

- [ ] **Step 4: Add direct prompt-safety dependencies**

In `crates/right-memory/Cargo.toml`, replace:

```toml
right-core = { path = "../right-core", version = "*" }
```

with:

```toml
right-prompt-safety = { path = "../right-prompt-safety", version = "*" }
```

In `crates/bot/Cargo.toml`, add to `[dependencies]`:

```toml
right-prompt-safety = { path = "../right-prompt-safety", version = "*" }
```

- [ ] **Step 5: Update bot prompt-safety call sites**

In `crates/bot/src/cc/prompt.rs`, replace:

```rust
let prefix = right_core::injection_guard::memory_wrap_prefix()
    .replace('\'', "'\\''");
let suffix = right_core::injection_guard::memory_wrap_suffix()
    .replace('\'', "'\\''");
```

with:

```rust
let prefix = right_prompt_safety::memory_wrap_prefix().replace('\'', "'\\''");
let suffix = right_prompt_safety::memory_wrap_suffix().replace('\'', "'\\''");
```

In the same file, replace:

```rust
let wrapped = right_core::injection_guard::wrap_memory_for_prompt(content);
```

with:

```rust
let wrapped = right_prompt_safety::wrap_memory_for_prompt(content);
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
devenv shell -- cargo test -p right-prompt-safety
devenv shell -- cargo test -p right-memory
devenv shell -- cargo test -p right-bot format_composite_memory
devenv shell -- cargo test -p right-bot prompt
```

Expected: all PASS. If `cargo test -p right-bot prompt` matches no tests, run `devenv shell -- cargo test -p right-bot cc::prompt` instead and record the exact passing command in the task checklist.

- [ ] **Step 7: Prove old `right_core::injection_guard` path is gone**

Run:

```bash
devenv shell -- rg -n "right_core::injection_guard|pub mod injection_guard|injection_guard.rs|ironclaw_safety" crates/right-core crates/right-memory/src crates/bot/src
```

Expected: no `right_core::injection_guard`, no `pub mod injection_guard`, no `crates/right-core/src/injection_guard.rs`, and no `ironclaw_safety` in `crates/right-core/Cargo.toml`. Matches in `crates/right-prompt-safety` are expected if the command scope is widened.

- [ ] **Step 8: Commit prompt-safety move**

Stage only:

```bash
devenv shell -- git add crates/right-prompt-safety/src/lib.rs \
  crates/right-core/Cargo.toml crates/right-core/src/lib.rs crates/right-core/src/injection_guard.rs \
  crates/right-memory/Cargo.toml crates/right-memory/src/resilient.rs \
  crates/bot/Cargo.toml crates/bot/src/cc/prompt.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(memory): move prompt safety out of right-core"
```

Expected staged files: only the files listed in this task.

## Task 4: Move runtime state out of right-core

**Files:**

- Modify: `crates/right-runtime-state/src/lib.rs`
- Modify: `crates/right-core/Cargo.toml`
- Modify: `crates/right-core/src/lib.rs`
- Delete: `crates/right-core/src/runtime_state.rs`
- Modify: `crates/right-codegen/Cargo.toml`
- Modify: `crates/right-codegen/src/process_compose.rs`
- Modify: `crates/right-codegen/src/pipeline.rs`
- Modify: `crates/right-agent/Cargo.toml`
- Modify: `crates/right-agent/src/init.rs`
- Modify: `crates/right-agent/src/runtime/mod.rs`
- Delete: `crates/right-agent/src/runtime/state.rs`
- Modify: `crates/right-agent/src/runtime/pc_client.rs`
- Modify: `crates/right-agent/src/runtime/pc_client_tests.rs`
- Modify: `crates/right/Cargo.toml`
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/tests/cli_integration.rs`

- [ ] **Step 1: Write the failing import change**

In `crates/right-codegen/src/process_compose.rs`, replace:

```rust
use right_core::runtime_state::{MCP_HTTP_PORT, PC_PORT};
```

with:

```rust
use right_runtime_state::{MCP_HTTP_PORT, PC_PORT};
```

- [ ] **Step 2: Run the failing process-compose test build**

Run:

```bash
devenv shell -- cargo test -p right-codegen process_compose
```

Expected: FAIL because `right_runtime_state` is not yet a dependency/exporting the constants.

- [ ] **Step 3: Move runtime-state implementation**

Move the entire current content of `crates/right-core/src/runtime_state.rs` into `crates/right-runtime-state/src/lib.rs`.

Add this crate attribute after any module-level docs or at the top if there are no module docs:

```rust
#![warn(unreachable_pub)]
```

Then delete `crates/right-core/src/runtime_state.rs`.

In `crates/right-core/src/lib.rs`, remove:

```rust
pub mod runtime_state;
```

In `crates/right-core/Cargo.toml`, remove these dependencies if `rg -n "base64|rand" crates/right-core/src` returns no matches after the move:

```toml
base64 = { workspace = true }
rand = { workspace = true }
```

Do not remove `serde`, `serde_json`, or `miette`; other `right-core` modules still use them.

- [ ] **Step 4: Add direct runtime-state dependencies**

In `crates/right-codegen/Cargo.toml`, add to `[dependencies]`:

```toml
right-runtime-state = { path = "../right-runtime-state", version = "*" }
```

In `crates/right-agent/Cargo.toml`, add to `[dependencies]`:

```toml
right-runtime-state = { path = "../right-runtime-state", version = "*" }
```

In `crates/right/Cargo.toml`, add to `[dependencies]`:

```toml
right-runtime-state = { path = "../right-runtime-state", version = "*" }
```

- [ ] **Step 5: Update right-codegen runtime-state imports**

In `crates/right-codegen/src/pipeline.rs`, replace:

```rust
use right_core::runtime_state::{
    AgentState, MCP_HTTP_PORT, PC_PORT, RuntimeState, generate_pc_api_token, read_state,
    write_state,
};
```

with:

```rust
use right_runtime_state::{
    AgentState, MCP_HTTP_PORT, PC_PORT, RuntimeState, generate_pc_api_token, read_state,
    write_state,
};
```

- [ ] **Step 6: Update right-agent runtime-state imports and remove re-export module**

In `crates/right-agent/src/runtime/mod.rs`, replace:

```rust
pub mod state;

pub use deps::verify_dependencies;
pub use pc_client::{PcClient, ProcessInfo};
pub use right_core::runtime_state::{
    AgentState, MCP_HTTP_PORT, PC_PORT, RuntimeState, generate_pc_api_token, read_state,
    write_state,
};
```

with:

```rust
pub use deps::verify_dependencies;
pub use pc_client::{PcClient, ProcessInfo};
```

Delete `crates/right-agent/src/runtime/state.rs`.

In `crates/right-agent/src/runtime/pc_client.rs`, replace:

```rust
use right_core::runtime_state::read_state;
```

with:

```rust
use right_runtime_state::read_state;
```

In `crates/right-agent/src/init.rs`, replace `crate::runtime::MCP_HTTP_PORT` with `right_runtime_state::MCP_HTTP_PORT`.

In `crates/right-agent/src/runtime/pc_client_tests.rs`, replace:

```rust
use crate::runtime::PC_PORT;
```

with:

```rust
use right_runtime_state::PC_PORT;
```

Replace:

```rust
use crate::runtime::state::{AgentState, RuntimeState, write_state};
```

with:

```rust
use right_runtime_state::{AgentState, RuntimeState, write_state};
```

- [ ] **Step 7: Update right CLI runtime-state imports**

In `crates/right/src/main.rs`, replace these call paths:

```rust
right_agent::runtime::read_state
right_agent::runtime::MCP_HTTP_PORT
right_agent::runtime::PC_PORT
```

with:

```rust
right_runtime_state::read_state
right_runtime_state::MCP_HTTP_PORT
right_runtime_state::PC_PORT
```

In `crates/right/tests/cli_integration.rs`, replace:

```rust
right_agent::runtime::MCP_HTTP_PORT
```

with:

```rust
right_runtime_state::MCP_HTTP_PORT
```

- [ ] **Step 8: Run focused tests**

Run:

```bash
devenv shell -- cargo test -p right-runtime-state
devenv shell -- cargo test -p right-codegen process_compose
devenv shell -- cargo test -p right-codegen pipeline
devenv shell -- cargo test -p right-agent pc_client
devenv shell -- cargo test -p right cli_integration
```

Expected: all PASS. If `cargo test -p right-codegen pipeline` matches no tests, run `devenv shell -- cargo test -p right-codegen` and record the exact passing command.

- [ ] **Step 9: Prove old runtime-state paths are gone**

Run:

```bash
devenv shell -- rg -n "right_core::runtime_state|pub mod runtime_state|runtime_state.rs|right_agent::runtime::(read_state|MCP_HTTP_PORT|PC_PORT)|runtime::state" crates
```

Expected: no matches. If a match remains in historical docs under `docs/superpowers/`, ignore it; this command scopes to `crates`.

- [ ] **Step 10: Commit runtime-state move**

Stage only:

```bash
devenv shell -- git add crates/right-runtime-state/src/lib.rs \
  crates/right-core/Cargo.toml crates/right-core/src/lib.rs crates/right-core/src/runtime_state.rs \
  crates/right-codegen/Cargo.toml crates/right-codegen/src/process_compose.rs crates/right-codegen/src/pipeline.rs \
  crates/right-agent/Cargo.toml crates/right-agent/src/init.rs crates/right-agent/src/runtime/mod.rs \
  crates/right-agent/src/runtime/state.rs crates/right-agent/src/runtime/pc_client.rs crates/right-agent/src/runtime/pc_client_tests.rs \
  crates/right/Cargo.toml crates/right/src/main.rs crates/right/tests/cli_integration.rs
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "refactor(runtime): move runtime state out of right-core"
```

Expected staged files: only the files listed in this task.

## Task 5: Update architecture docs for Phase 1

**Files:**

- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/modules.md`
- Modify: `docs/architecture/memory.md`
- Modify: `docs/architecture/sessions.md`

- [ ] **Step 1: Write the failing doc drift search**

Run:

```bash
devenv shell -- rg -n "time constants|time_constants|runtime_state.rs|right_core::injection_guard|right-core.*runtime-state|right-core.*time" ARCHITECTURE.md docs/architecture
```

Expected: FAIL for this task because matches still describe Phase 0 ownership.

- [ ] **Step 2: Update `ARCHITECTURE.md` workspace table**

In `ARCHITECTURE.md`, change:

```markdown
Eight crates in a Cargo workspace:
```

to:

```markdown
Eleven crates in a Cargo workspace:
```

Replace the existing table rows with:

```markdown
| Crate | Path | Role |
|-------|------|------|
| **right-platform-knobs** | `crates/right-platform-knobs/` | UX/prose tunables that should not invalidate platform foundations |
| **right-prompt-safety** | `crates/right-prompt-safety/` | Prompt-injection safety wrappers over `ironclaw_safety` |
| **right-runtime-state** | `crates/right-runtime-state/` | process-compose ports, runtime state JSON, and API-token generation |
| **right-core** | `crates/right-core/` | Stable platform foundation: error/ui/config/OpenShell/proto/platform_store/stt/test_support |
| **right-db** | `crates/right-db/` | Per-agent SQLite plumbing: `open_connection`, central migration registry, `sql/v*.sql` |
| **right-mcp** | `crates/right-mcp/` | MCP aggregator backend, proxy, reconnect, credentials, token derivation, auth tokens |
| **right-codegen** | `crates/right-codegen/` | Per-agent codegen: settings.json, .mcp.json, prompts, process-compose, cloudflared, sandbox policy, bundled skills |
| **right-memory** | `crates/right-memory/` | Hindsight-resilience layer and retain queue |
| **right-agent** | `crates/right-agent/` | Slim orchestrator: agent discovery, runtime, init, doctor, rebootstrap, cron_spec, tunnel, usage |
| **right** | `crates/right/` | CLI binary (`right`) + MCP Aggregator (HTTP) |
| **right-bot** | `crates/bot/` | Telegram bot runtime (teloxide) + cron engine + login flow |
```

In the `right-core hosts stable platform primitives` paragraph, remove `runtime-state primitives` and `time constants` references. Add this paragraph immediately after it:

```markdown
`right-platform-knobs`, `right-prompt-safety`, and `right-runtime-state`
are deliberately outside `right-core`: edits to UX/prose constants, memory
prompt-safety wrappers, or process-compose runtime-state JSON must not
invalidate OpenShell/proto/UI/STT foundation code.
```

- [ ] **Step 3: Update `docs/architecture/modules.md`**

Insert this section before `### right-core (stable platform foundation)`:

```markdown
### right-platform-knobs

- `IDLE_THRESHOLD_SECS` / `IDLE_THRESHOLD_MIN` - UX-politeness gate for cron delivery and matching agent-facing prose.

### right-prompt-safety

- `sanitize_memory_content` - write-side Hindsight memory sanitization.
- `wrap_memory_for_prompt`, `memory_wrap_prefix`, `memory_wrap_suffix`, `escape_memory_close_delimiter` - read-side untrusted-content wrapping for memory prompt assembly.

### right-runtime-state

- `PC_PORT` and `MCP_HTTP_PORT` - process-compose and MCP HTTP default ports.
- `RuntimeState` / `AgentState` - persisted `<home>/run/state.json` schema.
- `read_state`, `write_state`, `generate_pc_api_token` - runtime-state IO and process-compose API token generation.
```

In the `right-core` section, remove these bullets:

```markdown
- `runtime_state.rs` - process-compose ports, runtime state JSON, and API-token generation.
- Single-file modules: `error.rs`, `process_group.rs`, `time_constants.rs`.
```

Replace the single-file modules bullet with:

```markdown
- Single-file modules: `error.rs`, `process_group.rs`.
```

In the `right-agent` section, replace:

```markdown
- `runtime/` — process-compose REST client, dependency checks, and compatibility re-exports for runtime state primitives.
```

with:

```markdown
- `runtime/` — process-compose REST client and dependency checks. Runtime-state primitives live in `right-runtime-state`.
```

- [ ] **Step 4: Update memory architecture doc**

In `docs/architecture/memory.md`, replace:

```markdown
Two layers, both routing through `right_core::injection_guard` (a
thin facade over the `ironclaw_safety` crate):
```

with:

```markdown
Two layers, both routing through `right_prompt_safety` (a thin facade
over the `ironclaw_safety` crate):
```

Replace:

```markdown
through that crate's releases. The `right_core::injection_guard`
facade exists to centralize the source label (`"memory"`), expose
```

with:

```markdown
through that crate's releases. The `right-prompt-safety` crate exists
to centralize the source label (`"memory"`), expose
```

- [ ] **Step 5: Update sessions architecture doc**

In `docs/architecture/sessions.md`, replace:

```markdown
`IDLE_THRESHOLD_SECS = 120` remains as UX politeness ("don't interrupt the
user mid-conversation"), but correctness now lives in the mutex.
```

with:

```markdown
`right-platform-knobs::IDLE_THRESHOLD_SECS = 120` remains as UX politeness
("don't interrupt the user mid-conversation"), but correctness now lives in
the mutex.
```

- [ ] **Step 6: Run doc drift searches**

Run:

```bash
devenv shell -- rg -n "right_core::injection_guard|time_constants|runtime_state.rs|right-core.*time constants|right-core.*runtime-state" ARCHITECTURE.md docs/architecture
devenv shell -- rg -n "right-platform-knobs|right-prompt-safety|right-runtime-state" ARCHITECTURE.md docs/architecture/modules.md docs/architecture/memory.md docs/architecture/sessions.md
```

Expected: first command has no matches. Second command shows the new ownership docs.

- [ ] **Step 7: Commit docs**

Stage only:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/modules.md docs/architecture/memory.md docs/architecture/sessions.md
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "docs(architecture): document phase one core split"
```

Expected staged files: only the four docs listed in this task.

## Task 6: Final verification and rebuild fan-out proof

**Files:**

- Temporarily modify and revert: `crates/right-platform-knobs/src/lib.rs`
- Temporarily modify and revert: `crates/right-prompt-safety/src/lib.rs`
- Temporarily modify and revert: `crates/right-runtime-state/src/lib.rs`

- [ ] **Step 1: Run full build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 2: Run full test suite**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 3: Probe platform-knobs rebuild fan-out**

Temporarily change this line in `crates/right-platform-knobs/src/lib.rs`:

```rust
pub const IDLE_THRESHOLD_SECS: i64 = 120;
```

to:

```rust
pub const IDLE_THRESHOLD_SECS: i64 = 121;
```

Run:

```bash
devenv shell -- cargo build --workspace -v 2>&1 | rg -i "Compiling (right-|right_)"
```

Expected compiled crates include `right-platform-knobs` and direct consumers such as `right-codegen`, `right-agent`, and `right-bot`. Expected compiled crates do not include `right-core`, `right-db`, `right-mcp`, or `right-memory`.

Revert the constant to:

```rust
pub const IDLE_THRESHOLD_SECS: i64 = 120;
```

- [ ] **Step 4: Probe prompt-safety rebuild fan-out**

Temporarily change this line in a doc comment in `crates/right-prompt-safety/src/lib.rs`:

```rust
//! Memory-content safety facade over `ironclaw_safety`.
```

to:

```rust
//! Memory-content safety facade around `ironclaw_safety`.
```

Run:

```bash
devenv shell -- cargo build --workspace -v 2>&1 | rg -i "Compiling (right-|right_)"
```

Expected compiled crates include `right-prompt-safety`, `right-memory`, and `right-bot`. Expected compiled crates do not include `right-core`, `right-db`, `right-mcp`, or `right-codegen`.

Revert the doc comment to:

```rust
//! Memory-content safety facade over `ironclaw_safety`.
```

- [ ] **Step 5: Probe runtime-state rebuild fan-out**

Temporarily change this line in `crates/right-runtime-state/src/lib.rs`:

```rust
pub const PC_PORT: u16 = 18927;
```

to:

```rust
pub const PC_PORT: u16 = 18928;
```

Run:

```bash
devenv shell -- cargo build --workspace -v 2>&1 | rg -i "Compiling (right-|right_)"
```

Expected compiled crates include `right-runtime-state`, `right-codegen`, `right-agent`, and `right`. Expected compiled crates do not include `right-core`, `right-db`, `right-mcp`, or `right-memory`.

Revert the port to:

```rust
pub const PC_PORT: u16 = 18927;
```

- [ ] **Step 6: Confirm old imports and docs are gone**

Run:

```bash
devenv shell -- rg -n "right_core::(time_constants|injection_guard|runtime_state)|pub mod (time_constants|injection_guard|runtime_state)|crates/right-core/src/(time_constants|injection_guard|runtime_state)\\.rs" crates ARCHITECTURE.md docs/architecture
```

Expected: no matches.

- [ ] **Step 7: Final status check**

Run:

```bash
devenv shell -- git status --short
```

Expected: no uncommitted changes from Phase 1. Unrelated pre-existing modified files are allowed only if they were present before execution and were not touched by this plan.

- [ ] **Step 8: Final commit if verification caused doc updates**

If verification required any small doc corrections, commit only those corrections:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/modules.md docs/architecture/memory.md docs/architecture/sessions.md
devenv shell -- git diff --cached --name-only
devenv shell -- git commit -m "docs(architecture): fix phase one split drift"
```

Expected: skip this step if no files changed.
