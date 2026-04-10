# Session Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add multi-session support (/new, /switch, /list) to the Telegram bot and fix slash command autocomplete.

**Architecture:** Replace single-session `telegram_sessions` table with multi-session `sessions` table. New BotCommand variants route to handlers that manage session lifecycle via `is_active` flag. Autocomplete fixed by scoping `set_my_commands` per chat_id.

**Tech Stack:** Rust, teloxide, rusqlite, rusqlite_migration, chrono

---

### Task 1: Schema migration — drop `telegram_sessions`, create `sessions`

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v4_sessions.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/rightclaw/src/memory/migrations.rs` (or the existing test location for migrations):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn migrations_apply_cleanly_to_v4() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        // sessions table exists with expected columns
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name IN ('id','chat_id','thread_id','root_session_id','label','is_active','created_at','last_used_at')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 8, "sessions table should have all 8 columns");
        // telegram_sessions should NOT exist
        let old_exists: bool = conn
            .prepare("SELECT 1 FROM telegram_sessions LIMIT 1")
            .is_ok();
        assert!(!old_exists, "telegram_sessions should be dropped");
    }

    #[test]
    fn sessions_partial_unique_index_enforces_single_active() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'aaa', 1)",
            [],
        ).unwrap();
        // Second active session for same (chat_id, thread_id) should fail
        let result = conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'bbb', 1)",
            [],
        );
        assert!(result.is_err(), "partial unique index should prevent two active sessions");
    }

    #[test]
    fn sessions_allows_multiple_inactive() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'aaa', 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'bbb', 0)",
            [],
        ).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions WHERE chat_id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib memory::migrations`
Expected: FAIL — `MIGRATIONS` only has 3 entries, no V4, `sessions` table doesn't exist.

- [ ] **Step 3: Create SQL migration file**

Create `crates/rightclaw/src/memory/sql/v4_sessions.sql`:

```sql
-- V4 schema: multi-session support
-- Replaces telegram_sessions (single session per chat+thread)
-- with sessions (multiple sessions per chat+thread, one active at a time).

DROP TABLE IF EXISTS telegram_sessions;

CREATE TABLE sessions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id         INTEGER NOT NULL,
    thread_id       INTEGER NOT NULL DEFAULT 0,
    root_session_id TEXT    NOT NULL,
    label           TEXT,
    is_active       INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
    last_used_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);

CREATE UNIQUE INDEX idx_sessions_active
    ON sessions(chat_id, thread_id) WHERE is_active = 1;
```

- [ ] **Step 4: Register migration in migrations.rs**

In `crates/rightclaw/src/memory/migrations.rs`:

