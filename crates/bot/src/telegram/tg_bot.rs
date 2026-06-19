//! Frankenstein Telegram client wrapper.
//!
//! This is the **only** module that imports `frankenstein::client_reqwest`.
//! It centralizes everything we want uniform across every outbound Telegram
//! call:
//!
//! - A single [`TgError`] type with a stable `Display` chain.
//! - Outbound rate limiting via [`Throttle`] (global + per-chat), approximating
//!   the limits teloxide's `Throttle` adaptor used to enforce.
//! - A cached `get_me` identity resolved once at [`RightBot::connect`] time,
//!   replacing teloxide's `CacheMe` adaptor.
//! - Uniform defaults: HTML parse-mode for our message/edit helpers, optional
//!   thread-id threading, single 429 retry honoring `retry_after`.
//!
//! Sibling `telegram::*` modules call into [`RightBot`] rather than touching
//! `frankenstein` directly.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use thiserror::Error;
use tokio::time::Instant;

use frankenstein::AsyncTelegramApi;
use frankenstein::ParseMode;
use frankenstein::client_reqwest::Bot as FBot;
use frankenstein::input_file::FileUpload;
use frankenstein::input_media::MediaGroupInputMedia;
use frankenstein::types::{
    AllowedUpdate, BotCommand, BotCommandScope, ChatAction, InlineKeyboardMarkup, Message,
    ReplyMarkup, ReplyParameters, User,
};

/// Default global send rate (messages/second) across all chats. Matches the
/// teloxide `Limits::default()` global cap we relied on previously.
const DEFAULT_GLOBAL_PER_SEC: u32 = 30;

/// Default minimum spacing between two sends to the *same* chat. Matches the
/// teloxide per-chat cap (~1 message/second/chat).
const DEFAULT_PER_CHAT_INTERVAL: Duration = Duration::from_millis(1000);

