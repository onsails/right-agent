# Switchable Response Modes (addressed/all) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a trusted user switch a group or a single topic between `addressed` (respond only when mentioned/replied/commanded) and `all` (respond to every participant's message), and fix the forum-topic-root bug that makes the bot answer everything only in topics it created.

**Architecture:** Mode is stored per-group (default) and per-topic (override) inside `allowlist.yaml` (bumped to version 2, backward-compatible serde defaults). `make_routing_filter` consults the effective mode for `(chat_id, effective_thread_id)`. Inline-keyboard commands `/mode` and `/mode_group` (mirroring `/model`) toggle it, gated to trusted users. A one-line guard in `is_bot_addressed` stops treating the topic-root service message as a reply-to-bot.

**Tech Stack:** Rust 2024, teloxide 0.17 (teloxide-core 0.13), serde, serde_saphyr (YAML), dptree dispatch.

---

## Spec

`docs/superpowers/specs/2026-06-06-response-modes-design.md`

## Verification cadence

Targeted package tests during development (`devenv shell -- cargo test -p <crate> <filter>`); one full `devenv shell -- cargo test --workspace` as the final gate (Task 8). Do not run the full workspace suite between every task.

## File Structure

- `crates/right-agent/src/agent/allowlist.rs` — `ResponseMode`, `TopicMode`, `AllowedGroup` fields, version/parse/serialize, lookup + setters. (+ `allowlist_tests.rs`)
- `crates/bot/src/telegram/filter.rs` — mode-aware gating in `make_routing_filter`.
- `crates/bot/src/telegram/mention.rs` — topic-root reply guard in `is_bot_addressed`.
- `crates/bot/src/telegram/mode_command.rs` — **new** — `/mode`, `/mode_group`, keyboard render, callback.
- `crates/bot/src/telegram/dispatch.rs` — `BotCommand` variants, command branches, callback filter.
- `crates/bot/src/telegram/allowlist_commands.rs` — make `persist_new` reusable.
- `crates/right/src/main.rs` — CLI `agent mode` / `agent mode-group` subcommands.

---

## Task 1: Schema v2 — `ResponseMode`, `TopicMode`, `AllowedGroup` fields, parse/serialize

**Files:**
- Modify: `crates/right-agent/src/agent/allowlist.rs`
- Test: `crates/right-agent/src/agent/allowlist_tests.rs`
- Modify (compile fix, add two fields to every `AllowedGroup {…}` literal): `crates/right-agent/src/agent/allowlist.rs` (migrate_from_legacy), `crates/right-agent/src/doctor.rs`, `crates/right-agent/src/agent/allowlist_tests.rs`, `crates/bot/src/async_delivery.rs`, `crates/bot/src/telegram/dispatch.rs` (test), `crates/bot/src/telegram/filter.rs` (test builder), `crates/right/src/right_backend_tests.rs`

- [ ] **Step 1: Write the failing tests** (append to `allowlist_tests.rs`)

```rust
#[test]
fn response_mode_defaults_to_addressed() {
    assert_eq!(ResponseMode::default(), ResponseMode::Addressed);
}

#[test]
fn parse_v1_file_upgrades_to_v2_addressed() {
    let yaml = "version: 1\nusers: []\ngroups:\n  - id: -100\n    label: null\n    opened_by: null\n    opened_at: 2026-06-06T00:00:00Z\n";
    let file = parse_yaml(yaml).expect("v1 must parse");
    assert_eq!(file.version, CURRENT_VERSION);
    assert_eq!(file.groups[0].mode, ResponseMode::Addressed);
    assert!(file.groups[0].topics.is_empty());
}

#[test]
fn parse_v2_roundtrip_preserves_modes() {
    let mut file = AllowlistFile::default();
    file.groups.push(AllowedGroup {
        id: -100,
        label: None,
        opened_by: None,
        opened_at: chrono::DateTime::parse_from_rfc3339("2026-06-06T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        mode: ResponseMode::All,
        topics: vec![TopicMode { thread_id: 8, mode: ResponseMode::Addressed }],
    });
    let text = serialize_yaml(&file);
    let reparsed = parse_yaml(&text).expect("v2 roundtrip");
    assert_eq!(reparsed.groups[0].mode, ResponseMode::All);
    assert_eq!(reparsed.groups[0].topics, vec![TopicMode { thread_id: 8, mode: ResponseMode::Addressed }]);
}

#[test]
fn serialize_omits_mode_for_addressed_group_without_topics() {
    // Byte-stability with v1-era groups: an addressed group with no topics
    // must not emit `mode:` or `topics:` lines.
    let mut file = AllowlistFile::default();
    file.groups.push(AllowedGroup {
        id: -100,
        label: None,
        opened_by: None,
        opened_at: chrono::DateTime::parse_from_rfc3339("2026-06-06T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        mode: ResponseMode::Addressed,
        topics: vec![],
    });
    let text = serialize_yaml(&file);
    assert!(!text.contains("mode:"), "no mode line:\n{text}");
    assert!(!text.contains("topics:"), "no topics line:\n{text}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-agent --lib agent::allowlist 2>&1 | tail -20`
Expected: FAIL — `ResponseMode` / `TopicMode` / `AllowedGroup.mode` not found (compile error).

- [ ] **Step 3: Add the types and fields** (`allowlist.rs`)

After the `use serde::{Deserialize, Serialize};` block, add:

