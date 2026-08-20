# CC message format: reply-context & author rendering

**Date:** 2026-06-07
**Status:** Design approved, pending implementation plan

## Problem

Agent "him" replied "🍄?" (confusion) to a bare-mention reply. Root cause,
verified from the sandbox CC transcript for session
`f7d5a319-447f-4e58-ba8f-3c23dd476367`, turn 6:

```yaml
messages:
  - id: 4570
    reply_to_id: 4569
    reply_to:
      author: { name: "Andrey Kuznetsov", ... }
      note: "body omitted - fetch with mcp__right__get_messages_by_id if not in your context"
    text: ""
```

The user replied to his own earlier message 4569 ("Сравни по времени в море")
with a bare "@rightaww_bot". After mention-stripping the text became `Some("")`
→ rendered as `text: ""`. The reply body was stripped to a fetch note because
message 4569 was **recoverable from the archive**. But 4569 was **non-routed**
(`addressed_to_bot=0, routed_to_agent=0`) — it was never sent to the model, so
it was **not in the model's session context**. The model was left with empty
text + an optional-sounding fetch note, treated the note as not-needed, and
responded "🍄?" without calling `get_messages_by_id`. Stream log confirms
`num_turns: 2`, zero MCP tool calls.

The retry (turn 7) succeeded only because the instruction was re-typed inline
in `text`.

Three deeper tensions surfaced while designing the fix:

1. We must not make the model fetch over MCP for replies to **recent** messages.
2. The model **does not know the Telegram message_id of its own outputs** (it
   only emits StructuredOutput; the sent id is never fed back).
3. Inlining a full previous message **bloats context** with a duplicate.

## Prior art (researched from local checkouts)

- **OpenClaw / IronClaw** (`~/molt/ironclaw`, `channels-src/telegram/src/lib.rs`):
  `reply_to_message` is parsed and silently dropped — the model never sees reply
  context. Not a usable model.
- **Hermes** (`NousResearch/hermes-agent`, `gateway/run.py:2618`): inlines a
  truncated snippet (`reply_to_text[:500]`), but **skips it when the quoted text
  already appears in session history** (substring match on first 200 chars
  across all roles). Outgoing message_ids are never persisted; dedup is purely
  by **content**, not id.

Hermes's key insight, adapted here: gate on **"is this content already in the
model's context"**, not on **"is it recoverable from the archive"**. That gate
both resolves tensions 1–3 and explains our bug (4569 was archived but never in
context). Because dedup is content/context-based, the model-doesn't-know-its-own-id
problem (tension 2) dissolves.

## Design

### 1. Author rendering — compact, everywhere

Everywhere a person is rendered — message author, `reply_to.author`,
`forward_from` — emit **`user_id` always, plus `username` when present**. Drop
the `name` line. DMs keep their current behavior (no per-message author; the
single user is established once in `## Current Conversation`).

Rationale: repeating `name`+`username`+`user_id` on every group message is
wasteful; `username`/`user_id` are enough to attribute, and the full identity is
available on demand (see tool below).

Touch: `crates/bot/src/telegram/attachments.rs` (`format_cc_input`, the author,
`reply_to.author`, and `forward_from` blocks).

### 2. New MCP tool `get_chat_member`

- **Input:** `user_id` (integer). No `chat_id` argument — the server injects the
  invocation's `chat_id` (same server-enforced scoping as the other conversation
  tools).
- **Output:** fresh `name`, `username`, status — from Telegram `getChatMember`.
- **Backing:** Telegram Bot API. The MCP aggregator (`RightBackend`) does **not**
  own a Telegram client, so this tool — unlike the DB-backed conversation tools —
  must reach the **bot process** (which owns the teloxide `Bot`).
  - **Open implementation choice (for the plan):** route from `RightBackend`
    through the bot's internal socket, **or** have the aggregator construct its
    own `Bot` from the agent token. Recommendation: route to the bot to keep a
    single Telegram client and token path.
- **Resolution is always by `user_id`** (Telegram `getChatMember` cannot resolve
  by username). `user_id` is always present in the rendered author, so this is
  sufficient.
- Document in `OPERATING_INSTRUCTIONS.md` (tool inventory + reply-metadata note)
  and `PROMPT_SYSTEM.md`. Update `with_instructions()` in `memory_server.rs` and
  `aggregator.rs` per the MCP-tool convention. Add the CC-prefixed constant
  `mcp__right__get_chat_member` in `internal_client.rs`.

### 3. Empty text

Never emit `text: ""`. When a message has no text of its own (whitespace-only
after mention-stripping), **omit the field**. Normalize empty → absent upstream
(`handler.rs`, after `strip_bot_mentions`) so the renderer sees `None`.

### 4. Reply context — three tiers gated on "in model's context"