/// Error type for every outbound Telegram operation routed through [`RightBot`].
///
/// Both `frankenstein::Error` and `reqwest::Error` already implement `Display`
/// with their own source chains, so the plain `{0}` formatting is sufficient
/// here — the `{:#}` alternate-Display rule in `AGENTS.rust.md` is specifically
/// for flattening `anyhow::Error` context chains, which neither of these is.
#[derive(Debug, Error)]
pub(crate) enum TgError {
    #[error("telegram api: {0}")]
    Api(#[from] frankenstein::Error),
    #[error("file download: {0}")]
    Download(#[from] reqwest::Error),
    #[error("{0}")]
    Other(String),
}

/// Global + per-chat outbound rate gate, approximating teloxide's `Limits`.
///
/// The per-chat gate is a last-send timestamp map (keyed `tokio::time::Instant`)
/// enforcing a minimum interval per chat; because it uses the tokio clock, it
/// advances correctly under the tokio test clock (`start_paused = true`). The
/// global limiter is a [`governor`] direct rate limiter on `DefaultClock`
/// (governor's real monotonic wall-clock), so its `until_ready().await` sleeps
/// in real time regardless of `start_paused` — it is therefore not exercised by
/// the paused-clock unit tests (their volumes stay under the global cap, so it
/// returns immediately).
pub(crate) struct Throttle {
    global: RateLimiter<NotKeyed, InMemoryState, DefaultClock>,
    per_chat_interval: Duration,
    last_per_chat: DashMap<i64, Instant>,
}

impl Throttle {
    pub(crate) fn new(global_per_sec: u32, per_chat_interval: Duration) -> Self {
        let quota =
            Quota::per_second(NonZeroU32::new(global_per_sec).expect("global_per_sec must be > 0"));
        Self {
            global: RateLimiter::direct(quota),
            per_chat_interval,
            last_per_chat: DashMap::new(),
        }
    }

    /// Block until it is permissible to send to `chat_id`: first wait out the
    /// per-chat interval, then acquire a global token.
    ///
    /// Best-effort under concurrency: two concurrent `acquire` calls for the
    /// same chat read the timestamp before either writes it, so they may both
    /// proceed and briefly exceed the per-chat interval. The global governor
    /// limiter remains the hard cap.
    pub(crate) async fn acquire(&self, chat_id: i64) {
        loop {
            let now = Instant::now();
            let wait = match self.last_per_chat.get(&chat_id) {
                Some(prev) => self
                    .per_chat_interval
                    .checked_sub(now.duration_since(*prev)),
                None => None,
            };
            match wait {
                Some(d) if !d.is_zero() => tokio::time::sleep(d).await,
                _ => break,
            }
        }
        self.last_per_chat.insert(chat_id, Instant::now());
        self.global.until_ready().await;
    }
}

/// Extract the 429 `retry_after` seconds from a frankenstein error, if present.
///
/// Returns `Some(secs)` only for `Error::Api` responses carrying
/// `parameters.retry_after`; any other error variant (transport, decode, …)
/// returns `None`.
fn retry_after_secs(e: &frankenstein::Error) -> Option<u64> {
    if let frankenstein::Error::Api(resp) = e {
        resp.parameters.and_then(|p| p.retry_after).map(u64::from)
    } else {
        None
    }
}

/// Run `call`, and on a 429 `retry_after` error sleep then retry exactly once.
///
/// `call` is a closure (not a future) because a single future cannot be awaited
/// twice. The retry intentionally does NOT re-acquire the per-chat/global rate
/// gate — it already sleeps the server-supplied `retry_after`, which is the
/// authoritative backoff.
async fn with_retry<F, Fut, T>(call: F) -> Result<T, TgError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, frankenstein::Error>>,
{
    match call().await {
        Ok(v) => Ok(v),
        Err(e) => {
            if let Some(after) = retry_after_secs(&e) {
                tracing::warn!(retry_after = after, "telegram 429 — retrying once");
                tokio::time::sleep(Duration::from_secs(after)).await;
                Ok(call().await?)
            } else {
                Err(e.into())
            }
        }
    }
}

/// Thin, rate-limited, identity-cached wrapper over the frankenstein reqwest
/// client. Cloneable: clones share the same `reqwest::Client`, cached identity,
/// and throttle.
#[derive(Clone)]
pub(crate) struct RightBot {
    bot: FBot,
    token: Arc<String>,
    me: Arc<User>,
    rate: Arc<Throttle>,
}

impl RightBot {
    /// Construct and resolve identity (`get_me`) once — replaces teloxide
    /// `CacheMe`. Performs one live network round-trip; do not call in unit
    /// tests.
    pub(crate) async fn connect(token: String) -> Result<Self, TgError> {
        let bot = FBot::new(&token);
        let me = bot.get_me().await?.result;
        Ok(Self {
            bot,
            token: Arc::new(token),
            me: Arc::new(me),
            rate: Arc::new(Throttle::new(
                DEFAULT_GLOBAL_PER_SEC,
                DEFAULT_PER_CHAT_INTERVAL,
            )),
        })
    }

    /// The bot's own resolved identity (cached at `connect` time).
    pub(crate) fn me(&self) -> &User {
        &self.me
    }

    // ---- core sends -------------------------------------------------------

    /// Send an HTML-formatted message, optionally threaded, replying, and with
    /// an inline keyboard. Returns the sent [`Message`] (callers read
    /// `message_id`).
    pub(crate) async fn send_html(
        &self,
        chat_id: i64,
        thread: Option<i32>,
        text: &str,
        reply_to: Option<i32>,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::Html)
            .maybe_message_thread_id(thread)
            .maybe_reply_parameters(
                reply_to.map(|r| ReplyParameters::builder().message_id(r).build()),
            )
            .maybe_reply_markup(markup.map(ReplyMarkup::InlineKeyboardMarkup))
            .build();
        let resp = with_retry(|| self.bot.send_message(&params)).await?;
        Ok(resp.result)
    }

    /// Send a plain-text message (no parse mode). Returns the sent [`Message`].
    pub(crate) async fn send_text(&self, chat_id: i64, text: &str) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .build();
        let resp = with_retry(|| self.bot.send_message(&params)).await?;
        Ok(resp.result)
    }

