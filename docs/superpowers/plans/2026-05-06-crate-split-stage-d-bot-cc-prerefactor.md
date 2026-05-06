# Crate Split Stage D — In-Bot `telegram → cc` Pre-Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Untangle the `bot::telegram::` subtree by hoisting all generic Claude Code (CC) invocation plumbing into a new `bot::cc::` module — without creating a new crate. After this stage, `bot::cron`, `bot::cron_delivery`, and `bot::reflection` no longer import from `bot::telegram::*`, and `bot::telegram::*` becomes Telegram-specific again. This unblocks Stage E's clean extraction of both `right-cc` and `right-telegram`.

**Architecture:** Create a sibling `bot::cc` module under `crates/bot/src/cc/`. Move four files wholesale (`invocation`, `prompt`, `stream`, plus a new `worker_reply` lifted out of `telegram::worker`). Surgically extract two types and three helpers (`OutboundAttachment` + `OutboundKind`, `html_escape` + `strip_html_tags` + `html_escape_into`) into new files (`cc::attachments_dto`, `cc::markdown_utils`). The remaining Telegram-specific code in `telegram::attachments`, `telegram::markdown`, `telegram::worker` keeps its identity and re-imports the moved items via `crate::cc::*`. Build-time effect on its own: zero. The reward arrives in Stage E.

**Tech Stack:** Rust 2024 (no Cargo changes — same crate). Spec at `docs/superpowers/specs/2026-05-06-crate-split-design.md` (commit `16429d54`). All commands run via `devenv shell -- <cmd>` because the project's CLAUDE.md mandates it when `devenv.nix` exists at repo root.

**Pre-existing context:**
- This plan can run **in parallel** with Stages A/B/C — it touches only `crates/bot/src/`. The spec's stage-DAG documents this.
- Files moving wholesale: `crates/bot/src/telegram/{invocation.rs (427 LoC), prompt.rs (576 LoC), stream.rs (593 LoC)}` → `crates/bot/src/cc/{invocation.rs, prompt.rs, stream.rs}`. They contain no Telegram-specific symbols; the names are misleading historical artefacts.
- Surgical extraction targets:
  - `crates/bot/src/telegram/worker.rs:148-156` defines `pub struct ReplyOutput` (5 fields, `serde::Deserialize`).
  - `crates/bot/src/telegram/worker.rs:314-...` defines `pub fn parse_reply_output` (90 LoC body) and `BOOTSTRAP_REQUIRED_FILES` constant + `should_accept_bootstrap` helper used inside the parse path.
  - `crates/bot/src/telegram/worker.rs` lines 2572-end host ~17 unit tests for `parse_reply_output` (named `parse_reply_output_*`). They migrate with the function.
  - `crates/bot/src/telegram/attachments.rs:56-86` defines `pub struct OutboundAttachment` and `pub enum OutboundKind`. The rest of `attachments.rs` (`GroupKind`, `classify_media_group`, `send_attachments`, `send_single`, `send_group`, `send_voice`, etc.) consumes these types and stays in `telegram::`.
  - `crates/bot/src/telegram/markdown.rs:265-294` defines `pub(crate) fn html_escape` + `pub(crate) fn html_escape_into`. Line 396-... defines `pub fn strip_html_tags`. The functions `md_to_telegram_html` (line 13) and `split_html_message` (line 299) stay in `telegram::markdown` because they emit Telegram-flavoured HTML. `md_to_telegram_html` calls `html_escape_into` internally — so after the move, `telegram::markdown` re-imports `html_escape_into` from `crate::cc::markdown_utils`.
