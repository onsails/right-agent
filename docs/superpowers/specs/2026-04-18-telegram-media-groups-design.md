# Telegram Media Groups

## Overview

Let an agent group multiple outbound attachments into a single Telegram
"media group" (album) by tagging them with a shared `media_group_id` in the
structured reply. The bot honours the grouping when it matches Telegram's
homogeneity rules; otherwise it degrades to individual sends and logs a warning.

## Goals

- Single-message UX for coherent sets (multiple photos from one event, pages of
  a report, etc.) without changing turn structure or prompt flow.
- Schema mirrors Telegram semantics: the outbound field name is `media_group_id`,
  the same name Telegram puts on inbound `Message.media_group_id`. Symmetric
  read/write keeps the agent's mental model small.
- Forgiving failure: invalid grouping does not drop content. The bot warns and
  falls back to individual sends. Only genuine Telegram API errors surface as
  errors.
- Backward compatible: attachments without `media_group_id` behave exactly as
  today (one `send_photo` / `send_document` / … per item).

## Non-Goals

- Inbound media-group reassembly: grouping on *incoming* Telegram messages.
  Today the bot already sees each inbound message separately; grouping those
  into one CC turn is a separate concern.
- Cross-reply grouping: an album must live inside one structured reply.
  Attempts to reuse a `media_group_id` across turns have no meaning.
- Automatic grouping heuristics: the bot never invents a `media_group_id` the
  agent did not supply.

---

## Schema Change

`crates/rightclaw/src/codegen/agent_def.rs` holds three JSON schemas that all
embed the same `attachments` item shape:

- `REPLY_SCHEMA_JSON` (normal replies)
- `BOOTSTRAP_SCHEMA_JSON` (bootstrap mode)
- `CRON_SCHEMA_JSON` (cron `notify.attachments`)

Add one optional nullable field to the item:

```json
"media_group_id": { "type": ["string", "null"] }
```

`OutboundAttachment` in `crates/bot/src/telegram/attachments.rs` gains
`pub media_group_id: Option<String>`. Existing fields (`type`, `path`,
`filename`, `caption`) unchanged.

## Bot Behaviour

### Partition

`send_attachments` walks the `attachments` slice once and produces an ordered
list of *sends*. A send is one of:

- `Individual(&OutboundAttachment)` — existing per-type `send_*` path.
- `Group { id: String, items: Vec<&OutboundAttachment> }` — candidate for
  `sendMediaGroup`.

Items without `media_group_id` become `Individual`. Items sharing a
`media_group_id` collapse into one `Group`. Ordering in the output list is
determined by the first occurrence of each group id (agent controls it).

### Classify

A pure helper `classify_media_group(items: &[&OutboundAttachment]) -> GroupPlan`
decides what to do with each `Group`. Possible outcomes:

| Composition                                                 | Plan                                    |
|-------------------------------------------------------------|-----------------------------------------|
| 2–10 items, all `photo` or `video` (may mix)                | `SendAsGroup`                           |
| 2–10 items, all `document`                                  | `SendAsGroup`                           |
| 2–10 items, all `audio`                                     | `SendAsGroup`                           |
| Any mix across categories, or any `voice`/`video_note`/`sticker`/`animation` | `Degrade { reason: "incompatible types" }` |
| 1 item                                                      | `Degrade { reason: "group of 1" }`      |
| 11+ items, all compatible type                              | `Split { chunks: Vec<Vec<_>>, reason }` — chunks of ≤10 of the same type, each a valid group |
| 11+ items, mixed compatible + incompatible                  | `Degrade` (simplest — size plus mix is rare enough not to warrant a split path) |

`Degrade` and `Split` both emit `WARN` logs identifying the `media_group_id`,
the composition, and the chosen fallback. Example:

```
media_group_id="shots" contains incompatible types [photo, voice] — falling back to individual sends
```

### Send

For every final send:

- `Individual` — unchanged from today.
- `SendAsGroup` / each chunk of `Split` — build `Vec<InputMedia>` (teloxide
  `InputMediaPhoto`/`Video`/`Document`/`Audio`), resolve every item's host
  path through the existing staging logic (sandboxed = `download_file` into
  `tmp/outbox/`), call `bot.send_media_group(chat_id, media).message_thread_id(...)`.

**Captions.** Telegram shows one caption per media group, taken from the first
item. When multiple items carry a caption, join them with `"\n\n"` into the
first item's caption before building `InputMedia`; blank the rest. This
prevents silent text loss when the agent puts a caption on a later item.

**Errors.** Send results feed the same `errors: Vec<String>` that already
collects per-item failures today. One failed group produces one error entry.
Other groups and individuals still run. Error message format uses
`display_error_chain` exactly like the individual path.

**Cleanup.** Temp files for each group member are deleted after the group
send attempt finishes (success or failure), same pattern as today's per-item
loop.

## Prompt Change

Extend `## Sending Attachments` in
`crates/rightclaw/templates/right/prompt/OPERATING_INSTRUCTIONS.md` with a
`### Media Groups (Albums)` subsection that:

- Explains same-`media_group_id` semantics and why it mirrors inbound
  `Message.media_group_id`.
- Lists Telegram's homogeneity rules (2–10, photo+video mixable, documents
  alone, audios alone, voice/video_note/sticker/animation ungroupable).
- Mentions that the bot warns and falls back on violation — failures are not
  silent but also do not drop content.
- Explains the caption merge rule.
- Shows one short JSON example: two grouped photos plus one standalone
  document in the same reply.

`PROMPT_SYSTEM.md` gets a matching paragraph under the existing
attachments/reply-schema section (project convention requires it stays in
sync).

No change to the bootstrap or cron operating instructions — they consume the
same schema and the same prompt chunk.

## Tests

All new logic is pure and unit-testable. No live-Telegram integration test.

In `crates/bot/src/telegram/attachments.rs`:

- `OutboundAttachment` deserializes with and without `media_group_id`.
- `classify_media_group` table test covering every row in the classify table
  above (including the 11-photo split case and the mix-plus-oversize degrade
  case).
- Caption merge helper: `["a", None, "b"]` → `Some("a\n\nb")` on the first
  item, `None` on the rest.
- Partitioner: a reply with two groups and one standalone returns the
  expected number, order, and membership of sends.

In `crates/rightclaw/src/codegen/agent_def_tests.rs`:

- Parse `REPLY_SCHEMA_JSON`, `BOOTSTRAP_SCHEMA_JSON`, `CRON_SCHEMA_JSON` and
  assert each attachments item schema lists `media_group_id` as nullable
  string.
- Snapshot-style substring check that `OPERATING_INSTRUCTIONS` contains
  `Media Groups` — cheap guard against accidental deletion.

## Files Touched

- `crates/rightclaw/src/codegen/agent_def.rs` — three schema constants.
- `crates/rightclaw/src/codegen/agent_def_tests.rs` — schema + prompt tests.
- `crates/rightclaw/templates/right/prompt/OPERATING_INSTRUCTIONS.md` — new
  subsection.
- `crates/bot/src/telegram/attachments.rs` — `OutboundAttachment` field,
  `classify_media_group`, partitioner, rewritten `send_attachments`, unit
  tests.
- `PROMPT_SYSTEM.md` — matching paragraph.

## Rollout

Pure additive change. No migration, no sandbox state, no DB schema touch.
Once agents pick up the new prompt they can start setting `media_group_id`;
replies that omit it keep the current behaviour.
