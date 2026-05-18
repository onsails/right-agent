# Unauthorized DM Ingress Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove unauthorized Telegram DMs never pass the existing allowlist gate, and remove private-message content from pre-filter dispatcher logs.

**Architecture:** Keep `telegram::filter::make_routing_filter` as the single ingress authorization boundary. Add regression tests for private text and media/caption messages, then replace the pre-filter dispatcher text preview with a content-free metadata helper.

**Tech Stack:** Rust 2024, teloxide `Message`/`ChatKind`, existing `right-bot` test modules, `devenv shell -- cargo test`.

---

## File Structure

- `crates/bot/src/telegram/filter.rs`
  - Existing routing filter and unit tests.
  - Add private-chat fixtures and regression tests only.
- `crates/bot/src/telegram/dispatch.rs`
  - Existing teloxide dispatcher wiring and pre-filter logging.
  - Add a small private `PreFilterLogMeta` helper and tests.
  - Replace the pre-filter `text_preview` logging with content-free metadata.
- `docs/architecture/sessions.md`
  - Read during implementation because this touches Telegram ingress.
  - Expected to stay unchanged: it already documents that routed DM archive happens only after routing checks.

## Verification Cadence

- Baseline: run targeted `right-bot` filter and dispatch tests before edits.
- Intermediate: run the narrow test target after each behavior slice.
- Final: run `devenv shell -- cargo test --workspace` before claiming implementation complete.

---

### Task 1: Baseline and Context Check

**Files:**
- Read: `docs/superpowers/specs/2026-05-19-unauthorized-dm-ingress-design.md`
- Read: `docs/architecture/sessions.md`
- Read: `crates/bot/src/telegram/filter.rs`
- Read: `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Confirm a clean or understood worktree**

Run:

```sh
devenv shell -- git status --short
```

Expected: either no output, or only unrelated user changes that are not touched by this plan.

- [ ] **Step 2: Re-read the approved design**

Run:

```sh
devenv shell -- sed -n '1,220p' docs/superpowers/specs/2026-05-19-unauthorized-dm-ingress-design.md
```

Expected: design goals cover unauthorized DM regression tests and content-free pre-filter logging.

- [ ] **Step 3: Re-read the sessions architecture doc**

Run:

```sh
devenv shell -- sed -n '1,90p' docs/architecture/sessions.md
```

Expected: it still says routed DMs are archived only after auth-code/MCP-token intercepts and routing checks have allowed the message through.

- [ ] **Step 4: Run targeted baseline routing tests**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::filter
```

Expected: PASS. Record any pre-existing failure before editing.

- [ ] **Step 5: Run targeted baseline dispatcher tests**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::dispatch
```

Expected: PASS. Record any pre-existing failure before editing.

---

### Task 2: Add Unauthorized DM Routing Regression Tests

**Files:**
- Modify: `crates/bot/src/telegram/filter.rs`

- [ ] **Step 1: Add private-message fixtures and routing tests**

In `crates/bot/src/telegram/filter.rs`, inside the existing `#[cfg(test)] mod tests`, add this code after `group_msg_with_media_group`:

