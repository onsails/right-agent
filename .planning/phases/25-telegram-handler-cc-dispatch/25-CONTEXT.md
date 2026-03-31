# Phase 25: Telegram Handler + CC Dispatch - Context

**Gathered:** 2026-03-31
**Status:** Ready for planning

<domain>
## Phase Boundary

Bot receives Telegram messages, maps them to CC sessions, invokes `claude -p` serially per thread
via a `reply` tool-use protocol, and routes replies back to Telegram. Includes debounce (batch
rapid messages), structured output (tool-use schema), typing indicator loop, and session CRUD.

**Explicitly out of scope:**
- Media file sending (schema stub only — implement in a dedicated media phase)
- Receiving Telegram files/images (future phase)
- process-compose PC wiring (Phase 26)

</domain>

<decisions>
## Implementation Decisions

### Debounce

- **D-01:** 500ms debounce window — collect messages for 500ms after the last received message
  before firing `claude -p`. Absorbs forward-bursts without adding noticeable delay for single
  messages. Window resets on each new message arrival.

- **D-02:** Batched messages are combined into an XML-wrapped input prompt:
  ```
  <messages>
  <msg id="123" ts="2026-03-31T12:00:00Z" from="user">text1</msg>
  <msg id="124" ts="2026-03-31T12:00:01Z" from="user">text2</msg>
  </messages>
  ```
  The `id` attribute exposes Telegram message_id so CC can reference it in `reply_to_message_id`.
  The `ts` attribute is ISO 8601 UTC. The `from` attribute is "user" (extend later for forwarded
  messages with original sender info).

### Structured Output Protocol (reply tool)

- **D-03:** `claude -p` is invoked with a `reply` tool definition in the system prompt (via
  AGENTS.md or prepended to the prompt). CC is instructed to ALWAYS call this tool — never reply
  with plain text directly. rightclaw parses the `tool_use` block from `--output-format json`.

- **D-04:** Reply tool schema:
  ```json
  {
    "name": "reply",
    "description": "Send a reply to the user or stay silent",
    "input_schema": {
      "type": "object",
      "properties": {
        "content": {
          "type": ["string", "null"],
          "description": "Message text to send. null = silent (no Telegram reply)"
        },
        "reply_to_message_id": {
          "type": ["integer", "null"],
          "description": "Telegram message_id to reply to. null = reply to thread only (no quote)"
        },
        "media_paths": {
          "type": ["array", "null"],
          "items": {"type": "string"},
          "description": "STUB: file paths for media to send. Phase 25 logs a warning, does not send."
        }
      },
      "required": ["content"]
    }
  }
  ```

- **D-05:** If CC returns `content: null` (or calls `reply` with `silent: true` pattern) → no
  Telegram message sent. If CC does NOT call the `reply` tool at all (malformed response) →
  send an error reply (DIS-06 format) to Telegram.

- **D-06:** `media_paths` stub: if CC returns non-null `media_paths`, log a warning
  `tracing::warn!("media_paths returned but not yet implemented — skipping")` and proceed with
  `content` only. No media sending in Phase 25.

### Session CRUD

- **D-07:** `telegram_sessions` CRUD lives in `crates/bot/src/telegram/session.rs`. Keeps
  `rightclaw` core lib free of Telegram-specific code. Consistent with Phase 23's decision to
  isolate all Telegram code in `crates/bot`.

- **D-08:** Session key is `(chat_id: i64, effective_thread_id: i64)`. `effective_thread_id`
  normalises per SES-04: `thread_id = Some(1)` → `0` (Telegram General topic).

- **D-09:** `session.rs` exposes:
  - `get_session(db, chat_id, thread_id) -> Option<String>` — returns `root_session_id`
  - `create_session(db, chat_id, thread_id, session_uuid) -> Result<()>`
  - `delete_session(db, chat_id, thread_id) -> Result<()>` — for `/reset`
  - `touch_session(db, chat_id, thread_id) -> Result<()>` — updates `last_used_at`

### Typing Indicator

- **D-10:** Repeat strategy — spawn a tokio task that sends `ChatAction::Typing` every 4 seconds
  while `claude -p` subprocess is running. Task cancelled via `CancellationToken` or
  `tokio::select!` when subprocess completes. Prevents indicator from disappearing mid-call.
  Claude decides whether to use `CancellationToken` (tokio_util) or `tokio::select!` on a
  `oneshot` channel.

### Dispatch Architecture

- **D-11:** Per-session serialisation via `tokio::sync::mpsc`. A `DashMap<(i64,i64), mpsc::Sender<DebounceMsg>>` holds the sender per session. On first message for a session, a worker task is spawned and the sender stored. Worker task owns the debounce timer and fires `claude -p` when the window expires.

- **D-12:** CC binary resolution: `which("claude").or_else(|_| which("claude-bun"))` — same
  pattern as `cmd_replay` in `rightclaw-cli`. No new config needed.

- **D-13:** Subprocess invocation (per DIS-01, DIS-02, DIS-03):
  - `HOME=$AGENT_DIR`, `cwd=$AGENT_DIR`
  - `wait_with_output()`, `stdin(Stdio::null())`
  - No `--bare` — full env (MCP, CLAUDE.md, rightmemory)

