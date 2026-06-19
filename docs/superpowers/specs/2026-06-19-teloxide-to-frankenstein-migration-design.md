# teloxide → frankenstein migration (SP1: behavior-preserving swap)

**Date:** 2026-06-19
**Status:** Design approved; ready for implementation plan
**Scope:** `crates/bot` only (plus 3 stale comments outside it)

## Motivation

teloxide `0.17` lags the Telegram Bot API by many versions (≈ Bot API 7.x,
mid-2025). The driving product goal is **Rich Messages** — Markdown tables and
streamed AI replies — introduced in **Bot API 10.1** (2026-06-11):
`sendRichMessage`, `sendRichMessageDraft` (streaming), an `editMessageText`
rich-message parameter, and the `RichBlock` / `RichBlockTable` / `RichText*`
type family.

frankenstein tracks the Bot API closely (current release `0.50.0`, 2026-05-26,
= Bot API 10.0; Bot API 10.1 is in-flight upstream as PR
[ayrat555/frankenstein#325](https://github.com/ayrat555/frankenstein/pull/325),
opened 2026-06-14 by the maintainer). teloxide has no 10.1 support and is
unlikely to gain it quickly.

### Decomposition (decided)

The work splits into two sub-projects with different blockers:

- **SP1 — Framework migration (this spec).** Replace teloxide with frankenstein
  `0.50` (Bot API 10.0). **Behavior-preserving — no new user-facing features.**
  Acceptance = feature parity. No dependency on Bot API 10.1.
- **SP2 — Rich Messages (separate follow-on spec).** Markdown→`RichBlock`
  conversion, table rendering, and the streaming-edit loop. Gated on frankenstein
  10.1 (PR #325 released or branch-pinned). Out of scope here.

SP1 is the prerequisite and the bulk of the work. It de-risks by moving us onto a
fast-tracking client now; SP2 lands the moment #325 ships. SP1's send seam is
designed so SP2's `send_rich` / `stream_rich` methods drop in without touching
call sites.

## Current teloxide footprint

Concentrated in `crates/bot/src/telegram/`. Four framework layers frankenstein
does **not** provide (this is the real work), plus a small, enumerable Bot-API
call surface (all of which frankenstein has):

**Framework layers to rebuild:**
1. `Dispatcher` + `dptree` handler tree — `dispatch.rs`.
2. `BotCommands` derive macro — ~22 commands.
3. Adaptors: `Throttle` (rate-limit / anti-429) + `CacheMe` (`get_me` cache) —
   `bot.rs`, alias in `mod.rs` (`CacheMe<Throttle<Bot>>`).
4. `update_listeners::webhooks::axum_no_setup` + the `ShutdownAware` stream hack
   — `webhook.rs`, `shutdown_listener.rs`.

**Bot-API call surface (≈18 methods, ≈10 setters):** `send_message` (×24),
`answer_callback_query` (×20), `edit_message_text` (×9, load-bearing for
progress/streaming and the model/mode menus), `delete_message`, `send_photo`,
`send_voice/video/video_note/sticker/media_group/document/audio/animation`,
`get_me`, `get_file` + `download_file`, `set_my_commands` / `delete_my_commands`,
`close/edit/reopen_forum_topic`. Setters: `message_thread_id`, `parse_mode`,
`reply_markup`, `caption`, `reply_parameters`, `scope`, `show_alert`, `entities`,
`language_code`, `caption_entities`.

Transport is **webhook only** (via cloudflared tunnel), mounted on the bot.sock
UDS axum app under `/tg/<agent>`. No long-polling path.

Outside `crates/bot`, the only `teloxide` references are stale text: a doc
comment in `right-codegen/src/agent_def.rs`, an assertion message in
`right-agent/src/init.rs`, and a doc comment in `right/src/main.rs`. No code
dependency.

## Chosen approach

**A — `RightBot` wrapper + thin purpose-built helpers.** One module owns
frankenstein behind a `RightBot` struct that exposes only the operations we
actually use; it centralizes the three cross-cutting concerns teloxide gave us
(throttle, `get_me` cache, uniform error + parse-mode/thread defaults).

Rejected alternatives:
- **B — full teloxide-shaped compat facade** (replicate `Requester` + builder
  ergonomics): minimal call-site diff, but a permanent parallel framework to
  maintain — the "invariant hybrid" `AGENTS.md` warns against.
- **C — native frankenstein, no wrapper**: pure frankenstein, but duplicates
  throttle / me-cache / error-mapping across ~100 verbose call sites with worse
  cohesion and more churn than A.

## Design

### 1. Dependencies & module layout

- **Remove** `teloxide` (features `macros`, `throttle`, `cache-me`, `rustls`,
  `webhooks-axum`) from root `Cargo.toml` and `crates/bot/Cargo.toml`.
- **Add** `frankenstein` with the async reqwest client and a rustls TLS backend.
  Verify the exact feature names against frankenstein `0.50` at implementation
  time (`client-reqwest`; confirm whether rustls is a separate feature or a
  reqwest passthrough). Pin `frankenstein = "0.50"`.
- **Add** `governor` (latest) for the global rate limiter.
- **New files** in `crates/bot/src/telegram/`:
  - `tg_bot.rs` — `RightBot` wrapper, `Throttle`, `TgError`.
  - `command.rs` — `BotCommand` enum + manual `parse`.
  - `router.rs` — `HandlerCtx` + `route_update`.
- **Rewrite in place:** `bot.rs` (builds `RightBot`), `dispatch.rs` (routing logic
  → `router.rs`; `run_telegram` loses the `update_listener: L` generic),
  `webhook.rs` (axum handler that routes, not a listener).
- **Delete:** `shutdown_listener.rs` (`ShutdownAware` no longer needed).
- **Type sweep:** `filter.rs`, `mention.rs`, `session.rs`, `attachments.rs`,
  `archive.rs`, `worker.rs`, `progress.rs`, `async_delivery.rs`, the `*_command.rs`
  handlers, `mod.rs`, plus tests.
- **Outside `crates/bot`:** update the 3 stale comments/strings.

### 2. `RightBot` wrapper (replaces `CacheMe<Throttle<Bot>>`)

```rust
#[derive(Clone)]
pub struct RightBot {
    bot: frankenstein::client_reqwest::Bot,
    me: Arc<frankenstein::types::User>,
    rate: Arc<Throttle>,
}
pub type BotType = RightBot; // keeps existing bot.clone() / &BotType / struct-field usage
```

- All fields are cheap to clone (`frankenstein::Bot` holds a `reqwest::Client`
  which is `Arc` internally; `me`/`rate` are `Arc`). `RightBot: Clone` preserves
  every current `bot.clone()`, `&BotType`, and struct-field pattern, so the alias
  swap is the only change at those sites.
- **me-cache:** resolve `get_me` once during setup, store `Arc<User>`; `me()`
  returns it. Replaces `CacheMe`.
- **methods** (only what we use; each builds frankenstein `*Params::builder()`,
  applies throttle, maps errors, returns `Result<T, TgError>`):
  `send_html`, `send_text`, `edit_html`, `answer_callback`,
  `send_photo`/`voice`/`video`/`video_note`/`sticker`/`media_group`/`document`/
  `audio`/`animation`, `download_file`, `close_forum_topic`/`edit_forum_topic`/
  `reopen_forum_topic`, `set_my_commands`, `delete_my_commands`, `set_webhook`,
  `me`. Args are plain (`chat_id: i64`, `thread: Option<i32>`, `reply_to`,
  `markup`, …); the wrapper centralizes the `parse_mode = Html` and thread-id
  defaults that are currently repeated at call sites.
- **file download:** `get_file` → GET `{api_url}/file/bot{token}/{file_path}` via
  the reqwest client, streaming to disk. (Confirm whether frankenstein exposes a
  download helper; otherwise this small GET lives in `RightBot::download_file`.)

### 3. Throttle (replaces `Throttle` adaptor)

- `governor` global limiter sized to teloxide's default global cap (~30 msg/s).
- `DashMap<i64, Instant>` per-chat min-interval gate (~1 msg/s/chat) to preserve
  teloxide's per-chat semantics.
- On `frankenstein::Error::Api` carrying `retry_after` (HTTP 429 / flood-wait):
  sleep `retry_after`, retry **once**, then propagate.
- Applied inside the `RightBot` send/edit methods so all outbound traffic is
  gated uniformly. Decision rationale: a real throttle (not just 429-retry) is
  kept because SP2's streaming edits will lean on it; 429-retry is the floor, not
  the ceiling.

### 4. Update router (replaces `Dispatcher` / `dptree`)

```rust
pub struct HandlerCtx { /* the Arcs currently in dptree::deps! */ }
pub async fn route_update(update: frankenstein::Update, ctx: &HandlerCtx);
```

- `HandlerCtx` holds the dependencies currently injected via `dptree::deps!`
  (worker_map, settings, allowlist, identity, internal_api, home, ssh_config,
  intercept/token/auth-choice slots, idle_ts, worker-control deps, …). Replaces
  the runtime DI bag with a compile-checked struct.
- `route_update` matches `update.content` (`frankenstein::UpdateContent`):
  - `Message` / `EditedMessage` → `pre_filter_log_meta` inspect +
    `archive_seen_group_message` → `make_routing_filter` → if the text parses as a
    `BotCommand`, dispatch to the matching `handle_*`; else `handle_message`.
  - `CallbackQuery` → prefix match on `data`: `model:` → `handle_model_callback`,
    `mode:`/`modegroup:` → `handle_mode_callback`, `think:` →
    `handle_thinking_toggle_callback`, `bg:` → `handle_bg_callback`, `errdet:` →
    `handle_error_details_callback`, else `handle_stop_callback`.
  - Other variants ignored (matches `webhook_allowed_updates`).
- Handler fns keep their bodies; signatures change from dptree-injected params to
  `(&HandlerCtx, message/query, …)`. Routing must stay non-blocking — handlers
  delegate slow work to the per-session worker mpsc, as today.

### 5. Command parser (replaces `BotCommands` derive)

- Re-declare `enum BotCommand` with the same variants (`Start(String)`,
  `New(String)`, `List`, `Switch(String)`, `Mcp(String)`, `Providers(String)`,
  `SetFocus(String)`, `Doctor`, `Model`, `Mode`, `ModeGroup`, `Dashboard`,
  `Debug(String)`, `Cron(String)`, `Allow(String)`, `Deny(String)`, `Allowed`,
  `AllowAll`, `DenyAll`, `Usage(String)`).
- `fn parse(text: &str, bot_username: &str) -> Option<BotCommand>`: take the first
  whitespace-delimited token; require a leading `/`; strip an optional
  `@<bot_username>` suffix; lowercase-match the command name, preserving the
  non-default spellings `set_focus`, `mode_group`, `allow_all`, `deny_all`;
  payload = the remainder of the message (for the `String`-carrying variants).
- `visible_bot_commands()` (hides `usage`) and the three-scope registration
  (`Default`, `AllPrivateChats`, `AllGroupChats`) + stale-scope cleanup logic
  port over unchanged in spirit, calling `RightBot::set_my_commands` /
  `delete_my_commands` with `frankenstein` `BotCommand` / `BotCommandScope`.

### 6. Webhook handler + lifecycle (replaces listener + `ShutdownAware`)

- `build_webhook_router(secret: String, ctx: HandlerCtx) -> axum::Router`: a
  single POST route that
  1. validates `X-Telegram-Bot-Api-Secret-Token` → **401** on miss/mismatch;
  2. `serde_json::from_slice::<frankenstein::Update>(body)` — on parse error,
     **log + 200** (a non-2xx makes Telegram retry the same bad update forever);
  3. `route_update(update, &ctx).await` → **200**.
  Routing is non-blocking (delegates to worker mpsc), so inline-await-then-ack is
  acceptable.
- `setWebhook` (allowed_updates = `message`, `edited_message`, `callback_query`;
  secret token) moves to `RightBot::set_webhook`. URL + secret derivation
  (`derive_token(secret, "tg-webhook")`) unchanged.
- **Wiring (`run_telegram` / `lib.rs`):** split `run_telegram` into
  - *setup*: build `RightBot`, resolve identity (`get_me`), build `HandlerCtx`,
    register commands, produce the `axum::Router`;
  - *lifecycle*: the existing signal listener, `worker_shutdown` cancellation, and
    handoff-gate drain — kept as-is.
  `lib.rs` nests the returned router into the bot.sock UDS app **before** spawning
  it (replacing the current `build_webhook_router` call at lib.rs:606 and the
  `update_listener` argument at lib.rs:1175). The dispatch-loop `tokio::select!`
  arm becomes an await on the lifecycle/shutdown token. Removing the listener
  stream **eliminates** the hang-on-shutdown the `ShutdownAware` wrapper existed
  to patch.

### 7. Type migration (dominant mechanical work)

Apply consistently across the module:

| teloxide | frankenstein |
|---|---|
| `msg.text()` / `msg.caption()` (methods) | `message.text` / `message.caption` (`Option<String>` fields) |
| `msg.entities()` / `caption_entities()` / `media_group_id()` / `forward_origin()` | same-named fields |
| `msg.chat.id.0` (`ChatId(i64)`) | `message.chat.id` (`i64`) |
| `user.id.0` (`UserId(u64)`) | `user.id` (`u64`) |
| `MessageId(i32)` | `i32` (`message.message_id`) |
| `ThreadId` / `message_thread_id` | `Option<i32>` |
| `ChatId(id)` in send params | `ChatId::Integer(id)` (params enum) |
| `ParseMode::Html` | `frankenstein::ParseMode::Html` |
| `InlineKeyboardMarkup` / `InlineKeyboardButton::callback` | frankenstein equivalents, wrapped in `ReplyMarkup` |
| `teloxide::RequestError` | `TgError` (wraps `frankenstein::Error`) |
| `ChatKind` / `PublicChatKind` matching | frankenstein `Chat` / `ChatType` shape |

Test fixtures that build messages via `serde_json::from_value` of Bot-API-shaped
JSON deserialize into `frankenstein::Message` unchanged; only field access in
assertions/helpers updates. Verify `ChatKind` → frankenstein chat-type access in
`filter.rs`/`dispatch.rs` (`chat_kind_label`) during the sweep — frankenstein
models chat type differently from teloxide's nested `ChatKind`/`PublicChatKind`.

### 8. Error handling

- `TgError` (thiserror) wraps `frankenstein::Error`, preserving the FAIL-FAST
  rule: send helpers propagate with `?`; anyhow/error→string conversions use
  `{:#}`.
- Best-effort sites keep their current log-and-continue behavior exactly:
  command registration, stale-scope cleanup, `broadcast_to_chats`, archive,
  memory-alert broadcast.
- `broadcast_to_chats<R: Requester>` → concrete `async fn (&RightBot, …)` calling
  `RightBot::send_text`.

### 9. Testing

Port + add:
- `filter.rs` routing tests — port field access; assertions unchanged.
- `command.rs` parse tests — mirror the old `BotCommand::parse` asserts
  (lowercase, `set_focus` snake-case, `@bot` stripping, `usage` payload).
- webhook secret-rejection tests — retarget from the teloxide listener to the new
  axum handler (`tests/webhook_integration.rs`).
- `model_command` / `mode_command` keyboard + callback-data tests — port types.
- **New** router-dispatch unit tests (message→command vs message→`handle_message`
  vs callback-prefix routing) replacing `dispatcher_builds_without_panic`.
- **New** throttle unit tests (per-chat spacing, global cap, single 429-retry
  honoring `retry_after`).

**Cadence:** targeted `devenv shell -- cargo nextest run -p bot <filter>` during a
TDD red/green loop and after each coherent slice; **one** final
`devenv shell -- cargo nextest run --workspace` plus
`devenv shell -- cargo test --doc --workspace` from the worktree at completion.
Not full-workspace after every edit.

### 10. Out of scope / invariants

- **No behavior change.** Same commands, routing, webhook transport, throttle
  semantics, command scopes. Acceptance = feature parity.
- **No Rich Messages** (Bot API 10.1) — SP2, gated on frankenstein #325. The
  `RightBot` send seam is designed so `send_rich` / `stream_rich` drop in later
  without touching call sites.
- **Upgrade-compat.** Webhook URL, secret derivation, allowed_updates, and
  command scopes are unchanged; deployed agents adopt the change via
  `right restart <agent>` with no migration step, per the Upgrade & Migration
  Model.

## Risks & open questions (resolve during planning/impl)

1. **frankenstein TLS feature names** — confirm `client-reqwest` + rustls wiring
   on `0.50`. Verify by reading frankenstein's `Cargo.toml`/features, not docs.
2. **`Chat` / chat-type shape** — frankenstein's chat model differs from
   teloxide's `ChatKind`/`PublicChatKind`; confirm the mapping powering
   `chat_kind_label` and the private-vs-group branch in `filter.rs`.
3. **File download helper** — confirm whether frankenstein provides a download
   API or we issue the `{api_url}/file/bot{token}/{path}` GET ourselves.
4. **Throttle parity** — teloxide's exact `Limits` (per-chat, per-group/min,
   global) vs our governor + per-chat-map approximation. Approximation is
   acceptable for parity; document the chosen numbers.
5. **Webhook ack semantics** — confirm 200-ack on malformed body is the desired
   policy (prevents Telegram retry storms; loses nothing since the update is
   unparseable). Decided: yes.
6. **`reqwest` already in the tree?** — if added, align the version/features with
   the workspace and TLS choices.
