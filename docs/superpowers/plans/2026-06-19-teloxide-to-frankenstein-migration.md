# teloxide → frankenstein Migration Implementation Plan (SP1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the teloxide framework with the frankenstein `0.50` Bot-API client in `crates/bot`, preserving all current behavior (commands, routing, webhook transport, throttle semantics) — no new user-facing features.

**Architecture:** Approach **A** from the spec. A `RightBot` wrapper owns frankenstein and centralizes the three cross-cutting concerns teloxide gave us (throttle, `get_me` cache, uniform error + parse-mode/thread defaults). The four framework layers teloxide provided are rebuilt by hand: an update **router** (replaces `Dispatcher`/`dptree`), a manual **command parser** (replaces the `BotCommands` derive), a **throttle** (replaces the `Throttle` adaptor), and an axum **webhook handler** (replaces the `axum_no_setup` listener + `ShutdownAware`). The new modules are built and unit-tested additively while teloxide is still present; the framework/type switch then lands as one red→green phase.

**Tech Stack:** Rust (edition 2024), `frankenstein = "0.50"` (feature `client-reqwest`, rustls built-in), `governor` (rate limiting), `axum` (already in tree), `serde_json`, `tokio`, `thiserror`.

**Reference:** Spec at `docs/superpowers/specs/2026-06-19-teloxide-to-frankenstein-migration-design.md`.

---

## Working agreement (read before starting)

- **Worktree.** Do this in a worktree under `.worktrees/` (per project rule — never do code work on the shared master checkout). Land via fast-forward push to `origin/master` at the end. See Task 0.1.
- **All cargo runs go through devenv:** `devenv shell -- cargo …`.
- **Recommended runner:** `cargo nextest run`. Doctests only under `cargo test --doc`.
- **Verification cadence:** targeted `-p bot` tests during the TDD loop; **one** full `cargo nextest run --workspace` + `cargo test --doc --workspace` at the very end (Phase 3). Do not run full-workspace after every edit.
- **FAIL FAST:** every error propagates (`?` / `return Err`). Convert `anyhow`/error→string with `{:#}`. Best-effort sites (command registration, `broadcast_to_chats`, archive, memory-alert broadcast) keep their current log-and-continue behavior — do not change it.
- **Phase 2 is a single red→green unit.** From Task 2.1 until Task 2.9 the crate will not compile (flipping the `BotType` alias breaks every consumer at once). Make WIP commits if you like, but the green gate and the "real" commit is Task 2.9. This is expected for a framework swap.
- **frankenstein API confirmation:** the code blocks use frankenstein's conventional one-to-one mapping (method = snake_case Bot-API method; params = `<Method>Params::builder()…build()`). Task 0.2 confirms exact field names against `cargo doc -p frankenstein`; if a field name differs, adjust the wrapper in Phase 1 (only the wrapper knows frankenstein — call sites use `RightBot`).

---

## File Structure

**New files (Phase 1, additive):**
- `crates/bot/src/telegram/tg_bot.rs` — `RightBot` wrapper, `TgError`, `Throttle`. The only module that imports `frankenstein::client_reqwest`.
- `crates/bot/src/telegram/command.rs` — `BotCommand` enum + `parse()` (replaces the `BotCommands` derive).
- `crates/bot/src/telegram/router.rs` — `HandlerCtx`, pure routing-decision fns, and (Phase 2) `route_update`.

**Rewritten in place (Phase 2):**
- `bot.rs` — `build_bot` returns `RightBot`.
- `dispatch.rs` — dispatcher removed; `run_telegram` split into setup + lifecycle; routing logic moves to `router.rs`.
- `webhook.rs` — listener replaced by an axum handler that routes.
- `mod.rs` — `BotType` alias → `RightBot`; `broadcast_to_chats` concrete.
- Type sweep: `mention.rs`, `session.rs`, `filter.rs`, `attachments.rs`, `archive.rs`, `handler.rs`, `model_command.rs`, `mode_command.rs`, `debug_command.rs`, `allowlist_commands.rs`, `error_details.rs`, `worker.rs`, `progress.rs`, `async_delivery.rs`, `memory_alerts.rs`, `bootstrap_photo.rs`, `attachments.rs`, plus `sandbox_runtime.rs`/`sandbox_runtime_tests.rs`/`sync.rs` where they touch teloxide types.

**Deleted (Phase 2):**
- `shutdown_listener.rs` (+ its tests) — the `ShutdownAware` stream hack is unnecessary without a listener stream.

**Touched outside `crates/bot` (Phase 3, comments only):**
- `crates/right-codegen/src/agent_def.rs:25`, `crates/right-agent/src/init.rs:868`, `crates/right/src/main.rs:763`.

---

## Type migration cheat-sheet (apply throughout Phase 2)

| teloxide | frankenstein |
|---|---|
| `msg.text()` / `msg.caption()` (methods) | `message.text` / `message.caption` (`Option<String>` fields) |
| `msg.entities()` / `caption_entities()` / `media_group_id()` / `forward_origin()` | same-named fields |
| `msg.chat.id.0` (`ChatId(i64)`) | `message.chat.id` (`i64`) |
| `user.id.0` (`UserId(u64)`) | `user.id` (`u64`) |
| `MessageId(i32)` | `i32` (`message.message_id`) |
| `ThreadId(i32)` / `message_thread_id` | `Option<i32>` |
| `teloxide::types::ChatId(id)` (send) | `frankenstein::types::ChatId::Integer(id)` |
| `ParseMode::Html` | `frankenstein::ParseMode::Html` |
| `InlineKeyboardButton::callback(t, d)` | `InlineKeyboardButton::builder().text(t).callback_data(d).build()` → confirm in 0.2 |
| `InlineKeyboardMarkup::new(rows)` | `InlineKeyboardMarkup::builder().inline_keyboard(rows).build()` → wrap in `ReplyMarkup::InlineKeyboardMarkup(..)` |
| `teloxide::RequestError` | `crate::telegram::tg_bot::TgError` |
| `msg.chat.kind` `ChatKind`/`PublicChatKind` | `message.chat.type_field` (`frankenstein::ChatType`) — see Task 0.2 |
| `teloxide::types::CallbackQuery` | `frankenstein::types::CallbackQuery` (`.data: Option<String>`, `.id: String`) |
| `teloxide::types::Update` | `frankenstein::Update` (`.update_id`, `.content: UpdateContent`) |

---

# Phase 0 — Setup & API grounding

### Task 0.1: Worktree + baseline

**Files:** none (environment).

- [ ] **Step 1: Create the worktree**

