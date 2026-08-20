# Telegram Forward Admission and Reply-To Attachments Design

**Date:** 2026-05-01
**Status:** Draft

## Problem

Two bugs prevent agents from seeing forwarded content shared in groups.

**Bug A — single forwards dropped at the routing filter.**
`crates/bot/src/telegram/filter.rs:55-57` requires every group message to be
either explicitly addressed (`@mention` / reply-to-bot / `/cmd`) or part of a
`media_group_id` album. A Telegram forward operation does NOT produce a shared
`media_group_id` — only original albums preserve theirs through forwarding. So
each forwarded message arrives as a separate `Message` update with no shared
identifier, and a forward without `@bot` in its caption is dropped silently.

Concrete reproduction from `~/.right/logs/him.log.2026-05-01`:

```
10:31:03.177  text "@rightaww_bot наш пруф..."   entities=1   ← routed (handle_message)
10:31:03.228  text=None                          entities=0   ← forward edf.pdf
                                                                  no handle_message log
                                                                  → DROPPED by filter
```

The forward carries the original sender's caption (`Votre document edf.pdf`),
which contains no bot mention. The user's `@bot` text arrived 51 ms before the
forward but they live in separate `Message` updates with no shared key, so the
filter has no signal to admit the second one.

**Bug B — reply-to attachments not extracted.**
When a user replies to a message with an `@bot` mention, `handler.rs:274-287`
populates `ReplyToBody { author, text }` from the replied-to message — but
not its attachments. So when the user types `@bot вот этот док` as a reply
to a forwarded PDF, the agent sees only the original caption text, not the
PDF itself.

## Goal

When a user shares attachments with the bot via either pattern below, the
agent must receive the actual files in the same logical CC turn:

1. **Comment + forwards.** User types `@bot ...` text first, then forwards N
   messages.
2. **Reply with mention.** User replies to any prior message (forward or not,
   with attachment or not) carrying `@bot` in the reply text.

## Non-goals

- Recovering forwards that arrived **before** any addressing message and were
  never replied to. Once the worker debounce closes on a lone-forward batch,
  the message is dropped — by design. The user can recover it via the reply
  pattern (Bug B fix) if they want to reference it later.
- Following `reply_to_message` chains transitively. One hop only.
- Aggregating forwards across long pauses or across CC invocations. The same
  rules used for `media_group_id` apply: one debounce window, one CC turn.
- Admitting peer-to-peer forwards in open groups that have no addressing
  context at all. Those continue to be dropped by the worker post-debounce
  `batch_is_addressed` gate.

## Approach

Two independent changes that compose naturally:

### Fix A — admit forwards through the filter

`crates/bot/src/telegram/filter.rs:55-57`:

```rust
// before
if addressed.is_none() && msg.media_group_id().is_none() {
    return None;
}

// after
if addressed.is_none()
    && msg.media_group_id().is_none()
    && msg.forward_origin().is_none()
{
    return None;
}
```

Intent verification is delegated to the existing post-debounce
`batch_is_addressed` gate (`worker.rs:404-411`), exactly as media-group
siblings already do.

**Why no new state is required.**
- Telegram clients send "comment + forwards" as a tight burst. In the
  reproduction log the gap between the comment and the first forward is
  **51 ms**.
- The worker's rolling idle debounce is **`DEBOUNCE_MS = 500`** with no hard
  cap. Each new arrival resets the timer; the burst stays in one batch.
- After the window closes, `batch_is_addressed` returns `true` if any
  message in the batch was explicitly addressed. The comment qualifies; the
  whole batch — comment plus forwards — is processed in one CC turn.
- Lone forwards with no addressed neighbor → `batch_is_addressed = false` →
  silent drop. Downloads happen in the worker pipeline **after** the gate,
  so this costs only a debounce cycle, not bandwidth.

**What this gives up.**
Forwards arriving after the comment's debounce already closed (user paused
1+ seconds between typing `@bot` and starting to forward, or worker is busy
running CC for the previous batch) form a lone batch and drop. Fix B covers
this case via reply.

### Fix B — extract attachments from `reply_to_message`

Extend `ReplyToBody` (`crates/bot/src/telegram/attachments.rs:415`) to carry
attachments resolved alongside the primary message's attachments:

```rust
pub struct ReplyToBody {
    pub author: MessageAuthor,
    pub text: Option<String>,
    pub attachments: Vec<ResolvedAttachment>,  // NEW
}
```

Because attachment resolution happens in the worker (download + sandbox
upload), the debounce-stage representation needs unresolved inbound items.
Add a parallel field on `DebounceMsg`:

```rust
pub struct DebounceMsg {
    // ...existing fields...
    pub reply_to_attachments: Vec<InboundAttachment>,  // NEW
}
```

In `handler.rs:274-287`, when the reply target is a non-bot message, also
call `extract_attachments(reply_to_message)` and store the result in
`reply_to_attachments`. `extract_attachments` already accepts any `Message`
and is unaware of message origin, so no changes there.

In `worker.rs`, after the debounce closes and primary attachments are
downloaded, run `download_attachments` once more for
`reply_to_attachments` and splice the result into the `ReplyToBody`
constructed for `InputMessage`. Same code path, same `tmp/inbox` →
`/sandbox/inbox/` flow, same STT for voice/video_note.

