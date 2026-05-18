# Thread and Chat Search Design

## Problem

Right Agent needs Hermes-style conversation search without Hermes' global-scope
leakage risk. Agents should be able to search past messages in the current
Telegram thread or chat, but not reach across chats, DMs, or groups by prompt
request.

Current Right Agent storage does not provide this directly:

- `sessions` maps `(chat_id, thread_id)` to Claude session UUIDs.
- `usage_events` stores invocation telemetry, not message text.
- legacy `memories` / `memories_fts` are not the active conversation transcript
  path.
- Hindsight stores extracted semantic memory, not authoritative raw transcript
  history.
- stream logs and Claude JSONL contain useful history but are not the live query
  substrate for this feature.

## Goals

- Add two agent-facing tools:
  - `mcp__right__thread_search`: search current `chat_id + effective_thread_id`.
  - `mcp__right__chat_search`: search current `chat_id` across all threads.
- Store new Telegram messages in SQLite for deterministic local FTS search.
- Archive every group message Teloxide delivers to the bot, whether or not it
  addresses the bot or routes to Claude.
- Keep search scope server-enforced. The agent cannot pass `chat_id`,
  `thread_id`, user IDs, session IDs, or a broader scope.
- Do not backfill historical stream logs or Claude JSONL in v1.
- Keep Hindsight as semantic memory, not conversation-history search.

## Non-Goals

- No agent-wide search.
- No cross-chat or cross-group search.
- No group-thread request that can search a DM.
- No vector search in v1.
- No historical backfill.
- No change to group routing rules: archiving a message does not mean Claude is
  invoked for that message.

## Scope Semantics

`thread_search(query)` searches only the current conversation lane:

- DM: current DM, thread `0`.
- Group general topic: current group, thread `0`.
- Group forum topic: current group, that topic only.

`chat_search(query)` searches the current Telegram chat:

- DM: only that DM.
- Group: the whole group across all topics/threads.

Both tools require current foreground Telegram invocation context. Without that
context, they return `conversation_scope_unavailable`.

## Data Model

Add `conversation_messages` to the per-agent `data.db`:

- `id`
- `platform`: initially `telegram`
- `chat_id`
- `thread_id`, normalized with existing `effective_thread_id`
- `message_id`, Telegram inbound message id
- `sender_user_id`, nullable for channel/system cases
- `sender_name`, best-effort display label
- `addressed_to_bot`, boolean
- `routed_to_agent`, boolean
- `root_session_id`, nullable for passive messages that did not invoke Claude
- `turn_id`, nullable for passive messages
- `role`: `user` or `assistant`
- `content`
- `created_at`

Add `conversation_messages_fts` as an FTS5 external-content index over
`content`.

Uniqueness:

- Telegram inbound rows use unique `(platform, chat_id, message_id, role)`.
- Assistant reply rows do not have a Telegram inbound `message_id`; they are
  associated through `root_session_id` and `turn_id`.

## Write Flow

Telegram ingress archives every group message Teloxide delivers to the bot after
metadata/text extraction, regardless of allowlist trust or mention/reply status.
This records ambient group context without invoking Claude.

DM archiving follows existing routing trust rules. Random untrusted DMs are not
retained by this feature.

When a message routes to Claude, the worker marks the corresponding inbound
record with `routed_to_agent = true`, `root_session_id`, and `turn_id` once the
invocation starts.

After a successful foreground reply, the worker inserts one or more assistant
rows with the same `root_session_id` and `turn_id`.

Backgrounded turns archive the user message before handoff. Cron, delivery, and
reflection are outside v1 unless they later flow through the normal foreground
Telegram reply path.

Archive failures are logged and do not block Telegram routing or Claude
invocation.

## Search Flow

The Right MCP backend exposes:

- `thread_search({ query, limit? })`
- `chat_search({ query, limit? })`

Validation:

- `query` must be non-empty after trimming.
- `limit` defaults to `10`.
- `limit` is clamped to `1..=50`.

Execution:

- Resolve current `chat_id` and `thread_id` from server-side invocation state.
- For `thread_search`, filter `chat_id = current_chat_id AND thread_id =
  current_thread_id`.
- For `chat_search`, filter `chat_id = current_chat_id`.
- Run FTS5 search only inside that scope.

Results include:

- snippet
- role
- sender label/id when present
- timestamp
- `thread_id`
- `message_id` when available
- `root_session_id` for traceability

No result should reveal that denied scopes exist. A query that has matches only
outside the current scope simply returns an empty result set.

## Agent Semantics

The prompt and MCP instructions must distinguish three tiers:

- **Current session context:** Claude's active `--resume` context.
- **Conversation search:** exact local transcript search via
  `mcp__right__thread_search` and `mcp__right__chat_search`.
- **Semantic memory:** Hindsight `memory_recall` / `memory_reflect`, which may
  summarize or omit details and is not authoritative transcript search.

The agent-facing description should say:

- Use `thread_search` for "what did we say in this topic/thread?"
- Use `chat_search` for "what did this chat/group discuss?"
- Do not use memory recall when the user asks for exact past wording or past
  messages.

## Search Technology

Use SQLite FTS5 for v1.

FTS5 is local, deterministic, easy to scope with SQL before returning results,
and matches the transcript-search shape. Vector search can be considered later
as reranking or semantic fallback, but only after the authorization boundary is
applied in SQL. Hindsight remains the semantic tier.

## Documentation Updates

Update:

- `docs/architecture/memory.md`: describe transcript search as separate from
  Hindsight memory.
- `docs/architecture/sessions.md`: describe archived foreground/passive
  Telegram message storage.
- `ARCHITECTURE.md`: add the prescriptive scope rule for `thread_search` and
  `chat_search`.
- `PROMPT_SYSTEM.md`: document agent-facing semantics and prefixed tool names.
- Right MCP `with_instructions()` text in `memory_server.rs` and
  `aggregator.rs` if the tool catalog changes there.

## Testing

- Migration creates `conversation_messages`, FTS table, sync triggers, scope
  indexes, and uniqueness constraints.
- Archive helper stores ambient group messages even when not routed to Claude.
- Closed/untrusted group messages that Teloxide delivers are archived but do not
  invoke Claude.
- Routed messages get `routed_to_agent`, `root_session_id`, and `turn_id`.
- Assistant replies are inserted with the same `root_session_id` and `turn_id`.
- `thread_search` excludes other threads in the same group.
- `chat_search` includes other threads in the same group.
- `chat_search` in a DM searches only that DM.
- Tools reject empty queries and unavailable invocation scope.
- Tool schemas do not expose `chat_id`, `thread_id`, or broader scope controls.

Final verification for implementation must include targeted tests during the
TDD loop and `devenv shell -- cargo test --workspace` before completion.
