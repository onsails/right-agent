# Bot Forum-Topic Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a Right agent create, rename, close, and reopen Telegram forum topics (never delete) via five MCP tools, with a `data.db` registry that is strictly scoped to the current chat.

**Architecture:** MCP tools live on `RightBackend` (crate `right`). They mirror `send_progress`/`thread_search` exactly: the agent supplies only operation args (never `chat_id`); the server resolves the current chat from the per-invocation registry (`X-Right-Invocation` header → `ProgressRegistry`). Write ops (create/edit/close/reopen) POST to a new bot-local UDS endpoint that performs the teloxide call; the registry write and the `forum_topic_list` read happen in `RightBackend` against the per-agent `data.db`. Telegram itself enforces "no delete" via the `can_manage_topics`/`can_delete_messages` rights split — we additionally never expose a delete tool.

**Tech Stack:** Rust (edition 2024), teloxide 0.17 (teloxide-core 0.13.0), axum UDS servers, Turso/`right-db` (SQLite-compatible), rmcp.

---

## Spec

`docs/superpowers/specs/2026-06-01-bot-forum-topic-management-design.md`

## File map

| File | Change |
|---|---|
| `crates/right-db/src/sql/v40_forum_topics.sql` | **Create** — `forum_topics` table DDL |
| `crates/right-db/src/migrations.rs` | Modify — bump `LATEST_SCHEMA_VERSION` to 40, add `V40_SCHEMA` const + `Migration` entry |
| `crates/right-db/src/forum_topics.rs` | **Create** — CRUD helpers (`upsert_created`, `update_edited`, `set_state`, `list`) + `ForumTopicRow` |
| `crates/right-db/src/lib.rs` | Modify — `pub mod forum_topics;` |
| `crates/right-mcp/src/internal_client.rs` | Modify — 4 request structs, 1 shared ok-response, `ForumTopicCreateResponse`, 4 `InternalClient` methods |
| `crates/right/src/progress.rs` | Modify — `ForumTarget` struct + `ProgressRegistry::forum_target()` |
| `crates/bot/src/telegram/progress.rs` | Modify — 4 UDS handlers + routes in `build_progress_router` + error-mapping helper |
| `crates/right/src/right_backend.rs` | Modify — 5 param structs, 5 `tools_list` entries, 5 dispatch arms, 5 `call_forum_topic_*` methods |
| `crates/right/src/aggregator.rs` | Modify — add Forum Topics block to `with_instructions()` |
| `crates/right/src/memory_server.rs` | Modify — add Forum Topics block to `with_instructions()` |
| `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` | Modify — capability note |
| `crates/right-codegen/templates/right/agent/BOOTSTRAP.md` | Modify — group/admin nudge |
| `ARCHITECTURE.md` | Modify — one-line scope invariant in MCP Aggregator section |
| `PROMPT_SYSTEM.md` | Modify — document the new tools |

## Verification cadence

Targeted package tests after each task (`devenv shell -- cargo test -p <crate> <filter>`). **One mandatory full `devenv shell -- cargo test --workspace` at the end** (Task 11). Do not run the full workspace suite after every task.

## Conventions reminder

- All commands run under `devenv shell -- …`.
- Edition 2024, FAIL FAST: every error propagates with `?`; `anyhow`→string uses `format!("{e:#}")`.
- Load the `rust-dev:rust-dev` skill before writing Rust.

---

### Task 1: `forum_topics` table migration

**Files:**
- Create: `crates/right-db/src/sql/v40_forum_topics.sql`
- Modify: `crates/right-db/src/migrations.rs` (const `LATEST_SCHEMA_VERSION` near line 36; `MIGRATIONS` array tail near line 933)

- [ ] **Step 1: Write the SQL file**

Create `crates/right-db/src/sql/v40_forum_topics.sql`:

```sql
CREATE TABLE IF NOT EXISTS forum_topics (
  chat_id              INTEGER NOT NULL,
  message_thread_id    INTEGER NOT NULL,
  name                 TEXT,
  icon_color           INTEGER,
  icon_custom_emoji_id TEXT,
  state                TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'closed')),
  updated_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  PRIMARY KEY (chat_id, message_thread_id)
);

CREATE INDEX IF NOT EXISTS idx_forum_topics_chat
  ON forum_topics (chat_id, updated_at);
```

The composite PRIMARY KEY is the single unique constraint that bare `ON CONFLICT` upserts will target (Turso rejects the `ON CONFLICT (col) WHERE…` form).

- [ ] **Step 2: Register the migration**

In `crates/right-db/src/migrations.rs`, find where the other `V##_SCHEMA` consts are defined (search `const V39_SCHEMA`) and add next to it:

```rust
const V40_SCHEMA: &str = include_str!("sql/v40_forum_topics.sql");
```

Find `LATEST_SCHEMA_VERSION` (around line 36) and bump it:

```rust
pub const LATEST_SCHEMA_VERSION: u32 = 40;
```

Append to the `MIGRATIONS` array, immediately after the `version: 39` entry (around line 933):

```rust
        Migration {
            version: 40,
            sql: V40_SCHEMA,
            hook: None,
        },
```