- Consumers of the items being moved (callsite-level inventory; the implementer should re-grep at task time to catch drift):
  - `bot::cron` (`crates/bot/src/cron.rs`): uses `crate::telegram::invocation::*` (4 refs), `crate::telegram::prompt::*` (3 refs), `crate::telegram::stream::*` (4 refs), `crate::telegram::attachments::OutboundAttachment` (1 ref).
  - `bot::cron_delivery` (`crates/bot/src/cron_delivery.rs`): `invocation::*` (4 refs), `prompt::*` (3 refs), `worker::parse_reply_output` (1 ref), `markdown::strip_html_tags` (1 ref), `markdown::md_to_telegram_html` + `split_html_message` (2 refs — STAY in telegram::markdown).
  - `bot::reflection` (`crates/bot/src/reflection.rs`): `invocation::*` (4 refs), `stream::*` (3 refs), `prompt::*` (2 refs), `worker::parse_reply_output` (1 ref).
  - `bot::telegram::worker`: imports its own `markdown::{html_escape, strip_html_tags}` (line 175); `ReplyOutput`'s `attachments` field references `super::attachments::OutboundAttachment` (line 153).
  - `bot::telegram::handler`: `markdown::html_escape` (1 ref).
  - `bot::cron::CronNotify`: holds `Vec<crate::telegram::attachments::OutboundAttachment>` (line 44) — the type-import path moves to `crate::cc::attachments_dto`.

**Verification commands** (run from repo root):
- Build: `devenv shell -- cargo build -p right-bot` (or `--workspace`)
- Test: `devenv shell -- cargo test -p right-bot`
- Lint: `devenv shell -- cargo clippy -p right-bot --all-targets -- -D warnings`

---

## Task 1: Create `bot::cc` module skeleton

**Files:**
- Create: `crates/bot/src/cc/mod.rs`
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Create the directory and a stub `mod.rs`**

```bash
mkdir -p crates/bot/src/cc
```

Create `crates/bot/src/cc/mod.rs`:

```rust
//! CC (Claude Code) subprocess plumbing — generic, not Telegram-specific.
//!
//! Holds the `ClaudeInvocation` builder, prompt-assembly script generation,
//! stream-event parser, structured-reply parser, and DTOs (`OutboundAttachment`,
//! markdown utilities) shared between Telegram delivery and cron jobs.
//!
//! Stage E will lift this entire subtree into a `right-cc` crate.

pub mod attachments_dto;
pub mod invocation;
pub mod markdown_utils;
pub mod prompt;
pub mod stream;
pub mod worker_reply;
```

