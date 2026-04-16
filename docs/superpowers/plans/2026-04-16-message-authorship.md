# Message Authorship & Forward Metadata Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Propagate Telegram message authorship, forward origin, and reply-to metadata through the bot pipeline so agents can distinguish who said what.

**Architecture:** Add `MessageAuthor` and `ForwardInfo` types to `attachments.rs`. Thread them through `DebounceMsg` → `InputMessage` → YAML output. Extract metadata from teloxide `Message` in `handler.rs`. Remove raw text shortcut — always emit YAML.

**Tech Stack:** Rust, teloxide (MessageOrigin, User, Chat), chrono

**Spec:** `docs/superpowers/specs/2026-04-16-message-authorship-design.md`

---

### Task 1: Add `MessageAuthor` and `ForwardInfo` types

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs:121-128`

- [ ] **Step 1: Add types above `InputMessage`**

In `crates/bot/src/telegram/attachments.rs`, add before the `InputMessage` struct (line 121):

```rust
/// Telegram user/chat identity for message authorship.
#[derive(Debug, Clone)]
pub struct MessageAuthor {
    pub name: String,
    pub username: Option<String>,
    pub user_id: Option<i64>,
}

/// Forward origin metadata.
#[derive(Debug, Clone)]
pub struct ForwardInfo {
    pub from: MessageAuthor,
    pub date: DateTime<Utc>,
}
```

- [ ] **Step 2: Add new fields to `InputMessage`**

Extend `InputMessage`:

```rust
pub struct InputMessage {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<ResolvedAttachment>,
    pub author: MessageAuthor,
    pub forward_info: Option<ForwardInfo>,
    pub reply_to_id: Option<i32>,
}
```

- [ ] **Step 3: Run `cargo check -p rightclaw-bot`**

Expected: compile errors at every `InputMessage {}` construction site (worker.rs:333, all tests in attachments.rs). This confirms we found all callsites. Do NOT fix them yet.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(bot): add MessageAuthor, ForwardInfo types and extend InputMessage"
```

---

### Task 2: Add `author`, `forward_info`, `reply_to_id` to `DebounceMsg`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:48-54`

- [ ] **Step 1: Extend `DebounceMsg`**

In `crates/bot/src/telegram/worker.rs`, update the struct:

```rust
pub struct DebounceMsg {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<super::attachments::InboundAttachment>,
    pub author: super::attachments::MessageAuthor,
    pub forward_info: Option<super::attachments::ForwardInfo>,
    pub reply_to_id: Option<i32>,
}
```

- [ ] **Step 2: Update `InputMessage` construction in worker (line ~333)**

Where `input_messages.push(super::attachments::InputMessage { ... })`, add the new fields:

```rust
input_messages.push(super::attachments::InputMessage {
    message_id: msg.message_id,
    text: msg.text.clone(),
    timestamp: msg.timestamp,
    attachments: resolved,
    author: msg.author.clone(),
    forward_info: msg.forward_info.clone(),
    reply_to_id: msg.reply_to_id,
});
```

- [ ] **Step 3: Run `cargo check -p rightclaw-bot`**

Expected: compile error at `handler.rs:180` (DebounceMsg construction) and test sites. Do NOT fix handler yet.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): extend DebounceMsg with author, forward_info, reply_to_id"
```

---

### Task 3: Extract metadata in `handler.rs`

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs:136-185`

- [ ] **Step 1: Add metadata extraction before `DebounceMsg` construction**

In `handler.rs`, after `let attachments = ...` (line 143) and before the intercept checks, add:

