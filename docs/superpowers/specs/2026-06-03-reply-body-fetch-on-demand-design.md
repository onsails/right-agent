# Reply-body fetch-on-demand — design

- **Date:** 2026-06-03
- **Status:** approved (brainstorm), pending implementation plan
- **Scope:** Spec 3 of 3 from the foreground context-usage audit. Spec 1
  (context placement) is merged; spec 2 (deterministic tool advertisement)
  has a committed spec + plan.

## Problem

When a user replies to a non-bot message, the foreground YAML inlines the
full replied-to body — `reply_to.text` + author + attachment metadata
(`crates/bot/src/telegram/attachments.rs:513-566`), populated from the
Telegram reply payload (`handler.rs:278-296`). For many replies this is
**duplicated context**: the replied-to message is already in the model's
recent context (resolvable by its `id`, which we already emit), or it is in
the per-agent conversation archive and therefore recoverable on demand. Under
spec 1 (option A) the inlined body now also persists in the transcript and
accumulates.

But it cannot be stripped unconditionally. When the reply payload is the
**only source** — the replied-to message is neither in the model's context
nor in the archive — stripping it loses information the agent needs. This is
common in **privacy-on groups**: the bot reacts only to mentions/replies
(code-level), and Telegram delivers a non-trigger original only as the reply
payload, so the bot never archived it.

Archive coverage (verified): `conversation_messages` stores telegram
`message_id`, `thread_id`, `role`, `root_session_id`, `turn_id`
(`right-db/src/conversation.rs:21-44`, schema `migrations.rs:344-357`); the
bot archives every *received* message (`dispatch.rs:569` for groups), so the
archive is a superset of what the model has processed. There is no
fetch-by-id reader yet — only `archive_message`, `mark_routed`, and FTS
`search_thread`/`search_chat`.

## Design

**Conditional strip, gated on recoverability**, plus a scope-enforced
fetch-by-id MCP tool and just-in-time agent guidance.

### 1. Mechanism

- **New reader `right_db::conversation::fetch_by_ids`:**

  ```rust
  pub async fn fetch_by_ids(
      conn: &Connection,
      platform: &str,        // "telegram"
      chat_id: i64,
      thread_id: i64,
      message_ids: &[i32],
  ) -> Result<Vec<FetchedMessage>>   // { message_id, sender_name, text, role }
  ```

  One `SELECT … WHERE platform=? AND chat_id=? AND thread_id=? AND message_id
  IN (…)`. Used by both the MCP tool (returns content) and the strip decision
  (recoverable ⇔ the id is present in the result).

- **New MCP tool `get_messages_by_id`** (`right_backend.rs`): the agent passes
  only `message_ids: [int]`. `chat_id` and `effective_thread_id` are resolved
  **server-side from the invocation** exactly as `thread_search` does — never
  agent-supplied. Returns the found messages (`message_id`, `sender`, `text`);
  ids outside the current `(chat_id, thread_id)` scope or not archived are
  simply absent from the result. It is a **foreground-only tool**: added to
  `disallow_conversation_search` (`crates/bot/src/cc/invocation.rs:70-80`)
  alongside `thread_search`/`chat_search`, so non-foreground invocations
  (cron/delivery/background) strip it consistently.

- **Conditional strip** at `worker.rs:1295-1302` (the existing
  `reply_to_body` transform, which already attaches resolved attachments /
  voice markers): when `msg.reply_to_id` is `Some(rid)` and `reply_to_body`
  is `Some`, look `rid` up via `fetch_by_ids` for the current
  `(chat_id, eff_thread_id)`. If present → **strip**; if absent → keep the
  full inline body (the payload is the only source). One archive query per
  reply-bearing message (replies are not every turn); open/reuse a connection
  in the batch loop.

### 2. What a stripped reply emits (form "ii")

