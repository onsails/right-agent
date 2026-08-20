# `/model` Telegram Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `/model` Telegram command that opens an inline-keyboard menu of curated Claude models (Default / Sonnet / Sonnet 1M / Haiku) and switches the agent's model on click — written to `agent.yaml`, hot-reloaded into memory without bot restart, applied to the next CC invocation.

**Architecture:** Single source of truth is `agent.yaml::model`. In-memory cache is `AgentSettings.model: Arc<ArcSwap<Option<String>>>`. A smart-diff in `config_watcher` classifies yaml changes: model-only → swap in-memory, no restart; anything else → existing graceful-restart path. Group chats gated by the trusted-users allowlist (same gate as `/allow`).

**Tech Stack:** Rust 2024, teloxide (Telegram), `arc-swap` (lock-free swap), `serde-saphyr` (YAML deserialize, already in workspace), `notify-debouncer-mini` (file watch, already in workspace). Spec at `docs/superpowers/specs/2026-05-06-model-command-design.md` (commit `bd4b2a07`).

**Pre-existing context:**
- `AgentConfig.model: Option<String>` already exists at `crates/right-agent/src/agent/types.rs:208`. No struct change is needed.
- `write_merged_rmw(path, FnOnce(Option<&str>) -> Result<String>)` exists at `crates/right-agent/src/codegen/contract.rs:87`. We will use it.
- `ClaudeInvocation.model: Option<String>` is already wired through `worker.rs:1154` and `worker.rs:1652` via `model: ctx.model.clone()`. We adjust those two call sites.
- `config_watcher::spawn_config_watcher(agent_yaml, token, config_changed)` lives at `crates/bot/src/config_watcher.rs:16`. We add a 4th parameter (the ArcSwap cell) and a diff path.
- Allowlist gate pattern (see `crates/bot/src/telegram/allowlist_commands.rs:90-99`): `allowlist.0.read().is_user_trusted(user_id)`. Reuse this exact pattern.

---

## Task 1: Add `arc-swap` dependency

**Files:**
- Modify: `crates/bot/Cargo.toml`

- [ ] **Step 1: Add the dependency**

Open `crates/bot/Cargo.toml` and add `arc-swap = "1.7"` to the `[dependencies]` table, alphabetically positioned (after `anyhow`/`async-trait`-like entries, before `axum`/`bytes`-style entries — pick the right spot in the existing alphabetical block).

- [ ] **Step 2: Verify it compiles**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo check -p right-bot`
Expected: succeeds (the dep is fetched but not yet used; that's fine).

- [ ] **Step 3: Commit**

```bash
git add crates/bot/Cargo.toml Cargo.lock
git commit -m "deps(bot): add arc-swap 1.7 for model hot-swap"
```

---

## Task 2: Add `write_agent_yaml_model` helper in `right-agent`

This helper is the only sanctioned way to update `agent.yaml::model`. It uses surgical line-oriented editing (read lines → find/replace/append `^model:` → write) to preserve unknown fields and yaml comments. We pick this approach over full serde round-trip because `init.rs` already uses raw-string yaml editing, and serializing `AgentConfig` would lose comments + reorder fields.

**Files:**
- Modify: `crates/right-agent/src/agent/types.rs` (add helper function near `AgentConfig` impl, ~line 320)
- Test: `crates/right-agent/src/agent/types.rs` (extend existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module starting around `crates/right-agent/src/agent/types.rs:702` (right before `mod stt_config_tests`):

```rust
    use std::io::Write;

    #[test]
    fn write_agent_yaml_model_appends_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(&path, "restart: never\nmax_restarts: 5\n").unwrap();

        super::write_agent_yaml_model(&path, Some("claude-sonnet-4-6")).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("restart: never"), "preserve existing fields:\n{result}");
        assert!(result.contains("max_restarts: 5"), "preserve existing fields:\n{result}");
        assert!(
            result.contains("model: \"claude-sonnet-4-6\""),
            "append model when absent:\n{result}"
        );
        // Round-trip: parse back into AgentConfig
        let parsed: AgentConfig = serde_saphyr::from_str(&result).unwrap();
        assert_eq!(parsed.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(parsed.max_restarts, 5);
    }

    #[test]
    fn write_agent_yaml_model_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(
            &path,
            "restart: never\nmodel: sonnet\nmax_restarts: 5\n",
        )
        .unwrap();

        super::write_agent_yaml_model(&path, Some("claude-haiku-4-5")).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(
            !result.contains("model: sonnet"),
            "old value must be gone:\n{result}"
        );
        assert!(
            result.contains("model: \"claude-haiku-4-5\""),
            "new value must be present:\n{result}"
        );
        // Field order roughly preserved — restart still before model in original
        let restart_pos = result.find("restart:").unwrap();
        let model_pos = result.find("model:").unwrap();
        assert!(restart_pos < model_pos, "field order preserved:\n{result}");
    }

    #[test]
    fn write_agent_yaml_model_removes_when_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(
            &path,
            "restart: never\nmodel: \"claude-sonnet-4-6\"\nmax_restarts: 5\n",
        )
        .unwrap();

        super::write_agent_yaml_model(&path, None).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(!result.contains("model:"), "model line removed:\n{result}");
        assert!(result.contains("restart: never"));
        assert!(result.contains("max_restarts: 5"));
        // Round-trip: model is None
        let parsed: AgentConfig = serde_saphyr::from_str(&result).unwrap();
        assert!(parsed.model.is_none());
    }

    #[test]
    fn write_agent_yaml_model_none_when_already_absent_is_noop_safe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        let original = "restart: never\nmax_restarts: 5\n";
        std::fs::write(&path, original).unwrap();

        super::write_agent_yaml_model(&path, None).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("restart: never"));
        assert!(!result.contains("model:"));
    }

    #[test]
    fn write_agent_yaml_model_preserves_comments_and_blank_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(
            &path,
            "# Agent config\nrestart: never\n\n# Restart policy bump\nmax_restarts: 5\n",
        )
        .unwrap();

        super::write_agent_yaml_model(&path, Some("claude-haiku-4-5")).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("# Agent config"), "leading comment preserved:\n{result}");
        assert!(
            result.contains("# Restart policy bump"),
            "interior comment preserved:\n{result}"
        );
    }

    #[test]
    fn write_agent_yaml_model_value_with_brackets() {
        // [1m] suffix must serialize through unscathed.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(&path, "restart: never\n").unwrap();

        super::write_agent_yaml_model(&path, Some("claude-sonnet-4-6[1m]")).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(
            result.contains("model: \"claude-sonnet-4-6[1m]\""),
            "bracketed value double-quoted:\n{result}"
        );
        let parsed: AgentConfig = serde_saphyr::from_str(&result).unwrap();
        assert_eq!(parsed.model.as_deref(), Some("claude-sonnet-4-6[1m]"));
    }
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-agent --lib write_agent_yaml_model`
Expected: FAIL with "cannot find function `write_agent_yaml_model` in module `super`" (or similar).

- [ ] **Step 3: Implement the helper**

Append immediately before the `#[cfg(test)] mod tests {` block in `crates/right-agent/src/agent/types.rs` (around line 451):

