# Reply-Body Fetch-On-Demand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop inlining replied-to message bodies that the agent can recover, by stripping the body when the message is in the conversation archive and adding a scope-enforced `get_messages_by_id` MCP tool to fetch it on demand.

**Architecture:** A new `right_db::conversation::fetch_by_ids` reader backs both a new foreground-only `get_messages_by_id` MCP tool (scope resolved server-side, like `thread_search`) and a conditional strip in the worker: when a reply's target is in the archive for the current `(chat_id, thread_id)`, the YAML emits author + a fetch note instead of the full body (keeping `reply_to_id` and `quoted_text`); when it isn't archived (only source is the reply payload), the body is inlined as today.

**Tech Stack:** Rust (edition 2024), tokio, Turso via `right-db`, rmcp. Crates: `right-db`, `right`, `right-bot`. Spec: `docs/superpowers/specs/2026-06-03-reply-body-fetch-on-demand-design.md`.

**Conventions:** FAIL FAST (`?` / `{:#}`). MCP tool-name changes require updating `with_instructions()` in BOTH `aggregator.rs` and `memory_server.rs`, and using the `mcp__right__` prefix in agent-facing text. Spec 1 already modified `attachments.rs` and `worker.rs`, so edits there are anchored by code pattern, not line number. Final `devenv shell -- cargo test --workspace` is mandatory.

---

## File Structure

- `crates/right-db/src/conversation.rs` — `FetchedMessage` + `fetch_by_ids` (and `fetched_from_row`).
- `crates/right/src/right_backend.rs` — `GetMessagesByIdParams`, tool def, dispatch arm, `call_get_messages_by_id` handler.
- `crates/bot/src/cc/invocation.rs` — add `get_messages_by_id` to `disallow_conversation_search`.
- `crates/right/src/aggregator.rs`, `crates/right/src/memory_server.rs` — `with_instructions()` inventory line.
- `crates/bot/src/telegram/attachments.rs` — `ReplyToBody.omitted` + omitted emission in `format_cc_input`.
- `crates/bot/src/telegram/worker.rs` — conditional strip in the `reply_to_body` transform.
- `PROMPT_SYSTEM.md`, `ARCHITECTURE.md` — doc sync.

---

## Task 1: `fetch_by_ids` reader in right-db

