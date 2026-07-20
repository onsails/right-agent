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
use governor::state::keyed::DefaultKeyedStateStore;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use thiserror::Error;
use tokio::time::Instant;

use frankenstein::AsyncTelegramApi;
use frankenstein::ParseMode;
use frankenstein::client_reqwest::Bot as FBot;
use frankenstein::input_file::{FileUpload, InputFile};
use frankenstein::input_media::MediaGroupInputMedia;
use frankenstein::types::{
    AllowedUpdate, BotCommand, BotCommandScope, ChatAction, InlineKeyboardMarkup, MenuButton,
    MenuButtonWebApp, Message, ReplyMarkup, ReplyParameters, User, WebAppInfo,
};

/// Default global send rate (messages/second) across all chats. Matches the
/// teloxide `Limits::default()` global cap we relied on previously.
const DEFAULT_GLOBAL_PER_SEC: u32 = 30;

/// Default minimum spacing between two sends to the *same* chat. Matches the
/// teloxide per-chat cap (~1 message/second/chat).
const DEFAULT_PER_CHAT_INTERVAL: Duration = Duration::from_millis(1000);

/// Default per-chat per-minute cap for private chats and basic groups. Matches
/// teloxide `Limits::default()`'s `messages_per_min_chat = 20`.
const DEFAULT_PER_CHAT_PER_MIN: u32 = 20;

/// Stricter per-minute cap for channels and supergroups. Matches teloxide
/// `Limits::default()`'s `messages_per_min_channel_or_supergroup = 10`.
const DEFAULT_CHANNEL_SUPERGROUP_PER_MIN: u32 = 10;

/// Telegram encodes channel/supergroup chat ids as `-100…` (magnitude ≥ 10¹²);
/// basic-group ids are smaller-magnitude negatives and private-chat ids are
/// positive. A chat id strictly below this threshold is a channel/supergroup —
/// the same partition teloxide's `Chat::is_channel_or_supergroup()` made when
/// selecting the per-minute cap.
const SUPERGROUP_CHANNEL_ID_THRESHOLD: i64 = -1_000_000_000_000;

/// Prune the per-chat reservation map (and shed caught-up keyed-limiter keys)
/// every this many `acquire` calls, bounding throttle memory for a long-lived
/// bot that sends to many distinct chats.
const THROTTLE_PRUNE_INTERVAL: u64 = 256;

/// Maximum automatic retries on a 429 `retry_after` response. teloxide re-queued
/// throttled sends until success; we bound it so a persistent 429 cannot block a
/// single send indefinitely.
const MAX_429_RETRIES: u32 = 3;

/// True when `chat_id` is a Telegram channel or supergroup (encoded `-100…`).
fn is_channel_or_supergroup_id(chat_id: i64) -> bool {
    chat_id < SUPERGROUP_CHANNEL_ID_THRESHOLD
}

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
/// Reproduces teloxide `Limits::default()`'s caps:
/// - global ~30 messages/second (`messages_per_sec_overall`),
/// - per-chat ~1 message/second (`messages_per_sec_chat`),
/// - 20 messages/minute per private chat / basic group (`messages_per_min_chat`),
/// - 10 messages/minute per channel/supergroup
///   (`messages_per_min_channel_or_supergroup`), selected by chat-id encoding.
///
/// The per-chat 1s gate is a *reservation* map (keyed `tokio::time::Instant`):
/// each `acquire` atomically advances the chat's next-allowed instant by the
/// interval, so concurrent acquires to the same chat serialize at ~1/interval
/// with no read-then-write race. Because it uses the tokio clock it advances
/// correctly under the tokio test clock (`start_paused = true`). The global and
/// keyed per-minute limiters are [`governor`] rate limiters on `DefaultClock`
/// (real monotonic wall-clock), so their `until_ready().await` sleeps in real
/// time regardless of `start_paused` — they are not exercised by the
/// paused-clock unit tests (which keep volumes under both caps, so the calls
/// return immediately).
pub(crate) struct Throttle {
    global: RateLimiter<NotKeyed, InMemoryState, DefaultClock>,
    /// 20/min cap for private chats and basic groups.
    per_chat_per_min: RateLimiter<i64, DefaultKeyedStateStore<i64>, DefaultClock>,
    /// Stricter 10/min cap for channels/supergroups.
    per_channel_per_min: RateLimiter<i64, DefaultKeyedStateStore<i64>, DefaultClock>,
    per_chat_interval: Duration,
    /// Next instant a send to each chat is permitted; advanced atomically per
    /// `acquire`. Pruned periodically (see [`Throttle::maybe_prune`]).
    next_per_chat: DashMap<i64, Instant>,
    /// Counts `acquire` calls to trigger periodic pruning.
    acquire_count: std::sync::atomic::AtomicU64,
}