```rust
/// Per-scope response mode. Default `Addressed` preserves the historical
/// "must address the bot" behaviour.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ResponseMode {
    /// Respond only when addressed (mention / reply-to-bot / command).
    #[default]
    Addressed,
    /// Respond to every message in the scope, from any participant.
    All,
}

/// A per-topic override of the group's default mode. `thread_id` is the
/// normalised effective thread id (General = 0).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TopicMode {
    pub thread_id: i64,
    pub mode: ResponseMode,
}
```

In `AllowedGroup`, add two fields after `opened_at`:

```rust
    #[serde(default)]
    pub mode: ResponseMode,
    #[serde(default)]
    pub topics: Vec<TopicMode>,
```

Bump the version constant:

```rust
pub const CURRENT_VERSION: u32 = 2;
```

Relax `parse_yaml` to accept v1 or v2 and upgrade in memory. Replace the version check body:

```rust
pub fn parse_yaml(text: &str) -> Result<AllowlistFile, String> {
    let mut parsed: AllowlistFile =
        serde_saphyr::from_str(text).map_err(|e| format!("allowlist.yaml parse error: {e:#}"))?;
    if parsed.version != 1 && parsed.version != CURRENT_VERSION {
        return Err(format!(
            "allowlist.yaml version {} is not supported (expected 1 or {})",
            parsed.version, CURRENT_VERSION
        ));
    }
    // Upgrade-on-read: serde defaults already filled mode/topics; normalise the
    // version so any subsequent serialize writes v2.
    parsed.version = CURRENT_VERSION;
    Ok(parsed)
}
```

In `serialize_yaml`, inside the `for g in &file.groups` loop, after the `opened_at` `writeln!`, emit mode (only when non-default) and topics (only when non-empty):

```rust
            if g.mode != ResponseMode::default() {
                writeln!(out, "    mode: {}", response_mode_str(g.mode)).unwrap();
            }
            if !g.topics.is_empty() {
                out.push_str("    topics:\n");
                for t in &g.topics {
                    writeln!(out, "      - thread_id: {}", t.thread_id).unwrap();
                    writeln!(out, "        mode: {}", response_mode_str(t.mode)).unwrap();
                }
            }
```

Add a small helper near `write_opt_i64`:

```rust
fn response_mode_str(m: ResponseMode) -> &'static str {
    match m {
        ResponseMode::Addressed => "addressed",
        ResponseMode::All => "all",
    }
}
```

- [ ] **Step 4: Fix every `AllowedGroup {…}` literal to compile**

Add `mode: ResponseMode::Addressed,` and `topics: Vec::new(),` to each literal. Sites (the compiler will also list them):
- `crates/right-agent/src/agent/allowlist.rs` — `migrate_from_legacy` (the `file.groups.push(AllowedGroup { … })`).
- `crates/right-agent/src/doctor.rs` — the `file.groups.push(AllowedGroup { … })`.
- `crates/right-agent/src/agent/allowlist_tests.rs` — every `groups: vec![AllowedGroup { … }]` and `add_group(AllowedGroup { … })`.
- `crates/bot/src/async_delivery.rs` — `add_group(AllowedGroup { … })`.
- `crates/bot/src/telegram/dispatch.rs` — test `groups: vec![AllowedGroup { … }]`.
- `crates/bot/src/telegram/filter.rs` — test builder `.map(|id| AllowedGroup { … })` (~line 91).
- `crates/right/src/right_backend_tests.rs` — `.push(right_agent::agent::allowlist::AllowedGroup { … })`.

For the `filter.rs` test builder, the closure must also import the type; it already references `AllowedGroup`, so just add the two fields.

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-agent --lib agent::allowlist 2>&1 | tail -20`
Expected: PASS (all four new tests + existing allowlist tests).

- [ ] **Step 6: Confirm dependent crates still compile**

Run: `devenv shell -- cargo check -p right-bot -p right 2>&1 | tail -15`
Expected: no errors (all literals fixed).

- [ ] **Step 7: Commit**

```bash
git add crates/right-agent crates/bot crates/right
git commit -m "feat(allowlist): response-mode schema v2 (addressed/all) with backward-compatible parse"
```

---

## Task 2: Effective-mode lookup + setters

**Files:**
- Modify: `crates/right-agent/src/agent/allowlist.rs` (impl `AllowlistState`)
- Test: `crates/right-agent/src/agent/allowlist_tests.rs`

- [ ] **Step 1: Write the failing tests** (append to `allowlist_tests.rs`)

```rust
fn group(id: i64, mode: ResponseMode, topics: Vec<TopicMode>) -> AllowedGroup {
    AllowedGroup {
        id,
        label: None,
        opened_by: None,
        opened_at: chrono::Utc::now(),
        mode,
        topics,
    }
}

#[test]
fn response_mode_precedence_topic_over_group_over_default() {
    let mut file = AllowlistFile::default();
    file.groups.push(group(-100, ResponseMode::All, vec![TopicMode { thread_id: 8, mode: ResponseMode::Addressed }]));
    let s = AllowlistState::from_file(file);
    // topic override wins
    assert_eq!(s.response_mode(-100, 8), ResponseMode::Addressed);
    // group default for an unlisted topic (General = 0)
    assert_eq!(s.response_mode(-100, 0), ResponseMode::All);
    // unknown group falls back to Addressed
    assert_eq!(s.response_mode(-999, 0), ResponseMode::Addressed);
}

