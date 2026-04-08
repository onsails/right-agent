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
