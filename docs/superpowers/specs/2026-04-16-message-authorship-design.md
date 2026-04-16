# Message Authorship & Forward Metadata

**Date:** 2026-04-16
**Status:** Approved

## Problem

When a user forwards messages from another person to the Telegram bot, the agent sees all messages as coming from the same sender. There is no way to distinguish:
- Owner's messages vs forwarded messages from others
- Who originally wrote a forwarded message
- Which message is a reply to which

This causes the agent to misattribute statements (e.g., treating someone else's advice as the owner's own thought).

## Solution

Propagate Telegram message metadata (author, forward origin, reply-to) through the entire pipeline and expose it to the agent in structured YAML.

## Data Structures

New types in `attachments.rs`:

```rust
pub struct MessageAuthor {
    pub name: String,              // first_name + " " + last_name, trimmed
    pub username: Option<String>,  // "@username"
    pub user_id: Option<i64>,      // telegram user id
}

pub struct ForwardInfo {
    pub from: MessageAuthor,       // original author
    pub date: DateTime<Utc>,       // original send date
}
```

Extended `DebounceMsg` (`worker.rs`):

```rust
pub struct DebounceMsg {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<InboundAttachment>,
    pub author: MessageAuthor,             // NEW
    pub forward_info: Option<ForwardInfo>, // NEW
    pub reply_to_id: Option<i32>,          // NEW
}
```

Extended `InputMessage` (`attachments.rs`):

```rust
pub struct InputMessage {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<ResolvedAttachment>,
    pub author: MessageAuthor,             // NEW
    pub forward_info: Option<ForwardInfo>, // NEW
    pub reply_to_id: Option<i32>,          // NEW
}
```

## YAML Format

All messages (including single text-only messages) are now formatted as YAML. The previous raw text shortcut is removed.

```yaml
messages:
  - id: 123
    ts: "2026-04-16T11:45:37Z"
    author:
      name: "Миша Петров"
      username: "@mishapetrov"
      user_id: 12345678
    forward_from:
      name: "Вася Иванов"
      user_id: 87654321
    forward_date: "2026-04-15T20:00:00Z"
    reply_to_id: 119
    text: "ложиться в 5 утра"
    attachments:
      - type: photo
        path: /sandbox/inbox/photo_123.jpg
        mime_type: image/jpeg
```

### Field rules

| Field | Presence |
|-------|----------|
| `author` | Always |
| `author.username` | Omitted if unavailable |
| `author.user_id` | Omitted if unavailable (channel origins) |
| `forward_from` | Only if message is forwarded |
| `forward_date` | Only if message is forwarded |
| `reply_to_id` | Only if message is a reply |
| `text` | Omitted if no text (pure attachment) |
| `attachments` | Omitted if no attachments |

## Metadata Extraction

In `handler.rs`, before creating `DebounceMsg`:

### Author (from `msg.from()`)

- Normal messages/groups: `msg.from()` returns `User` — extract name, username, user_id.
- Channel posts: `msg.from()` is None — fall back to `msg.chat.title()` and `msg.chat.username()`, user_id = None.

### Forward info (from `msg.forward_origin()`)

Map all four `MessageOrigin` variants:

| Variant | name | username | user_id |
|---------|------|----------|---------|
| `User` | first + last name | @username | user.id |
| `HiddenUser` | sender_user_name | None | None |
| `Chat` | chat.title | @chat.username | None |
| `Channel` | chat.title | @chat.username | None |

All variants provide `date` (original send time).

### Reply (from `msg.reply_to_message()`)

Extract `message_id` only. Full reply content retrieval deferred to future MCP tool.

## Breaking Changes

- `format_cc_input` no longer returns raw text for single messages — always YAML. The agent parses via LLM, so this is transparent.
- `DebounceMsg` and `InputMessage` gain 3 required fields — all construction sites must be updated.

## What Does NOT Change

- `worker.rs` debounce logic
- `worker.rs` recall query (uses `first_text` from `batch.first().text`, not formatted input)
- `worker.rs` session label (uses `first_text`)
- Attachment download/upload pipeline
- `extract_attachments()`
- Reply parsing / outbound attachments
- Cron/delivery invocations (no Telegram messages)

## Edge Cases

- **Forward own message:** `author` = you (sender), `forward_from` = also you (original). Agent infers from `forward_date` that it's an old message.
- **Hidden forward:** `HiddenUser` — name only, no id/username. Telegram privacy restriction.
- **Reply to bot message:** `reply_to_id` points to bot's message_id. Agent won't see text (not in batch). Future MCP tool will resolve this.
- **Bot/system messages in groups:** `msg.from()` may be a bot. Displayed as-is.

## Testing

### Updated tests
- `format_cc_input_single_text_returns_plain_string` → now returns YAML with author

### New tests
- Single message with author → YAML with author block
- Message with forward_from → includes `forward_from` + `forward_date`
- Message with reply_to_id → includes `reply_to_id`
- HiddenUser forward → username and user_id absent
- Channel forward → name = chat title, user_id absent
- Multiple messages with mixed authors → correct per-message attribution