#[test]
fn set_group_mode_requires_open_group() {
    let mut s = AllowlistState::from_file(AllowlistFile::default());
    assert!(!s.set_group_mode(-100, ResponseMode::All), "closed group → false");
    s.add_group(group(-100, ResponseMode::Addressed, vec![]));
    assert!(s.set_group_mode(-100, ResponseMode::All));
    assert_eq!(s.response_mode(-100, 0), ResponseMode::All);
}

#[test]
fn set_and_clear_topic_mode() {
    let mut s = AllowlistState::from_file(AllowlistFile::default());
    s.add_group(group(-100, ResponseMode::Addressed, vec![]));
    assert!(s.set_topic_mode(-100, 0, ResponseMode::All));
    assert_eq!(s.response_mode(-100, 0), ResponseMode::All);
    // overwrite existing entry
    assert!(s.set_topic_mode(-100, 0, ResponseMode::Addressed));
    assert_eq!(s.response_mode(-100, 0), ResponseMode::Addressed);
    // clear removes the override → back to group default
    assert!(s.clear_topic_mode(-100, 0));
    assert_eq!(s.response_mode(-100, 0), ResponseMode::Addressed);
    // clearing a non-existent override → false
    assert!(!s.clear_topic_mode(-100, 0));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-agent --lib agent::allowlist::tests::response_mode 2>&1 | tail -15`
Expected: FAIL — `response_mode` / `set_group_mode` not found.

- [ ] **Step 3: Implement the methods** (inside `impl AllowlistState`, after `is_chat_allowed`)

```rust
    /// Effective response mode for a scope. Precedence: explicit topic entry →
    /// group default → built-in `Addressed`. Unknown (closed) group → `Addressed`.
    pub fn response_mode(&self, chat_id: i64, thread_id: i64) -> ResponseMode {
        let Some(g) = self.inner.groups.iter().find(|g| g.id == chat_id) else {
            return ResponseMode::Addressed;
        };
        if let Some(t) = g.topics.iter().find(|t| t.thread_id == thread_id) {
            return t.mode;
        }
        g.mode
    }

    /// Set the group-level default mode. Returns false if the group is not open.
    pub fn set_group_mode(&mut self, chat_id: i64, mode: ResponseMode) -> bool {
        match self.inner.groups.iter_mut().find(|g| g.id == chat_id) {
            Some(g) => {
                g.mode = mode;
                true
            }
            None => false,
        }
    }

    /// Set (or overwrite) a per-topic mode override. Returns false if the group
    /// is not open.
    pub fn set_topic_mode(&mut self, chat_id: i64, thread_id: i64, mode: ResponseMode) -> bool {
        let Some(g) = self.inner.groups.iter_mut().find(|g| g.id == chat_id) else {
            return false;
        };
        match g.topics.iter_mut().find(|t| t.thread_id == thread_id) {
            Some(t) => t.mode = mode,
            None => g.topics.push(TopicMode { thread_id, mode }),
        }
        true
    }

    /// Remove a per-topic override. Returns true iff an override was removed.
    pub fn clear_topic_mode(&mut self, chat_id: i64, thread_id: i64) -> bool {
        let Some(g) = self.inner.groups.iter_mut().find(|g| g.id == chat_id) else {
            return false;
        };
        let before = g.topics.len();
        g.topics.retain(|t| t.thread_id != thread_id);
        g.topics.len() != before
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-agent --lib agent::allowlist 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent
git commit -m "feat(allowlist): effective response-mode lookup and setters"
```

---

## Task 3: Mode-aware gating in the routing filter

**Files:**
- Modify: `crates/bot/src/telegram/filter.rs`
- Test: same file (`mod tests`)

- [ ] **Step 1: Write the failing tests** (append inside `filter.rs` `mod tests`)

```rust
    fn open_group_with_mode(chat_id: i64, mode: right_agent::agent::allowlist::ResponseMode) -> AllowlistHandle {
        use right_agent::agent::allowlist::{AllowedGroup, AllowlistFile, AllowlistState};
        let g = AllowedGroup {
            id: chat_id,
            label: None,
            opened_by: None,
            opened_at: Utc::now(),
            mode,
            topics: vec![],
        };
        let file = AllowlistFile {
            version: right_agent::agent::allowlist::CURRENT_VERSION,
            users: vec![],
            groups: vec![g],
        };
        AllowlistHandle(Arc::new(std::sync::RwLock::new(AllowlistState::from_file(file))))
    }

    fn plain_group_text(chat_id: i64, sender_id: i64, text: &str) -> teloxide::types::Message {
        serde_json::from_value(serde_json::json!({
            "message_id": 1, "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
            "text": text
        })).unwrap()
    }

    #[tokio::test]
    async fn all_mode_admits_untrusted_unaddressed_text() {
        use right_agent::agent::allowlist::ResponseMode;
        let identity = BotIdentity { username: "rightaww_bot".into(), user_id: 999 };
        let chat_id = -1001;
        let allowlist = open_group_with_mode(chat_id, ResponseMode::All);
        let msg = plain_group_text(chat_id, 42, "какие у нас кроны есть?");
        let f = make_routing_filter(allowlist, identity);
        let d = f(msg).expect("All mode admits plain text");
        assert!(d.address.is_none());
        assert!(d.group_open);
    }

    #[tokio::test]
    async fn addressed_mode_drops_unaddressed_text_even_when_open() {
        use right_agent::agent::allowlist::ResponseMode;
        let identity = BotIdentity { username: "rightaww_bot".into(), user_id: 999 };
        let chat_id = -1001;
        let allowlist = open_group_with_mode(chat_id, ResponseMode::Addressed);
        let msg = plain_group_text(chat_id, 42, "just chatting");
        let f = make_routing_filter(allowlist, identity);
        assert!(f(msg).is_none());
    }

    #[tokio::test]
    async fn all_mode_ignores_other_bots() {
        use right_agent::agent::allowlist::ResponseMode;
        let identity = BotIdentity { username: "rightaww_bot".into(), user_id: 999 };
        let chat_id = -1001;
        let allowlist = open_group_with_mode(chat_id, ResponseMode::All);
        let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1, "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": 5000, "is_bot": true, "first_name": "OtherBot"},
            "text": "loop bait"
        })).unwrap();
        let f = make_routing_filter(allowlist, identity);
        assert!(f(msg).is_none(), "All mode must not answer other bots");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot --lib telegram::filter 2>&1 | tail -20`
Expected: FAIL — `open_group_with_mode` admits nothing yet / `address` mismatch (current filter drops plain text).

- [ ] **Step 3: Implement mode-aware gating** (`filter.rs`)

Add the import near the top of the file (after the existing `use`):

```rust
use right_agent::agent::allowlist::ResponseMode;
```

Inside the closure, compute the mode while the read lock is held — replace the existing read block (lines ~30-33):

```rust
        let state = allowlist.0.read().expect("allowlist lock poisoned");
        let sender_trusted = state.is_user_trusted(sender_id);
        let group_open = state.is_group_open(chat_id);
        let response_mode =
            state.response_mode(chat_id, super::session::effective_thread_id(&msg));
        drop(state);