```rust
use rusqlite_migration::{Migrations, M};

const V1_SCHEMA: &str = include_str!("sql/v1_schema.sql");
const V2_SCHEMA: &str = include_str!("sql/v2_telegram_sessions.sql");
const V3_SCHEMA: &str = include_str!("sql/v3_cron_runs.sql");
const V4_SCHEMA: &str = include_str!("sql/v4_sessions.sql");

pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| {
        Migrations::new(vec![M::up(V1_SCHEMA), M::up(V2_SCHEMA), M::up(V3_SCHEMA), M::up(V4_SCHEMA)])
    });
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rightclaw --lib memory::migrations`
Expected: PASS — all 3 tests green.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v4_sessions.sql crates/rightclaw/src/memory/migrations.rs
git commit -m "feat: add V4 migration — sessions table replaces telegram_sessions"
```

---

### Task 2: Rewrite `session.rs` — new CRUD for `sessions` table

**Files:**
- Modify: `crates/bot/src/telegram/session.rs`

- [ ] **Step 1: Write tests for new session CRUD**

Replace the existing test module in `crates/bot/src/telegram/session.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rightclaw::memory::open_connection;
    use tempfile::tempdir;

    fn test_conn() -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path()).unwrap();
        (dir, conn)
    }

    fn normalise_thread_id(thread_id: Option<ThreadId>) -> i64 {
        match thread_id {
            Some(ThreadId(MessageId(1))) => 0,
            Some(ThreadId(MessageId(n))) => i64::from(n),
            None => 0,
        }
    }

    #[test]
    fn effective_thread_id_general_topic() {
        assert_eq!(normalise_thread_id(Some(ThreadId(MessageId(1)))), 0);
    }

    #[test]
    fn effective_thread_id_none() {
        assert_eq!(normalise_thread_id(None), 0);
    }

    #[test]
    fn effective_thread_id_real_topic() {
        assert_eq!(normalise_thread_id(Some(ThreadId(MessageId(5)))), 5);
    }

    #[test]
    fn get_active_returns_none_for_empty_db() {
        let (_dir, conn) = test_conn();
        assert!(get_active_session(&conn, 100, 0).unwrap().is_none());
    }

    #[test]
    fn create_then_get_active() {
        let (_dir, conn) = test_conn();
        let id = create_session(&conn, 100, 0, "uuid-1", Some("hello world")).unwrap();
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        assert_eq!(active.id, id);
        assert_eq!(active.root_session_id, "uuid-1");
        assert_eq!(active.label.as_deref(), Some("hello world"));
    }

    #[test]
    fn deactivate_clears_active() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "uuid-1", None).unwrap();
        let prev = deactivate_current(&conn, 100, 0).unwrap();
        assert_eq!(prev.as_deref(), Some("uuid-1"));
        assert!(get_active_session(&conn, 100, 0).unwrap().is_none());
    }

    #[test]
    fn deactivate_returns_none_when_no_active() {
        let (_dir, conn) = test_conn();
        let prev = deactivate_current(&conn, 100, 0).unwrap();
        assert!(prev.is_none());
    }

    #[test]
    fn activate_session_by_id() {
        let (_dir, conn) = test_conn();
        let id = create_session(&conn, 100, 0, "uuid-1", None).unwrap();
        deactivate_current(&conn, 100, 0).unwrap();
        activate_session(&conn, id).unwrap();
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        assert_eq!(active.root_session_id, "uuid-1");
    }

    #[test]
    fn list_sessions_ordered_by_last_used() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "uuid-old", Some("old")).unwrap();
        deactivate_current(&conn, 100, 0).unwrap();
        create_session(&conn, 100, 0, "uuid-new", Some("new")).unwrap();
        // Touch the new one to ensure ordering
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        touch_session(&conn, active.id).unwrap();

        let sessions = list_sessions(&conn, 100, 0).unwrap();
        assert_eq!(sessions.len(), 2);
        // Most recently used first
        assert_eq!(sessions[0].root_session_id, "uuid-new");
    }

    #[test]
    fn find_session_by_partial_uuid() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "550e8400-e29b-41d4", None).unwrap();
        deactivate_current(&conn, 100, 0).unwrap();
        create_session(&conn, 100, 0, "7a3f1b22-c9d8-4e5f", None).unwrap();

        let matches = find_sessions_by_uuid(&conn, 100, 0, "550e").unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].root_session_id, "550e8400-e29b-41d4");
    }

    #[test]
    fn find_session_partial_returns_multiple() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "aaa-111", None).unwrap();
        deactivate_current(&conn, 100, 0).unwrap();
        create_session(&conn, 100, 0, "aaa-222", None).unwrap();

        let matches = find_sessions_by_uuid(&conn, 100, 0, "aaa").unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn truncate_label_at_60_chars() {
        let long = "a".repeat(100);
        assert_eq!(truncate_label(&long).len(), 60);
        assert_eq!(truncate_label("short"), "short");
    }

    #[test]
    fn sessions_isolated_by_thread_id() {
        let (_dir, conn) = test_conn();
        create_session(&conn, 100, 0, "thread0", None).unwrap();
        create_session(&conn, 100, 5, "thread5", None).unwrap();

        let t0 = get_active_session(&conn, 100, 0).unwrap().unwrap();
        let t5 = get_active_session(&conn, 100, 5).unwrap().unwrap();
        assert_eq!(t0.root_session_id, "thread0");
        assert_eq!(t5.root_session_id, "thread5");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot --lib telegram::session`
Expected: FAIL — new functions don't exist yet.

- [ ] **Step 3: Implement new session.rs**

Replace the body of `crates/bot/src/telegram/session.rs` (keep the tests from Step 1):

```rust
//! Per-thread session CRUD against `sessions` SQLite table.
//!
//! Supports multiple sessions per (chat_id, thread_id) with at most one active.

use teloxide::types::{Message, MessageId, ThreadId};

/// A session row from the `sessions` table.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: i64,
    pub chat_id: i64,
    pub thread_id: i64,
    pub root_session_id: String,
    pub label: Option<String>,
    pub is_active: bool,
    pub created_at: String,
    pub last_used_at: String,
}

