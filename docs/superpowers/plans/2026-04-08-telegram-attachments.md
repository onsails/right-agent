# Telegram Attachments Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add full Telegram attachment support (inbound and outbound) with unified YAML stdin input format, replacing the XML batch and `media_paths` stub.

**Architecture:** New `attachments.rs` module in `crates/bot/src/telegram/` handles extraction, download/upload, sending, and cleanup. `worker.rs` switches from positional arg to stdin piping. `handler.rs` extracts attachments from all Telegram media types. Reply schema replaces `media_paths` with typed `attachments` array.

**Tech Stack:** teloxide 0.17 (photo/document/video/audio/voice/sticker/animation APIs), tokio (async file I/O, cleanup task), serde/serde_json (output parsing), manual YAML string building (serde-saphyr is deserialize-only).

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/bot/src/telegram/attachments.rs` | Create | All attachment types, extraction, download/upload, send, cleanup, YAML formatting |
| `crates/bot/src/telegram/mod.rs` | Modify | Add `pub mod attachments;` |
| `crates/bot/src/telegram/worker.rs` | Modify | Replace XML with YAML, stdin piping, new `ReplyOutput.attachments`, outbound send logic |
| `crates/bot/src/telegram/handler.rs` | Modify | Extract text+attachments from all media types |
| `crates/rightclaw/src/codegen/agent_def.rs` | Modify | Update `REPLY_SCHEMA_JSON`, add system prompt section |
| `crates/rightclaw/src/codegen/agent_def_tests.rs` | Modify | Update test for new schema |
| `crates/rightclaw/src/agent/types.rs` | Modify | Add `AttachmentsConfig` to `AgentConfig` |
| `crates/bot/src/lib.rs` | Modify | Create inbox/outbox dirs on startup, spawn cleanup task |

---

### Task 1: AttachmentKind enum and core types

**Files:**
- Create: `crates/bot/src/telegram/attachments.rs`
- Modify: `crates/bot/src/telegram/mod.rs:1-7`

- [ ] **Step 1: Write tests for AttachmentKind and mime_to_extension**

Create `crates/bot/src/telegram/attachments.rs` with types and tests:

```rust
use serde::Deserialize;
use std::path::PathBuf;

/// All Telegram media types we handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Photo,
    Document,
    Video,
    Audio,
    Voice,
    VideoNote,
    Sticker,
    Animation,
}

impl AttachmentKind {
    /// Lowercase string for YAML `type:` field and file naming prefix.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Photo => "photo",
            Self::Document => "document",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Voice => "voice",
            Self::VideoNote => "video_note",
            Self::Sticker => "sticker",
            Self::Animation => "animation",
        }
    }
}

/// Extracted from Telegram message, before download.
#[derive(Debug, Clone)]
pub struct InboundAttachment {
    pub file_id: String,
    pub kind: AttachmentKind,
    pub mime_type: Option<String>,
    pub filename: Option<String>,
    pub file_size: Option<u32>,
}

/// After download, with resolved filesystem path.
#[derive(Debug, Clone)]
pub struct ResolvedAttachment {
    pub kind: AttachmentKind,
    pub path: PathBuf,
    pub mime_type: String,
    pub filename: Option<String>,
}

/// From CC JSON response.
#[derive(Debug, Clone, Deserialize)]
pub struct OutboundAttachment {
    #[serde(rename = "type")]
    pub kind: OutboundKind,
    pub path: String,
    pub filename: Option<String>,
    pub caption: Option<String>,
}

/// Attachment kinds CC can produce in output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundKind {
    Photo,
    Document,
    Video,
    Audio,
    Voice,
    VideoNote,
    Sticker,
    Animation,
}

/// Derive file extension from MIME type. Fallback to `.bin`.
pub fn mime_to_extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "application/pdf" => "pdf",
        "application/json" => "json",
        "application/zip" => "zip",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "text/html" => "html",
        _ => "bin",
    }
}

/// Size limits (bytes) for Telegram Bot API.
pub const TELEGRAM_DOWNLOAD_LIMIT: u64 = 20 * 1024 * 1024; // 20 MB
pub const TELEGRAM_PHOTO_UPLOAD_LIMIT: u64 = 10 * 1024 * 1024; // 10 MB
pub const TELEGRAM_FILE_UPLOAD_LIMIT: u64 = 50 * 1024 * 1024; // 50 MB

/// Default attachment retention in days.
pub const DEFAULT_RETENTION_DAYS: u32 = 7;

/// Cleanup interval.
pub const CLEANUP_INTERVAL_SECS: u64 = 3600; // 1 hour

/// Fixed sandbox paths.
pub const SANDBOX_INBOX: &str = "/sandbox/inbox";
pub const SANDBOX_OUTBOX: &str = "/sandbox/outbox";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_kind_as_str_roundtrip() {
        assert_eq!(AttachmentKind::Photo.as_str(), "photo");
        assert_eq!(AttachmentKind::Document.as_str(), "document");
        assert_eq!(AttachmentKind::Video.as_str(), "video");
        assert_eq!(AttachmentKind::Audio.as_str(), "audio");
        assert_eq!(AttachmentKind::Voice.as_str(), "voice");
        assert_eq!(AttachmentKind::VideoNote.as_str(), "video_note");
        assert_eq!(AttachmentKind::Sticker.as_str(), "sticker");
        assert_eq!(AttachmentKind::Animation.as_str(), "animation");
    }

    #[test]
    fn mime_to_extension_known_types() {
        assert_eq!(mime_to_extension("image/jpeg"), "jpg");
        assert_eq!(mime_to_extension("image/png"), "png");
        assert_eq!(mime_to_extension("audio/ogg"), "ogg");
        assert_eq!(mime_to_extension("application/pdf"), "pdf");
        assert_eq!(mime_to_extension("video/mp4"), "mp4");
    }

    #[test]
    fn mime_to_extension_unknown_fallback() {
        assert_eq!(mime_to_extension("application/x-unknown-thing"), "bin");
        assert_eq!(mime_to_extension(""), "bin");
    }

    #[test]
    fn outbound_kind_deserialize() {
        let json = r#"{"type":"photo","path":"/sandbox/outbox/img.png"}"#;
        let att: OutboundAttachment = serde_json::from_str(json).unwrap();
        assert_eq!(att.kind, OutboundKind::Photo);
        assert_eq!(att.path, "/sandbox/outbox/img.png");
        assert!(att.filename.is_none());
        assert!(att.caption.is_none());
    }

    #[test]
    fn outbound_kind_deserialize_with_all_fields() {
        let json = r#"{"type":"document","path":"/sandbox/outbox/data.csv","filename":"results.csv","caption":"Here's the data"}"#;
        let att: OutboundAttachment = serde_json::from_str(json).unwrap();
        assert_eq!(att.kind, OutboundKind::Document);
        assert_eq!(att.filename.as_deref(), Some("results.csv"));
        assert_eq!(att.caption.as_deref(), Some("Here's the data"));
    }

    #[test]
    fn outbound_kind_deserialize_snake_case() {
        let json = r#"{"type":"video_note","path":"/sandbox/outbox/note.mp4"}"#;
        let att: OutboundAttachment = serde_json::from_str(json).unwrap();
        assert_eq!(att.kind, OutboundKind::VideoNote);
    }
}
```

- [ ] **Step 2: Add module declaration**

In `crates/bot/src/telegram/mod.rs`, add after line 7:

```rust
pub mod attachments;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot --lib telegram::attachments`
Expected: All 6 tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): add attachment types, mime_to_extension, and constants"
```

