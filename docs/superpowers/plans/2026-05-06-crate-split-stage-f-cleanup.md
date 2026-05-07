# Crate Split Stage F — Release-plz Wiring + Re-export Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the final piece of the crate-split refactor. Wire `release-plz.toml` to know about the 5 new internal crates so versions stay in lockstep and `CHANGELOG.md` aggregates everything; pin `publish = false` per-crate as a defence-in-depth; remove all transitional re-export shims from `right-agent` so editing a leaf crate no longer invalidates `right-agent`'s build cache; finalise `ARCHITECTURE.md`.

**Architecture:** Two independent groups of edits. **Group 1 (release-plz):** purely TOML — 5 `[[package]]` entries, one `changelog_include` extension, 5 `publish = false` lines. Zero impact on Rust code. **Group 2 (re-export cleanup):** drop ~15 `pub use right_core::X;` lines from `crates/right-agent/src/lib.rs` plus the three thin shim files in `mcp/mod.rs`, `codegen/mod.rs`, `memory/mod.rs`, then rewrite ~25 `crate::X::*` callsites inside `right-agent/src/` that currently route through those re-exports. External callers (in `bot/`, `right/`, `right-agent/tests/`) were already migrated by Stages B/C/D and verified zero-hits — they need no changes.

**Tech Stack:** Rust 2024, Cargo workspace, release-plz config (TOML), `rg`/`sed` for callsite rewrites. Spec at `docs/superpowers/specs/2026-05-06-crate-split-design.md` (commit `16429d54`). All commands run via `devenv shell -- <cmd>` because the project's CLAUDE.md mandates it when `devenv.nix` exists at repo root.

**Pre-existing context:**
- This plan **assumes Stages A, B, C, D are merged**. The branch tip (`feat/crate-split-stage-a-right-db` HEAD = `98c5fa3a`) reflects all four implemented.
- Verified inventory of internal `crate::*` callsites that route through re-export shims (run from worktree HEAD):
  - `crate::error::*` — 1 file (`agent/discovery.rs`).
  - `crate::ui::*` — 2 files (`doctor.rs`, `doctor_tests.rs`).
  - `crate::config::*` — 7 files (`doctor.rs`, `doctor_tests.rs`, `init.rs`, `tunnel/health.rs`, `agent/destroy.rs`, `agent/register.rs`, `rebootstrap.rs`).
  - `crate::openshell::*` — 3 files (`doctor.rs`, `agent/destroy.rs`, `rebootstrap.rs`).
  - `crate::openshell_proto::*` — 2 files (`doctor.rs:1002`, `rebootstrap.rs:21`).
  - `crate::stt::*` — 1 file (`doctor.rs`).
  - `crate::mcp::*` — 2 files (`doctor.rs`, `doctor_tests.rs`).
  - `crate::codegen::*` — 5 files (`init.rs`, `agent/destroy.rs`, `agent/register.rs`, `agent/types.rs`, `rebootstrap.rs`).
  - `crate::memory::*` — 3 files (`init.rs`, `doctor.rs`, `doctor_tests.rs`).
- No internal users of `crate::process_group`, `crate::sandbox_exec`, `crate::platform_store`, `crate::test_cleanup`, `crate::test_support` — those re-exports can simply be deleted with no callsite work.
- Current `release-plz.toml` (worktree HEAD): workspace defaults set `publish = false`, `git_tag_enable = false`, `git_release_enable = false`, `git_only = true`. Three explicit `[[package]]` blocks: `right-agent`, `right-bot`, `right` — only `right` has `git_release_enable = true` and `changelog_update = true`. The 5 new crates have NO entry — they currently inherit the workspace defaults but are NOT in the `version_group = "workspace"` cohort.
- Verified zero external `right_agent::{error,ui,config,openshell,openshell_proto,platform_store,process_group,sandbox_exec,stt,test_cleanup,test_support,mcp,codegen,memory}` callsites in `bot/src`, `right/src`, `right-agent/tests`. Stages B-D's bulk-rewrite did the right thing.