```rust
use super::attachments::{MessageAuthor, ForwardInfo};
use teloxide::types::MessageOrigin;

// Extract author from sender
let author = match msg.from() {
    Some(user) => MessageAuthor {
        name: user.full_name(),
        username: user.username.as_ref().map(|u| format!("@{u}")),
        user_id: Some(user.id.0 as i64),
    },
    None => MessageAuthor {
        name: msg.chat.title().unwrap_or("unknown").to_owned(),
        username: msg.chat.username().map(|u| format!("@{u}")),
        user_id: None,
    },
};

// Extract forward origin
let forward_info = msg.forward_origin().map(|origin| {
    let (from, date) = match origin {
        MessageOrigin::User { sender_user, date } => (
            MessageAuthor {
                name: sender_user.full_name(),
                username: sender_user.username.as_ref().map(|u| format!("@{u}")),
                user_id: Some(sender_user.id.0 as i64),
            },
            *date,
        ),
        MessageOrigin::HiddenUser { sender_user_name, date } => (
            MessageAuthor {
                name: sender_user_name.clone(),
                username: None,
                user_id: None,
            },
            *date,
        ),
        MessageOrigin::Chat { sender_chat, date, .. } => (
            MessageAuthor {
                name: sender_chat.title().unwrap_or("unknown").to_owned(),
                username: sender_chat.username().map(|u| format!("@{u}")),
                user_id: None,
            },
            *date,
        ),
        MessageOrigin::Channel { chat, date, .. } => (
            MessageAuthor {
                name: chat.title().unwrap_or("unknown").to_owned(),
                username: chat.username().map(|u| format!("@{u}")),
                user_id: None,
            },
            *date,
        ),
    };
    ForwardInfo { from, date }
});

// Extract reply-to message ID
let reply_to_id = msg.reply_to_message().map(|m| m.id.0);
```

- [ ] **Step 2: Update `DebounceMsg` construction**

Change the construction at line ~180:

```rust
let debounce_msg = DebounceMsg {
    message_id: msg.id.0,
    text,
    timestamp: chrono::Utc::now(),
    attachments,
    author,
    forward_info,
    reply_to_id,
};
```

- [ ] **Step 3: Run `cargo check -p rightclaw-bot`**

Expected: compile errors only in test code (attachments.rs tests with old `InputMessage` constructors). Main code should compile.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/handler.rs
git commit -m "feat(bot): extract author, forward_info, reply_to_id from Telegram messages"
```

---

### Task 4: Update `format_cc_input` to emit metadata YAML

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs:130-176`

- [ ] **Step 1: Write failing test for author in YAML**

Add test in the `#[cfg(test)]` module of `attachments.rs`:

```rust
#[test]
fn format_cc_input_includes_author() {
    let ts = DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let msgs = vec![InputMessage {
        message_id: 1,
        text: Some("hello".into()),
        timestamp: ts,
        attachments: vec![],
        author: MessageAuthor {
            name: "Андрей Кузнецов".into(),
            username: Some("@right".into()),
            user_id: Some(12345678),
        },
        forward_info: None,
        reply_to_id: None,
    }];
    let result = format_cc_input(&msgs).unwrap();
    assert!(result.starts_with("messages:\n"), "should be YAML, not raw text");
    assert!(result.contains("    author:\n"));
    assert!(result.contains("      name: \"Андрей Кузнецов\"\n"));
    assert!(result.contains("      username: \"@right\"\n"));
    assert!(result.contains("      user_id: 12345678\n"));
}
```

- [ ] **Step 2: Write failing test for forward_from in YAML**

```rust
#[test]
fn format_cc_input_includes_forward_info() {
    let ts = DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let fwd_date = DateTime::parse_from_rfc3339("2026-04-07T20:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let msgs = vec![InputMessage {
        message_id: 1,
        text: Some("forwarded text".into()),
        timestamp: ts,
        attachments: vec![],
        author: MessageAuthor {
            name: "Андрей Кузнецов".into(),
            username: Some("@right".into()),
            user_id: Some(12345678),
        },
        forward_info: Some(ForwardInfo {
            from: MessageAuthor {
                name: "Миша Петров".into(),
                username: Some("@mishapetrov".into()),
                user_id: Some(12345678),
            },
            date: fwd_date,
        }),
        reply_to_id: None,
    }];
    let result = format_cc_input(&msgs).unwrap();
    assert!(result.contains("    forward_from:\n"));
    assert!(result.contains("      name: \"Миша Петров\"\n"));
    assert!(result.contains("      username: \"@mishapetrov\"\n"));
    assert!(result.contains("      user_id: 12345678\n"));
    assert!(result.contains("    forward_date: \"2026-04-07T20:00:00Z\"\n"));
}
```