---

### Task 2: YAML input formatting

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`
- Modify: `crates/bot/src/telegram/worker.rs:31-36` (DebounceMsg)

- [ ] **Step 1: Write YAML formatting functions and tests**

Add to `crates/bot/src/telegram/attachments.rs`:

```rust
use chrono::{DateTime, Utc};

/// Message in a debounce batch -- text and/or attachments.
#[derive(Debug, Clone)]
pub struct InputMessage {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<ResolvedAttachment>,
}

/// Format input for CC stdin.
///
/// - Single message with no attachments: raw text string.
/// - Otherwise: YAML with `messages:` root key.
///
/// Returns None if there is nothing to send.
pub fn format_cc_input(msgs: &[InputMessage]) -> Option<String> {
    if msgs.is_empty() {
        return None;
    }

    // Single message, no attachments, has text: plain text
    if msgs.len() == 1 && msgs[0].attachments.is_empty() {
        return msgs[0].text.clone();
    }

    // YAML format
    let mut out = String::from("messages:\n");
    for m in msgs {
        out.push_str(&format!("  - id: {}\n", m.message_id));
        out.push_str(&format!(
            "    ts: \"{}\"\n",
            m.timestamp.format("%Y-%m-%dT%H:%M:%SZ")
        ));
        if let Some(ref text) = m.text {
            let escaped = yaml_escape_string(text);
            out.push_str(&format!("    text: \"{escaped}\"\n"));
        }
        if !m.attachments.is_empty() {
            out.push_str("    attachments:\n");
            for att in &m.attachments {
                out.push_str(&format!("      - type: {}\n", att.kind.as_str()));
                out.push_str(&format!("        path: {}\n", att.path.display()));
                out.push_str(&format!("        mime_type: {}\n", att.mime_type));
                if let Some(ref fname) = att.filename {
                    let escaped = yaml_escape_string(fname);
                    out.push_str(&format!("        filename: \"{escaped}\"\n"));
                }
            }
        }
    }
    Some(out)
}

/// Escape a string for inclusion in YAML double-quoted scalar.
fn yaml_escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}
```

Add tests to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn format_cc_input_single_text_returns_plain_string() {
        let msgs = vec![InputMessage {
            message_id: 1,
            text: Some("hello world".into()),
            timestamp: DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            attachments: vec![],
        }];
        let result = format_cc_input(&msgs).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn format_cc_input_empty_returns_none() {
        assert!(format_cc_input(&[]).is_none());
    }

    #[test]
    fn format_cc_input_single_no_text_no_attachments_returns_none() {
        let msgs = vec![InputMessage {
            message_id: 1,
            text: None,
            timestamp: Utc::now(),
            attachments: vec![],
        }];
        assert!(format_cc_input(&msgs).is_none());
    }

    #[test]
    fn format_cc_input_multiple_messages_returns_yaml() {
        let ts = DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let msgs = vec![
            InputMessage {
                message_id: 1,
                text: Some("first".into()),
                timestamp: ts,
                attachments: vec![],
            },
            InputMessage {
                message_id: 2,
                text: Some("second".into()),
                timestamp: ts,
                attachments: vec![],
            },
        ];
        let result = format_cc_input(&msgs).unwrap();
        assert!(result.starts_with("messages:\n"));
        assert!(result.contains("  - id: 1\n"));
        assert!(result.contains("    text: \"first\"\n"));
        assert!(result.contains("  - id: 2\n"));
        assert!(result.contains("    text: \"second\"\n"));
    }

    #[test]
    fn format_cc_input_with_attachments_returns_yaml() {
        let ts = DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let msgs = vec![InputMessage {
            message_id: 42,
            text: Some("check this".into()),
            timestamp: ts,
            attachments: vec![ResolvedAttachment {
                kind: AttachmentKind::Photo,
                path: PathBuf::from("/sandbox/inbox/photo_42_0.jpg"),
                mime_type: "image/jpeg".into(),
                filename: None,
            }],
        }];
        let result = format_cc_input(&msgs).unwrap();
        assert!(result.starts_with("messages:\n"));
        assert!(result.contains("    attachments:\n"));
        assert!(result.contains("      - type: photo\n"));
        assert!(result.contains("        path: /sandbox/inbox/photo_42_0.jpg\n"));
        assert!(result.contains("        mime_type: image/jpeg\n"));
        assert!(!result.contains("filename:"));
    }

    #[test]
    fn format_cc_input_document_with_filename() {
        let ts = Utc::now();
        let msgs = vec![InputMessage {
            message_id: 10,
            text: None,
            timestamp: ts,
            attachments: vec![ResolvedAttachment {
                kind: AttachmentKind::Document,
                path: PathBuf::from("/sandbox/inbox/doc_10_0.pdf"),
                mime_type: "application/pdf".into(),
                filename: Some("report.pdf".into()),
            }],
        }];
        let result = format_cc_input(&msgs).unwrap();
        assert!(result.contains("      - type: document\n"));
        assert!(result.contains("        filename: \"report.pdf\"\n"));
    }

    #[test]
    fn format_cc_input_text_with_special_chars_escaped() {
        let msgs = vec![
            InputMessage {
                message_id: 1,
                text: Some("hello".into()),
                timestamp: Utc::now(),
                attachments: vec![],
            },
            InputMessage {
                message_id: 2,
                text: Some("line1\nline2\ttab \"quoted\"".into()),
                timestamp: Utc::now(),
                attachments: vec![],
            },
        ];
        let result = format_cc_input(&msgs).unwrap();
        assert!(result.contains(r#"    text: "line1\nline2\ttab \"quoted\""#));
    }

    #[test]
    fn yaml_escape_handles_all_special_chars() {
        assert_eq!(yaml_escape_string(r#"a"b"#), r#"a\"b"#);
        assert_eq!(yaml_escape_string("a\nb"), r"a\nb");
        assert_eq!(yaml_escape_string("a\\b"), r"a\\b");
        assert_eq!(yaml_escape_string("a\rb"), r"a\rb");
        assert_eq!(yaml_escape_string("a\tb"), r"a\tb");
    }
```

