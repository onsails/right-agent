# Telegram Markdown for Captions & send_progress — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render agent-authored Markdown (`**bold**`, links, lists) correctly in Telegram attachment captions and `send_progress` messages, instead of showing literal `**`.

**Architecture:** Mirror the existing worker-reply path: convert Markdown → Telegram HTML with `md_to_telegram_html`, send with `ParseMode::Html`, and retry once as plain text on a Telegram parse rejection. Two unconverted paths are fixed: attachment captions (`crates/bot/src/telegram/attachments.rs`, single + media-group) and `send_progress` (`crates/bot/src/telegram/progress.rs`).

**Tech Stack:** Rust (edition 2024), teloxide, pulldown-cmark (already wired in `crates/bot/src/telegram/markdown.rs`), tokio, cargo-nextest.

**Spec:** `docs/superpowers/specs/2026-06-13-telegram-markdown-caption-progress-design.md`

**Working dir:** worktree `.claude/worktrees/tg-md-caption-progress`. Run all commands from the repo root inside this worktree. Prefix cargo with `devenv shell --`.

---

## Background the implementer needs

- `crate::telegram::markdown::md_to_telegram_html(md: &str) -> String` converts GFM Markdown to the HTML subset Telegram supports (`b/i/s/code/pre/a/blockquote`), escaping all text. Its output is always tag-balanced (pulldown-cmark closes every tag), so truncating the **raw** input before conversion can never produce broken HTML.
- `crate::telegram::markdown::strip_html_tags(html: &str) -> String` returns visible text with tags removed (used for the plain fallback).
- `attachments.rs` is large (~2900 lines) with an inline `#[cfg(test)] mod tests` at line 1552. Add unit tests there.
- `OutboundKind` derives `Copy` (`crates/bot/src/cc/attachments_dto.rs:16`) — pass it by value.
- `OutboundAttachment.caption` is `Option<String>` holding **raw agent Markdown** straight from structured-output JSON — never pre-escaped. Convert at send time; no double-conversion risk.
- `TELEGRAM_CAPTION_LIMIT = 1024` (`attachments.rs:363`). Telegram counts caption length by *visible* text after entity parsing — tags don't count — so truncating raw to 1024 chars keeps the converted HTML within the limit.
- Commit messages: Conventional Commits. Commit with `PREK_ALLOW_NO_CONFIG=1 git commit ...` (the worktree has no `.pre-commit-config.yaml`; the hook errors without this env var).

---

## File Structure

- `crates/bot/src/telegram/attachments.rs` — add `caption_to_html` / `caption_to_plain` helpers; refactor `send_single` to convert + `parse_mode` + plain retry via a new `send_single_attempt`; add `html: bool` to `build_group_input_media`; add a group-level plain-caption retry in `send_group`. Unit tests in the inline test module.
- `crates/bot/src/telegram/progress.rs` — convert + `parse_mode` + plain fallback in `handle_progress_send`.

No new files. No new shared module — both call sites use the existing `crate::telegram::markdown`.

---