    /// Edit an existing message's HTML text and inline keyboard. The edited
    /// payload (`MessageOrBool`) is discarded — callers only care about
    /// success/failure.
    pub(crate) async fn edit_html(
        &self,
        chat_id: i64,
        message_id: i32,
        text: &str,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<(), TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(text)
            .parse_mode(ParseMode::Html)
            .maybe_reply_markup(markup)
            .build();
        with_retry(|| self.bot.edit_message_text(&params)).await?;
        Ok(())
    }

    /// Answer a callback query (optional toast text, optional alert popup).
    /// Not retried — callback answers are short-lived and best-effort.
    pub(crate) async fn answer_callback(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
        show_alert: bool,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::AnswerCallbackQueryParams::builder()
            .callback_query_id(callback_query_id)
            .maybe_text(text)
            .maybe_show_alert(show_alert.then_some(true))
            .build();
        self.bot.answer_callback_query(&params).await?;
        Ok(())
    }

    /// Delete a message. Not retried — callers always discard the result.
    pub(crate) async fn delete_message(
        &self,
        chat_id: i64,
        message_id: i32,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::DeleteMessageParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .build();
        self.bot.delete_message(&params).await?;
        Ok(())
    }

    // ---- file download ----------------------------------------------------

    /// Resolve `file_id` to a file path via `get_file`, then download the bytes
    /// from Telegram's file endpoint to `dest`.
    ///
    /// SECURITY: the download URL embeds the bot token; it is never logged.
    pub(crate) async fn download_file(
        &self,
        file_id: &str,
        dest: &std::path::Path,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::GetFileParams::builder()
            .file_id(file_id.to_string())
            .build();
        let file = self.bot.get_file(&params).await?.result;
        let file_path = file.file_path.ok_or_else(|| {
            TgError::Other(format!(
                "get_file returned no file_path for file_id {file_id}"
            ))
        })?;
        // Token-bearing URL — must never be logged. Download through the bot's
        // already-configured reqwest client (timeouts, pooling, TLS/proxy), and
        // strip the URL from every reqwest error via `.without_url()` so the
        // token can never leak through a `TgError::Download` Display chain
        // (frankenstein's own paths do the same in its `From<reqwest::Error>`).
        let url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.token, file_path
        );
        let resp = self
            .bot
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.without_url())?;
        let resp = resp.error_for_status().map_err(|e| e.without_url())?;
        let bytes = resp.bytes().await.map_err(|e| e.without_url())?;
        tokio::fs::write(dest, &bytes)
            .await
            .map_err(|e| TgError::Other(format!("{e:#}")))?;
        Ok(())
    }

    // ---- webhook & commands ----------------------------------------------

    /// Register the webhook URL with a secret token, allowed-update filter, and
    /// max connections.
    pub(crate) async fn set_webhook(
        &self,
        url: &str,
        secret: &str,
        allowed: Vec<AllowedUpdate>,
        max_connections: u32,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::SetWebhookParams::builder()
            .url(url)
            .secret_token(secret)
            .allowed_updates(allowed)
            .max_connections(max_connections)
            .build();
        self.bot.set_webhook(&params).await?;
        Ok(())
    }

    /// Set the bot's command list for a scope / language.
    pub(crate) async fn set_my_commands(
        &self,
        commands: Vec<BotCommand>,
        scope: Option<BotCommandScope>,
        language_code: Option<String>,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::SetMyCommandsParams::builder()
            .commands(commands)
            .maybe_scope(scope)
            .maybe_language_code(language_code)
            .build();
        self.bot.set_my_commands(&params).await?;
        Ok(())
    }

    /// Clear the bot's command list for a scope / language.
    pub(crate) async fn delete_my_commands(
        &self,
        scope: Option<BotCommandScope>,
        language_code: Option<String>,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::DeleteMyCommandsParams::builder()
            .maybe_scope(scope)
            .maybe_language_code(language_code)
            .build();
        self.bot.delete_my_commands(&params).await?;
        Ok(())
    }

    // ---- chat action ------------------------------------------------------

    /// Send a chat action (e.g. `ChatAction::Typing`), optionally threaded.
    pub(crate) async fn send_chat_action(
        &self,
        chat_id: i64,
        action: ChatAction,
        thread: Option<i32>,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::SendChatActionParams::builder()
            .chat_id(chat_id)
            .action(action)
            .maybe_message_thread_id(thread)
            .build();
        self.bot.send_chat_action(&params).await?;
        Ok(())
    }

    // ---- media sends ------------------------------------------------------
    //
    // `media` is a `frankenstein::input_file::FileUpload`: pass a `PathBuf` for
    // a local-file upload (sandbox-outbox paths), or a `String` for a Telegram
    // `file_id`/URL. `&str` does NOT auto-convert.

    /// Send a photo, optional HTML caption, threaded, replying.
    pub(crate) async fn send_photo(
        &self,
        chat_id: i64,
        media: FileUpload,
        caption_html: Option<&str>,
        thread: Option<i32>,
        reply_to: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendPhotoParams::builder()
            .chat_id(chat_id)
            .photo(media)
            .maybe_caption(caption_html)
            .maybe_parse_mode(caption_html.map(|_| ParseMode::Html))
            .maybe_message_thread_id(thread)
            .maybe_reply_parameters(
                reply_to.map(|r| ReplyParameters::builder().message_id(r).build()),
            )
            .build();
        let resp = with_retry(|| self.bot.send_photo(&params)).await?;
        Ok(resp.result)
    }

    /// Send a document, optional HTML caption, threaded, replying.
    pub(crate) async fn send_document(
        &self,
        chat_id: i64,
        media: FileUpload,
        caption_html: Option<&str>,
        thread: Option<i32>,
        reply_to: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(media)
            .maybe_caption(caption_html)
            .maybe_parse_mode(caption_html.map(|_| ParseMode::Html))
            .maybe_message_thread_id(thread)
            .maybe_reply_parameters(
                reply_to.map(|r| ReplyParameters::builder().message_id(r).build()),
            )
            .build();
        let resp = with_retry(|| self.bot.send_document(&params)).await?;
        Ok(resp.result)
    }

    /// Send a video, optional HTML caption, threaded.
    pub(crate) async fn send_video(
        &self,
        chat_id: i64,
        media: FileUpload,
        caption_html: Option<&str>,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendVideoParams::builder()
            .chat_id(chat_id)
            .video(media)
            .maybe_caption(caption_html)
            .maybe_parse_mode(caption_html.map(|_| ParseMode::Html))
            .maybe_message_thread_id(thread)
            .build();
        let resp = with_retry(|| self.bot.send_video(&params)).await?;
        Ok(resp.result)
    }

    /// Send a voice message, optional HTML caption, threaded.
    pub(crate) async fn send_voice(
        &self,
        chat_id: i64,
        media: FileUpload,
        caption_html: Option<&str>,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendVoiceParams::builder()
            .chat_id(chat_id)
            .voice(media)
            .maybe_caption(caption_html)
            .maybe_parse_mode(caption_html.map(|_| ParseMode::Html))
            .maybe_message_thread_id(thread)
            .build();
        let resp = with_retry(|| self.bot.send_voice(&params)).await?;
        Ok(resp.result)
    }

    /// Send an audio file, optional HTML caption, threaded.
    pub(crate) async fn send_audio(
        &self,
        chat_id: i64,
        media: FileUpload,
        caption_html: Option<&str>,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendAudioParams::builder()
            .chat_id(chat_id)
            .audio(media)
            .maybe_caption(caption_html)
            .maybe_parse_mode(caption_html.map(|_| ParseMode::Html))
            .maybe_message_thread_id(thread)
            .build();
        let resp = with_retry(|| self.bot.send_audio(&params)).await?;
        Ok(resp.result)
    }

    /// Send an animation (GIF/MP4), optional HTML caption, threaded.
    pub(crate) async fn send_animation(
        &self,
        chat_id: i64,
        media: FileUpload,
        caption_html: Option<&str>,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendAnimationParams::builder()
            .chat_id(chat_id)
            .animation(media)
            .maybe_caption(caption_html)
            .maybe_parse_mode(caption_html.map(|_| ParseMode::Html))
            .maybe_message_thread_id(thread)
            .build();
        let resp = with_retry(|| self.bot.send_animation(&params)).await?;
        Ok(resp.result)
    }

    /// Send a video note (round video). No caption / parse mode in the API.
    pub(crate) async fn send_video_note(
        &self,
        chat_id: i64,
        media: FileUpload,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendVideoNoteParams::builder()
            .chat_id(chat_id)
            .video_note(media)
            .maybe_message_thread_id(thread)
            .build();
        let resp = with_retry(|| self.bot.send_video_note(&params)).await?;
        Ok(resp.result)
    }

    /// Send a sticker. No caption / parse mode in the API.
    pub(crate) async fn send_sticker(
        &self,
        chat_id: i64,
        media: FileUpload,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendStickerParams::builder()
            .chat_id(chat_id)
            .sticker(media)
            .maybe_message_thread_id(thread)
            .build();
        let resp = with_retry(|| self.bot.send_sticker(&params)).await?;
        Ok(resp.result)
    }

    /// Send a media group (album). Captions / parse modes / keyboards live on
    /// each `MediaGroupInputMedia` member, not at the top level.
    pub(crate) async fn send_media_group(
        &self,
        chat_id: i64,
        media: Vec<MediaGroupInputMedia>,
        thread: Option<i32>,
    ) -> Result<Vec<Message>, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendMediaGroupParams::builder()
            .chat_id(chat_id)
            .media(media)
            .maybe_message_thread_id(thread)
            .build();
        let resp = with_retry(|| self.bot.send_media_group(&params)).await?;
        Ok(resp.result)
    }

    // ---- forum topics -----------------------------------------------------

    /// Close a forum topic.
    pub(crate) async fn close_forum_topic(
        &self,
        chat_id: i64,
        message_thread_id: i32,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::CloseForumTopicParams::builder()
            .chat_id(chat_id)
            .message_thread_id(message_thread_id)
            .build();
        self.bot.close_forum_topic(&params).await?;
        Ok(())
    }

    /// Reopen a forum topic.
    pub(crate) async fn reopen_forum_topic(
        &self,
        chat_id: i64,
        message_thread_id: i32,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::ReopenForumTopicParams::builder()
            .chat_id(chat_id)
            .message_thread_id(message_thread_id)
            .build();
        self.bot.reopen_forum_topic(&params).await?;
        Ok(())
    }

    /// Edit a forum topic's name and/or icon.
    pub(crate) async fn edit_forum_topic(
        &self,
        chat_id: i64,
        message_thread_id: i32,
        name: Option<String>,
        icon_custom_emoji_id: Option<String>,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::EditForumTopicParams::builder()
            .chat_id(chat_id)
            .message_thread_id(message_thread_id)
            .maybe_name(name)
            .maybe_icon_custom_emoji_id(icon_custom_emoji_id)
            .build();
        self.bot.edit_forum_topic(&params).await?;
        Ok(())
    }
}

