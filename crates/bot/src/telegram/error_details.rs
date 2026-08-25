//! Persisted raw-error details behind the `🔍 Details` button on prettified
//! CC-error messages. Write path: `invoke_cc` best-effort-stores the raw JSON.
//! Read path: the `errdet:<id>` callback handler looks it up (scoped by
//! chat_id) and replies with the JSON.

use frankenstein::types::{
    CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, MaybeInaccessibleMessage,
};

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
        Some(id) => vec![vec![
            InlineKeyboardButton::builder()
                .text("🔍 Details")
                .callback_data(format!("errdet:{id}"))
                .build(),
        ]],
        None => Vec::new(),
    };
    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
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

/// Store raw error JSON and sweep expired rows atomically in the owner.
pub(crate) async fn insert_error_detail(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    chat_id: i64,
    thread_id: i64,
    raw_json: &str,
    now: i64,
) -> Result<i64, right_mcp::internal_db::InternalDbError> {
    client
        .error_detail_insert(&right_mcp::internal_db::ErrorDetailInsertRequest {
            agent: agent.to_owned(),
            request_id: crate::db::request_id(),
            chat_id,
            thread_id,
            raw_json: raw_json.to_owned(),
            created_at_unix: now,
        })
        .await
        .map(|response| response.id)
}

/// Handle `errdet:<id>` callback queries: reply with the stored raw error JSON,
/// scoped to the clicking chat. Callback data format: `errdet:{row_id}`.
pub(crate) async fn handle_error_details_callback(
    ctx: &HandlerCtx,
    q: &CallbackQuery,
) -> Result<(), TgError> {
    let bot = &ctx.bot;
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

    let agent_name = ctx
        .agent_dir
        .0
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    let raw = match ctx
        .internal_api
        .0
        .error_detail_get(&right_mcp::internal_db::ErrorDetailGetRequest {
            agent: agent_name,
            id,
            chat_id: chat,
        })
        .await
    {
        Ok(response) => response.raw_json,
        Err(error) => {
            tracing::error!(
                chat_id = chat,
                "get_error_detail owner read failed: {error:#}"
            );
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
                .send_document_bytes(
                    chat,
                    &bytes,
                    "error.json",
                    None,
                    false,
                    None,
                    Some(reply_to_msg_id),
                )
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
}