## Task 1: Caption conversion helpers (pure, TDD)

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs` (add helpers after `merge_group_captions`, which ends at line 185; add tests in `mod tests` at line 1552)

- [ ] **Step 1: Write the failing tests**

Add to the inline `mod tests` (just inside the `{` at line 1553):

```rust
    #[test]
    fn caption_to_html_renders_bold() {
        assert_eq!(super::caption_to_html("**x**"), "<b>x</b>");
    }

    #[test]
    fn caption_to_html_truncates_raw_to_limit() {
        let raw = "a".repeat(super::TELEGRAM_CAPTION_LIMIT + 50);
        let html = super::caption_to_html(&raw);
        assert!(
            html.chars().count() <= super::TELEGRAM_CAPTION_LIMIT,
            "converted caption visible length {} exceeds limit",
            html.chars().count()
        );
    }

    #[test]
    fn caption_to_plain_strips_formatting() {
        assert_eq!(super::caption_to_plain("**x**"), "x");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo nextest run -p right-bot caption_to_`
Expected: FAIL — `cannot find function caption_to_html` / `caption_to_plain` (compile error).

- [ ] **Step 3: Add the helpers**

Insert immediately after `merge_group_captions` (after its closing `}` at line 185):

```rust
/// Truncate a raw caption to at most `TELEGRAM_CAPTION_LIMIT` characters
/// (char-safe), then convert agent Markdown to Telegram-supported HTML.
/// Truncating raw text before conversion keeps tags balanced and the visible
/// length within the caption limit (Telegram counts captions by visible text,
/// not HTML tag characters).
fn caption_to_html(raw: &str) -> String {
    let truncated: String = raw.chars().take(TELEGRAM_CAPTION_LIMIT).collect();
    super::markdown::md_to_telegram_html(&truncated)
}

/// Plain-text form of a caption for the `ParseMode::Html` fallback: visible
/// text with all formatting removed.
fn caption_to_plain(raw: &str) -> String {
    super::markdown::strip_html_tags(&caption_to_html(raw))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo nextest run -p right-bot caption_to_`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
PREK_ALLOW_NO_CONFIG=1 git commit -m "feat(attachments): caption_to_html/plain markdown helpers"
```

---

## Task 2: Convert single-attachment captions + plain fallback

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs` — `send_single` (lines 1190-1295), add `send_single_attempt` helper.

This is send wiring (needs live Telegram to exercise the parse_mode/fallback branch), so it is verified by compile + the existing `right-bot` suite, not a new unit test. The pure conversion is already covered by Task 1.

- [ ] **Step 1: Add the `send_single_attempt` helper**

Insert immediately **before** `async fn send_single` (before line 1190):

```rust
/// One Telegram send for a single attachment. Rebuilds `InputFile` on each call
/// so a failed send (which consumes the file) can be retried with a different
/// caption. `html = true` sets `ParseMode::Html`; `false` sends plain text.
async fn send_single_attempt(
    ctx: &SendCtx<'_>,
    host_path: &std::path::Path,
    kind: OutboundKind,
    thread_id: Option<teloxide::types::ThreadId>,
    caption: Option<&str>,
    html: bool,
) -> Result<(), teloxide::RequestError> {
    use teloxide::payloads::{
        SendAnimationSetters, SendAudioSetters, SendDocumentSetters, SendPhotoSetters,
        SendVideoSetters, SendVoiceSetters,
    };
    use teloxide::requests::Requester;
    use teloxide::types::{InputFile, ParseMode};

    let input_file = InputFile::file(host_path.to_path_buf());

    macro_rules! captioned {
        ($req:expr) => {{
            let mut req = $req;
            if let Some(cap) = caption {
                req = req.caption(cap);
                if html {
                    req = req.parse_mode(ParseMode::Html);
                }
            }
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }};
    }

    match kind {
        OutboundKind::Photo => captioned!(ctx.bot.send_photo(ctx.chat_id, input_file)),
        OutboundKind::Document => captioned!(ctx.bot.send_document(ctx.chat_id, input_file)),
        OutboundKind::Video => captioned!(ctx.bot.send_video(ctx.chat_id, input_file)),
        OutboundKind::Audio => captioned!(ctx.bot.send_audio(ctx.chat_id, input_file)),
        OutboundKind::Voice => captioned!(ctx.bot.send_voice(ctx.chat_id, input_file)),
        OutboundKind::Animation => captioned!(ctx.bot.send_animation(ctx.chat_id, input_file)),
        OutboundKind::VideoNote => {
            use teloxide::payloads::SendVideoNoteSetters;
            let mut req = ctx.bot.send_video_note(ctx.chat_id, input_file);
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }
        OutboundKind::Sticker => {
            use teloxide::payloads::SendStickerSetters;
            let mut req = ctx.bot.send_sticker(ctx.chat_id, input_file);
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }
    }
}
```

- [ ] **Step 2: Replace the body of `send_single`**

Replace the entire current `send_single` function (lines 1190-1295) with:

```rust
async fn send_single(att: &OutboundAttachment, ctx: &SendCtx<'_>) -> Result<(), SendError> {
    use teloxide::types::{MessageId, ThreadId};

    let host_path = resolve_host_path(att, ctx, "skipping")
        .await
        .map_err(SendError::Skip)?;

    let thread_id = if ctx.eff_thread_id != 0 {
        Some(ThreadId(MessageId(ctx.eff_thread_id as i32)))
    } else {
        None
    };

    let result: Result<(), teloxide::RequestError> = if let Some(raw) = att.caption.as_deref() {
        let html_cap = caption_to_html(raw);
        match send_single_attempt(ctx, &host_path, att.kind, thread_id, Some(&html_cap), true)
            .await
        {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::warn!(
                    "caption HTML send failed, retrying as plain text: {}",
                    display_error_chain(&e)
                );
                let plain_cap = caption_to_plain(raw);
                send_single_attempt(ctx, &host_path, att.kind, thread_id, Some(&plain_cap), false)
                    .await
            }
        }
    } else {
        send_single_attempt(ctx, &host_path, att.kind, thread_id, None, false).await
    };

    if ctx.sandboxed
        && let Err(e) = tokio::fs::remove_file(&host_path).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("failed to remove temp file {}: {e}", host_path.display());
    }

    result.map_err(SendError::Api)
}
```

- [ ] **Step 3: Build + clippy**

Run: `devenv shell -- cargo clippy -p right-bot --all-targets 2>&1 | tail -20`
Expected: no errors, no new warnings. (If `display_error_chain` needs `&dyn Error`: it takes `&(dyn std::error::Error + 'static)`; `&e` where `e: teloxide::RequestError` coerces — already used this way at `attachments.rs:1433`.)

- [ ] **Step 4: Run the right-bot attachment tests**

Run: `devenv shell -- cargo nextest run -p right-bot attachments`
Expected: PASS — existing attachment tests still green (path validation, group classification, etc.).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
PREK_ALLOW_NO_CONFIG=1 git commit -m "fix(attachments): render markdown in single captions with plain fallback"
```

---

## Task 3: Convert media-group captions + group plain retry

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs` — `build_group_input_media` (lines 1334-1384), `send_group` (lines 1386-1444), and the existing test call site at line 2673.

- [ ] **Step 1: Add `html: bool` to `build_group_input_media`**

Change the signature (line 1334-1337) and caption handling. Replace the function header and the `let cap` line:

```rust
fn build_group_input_media(
    att: &OutboundAttachment,
    host_path: &std::path::Path,
    html: bool,
) -> teloxide::types::InputMedia {
    use teloxide::types::{
        InputFile, InputMedia, InputMediaAudio, InputMediaDocument, InputMediaPhoto,
        InputMediaVideo, ParseMode,
    };

    let file = InputFile::file(host_path.to_path_buf());
    let cap = att.caption.as_deref().map(|raw| {
        if html {
            caption_to_html(raw)
        } else {
            caption_to_plain(raw)
        }
    });
```

Then in each captioned arm, set `parse_mode` when `html`. Replace the Photo arm (and apply the same shape to Video, Document, Audio):

```rust
        OutboundKind::Photo => {
            let mut media = InputMediaPhoto::new(file);
            if let Some(caption) = cap {
                media = media.caption(caption);
                if html {
                    media = media.parse_mode(ParseMode::Html);
                }
            }
            InputMedia::Photo(media)
        }
        OutboundKind::Video => {
            let mut media = InputMediaVideo::new(file);
            if let Some(caption) = cap {
                media = media.caption(caption);
                if html {
                    media = media.parse_mode(ParseMode::Html);
                }
            }
            InputMedia::Video(media)
        }
        OutboundKind::Document => {
            let mut media = InputMediaDocument::new(file);
            media.disable_content_type_detection = Some(true);
            if let Some(caption) = cap {
                media = media.caption(caption);
                if html {
                    media = media.parse_mode(ParseMode::Html);
                }
            }
            InputMedia::Document(media)
        }
        OutboundKind::Audio => {
            let mut media = InputMediaAudio::new(file);
            if let Some(caption) = cap {
                media = media.caption(caption);
                if html {
                    media = media.parse_mode(ParseMode::Html);
                }
            }
            InputMedia::Audio(media)
        }
```

Leave the `_ =>` arm (ungroupable kinds) unchanged.

- [ ] **Step 2: Add the group plain-caption retry in `send_group`**

Replace the media-build + send block in `send_group` (lines 1414-1443, from `let media: Vec<InputMedia> = ...` through the `let result = match req.await { ... };` and `cleanup_host_paths` / `result`) with:

```rust
    let thread_id = if ctx.eff_thread_id != 0 {
        Some(ThreadId(MessageId(ctx.eff_thread_id as i32)))
    } else {
        None
    };

    // Send an album once with the given caption rendering.
    async fn send_album(
        ctx: &SendCtx<'_>,
        items: &[OutboundAttachment],
        host_paths: &[PathBuf],
        thread_id: Option<ThreadId>,
        html: bool,
    ) -> Result<(), teloxide::RequestError> {
        use teloxide::payloads::SendMediaGroupSetters;
        use teloxide::requests::Requester;
        use teloxide::types::InputMedia;

        let media: Vec<InputMedia> = items
            .iter()
            .zip(host_paths.iter())
            .map(|(att, host)| build_group_input_media(att, host, html))
            .collect();
        let mut req = ctx.bot.send_media_group(ctx.chat_id, media);
        if let Some(tid) = thread_id {
            req = req.message_thread_id(tid);
        }
        req.await.map(|_| ())
    }

    let result = match send_album(ctx, items, &host_paths, thread_id, true).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let reason = display_error_chain(&e);
            if is_media_group_validation_error_text(&reason) {
                // Real album incompatibility — degrade to individual sends
                // (each send_single then runs its own HTML+plain fallback).
                Err(SendError::FallbackToSingles { reason })
            } else {
                // Likely a caption parse rejection — retry the album once with
                // plain captions, preserving the album and dropping only the
                // caption's formatting.
                tracing::warn!(
                    "media-group HTML caption send failed, retrying as plain text: {reason}"
                );
                match send_album(ctx, items, &host_paths, thread_id, false).await {
                    Ok(()) => Ok(()),
                    Err(e2) => Err(SendError::Api(e2)),
                }
            }
        }
    };

    cleanup_host_paths(&host_paths, ctx.sandboxed).await;
    result
```

Note: `ThreadId` / `MessageId` are already imported at the top of `send_group` (`use teloxide::types::{InputMedia, MessageId, ThreadId};`). Keep that import; the inner `send_album` re-imports `InputMedia` locally, which is fine.

- [ ] **Step 3: Update the existing test call site**

At `attachments.rs:2673`, the test `build_group_input_media_document_disables_content_type_detection` calls `build_group_input_media(&att, &host)`. Add the new arg:

Run: `rg -n "build_group_input_media\(" crates/bot/src/telegram/attachments.rs`
Update each non-definition call in tests to pass `true`, e.g.:

```rust
        let media = build_group_input_media(&att, &host, true);
```

- [ ] **Step 4: Build + clippy**

Run: `devenv shell -- cargo clippy -p right-bot --all-targets 2>&1 | tail -20`
Expected: no errors, no new warnings.

- [ ] **Step 5: Run group tests**

Run: `devenv shell -- cargo nextest run -p right-bot build_group_input_media`
Run: `devenv shell -- cargo nextest run -p right-bot attachments`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
PREK_ALLOW_NO_CONFIG=1 git commit -m "fix(attachments): render markdown in media-group captions with plain retry"
```

---

## Task 4: Convert send_progress messages + plain fallback

**Files:**
- Modify: `crates/bot/src/telegram/progress.rs` — `handle_progress_send` (the send at line 129), `teloxide::types` import (line 18).

- [ ] **Step 1: Add `ParseMode` to imports**

Change `progress.rs:18` from:

```rust
use teloxide::types::{ChatId, CustomEmojiId, MessageId, Rgb, ThreadId};
```

to:

```rust
use teloxide::types::{ChatId, CustomEmojiId, MessageId, ParseMode, Rgb, ThreadId};
```

- [ ] **Step 2: Convert + parse_mode + plain fallback**

In `handle_progress_send`, the raw-length validation (`message.chars().count() > PROGRESS_MESSAGE_MAX_CHARS`, lines 116-127) stays unchanged — it gates the **raw** input. Replace the send-builder block (lines 129-132):

```rust
    let mut send = state.bot.send_message(ChatId(target.chat_id), message);
    if target.thread_id != 0 {
        send = send.message_thread_id(ThreadId(MessageId(target.thread_id as i32)));
    }
```

with a converted send wrapped in a helper that retries plain on a parse error. Replace it with:

```rust
    let html = crate::telegram::markdown::md_to_telegram_html(message);
    let thread = if target.thread_id != 0 {
        Some(ThreadId(MessageId(target.thread_id as i32)))
    } else {
        None
    };

    // Build the send fresh per attempt; `html = true` sets ParseMode::Html.
    let send_attempt = |text: String, as_html: bool| {
        let mut send = state.bot.send_message(ChatId(target.chat_id), text);
        if as_html {
            send = send.parse_mode(ParseMode::Html);
        }
        if let Some(tid) = thread {
            send = send.message_thread_id(tid);
        }
        send
    };
```

Then update the `tokio::time::timeout(PROGRESS_SEND_TIMEOUT, send)` block (lines 139-175) to try HTML, and on a Telegram `RequestError` retry plain before returning the error. Replace the whole `match tokio::time::timeout(...).await { ... }` with:

```rust
    let outcome = tokio::time::timeout(PROGRESS_SEND_TIMEOUT, send_attempt(html.clone(), true)).await;
    let outcome = match outcome {
        Ok(Err(e)) => {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                "progress HTML send failed, retrying as plain text: {e:#}",
            );
            let plain = crate::telegram::markdown::strip_html_tags(&html);
            tokio::time::timeout(PROGRESS_SEND_TIMEOUT, send_attempt(plain, false)).await
        }
        other => other,
    };

    match outcome {
        Ok(Ok(message)) => (
            StatusCode::OK,
            Json(ProgressSendResponse {
                ok: true,
                message_id: Some(message.id.0),
            }),
        )
            .into_response(),
        Ok(Err(e)) => {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                "telegram send failed: {e:#}",
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(ProgressErrorResponse {
                    error: "telegram_send_failed".to_owned(),
                }),
            )
                .into_response()
        }
        Err(_) => {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                "telegram send timed out after {}s",
                PROGRESS_SEND_TIMEOUT.as_secs(),
            );
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ProgressErrorResponse {
                    error: "telegram_send_timeout".to_owned(),
                }),
            )
                .into_response()
        }
    }
