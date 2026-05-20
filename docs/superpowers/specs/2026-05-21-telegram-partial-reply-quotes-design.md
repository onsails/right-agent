# Telegram Partial Reply Quotes

## Problem

Telegram lets a user reply to a message while quoting only a selected fragment
of that message. Teloxide exposes that fragment as `Message::quote()`, but the
bot currently ignores it.

The current prompt shape preserves `reply_to_id` for replies. It may also
include a `reply_to:` block for replies to non-bot messages, where the full
replied-to text and attachments are useful because they may not be in the
Claude session. For replies to the bot's own message, the bot intentionally
does not duplicate the full text, assuming the message is already in session
history.

That loses the user's selected fragment. If the user replies to a long bot
message and asks "what do you mean here?", the agent sees only the new text and
`reply_to_id`, not the specific phrase the user highlighted.

## Goals

- Preserve Telegram's partial quote text in the Claude prompt.
- Keep full-message reply behavior unchanged.
- Avoid archive/session lookup and avoid duplicating full bot replies.
- Make the selected quote explicit and separate from `reply_to.text`.
- Add focused regression coverage for prompt formatting and pass-through.

## Non-Goals

- No attempt to resolve full bot messages from archives or Claude history.
- No synthesized quote for normal full-message replies.
- No use of Telegram quote position or entity metadata in the prompt.
- No outbound Telegram reply behavior changes.
- No changes to attachment download, STT, or media-group handling.

## Decision

Add a minimal inbound quote field that carries only `TextQuote.text`:

```yaml
messages:
  - id: 123
    text: "что тут имеешь в виду?"
    reply_to_id: 122
    quoted_text: "Этот duplicate апостиля прилетает уже в 3-й раз..."
```

`quoted_text` is emitted only when Telegram sends `msg.quote()`. It is a sibling
of `reply_to_id`, not nested under `reply_to:`, because it describes the
triggering user's selection rather than the replied-to message body.

If the message is a reply to a non-bot message and Telegram also provides a
quote, the prompt includes both signals:

```yaml
reply_to_id: 10
reply_to:
  text: "полный текст чужого сообщения"
quoted_text: "выделенный пользователем фрагмент"
```

`reply_to.text` means "the full available body of the replied-to non-bot
message." `quoted_text` means "the fragment the user selected in this reply."

For replies to bot messages, behavior stays intentionally compact:

```yaml
reply_to_id: 11
quoted_text: "выделенный фрагмент ответа агента"
```

No `reply_to:` block is emitted for bot-message targets.

## Components

### Inbound model

Add `quoted_text: Option<String>` to `DebounceMsg` in
`crates/bot/src/telegram/worker.rs`.

`handler.rs` reads `msg.quote().map(|q| q.text.clone())` while building the
`DebounceMsg`. This is local Telegram update metadata and does not add a
fallible runtime step.

### Worker pass-through

Add `quoted_text: Option<String>` to `InputMessage` in
`crates/bot/src/telegram/attachments.rs`.

When the worker converts `DebounceMsg` to `InputMessage`, pass the value through
unchanged. Quote text is not affected by attachment resolution, STT, sandbox
upload, memory recall, or debounce batching.

### Prompt formatting

Extend `format_cc_input` to emit:

```yaml
    quoted_text: "..."
```

when `InputMessage.quoted_text` is present. Use the existing YAML string
escaping helper so quotes, newlines, and backslashes behave like other text
fields.

## Edge Cases

- Full-message reply without a Telegram quote: unchanged. The prompt contains
  `reply_to_id` and, for non-bot targets, the existing `reply_to:` block.
- Partial quote of a bot message: prompt contains `reply_to_id` and
  `quoted_text`, but no `reply_to:` block.
- Partial quote of a non-bot message: prompt can contain `reply_to_id`,
  `reply_to:`, and `quoted_text`.
- Telegram quote text with newlines or quotes: escaped through the same helper
  as message text.
- Empty quote text: Telegram's `TextQuote.text` is non-optional. If Teloxide
  deserializes an empty string, emit it as-is rather than inventing fallback
  behavior.
- External replies and reply-to-story remain out of scope. This design handles
  `Message::quote()` only.

## Error Handling

No new fallible user-facing path is introduced. Quote extraction cannot require
network or filesystem access. If the update has no quote, prompt output is
identical to current behavior.

## Documentation

During implementation, update `docs/architecture/sessions.md` because this
changes Telegram ingress prompt shape. The update should document that
conversation input YAML may include `quoted_text` when Telegram supplies a
partial reply quote.

No `PROMPT_SYSTEM.md` change is expected unless implementation finds existing
agent-facing prompt documentation that enumerates inbound YAML fields.

## Testing

Use TDD for the behavior change:

1. Add the narrow formatter regression test first and verify it fails:
   `format_cc_input` should emit `quoted_text`.
2. Add a formatter test showing `reply_to.text` and `quoted_text` can coexist.
3. Add a test for escaping newlines and quotes in `quoted_text`.
4. Add pass-through coverage for `DebounceMsg` to `InputMessage` if the existing
   worker tests can cover it without live Telegram.
5. Add handler-level coverage for `msg.quote().text` capture if message fixture
   construction is practical. If Teloxide fixtures make this brittle, keep the
   handler change simple and rely on formatter plus pass-through tests.

Targeted verification during implementation:

```sh
devenv shell -- cargo test -p right-bot telegram::attachments
devenv shell -- cargo test -p right-bot telegram::worker
```

Final verification remains mandatory:

```sh
devenv shell -- cargo test --workspace
```
