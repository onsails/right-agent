# "Show details" button on prettified errors — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Attach a `🔍 Details` inline button to every prettified CC-error Telegram message that, when pressed, replies with the raw `claude -p` JSON we classified at failure time.

**Architecture:** Persist the raw JSON in a new per-agent `data.db` table `error_details` (migration v39, 7-day TTL). On a prettified failure, `invoke_cc` best-effort-stores the raw JSON and threads the row id (`details_id: Option<i64>`) through the `RateLimited` / `Reflectable` `InvokeCcFailure` variants. The worker renders an `errdet:<id>` callback button on the send/edit sites. A new dispatcher branch + callback handler looks the row up (scoped by `chat_id`), and replies with the JSON in `<pre>` (or a `error.json` document when it exceeds Telegram's 4096-char limit).

**Tech Stack:** Rust 2024, teloxide 0.17, `right-db` (Turso/SQLite), tokio.

**Spec:** `docs/superpowers/specs/2026-06-01-error-details-button-design.md`

**Scope note:** Only `RateLimited` and `Reflectable` surfaces get the button — these hide the raw JSON behind friendly copy. The `NonReflectable` parse-failure path already shows truncated raw `stdout` inline (`worker.rs:4142-4148`), so it is intentionally excluded.

---

## File Structure

- `crates/right-db/src/sql/v39_error_details.sql` — **new**: table + index DDL.
- `crates/right-db/src/migrations.rs` — **modify**: register v39, bump `LATEST_SCHEMA_VERSION`.
- `crates/bot/src/telegram/error_details.rs` — **new**: pure helpers (`parse_errdet_id`, `details_keyboard`, `details_payload`), DB helpers (`insert_error_detail`, `get_error_detail`), and the callback handler (`handle_error_details_callback`). Tests colocated in a `#[cfg(test)]` module.
- `crates/bot/src/telegram/mod.rs` — **modify**: `mod error_details;`.
- `crates/bot/src/telegram/worker.rs` — **modify**: add `details_id` to two `InvokeCcFailure` variants, store on failure, render the keyboard at send/edit sites, add `send_error_to_telegram_with_markup`.
- `crates/bot/src/telegram/dispatch.rs` — **modify**: add the `errdet:` callback branch.

---

## Task 1: Migration v39 — `error_details` table

**Files:**
- Create: `crates/right-db/src/sql/v39_error_details.sql`
- Modify: `crates/right-db/src/migrations.rs` (const + registry entry + `LATEST_SCHEMA_VERSION`)

- [ ] **Step 1: Write the SQL file**

Create `crates/right-db/src/sql/v39_error_details.sql`:

```sql
CREATE TABLE IF NOT EXISTS error_details (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  chat_id    INTEGER NOT NULL,
  thread_id  INTEGER NOT NULL,
  raw_json   TEXT    NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_error_details_created_at
  ON error_details (created_at);
```

- [ ] **Step 2: Register the const**

In `crates/right-db/src/migrations.rs`, add next to the other `const V..._SCHEMA` lines (after the `V38_SCHEMA` line, ~line 33):

```rust
const V39_SCHEMA: &str = include_str!("sql/v39_error_details.sql");
```

- [ ] **Step 3: Bump `LATEST_SCHEMA_VERSION`**

In `crates/right-db/src/migrations.rs:35`, change:

```rust
pub const LATEST_SCHEMA_VERSION: u32 = 38;
```

to:

```rust
pub const LATEST_SCHEMA_VERSION: u32 = 39;
```

- [ ] **Step 4: Add the registry entry**

In `crates/right-db/src/migrations.rs`, in the `pub static MIGRATIONS: Migrations` array, after the `version: 38` entry (the last one, ~line 923-925), add:

```rust
        Migration {
            version: 39,
            sql: V39_SCHEMA,
            hook: None,
        },
```

- [ ] **Step 5: Write the migration test**

In the `#[cfg(test)] mod tests` of `crates/right-db/src/migrations.rs`, add:

```rust
#[tokio::test]
async fn v39_creates_error_details_table_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    // First open runs migrations to LATEST.
    crate::open_connection(dir.path(), true).await.unwrap();
    // Second open must be a no-op (CREATE TABLE IF NOT EXISTS), not error.
    let conn = crate::open_connection(dir.path(), true).await.unwrap();

    let name: String = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='error_details'",
            (),
            |row| row.get::<_, String>(0),
        )
        .await
        .unwrap();
    assert_eq!(name, "error_details");

    let version: i64 = conn
        .query_row("PRAGMA user_version", (), |row| row.get(0))
        .await
        .unwrap();
    assert_eq!(version, i64::from(crate::migrations::LATEST_SCHEMA_VERSION));
}
```

- [ ] **Step 6: Run the test (expect PASS)**

Run: `devenv shell -- cargo test -p right-db v39_creates_error_details_table_and_is_idempotent -- --nocapture`
Expected: PASS. (Also confirms the existing `final user_version must equal LATEST_SCHEMA_VERSION` assertions still hold for 39.)

- [ ] **Step 7: Run the existing migration suite**

Run: `devenv shell -- cargo test -p right-db migration`
Expected: PASS — no regression from the version bump.

- [ ] **Step 8: Commit**

```bash
git add crates/right-db/src/sql/v39_error_details.sql crates/right-db/src/migrations.rs
git commit -m "feat(db): add error_details table (migration v39)"
```

---

## Task 2: Pure helpers — `parse_errdet_id`, `details_keyboard`, `details_payload`

**Files:**
- Create: `crates/bot/src/telegram/error_details.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Create the module with pure helpers**

Create `crates/bot/src/telegram/error_details.rs`:

```rust
//! Persisted raw-error details behind the `🔍 Details` button on prettified
//! CC-error messages. Write path: `invoke_cc` best-effort-stores the raw JSON.
//! Read path: the `errdet:<id>` callback handler looks it up (scoped by
//! chat_id) and replies with the JSON.

use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use crate::cc::markdown_utils::html_escape;

/// Days a stored error detail is retained before the next insert sweeps it.
pub(crate) const ERROR_DETAILS_TTL_DAYS: i64 = 7;
/// Telegram's per-message character ceiling. Above this we send a file.
const TELEGRAM_MESSAGE_LIMIT: usize = 4096;

/// Parse an `errdet:<id>` callback payload into the row id.
pub(crate) fn parse_errdet_id(data: &str) -> Option<i64> {
    data.strip_prefix("errdet:")?.parse::<i64>().ok()
}

/// Build the one-button keyboard. `None` → empty markup (no button rendered).
pub(crate) fn details_keyboard(details_id: Option<i64>) -> InlineKeyboardMarkup {
    match details_id {
        Some(id) => InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            "🔍 Details",
            format!("errdet:{id}"),
        )]]),
        None => InlineKeyboardMarkup::default(),
    }
}