**Files:**
- Modify: `crates/right-db/src/conversation.rs` (add type + fn near `search_chat`, after line ~232; tests in the existing `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing tests**

In the `conversation.rs` test module (uses `migrated_connection()`, `user_message(chat_id, thread_id, message_id, content)`, `archive_message`), add:

```rust
    #[tokio::test]
    async fn fetch_by_ids_returns_matching_scoped_messages() {
        let conn = migrated_connection().await;
        archive_message(&conn, user_message(100, 10, 25, "hello")).await.unwrap();
        archive_message(&conn, user_message(100, 10, 26, "world")).await.unwrap();
        archive_message(&conn, user_message(100, 99, 27, "other thread")).await.unwrap();

        let rows = fetch_by_ids(&conn, "telegram", 100, 10, &[25, 26, 27, 999])
            .await
            .unwrap();

        let ids: Vec<Option<i32>> = rows.iter().map(|r| r.message_id).collect();
        // 27 is in thread 99 (out of scope); 999 does not exist.
        assert_eq!(ids, vec![Some(25), Some(26)]);
        assert_eq!(rows[0].text, "hello");
        assert_eq!(rows[0].role, "user");
    }

    #[tokio::test]
    async fn fetch_by_ids_empty_input_returns_empty() {
        let conn = migrated_connection().await;
        let rows = fetch_by_ids(&conn, "telegram", 100, 10, &[]).await.unwrap();
        assert!(rows.is_empty());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-db fetch_by_ids`
Expected: FAIL to compile — `fetch_by_ids` / `FetchedMessage` not found.

- [ ] **Step 3: Implement**

In `crates/right-db/src/conversation.rs`, after `search_chat` (line ~232), add:

```rust
/// A message fetched by id for on-demand reply recovery.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchedMessage {
    pub message_id: Option<i32>,
    pub sender_name: Option<String>,
    pub text: String,
    pub role: String,
}

/// Fetch archived messages by telegram message id, scoped to one
/// `(chat_id, thread_id)`. Ids outside the scope or not archived are absent
/// from the result. Empty `message_ids` returns an empty Vec.
pub async fn fetch_by_ids(
    conn: &Connection,
    platform: &str,
    chat_id: i64,
    thread_id: i64,
    message_ids: &[i32],
) -> Result<Vec<FetchedMessage>> {
    if message_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat("?")
        .take(message_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT message_id, sender_name, content, role
         FROM conversation_messages
         WHERE platform = ? AND chat_id = ? AND thread_id = ?
           AND message_id IN ({placeholders})
         ORDER BY message_id ASC"
    );
    let mut params = crate::params::ParamsBuilder::new();
    params.push(platform)?;
    params.push(chat_id)?;
    params.push(thread_id)?;
    for id in message_ids {
        params.push(*id)?;
    }
    conn.query_all(&sql, params, fetched_from_row).await
}

fn fetched_from_row(row: &crate::row::Row<'_>) -> Result<FetchedMessage> {
    Ok(FetchedMessage {
        message_id: row.get(0)?,
        sender_name: row.get(1)?,
        text: row.get(2)?,
        role: row.get(3)?,
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-db fetch_by_ids`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-db/src/conversation.rs
git commit -m "feat(right-db): fetch_by_ids — scoped message lookup by id"
```

---

## Task 2: `ReplyToBody.omitted` + omitted emission in `format_cc_input`

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs` (`ReplyToBody` struct; `format_cc_input` reply emission)
- Modify: `crates/bot/src/telegram/handler.rs` (`ReplyToBody { … }` construction — set `omitted: false`)
- Test: `crates/bot/src/telegram/attachments.rs` test module

- [ ] **Step 1: Add the field and fix constructors**

In `crates/bot/src/telegram/attachments.rs`, add a field to `ReplyToBody`:

```rust
pub struct ReplyToBody {
    pub author: MessageAuthor,
    pub text: Option<String>,
    pub attachments: Vec<ResolvedAttachment>,
    /// True when the body was stripped (recoverable from the archive); the
    /// YAML then emits author + a fetch note instead of text/attachments.
    pub omitted: bool,
}
```

Run `rg -n 'ReplyToBody \{' crates/bot/src` and add `omitted: false,` to every constructor (the real one in `handler.rs`, and the `Some(ReplyToBody { … })` literals in the `attachments.rs` test module). This keeps current behavior (nothing stripped yet).

- [ ] **Step 2: Write the failing test**

In the `attachments.rs` test module, add (mirror the existing `format_cc_input_*` `InputMessage` literal style; set the new `omitted` field):

```rust
    #[tokio::test]
    async fn format_cc_input_omitted_reply_emits_note_not_body() {
        let m = InputMessage {
            message_id: 2,
            text: Some("and this?".into()),
            timestamp: Utc::now(),
            attachments: vec![],
            author: MessageAuthor { name: "Bob".into(), username: None, user_id: Some(7) },
            forward_info: None,
            reply_to_id: Some(41),
            quoted_text: None,
            chat: ChatContext::Group { id: -100, title: Some("T".into()), topic_id: Some(3) },
            reply_to_body: Some(ReplyToBody {
                author: MessageAuthor { name: "Alice".into(), username: Some("@alice".into()), user_id: Some(9) },
                text: Some("SECRET BODY".into()),
                attachments: vec![],
                omitted: true,
            }),
        };
        let yaml = format_cc_input(&[m]).unwrap();
        assert!(yaml.contains("reply_to_id: 41"), "reply_to_id kept");
        assert!(yaml.contains("Alice"), "author kept");
        assert!(yaml.contains("get_messages_by_id"), "fetch note present");
        assert!(!yaml.contains("SECRET BODY"), "stripped body must not appear");
    }
```

- [ ] **Step 2b: Run it to verify it fails**

Run: `devenv shell -- cargo test -p right-bot format_cc_input_omitted_reply`
Expected: FAIL — the stripped body still appears (omitted not yet handled).

- [ ] **Step 3: Implement omitted emission**

In `format_cc_input`, locate the reply-body emission block (anchor: `if let Some(ref r) = m.reply_to_body {`). Inside it, after the `author:` sub-block is written, branch on `r.omitted`: when omitted, write the note and skip text + attachments; otherwise keep the existing `text` + `attachments` emission. Concretely, wrap the existing `text`/`attachments` writes:

```rust
            if r.omitted {
                out.push_str(
                    "      note: \"body omitted — fetch with mcp__right__get_messages_by_id if not in your context\"\n",
                );
            } else {
                if let Some(ref t) = r.text {
                    writeln!(out, "      text: \"{}\"", yaml_escape_string(t)).expect("infallible");
                }
                if !r.attachments.is_empty() {
                    // ...keep the existing reply_to attachments emission unchanged...
                }
            }
```

(Move the existing `r.text` and `r.attachments` emission into the `else` arm verbatim; the `author:` emission stays above the branch and is always written.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot format_cc_input`
Expected: PASS, including the new test and the existing reply tests (which use `omitted: false` → unchanged output).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs crates/bot/src/telegram/handler.rs
git commit -m "feat(attachments): ReplyToBody.omitted — emit fetch note instead of body"
```

---

## Task 3: `get_messages_by_id` MCP tool

**Files:**
- Modify: `crates/right/src/right_backend.rs` (param struct near `ConversationSearchParams`; tool def beside `thread_search`/`chat_search`; dispatch arm; handler)
- Test: `crates/right/src/aggregator.rs` test module (advertisement)

- [ ] **Step 1: Write the failing advertisement test**

In the `aggregator.rs` test module (Task is post-spec-2; `tools_list` is `async`), add:

```rust
    #[tokio::test]
    async fn tools_list_includes_get_messages_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_dispatcher(tmp.path());
        let tools = dispatcher.tools_list("test-agent").await;
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains(&"get_messages_by_id"), "missing get_messages_by_id");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `devenv shell -- cargo test -p right tools_list_includes_get_messages_by_id`
Expected: FAIL — tool not registered.

- [ ] **Step 3: Add the param struct**

In `crates/right/src/right_backend.rs`, near `ConversationSearchParams`:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetMessagesByIdParams {
    /// Telegram message ids to fetch. Resolved within the CURRENT chat/topic
    /// only — you cannot fetch from other chats.
    pub(crate) message_ids: Vec<i32>,
}
```

- [ ] **Step 4: Register the tool definition**

In the tool-list builder, beside the `thread_search` / `chat_search` entries, add an entry in the same form the file uses (name, description, `schema_for_type`):

```rust
            (
                "get_messages_by_id",
                "Fetch the full content of messages in the CURRENT chat/topic by their ids. \
                 Scope is server-enforced from the current invocation — you cannot fetch from \
                 other chats. Use this to read a replied-to message that isn't already in your \
                 context, or to revisit an earlier message.",
                schema_for_type::<GetMessagesByIdParams>(),
            ),
```

(Match the exact entry shape of the adjacent `thread_search` entry — tuple vs builder call — in this file.)

- [ ] **Step 5: Add the dispatch arm**

In the tool-name `match` (beside `"thread_search"` / `"forum_topic_edit"`), add:

```rust
            "get_messages_by_id" => {
                self.call_get_messages_by_id(agent_name, context, &args).await
            }
```

- [ ] **Step 6: Implement the handler**

Add a method on the same impl as `call_forum_topic_create`, mirroring the conversation-search scope resolution:

```rust
    async fn call_get_messages_by_id(
        &self,
        agent_name: &str,
        context: crate::progress::ToolCallContext,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: GetMessagesByIdParams = match serde_json::from_value(args.clone()) {
            Ok(p) => p,
            Err(e) => {
                return Ok(tool_error(
                    "invalid_argument",
                    format!("invalid get_messages_by_id params: {e:#}"),
                    None,
                ));
            }
        };

        let Some(invocation_id) = context.invocation_id else {
            return Ok(conversation_scope_unavailable());
        };
        let scope = match self.progress.conversation_scope(&invocation_id).await {
            Ok(scope) => scope,
            Err(_) => return Ok(conversation_scope_unavailable()),
        };

        let conn_arc = self.get_conn(agent_name).await?;
        let conn = conn_arc.lock().await;
        let rows = right_db::conversation::fetch_by_ids(
            &conn,
            "telegram",
            scope.chat_id,
            scope.thread_id,
            &params.message_ids,
        )
        .await
        .map_err(|e| anyhow::anyhow!("get_messages_by_id failed: {e}"))?;

        let messages: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|row| {
                serde_json::json!({
                    "message_id": row.message_id,
                    "sender_name": row.sender_name,
                    "text": row.text,
                    "role": row.role,
                })
            })
            .collect();

        let output = serde_json::json!({ "messages": messages });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&output)?,
        )]))
    }
