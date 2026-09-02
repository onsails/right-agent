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
//! - Uniform defaults for regular HTML/plain messages and validated rich-block sends.
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
use frankenstein::rich_message::InputRichMessage;
use frankenstein::types::{
    AllowedUpdate, BotCommand, BotCommandScope, ChatAction, ChatMember, InlineKeyboardButton,
    InlineKeyboardMarkup, MenuButton, MenuButtonWebApp, Message, ReplyMarkup, ReplyParameters,
    User, WebAppInfo,
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

/// Per-attempt ceiling for text-class Telegram API calls (message sends,
/// edits, chat actions). frankenstein's default reqwest client allows 500s per
/// request — far too long for the worker's turn path: a blackholed
/// api.telegram.org parked the thinking-anchor send before the CC turn
/// deadline was even armed, wedging the chat queue until restart
/// (riskoff, 2026-07-19). Media uploads keep the client's 500s cap.
const TELEGRAM_TEXT_TIMEOUT: Duration = Duration::from_secs(30);
/// Per-attempt ceiling for typed rich-media uploads.
const TELEGRAM_MEDIA_TIMEOUT: Duration = Duration::from_secs(500);

/// Maximum automatic retries on a 429 `retry_after` response. teloxide re-queued
/// throttled sends until success; we bound it so a persistent 429 cannot block a
/// single send indefinitely.
const MAX_429_RETRIES: u32 = 3;

/// Telegram Bot API origin used for live token authentication checks.
const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Maximum time init waits for Telegram `getMe` readiness validation.
const TELEGRAM_AUTH_VALIDATION_TIMEOUT: Duration = Duration::from_secs(15);

/// Validate a Telegram bot token with a live `getMe` request.
///
/// The underlying client URL contains the token, so failures are deliberately
/// collapsed to a stable diagnostic before crossing the crate boundary.
pub async fn validate_telegram_token_live(token: &str) -> anyhow::Result<()> {
    validate_telegram_token_live_with_api_base_and_timeout(
        token,
        TELEGRAM_API_BASE,
        TELEGRAM_AUTH_VALIDATION_TIMEOUT,
    )
    .await
}

#[cfg(test)]
async fn validate_telegram_token_live_with_api_base(
    token: &str,
    api_base: &str,
) -> anyhow::Result<()> {
    validate_telegram_token_live_with_api_base_and_timeout(
        token,
        api_base,
        TELEGRAM_AUTH_VALIDATION_TIMEOUT,
    )
    .await
}

async fn validate_telegram_token_live_with_api_base_and_timeout(
    token: &str,
    api_base: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let api_url = format!("{}/bot{token}", api_base.trim_end_matches('/'));
    let bot = FBot::builder().api_url(api_url).build();
    match tokio::time::timeout(timeout, bot.get_me()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => anyhow::bail!("Telegram authentication failed"),
        Err(_) => anyhow::bail!("Telegram authentication validation timed out"),
    }
}

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
    #[error("telegram request timed out after {0:?}")]
    Timeout(Duration),
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
/// backoff) and intentionally does not re-acquire the per-chat/global rate gate.
async fn with_retry<F, Fut, T>(call: F) -> Result<T, TgError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, frankenstein::Error>>,
{
    with_retry_bounded(call, None).await
}

async fn with_optional_retry<F, Fut, T>(call: F, retry_429: bool) -> Result<T, TgError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, frankenstein::Error>>,
{
    if retry_429 {
        with_retry(call).await
    } else {
        call().await.map_err(TgError::Api)
    }
}

/// [`with_retry`] for text-class calls, with a per-attempt ceiling of
/// [`TELEGRAM_TEXT_TIMEOUT`] so a stalled api.telegram.org cannot park the
/// worker. A timed-out attempt is terminal (no retry): retrying a blackholed
/// endpoint would just stall again.
async fn with_retry_text<F, Fut, T>(call: F) -> Result<T, TgError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, frankenstein::Error>>,
{
    with_retry_bounded(call, Some(TELEGRAM_TEXT_TIMEOUT)).await
}

async fn with_retry_bounded<F, Fut, T>(
    call: F,
    attempt_timeout: Option<Duration>,
) -> Result<T, TgError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, frankenstein::Error>>,
{
    let mut attempt: u32 = 0;
    loop {
        let outcome = match attempt_timeout {
            Some(d) => match tokio::time::timeout(d, call()).await {
                Err(_) => return Err(TgError::Timeout(d)),
                Ok(r) => r,
            },
            None => call().await,
        };
        match outcome {
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
        supports_join_request_queries: None,
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

    /// Test-only constructor pointed at a stub API base URL (e.g. a local
    /// stall server), for exercising send-path behavior without Telegram.
    #[cfg(test)]
    pub(crate) fn new_for_test(api_url: String) -> Self {
        let bot = FBot::builder().api_url(api_url).build();
        Self {
            bot,
            token: Arc::new("test-token".to_owned()),
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

    /// Fetch a user's membership in a chat. Used to verify the bot is still a
    /// channel administrator before committing an allowlist write.
    pub(crate) async fn get_chat_member(
        &self,
        chat_id: i64,
        user_id: u64,
    ) -> Result<ChatMember, TgError> {
        let params = frankenstein::methods::GetChatMemberParams::builder()
            .chat_id(chat_id)
            .user_id(user_id)
            .build();
        let resp = with_retry(|| self.bot.get_chat_member(&params)).await?;
        Ok(resp.result)
    }

    /// Remove the inline keyboard from a message (empty markup). Used to retire
    /// one-shot confirm buttons after they are consumed.
    pub(crate) async fn remove_reply_keyboard(
        &self,
        chat_id: i64,
        message_id: i32,
    ) -> Result<(), TgError> {
        let params = frankenstein::methods::EditMessageReplyMarkupParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .reply_markup(
                InlineKeyboardMarkup::builder()
                    .inline_keyboard(Vec::<Vec<InlineKeyboardButton>>::new())
                    .build(),
            )
            .build();
        with_retry(|| self.bot.edit_message_reply_markup(&params)).await?;
        Ok(())
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
        let resp = with_retry_text(|| self.bot.send_message(&params)).await?;
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
        let resp = with_retry_text(|| self.bot.send_message(&params)).await?;
        Ok(resp.result)
    }

    /// Send a validated Telegram rich block tree.
    pub(crate) async fn send_rich_content(
        &self,
        chat_id: i64,
        rich_message: InputRichMessage,
        thread: Option<i32>,
        reply_to: Option<i32>,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendRichMessageParams::builder()
            .chat_id(chat_id)
            .rich_message(rich_message)
            .maybe_message_thread_id(thread)
            .maybe_reply_parameters(
                reply_to.map(|id| ReplyParameters::builder().message_id(id).build()),
            )
            .maybe_reply_markup(markup.map(ReplyMarkup::InlineKeyboardMarkup))
            .build();
        let resp = with_retry_text(|| self.bot.send_rich_message(&params)).await?;
        Ok(resp.result)
    }

    /// Channel publication send: one bounded attempt, with no 429 retry.
    pub(crate) async fn send_rich_content_once(
        &self,
        chat_id: i64,
        rich_message: InputRichMessage,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendRichMessageParams::builder()
            .chat_id(chat_id)
            .rich_message(rich_message)
            .maybe_message_thread_id(thread)
            .build();
        let resp = tokio::time::timeout(TELEGRAM_TEXT_TIMEOUT, self.bot.send_rich_message(&params))
            .await
            .map_err(|_| TgError::Timeout(TELEGRAM_TEXT_TIMEOUT))??;
        Ok(resp.result)
    }
    /// Send one typed rich-media message. Every attempt is bounded; ordinary
    /// delivery retries Telegram 429 responses, while channel delivery makes a
    /// single attempt because an ambiguous result must not be duplicated.
    pub(crate) async fn send_rich_media(
        &self,
        chat_id: i64,
        rich_message: InputRichMessage,
        thread: Option<i32>,
        retry_429: bool,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendRichMessageParams::builder()
            .chat_id(chat_id)
            .rich_message(rich_message)
            .maybe_message_thread_id(thread)
            .build();
        let resp = if retry_429 {
            with_retry_bounded(
                || self.bot.send_rich_message(&params),
                Some(TELEGRAM_MEDIA_TIMEOUT),
            )
            .await?
        } else {
            tokio::time::timeout(TELEGRAM_MEDIA_TIMEOUT, self.bot.send_rich_message(&params))
                .await
                .map_err(|_| TgError::Timeout(TELEGRAM_MEDIA_TIMEOUT))??
        };
        Ok(resp.result)
    }

    /// Plain channel fallback: one bounded, non-retried attempt.
    pub(crate) async fn send_message_once(
        &self,
        chat_id: i64,
        text: &str,
        thread: Option<i32>,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .maybe_message_thread_id(thread)
            .build();
        let resp = tokio::time::timeout(TELEGRAM_TEXT_TIMEOUT, self.bot.send_message(&params))
            .await
            .map_err(|_| TgError::Timeout(TELEGRAM_TEXT_TIMEOUT))??;
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
        with_retry_text(|| self.bot.edit_message_text(&params)).await?;
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
        with_retry_text(|| self.bot.edit_message_text(&params)).await?;
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
        // Text-class call: bound it like the other worker-path sends so a
        // stalled api.telegram.org cannot park the worker before a turn.
        tokio::time::timeout(TELEGRAM_TEXT_TIMEOUT, self.bot.send_chat_action(&params))
            .await
            .map_err(|_| TgError::Timeout(TELEGRAM_TEXT_TIMEOUT))??;
        Ok(())
    }

    // ---- media sends ------------------------------------------------------
    //
    // `media` is a `frankenstein::input_file::FileUpload`: pass a `PathBuf` for
    // a local-file upload (sandbox-outbox paths), or a `String` for a Telegram
    // `file_id`/URL. `&str` does NOT auto-convert.

    /// Send a photo, threaded, replying, without a caption.
    pub(crate) async fn send_photo(
        &self,
        chat_id: i64,
        media: FileUpload,
        thread: Option<i32>,
        reply_to: Option<i32>,
        retry_429: bool,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendPhotoParams::builder()
            .chat_id(chat_id)
            .photo(media)
            .maybe_message_thread_id(thread)
            .maybe_reply_parameters(
                reply_to.map(|r| ReplyParameters::builder().message_id(r).build()),
            )
            .build();
        let resp = with_optional_retry(|| self.bot.send_photo(&params), retry_429).await?;
        Ok(resp.result)
    }

    /// Send a document, threaded, replying, without a caption.
    pub(crate) async fn send_document(
        &self,
        chat_id: i64,
        media: FileUpload,
        thread: Option<i32>,
        reply_to: Option<i32>,
        retry_429: bool,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(media)
            .maybe_message_thread_id(thread)
            .maybe_reply_parameters(
                reply_to.map(|r| ReplyParameters::builder().message_id(r).build()),
            )
            .build();
        let resp = with_optional_retry(|| self.bot.send_document(&params), retry_429).await?;
        Ok(resp.result)
    }

    /// Send a video note (round video), threaded.
    pub(crate) async fn send_video_note(
        &self,
        chat_id: i64,
        media: FileUpload,
        thread: Option<i32>,
        retry_429: bool,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendVideoNoteParams::builder()
            .chat_id(chat_id)
            .video_note(media)
            .maybe_message_thread_id(thread)
            .build();
        let resp = with_optional_retry(|| self.bot.send_video_note(&params), retry_429).await?;
        Ok(resp.result)
    }

    /// Send a sticker, threaded.
    pub(crate) async fn send_sticker(
        &self,
        chat_id: i64,
        media: FileUpload,
        thread: Option<i32>,
        retry_429: bool,
    ) -> Result<Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendStickerParams::builder()
            .chat_id(chat_id)
            .sticker(media)
            .maybe_message_thread_id(thread)
            .build();
        let resp = with_optional_retry(|| self.bot.send_sticker(&params), retry_429).await?;
        Ok(resp.result)
    }

    /// Send a captionless media group (album).
    pub(crate) async fn send_media_group(
        &self,
        chat_id: i64,
        media: Vec<MediaGroupInputMedia>,
        thread: Option<i32>,
        retry_429: bool,
    ) -> Result<Vec<Message>, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendMediaGroupParams::builder()
            .chat_id(chat_id)
            .media(media)
            .maybe_message_thread_id(thread)
            .build();
        let resp = with_optional_retry(|| self.bot.send_media_group(&params), retry_429).await?;
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
    pub(crate) async fn send_photo_bytes(
        &self,
        chat_id: i64,
        bytes: &[u8],
        filename: &str,
        thread: Option<i32>,
        reply_to: Option<i32>,
    ) -> Result<Message, TgError> {
        let spool = SpooledUpload::new(bytes, filename).await?;
        let upload = FileUpload::InputFile(InputFile {
            path: spool.path().to_path_buf(),
        });
        self.send_photo(chat_id, upload, thread, reply_to, true)
            .await
        // `spool` (the TempDir) drops here, after the multipart upload completes.
    }

    /// Send an in-memory document (spooled to a temp file for the upload).
    pub(crate) async fn send_document_bytes(
        &self,
        chat_id: i64,
        bytes: &[u8],
        filename: &str,
        thread: Option<i32>,
        reply_to: Option<i32>,
    ) -> Result<Message, TgError> {
        let spool = SpooledUpload::new(bytes, filename).await?;
        let upload = FileUpload::InputFile(InputFile {
            path: spool.path().to_path_buf(),
        });
        self.send_document(chat_id, upload, thread, reply_to, true)
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

    #[tokio::test(start_paused = true)]
    async fn stalled_attempt_times_out_instead_of_pending_forever() {
        let calls = AtomicUsize::new(0);
        let result: Result<i32, TgError> = with_retry_bounded(
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                std::future::pending()
            },
            Some(Duration::from_millis(10)),
        )
        .await;
        assert!(matches!(result, Err(TgError::Timeout(_))));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a stalled attempt is terminal, not retried"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn slow_attempt_within_timeout_succeeds() {
        let result: Result<i32, TgError> = with_retry_bounded(
            || async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(7)
            },
            Some(Duration::from_secs(30)),
        )
        .await;
        assert!(matches!(result, Ok(7)));
    }
}

#[cfg(test)]
mod rich_content_tests {
    use super::*;
    use axum::{Router, body::Bytes, http::StatusCode, response::IntoResponse, routing::post};

    #[tokio::test]
    async fn riskoff_channel_post_uses_typed_rich_blocks_without_markdown_parsing() {
        let (body_tx, mut body_rx) = tokio::sync::mpsc::unbounded_channel();
        let app = Router::new().route("/botTEST-TOKEN/sendRichMessage", post(move |body: Bytes| {
            let body_tx = body_tx.clone();
            async move {
                body_tx.send(body).expect("capture request");
                (StatusCode::OK, r#"{"ok":true,"result":{"message_id":321,"date":0,"chat":{"id":-1001234567890,"type":"channel"}}}"#).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let bot = RightBot::new_for_test(format!("http://{address}/botTEST-TOKEN"));
        let content = right_rich_content::RichContent::literal("#HYPE ~$18.5M").unwrap();
        let outcome =
            crate::telegram::rich_content::send(&bot, -1001234567890, &content, None, None, None)
                .await;
        assert!(outcome.is_complete());
        assert_eq!(outcome.delivered[0].message_id, 321);
        let payload: serde_json::Value =
            serde_json::from_slice(&body_rx.recv().await.unwrap()).unwrap();
        assert!(payload["rich_message"].get("markdown").is_none());
        assert_eq!(payload["rich_message"]["blocks"][0]["type"], "paragraph");
        assert_eq!(
            payload["rich_message"]["blocks"][0]["text"],
            "#HYPE ~$18.5M"
        );
        assert!(payload.get("parse_mode").is_none());
        assert_eq!(
            payload["rich_message"]["skip_entity_detection"], true,
            "typed rich text must disable Telegram entity detection",
        );
    }

    #[tokio::test]
    async fn rich_split_replies_each_part_and_attaches_markup_only_to_last() {
        let (body_tx, mut body_rx) = tokio::sync::mpsc::unbounded_channel();
        let next_id = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(10));
        let app = Router::new().route(
            "/botTEST-TOKEN/sendRichMessage",
            post(move |body: Bytes| {
                let body_tx = body_tx.clone();
                let next_id = next_id.clone();
                async move {
                    body_tx.send(body).expect("capture request");
                    let id = next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    (
                        StatusCode::OK,
                        axum::Json(serde_json::json!({
                            "ok": true,
                            "result": {"message_id": id, "date": 0, "chat": {"id": 7, "type": "private"}}
                        })),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bot = RightBot::new_for_test(format!("http://{address}/botTEST-TOKEN"));
        let text = "界".repeat(20_000);
        let content: right_rich_content::RichContent = serde_json::from_value(serde_json::json!({
            "blocks": [
                {"type":"paragraph", "runs":[{"text": text}]},
                {"type":"paragraph", "runs":[{"text": text}]}
            ]
        }))
        .unwrap();
        let markup = InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![vec![
                InlineKeyboardButton::builder()
                    .text("Details")
                    .callback_data("errdet:9")
                    .build(),
            ]])
            .build();

        let outcome =
            crate::telegram::rich_content::send(&bot, 7, &content, None, Some(42), Some(markup))
                .await;

        assert!(outcome.is_complete());
        assert_eq!(
            outcome
                .delivered
                .iter()
                .map(|message| message.message_id)
                .collect::<Vec<_>>(),
            [10, 11]
        );
        let first: serde_json::Value =
            serde_json::from_slice(&body_rx.recv().await.unwrap()).unwrap();
        let second: serde_json::Value =
            serde_json::from_slice(&body_rx.recv().await.unwrap()).unwrap();
        assert_eq!(first["reply_parameters"]["message_id"], 42);
        assert_eq!(second["reply_parameters"]["message_id"], 42);
        assert!(first.get("reply_markup").is_none());
        assert_eq!(
            second["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
            "errdet:9"
        );
    }

    #[tokio::test]
    async fn deterministic_rich_rejection_falls_back_to_4096_character_parts() {
        let (body_tx, mut body_rx) = tokio::sync::mpsc::unbounded_channel();
        let app = Router::new()
            .route(
                "/botTEST-TOKEN/sendRichMessage",
                post(|| async {
                    (
                        StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({
                            "ok": false,
                            "error_code": 400,
                            "description": "Bad Request: unsupported rich block"
                        })),
                    )
                }),
            )
            .route(
                "/botTEST-TOKEN/sendMessage",
                post(move |body: Bytes| {
                    let body_tx = body_tx.clone();
                    async move {
                        body_tx.send(body).expect("capture fallback");
                        (
                            StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "ok": true,
                                "result": {"message_id": 77, "date": 0, "chat": {"id": 7, "type": "private"}}
                            })),
                        )
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bot = RightBot::new_for_test(format!("http://{address}/botTEST-TOKEN"));
        let content = right_rich_content::RichContent::literal("🦀".repeat(4_097)).unwrap();

        let outcome =
            crate::telegram::rich_content::send(&bot, 7, &content, None, Some(42), None).await;
        // 4,097 astral emoji = 8,194 UTF-16 units → three chunks bounded at
        // 4,096 units (2,048 emoji) each.
        assert!(outcome.is_complete());
        assert_eq!(outcome.delivered.len(), 3);
        let first: serde_json::Value =
            serde_json::from_slice(&body_rx.recv().await.unwrap()).unwrap();
        let second: serde_json::Value =
            serde_json::from_slice(&body_rx.recv().await.unwrap()).unwrap();
        let third: serde_json::Value =
            serde_json::from_slice(&body_rx.recv().await.unwrap()).unwrap();
        // Plain fallback chunks are bounded in UTF-16 units: 4,096 units of
        // astral emoji is 2,048 scalars.
        assert_eq!(
            first["text"].as_str().unwrap().encode_utf16().count(),
            4_096
        );
        assert_eq!(
            second["text"].as_str().unwrap().encode_utf16().count(),
            4_096
        );
        assert_eq!(third["text"].as_str().unwrap(), "🦀");
        assert_eq!(first["reply_parameters"]["message_id"], 42);
        assert_eq!(third["reply_parameters"]["message_id"], 42);
    }
}

#[cfg(test)]
mod stalled_api_tests {
    use super::*;

    /// Regression: 2026-07-19 riskoff worker wedge. A blackholed
    /// api.telegram.org (TCP accepted, response never sent) parked the
    /// worker's thinking-anchor send for frankenstein's 500s client default,
    /// before the CC turn deadline was even armed — the chat queue wedged
    /// until the bot was restarted. Text-class sends must abort promptly.
    async fn stall_server() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stall server");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                // Hold the connection open without ever answering.
                std::mem::forget(stream);
            }
        });
        addr
    }

    #[tokio::test(start_paused = true)]
    async fn send_message_opts_aborts_when_api_stalls() {
        let addr = stall_server().await;
        let bot = RightBot::new_for_test(format!("http://{addr}/botTEST-TOKEN"));
        tokio::task::yield_now().await;
        let err = bot
            .send_message_opts(1, "hi", false, None, None, None)
            .await
            .expect_err("a stalled Telegram API must not pend the send forever");
        assert!(
            matches!(err, TgError::Timeout(_) | TgError::Api(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn send_chat_action_aborts_when_api_stalls() {
        let addr = stall_server().await;
        let bot = RightBot::new_for_test(format!("http://{addr}/botTEST-TOKEN"));
        tokio::task::yield_now().await;
        let err = bot
            .send_chat_action(1, ChatAction::Typing, None)
            .await
            .expect_err("a stalled Telegram API must not pend the chat action forever");
        assert!(
            matches!(err, TgError::Timeout(_) | TgError::Api(_)),
            "got: {err:?}"
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

#[cfg(test)]
mod telegram_token_validation_tests {
    use super::*;
    use axum::Router;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::post;

    const TEST_TOKEN: &str = "123456:FAKE_SENTINEL_TOKEN_DO_NOT_LEAK";

    async fn mock_telegram_api(status: StatusCode, body: &'static str) -> String {
        let path = format!("/bot{TEST_TOKEN}/getMe");
        let app = Router::new().route(
            &path,
            post(move || async move { (status, body).into_response() }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Telegram API");
        let address = listener.local_addr().expect("read mock API address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock Telegram API");
        });
        format!("http://{address}")
    }
    async fn stalled_mock_telegram_api() -> String {
        let path = format!("/bot{TEST_TOKEN}/getMe");
        let app = Router::new().route(
            &path,
            post(|| async { std::future::pending::<StatusCode>().await }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stalled mock Telegram API");
        let address = listener.local_addr().expect("read mock API address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve stalled mock Telegram API");
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn telegram_token_live_validation_accepts_get_me_success() {
        let base = mock_telegram_api(
            StatusCode::OK,
            r#"{"ok":true,"result":{"id":1,"is_bot":true,"first_name":"Right","username":"right_test_bot"}}"#,
        )
        .await;

        validate_telegram_token_live_with_api_base(TEST_TOKEN, &base)
            .await
            .expect("valid getMe response must pass authentication validation");
    }

    #[tokio::test]
    async fn telegram_token_live_validation_redacts_unauthorized_response() {
        const SECRET_BODY: &str = "SECRET_UPSTREAM_BODY_DO_NOT_LEAK";
        let base = mock_telegram_api(
            StatusCode::UNAUTHORIZED,
            r#"{"ok":false,"error_code":401,"description":"SECRET_UPSTREAM_BODY_DO_NOT_LEAK"}"#,
        )
        .await;

        let error = validate_telegram_token_live_with_api_base(TEST_TOKEN, &base)
            .await
            .expect_err("Telegram 401 must fail authentication validation");
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("Telegram authentication failed"));
        assert!(!diagnostic.contains(TEST_TOKEN));
        assert!(!diagnostic.contains(SECRET_BODY));
    }

    #[tokio::test]
    async fn telegram_token_live_validation_debug_never_contains_token_or_response_body() {
        const SECRET_BODY: &str = "SECRET_SERVER_FAILURE_DO_NOT_LEAK";
        let base = mock_telegram_api(StatusCode::INTERNAL_SERVER_ERROR, SECRET_BODY).await;

        let error = validate_telegram_token_live_with_api_base(TEST_TOKEN, &base)
            .await
            .expect_err("Telegram server failure must fail authentication validation");
        let debug = format!("{error:?}");
        assert!(debug.contains("Telegram authentication failed"));
        assert!(!debug.contains(TEST_TOKEN));
        assert!(!debug.contains(SECRET_BODY));
    }

    #[tokio::test]
    async fn telegram_token_live_validation_times_out_with_redacted_error() {
        let base = stalled_mock_telegram_api().await;

        let error = validate_telegram_token_live_with_api_base_and_timeout(
            TEST_TOKEN,
            &base,
            Duration::from_millis(10),
        )
        .await
        .expect_err("stalled Telegram getMe must time out");
        let diagnostic = format!("{error:#}");
        assert_eq!(diagnostic, "Telegram authentication validation timed out");
        assert!(!diagnostic.contains(TEST_TOKEN));
        assert!(!format!("{error:?}").contains(TEST_TOKEN));
    }
}