```rust
/// Write `agent.yaml::model` via line-oriented MergedRMW.
///
/// `Some(value)` replaces or appends a `model: "<value>"` line.
/// `None` removes the existing `model:` line, leaving the key absent
/// (CC will use its default model).
///
/// Atomic via tempfile + rename. Preserves all unknown fields, comments,
/// and blank lines. The value is always double-quoted to handle YAML
/// special characters (e.g. the `[` in `claude-sonnet-4-6[1m]`).
pub fn write_agent_yaml_model(
    path: &std::path::Path,
    new_value: Option<&str>,
) -> miette::Result<()> {
    crate::codegen::contract::write_merged_rmw(path, |existing| {
        let original = existing.unwrap_or("");

        // Walk lines, replacing or removing the first `^model:` line.
        let mut found = false;
        let mut out = String::with_capacity(original.len() + 64);
        for line in original.split_inclusive('\n') {
            // Match a line whose first non-whitespace token is `model:`.
            // Must match top-level only — indentation = nested key (e.g. memory.model).
            let is_top_level_model = line
                .strip_prefix("model:")
                .map(|rest| rest.starts_with(' ') || rest.starts_with('\t') || rest.is_empty() || rest.starts_with('\n') || rest.starts_with('\r'))
                .unwrap_or(false);
            if is_top_level_model {
                found = true;
                if let Some(v) = new_value {
                    let needs_newline = line.ends_with('\n');
                    out.push_str(&format!(
                        "model: \"{}\"{}",
                        v.replace('\\', "\\\\").replace('"', "\\\""),
                        if needs_newline { "\n" } else { "" }
                    ));
                }
                // else: skip this line entirely (removal)
            } else {
                out.push_str(line);
            }
        }

        // Append if the key was absent and we have a new value.
        if !found && new_value.is_some() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!(
                "model: \"{}\"\n",
                new_value
                    .unwrap()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\""),
            ));
        }

        Ok(out)
    })
}
```

- [ ] **Step 4: Run tests, verify they pass**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-agent --lib write_agent_yaml_model`
Expected: all 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/agent/types.rs
git commit -m "feat(right-agent): write_agent_yaml_model helper

Line-oriented MergedRMW for agent.yaml::model. Preserves unknown
fields, comments, and blank lines. Always double-quotes the value to
handle YAML special chars (e.g. claude-sonnet-4-6[1m])."
```

---

## Task 3: Refactor `AgentSettings.model` to `Arc<ArcSwap<Option<String>>>`

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs:86-102` (struct field type)
- Modify: `crates/bot/src/telegram/dispatch.rs:145-154` (construction site)

- [ ] **Step 1: Update the struct field**

In `crates/bot/src/telegram/handler.rs:86-102`, change the `model` field type. Replace:

```rust
    /// Claude model override (passed as --model). None = inherit CLI default.
    pub model: Option<String>,
```

with:

```rust
    /// Claude model override (passed as --model). None = inherit CLI default.
    /// Lock-free swap cell — `/model` callback and `config_watcher` (model-only diff)
    /// store new values; CC invocations load on every call.
    pub model: std::sync::Arc<arc_swap::ArcSwap<Option<String>>>,
```

- [ ] **Step 2: Update the construction site**

In `crates/bot/src/telegram/dispatch.rs:145-154`, change the `AgentSettings` construction. Replace:

```rust
    let settings_arc: Arc<AgentSettings> = Arc::new(AgentSettings {
        show_thinking,
        model,
        resolved_sandbox,
```

with:

```rust
    let settings_arc: Arc<AgentSettings> = Arc::new(AgentSettings {
        show_thinking,
        model: Arc::new(arc_swap::ArcSwap::from_pointee(model)),
        resolved_sandbox,
```

- [ ] **Step 3: Verify it compiles (will fail until Task 4)**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo check -p right-bot`
Expected: FAIL with type errors at `worker.rs:1154` and `worker.rs:1652` (`model: ctx.model.clone()` type mismatch). That's expected — Task 4 fixes both.

- [ ] **Step 4: Don't commit yet** — wait until Task 4 closes the loop.

---

## Task 4: Refactor `WorkerContext.model` and adjust call sites

`WorkerContext.model` shares the same `Arc<ArcSwap<...>>` as `AgentSettings.model` so a swap is visible to all in-flight workers (no per-worker snapshot). Both `ClaudeInvocation` build sites do `.load()` immediately before consuming the value.

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (struct field at ~line 122, two call sites at ~1154 and ~1652)
- Modify: `crates/bot/src/telegram/handler.rs` (worker spawn site — search for `model: settings.model.clone()` or similar)

- [ ] **Step 1: Update WorkerContext struct field**

In `crates/bot/src/telegram/worker.rs:121-122`, change:

```rust
    /// Claude model override (passed as --model). None = inherit CLI default.
    pub model: Option<String>,
```

to:

```rust
    /// Claude model override (passed as --model). None = inherit CLI default.
    /// Shared swap cell — load on each CC invocation so /model takes effect immediately.
    pub model: std::sync::Arc<arc_swap::ArcSwap<Option<String>>>,
```

- [ ] **Step 2: Update call site at worker.rs:1154**

Find the `ClaudeInvocation { ... model: ctx.model.clone(), ... }` block around line 1154. Replace `model: ctx.model.clone(),` with:

```rust
                        model: (**ctx.model.load()).clone(),
```

(`load()` returns `Guard<Arc<Option<String>>>`; `**` derefs Guard then Arc; `.clone()` clones the inner `Option<String>`.)

- [ ] **Step 3: Update call site at worker.rs:1652**

Find the same pattern around line 1652. Same replacement:

```rust
        model: (**ctx.model.load()).clone(),
```

- [ ] **Step 4: Update WorkerContext construction in handler.rs**

Search for the spawn site that constructs a `WorkerContext`. Run `rg -n "WorkerContext \{" crates/bot/src/telegram/`. There should be one site in `handler.rs` (~line 364 per prior exploration). Read the surrounding context to find the `model:` line.

The line is likely already `model: settings.model.clone(),`. The literal text stays the same — but the *meaning* changes: pre-refactor it cloned an `Option<String>`; post-refactor it clones an `Arc<ArcSwap<...>>` (cheap pointer bump), so `WorkerContext.model` and `AgentSettings.model` share the same swap cell.

If the existing line is anything else (e.g. `model: settings.model.as_ref().map(String::clone),`), replace it with:

```rust
        model: settings.model.clone(),
```

The invariant: both fields must hold a clone of the same `Arc<ArcSwap<Option<String>>>`. Any swap from the watcher or from `/model` is then visible to all workers and all CC invocations.

- [ ] **Step 5: Run cargo check**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo check -p right-bot`
Expected: PASS.

- [ ] **Step 6: Run all bot tests**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-bot --lib`
Expected: PASS (no behavior change yet — model field is still set the same way at startup).

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs crates/bot/src/telegram/dispatch.rs
git commit -m "refactor(bot): AgentSettings/WorkerContext model use ArcSwap

Both fields now share an Arc<ArcSwap<Option<String>>> so /model can
swap atomically; CC invocations load on each call."
```

---

## Task 5: Create `model_command.rs` skeleton with `MODEL_CHOICES`

This task lays down the static data and helpers used by `handle_model` and `handle_model_callback` in subsequent tasks.

**Files:**
- Create: `crates/bot/src/telegram/model_command.rs`
- Modify: `crates/bot/src/telegram/mod.rs` (or wherever sibling modules are declared)

- [ ] **Step 1: Find where modules are declared**

Run: `rg -n "pub(crate)? mod (allowlist_commands|handler|worker|dispatch)" crates/bot/src/telegram/mod.rs crates/bot/src/lib.rs 2>&1 | head -10`

Note the file and pattern. We'll add `model_command` next to its siblings.

- [ ] **Step 2: Create the new module file with skeleton + tests**

Create `crates/bot/src/telegram/model_command.rs` with:

```rust
//! `/model` command — inline-keyboard menu for switching the agent's Claude model.
//!
//! UI: 4 curated options (Default / Sonnet / Sonnet 1M / Haiku) matching the
//! Claude Code CLI `/model` picker.
//!
//! Persistence: writes `agent.yaml::model` via `right_agent::agent::types::write_agent_yaml_model`.
//! In-memory: stores into `AgentSettings.model: Arc<ArcSwap<Option<String>>>`.
//! Group chats are gated by the trusted-users allowlist (same gate as `/allow`).

/// One row in the curated model menu.
///
/// `model_id == None` represents the "Default" option — no `--model`
/// flag, CC chooses its own default. All other rows pin a specific
/// model via the exact model-ID string CC accepts on the command line.
#[derive(Debug, Clone, Copy)]
pub struct ModelChoice {
    /// Short alias used in callback_data (≤ 16 bytes; stays under
    /// Telegram's 64-byte callback_data limit even with the `model:` prefix).
    pub alias: &'static str,
    /// Button label (also row label in the body text).
    pub label: &'static str,
    /// Value written to `agent.yaml::model`. `None` = field absent.
    pub model_id: Option<&'static str>,
    /// One-line description shown in the menu body.
    pub description: &'static str,
}

/// Curated model menu — order is the order shown in the keyboard.
///
/// **Local registry, not a project-wide one.** Per the project memory
/// `feedback_no_central_registries`, this stays here rather than in a
/// shared types module.
pub const MODEL_CHOICES: &[ModelChoice] = &[
    ModelChoice {
        alias: "default",
        label: "Default",
        model_id: None,
        description: "Opus 4.7 (1M context) · Most capable",
    },
    ModelChoice {
        alias: "sonnet",
        label: "Sonnet",
        model_id: Some("claude-sonnet-4-6"),
        description: "Sonnet 4.6 · Best for everyday tasks",
    },
    ModelChoice {
        alias: "sonnet1m",
        label: "Sonnet 1M",
        model_id: Some("claude-sonnet-4-6[1m]"),
        description: "Sonnet 4.6 (1M context) · Extra usage billing",
    },
    ModelChoice {
        alias: "haiku",
        label: "Haiku",
        model_id: Some("claude-haiku-4-5"),
        description: "Haiku 4.5 · Fastest",
    },
];

/// Resolve a callback alias to a `ModelChoice`.
pub fn lookup(alias: &str) -> Option<&'static ModelChoice> {
    MODEL_CHOICES.iter().find(|c| c.alias == alias)
}

/// Find the choice that matches the given current `model_id` (from `agent.yaml`).
/// Returns `None` if the value is non-canonical (a "Custom" model).
pub fn active_choice(current: Option<&str>) -> Option<&'static ModelChoice> {
    MODEL_CHOICES
        .iter()
        .find(|c| c.model_id == current)
}

/// Render the menu body text. Includes a "Current: ... (custom)" prefix line
/// when the active model is non-canonical.
pub fn render_menu_body(current: Option<&str>) -> String {
    let active = active_choice(current);
    let mut out = String::from("🤖 Choose Claude model\n\n");
    if active.is_none() && current.is_some() {
        out.push_str(&format!("Current: {} (custom)\n\n", current.unwrap()));
    }
    for choice in MODEL_CHOICES {
        let mark = if active.map(|a| a.alias) == Some(choice.alias) {
            "✓ "
        } else {
            "   "
        };
        out.push_str(&format!(
            "{}{} — {}\n",
            mark, choice.label, choice.description
        ));
    }
    out
}

/// Render the inline keyboard — 2 columns × 2 rows, with `✓` prefix on the active button.
pub fn render_keyboard(current: Option<&str>) -> teloxide::types::InlineKeyboardMarkup {
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
    let active = active_choice(current);
    let button = |c: &ModelChoice| {
        let label = if active.map(|a| a.alias) == Some(c.alias) {
            format!("✓ {}", c.label)
        } else {
            c.label.to_string()
        };
        InlineKeyboardButton::callback(label, format!("model:{}", c.alias))
    };
    InlineKeyboardMarkup::new(vec![
        vec![button(&MODEL_CHOICES[0]), button(&MODEL_CHOICES[1])],
        vec![button(&MODEL_CHOICES[2]), button(&MODEL_CHOICES[3])],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in MODEL_CHOICES {
            assert!(seen.insert(c.alias), "duplicate alias: {}", c.alias);
        }
    }

    #[test]
    fn aliases_short_enough_for_callback_data() {
        // "model:" prefix = 6 bytes; Telegram limit = 64.
        for c in MODEL_CHOICES {
            assert!(
                c.alias.len() <= 32,
                "alias {} too long ({} bytes)",
                c.alias,
                c.alias.len()
            );
        }
    }

    #[test]
    fn lookup_known_alias() {
        let c = lookup("sonnet").unwrap();
        assert_eq!(c.model_id, Some("claude-sonnet-4-6"));
    }

    #[test]
    fn lookup_unknown_alias_returns_none() {
        assert!(lookup("nonsense").is_none());
    }

    #[test]
    fn active_choice_default_for_none() {
        let c = active_choice(None).unwrap();
        assert_eq!(c.alias, "default");
    }

    #[test]
    fn active_choice_canonical_model() {
        let c = active_choice(Some("claude-haiku-4-5")).unwrap();
        assert_eq!(c.alias, "haiku");
    }

    #[test]
    fn active_choice_one_m_suffix() {
        let c = active_choice(Some("claude-sonnet-4-6[1m]")).unwrap();
        assert_eq!(c.alias, "sonnet1m");
    }

    #[test]
    fn active_choice_custom_model_returns_none() {
        assert!(active_choice(Some("claude-opus-4-old")).is_none());
    }

    #[test]
    fn menu_body_shows_checkmark_on_active() {
        let body = render_menu_body(Some("claude-sonnet-4-6"));
        assert!(body.contains("✓ Sonnet"), "expected checkmark on Sonnet:\n{body}");
        assert!(!body.contains("✓ Default"), "no checkmark on Default:\n{body}");
    }

    #[test]
    fn menu_body_shows_default_when_none() {
        let body = render_menu_body(None);
        assert!(body.contains("✓ Default"), "expected checkmark on Default:\n{body}");
    }

    #[test]
    fn menu_body_shows_custom_prefix_for_non_canonical() {
        let body = render_menu_body(Some("claude-opus-4-old"));
        assert!(
            body.contains("Current: claude-opus-4-old (custom)"),
            "custom prefix:\n{body}"
        );
        assert!(
            !body.contains("✓"),
            "no checkmark anywhere when custom:\n{body}"
        );
    }
}

```

- [ ] **Step 3: Declare the module**

Find the module declaration block in `crates/bot/src/telegram/mod.rs` (or wherever the sibling modules are declared — check based on Step 1's output). Add:

```rust
pub(crate) mod model_command;
```

next to the existing `pub(crate) mod handler;` / `pub(crate) mod allowlist_commands;` lines.

- [ ] **Step 4: Run tests**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-bot --lib model_command`
Expected: 10 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/model_command.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): MODEL_CHOICES + menu rendering for /model

Static curated menu matching CC's /model picker (Default / Sonnet /
Sonnet 1M / Haiku). Renders body text and inline keyboard with ✓ on
the active choice. Custom (non-canonical) values shown without ✓."
```

---

## Task 6: Implement `handle_model` (open menu)

The handler that fires when a user types `/model`. Sends a message with the menu body and inline keyboard. In groups, the trusted-users gate runs first (silent ignore on fail, matching the `/allow` pattern at `allowlist_commands.rs:118-121`).

**Files:**
- Modify: `crates/bot/src/telegram/model_command.rs`

- [ ] **Step 1: Add the handler at the bottom of `model_command.rs` (above `#[cfg(test)]`)**

```rust
use teloxide::RequestError;
use teloxide::prelude::*;
use teloxide::types::Message;

use right_agent::agent::allowlist::AllowlistHandle;

use super::BotType;
use super::handler::{AgentSettings, is_private_chat};

/// Open the `/model` menu. Allowlist-gated in groups.
pub async fn handle_model(
    bot: BotType,
    msg: Message,
    settings: Arc<AgentSettings>,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    // Group gate: trusted users only.
    if !is_private_chat(&msg.chat.kind) && !sender_is_trusted(&msg, &allowlist) {
        tracing::debug!(
            chat_id = msg.chat.id.0,
            user_id = msg.from.as_ref().map(|u| u.id.0),
            "/model ignored: non-trusted sender in group"
        );
        return Ok(());
    }

    let current = settings.model.load();
    let current_str: Option<&str> = (*current).as_deref();
    let body = render_menu_body(current_str);
    let keyboard = render_keyboard(current_str);

    let mut send = bot
        .send_message(msg.chat.id, body)
        .reply_markup(keyboard);
    if let Some(thread_id) = msg.thread_id {
        send = send.message_thread_id(thread_id);
    }
    send.await?;
    Ok(())
}

fn sender_is_trusted(msg: &Message, allowlist: &AllowlistHandle) -> bool {
    let Some(sender) = msg.from.as_ref() else {
        return false;
    };
    allowlist
        .0
        .read()
        .expect("allowlist lock poisoned")
        .is_user_trusted(sender.id.0 as i64)
}
```

Remove the `_unused_arc_marker` placeholder from Task 5.

- [ ] **Step 2: Add a unit test for the rendering output**

Append to the `tests` mod in `model_command.rs`:

```rust
    #[test]
    fn render_keyboard_has_4_buttons_in_2_rows() {
        let kb = render_keyboard(None);
        assert_eq!(kb.inline_keyboard.len(), 2);
        assert_eq!(kb.inline_keyboard[0].len(), 2);
        assert_eq!(kb.inline_keyboard[1].len(), 2);
    }

    #[test]
    fn render_keyboard_callback_data_format() {
        let kb = render_keyboard(None);
        let data: Vec<String> = kb
            .inline_keyboard
            .iter()
            .flatten()
            .filter_map(|b| match &b.kind {
                teloxide::types::InlineKeyboardButtonKind::CallbackData(d) => Some(d.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            data,
            vec!["model:default", "model:sonnet", "model:sonnet1m", "model:haiku"]
        );
    }
```

- [ ] **Step 3: Run tests**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-bot --lib model_command`
Expected: 12 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/model_command.rs
git commit -m "feat(bot): handle_model opens the /model menu

Allowlist-gated in groups (silent ignore for non-trusted senders,
matching the /allow pattern). DM has no gate."
```

---

## Task 7: Implement `handle_model_callback` (button click)

Handles the inline-button click. Re-checks allowlist (callback can come from any user who sees the keyboard, not just the original `/model` invoker). Writes to `agent.yaml`, swaps in-memory, edits the message to refresh the keyboard, sends a toast.

**Files:**
- Modify: `crates/bot/src/telegram/model_command.rs`

- [ ] **Step 1: Add the callback handler**

Append above the `#[cfg(test)]` block:

```rust
use std::path::Path;
use teloxide::types::CallbackQuery;

use super::handler::AgentDir;

/// Handle a click on a `/model` keyboard button.
pub async fn handle_model_callback(
    bot: BotType,
    q: CallbackQuery,
    settings: Arc<AgentSettings>,
    agent_dir: Arc<AgentDir>,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    let Some(data) = q.data.as_deref() else {
        // No data — nothing to do. Ack so Telegram clears the loading spinner.
        bot.answer_callback_query(&q.id).await?;
        return Ok(());
    };
    let Some(alias) = data.strip_prefix("model:") else {
        bot.answer_callback_query(&q.id).await?;
        return Ok(());
    };

    let Some(choice) = lookup(alias) else {
        tracing::warn!(callback_data = data, "unknown /model alias");
        bot.answer_callback_query(&q.id)
            .text("Unknown option")
            .await?;
        return Ok(());
    };

    // Group gate: re-check on the click, not just on /model.
    let in_group = q
        .message
        .as_ref()
        .map(|m| !is_private_chat(&m.chat().kind))
        .unwrap_or(false);
    if in_group {
        let user_id = q.from.id.0 as i64;
        let trusted = allowlist
            .0
            .read()
            .expect("allowlist lock poisoned")
            .is_user_trusted(user_id);
        if !trusted {
            bot.answer_callback_query(&q.id).text("Not allowed").await?;
            return Ok(());
        }
    }

    let agent_yaml_path: std::path::PathBuf = agent_dir.0.join("agent.yaml");
    let old_value = (*settings.model.load()).clone();

    // ① Persist to disk first. If this fails, in-memory stays untouched.
    if let Err(e) = persist_model(&agent_yaml_path, choice.model_id) {
        tracing::error!(error = %format!("{e:#}"), "/model: failed to write agent.yaml");
        bot.answer_callback_query(&q.id)
            .text("Failed to save model — see bot logs")
            .await?;
        return Ok(());
    }

    // ② Hot-swap in-memory.
    settings
        .model
        .store(Arc::new(choice.model_id.map(str::to_owned)));

    let user_id = q.from.id.0 as i64;
    let chat_id = q.message.as_ref().map(|m| m.chat().id.0).unwrap_or(0);
    tracing::info!(
        from = ?old_value.as_deref().unwrap_or("default"),
        to = ?choice.model_id.unwrap_or("default"),
        chat_id,
        user_id,
        "model switched via /model"
    );

    // ③ Refresh the menu UI (best-effort — failure logs but does not abort).
    if let Some(message) = q.message.as_ref() {
        let new_body = render_menu_body(choice.model_id);
        let new_kb = render_keyboard(choice.model_id);
        if let Err(e) = bot
            .edit_message_text(message.chat().id, message.id(), new_body)
            .reply_markup(new_kb)
            .await
        {
            tracing::warn!(error = %e, "failed to edit /model menu after switch");
        }
    }

    // ④ Toast confirming the switch.
    bot.answer_callback_query(&q.id)
        .text(format!("Switched to {}", choice.label))
        .await?;
    Ok(())
}

fn persist_model(agent_yaml: &Path, model_id: Option<&str>) -> miette::Result<()> {
    right_agent::agent::types::write_agent_yaml_model(agent_yaml, model_id)
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo check -p right-bot`
Expected: PASS.

- [ ] **Step 3: Run existing tests**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-bot --lib model_command`
Expected: 12 tests PASS (no new tests yet — handler is hard to unit-test without a mock bot; integration covered in Task 11).

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/model_command.rs
git commit -m "feat(bot): handle_model_callback persists and hot-swaps

On click: writes agent.yaml::model, stores into AgentSettings.model
ArcSwap, edits the menu (best-effort), sends a toast. Re-checks
allowlist in groups."
```

---

## Task 8: Wire `BotCommand::Model` into the dispatcher

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs:36-68` (enum)
- Modify: `crates/bot/src/telegram/dispatch.rs:393-419` (command branch)
- Modify: `crates/bot/src/telegram/dispatch.rs:453-460` (callback branch)
- Modify: `crates/bot/src/telegram/dispatch.rs:26-31` (use clause)

- [ ] **Step 1: Add the enum variant**

Insert after `BotCommand::Doctor` in the enum at `dispatch.rs:36-68`:

```rust
    #[command(description = "Switch Claude model (menu)")]
    Model,
```

- [ ] **Step 2: Import the new module's handlers**

Replace lines 26-31 (the `use super::handler::{...}` block) by adding `handle_model` and `handle_model_callback` from `model_command`. Add a fresh `use` line below the existing handler import:

```rust
use super::model_command::{handle_model, handle_model_callback};
```

- [ ] **Step 3: Add the command-branch dispatch**

In the command-handler tree at `dispatch.rs:393-419`, add a branch for `Model`. Insert (after the `Doctor` branch is fine):

```rust
        .branch(dptree::case![BotCommand::Model].endpoint(handle_model))
```

- [ ] **Step 4: Add the callback-branch dispatch**

In `dispatch.rs:453-460`, the current callback handler routes `bg:` to `handle_bg_callback` and falls through to `handle_stop_callback`. Insert a `model:` branch BEFORE the `bg:` branch (order matters — first match wins on `dptree::filter` chains):

```rust
    let callback_handler = Update::filter_callback_query()
        .branch(
            dptree::filter(|q: CallbackQuery| {
                q.data.as_deref().is_some_and(|d| d.starts_with("model:"))
            })
            .endpoint(handle_model_callback),
        )
        .branch(
            dptree::filter(|q: CallbackQuery| {
                q.data.as_deref().is_some_and(|d| d.starts_with("bg:"))
            })
            .endpoint(handle_bg_callback),
        )
        .endpoint(handle_stop_callback);
```

- [ ] **Step 5: Verify it compiles + smoke test**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-bot --lib dispatcher_builds_without_panic`
Expected: PASS. (This is the dptree DI sanity smoke test mentioned at `dispatch.rs:507`. Any unsatisfied DI dependency would panic at build time.)

If it fails with `type X not provided`, the missing dep is one of `AllowlistHandle` (already wired), `Arc<AgentSettings>` (already wired as `settings_arc`), or `Arc<AgentDir>` (already wired as `agent_dir_arc`). Confirm the existing `.dependencies(dptree::deps![...])` block at `dispatch.rs:467-480` already includes these — it does.

- [ ] **Step 6: Run all bot tests**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-bot --lib`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/dispatch.rs
git commit -m "feat(bot): wire /model command + callback into dispatcher

Adds BotCommand::Model and routes callback_query data starting with
'model:' to handle_model_callback (before bg:/stop:)."
```

---

## Task 9: Smart-diff in `config_watcher` — hot-reload model-only changes

The watcher currently restarts the bot on any `agent.yaml` change. We add a diff step: parse old + new yaml into `AgentConfig`; if everything except `model` is equal, store the new model into the ArcSwap cell and skip the restart. Anything else: existing restart path.

**Files:**
- Modify: `crates/bot/src/config_watcher.rs`
- Test: same file, `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing tests**

Append to `crates/bot/src/config_watcher.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use right_agent::agent::types::AgentConfig;

    fn classify(old: &str, new: &str) -> ChangeKind {
        diff_classify(old, new)
    }

    #[test]
    fn diff_model_only_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\nmodel: \"claude-sonnet-4-6\"\n";
        let new = "restart: never\nmax_restarts: 5\nmodel: \"claude-haiku-4-5\"\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_model } => {
                assert_eq!(new_model.as_deref(), Some("claude-haiku-4-5"));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn diff_model_added_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\n";
        let new = "restart: never\nmax_restarts: 5\nmodel: \"claude-haiku-4-5\"\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_model } => {
                assert_eq!(new_model.as_deref(), Some("claude-haiku-4-5"));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn diff_model_removed_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\nmodel: \"claude-haiku-4-5\"\n";
        let new = "restart: never\nmax_restarts: 5\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_model } => {
                assert!(new_model.is_none());
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn diff_other_field_changed_is_restart_required() {
        let old = "restart: never\nmax_restarts: 5\nmodel: \"claude-sonnet-4-6\"\n";
        let new = "restart: always\nmax_restarts: 5\nmodel: \"claude-sonnet-4-6\"\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[test]
    fn diff_model_and_other_field_is_restart_required() {
        let old = "restart: never\nmodel: \"claude-sonnet-4-6\"\n";
        let new = "restart: always\nmodel: \"claude-haiku-4-5\"\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[test]
    fn diff_parse_failure_is_restart_required() {
        // Malformed yaml on the new side — fail-safe to restart.
        let old = "restart: never\n";
        let new = "{ this is not yaml";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[test]
    fn diff_unchanged_yaml_is_hot_reloadable_with_same_model() {
        // Spurious watcher fire (e.g. touch with no content change) — treat
        // as hot-reloadable; storing the same value is idempotent.
        let yaml = "restart: never\nmodel: \"claude-haiku-4-5\"\n";
        match classify(yaml, yaml) {
            ChangeKind::HotReloadable { new_model } => {
                assert_eq!(new_model.as_deref(), Some("claude-haiku-4-5"));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn agent_config_partial_eq_smoke_test() {
        // Sanity: AgentConfig must derive PartialEq for the diff to compile.
        let a: AgentConfig = serde_saphyr::from_str("restart: never\n").unwrap();
        let b: AgentConfig = serde_saphyr::from_str("restart: never\n").unwrap();
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-bot --lib config_watcher::tests`
Expected: FAIL — `cannot find type ChangeKind` and `cannot find function diff_classify`.

- [ ] **Step 3: Add `ChangeKind`, `diff_classify`, and update the watcher**

Replace the entire content of `crates/bot/src/config_watcher.rs` with:

```rust
//! Watch agent.yaml for changes. Model-only changes are hot-reloaded
//! into the in-memory ArcSwap cell; any other change triggers graceful
//! restart.
//!
//! Uses `notify` with debouncing (2s) to avoid reacting to partial writes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arc_swap::ArcSwap;
use right_agent::agent::types::AgentConfig;
use tokio_util::sync::CancellationToken;

/// Classification of a single agent.yaml change event.
#[derive(Debug)]
pub(crate) enum ChangeKind {
    /// Only `model` changed — apply in-memory and continue running.
    HotReloadable { new_model: Option<String> },
    /// Anything else — graceful restart.
    RestartRequired,
}

/// Decide whether a change can be hot-reloaded or requires a restart.
///
/// Compares old + new yaml as parsed `AgentConfig` values with `model`
/// nulled out on both sides. If the rest is equal, hot-reload; else
/// restart. Parse failure on either side fails-safe to restart.
pub(crate) fn diff_classify(old_yaml: &str, new_yaml: &str) -> ChangeKind {
    let old: AgentConfig = match serde_saphyr::from_str(old_yaml) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                error = %format!("{e:#}"),
                "config_watcher: failed to parse old agent.yaml — restart required"
            );
            return ChangeKind::RestartRequired;
        }
    };
    let new: AgentConfig = match serde_saphyr::from_str(new_yaml) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                error = %format!("{e:#}"),
                "config_watcher: failed to parse new agent.yaml — restart required"
            );
            return ChangeKind::RestartRequired;
        }
    };
    let mut old_no_model = old.clone();
    let mut new_no_model = new.clone();
    old_no_model.model = None;
    new_no_model.model = None;
    if old_no_model == new_no_model {
        ChangeKind::HotReloadable { new_model: new.model }
    } else {
        ChangeKind::RestartRequired
    }
}

/// Spawn a blocking thread that watches `agent.yaml` for modifications.
///
/// On change:
/// - `HotReloadable` → store new model into `model_swap`, log info, do not cancel.
/// - `RestartRequired` → set `config_changed`, cancel `token` (existing path).
pub fn spawn_config_watcher(
    agent_yaml: &Path,
    token: CancellationToken,
    config_changed: Arc<AtomicBool>,
    model_swap: Arc<ArcSwap<Option<String>>>,
) -> miette::Result<()> {
    use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
    use std::sync::mpsc;
    use std::time::Duration;

    let watch_dir = agent_yaml
        .parent()
        .ok_or_else(|| miette::miette!("agent.yaml has no parent directory"))?
        .to_path_buf();
    let yaml_filename = agent_yaml
        .file_name()
        .ok_or_else(|| miette::miette!("agent.yaml has no filename"))?
        .to_os_string();
    let yaml_path: PathBuf = agent_yaml.to_path_buf();

    // Cache the initial yaml content so the first event has something to diff against.
    let initial_yaml = std::fs::read_to_string(&yaml_path)
        .map_err(|e| miette::miette!("failed to read {} for watcher: {e:#}", yaml_path.display()))?;

    let (tx, rx) = mpsc::channel();

    let mut debouncer = new_debouncer(Duration::from_secs(2), tx)
        .map_err(|e| miette::miette!("failed to create file watcher: {e:#}"))?;

    debouncer
        .watcher()
        .watch(&watch_dir, notify::RecursiveMode::NonRecursive)
        .map_err(|e| miette::miette!("failed to watch {}: {e:#}", watch_dir.display()))?;

    std::thread::spawn(move || {
        let _debouncer = debouncer;
        let mut last_yaml = initial_yaml;

        for result in rx {
            match result {
                Ok(events) => {
                    let relevant = events.iter().any(|e| {
                        e.kind == DebouncedEventKind::Any
                            && e.path.file_name() == Some(&yaml_filename)
                    });
                    if !relevant {
                        continue;
                    }

                    let new_yaml = match std::fs::read_to_string(&yaml_path) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "config_watcher: failed to read {} after change — restart",
                                yaml_path.display()
                            );
                            config_changed.store(true, Ordering::Release);
                            token.cancel();
                            return;
                        }
                    };

                    match diff_classify(&last_yaml, &new_yaml) {
                        ChangeKind::HotReloadable { new_model } => {
                            tracing::info!(
                                model = ?new_model.as_deref().unwrap_or("default"),
                                "agent.yaml: model-only change — hot-reloading"
                            );
                            model_swap.store(Arc::new(new_model));
                            last_yaml = new_yaml;
                            // Continue watching; do not cancel.
                        }
                        ChangeKind::RestartRequired => {
                            tracing::info!(
                                "agent.yaml changed (non-model) — initiating graceful restart"
                            );
                            config_changed.store(true, Ordering::Release);
                            token.cancel();
                            return;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("file watcher error: {e:#}");
                }
            }
        }
    });

    Ok(())
}
```

- [ ] **Step 4: Run tests, verify they pass**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-bot --lib config_watcher`
Expected: 8 tests PASS.

- [ ] **Step 5: Don't commit yet** — Task 10 wires this through and the workspace won't compile until then.

---

## Task 10: Wire the new watcher signature into `lib.rs`

`spawn_config_watcher` now takes a 4th argument. Find the call site at `crates/bot/src/lib.rs:449` and update it.

**Files:**
- Modify: `crates/bot/src/lib.rs` (~line 449, callsite of `spawn_config_watcher`)

- [ ] **Step 1: Locate the call site**

Run: `rg -n "spawn_config_watcher" crates/bot/src/lib.rs`

Read the surrounding context to find `settings_arc` (or whichever local variable holds the `Arc<AgentSettings>`).

- [ ] **Step 2: Update the call**

The current call is:

```rust
config_watcher::spawn_config_watcher(
    &agent_yaml_path,
    shutdown_token.clone(),
    config_changed.clone(),
)?;
```

Add a 4th argument — `Arc::clone(&settings_arc.model)`:

```rust
config_watcher::spawn_config_watcher(
    &agent_yaml_path,
    shutdown_token.clone(),
    config_changed.clone(),
    Arc::clone(&settings_arc.model),
)?;
```

Confirm the local variable name (`settings_arc` per `dispatch.rs`; in `lib.rs` it might be different — read the file). The argument must be the same `Arc` that `AgentSettings.model` holds, so any swap from the watcher is observed by all CC invocations.

- [ ] **Step 3: cargo check**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo check -p right-bot`
Expected: PASS.

- [ ] **Step 4: Run all bot tests**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-bot`
Expected: PASS.

- [ ] **Step 5: Commit (combines Tasks 9 + 10)**

```bash
git add crates/bot/src/config_watcher.rs crates/bot/src/lib.rs
git commit -m "feat(bot): smart-diff config watcher — hot-reload model-only changes

Watcher parses old + new agent.yaml; if only model changed, stores
into AgentSettings.model ArcSwap and continues running. Any other
change keeps existing graceful-restart behavior.

Closes the loop for /model: callback writes yaml, watcher fires,
diff sees model-only change, idempotent re-store. No restart."
```

---

## Task 11: Integration test — full `/model` flow

Exercise the end-to-end path: `handle_model` produces the right keyboard, `handle_model_callback` writes to a fixture `agent.yaml`, swaps the ArcSwap, and (mocked) edits the message. We do not boot a real Telegram bot — we drive the handlers with constructed inputs and assert side effects on the filesystem and the swap cell.

**Files:**
- Create: `crates/bot/tests/model_command.rs`

- [ ] **Step 1: Look at existing integration test scaffolding**

Run: `ls crates/bot/tests/ 2>/dev/null && rg -n "AgentSettings|AllowlistHandle|test_support" crates/bot/tests/*.rs 2>/dev/null | head -20`

If `crates/bot/tests/` does not exist or has no `AgentSettings` example, peek at `crates/bot/src/telegram/dispatch.rs:507` (the `dispatcher_builds_without_panic` smoke test) for fixture patterns.

- [ ] **Step 2: Create the integration test**

Write `crates/bot/tests/model_command.rs`:

```rust
//! End-to-end test of /model — writes to a fixture agent.yaml and verifies
//! the in-memory ArcSwap is updated. Does NOT exercise teloxide HTTP — the
//! handler-level logic (allowlist gate + persist + swap) is what we cover.

use std::sync::Arc;

use arc_swap::ArcSwap;
use right_agent::agent::types::{AgentConfig, write_agent_yaml_model};

#[test]
fn write_yaml_then_diff_classifies_as_hot_reloadable() {
    // Simulates the steady-state /model flow:
    //   ① write_agent_yaml_model writes to disk
    //   ② diff_classify (used by config_watcher) sees a model-only change
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.yaml");
    std::fs::write(&path, "restart: never\nmax_restarts: 5\n").unwrap();
    let old_yaml = std::fs::read_to_string(&path).unwrap();

    write_agent_yaml_model(&path, Some("claude-haiku-4-5")).unwrap();
    let new_yaml = std::fs::read_to_string(&path).unwrap();

    // Reproduce the watcher's diff logic — only model differs.
    let old: AgentConfig = serde_saphyr::from_str(&old_yaml).unwrap();
    let new: AgentConfig = serde_saphyr::from_str(&new_yaml).unwrap();
    let mut o = old.clone();
    let mut n = new.clone();
    o.model = None;
    n.model = None;
    assert_eq!(o, n, "non-model fields must be unchanged");
    assert_eq!(new.model.as_deref(), Some("claude-haiku-4-5"));
    assert!(old.model.is_none());
}

#[test]
fn arc_swap_visible_across_threads() {
    // Sanity: the ArcSwap cell shared between watcher and CC invocation
    // path actually propagates a store across threads.
    let cell: Arc<ArcSwap<Option<String>>> =
        Arc::new(ArcSwap::from_pointee(None));

    let cell_clone = Arc::clone(&cell);
    let writer = std::thread::spawn(move || {
        cell_clone.store(Arc::new(Some("claude-haiku-4-5".to_owned())));
    });
    writer.join().unwrap();

    let observed = (**cell.load()).clone();
    assert_eq!(observed.as_deref(), Some("claude-haiku-4-5"));
}

#[test]
fn write_then_clear_round_trips_to_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.yaml");
    std::fs::write(&path, "restart: never\n").unwrap();

    write_agent_yaml_model(&path, Some("claude-sonnet-4-6")).unwrap();
    write_agent_yaml_model(&path, None).unwrap();

    let final_yaml = std::fs::read_to_string(&path).unwrap();
    let parsed: AgentConfig = serde_saphyr::from_str(&final_yaml).unwrap();
    assert!(parsed.model.is_none());
    assert!(!final_yaml.contains("model:"));
}
```

- [ ] **Step 3: Add `tempfile` and `arc-swap` to bot dev-dependencies if missing**

Run: `grep -E "tempfile|arc-swap" crates/bot/Cargo.toml`. If not present in `[dev-dependencies]`, add:

```toml
[dev-dependencies]
tempfile = { workspace = true }
arc-swap = "1.7"
```

(`tempfile` is in the workspace; `arc-swap` was added in Task 1 to `[dependencies]` and is therefore already available — no separate dev-deps entry needed for it. Confirm by running cargo check.)

- [ ] **Step 4: Run the integration test**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-bot --test model_command`
Expected: 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/tests/model_command.rs crates/bot/Cargo.toml
git commit -m "test(bot): integration test for /model flow

Covers (1) write_agent_yaml_model + diff_classify steady-state path,
(2) ArcSwap cross-thread visibility, (3) round-trip None/Some/None."
```

---

## Task 12: Update `agent.yaml` template to mention `model`

**Files:**
- Modify: `crates/right-agent/templates/right/agent/agent.yaml`

- [ ] **Step 1: Append a commented example**

Append to `crates/right-agent/templates/right/agent/agent.yaml` (after the existing `show_thinking: true` section):

```yaml

# Claude model. Switch via Telegram /model command, or set explicitly:
#   model: "claude-sonnet-4-6"        # Sonnet 4.6 — everyday tasks
#   model: "claude-sonnet-4-6[1m]"    # Sonnet 4.6 with 1M context (extra usage)
#   model: "claude-haiku-4-5"         # Haiku 4.5 — fastest
# Omit this field to use Claude Code's default (currently Opus 4.7 with 1M).
```

- [ ] **Step 2: Verify the template still parses**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test -p right-agent --lib agent_config`
Expected: PASS — the template is loaded by tests via `include_str!` at `init.rs:29` and parsed in init tests.

- [ ] **Step 3: Commit**

```bash
git add crates/right-agent/templates/right/agent/agent.yaml
git commit -m "docs(template): document model field with /model alternative"
```

---

## Task 13: Update `ARCHITECTURE.md` and `PROMPT_SYSTEM.md`

**Files:**
- Modify: `ARCHITECTURE.md` (Configuration Hierarchy section)
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Locate the Configuration Hierarchy table in ARCHITECTURE.md**

The table lists `Per-agent | agents/<name>/agent.yaml | Restart, model, telegram, ...` Find this row.

- [ ] **Step 2: Add a hot-reload note**

Below the Configuration Hierarchy table in ARCHITECTURE.md, add a new paragraph (preserving existing formatting style):

```markdown
**Hot-reloadable fields in `agent.yaml`.** Most fields trigger a graceful
restart on change (via `config_watcher`). The exception is `model`:
the watcher's smart-diff classifies a model-only change as hot-reloadable
and stores the new value into `AgentSettings.model` (an `Arc<ArcSwap<...>>`)
without restarting. The Telegram `/model` command exploits this path —
in-flight CC subprocesses keep their old `--model`; the next invocation
in any chat picks up the new value. Adding more hot-reloadable fields
requires extending the diff in `crates/bot/src/config_watcher.rs::diff_classify`.
```

- [ ] **Step 3: Update PROMPT_SYSTEM.md**

Find a section in `PROMPT_SYSTEM.md` discussing `--model` or model selection. Add a one-paragraph note:

```markdown
**Model selection.** The agent's Claude model is read from
`agent.yaml::model` (or omitted for CC's default). Users can switch via
the Telegram `/model` command, which writes to `agent.yaml` and hot-reloads
without restart — the next CC invocation passes `--model <new>`.
```

(If `PROMPT_SYSTEM.md` already mentions `--model` somewhere, place the note adjacent to that mention.)

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md PROMPT_SYSTEM.md
git commit -m "docs: document /model command and hot-reload model path"
```

---

## Task 14: Final build + manual smoke

Verify the whole workspace builds and tests pass; do a manual smoke if the dev environment supports it.

- [ ] **Step 1: Workspace build**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo build --workspace`
Expected: PASS.

- [ ] **Step 2: Workspace tests**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo test --workspace --lib`
Expected: PASS.

- [ ] **Step 3: Workspace clippy (warning floor)**

Run: `cd /Users/molt/dev/rightclaw && devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS. If clippy flags `arc-swap` `Guard` patterns or similar, address inline (the deref-deref-clone pattern `(**guard).clone()` is idiomatic and clippy should accept it).

- [ ] **Step 4: Manual smoke (only if a dev agent is running)**

If you have a live `right` instance with a Telegram bot:

1. In a DM with the bot: send `/model`. Confirm a 4-button keyboard appears with the current model checkmarked.
2. Click another model (e.g. Sonnet). Confirm:
   - Toast: "Switched to Sonnet"
   - Menu updates with ✓ on the new row
   - `agent.yaml` now contains `model: "claude-sonnet-4-6"`
   - Bot logs: `INFO model switched: from=... to=claude-sonnet-4-6 ...`
   - **No bot restart** in process-compose logs (no `right-bot` process restart event)
3. Send a regular message. Confirm the bot replies and the CC invocation in `~/.right/logs/<agent>.log` shows `--model claude-sonnet-4-6`.
4. In a group where the bot is added: send `/model` from a non-allowlisted account. Confirm: nothing happens (silently ignored).
5. Send `/model` from an allowlisted account in the same group. Confirm: menu appears.

If something fails, the most likely culprits are (a) MCP/dispatch DI mismatch (run `cargo test dispatcher_builds_without_panic`); (b) yaml round-trip damaging a field (re-read after switch and parse with `serde_saphyr::from_str::<AgentConfig>`); (c) watcher firing twice and racing on the swap (idempotent — final state should still match what's on disk).

- [ ] **Step 5: Final commit (only if any fixups were needed)**

If steps 1–3 produced changes (e.g. clippy fixes), commit them:

```bash
git add -p   # interactively pick changes
git commit -m "chore(bot): post-/model fixups from final build"
```

---

## Spec coverage check

Mapping each requirement from `docs/superpowers/specs/2026-05-06-model-command-design.md` to a task:

| Spec requirement | Task |
|---|---|
| `AgentConfig.model: Option<String>` | Already exists; verified pre-Task 1 |
| `write_agent_yaml_model` MergedRMW helper | Task 2 |
| `BotCommand::Model` variant + autocomplete | Task 8 |
| `model_command.rs` module + `MODEL_CHOICES` | Task 5 |
| `AgentSettings.model: Arc<ArcSwap<...>>` | Task 3 |
| `WorkerContext.model` swap-able + load on each invoke | Task 4 |
| Smart-diff `config_watcher` | Task 9 |
| `lib.rs:449` watcher wiring | Task 10 |
| Group allowlist gate (initial + callback re-check) | Tasks 6, 7 |
| Custom-model display ("Current: X (custom)") | Task 5 |
| Model-only change → no restart | Task 9 (asserted in tests), Task 14 (manual) |
| In-flight CC keeps old model; next invocation new | Task 4 (`(**ctx.model.load()).clone()` on each invoke) |
| FAIL FAST error handling | Tasks 2, 7, 9 (no `unwrap_or_default` / `.ok()`; all errors propagate) |
| Tests: alias uniqueness, lookup, active match | Task 5 |
| Tests: yaml round-trip, atomic write | Task 2 (round-trip via `write_merged_rmw` which writes atomically) |
| Tests: diff classification | Task 9 |
| Tests: integration | Task 11 |
| Docs: ARCHITECTURE.md hot-reload note | Task 13 |
| Docs: PROMPT_SYSTEM.md model-selection note | Task 13 |
| Template: agent.yaml comment | Task 12 |

---

## Notes for the implementing agent

- Do **not** invoke `right-bot` to test — the integration test in Task 11 exercises the persist + swap path without booting a bot. Manual smoke (Task 14, Step 4) is optional and only meaningful with a live `right` install.
- The `[1m]` suffix in `claude-sonnet-4-6[1m]` is what CC's command line accepts (verified from the system prompt's listed canonical model ID `claude-opus-4-7[1m]`). If shell quoting in any specific deployment escapes it incorrectly, the value still round-trips through `agent.yaml` and `--model` (both pass it through verbatim).
- Per CLAUDE.md, never delete sandboxes for migration. This change does not require sandbox recreation — model is a CC CLI flag, not a sandbox config field.
- Per CLAUDE.rust.md, Edition 2024 + FAIL FAST. Every error must propagate via `?` or explicit return — no `unwrap_or_default`, no `.ok()`, no `let _ = ...`.
- When converting `anyhow::Error` / `miette::Error` to `String` for logging or wrapping, use `format!("{:#}", e)` (alternate Display) — preserves the error chain.