```bash
cd /Users/developer/dev/rightclaw
git worktree add .worktrees/frankenstein-migration -b feat/frankenstein-migration master
cd .worktrees/frankenstein-migration
```

- [ ] **Step 2: Baseline test run (record pre-existing failures)**

Run: `devenv shell -- cargo nextest run -p bot`
Expected: PASS (or note any pre-existing flakes — see memory: cc/invocation pid race + dashboard warn-count can flake under load; re-run isolated before blaming your change).

- [ ] **Step 3: Confirm current teloxide version compiles**

Run: `devenv shell -- cargo build -p bot`
Expected: compiles clean.

---

### Task 0.2: Add dependencies (coexist with teloxide) + confirm frankenstein API

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)
- Modify: `crates/bot/Cargo.toml`

- [ ] **Step 1: Add frankenstein + governor to workspace deps**

In root `Cargo.toml` `[workspace.dependencies]`, alongside the existing `teloxide` line, add:

```toml
frankenstein = { version = "0.50", features = ["client-reqwest"] }
governor = "0.10"
```

(Confirm the latest `governor` version from the registry; `0.10` is the expected major. rustls needs no feature — frankenstein's reqwest client enables it for non-wasm targets.)

- [ ] **Step 2: Reference them from the bot crate**

In `crates/bot/Cargo.toml`, keep the `teloxide` line for now and add:

```toml
frankenstein = { workspace = true }
governor = { workspace = true }
```

- [ ] **Step 3: Verify both libraries coexist**

Run: `devenv shell -- cargo build -p bot`
Expected: compiles (teloxide + frankenstein both present, frankenstein unused so far — a dead-dependency warning is fine).

- [ ] **Step 4: Confirm the exact frankenstein API surface (record notes inline in `tg_bot.rs` later)**

Run: `devenv shell -- cargo doc -p frankenstein --no-deps --open` (or browse https://docs.rs/frankenstein/0.50.0). Confirm and note:
- `frankenstein::client_reqwest::Bot` — `Bot::new(token)` and `Bot::builder().api_url(..).client(..).build()`.
- `frankenstein::AsyncTelegramApi` trait method names: `get_me`, `send_message`, `edit_message_text`, `answer_callback_query`, `delete_message`, `send_photo`/`send_voice`/`send_video`/`send_video_note`/`send_sticker`/`send_media_group`/`send_document`/`send_audio`/`send_animation`, `send_chat_action`, `get_file`, `set_webhook`, `set_my_commands`, `delete_my_commands`, `close_forum_topic`/`edit_forum_topic`/`reopen_forum_topic`.
- Param builder field names for `SendMessageParams`, `EditMessageTextParams`, `AnswerCallbackQueryParams`, `SetWebhookParams` (esp. `chat_id: ChatId`, `message_id: i32`, `text`, `parse_mode`, `reply_markup: ReplyMarkup`, `reply_parameters`, `message_thread_id`, `caption`, `secret_token`, `allowed_updates`, `show_alert`).
- `frankenstein::Error` variants (`Api`, `HttpReqwest`, …) and where `retry_after` lives (likely `Error::Api(ErrorResponse { parameters: Some(ResponseParameters { retry_after, .. }), .. })`).
- `Message` chat-type access: `message.chat.type_field` of type `frankenstein::ChatType` (variants `Private`, `Group`, `Supergroup`, `Channel`).
- `get_file` → `File { file_path: Option<String> }`; download via `GET {api_url}/file/bot{token}/{file_path}`.

If any name differs from the cheat-sheet, the only place it matters is `tg_bot.rs` — adjust there.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/bot/Cargo.toml Cargo.lock
git commit -m "build(bot): add frankenstein + governor deps alongside teloxide"
```

---

# Phase 1 — New isolated modules (additive, fully TDD'd)

All of Phase 1 compiles with teloxide still present. These modules do not import teloxide.

### Task 1.1: `TgError`

**Files:**
- Create: `crates/bot/src/telegram/tg_bot.rs`
- Modify: `crates/bot/src/telegram/mod.rs` (add `mod tg_bot;`)

- [ ] **Step 1: Declare the module**

In `crates/bot/src/telegram/mod.rs`, add near the other `mod` lines:

```rust
pub(crate) mod tg_bot;
```

- [ ] **Step 2: Write the error type**

Create `crates/bot/src/telegram/tg_bot.rs`:

```rust
//! frankenstein-backed Telegram client wrapper. This is the ONLY module that
//! imports `frankenstein::client_reqwest`. It centralizes throttling, the
//! cached `get_me`, and uniform error + parse-mode/thread-id defaults so call
//! sites use a small purpose-built surface (`RightBot`) instead of teloxide.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TgError {
    #[error("telegram api: {0:#}")]
    Api(#[from] frankenstein::Error),
    #[error("file download: {0:#}")]
    Download(#[from] reqwest::Error),
    #[error("{0}")]
    Other(String),
}
```

- [ ] **Step 3: Compile**

Run: `devenv shell -- cargo build -p bot`
Expected: compiles (note: `reqwest` is pulled in transitively via frankenstein's `client-reqwest`; if `reqwest` is not directly referenceable, add `reqwest = { workspace = true }` to `crates/bot/Cargo.toml` aligning version `0.13` + `rustls-tls`/`stream` features, then re-run).

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/tg_bot.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): add TgError for frankenstein client wrapper"
```

---

### Task 1.2: `Throttle`

**Files:**
- Modify: `crates/bot/src/telegram/tg_bot.rs`

- [ ] **Step 1: Write failing tests**

Append to `crates/bot/src/telegram/tg_bot.rs`:

```rust
#[cfg(test)]
mod throttle_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test(start_paused = true)]
    async fn per_chat_gate_spaces_same_chat_sends() {
        let t = Throttle::new(30, Duration::from_millis(1000));
        let start = Instant::now();
        t.acquire(100).await; // first is immediate
        t.acquire(100).await; // second must wait ~the per-chat interval
        assert!(start.elapsed() >= Duration::from_millis(1000));
    }

    #[tokio::test(start_paused = true)]
    async fn different_chats_do_not_block_each_other_on_per_chat_gate() {
        let t = Throttle::new(30, Duration::from_millis(1000));
        let start = Instant::now();
        t.acquire(1).await;
        t.acquire(2).await; // different chat: per-chat gate does not delay
        assert!(start.elapsed() < Duration::from_millis(1000));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `devenv shell -- cargo nextest run -p bot throttle_tests`
Expected: FAIL ("cannot find type `Throttle`").

- [ ] **Step 3: Implement `Throttle`**

Add to `crates/bot/src/telegram/tg_bot.rs` (above the tests):

```rust
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use governor::{Quota, RateLimiter};
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};

/// Global + per-chat outbound rate gate, approximating teloxide's `Limits`.
/// Global: `global_per_sec` messages/second across all chats.
/// Per-chat: at most one message per `per_chat_interval` to a given chat.
pub struct Throttle {
    global: RateLimiter<NotKeyed, InMemoryState, DefaultClock>,
    per_chat_interval: Duration,
    last_per_chat: DashMap<i64, Instant>,
}

impl Throttle {
    pub fn new(global_per_sec: u32, per_chat_interval: Duration) -> Self {
        let quota = Quota::per_second(
            NonZeroU32::new(global_per_sec).expect("global_per_sec must be > 0"),
        );
        Self {
            global: RateLimiter::direct(quota),
            per_chat_interval,
            last_per_chat: DashMap::new(),
        }
    }

    /// Block until it is permissible to send to `chat_id`.
    pub async fn acquire(&self, chat_id: i64) {
        // Per-chat spacing.
        loop {
            let now = Instant::now();
            let wait = {
                match self.last_per_chat.get(&chat_id) {
                    Some(prev) => {
                        let elapsed = now.duration_since(*prev);
                        self.per_chat_interval.checked_sub(elapsed)
                    }
                    None => None,
                }
            };
            match wait {
                Some(d) if !d.is_zero() => tokio::time::sleep(d).await,
                _ => break,
            }
        }
        self.last_per_chat.insert(chat_id, Instant::now());
        // Global cap.
        self.global.until_ready().await;
    }
}
```

Telegram default limits (match teloxide): `Throttle::new(30, Duration::from_millis(1000))` — ~30 msg/s global, ~1 msg/s/chat.

- [ ] **Step 4: Run tests**

Run: `devenv shell -- cargo nextest run -p bot throttle_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/tg_bot.rs
git commit -m "feat(bot): add Throttle (global + per-chat rate gate) for RightBot"
```

---

### Task 1.3: `RightBot` wrapper

**Files:**
- Modify: `crates/bot/src/telegram/tg_bot.rs`

- [ ] **Step 1: Implement the wrapper struct + constructor + me-cache**

Add to `crates/bot/src/telegram/tg_bot.rs`:

```rust
use frankenstein::AsyncTelegramApi;
use frankenstein::client_reqwest::Bot as FBot;
use frankenstein::types::{
    ChatId, InlineKeyboardMarkup, ReplyMarkup, ReplyParameters, User,
};
use frankenstein::ParseMode;

#[derive(Clone)]
pub struct RightBot {
    bot: FBot,
    token: Arc<String>,
    me: Arc<User>,
    rate: Arc<Throttle>,
}

impl RightBot {
    /// Construct and resolve identity (`get_me`) once — replaces CacheMe.
    pub async fn connect(token: String) -> Result<Self, TgError> {
        let bot = FBot::new(&token);
        let me = bot.get_me().await?.result;
        Ok(Self {
            bot,
            token: Arc::new(token),
            me: Arc::new(me),
            rate: Arc::new(Throttle::new(30, Duration::from_millis(1000))),
        })
    }

    pub fn me(&self) -> &User {
        &self.me
    }
}
```

(`get_me().await?.result` — frankenstein wraps responses in `MethodResponse { result, .. }`. Confirm the `.result` access in Task 0.2.)

- [ ] **Step 2: Add the send/edit/answer methods**

Append impl methods (build params, throttle, propagate). Field names per Task 0.2:

```rust
impl RightBot {
    pub async fn send_html(
        &self,
        chat_id: i64,
        thread: Option<i32>,
        text: &str,
        reply_to: Option<i32>,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<frankenstein::types::Message, TgError> {
        self.rate.acquire(chat_id).await;
        let mut b = frankenstein::methods::SendMessageParams::builder()
            .chat_id(ChatId::Integer(chat_id))
            .text(text.to_string())
            .parse_mode(ParseMode::Html);
        if let Some(t) = thread {
            b = b.message_thread_id(t);
        }
        if let Some(r) = reply_to {
            b = b.reply_parameters(ReplyParameters::builder().message_id(r).build());
        }
        if let Some(m) = markup {
            b = b.reply_markup(ReplyMarkup::InlineKeyboardMarkup(m));
        }
        Ok(self.with_retry(self.bot.send_message(&b.build())).await?.result)
    }

    pub async fn send_text(
        &self,
        chat_id: i64,
        text: &str,
    ) -> Result<frankenstein::types::Message, TgError> {
        self.rate.acquire(chat_id).await;
        let params = frankenstein::methods::SendMessageParams::builder()
            .chat_id(ChatId::Integer(chat_id))
            .text(text.to_string())
            .build();
        Ok(self.with_retry(self.bot.send_message(&params)).await?.result)
    }

    pub async fn edit_html(
        &self,
        chat_id: i64,
        message_id: i32,
        text: &str,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<(), TgError> {
        self.rate.acquire(chat_id).await;
        let mut b = frankenstein::methods::EditMessageTextParams::builder()
            .chat_id(ChatId::Integer(chat_id))
            .message_id(message_id)
            .text(text.to_string())
            .parse_mode(ParseMode::Html);
        if let Some(m) = markup {
            b = b.reply_markup(m); // edit takes InlineKeyboardMarkup directly; confirm in 0.2
        }
        self.with_retry(self.bot.edit_message_text(&b.build())).await?;
        Ok(())
    }

    pub async fn answer_callback(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
        show_alert: bool,
    ) -> Result<(), TgError> {
        let mut b = frankenstein::methods::AnswerCallbackQueryParams::builder()
            .callback_query_id(callback_query_id.to_string())
            .show_alert(show_alert);
        if let Some(t) = text {
            b = b.text(t.to_string());
        }
        self.bot.answer_callback_query(&b.build()).await?;
        Ok(())
    }

    pub async fn delete_message(&self, chat_id: i64, message_id: i32) -> Result<(), TgError> {
        let params = frankenstein::methods::DeleteMessageParams::builder()
            .chat_id(ChatId::Integer(chat_id))
            .message_id(message_id)
            .build();
        self.bot.delete_message(&params).await?;
        Ok(())
    }
}
```

- [ ] **Step 3: Add the 429-retry helper**

```rust
impl RightBot {
    /// Await a frankenstein call; on a 429/flood error carrying `retry_after`,
    /// sleep that long and retry exactly once.
    async fn with_retry<F, Fut, T>(&self, _first: Fut) -> Result<T, TgError>
    where
        Fut: std::future::Future<Output = Result<T, frankenstein::Error>>,
    {
        // NOTE: futures are single-use; the real impl takes a closure `F: Fn() -> Fut`.
        unimplemented!("replaced in Step 4")
    }
}
```

This intermediate shape does not compile-pass logically; Step 4 fixes the signature to a closure (a future cannot be awaited twice). It is split out so the closure form is explicit.

- [ ] **Step 4: Fix `with_retry` to take a closure and wire retry_after**

Replace the Step-3 stub with:

```rust
impl RightBot {
    async fn with_retry<F, Fut, T>(&self, call: F) -> Result<T, TgError>
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
}

/// Extract `retry_after` (seconds) from a frankenstein API error, if present.
fn retry_after_secs(e: &frankenstein::Error) -> Option<u64> {
    // Confirm exact shape in Task 0.2; expected:
    // frankenstein::Error::Api(ErrorResponse { parameters: Some(ResponseParameters { retry_after: Some(n), .. }), .. })
    if let frankenstein::Error::Api(resp) = e {
        if let Some(params) = &resp.parameters {
            return params.retry_after.map(|n| n as u64);
        }
    }
    None
}
```

Then update the send/edit call sites to pass a closure, e.g.:

```rust
Ok(self.with_retry(|| self.bot.send_message(&b.build())).await?.result)
```

(Build the params once before the closure if the builder is consumed; e.g. `let params = b.build();` then `self.with_retry(|| self.bot.send_message(&params))`.)

- [ ] **Step 5: Add media, file-download, forum, webhook, and command methods**

```rust
impl RightBot {
    pub async fn download_file(&self, file_id: &str, dest: &std::path::Path) -> Result<(), TgError> {
        let f = self
            .bot
            .get_file(&frankenstein::methods::GetFileParams::builder().file_id(file_id.to_string()).build())
            .await?
            .result;
        let path = f.file_path.ok_or_else(|| TgError::Other("get_file returned no file_path".into()))?;
        let url = format!("https://api.telegram.org/file/bot{}/{}", self.token, path);
        let bytes = reqwest::get(&url).await?.error_for_status()?.bytes().await?;
        tokio::fs::write(dest, &bytes).await.map_err(|e| TgError::Other(format!("{e:#}")))?;
        Ok(())
    }

    pub async fn set_webhook(&self, url: &str, secret: &str, allowed: Vec<frankenstein::types::AllowedUpdate>) -> Result<(), TgError> {
        let params = frankenstein::methods::SetWebhookParams::builder()
            .url(url.to_string())
            .secret_token(secret.to_string())
            .allowed_updates(allowed)
            .build();
        self.bot.set_webhook(&params).await?;
        Ok(())
    }

    pub async fn set_my_commands(
        &self,
        commands: Vec<frankenstein::types::BotCommand>,
        scope: Option<frankenstein::types::BotCommandScope>,
        language_code: Option<String>,
    ) -> Result<(), TgError> {
        let mut b = frankenstein::methods::SetMyCommandsParams::builder().commands(commands);
        if let Some(s) = scope { b = b.scope(s); }
        if let Some(l) = language_code { b = b.language_code(l); }
        self.bot.set_my_commands(&b.build()).await?;
        Ok(())
    }

    pub async fn delete_my_commands(
        &self,
        scope: Option<frankenstein::types::BotCommandScope>,
        language_code: Option<String>,
    ) -> Result<(), TgError> {
        let mut b = frankenstein::methods::DeleteMyCommandsParams::builder();
        if let Some(s) = scope { b = b.scope(s); }
        if let Some(l) = language_code { b = b.language_code(l); }
        self.bot.delete_my_commands(&b.build()).await?;
        Ok(())
    }
}
```

Add `send_photo`, `send_voice`, `send_video`, `send_video_note`, `send_sticker`, `send_media_group`, `send_document`, `send_audio`, `send_animation`, `send_chat_action`, `close_forum_topic`, `edit_forum_topic`, `reopen_forum_topic` following the same pattern — read the current call sites in `worker.rs`/`progress.rs` (Phase 2) to fix each signature to exactly what the call site needs (caption, parse_mode, thread, reply markup). Each is mechanical.

- [ ] **Step 6: Build**

Run: `devenv shell -- cargo build -p bot`
Expected: compiles.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/tg_bot.rs crates/bot/Cargo.toml
git commit -m "feat(bot): RightBot frankenstein wrapper (send/edit/answer/download/commands/webhook)"
```

---

### Task 1.4: Command parser

**Files:**
- Create: `crates/bot/src/telegram/command.rs`
- Modify: `crates/bot/src/telegram/mod.rs` (`pub(crate) mod command;`)

- [ ] **Step 1: Write failing tests**

Create `crates/bot/src/telegram/command.rs`:

```rust
//! Manual Telegram command parser. Replaces teloxide's `BotCommands` derive.
//! Behavior preserved: lowercase command names; the renamed spellings
//! `set_focus` / `mode_group` / `allow_all` / `deny_all`; an optional
//! `@botusername` suffix is stripped; the payload is the remainder of the text.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotCommand {
    Start(String),
    New(String),
    List,
    Switch(String),
    Mcp(String),
    Providers(String),
    SetFocus(String),
    Doctor,
    Model,
    Mode,
    ModeGroup,
    Dashboard,
    Debug(String),
    Cron(String),
    Allow(String),
    Deny(String),
    Allowed,
    AllowAll,
    DenyAll,
    Usage(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_command() {
        assert_eq!(BotCommand::parse("/list", "right_bot"), Some(BotCommand::List));
    }

    #[test]
    fn parses_command_with_payload() {
        assert_eq!(
            BotCommand::parse("/usage detail", "right_bot"),
            Some(BotCommand::Usage("detail".into()))
        );
    }

    #[test]
    fn strips_at_botusername() {
        assert_eq!(
            BotCommand::parse("/doctor@right_bot", "right_bot"),
            Some(BotCommand::Doctor)
        );
    }

    #[test]
    fn parses_snake_case_renames() {
        assert_eq!(
            BotCommand::parse("/set_focus now", "right_bot"),
            Some(BotCommand::SetFocus("now".into()))
        );
        assert_eq!(BotCommand::parse("/mode_group", "right_bot"), Some(BotCommand::ModeGroup));
        assert_eq!(BotCommand::parse("/allow_all", "right_bot"), Some(BotCommand::AllowAll));
        assert_eq!(BotCommand::parse("/deny_all", "right_bot"), Some(BotCommand::DenyAll));
    }

    #[test]
    fn empty_payload_is_empty_string() {
        assert_eq!(BotCommand::parse("/new", "right_bot"), Some(BotCommand::New(String::new())));
    }

    #[test]
    fn non_command_is_none() {
        assert_eq!(BotCommand::parse("hello", "right_bot"), None);
        assert_eq!(BotCommand::parse("", "right_bot"), None);
    }

    #[test]
    fn wrong_at_target_still_parses_name() {
        // teloxide parses `/doctor@other_bot` as the command for THIS bot too;
        // preserve current behavior: name matched, @suffix stripped regardless.
        assert_eq!(BotCommand::parse("/doctor@other_bot", "right_bot"), Some(BotCommand::Doctor));
    }
}
```

(The `wrong_at_target` test pins current behavior. If you want strict `@botusername` matching, decide explicitly — but the spec says behavior-preserving, and teloxide's default for a single registered bot is lenient. Keep lenient.)

- [ ] **Step 2: Register module + run to verify failure**

Add `pub(crate) mod command;` to `mod.rs`.
Run: `devenv shell -- cargo nextest run -p bot command::tests`
Expected: FAIL ("no function `parse`").

- [ ] **Step 3: Implement `parse`**

Add to `crates/bot/src/telegram/command.rs`:

```rust
impl BotCommand {
    pub fn parse(text: &str, _bot_username: &str) -> Option<BotCommand> {
        let text = text.trim_start();
        let mut parts = text.splitn(2, char::is_whitespace);
        let head = parts.next()?;
        let payload = parts.next().unwrap_or("").trim().to_string();

        let head = head.strip_prefix('/')?;
        // Strip an optional @botusername suffix.
        let name = head.split('@').next().unwrap_or(head).to_ascii_lowercase();

        Some(match name.as_str() {
            "start" => BotCommand::Start(payload),
            "new" => BotCommand::New(payload),
            "list" => BotCommand::List,
            "switch" => BotCommand::Switch(payload),
            "mcp" => BotCommand::Mcp(payload),
            "providers" => BotCommand::Providers(payload),
            "set_focus" => BotCommand::SetFocus(payload),
            "doctor" => BotCommand::Doctor,
            "model" => BotCommand::Model,
            "mode" => BotCommand::Mode,
            "mode_group" => BotCommand::ModeGroup,
            "dashboard" => BotCommand::Dashboard,
            "debug" => BotCommand::Debug(payload),
            "cron" => BotCommand::Cron(payload),
            "allow" => BotCommand::Allow(payload),
            "deny" => BotCommand::Deny(payload),
            "allowed" => BotCommand::Allowed,
            "allow_all" => BotCommand::AllowAll,
            "deny_all" => BotCommand::DenyAll,
            "usage" => BotCommand::Usage(payload),
            _ => return None,
        })
    }

    /// The visible command list for `setMyCommands` (hides `usage`), as
    /// frankenstein `BotCommand` rows.
    pub fn visible() -> Vec<frankenstein::types::BotCommand> {
        // (name, description) pairs — descriptions copied verbatim from the
        // former #[command(description=...)] attributes in dispatch.rs.
        const ENTRIES: &[(&str, &str)] = &[
            ("start", "Start interacting with this agent"),
            ("new", "Start a new conversation"),
            ("list", "List all sessions"),
            ("switch", "Switch to another session"),
            ("mcp", "Open MCP dashboard"),
            ("providers", "Open providers dashboard"),
            ("set_focus", "Set the focus for this conversation"),
            ("doctor", "Run diagnostics"),
            ("model", "Switch Claude model (menu)"),
            ("mode", "Set response mode for this topic"),
            ("mode_group", "Set response mode for this group"),
            ("dashboard", "Open dashboard"),
            ("debug", "Toggle debug mode (on/off/status)"),
            ("cron", "Cron job status (list or detail)"),
            ("allow", "Add trusted user (reply to user, or /allow <user_id>)"),
            ("deny", "Remove trusted user"),
            ("allowed", "List trusted users and opened groups"),
            ("allow_all", "Open this group for all members (group only)"),
            ("deny_all", "Close this group (group only)"),
            // `usage` intentionally omitted (hidden command).
        ];
        ENTRIES
            .iter()
            .map(|(c, d)| {
                frankenstein::types::BotCommand::builder()
                    .command((*c).to_string())
                    .description((*d).to_string())
                    .build()
            })
            .collect()
    }
}
```

(Cross-check the descriptions against the current `#[command(description=...)]` strings in `dispatch.rs:41-95` and the `visible_bot_commands` hide-`usage` filter — copy them verbatim.)

- [ ] **Step 4: Run tests**

Run: `devenv shell -- cargo nextest run -p bot command::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/command.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): manual BotCommand parser replacing teloxide derive"
```

---

### Task 1.5: Router decision functions

**Files:**
- Create: `crates/bot/src/telegram/router.rs`
- Modify: `crates/bot/src/telegram/mod.rs` (`pub(crate) mod router;`)

Build the *pure* routing-decision logic now (unit-testable, no handler wiring). The `HandlerCtx` struct and the `route_update` glue that calls `handle_*` land in Phase 2 (Task 2.7), once handler signatures are migrated.

- [ ] **Step 1: Write failing tests for callback classification**

Create `crates/bot/src/telegram/router.rs`:

```rust
//! Update routing. Replaces teloxide's `Dispatcher`/`dptree`. The pure
//! classification fns here are unit-tested; `route_update` (Phase 2) maps an
//! `UpdateContent` to a handler call using `HandlerCtx`.

/// Which callback-query handler a callback's `data` routes to.
#[derive(Debug, PartialEq, Eq)]
pub enum CallbackRoute {
    Model,
    Mode,
    Thinking,
    Bg,
    ErrorDetails,
    Stop,
}

pub fn classify_callback(data: Option<&str>) -> CallbackRoute {
    match data {
        Some(d) if d.starts_with("model:") => CallbackRoute::Model,
        Some(d) if d.starts_with("mode:") || d.starts_with("modegroup:") => CallbackRoute::Mode,
        Some(d) if d.starts_with("think:") => CallbackRoute::Thinking,
        Some(d) if d.starts_with("bg:") => CallbackRoute::Bg,
        Some(d) if d.starts_with("errdet:") => CallbackRoute::ErrorDetails,
        _ => CallbackRoute::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_prefixes_route_correctly() {
        assert_eq!(classify_callback(Some("model:opus")), CallbackRoute::Model);
        assert_eq!(classify_callback(Some("mode:all")), CallbackRoute::Mode);
        assert_eq!(classify_callback(Some("modegroup:x")), CallbackRoute::Mode);
        assert_eq!(classify_callback(Some("think:on")), CallbackRoute::Thinking);
        assert_eq!(classify_callback(Some("bg:123")), CallbackRoute::Bg);
        assert_eq!(classify_callback(Some("errdet:1")), CallbackRoute::ErrorDetails);
        assert_eq!(classify_callback(Some("stop:1")), CallbackRoute::Stop);
        assert_eq!(classify_callback(None), CallbackRoute::Stop);
    }
}
```

- [ ] **Step 2: Register module + run**

Add `pub(crate) mod router;` to `mod.rs`.
Run: `devenv shell -- cargo nextest run -p bot router::tests`
Expected: PASS (the fn is implemented inline above — this task is mostly establishing the module + the classification contract the dispatcher tree currently encodes in `dispatch.rs:609-642`).

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/router.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): callback-route classification for the update router"
```

---

### Task 1.6: Webhook handler (secret + parse + ack)

**Files:**
- Modify: `crates/bot/src/telegram/router.rs` (add a pure helper) OR keep in `webhook.rs` during Phase 2.

The full axum handler needs `HandlerCtx` (Phase 2). Here, lock the two pure decisions with tests: secret comparison and the malformed-body ack policy.

- [ ] **Step 1: Write failing tests**

Append to `crates/bot/src/telegram/router.rs`:

```rust
/// The HTTP status a webhook POST should return.
#[derive(Debug, PartialEq, Eq)]
pub enum WebhookOutcome {
    Unauthorized,        // 401 — missing/wrong secret
    AckIgnore,           // 200 — malformed body; ack so Telegram stops retrying
    Routed,              // 200 — parsed and handed to route_update
}

pub fn webhook_outcome(secret_header: Option<&str>, expected_secret: &str, body_parses: bool) -> WebhookOutcome {
    match secret_header {
        Some(s) if s == expected_secret => {
            if body_parses { WebhookOutcome::Routed } else { WebhookOutcome::AckIgnore }
        }
        _ => WebhookOutcome::Unauthorized,
    }
}

#[cfg(test)]
mod webhook_tests {
    use super::*;

    #[test]
    fn missing_or_wrong_secret_is_unauthorized() {
        assert_eq!(webhook_outcome(None, "s", true), WebhookOutcome::Unauthorized);
        assert_eq!(webhook_outcome(Some("nope"), "s", true), WebhookOutcome::Unauthorized);
    }

    #[test]
    fn correct_secret_routes_or_acks() {
        assert_eq!(webhook_outcome(Some("s"), "s", true), WebhookOutcome::Routed);
        assert_eq!(webhook_outcome(Some("s"), "s", false), WebhookOutcome::AckIgnore);
    }
}
```

- [ ] **Step 2: Run**

Run: `devenv shell -- cargo nextest run -p bot router::webhook_tests`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/router.rs
git commit -m "feat(bot): webhook outcome decision (secret + malformed-body ack policy)"
```

---

# Phase 2 — The switch (single red→green unit)

> The crate will not compile from Task 2.1 until Task 2.9. Work top-to-bottom. WIP commits optional; the green gate is Task 2.9.

### Task 2.1: Flip the `BotType` alias + `broadcast_to_chats`

**Files:**
- Modify: `crates/bot/src/telegram/mod.rs:34-58`

- [ ] **Step 1: Replace the alias and the generic broadcast helper**

```rust
/// Bot client used by WorkerContext and routing. frankenstein-backed wrapper.
pub type BotType = tg_bot::RightBot;

/// Best-effort broadcast to chat IDs. Errors logged and swallowed.
pub(crate) async fn broadcast_to_chats(bot: &BotType, chat_ids: &[i64], text: &str) {
    for &chat_id in chat_ids {
        if let Err(e) = bot.send_text(chat_id, text).await {
            tracing::warn!(chat_id, "broadcast_to_chats send failed: {e:#}");
        }
    }
}
```

Update the two `broadcast_to_chats` call sites (memory_alerts + any other) to drop the generic bound — they already pass the real bot.

---

### Task 2.2: Type sweep — `mention.rs`, `session.rs`, `filter.rs`

**Files:**
- Modify: `crates/bot/src/telegram/mention.rs`, `session.rs`, `filter.rs` (+ their test modules)

- [ ] **Step 1: Rewrite field/type access using the cheat-sheet**

Apply: `msg.text()` → `message.text.as_deref()`; `msg.caption()` → `message.caption.as_deref()`; `msg.entities()`/`caption_entities()`/`media_group_id()`/`forward_origin()` → fields; `msg.chat.id.0` → `message.chat.id`; `sender.id.0 as i64` → `user.id as i64`; `ChatKind::Private(_)` match → `message.chat.type_field == ChatType::Private` (and the group/supergroup/channel arms via `ChatType`). Replace all `teloxide::types::*` imports with `frankenstein::types::*`.

- [ ] **Step 2: Port the test fixtures**

The `serde_json::from_value::<teloxide::types::Message>` fixtures become `frankenstein::Message` (the JSON is Bot-API-shaped and unchanged). Update assertion field access only. Keep every test name and assertion intent identical.

- [ ] **Step 3: (gate deferred to 2.9)** — these files reference handler/types not yet migrated; do not expect a clean compile until 2.9.

---

### Task 2.3: Type sweep — `attachments.rs`, `archive.rs`, `bootstrap_photo.rs`

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`, `archive.rs`, `bootstrap_photo.rs`

- [ ] **Step 1: attachments inbound**

Replace `best.file.id.0` → `best.file_id` (frankenstein `PhotoSize.file_id: String`; confirm each media type's id field name in Task 0.2). Replace the teloxide `bot.download_file(&file.path, &mut dst)` block (`attachments.rs:802-808`) with `bot.download_file(&att.file_id, &dest).await?`. Remove `use teloxide::net::Download`.

- [ ] **Step 2: archive + bootstrap_photo**

Port `Message` field access and any `ParseMode`/send calls to `RightBot` methods.

---

### Task 2.4: Type sweep — command handlers

**Files:**
- Modify: `model_command.rs`, `mode_command.rs`, `debug_command.rs`, `allowlist_commands.rs`, `error_details.rs`

- [ ] **Step 1: Migrate signatures from dptree injection to `&HandlerCtx`**

Each `handle_*` currently takes dptree-injected args (`bot: BotType`, `msg: Message`, plus DI deps) and returns `ResponseResult<()>`. Change to `async fn handle_x(ctx: &HandlerCtx, msg: &frankenstein::Message, args: …) -> Result<(), TgError>` (callbacks take `&frankenstein::CallbackQuery`). Replace keyboard construction (`InlineKeyboardButton::callback`, `InlineKeyboardMarkup::new`) per cheat-sheet; replace `bot.edit_message_text(...).await` / `bot.answer_callback_query(...)` with `ctx.bot.edit_html(...)` / `ctx.bot.answer_callback(...)`.

- [ ] **Step 2: Port the `model_command`/`mode_command` keyboard + callback tests** (`tests/model_command.rs` and inline tests) to frankenstein types; keep assertions identical.

---

### Task 2.5: Type sweep — `handler.rs`

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Migrate the DI dependency structs + handler signatures**

The DI marker structs (`AgentDir`, `AgentSettings`, `RightHome`, `InternalApi`, slots, etc.) move into `HandlerCtx` fields (defined in Task 2.7). Rewrite `handle_message`, `handle_start`, `handle_new`, `handle_list`, `handle_switch`, `handle_mcp`, `handle_providers`, `handle_set_focus`, `handle_doctor`, `handle_dashboard`, `handle_cron`, `handle_usage`, and the callback handlers to take `&HandlerCtx` + the message/query, returning `Result<(), TgError>`. Replace all teloxide types/sends.

---

### Task 2.6: Type sweep — `worker.rs`, `progress.rs`, `async_delivery.rs`, `memory_alerts.rs`

**Files:**
- Modify: `worker.rs`, `progress.rs`, `async_delivery.rs`, `memory_alerts.rs`

- [ ] **Step 1: Route every send/edit through `RightBot`**

Replace the funnel helpers' bodies first (`send_tg`, `send_tg_html`, `send_tg_inner`, `send_text_message`, `send_thinking_anchor`, `send_error_to_telegram*`, progress `send_text_message`, `deliver_through_session`) to call `RightBot` methods. Then fix the direct `bot.send_message(...).parse_mode()...await`, `bot.edit_message_text(...)`, and `bot.answer_callback_query(...)` sites to the matching `RightBot` method (chat-id as `i64`, thread as `Option<i32>`). Replace `teloxide::types::{ChatId, MessageId, ThreadId, ParseMode, InlineKeyboardMarkup, InlineKeyboardButton, CallbackQuery, ChatAction, ReplyParameters, CustomEmojiId, Rgb}` with frankenstein equivalents (confirm `CustomEmojiId`/`Rgb` usage in `progress.rs` — these are used for the reaction/forum-topic color path; map to frankenstein's types or plain values).

- [ ] **Step 2: `RightBot` clone usages** — `bot.clone()`, `bot.inner()` (worker.rs) — `RightBot` is `Clone`; remove `bot.inner()` (it returned the underlying teloxide `Bot`; replace its single use with the needed `RightBot` method or `me()`).

---

### Task 2.7: Router glue, webhook handler, `run_telegram` split, delete `shutdown_listener`

**Files:**
- Modify: `crates/bot/src/telegram/router.rs` (add `HandlerCtx` + `route_update`)
- Modify: `crates/bot/src/telegram/webhook.rs` (axum handler)
- Modify: `crates/bot/src/telegram/dispatch.rs` (`run_telegram` setup/lifecycle split; drop `update_listener: L`)
- Modify: `crates/bot/src/telegram/bot.rs` (`build_bot` → `RightBot::connect`)
- Delete: `crates/bot/src/telegram/shutdown_listener.rs`
- Modify: `crates/bot/src/telegram/mod.rs` (remove `mod shutdown_listener;`)

- [ ] **Step 1: Define `HandlerCtx` and `route_update`**

In `router.rs`, define `HandlerCtx` holding every Arc currently in `dispatch.rs`'s `dptree::deps![...]` (worker_map, agent_dir, pending_auth, home, ssh_config, intercept/token/auth-choice slots, internal_api, settings, idle_ts, identity, allowlist, worker_ctl) plus `bot: BotType`. Implement:

```rust
pub async fn route_update(update: frankenstein::Update, ctx: &HandlerCtx) {
    use frankenstein::updates::UpdateContent;
    match update.content {
        UpdateContent::Message(m) | UpdateContent::EditedMessage(m) => {
            super::dispatch::on_message(ctx, *m).await;
        }
        UpdateContent::CallbackQuery(q) => {
            super::dispatch::on_callback(ctx, *q).await;
        }
        _ => {}
    }
}
```

`on_message` reproduces `dispatch.rs`'s message branch: `pre_filter_log_meta` log + `archive_seen_group_message` → `make_routing_filter` → `BotCommand::parse(text, ctx.bot.me().username)` → matching `handle_*` (or `handle_message` with the `RoutingDecision`). `on_callback` uses `classify_callback(q.data.as_deref())` → the matching handler. All handler calls `.await` and log errors (best-effort; one failed update must not kill the server).

- [ ] **Step 2: Webhook axum handler**

Rewrite `webhook.rs`:

```rust
pub fn build_webhook_router(secret: String, ctx: std::sync::Arc<super::router::HandlerCtx>) -> axum::Router {
    use axum::{routing::post, extract::State, http::{HeaderMap, StatusCode}, body::Bytes};

    #[derive(Clone)]
    struct WState { secret: String, ctx: std::sync::Arc<super::router::HandlerCtx> }

    async fn handle(State(st): State<WState>, headers: HeaderMap, body: Bytes) -> StatusCode {
        let provided = headers.get("X-Telegram-Bot-Api-Secret-Token").and_then(|v| v.to_str().ok());
        let parsed = serde_json::from_slice::<frankenstein::Update>(&body);
        match super::router::webhook_outcome(provided, &st.secret, parsed.is_ok()) {
            super::router::WebhookOutcome::Unauthorized => StatusCode::UNAUTHORIZED,
            super::router::WebhookOutcome::AckIgnore => {
                tracing::warn!("webhook: unparseable update body, acking to stop retries");
                StatusCode::OK
            }
            super::router::WebhookOutcome::Routed => {
                super::router::route_update(parsed.expect("checked Ok"), &st.ctx).await;
                StatusCode::OK
            }
        }
    }

    axum::Router::new().route("/", post(handle)).with_state(WState { secret, ctx })
}

pub fn webhook_allowed_updates() -> Vec<frankenstein::types::AllowedUpdate> {
    use frankenstein::types::AllowedUpdate;
    vec![AllowedUpdate::Message, AllowedUpdate::EditedMessage, AllowedUpdate::CallbackQuery]
}
```

Port `webhook.rs`'s secret-rejection tests to drive this router (they already use `tower::ServiceExt::oneshot` — keep that shape).

- [ ] **Step 3: `build_bot` → `RightBot::connect`**

Rewrite `bot.rs` so `build_bot` is replaced by an `async fn` returning `RightBot` via `RightBot::connect(token).await`, or inline `RightBot::connect` at the single call site and delete `bot.rs`. Remove the `dispatcher_builds_without_panic` smoke test (no dptree to type-check) — router unit tests (Task 1.5) cover routing.

- [ ] **Step 4: Split `run_telegram`**

Refactor `dispatch.rs::run_telegram` to: build `RightBot` (`connect`), resolve identity from `bot.me()` (no second `get_me`), build `HandlerCtx`, register commands via `RightBot::set_my_commands`/`delete_my_commands` (port the three-scope + stale-scope cleanup loops), build the webhook router via `build_webhook_router`, and return it to `lib.rs`. Keep the signal-listener thread, `worker_shutdown` cancellation, and handoff-gate drain — but the `dispatch_with_listener(...).await` loop and the `update_listener: L` generic parameter are removed. The post-shutdown drain runs when the shutdown token fires.

- [ ] **Step 5: Delete `shutdown_listener.rs`** and its `mod` declaration.

---

### Task 2.8: `lib.rs` wiring

**Files:**
- Modify: `crates/bot/src/lib.rs` (around 596-607, 1148-1183)

- [ ] **Step 1: Build bot/ctx, nest the router, remove `update_listener`**

Replace the `build_webhook_router(...) -> (update_listener, _stop, router)` call (lib.rs:606) with the new flow: `run_telegram` (or a new `setup_telegram`) returns the `axum::Router`; nest it into the bot.sock UDS app under `/tg/<agent>` **before** spawning `axum_handle`; call `RightBot::set_webhook(url, secret, webhook_allowed_updates())`. Remove the `update_listener` argument at lib.rs:1175 and adjust the `tokio::select!` (lib.rs:1148-1179): the telegram arm now awaits the lifecycle/shutdown future instead of the dispatch loop. Keep the subsequent `shutdown.cancel()` + cron/delivery drain unchanged.

---

### Task 2.9: Green gate — remove teloxide, build, test

**Files:**
- Modify: `Cargo.toml`, `crates/bot/Cargo.toml`

- [ ] **Step 1: Remove the teloxide dependency**

Delete the `teloxide` line from `crates/bot/Cargo.toml` and `[workspace.dependencies]`.

- [ ] **Step 2: Build and fix residuals**

Run: `devenv shell -- cargo build -p bot`
Expected: iterate until clean. Common residuals: leftover `teloxide::` paths, `ResponseResult` return types, `.0` newtype access, `MessageId`/`ThreadId` wraps.

- [ ] **Step 3: Run the bot test suite**

Run: `devenv shell -- cargo nextest run -p bot`
Expected: PASS (all ported tests + new router/throttle/command/webhook tests).

- [ ] **Step 4: Clippy**

Run: `devenv shell -- cargo clippy -p bot --all-targets`
Expected: no new warnings (fix any introduced).

- [ ] **Step 5: Commit the switch**

```bash
git add -A
git commit -m "feat(bot)!: replace teloxide with frankenstein (behavior-preserving)"
```

---

# Phase 3 — Finalize

### Task 3.1: Stale comments outside the bot crate

**Files:**
- Modify: `crates/right-codegen/src/agent_def.rs:25`, `crates/right-agent/src/init.rs:868`, `crates/right/src/main.rs:763`

- [ ] **Step 1:** Reword the three teloxide references (drop "teloxide" / "long-polling, teloxide"; the init.rs assertion message should say "native Telegram bot" instead of "native teloxide bot"; main.rs doc comment "webhook" not "long-polling, teloxide").

- [ ] **Step 2: Commit**

```bash
git add crates/right-codegen/src/agent_def.rs crates/right-agent/src/init.rs crates/right/src/main.rs
git commit -m "docs: drop stale teloxide references after frankenstein migration"
```

---

### Task 3.2: Architecture-doc cite-on-touch

**Files:**
- Modify: any `docs/architecture/*.md` mentioning teloxide; `ARCHITECTURE.md` if affected.

- [ ] **Step 1: Find references**

Run: `rg -n 'teloxide|axum_no_setup|ShutdownAware|dptree|dispatch_with_listener' ARCHITECTURE.md docs/architecture PROMPT_SYSTEM.md`

- [ ] **Step 2:** Update the prompting/sessions/lifecycle satellites that narrate the dispatcher/listener to describe the frankenstein router + axum webhook handler. Keep ARCHITECTURE.md under its 40k budget (prefer satellites). If no contract/invariant changed, only the descriptive satellites need edits.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "docs(architecture): update dispatch/webhook narration for frankenstein"
```

---

### Task 3.3: Final full-workspace verification

**Files:** none.

- [ ] **Step 1: Full workspace tests**

Run: `devenv shell -- cargo nextest run --workspace`
Expected: PASS (re-run any known flakes isolated before concluding).

- [ ] **Step 2: Doctests**

Run: `devenv shell -- cargo test --doc --workspace`
Expected: PASS.

- [ ] **Step 3: Full build**

Run: `devenv shell -- cargo build --workspace`
Expected: clean.

- [ ] **Step 4:** If all green, the branch is ready. Land per project workflow: rebase on `master`, fast-forward push to `origin/master` (no new long-lived branch needed). Then `git worktree remove .worktrees/frankenstein-migration`.

---

## Self-review notes (author)

- **Spec coverage:** Deps/feature swap (0.2, 2.9) ✓; `RightBot` + me-cache + methods (1.3) ✓; throttle incl. 429-retry (1.2, 1.3) ✓; router + HandlerCtx (1.5, 2.7) ✓; command parser (1.4) ✓; webhook handler + ack policy + run_telegram/lib.rs split + delete ShutdownAware (1.6, 2.7, 2.8) ✓; type-migration table applied (2.2–2.6) ✓; error handling/FAIL-FAST + concrete broadcast (1.1, 2.1, 2.6) ✓; testing (every new module + ported suites) ✓; out-of-scope/upgrade invariants honored (no setWebhook/secret/allowed_updates/scope changes) ✓; the six open questions are resolved or pinned to Task 0.2.
- **Known coarse task:** Phase 2 is intentionally a single red→green unit (alias flip is atomic). Sub-tasks 2.2–2.8 are ordered edit checkpoints, not independent green commits — this is the honest shape of a framework swap, not a placeholder.
- **frankenstein API field names** in the wrapper/handlers are frankenstein's conventional `*Params::builder()` mapping; Task 0.2 confirms them against `cargo doc` and any drift is fixed in `tg_bot.rs` only.