**Verification commands** (run from repo root or worktree):
- Build: `devenv shell -- cargo build --workspace`
- Test: `devenv shell -- cargo test --workspace`
- Lint: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
- Single-package check: `devenv shell -- cargo check -p <name>`
- Cargo metadata sanity: `devenv shell -- cargo metadata --no-deps --format-version 1 > /dev/null` (validates manifest syntax across the whole workspace)

---

## Task 1: Add `publish = false` to each new crate's `Cargo.toml`

**Files:**
- Modify: `crates/right-core/Cargo.toml`
- Modify: `crates/right-db/Cargo.toml`
- Modify: `crates/right-mcp/Cargo.toml`
- Modify: `crates/right-codegen/Cargo.toml`
- Modify: `crates/right-memory/Cargo.toml`

(`right-agent`, `right-bot`, `right` already work via the workspace-level `publish = false` in `release-plz.toml`. Adding it per-package is defence-in-depth: a stray `cargo publish` from the directory of a new crate cannot reach crates.io.)

- [ ] **Step 1: Edit each `[package]` block to add `publish = false`**

Open each file and insert `publish = false` immediately after the `edition.workspace = true` line. The resulting `[package]` block should look like (example for `right-core`):

```toml
[package]
name = "right-core"
version.workspace = true
edition.workspace = true
publish = false
```

Repeat for `right-db`, `right-mcp`, `right-codegen`, `right-memory`.

- [ ] **Step 2: Verify cargo still loads the manifest**

Run: `devenv shell -- cargo metadata --no-deps --format-version 1 > /dev/null`
Expected: succeeds (no manifest parse errors).

- [ ] **Step 3: Verify build still passes**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds.

- [ ] **Step 4: Commit**

```bash
git add crates/right-core/Cargo.toml crates/right-db/Cargo.toml crates/right-mcp/Cargo.toml crates/right-codegen/Cargo.toml crates/right-memory/Cargo.toml
git commit -m "chore(stage-f): pin publish = false on new internal crates"
```

---

## Task 2: Add `[[package]]` entries for the 5 new crates in `release-plz.toml`

**Files:**
- Modify: `release-plz.toml`

- [ ] **Step 1: Append five new `[[package]]` blocks**

Open `release-plz.toml`. After the existing block for `right` (currently ends near line 33), append:

```toml

[[package]]
name = "right-core"
version_group = "workspace"

[[package]]
name = "right-db"
version_group = "workspace"

[[package]]
name = "right-mcp"
version_group = "workspace"

[[package]]
name = "right-codegen"
version_group = "workspace"

[[package]]
name = "right-memory"
version_group = "workspace"
```

These blocks declare nothing else: `git_tag_enable`, `git_release_enable`, `publish`, `changelog_update` all inherit the `[workspace]` defaults already at the top of the file (`false`, `false`, `false`, `false` respectively). The only thing they add is membership in `version_group = "workspace"` so release-plz keeps them in lockstep with `right-agent`, `right-bot`, `right`.

- [ ] **Step 2: Verify release-plz parses the file (if available)**

Run (only if `release-plz` is on the path):

```bash
devenv shell -- which release-plz && devenv shell -- release-plz --help > /dev/null
```

If `release-plz` isn't installed locally, skip to Step 3 — the actual validation happens when CI runs release-plz, and TOML syntax is verified by `cargo metadata` indirectly.

- [ ] **Step 3: Verify `cargo metadata` still loads the workspace**

Run: `devenv shell -- cargo metadata --no-deps --format-version 1 > /dev/null`
Expected: succeeds.

- [ ] **Step 4: Commit**

```bash
git add release-plz.toml
git commit -m "chore(stage-f): register new internal crates with release-plz workspace version-group"
```

---

## Task 3: Extend `changelog_include` on the `right` package block

**Files:**
- Modify: `release-plz.toml`

- [ ] **Step 1: Edit the `right` block's `changelog_include`**

Find the `[[package]] name = "right"` block. Currently:

```toml
[[package]]
name = "right"
version_group = "workspace"
changelog_update = true
changelog_path = "CHANGELOG.md"
changelog_include = ["right-agent", "right-bot"]
git_release_enable = true
git_release_name = "v{{ version }}"
git_tag_enable = true
git_tag_name = "v{{ version }}"
```

Change the `changelog_include` line to:

```toml
changelog_include = ["right-agent", "right-bot", "right-core", "right-db", "right-mcp", "right-codegen", "right-memory"]
```

This makes `CHANGELOG.md` (cliff-rendered, owned by the `right` package) aggregate commits across every internal crate. Without this change, the next release's changelog would only see commits touching `right-agent`/`right-bot` and miss the bulk of post-Stage-B/C/D edits.

- [ ] **Step 2: Verify the file still parses**

Run: `devenv shell -- cargo metadata --no-deps --format-version 1 > /dev/null`
Expected: succeeds.

- [ ] **Step 3: Commit**

```bash
git add release-plz.toml
git commit -m "chore(stage-f): include all internal crates in CHANGELOG.md aggregation"
```

---

## Task 4: Drop unused `pub use right_core::*` re-exports from `right-agent/src/lib.rs`

**Files:**
- Modify: `crates/right-agent/src/lib.rs`

There are 5 re-exports with NO internal callers. Removing them is mechanical and immediate.

- [ ] **Step 1: Inventory — confirm zero internal users for each candidate**

Run from worktree:

```bash
for mod in process_group sandbox_exec platform_store test_cleanup test_support; do
  echo "=== crate::${mod} internal usage ==="
  rg -n "crate::${mod}\b" crates/right-agent/src 2>/dev/null
done
```

Expected: **zero hits** for every name. (Internal usage was inventoried at plan-write time; if the count drifted because Stages A-D introduced new uses, fold those into the appropriate later task instead of this one.)

- [ ] **Step 2: Delete five lines from `lib.rs`**

Open `crates/right-agent/src/lib.rs`. Delete these five lines:

```rust
pub use right_core::platform_store;
#[cfg(unix)]
pub use right_core::process_group;
pub use right_core::sandbox_exec;
#[cfg(unix)]
pub use right_core::test_cleanup;
#[cfg(all(unix, any(test, feature = "test-support")))]
pub use right_core::test_support;
```

(There are 5 logical re-exports; some come with two-line `#[cfg]` attributes — make sure to delete the `#[cfg]` line that immediately precedes each `pub use` it gates.)

- [ ] **Step 3: Build the workspace**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds. If anything fails, the inventory in Step 1 was wrong — locate the user, decide whether to fix it now (rewrite to `right_core::*`) or revert this task.

- [ ] **Step 4: Run tests to confirm nothing broke**

Run: `devenv shell -- cargo test --workspace`
Expected: passes.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/lib.rs
git commit -m "refactor(stage-f): drop unused right-core re-exports from right-agent"
```

---

## Task 5: Rewrite `crate::error::*` callsites + drop the `error` re-export

**Files:**
- Modify: `crates/right-agent/src/agent/discovery.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Rewrite the single callsite**

Open `crates/right-agent/src/agent/discovery.rs:4`:

```rust
use crate::error::AgentError;
```

Change to:

```rust
use right_core::error::AgentError;
```

- [ ] **Step 2: Inventory — confirm no other internal users**

Run:

```bash
rg -n 'crate::error\b' crates/right-agent/src
```

Expected: zero hits. If any remains, edit it to `right_core::error::*` before proceeding.

- [ ] **Step 3: Drop the re-export from `lib.rs`**

Delete the line:

```rust
pub use right_core::error;
```

- [ ] **Step 4: Build + test**

Run: `devenv shell -- cargo build -p right-agent && devenv shell -- cargo test -p right-agent --lib`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/lib.rs crates/right-agent/src/agent/discovery.rs
git commit -m "refactor(stage-f): switch right-agent internal callers to right_core::error"
```

---

## Task 6: Rewrite `crate::ui::*` callsites + drop the `ui` re-export

**Files:**
- Modify: `crates/right-agent/src/doctor.rs`
- Modify: `crates/right-agent/src/doctor_tests.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Rewrite callsites — bulk sed**

```bash
rg -l 'crate::ui\b' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::ui::|right_core::ui::|g; s|use crate::ui\b|use right_core::ui|g'
find crates/right-agent/src -name '*.bak' -delete
```

- [ ] **Step 2: Inventory — confirm zero remaining**

