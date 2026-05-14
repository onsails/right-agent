# Telegram WebP Document Media Group Fallback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure outbound Telegram attachments are still delivered when a document media group contains WebP files, or when Telegram rejects an otherwise well-formed media group with an album-level validation error.

**Architecture:** Keep the existing pipeline: parse agent attachment JSON, resolve sandbox paths to host temp files, send Telegram attachments, then clean temp files. Add a narrow best-effort delivery branch inside the Telegram attachment sender: document groups with WebP magic bytes are sent as individual documents without attempting `sendMediaGroup`, and Telegram media-group validation failures fall back to individual sends.

**Tech Stack:** Rust 2024, `right-bot`, `teloxide`, `tokio`, existing `tempfile` test dependency, existing `devenv shell -- cargo ...` verification commands.

---

## Evidence

The reproduced `him-bot` failure was not caused by missing sandbox files, bad `attach://` wiring, or a broken local path. The agent emitted four existing WebP files under `/sandbox/outbox/logos-webp/*.webp`, and the sandbox files had valid `RIFF....WEBP` headers.

Live Bot API probes showed:

- `sendMediaGroup` with WebP files as `InputMediaDocument` fails with `Wrong file identifier/HTTP URL specified`.
- `sendDocument` for the same WebP file succeeds.
- `sendMediaGroup` with `.txt` documents succeeds.
- `sendMediaGroup` with `.png` documents succeeds.
- `sendMediaGroup` with WebP as `InputMediaPhoto` succeeds.

Conclusion: document media groups generally work, but Telegram rejects WebP when it is sent as a document album. The error text is misleading and must be treated as a recoverable album-validation failure.

## Files

- `crates/bot/src/telegram/attachments.rs`
- `docs/architecture/lifecycle.md` only for cite-on-touch review; no edit is expected because the data flow remains "download outbound attachments, send to Telegram".

## Implementation Tasks

### 1. Add classifier tests and pure helpers

- [ ] Run the current attachment test baseline:

```bash
devenv shell -- cargo test -p right-bot telegram::attachments::tests::
```

- [ ] In `crates/bot/src/telegram/attachments.rs`, add these failing tests inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn webp_magic_header_detects_riff_webp() {
    assert!(is_webp_file_header(b"RIFF\x00\x00\x00\x00WEBPVP8 "));
}