(Submodule files don't yet exist; subsequent tasks fill them.)

- [ ] **Step 2: Declare `cc` in `bot/src/lib.rs`**

Open `crates/bot/src/lib.rs`. Find the cluster of `pub mod` declarations near the top (`pub mod cron; pub mod cron_delivery; ... pub mod telegram;`). Insert (alphabetically, between `cron_delivery` and `error`):

```rust
pub mod cc;
```

- [ ] **Step 3: Verify the bot crate doesn't compile yet (because submodule files are missing) — that's expected**

Run: `devenv shell -- cargo check -p right-bot 2>&1 | head -10`
Expected: errors about missing module files. Subsequent tasks add them.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/cc crates/bot/src/lib.rs
git commit -m "feat(bot): scaffold cc module"
```

---

## Task 2: Move `telegram/invocation.rs` → `cc/invocation.rs`

**Files:**
- Move: `crates/bot/src/telegram/invocation.rs` → `crates/bot/src/cc/invocation.rs`
- Modify: `crates/bot/src/telegram/mod.rs` (drop `pub mod invocation;` if present)

- [ ] **Step 1: Move the file**

```bash
git mv crates/bot/src/telegram/invocation.rs crates/bot/src/cc/invocation.rs
```

- [ ] **Step 2: Drop the `telegram::invocation` declaration**

Open `crates/bot/src/telegram/mod.rs` and remove the line `pub mod invocation;`. (If the line uses `pub(crate)` or another modifier, remove that whole line.)

- [ ] **Step 3: Rewrite imports inside the moved file**

The file may reference `super::*` (which was `telegram::*`). After the move, `super` is `cc`, not `telegram`. Inventory:

```bash
devenv shell -- rg -n 'super::' crates/bot/src/cc/invocation.rs
```

Replace each `super::X` with the appropriate full path:
- If it referred to a sibling that's also moving (e.g. `super::prompt::*`) → `crate::cc::prompt::*` (will resolve once Task 4 lands; OK to leave for now and let cargo error confirm).
- If it referred to a Telegram-specific sibling that stays → `crate::telegram::X`.

Use targeted edits, not bulk sed (substitutions vary).

- [ ] **Step 4: Update consumers — bulk replace `crate::telegram::invocation::` with `crate::cc::invocation::`**

```bash
devenv shell -- rg -l 'crate::telegram::invocation' crates/bot/src \
  | xargs sed -i.bak 's|crate::telegram::invocation|crate::cc::invocation|g'
devenv shell -- find crates/bot/src -name '*.bak' -delete
```

- [ ] **Step 5: Build to validate**

Run: `devenv shell -- cargo check -p right-bot`
Expected: still fails because `cc::prompt`, `cc::stream`, `cc::attachments_dto`, `cc::markdown_utils`, `cc::worker_reply` files don't exist yet. The errors should mention only those — not `cc::invocation`.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cc/invocation.rs crates/bot/src/telegram crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs crates/bot/src/reflection.rs
git commit -m "refactor(bot): move telegram::invocation to cc::invocation"
```

---

## Task 3: Move `telegram/prompt.rs` → `cc/prompt.rs`

**Files:**
- Move: `crates/bot/src/telegram/prompt.rs` → `crates/bot/src/cc/prompt.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Move the file**

```bash
git mv crates/bot/src/telegram/prompt.rs crates/bot/src/cc/prompt.rs
```

- [ ] **Step 2: Drop the `telegram::prompt` declaration**

Remove `pub mod prompt;` from `crates/bot/src/telegram/mod.rs`.

- [ ] **Step 3: Rewrite internal `super::*` paths**

```bash
devenv shell -- rg -n 'super::' crates/bot/src/cc/prompt.rs
```

Edit each manually, mapping `super::X` → `crate::cc::X` if X is also moving, or `crate::telegram::X` if X stays.

- [ ] **Step 4: Bulk-replace consumers**

```bash
devenv shell -- rg -l 'crate::telegram::prompt' crates/bot/src \
  | xargs sed -i.bak 's|crate::telegram::prompt|crate::cc::prompt|g'
devenv shell -- find crates/bot/src -name '*.bak' -delete
```

- [ ] **Step 5: Validate build**

Run: `devenv shell -- cargo check -p right-bot`
Expected: errors only about remaining missing `cc::*` modules.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cc/prompt.rs crates/bot/src/telegram crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs crates/bot/src/reflection.rs
git commit -m "refactor(bot): move telegram::prompt to cc::prompt"
```

---

## Task 4: Move `telegram/stream.rs` → `cc/stream.rs`

**Files:**
- Move: `crates/bot/src/telegram/stream.rs` → `crates/bot/src/cc/stream.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Move the file**

```bash
git mv crates/bot/src/telegram/stream.rs crates/bot/src/cc/stream.rs
```

- [ ] **Step 2: Drop the `telegram::stream` declaration**

Remove `pub mod stream;` from `crates/bot/src/telegram/mod.rs`.

- [ ] **Step 3: Rewrite internal `super::*` paths in the moved file**

```bash
devenv shell -- rg -n 'super::' crates/bot/src/cc/stream.rs
```

Edit each manually as in Tasks 2-3.

- [ ] **Step 4: Bulk-replace consumers**

```bash
devenv shell -- rg -l 'crate::telegram::stream' crates/bot/src \
  | xargs sed -i.bak 's|crate::telegram::stream|crate::cc::stream|g'
devenv shell -- find crates/bot/src -name '*.bak' -delete
```

- [ ] **Step 5: Validate build**

Run: `devenv shell -- cargo check -p right-bot`
Expected: errors only about remaining missing `cc::*` modules.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cc/stream.rs crates/bot/src/telegram crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs crates/bot/src/reflection.rs
git commit -m "refactor(bot): move telegram::stream to cc::stream"
```

---

## Task 5: Extract `OutboundAttachment` and `OutboundKind` into `cc::attachments_dto`

**Files:**
- Create: `crates/bot/src/cc/attachments_dto.rs`
- Modify: `crates/bot/src/telegram/attachments.rs` (remove the two type definitions, import them from `cc`)

- [ ] **Step 1: Create `cc/attachments_dto.rs` with the two types**

Open `crates/bot/src/telegram/attachments.rs`. Locate the `pub struct OutboundAttachment` block (≈line 56) and the `pub enum OutboundKind` block (≈line 69). Copy both verbatim, including their derives, into a new file:

`crates/bot/src/cc/attachments_dto.rs`:

```rust
//! Outbound-attachment DTO. Generic to CC structured output —
//! consumed by Telegram-side delivery (`bot::telegram::attachments`)
//! and cron jobs (`bot::cron`, `bot::cron_delivery`).

use serde::Deserialize;

/// From CC JSON response.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct OutboundAttachment {
    #[serde(rename = "type")]
    pub kind: OutboundKind,
    pub path: String,
    pub filename: Option<String>,
    pub caption: Option<String>,
    #[serde(default)]
    pub media_group_id: Option<String>,
}

/// Attachment kinds CC can produce in output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundKind {
    Photo,
    Document,
    Video,
    Audio,
    Voice,
    VideoNote,
    Sticker,
    Animation,
}
```

(If the source has additional derives or attributes — `#[serde(...)]`, `#[non_exhaustive]`, etc. — copy them verbatim. Verify by diffing against the original.)

- [ ] **Step 2: Remove the original definitions in `telegram/attachments.rs`**

Open `crates/bot/src/telegram/attachments.rs`. Delete the two blocks you copied (`pub struct OutboundAttachment { ... }` and `pub enum OutboundKind { ... }`). Add an import at the top of the file (with the other `use` statements):

```rust
pub use crate::cc::attachments_dto::{OutboundAttachment, OutboundKind};
```

The `pub use` keeps `crate::telegram::attachments::OutboundAttachment` and `OutboundKind` resolvable from external callers — no need to update consumers in this task.

- [ ] **Step 3: Build to validate**

Run: `devenv shell -- cargo check -p right-bot`
Expected: errors about remaining missing `cc::*` modules (`worker_reply`, `markdown_utils`). The attachments-DTO move itself should compile.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/cc/attachments_dto.rs crates/bot/src/telegram/attachments.rs
git commit -m "refactor(bot): extract OutboundAttachment/OutboundKind to cc::attachments_dto"
```

---

## Task 6: Extract `html_escape`, `html_escape_into`, `strip_html_tags` into `cc::markdown_utils`

**Files:**
- Create: `crates/bot/src/cc/markdown_utils.rs`
- Modify: `crates/bot/src/telegram/markdown.rs`

- [ ] **Step 1: Create `cc/markdown_utils.rs`**

Open `crates/bot/src/telegram/markdown.rs`. Locate:
- `pub(crate) fn html_escape(s: &str) -> String { ... }` (≈line 265-269)
- `pub(crate) fn html_escape_into(s: &str, out: &mut String) { ... }` (≈line 271-298)
- `pub fn strip_html_tags(html: &str) -> String { ... }` (≈line 396-...)

Copy all three (with their doc comments) into a new file:

`crates/bot/src/cc/markdown_utils.rs`:

```rust
//! HTML-escape and tag-stripping helpers used by both Telegram-flavoured
//! markdown rendering and CC-side prompt assembly. Visibility lifted from
//! `pub(crate)` to `pub` because they cross sibling-module boundaries.

/// Escape `&`, `<`, `>` in a string for safe inclusion in HTML.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    html_escape_into(s, &mut out);
    out
}