- [ ] **Step 2: Update DebounceMsg in worker.rs**

In `crates/bot/src/telegram/worker.rs`, change the `DebounceMsg` struct (lines 31-36):

```rust
#[derive(Clone)]
pub struct DebounceMsg {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<super::attachments::InboundAttachment>,
}
```

Note: `text` changes from `String` to `Option<String>`.

- [ ] **Step 3: Run attachment tests**

Run: `cargo test -p rightclaw-bot --lib telegram::attachments`
Expected: All tests PASS (worker.rs may not compile yet due to DebounceMsg changes -- that is addressed in later tasks)

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): add YAML input formatting and update DebounceMsg for attachments"
```

---

### Task 3: Update reply schema and agent_def codegen

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs:7` (REPLY_SCHEMA_JSON)
- Modify: `crates/rightclaw/src/codegen/agent_def_tests.rs:157-194` (schema test)

- [ ] **Step 1: Write failing test for new schema**

In `crates/rightclaw/src/codegen/agent_def_tests.rs`, replace test `reply_schema_json_is_valid_and_has_required_fields` (lines 157-194):

```rust
/// Test 6: REPLY_SCHEMA_JSON has typed attachments (not media_paths)
#[test]
fn reply_schema_json_is_valid_and_has_attachments() {
    let value: serde_json::Value =
        serde_json::from_str(REPLY_SCHEMA_JSON).expect("REPLY_SCHEMA_JSON must be valid JSON");

    assert_eq!(value["type"].as_str(), Some("object"));

    let props = &value["properties"];
    assert!(!props["content"].is_null(), "must have 'content' property");
    assert!(
        !props["reply_to_message_id"].is_null(),
        "must have 'reply_to_message_id' property"
    );

    // media_paths must NOT exist (replaced by attachments)
    assert!(
        props["media_paths"].is_null(),
        "media_paths must be removed from schema"
    );

    // attachments must exist with correct structure
    let atts = &props["attachments"];
    assert!(!atts.is_null(), "must have 'attachments' property");
    let items = &atts["items"];
    assert!(!items.is_null(), "attachments must have 'items'");
    let item_props = &items["properties"];
    assert!(!item_props["type"].is_null(), "attachment items must have 'type'");
    assert!(!item_props["path"].is_null(), "attachment items must have 'path'");
    assert!(!item_props["filename"].is_null(), "attachment items must have 'filename'");
    assert!(!item_props["caption"].is_null(), "attachment items must have 'caption'");

    // type must be an enum with expected variants
    let type_enum = items["properties"]["type"]["enum"]
        .as_array()
        .expect("type must be an enum");
    assert!(type_enum.iter().any(|v| v.as_str() == Some("photo")));
    assert!(type_enum.iter().any(|v| v.as_str() == Some("document")));
    assert!(type_enum.iter().any(|v| v.as_str() == Some("video")));
    assert!(type_enum.iter().any(|v| v.as_str() == Some("voice")));

    // required must include "content"
    let required = value["required"].as_array().expect("required must be an array");
    assert!(required.iter().any(|v| v.as_str() == Some("content")));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib codegen::agent_def::tests::reply_schema_json_is_valid_and_has_attachments`
Expected: FAIL -- `media_paths must be removed from schema`

- [ ] **Step 3: Update REPLY_SCHEMA_JSON constant**

In `crates/rightclaw/src/codegen/agent_def.rs`, replace line 7:

```rust
pub const REPLY_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"content":{"type":["string","null"]},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]}"#;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rightclaw --lib codegen::agent_def::tests`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs crates/rightclaw/src/codegen/agent_def_tests.rs
git commit -m "feat(codegen): replace media_paths with typed attachments in reply schema"
```

---

### Task 4: Add system prompt section for input/output format

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs:38-75`
- Modify: `crates/rightclaw/src/codegen/agent_def_tests.rs`

- [ ] **Step 1: Write failing test for system prompt section**

Add to `crates/rightclaw/src/codegen/agent_def_tests.rs`:

```rust
/// Test: generated agent definition includes message input/output format documentation
#[test]
fn agent_definition_includes_attachment_format_docs() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");

    let agent = make_agent_at(tmp.path().to_path_buf(), None, false, false, false);
    let result = generate_agent_definition(&agent).unwrap();

    assert!(
        result.contains("## Message Input Format"),
        "must contain input format section"
    );
    assert!(
        result.contains("## Sending Attachments"),
        "must contain output format section"
    );
    assert!(
        result.contains("/sandbox/outbox/"),
        "must mention outbox directory"
    );
    assert!(
        result.contains("Photos: max 10MB"),
        "must mention photo size limit"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib codegen::agent_def::tests::agent_definition_includes_attachment_format_docs`
Expected: FAIL

- [ ] **Step 3: Add system prompt constant and append to generate_agent_definition**

In `crates/rightclaw/src/codegen/agent_def.rs`, add after `REPLY_SCHEMA_JSON`:

```rust
/// System prompt section describing message input/output format for CC agents.
const ATTACHMENT_FORMAT_DOCS: &str = "\n\n## Message Input Format\n\n\
You receive user messages via stdin in one of two formats:\n\n\
1. **Plain text** -- a single message with no attachments\n\
2. **YAML** -- multiple messages or messages with attachments, with a `messages:` root key\n\n\
YAML schema:\n\
```yaml\n\
messages:\n\
  - id: <telegram_message_id>\n\
    ts: <ISO 8601 timestamp>\n\
    text: <message text or caption>\n\
    attachments:\n\
      - type: photo|document|video|audio|voice|video_note|sticker|animation\n\
        path: <absolute path to file>\n\
        mime_type: <MIME type>\n\
        filename: <original filename, documents only>\n\
```\n\n\
Use the Read tool to view images and files at the given paths.\n\n\
## Sending Attachments\n\n\
Write files to /sandbox/outbox/ (or the outbox/ directory in your working directory).\n\
Include them in your JSON response under the `attachments` array.\n\n\
Size limits enforced by the bot:\n\
- Photos: max 10MB\n\
- Documents, videos, audio, voice, animations: max 50MB\n\n\
Do not produce files exceeding these limits. If you need to send large data,\n\
split into multiple smaller files or use a different format.\n";
```

Then modify `generate_agent_definition`'s final `Ok(format!(...))` to append:

```rust
    Ok(format!(
        "---\nname: {}\nmodel: {}\ndescription: \"RightClaw agent: {}\"\n---\n\n{}{}",
        agent.name, model, agent.name, body, ATTACHMENT_FORMAT_DOCS
    ))
```

- [ ] **Step 4: Fix test `soul_present_user_absent_skips_user`**

This test asserts exact body equality and will now fail. Replace its assertion (lines 148-154):

```rust
    assert!(result.contains("soul-text"), "must contain soul section");
    assert!(result.contains("## Message Input Format"), "must contain attachment format docs");
    let soul_pos = result.find("soul-text").unwrap();
    let format_pos = result.find("## Message Input Format").unwrap();
    assert!(format_pos > soul_pos, "attachment format docs must come after body sections");
```

- [ ] **Step 5: Run all agent_def tests**

Run: `cargo test -p rightclaw --lib codegen::agent_def::tests`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs crates/rightclaw/src/codegen/agent_def_tests.rs
git commit -m "feat(codegen): add message input/output format docs to agent definition"
```

---

### Task 5: Extract attachments from Telegram messages

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`

- [ ] **Step 1: Write extract_attachments function**

Add to `crates/bot/src/telegram/attachments.rs`:

```rust
use teloxide::types::Message;

/// Extract all attachments from a Telegram message.
///
/// Checks: photo, document, video, audio, voice, video_note, sticker, animation.
/// For photo, picks the highest-resolution variant (last in array).
pub fn extract_attachments(msg: &Message) -> Vec<InboundAttachment> {
    let mut attachments = Vec::new();

    if let Some(photos) = msg.photo() {
        if let Some(best) = photos.last() {
            attachments.push(InboundAttachment {
                file_id: best.file.id.clone(),
                kind: AttachmentKind::Photo,
                mime_type: Some("image/jpeg".into()),
                filename: None,
                file_size: best.file.size.map(|s| s as u32),
            });
        }
    }

    if let Some(doc) = msg.document() {
        attachments.push(InboundAttachment {
            file_id: doc.file.id.clone(),
            kind: AttachmentKind::Document,
            mime_type: doc.mime_type.as_ref().map(|m| m.to_string()),
            filename: doc.file_name.clone(),
            file_size: doc.file.size.map(|s| s as u32),
        });
    }

    if let Some(video) = msg.video() {
        attachments.push(InboundAttachment {
            file_id: video.file.id.clone(),
            kind: AttachmentKind::Video,
            mime_type: video.mime_type.as_ref().map(|m| m.to_string()),
            filename: video.file_name.clone(),
            file_size: video.file.size.map(|s| s as u32),
        });
    }

    if let Some(audio) = msg.audio() {
        attachments.push(InboundAttachment {
            file_id: audio.file.id.clone(),
            kind: AttachmentKind::Audio,
            mime_type: audio.mime_type.as_ref().map(|m| m.to_string()),
            filename: audio.file_name.clone(),
            file_size: audio.file.size.map(|s| s as u32),
        });
    }

    if let Some(voice) = msg.voice() {
        attachments.push(InboundAttachment {
            file_id: voice.file.id.clone(),
            kind: AttachmentKind::Voice,
            mime_type: voice.mime_type.as_ref().map(|m| m.to_string()),
            filename: None,
            file_size: voice.file.size.map(|s| s as u32),
        });
    }

    if let Some(video_note) = msg.video_note() {
        attachments.push(InboundAttachment {
            file_id: video_note.file.id.clone(),
            kind: AttachmentKind::VideoNote,
            mime_type: Some("video/mp4".into()),
            filename: None,
            file_size: video_note.file.size.map(|s| s as u32),
        });
    }

    if let Some(sticker) = msg.sticker() {
        let mime = if sticker.is_video {
            "video/webm"
        } else if sticker.is_animated {
            "application/x-tgsticker"
        } else {
            "image/webp"
        };
        attachments.push(InboundAttachment {
            file_id: sticker.file.id.clone(),
            kind: AttachmentKind::Sticker,
            mime_type: Some(mime.into()),
            filename: None,
            file_size: sticker.file.size.map(|s| s as u32),
        });
    }

    if let Some(animation) = msg.animation() {
        attachments.push(InboundAttachment {
            file_id: animation.file.id.clone(),
            kind: AttachmentKind::Animation,
            mime_type: animation.mime_type.as_ref().map(|m| m.to_string()),
            filename: animation.file_name.clone(),
            file_size: animation.file.size.map(|s| s as u32),
        });
    }

    attachments
}
```

**Important:** The exact field access patterns (`.file.id`, `.file.size`, `.mime_type`, `.file_name`) must be verified against teloxide 0.17's types. Use context7 MCP to check `teloxide::types::PhotoSize`, `Document`, `Video`, `Audio`, `Voice`, `VideoNote`, `Sticker`, `Animation` structs. Fields may be named differently (e.g., `file_id` vs `file.id`).

- [ ] **Step 2: Run compilation check**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles (or reveals teloxide field name differences to fix)

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(bot): add extract_attachments for all Telegram media types"
```

---

### Task 6: Update handler.rs to extract attachments

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs:79-105`

- [ ] **Step 1: Replace text-only extraction with text+attachments**

In `crates/bot/src/telegram/handler.rs`, replace lines 79-105 with:

```rust
    // Extract text from message body OR caption (media messages use captions)
    let text = msg.text().or(msg.caption()).map(|t| t.to_string());

    // Extract attachments from all media types
    let attachments = super::attachments::extract_attachments(&msg);

    // Skip messages with neither text nor attachments
    if text.is_none() && attachments.is_empty() {
        return Ok(());
    }

    // Intercept auth code: if login flow is waiting for a code, forward this message.
    if let Some(ref text_val) = text {
        let mut slot = auth_code_slot.0.lock().await;
        if let Some(sender) = slot.take() {
            tracing::info!("handle_message: forwarding message as auth code");
            let _ = sender.send(text_val.clone());
            return Ok(());
        }
    }

    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);
    let worker_exists = worker_map.contains_key(&key);
    tracing::info!(?key, worker_exists, has_text = text.is_some(), attachment_count = attachments.len(), "handle_message: routing");

    let debounce_msg = DebounceMsg {
        message_id: msg.id.0,
        text,
        timestamp: chrono::Utc::now(),
        attachments,
    };
```