```

Replace the group/supergroup arm (`_ => { … }`) with:

```rust
            _ => {
                // `All` mode in an open group answers everyone, no addressing —
                // but never another bot (loop guard) and never the bot itself.
                if response_mode == ResponseMode::All && group_open && !sender.is_bot {
                    return Some(RoutingDecision {
                        address: addressed,
                        sender_trusted,
                        group_open,
                    });
                }
                if !sender_trusted && !group_open {
                    return None;
                }
                // Non-album group messages still require an explicit address.
                if addressed.is_none()
                    && msg.media_group_id().is_none()
                    && msg.forward_origin().is_none()
                {
                    return None;
                }
                Some(RoutingDecision {
                    address: addressed,
                    sender_trusted,
                    group_open,
                })
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot --lib telegram::filter 2>&1 | tail -20`
Expected: PASS (new tests + all existing filter tests, since default mode is `Addressed`).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/filter.rs
git commit -m "feat(filter): respond-to-all mode gating per scope"
```

---

## Task 4: Fix forum-topic-root reply mis-detection

**Files:**
- Modify: `crates/bot/src/telegram/mention.rs`
- Test: same file (`mod tests`)

- [ ] **Step 1: Write the failing tests** (append inside `mention.rs` `mod tests`)

```rust
    #[tokio::test]
    async fn topic_root_reply_is_not_addressing() {
        // A plain message in a bot-created topic: reply_to_message is the
        // forum_topic_created service message whose `from` is the bot.
        // It must NOT count as addressing. (teloxide-core fixture shape.)
        let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 5, "date": 0,
            "chat": {"id": -1001, "is_forum": true, "type": "supergroup", "title": "g"},
            "from": {"id": 42, "is_bot": false, "first_name": "U"},
            "is_topic_message": true,
            "message_thread_id": 4,
            "text": "привет",
            "reply_to_message": {
                "message_id": 4, "date": 0,
                "chat": {"id": -1001, "is_forum": true, "type": "supergroup", "title": "g"},
                "from": {"id": 999, "is_bot": true, "first_name": "Bot"},
                "is_topic_message": true,
                "message_thread_id": 4,
                "forum_topic_created": {"name": "Socials", "icon_color": 9367192}
            }
        })).unwrap();
        let identity = BotIdentity { username: "rightaww_bot".into(), user_id: 999 };
        assert_eq!(is_bot_addressed(&msg, &identity), None);
    }

    #[tokio::test]
    async fn real_reply_to_bot_is_addressing() {
        let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 6, "date": 0,
            "chat": {"id": -1001, "type": "supergroup", "title": "g"},
            "from": {"id": 42, "is_bot": false, "first_name": "U"},
            "text": "thanks",
            "reply_to_message": {
                "message_id": 4, "date": 0,
                "chat": {"id": -1001, "type": "supergroup", "title": "g"},
                "from": {"id": 999, "is_bot": true, "first_name": "Bot"},
                "text": "here you go"
            }
        })).unwrap();
        let identity = BotIdentity { username: "rightaww_bot".into(), user_id: 999 };
        assert_eq!(is_bot_addressed(&msg, &identity), Some(AddressKind::GroupReplyToBot));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot --lib telegram::mention 2>&1 | tail -15`
Expected: FAIL — `topic_root_reply_is_not_addressing` returns `Some(GroupReplyToBot)` (the bug).

- [ ] **Step 3: Add the guard** (`mention.rs`, the reply branch ~line 31)

```rust
            // 1) reply to bot's message — but NOT the forum-topic-root service
            //    message. Telegram threads topic membership via a reply to the
            //    `forum_topic_created` message, whose author is the topic
            //    creator; when the bot created the topic this would otherwise
            //    make every message look like a reply to the bot.
            if let Some(reply) = msg.reply_to_message()
                && reply.forum_topic_created().is_none()
                && let Some(from) = reply.from.as_ref()
                && from.id.0 == identity.user_id
            {
                return Some(AddressKind::GroupReplyToBot);
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot --lib telegram::mention 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/mention.rs
git commit -m "fix(mention): topic-root service message is not a reply-to-bot"
```

---

## Task 5: `/mode` and `/mode_group` commands + callback

**Files:**
- Create: `crates/bot/src/telegram/mode_command.rs`
- Modify: `crates/bot/src/telegram/allowlist_commands.rs` (make `persist_new` reusable)
- Modify: `crates/bot/src/telegram/mod.rs` (declare the module)
- Test: `mode_command.rs` (`mod tests`)

- [ ] **Step 1: Expose `persist_new`** (`allowlist_commands.rs`)

Change the signature from `async fn persist_new(` to:

```rust
pub(crate) async fn persist_new(
```

- [ ] **Step 2: Declare the module** (`crates/bot/src/telegram/mod.rs`)

Add alongside the other `mod` lines (e.g. near `mod model_command;`):

```rust
mod mode_command;
```

If `model_command` is declared `pub(crate) mod`, match that visibility.

- [ ] **Step 3: Write the failing tests** (the pure render/parse helpers; create `mode_command.rs` with only these + stubs first)

Create `crates/bot/src/telegram/mode_command.rs` starting with the module doc, the `CallbackAction` parse, the keyboard renderers, and a `#[cfg(test)] mod tests`:

```rust
//! `/mode` (current topic) and `/mode_group` (whole group) — inline-keyboard
//! toggles for the per-scope response mode. Trusted-only, group-only.
//! Mirrors `model_command.rs`.

use std::sync::Arc;

use right_agent::agent::allowlist::{AllowlistHandle, ResponseMode};
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, Message};

use super::BotType;
use super::handler::AgentDir;

/// Which scope a `/mode*` interaction targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModeScope {
    Topic,
    Group,
}

/// A parsed callback: scope + requested change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModeAction {
    Set(ResponseMode),
    /// Topic-only: clear the override, inherit the group default.
    ClearTopic,
}

/// Parse callback data. Topic prefix `mode:`, group prefix `modegroup:`.
/// Returns `None` for anything unrecognised.
pub(crate) fn parse_callback(data: &str) -> Option<(ModeScope, ModeAction)> {
    if let Some(rest) = data.strip_prefix("modegroup:") {
        let action = match rest {
            "addressed" => ModeAction::Set(ResponseMode::Addressed),
            "all" => ModeAction::Set(ResponseMode::All),
            _ => return None,
        };
        return Some((ModeScope::Group, action));
    }
    if let Some(rest) = data.strip_prefix("mode:") {
        let action = match rest {
            "addressed" => ModeAction::Set(ResponseMode::Addressed),
            "all" => ModeAction::Set(ResponseMode::All),
            "clear" => ModeAction::ClearTopic,
            _ => return None,
        };
        return Some((ModeScope::Topic, action));
    }
    None
}

fn mode_label(m: ResponseMode) -> &'static str {
    match m {
        ResponseMode::Addressed => "Addressed",
        ResponseMode::All => "All",
    }
}

/// Topic keyboard: [Addressed][All] then [Inherit group]. `✓` marks the
/// effective mode; the Inherit button is shown only when an override exists.
pub(crate) fn topic_keyboard(effective: ResponseMode, has_override: bool) -> InlineKeyboardMarkup {
    let btn = |m: ResponseMode, data: &str| {
        let label = if effective == m && has_override {
            format!("✓ {}", mode_label(m))
        } else {
            mode_label(m).to_string()
        };
        InlineKeyboardButton::callback(label, format!("mode:{data}"))
    };
    let mut rows = vec![vec![
        btn(ResponseMode::Addressed, "addressed"),
        btn(ResponseMode::All, "all"),
    ]];
    if has_override {
        rows.push(vec![InlineKeyboardButton::callback(
            "↩︎ Inherit group",
            "mode:clear",
        )]);
    }
    InlineKeyboardMarkup::new(rows)
}

/// Group keyboard: [Addressed][All], `✓` on the active default.
pub(crate) fn group_keyboard(current: ResponseMode) -> InlineKeyboardMarkup {
    let btn = |m: ResponseMode, data: &str| {
        let label = if current == m {
            format!("✓ {}", mode_label(m))
        } else {
            mode_label(m).to_string()
        };
        InlineKeyboardButton::callback(label, format!("modegroup:{data}"))
    };
    InlineKeyboardMarkup::new(vec![vec![
        btn(ResponseMode::Addressed, "addressed"),
        btn(ResponseMode::All, "all"),
    ]])
}

fn topic_body(effective: ResponseMode, has_override: bool) -> String {
    let src = if has_override { "topic override" } else { "inherited from group" };
    format!(
        "💬 Response mode — this topic\n\nCurrent: {} ({src})\n\nAddressed — reply only when @mentioned, replied-to, or commanded.\nAll — reply to every message from anyone here.",
        mode_label(effective)
    )
}

fn group_body(current: ResponseMode) -> String {
    format!(
        "💬 Response mode — whole group (default for topics without their own)\n\nCurrent: {}\n\nAddressed — reply only when addressed.\nAll — reply to every message from anyone.",
        mode_label(current)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parse_group_callbacks() {
        assert_eq!(parse_callback("modegroup:all"), Some((ModeScope::Group, ModeAction::Set(ResponseMode::All))));
        assert_eq!(parse_callback("modegroup:addressed"), Some((ModeScope::Group, ModeAction::Set(ResponseMode::Addressed))));
    }

    #[tokio::test]
    async fn parse_topic_callbacks() {
        assert_eq!(parse_callback("mode:all"), Some((ModeScope::Topic, ModeAction::Set(ResponseMode::All))));
        assert_eq!(parse_callback("mode:clear"), Some((ModeScope::Topic, ModeAction::ClearTopic)));
    }

    #[tokio::test]
    async fn parse_rejects_unknown() {
        assert!(parse_callback("mode:bogus").is_none());
        assert!(parse_callback("other:all").is_none());
    }

    #[tokio::test]
    async fn topic_keyboard_hides_inherit_without_override() {
        let kb = topic_keyboard(ResponseMode::Addressed, false);
        assert_eq!(kb.inline_keyboard.len(), 1);
    }

    #[tokio::test]
    async fn topic_keyboard_shows_inherit_with_override() {
        let kb = topic_keyboard(ResponseMode::All, true);
        assert_eq!(kb.inline_keyboard.len(), 2);
        match &kb.inline_keyboard[1][0].kind {
            teloxide::types::InlineKeyboardButtonKind::CallbackData(d) => assert_eq!(d, "mode:clear"),
            _ => panic!("expected callback"),
        }
    }

    #[tokio::test]
    async fn group_keyboard_callback_data() {
        let kb = group_keyboard(ResponseMode::Addressed);
        let data: Vec<String> = kb.inline_keyboard.iter().flatten().filter_map(|b| match &b.kind {
            teloxide::types::InlineKeyboardButtonKind::CallbackData(d) => Some(d.clone()),
            _ => None,
        }).collect();
        assert_eq!(data, vec!["modegroup:addressed", "modegroup:all"]);
    }
}
```

- [ ] **Step 4: Run the pure tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot --lib telegram::mode_command 2>&1 | tail -20`
Expected: PASS (render/parse are pure; no handlers needed yet).

- [ ] **Step 5: Add the command + callback handlers** (append to `mode_command.rs`, before `mod tests`)

```rust
/// `/mode` — toggle the **current topic's** mode. Group-only, trusted-only.
pub(crate) async fn handle_mode(
    bot: BotType,
    msg: Message,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    if super::handler::is_private_chat(&msg.chat.kind) {
        send_in_thread(&bot, &msg, "/mode is only valid in group chats").await?;
        return Ok(());
    }
    if !super::allowlist_commands::sender_is_trusted(&msg, &allowlist) {
        tracing::debug!("/mode ignored: non-trusted sender");
        return Ok(());
    }
    let chat_id = msg.chat.id.0;
    let thread_id = super::session::effective_thread_id(&msg);
    let (open, effective, has_override) = {
        let s = allowlist.0.read().expect("allowlist lock poisoned");
        let open = s.is_group_open(chat_id);
        let effective = s.response_mode(chat_id, thread_id);
        let has_override = s
            .groups()
            .iter()
            .find(|g| g.id == chat_id)
            .map(|g| g.topics.iter().any(|t| t.thread_id == thread_id))
            .unwrap_or(false);
        (open, effective, has_override)
    };
    if !open {
        send_in_thread(&bot, &msg, "Open the group first with /allow_all, then set a mode").await?;
        return Ok(());
    }
    let body = topic_body(effective, has_override);
    let kb = topic_keyboard(effective, has_override);
    let mut send = bot.send_message(msg.chat.id, body).reply_markup(kb);
    if let Some(t) = msg.thread_id {
        send = send.message_thread_id(t);
    }
    send.await?;
    Ok(())
}

/// `/mode_group` — toggle the **whole-group** default. Group-only, trusted-only.
pub(crate) async fn handle_mode_group(
    bot: BotType,
    msg: Message,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    if super::handler::is_private_chat(&msg.chat.kind) {
        send_in_thread(&bot, &msg, "/mode_group is only valid in group chats").await?;
        return Ok(());
    }
    if !super::allowlist_commands::sender_is_trusted(&msg, &allowlist) {
        tracing::debug!("/mode_group ignored: non-trusted sender");
        return Ok(());
    }
    let chat_id = msg.chat.id.0;
    let (open, current) = {
        let s = allowlist.0.read().expect("allowlist lock poisoned");
        let open = s.is_group_open(chat_id);
        let current = s.groups().iter().find(|g| g.id == chat_id).map(|g| g.mode).unwrap_or_default();
        (open, current)
    };
    if !open {
        send_in_thread(&bot, &msg, "Open the group first with /allow_all, then set a mode").await?;
        return Ok(());
    }
    let body = group_body(current);
    let kb = group_keyboard(current);
    let mut send = bot.send_message(msg.chat.id, body).reply_markup(kb);
    if let Some(t) = msg.thread_id {
        send = send.message_thread_id(t);
    }
    send.await?;
    Ok(())
}

async fn send_in_thread(bot: &BotType, msg: &Message, text: &str) -> ResponseResult<()> {
    let mut send = bot.send_message(msg.chat.id, text);
    if let Some(t) = msg.thread_id {
        send = send.message_thread_id(t);
    }
    send.await?;
    Ok(())
}

/// Handle a tap on a `/mode` or `/mode_group` button. Re-checks trusted on
/// every click; chat is taken from the message (no cross-chat targeting).
pub(crate) async fn handle_mode_callback(
    bot: BotType,
    q: teloxide::types::CallbackQuery,
    agent_dir: Arc<AgentDir>,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    let Some(data) = q.data.as_deref() else {
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };
    let Some((scope, action)) = parse_callback(data) else {
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };

    // Trusted re-check (keyboard persists; any member can tap).
    let trusted = allowlist
        .0
        .read()
        .expect("allowlist lock poisoned")
        .is_user_trusted(q.from.id.0 as i64);
    if !trusted {
        bot.answer_callback_query(q.id).text("Not allowed").await?;
        return Ok(());
    }

    // Chat + thread from the menu message (security boundary: never cross-chat).
    let Some(message) = q.message.as_ref() else {
        bot.answer_callback_query(q.id).text("Message unavailable").await?;
        return Ok(());
    };
    let chat_id = message.chat().id.0;
    let thread_id = message
        .regular_message()
        .map(super::session::effective_thread_id)
        .unwrap_or(0);

    // RMW the allowlist.
    let mut next = allowlist.0.read().expect("allowlist lock poisoned").clone();
    let ok = match (scope, action) {
        (ModeScope::Group, ModeAction::Set(m)) => next.set_group_mode(chat_id, m),
        (ModeScope::Topic, ModeAction::Set(m)) => next.set_topic_mode(chat_id, thread_id, m),
        (ModeScope::Topic, ModeAction::ClearTopic) => {
            // Treat "already absent" as success — the end state is what matters.
            next.clear_topic_mode(chat_id, thread_id);
            next.is_group_open(chat_id)
        }
        (ModeScope::Group, ModeAction::ClearTopic) => false, // unreachable via parse
    };
    if !ok {
        bot.answer_callback_query(q.id).text("Group is not opened").await?;
        return Ok(());
    }
    if let Err(e) = super::allowlist_commands::persist_new(&allowlist, &agent_dir.0, next).await {
        tracing::error!(error = %e, "mode persist failed");
        bot.answer_callback_query(q.id).text("Failed to save — see bot logs").await?;
        return Ok(());
    }

    // Recompute display state and refresh the menu + toast.
    let (effective, has_override, group_mode, is_group_scope) = {
        let s = allowlist.0.read().expect("allowlist lock poisoned");
        let effective = s.response_mode(chat_id, thread_id);
        let has_override = s.groups().iter().find(|g| g.id == chat_id)
            .map(|g| g.topics.iter().any(|t| t.thread_id == thread_id)).unwrap_or(false);
        let group_mode = s.groups().iter().find(|g| g.id == chat_id).map(|g| g.mode).unwrap_or_default();
        (effective, has_override, group_mode, scope == ModeScope::Group)
    };

    tracing::info!(chat_id, thread_id, ?scope, ?action, "response mode changed");

    let toast = bot.answer_callback_query(q.id).text("Mode updated");
    let (body, kb) = if is_group_scope {
        (group_body(group_mode), group_keyboard(group_mode))
    } else {
        (topic_body(effective, has_override), topic_keyboard(effective, has_override))
    };
    let edit = bot.edit_message_text(message.chat().id, message.id(), body).reply_markup(kb);
    let (edit_result, toast_result) = tokio::join!(edit.send(), toast.send());
    if let Err(e) = edit_result {
        tracing::warn!(error = %e, "failed to edit /mode menu after change");
    }
    toast_result?;
    Ok(())
}
```

- [ ] **Step 6: Run the crate build + module tests**

Run: `devenv shell -- cargo test -p right-bot --lib telegram::mode_command 2>&1 | tail -20`
Expected: PASS, crate compiles (handlers included).

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/mode_command.rs crates/bot/src/telegram/allowlist_commands.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): /mode and /mode_group inline-keyboard handlers"
```

---

## Task 6: Wire commands + callback into the dispatcher

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Add `BotCommand` variants** (after `DenyAll`, before `Usage`)

```rust
    #[command(description = "Set response mode for this topic (menu)")]
    Mode,
    #[command(description = "Set response mode for the whole group (menu)", rename = "mode_group")]
    ModeGroup,
```

- [ ] **Step 2: Add command branches** (in `command_handler`, after the `DenyAll` branch)

```rust
        .branch(dptree::case![BotCommand::Mode].endpoint(super::mode_command::handle_mode))
        .branch(dptree::case![BotCommand::ModeGroup].endpoint(super::mode_command::handle_mode_group));
```

- [ ] **Step 3: Add the callback filter** (in `callback_handler`, before the final `.endpoint(handle_stop_callback)`)

```rust
        .branch(
            dptree::filter(|q: CallbackQuery| {
                q.data.as_deref().is_some_and(|d| {
                    d.starts_with("mode:") || d.starts_with("modegroup:")
                })
            })
            .endpoint(super::mode_command::handle_mode_callback),
        )
```

- [ ] **Step 4: Verify the crate builds and the dispatcher tests pass**

Run: `devenv shell -- cargo test -p right-bot --lib telegram::dispatch 2>&1 | tail -20`
Expected: PASS / compiles. (`handle_mode*` take only deps present in `dptree::deps![…]`: `BotType`, `Message`/`CallbackQuery`, `AllowlistHandle`, `Arc<AgentDir>` — all already registered for the existing model callback.)

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/dispatch.rs
git commit -m "feat(dispatch): register /mode, /mode_group and their callback"
```

---

## Task 7: CLI mirror — `right agent mode` / `mode-group`

**Files:**
- Modify: `crates/right/src/main.rs`

- [ ] **Step 1: Add the subcommand variants** (in `enum AgentCommands`, after `DenyAll`)

```rust
    /// Set the response mode for a topic (or the group default with --group)
    #[command(name = "mode")]
    Mode {
        /// Agent name
        name: String,
        /// Telegram group chat ID
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
        /// Effective thread id (0 = General). Ignored with --group.
        #[arg(long, default_value_t = 0)]
        thread_id: i64,
        /// Set the group-level default instead of a topic
        #[arg(long)]
        group: bool,
        /// One of: addressed | all | clear (clear is topic-only)
        value: String,
    },
```

- [ ] **Step 2: Add the handler arm** (next to `AgentCommands::AllowAll { … } => { … }`)

```rust
            AgentCommands::Mode {
                name,
                chat_id,
                thread_id,
                group,
                value,
            } => {
                let dir = right_config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist::{self, AllowlistState, ResponseMode};
                let mode = match value.as_str() {
                    "addressed" => Some(ResponseMode::Addressed),
                    "all" => Some(ResponseMode::All),
                    "clear" => None,
                    other => return Err(miette::miette!("invalid mode '{other}' (addressed|all|clear)")),
                };
                if group && mode.is_none() {
                    return Err(miette::miette!("`clear` is topic-only; --group needs addressed|all"));
                }
                let applied = allowlist::with_lock(&dir, |d| -> Result<bool, String> {
                    let file = allowlist::read_file(d)?.unwrap_or_default();
                    let mut state = AllowlistState::from_file(file);
                    let ok = if group {
                        state.set_group_mode(chat_id, mode.expect("checked above"))
                    } else if let Some(m) = mode {
                        state.set_topic_mode(chat_id, thread_id, m)
                    } else {
                        state.clear_topic_mode(chat_id, thread_id) || state.is_chat_allowed(chat_id)
                    };
                    if ok {
                        allowlist::write_file_inner(d, &state.to_file())?;
                    }
                    Ok(ok)
                })
                .map_err(|e| miette::miette!("{e}"))?;
                if applied {
                    println!("mode updated for {chat_id}");
                } else {
                    println!("group {chat_id} is not opened — run `right agent allow_all` first");
                }
                Ok(())
            }
```

Note: this mirrors the existing `AllowAll`/`DenyAll` arms' use of `println!` (the established pattern in this file for allowlist subcommands).

- [ ] **Step 3: Verify build + a smoke test**

Run: `devenv shell -- cargo run --bin right -- agent mode --help 2>&1 | tail -20`
Expected: help text lists `--group`, `--thread-id`, and the `value` positional.

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(cli): right agent mode / mode-group allowlist mirror"
```

---

## Task 8: Docs cite-on-touch + final workspace verification

**Files:**
- Modify: `crates/bot/PROMPT_SYSTEM.md` only if mode affects prompt assembly (it does not — no change expected; confirm by search).
- Modify: `docs/architecture/*.md` if an addressing/allowlist satellite exists.

- [ ] **Step 1: Check for a satellite doc to update**

Run: `rg -l "allow_all|is_group_open|addressed|allowlist" docs/architecture ARCHITECTURE.md 2>&1 | head`
If a satellite documents group addressing, add 2-3 sentences: response modes (`addressed` default / `all`), per-topic override, `/mode` + `/mode_group`, and the forum-topic-root fix. Do **not** expand `ARCHITECTURE.md` past its budget — prefer the satellite. If none exists, no doc change is required (commands are self-describing).

- [ ] **Step 2: Run the full workspace test suite (mandatory final gate)**

Run: `devenv shell -- cargo test --workspace 2>&1 | tail -30`
Expected: PASS. Note any pre-existing flakes per memory (cc/invocation pid race, dashboard warn-count) — re-run those isolated if they fail before blaming this change.

- [ ] **Step 3: Clippy on the touched crates**

Run: `devenv shell -- cargo clippy -p right-agent -p right-bot -p right 2>&1 | tail -20`
Expected: no new warnings.

- [ ] **Step 4: Commit any doc changes**

```bash
git add -A
git commit -m "docs: note switchable response modes in architecture satellite" || echo "no doc changes"
```

---

## Self-Review

**Spec coverage:**
- Model & precedence (topic > group > default `Addressed`) → Task 2 `response_mode` + tests.
- `All` only in open group; closed unchanged → Task 3 gating (`group_open` guard) + Task 2 (closed → Addressed).
- Storage `allowlist.yaml` v1→v2, serde defaults, accept v1 or v2 → Task 1.
- Gating in `filter.rs`, `address: None` passes downstream (album/forward path already handles it) → Task 3.
- `/mode`, `/mode_group` inline keyboards mirroring `/model`, trusted-only, RMW persist, edit message → Task 5 + Task 6.
- CLI mirror → Task 7.
- `forum_topic_created` fix → Task 4.
- Upgrade-friendly (hot-reload watcher, no recreation) → Task 1 (backward-compatible parse); no migration code needed.
- Security trade-off (choice A) — implemented as choice A in Task 3; loop guard (`!sender.is_bot`) added as a safety refinement.

**Placeholder scan:** No TBDs; every code step shows full code; teloxide accessors (`forum_topic_created`, `regular_message`, `MaybeInaccessibleMessage::{chat,id}`) verified against teloxide-core 0.13.

**Type consistency:** `ResponseMode { Addressed, All }`, `TopicMode { thread_id, mode }`, methods `response_mode/set_group_mode/set_topic_mode/clear_topic_mode`, callbacks `mode:{addressed,all,clear}` / `modegroup:{addressed,all}`, parse via `parse_callback` — names consistent across Tasks 1-7.