Run: `rg -n 'crate::ui\b' crates/right-agent/src`
Expected: zero hits.

- [ ] **Step 3: Drop the re-export from `lib.rs`**

Delete the line:

```rust
pub use right_core::ui;
```

- [ ] **Step 4: Build + test**

Run: `devenv shell -- cargo build -p right-agent && devenv shell -- cargo test -p right-agent --lib`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/lib.rs crates/right-agent/src/doctor.rs crates/right-agent/src/doctor_tests.rs
git commit -m "refactor(stage-f): switch right-agent internal callers to right_core::ui"
```

---

## Task 7: Rewrite `crate::config::*` callsites + drop the `config` re-export

**Files:**
- Modify: `crates/right-agent/src/doctor.rs`
- Modify: `crates/right-agent/src/doctor_tests.rs`
- Modify: `crates/right-agent/src/init.rs`
- Modify: `crates/right-agent/src/tunnel/health.rs`
- Modify: `crates/right-agent/src/agent/destroy.rs`
- Modify: `crates/right-agent/src/agent/register.rs`
- Modify: `crates/right-agent/src/rebootstrap.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Rewrite callsites — bulk sed**

```bash
rg -l 'crate::config\b' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::config::|right_core::config::|g; s|use crate::config\b|use right_core::config|g'
find crates/right-agent/src -name '*.bak' -delete
```

- [ ] **Step 2: Inventory**

Run: `rg -n 'crate::config\b' crates/right-agent/src`
Expected: zero hits.

- [ ] **Step 3: Drop the re-export**

Delete from `lib.rs`:

```rust
pub use right_core::config;
```

- [ ] **Step 4: Build + test**

Run: `devenv shell -- cargo build -p right-agent && devenv shell -- cargo test -p right-agent --lib`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/lib.rs crates/right-agent/src/doctor.rs crates/right-agent/src/doctor_tests.rs crates/right-agent/src/init.rs crates/right-agent/src/tunnel/health.rs crates/right-agent/src/agent/destroy.rs crates/right-agent/src/agent/register.rs crates/right-agent/src/rebootstrap.rs
git commit -m "refactor(stage-f): switch right-agent internal callers to right_core::config"
```

---

## Task 8: Rewrite `crate::openshell::*` and `crate::openshell_proto::*` callsites + drop both re-exports

**Files:**
- Modify: `crates/right-agent/src/doctor.rs`
- Modify: `crates/right-agent/src/agent/destroy.rs`
- Modify: `crates/right-agent/src/rebootstrap.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Rewrite callsites — bulk sed**

```bash
rg -l 'crate::openshell\b' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::openshell_proto::|right_core::openshell_proto::|g; s|crate::openshell::|right_core::openshell::|g; s|use crate::openshell_proto\b|use right_core::openshell_proto|g; s|use crate::openshell\b|use right_core::openshell|g'
find crates/right-agent/src -name '*.bak' -delete
```

The order of the substitutions matters: rewrite the longer prefix `openshell_proto` BEFORE `openshell` so the second sed pass doesn't match part of the first.

- [ ] **Step 2: Inventory**

Run: `rg -n 'crate::openshell\b|crate::openshell_proto\b' crates/right-agent/src`
Expected: zero hits.

- [ ] **Step 3: Drop the two re-exports**

Delete from `lib.rs`:

```rust
pub use right_core::openshell;
pub use right_core::openshell_proto;
```

- [ ] **Step 4: Build + test**

Run: `devenv shell -- cargo build -p right-agent && devenv shell -- cargo test -p right-agent --lib`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/lib.rs crates/right-agent/src/doctor.rs crates/right-agent/src/agent/destroy.rs crates/right-agent/src/rebootstrap.rs
git commit -m "refactor(stage-f): switch right-agent internal callers to right_core::openshell{,_proto}"
```

---

## Task 9: Rewrite `crate::stt::*` callsites + drop the `stt` re-export

**Files:**
- Modify: `crates/right-agent/src/doctor.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Rewrite callsites**

```bash
rg -l 'crate::stt\b' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::stt::|right_core::stt::|g; s|use crate::stt\b|use right_core::stt|g'
find crates/right-agent/src -name '*.bak' -delete
```

