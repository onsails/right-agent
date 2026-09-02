use frankenstein::rich_message::{
    InputRichBlock, InputRichBlockBlockQuotation, InputRichBlockList, InputRichBlockListItem,
    InputRichBlockParagraph, InputRichBlockPreformatted, InputRichBlockSectionHeading,
    InputRichBlockTable, InputRichMessage, RichBlockTableCell, RichText, RichTextBold,
    RichTextCode, RichTextItalic, RichTextStrikethrough, RichTextUrl,
};
use frankenstein::types::{InlineKeyboardMarkup, Message};
use right_rich_content::{
    Block, BlockRef, MAX_PLAIN_MESSAGE_UTF16, Mark, RichContent, RichContentRef, Run,
};

use super::tg_bot::{RightBot, TgError};

pub(crate) fn to_telegram(content: &RichContent) -> InputRichMessage {
    InputRichMessage::builder()
        .blocks(match content.as_ref() {
            RichContentRef::Text(text) => vec![paragraph_text(text)],
            RichContentRef::Blocks(blocks) => blocks.iter().map(map_block).collect(),
        })
        .skip_entity_detection(true)
        .build()
}

/// Terminal outcome of a multi-part rich send.
///
/// Delivery fans a single `RichContent` out to one or more Telegram messages
/// (plus plain-text fallback chunks when a part is deterministically rejected).
/// A failure of one part never discards the parts already published, so the
/// outcome carries them alongside the first terminal error. Callers use
/// `delivered` to archive what the user actually saw and `error` to report
/// partial publication — retrying blindly would duplicate the prefix.
#[derive(Debug)]
pub(crate) struct RichSendOutcome {
    /// Every message Telegram accepted, in delivery order.
    pub(crate) delivered: Vec<Message>,
    /// Normalized text of exactly what was published, joined the way delivery
    /// split it. Tracked from the accepted sends (rich part or plain-fallback
    /// chunk), so it matches what the user saw even after a fallback.
    pub(crate) delivered_text: String,
    /// The first terminal (non-retryable) error, if any part failed. Delivery
    /// of the remaining parts is still attempted, so later parts may have
    /// succeeded after this error occurred.
    pub(crate) error: Option<TgError>,
}

impl RichSendOutcome {
    pub(crate) fn is_complete(&self) -> bool {
        self.error.is_none()
    }

    /// Telegram id of the last message that was published, if any — the id
    /// callers archive against.
    pub(crate) fn last_message_id(&self) -> Option<i32> {
        self.delivered.last().map(|message| message.message_id)
    }

    /// Display form of the terminal error for logs and error bodies; a
    /// non-complete outcome always carries one, so the fallback only guards
    /// the type.
    pub(crate) fn error_display(&self) -> std::borrow::Cow<'static, str> {
        self.error
            .as_ref()
            .map(|error| std::borrow::Cow::Owned(error.to_string()))
            .unwrap_or_else(|| std::borrow::Cow::Borrowed("unknown telegram error"))
    }
}

/// Send every rich-message part, falling back only for deterministic content
/// rejection. Reply semantics apply to every part; markup is attached only to
/// the final delivered part.
///
/// On a non-retryable part failure the remaining parts are still attempted
/// best-effort and everything already published is returned alongside the
/// error: see [`RichSendOutcome`].
pub(crate) async fn send(
    bot: &RightBot,
    chat_id: i64,
    content: &RichContent,
    thread: Option<i32>,
    reply_to: Option<i32>,
    markup: Option<InlineKeyboardMarkup>,
) -> RichSendOutcome {
    let parts = content.delivery_parts();
    let last_part = parts.len().saturating_sub(1);
    let mut delivered = Vec::with_capacity(parts.len());
    let mut delivered_text = String::new();
    let mut terminal: Option<TgError> = None;

    for (index, part) in parts.iter().enumerate() {
        let part_markup = (index == last_part).then(|| markup.clone()).flatten();
        let attempt = bot
            .send_rich_content(
                chat_id,
                to_telegram(part),
                thread,
                reply_to,
                part_markup.clone(),
            )
            .await;
        match attempt {
            Ok(message) => {
                append_delivered_text(&mut delivered_text, &part.normalized_text());
                delivered.push(message);
            }
            Err(error) if is_retryable_rich_content_error(&error) => {
                tracing::warn!(
                    chat_id,
                    "rich content rejected, retrying normalized plain text: {error}"
                );
                let chunks = split_plain(&part.normalized_text());
                let last_chunk = chunks.len().saturating_sub(1);
                for (chunk_index, chunk) in chunks.iter().enumerate() {
                    let chunk_markup = (index == last_part && chunk_index == last_chunk)
                        .then(|| markup.clone())
                        .flatten();
                    match bot
                        .send_message_opts(chat_id, chunk, false, thread, reply_to, chunk_markup)
                        .await
                    {
                        Ok(message) => {
                            append_delivered_text(&mut delivered_text, chunk);
                            delivered.push(message);
                        }
                        // A plain chunk that fails deterministically is not
                        // retried again: record it and keep delivering the
                        // remaining parts.
                        Err(error) => record_terminal(&mut terminal, error, &delivered),
                    }
                }
            }
            Err(error) => record_terminal(&mut terminal, error, &delivered),
        }
    }

    RichSendOutcome {
        delivered,
        delivered_text,
        error: terminal,
    }
}