#[test]
fn webp_magic_header_rejects_png_and_short_input() {
    assert!(!is_webp_file_header(b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0d"));
    assert!(!is_webp_file_header(b"RIFF"));
}

#[test]
fn media_group_validation_error_text_matches_wrong_file_identifier() {
    let err = "Bad Request: failed to send message #1 with the error message \"Wrong file identifier/HTTP URL specified\"";
    assert!(is_media_group_validation_error_text(err));
}

#[test]
fn media_group_validation_error_text_matches_media_group_invalid() {
    assert!(is_media_group_validation_error_text(
        "Bad Request: MEDIA_GROUP_INVALID",
    ));
}

#[test]
fn media_group_validation_error_text_rejects_non_album_errors() {
    assert!(!is_media_group_validation_error_text(
        "Too Many Requests: retry after 5",
    ));
    assert!(!is_media_group_validation_error_text(
        "Bad Request: message text is empty",
    ));
}
```

- [ ] Verify the tests fail because the helper functions do not exist yet:

```bash
devenv shell -- cargo test -p right-bot webp_magic_header
devenv shell -- cargo test -p right-bot media_group_validation_error_text
```

- [ ] In `crates/bot/src/telegram/attachments.rs`, add the helpers near `display_error_chain`:

```rust
fn is_webp_file_header(header: &[u8]) -> bool {
    header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WEBP"
}

fn is_media_group_validation_error_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("wrong file identifier/http url specified")
        || lower.contains("media_group_invalid")
        || (lower.contains("media group")
            && lower.contains("bad request")
            && (lower.contains("file identifier") || lower.contains("http url")))
}
```

- [ ] Verify the new helper tests pass:

```bash
devenv shell -- cargo test -p right-bot webp_magic_header
devenv shell -- cargo test -p right-bot media_group_validation_error_text
```

- [ ] Commit this task:

```bash
devenv shell -- git add crates/bot/src/telegram/attachments.rs
devenv shell -- git commit -m "test(bot): cover Telegram media group fallback classifiers"
```

### 2. Add document-group preflight tests and media builder tests

- [ ] In `crates/bot/src/telegram/attachments.rs`, add these failing tests inside the existing test module after `att_with`:

```rust
#[tokio::test]
async fn document_group_preflight_requests_fallback_for_webp_document() {
    let dir = tempfile::tempdir().unwrap();
    let webp = dir.path().join("mark.webp");
    let png = dir.path().join("mark.png");
    tokio::fs::write(&webp, b"RIFF\x00\x00\x00\x00WEBPVP8 ")
        .await
        .unwrap();
    tokio::fs::write(&png, b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0d")
        .await
        .unwrap();

    let items = vec![
        att_with(OutboundKind::Document, Some("logos"), Some("webp")),
        att_with(OutboundKind::Document, Some("logos"), Some("png")),
    ];
    let host_paths = vec![webp, png];

    let reason = document_group_preflight_fallback_reason(&items, &host_paths).await;

    assert_eq!(
        reason.as_deref(),
        Some("document media group contains WebP file /sandbox/outbox/document-webp.bin"),
    );
}

#[tokio::test]
async fn document_group_preflight_allows_png_document_group() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.png");
    let b = dir.path().join("b.txt");
    tokio::fs::write(&a, b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0d")
        .await
        .unwrap();
    tokio::fs::write(&b, b"plain text").await.unwrap();

    let items = vec![
        att_with(OutboundKind::Document, Some("docs"), Some("a")),
        att_with(OutboundKind::Document, Some("docs"), Some("b")),
    ];
    let host_paths = vec![a, b];

    assert!(
        document_group_preflight_fallback_reason(&items, &host_paths)
            .await
            .is_none()
    );
}

#[test]
fn build_group_input_media_document_disables_content_type_detection() {
    let att = att_with(OutboundKind::Document, Some("docs"), Some("report"));
    let media = build_group_input_media(&att, std::path::Path::new("/tmp/report.pdf"));

    match media {
        teloxide::types::InputMedia::Document(document) => {
            assert_eq!(document.disable_content_type_detection, Some(true));
        }
        _ => panic!("expected document media"),
    }
}
```

- [ ] Verify these tests fail because the preflight and media-builder functions do not exist yet:

```bash
devenv shell -- cargo test -p right-bot document_group_preflight
devenv shell -- cargo test -p right-bot build_group_input_media_document_disables_content_type_detection
```

- [ ] In `crates/bot/src/telegram/attachments.rs`, add these helpers before `send_group`:

```rust
async fn document_group_preflight_fallback_reason(
    items: &[OutboundAttachment],
    host_paths: &[PathBuf],
) -> Option<String> {
    for (att, host_path) in items.iter().zip(host_paths.iter()) {
        if att.kind == OutboundKind::Document && file_has_webp_header(host_path).await {
            return Some(format!(
                "document media group contains WebP file {}",
                att.path
            ));
        }
    }
    None
}

async fn file_has_webp_header(path: &std::path::Path) -> bool {
    use tokio::io::AsyncReadExt;

    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!("failed to open {} for media sniffing: {e}", path.display());
            return false;
        }
    };

    let mut header = [0_u8; 12];
    match file.read_exact(&mut header).await {
        Ok(_) => is_webp_file_header(&header),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => false,
        Err(e) => {
            tracing::warn!("failed to read {} for media sniffing: {e}", path.display());
            false
        }
    }
}

fn build_group_input_media(
    att: &OutboundAttachment,
    host_path: &std::path::Path,
) -> teloxide::types::InputMedia {
    use teloxide::types::{
        InputFile, InputMedia, InputMediaAudio, InputMediaDocument, InputMediaPhoto,
        InputMediaVideo,
    };

    let file = InputFile::file(host_path.to_path_buf());
    let cap = att.caption.clone();
    match att.kind {
        OutboundKind::Photo => {
            let mut media = InputMediaPhoto::new(file);
            if let Some(caption) = cap {
                media = media.caption(caption);
            }
            InputMedia::Photo(media)
        }
        OutboundKind::Video => {
            let mut media = InputMediaVideo::new(file);
            if let Some(caption) = cap {
                media = media.caption(caption);
            }
            InputMedia::Video(media)
        }
        OutboundKind::Document => {
            let mut media = InputMediaDocument::new(file);
            media.disable_content_type_detection = Some(true);
            if let Some(caption) = cap {
                media = media.caption(caption);
            }
            InputMedia::Document(media)
        }
        OutboundKind::Audio => {
            let mut media = InputMediaAudio::new(file);
            if let Some(caption) = cap {
                media = media.caption(caption);
            }
            InputMedia::Audio(media)
        }
        _ => {
            tracing::error!(
                "send_group received ungroupable kind {:?} for {} - classifier bug",
                att.kind,
                att.path,
            );
            InputMedia::Document(InputMediaDocument::new(file))
        }
    }
}
```

- [ ] Replace the inline `InputMedia` construction in `send_group` with:

```rust
let media: Vec<InputMedia> = items
    .iter()
    .zip(host_paths.iter())
    .map(|(att, host)| build_group_input_media(att, host))
    .collect();