- [ ] **Step 3: Write failing test for reply_to_id**

```rust
#[test]
fn format_cc_input_includes_reply_to_id() {
    let ts = Utc::now();
    let msgs = vec![InputMessage {
        message_id: 5,
        text: Some("replying".into()),
        timestamp: ts,
        attachments: vec![],
        author: MessageAuthor {
            name: "Андрей".into(),
            username: None,
            user_id: Some(12345678),
        },
        forward_info: None,
        reply_to_id: Some(3),
    }];
    let result = format_cc_input(&msgs).unwrap();
    assert!(result.contains("    reply_to_id: 3\n"));
}
```

- [ ] **Step 4: Write failing test for hidden user forward (no username, no user_id)**

```rust
#[test]
fn format_cc_input_hidden_user_forward_omits_missing_fields() {
    let ts = Utc::now();
    let fwd_date = Utc::now();
    let msgs = vec![InputMessage {
        message_id: 1,
        text: Some("secret".into()),
        timestamp: ts,
        attachments: vec![],
        author: MessageAuthor {
            name: "Андрей".into(),
            username: None,
            user_id: Some(12345678),
        },
        forward_info: Some(ForwardInfo {
            from: MessageAuthor {
                name: "Hidden Person".into(),
                username: None,
                user_id: None,
            },
            date: fwd_date,
        }),
        reply_to_id: None,
    }];
    let result = format_cc_input(&msgs).unwrap();
    assert!(result.contains("      name: \"Hidden Person\"\n"));
    // username and user_id lines should NOT be present under forward_from
    let fwd_idx = result.find("    forward_from:\n").unwrap();
    let after_fwd = &result[fwd_idx..];
    let fwd_block_end = after_fwd.find("    forward_date:").unwrap();
    let fwd_block = &after_fwd[..fwd_block_end];
    assert!(!fwd_block.contains("username:"));
    assert!(!fwd_block.contains("user_id:"));
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot -- format_cc_input 2>&1 | head -40`

Expected: compilation errors because `InputMessage` now requires new fields and `format_cc_input` doesn't emit them.

- [ ] **Step 6: Update `format_cc_input` implementation**

Replace the function body in `attachments.rs`:

```rust
pub fn format_cc_input(msgs: &[InputMessage]) -> Option<String> {
    if msgs.is_empty() {
        return None;
    }

    // Check if all messages have no text and no attachments
    if msgs.iter().all(|m| m.text.is_none() && m.attachments.is_empty()) {
        return None;
    }

    use std::fmt::Write;
    let mut out = String::with_capacity(512);
    out.push_str("messages:\n");
    for m in msgs {
        writeln!(out, "  - id: {}", m.message_id).expect("infallible");
        writeln!(out, "    ts: \"{}\"", m.timestamp.format("%Y-%m-%dT%H:%M:%SZ"))
            .expect("infallible");

        // Author block (always present)
        out.push_str("    author:\n");
        writeln!(out, "      name: \"{}\"", yaml_escape_string(&m.author.name))
            .expect("infallible");
        if let Some(ref username) = m.author.username {
            writeln!(out, "      username: \"{}\"", yaml_escape_string(username))
                .expect("infallible");
        }
        if let Some(user_id) = m.author.user_id {
            writeln!(out, "      user_id: {user_id}").expect("infallible");
        }

        // Forward info (only if forwarded)
        if let Some(ref fwd) = m.forward_info {
            out.push_str("    forward_from:\n");
            writeln!(out, "      name: \"{}\"", yaml_escape_string(&fwd.from.name))
                .expect("infallible");
            if let Some(ref username) = fwd.from.username {
                writeln!(out, "      username: \"{}\"", yaml_escape_string(username))
                    .expect("infallible");
            }
            if let Some(user_id) = fwd.from.user_id {
                writeln!(out, "      user_id: {user_id}").expect("infallible");
            }
            writeln!(out, "    forward_date: \"{}\"", fwd.date.format("%Y-%m-%dT%H:%M:%SZ"))
                .expect("infallible");
        }

        // Reply-to (only if reply)
        if let Some(reply_id) = m.reply_to_id {
            writeln!(out, "    reply_to_id: {reply_id}").expect("infallible");
        }

        // Text
        if let Some(ref text) = m.text {
            let escaped = yaml_escape_string(text);
            writeln!(out, "    text: \"{escaped}\"").expect("infallible");
        }

        // Attachments
        if !m.attachments.is_empty() {
            out.push_str("    attachments:\n");
            for att in &m.attachments {
                writeln!(out, "      - type: {}", att.kind.as_str()).expect("infallible");
                writeln!(out, "        path: {}", att.path.display()).expect("infallible");
                writeln!(out, "        mime_type: {}", att.mime_type).expect("infallible");
                if let Some(ref fname) = att.filename {
                    let escaped = yaml_escape_string(fname);
                    writeln!(out, "        filename: \"{escaped}\"").expect("infallible");
                }
            }
        }
    }
    Some(out)
}
```