```rust
fn private_text_msg(chat_id: i64, sender_id: i64, text: &str) -> teloxide::types::Message {
    serde_json::from_value(serde_json::json!({
        "message_id": 1,
        "date": 0,
        "chat": {"id": chat_id, "type": "private", "first_name": "User"},
        "from": {"id": sender_id, "is_bot": false, "first_name": "User"},
        "text": text
    }))
    .unwrap()
}

fn private_photo_caption_msg(
    chat_id: i64,
    sender_id: i64,
    caption: &str,
) -> teloxide::types::Message {
    serde_json::from_value(serde_json::json!({
        "message_id": 2,
        "date": 0,
        "chat": {"id": chat_id, "type": "private", "first_name": "User"},
        "from": {"id": sender_id, "is_bot": false, "first_name": "User"},
        "caption": caption,
        "photo": [{
            "file_id": "AgAD-private",
            "file_unique_id": "private-photo",
            "width": 1,
            "height": 1
        }]
    }))
    .unwrap()
}

#[test]
fn untrusted_private_text_message_is_dropped() {
    let identity = BotIdentity {
        username: "rightaww_bot".into(),
        user_id: 999,
    };
    let sender_id = 42;
    let msg = private_text_msg(sender_id, sender_id, "spam text");
    let allowlist = allowlist_with(vec![], vec![]);

    let f = make_routing_filter(allowlist, identity);

    assert!(f(msg).is_none());
}

#[test]
fn untrusted_private_media_caption_message_is_dropped() {
    let identity = BotIdentity {
        username: "rightaww_bot".into(),
        user_id: 999,
    };
    let sender_id = 42;
    let msg = private_photo_caption_msg(sender_id, sender_id, "spam caption");
    let allowlist = allowlist_with(vec![], vec![]);

    let f = make_routing_filter(allowlist, identity);

    assert!(f(msg).is_none());
}

#[test]
fn trusted_private_text_routes_as_direct_message() {
    let identity = BotIdentity {
        username: "rightaww_bot".into(),
        user_id: 999,
    };
    let sender_id = 42;
    let msg = private_text_msg(sender_id, sender_id, "hello");
    let allowlist = allowlist_with(vec![sender_id], vec![]);

    let f = make_routing_filter(allowlist, identity);
    let decision = f(msg).expect("trusted private text should route");

    assert_eq!(decision.address, Some(AddressKind::DirectMessage));
    assert!(decision.sender_trusted);
    assert!(!decision.group_open);
}

#[test]
fn trusted_private_media_caption_routes_as_direct_message() {
    let identity = BotIdentity {
        username: "rightaww_bot".into(),
        user_id: 999,
    };
    let sender_id = 42;
    let msg = private_photo_caption_msg(sender_id, sender_id, "hello with media");
    let allowlist = allowlist_with(vec![sender_id], vec![]);

    let f = make_routing_filter(allowlist, identity);
    let decision = f(msg).expect("trusted private media should route");

    assert_eq!(decision.address, Some(AddressKind::DirectMessage));
    assert!(decision.sender_trusted);
    assert!(!decision.group_open);
}
```

- [ ] **Step 2: Run the routing regression tests**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::filter
```

Expected: PASS. These codify existing behavior, so they are expected to pass before any implementation change.

- [ ] **Step 3: Commit the routing regression tests**

Run:

```sh
devenv shell -- git add crates/bot/src/telegram/filter.rs
devenv shell -- git commit -m "test(bot): verify unauthorized dm routing gate"
```

Expected: commit succeeds and includes only `crates/bot/src/telegram/filter.rs`.

---

### Task 3: Add Failing Dispatcher Metadata Test

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Add a failing metadata test**

In `crates/bot/src/telegram/dispatch.rs`, inside the existing `#[cfg(test)] mod tests`, add this test after `dispatcher_builds_without_panic`:

```rust
#[test]
fn pre_filter_log_meta_omits_private_text_and_caption_content() {
    let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
        "message_id": 10,
        "date": 0,
        "chat": {"id": 42, "type": "private", "first_name": "Spammer"},
        "from": {"id": 42, "is_bot": false, "first_name": "Spammer"},
        "caption": "SPAM-CAPTION-SHOULD-NOT-LOG",
        "caption_entities": [{
            "type": "bold",
            "offset": 0,
            "length": 4
        }],
        "document": {
            "file_id": "BAAD-private",
            "file_unique_id": "private-doc",
            "file_name": "spam.pdf",
            "mime_type": "application/pdf",
            "file_size": 1024
        }
    }))
    .unwrap();

    let meta = pre_filter_log_meta(&msg);

    assert_eq!(meta.chat_id, 42);
    assert_eq!(meta.chat_kind, "private");
    assert!(!meta.has_text);
    assert!(meta.has_caption);
    assert_eq!(meta.attachment_count, 1);
    assert_eq!(meta.entity_count, 1);

    let rendered = format!("{meta:?}");
    assert!(!rendered.contains("SPAM-CAPTION-SHOULD-NOT-LOG"));
    assert!(!rendered.contains("spam.pdf"));
}
```