```

- [ ] Remove now-unused media type imports from the `send_group` local `use` list, leaving only the names still used by that function:

```rust
use teloxide::types::{InputMedia, MessageId, ThreadId};
```

- [ ] Verify the new tests and existing attachment tests pass:

```bash
devenv shell -- cargo test -p right-bot document_group_preflight
devenv shell -- cargo test -p right-bot build_group_input_media_document_disables_content_type_detection
devenv shell -- cargo test -p right-bot telegram::attachments::tests::
```

- [ ] Commit this task:

```bash
devenv shell -- git add crates/bot/src/telegram/attachments.rs
devenv shell -- git commit -m "test(bot): cover document group WebP preflight"
```

### 3. Wire preflight fallback to individual sends

- [ ] In `crates/bot/src/telegram/attachments.rs`, add these failing tests inside the existing test module:

```rust
#[test]
fn fallback_to_singles_error_message_contains_reason_if_leaked() {
    let msg = SendError::FallbackToSingles {
        reason: "preflight rejected WebP document group".to_owned(),
    }
    .into_user_msg("Document media group of 2 items");

    assert_eq!(
        msg,
        "media group fallback requested for Document media group of 2 items: preflight rejected WebP document group",
    );
}

#[test]
fn attachment_error_label_names_kind_and_path() {
    let att = att_with(OutboundKind::Document, Some("docs"), Some("report"));

    assert_eq!(
        attachment_error_label(&att),
        "Document attachment /sandbox/outbox/document-report.bin",
    );
}
```

- [ ] Verify the tests fail because the fallback variant and label helper do not exist yet:

```bash
devenv shell -- cargo test -p right-bot fallback_to_singles_error_message_contains_reason_if_leaked
devenv shell -- cargo test -p right-bot attachment_error_label_names_kind_and_path
```

- [ ] In `crates/bot/src/telegram/attachments.rs`, extend `SendError`:

```rust
enum SendError {
    Skip(String),
    Api(teloxide::RequestError),
    FallbackToSingles { reason: String },
}
```

- [ ] Update `SendError::into_user_msg` in `crates/bot/src/telegram/attachments.rs`:

```rust
Self::FallbackToSingles { reason } => {
    format!("media group fallback requested for {label}: {reason}")
}
```

- [ ] Add an individual-send fallback helper in `crates/bot/src/telegram/attachments.rs` near `send_attachments`:

```rust
fn attachment_error_label(att: &OutboundAttachment) -> String {
    format!("{:?} attachment {}", att.kind, att.path)
}