#[cfg(test)]
mod throttle_tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::Instant;

    #[tokio::test(start_paused = true)]
    async fn per_chat_gate_spaces_same_chat_sends() {
        let t = Throttle::new(30, Duration::from_millis(1000));
        let start = Instant::now();
        t.acquire(100).await;
        t.acquire(100).await;
        assert!(start.elapsed() >= Duration::from_millis(1000));
    }

    #[tokio::test(start_paused = true)]
    async fn different_chats_do_not_block_each_other_on_per_chat_gate() {
        let t = Throttle::new(30, Duration::from_millis(1000));
        let start = Instant::now();
        t.acquire(1).await;
        t.acquire(2).await;
        assert!(start.elapsed() < Duration::from_millis(1000));
    }
}

#[cfg(test)]
mod retry_after_tests {
    use super::*;
    use frankenstein::response::{ErrorResponse, ResponseParameters};

    #[test]
    fn retry_after_secs_extracts_from_api_error() {
        let err = frankenstein::Error::Api(ErrorResponse {
            ok: false,
            description: "Too Many Requests".to_string(),
            error_code: 429,
            parameters: Some(ResponseParameters {
                migrate_to_chat_id: None,
                retry_after: Some(7),
            }),
        });
        assert_eq!(retry_after_secs(&err), Some(7));
    }