```

Note: `message` here is the trimmed `&str` from line 116 (`let message = req.message.trim();`). `md_to_telegram_html(message)` borrows it before any move. `send_message` takes `String`, so pass `html.clone()` (the HTML attempt) and the owned `plain` (fallback). No `split_html_message`: 2000 raw chars convert to ≤2000 visible chars, under Telegram's 4096 limit; `send_progress` is a single-message primitive.

- [ ] **Step 3: Build + clippy**

Run: `devenv shell -- cargo clippy -p right-bot --all-targets 2>&1 | tail -20`
Expected: no errors. (If the closure trips a borrow on `state`/`thread`, they are `Copy`/`Clone`: `thread` is `Option<ThreadId>` (Copy), `state.bot` is `Clone`; the closure borrows `state` immutably and is fine for sequential `.await`s.)

- [ ] **Step 4: Run progress tests**

Run: `devenv shell -- cargo nextest run -p right-bot progress`
Expected: PASS — existing progress tests (token redaction, registry roundtrip, forum error mapping) unaffected.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/progress.rs
PREK_ALLOW_NO_CONFIG=1 git commit -m "fix(progress): render markdown in send_progress with plain fallback"
```

---

## Task 5: Final workspace verification

**Files:** none (verification only).

- [ ] **Step 1: Full workspace test (mandatory in this worktree)**