- [ ] **Step 2: Run compilation check**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles (remaining errors in worker.rs are addressed in Task 7)

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/handler.rs
git commit -m "feat(bot): extract text and attachments from all Telegram media types in handler"
```

---

### Task 7: Switch worker.rs to stdin piping with YAML input

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

This is the largest task. Changes:
1. `ReplyOutput`: replace `media_paths` with `attachments`
2. `parse_reply_output`: update for new schema
3. `invoke_cc`: stdin piping, remove positional arg
4. `spawn_worker`: download attachments, use `format_cc_input`, outbound attachment sending
5. Remove `format_batch_xml`

- [ ] **Step 1: Update ReplyOutput struct**

Replace `ReplyOutput` (lines 61-67):

```rust
#[derive(Debug, serde::Deserialize)]
pub struct ReplyOutput {
    pub content: Option<String>,
    pub reply_to_message_id: Option<i32>,
    pub attachments: Option<Vec<super::attachments::OutboundAttachment>>,
}
```

- [ ] **Step 2: Update parse_reply_output**

In `parse_reply_output` (lines 205-242):

a) Change the plain-string fallback (around line 225) to use `attachments: None` instead of `media_paths: None`:
```rust
        ReplyOutput {
            content: if text.is_empty() { None } else { Some(text.to_string()) },
            reply_to_message_id: None,
            attachments: None,
        }
```

b) Remove the `media_paths` warning block (lines 235-239 -- the `if let Some(ref paths) = output.media_paths` block).

- [ ] **Step 3: Add tests for parse_reply_output**

Add to `worker.rs` (or create `worker_tests.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reply_output_with_attachments() {
        let json = r#"{"session_id":"abc","result":{"content":"Here you go","attachments":[{"type":"document","path":"/sandbox/outbox/data.csv","filename":"results.csv","caption":"Exported data"}]}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Here you go"));
        assert_eq!(session_id.as_deref(), Some("abc"));
        let atts = output.attachments.unwrap();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].path, "/sandbox/outbox/data.csv");
        assert_eq!(atts[0].filename.as_deref(), Some("results.csv"));
    }

    #[test]
    fn parse_reply_output_text_only() {
        let json = r#"{"result":{"content":"hello","reply_to_message_id":null,"attachments":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello"));
        assert!(output.attachments.is_none());
    }

    #[test]
    fn parse_reply_output_plain_string_fallback() {
        let json = r#"{"result":"plain text fallback"}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("plain text fallback"));
        assert!(output.attachments.is_none());
    }
}
```

- [ ] **Step 4: Change invoke_cc to use stdin piping**

In `invoke_cc` (lines 544-770):

a) Change signature -- replace `xml: &str` with `input: &str`:
```rust
async fn invoke_cc(
    input: &str,
    chat_id: i64,
    eff_thread_id: i64,
    ctx: &WorkerContext,
) -> Result<Option<ReplyOutput>, String> {
```

b) Remove the positional arg lines (615-616). Delete:
```rust
    claude_args.push("--".into());
    claude_args.push(xml.to_string());
```

c) Change `Stdio::null()` to `Stdio::piped()` (line 646):
```rust
    cmd.stdin(Stdio::piped());
```

d) Replace the spawn + wait section. Instead of:
```rust
    let child = cmd
        .spawn()
        .map_err(|e| format_error_reply(-1, &format!("spawn failed: {:#}", e)))?;
    let output = wait_with_timeout(child, CC_TIMEOUT_SECS).await?;
```

Use:
```rust
    let mut child = cmd
        .spawn()
        .map_err(|e| format_error_reply(-1, &format!("spawn failed: {:#}", e)))?;

    // Write input to stdin, then drop to signal EOF.
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(input.as_bytes()).await
            .map_err(|e| format_error_reply(-1, &format!("stdin write failed: {:#}", e)))?;
        // stdin dropped here -- signals EOF to child process
    }

    let output = wait_with_timeout(child, CC_TIMEOUT_SECS).await?;
```

- [ ] **Step 5: Remove format_batch_xml function**

Delete `format_batch_xml` (lines 79-96).

- [ ] **Step 6: Update spawn_worker to download attachments and use new format**

In `spawn_worker`, replace the XML batch building and invoke_cc call (around lines 317-340):

```rust
            // Download attachments for all messages in batch
            let mut input_messages = Vec::with_capacity(batch.len());
            for msg in &batch {
                let resolved = if msg.attachments.is_empty() {
                    vec![]
                } else {
                    match super::attachments::download_attachments(
                        &msg.attachments,
                        msg.message_id,
                        &ctx.bot,
                        &ctx.agent_dir,
                        ctx.ssh_config_path.as_deref(),
                        &ctx.agent_name,
                        tg_chat_id,
                        eff_thread_id,
                    ).await {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!(?key, "attachment download failed: {:#}", e);
                            let _ = send_tg(&ctx.bot, tg_chat_id, eff_thread_id, &format!("Failed to download attachments: {e}")).await;
                            vec![]
                        }
                    }
                };
                input_messages.push(super::attachments::InputMessage {
                    message_id: msg.message_id,
                    text: msg.text.clone(),
                    timestamp: msg.timestamp,
                    attachments: resolved,
                });
            }

            let Some(input) = super::attachments::format_cc_input(&input_messages) else {
                tracing::warn!(?key, "empty input after formatting -- skipping CC invocation");
                continue;
            };

            // ... typing indicator code stays the same ...

            let reply_result = invoke_cc(&input, chat_id, eff_thread_id, &ctx).await;
```

- [ ] **Step 7: Update reply sending to handle outbound attachments**

Replace the reply sending block (lines 347-401) in spawn_worker:

```rust
                Ok(Some(output)) => {
                    // Single-message input: auto reply_to the triggering message
                    let reply_to = if batch.len() == 1 {
                        Some(batch[0].message_id)
                    } else {
                        output.reply_to_message_id
                    };

                    if let Some(content) = output.content {
                        let parts = split_message(&content);
                        tracing::info!(
                            ?key,
                            content_len = content.len(),
                            parts = parts.len(),
                            ?reply_to,
                            "sending reply to Telegram"
                        );
                        for part in parts {
                            let mut send = ctx.bot.send_message(tg_chat_id, &part);
                            if eff_thread_id != 0 {
                                send = send.message_thread_id(ThreadId(MessageId(
                                    eff_thread_id as i32,
                                )));
                            }
                            if let Some(ref_id) = reply_to {
                                send = send.reply_parameters(ReplyParameters {
                                    message_id: MessageId(ref_id),
                                    ..Default::default()
                                });
                            }
                            if let Err(e) = send.await {
                                tracing::error!(?key, "failed to send Telegram reply: {:#}", e);
                            }
                        }
                    } else {
                        tracing::warn!(
                            ?key,
                            "CC returned content: null -- no text reply sent"
                        );
                    }

                    // Send outbound attachments
                    if let Some(ref atts) = output.attachments {
                        if !atts.is_empty() {
                            if let Err(e) = super::attachments::send_attachments(
                                atts,
                                &ctx.bot,
                                tg_chat_id,
                                eff_thread_id,
                                &ctx.agent_dir,
                                ctx.ssh_config_path.as_deref(),
                                &ctx.agent_name,
                            ).await {
                                tracing::error!(?key, "failed to send attachments: {:#}", e);
                                let _ = send_tg(&ctx.bot, tg_chat_id, eff_thread_id, &format!("Failed to send attachments: {e}")).await;
                            }
                        }
                    }
                }