/// In-place variant of [`html_escape`] that appends to a caller-supplied buffer.
pub fn html_escape_into(s: &str, out: &mut String) {
    // ... (copy verbatim from telegram/markdown.rs)
}

/// Strip all HTML tags from a string, returning the visible text.
pub fn strip_html_tags(html: &str) -> String {
    // ... (copy verbatim from telegram/markdown.rs)
}
```

(Replace the body comments with the actual function bodies copied from the source. The implementer should diff against the original to confirm parity.)

- [ ] **Step 2: Remove originals from `telegram/markdown.rs` and re-import**

Open `crates/bot/src/telegram/markdown.rs`. Delete the three function bodies. Add at the top of the file (with other `use` statements):

```rust
use crate::cc::markdown_utils::{html_escape, html_escape_into, strip_html_tags};
```

(`md_to_telegram_html` and `split_html_message` stay in this file and call `html_escape_into` / `html_escape` — those calls now resolve via the use-import.)

- [ ] **Step 3: Add `pub use` re-exports for backward-compatible callsites**

Inside the same `crates/bot/src/telegram/markdown.rs`, append:

```rust
// Re-export so existing `crate::telegram::markdown::{html_escape, strip_html_tags}`
// imports keep compiling. Removed at Stage E or F.
pub use crate::cc::markdown_utils::{html_escape, strip_html_tags};
```

This means consumers like `bot/src/telegram/worker.rs:175` (`use super::markdown::{html_escape, strip_html_tags};`) still work without edits.

- [ ] **Step 4: Build to validate**

Run: `devenv shell -- cargo check -p right-bot`
Expected: error remaining is only about missing `cc::worker_reply` module.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cc/markdown_utils.rs crates/bot/src/telegram/markdown.rs
git commit -m "refactor(bot): extract html_escape/strip_html_tags to cc::markdown_utils"
```