```

- [ ] **Step 7: Run the advertisement test + build**

Run: `devenv shell -- cargo test -p right tools_list_includes_get_messages_by_id && devenv shell -- cargo build -p right`
Expected: PASS + compiles.

- [ ] **Step 8: Commit**

```bash
git add crates/right/src/right_backend.rs crates/right/src/aggregator.rs
git commit -m "feat(right-backend): get_messages_by_id scope-enforced fetch tool"
```

---

## Task 4: Foreground-only gating + MCP instruction inventory

**Files:**
- Modify: `crates/bot/src/cc/invocation.rs` (`disallow_conversation_search`)
- Modify: `crates/right/src/aggregator.rs`, `crates/right/src/memory_server.rs` (`with_instructions()`)

- [ ] **Step 1: Add a stable tool-name constant (if needed) and gate it foreground-only**

`disallow_conversation_search` (`crates/bot/src/cc/invocation.rs`) currently lists `THREAD_SEARCH_MCP_TOOL` and `CHAT_SEARCH_MCP_TOOL`. Add the new tool. First add a constant in `crates/right-mcp/src/internal_client.rs` beside `THREAD_SEARCH_MCP_TOOL`:

```rust
pub const GET_MESSAGES_BY_ID_MCP_TOOL: &str = "mcp__right__get_messages_by_id";
```

Then extend `disallow_conversation_search` in `invocation.rs`:

```rust
pub(crate) fn disallow_conversation_search(mut tools: Vec<String>) -> Vec<String> {
    for tool_name in [
        right_mcp::internal_client::THREAD_SEARCH_MCP_TOOL,
        right_mcp::internal_client::CHAT_SEARCH_MCP_TOOL,
        right_mcp::internal_client::GET_MESSAGES_BY_ID_MCP_TOOL,
    ] {
        if !tools.iter().any(|tool| tool == tool_name) {
            tools.push(tool_name.to_owned());
        }
    }
    tools
}
```

- [ ] **Step 2: Update `with_instructions()` in both files**

Run: `rg -n 'thread_search' crates/right/src/aggregator.rs crates/right/src/memory_server.rs` to find the conversation-tool inventory lines in each `with_instructions()`. Beside the `thread_search`/`chat_search` line in EACH file, add a parallel line:

```
- mcp__right__get_messages_by_id: fetch full content of messages in the current chat/topic by id (scope server-enforced)
```

(Match each file's existing bullet/format exactly.)

- [ ] **Step 3: Build + relevant tests**

Run: `devenv shell -- cargo build -p right-bot -p right && devenv shell -- cargo test -p right with_instructions`
Expected: compiles; any `with_instructions` assertion tests pass (update them if they assert an exact tool inventory — add the new tool to the expected set).

- [ ] **Step 4: Commit**

```bash
git add crates/right-mcp/src/internal_client.rs crates/bot/src/cc/invocation.rs crates/right/src/aggregator.rs crates/right/src/memory_server.rs
git commit -m "feat: gate get_messages_by_id foreground-only + MCP instruction inventory"
```

---

## Task 5: Conditional reply-body strip in the worker

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (the `reply_to_body` transform in the batch loop)

- [ ] **Step 1: Locate the strip point**

Anchor: `let reply_to_body = msg.reply_to_body.clone().map(|mut body| {` in the batch-building loop (it currently sets `body.attachments` and `body.text` from resolved attachments / voice markers). The loop has `chat_id`, `eff_thread_id`, and `ctx` (for `ctx.agent_dir`) in scope.

- [ ] **Step 2: Add an archive-existence check and set `omitted`**

After the existing `reply_to_body` map closure (which produces `reply_to_body: Option<ReplyToBody>`), add a strip step that runs only when there is a reply id and a body. Insert immediately before `input_messages.push(build_input_message_from_debounce(...))`:

```rust
                // Strip the inlined reply body when it is recoverable from the
                // archive (the agent resolves it by id from context, or fetches
                // via mcp__right__get_messages_by_id). When NOT archived (the
                // reply payload is the only source — e.g. privacy-on group
                // replies to unseen messages), keep it inline (stripping would
                // lose it).
                let reply_to_body = match (msg.reply_to_id, reply_to_body) {
                    (Some(rid), Some(mut body)) if !body.omitted => {
                        let recoverable = match right_db::open_connection(&ctx.agent_dir, false).await {
                            Ok(conn) => right_db::conversation::fetch_by_ids(
                                &conn, "telegram", chat_id, eff_thread_id, &[rid],
                            )
                            .await
                            .map(|rows| !rows.is_empty())
                            .unwrap_or(false),
                            Err(e) => {
                                tracing::warn!(?chat_id, "reply strip: open_connection failed: {e:#}");
                                false
                            }
                        };
                        if recoverable {
                            body.text = None;
                            body.attachments = vec![];
                            body.omitted = true;
                        }
                        Some(body)
                    }
                    (_, other) => other,
                };
```

(`chat_id` / `eff_thread_id` are the loop's session scope; if their identifiers differ at this point in the function, use the same ones passed to `invoke_cc`.)

- [ ] **Step 3: Build**

Run: `devenv shell -- cargo build -p right-bot`
Expected: compiles. (On a DB-open failure we conservatively keep the body inline — fail-safe, never strip-then-lose.)

- [ ] **Step 4: Targeted worker tests**

Run: `devenv shell -- cargo test -p right-bot worker`
Expected: PASS (existing worker tests unaffected; this path only flips `omitted` when the id is archived).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(worker): strip recoverable reply bodies (archive-gated)"
```

---

## Task 6: Docs + final verification

**Files:**
- Modify: `PROMPT_SYSTEM.md`, `ARCHITECTURE.md`

- [ ] **Step 1: PROMPT_SYSTEM.md**

Document: replied-to bodies are stripped to `author` + a fetch note when the message is archived (recoverable); the agent resolves by `reply_to_id` from context or fetches via `mcp__right__get_messages_by_id`; non-archived replies stay inlined.

- [ ] **Step 2: ARCHITECTURE.md**

In the MCP Aggregator section (the conversation-search scope rules beside `thread_search`/`chat_search`), add: `mcp__right__get_messages_by_id` is scoped server-side to the current `(chat_id, effective_thread_id)`, agent supplies only `message_ids`, ids outside scope/not archived return absent. Keep within the 40k budget — one rule line; push any narration to a satellite if needed.

- [ ] **Step 3: Commit docs**

```bash
git add PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "docs: reply-body fetch-on-demand + get_messages_by_id scope rule"
```

- [ ] **Step 4: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. (Re-run any known-flaky cc/invocation pid-race or dashboard warn-count failure in isolation before attributing it to this change.)

- [ ] **Step 5: Clippy + build**

Run: `devenv shell -- cargo clippy --workspace -- -D warnings && devenv shell -- cargo build --workspace`
Expected: clean + success.

---

## Self-Review

**Spec coverage:**
- §1 `fetch_by_ids` → Task 1. `get_messages_by_id` tool (scope server-resolved, returns found, foreground-only) → Tasks 3 + 4. Conditional strip (archived→strip, else inline) → Task 5.
- §2 stripped form (author + note, drop text/attachments, keep reply_to_id + quoted_text) → Task 2 (`ReplyToBody.omitted` + emission). `quoted_text` is emitted by existing code outside the reply-body branch, so it is untouched (kept) — confirmed.
- §3 tool description → Task 3 Step 4; per-reply note → Task 2 Step 3; `with_instructions` both files → Task 4 Step 2; no OPERATING_INSTRUCTIONS change → respected.
- Security/scope (server-resolved, agent passes only ids) → Task 3 (param struct has only `message_ids`; scope from `conversation_scope`). Foreground-only → Task 4 Step 1.
- Upgrade/compat (additive, restart) → no migration task (correct; column set unchanged).
- Docs → Task 6. Final workspace test → Task 6 Step 4.

**Placeholder scan:** none. Edits in spec-1-modified files (`attachments.rs`, `worker.rs`) are anchored by code pattern with the exact code to insert; the "match the file's existing entry shape" notes (Task 3 Step 4, Task 4 Step 2) point at a named adjacent construct and give the full content to add.

**Type consistency:** `fetch_by_ids(&Connection, &str, i64, i64, &[i32]) -> Result<Vec<FetchedMessage>>` and `FetchedMessage { message_id: Option<i32>, sender_name: Option<String>, text: String, role: String }` are used identically in Tasks 1, 3, 5. `ReplyToBody.omitted: bool` set in Task 2 (constructors), flipped in Task 5, read in Task 2's emission. `GetMessagesByIdParams { message_ids: Vec<i32> }` consistent across Task 3. Handler uses `crate::progress::ToolCallContext` (matches `call_forum_topic_create`) and `CallToolResult::success(vec![Content::text(...)])` (matches the conversation-search handler).