```

- [ ] **Step 8: Make send_tg pub(crate)**

Change `send_tg` visibility so `attachments.rs` can call it:

```rust
pub(crate) async fn send_tg(...)
```

- [ ] **Step 9: Run compilation check**

Run: `cargo check -p rightclaw-bot`
Expected: May fail -- `download_attachments` and `send_attachments` don't exist yet. That is OK, they are implemented in Tasks 8 and 9.

- [ ] **Step 10: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): switch to stdin piping, YAML input, typed attachment output in worker"
```

---

### Task 8: Implement download_attachments

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`

- [ ] **Step 1: Add download_attachments function**

```rust
use teloxide::requests::Requester;
use tokio::io::AsyncWriteExt;

/// Download inbound attachments from Telegram, upload to sandbox (or move to agent dir).
///
/// Returns resolved attachments with filesystem paths. Skips attachments exceeding 20MB
/// and notifies the user for each skipped one.
pub async fn download_attachments(
    attachments: &[InboundAttachment],
    message_id: i32,
    bot: &super::BotType,
    agent_dir: &std::path::Path,
    ssh_config_path: Option<&std::path::Path>,
    agent_name: &str,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
) -> Result<Vec<ResolvedAttachment>, String> {
    let mut resolved = Vec::new();
    let tmp_dir = agent_dir.join("tmp").join("inbox");
    tokio::fs::create_dir_all(&tmp_dir).await
        .map_err(|e| format!("failed to create tmp/inbox: {e:#}"))?;

    for (idx, att) in attachments.iter().enumerate() {
        // Check size limit
        if let Some(size) = att.file_size {
            if u64::from(size) > TELEGRAM_DOWNLOAD_LIMIT {
                let label = att.filename.as_deref().unwrap_or(att.kind.as_str());
                let msg = format!("Attachment skipped: {label} exceeds 20MB limit");
                let _ = super::worker::send_tg(bot, chat_id, eff_thread_id, &msg).await;
                continue;
            }
        }

        // Determine extension and filename
        let mime = att.mime_type.as_deref().unwrap_or("application/octet-stream");
        let ext = mime_to_extension(mime);
        let file_name = format!("{}_{}_{}.{}", att.kind.as_str(), message_id, idx, ext);

        // Download from Telegram
        let file = bot.get_file(&att.file_id).await
            .map_err(|e| format!("get_file failed: {e:#}"))?;
        let host_path = tmp_dir.join(&file_name);
        let mut dst = tokio::fs::File::create(&host_path).await
            .map_err(|e| format!("failed to create {}: {e:#}", host_path.display()))?;
        bot.download_file(&file.path, &mut dst).await
            .map_err(|e| format!("download_file failed: {e:#}"))?;
        dst.flush().await.map_err(|e| format!("flush failed: {e:#}"))?;

        // Upload to sandbox or move to agent inbox
        let final_path = if ssh_config_path.is_some() {
            let sandbox_path = format!("{}/{}", SANDBOX_INBOX, file_name);
            rightclaw::openshell::upload_file(agent_name, &host_path, &sandbox_path)
                .await
                .map_err(|e| format!("sandbox upload failed: {e:#}"))?;
            // Clean up host temp file
            let _ = tokio::fs::remove_file(&host_path).await;
            PathBuf::from(sandbox_path)
        } else {
            let inbox_dir = agent_dir.join("inbox");
            tokio::fs::create_dir_all(&inbox_dir).await
                .map_err(|e| format!("failed to create inbox: {e:#}"))?;
            let dest = inbox_dir.join(&file_name);
            tokio::fs::rename(&host_path, &dest).await
                .map_err(|e| format!("failed to move to inbox: {e:#}"))?;
            dest
        };

        resolved.push(ResolvedAttachment {
            kind: att.kind,
            path: final_path,
            mime_type: mime.to_string(),
            filename: att.filename.clone(),
        });
    }

    Ok(resolved)
}
```

**Important:** Verify `rightclaw::openshell::upload_file` signature. It takes `(sandbox: &str, host_path: &Path, sandbox_path: &str)`. The `agent_name` serves as sandbox identifier. Also verify `bot.get_file()` and `bot.download_file()` signatures against teloxide 0.17 -- use context7 MCP.

- [ ] **Step 2: Run compilation check**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles (or reveals API differences to fix)

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(bot): implement attachment download and sandbox upload pipeline"
```

---

### Task 9: Implement send_attachments (outbound pipeline)

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`

- [ ] **Step 1: Add send_attachments function**

```rust
use teloxide::types::{InputFile, MessageId, ThreadId};