- [ ] **Step 2: Inventory**

Run: `rg -n 'crate::stt\b' crates/right-agent/src`
Expected: zero hits.

- [ ] **Step 3: Drop the re-export**

Delete from `lib.rs`:

```rust
pub use right_core::stt;
```

- [ ] **Step 4: Build + test**

Run: `devenv shell -- cargo build -p right-agent && devenv shell -- cargo test -p right-agent --lib`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/lib.rs crates/right-agent/src/doctor.rs
git commit -m "refactor(stage-f): switch right-agent internal callers to right_core::stt"
```

---

## Task 10: Rewrite `crate::mcp::*` callsites + delete the `mcp/mod.rs` shim

**Files:**
- Modify: `crates/right-agent/src/doctor.rs`
- Modify: `crates/right-agent/src/doctor_tests.rs`
- Delete: `crates/right-agent/src/mcp/mod.rs`
- Modify: `crates/right-agent/src/lib.rs`

After this task, `crate::mcp` does not exist as a module path within `right-agent`. The slim `right-agent` crate has no need for it (all real code lives in `right-mcp`).

- [ ] **Step 1: Rewrite callsites**

```bash
rg -l 'crate::mcp\b' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::mcp::|right_mcp::|g; s|use crate::mcp\b|use right_mcp|g'
find crates/right-agent/src -name '*.bak' -delete
```

- [ ] **Step 2: Inventory**

Run: `rg -n 'crate::mcp\b' crates/right-agent/src`
Expected: zero hits.

- [ ] **Step 3: Delete the shim file and its parent declaration**

Confirm `mcp/` is empty except for `mod.rs`:

```bash
ls crates/right-agent/src/mcp/
```

Expected output: only `mod.rs`.

```bash
git rm -r crates/right-agent/src/mcp
```

In `crates/right-agent/src/lib.rs`, delete the line:

```rust
pub mod mcp;
```

- [ ] **Step 4: Build + test**

Run: `devenv shell -- cargo build -p right-agent && devenv shell -- cargo test -p right-agent --lib`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/lib.rs crates/right-agent/src/doctor.rs crates/right-agent/src/doctor_tests.rs crates/right-agent/src/mcp
git commit -m "refactor(stage-f): drop right-agent::mcp shim, switch internal callers to right_mcp"
```

---

## Task 11: Rewrite `crate::codegen::*` callsites + delete the `codegen/mod.rs` shim

**Files:**
- Modify: `crates/right-agent/src/init.rs`
- Modify: `crates/right-agent/src/agent/destroy.rs`
- Modify: `crates/right-agent/src/agent/register.rs`
- Modify: `crates/right-agent/src/agent/types.rs`
- Modify: `crates/right-agent/src/rebootstrap.rs`
- Delete: `crates/right-agent/src/codegen/mod.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Rewrite callsites**

```bash
rg -l 'crate::codegen\b' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::codegen::|right_codegen::|g; s|use crate::codegen\b|use right_codegen|g'
find crates/right-agent/src -name '*.bak' -delete
```

- [ ] **Step 2: Inventory**

Run: `rg -n 'crate::codegen\b' crates/right-agent/src`
Expected: zero hits.

There may be doc-comment references like `/// Delegates to [`crate::codegen::contract::write_merged_rmw`]` (`agent/types.rs:9`) — those are inside `///` lines and `rg`'s pattern still catches them. Edit the doc comment to read `[\`right_codegen::contract::write_merged_rmw\`]` so rustdoc resolves the link.

- [ ] **Step 3: Delete the shim file and its parent declaration**

```bash
ls crates/right-agent/src/codegen/
```

Expected: only `mod.rs` (post-Stage-C). If anything else remains, the previous stages didn't fully evacuate; investigate before deleting.

```bash
git rm -r crates/right-agent/src/codegen
```

In `crates/right-agent/src/lib.rs`, delete:

```rust
pub mod codegen;
```

- [ ] **Step 4: Build + test**