/// Channel publication variant: stop after the first terminal failure.
pub(crate) async fn send_until_failure(
    bot: &RightBot,
    chat_id: i64,
    content: &RichContent,
    thread: Option<i32>,
) -> RichSendOutcome {
    let parts = content.delivery_parts();
    let mut delivered = Vec::with_capacity(parts.len());
    let mut delivered_text = String::new();

    for part in &parts {
        match bot
            .send_rich_content_once(chat_id, to_telegram(part), thread)
            .await
        {
            Ok(message) => {
                append_delivered_text(&mut delivered_text, &part.normalized_text());
                delivered.push(message);
            }
            Err(error) if is_retryable_rich_content_error(&error) => {
                tracing::warn!(
                    chat_id,
                    "rich content rejected, retrying normalized plain text: {error}"
                );
                for chunk in split_plain(&part.normalized_text()) {
                    match bot.send_message_once(chat_id, &chunk, thread).await {
                        Ok(message) => {
                            append_delivered_text(&mut delivered_text, &chunk);
                            delivered.push(message);
                        }
                        Err(error) => {
                            return RichSendOutcome {
                                delivered,
                                delivered_text,
                                error: Some(error),
                            };
                        }
                    }
                }
            }
            Err(error) => {
                return RichSendOutcome {
                    delivered,
                    delivered_text,
                    error: Some(error),
                };
            }
        }
    }

    RichSendOutcome {
        delivered,
        delivered_text,
        error: None,
    }
}

/// Append one delivered fragment, restoring the `\n\n` separator delivery
/// removed when it split the content.
fn append_delivered_text(delivered_text: &mut String, fragment: &str) {
    if !delivered_text.is_empty() {
        delivered_text.push_str("\n\n");
    }
    delivered_text.push_str(fragment);
}

/// Keep the first terminal error and log the delivered prefix length; later
/// failures are logged but do not overwrite the first, so the reported error
/// always names the part that broke the stream.
fn record_terminal(terminal: &mut Option<TgError>, error: TgError, delivered: &[Message]) {
    if terminal.is_none() {
        tracing::warn!(
            delivered_messages = delivered.len(),
            "rich delivery part failed; continuing remaining parts best-effort: {error}"
        );
        *terminal = Some(error);
    } else {
        tracing::warn!(
            delivered_messages = delivered.len(),
            "rich delivery part failed after an earlier failure: {error}"
        );
    }
}

/// Split a part's normalized text into Telegram plain-message chunks measured
/// in UTF-16 code units (Telegram's unit), cutting only at Unicode scalar
/// boundaries so an astral surrogate pair is never divided.
///
/// Delegates to [`right_rich_content::split_visible_utf16`]: a cut that would
/// strand an interior whitespace run must not produce a whitespace-only chunk,
/// because Telegram rejects those as empty text — a 400 this fallback's own
/// retry predicate does not match, turning a recoverable degradation into a
/// spurious terminal error.
fn split_plain(text: &str) -> Vec<String> {
    right_rich_content::split_visible_utf16(text, MAX_PLAIN_MESSAGE_UTF16)
}

fn is_retryable_rich_content_error(error: &TgError) -> bool {
    let TgError::Api(frankenstein::Error::Api(response)) = error else {
        return false;
    };
    if response.error_code != 400u64 {
        return false;
    }
    let description = response.description.to_ascii_lowercase();
    [
        "unsupported rich block",
        "rich message is too long",
        "rich text is too long",
        "can't parse rich",
        "cannot parse rich",
        "invalid rich",
        "rich message format",
    ]
    .iter()
    .any(|known| description.contains(known))
}

fn map_block(block: &Block) -> InputRichBlock {
    match block.as_ref() {
        BlockRef::Paragraph { runs } => paragraph(runs),
        BlockRef::Heading { level, runs } => {
            InputRichBlock::Heading(InputRichBlockSectionHeading {
                text: map_runs(runs),
                size: level,
            })
        }
        BlockRef::List { ordered, items } => InputRichBlock::List(InputRichBlockList {
            items: items
                .iter()
                .enumerate()
                .map(|(index, item)| InputRichBlockListItem {
                    blocks: vec![paragraph(item.runs())],
                    has_checkbox: None,
                    is_checked: None,
                    value: ordered.then_some((index + 1) as i32),
                    type_field: ordered.then(|| "1".to_owned()),
                })
                .collect(),
        }),
        BlockRef::Quote { runs } => InputRichBlock::Blockquote(InputRichBlockBlockQuotation {
            blocks: vec![paragraph(runs)],
            credit: None,
        }),
        BlockRef::Code { text, language } => InputRichBlock::Pre(InputRichBlockPreformatted {
            text: RichText::Text(text.to_owned()),
            language: language.map(str::to_owned),
        }),
        BlockRef::Table { rows } => InputRichBlock::Table(InputRichBlockTable {
            cells: rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| RichBlockTableCell {
                            text: (!cell.runs().is_empty()).then(|| map_runs(cell.runs())),
                            is_header: None,
                            colspan: None,
                            rowspan: None,
                            align: "left".to_owned(),
                            valign: "top".to_owned(),
                        })
                        .collect()
                })
                .collect(),
            is_bordered: None,
            is_striped: None,
            is_compact: None,
            caption: None,
        }),
    }
}