/// Send outbound attachments from CC response to Telegram.
///
/// Validates paths are under allowed outbox directory. Downloads from sandbox if needed.
/// Sends each attachment via the appropriate Telegram method.
pub async fn send_attachments(
    attachments: &[OutboundAttachment],
    bot: &super::BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    agent_dir: &std::path::Path,
    ssh_config_path: Option<&std::path::Path>,
    agent_name: &str,
) -> Result<(), String> {
    let outbox_prefix = if ssh_config_path.is_some() {
        SANDBOX_OUTBOX.to_string()
    } else {
        agent_dir.join("outbox").to_string_lossy().into_owned()
    };

    for att in attachments {
        // Validate path is under outbox
        if !att.path.starts_with(&outbox_prefix) {
            let msg = format!("Attachment rejected: path {} is outside outbox", att.path);
            let _ = super::worker::send_tg(bot, chat_id, eff_thread_id, &msg).await;
            continue;
        }

        // Download from sandbox to host temp if sandboxed
        let host_path = if ssh_config_path.is_some() {
            let tmp_dir = agent_dir.join("tmp").join("outbox");
            tokio::fs::create_dir_all(&tmp_dir).await
                .map_err(|e| format!("failed to create tmp/outbox: {e:#}"))?;
            let filename = std::path::Path::new(&att.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file");
            let dest = tmp_dir.join(filename);
            rightclaw::openshell::download_file(agent_name, &att.path, &dest)
                .await
                .map_err(|e| format!("sandbox download failed: {e:#}"))?;
            dest
        } else {
            PathBuf::from(&att.path)
        };

        // Check file size
        let metadata = tokio::fs::metadata(&host_path).await
            .map_err(|e| format!("failed to stat {}: {e:#}", host_path.display()))?;
        let size = metadata.len();
        let size_limit = match att.kind {
            OutboundKind::Photo => TELEGRAM_PHOTO_UPLOAD_LIMIT,
            _ => TELEGRAM_FILE_UPLOAD_LIMIT,
        };
        if size > size_limit {
            let label = att.filename.as_deref()
                .unwrap_or(host_path.file_name().and_then(|n| n.to_str()).unwrap_or("file"));
            let limit_mb = size_limit / (1024 * 1024);
            let msg = format!("Attachment too large: {label} ({limit_mb} MB limit)");
            let _ = super::worker::send_tg(bot, chat_id, eff_thread_id, &msg).await;
            if ssh_config_path.is_some() {
                let _ = tokio::fs::remove_file(&host_path).await;
            }
            continue;
        }

        let input_file = InputFile::file(&host_path);
        let thread_id = if eff_thread_id != 0 {
            Some(ThreadId(MessageId(eff_thread_id as i32)))
        } else {
            None
        };

        let send_result = match att.kind {
            OutboundKind::Photo => {
                let mut req = bot.send_photo(chat_id, input_file);
                if let Some(ref cap) = att.caption { req = req.caption(cap); }
                if let Some(tid) = thread_id { req = req.message_thread_id(tid); }
                req.await.map(|_| ())
            }
            OutboundKind::Document => {
                let mut req = bot.send_document(chat_id, input_file);
                if let Some(ref cap) = att.caption { req = req.caption(cap); }
                if let Some(tid) = thread_id { req = req.message_thread_id(tid); }
                req.await.map(|_| ())
            }
            OutboundKind::Video => {
                let mut req = bot.send_video(chat_id, input_file);
                if let Some(ref cap) = att.caption { req = req.caption(cap); }
                if let Some(tid) = thread_id { req = req.message_thread_id(tid); }
                req.await.map(|_| ())
            }
            OutboundKind::Audio => {
                let mut req = bot.send_audio(chat_id, input_file);
                if let Some(ref cap) = att.caption { req = req.caption(cap); }
                if let Some(tid) = thread_id { req = req.message_thread_id(tid); }
                req.await.map(|_| ())
            }
            OutboundKind::Voice => {
                let mut req = bot.send_voice(chat_id, input_file);
                if let Some(ref cap) = att.caption { req = req.caption(cap); }
                if let Some(tid) = thread_id { req = req.message_thread_id(tid); }
                req.await.map(|_| ())
            }
            OutboundKind::VideoNote => {
                let mut req = bot.send_video_note(chat_id, input_file);
                if let Some(tid) = thread_id { req = req.message_thread_id(tid); }
                req.await.map(|_| ())
            }
            OutboundKind::Sticker => {
                let mut req = bot.send_sticker(chat_id, input_file);
                if let Some(tid) = thread_id { req = req.message_thread_id(tid); }
                req.await.map(|_| ())
            }
            OutboundKind::Animation => {
                let mut req = bot.send_animation(chat_id, input_file);
                if let Some(ref cap) = att.caption { req = req.caption(cap); }
                if let Some(tid) = thread_id { req = req.message_thread_id(tid); }
                req.await.map(|_| ())
            }
        };

        if let Err(e) = send_result {
            tracing::error!("failed to send attachment {}: {e:#}", att.path);
        }

        // Clean up temp file if sandboxed
        if ssh_config_path.is_some() {
            let _ = tokio::fs::remove_file(&host_path).await;
        }
    }

    Ok(())
}
```

**Note:** Verify teloxide 0.17's `send_document`, `send_video_note`, `send_sticker` APIs with context7. Some methods may not have `.caption()` or `.message_thread_id()`.

- [ ] **Step 2: Run compilation check**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(bot): implement outbound attachment sending pipeline"
```

---

### Task 10: Add AttachmentsConfig to AgentConfig

**Files:**
- Modify: `crates/rightclaw/src/agent/types.rs:47-87`
- Modify: `crates/bot/src/lib.rs:67-78` (fallback config)
- Modify: `crates/rightclaw/src/codegen/agent_def_tests.rs:15-26` (test helper)

- [ ] **Step 1: Write failing tests for AttachmentsConfig**

Add to the existing test module in `crates/rightclaw/src/agent/types.rs` (or its test file):

```rust
#[test]
fn agent_config_with_attachments_section() {
    let yaml = r#"
attachments:
  retention_days: 14
"#;
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    assert_eq!(config.attachments.retention_days, 14);
}

#[test]
fn agent_config_default_attachments() {
    let yaml = "";
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    assert_eq!(config.attachments.retention_days, 7);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib agent::types`
Expected: FAIL -- no `attachments` field on `AgentConfig`

- [ ] **Step 3: Add AttachmentsConfig struct and field**

In `crates/rightclaw/src/agent/types.rs`, add before `AgentConfig`:

```rust
/// Configuration for attachment handling.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentsConfig {
    /// How long to keep inbox/outbox files before cleanup (days).
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

impl Default for AttachmentsConfig {
    fn default() -> Self {
        Self {
            retention_days: default_retention_days(),
        }
    }
}

fn default_retention_days() -> u32 {
    7
}
```

Add field to `AgentConfig`:

```rust
    /// Attachment handling configuration.
    #[serde(default)]
    pub attachments: AttachmentsConfig,
```

- [ ] **Step 4: Fix all places that construct AgentConfig manually**

Add `attachments: Default::default()` to:

a) `crates/bot/src/lib.rs` fallback config (lines 67-78):
```rust
    let config = parse_agent_config(&agent_dir)?.unwrap_or_else(|| {
        rightclaw::agent::types::AgentConfig {
            // ... existing fields ...
            secret: None,
            attachments: Default::default(),
        }
    });
```

b) `crates/rightclaw/src/codegen/agent_def_tests.rs` test helper (lines 15-26):
```rust
    let config = model.map(|m| AgentConfig {
        // ... existing fields ...
        secret: None,
        attachments: Default::default(),
    });
```

c) Search for any other `AgentConfig { ... }` constructions:
Run: `cargo check --workspace` to find all compilation errors.

- [ ] **Step 5: Run tests**

