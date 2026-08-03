# Telegram Channel Support — Design

Date: 2026-07-20
Status: approved (design), pending implementation plan

## Problem

Agents added to a Telegram channel cannot see it or post to it. Channel
support is absent at three levels:

1. **Webhook subscription.** `crates/bot/src/telegram/webhook.rs::webhook_allowed_updates()`
   registers only `message`, `edited_message`, `callback_query`. Channel
   posts arrive as the separate `channel_post` update kind, which Telegram
   never sends to us.
2. **Router.** `crates/bot/src/telegram/router.rs::route_update` handles only
   `Message` and `CallbackQuery`; everything else falls through to `_ => {}`.
3. **Routing filter.** `crates/bot/src/telegram/filter.rs::route_decision`
   starts with `let sender = msg.from.as_ref()?;` — channel posts have no
   `from` (only `sender_chat`, the channel itself), so they are dropped by
   construction.

Existing pieces that are reused as-is: `-100…` id rate limiting
(`tg_bot.rs::is_channel_or_supergroup_id`), the non-private archive
predicate (`archive.rs::should_archive_seen_group_message`), and
`allowlist.rs::AllowedGroup` (channel ids share the group id namespace;
`is_group_open` works unchanged).

## Decisions (from brainstorm)

- **Trigger mode: read + on request.** The bot never starts an agent turn
  from a channel post. The agent reads the channel and posts to it only
  when a trusted user asks in DM.
- **Visibility: search + read-before-post.** Posts are archived and
  searchable; the agent calls `channel_read` before publishing.
- **Publishing: direct, no confirmation button.** A DM request is already
  explicit consent.

## Non-goal

History backfill: the Bot API has no `getChatHistory`; a bot can only
receive live `channel_post` updates. The archive starts at connection
time. This is a Telegram limitation and is documented as such.

## Design

### 1. Update intake and archive

- `webhook_allowed_updates()` += `AllowedUpdate::ChannelPost`,
  `AllowedUpdate::MyChatMember`.
- `route_update` gains a `ChannelPost` branch:
  - accept only when `msg.chat.type_field == ChatType::Channel` and the
    channel id is an opened group in the allowlist
    (`AllowlistState::is_group_open`);
  - accepted posts go to `archive_user_message` (archive path already
    treats `from`-less messages via `sender_chat`);
  - **no routing to the worker, ever** — `filter.rs` is unchanged and
    channel posts never reach `route_decision`.
- Rate limiting on send already classifies `-100…` ids; no change.

### 2. Channel registration (open/close)

- `route_update` gains a `MyChatMember` branch: when the bot becomes a
  channel administrator, it DMs the first trusted user (allowlist
  `users[0]`) with the channel title
  and an inline "Open channel" button.
- The confirm callback writes `AllowedGroup { id, label: <channel title>,
  opened_by: <confirming user>, mode: default, topics: [] }` to
  `allowlist.yaml` via the same `with_lock` + `write_file` path used by
  `/allow_all`. Channel entries are distinguished from real groups by
  `chat.type` at intake time; no schema change and no marker field.
- Close path: extend `/deny_all` with an optional `<chat_id>` argument
  usable from DM (today it is group-only, `allowlist_commands.rs`).
- Bot removed from channel admins: out of scope for v1 (entry stays; posts
  stop arriving; `channel_post` tool call will fail with the Telegram
  error surfaced to the agent).

### 3. Agent-facing MCP tools (server "right")

New tools, routed unprefixed through `RightBackend` like the other
built-ins. Scope is always server-resolved; the agent never supplies
arbitrary chat ids outside its allowlist.

- `channel_list` → channels opened for this agent: `{ id, label }[]`.
- `channel_read(channel, limit?)` → last N archived posts for the channel
  (default 20, cap 100), newest first, from the existing message archive
  / FTS index. Output is wrapped as untrusted external content using the
  same framing convention as `thread_search` results, because channel
  posts are prompt-injection surface.
- `channel_post(channel, text)` → send a message to the channel.
  Validation server-side: the channel must be an opened allowlist entry
  AND have `chat.type == channel` (via cached intake knowledge or a
  `getChat` call); failures propagate as tool errors. No confirmation
  step. Available in foreground and cron invocations (channels are
  one-way broadcast; the `send_message` foreground-only restriction
  targets chat-hijack, which does not apply here).

`with_instructions()` in `memory_server.rs` and `aggregator.rs` is
updated with the three tools and one line of usage guidance: "before
publishing to a channel, read it with `channel_read`; publish with
`channel_post`" (project rule: MCP tool-set changes must update both).

### 4. Prompt

No system-prompt change. The channel list is dynamic and the system
prompt must stay stable; agents discover channels via `channel_list`.
The one usage line lives in MCP `with_instructions()` (see above).

### 5. Telegram mechanics

- The bot must be a channel **administrator** to receive `channel_post`
  updates and to `sendMessage` into the channel (post_messages right).
- Replies are sent without `message_thread_id` (channels do not support
  topics); channel ids take the existing `-100…` send path.

## Error handling

- Unopened channel post → silently skipped after archive-check (no log
  spam beyond debug).
- `channel_post` to an unopened or non-channel target → tool error
  naming the channel; never falls back to another chat.
- Telegram send failure (bot lost admin rights) → error propagates to the
  agent turn; no retry loop in v1.
- Callback registration failure (allowlist write) → error reply in DM;
  nothing half-written (single-file locked write).

## Testing

Unit:
- router: `ChannelPost` accepted only for opened channels; `MyChatMember`
  admin transition produces exactly one DM with confirm button.
- registration callback writes the allowlist entry; duplicate confirm is
  idempotent (`AddOutcome::AlreadyPresent`).
- `channel_post` rejects unopened channel / non-channel chat; accepts
  opened channel.
- `/deny_all <chat_id>` from DM closes a channel entry.

Integration (existing archive test infra):
- inbound channel post → archived → `channel_read` returns it in order.
- `channel_list` reflects allowlist add/remove.

Final gate per project cadence:
`devenv shell -- cargo nextest run --workspace` and
`devenv shell -- cargo test --doc --workspace`.
