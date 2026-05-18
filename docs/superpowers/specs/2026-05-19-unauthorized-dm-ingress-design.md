# Unauthorized DM Ingress Gate

## Problem

Issue #69 asks to verify that the Telegram bot completely ignores messages and
attachments from users outside `allowed_users` in direct messages.

The current routing filter already drops private-chat messages when the sender
is not trusted. That gate runs before command handling, `handle_message`,
debounce, attachment download/upload, routed DM archive, and Claude invocation.

One gap remains: the dispatcher's pre-filter diagnostic log currently builds a
text/caption preview before the allowlist filter runs. That means spam content
from unauthorized DMs can still affect operator experience even though it does
not reach the user-facing bot flow.

## Goals

- Add regression coverage proving unauthorized DMs are dropped by the existing
  allowlist gate.
- Cover text-only DMs and media/caption DMs, because attachment-bearing spam is
  the high-risk path for both user and operator experience.
- Preserve trusted-user DM routing.
- Remove private-message body/caption previews from pre-filter logs.
- Keep enough low-cardinality metadata for diagnostics without recording spam
  content.

## Non-Goals

- No new allowlist model.
- No command behavior changes for trusted users.
- No group routing changes. Group pre-routing archive remains separate and is
  intentionally documented in `docs/architecture/sessions.md`.
- No live Telegram integration test. This can be proven with pure routing and
  metadata tests.

## Decision

Keep `telegram::filter::make_routing_filter` as the single ingress authority.
Unauthorized private chats continue returning `None` before downstream handlers
see the message.

Change the pre-filter dispatcher log in `telegram::dispatch` so it does not
extract or log `msg.text()` or `msg.caption()`. It should log metadata only:

- `chat_id`
- chat kind, such as `private`, `group`, `supergroup`, `channel`, or `unknown`
- `has_text`
- `has_caption`
- inbound attachment count
- entity count

The log may still run before routing because the content-free shape is useful
for diagnosing delivery without surfacing spam text. This is preferable to
removing the log entirely.

## Existing Boundary

The important current sequence is:

1. `Update::filter_message()` receives a Telegram message.
2. The pre-filter `.inspect` logs the update and archives only seen group
   messages.
3. `make_routing_filter` checks the sender and chat allowlist.
4. Unauthorized DMs return `None`.
5. Only routed messages reach command handlers or `handle_message`.

For DMs, `archive_seen_group_message` is a no-op, and
`archive_routed_dm_message` is called only inside `handle_message`, after the
routing filter. Therefore unauthorized DMs should not be archived, debounced,
downloaded, or sent to Claude.

## Components

### Routing tests

Extend `crates/bot/src/telegram/filter.rs` tests with small Telegram `Message`
fixtures that prove:

- untrusted private text message returns `None`
- untrusted private photo/document with caption returns `None`
- trusted private text message returns `Some(RoutingDecision)` with
  `address = Some(DirectMessage)`
- trusted private media/caption message also routes

These tests verify the existing gate rather than introducing a second
authorization path.

### Dispatcher log metadata

Add a small helper in `crates/bot/src/telegram/dispatch.rs`, or a private
nearby module if that file would become awkward, to derive content-free log
metadata from a `Message`.

The helper must not return text, caption, or a preview string. Its tests should
construct a private message with obvious body/caption content and assert the
metadata includes only booleans/counts/kind. The point is to make accidental
preview reintroduction mechanically visible.

### Attachment count

The metadata helper can reuse `attachments::extract_attachments(&msg).len()`
because this only inspects Telegram metadata already in the update. It must not
download files or resolve Telegram file paths.

## Error Handling

No new fallible user-facing path is introduced. If metadata extraction sees an
unrecognized chat kind, log `unknown` rather than failing. The routing filter
remains fail-closed for messages without `from`.

## Documentation

No architecture contract changes are expected. During implementation, re-read
`docs/architecture/sessions.md` because this touches Telegram ingress. Update it
only if the code no longer matches its direct-message archive description.

## Testing

Use TDD for the behavior change:

- First add the unauthorized-DM routing regression tests and run the targeted
  `right-bot` filter tests.
- Add the dispatcher metadata test before changing the pre-filter log.
- Implement the logging change.
- Rerun the targeted `right-bot` tests.

Targeted commands should use:

```sh
devenv shell -- cargo test -p right-bot telegram::filter
devenv shell -- cargo test -p right-bot telegram::dispatch
```

Final verification for implementation remains mandatory:

```sh
devenv shell -- cargo test --workspace
```
