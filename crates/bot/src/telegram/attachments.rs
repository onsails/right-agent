use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::PathBuf;
use teloxide::types::Message;

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

/// Extract all attachments from a Telegram message.
///
/// Checks: photo, document, video, audio, voice, video_note, sticker, animation.
/// For photo, picks the highest-resolution variant (last in the array).
pub fn extract_attachments(msg: &Message) -> Vec<InboundAttachment> {
    let mut out = Vec::new();

    // Photo: array of sizes, last = highest resolution. Always JPEG.
    if let Some(sizes) = msg.photo() {
        if let Some(best) = sizes.last() {
            out.push(InboundAttachment {
                file_id: best.file.id.0.clone(),
                kind: AttachmentKind::Photo,
                mime_type: Some("image/jpeg".to_owned()),
                filename: None,
                file_size: Some(best.file.size),
            });
        }
    }

    // Document
    if let Some(doc) = msg.document() {
        out.push(InboundAttachment {
            file_id: doc.file.id.0.clone(),
            kind: AttachmentKind::Document,
            mime_type: doc.mime_type.as_ref().map(|m| m.to_string()),
            filename: doc.file_name.clone(),
            file_size: Some(doc.file.size),
        });
    }

    // Video
    if let Some(vid) = msg.video() {
        out.push(InboundAttachment {
            file_id: vid.file.id.0.clone(),
            kind: AttachmentKind::Video,
            mime_type: vid.mime_type.as_ref().map(|m| m.to_string()),
            filename: vid.file_name.clone(),
            file_size: Some(vid.file.size),
        });
    }

    // Audio
    if let Some(aud) = msg.audio() {
        out.push(InboundAttachment {
            file_id: aud.file.id.0.clone(),
            kind: AttachmentKind::Audio,
            mime_type: aud.mime_type.as_ref().map(|m| m.to_string()),
            filename: aud.file_name.clone(),
            file_size: Some(aud.file.size),
        });
    }

    // Voice
    if let Some(voice) = msg.voice() {
        out.push(InboundAttachment {
            file_id: voice.file.id.0.clone(),
            kind: AttachmentKind::Voice,
            mime_type: voice.mime_type.as_ref().map(|m| m.to_string()),
            filename: None,
            file_size: Some(voice.file.size),
        });
    }

    // VideoNote — always mp4, no filename
    if let Some(vn) = msg.video_note() {
        out.push(InboundAttachment {
            file_id: vn.file.id.0.clone(),
            kind: AttachmentKind::VideoNote,
            mime_type: Some("video/mp4".to_owned()),
            filename: None,
            file_size: Some(vn.file.size),
        });
    }

    // Sticker — mime depends on format
    if let Some(stk) = msg.sticker() {
        let mime = if stk.is_video() {
            "video/webm"
        } else if stk.is_animated() {
            "application/x-tgsticker"
        } else {
            "image/webp"
        };
        out.push(InboundAttachment {
            file_id: stk.file.id.0.clone(),
            kind: AttachmentKind::Sticker,
            mime_type: Some(mime.to_owned()),
            filename: None,
            file_size: Some(stk.file.size),
        });
    }

    // Animation (GIF)
    if let Some(anim) = msg.animation() {
        out.push(InboundAttachment {
            file_id: anim.file.id.0.clone(),
            kind: AttachmentKind::Animation,
            mime_type: anim.mime_type.as_ref().map(|m| m.to_string()),
            filename: anim.file_name.clone(),
            file_size: Some(anim.file.size),
        });
    }

    out
}

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
}
