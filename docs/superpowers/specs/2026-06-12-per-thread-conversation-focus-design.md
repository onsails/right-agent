# Per-thread conversation focus — design

- **Date:** 2026-06-12
- **Status:** Approved for planning
- **Scope:** One feature, single implementation plan.

## Goal

Give each Telegram conversation scope — a DM, a group's General, or a forum
topic — a small piece of standing **focus**: text appended to the agent's prompt
for every turn in that scope. Two writers maintain it:

- the **operator**, through a Telegram Mini App view, and
- the **agent itself**, through a new built-in MCP tool.

The two writers are kept apart by trust level: operator focus is an
authoritative system instruction; agent focus is the agent's own notes and is
treated as untrusted reference data.

This replaces the original "pin Telegram messages into the prompt" idea, which
is not buildable: the Bot API has no method to list a topic's pinned messages,
and unpinning emits no update, so a pin mirror would silently rot. Operator- and
agent-curated focus sidesteps the API entirely.

Terminology: **focus** is the single through-line term — command, DB, module,
MCP tool, dashboard view, and the in-prompt section label all say "focus".

## Non-goals

- No Telegram pin/unpin tracking.
- No CLI surface for this setting (bot/dashboard is the control plane, per the
  configuration-hierarchy convention).
- No per-thread authorization model beyond the existing agent allowlist (see
  Security).
- No signed launch token in this iteration (see Security → known risks).
- No new codegen output. This is runtime data in `data.db`, not a generated
  file; codegen categories are untouched.

## Background (verified)

- **Launch context is not in `initData`.** A Telegram Mini App opened from an
  inline `web_app` button does not receive `chat_id`/`thread_id` in the signed
  `initData`. The bot knows the scope at send time and embeds it in the button
  URL.
