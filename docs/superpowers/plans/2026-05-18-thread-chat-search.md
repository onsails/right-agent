# Thread And Chat Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add scoped Hermes-style transcript search with `mcp__right__thread_search` and `mcp__right__chat_search`.

**Architecture:** Archive new Telegram messages into per-agent SQLite, index them with FTS5, and expose search through the existing Right MCP backend. Scope is derived from server-side foreground invocation registration; the agent never supplies chat/thread identifiers.

**Tech Stack:** Rust 2024, rusqlite, rusqlite_migration, SQLite FTS5, teloxide, rmcp, axum internal API.

---

## Assumptions And Boundaries

- No backfill. Only newly observed messages become searchable.
- Group archive runs before the routing filter: every group message Teloxide delivers is archived, including closed, untrusted, unaddressed messages.
- DM archive runs after auth/token intercepts and existing routing checks. Do not retain setup tokens or untrusted random DMs.
- `thread_search` searches only `(current chat_id, current effective_thread_id)`.
- `chat_search` searches only `current chat_id`: in a DM only that DM, in a group the whole group across topics.
- Search tool schemas expose only `query` and optional `limit`.
- Use SQLite FTS5 only. Do not add vector search in this implementation.
- Before implementation, load `rust-dev:rust-dev` if available. If unavailable, state that and follow the repo's Rust conventions.

## File Map

- `crates/right-db/src/sql/v21_conversation_messages.sql`: new transcript table, indexes, FTS5 table, triggers.
- `crates/right-db/src/migrations.rs`: v21 registration and migration tests.
- `crates/right-db/src/conversation.rs`: archive, mark-routed, assistant insert, FTS query normalization, scoped search helpers.
- `crates/right-db/src/lib.rs`: module export.
- `crates/right-db/tests/smoke.rs`: schema version and table smoke checks.
- `crates/bot/src/telegram/archive.rs`: Teloxide message to transcript row conversion.
- `crates/bot/src/telegram/mod.rs`: archive module export.
- `crates/bot/src/telegram/dispatch.rs`: pre-routing group archive.
- `crates/bot/src/telegram/handler.rs`: routed DM archive after intercepts.
- `crates/bot/src/telegram/worker.rs`: mark routed rows and archive successful assistant replies.
- `crates/right-mcp/src/internal_client.rs`: progress registration carries optional chat/thread scope.
- `crates/right/src/progress.rs`: invocation registry stores conversation scope.
- `crates/right/src/internal_api.rs`: `/progress/register` accepts scope.
- `crates/right/src/right_backend.rs`: `thread_search` and `chat_search` tools.
- `crates/right/src/right_backend_tests.rs`: backend schema/scope/search tests.
- `crates/right/src/aggregator.rs`: tool list expectation and instructions.
- `crates/right/src/memory_server.rs`: stdio instructions.
- `PROMPT_SYSTEM.md`, `ARCHITECTURE.md`, `docs/architecture/memory.md`, `docs/architecture/sessions.md`: agent semantics and architecture docs.

## Baseline

- [ ] **Step 1: Verify current baseline before edits**

Run:

```bash
devenv shell -- cargo test -p right-db
devenv shell -- cargo test -p right-bot telegram
devenv shell -- cargo test -p right right_backend
```

Expected: all exit 0. If one fails before edits, record the failing test names in the implementation notes and avoid using that failure as evidence for this feature.

---

### Task 1: Add SQLite Transcript Storage

**Files:**
- Create: `crates/right-db/src/sql/v21_conversation_messages.sql`
- Create: `crates/right-db/src/conversation.rs`
- Modify: `crates/right-db/src/migrations.rs`
- Modify: `crates/right-db/src/lib.rs`
- Modify: `crates/right-db/tests/smoke.rs`

- [ ] **Step 1: Add failing migration tests**

In `crates/right-db/src/migrations.rs`, add tests named:

- `conversation_messages_schema_exists`
- `conversation_messages_unique_inbound_message`
- `conversation_messages_fts_tracks_updates`

Assertions:

- `conversation_messages` table exists after `MIGRATIONS.to_latest`.
- `conversation_messages_fts` table exists after `MIGRATIONS.to_latest`.
- two inbound rows with `('telegram', chat_id, message_id, 'user')` violate uniqueness.
- updating `content` removes the old term from FTS and inserts the new term.