Run: `devenv shell -- cargo build -p right-agent && devenv shell -- cargo test -p right-agent --lib`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/lib.rs crates/right-agent/src/init.rs crates/right-agent/src/agent crates/right-agent/src/rebootstrap.rs crates/right-agent/src/codegen
git commit -m "refactor(stage-f): drop right-agent::codegen shim, switch internal callers to right_codegen"
```

---

## Task 12: Rewrite `crate::memory::*` callsites + delete the `memory/mod.rs` shim

**Files:**
- Modify: `crates/right-agent/src/init.rs`
- Modify: `crates/right-agent/src/doctor.rs`
- Modify: `crates/right-agent/src/doctor_tests.rs`
- Delete: `crates/right-agent/src/memory/mod.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Triage `crate::memory::*` patterns**

`crate::memory::open_db` and `crate::memory::open_connection` should map to `right_db::*`. Everything else (`crate::memory::retain_queue`, `crate::memory::hindsight`, `crate::memory::MemoryError`, `crate::memory::alert_types`) maps to `right_memory::*`.

```bash
# First the right-db helpers (open_*):
rg -l 'crate::memory::(open_connection|open_db)' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::memory::open_connection|right_db::open_connection|g; s|crate::memory::open_db|right_db::open_db|g'
# Then everything else under crate::memory::* → right_memory::*:
rg -l 'crate::memory\b' crates/right-agent/src \
  | xargs sed -i.bak 's|crate::memory::|right_memory::|g; s|use crate::memory\b|use right_memory|g'
find crates/right-agent/src -name '*.bak' -delete
```

- [ ] **Step 2: Inventory**

Run: `rg -n 'crate::memory\b' crates/right-agent/src`
Expected: zero hits.

- [ ] **Step 3: Delete the shim file and its parent declaration**

```bash
ls crates/right-agent/src/memory/
```

Expected: only `mod.rs` (post-Stage-C, after `right-memory` extracted).

```bash
git rm -r crates/right-agent/src/memory
```

In `crates/right-agent/src/lib.rs`, delete:

```rust
pub mod memory;
```

- [ ] **Step 4: Build + test**

Run: `devenv shell -- cargo build -p right-agent && devenv shell -- cargo test -p right-agent --lib`
Expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/lib.rs crates/right-agent/src/init.rs crates/right-agent/src/doctor.rs crates/right-agent/src/doctor_tests.rs crates/right-agent/src/memory
git commit -m "refactor(stage-f): drop right-agent::memory shim, switch internal callers to right_db/right_memory"
```

---

## Task 13: Final inventory — confirm `right-agent` is shim-free

**Files:** none (verification + tiny `lib.rs` adjustment if needed)

- [ ] **Step 1: Inspect the final `lib.rs`**

Run: `cat crates/right-agent/src/lib.rs`

Expected content (no `pub use right_core::*` lines, no `pub mod {mcp,codegen,memory}`):

```rust
pub mod agent;
pub mod cron_spec;
pub mod doctor;
pub mod init;
pub mod rebootstrap;
pub mod runtime;
pub mod tunnel;
pub mod usage;
```

If anything else remains (an attribute `#[cfg(...)]` left orphaned after a deletion, a stray `pub mod` for an already-deleted directory), fix in-place.

- [ ] **Step 2: Confirm no internal `crate::*` route through deleted re-exports**

```bash
rg -n 'crate::(error|ui|config|openshell|openshell_proto|platform_store|process_group|sandbox_exec|stt|test_cleanup|test_support|mcp|codegen|memory)\b' crates/right-agent/src
```

Expected: zero hits.

- [ ] **Step 3: Confirm zero external callers route through deleted re-exports**

```bash
rg -n 'right_agent::(error|ui|config|openshell|openshell_proto|platform_store|process_group|sandbox_exec|stt|test_cleanup|test_support|mcp|codegen|memory)\b' crates/bot crates/right crates/right-agent/tests
```

Expected: zero hits. (Stages B-D verified at write-time; Stage F only confirms the property still holds.)

If any external hit appears, it's a bug — rewrite the offending file and add the fix to whichever earlier-stage commit context fits.

- [ ] **Step 4: Commit any cleanup**

```bash
git add <fixed files>
git commit -m "fix(stage-f): final cleanup of stale re-export references"
```

(Skip if no fixes needed.)

---

## Task 14: Whole-workspace build, test, lint pass

**Files:** none (verification only)