- **D-14:** First-message call (SES-02):
  ```
  claude -p --session-id <uuid> --output-format json --system-prompt-file .claude/system-prompt.txt <batched_xml>
  ```
  Resume call (SES-03):
  ```
  claude -p --resume <root_session_id> --output-format json <batched_xml>
  ```
  `--system-prompt-file` only on first call (Phase 24 decision; resume relies on session context).

- **D-15:** Session UUID generated by rightclaw (`uuid` crate), not parsed from CC response.
  DIS-04's "verify session_id from CC JSON response" is implemented as a debug-level log comparison
  only — mismatch logs a warning but does NOT block the reply.

### Error Reply Format

- **D-16:** `claude -p` non-zero exit or non-empty stderr → send to Telegram:
  ```
  ⚠️ Agent error (exit N):
  ```
  stderr content (truncated to 300 chars)
  ```
  ```
  Exit code always shown. Stderr truncated at 300 chars to avoid hitting Telegram 4096-char limit
  on error messages alone.

### Response Text Splitting

- **D-17:** Responses over 4096 chars split into multiple messages. Split at the last `\n` before
  the 4096-char boundary (word/paragraph boundary). If no `\n` found in the last 200 chars,
  hard-cut at 4096. Claude decides the exact splitting helper — keep it simple.

### Claude's Discretion

- Exact DashMap vs `Mutex<HashMap>` for the session worker map — Claude decides based on
  contention characteristics (DashMap recommended for high-concurrency, simple for low).
- Whether to use `tokio_util::sync::CancellationToken` or `oneshot` channel for typing indicator
  cancellation.
- Exact module layout inside `crates/bot/src/telegram/` — `session.rs`, `worker.rs`, `handler.rs`
  or similar reasonable split.
- Whether `reply` tool definition is embedded in code as a const JSON string or loaded from a
  file in agent_dir.

### Folded Todos

- **"Document CC gotcha — Telegram messages dropped while agent is streaming"** — With Phase 25
  implementing the mpsc queue (SES-05), messages are queued (not dropped) while another is
  processing. The gotcha to document: if the debounce worker task panics or the session is reset
  mid-queue, queued messages are silently lost. Planner should include a documentation task
  (comment in code or GOTCHAS.md entry) capturing this behavior.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` §BOT-02, BOT-06, SES-02..06, DIS-01..06 — exact requirements

### Prior phase decisions
- `.planning/phases/22-db-schema/22-CONTEXT.md` — telegram_sessions schema (column types, UNIQUE constraint)
- `.planning/phases/23-bot-skeleton/23-CONTEXT.md` — crate structure, signal handling, `children` Arc, token resolution order (D-07..D-15)
- `.planning/phases/24-system-prompt-codegen/24-CONTEXT.md` — `--system-prompt-file` flag, first-call-only rule

### Existing code to extend
- `crates/bot/src/telegram/dispatch.rs` — Phase 23 no-op dispatcher; Phase 25 replaces endpoint
- `crates/bot/src/telegram/filter.rs` — chat_id allow-list filter (already wired)
- `crates/bot/src/telegram/bot.rs` — `build_bot()` (already built)
- `crates/bot/src/lib.rs` — `run_async()` entry point; wires agent_dir + token + db into dispatch
- `crates/rightclaw/src/runtime/deps.rs` — `find_binary()` for CC binary resolution
- `crates/rightclaw/src/memory/mod.rs` — `open_connection()` for DB access in session.rs

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `dispatch.rs` `children: Arc<Mutex<Vec<Child>>>` — already present, Phase 25 uses it for
  in-flight subprocess tracking (add children there, remove on completion)
- `filter.rs` `make_chat_id_filter()` — already filters messages by allowed_chat_ids
- `runtime::deps::find_binary()` — CC binary resolution pattern to replicate in bot crate
- `memory::open_connection()` — returns `Connection`; session.rs takes a `&Connection`

### Integration Points
- `dispatch.rs` schema: replace no-op `.endpoint()` with a handler that routes to the debounce
  worker map
- `/reset` command: add `Update::filter_message().filter_command::<BotCommand>()` branch in schema

</code_context>

<specifics>
## Specific Ideas

- User explicitly wants batched messages in XML `<msg id="" ts="" from="">` format — CC can
  reference specific message IDs in `reply_to_message_id`.
- "Silent" action is important — CC must be able to receive a message and deliberately not reply
  (e.g., monitoring messages, internal processing).
- Media stub is intentional: define the schema field now so CC can start using it; rightclaw
  silently skips media in Phase 25 and warns. Full implementation in a dedicated media phase.
- User confirmed: 500ms debounce window is fixed (not configurable per-agent in Phase 25).

</specifics>

<deferred>
## Deferred Ideas

- **Media file sending** — CC outputs `media_paths`; rightclaw sends via Telegram sendPhoto/sendDocument.
  Requires file download from agent sandbox, Telegram upload, caption handling. Separate phase.
- **Receiving Telegram files/images** — download attachment, pass file path to CC via `<msg>` tag.
  Separate phase.
- **Configurable debounce per agent.yaml** — default 500ms, per-agent override. Future if needed.
- **Streaming responses / edit-in-place** — `--output-format stream-json` + `editMessageText`. v3.1.
- **`/fast` bare mode** — `--bare` flag per message prefix. v3.1.

### Reviewed Todos (not folded)
- None beyond the one folded above.

</deferred>

---

*Phase: 25-telegram-handler-cc-dispatch*
*Context gathered: 2026-03-31*