---

## Task 7: Extract `ReplyOutput` + `parse_reply_output` (with tests) into `cc::worker_reply`

**Files:**
- Create: `crates/bot/src/cc/worker_reply.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

This is the most surgical task. Care needed because `parse_reply_output` references `ReplyOutput`, `OutboundAttachment` (now in `cc::attachments_dto`), and `should_accept_bootstrap` (currently a free function in `worker.rs`).

- [ ] **Step 1: Create `cc/worker_reply.rs` with `ReplyOutput`, `parse_reply_output`, and supporting helpers**

Identify the boundaries in `crates/bot/src/telegram/worker.rs`:
- `pub struct ReplyOutput { ... }` block (≈line 148-156).
- `const BOOTSTRAP_REQUIRED_FILES: &[&str] = ...` (line ≈158).
- `fn should_accept_bootstrap(agent_dir: &Path) -> bool { ... }` (≈line 165-173).
- `pub fn parse_reply_output(raw_json: &str) -> Result<(ReplyOutput, Option<String>), String> { ... }` (line 314 + ~90 LoC body).
- The unit tests for `parse_reply_output_*` and `should_accept_bootstrap_*` (`crates/bot/src/telegram/worker.rs:2572-end` — find the actual end-of-tests; re-grep to make sure no stray `parse_reply_output_*` functions live elsewhere).

Copy all of the above into `crates/bot/src/cc/worker_reply.rs`.

Adjust imports at the top of the new file:
- `use std::path::Path;` (for `should_accept_bootstrap`'s signature).
- `use crate::cc::attachments_dto::OutboundAttachment;` (for the `attachments` field of `ReplyOutput`).
- The original `super::attachments::OutboundAttachment` in `ReplyOutput` becomes `crate::cc::attachments_dto::OutboundAttachment`.

If `should_accept_bootstrap` and `BOOTSTRAP_REQUIRED_FILES` are referenced **outside** `parse_reply_output` in `worker.rs`, **leave them in `worker.rs`** and import the bootstrap-completeness check via a `use` from `worker_reply` only inside `parse_reply_output`. Re-grep:

```bash
devenv shell -- rg -n 'should_accept_bootstrap|BOOTSTRAP_REQUIRED_FILES' crates/bot/src
```

If the only callers are `parse_reply_output`-internal, move them too. Otherwise, expose `should_accept_bootstrap` as `pub(crate)` from `worker_reply` and keep `worker.rs`'s callers calling `crate::cc::worker_reply::should_accept_bootstrap`.

- [ ] **Step 2: Update `crates/bot/src/telegram/worker.rs`**

Delete the moved blocks. Add imports near the top:

```rust
pub use crate::cc::worker_reply::{ReplyOutput, parse_reply_output};
```

(`pub use` keeps `super::worker::ReplyOutput` callsites compiling.)

If `should_accept_bootstrap` moved, also re-export it for `pub(crate)` callers:

```rust
pub(crate) use crate::cc::worker_reply::should_accept_bootstrap;
```

Or keep it inline in `worker.rs` if it has external callers and the re-export form is awkward.

- [ ] **Step 3: Bulk-replace remote consumers**

```bash
devenv shell -- rg -l 'crate::telegram::worker::parse_reply_output' crates/bot/src \
  | xargs sed -i.bak 's|crate::telegram::worker::parse_reply_output|crate::cc::worker_reply::parse_reply_output|g'