- [ ] **Step 3: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/right-db/src/migrations.rs` (follow the existing in-memory migration tests around lines 958–1001):

```rust
    #[tokio::test]
    async fn migration_v40_creates_forum_topics_table() {
        let conn = Connection::open_in_memory().await.unwrap();
        run_migrations(&conn).await.unwrap();
        let count = conn
            .query_one(
                "SELECT COUNT(*) FROM pragma_table_info('forum_topics') \
                 WHERE name IN ('chat_id','message_thread_id','name','icon_color','icon_custom_emoji_id','state','updated_at')",
                (),
                |r| r.get::<i64>(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 7, "forum_topics must have all 7 columns");
    }
```

If `run_migrations` is not the exact runner name in that test module, copy the call used by the neighbouring `migration_*` tests verbatim.

- [ ] **Step 4: Run test to verify it fails, then passes**

Run: `devenv shell -- cargo test -p right-db migration_v40_creates_forum_topics_table`
Expected: fails before Steps 1–2 are saved; passes after.

- [ ] **Step 5: Commit**

```bash
git add crates/right-db/src/sql/v40_forum_topics.sql crates/right-db/src/migrations.rs
git commit -m "feat(db): add forum_topics table (migration v40)"
```

---

### Task 2: `forum_topics` query helpers

**Files:**
- Create: `crates/right-db/src/forum_topics.rs`
- Modify: `crates/right-db/src/lib.rs` (module list around lines 10–27)

- [ ] **Step 1: Declare the module**

In `crates/right-db/src/lib.rs`, add to the `pub mod` block (alphabetical, after `pub mod error;`):

```rust
pub mod forum_topics;
```

- [ ] **Step 2: Write the helper module**

Create `crates/right-db/src/forum_topics.rs`:

```rust
//! Per-agent registry of Telegram forum topics the agent has created or
//! managed. Authoritative source: results of the agent's own
//! create/edit/close/reopen tool calls. Rows are scoped by `chat_id`; the
//! MCP layer must always pass the server-resolved current chat id and never
//! an agent-supplied value.

use crate::{Connection, Row};

type Result<T> = std::result::Result<T, crate::DbError>;

/// One registry row, returned by [`list`].
#[derive(Debug, Clone, PartialEq)]
pub struct ForumTopicRow {
    pub message_thread_id: i64,
    pub name: Option<String>,
    pub icon_color: Option<i64>,
    pub icon_custom_emoji_id: Option<String>,
    pub state: String,
    pub updated_at: String,
}

fn row_to_topic(r: &Row<'_>) -> Result<ForumTopicRow> {
    Ok(ForumTopicRow {
        message_thread_id: r.get(0)?,
        name: r.get(1)?,
        icon_color: r.get(2)?,
        icon_custom_emoji_id: r.get(3)?,
        state: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

/// Upsert a topic the agent just created. Resets state to 'open' and
/// refreshes metadata. Single-statement write — no transaction needed.
pub async fn upsert_created(
    conn: &Connection,
    chat_id: i64,
    message_thread_id: i64,
    name: &str,
    icon_color: Option<i64>,
    icon_custom_emoji_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO forum_topics
            (chat_id, message_thread_id, name, icon_color, icon_custom_emoji_id, state, updated_at)
         VALUES (?, ?, ?, ?, ?, 'open', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         ON CONFLICT DO UPDATE SET
            name = excluded.name,
            icon_color = excluded.icon_color,
            icon_custom_emoji_id = excluded.icon_custom_emoji_id,
            state = 'open',
            updated_at = excluded.updated_at",
        crate::params![chat_id, message_thread_id, name, icon_color, icon_custom_emoji_id],
    )
    .await?;
    Ok(())
}

/// Update name/icon for an existing tracked topic. No-op (0 rows) if the
/// topic is not in the registry (e.g. a human-created topic). `None` fields
/// are left unchanged via COALESCE.
pub async fn update_edited(
    conn: &Connection,
    chat_id: i64,
    message_thread_id: i64,
    name: Option<&str>,
    icon_custom_emoji_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE forum_topics SET
            name = COALESCE(?, name),
            icon_custom_emoji_id = COALESCE(?, icon_custom_emoji_id),
            updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
         WHERE chat_id = ? AND message_thread_id = ?",
        crate::params![name, icon_custom_emoji_id, chat_id, message_thread_id],
    )
    .await?;
    Ok(())
}

/// Set open/closed state. No-op if the topic is not tracked.
pub async fn set_state(
    conn: &Connection,
    chat_id: i64,
    message_thread_id: i64,
    state: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE forum_topics SET
            state = ?,
            updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
         WHERE chat_id = ? AND message_thread_id = ?",
        crate::params![state, chat_id, message_thread_id],
    )
    .await?;
    Ok(())
}

/// List all tracked topics for ONE chat, newest-updated first. The caller
/// MUST pass the server-resolved current chat id.
pub async fn list(conn: &Connection, chat_id: i64) -> Result<Vec<ForumTopicRow>> {
    conn.query_all(
        "SELECT message_thread_id, name, icon_color, icon_custom_emoji_id, state, updated_at
         FROM forum_topics
         WHERE chat_id = ?
         ORDER BY updated_at DESC, message_thread_id DESC",
        crate::params![chat_id],
        row_to_topic,
    )
    .await
}

#[cfg(test)]
#[path = "forum_topics_tests.rs"]
mod tests;
```

If `Connection::execute` returns `Result<usize, DbError>` (it does), discarding the count with `?` and `Ok(())` is correct. If `Row::get`'s turbofish form differs from neighbouring code in `conversation.rs`, match that file's style.

- [ ] **Step 3: Write the failing tests**

Create `crates/right-db/src/forum_topics_tests.rs`:

```rust
use super::*;
use tempfile::TempDir;

struct TestDb {
    _dir: TempDir,
    conn: Connection,
}

impl std::ops::Deref for TestDb {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

async fn migrated() -> TestDb {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::open_connection(dir.path(), true).await.unwrap();
    TestDb { _dir: dir, conn }
}

#[tokio::test]
async fn upsert_then_list_roundtrips() {
    let db = migrated().await;
    upsert_created(&db, 100, 5, "Bugs", Some(7322096), None)
        .await
        .unwrap();
    let rows = list(&db, 100).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_thread_id, 5);
    assert_eq!(rows[0].name.as_deref(), Some("Bugs"));
    assert_eq!(rows[0].icon_color, Some(7322096));
    assert_eq!(rows[0].state, "open");
}

#[tokio::test]
async fn list_is_strictly_scoped_to_one_chat() {
    let db = migrated().await;
    upsert_created(&db, 100, 5, "ChatA topic", None, None)
        .await
        .unwrap();
    upsert_created(&db, 200, 9, "ChatB topic", None, None)
        .await
        .unwrap();
    let a = list(&db, 100).await.unwrap();
    let b = list(&db, 200).await.unwrap();
    assert_eq!(a.len(), 1, "chat 100 must see only its own topic");
    assert_eq!(a[0].name.as_deref(), Some("ChatA topic"));
    assert_eq!(b.len(), 1, "chat 200 must see only its own topic");
    assert_eq!(b[0].name.as_deref(), Some("ChatB topic"));
}

#[tokio::test]
async fn set_state_closes_and_reopens() {
    let db = migrated().await;
    upsert_created(&db, 100, 5, "Bugs", None, None)
        .await
        .unwrap();
    set_state(&db, 100, 5, "closed").await.unwrap();
    assert_eq!(list(&db, 100).await.unwrap()[0].state, "closed");
    set_state(&db, 100, 5, "open").await.unwrap();
    assert_eq!(list(&db, 100).await.unwrap()[0].state, "open");
}

#[tokio::test]
async fn update_edited_changes_name_only() {
    let db = migrated().await;
    upsert_created(&db, 100, 5, "Old", Some(7322096), None)
        .await
        .unwrap();
    update_edited(&db, 100, 5, Some("New"), None).await.unwrap();
    let rows = list(&db, 100).await.unwrap();
    assert_eq!(rows[0].name.as_deref(), Some("New"));
    assert_eq!(rows[0].icon_color, Some(7322096), "icon untouched");
}

#[tokio::test]
async fn update_edited_is_noop_for_untracked_topic() {
    let db = migrated().await;
    update_edited(&db, 100, 999, Some("ghost"), None)
        .await
        .unwrap();
    assert!(list(&db, 100).await.unwrap().is_empty());
}
```

- [ ] **Step 4: Run tests**

Run: `devenv shell -- cargo test -p right-db forum_topics`
Expected: all five pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right-db/src/forum_topics.rs crates/right-db/src/forum_topics_tests.rs crates/right-db/src/lib.rs
git commit -m "feat(db): forum_topics CRUD helpers with strict chat scoping"
```

---

### Task 3: Internal request/response types + `InternalClient` methods

**Files:**
- Modify: `crates/right-mcp/src/internal_client.rs` (add near the `ProgressSendRequest` definitions around lines 556–577; add methods near `progress_send` around line 240)

- [ ] **Step 1: Add the wire types**

In `crates/right-mcp/src/internal_client.rs`, after `ProgressSendResponse` (around line 577), add:

```rust
// ---------------------------------------------------------------------------
// Forum topic management (bot-local UDS endpoints)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct ForumTopicCreateRequest {
    pub invocation_id: String,
    pub token: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_custom_emoji_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ForumTopicEditRequest {
    pub invocation_id: String,
    pub token: String,
    pub message_thread_id: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_custom_emoji_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ForumTopicThreadRequest {
    pub invocation_id: String,
    pub token: String,
    pub message_thread_id: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ForumTopicCreateResponse {
    pub ok: bool,
    pub message_thread_id: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ForumTopicOkResponse {
    pub ok: bool,
}
```

`ForumTopicThreadRequest` is shared by close and reopen (same shape).

- [ ] **Step 2: Add the `Debug` redaction guard**

These structs carry a `token`. The progress structs use a manual `Debug` impl that redacts the token (search `impl std::fmt::Debug for ProgressSendRequest`). The three new request structs derive only `Clone, Serialize, Deserialize` (NOT `Debug`) above, so they cannot be logged with `{:?}` — this is the redaction guarantee. Do not add `#[derive(Debug)]` to them.

- [ ] **Step 3: Add the client methods**

After `progress_send` (around line 244), add:

```rust
    /// Create a forum topic via the bot-local UDS endpoint.
    pub async fn forum_topic_create(
        &self,
        request: &ForumTopicCreateRequest,
    ) -> Result<ForumTopicCreateResponse, InternalClientError> {
        self.post("/forum-topic/create", request).await
    }

    /// Edit (rename / change icon) a forum topic.
    pub async fn forum_topic_edit(
        &self,
        request: &ForumTopicEditRequest,
    ) -> Result<ForumTopicOkResponse, InternalClientError> {
        self.post("/forum-topic/edit", request).await
    }

    /// Close a forum topic.
    pub async fn forum_topic_close(
        &self,
        request: &ForumTopicThreadRequest,
    ) -> Result<ForumTopicOkResponse, InternalClientError> {
        self.post("/forum-topic/close", request).await
    }

    /// Reopen a forum topic.
    pub async fn forum_topic_reopen(
        &self,
        request: &ForumTopicThreadRequest,
    ) -> Result<ForumTopicOkResponse, InternalClientError> {
        self.post("/forum-topic/reopen", request).await
    }
```

- [ ] **Step 4: Write the failing test**

In the `#[cfg(test)] mod tests` block of `internal_client.rs` (find `progress_send_request_serializes_expected_fields`), add:

```rust
    #[test]
    fn forum_topic_create_request_serializes_expected_fields() {
        let req = ForumTopicCreateRequest {
            invocation_id: "inv-1".to_owned(),
            token: "secret".to_owned(),
            name: "Bugs".to_owned(),
            icon_color: Some(7322096),
            icon_custom_emoji_id: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["invocation_id"], "inv-1");
        assert_eq!(v["name"], "Bugs");
        assert_eq!(v["icon_color"], 7322096);
        assert!(v.get("icon_custom_emoji_id").is_none(), "None is skipped");
    }
```

- [ ] **Step 5: Run test, then build the crate**

Run: `devenv shell -- cargo test -p right-mcp forum_topic_create_request_serializes_expected_fields`
Then: `devenv shell -- cargo build -p right-mcp`
Expected: test passes, crate builds.

- [ ] **Step 6: Commit**

```bash
git add crates/right-mcp/src/internal_client.rs
git commit -m "feat(mcp): internal-client wire types + methods for forum topics"
```

---

### Task 4: `ProgressRegistry::forum_target` accessor

**Files:**
- Modify: `crates/right/src/progress.rs` (near `conversation_scope` around lines 229–241, and `ConversationScope` around lines 52–55)

- [ ] **Step 1: Add the `ForumTarget` struct**

In `crates/right/src/progress.rs`, near `ConversationScope` (around line 55), add:

```rust
/// Everything the aggregator needs to perform a forum-topic operation for an
/// invocation: where to reach the bot, the shared send token, and the
/// server-resolved chat id (never agent-supplied).
#[derive(Debug, Clone)]
pub(crate) struct ForumTarget {
    pub(crate) bot_socket_path: std::path::PathBuf,
    pub(crate) bot_send_token: String,
    pub(crate) chat_id: i64,
}
```

- [ ] **Step 2: Add the accessor**

After `conversation_scope` (around line 241), add:

```rust
    /// Resolve the bot endpoint + token + chat id for a forum-topic
    /// operation. Foreground-only (like progress and conversation search):
    /// cron/delivery/reflection/background turns must not manage topics.
    pub(crate) async fn forum_target(
        &self,
        invocation_id: &str,
    ) -> Result<ForumTarget, ProgressError> {
        let inner = self.inner.lock().await;
        let invocation = inner.get(invocation_id).ok_or(ProgressError::Unavailable)?;
        if !matches!(invocation.kind, ProgressInvocationKind::Foreground) {
            return Err(ProgressError::Forbidden);
        }
        let scope = invocation
            .conversation_scope
            .ok_or(ProgressError::Unavailable)?;
        Ok(ForumTarget {
            bot_socket_path: invocation.bot_socket_path.clone(),
            bot_send_token: invocation.bot_send_token.clone(),
            chat_id: scope.chat_id,
        })
    }
```

Field names (`bot_socket_path`, `bot_send_token`, `conversation_scope`, `kind`) are taken verbatim from the `ProgressInvocation` struct (progress.rs lines 106–112). If `ConversationScope` is not `Copy`, change `invocation.conversation_scope` to `.as_ref()` and clone `chat_id` (it's `i64`, so `.map(|s| s.chat_id)` works either way).

- [ ] **Step 3: Write the failing test**

Find the existing progress-registry tests in `progress.rs` (search `mod tests` / `register`). Mirror how they construct a registration. Add:

```rust
    #[tokio::test]
    async fn forum_target_returns_scope_for_foreground() {
        let reg = ProgressRegistry::default();
        // Use the SAME registration constructor the neighbouring tests use.
        reg.register(ProgressRegistration {
            invocation_id: "inv-1".to_owned(),
            kind: ProgressInvocationKind::Foreground,
            bot_socket_path: "/tmp/bot.sock".into(),
            bot_send_token: "tok".to_owned(),
            conversation_scope: Some(ConversationScope { chat_id: 42, thread_id: 7 }),
        })
        .await;
        let target = reg.forum_target("inv-1").await.unwrap();
        assert_eq!(target.chat_id, 42);
        assert_eq!(target.bot_send_token, "tok");
    }

    #[tokio::test]
    async fn forum_target_forbidden_for_non_foreground() {
        let reg = ProgressRegistry::default();
        reg.register(ProgressRegistration {
            invocation_id: "inv-2".to_owned(),
            kind: ProgressInvocationKind::Cron,
            bot_socket_path: "/tmp/bot.sock".into(),
            bot_send_token: "tok".to_owned(),
            conversation_scope: Some(ConversationScope { chat_id: 42, thread_id: 7 }),
        })
        .await;
        assert!(matches!(
            reg.forum_target("inv-2").await,
            Err(ProgressError::Forbidden)
        ));
    }
```

Adjust `ProgressRegistration { … }` field names and the `register(...)` signature to match the real constructor used by the existing tests — read them first and copy exactly. If the enum variant for cron is not `ProgressInvocationKind::Cron`, use whatever non-`Foreground` variant exists.

- [ ] **Step 4: Run tests**

Run: `devenv shell -- cargo test -p right forum_target`
Expected: both pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/progress.rs
git commit -m "feat(mcp): ProgressRegistry::forum_target (foreground-only chat resolution)"
```

---

### Task 5: Bot-local UDS endpoints for topic operations

**Files:**
- Modify: `crates/bot/src/telegram/progress.rs` (router `build_progress_router`; handlers near `handle_progress_send` lines 88–171; imports near top lines 14–24)

- [ ] **Step 1: Confirm the router and imports**

Read `build_progress_router` in `crates/bot/src/telegram/progress.rs` and confirm it builds an axum `Router` over `ProgressEndpointState { bot: teloxide::Bot, progress: ProgressState }`. Confirm the imports at the top include `ChatId, MessageId, ThreadId`, `Requester`, `SendMessageSetters`. Add to the imports:

```rust
use right_mcp::internal_client::{
    ForumTopicCreateRequest, ForumTopicCreateResponse, ForumTopicEditRequest,
    ForumTopicOkResponse, ForumTopicThreadRequest,
};
use teloxide::types::{CustomEmojiId, Rgb};
```

(`ProgressErrorResponse` is already defined in this file — reuse it for errors.)

- [ ] **Step 2: Add the error-mapping helper (pure fn + test-first)**

Add this pure function and its test to `progress.rs`:

```rust
/// Map a teloxide forum error description to a clear, actionable sentence for
/// the agent to relay. Falls back to the raw description.
fn forum_error_message(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("not enough rights") || lower.contains("manage topics") {
        "I don't have the \"Manage Topics\" admin right in this group.".to_owned()
    } else if lower.contains("not a forum") || lower.contains("topic_closed") && raw.is_empty() {
        "Forum topics exist only in forum supergroups (enable Topics in group settings).".to_owned()
    } else if lower.contains("not a forum") {
        "Forum topics exist only in forum supergroups (enable Topics in group settings).".to_owned()
    } else {
        raw.to_owned()
    }
}
```

Test (add to the `#[cfg(test)] mod tests` of `progress.rs`, or create one if absent):

```rust
    #[test]
    fn forum_error_message_maps_known_cases() {
        assert!(super::forum_error_message("Bad Request: not enough rights to manage forum topics")
            .contains("Manage Topics"));
        assert!(super::forum_error_message("Bad Request: the chat is not a forum")
            .contains("forum supergroups"));
        assert_eq!(super::forum_error_message("weird error"), "weird error");
    }
```

(Simplify the `else if` chain if clippy flags the redundant branch — the intent is: rights → rights message; not-a-forum → forum message; else passthrough.)

- [ ] **Step 3: Add the four handlers**

Add to `progress.rs`. Each mirrors `handle_progress_send`'s lookup + token check, then performs the teloxide call:

```rust
async fn handle_forum_topic_create(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<ForumTopicCreateRequest>,
) -> axum::response::Response {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return forum_not_found();
    };
    if !target.token_matches(&req.token) {
        return forum_forbidden();
    }
    let mut call = state.bot.create_forum_topic(ChatId(target.chat_id), req.name.clone());
    if let Some(color) = req.icon_color {
        call = call.icon_color(Rgb::from_u32(color as u32));
    }
    if let Some(emoji) = req.icon_custom_emoji_id.clone() {
        call = call.icon_custom_emoji_id(CustomEmojiId(emoji));
    }
    match call.await {
        Ok(topic) => (
            StatusCode::OK,
            Json(ForumTopicCreateResponse { ok: true, message_thread_id: topic.thread_id.0.0 }),
        )
            .into_response(),
        Err(e) => forum_telegram_error(&req.invocation_id, e),
    }
}

async fn handle_forum_topic_edit(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<ForumTopicEditRequest>,
) -> axum::response::Response {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return forum_not_found();
    };
    if !target.token_matches(&req.token) {
        return forum_forbidden();
    }
    let thread = ThreadId(MessageId(req.message_thread_id));
    let mut call = state.bot.edit_forum_topic(ChatId(target.chat_id), thread);
    if let Some(name) = req.name.clone() {
        call = call.name(name);
    }
    if let Some(emoji) = req.icon_custom_emoji_id.clone() {
        call = call.icon_custom_emoji_id(CustomEmojiId(emoji));
    }
    match call.await {
        Ok(_) => forum_ok(),
        Err(e) => forum_telegram_error(&req.invocation_id, e),
    }
}

async fn handle_forum_topic_close(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<ForumTopicThreadRequest>,
) -> axum::response::Response {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return forum_not_found();
    };
    if !target.token_matches(&req.token) {
        return forum_forbidden();
    }
    let thread = ThreadId(MessageId(req.message_thread_id));
    match state.bot.close_forum_topic(ChatId(target.chat_id), thread).await {
        Ok(_) => forum_ok(),
        Err(e) => forum_telegram_error(&req.invocation_id, e),
    }
}

async fn handle_forum_topic_reopen(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<ForumTopicThreadRequest>,
) -> axum::response::Response {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return forum_not_found();
    };
    if !target.token_matches(&req.token) {
        return forum_forbidden();
    }
    let thread = ThreadId(MessageId(req.message_thread_id));
    match state.bot.reopen_forum_topic(ChatId(target.chat_id), thread).await {
        Ok(_) => forum_ok(),
        Err(e) => forum_telegram_error(&req.invocation_id, e),
    }
}

fn forum_ok() -> axum::response::Response {
    (StatusCode::OK, Json(ForumTopicOkResponse { ok: true })).into_response()
}

fn forum_not_found() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        Json(ProgressErrorResponse { error: "forum invocation not found".to_owned() }),
    )
        .into_response()
}

fn forum_forbidden() -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(ProgressErrorResponse { error: "forum token mismatch".to_owned() }),
    )
        .into_response()
}

fn forum_telegram_error(
    invocation_id: &str,
    e: teloxide::RequestError,
) -> axum::response::Response {
    let raw = format!("{e:#}");
    tracing::warn!(invocation_id = %invocation_id, "forum topic op failed: {raw}");
    (
        StatusCode::BAD_GATEWAY,
        Json(ProgressErrorResponse { error: forum_error_message(&raw) }),
    )
        .into_response()
}
```

Notes:
- `topic.thread_id.0.0` — `ForumTopic.thread_id: ThreadId(MessageId(i32))`; `.0.0` yields the `i32`. Verify against `teloxide::types::ForumTopic` if it fails to compile (the serde field is `message_thread_id` but the Rust field is `thread_id`).
- `Rgb::from_u32` — if that constructor name is wrong, construct from bytes: `Rgb { r: (color >> 16) as u8, g: (color >> 8) as u8, b: color as u8 }` (the struct has public `r`/`g`/`b`). The `cargo build` step will tell you.
- The builder setter names (`.icon_color`, `.icon_custom_emoji_id`, `.name`) come from teloxide's `impl_payload!` macro; confirm with `cargo doc` if a setter is missing.

- [ ] **Step 4: Register the routes**

In `build_progress_router`, add the four routes alongside `/progress/send` (match the existing `.route(...)` style):

```rust
        .route("/forum-topic/create", post(handle_forum_topic_create))
        .route("/forum-topic/edit", post(handle_forum_topic_edit))
        .route("/forum-topic/close", post(handle_forum_topic_close))
        .route("/forum-topic/reopen", post(handle_forum_topic_reopen))
```

Ensure `axum::routing::post` is imported (it already is, for `/progress/send`).

- [ ] **Step 5: Run the error-mapping test + build the bot**

Run: `devenv shell -- cargo test -p right-bot forum_error_message_maps_known_cases`
Then: `devenv shell -- cargo build -p right-bot`
Expected: test passes; bot builds (fix `Rgb`/`thread_id` accessor here if the compiler objects).

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/progress.rs
git commit -m "feat(bot): UDS endpoints for forum topic create/edit/close/reopen"
```

---

### Task 6: MCP tool surface on `RightBackend`

**Files:**
- Modify: `crates/right/src/right_backend.rs` (param structs near line 41; `tools_list` lines 74–156; dispatch lines 163–210; new `call_forum_topic_*` methods near the conversation-search section line 855+)

- [ ] **Step 1: Add the param structs**

In `crates/right/src/right_backend.rs`, after `ConversationSearchParams` (around line 45), add:

```rust
/// Allowed forum-topic icon colors (RGB ints), per Telegram Bot API.
const ALLOWED_ICON_COLORS: [i32; 6] =
    [7322096, 16766590, 13338331, 9367192, 16749490, 16478047];

/// End-to-end timeout for a forum-topic bot round-trip.
const FORUM_TOPIC_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForumTopicCreateParams {
    /// Topic name, 1–128 characters.
    pub(crate) name: String,
    /// Optional icon color (one of the 6 Telegram-allowed RGB integers).
    pub(crate) icon_color: Option<i32>,
    /// Optional custom-emoji icon id (from getForumTopicIconStickers).
    pub(crate) icon_custom_emoji_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForumTopicEditParams {
    /// Target topic's message_thread_id.
    pub(crate) message_thread_id: i32,
    /// New name (1–128 chars). Omit to keep current.
    pub(crate) name: Option<String>,
    /// New custom-emoji icon id; empty string removes the icon. Omit to keep.
    pub(crate) icon_custom_emoji_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForumTopicThreadParams {
    /// Target topic's message_thread_id.
    pub(crate) message_thread_id: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForumTopicListParams {}
```

- [ ] **Step 2: Add `tools_list` entries**

In `tools_list` (inside the `vec![…]`, after the `chat_search` entry around line 145), add:

```rust
            // Forum topic management (forum supergroups only; never deletes)
            Tool::new(
                "forum_topic_create",
                "Create a forum topic in the current Telegram forum supergroup. Returns the new message_thread_id. Forum supergroups only; the bot needs the 'Manage Topics' admin right. icon_color must be one of the 6 Telegram-allowed RGB integers if set.",
                schema_for_type::<ForumTopicCreateParams>(),
            ),
            Tool::new(
                "forum_topic_edit",
                "Rename a forum topic and/or change its custom-emoji icon, by message_thread_id, in the current chat. Empty icon_custom_emoji_id removes the icon.",
                schema_for_type::<ForumTopicEditParams>(),
            ),
            Tool::new(
                "forum_topic_close",
                "Close (archive) a forum topic by message_thread_id in the current chat. Reversible with forum_topic_reopen; does not delete the topic or its messages.",
                schema_for_type::<ForumTopicThreadParams>(),
            ),
            Tool::new(
                "forum_topic_reopen",
                "Reopen a previously closed forum topic by message_thread_id in the current chat.",
                schema_for_type::<ForumTopicThreadParams>(),
            ),
            Tool::new(
                "forum_topic_list",
                "List forum topics this agent has created or managed in the CURRENT chat only. Scope is server-enforced and not agent-controlled. There is no Telegram API to enumerate all topics, so this returns only tracked topics.",
                schema_for_type::<ForumTopicListParams>(),
            ),
```

- [ ] **Step 3: Add dispatch arms**

In `tools_call`'s match (after the `chat_search` arm around line 207), add:

```rust
            "forum_topic_create" => self.call_forum_topic_create(agent_name, context, &args).await,
            "forum_topic_edit" => self.call_forum_topic_edit(agent_name, context, &args).await,
            "forum_topic_close" => self.call_forum_topic_close(agent_name, context, &args).await,
            "forum_topic_reopen" => self.call_forum_topic_reopen(agent_name, context, &args).await,
            "forum_topic_list" => self.call_forum_topic_list(agent_name, context, &args).await,
```

- [ ] **Step 4: Add the implementations**

Add a new section after `call_conversation_search` (after the conversation-search section that ends around line 935). Imports needed at top of file: add `ForumTopicCreateRequest, ForumTopicEditRequest, ForumTopicOkResponse, ForumTopicThreadRequest, ForumTopicCreateResponse` to the existing `use right_mcp::internal_client::{…}` line (line 14–16).

```rust
    // ------------------------------------------------------------------
    // Forum topic management
    // ------------------------------------------------------------------

    async fn call_forum_topic_create(
        &self,
        agent_name: &str,
        context: crate::progress::ToolCallContext,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: ForumTopicCreateParams = match serde_json::from_value(args.clone()) {
            Ok(p) => p,
            Err(e) => {
                return Ok(tool_error("invalid_argument", format!("invalid forum_topic_create params: {e:#}"), None));
            }
        };
        let name = params.name.trim();
        if name.is_empty() || name.chars().count() > 128 {
            return Ok(tool_error("invalid_argument", "topic name must be 1–128 characters", None));
        }
        if let Some(color) = params.icon_color {
            if !ALLOWED_ICON_COLORS.contains(&color) {
                return Ok(tool_error(
                    "invalid_argument",
                    format!("icon_color must be one of {ALLOWED_ICON_COLORS:?}"),
                    None,
                ));
            }
        }
        let Some(invocation_id) = context.invocation_id else {
            return Ok(forum_scope_unavailable());
        };
        let target = match self.progress.forum_target(&invocation_id).await {
            Ok(t) => t,
            Err(_) => return Ok(forum_scope_unavailable()),
        };
        let client = InternalClient::new(target.bot_socket_path);
        let request = ForumTopicCreateRequest {
            invocation_id,
            token: target.bot_send_token,
            name: name.to_owned(),
            icon_color: params.icon_color,
            icon_custom_emoji_id: params.icon_custom_emoji_id.clone(),
        };
        let resp: ForumTopicCreateResponse = match tokio::time::timeout(
            FORUM_TOPIC_TIMEOUT,
            client.forum_topic_create(&request),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Ok(forum_op_error(e)),
            Err(_) => return Ok(tool_error("forum_op_failed", "forum create timed out", None)),
        };
        // Persist to the registry (server-resolved chat id).
        let conn_arc = self.get_conn(agent_name).await?;
        let conn = conn_arc.lock().await;
        right_db::forum_topics::upsert_created(
            &conn,
            target.chat_id,
            i64::from(resp.message_thread_id),
            name,
            params.icon_color.map(i64::from),
            params.icon_custom_emoji_id.as_deref(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("forum registry write failed: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({ "message_thread_id": resp.message_thread_id, "name": name }).to_string(),
        )]))
    }

    async fn call_forum_topic_edit(
        &self,
        agent_name: &str,
        context: crate::progress::ToolCallContext,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: ForumTopicEditParams = match serde_json::from_value(args.clone()) {
            Ok(p) => p,
            Err(e) => return Ok(tool_error("invalid_argument", format!("invalid forum_topic_edit params: {e:#}"), None)),
        };
        if let Some(name) = params.name.as_deref() {
            if name.chars().count() > 128 {
                return Ok(tool_error("invalid_argument", "topic name must be at most 128 characters", None));
            }
        }
        let Some(invocation_id) = context.invocation_id else {
            return Ok(forum_scope_unavailable());
        };
        let target = match self.progress.forum_target(&invocation_id).await {
            Ok(t) => t,
            Err(_) => return Ok(forum_scope_unavailable()),
        };
        let client = InternalClient::new(target.bot_socket_path);
        let request = ForumTopicEditRequest {
            invocation_id,
            token: target.bot_send_token,
            message_thread_id: params.message_thread_id,
            name: params.name.clone(),
            icon_custom_emoji_id: params.icon_custom_emoji_id.clone(),
        };
        match tokio::time::timeout(FORUM_TOPIC_TIMEOUT, client.forum_topic_edit(&request)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Ok(forum_op_error(e)),
            Err(_) => return Ok(tool_error("forum_op_failed", "forum edit timed out", None)),
        }
        let conn_arc = self.get_conn(agent_name).await?;
        let conn = conn_arc.lock().await;
        right_db::forum_topics::update_edited(
            &conn,
            target.chat_id,
            i64::from(params.message_thread_id),
            params.name.as_deref(),
            params.icon_custom_emoji_id.as_deref(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("forum registry write failed: {e}"))?;
        Ok(forum_success())
    }

    async fn call_forum_topic_close(
        &self,
        agent_name: &str,
        context: crate::progress::ToolCallContext,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        self.forum_set_state(agent_name, context, args, "closed").await
    }

    async fn call_forum_topic_reopen(
        &self,
        agent_name: &str,
        context: crate::progress::ToolCallContext,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        self.forum_set_state(agent_name, context, args, "open").await
    }

    /// Shared close/reopen path. `new_state` is "closed" or "open".
    async fn forum_set_state(
        &self,
        agent_name: &str,
        context: crate::progress::ToolCallContext,
        args: &serde_json::Value,
        new_state: &str,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: ForumTopicThreadParams = match serde_json::from_value(args.clone()) {
            Ok(p) => p,
            Err(e) => return Ok(tool_error("invalid_argument", format!("invalid params: {e:#}"), None)),
        };
        let Some(invocation_id) = context.invocation_id else {
            return Ok(forum_scope_unavailable());
        };
        let target = match self.progress.forum_target(&invocation_id).await {
            Ok(t) => t,
            Err(_) => return Ok(forum_scope_unavailable()),
        };
        let client = InternalClient::new(target.bot_socket_path);
        let request = ForumTopicThreadRequest {
            invocation_id,
            token: target.bot_send_token,
            message_thread_id: params.message_thread_id,
        };
        let fut = async {
            if new_state == "closed" {
                client.forum_topic_close(&request).await
            } else {
                client.forum_topic_reopen(&request).await
            }
        };
        match tokio::time::timeout(FORUM_TOPIC_TIMEOUT, fut).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Ok(forum_op_error(e)),
            Err(_) => return Ok(tool_error("forum_op_failed", "forum state change timed out", None)),
        }
        let conn_arc = self.get_conn(agent_name).await?;
        let conn = conn_arc.lock().await;
        right_db::forum_topics::set_state(
            &conn,
            target.chat_id,
            i64::from(params.message_thread_id),
            new_state,
        )
        .await
        .map_err(|e| anyhow::anyhow!("forum registry write failed: {e}"))?;
        Ok(forum_success())
    }

    async fn call_forum_topic_list(
        &self,
        agent_name: &str,
        context: crate::progress::ToolCallContext,
        _args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let Some(invocation_id) = context.invocation_id else {
            return Ok(forum_scope_unavailable());
        };
        // chat-level scope: reuse conversation_scope (chat_id only).
        let scope = match self.progress.conversation_scope(&invocation_id).await {
            Ok(s) => s,
            Err(_) => return Ok(forum_scope_unavailable()),
        };
        let conn_arc = self.get_conn(agent_name).await?;
        let conn = conn_arc.lock().await;
        let rows = right_db::forum_topics::list(&conn, scope.chat_id)
            .await
            .map_err(|e| anyhow::anyhow!("forum list failed: {e}"))?;
        let json: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|t| {
                serde_json::json!({
                    "message_thread_id": t.message_thread_id,
                    "name": t.name,
                    "icon_color": t.icon_color,
                    "icon_custom_emoji_id": t.icon_custom_emoji_id,
                    "state": t.state,
                })
            })
            .collect();
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({ "topics": json }).to_string(),
        )]))
    }