Run:

```bash
devenv shell -- cargo test -p right-db conversation_messages
```

Expected: fails because v21 does not exist.

- [ ] **Step 2: Add v21 SQL**

In `crates/right-db/src/sql/v21_conversation_messages.sql`, create:

```sql
CREATE TABLE IF NOT EXISTS conversation_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    platform TEXT NOT NULL DEFAULT 'telegram',
    chat_id INTEGER NOT NULL,
    thread_id INTEGER NOT NULL DEFAULT 0,
    message_id INTEGER,
    sender_user_id INTEGER,
    sender_name TEXT,
    addressed_to_bot INTEGER NOT NULL DEFAULT 0 CHECK (addressed_to_bot IN (0, 1)),
    routed_to_agent INTEGER NOT NULL DEFAULT 0 CHECK (routed_to_agent IN (0, 1)),
    root_session_id TEXT,
    turn_id INTEGER,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_conversation_messages_inbound_unique
ON conversation_messages (platform, chat_id, message_id, role)
WHERE message_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_conversation_messages_thread_created
ON conversation_messages (platform, chat_id, thread_id, created_at);

CREATE INDEX IF NOT EXISTS idx_conversation_messages_chat_created
ON conversation_messages (platform, chat_id, created_at);

CREATE INDEX IF NOT EXISTS idx_conversation_messages_session_turn
ON conversation_messages (root_session_id, turn_id)
WHERE root_session_id IS NOT NULL;

CREATE VIRTUAL TABLE IF NOT EXISTS conversation_messages_fts USING fts5(
    content,
    content='conversation_messages',
    content_rowid='id'
);
```

Also add insert/delete/update sync triggers matching the existing `memories_fts` trigger style in `crates/right-db/src/sql/v1_schema.sql`.

- [ ] **Step 3: Register migration and smoke version**

In `crates/right-db/src/migrations.rs`, add `V21_SCHEMA` and append `M::up(V21_SCHEMA)` after v20.

In `crates/right-db/tests/smoke.rs`, change latest schema version from `20` to `21`, and add `schema_has_conversation_messages_table` asserting both `conversation_messages` and `conversation_messages_fts` exist.

Run:

```bash
devenv shell -- cargo test -p right-db conversation_messages
devenv shell -- cargo test -p right-db open_connection_applies_migrations
```

Expected: both exit 0.

- [ ] **Step 4: Add failing helper tests**

In `crates/right-db/src/conversation.rs`, write tests named:

- `archive_message_is_idempotent_for_inbound_telegram_message`
- `mark_routed_sets_session_and_turn`
- `thread_search_filters_to_current_thread`
- `chat_search_includes_all_threads_in_current_chat`
- `empty_search_query_is_rejected`

Use in-memory SQLite with `crate::MIGRATIONS.to_latest`. Insert rows through the public helper API, not raw SQL except for assertions.

Run:

```bash
devenv shell -- cargo test -p right-db conversation
```

Expected: compile fails because the helper API does not exist.

- [ ] **Step 5: Implement helper API**

In `crates/right-db/src/conversation.rs`, implement:

```rust
pub enum ConversationRole { User, Assistant }

pub struct ConversationMessage<'a> {
    pub platform: &'a str,
    pub chat_id: i64,
    pub thread_id: i64,
    pub message_id: Option<i32>,
    pub sender_user_id: Option<i64>,
    pub sender_name: Option<&'a str>,
    pub addressed_to_bot: bool,
    pub routed_to_agent: bool,
    pub root_session_id: Option<&'a str>,
    pub turn_id: Option<u64>,
    pub role: ConversationRole,
    pub content: &'a str,
}

pub struct ConversationSearchResult {
    pub id: i64,
    pub role: String,
    pub snippet: String,
    pub sender_user_id: Option<i64>,
    pub sender_name: Option<String>,
    pub created_at: String,
    pub thread_id: i64,
    pub message_id: Option<i32>,
    pub root_session_id: Option<String>,
}
```

Public functions:

- `archive_message(conn, message) -> rusqlite::Result<i64>`
- `mark_routed(conn, platform, chat_id, message_id, root_session_id, turn_id) -> rusqlite::Result<usize>`
- `search_thread(conn, query, limit, chat_id, thread_id) -> rusqlite::Result<Vec<ConversationSearchResult>>`
- `search_chat(conn, query, limit, chat_id) -> rusqlite::Result<Vec<ConversationSearchResult>>`

Rules:

- Trim content and reject empty content.
- Inbound rows use `ON CONFLICT(platform, chat_id, message_id, role) DO UPDATE`.
- Assistant rows have `message_id = NULL` and use plain insert.
- Convert `turn_id` to `i64` with checked conversion.
- Clamp `limit` to `1..=50`.
- Normalize FTS input by retaining alphanumeric/underscore terms, quoting each term, and joining terms with `AND`.
- Reject normalized empty queries.
- `search_thread` SQL must include `m.chat_id = ?` and `m.thread_id = ?`.
- `search_chat` SQL must include `m.chat_id = ?` and no thread predicate.

In `crates/right-db/src/lib.rs`, export:

```rust
pub mod conversation;
```

Run:

```bash
devenv shell -- cargo test -p right-db conversation
devenv shell -- cargo test -p right-db --test smoke
```

Expected: both exit 0.

- [ ] **Step 6: Commit DB slice**

Run:

```bash
git add crates/right-db/src/sql/v21_conversation_messages.sql crates/right-db/src/migrations.rs crates/right-db/src/conversation.rs crates/right-db/src/lib.rs crates/right-db/tests/smoke.rs
git commit -m "feat(memory): add conversation transcript storage"
```

---

### Task 2: Archive Telegram Messages

**Files:**
- Create: `crates/bot/src/telegram/archive.rs`
- Modify: `crates/bot/src/telegram/mod.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/filter.rs`

- [ ] **Step 1: Add failing archive tests**

In `crates/bot/src/telegram/archive.rs`, add tests named:

- `archive_content_uses_text`
- `archive_content_uses_caption`
- `archive_content_records_media_without_text`
- `group_messages_are_archivable_before_routing`

Build Teloxide `Message` values with `serde_json::json!`. Expected content:

- text message: raw trimmed text.
- photo with caption: `"caption\n[photo]"`.
- document without text and filename `plan.pdf`: `"[document: plan.pdf]"`.
- supergroup message returns true from `should_archive_seen_group_message`.

Run:

```bash
devenv shell -- cargo test -p right-bot telegram::archive
```

Expected: compile fails because the module does not exist.

- [ ] **Step 2: Implement archive helpers**

In `crates/bot/src/telegram/archive.rs`, implement:

- `should_archive_seen_group_message(msg: &Message) -> bool`: true for non-private chats.
- `archive_content(msg: &Message) -> Option<String>`: combine trimmed `text`/`caption` with attachment labels from `super::attachments::extract_attachments`.
- `archive_seen_group_message(agent_dir: &Path, identity: &BotIdentity, msg: &Message)`: open DB with `migrate=false`, compute `effective_thread_id`, `sender_user_id`, `sender_name`, `addressed_to_bot`, and archive a user row. Log archive errors; do not return errors.
- `archive_routed_dm_message(agent_dir: &Path, msg: &Message, address: Option<AddressKind>)`: private-chat only, same archive flow, called only after intercepts.

In `crates/bot/src/telegram/mod.rs`, add:

```rust
pub(crate) mod archive;
```

- [ ] **Step 3: Wire pre-routing group archive**

In `crates/bot/src/telegram/dispatch.rs`, capture:

```rust
let archive_agent_dir = Arc::clone(&agent_dir_arc);
let archive_identity = Arc::clone(&identity_arc);
```

In the first `Update::filter_message().inspect(...)`, call:

```rust
super::archive::archive_seen_group_message(
    &archive_agent_dir.0,
    archive_identity.as_ref(),
    &msg,
);
```

This must happen before `.filter_map(filter)`.

- [ ] **Step 4: Wire routed DM archive after intercepts**

In `crates/bot/src/telegram/handler.rs`, after auth-code and MCP-token intercept blocks and before creating `SessionKey`, call:

```rust
super::archive::archive_routed_dm_message(&agent_dir.0, &msg, decision.address.clone());
```

- [ ] **Step 5: Add routing regression test**

