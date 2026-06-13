# Telegram Markdown Rendering: Attachment Captions & `send_progress`

**Date:** 2026-06-13
**Status:** Design approved, pending implementation plan

## Problem

Agent-authored Markdown (`**bold**`, `*italic*`, lists, links) renders
literally — the user sees raw `**` asterisks — when the text is sent through
two Telegram paths that skip Markdown→HTML conversion:

1. **Attachment captions** (`crates/bot/src/telegram/attachments.rs`) — the
   `caption` of a photo/video/document/audio/media-group is passed to teloxide
   verbatim with **no `ParseMode::Html` and no `md_to_telegram_html`** (the file
   has zero references to either).
2. **`send_progress`** (`crates/bot/src/telegram/progress.rs:129`) — the agent's
   progress message is sent with `bot.send_message(...)` and **no parse mode**.

Every other agent-content path already converts: the worker reply
(`worker.rs:1852`), cron/async delivery (`async_delivery.rs`), and reflection
(`worker.rs:2105`) all run `md_to_telegram_html` + `split_html_message` and set
`ParseMode::Html`.

### Root-cause evidence (riskoff, 2026-06-13)

The reported `**Стоп-лосс…**` literal post was **not** `send_progress`. The
session stream (`aedc2d10…ndjson`) shows the agent returned the post as a
**structured-output photo attachment with a caption**:

```
"path":"/sandbox/outbox/stop_loss_post.jpg",
"caption":"**Стоп-лосс — это не убыток. Это страховка.**\n\nБольшинство новичков…"
```

`send_progress` was invoked once in that turn, only for the narration line
"Пишу пост…". The caption path is the culprit; `send_progress` is the sibling
path with the identical defect.

Out of scope: the separate cron attachment-delivery failure (host vs
`/sandbox/outbox/` path) — already fixed in master `665e8f1d`.

## Goal

Markdown renders identically on **every** path where an agent writes to
Telegram. Fix both unconverted paths to mirror the reply path: convert
Markdown → Telegram HTML, set `ParseMode::Html`, and fall back to plain text on
a Telegram parse rejection.

## Approach

**Convert at the send site, reusing the existing `md_to_telegram_html`**, with a
plain-text fallback — exactly the pattern the worker reply path already uses
(`worker.rs:1896`). No new shared abstraction; changes are local to the two
files. (Rejected alternatives: normalizing captions to HTML at
`OutboundAttachment` construction — breaks raw-text truncation/merge and muddies
the field's semantics; a shared `TelegramText{html,plain}` type — overengineered
for two call sites.)

## Design

### 1. Attachment captions (`attachments.rs`) — primary fix