Always emit `reply_to_id: N`. The gate computes whether the reply target's
content is already in the **current session's** model context, queried from
`conversation_messages WHERE root_session_id = <session_uuid>` (routed-user rows
+ assistant rows) within a recency window of `IN_CONTEXT_WINDOW` messages.
`ctx.session_uuid` is available at the gate site (`worker.rs`, where
`strip_recoverable_reply_to_body` is called today); it equals
`conversation_messages.root_session_id`.

| Condition | `reply_to` rendering |
|---|---|
| Target is the bot's **own freshest** message (the single most-recent assistant message in the session) | `note: "your own previous message"` — no text, no author block |
| Target **in context, not last** (own or other) | `truncated_text: "<≤LOCATOR_MAX chars>…"` — locator only, no fetch note |
| Target **not in context**, body ≤ `REPLY_BODY_INLINE_MAX` | `text: "<full body>"` — complete |
| Target **not in context**, body > `REPLY_BODY_INLINE_MAX` | `truncated_text: "<first REPLY_BODY_INLINE_MAX>…"` + `note: "full: get_messages_by_id(<id>)"` |

**Key field semantics:**
- `text` = the complete replied-to text.
- `truncated_text` = an incomplete preview (always ends with `…`). The distinct
  key name signals incompleteness so the model knows the text is partial and can
  fetch the rest only when needed.

A **locator** is a short truncated quote (`≤ LOCATOR_MAX`) whose sole purpose is
to let the model identify *which* message is being replied to — including which
of its **own** past messages (tension 2). It is not meant to reproduce content;
the full content is in the model's own context for the in-context tiers.

For every tier except "own freshest", the `reply_to.author` block renders per
§1 (`user_id` + `username`, no `name`) so the model knows *who* it is replying
to. An earlier own-message (not the freshest) falls to the locator tier like any
other in-context target.

This replaces the archive-recoverability gate in
`strip_recoverable_reply_to_body`, fixes the 4569 bug (non-routed → "not in
context" → inline body), and collapses the special case at `handler.rs:280`
(reply-to-bot-own → `reply_to_body = None`) into the unified context gate.

Voice/video-note reply targets carrying STT markers must still never be stripped
to a locator (the archive stores only a `[voice]` placeholder; the marker is not
recoverable) — preserve the existing `had_voice_markers` carve-out.

### 5. `quoted_text` unchanged

The Telegram partial-quote fragment (`msg.quote()`, `quoted_text` field) stays a
separate field as-is. It is the user-selected substring, distinct from the
reply-to body, and is never stripped.

## Constants (tunable)

- `LOCATOR_MAX = 120` (chars of a locator quote)
- `REPLY_BODY_INLINE_MAX = 500` (chars before truncation + fetch note; matches
  Hermes)
- `IN_CONTEXT_WINDOW ≈ 30` (recent session messages considered "in context")

## Affected files

- `crates/bot/src/telegram/attachments.rs` — author/reply_to/forward rendering,
  `text` omission, `truncated_text` vs `text`, locator.
- `crates/bot/src/telegram/worker.rs` — replace `strip_recoverable_reply_to_body`
  with the context-gate; the three-tier renderer feeds off the gate result.
- `crates/bot/src/telegram/handler.rs` — normalize empty text → `None`; remove
  the reply-to-bot-own `reply_to_body = None` special case.
- `crates/right-mcp/` (`aggregator.rs`, `memory_server.rs` or `proxy`/backend,
  `internal_client.rs`) + bot internal API — new `get_chat_member` tool and its
  routing to the bot's Telegram client.
- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` and
  `PROMPT_SYSTEM.md` — document the new author format, `truncated_text`/`text`
  semantics, the reply tiers, and `get_chat_member`.

## Testing & verification cadence

- **TDD, regression first:** the 4569 case — empty own text + a reply target that
  is non-routed / not in session context → body inlined (not a fetch note),
  no `text: ""`. Write it failing first.
- Unit tests for each of the three reply tiers (own-freshest → note;
  in-context-not-last → `truncated_text` locator; not-in-context short → `text`;
  not-in-context long → `truncated_text` + fetch note).
- Author rendering tests: group message emits `user_id` (+`username`), never
  `name`; same for `reply_to.author` and `forward_from`.
- `get_chat_member`: tool schema/scope test (server injects `chat_id`, rejects
  agent-supplied chat scope); a live path test under the appropriate
  `ci-openshell`/`ci-claude` ignore-prefix if it needs a real Telegram round-trip.
- Targeted package tests during the loop
  (`devenv shell -- cargo test -p right-bot <filter>`,
  `-p right-mcp <filter>`); **final** `devenv shell -- cargo test --workspace`
  before declaring complete.

## Non-goals

- Persisting outgoing Telegram message_ids (the context gate makes it
  unnecessary; tension 2 is solved by the locator, not by id mapping).
- Resolving identities by username (Telegram API can't; `user_id`-keyed
  `get_chat_member` suffices).
- Changing `quoted_text` handling.
- A general people-directory / membership cache (out of scope; `get_chat_member`
  is on-demand).