In `crates/bot/src/telegram/filter.rs`, add `unaddressed_group_message_still_dropped_by_routing_filter`. Use an open group, an unaddressed text message, and assert `make_routing_filter(...)(msg).is_none()`. This verifies archival did not change invocation routing.

- [ ] **Step 6: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right-bot telegram::archive
devenv shell -- cargo test -p right-bot telegram::filter
devenv shell -- cargo test -p right-bot telegram::dispatch::tests::dispatcher_builds_without_panic
```

Expected: all exit 0.

Commit:

```bash
git add crates/bot/src/telegram/archive.rs crates/bot/src/telegram/mod.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/filter.rs
git commit -m "feat(memory): archive telegram transcript messages"
```

---

### Task 3: Link Transcript Rows To Agent Turns

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add failing helper tests**

In `crates/bot/src/telegram/worker.rs`, add tests named:

- `routed_message_ids_preserve_batch_order`: two `DebounceMsg` values with ids `10`, `11` produce `vec![10, 11]`.
- `assistant_archive_content_trims_empty_reply`: `"  hello  "` returns `"hello"`, whitespace returns `None`.

Run:

```bash
devenv shell -- cargo test -p right-bot routed_message_ids
devenv shell -- cargo test -p right-bot assistant_archive_content
```

Expected: compile fails because the helpers do not exist.

- [ ] **Step 2: Implement helper functions**

In `crates/bot/src/telegram/worker.rs`, add:

```rust
fn routed_message_ids(batch: &[DebounceMsg]) -> Vec<i32> {
    batch.iter().map(|message| message.message_id).collect()
}