Run: `devenv shell -- cargo nextest run --workspace`
Expected: PASS. If a known-flaky test fails (cc/invocation pid race or dashboard warn-count — see project memory), re-run it isolated before blaming this change.

- [ ] **Step 2: Doctests**

Run: `devenv shell -- cargo test --doc --workspace`
Expected: PASS.

- [ ] **Step 3: Final clippy gate**

Run: `devenv shell -- cargo clippy --workspace --all-targets 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 4: Manual smoke (optional, if a dev agent + sandbox are available)**

Trigger an agent turn that returns a photo attachment whose caption contains `**bold**` and a link, and one `send_progress` call with `**bold**`. Confirm in Telegram that bold renders (not literal `**`). Reproduce a fallback by using a caption with an invalid link like `[x](javascript:1)` and confirm the photo still arrives with plain text.

- [ ] **Step 5: Confirm no stray spec drift**

Run: `rg -n "ParseMode|md_to_telegram_html|caption_to_html|caption_to_plain" crates/bot/src/telegram/attachments.rs crates/bot/src/telegram/progress.rs`
Expected: captions and send_progress now reference the converter + `ParseMode::Html`; reply/cron/reflection paths untouched.

---

## Self-Review notes (author)

- **Spec coverage:** caption single (Task 2), caption media-group + group retry (Task 3), send_progress (Task 4), char-safe truncate + helpers (Task 1), final full-workspace verification (Task 5). All spec sections mapped.
- **Type consistency:** `caption_to_html`/`caption_to_plain` defined in Task 1 are used verbatim in Tasks 2-3; `send_single_attempt` and `build_group_input_media(.., html)` signatures match their call sites; `ParseMode` imported where used.
- **No live-Telegram unit tests** for send wiring — pure conversion is unit-tested (Task 1); wiring verified by compile + existing suite + manual smoke (Task 5). This matches the project convention of extracting pure logic and unit-testing it.