```

Add these free helpers near `conversation_scope_unavailable()` (search for that fn):

```rust
fn forum_scope_unavailable() -> CallToolResult {
    tool_error(
        "forum_scope_unavailable",
        "forum topic tools are available only in the current foreground invocation in a group chat",
        None,
    )
}

fn forum_success() -> CallToolResult {
    CallToolResult::success(vec![Content::text(
        serde_json::json!({ "status": "ok" }).to_string(),
    )])
}

fn forum_op_error(e: right_mcp::internal_client::InternalClientError) -> CallToolResult {
    // The bot endpoint already mapped Telegram errors to a friendly message in
    // the response body; surface it verbatim.
    let msg = match &e {
        right_mcp::internal_client::InternalClientError::Server { body, .. } => {
            // body is JSON { "error": "<message>" }
            serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v.get("error").and_then(|s| s.as_str()).map(ToOwned::to_owned))
                .unwrap_or_else(|| format!("{e:#}"))
        }
        _ => format!("{e:#}"),
    };
    tool_error("forum_op_failed", msg, None)
}
```

- [ ] **Step 5: Write the validation tests**

Add to the `#[cfg(test)] mod tests` block in `right_backend.rs` (or its `*_tests.rs` if it has one):

```rust
    #[tokio::test]
    async fn forum_topic_create_rejects_bad_icon_color() {
        let backend = RightBackend::new(std::env::temp_dir(), None);
        let args = serde_json::json!({ "name": "Bugs", "icon_color": 123 });
        let ctx = crate::progress::ToolCallContext { invocation_id: Some("x".to_owned()) };
        let res = backend
            .tools_call("agent", std::path::Path::new("/tmp"), "forum_topic_create", args, ctx)
            .await
            .unwrap();
        assert!(res.is_error.unwrap_or(false), "bad icon_color must be a tool error");
    }

    #[tokio::test]
    async fn forum_topic_create_rejects_empty_name() {
        let backend = RightBackend::new(std::env::temp_dir(), None);
        let args = serde_json::json!({ "name": "   " });
        let ctx = crate::progress::ToolCallContext { invocation_id: Some("x".to_owned()) };
        let res = backend
            .tools_call("agent", std::path::Path::new("/tmp"), "forum_topic_create", args, ctx)
            .await
            .unwrap();
        assert!(res.is_error.unwrap_or(false));
    }
```

