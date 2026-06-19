//! Persisted raw-error details behind the `🔍 Details` button on prettified
//! CC-error messages. Write path: `invoke_cc` best-effort-stores the raw JSON.
//! Read path: the `errdet:<id>` callback handler looks it up (scoped by
//! chat_id) and replies with the JSON.

use frankenstein::types::{
    CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, MaybeInaccessibleMessage,
};
use right_db::{Connection, DbError, OptionalExtension as _};

use super::router::HandlerCtx;
use super::tg_bot::TgError;
use crate::cc::markdown_utils::{html_escape, strip_html_tags};

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
    let rows = match details_id {
        Some(id) => vec![vec![InlineKeyboardButton::builder()
            .text("🔍 Details")
            .callback_data(format!("errdet:{id}"))
            .build()]],
        None => Vec::new(),
    };
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
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

/// Decide how to present `raw_json`. Measured in UTF-16 code units to match
/// Telegram's message-length limit exactly (`char` count would undercount
/// astral-plane characters); oversize falls back to a file.
pub(crate) fn details_payload(raw_json: &str) -> DetailsPayload {
    let wrapped = format!("<pre>{}</pre>", html_escape(raw_json));
    if wrapped.encode_utf16().count() <= TELEGRAM_MESSAGE_LIMIT {
        DetailsPayload::Inline(wrapped)
    } else {
        DetailsPayload::File(raw_json.as_bytes().to_vec())
    }
}

/// Store the raw error JSON and sweep rows older than the TTL. Insert + delete
/// are two writes → one immediate transaction. Returns the new row id.
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

/// Handle `errdet:<id>` callback queries: reply with the stored raw error JSON,
/// scoped to the clicking chat. Callback data format: `errdet:{row_id}`.
pub(crate) async fn handle_error_details_callback(
    ctx: &HandlerCtx,
    q: &CallbackQuery,
) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let agent_dir = &ctx.agent_dir;

    // Resolve id + chat from the callback. Missing message or bad id → alert.
    let id = q.data.as_deref().and_then(parse_errdet_id);
    // The callback message carries the chat + the message to reply to. An
    // inaccessible message (too old) still has chat + id, so both variants work.
    let (chat, reply_to_msg_id) = match q.message.as_ref() {
        Some(MaybeInaccessibleMessage::Message(m)) => (Some(m.chat.id), Some(m.message_id)),
        Some(MaybeInaccessibleMessage::InaccessibleMessage(m)) => {
            (Some(m.chat.id), Some(m.message_id))
        }
        None => (None, None),
    };

    let (Some(id), Some(chat), Some(reply_to_msg_id)) = (id, chat, reply_to_msg_id) else {
        bot.answer_callback(&q.id, Some("Details no longer available."), true)
            .await?;
        return Ok(());
    };

    // Open the per-agent DB (no migration on runtime opens) and fetch, scoped.
    let raw = match right_db::open_connection(&agent_dir.0, false).await {
        Ok(conn) => match get_error_detail(&conn, id, chat).await {
            Ok(found) => found,
            Err(e) => {
                tracing::error!(chat_id = chat, "get_error_detail failed: {:#}", e);
                None
            }
        },
        Err(e) => {
            tracing::error!(chat_id = chat, "open_connection failed: {:#}", e);
            None
        }
    };

    let Some(raw) = raw else {
        bot.answer_callback(&q.id, Some("Details no longer available."), true)
            .await?;
        return Ok(());
    };

    // Reply to the button's message (a reply auto-stays in the same topic).
    match details_payload(&raw) {
        DetailsPayload::Inline(html) => {
            if let Err(e) = bot
                .send_message_opts(chat, &html, true, None, Some(reply_to_msg_id), None)
                .await
            {
                tracing::warn!(chat_id = chat, "details HTML send failed, plain: {:#}", e);
                let _ = bot
                    .send_message_opts(
                        chat,
                        &strip_html_tags(&html),
                        false,
                        None,
                        Some(reply_to_msg_id),
                        None,
                    )
                    .await;
            }
        }
        DetailsPayload::File(bytes) => {
            if let Err(e) = bot
                .send_document_bytes(chat, &bytes, "error.json", None, None, Some(reply_to_msg_id))
                .await
            {
                tracing::warn!(chat_id = chat, "details document send failed: {:#}", e);
            }
        }
    }

    bot.answer_callback(&q.id, None, false).await?;
    Ok(())
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
        assert_eq!(buttons[0].callback_data.as_deref(), Some("errdet:7"));
    }

    #[test]
    fn details_keyboard_none_is_empty() {
        assert!(
            details_keyboard(None)
                .inline_keyboard
                .iter()
                .all(|r| r.is_empty())
        );
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
        assert_eq!(get_error_detail(&conn, id, 222).await.unwrap(), None);
        assert_eq!(get_error_detail(&conn, id + 999, 111).await.unwrap(), None);
    }

    #[tokio::test]
    async fn insert_sweeps_rows_older_than_ttl() {
        let (_dir, conn) = migrated_conn().await;
        let day = 86_400;
        let now = 1_700_000_000;
        let old = insert_error_detail(&conn, 111, 0, "old", now - 8 * day)
            .await
            .unwrap();
        let fresh = insert_error_detail(&conn, 111, 0, "fresh", now)
            .await
            .unwrap();
        assert_eq!(get_error_detail(&conn, old, 111).await.unwrap(), None);
        assert_eq!(
            get_error_detail(&conn, fresh, 111)
                .await
                .unwrap()
                .as_deref(),
            Some("fresh")
        );
    }
}