**Conversion order (load-bearing):** truncate the **raw** caption first, then
convert. Telegram counts a caption's length by its *visible* text after entity
parsing (the `<b>` tags don't count), so truncating raw Markdown to
`TELEGRAM_CAPTION_LIMIT` (1024) guarantees the converted HTML stays within the
limit. Truncating raw before conversion also means `md_to_telegram_html` always
emits balanced tags — truncation can never produce broken HTML.

- **Media groups:** keep the existing flow. `merge_group_captions` joins and
  truncates the **raw** caption (≤1024). Convert the merged raw caption to HTML
  **after** the merge. Set `.parse_mode(ParseMode::Html)` on the first
  `InputMedia*` item (the only item carrying a caption after the merge blanks the
  rest).
- **Single sends (`send_single`):** truncate the raw caption to 1024, convert to
  HTML, apply `.caption(html).parse_mode(ParseMode::Html)` on every captioned
  kind (Photo, Document, Video, Audio, Voice, Animation; VideoNote/Sticker take
  no caption).
- **Helper:** a thin, pure, caption-local helper:
  ```rust
  /// Truncate the raw caption to the Telegram limit, then convert Markdown to
  /// Telegram-supported HTML. Truncating before conversion keeps tags balanced
  /// and the visible length within TELEGRAM_CAPTION_LIMIT.
  fn caption_to_html(raw: &str) -> String
  ```
  Single-caption truncation moves into this helper (today only the media-group
  path truncates). The existing media-group raw truncation in
  `merge_group_captions` stays; the merged result is passed through
  `caption_to_html` (which re-applies the limit harmlessly).

**Plain-text fallback (`send_single`):** build both `caption_html` and
`caption_plain = strip_html_tags(caption_html)` before sending. Try the HTML
send; on `teloxide::RequestError` (mapped to `SendError::Api`), retry **once**
with `.caption(caption_plain)` and no parse mode, **before** the temp-file
removal. Mirrors `worker.rs:1896-1932`.

When the fallback fires (narrow but real): the agent wrote a Markdown link with
a URL Telegram rejects (`[t](javascript:…)`, empty `[t]()`), or a rare nested
entity Telegram won't parse. Plain prose with `**bold**` never triggers it. The
fallback guarantees the attachment is still delivered (caption as plain text)
instead of the whole send failing — important for a cron poster where a failed
photo send is a silent missed post.

**Media-group fallback:** unchanged. A rejected group send already degrades to
individual `send_single` calls via the existing `fallback_items` machinery;
those inherit the per-send HTML+fallback robustness, so no extra handling is
needed at the group layer.

### 2. `send_progress` (`crates/bot/src/telegram/progress.rs`)

In `handle_progress_send`:

- Keep the raw-length validation (`PROGRESS_MESSAGE_MAX_CHARS = 2000`) on the
  **raw** input — the limit is on what the agent submits.
- Convert: `md_to_telegram_html(message)`, send with
  `.parse_mode(ParseMode::Html)`.
- On a Telegram `RequestError`, retry once with `strip_html_tags(html)` and no
  parse mode, then return the existing `telegram_send_failed` error only if the
  fallback also fails.
- No `split_html_message`: 2000 raw chars convert to ≤2000 visible chars, well
  under Telegram's 4096 message limit. `send_progress` is a single-message
  concept; splitting it into multiple messages would change its semantics.

### 3. Shared helpers

Both files already have access to `crate::telegram::markdown`:
`md_to_telegram_html` (`pub fn`) and `strip_html_tags` (`pub`). No new shared
module. `caption_to_html` is caption-specific (1024 truncation) and lives in
`attachments.rs`.

## What is NOT changed

- Reply, cron/async text, and reflection paths — already correct.
- Attachment **delivery** (host↔sandbox path) — fixed in `665e8f1d`.
- `md_to_telegram_html` itself — already covered by `markdown_tests.rs`.
- `send_progress` semantics, rate limit, foreground-only gating, and the
  `--disallowedTools` block in cron — unchanged.

## Testing

TDD; targeted package tests during the loop, full workspace test at the end.

- **`caption_to_html` (pure, unit):**
  - `**x**` → `<b>x</b>`; `*x*` → `<i>x</i>`; plain text passes through escaped.
  - A raw caption longer than 1024 is truncated so the converted HTML's visible
    length ≤ 1024.
  - Merge-then-convert for a media group yields one valid HTML caption from
    multiple raw parts.
  - A raw link with a normal URL converts to `<a href="…">`.
- **`send_progress` HTML prep (pure, unit):** extract a pure
  `progress_message_html(raw) -> String` (or reuse `md_to_telegram_html`
  directly) and assert `**x**` → `<b>x</b>`. The send/parse_mode/fallback wiring
  needs a Telegram mock and is left to manual/integration verification.
- **No live Telegram in unit tests** — the actual `parse_mode` + fallback branch
  is integration-only; cover the pure conversion helpers directly per project
  convention (extract pure logic, unit-test it).

### Verification cadence

- Baseline: `devenv shell -- cargo nextest run -p right-bot` at worktree start.
- During: targeted `-p right-bot` filters after each red/green slice.
- Final (mandatory, in the worktree): `devenv shell -- cargo nextest run
  --workspace` plus `devenv shell -- cargo test --doc --workspace`.

## Files touched

- `crates/bot/src/telegram/attachments.rs` — `caption_to_html` helper,
  conversion + `parse_mode` + plain fallback in `send_single`, `parse_mode` on
  media-group `InputMedia`, conversion after `merge_group_captions`.
- `crates/bot/src/telegram/attachments_tests.rs` (or inline `#[cfg(test)]`) —
  `caption_to_html` unit tests.
- `crates/bot/src/telegram/progress.rs` — convert + `parse_mode` + plain
  fallback in `handle_progress_send`; optional pure helper + its test.