If `RightBackend::new` / `ToolCallContext` construction differs, copy the pattern an existing `right_backend.rs` test uses. These validate the argument gate before any registry/invocation lookup is reached (the bad-icon and empty-name checks run first).

- [ ] **Step 6: Run tests + build crate**

Run: `devenv shell -- cargo test -p right forum_topic`
Then: `devenv shell -- cargo build -p right`
Expected: validation tests pass; crate builds.

- [ ] **Step 7: Commit**

```bash
git add crates/right/src/right_backend.rs
git commit -m "feat(mcp): forum_topic create/edit/close/reopen/list tools"
```

---

### Task 7: Aggregator + memory-server instructions

**Files:**
- Modify: `crates/right/src/aggregator.rs` (`with_instructions()` lines 581–617)
- Modify: `crates/right/src/memory_server.rs` (`with_instructions()` lines 513–538)

- [ ] **Step 1: Add the Forum Topics block to aggregator.rs**

In `with_instructions()` of `aggregator.rs`, insert after the `## Progress` block and before `## Learning` (keep the `\n\` line-continuation style exactly):

```rust
     ## Forum Topics (forum supergroups only)\n\
     - mcp__right__forum_topic_create: Create a topic in the current group; returns its message_thread_id.\n\
     - mcp__right__forum_topic_edit: Rename / re-icon a topic by message_thread_id.\n\
     - mcp__right__forum_topic_close / mcp__right__forum_topic_reopen: Archive / restore a topic (reversible; never deletes).\n\
     - mcp__right__forum_topic_list: List topics this agent tracks in the CURRENT chat only (server-scoped).\n\
     You cannot delete topics. Requires the bot's 'Manage Topics' admin right; errors surface as forum_op_failed with an actionable message.\n\n\
```

- [ ] **Step 2: Add the same block to memory_server.rs**

In `with_instructions()` of `memory_server.rs`, insert an equivalent `## Forum Topics` block after the `## Progress` block. Add a stdio caveat sentence: `DO NOT call in stdio mode — these require the HTTP aggregator scope.` (mirrors how conversation search / progress are annotated there).

- [ ] **Step 3: Build + run any instructions tests**

Run: `devenv shell -- cargo build -p right`
Then: `devenv shell -- cargo test -p right with_instructions` (if no such test exists, this is a no-op — proceed).
Expected: builds.

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/aggregator.rs crates/right/src/memory_server.rs
git commit -m "docs(mcp): list forum topic tools in aggregator + memory-server instructions"
```

---

### Task 8: Prompt — capability awareness

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` (Communication section)

- [ ] **Step 1: Add a compact subsection**

Under `## Communication`, after the `### Formatting` subsection, add:

```markdown
### Forum Topics

In a Telegram forum supergroup you can organize the conversation into topics:
create, rename, close, and reopen them via `mcp__right__forum_topic_*` tools.
You cannot delete topics. `mcp__right__forum_topic_list` shows topics you track
in the current chat. These need the bot's "Manage Topics" admin right; if it's
missing the tool returns an actionable error to relay.
```

Keep it to this paragraph — prompt-tier brevity budget.

- [ ] **Step 2: Verify codegen still renders**

Run: `devenv shell -- cargo test -p right-codegen`
Expected: existing template/codegen tests pass (the file is bundled at build time).

- [ ] **Step 3: Commit**

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "docs(prompt): note forum-topic capability in operating instructions"
```

---

### Task 9: Prompt — bootstrap group/admin nudge

**Files:**
- Modify: `crates/right-codegen/templates/right/agent/BOOTSTRAP.md` (Sequence section, step 6)

- [ ] **Step 1: Extend step 6**

Change the Sequence step 6 line from:

```markdown
6. Quick recap, then write IDENTITY.md, SOUL.md, USER.md.
```

to:

```markdown
6. Quick recap. Mention you work best in a group where you're an admin — then you can organize it into topics and manage the chat. Then write IDENTITY.md, SOUL.md, USER.md.
```

One sentence, no new step, within brevity budget.

- [ ] **Step 2: Verify codegen**

Run: `devenv shell -- cargo test -p right-codegen`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add crates/right-codegen/templates/right/agent/BOOTSTRAP.md
git commit -m "docs(prompt): bootstrap nudge to add the agent as a group admin"
```

---

### Task 10: Architecture + PROMPT_SYSTEM docs

**Files:**
- Modify: `ARCHITECTURE.md` (MCP Aggregator section — conversation-search scope rule)
- Modify: `PROMPT_SYSTEM.md` (tool inventory)

- [ ] **Step 1: Add the scope invariant to ARCHITECTURE.md**

In the `### MCP Aggregator` section, after the paragraph describing `thread_search`/`chat_search` scope enforcement, add:

```markdown
`mcp__right__forum_topic_list` is scoped the same way: it returns only the
current `chat_id`'s tracked topics, resolved server-side from the invocation —
never agent-supplied. Forum write tools (`forum_topic_create`/`_edit`/`_close`/
`_reopen`) resolve `chat_id` identically and never accept it as an argument; no
delete tool exists.
```

This is a load-bearing scope rule (passes the rule + enforcement + brevity tests), so it belongs in the prescriptive doc. Confirm `ARCHITECTURE.md` stays under 40k chars after the edit (`wc -c ARCHITECTURE.md`); if it would exceed, trim elsewhere per the AGENTS.md budget rule.

- [ ] **Step 2: Document the tools in PROMPT_SYSTEM.md**

Find where PROMPT_SYSTEM.md enumerates the `mcp__right__*` tools (search `send_progress` or `thread_search`) and add the five `forum_topic_*` tools with one-line descriptions matching the `tools_list` text. Keep operator-facing detail here (not in the prompt templates).

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md PROMPT_SYSTEM.md
git commit -m "docs: forum-topic scope invariant + PROMPT_SYSTEM tool inventory"
```

---

### Task 11: Final verification

- [ ] **Step 1: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: all pass. Note: two tests (`cc/invocation` pid race, dashboard warn-count) are known to flake under parallel load — if either fails, re-run it in isolation before attributing the failure to this change.

- [ ] **Step 2: Clippy + build**

Run: `devenv shell -- cargo clippy --workspace --all-targets` and `devenv shell -- cargo build --workspace`
Expected: no new warnings; debug build succeeds.

- [ ] **Step 3: Rust review subagent**

Dispatch the `rust-dev:review-rust-code` subagent over the diff. Turn any real findings into follow-up fixes (FAIL FAST, error-chain preservation `{e:#}`, no swallowed errors, no `unwrap` in non-test code).

- [ ] **Step 4: Confirm "no delete" guarantee**

Grep the diff: there must be NO `delete_forum_topic`, `deleteForumTopic`, or any forum-delete tool/route anywhere.

Run: `git diff master --stat` and `rg -i "delete_forum|deleteForum"` — expected: no matches in new code.

- [ ] **Step 5: Final commit (if review produced fixes)**

```bash
git add -A
git commit -m "fix: address rust review findings for forum topic management"
```

---

## Self-review checklist (author-completed)

**Spec coverage:** B-operations (create/edit/close/reopen) → Tasks 5–6; no delete → Tasks 6, 11/Step 4; no General methods → omitted; gating via Telegram rights only → no config task (intentional); 4 explicit tools + `forum_topic_list` → Task 6; data.db registry → Tasks 1–2; authoritative population from tool results → Task 6 (writes after bot success); deferred passive human-topic observation → not implemented (spec: deferred); strict current-chat scope → Tasks 2 (test), 4 (`forum_target`), 6 (`forum_topic_list` uses `conversation_scope`), 10 (invariant doc); prompt awareness → Task 8; bootstrap nudge → Task 9; with_instructions update → Task 7; FAIL FAST error mapping → Task 5 (`forum_error_message`) + Task 6 (`forum_op_error`); tests + final workspace → Tasks 1–6, 11.

**Type consistency:** `ForumTopicCreateRequest`/`EditRequest`/`ThreadRequest`, `ForumTopicCreateResponse`/`OkResponse` used identically in Tasks 3, 5, 6. `forum_target` (Task 4) consumed in Task 6. `upsert_created`/`update_edited`/`set_state`/`list` (Task 2) called with matching signatures in Task 6. `ALLOWED_ICON_COLORS` defined and used in Task 6.

**Known adaptation points (compiler-verified per task):** `Rgb::from_u32` vs byte-construction; `ForumTopic.thread_id.0.0` accessor; teloxide builder setter names; exact `ProgressRegistration`/`ToolCallContext`/`RightBackend::new` constructors — each task says to confirm against the neighbouring real code and the `cargo build`/`cargo test` step at the end of the task catches mismatches.