- [ ] **Step 7: Run new tests to verify they pass**

Run: `cargo test -p rightclaw-bot -- format_cc_input_includes 2>&1`

Expected: all 4 new tests PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(bot): emit author, forward_from, reply_to_id in YAML input format"
```

---

### Task 5: Fix existing tests

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs` (test module)

All existing `InputMessage` constructors in tests need the 3 new fields. Use a helper to reduce boilerplate.

- [ ] **Step 1: Add test helper**

At the top of the `#[cfg(test)]` module:

```rust
fn test_author() -> MessageAuthor {
    MessageAuthor {
        name: "Test User".into(),
        username: None,
        user_id: Some(1),
    }
}
```

- [ ] **Step 2: Update all existing `InputMessage` constructors in tests**

Add to every `InputMessage { ... }` in tests:

```rust
author: test_author(),
forward_info: None,
reply_to_id: None,
```

Affected tests:
- `format_cc_input_single_text_returns_plain_string` — also change assertion: no longer `assert_eq!(result, "hello world")`, now assert YAML containing `text: "hello world"` and `author:`.
- `format_cc_input_single_no_text_no_attachments_returns_none` — just add fields.
- `format_cc_input_multiple_messages_returns_yaml` — just add fields.
- `format_cc_input_with_attachments_returns_yaml` — just add fields.
- `format_cc_input_document_with_filename` — just add fields.
- `format_cc_input_text_with_special_chars_escaped` — just add fields.

- [ ] **Step 3: Update `format_cc_input_single_text_returns_plain_string` assertion**

The test name is now misleading. Rename to `format_cc_input_single_text_returns_yaml` and update:

```rust
#[test]
fn format_cc_input_single_text_returns_yaml() {
    let msgs = vec![InputMessage {
        message_id: 1,
        text: Some("hello world".into()),
        timestamp: DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        attachments: vec![],
        author: test_author(),
        forward_info: None,
        reply_to_id: None,
    }];
    let result = format_cc_input(&msgs).unwrap();
    assert!(result.starts_with("messages:\n"));
    assert!(result.contains("    text: \"hello world\"\n"));
    assert!(result.contains("    author:\n"));
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test -p rightclaw-bot -- format_cc_input 2>&1`

Expected: all tests PASS (old + new).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "test(bot): update existing format_cc_input tests for new metadata fields"
```

---

### Task 6: Build and verify

**Files:** None (verification only)

- [ ] **Step 1: Run full workspace build**

Run: `cargo build --workspace`

Expected: clean build, no warnings related to our changes.

- [ ] **Step 2: Run full bot test suite**

Run: `cargo test -p rightclaw-bot 2>&1`

Expected: all tests pass.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings 2>&1`

Expected: no warnings.