devenv shell -- find crates/bot/src -name '*.bak' -delete
```

- [ ] **Step 4: Validate**

Run: `devenv shell -- cargo build -p right-bot`
Expected: succeeds. All `cc::*` modules now exist.

- [ ] **Step 5: Run the moved tests**

Run: `devenv shell -- cargo test -p right-bot --lib cc::worker_reply`
Expected: all 17+ `parse_reply_output_*` tests pass.

- [ ] **Step 6: Run the full bot test suite**

Run: `devenv shell -- cargo test -p right-bot`
Expected: passes (Telegram-side worker tests, cron tests, reflection tests).

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/cc/worker_reply.rs crates/bot/src/telegram/worker.rs crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs crates/bot/src/reflection.rs
git commit -m "refactor(bot): extract parse_reply_output to cc::worker_reply"
```

---

## Task 8: Switch `crate::telegram::attachments::OutboundAttachment` consumers to `crate::cc::attachments_dto::OutboundAttachment`

**Files:**
- Modify: `crates/bot/src/cron.rs`
- Modify: any other file referencing `crate::telegram::attachments::Outbound*`

- [ ] **Step 1: Inventory**

```bash
devenv shell -- rg -n 'crate::telegram::attachments::Outbound' crates/bot/src
```

- [ ] **Step 2: Bulk replace**

```bash
devenv shell -- rg -l 'crate::telegram::attachments::OutboundAttachment' crates/bot/src \
  | xargs sed -i.bak 's|crate::telegram::attachments::OutboundAttachment|crate::cc::attachments_dto::OutboundAttachment|g'
devenv shell -- rg -l 'crate::telegram::attachments::OutboundKind' crates/bot/src \
  | xargs sed -i.bak 's|crate::telegram::attachments::OutboundKind|crate::cc::attachments_dto::OutboundKind|g'
devenv shell -- find crates/bot/src -name '*.bak' -delete
```

(The `pub use` re-export in Task 5 Step 2 means leaving the old paths is also valid — but pointing each callsite at the new location pays the rename forward and lets us delete the re-export at Stage E without surprises.)

- [ ] **Step 3: Validate**

Run: `devenv shell -- cargo build -p right-bot && devenv shell -- cargo test -p right-bot`
Expected: succeeds.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src
git commit -m "refactor(bot): switch consumers to cc::attachments_dto::OutboundAttachment"
```

---

## Task 9: Switch `crate::telegram::markdown::{html_escape,strip_html_tags}` callsites where appropriate

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`, `crates/bot/src/telegram/handler.rs`, others touching the helpers

- [ ] **Step 1: Inventory**

```bash
devenv shell -- rg -n 'crate::telegram::markdown::(html_escape|strip_html_tags)|use super::markdown::\{?(html_escape|strip_html_tags)' crates/bot/src
```

- [ ] **Step 2: For files that use the helpers as generic CC-side utilities (`worker`, `handler`, `cron_delivery`), point them at `cc::markdown_utils`**