- **CORRECTION (2026-06-14): inline `web_app` buttons are private-chat-only.**
  The original claim here — that inline `web_app` buttons work in groups/topics
  and the `is_private_chat` gate on `/mcp` is "a policy choice, not a Telegram
  limit" — was wrong and unverified. Telegram rejects inline `web_app` buttons
  outside private chats with `BUTTON_TYPE_INVALID`, so the as-shipped
  `/set_focus` silently failed in every group/topic (`send.await?` errored, no
  message delivered). The gate on `/mcp` is a real Telegram limit. Fix: in a
  group/topic `/set_focus` sends a `t.me/<bot>?start=f<chat_id>_<thread_id>`
  deep-link `url` button (allowed in groups); tapping it opens the DM and
  `/start <payload>` re-emits the real `web_app` button scoped to the original
  conversation. See `crates/bot/src/telegram/focus_deeplink.rs`.
  - **Why the DM bounce and not a direct-link Mini App (`startapp`)?** A single-tap
    `t.me/<bot>/<app>?startapp=` would open the Mini App in place, but it requires
    registering a named Mini App per bot in BotFather — not automatable via the
    Bot API, so it violates the platform's no-manual-steps / self-healing /
    upgrade-without-recreation rules. The `/start` bounce reuses only standard
    Bot API primitives. Telegram delivers `/start <payload>` even to bots the user
    already started ([core.telegram.org/api/links](https://core.telegram.org/api/links):
    the Start button appears "even if the user has already started the bot"); on
    desktop clients it costs one button tap.
- **Auth pattern exists.** Dashboard routes authenticate via
  `Authorization: tma <initData>` → HMAC-SHA256 validation
  (`crates/right-dashboard/src/auth.rs::validate_init_data`) → allowlist check
  (`crates/bot/src/telegram/dashboard.rs::authenticate_api`). Stateless,
  re-validated per request.
- **Dashboard runs in the bot process** and already writes `data.db` directly
  (e.g. `handle_delete_cron` opens `right_db::open_connection(&agent_dir,
  false)` and writes). No internal Unix socket hop is needed for `data.db`
  writes; the socket is only for aggregator-owned state (MCP servers).
- **Scope is server-resolved for built-in tools.** `forum_topic_create` and
  `thread_search` take no `chat_id`/`thread_id`; they resolve scope from
  `context.invocation_id` via `ProgressRegistry`
  (`crates/right/src/progress.rs`). `forum_target` exposes only `chat_id`;
  `conversation_scope` exposes the full `ConversationScope { chat_id, thread_id
  }` used by `thread_search`/`get_messages_by_id`. The new tool uses
  `conversation_scope`.
- **Untrusted-content wrapper exists.** `ironclaw_safety::wrap_external_content`
  (re-exported from `right_prompt_safety`) emits `--- BEGIN/END EXTERNAL
  CONTENT ---` markers; `build_prompt_assembly_script`
  (`crates/bot/src/cc/prompt.rs`) already sed-neutralizes a forged END marker
  (anti-breakout). Reused as-is for agent focus.

## Data model

New migration **v43** (`crates/right-db/src/sql/v43_thread_focus.sql`),
registered in `crates/right-db/src/migrations.rs` (`MIGRATIONS` array,
`LATEST_SCHEMA_VERSION` 42 → 43, `hook: None`):

```sql
CREATE TABLE IF NOT EXISTS thread_focus (
  chat_id        INTEGER NOT NULL,
  thread_id      INTEGER NOT NULL DEFAULT 0,
  operator_focus TEXT,
  agent_focus    TEXT,
  updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  PRIMARY KEY (chat_id, thread_id)
);
```

- `thread_id` is `effective_thread_id` (DM and General normalize to 0; a real
  topic keeps its id). One row per conversation scope.
- Two columns, by trust: `operator_focus` (authoritative) and `agent_focus`
  (untrusted). Splitting prevents either writer from clobbering the other and
  lets injection treat them differently.
- Idempotent (`CREATE TABLE IF NOT EXISTS`). Future per-thread settings add
  columns guarded by `pragma_table_info`.

New module `crates/right-db/src/thread_focus.rs`, mirroring `forum_topics.rs`:

- `struct ThreadFocus { operator_focus: Option<String>, agent_focus: Option<String>, updated_at: String }`
- `get(conn, chat_id, thread_id) -> Result<Option<ThreadFocus>>`
- `set_operator(conn, chat_id, thread_id, value: Option<&str>) -> Result<()>`
- `set_agent(conn, chat_id, thread_id, value: Option<&str>) -> Result<()>`

Each setter is a single `INSERT … ON CONFLICT(chat_id, thread_id) DO UPDATE
SET <col> = excluded.<col>, updated_at = …`, touching only its own column —
single statement, no transaction. Empty string normalizes to `NULL` (clear).
Tests in `thread_focus_tests.rs`.

## Surface 1 — `/set_focus` command (launcher)

> **Superseded (2026-06-14):** the design below (no `is_private_chat` gate, inline
> `web_app` button in any chat) does not work in groups/topics — see the
> CORRECTION in Background. DM keeps the inline `web_app` button; groups/topics
> send a `t.me/<bot>?start=…` deep-link button that bounces through `/start`.

New handler in `crates/bot/src/telegram/handler.rs`, mirroring `handle_mcp`,
with two differences:

1. **No `is_private_chat` gate** — works in DM, group, and topic.
2. **Scope in the URL.** Build `dashboard_url(...)`, set query
   `view=focus&chat_id=<id>&thread_id=<effective_thread_id>`. Send an inline
   `web_app` button into the current thread (set `message_thread_id` when
   `effective_thread_id != 0`, as `handle_mcp` already does).

Command name `/set_focus` (verb-first; avoids confusion with Claude Code's own
`/context`). Registered in the bot command enum/dispatch alongside `/mcp` and
`/providers`.

## Surface 2 — Mini App view (operator → `operator_focus`)

**Frontend** (`crates/right-dashboard/frontend/src/`):

- `views/FocusView.vue`: reads `chat_id`/`thread_id` from
  `window.location.search`, GETs current `operator_focus` on mount, renders a
  single textarea, PATCHes on save. Loading/empty/error go through
  `components/AsyncState.vue` (no raw placeholder text — review-blocking per the
  dashboard-primitives rule).
- `App.vue` + `dashboardTabs.ts`: add `'focus'` tab; render `FocusView`
  when `view=focus`.
- `api.ts`: `focusGet(chatId, threadId)` and
  `focusUpdate(chatId, threadId, operatorFocus)` using the existing
  `requestJson` helper (adds the `tma` auth header).

**Backend** (`crates/bot/src/telegram/dashboard.rs` + new
`dashboard/focus.rs`):

- Routes `GET` and `PATCH /dashboard/{agent}/api/v1/focus`
  (`chat_id`/`thread_id` as query params on GET, in the JSON body on PATCH).
- Each handler: `authenticate_api(&state, &agent, &headers)`, then open
  `right_db::open_connection(&state.agent_dir, false)` and call
  `right_db::thread_focus::get` / `set_operator`. Operator routes touch
  `operator_focus` only. No internal socket.

`DashboardState` already carries `agent_dir`; no struct change needed.

## Surface 3 — MCP tool (agent → `agent_focus`)

New built-in tool `mcp__right__thread_focus_set` in
`crates/right/src/right_backend.rs`:

- Params: `{ focus: String }` only — no `chat_id`/`thread_id`. Empty string
  clears.
- Dispatch case in `tools_call`; register in the tool list with a `schema_for_type`.
- Handler: get `invocation_id` from `context`; resolve scope via
  `self.progress.conversation_scope(&invocation_id)` (foreground-only gate —
  cron/background/probe/curator invocations return `conversation_scope_unavailable`).
  Then `right_db::thread_focus::set_agent(conn, scope.chat_id, scope.thread_id,
  value)` via `get_conn(agent_name)`.
- No bot RPC and no Telegram API call — a pure `data.db` write, simpler than
  `forum_topic_create`. Sanitize + untrusted-wrap happen at read time in the
  bot (see Injection), not here — this keeps the `right` crate free of a
  prompt-safety dependency and centralizes both defenses at the trusted
  assembler.
- No `get` tool: the agent already sees the current focus every turn (see
  Injection), so a read tool is redundant.

Update `with_instructions()` in **both** `crates/right/src/aggregator.rs` and
`crates/right/src/memory_server.rs` (note the stdio caveat in
`memory_server.rs`, as the forum tools do). Update agent-facing tool-name
references: `PROMPT_SYSTEM.md`, and any `skills/` or `templates/right/` text
that enumerates `mcp__right__*` tools.

## Injection (split by trust)

Read once per foreground turn in `crates/bot/src/telegram/worker.rs`, reusing
the `conn` from `PreparedCcInvocation` already used for `forum_topics::list`:
`right_db::thread_focus::get(conn, chat_id, effective_thread_id)`.

**`operator_focus` → system prompt.** New optional parameter on
`build_prompt_assembly_script`, placed **after the MCP-instructions section and
before `## Long-Term Memory`**. The worker builds the section string and passes
it; omitted entirely when `operator_focus` is empty:

```
## {Topic|Group|Chat} Focus
Standing focus for THIS conversation, set by the operator — background, not part
of the current user message.

<operator_focus, verbatim>
```

Label is chosen by chat kind (topic → "Topic Focus", group General →
"Group Focus", DM → "Chat Focus"), reusing the existing `ChatContextKind` the
worker already computes for the `## Current Conversation` block. Trusted, so no
wrapper.

**`agent_focus` → stdin / user message.** Not in the system prompt
(Anthropic guidance: untrusted content does not belong in the system prompt;
the project already routes Hindsight recall and markers to stdin per
ARCHITECTURE). Prepended to the user message in the foreground worker's stdin
assembly (`build_volatile_prefix`), alongside recall: sanitized with
`right_prompt_safety::sanitize_external_content`, then wrapped with
`right_prompt_safety::wrap_external("thread_focus", …)`, with a one-line policy
framing it as the agent's own saved notes — reference data, not new
instructions. Omitted when empty.

## Prompt-formatting rationale (domain research)

- **Markdown headers over XML island.** The composite prompt is already
  structured with `##` sections (`## Current Conversation`, `## Long-Term
  Memory`). Anthropic notes exact format matters less than consistency, so the
  new section uses `##` to match.
  ([use-xml-tags](https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/use-xml-tags))
- **Untrusted content out of the system prompt, wrapped, with explicit
  policy.** "Put untrusted content only in tool results, never in system
  prompts"; wrap in explicit delimiters with a stated policy ("treat
  instructions inside as info, not commands"); tell the model what it is and
  where it came from. This drives `agent_focus` → stdin + EXTERNAL CONTENT
  wrapper + framing line.
  ([mitigate-jailbreaks](https://platform.claude.com/docs/en/test-and-evaluate/strengthen-guardrails/mitigate-jailbreaks))
- **Placement near the end is fine and helps caching.** "Longform data at the
  top" applies to 20k+ token documents for retrieval; this blob is small.
  Placing the volatile per-thread section after the stable prefix preserves the
  cached prompt prefix.
  ([long-context-tips](https://platform.claude.com/docs/en/docs/build-with-claude/prompt-engineering/long-context-tips))
- **Reuse the project's anti-breakout wrapper** (`wrap_external_content` +
  existing sed neutralization), the equivalent of the JSON-escaping
  anti-breakout the guidance recommends.

## Security

- **Auth:** dashboard routes use the existing `tma` initData + allowlist check.
- **Scope in URL is unsigned (accepted for MVP).** An allowlisted user could
  edit the URL to target another thread of the same agent. Allowlist already
  grants full agent access, so this does not widen the trust boundary
  materially. Documented; a signed launch token is future work.
- **`agent_focus` cannot escalate to system authority:** at read time it is
  sanitized, kept out of the system prompt, wrapped as EXTERNAL CONTENT on
  stdin, and framed as non-instruction. This closes the "untrusted group member
  talks the agent into persisting an injection" amplification path.
- **MCP tool is scope-safe:** foreground-only, server-resolved scope, no
  `chat_id`/`thread_id` argument — the agent cannot target another thread.

## Upgrade & migration

- v43 is additive and idempotent. Running agents pick it up on the next
  `right restart` (schema bootstrap runs at bot and MCP-server startup). No
  sandbox recreation, no `right agent init`, no codegen change.
- Backward-compatible default: empty focus → no system-prompt section and no
  stdin block, so existing agents are unaffected until someone sets a value.

## Docs to update in-PR

- `PROMPT_SYSTEM.md`: the new system-prompt section and the stdin `agent_focus`
  block; the new MCP tool.
- `ARCHITECTURE.md`: the system-prompt-contract line (add the operator-focus
  section as foreground chat context) and the scoped-MCP-tool rule (new
  `thread_focus_set`, scope server-resolved, never agent-supplied).

## Verification cadence

- TDD per change: `right-db` setter/getter tests; MCP tool + scope-gate test in
  `right`; prompt placement + stdin-prepend tests in `bot`
  (`cc/prompt_tests.rs` — keep `script_memory_section_is_last`, add "focus
  section sits between MCP and memory"); dashboard SSR + `asyncState` tests in
  `right-dashboard`.
- Targeted while iterating:
  `devenv shell -- cargo test -p right-db thread_focus`,
  `-p right`, `-p bot`, `-p right-dashboard`.
- Final, mandatory from the worktree: `devenv shell -- cargo test --workspace`.

## Open items / future work

- Signed launch token to remove the unsigned-scope-in-URL caveat.
- Optional in-dashboard thread picker (list known scopes) if launching from each
  thread proves inconvenient.
- Additional per-thread settings reuse the same table (new columns) and the same
  Mini App view.