- [ ] **Step 1: Whole-workspace build (debug)**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds with zero warnings.

- [ ] **Step 2: Whole-workspace build (release)**

Run: `devenv shell -- cargo build --workspace --release`
Expected: succeeds.

- [ ] **Step 3: Whole-workspace test**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass — including `TestSandbox`-using integration tests (dev machine has OpenShell per CLAUDE.md). Total test count should match the Stage-D-end count of ~1430+ (1424 from Stage D verification + 8 ported by the test-restore agent + 9 net new = 1441 ± deltas from any new tests during Stage F).

- [ ] **Step 4: Whole-workspace clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 5: Build-time benchmark — verify Stage F actually improves incremental builds**

The whole point of removing the re-exports is so editing a leaf crate stops invalidating `right-agent`. Test it:

```bash
devenv shell -- cargo clean
devenv shell -- cargo build --workspace --timings  # baseline cold build
# Touch one file in right-codegen
touch crates/right-codegen/src/skills.rs
devenv shell -- cargo build --workspace --timings  # incremental build after edit
```

Save both `target/cargo-timings/cargo-timing-*.html` files outside the repo (e.g. `~/Desktop/stage-f-timing-*.html`). Note the second build's wall-clock time — it should rebuild only `right-codegen`, `right-agent`, `right-bot`, `right` (NOT `right-core`, `right-db`, `right-mcp`, `right-memory`). Compare against the Stage D timing artifact (saved during Stage D Task 11 Step 5) to quantify the win.

Document the wall-clock numbers in the eventual PR description.

- [ ] **Step 6: Fix in-place if any of the above fails**

Common Stage F failure modes:
- A `crate::` path inside a `///` doc-comment that `rg` flagged but you missed editing (rustdoc fails the build).
- An orphaned `#[cfg(unix)]` attribute left behind when its `pub use` line was deleted (compile error).
- A leaf-crate's `Cargo.toml` is missing `publish = false` and someone tries `cargo publish --dry-run` (defence-in-depth — fix by adding the line).

```bash
git add <fixed files>
git commit -m "fix(stage-f): resolve cleanups after re-export removal"
```

---

## Task 15: Run `rust-dev:review-rust-code` agent

**Files:** none (review only)

- [ ] **Step 1: Dispatch**

Use the `rust-dev:review-rust-code` agent with this prompt:

> Review changes on the current branch since `<sha-of-stage-f-start>`. Focus on:
> 1. The release-plz config — are the 5 new `[[package]]` blocks consistent with the existing three? Any missing inheritance (e.g. did I forget to set `version_group = "workspace"` on one)?
> 2. The `changelog_include` extension — does it cover all internal crates, AND only internal crates (not e.g. `right-bin` if such a thing existed by mistake)?
> 3. The internal-callsite rewrites in `right-agent/src/*` — did the bulk-sed accidentally rewrite a doc-comment that mentioned `crate::ui::Theme` as an example, breaking rustdoc?
> 4. The deleted shim files (`right-agent/src/{mcp,codegen,memory}/mod.rs`) — confirm no other source files in `right-agent` had `mod mcp;` / `mod codegen;` / `mod memory;` declarations elsewhere (should only be `lib.rs`).
> 5. The `lib.rs` after cleanup — is it minimal and only declaring the genuine slim-agent modules (`agent, cron_spec, doctor, init, rebootstrap, runtime, tunnel, usage`)?
>
> Don't fix; report. Output as TODO list with file:line references.

- [ ] **Step 2: Triage findings**

Bugs → followup file `docs/superpowers/plans/2026-05-06-stage-f-followups.md`, fixed one per commit. Nitpicks → defer. Misunderstandings → ignore.

- [ ] **Step 3: Confirm tests after fixes**

Run: `devenv shell -- cargo test --workspace`

- [ ] **Step 4: Commit fixes / followup file**

```bash
git add <files>
git commit -m "fix(stage-f): address review-rust-code findings"
```

---

## Task 16: Update `ARCHITECTURE.md` (final pass)

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Refresh the `## Workspace` table**

Open `ARCHITECTURE.md`. Confirm the workspace table reflects the final 8-crate layout (it should already from Stages A/B/C; this step confirms and patches anything off):

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