fn paragraph_text(text: &str) -> InputRichBlock {
    InputRichBlock::Paragraph(InputRichBlockParagraph {
        text: RichText::Text(text.to_owned()),
    })
}

fn paragraph(runs: &[Run]) -> InputRichBlock {
    InputRichBlock::Paragraph(InputRichBlockParagraph {
        text: map_runs(runs),
    })
}

fn map_runs(runs: &[Run]) -> RichText {
    let mapped: Vec<_> = runs.iter().map(map_run).collect();
    if mapped.len() == 1 {
        mapped.into_iter().next().expect("one rich text run")
    } else {
        RichText::List(mapped)
    }
}

fn map_run(run: &Run) -> RichText {
    let mut text = RichText::Text(run.text().to_owned());
    for mark in run.marks().unwrap_or_default() {
        text = match mark {
            Mark::Bold => RichTextBold {
                text: Box::new(text),
            }
            .into(),
            Mark::Italic => RichTextItalic {
                text: Box::new(text),
            }
            .into(),
            Mark::Strikethrough => RichTextStrikethrough {
                text: Box::new(text),
            }
            .into(),
            Mark::Code => RichTextCode {
                text: Box::new(text),
            }
            .into(),
        };
    }
    if let Some(url) = run.link() {
        text = RichTextUrl {
            text: Box::new(text),
            url: url.to_owned(),
        }
        .into();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_error(code: u64, description: &str) -> TgError {
        TgError::Api(frankenstein::Error::Api(
            frankenstein::response::ErrorResponse {
                ok: false,
                error_code: code,
                description: description.to_owned(),
                parameters: None,
            },
        ))
    }

    #[test]
    fn maps_ordered_lists_tables_and_inline_wrappers() {
        let content: RichContent = serde_json::from_str(r#"{"blocks":[{"type":"list","ordered":true,"items":[{"runs":[{"text":"one"}]}]},{"type":"table","rows":[[{"runs":[]}]]},{"type":"paragraph","runs":[{"text":"site","marks":["bold"],"link":"https://example.com"}]}]}"#).unwrap();
        let value = serde_json::to_value(to_telegram(&content)).unwrap();
        assert_eq!(value["blocks"][0]["items"][0]["type"], "1");
        assert_eq!(value["blocks"][0]["items"][0]["value"], 1);
        assert_eq!(value["blocks"][1]["cells"][0][0]["align"], "left");
        assert_eq!(value["blocks"][1]["cells"][0][0]["valign"], "top");
        assert!(value["blocks"][1]["cells"][0][0].get("text").is_none());
        assert_eq!(value["blocks"][2]["text"]["type"], "url");
        assert_eq!(value["blocks"][2]["text"]["text"]["type"], "bold");
    }

    #[test]
    fn retries_only_known_content_bad_requests() {
        assert!(is_retryable_rich_content_error(&api_error(
            400,
            "Bad Request: unsupported rich block"
        )));
        assert!(is_retryable_rich_content_error(&api_error(
            400,
            "Bad Request: can't parse rich message"
        )));
        assert!(!is_retryable_rich_content_error(&api_error(
            400,
            "Bad Request: reply message not found"
        )));
        assert!(!is_retryable_rich_content_error(&api_error(
            429,
            "Too Many Requests"
        )));
        assert!(!is_retryable_rich_content_error(&api_error(
            500,
            "Internal Server Error"
        )));
        assert!(!is_retryable_rich_content_error(&TgError::Timeout(
            std::time::Duration::from_secs(1)
        )));
    }

    #[test]
    fn plain_fallback_chunks_astral_text_at_utf16_limit() {
        // 2,049 astral emoji = 4,098 UTF-16 units: one over the plain limit,
        // so the splitter must cut after exactly 4,096 units without ever
        // splitting a surrogate pair.
        let text = "🦀".repeat(MAX_PLAIN_MESSAGE_UTF16 / 2 + 1);
        let chunks = split_plain(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].encode_utf16().count(), MAX_PLAIN_MESSAGE_UTF16);
        assert_eq!(chunks[1], "🦀");
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn plain_fallback_never_exceeds_utf16_limit_for_astral_text() {
        // A scalar-counted budget of 4,096 would emit 8,192-unit chunks for
        // astral text, which Telegram rejects; the UTF-16 budget must hold.
        let text = "🎉".repeat(MAX_PLAIN_MESSAGE_UTF16);
        for chunk in split_plain(&text) {
            assert!(
                chunk.encode_utf16().count() <= MAX_PLAIN_MESSAGE_UTF16,
                "chunk exceeds the UTF-16 plain limit"
            );
            assert!(chunk.chars().all(|character| character == '🎉'));
        }
    }
}