Run: `cargo test -p rightclaw --lib agent::types`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/agent/types.rs crates/bot/src/lib.rs crates/rightclaw/src/codegen/agent_def_tests.rs
git commit -m "feat(config): add attachments.retention_days to AgentConfig"
```

---

### Task 11: Create inbox/outbox directories on bot startup

**Files:**
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Add host directory creation after agent_dir resolution**

In `crates/bot/src/lib.rs`, after the agent_dir resolution (around line 63), add:

```rust
    // Create inbox/outbox directories for attachment handling
    for subdir in &["inbox", "outbox", "tmp/inbox", "tmp/outbox"] {
        let dir = agent_dir.join(subdir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| miette::miette!("failed to create {}: {e:#}", dir.display()))?;
    }
```

- [ ] **Step 2: Add sandbox directory creation after SSH config is ready**

Find the location in `lib.rs` where sandbox is ready and SSH config is available (after `generate_ssh_config` and sandbox creation/reuse). Add:

```rust
    // Create inbox/outbox inside sandbox
    if !args.no_sandbox {
        if let Some(ref ssh_cfg) = ssh_config_path {
            let ssh_host = rightclaw::openshell::ssh_host(&args.agent);
            for dir in &["/sandbox/inbox", "/sandbox/outbox"] {
                rightclaw::openshell::ssh_exec(ssh_cfg, &ssh_host, &["mkdir", "-p", dir], 10)
                    .await
                    .map_err(|e| miette::miette!("failed to create {dir} in sandbox: {e:#}"))?;
            }
        }
    }
```

**Note:** The implementer must find the exact insertion point by reading `lib.rs` fully. Look for the section after `generate_ssh_config` and before teloxide dispatcher starts.

- [ ] **Step 3: Run compilation check**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "feat(bot): create inbox/outbox directories on startup"
```

---

### Task 12: Implement periodic cleanup task

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Add cleanup functions to attachments.rs**

```rust
/// Spawn a background task that periodically cleans up old attachment files.
pub fn spawn_cleanup_task(
    agent_dir: PathBuf,
    ssh_config_path: Option<PathBuf>,
    agent_name: String,
    retention_days: u32,
) {
    tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(CLEANUP_INTERVAL_SECS);
        loop {
            tokio::time::sleep(interval).await;
            if let Err(e) = run_cleanup(&agent_dir, ssh_config_path.as_deref(), &agent_name, retention_days).await {
                tracing::warn!("attachment cleanup failed: {e:#}");
            }
        }
    });
}

async fn run_cleanup(
    agent_dir: &std::path::Path,
    ssh_config_path: Option<&std::path::Path>,
    agent_name: &str,
    retention_days: u32,
) -> Result<(), String> {
    if let Some(ssh_config) = ssh_config_path {
        let ssh_host = rightclaw::openshell::ssh_host(agent_name);
        let mtime_arg = format!("+{retention_days}");
        rightclaw::openshell::ssh_exec(
            ssh_config,
            &ssh_host,
            &["find", SANDBOX_INBOX, SANDBOX_OUTBOX, "-type", "f", "-mtime", &mtime_arg, "-delete"],
            30,
        ).await
            .map_err(|e| format!("sandbox cleanup failed: {e:#}"))?;
    } else {
        for subdir in &["inbox", "outbox"] {
            let dir = agent_dir.join(subdir);
            if dir.exists() {
                cleanup_local_dir(&dir, retention_days).await?;
            }
        }
    }
    Ok(())
}

async fn cleanup_local_dir(dir: &std::path::Path, retention_days: u32) -> Result<(), String> {
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(u64::from(retention_days) * 86400);

    let mut entries = tokio::fs::read_dir(dir).await
        .map_err(|e| format!("read_dir {} failed: {e:#}", dir.display()))?;

    while let Some(entry) = entries.next_entry().await
        .map_err(|e| format!("next_entry failed: {e:#}"))?
    {
        let metadata = entry.metadata().await
            .map_err(|e| format!("metadata failed: {e:#}"))?;
        if !metadata.is_file() {
            continue;
        }
        if let Ok(modified) = metadata.modified() {
            if modified < cutoff {
                tracing::debug!("cleaning up old attachment: {}", entry.path().display());
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Spawn cleanup task in lib.rs**

In `crates/bot/src/lib.rs`, before the Telegram dispatcher starts (after SSH config is resolved), add:

```rust
    // Spawn periodic attachment cleanup
    telegram::attachments::spawn_cleanup_task(
        agent_dir.clone(),
        ssh_config_path.clone(),
        args.agent.clone(),
        config.attachments.retention_days,
    );
```

The implementer must find the right location -- after `ssh_config_path` is resolved but before `run_telegram()` is called.

- [ ] **Step 3: Run compilation check**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs crates/bot/src/lib.rs
git commit -m "feat(bot): add periodic attachment cleanup task"
```

---

### Task 13: Full workspace build and integration verification

**Files:** None new -- verification only

- [ ] **Step 1: Build entire workspace**

Run: `cargo build --workspace`
Expected: Clean build, no errors

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 4: Fix any issues found**

Common issues to watch for:
- `DebounceMsg.text` changed from `String` to `Option<String>` -- find all `.text` accesses in worker.rs
- Missing imports (teloxide types, tokio::io)
- Teloxide API differences from the code written above (field names, method signatures)
- `send_tg` visibility (must be `pub(crate)`)

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "fix(bot): address build issues from attachment integration"
```

---

### Task 14: Update ARCHITECTURE.md

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update module map**

Add `attachments.rs` to the bot module map under `telegram/`:

```
|   +-- attachments.rs  # Attachment extraction, download/upload, send, cleanup, YAML formatting
```

- [ ] **Step 2: Update data flow**

Add attachment flow to the "Per message:" section:

```
Per message:
  +-- Extract text + attachments from Telegram message
  +-- Check if login flow waiting for auth code -> forward to PTY
  +-- Route to worker task via DashMap<(chat_id, thread_id), Sender>
  +-- Worker: debounce 500ms -> download attachments -> upload to sandbox inbox
  +-- Format input: single text -> raw string, multi/attachments -> YAML
  +-- Pipe input to claude -p via stdin (SSH or direct)
  +-- Parse reply JSON with typed attachments
  +-- Send text reply to Telegram
  +-- Download outbound attachments from sandbox outbox -> send to Telegram
  +-- Periodic cleanup: hourly, configurable retention (default 7 days)
```

- [ ] **Step 3: Update directory layout**

Add inbox/outbox to the runtime directory structure:

```
+-- agents/<name>/
|   +-- inbox/          # Received Telegram attachments (no-sandbox mode)
|   +-- outbox/         # CC-generated files for Telegram (no-sandbox mode)
|   +-- tmp/inbox/      # Temporary download before sandbox upload
|   +-- tmp/outbox/     # Temporary download from sandbox for sending
```

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: update ARCHITECTURE.md with attachment pipeline"
```