async fn send_group_items_as_singles(
    items: &[OutboundAttachment],
    ctx: &SendCtx<'_>,
    errors: &mut Vec<String>,
) {
    for att in items {
        let label = attachment_error_label(att);
        if let Err(e) = send_single(att, ctx).await {
            if matches!(&e, SendError::Api(_)) {
                tracing::error!("failed to send {label}: see SendError::Api");
            }
            errors.push(e.into_user_msg(&label));
        }
    }
}
```

- [ ] Update the `send_attachments` loop in `crates/bot/src/telegram/attachments.rs` so grouped sends handle `FallbackToSingles` without adding a user-visible group failure when the single sends succeed:

```rust
for send in &sends {
    match send {
        OutboundSend::Single(att) => {
            let label = attachment_error_label(att);
            if let Err(e) = send_single(att, &ctx).await {
                if matches!(&e, SendError::Api(_)) {
                    tracing::error!("failed to send {label}: see SendError::Api");
                }
                errors.push(e.into_user_msg(&label));
            }
        }
        OutboundSend::Group { kind, items } => match send_group(items, &ctx).await {
            Ok(()) => {}
            Err(SendError::FallbackToSingles { reason }) => {
                tracing::warn!(
                    group_kind = ?kind,
                    item_count = items.len(),
                    reason = %reason,
                    "media group cannot be sent as an album; falling back to individual sends",
                );
                send_group_items_as_singles(items, &ctx, &mut errors).await;
            }
            Err(e) => {
                let label = format!("{kind:?} media group of {} items", items.len());
                if matches!(&e, SendError::Api(_)) {
                    tracing::error!("failed to send {label}: see SendError::Api");
                }
                errors.push(e.into_user_msg(&label));
            }
        },
    }
}
```

- [ ] In `send_group`, after resolving all `host_paths` and before building `media`, call the preflight:

```rust
if let Some(reason) = document_group_preflight_fallback_reason(items, &host_paths).await {
    cleanup_host_paths(&host_paths, ctx.sandboxed).await;
    return Err(SendError::FallbackToSingles { reason });
}
```

- [ ] Verify the focused tests pass:

```bash
devenv shell -- cargo test -p right-bot document_group_preflight
devenv shell -- cargo test -p right-bot telegram::attachments::tests::
```

- [ ] Commit this task:

```bash
devenv shell -- git add crates/bot/src/telegram/attachments.rs
devenv shell -- git commit -m "fix(bot): fall back for WebP document media groups"
```

### 4. Fall back on Telegram media-group validation errors

- [ ] In `send_group` in `crates/bot/src/telegram/attachments.rs`, replace the current request result handling with classification of Telegram album-validation errors:

```rust
let result = match req.await {
    Ok(_) => Ok(()),
    Err(e) => {
        let reason = display_error_chain(&e);
        if is_media_group_validation_error_text(&reason) {
            Err(SendError::FallbackToSingles { reason })
        } else {
            Err(SendError::Api(e))
        }
    }
};

cleanup_host_paths(&host_paths, ctx.sandboxed).await;
result
```

- [ ] Verify the classifier tests and focused attachment tests pass:

```bash
devenv shell -- cargo test -p right-bot media_group_validation_error_text
devenv shell -- cargo test -p right-bot telegram::attachments::tests::
```

- [ ] Commit this task:

```bash
devenv shell -- git add crates/bot/src/telegram/attachments.rs
devenv shell -- git commit -m "fix(bot): retry invalid media groups as single sends"
```

### 5. Architecture docs check

- [ ] Re-read `docs/architecture/lifecycle.md`, especially the per-message reply flow.

- [ ] Confirm the doc still accurately says outbound attachments are downloaded from sandbox outbox and sent to Telegram. The fallback changes the delivery strategy inside that step, not the lifecycle.

- [ ] If the doc already matches the code, make no docs edit. If a drift is found, update only the exact outbound-attachment sentence and commit it:

```bash
devenv shell -- git add docs/architecture/lifecycle.md
devenv shell -- git commit -m "docs: clarify Telegram attachment fallback"
```

### 6. Final verification

- [ ] Run formatting:

```bash
devenv shell -- cargo fmt --check
```

- [ ] Run focused bot tests:

```bash
devenv shell -- cargo test -p right-bot telegram::attachments::tests::
```

- [ ] Run the mandatory full workspace test suite:

```bash
devenv shell -- cargo test --workspace
```

- [ ] Inspect the final diff:

```bash
devenv shell -- git status --short
devenv shell -- git diff --stat HEAD
```

## Manual Validation

Manual validation needs a live Telegram bot token and must not be committed as an automated test.

- Send two WebP files from the agent as `document` attachments with the same `media_group_id`.
- Expected bot behavior: no user-visible attachment error; the WebP files arrive as individual documents.
- Send two PNG files as `document` attachments with the same `media_group_id`.
- Expected bot behavior: the PNG files still arrive as one document media group.
- Send two photo/video attachments with the same `media_group_id`.
- Expected bot behavior: existing photo/video album behavior is unchanged.

## Risk Controls

- The WebP detection uses file magic bytes, not extension or MIME inference.
- The preflight is limited to `OutboundKind::Document`, so photo/video WebP album behavior remains untouched.
- The generic API fallback only triggers on known Telegram media-group validation text, not rate limits, timeouts, or unrelated bad requests.
- Temp files are cleaned before fallback single sends redownload from the sandbox. This avoids reusing paths after a failed album attempt and keeps the change local to existing cleanup rules.