The decision is per-file. Two reasonable patterns:
- Telegram-rendering files (`telegram::worker`, `telegram::handler`) that are about to ship to `right-telegram` in Stage E should keep the import via `crate::cc::markdown_utils::*` (the new home).
- The `pub use` re-export added in Task 6 Step 3 provides fallback compatibility for any callsite the implementer doesn't touch in this stage.

For each file in the inventory above, edit the import line(s) to:

```rust
use crate::cc::markdown_utils::{html_escape, strip_html_tags};
```

(or only the function the file actually uses).

- [ ] **Step 3: Validate**

Run: `devenv shell -- cargo build -p right-bot && devenv shell -- cargo test -p right-bot`
Expected: succeeds.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src
git commit -m "refactor(bot): point cc-shared markdown helpers at cc::markdown_utils"
```

---

## Task 10: Sanity check — telegram subtree contains only Telegram-specific code

**Files:** none (verification only)

- [ ] **Step 1: List remaining `bot::telegram::` modules**

```bash
ls crates/bot/src/telegram/
```

Expected modules (post-Stage-D): `allowlist_commands`, `attachments`, `bootstrap_photo`, `bot`, `dispatch`, `filter`, `handler`, `markdown`, `markdown_tests`, `memory_alerts`, `mention`, `mod`, `model_command`, `oauth_callback`, `session`, `shutdown_listener`, `webhook`, `worker`. Missing (now in `cc/`): `invocation`, `prompt`, `stream`. The `worker.rs` no longer hosts `ReplyOutput` / `parse_reply_output`. The `attachments.rs` no longer hosts `OutboundAttachment` / `OutboundKind`. The `markdown.rs` no longer hosts `html_escape` / `strip_html_tags`.

- [ ] **Step 2: Search for any remaining cross-references that should have moved**

```bash
devenv shell -- rg -n 'crate::telegram::(invocation|prompt|stream|worker::parse_reply_output)' crates/bot/src
```

Expected: zero results.

```bash
devenv shell -- rg -n 'crate::telegram::attachments::OutboundAttachment|crate::telegram::attachments::OutboundKind' crates/bot/src
```

Expected: zero results, OR only inside `crates/bot/src/telegram/attachments.rs` itself (the `pub use` re-export keeps that path resolvable for back-compat).

```bash
devenv shell -- rg -n 'crate::telegram::markdown::(html_escape|strip_html_tags)' crates/bot/src
```

Expected: zero results outside `crates/bot/src/telegram/markdown.rs` (which keeps the re-export).

- [ ] **Step 3: If any unexpected hits remain, fix them**

For each, edit the file in question and switch to `crate::cc::*`. Commit:

```bash
git add <fixed files>
git commit -m "fix(stage-d): address remaining stale telegram-namespace imports"
```

---

## Task 11: Whole-bot build, test, lint pass

**Files:** none

- [ ] **Step 1: Build**

Run: `devenv shell -- cargo build -p right-bot`
Expected: succeeds with zero warnings.

- [ ] **Step 2: Build (release)**

Run: `devenv shell -- cargo build -p right-bot --release`
Expected: succeeds.

- [ ] **Step 3: Tests**

Run: `devenv shell -- cargo test -p right-bot`
Expected: passes — including the moved `cc::worker_reply` tests.

- [ ] **Step 4: Clippy**

Run: `devenv shell -- cargo clippy -p right-bot --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 5: Workspace sanity**

