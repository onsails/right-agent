# Bot Forum-Topic Management — Design

**Date:** 2026-06-01
**Status:** Approved (brainstorming)

## Goal

Let a Right agent organize a Telegram forum supergroup by **creating,
renaming, closing, and reopening forum topics** — but never deleting them.
The capability is agent-driven: the user asks in chat ("заведи топик под
X", "закрой топик про Y") and the agent calls MCP tools that the bot
executes against the Telegram Bot API.

## Why "create/edit but not delete" is natural, not a hack

Telegram itself separates these rights:

- `can_manage_topics` administrator right (verbatim from the Bot API):
  *"True, if the user is allowed to create, rename, close, and reopen
  forum topics; for supergroups only"* — covers create + edit + close +
  reopen, **not** delete.
- `deleteForumTopic` requires a **separate** right, `can_delete_messages`.

So the desired boundary is the boundary Telegram already draws. Grant the
bot `can_manage_topics` and withhold `can_delete_messages`. We additionally
**never expose a delete tool** in any form, so the agent has no path to
deletion even if a future operator over-grants rights.

`teloxide` 0.17 (teloxide-core 0.13.0) exposes all relevant methods as
first-class builder calls: `create_forum_topic`, `edit_forum_topic`,
`close_forum_topic`, `reopen_forum_topic`. `can_manage_topics` is present
on `ChatMemberKind::Administrator` and `ChatAdministratorRights`. The
existing `ThreadId(MessageId(i32))` construction used in
`telegram/handler.rs` is exactly correct for `message_thread_id`.

## Decisions (settled during brainstorming)

1. **Operation set:** create + edit + close + reopen. No delete. No
   General-topic methods (editGeneral/hide/unhide) — deferred, YAGNI.
2. **Gating:** none beyond Telegram rights. No `agent.yaml` flag. The
   `can_manage_topics` admin right (granted in Telegram's group-admin
   settings) *is* the user-facing switch. The "expose all settings via
   CLI" convention is about settings that otherwise can't be controlled;
   here control exists and is native to Telegram.
3. **Tool shape:** 4 explicit single-purpose tools (matches the
   `memory_retain`/`memory_recall`/`memory_reflect` one-tool-per-action
   convention), plus a 5th read tool `forum_topic_list`.
4. **Storage:** topic registry in `data.db` (local Turso), **not**
   Hindsight. Authoritative, exact, deterministic.
5. **Cross-chat isolation:** server-enforced — see Security below.

## Architecture

The MCP server holds no Telegram token; every Telegram side effect already
routes through the bot. Forum-topic operations follow the existing
`send_progress` path exactly:

```
agent → mcp__right__forum_topic_*  (RightBackend in right-mcp)
      → InternalClient (UDS)
      → new bot-local UDS endpoint(s) (sibling of /progress/send)
      → bot resolves chat_id from the registered in-flight invocation
      → teloxide: bot.create_forum_topic(chat_id, name) / edit / close / reopen
      → on success: upsert row into data.db `forum_topics`
      → return result (e.g. new message_thread_id) to the agent
```

`chat_id` is **never** supplied by the agent — the bot resolves it from
the invocation registered via `/progress/register` (same mechanism that
binds a progress message to the correct chat/thread). `message_thread_id`
(for edit/close/reopen) is agent-supplied but only ever applied within the
resolved current chat, so it cannot reach another chat's topics.

### Inherent Telegram limitation (designed-in, not worked around)

The Bot API has **no "list all topics" method**. Without persistence, an
agent can only address: (a) the current topic (its `topic_id` is already
in `ChatContext`), and (b) a topic it created this turn (the new id is
returned). The `data.db` registry exists to lift this limit across turns
and sessions — see below.

## MCP Tools

All exposed by `RightBackend` under the `right` server, so agents see the
`mcp__right__` prefix. Update tool descriptions and `with_instructions()`
in both `aggregator.rs` and `memory_server.rs` (MCP convention).

| Tool | Params | Returns |
|---|---|---|
| `forum_topic_create` | `name` (required, 1–128), `icon_color?` (one of the 6 allowed RGB ints), `icon_custom_emoji_id?` | new `message_thread_id`, `name` |
| `forum_topic_edit` | `message_thread_id` (required), `name?` (1–128), `icon_custom_emoji_id?` (empty string removes icon) | ok |
| `forum_topic_close` | `message_thread_id` (required) | ok |
| `forum_topic_reopen` | `message_thread_id` (required) | ok |
| `forum_topic_list` | none (chat scope is server-resolved) | array of `{message_thread_id, name, icon_color, icon_custom_emoji_id, state}` for the **current chat only** |

`icon_color` allowed values (createForumTopic only; edit cannot change
color): 7322096, 16766590, 13338331, 9367192, 16749490, 16478047.
`icon_custom_emoji_id` must be an id returned by
`getForumTopicIconStickers` — out of scope to expose that lookup as a
tool in v1; agents pass an id only if they already have one, otherwise
omit it.

## Storage — `data.db` registry

New table via an idempotent migration appended to
`right_db::migrations::MIGRATIONS` (the sole place to add tables):

```sql
CREATE TABLE IF NOT EXISTS forum_topics (
    chat_id              INTEGER NOT NULL,
    message_thread_id    INTEGER NOT NULL,
    name                 TEXT,
    icon_color           INTEGER,
    icon_custom_emoji_id TEXT,
    state                TEXT NOT NULL DEFAULT 'open',  -- 'open' | 'closed'
    updated_at           TEXT NOT NULL,
    PRIMARY KEY (chat_id, message_thread_id)
);
```

**Population is authoritative and deterministic** — written by the bot
after a successful Telegram call:

- `create` → upsert `(chat_id, new_thread_id, name, icon_color, icon_custom_emoji_id, 'open')`.
- `edit` → update `name`/`icon_custom_emoji_id` for the row.
- `close` → set `state='closed'`. `reopen` → set `state='open'`.

Each is a single-statement upsert/update → no transaction required (per
the Transaction Rule). `updated_at` is set from the bot's clock.

**Deferred (future extension):** passive learning of human-created topics
by observing `forum_topic_created` / `forum_topic_edited` /
`forum_topic_closed` / `forum_topic_reopened` service messages in the
Telegram handler. Deterministic but adds handler code; the v1 use case
("agent creates and then manages its own topics") is fully served without
it. Note the partial-name caveat: a topic the bot only ever saw via plain
messages (no create/edit service message) would have an id but no name.

## Security — cross-chat isolation (load-bearing)

`data.db` is per-agent, but one agent serves many chats (one DM + several
groups). The risk is leakage **between chats of the same agent**: a member
of group A must never learn group B's topics.

Enforcement (mirrors the `thread_search` / `chat_search` scope rule):

- `forum_topic_list` resolves `chat_id` server-side from the in-flight
  invocation. The agent cannot pass or override `chat_id`.
- The query is strictly `SELECT ... FROM forum_topics WHERE chat_id = ?`
  with the resolved current chat id.
- Write paths store rows under the resolved current `chat_id`, so a topic
  created in group A is only ever listable from group A.
- edit/close/reopen call Telegram with the resolved `current_chat_id`; an
  agent-supplied `message_thread_id` from another chat simply does not
  exist there and fails at the API — no cross-chat mutation is possible.

This becomes a server-enforced invariant alongside the existing
conversation-search scope rule in `ARCHITECTURE.md` (MCP Aggregator
section).

## Error handling (FAIL FAST)

- Missing `can_manage_topics`, non-forum chat, or DM → Telegram returns an
  error. The bot maps it to a clear, actionable message for the agent
  (e.g. *"I need the 'Manage Topics' admin right in this group"* /
  *"forum topics exist only in forum supergroups"*) and **propagates** it
  (no swallowing). The agent relays it to the user.
- `anyhow`→string conversions use `format!("{:#}", e)` to preserve the
  error chain.

## Prompt layer

Two requested changes, kept within the prompt-tier brevity budget:

1. **Capability awareness.** A compact note (2–3 sentences) in
   `OPERATING_INSTRUCTIONS.md` (Communication section): in forum groups
   you can organize the conversation into topics — create / rename /
   close / reopen; you cannot delete. Plus the tool descriptions and
   `with_instructions()` updates (the tool list is itself part of the
   prompt).
2. **Bootstrap nudge.** One sentence in `BOOTSTRAP.md` after the recap
   (step 6): tell the user the agent works best in a group where it is an
   admin (so it can manage topics and the chat). No pressure, one line.

## Testing

- Unit: permission-error → user-message mapping; internal-request
  (de)serialization (mirrors progress request tests, with token
  redaction); `forum_topics` upsert/update transitions;
  `forum_topic_list` returns only the current `chat_id`'s rows (the
  cross-chat isolation invariant — seed two chats, assert no leak).
- Migration: `registry_covers_all_per_agent_writes` stays green; new
  table is idempotent (`CREATE TABLE IF NOT EXISTS`).
- Confirm forum service messages (`forum_topic_created` etc.) do **not**
  trigger a spurious agent turn — the handler already filters
  non-actionable updates; verify, add no new handling.
- Final: `cargo test --workspace` (mandatory).

## Out of scope (YAGNI)

- `deleteForumTopic` — never exposed.
- General-topic methods (editGeneral / close / reopen / hide / unhide).
- `getForumTopicIconStickers` as a tool.
- Passive registry population from human-created topics.
- Agent posting an initial message into a freshly created topic
  (cross-topic send) — the agent gets the new `message_thread_id` back and
  the user can continue there.