In `format_cc_input` (`attachments.rs:534-553`), extend the `reply_to:`
YAML block to emit an `attachments:` list when non-empty, matching the
top-level format.

**Edge cases.**
- Reply to bot's own message → `reply_to_body` already returns `None` at
  `handler.rs:276`. Sustains. No attachment extraction in that branch.
- Reply to old message → `file_id` is valid as long as Telegram still
  hosts the file; in practice indefinitely.
- Replied-to message has voice/video_note → STT runs; the transcript
  is prepended to the reply's quoted text using the same Russian markers
  as for current-message voice. Consistent UX.
- Replied-to message has oversized attachment → existing 20 MB
  Telegram-download skip+notify path applies.
- Dedup is not attempted. If the user already sent the same file earlier,
  it is downloaded again. Agent infers from session context.

## Architecture

### Filter decision tree

```
Group message arrives, sender allowed
              │
       ┌──────┴──────┐
       ▼             ▼
   addressed?    addressed?
   = Some        = None
       │             │
       ▼             ▼
   [ADMIT]   ┌────────────────────────────────┐
             │ media_group_id.is_some()       │ → ADMIT (existing)
             │ forward_origin.is_some()       │ → ADMIT (NEW)
             │ neither                        │ → DROP
             └────────────────────────────────┘
```

Worker logic unchanged downstream: rolling 500 ms debounce → primary
download → reply-to download → `batch_is_addressed` gate → CC.

### Reply-to download flow

```
handler.rs builds DebounceMsg:
  attachments          = extract_attachments(msg)
  reply_to_attachments = msg.reply_to_message().map(extract_attachments)
                                                       .unwrap_or_default()
  reply_to_body        = Some(ReplyToBody { author, text, attachments: [] })
                                                                ↓
worker.rs (post-debounce, pre-CC):
  resolved_primary  = download_attachments(msg.attachments)
  resolved_replyto  = download_attachments(msg.reply_to_attachments)
                                                                ↓
  InputMessage {
    attachments: resolved_primary,
    reply_to_body: Some(ReplyToBody { ..., attachments: resolved_replyto }),
  }
                                                                ↓
format_cc_input emits:
  attachments: [...]
  reply_to:
    author: ...
    text:   ...
    attachments: [...]    ← NEW block
```

## Components

| File | Change |
|---|---|
| `crates/bot/src/telegram/filter.rs` | Add `&& msg.forward_origin().is_none()` to the drop predicate. |
| `crates/bot/src/telegram/attachments.rs` | `ReplyToBody`: add `attachments: Vec<ResolvedAttachment>`. `format_cc_input`: emit `attachments:` under `reply_to:` when non-empty. |
| `crates/bot/src/telegram/worker.rs` | Add `reply_to_attachments: Vec<InboundAttachment>` to `DebounceMsg`. Run `download_attachments` for it post-debounce; populate `ReplyToBody.attachments` on `InputMessage`. |
| `crates/bot/src/telegram/handler.rs` | When building `reply_to_body`, also extract attachments via `extract_attachments(reply_target)` and stash into `DebounceMsg.reply_to_attachments`. |

The two fixes are independent — A can ship without B and vice-versa, but
both are wanted.

## Testing

### `filter.rs` unit tests

- `forward_origin_passes_in_open_group` — mirrors the existing
  `media_group_sibling_without_mention_passes_for_open_group`. A forward
  without caption mention from a trusted sender in an open group must
  return `Some(_)` from the filter.
- `forward_origin_dropped_for_untrusted_sender_and_closed_group` —
  symmetric to existing media-group case.
- `forward_with_caption_mention_routes_as_addressed` — caption-bound
  `@mention` still wins; `address` is `Some(GroupMentionText)`, not just
  "admitted by forward gate".

### `worker.rs` unit tests

- `lone_forward_batch_dropped_by_addressedness_gate` — push a single
  `DebounceMsg` with `address: None`, `forward_origin` set; confirm
  `batch_is_addressed` returns `false`.
- `forward_batched_with_addressed_comment` — push addressed text, then a
  forward within 500 ms; confirm both end up in one batch and
  `batch_is_addressed` returns `true`.

### Reply-to attachments

- Unit test on `format_cc_input` — `ReplyToBody { attachments: [doc] }`
  emits the expected `reply_to:` → `attachments:` YAML block.
- Integration smoke (manual): reply to a forwarded PDF with
  `@bot вот этот док` → agent sees the file at
  `/sandbox/inbox/document_<id>_<idx>.pdf`.

### Manual reproduction

Replay the screenshot scenario in a real chat:
1. `@bot вот доки` followed by 2–3 forwards in burst → agent sees all
   attachments in one CC turn.
2. Forward a doc cold (no preceding comment), then reply to it with
   `@bot вот этот` → agent sees the doc via `reply_to.attachments`.

## Migration / upgrade path

No schema changes, no sandbox impact, no agent-side configuration.
Already-deployed agents pick up the new behavior on next bot restart via
`right restart <agent>`.

## Open questions

None. Window-based admission was considered and rejected during
brainstorming in favor of leveraging the existing 500 ms worker debounce
and the `batch_is_addressed` gate that media groups already use.