    #[test]
    fn retry_after_secs_none_without_parameters() {
        let err = frankenstein::Error::Api(ErrorResponse {
            ok: false,
            description: "Bad Request".to_string(),
            error_code: 400,
            parameters: None,
        });
        assert_eq!(retry_after_secs(&err), None);
    }

    #[test]
    fn retry_after_secs_none_when_parameters_lack_retry_after() {
        let err = frankenstein::Error::Api(ErrorResponse {
            ok: false,
            description: "no retry_after".to_string(),
            error_code: 429,
            parameters: Some(ResponseParameters {
                migrate_to_chat_id: Some(123),
                retry_after: None,
            }),
        });
        assert_eq!(retry_after_secs(&err), None);
    }

    #[test]
    fn retry_after_secs_none_for_non_api_error() {
        let err = frankenstein::Error::ReadFile(std::io::Error::other("boom"));
        assert_eq!(retry_after_secs(&err), None);
    }
}

#[cfg(test)]
mod with_retry_tests {
    use super::*;
    use frankenstein::response::{ErrorResponse, ResponseParameters};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn api_429_retry_after_zero() -> frankenstein::Error {
        frankenstein::Error::Api(ErrorResponse {
            ok: false,
            description: "Too Many Requests".to_string(),
            error_code: 429,
            parameters: Some(ResponseParameters {
                migrate_to_chat_id: None,
                // retry_after = 0 keeps the sleep instant under the paused clock.
                retry_after: Some(0),
            }),
        })
    }