- [ ] **Step 2: Run the dispatcher test and verify it fails**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::dispatch::tests::pre_filter_log_meta_omits_private_text_and_caption_content
```

Expected: FAIL with a compile error containing `cannot find function pre_filter_log_meta`.

---

### Task 4: Implement Content-Free Pre-Filter Logging

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Add explicit Teloxide type imports**

In `crates/bot/src/telegram/dispatch.rs`, replace this import:

```rust
use teloxide::RequestError;
```

with:

```rust
use teloxide::RequestError;
use teloxide::types::{ChatKind, Message, PublicChatKind};
```

- [ ] **Step 2: Add the metadata helper**

In `crates/bot/src/telegram/dispatch.rs`, add this code after the `BotCommand` enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreFilterLogMeta {
    chat_id: i64,
    chat_kind: &'static str,
    has_text: bool,
    has_caption: bool,
    attachment_count: usize,
    entity_count: usize,
}

fn pre_filter_log_meta(msg: &Message) -> PreFilterLogMeta {
    PreFilterLogMeta {
        chat_id: msg.chat.id.0,
        chat_kind: chat_kind_label(&msg.chat.kind),
        has_text: msg.text().is_some(),
        has_caption: msg.caption().is_some(),
        attachment_count: super::attachments::extract_attachments(msg).len(),
        entity_count: message_entity_count(msg),
    }
}

fn chat_kind_label(kind: &ChatKind) -> &'static str {
    match kind {
        ChatKind::Private(_) => "private",
        ChatKind::Public(public) => match &public.kind {
            PublicChatKind::Channel(_) => "channel",
            PublicChatKind::Group => "group",
            PublicChatKind::Supergroup(_) => "supergroup",
        },
    }
}

fn message_entity_count(msg: &Message) -> usize {
    let text_entities = msg.entities().map_or(0, |entities| entities.len());
    let caption_entities = msg
        .caption_entities()
        .map_or(0, |entities| entities.len());

    text_entities + caption_entities
}
```

- [ ] **Step 3: Replace the pre-filter log body**

In `crates/bot/src/telegram/dispatch.rs`, replace the current pre-filter `.inspect` body:

```rust
.inspect(move |msg: Message| {
    let text_preview = msg.text().or(msg.caption()).map(|t| {
        let trimmed: String = t.chars().take(80).collect();
        trimmed
    });
    let entities = msg.entities().map(|e| e.len()).unwrap_or(0);
    tracing::info!(
        chat_id = msg.chat.id.0,
        ?text_preview,
        entities,
        "message update received by dispatcher"
    );
    super::archive::archive_seen_group_message(
        &archive_agent_dir.0,
        archive_identity.as_ref(),
        &msg,
    );
})
```

with:

```rust
.inspect(move |msg: Message| {
    let meta = pre_filter_log_meta(&msg);
    tracing::info!(
        chat_id = meta.chat_id,
        chat_kind = meta.chat_kind,
        has_text = meta.has_text,
        has_caption = meta.has_caption,
        attachment_count = meta.attachment_count,
        entity_count = meta.entity_count,
        "message update received by dispatcher"
    );
    super::archive::archive_seen_group_message(
        &archive_agent_dir.0,
        archive_identity.as_ref(),
        &msg,
    );
})
```

- [ ] **Step 4: Run the focused dispatcher metadata test**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::dispatch::tests::pre_filter_log_meta_omits_private_text_and_caption_content
```

Expected: PASS.

- [ ] **Step 5: Run all dispatcher tests**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::dispatch
```

Expected: PASS.

- [ ] **Step 6: Commit the dispatcher logging change**

Run:

```sh
devenv shell -- git add crates/bot/src/telegram/dispatch.rs
devenv shell -- git commit -m "fix(bot): redact pre-filter message logs"
```

Expected: commit succeeds and includes only `crates/bot/src/telegram/dispatch.rs`.

---

### Task 5: Final Verification and Documentation Check

**Files:**
- Read: `docs/architecture/sessions.md`
- Inspect: `crates/bot/src/telegram/filter.rs`
- Inspect: `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Run targeted bot tests after both commits**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::filter
devenv shell -- cargo test -p right-bot telegram::dispatch
```

Expected: both commands PASS.

- [ ] **Step 2: Confirm sessions doc still matches the implementation**

Run:

```sh
devenv shell -- sed -n '35,70p' docs/architecture/sessions.md
```

Expected: the doc still accurately says routed DMs are archived only after auth-code/MCP-token intercepts and routing checks allow the message through. No documentation edit is needed because the implementation preserves that contract.

- [ ] **Step 3: Inspect the final diff**

Run:

```sh
devenv shell -- git status --short
devenv shell -- git log --oneline -3
```

Expected: worktree is clean, and the newest implementation commits are:

```text
fix(bot): redact pre-filter message logs
test(bot): verify unauthorized dm routing gate
```

- [ ] **Step 4: Run final workspace verification**

Run:

```sh
devenv shell -- cargo test --workspace
```

Expected: PASS. This final full workspace test is mandatory before reporting implementation complete.