impl Throttle {
    pub(crate) fn new(
        global_per_sec: u32,
        per_chat_interval: Duration,
        per_chat_per_min: u32,
        per_channel_per_min: u32,
    ) -> Self {
        let global_quota =
            Quota::per_second(NonZeroU32::new(global_per_sec).expect("global_per_sec must be > 0"));
        let per_min_quota = Quota::per_minute(
            NonZeroU32::new(per_chat_per_min).expect("per_chat_per_min must be > 0"),
        );
        let per_channel_quota = Quota::per_minute(
            NonZeroU32::new(per_channel_per_min).expect("per_channel_per_min must be > 0"),
        );
        Self {
            global: RateLimiter::direct(global_quota),
            per_chat_per_min: RateLimiter::keyed(per_min_quota),
            per_channel_per_min: RateLimiter::keyed(per_channel_quota),
            per_chat_interval,
            next_per_chat: DashMap::new(),
            acquire_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Block until it is permissible to send to `chat_id`: first wait out the
    /// per-chat 1s interval, then the per-minute cap (10/min for
    /// channels/supergroups, 20/min otherwise), then a global token.
    pub(crate) async fn acquire(&self, chat_id: i64) {
        // Reserve the next per-chat slot atomically (no read-then-write race):
        // the slot advances by `per_chat_interval` per send, so concurrent
        // acquires to the same chat serialize at ~1/interval instead of all
        // passing the gate at once. The entry guard is dropped before the await.
        let at = {
            let now = Instant::now();
            let mut slot = self.next_per_chat.entry(chat_id).or_insert(now);
            let at = (*slot).max(now);
            *slot = at + self.per_chat_interval;
            at
        };
        tokio::time::sleep_until(at).await;

        if is_channel_or_supergroup_id(chat_id) {
            self.per_channel_per_min.until_key_ready(&chat_id).await;
        } else {
            self.per_chat_per_min.until_key_ready(&chat_id).await;
        }
        self.global.until_ready().await;

        self.maybe_prune();
    }

    /// Periodically drop reservation entries already in the past (they impose no
    /// constraint) and let the keyed governor limiters shed caught-up keys.
    /// Bounds throttle memory for a long-lived bot in many chats.
    fn maybe_prune(&self) {
        use std::sync::atomic::Ordering;
        let n = self.acquire_count.fetch_add(1, Ordering::Relaxed);
        if !n.is_multiple_of(THROTTLE_PRUNE_INTERVAL) {
            return;
        }
        let now = Instant::now();
        self.next_per_chat.retain(|_, next| *next > now);
        self.per_chat_per_min.retain_recent();
        self.per_channel_per_min.retain_recent();
    }
}

/// The default outbound throttle matching teloxide `Limits::default()`.
fn default_throttle() -> Throttle {
    Throttle::new(
        DEFAULT_GLOBAL_PER_SEC,
        DEFAULT_PER_CHAT_INTERVAL,
        DEFAULT_PER_CHAT_PER_MIN,
        DEFAULT_CHANNEL_SUPERGROUP_PER_MIN,
    )
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

/// Run `call`, retrying on a 429 `retry_after` error up to [`MAX_429_RETRIES`]
/// times, sleeping the server-supplied `retry_after` before each retry.
///
/// `call` is a closure (not a future) because a single future cannot be awaited
/// twice. Each retry sleeps the server-supplied `retry_after` (the authoritative
/// backoff) and intentionally does NOT re-acquire the per-chat/global rate gate.
async fn with_retry<F, Fut, T>(call: F) -> Result<T, TgError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, frankenstein::Error>>,
{
    let mut attempt: u32 = 0;
    loop {
        match call().await {
            Ok(v) => return Ok(v),
            Err(e) => match retry_after_secs(&e) {
                Some(after) if attempt < MAX_429_RETRIES => {
                    attempt += 1;
                    tracing::warn!(retry_after = after, attempt, "telegram 429 — retrying");
                    tokio::time::sleep(Duration::from_secs(after)).await;
                }
                _ => return Err(e.into()),
            },
        }
    }
}

/// Placeholder identity for [`RightBot::new`] (no `get_me` round-trip). Never
/// surfaced — auxiliary send-only bots never call [`RightBot::me`].
fn placeholder_user() -> User {
    User {
        id: 0,
        is_bot: true,
        first_name: String::new(),
        last_name: None,
        username: None,
        language_code: None,
        is_premium: None,
        added_to_attachment_menu: None,
        can_join_groups: None,
        can_read_all_group_messages: None,
        supports_guest_queries: None,
        supports_inline_queries: None,
        can_connect_to_business: None,
        has_main_web_app: None,
        has_topics_enabled: None,
        allows_users_to_create_topics: None,
        can_manage_bots: None,
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
    /// Construct without resolving identity. No network I/O. The cached
    /// identity ([`Self::me`]) is a placeholder — only valid on a
    /// `connect`-ed bot. Use this for auxiliary send-only bots (menu,
    /// webhook-register, supervisor, delivery, focus notifier) that never call
    /// [`Self::me`]; the identity-bearing dispatcher bot uses [`Self::connect`].
    pub(crate) fn new(token: String) -> Self {
        let bot = FBot::new(&token);
        Self {
            bot,
            token: Arc::new(token),
            me: Arc::new(placeholder_user()),
            rate: Arc::new(default_throttle()),
        }
    }

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
            rate: Arc::new(default_throttle()),
        })
    }

    /// The bot's own resolved identity (cached at `connect` time). On a bot
    /// built via [`Self::new`] this is a placeholder — only the dispatcher bot
    /// (built via [`Self::connect`]) has a meaningful identity.
    pub(crate) fn me(&self) -> &User {
        &self.me
    }

    /// Resolve a chat's title, falling back to its numeric id when Telegram
    /// does not provide one.
    pub(crate) async fn get_chat_title(&self, chat_id: i64) -> Result<String, TgError> {
        let params = frankenstein::methods::GetChatParams::builder()
            .chat_id(chat_id)
            .build();
        let resp = self.bot.get_chat(&params).await?;
        Ok(resp.result.title.unwrap_or_else(|| chat_id.to_string()))
    }

    /// The raw bot token. Used for token-derived focus-scope MACs. Never log.
    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    // ---- core sends -------------------------------------------------------

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

    /// General message send. `html` selects `ParseMode::Html` (else no parse
    /// mode); `thread`/`reply_to`/`markup` are optional. Returns the sent
    /// [`Message`]. Covers every send call site that needs a mix of these
    /// options (worker replies, delivery, progress, focus, alerts).
    pub(crate) async fn send_message_opts(
        &self,
        chat_id: i64,
        text: &str,
        html: bool,
        thread: Option<i32>,
        reply_to: Option<i32>,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .maybe_parse_mode(html.then_some(ParseMode::Html))
            .maybe_message_thread_id(thread)
            .maybe_reply_parameters(
                reply_to.map(|r| ReplyParameters::builder().message_id(r).build()),
            )
            .maybe_reply_markup(markup.map(ReplyMarkup::InlineKeyboardMarkup))
            .build();
        let resp = with_retry(|| self.bot.send_message(&params)).await?;
        Ok(resp.result)
    }

    /// Edit an existing message's HTML text and inline keyboard. The edited
    /// payload (`MessageOrBool`) is discarded — callers only care about
    /// success/failure.
    ///
    /// NOT throttled — teloxide put `editMessageText` in its un-throttled
    /// passthrough block (only `send_*` were rate-limited). Keeping edits
    /// un-throttled preserves that and keeps progress-banner edit streaming
    /// responsive. The `with_retry` 429 backstop still covers a server flood.
    pub(crate) async fn edit_html(
        &self,
        chat_id: i64,
        message_id: i32,
        text: &str,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<(), TgError> {
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

    /// Edit an existing message's text as PLAIN text (no parse mode) and inline
    /// keyboard — a faithful port of a teloxide `editMessageText` call that did
    /// not set `.parse_mode`. Use for edits whose text is not HTML (menu bodies,
    /// status banners); use [`Self::edit_html`] when the text is HTML. Sending
    /// non-HTML text through `edit_html` would force `ParseMode::Html` and let an
    /// unescaped `<`/`&` trigger a Telegram 400.
    ///
    /// NOT throttled — same rationale as [`Self::edit_html`].
    pub(crate) async fn edit_text(
        &self,
        chat_id: i64,
        message_id: i32,
        text: &str,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(text)
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
        //
        // Derive the file endpoint from the bot's configured `api_url`
        // (`<base>/bot<token>`) so a non-default Bot API base (e.g. a local Bot
        // API server) is honored, replacing `/bot<token>` with `/file/bot<token>`.
        let api_base = self
            .bot
            .api_url
            .rsplit_once("/bot")
            .map_or("https://api.telegram.org", |(base, _)| base);
        let url = format!("{api_base}/file/bot{}/{}", self.token, file_path);
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
        caption: Option<&str>,
        html: bool,
        thread: Option<i32>,
        reply_to: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendPhotoParams::builder()
            .chat_id(chat_id)
            .photo(media)
            .maybe_caption(caption)
            .maybe_parse_mode((caption.is_some() && html).then_some(ParseMode::Html))
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
        caption: Option<&str>,
        html: bool,
        thread: Option<i32>,
        reply_to: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(media)
            .maybe_caption(caption)
            .maybe_parse_mode((caption.is_some() && html).then_some(ParseMode::Html))
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
        caption: Option<&str>,
        html: bool,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendVideoParams::builder()
            .chat_id(chat_id)
            .video(media)
            .maybe_caption(caption)
            .maybe_parse_mode((caption.is_some() && html).then_some(ParseMode::Html))
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
        caption: Option<&str>,
        html: bool,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendVoiceParams::builder()
            .chat_id(chat_id)
            .voice(media)
            .maybe_caption(caption)
            .maybe_parse_mode((caption.is_some() && html).then_some(ParseMode::Html))
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
        caption: Option<&str>,
        html: bool,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendAudioParams::builder()
            .chat_id(chat_id)
            .audio(media)
            .maybe_caption(caption)
            .maybe_parse_mode((caption.is_some() && html).then_some(ParseMode::Html))
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
        caption: Option<&str>,
        html: bool,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendAnimationParams::builder()
            .chat_id(chat_id)
            .animation(media)
            .maybe_caption(caption)
            .maybe_parse_mode((caption.is_some() && html).then_some(ParseMode::Html))
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

    /// Create a forum topic; returns the new `message_thread_id`.
    pub(crate) async fn create_forum_topic(
        &self,
        chat_id: i64,
        name: String,
        icon_color: Option<u32>,
        icon_custom_emoji_id: Option<String>,
    ) -> Result<i32, TgError> {
        let params = frankenstein::methods::CreateForumTopicParams::builder()
            .chat_id(chat_id)
            .name(name)
            .maybe_icon_color(icon_color)
            .maybe_icon_custom_emoji_id(icon_custom_emoji_id)
            .build();
        let resp = self.bot.create_forum_topic(&params).await?;
        Ok(resp.result.message_thread_id)
    }

    // ---- chat menu button -------------------------------------------------

    /// Set the chat menu button to launch a Mini App at `url`.
    pub(crate) async fn set_chat_menu_button_webapp(
        &self,
        text: &str,
        url: String,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::SetChatMenuButtonParams::builder()
            .menu_button(MenuButton::WebApp(MenuButtonWebApp {
                text: text.to_string(),
                web_app: WebAppInfo { url },
            }))
            .build();
        self.bot.set_chat_menu_button(&params).await?;
        Ok(())
    }

    // ---- in-memory uploads ------------------------------------------------
    //
    // frankenstein's `InputFile` is path-only (no in-memory variant), so these
    // helpers spool bytes to a temp file kept alive for the multipart upload,
    // then drop it (which deletes it).

    /// Send an in-memory photo (spooled to a temp file for the upload).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send_photo_bytes(
        &self,
        chat_id: i64,
        bytes: &[u8],
        filename: &str,
        caption: Option<&str>,
        html: bool,
        thread: Option<i32>,
        reply_to: Option<i32>,
    ) -> Result<Message, TgError> {
        let spool = SpooledUpload::new(bytes, filename).await?;
        let upload = FileUpload::InputFile(InputFile {
            path: spool.path().to_path_buf(),
        });
        self.send_photo(chat_id, upload, caption, html, thread, reply_to)
            .await
        // `spool` (the TempDir) drops here, after the multipart upload completes.
    }

    /// Send an in-memory document (spooled to a temp file for the upload).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send_document_bytes(
        &self,
        chat_id: i64,
        bytes: &[u8],
        filename: &str,
        caption: Option<&str>,
        html: bool,
        thread: Option<i32>,
        reply_to: Option<i32>,
    ) -> Result<Message, TgError> {
        let spool = SpooledUpload::new(bytes, filename).await?;
        let upload = FileUpload::InputFile(InputFile {
            path: spool.path().to_path_buf(),
        });
        self.send_document(chat_id, upload, caption, html, thread, reply_to)
            .await
    }
}

/// An in-memory payload spooled to disk for a frankenstein multipart upload.
/// The file lives at `<tempdir>/<filename>` so Telegram sees the exact
/// `filename` as the sent file name. The backing temp dir (and file) is deleted
/// when this drops — keep it alive for the full duration of the send.
struct SpooledUpload {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
}

impl SpooledUpload {
    async fn new(bytes: &[u8], filename: &str) -> Result<Self, TgError> {
        // `tempdir()` is a blocking syscall — run it off the async runtime.
        let dir = tokio::task::spawn_blocking(|| {
            tempfile::Builder::new().prefix("right-upload-").tempdir()
        })
        .await
        .map_err(|e| TgError::Other(format!("spool tempdir task failed: {e}")))?
        .map_err(|e| TgError::Other(format!("create temp upload dir: {e:#}")))?;
        // Use only the basename of `filename` to keep the write inside `dir`.
        let base = std::path::Path::new(filename)
            .file_name()
            .map(std::ffi::OsStr::to_owned)
            .unwrap_or_else(|| std::ffi::OsString::from("upload.bin"));
        let path = dir.path().join(base);
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| TgError::Other(format!("write temp upload file: {e:#}")))?;
        Ok(Self { _dir: dir, path })
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl TgError {
    /// True when the error is Telegram's invalid-token API response. teloxide
    /// mapped BOTH "Unauthorized" (HTTP 401) and "Not Found" (HTTP 404) to
    /// `ApiError::InvalidToken` — 404 is the "URL not handled by the Bot API at
    /// all" malformed-token case. Used by the webhook-register loop to fail fast
    /// (exit 2) on a bad bot token rather than retrying forever.
    pub(crate) fn is_invalid_token(&self) -> bool {
        matches!(
            self,
            TgError::Api(frankenstein::Error::Api(resp))
                if resp.error_code == 401 || resp.error_code == 404
        )
    }

    /// True when the error is Telegram's "file is too big" 400 — `getFile`
    /// rejects files larger than the 20 MB download limit. Used to surface a
    /// friendly oversized-attachment skip instead of aborting a download batch.
    pub(crate) fn is_file_too_big(&self) -> bool {
        matches!(
            self,
            TgError::Api(frankenstein::Error::Api(resp))
                if resp.error_code == 400
                    && resp.description.to_ascii_lowercase().contains("file is too big")
        )
    }
}

#[cfg(test)]
mod throttle_tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::Instant;

    #[tokio::test(start_paused = true)]
    async fn per_chat_gate_spaces_same_chat_sends() {
        let t = Throttle::new(30, Duration::from_millis(1000), 20, 10);
        let start = Instant::now();
        t.acquire(100).await;
        t.acquire(100).await;
        assert!(start.elapsed() >= Duration::from_millis(1000));
    }

    #[tokio::test(start_paused = true)]
    async fn different_chats_do_not_block_each_other_on_per_chat_gate() {
        let t = Throttle::new(30, Duration::from_millis(1000), 20, 10);
        let start = Instant::now();
        t.acquire(1).await;
        t.acquire(2).await;
        assert!(start.elapsed() < Duration::from_millis(1000));
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_same_chat_acquires_serialize_at_interval() {
        // Reservation gate: two concurrent acquires to the same chat must NOT
        // both pass immediately — the second is spaced one interval later.
        use std::sync::Arc;
        let t = Arc::new(Throttle::new(30, Duration::from_millis(1000), 20, 10));
        let start = Instant::now();
        let a = tokio::spawn({
            let t = Arc::clone(&t);
            async move { t.acquire(100).await }
        });
        let b = tokio::spawn({
            let t = Arc::clone(&t);
            async move { t.acquire(100).await }
        });
        a.await.unwrap();
        b.await.unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(1000),
            "two concurrent same-chat acquires must serialize at the per-chat interval"
        );
    }

    #[test]
    fn channel_supergroup_id_detection_matches_telegram_encoding() {
        assert!(is_channel_or_supergroup_id(-1001234567890)); // real supergroup id
        assert!(is_channel_or_supergroup_id(-1_000_000_000_001));
        // Boundary value is not a real id, but classifies as non-channel.
        assert!(!is_channel_or_supergroup_id(-1_000_000_000_000));
        assert!(!is_channel_or_supergroup_id(-123456789)); // basic group
        assert!(!is_channel_or_supergroup_id(42)); // private chat
    }
}

#[cfg(test)]
mod tg_error_tests {
    use super::*;
    use frankenstein::response::ErrorResponse;

    fn api(code: u64, desc: &str) -> TgError {
        TgError::Api(frankenstein::Error::Api(ErrorResponse {
            ok: false,
            description: desc.to_string(),
            error_code: code,
            parameters: None,
        }))
    }

    #[test]
    fn is_invalid_token_matches_401_and_404() {
        // teloxide mapped both "Unauthorized"(401) and "Not Found"(404) to InvalidToken.
        assert!(api(401, "Unauthorized").is_invalid_token());
        assert!(api(404, "Not Found").is_invalid_token());
        assert!(!api(400, "Bad Request").is_invalid_token());
        assert!(!api(429, "Too Many Requests").is_invalid_token());
        assert!(!TgError::Other("boom".to_string()).is_invalid_token());
    }

    #[test]
    fn is_file_too_big_matches_400_file_too_big_only() {
        assert!(api(400, "Bad Request: file is too big").is_file_too_big());
        assert!(!api(400, "Bad Request: chat not found").is_file_too_big());
        // Right code is a 400; a different code with the phrase must not match.
        assert!(!api(413, "file is too big").is_file_too_big());
        assert!(!TgError::Other("file is too big".to_string()).is_file_too_big());
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

    #[tokio::test(start_paused = true)]
    async fn stops_retrying_after_max_429_retries() {
        let calls = AtomicUsize::new(0);
        let result: Result<i32, TgError> = with_retry(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            // Always 429 — a persistent flood must not loop forever.
            async { Err(api_429_retry_after_zero()) }
        })
        .await;
        assert!(matches!(result, Err(TgError::Api(_))));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            (MAX_429_RETRIES + 1) as usize,
            "should call once + MAX_429_RETRIES retries, then give up"
        );
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