    #[tokio::test(start_paused = true)]
    async fn retries_once_on_retry_after_then_succeeds() {
        let calls = AtomicUsize::new(0);
        let result: Result<i32, TgError> = with_retry(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Err(api_429_retry_after_zero())
                } else {
                    Ok(42)
                }
            }
        })
        .await;
        assert!(matches!(result, Ok(42)));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "should call exactly twice");
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_retry_non_retryable_error() {
        let calls = AtomicUsize::new(0);
        let result: Result<i32, TgError> = with_retry(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            // Api error with no parameters → no retry_after → not retryable.
            async {
                Err(frankenstein::Error::Api(ErrorResponse {
                    ok: false,
                    description: "Bad Request".to_string(),
                    error_code: 400,
                    parameters: None,
                }))
            }
        })
        .await;
        assert!(matches!(result, Err(TgError::Api(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "should call exactly once");
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;

    /// Regression guard for the token-leak fix in `download_file`: a token-bearing
    /// download URL must never survive into the error's `Display` chain. We do not
    /// hit the network — a closed local port yields a deterministic
    /// connection-refused error, which we then run through the same
    /// `.without_url()` mapping `download_file` uses.
    #[tokio::test]
    async fn reqwest_error_display_does_not_leak_token_after_without_url() {
        const FAKE_TOKEN: &str = "123456:FAKE_SENTINEL_TOKEN_DO_NOT_LEAK";
        let url = format!("http://127.0.0.1:1/file/bot{FAKE_TOKEN}/x");
        let client = reqwest::Client::new();
        let err = client
            .get(&url)
            .send()
            .await
            .expect_err("connecting to a closed local port must fail")
            .without_url();
        let rendered = format!("{err}");
        assert!(
            !rendered.contains(FAKE_TOKEN),
            "token leaked into reqwest error Display: {rendered}"
        );
        // Sanity: also confirm wrapping into TgError::Download keeps it stripped.
        let tg: TgError = err.into();
        assert!(
            !format!("{tg}").contains(FAKE_TOKEN),
            "token leaked through TgError::Download Display"
        );
    }
}