- [ ] **Step 2: Add a "Re-export hygiene" note**

Append a paragraph under the workspace section:

```markdown
**Re-export discipline:** The slim `right-agent` does NOT re-export modules
from `right-core`, `right-db`, `right-mcp`, `right-codegen`, or `right-memory`.
Consumers (CLI, bot, agent itself) import directly from the source crate.
This is what keeps the build-cache invariant: an edit inside `right-codegen`
rebuilds `right-codegen` plus its direct consumers, not `right-agent`.
```

- [ ] **Step 3: Refresh the `Configuration Hierarchy` and `Codegen categories` sections**

These sections currently mention `right-agent` paths for codegen-output writers. Confirm they reference the new locations (`right_codegen::contract::write_regenerated`, etc.). Cite-on-touch per CLAUDE.md.

```bash
rg -n 'right_agent::codegen|right_agent::mcp|right_agent::memory|right_agent::error|right_agent::ui|right_agent::config|right_agent::openshell|right_agent::platform_store|right_agent::stt' ARCHITECTURE.md docs/architecture
```

For each hit, decide: keep (e.g. when documenting an intentional historical reference) or update to the appropriate `right_*::*` path. Default: update.

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md docs/architecture
git commit -m "docs(arch): finalize workspace map after Stage F shim removal"
```

---

## Task 17: Final verification + summary commit

**Files:** none (verification + an optional summary commit)

- [ ] **Step 1: Re-run the full check suite**

```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo build --workspace --release
devenv shell -- cargo test --workspace
devenv shell -- cargo clippy --workspace --all-targets -- -D warnings
devenv shell -- cargo metadata --no-deps --format-version 1 > /dev/null
```

Expected: all five pass.

- [ ] **Step 2: Inventory check — every shim and re-export is gone**

```bash
# A) right-agent has no transitional re-exports for moved modules:
rg -n 'pub use right_(core|db|mcp|codegen|memory)::' crates/right-agent/src/lib.rs
# Expected: empty.
# B) right-agent has no shim modules for moved subsystems:
rg -n '^pub mod (mcp|codegen|memory);$' crates/right-agent/src/lib.rs
# Expected: empty.
# C) No internal crate::* paths route through removed re-exports:
rg -n 'crate::(error|ui|config|openshell|openshell_proto|platform_store|process_group|sandbox_exec|stt|test_cleanup|test_support|mcp|codegen|memory)\b' crates/right-agent/src
# Expected: empty.
# D) No external right_agent::* paths route through removed re-exports:
rg -n 'right_agent::(error|ui|config|openshell|openshell_proto|platform_store|process_group|sandbox_exec|stt|test_cleanup|test_support|mcp|codegen|memory)\b' crates/bot crates/right crates/right-agent/tests
# Expected: empty.
```

If any check returns hits, fix in-place and commit.

- [ ] **Step 3: Confirm `release-plz.toml` covers all 8 internal crates in `version_group = "workspace"`**

```bash
rg -n 'version_group = "workspace"' release-plz.toml
```

Expected: 8 hits (right-agent, right-bot, right, right-core, right-db, right-mcp, right-codegen, right-memory).

```bash
rg -n 'changelog_include' release-plz.toml
```

Expected: one hit, listing all 7 internal-non-CLI crates: `["right-agent", "right-bot", "right-core", "right-db", "right-mcp", "right-codegen", "right-memory"]`.

- [ ] **Step 4: Optional summary commit**

```bash
git commit --allow-empty -m "chore(stage-f): release-plz wiring and re-export cleanup complete"
```

- [ ] **Step 5: Open a PR (if working on a branch) or merge directly**

If a feature branch was used for the whole crate-split work (likely `feat/crate-split-stage-a-right-db`), this PR title is `Stage F: release-plz + cleanup (final)`. Body references the spec at `docs/superpowers/specs/2026-05-06-crate-split-design.md` and this plan, plus the build-timing comparison from Task 14 Step 5.

If Stages A-E are already squash-merged or the branch is the whole crate-split, this might be the final PR for the entire effort — note that the reviewer should verify the cumulative diff makes sense, not just Stage F's contribution.