`ReplyToBody` gains `pub omitted: bool`. When stripping, set `text = None`,
`attachments = vec![]`, `omitted = true`, keep `author`. `format_cc_input`
(`attachments.rs:513-566`) emits, for an omitted reply: the `author` block, a
`note: "body omitted — fetch with mcp__right__get_messages_by_id if not in
your context"`, and **not** `text`/`attachments`. A non-omitted reply emits
as today. `reply_to_id` is already emitted regardless (`:518`), and
`quoted_text` (the user's selected fragment, if any) is **kept** — it is tiny
and is exactly what the user pointed at, giving a free hint even when the
body is omitted.

Rationale: stripping the bulky `text`/attachments removes the redundancy;
keeping the cheap `author` + `quoted_text` + the fetch note preserves a usable
anchor and cues the fetch path, minimizing both "answer blind" risk and
unnecessary fetches for short messages.

### 3. Agent guidance (prompt system)

- **Tool description** (in the `get_messages_by_id` definition, which ships in
  the `tools` array) carries the general *how* and *when*: "Fetch the full
  content of messages in the current chat/topic by id (scope is
  server-resolved). Use it to read a replied-to message that isn't in your
  context, or to revisit an earlier message." Prefixed form is
  `mcp__right__get_messages_by_id` in all agent-facing text.
- **Per-reply note** (§2) is the just-in-time *when* cue, present only when a
  reply is stripped — zero permanent prompt cost.
- **`with_instructions()`** in **both** `aggregator.rs` and `memory_server.rs`
  gains the new tool (project convention for any MCP tool-set change).
- **No `OPERATING_INSTRUCTIONS` change.** The tool description + just-in-time
  note cover when/how; adding a permanent prompt-tier rule would cost tokens
  every turn for a tool used only on some reply turns. (Belt-and-suspenders
  line is an option if review wants it, but omitted by default per
  prompt-tier brevity.)

## Affected code

- `crates/right-db/src/conversation.rs` — add `fetch_by_ids` + `FetchedMessage`.
- `crates/right/src/right_backend.rs` — `get_messages_by_id` param struct, tool
  def, dispatch arm, handler (scope from invocation, calls `fetch_by_ids`).
- `crates/right/src/aggregator.rs`, `crates/right/src/memory_server.rs` —
  `with_instructions()` inventory.
- `crates/bot/src/cc/invocation.rs` — add `get_messages_by_id` to
  `disallow_conversation_search`.
- `crates/bot/src/telegram/attachments.rs` — `ReplyToBody.omitted`;
  `format_cc_input` omitted-reply emission.
- `crates/bot/src/telegram/worker.rs` — strip decision in the
  `reply_to_body` transform (archive lookup).
- `PROMPT_SYSTEM.md`, `ARCHITECTURE.md` — fetch convention; new scope-enforced
  tool in the MCP Aggregator scope rules (beside `thread_search`/`chat_search`).

## Security & scope

- Scope is server-resolved `(chat_id, effective_thread_id)` from the
  invocation, identical to `thread_search`; the agent cannot pass chat/thread
  ids or widen scope. Cross-chat/cross-topic ids resolve to "not found".
- The fetched body is the same user-conversation content the bot already
  inlines today; no new ironclaw wrapping (consistent with current inline
  handling and with `thread_search`/`chat_search` results).

## Upgrade & compatibility

- New table reader is additive; no migration (column set unchanged). The new
  tool and the strip behavior take effect on bot restart (codegen / MCP
  inventory regenerate). Existing agents adopt via `right restart`. No sandbox
  recreation.
- Backward-compatible: agents that never call the tool still get a usable
  (author + note + quoted_text) anchor; non-archived replies are unchanged.

## Verification cadence

- TDD: `fetch_by_ids` (returns matching rows, scoped by chat/thread, omits
  non-existent ids) first; then `format_cc_input` omitted-reply emission (note
  present, text/attachments absent, author + reply_to_id + quoted_text kept);
  then the tool handler scope enforcement (agent-supplied chat/thread ignored).
- Targeted: `cargo test -p right-db conversation`, `cargo test -p right
  right_backend`, `cargo test -p right-bot attachments`.
- Final, mandatory: `devenv shell -- cargo test --workspace` from the worktree.

## Out of scope

- `quoted_text` vs `reply_to.text` substring dedup (audit micro-item #6) —
  negligible savings; not worth its own change. Won't-do unless that code is
  touched for another reason.
- Proactive/agent-initiated message browsing beyond replies — the tool
  supports it (general by-id fetch), but no extra UX/instruction is built for
  it here.

## Open questions

None — scope (Full: conditional strip + `get_messages_by_id`) and the stripped
form (ii: keep author + note + quoted_text, drop text/attachments) resolved in
brainstorming.