fn assistant_archive_content(content: &str) -> Option<String> {
    let trimmed = content.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
```

- [ ] **Step 3: Mark inbound rows with session and turn**

Change `invoke_cc` to accept `routed_message_ids: &[i32]`.

After `turn_id` and `session_uuid` are known, call `right_db::conversation::mark_routed` for every routed message id. Log missing rows and errors; never fail the invocation because archive metadata failed.

- [ ] **Step 4: Return turn id to the caller**

Add `turn_id: u64` to `CcReply`. Every `Ok(CcReply { ... })` in `invoke_cc` must include the current turn id.

In the caller, preserve `turn_id: Option<u64>` alongside `reply_result`, `session_uuid`, and `is_first_call`.

- [ ] **Step 5: Archive successful assistant replies**

In the normal success path after Telegram text send attempts:

- track `sent_any_text`, true only if an HTML send or plain fallback succeeds.
- if `sent_any_text`, `turn_id.is_some()`, and `assistant_archive_content(&content).is_some()`, insert an assistant `ConversationMessage` with `message_id = None`, `sender_name = ctx.agent_name`, `root_session_id = session_uuid`, and the same `turn_id`.
- log archive errors; do not alter user-visible reply behavior.

- [ ] **Step 6: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right-bot routed_message_ids
devenv shell -- cargo test -p right-bot assistant_archive_content
devenv shell -- cargo test -p right-bot telegram::worker
```

Expected: all exit 0.

Commit:

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(memory): link transcript rows to agent turns"
```

---

### Task 4: Carry Conversation Scope In Invocation State

**Files:**
- Modify: `crates/right-mcp/src/internal_client.rs`
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/right/src/progress.rs`
- Modify: `crates/right/src/internal_api.rs`

- [ ] **Step 1: Add failing serialization and registry tests**

In `crates/right-mcp/src/internal_client.rs`, update `progress_register_request_serializes_expected_fields` to assert `chat_id: Some(100)` and `thread_id: Some(7)` serialize as `100` and `7`.

In `crates/right/src/progress.rs`, add:

- `conversation_scope_available_for_foreground_invocation`
- `conversation_scope_rejects_missing_or_nonforeground_invocation`

Run:

```bash
devenv shell -- cargo test -p right-mcp progress_register_request_serializes_expected_fields
devenv shell -- cargo test -p right conversation_scope
```

Expected: compile fails because scope fields and registry API do not exist.

- [ ] **Step 2: Add request scope fields**

In `crates/right-mcp/src/internal_client.rs`, add to `ProgressRegisterRequest`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub chat_id: Option<i64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub thread_id: Option<i64>,
```

Update the `Debug` impl and all request literals.

- [ ] **Step 3: Send scope from foreground worker**

In `crates/bot/src/telegram/worker.rs`, update `start_progress_invocation` so `ProgressRegisterRequest` includes:

```rust
chat_id: Some(chat_id),
thread_id: Some(eff_thread_id),
```

- [ ] **Step 4: Store and expose scope in registry**

In `crates/right/src/progress.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConversationScope {
    pub(crate) chat_id: i64,
    pub(crate) thread_id: i64,
}
```

Add `conversation_scope: Option<ConversationScope>` to `ProgressRegistration` and internal `ProgressInvocation`.

Add `ProgressRegistry::conversation_scope(&self, invocation_id: &str) -> Result<ConversationScope, ProgressError>`:

- unknown invocation: `ProgressError::Unavailable`
- non-foreground invocation: `ProgressError::Forbidden`
- foreground without scope: `ProgressError::Unavailable`

- [ ] **Step 5: Accept scope in internal API**

In `crates/right/src/internal_api.rs`, build `Some(ConversationScope { chat_id, thread_id })` only when both request fields are present, and pass it to `ProgressRegistration`.

- [ ] **Step 6: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right-mcp progress_register_request
devenv shell -- cargo test -p right conversation_scope
devenv shell -- cargo test -p right internal_api::tests::progress_register_adds_foreground_invocation
devenv shell -- cargo test -p right-bot progress_registration_target_uses_effective_thread_id
```

Expected: all exit 0.

Commit:

```bash
git add crates/right-mcp/src/internal_client.rs crates/bot/src/telegram/worker.rs crates/right/src/progress.rs crates/right/src/internal_api.rs
git commit -m "feat(memory): bind mcp invocations to chat scope"
```

---

### Task 5: Expose Scoped MCP Search Tools

**Files:**
- Modify: `crates/right/src/right_backend.rs`
- Modify: `crates/right/src/right_backend_tests.rs`
- Modify: `crates/right/src/aggregator.rs`

- [ ] **Step 1: Add failing backend tests**

In `crates/right/src/right_backend_tests.rs`:

- change `tools_list_returns_expected_count` from `12` to `14`.
- add `tools_list_includes_conversation_search_tools_without_scope_params`.
- add `thread_search_without_invocation_scope_returns_tool_error`, expecting `conversation_scope_unavailable`.
- add `thread_search_filters_current_thread`, with rows in chat `100` thread `7`, chat `100` thread `8`, and chat `200` thread `7`; only message id `1` returns.
- add `chat_search_includes_other_threads_in_same_chat`, with the same rows; message ids `2` and `1` return, and chat `200` does not.
- assert both schemas lack `chat_id`, `thread_id`, `scope`, `user_id`, and `session_id`.

Run:

```bash
devenv shell -- cargo test -p right thread_search
devenv shell -- cargo test -p right chat_search
```

Expected: compile fails because the tools do not exist.

- [ ] **Step 2: Add params and tool definitions**

In `crates/right/src/right_backend.rs`, add:

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ConversationSearchParams {
    pub(crate) query: String,
    pub(crate) limit: Option<usize>,
}
```

Add `Tool::new("thread_search", ...)` and `Tool::new("chat_search", ...)` to `tools_list`. Descriptions must state that scope is server-enforced and not agent-controlled.

- [ ] **Step 3: Add dispatch and implementations**

In `tools_call`, dispatch `thread_search` and `chat_search`.

Implement both methods with these rules:

- parse params with serde; invalid shape returns `invalid_argument`.
- trim `query`; empty query returns `invalid_argument`.
- require `context.invocation_id`.
- look up `self.progress.conversation_scope(invocation_id).await`.
- `ProgressError::Unavailable` or `Forbidden` returns `conversation_scope_unavailable`.
- call `right_db::conversation::search_thread` or `search_chat`.
- return JSON text with `scope` and `results`.
- each result includes `snippet`, `role`, `sender_user_id`, `sender_name`, `created_at`, `thread_id`, `message_id`, and `root_session_id`.
- do not include `chat_id` in the response.

- [ ] **Step 4: Update aggregator tests**

In `crates/right/src/aggregator.rs`, update `tools_list_includes_right_and_meta` to assert `thread_search` and `chat_search` are present.

- [ ] **Step 5: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right right_backend_tests::tools_list
devenv shell -- cargo test -p right thread_search
devenv shell -- cargo test -p right chat_search
devenv shell -- cargo test -p right aggregator::tests::tools_list_includes_right_and_meta
```

Expected: all exit 0.

Commit:

```bash
git add crates/right/src/right_backend.rs crates/right/src/right_backend_tests.rs crates/right/src/aggregator.rs
git commit -m "feat(memory): expose scoped conversation search tools"
```

---

### Task 6: Update Agent Semantics And Docs

**Files:**
- Modify: `crates/right/src/aggregator.rs`
- Modify: `crates/right/src/memory_server.rs`
- Modify: `PROMPT_SYSTEM.md`
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/memory.md`
- Modify: `docs/architecture/sessions.md`

- [ ] **Step 1: Update MCP instructions**

In both `crates/right/src/aggregator.rs` and `crates/right/src/memory_server.rs`, add a `Conversation Search` section:

```text
- mcp__right__thread_search: Search exact archived transcript messages in the current Telegram chat/thread only. Use for "what did we say in this topic/thread?"
- mcp__right__chat_search: Search exact archived transcript messages in the current Telegram chat. In a DM this searches only that DM; in a group this searches the whole group across topics.
Use conversation search, not memory_recall, when the user asks for exact past wording or past messages.
```

For `memory_server.rs`, also state that stdio mode lacks foreground HTTP scope and returns `conversation_scope_unavailable`.

- [ ] **Step 2: Update prompt docs**

In `PROMPT_SYSTEM.md`, document three tiers:

- current session context: Claude `--resume`.
- conversation search: exact local transcript search through `mcp__right__thread_search` and `mcp__right__chat_search`.
- semantic memory: Hindsight `memory_recall` / `memory_reflect`, not authoritative transcript search.

Also add the two prefixed tools anywhere built-in Right MCP tools are enumerated.

- [ ] **Step 3: Update architecture docs**

In `ARCHITECTURE.md`, add the prescriptive rule:

```text
Conversation search scope is server-enforced. `mcp__right__thread_search` searches only the current `(chat_id, effective_thread_id)`. `mcp__right__chat_search` searches only the current `chat_id`; in DMs this is only that DM, and in groups this is the whole group across topics. Agents must never be allowed to pass chat_id, thread_id, user ids, session ids, or a broader scope to these tools.
```

In `docs/architecture/memory.md`, state that transcript search is local SQLite FTS5 and separate from Hindsight.

In `docs/architecture/sessions.md`, describe group pre-routing archive, routed DM archive after intercepts, routed user row marking, and assistant row insertion.

- [ ] **Step 4: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right memory_server_mcp_tests
devenv shell -- cargo test -p right aggregator::tests::tools_list_includes_right_and_meta
```

Expected: both exit 0.

Commit:

```bash
git add crates/right/src/aggregator.rs crates/right/src/memory_server.rs PROMPT_SYSTEM.md ARCHITECTURE.md docs/architecture/memory.md docs/architecture/sessions.md
git commit -m "docs(memory): document scoped conversation search"
```

---

### Task 7: Final Verification

**Files:**
- All files changed by Tasks 1-6.

- [ ] **Step 1: Run full workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: exits 0.

- [ ] **Step 2: Run full workspace build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: exits 0.

- [ ] **Step 3: Inspect working tree**

Run:

```bash
git status --short
```

Expected: no uncommitted implementation files. If docs/superpowers files remain uncommitted, commit them before completion.

- [ ] **Step 4: Manual boundary checklist**

Confirm:

- `thread_search` schema exposes only `query` and `limit`.
- `chat_search` schema exposes only `query` and `limit`.
- search scope comes only from `ProgressRegistry`.
- group archive runs before `make_routing_filter`.
- DM archive runs after auth/token intercepts.
- `thread_search` SQL filters by `chat_id` and `thread_id`.
- `chat_search` SQL filters by `chat_id`.
- no vector-search dependency was added.
- `PROMPT_SYSTEM.md`, `ARCHITECTURE.md`, `docs/architecture/memory.md`, and `docs/architecture/sessions.md` match the shipped semantics.