/// How to deliver the raw JSON: inline `<pre>` HTML, or an attached file when
/// the escaped+wrapped body would exceed Telegram's message limit.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DetailsPayload {
    /// Ready-to-send HTML (already escaped + `<pre>`-wrapped).
    Inline(String),
    /// Raw JSON bytes for an `error.json` document.
    File(Vec<u8>),
}

/// Decide how to present `raw_json`. Char count (not bytes) approximates
/// Telegram's limit conservatively; oversize falls back to a file.
pub(crate) fn details_payload(raw_json: &str) -> DetailsPayload {
    let wrapped = format!("<pre>{}</pre>", html_escape(raw_json));
    if wrapped.chars().count() <= TELEGRAM_MESSAGE_LIMIT {
        DetailsPayload::Inline(wrapped)
    } else {
        DetailsPayload::File(raw_json.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_errdet_id_accepts_valid() {
        assert_eq!(parse_errdet_id("errdet:42"), Some(42));
    }

    #[test]
    fn parse_errdet_id_rejects_malformed() {
        assert_eq!(parse_errdet_id("errdet:"), None);
        assert_eq!(parse_errdet_id("errdet:abc"), None);
        assert_eq!(parse_errdet_id("model:42"), None);
        assert_eq!(parse_errdet_id("42"), None);
    }

    #[test]
    fn details_keyboard_some_has_one_button() {
        let kb = details_keyboard(Some(7));
        let buttons: Vec<_> = kb.inline_keyboard.iter().flatten().collect();
        assert_eq!(buttons.len(), 1);
        match &buttons[0].kind {
            teloxide::types::InlineKeyboardButtonKind::CallbackData(d) => {
                assert_eq!(d, "errdet:7");
            }
            other => panic!("unexpected button kind: {other:?}"),
        }
    }

    #[test]
    fn details_keyboard_none_is_empty() {
        assert!(details_keyboard(None).inline_keyboard.iter().all(|r| r.is_empty()));
    }

    #[test]
    fn details_payload_small_is_inline_and_escaped() {
        let payload = details_payload(r#"{"err":"<bad> & stuff"}"#);
        match payload {
            DetailsPayload::Inline(html) => {
                assert!(html.starts_with("<pre>") && html.ends_with("</pre>"));
                assert!(html.contains("&lt;bad&gt;"));
                assert!(html.contains("&amp;"));
            }
            DetailsPayload::File(_) => panic!("expected inline for small payload"),
        }
    }

    #[test]
    fn details_payload_large_is_file() {
        let big = "x".repeat(5000);
        assert!(matches!(details_payload(&big), DetailsPayload::File(_)));
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/bot/src/telegram/mod.rs`, add alongside the other `mod` declarations (e.g. near `mod model_command;`):

```rust
pub(crate) mod error_details;
```

- [ ] **Step 3: Run the tests (expect PASS)**

Run: `devenv shell -- cargo test -p right-bot error_details::tests`
Expected: PASS for all 6 pure-helper tests.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/error_details.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): error-details pure helpers (keyboard, payload, callback parse)"
```

---

## Task 3: DB helpers — `insert_error_detail`, `get_error_detail`

**Files:**
- Modify: `crates/bot/src/telegram/error_details.rs`

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` in `crates/bot/src/telegram/error_details.rs`:

```rust
    async fn migrated_conn() -> (tempfile::TempDir, right_db::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
        (dir, conn)
    }

    #[tokio::test]
    async fn insert_then_get_round_trips() {
        let (_dir, conn) = migrated_conn().await;
        let now = 1_700_000_000;
        let id = insert_error_detail(&conn, 111, 0, r#"{"is_error":true}"#, now)
            .await
            .unwrap();
        let got = get_error_detail(&conn, id, 111).await.unwrap();
        assert_eq!(got.as_deref(), Some(r#"{"is_error":true}"#));
    }

    #[tokio::test]
    async fn get_is_scoped_by_chat_id() {
        let (_dir, conn) = migrated_conn().await;
        let id = insert_error_detail(&conn, 111, 0, "{}", 1_700_000_000)
            .await
            .unwrap();
        // Wrong chat_id must not reveal the row.
        assert_eq!(get_error_detail(&conn, id, 222).await.unwrap(), None);
        // Unknown id → None.
        assert_eq!(get_error_detail(&conn, id + 999, 111).await.unwrap(), None);
    }

    #[tokio::test]
    async fn insert_sweeps_rows_older_than_ttl() {
        let (_dir, conn) = migrated_conn().await;
        let day = 86_400;
        let now = 1_700_000_000;
        // Old row: 8 days ago (TTL is 7).
        let old = insert_error_detail(&conn, 111, 0, "old", now - 8 * day)
            .await
            .unwrap();
        // Fresh insert at `now` sweeps anything older than now - 7d.
        let fresh = insert_error_detail(&conn, 111, 0, "fresh", now)
            .await
            .unwrap();
        assert_eq!(get_error_detail(&conn, old, 111).await.unwrap(), None);
        assert_eq!(get_error_detail(&conn, fresh, 111).await.unwrap().as_deref(), Some("fresh"));
    }
```

Add the `tokio` test attribute import if not already present in the test module: the file's tests above use plain `#[test]`; these use `#[tokio::test]`, which is available without extra imports in this crate.

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot error_details::tests::insert_then_get_round_trips`
Expected: FAIL — `insert_error_detail` / `get_error_detail` not defined.

- [ ] **Step 3: Implement the DB helpers**

In `crates/bot/src/telegram/error_details.rs`, add above the `#[cfg(test)]` module (and add the `use` at the top of the file):

```rust
use right_db::{Connection, DbError, OptionalExtension as _};
```

```rust
/// Store the raw error JSON and sweep rows older than the TTL. Insert + delete
/// are two writes → one immediate transaction. Returns the new row id.
///
/// `now` is unix seconds (caller-supplied for testability).
pub(crate) async fn insert_error_detail(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
    raw_json: &str,
    now: i64,
) -> Result<i64, DbError> {
    let cutoff = now - ERROR_DETAILS_TTL_DAYS * 86_400;
    let tx = conn.transaction().await?;
    tx.execute(
        "DELETE FROM error_details WHERE created_at < ?1",
        right_db::params![cutoff],
    )
    .await?;
    tx.execute(
        "INSERT INTO error_details (chat_id, thread_id, raw_json, created_at) \
         VALUES (?1, ?2, ?3, ?4)",
        right_db::params![chat_id, thread_id, raw_json, now],
    )
    .await?;
    let id = tx.connection().last_insert_rowid();
    tx.commit().await?;
    Ok(id)
}

/// Fetch a stored error detail, scoped to `chat_id`. Returns `None` when the
/// row is absent, expired (swept), or belongs to a different chat.
pub(crate) async fn get_error_detail(
    conn: &Connection,
    id: i64,
    chat_id: i64,
) -> Result<Option<String>, DbError> {
    conn.query_row(
        "SELECT raw_json FROM error_details WHERE id = ?1 AND chat_id = ?2",
        right_db::params![id, chat_id],
        |row| row.get::<_, String>(0),
    )
    .await
    .optional()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot error_details::tests`
Expected: PASS — all pure + DB helper tests.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/error_details.rs
git commit -m "feat(bot): error_details DB helpers with TTL sweep and chat scoping"
```

---

## Task 4: Write path — store raw JSON in `invoke_cc`, thread `details_id`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (enum variants ~2511-2517 region; failure block ~4066-4099)

- [ ] **Step 1: Add `details_id` to the two prettified variants**

In `crates/bot/src/telegram/worker.rs`, in `enum InvokeCcFailure`, add a field to `RateLimited`:

```rust
    RateLimited {
        message: String,
        thinking_msg_id: Option<teloxide::types::MessageId>,
        /// Row id in `error_details` for the `🔍 Details` button, if stored.
        details_id: Option<i64>,
    },
```

and to `Reflectable`:

```rust
    Reflectable {
        kind: FailureKind,
        ring_buffer_tail: VecDeque<crate::cc::stream::StreamEvent>,
        session_uuid: String,
        raw_message: String,
        thinking_msg_id: Option<teloxide::types::MessageId>,
        /// Row id in `error_details` for the `🔍 Details` button, if stored.
        details_id: Option<i64>,
    },
```

- [ ] **Step 2: Compute the raw detail and best-effort store, in the failure block**

In `invoke_cc`, in the block at `worker.rs:4066` (just before `if matches!(cc_class, CcResultClass::RateLimited)`), insert:

```rust
        // Persist the raw JSON we classified, for the "🔍 Details" button.
        // Best-effort: a store failure logs and yields no button — delivering
        // the user-facing error message is the primary obligation (mirrors the
        // logged-and-continued touch_session site below).
        let raw_details = if !stdout_str.trim().is_empty() {
            stdout_str.to_string()
        } else {
            stderr_str.to_string()
        };
        let details_id = if raw_details.trim().is_empty() {
            None
        } else {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            match super::error_details::insert_error_detail(
                &conn,
                chat_id,
                eff_thread_id,
                &raw_details,
                now,
            )
            .await
            {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::error!(?chat_id, "store error_details failed: {:#}", e);
                    None
                }
            }
        };
```

- [ ] **Step 3: Pass `details_id` into both `return Err(...)` sites**

In the same block, update the `RateLimited` return (worker.rs:4069-4074):

```rust
        if matches!(cc_class, CcResultClass::RateLimited) {
            return Err(InvokeCcFailure::RateLimited {
                message: RATE_LIMIT_MESSAGE.to_string(),
                thinking_msg_id,
                details_id,
            });
        }
```

and the `Reflectable` return (worker.rs:4093-4099):

```rust
        return Err(InvokeCcFailure::Reflectable {
            kind: FailureKind::NonZeroExit { code: exit_code },
            ring_buffer_tail: ring_buffer.events().clone(),
            session_uuid: session_uuid.clone(),
            raw_message: raw,
            thinking_msg_id,
            details_id,
        });
```

- [ ] **Step 4: Verify it compiles**

Run: `devenv shell -- cargo check -p right-bot`
Expected: FAIL — the worker loop's `match` arms (Task 5) don't yet bind `details_id`. This confirms the only remaining consumers are the send sites. (If other constructors of these variants exist outside `invoke_cc`, the error list names them — update those to pass `details_id: None`.)

- [ ] **Step 5: Commit after Task 5 compiles** (no standalone commit — Task 4 + Task 5 land together).

---

## Task 5: Render the keyboard at the worker send/edit sites

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (worker loop ~1797-1965; `send_error_to_telegram` ~4153)

- [ ] **Step 1: Add a markup-aware send helper**

In `crates/bot/src/telegram/worker.rs`, immediately after `send_error_to_telegram` (ends ~line 4185), add:

```rust
/// Like `send_error_to_telegram` but attaches an inline keyboard (e.g. the
/// "🔍 Details" button). Falls back to plain text (keyboard preserved) on HTML
/// send failure.
async fn send_error_to_telegram_with_markup(
    ctx: &WorkerContext,
    tg_chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    message: &str,
    reply_markup: teloxide::types::InlineKeyboardMarkup,
) {
    use teloxide::types::{MessageId, ThreadId};
    let mut send = ctx
        .bot
        .send_message(tg_chat_id, message)
        .parse_mode(teloxide::types::ParseMode::Html)
        .reply_markup(reply_markup.clone());
    if eff_thread_id != 0 {
        send = send.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }
    if let Err(e) = send.await {
        tracing::warn!(
            chat_id = ?tg_chat_id,
            eff_thread_id,
            "HTML error send failed, retrying plain text: {:#}",
            e
        );
        let plain = strip_html_tags(message);
        let mut fallback = ctx
            .bot
            .send_message(tg_chat_id, &plain)
            .reply_markup(reply_markup);
        if eff_thread_id != 0 {
            fallback = fallback.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
        }
        if let Err(e2) = fallback.await {
            tracing::error!(
                chat_id = ?tg_chat_id,
                eff_thread_id,
                "plain-text error send also failed: {:#}",
                e2
            );
        }
    }
}
```

(Confirm `strip_html_tags` is already imported/in scope in `worker.rs`; `send_error_to_telegram` uses it, so it is.)

- [ ] **Step 2: Wire the `RateLimited` arm**

In the worker loop, replace the `RateLimited` match arm (worker.rs:1797-1828). Bind `details_id` and use the keyboard on the edit + both sends:

```rust
                Err(InvokeCcFailure::RateLimited {
                    message,
                    thinking_msg_id,
                    details_id,
                }) => {
                    tracing::info!(
                        ?key,
                        "rate-limited turn — sending human notice, skipping reflection"
                    );
                    let keyboard = super::error_details::details_keyboard(details_id);
                    match thinking_msg_id {
                        Some(msg_id) => {
                            let edit_result = ctx
                                .bot
                                .edit_message_text(tg_chat_id, msg_id, &message)
                                .parse_mode(teloxide::types::ParseMode::Html)
                                .reply_markup(keyboard.clone())
                                .await;
                            if let Err(edit_err) = edit_result {
                                tracing::warn!(
                                    ?key,
                                    "rate-limit banner edit failed ({:#}); sending as new message",
                                    edit_err
                                );
                                let _ = ctx.bot.delete_message(tg_chat_id, msg_id).await;
                                send_error_to_telegram_with_markup(
                                    &ctx,
                                    tg_chat_id,
                                    eff_thread_id,
                                    &message,
                                    keyboard,
                                )
                                .await;
                            }
                        }
                        None => {
                            send_error_to_telegram_with_markup(
                                &ctx,
                                tg_chat_id,
                                eff_thread_id,
                                &message,
                                keyboard,
                            )
                            .await;
                        }
                    }
                }
```

- [ ] **Step 3: Bind `details_id` in the `Reflectable` arm and reuse it**

In the `Reflectable` match arm (worker.rs:1829-1835), add `details_id` to the destructured fields:

```rust
                Err(InvokeCcFailure::Reflectable {
                    kind,
                    ring_buffer_tail,
                    session_uuid: failed_session_uuid,
                    raw_message,
                    thinking_msg_id,
                    details_id,
                }) => {
```

- [ ] **Step 4: Attach the keyboard to the LAST reflection-success part**

In the reflection-success branch (worker.rs:1886-1928), the loop sends `parts`. Attach the keyboard only to the final part. Replace the `for part in &parts { ... }` send loop with an enumerated version that adds the markup on the last index:

```rust
                            let keyboard = super::error_details::details_keyboard(details_id);
                            let last_idx = parts.len().saturating_sub(1);
                            for (idx, part) in parts.iter().enumerate() {
                                let mut send = ctx.bot.send_message(tg_chat_id, part);
                                send = send.parse_mode(teloxide::types::ParseMode::Html);
                                if eff_thread_id != 0 {
                                    send = send.message_thread_id(ThreadId(MessageId(
                                        eff_thread_id as i32,
                                    )));
                                }
                                if let Some(ref_id) = reply_to {
                                    send = send.reply_parameters(ReplyParameters {
                                        message_id: MessageId(ref_id),
                                        ..Default::default()
                                    });
                                }
                                if idx == last_idx {
                                    send = send.reply_markup(keyboard.clone());
                                }
                                if let Err(e) = send.await {
                                    tracing::warn!(
                                        ?key,
                                        "reflection HTML send failed, retrying plain: {:#}",
                                        e
                                    );
                                    let plain = strip_html_tags(part);
                                    let mut fb = ctx.bot.send_message(tg_chat_id, &plain);
                                    if eff_thread_id != 0 {
                                        fb = fb.message_thread_id(ThreadId(MessageId(
                                            eff_thread_id as i32,
                                        )));
                                    }
                                    if let Some(ref_id) = reply_to {
                                        fb = fb.reply_parameters(ReplyParameters {
                                            message_id: MessageId(ref_id),
                                            ..Default::default()
                                        });
                                    }
                                    if idx == last_idx {
                                        fb = fb.reply_markup(keyboard.clone());
                                    }
                                    if let Err(e2) = fb.await {
                                        tracing::error!(
                                            ?key,
                                            "reflection plain-text fallback also failed: {:#}",
                                            e2
                                        );
                                    }
                                }
                            }
```

- [ ] **Step 5: Attach the keyboard on the reflection-FAILURE raw-message send**

In the reflection-failure branch (worker.rs:1930+), the `Some(msg_id)` path edits `raw_message` with an empty keyboard and falls back to `send_error_to_telegram`. Replace the empty keyboard and the fallback call so they carry the button. Concretely:

- Change `.reply_markup(teloxide::types::InlineKeyboardMarkup::default())` on the `edit_message_text(... &raw_message ...)` call to:

```rust
                                        .reply_markup(
                                            super::error_details::details_keyboard(details_id),
                                        )
```

- Change the fallback `send_error_to_telegram(&ctx, tg_chat_id, eff_thread_id, &raw_message)` (the one inside this reflection-failure `Some(msg_id)` block, ~worker.rs:1954) to:

```rust
                                        send_error_to_telegram_with_markup(
                                            &ctx,
                                            tg_chat_id,
                                            eff_thread_id,
                                            &raw_message,
                                            super::error_details::details_keyboard(details_id),
                                        )
                                        .await;
```

- For the `None` (no `thinking_msg_id`) sub-branch of reflection failure that sends `&raw_message` or `&text` via `send_error_to_telegram`, switch that specific call to `send_error_to_telegram_with_markup(..., super::error_details::details_keyboard(details_id))`. Leave any `send_error_to_telegram` calls that send a different message (not the raw error) unchanged.

> Note: `details_id` is `Copy` (`Option<i64>`), so reusing it across branches needs no clone.

- [ ] **Step 6: Verify the whole crate compiles**

Run: `devenv shell -- cargo check -p right-bot`
Expected: PASS. If a `match` non-exhaustiveness or unbound-field error remains, it points to a send site still using the old signature — fix per the patterns above.

- [ ] **Step 7: Run the worker/classify tests**

Run: `devenv shell -- cargo test -p right-bot worker::tests`
Expected: PASS — existing `classify_cc_result` / `format_human_error` tests still hold (no logic in them changed).

- [ ] **Step 8: Commit (Tasks 4 + 5 together)**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): store raw error JSON and render Details button on prettified errors"
```

---

## Task 6: Read path — callback handler + dispatcher branch

**Files:**
- Modify: `crates/bot/src/telegram/error_details.rs` (handler)
- Modify: `crates/bot/src/telegram/dispatch.rs` (branch)

- [ ] **Step 1: Add the callback handler**

In `crates/bot/src/telegram/error_details.rs`, add the imports at the top:

```rust
use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, InputFile, ReplyParameters};

use super::handler::AgentDir;
use super::BotType;
use crate::cc::markdown_utils::strip_html_tags;
```

and add the handler (above the `#[cfg(test)]` module):

```rust
/// Handle `errdet:<id>` callback queries: reply with the stored raw error JSON,
/// scoped to the clicking chat. Callback data format: `errdet:{row_id}`.
pub(crate) async fn handle_error_details_callback(
    bot: BotType,
    q: CallbackQuery,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let qid = q.id.clone();

    // Resolve id + chat from the callback. Missing message or bad id → alert.
    let id = q.data.as_deref().and_then(parse_errdet_id);
    let chat = q.message.as_ref().map(|m| m.chat().id);
    let reply_to_msg_id = q.message.as_ref().map(|m| m.id());

    let (Some(id), Some(chat), Some(reply_to_msg_id)) = (id, chat, reply_to_msg_id) else {
        bot.answer_callback_query(qid)
            .text("Details no longer available.")
            .show_alert(true)
            .await?;
        return Ok(());
    };

    // Open the per-agent DB (no migration on runtime opens) and fetch, scoped.
    let raw = match right_db::open_connection(&agent_dir.0, false).await {
        Ok(conn) => match get_error_detail(&conn, id, chat.0).await {
            Ok(found) => found,
            Err(e) => {
                tracing::error!(chat_id = chat.0, "get_error_detail failed: {:#}", e);
                None
            }
        },
        Err(e) => {
            tracing::error!(chat_id = chat.0, "open_connection failed: {:#}", e);
            None
        }
    };

    let Some(raw) = raw else {
        bot.answer_callback_query(qid)
            .text("Details no longer available.")
            .show_alert(true)
            .await?;
        return Ok(());
    };

    // Reply to the button's message (a reply auto-stays in the same topic).
    match details_payload(&raw) {
        DetailsPayload::Inline(html) => {
            let send = bot
                .send_message(chat, &html)
                .parse_mode(teloxide::types::ParseMode::Html)
                .reply_parameters(ReplyParameters {
                    message_id: reply_to_msg_id,
                    ..Default::default()
                });
            if let Err(e) = send.await {
                tracing::warn!(chat_id = chat.0, "details HTML send failed, plain: {:#}", e);
                let _ = bot
                    .send_message(chat, strip_html_tags(&html))
                    .reply_parameters(ReplyParameters {
                        message_id: reply_to_msg_id,
                        ..Default::default()
                    })
                    .await;
            }
        }
        DetailsPayload::File(bytes) => {
            let file = InputFile::memory(bytes).file_name("error.json");
            if let Err(e) = bot
                .send_document(chat, file)
                .reply_parameters(ReplyParameters {
                    message_id: reply_to_msg_id,
                    ..Default::default()
                })
                .await
            {
                tracing::warn!(chat_id = chat.0, "details document send failed: {:#}", e);
            }
        }
    }

    bot.answer_callback_query(qid).await?;
    Ok(())
}
```

> If `super::BotType` / `ResponseResult` are not re-exported from the `telegram` module root, match the import other handlers use — `handle_bg_callback` in `handler.rs` resolves `BotType` and `ResponseResult` in scope; replicate its `use` lines (likely `use super::BotType;` and teloxide's `ResponseResult` via `teloxide::prelude::*`).

- [ ] **Step 2: Add the dispatcher branch**

In `crates/bot/src/telegram/dispatch.rs`, in the `callback_handler` chain (after the `bg:` branch, before `.endpoint(handle_stop_callback)` at line 612), add:

```rust
        .branch(
            dptree::filter(|q: CallbackQuery| {
                q.data.as_deref().is_some_and(|d| d.starts_with("errdet:"))
            })
            .endpoint(super::error_details::handle_error_details_callback),
        )
```

(`agent_dir_arc` is already in the dispatcher `dependencies(...)` at line 621, so the handler's `Arc<AgentDir>` dependency resolves with no change.)

- [ ] **Step 3: Verify compilation**

Run: `devenv shell -- cargo check -p right-bot`
Expected: PASS. Fix any import-path mismatch flagged for `BotType` / `ResponseResult` per the note in Step 1.

- [ ] **Step 4: Run the error_details tests again**

Run: `devenv shell -- cargo test -p right-bot error_details`
Expected: PASS — pure + DB helper tests unaffected.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/error_details.rs crates/bot/src/telegram/dispatch.rs
git commit -m "feat(bot): errdet callback handler + dispatcher branch for Details button"
```

---

## Task 7: Final verification

- [ ] **Step 1: Clippy on the touched crates**

Run: `devenv shell -- cargo clippy -p right-bot -p right-db -- -D warnings`
Expected: PASS (no warnings).

- [ ] **Step 2: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Note: two tests are known-flaky under parallel load (cc/invocation pid race, dashboard warn-count) — if either fails, re-run it isolated before attributing to this change.

- [ ] **Step 3: Manual smoke (optional, requires a live agent)**

Trigger a rate-limit/overload or a CC non-zero exit; confirm the friendly message carries a `🔍 Details` button, pressing it replies with the raw JSON in `<pre>`, an oversized payload arrives as `error.json`, and pressing after the 7-day sweep (or from a different chat) shows the `Details no longer available.` alert.

- [ ] **Step 4: Final commit (if any cleanup remained)**

```bash
git add -A
git commit -m "test(bot): final verification for error-details button"
```

---

## Self-Review notes

- **Spec coverage:** storage table (Task 1), TTL=7d + sweep (Task 3), write/best-effort/threading (Task 4), button on all prettified surfaces — RateLimited + Reflectable success + Reflectable failure (Task 5), callback read path with chat scoping + pre/file fallback + unavailable alert (Tasks 2, 6). Copy `🔍 Details` / `Details no longer available.` (Tasks 2, 6).
- **Excluded by design:** `NonReflectable` parse-failure already shows raw stdout inline — no button (stated in Scope note).
- **Type consistency:** `details_id: Option<i64>` is used identically in the enum (Task 4), `details_keyboard` (Task 2), and both worker arms (Task 5). `parse_errdet_id`/`get_error_detail`/`details_payload`/`DetailsPayload` names match between definition (Tasks 2-3) and use (Task 6).