/// Normalise Telegram thread_id for session keying and reply routing.
pub fn effective_thread_id(msg: &Message) -> i64 {
    match msg.thread_id {
        Some(ThreadId(MessageId(1))) => 0,
        Some(ThreadId(MessageId(n))) => i64::from(n),
        None => 0,
    }
}

/// Truncate a string to at most 60 chars for use as a session label.
pub fn truncate_label(s: &str) -> &str {
    match s.char_indices().nth(60) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Get the active session for (chat_id, thread_id), or None.
pub fn get_active_session(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<SessionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at \
         FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND is_active = 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![chat_id, thread_id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_session(row)?)),
        None => Ok(None),
    }
}

/// Create a new active session. Returns the row id.
pub fn create_session(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
    session_uuid: &str,
    label: Option<&str>,
) -> Result<i64, rusqlite::Error> {
    conn.execute(
        "INSERT INTO sessions (chat_id, thread_id, root_session_id, label, is_active) \
         VALUES (?1, ?2, ?3, ?4, 1)",
        rusqlite::params![chat_id, thread_id, session_uuid, label],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Deactivate the current active session for (chat_id, thread_id).
/// Returns the previous session's root_session_id, or None if no active session.
pub fn deactivate_current(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<String>, rusqlite::Error> {
    let prev = get_active_session(conn, chat_id, thread_id)?;
    conn.execute(
        "UPDATE sessions SET is_active = 0 WHERE chat_id = ?1 AND thread_id = ?2 AND is_active = 1",
        rusqlite::params![chat_id, thread_id],
    )?;
    Ok(prev.map(|s| s.root_session_id))
}

/// Re-activate a session by row id.
pub fn activate_session(
    conn: &rusqlite::Connection,
    session_id: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE sessions SET is_active = 1 WHERE id = ?1",
        rusqlite::params![session_id],
    )?;
    Ok(())
}

/// Update last_used_at for a session by row id.
pub fn touch_session(
    conn: &rusqlite::Connection,
    session_id: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE sessions SET last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id = ?1",
        rusqlite::params![session_id],
    )?;
    Ok(())
}

/// List all sessions for (chat_id, thread_id) ordered by last_used_at DESC.
pub fn list_sessions(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Vec<SessionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at \
         FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 ORDER BY last_used_at DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![chat_id, thread_id], |row| {
        row_to_session(row)
    })?;
    rows.collect()
}

/// Find sessions matching a partial UUID for (chat_id, thread_id).
pub fn find_sessions_by_uuid(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
    partial: &str,
) -> Result<Vec<SessionRow>, rusqlite::Error> {
    let pattern = format!("%{partial}%");
    let mut stmt = conn.prepare_cached(
        "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at \
         FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND root_session_id LIKE ?3",
    )?;
    let rows = stmt.query_map(rusqlite::params![chat_id, thread_id, pattern], |row| {
        row_to_session(row)
    })?;
    rows.collect()
}