Run: `devenv shell -- cargo build --workspace && devenv shell -- cargo test --workspace`
Expected: passes. (Stage D doesn't touch any other crate, so this only catches accidental disruption.)

- [ ] **Step 6: If anything fails, fix in-place and commit**

```bash
git add <fixed files>
git commit -m "fix(stage-d): resolve cleanups after bot::cc extraction"
```

---

## Task 12: Run `rust-dev:review-rust-code` agent

**Files:** none (review only)

- [ ] **Step 1: Dispatch**

> Review changes to `crates/bot/src/{cc,telegram,cron,cron_delivery,reflection}.rs` since `<sha-of-stage-d-start>`. Focus on:
> 1. Does any file under `bot::cc::*` accidentally import from `bot::telegram::*`? If yes, that breaks the Stage E extraction goal — flag every such case.
> 2. Are the `pub use` re-exports in `telegram::attachments::{OutboundAttachment,OutboundKind}` and `telegram::markdown::{html_escape,strip_html_tags}` minimal? Anything else that should be re-exported (or shouldn't)?
> 3. The `parse_reply_output` extraction — did all 17+ tests migrate correctly? Did `should_accept_bootstrap` end up where it makes sense (private to `cc::worker_reply` if only `parse_reply_output` calls it; otherwise visible from `cc::worker_reply` to outside callers).
> 4. The `html_escape_into` helper — visibility correctly lifted from `pub(crate)` to `pub` so `telegram::markdown` can call it across the module boundary.
> 5. Any leftover `crate::telegram::*` import in `bot::cron` / `bot::cron_delivery` / `bot::reflection` that should have switched to `crate::cc::*`.
>
> Don't fix; report. Output as TODO list with file:line references.

- [ ] **Step 2: Triage**

Bugs → followup file `docs/superpowers/plans/2026-05-06-stage-d-followups.md`, fixed one per commit. Nitpicks → defer. Misunderstandings → ignore.

- [ ] **Step 3: Confirm tests after fixes**

Run: `devenv shell -- cargo test -p right-bot`

- [ ] **Step 4: Commit fixes / followup file**

```bash
git add <files>
git commit -m "fix(stage-d): address review-rust-code findings"
```

---

## Task 13: Update `ARCHITECTURE.md`

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Add a `bot::cc` note**

Open `ARCHITECTURE.md`. Find the section that describes `right-bot` (likely the one describing the bot's module map, e.g. under `## Module Map` or in a `Crate contents` walkthrough). Add a short paragraph:

```markdown
The bot crate owns two sibling subtrees: `bot::cc::*` (CC subprocess plumbing —
`invocation`, `prompt`, `stream`, `worker_reply`, `attachments_dto`,
`markdown_utils`) and `bot::telegram::*` (Telegram-specific glue — `handler`,
`dispatch`, `filter`, `mention`, `oauth_callback`, `webhook`, `attachments::send_*`,
etc.). The `cc/` subtree is generic and Stage E will lift it into a separate
`right-cc` crate; the `telegram/` subtree depends on `cc/` for shared types
(`OutboundAttachment`, `html_escape`).
```

- [ ] **Step 2: Refresh `docs/architecture/modules.md` if it documents bot internals**

Cite-on-touch per CLAUDE.md.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md docs/architecture
git commit -m "docs(arch): document bot::cc / bot::telegram split"
```

---

## Task 14: Final verification

**Files:** none

- [ ] **Step 1: Re-run the full check suite**

```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo build --workspace --release
devenv shell -- cargo test --workspace
devenv shell -- cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all pass.

- [ ] **Step 2: Confirm the cycle invariant**

The whole point of Stage D is breaking circular dependencies between Telegram-glue and CC-plumbing. Verify by grep:

```bash
devenv shell -- rg -n 'use super::(invocation|prompt|stream|worker_reply|attachments_dto|markdown_utils)' crates/bot/src/cc
```

Expected: zero hits — `bot::cc::*` modules use absolute `crate::cc::*` paths between siblings, not `super::*`. (If any `super::*` reaches into `cc::*` siblings, that's fine; what would NOT be fine is `super::*` reaching out to `crate::telegram::*`.)

```bash
devenv shell -- rg -n 'crate::telegram::' crates/bot/src/cc
```

Expected: zero hits.

- [ ] **Step 3: Optional summary commit**

```bash
git commit --allow-empty -m "chore(stage-d): bot::cc pre-refactor complete"
```

- [ ] **Step 4: Open a PR (if working on a branch)**

Title: `Stage D: in-bot telegram→cc pre-refactor`. Body references the spec at `docs/superpowers/specs/2026-05-06-crate-split-design.md` and this plan, plus a one-line note that this stage produces no measurable build-time win on its own and exists to unblock Stage E.