fn row_to_session(row: &rusqlite::Row) -> Result<SessionRow, rusqlite::Error> {
    Ok(SessionRow {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        thread_id: row.get(2)?,
        root_session_id: row.get(3)?,
        label: row.get(4)?,
        is_active: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        last_used_at: row.get(7)?,
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot --lib telegram::session`
Expected: PASS — all tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/session.rs
git commit -m "feat: rewrite session.rs — multi-session CRUD for sessions table"
```

---

### Task 3: Update `worker.rs` — use new session functions

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

The worker currently imports `create_session`, `delete_session`, `get_session`, `touch_session` from `session.rs`. These must be replaced with the new functions.

- [ ] **Step 1: Update imports**

In `crates/bot/src/telegram/worker.rs`, replace:

```rust
use super::session::{create_session, delete_session, get_session, touch_session};
```

with:

```rust
use super::session::{create_session, deactivate_current, get_active_session, touch_session, truncate_label};
```

- [ ] **Step 2: Update `invoke_cc` session lookup/create block**

Find the session lookup block in `invoke_cc` (~line 771-787) and replace:

```rust
    // Session lookup / create (SES-02, SES-03)
    let (cmd_args, is_first_call) = match get_session(&conn, chat_id, eff_thread_id) {
        Ok(Some(root_id)) => {
            // Resume: --resume <root_session_id>
            (vec!["--resume".to_string(), root_id], false)
        }
        Ok(None) => {
            // First message: generate UUID, --session-id <uuid>
            let new_uuid = Uuid::new_v4().to_string();
            create_session(&conn, chat_id, eff_thread_id, &new_uuid)
                .map_err(|e| format!("⚠️ Agent error: session create failed: {:#}", e))?;
            (vec!["--session-id".to_string(), new_uuid], true)
        }
        Err(e) => {
            return Err(format!("⚠️ Agent error: session lookup failed: {:#}", e));
        }
    };
```

with:

```rust
    // Session lookup / create (SES-02, SES-03)
    let (cmd_args, is_first_call, session_row_id) = match get_active_session(&conn, chat_id, eff_thread_id) {
        Ok(Some(session)) => {
            // Resume: --resume <root_session_id>
            let id = session.id;
            (vec!["--resume".to_string(), session.root_session_id], false, id)
        }
        Ok(None) => {
            // First message: generate UUID, --session-id <uuid>
            let new_uuid = Uuid::new_v4().to_string();
            let label = input_text.map(|t| truncate_label(t).to_string());
            let id = create_session(&conn, chat_id, eff_thread_id, &new_uuid, label.as_deref())
                .map_err(|e| format!("⚠️ Agent error: session create failed: {:#}", e))?;
            (vec!["--session-id".to_string(), new_uuid], true, id)
        }
        Err(e) => {
            return Err(format!("⚠️ Agent error: session lookup failed: {:#}", e));
        }
    };
```

Note: `input_text` must be threaded into `invoke_cc`. Currently the function receives `input: &str` which is the formatted YAML/text. The label should be the user's first text message. The simplest approach: pass the raw first message text as an extra parameter to `invoke_cc`. Check the call site in the worker loop to see what's available.

Read `worker.rs` around the `invoke_cc` call site to find the raw text. The `DebounceMsg` struct has a `text: Option<String>` field. Pass `first_msg.text.as_deref()` as `input_text` to `invoke_cc`.

Update `invoke_cc` signature:

```rust
async fn invoke_cc(
    input: &str,
    input_text: Option<&str>,  // raw first message text for session label
    chat_id: i64,
    eff_thread_id: i64,
    ctx: &WorkerContext,
) -> Result<Option<ReplyOutput>, String> {
```

Update all call sites of `invoke_cc` to pass the additional parameter.

- [ ] **Step 3: Update `delete_session` calls to `deactivate_current`**

There are two `delete_session` calls in worker.rs:

1. After bootstrap completion (~line 400): replace `delete_session(&conn, chat_id, eff_thread_id)` with `deactivate_current(&conn, chat_id, eff_thread_id)`
2. After auth error (~line 1197): replace `delete_session(&conn, chat_id, eff_thread_id).ok()` with `deactivate_current(&conn, chat_id, eff_thread_id).ok()`

- [ ] **Step 4: Update `touch_session` call**

Replace:

```rust
touch_session(&conn, chat_id, eff_thread_id)
```

with:

```rust
touch_session(&conn, session_row_id)
```

The `session_row_id` variable is available from the session lookup block above. Ensure it's in scope where `touch_session` is called — it may need to be returned from the session lookup block alongside `cmd_args` and `is_first_call`.

- [ ] **Step 5: Update `get_session` call in session ID verification**

Find the D-15 verification block (~line 1245):

```rust
&& let Ok(Some(stored)) = get_session(&conn, chat_id, eff_thread_id)
```

Replace with:

```rust
&& let Ok(Some(stored)) = get_active_session(&conn, chat_id, eff_thread_id)
```

And update the comparison: `stored` is now a `SessionRow`, so compare `cc_sid != stored.root_session_id`.

- [ ] **Step 6: Build and verify**

Run: `cargo build --workspace`
Expected: Compiles without errors. (Tests may fail if handler.rs still references old session functions — that's fixed in Task 4.)

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat: update worker to use multi-session CRUD"
```

---

### Task 4: Update `dispatch.rs` — new BotCommand enum + per-chat `set_my_commands`

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Update BotCommand enum**

Replace:

```rust
#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum BotCommand {
    #[command(description = "Start interacting with this agent")]
    Start,
    #[command(description = "Reset conversation session for this thread")]
    Reset,
    #[command(description = "MCP server management (list/add/remove)")]
    Mcp(String),
    #[command(description = "Run diagnostics")]
    Doctor,
}
```

with:

```rust
#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum BotCommand {
    #[command(description = "Start interacting with this agent")]
    Start,
    #[command(description = "Start a new conversation")]
    New(String),
    #[command(description = "List all sessions")]
    List,
    #[command(description = "Switch to another session")]
    Switch(String),
    #[command(description = "MCP server management (list/add/remove)")]
    Mcp(String),
    #[command(description = "Run diagnostics")]
    Doctor,
}
```

Note: `New(String)` and `Switch(String)` capture everything after the command. Empty string = no argument.

- [ ] **Step 2: Update imports**

Replace the import line:

```rust
use super::handler::{handle_doctor, handle_mcp, handle_message, handle_reset, handle_start, handle_stop_callback, AgentDir, AgentSettings, AuthCodeSlot, AuthWatcherFlag, DebugFlag, RefreshTx, RightclawHome, SshConfigPath};
```

with:

```rust
use super::handler::{handle_doctor, handle_list, handle_mcp, handle_message, handle_new, handle_start, handle_stop_callback, handle_switch, AgentDir, AgentSettings, AuthCodeSlot, AuthWatcherFlag, DebugFlag, RefreshTx, RightclawHome, SshConfigPath};
```

- [ ] **Step 3: Update dispatch schema branches**

Replace:

```rust
    let command_handler = dptree::entry()
        .filter_command::<BotCommand>()
        .branch(
            dptree::case![BotCommand::Start].endpoint(handle_start),
        )
        .branch(
            dptree::case![BotCommand::Reset].endpoint(handle_reset),
        )
        .branch(
            dptree::case![BotCommand::Mcp(args)].endpoint(handle_mcp),
        )
        .branch(
            dptree::case![BotCommand::Doctor].endpoint(handle_doctor),
        );
```

with:

```rust
    let command_handler = dptree::entry()
        .filter_command::<BotCommand>()
        .branch(
            dptree::case![BotCommand::Start].endpoint(handle_start),
        )
        .branch(
            dptree::case![BotCommand::New(name)].endpoint(handle_new),
        )
        .branch(
            dptree::case![BotCommand::List].endpoint(handle_list),
        )
        .branch(
            dptree::case![BotCommand::Switch(uuid)].endpoint(handle_switch),
        )
        .branch(
            dptree::case![BotCommand::Mcp(args)].endpoint(handle_mcp),
        )
        .branch(
            dptree::case![BotCommand::Doctor].endpoint(handle_doctor),
        );
```

- [ ] **Step 4: Replace `set_my_commands` with per-chat scope**

Replace:

```rust
    // Register /reset command with Telegram Bot API.
    // First delete all existing commands to clear any leftover commands from other clients
    // (e.g., CC Telegram plugin sets /status /help /start -- these must be cleared).
    match bot.delete_my_commands().await {
        Ok(_) => tracing::info!("delete_my_commands succeeded"),
        Err(e) => tracing::warn!("delete_my_commands failed (non-fatal): {e:#}"),
    }
    match bot.set_my_commands(BotCommand::bot_commands()).await {
        Ok(_) => tracing::info!("set_my_commands succeeded -- commands registered"),
        Err(e) => tracing::warn!("set_my_commands failed (non-fatal): {e:#}"),
    }
```

with:

```rust
    // Register commands per-chat using BotCommandScope::Chat.
    // Per-chat scope has higher priority than default scope in Telegram's resolution,
    // so CC Telegram plugin's default-scope commands won't overwrite ours.
    let commands = BotCommand::bot_commands();
    for &cid in &allowed {
        let scope = teloxide::types::BotCommandScope::Chat {
            chat_id: teloxide::types::Recipient::Id(teloxide::types::ChatId(cid)),
        };
        match bot.delete_my_commands().scope(scope.clone()).await {
            Ok(_) => {}
            Err(e) => tracing::warn!(chat_id = cid, "delete_my_commands(chat) failed: {e:#}"),
        }
        match bot.set_my_commands(commands.clone()).scope(scope).await {
            Ok(_) => tracing::info!(chat_id = cid, "set_my_commands(chat) succeeded"),
            Err(e) => tracing::warn!(chat_id = cid, "set_my_commands(chat) failed: {e:#}"),
        }
    }
    if allowed.is_empty() {
        tracing::info!("no allowed_chat_ids — skipping set_my_commands");
    }
```

Note: `allowed` is already a `HashSet<i64>` available in scope from the existing code.

- [ ] **Step 5: Build to verify compilation**

Run: `cargo build --workspace`
Expected: May fail if handler functions don't exist yet — that's Task 5. If so, add stub functions in handler.rs to unblock.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/dispatch.rs
git commit -m "feat: update BotCommand enum and per-chat set_my_commands"
```

---

### Task 5: Add `/new`, `/list`, `/switch` handlers in `handler.rs`

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Update imports**

Replace:

```rust
use super::session::{delete_session, effective_thread_id};
```

with:

```rust
use super::session::{activate_session, create_session, deactivate_current, effective_thread_id, find_sessions_by_uuid, get_active_session, list_sessions, truncate_label};
```

- [ ] **Step 2: Implement `handle_new`**

Add after `handle_start`:

```rust
/// Handle the /new command — start a new session.
///
/// 1. Deactivate current active session.
/// 2. Kill worker (drop sender from DashMap).
/// 3. If name provided: create new session immediately with label.
/// 4. Reply with confirmation + previous session UUID.
pub async fn handle_new(
    bot: BotType,
    msg: Message,
    name: String,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);

    let conn = rightclaw::memory::open_connection(&agent_dir.0)
        .map_err(|e| to_request_err(format!("new: open DB: {:#}", e)))?;

    // Deactivate current session
    let prev_uuid = deactivate_current(&conn, chat_id.0, eff_thread_id)
        .map_err(|e| to_request_err(format!("new: deactivate: {:#}", e)))?;

    // Kill worker — channel closes, CC subprocess killed via kill_on_drop
    worker_map.remove(&key);

    let name = name.trim().to_string();
    let mut reply = String::new();

    if !name.is_empty() {
        // Named session: create immediately
        let new_uuid = uuid::Uuid::new_v4().to_string();
        let label = truncate_label(&name);
        create_session(&conn, chat_id.0, eff_thread_id, &new_uuid, Some(label))
            .map_err(|e| to_request_err(format!("new: create session: {:#}", e)))?;
        reply.push_str(&format!("New session: {name}\n"));
    } else {
        reply.push_str("Session cleared.\n");
    }

    if let Some(prev) = prev_uuid {
        reply.push_str(&format!(
            "Previous session:\n<pre>{prev}</pre>\nTap to copy, then /switch to return."
        ));
    }

    if name.is_empty() {
        reply.push_str("\nSend a message to start a new conversation.");
    }

    let mut send = bot.send_message(chat_id, &reply)
        .parse_mode(teloxide::types::ParseMode::Html);
    if eff_thread_id != 0 {
        send = send.message_thread_id(teloxide::types::ThreadId(
            teloxide::types::MessageId(eff_thread_id as i32),
        ));
    }
    send.await?;

    tracing::info!(?key, "new session");
    Ok(())
}
```

- [ ] **Step 3: Implement `handle_list`**

```rust
/// Handle the /list command — show all sessions for this chat+thread.
pub async fn handle_list(
    bot: BotType,
    msg: Message,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);

    let conn = rightclaw::memory::open_connection(&agent_dir.0)
        .map_err(|e| to_request_err(format!("list: open DB: {:#}", e)))?;

    let sessions = list_sessions(&conn, chat_id.0, eff_thread_id)
        .map_err(|e| to_request_err(format!("list: query: {:#}", e)))?;

    if sessions.is_empty() {
        bot.send_message(chat_id, "No sessions yet. Send a message to start one.")
            .await?;
        return Ok(());
    }

    let mut text = String::from("Sessions:\n");
    for s in &sessions {
        let marker = if s.is_active { "●" } else { " " };
        let label = s.label.as_deref().unwrap_or("(unnamed)");
        let ago = format_relative_time(&s.last_used_at);
        text.push_str(&format!(
            "{marker} {label} — {ago}\n<pre>{}</pre>\n",
            s.root_session_id
        ));
    }

    let mut send = bot.send_message(chat_id, &text)
        .parse_mode(teloxide::types::ParseMode::Html);
    if eff_thread_id != 0 {
        send = send.message_thread_id(teloxide::types::ThreadId(
            teloxide::types::MessageId(eff_thread_id as i32),
        ));
    }
    send.await?;
    Ok(())
}

/// Format an ISO timestamp as a relative time string (e.g. "5m ago", "2h ago").
fn format_relative_time(iso_timestamp: &str) -> String {
    let Ok(then) = chrono::NaiveDateTime::parse_from_str(iso_timestamp, "%Y-%m-%dT%H:%M:%SZ") else {
        return iso_timestamp.to_string();
    };
    let then_utc = then.and_utc();
    let now = chrono::Utc::now();
    let delta = now - then_utc;

    if delta.num_minutes() < 1 {
        "just now".to_string()
    } else if delta.num_minutes() < 60 {
        format!("{}m ago", delta.num_minutes())
    } else if delta.num_hours() < 24 {
        format!("{}h ago", delta.num_hours())
    } else {
        format!("{}d ago", delta.num_days())
    }
}
```

- [ ] **Step 4: Implement `handle_switch`**

```rust
/// Handle the /switch command — switch to a different session.
pub async fn handle_switch(
    bot: BotType,
    msg: Message,
    uuid: String,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);
    let uuid = uuid.trim().to_string();

    if uuid.is_empty() {
        bot.send_message(chat_id, "Usage: /switch <uuid>\nUse /list to see available sessions.")
            .await?;
        return Ok(());
    }

    let conn = rightclaw::memory::open_connection(&agent_dir.0)
        .map_err(|e| to_request_err(format!("switch: open DB: {:#}", e)))?;

    let matches = find_sessions_by_uuid(&conn, chat_id.0, eff_thread_id, &uuid)
        .map_err(|e| to_request_err(format!("switch: query: {:#}", e)))?;

    match matches.len() {
        0 => {
            let mut send = bot.send_message(
                chat_id,
                format!("No session matching <pre>{uuid}</pre>. Use /list to see available sessions."),
            ).parse_mode(teloxide::types::ParseMode::Html);
            if eff_thread_id != 0 {
                send = send.message_thread_id(teloxide::types::ThreadId(
                    teloxide::types::MessageId(eff_thread_id as i32),
                ));
            }
            send.await?;
        }
        1 => {
            let target = &matches[0];
            if target.is_active {
                bot.send_message(chat_id, "Already active.").await?;
                return Ok(());
            }

            // Deactivate current, activate target
            deactivate_current(&conn, chat_id.0, eff_thread_id)
                .map_err(|e| to_request_err(format!("switch: deactivate: {:#}", e)))?;
            activate_session(&conn, target.id)
                .map_err(|e| to_request_err(format!("switch: activate: {:#}", e)))?;

            // Kill worker so next message uses the new session
            worker_map.remove(&key);

            let label = target.label.as_deref().unwrap_or("(unnamed)");
            let mut send = bot.send_message(
                chat_id,
                format!("Switched to: {label}\n<pre>{}</pre>", target.root_session_id),
            ).parse_mode(teloxide::types::ParseMode::Html);
            if eff_thread_id != 0 {
                send = send.message_thread_id(teloxide::types::ThreadId(
                    teloxide::types::MessageId(eff_thread_id as i32),
                ));
            }
            send.await?;

            tracing::info!(?key, session = %target.root_session_id, "switched session");
        }
        _ => {
            let mut text = format!("Multiple sessions match <pre>{uuid}</pre>:\n\n");
            for m in &matches {
                let label = m.label.as_deref().unwrap_or("(unnamed)");
                let marker = if m.is_active { "●" } else { " " };
                text.push_str(&format!(
                    "{marker} {label}\n<pre>{}</pre>\n",
                    m.root_session_id
                ));
            }
            text.push_str("\nBe more specific.");
            let mut send = bot.send_message(chat_id, &text)
                .parse_mode(teloxide::types::ParseMode::Html);
            if eff_thread_id != 0 {
                send = send.message_thread_id(teloxide::types::ThreadId(
                    teloxide::types::MessageId(eff_thread_id as i32),
                ));
            }
            send.await?;
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Remove `handle_reset`**

Delete the entire `handle_reset` function and remove `handle_reset` from the public exports used by dispatch.rs.

- [ ] **Step 6: Add `use uuid;` if not already imported**

Check if `uuid` is already in `handler.rs` dependencies. If not, add:

```rust
use uuid::Uuid;
```

(Only needed if `handle_new` generates UUIDs. It does.)

- [ ] **Step 7: Build**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/telegram/handler.rs
git commit -m "feat: add /new, /list, /switch handlers; remove /reset"
```

---

### Task 6: Update `mod.rs` exports

**Files:**
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Check and update exports**

The current `mod.rs` exports `effective_thread_id` from session. This should still work since `effective_thread_id` exists in the new session.rs. No changes needed unless the build reveals import issues.

If there are callers of `delete_session` or `get_session` outside of worker.rs and handler.rs, update them too.

- [ ] **Step 2: Build full workspace**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 3: Commit (if changes needed)**

```bash
git add crates/bot/src/telegram/mod.rs
git commit -m "chore: update telegram module exports for session changes"
```

---

### Task 7: Integration test — full lifecycle

**Files:**
- Modify: `crates/bot/src/telegram/session.rs` (add integration-style test to existing test module)

- [ ] **Step 1: Write multi-session lifecycle test**

Add to the test module in `session.rs`:

```rust
    #[test]
    fn full_lifecycle_new_switch_list() {
        let (_dir, conn) = test_conn();

        // First message creates session 1
        let id1 = create_session(&conn, 100, 0, "uuid-1", Some("hello world")).unwrap();
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        assert_eq!(active.id, id1);

        // /new — deactivate, create session 2
        let prev = deactivate_current(&conn, 100, 0).unwrap();
        assert_eq!(prev.as_deref(), Some("uuid-1"));
        let id2 = create_session(&conn, 100, 0, "uuid-2", Some("second task")).unwrap();

        // /list — both visible, session 2 active
        let all = list_sessions(&conn, 100, 0).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|s| s.root_session_id == "uuid-2" && s.is_active));
        assert!(all.iter().any(|s| s.root_session_id == "uuid-1" && !s.is_active));

        // /switch — back to session 1
        deactivate_current(&conn, 100, 0).unwrap();
        activate_session(&conn, id1).unwrap();
        let active = get_active_session(&conn, 100, 0).unwrap().unwrap();
        assert_eq!(active.root_session_id, "uuid-1");
    }
```

- [ ] **Step 2: Run test**

Run: `cargo test -p rightclaw-bot --lib telegram::session::tests::full_lifecycle_new_switch_list`
Expected: PASS

- [ ] **Step 3: Run full test suite**

Run: `cargo test --workspace`
Expected: PASS — all existing tests still green.

- [ ] **Step 4: Build workspace in debug mode**

Run: `cargo build --workspace`
Expected: PASS — clean compilation, no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/session.rs
git commit -m "test: add multi-session lifecycle integration test"
```

---

### Task 8: Run clippy + final build

- [ ] **Step 1: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: PASS — no warnings.

- [ ] **Step 2: Fix any clippy issues**

If clippy reports issues, fix them.

- [ ] **Step 3: Final commit if needed**

```bash
git add -A
git commit -m "chore: clippy fixes"
```
