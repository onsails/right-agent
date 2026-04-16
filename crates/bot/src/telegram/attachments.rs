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
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct OutboundAttachment {
    #[serde(rename = "type")]
    pub kind: OutboundKind,
    pub path: String,
    pub filename: Option<String>,
    pub caption: Option<String>,
}

/// Attachment kinds CC can produce in output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
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
pub const SANDBOX_INBOX: &str = "/sandbox/inbox/";
pub const SANDBOX_OUTBOX: &str = "/sandbox/outbox/";

/// Telegram user/chat identity for message authorship.
#[derive(Debug, Clone)]
pub struct MessageAuthor {
    pub name: String,
    pub username: Option<String>,
    pub user_id: Option<i64>,
}

/// Forward origin metadata.
#[derive(Debug, Clone)]
pub struct ForwardInfo {
    pub from: MessageAuthor,
    pub date: DateTime<Utc>,
}

/// Message in a debounce batch -- text and/or attachments.
#[derive(Debug, Clone)]
pub struct InputMessage {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<ResolvedAttachment>,
    pub author: MessageAuthor,
    pub forward_info: Option<ForwardInfo>,
    pub reply_to_id: Option<i32>,
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

    // YAML format — use write! to avoid per-field allocations
    use std::fmt::Write;
    let mut out = String::with_capacity(256);
    out.push_str("messages:\n");
    for m in msgs {
        writeln!(out, "  - id: {}", m.message_id).expect("write to String is infallible");
        writeln!(out, "    ts: \"{}\"", m.timestamp.format("%Y-%m-%dT%H:%M:%SZ"))
            .expect("write to String is infallible");
        if let Some(ref text) = m.text {
            let escaped = yaml_escape_string(text);
            writeln!(out, "    text: \"{escaped}\"").expect("write to String is infallible");
        }
        if !m.attachments.is_empty() {
            out.push_str("    attachments:\n");
            for att in &m.attachments {
                writeln!(out, "      - type: {}", att.kind.as_str())
                    .expect("write to String is infallible");
                writeln!(out, "        path: {}", att.path.display())
                    .expect("write to String is infallible");
                writeln!(out, "        mime_type: {}", att.mime_type)
                    .expect("write to String is infallible");
                if let Some(ref fname) = att.filename {
                    let escaped = yaml_escape_string(fname);
                    writeln!(out, "        filename: \"{escaped}\"")
                        .expect("write to String is infallible");
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
    if let Some(sizes) = msg.photo()
        && let Some(best) = sizes.last()
    {
        out.push(InboundAttachment {
            file_id: best.file.id.0.clone(),
            kind: AttachmentKind::Photo,
            mime_type: Some("image/jpeg".to_owned()),
            filename: None,
            file_size: Some(best.file.size),
        });
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

/// Download inbound attachments from Telegram, save to disk, optionally upload to sandbox.
#[allow(clippy::too_many_arguments)]
pub async fn download_attachments(
    attachments: &[InboundAttachment],
    message_id: i32,
    bot: &super::BotType,
    agent_dir: &std::path::Path,
    ssh_config_path: Option<&std::path::Path>,
    resolved_sandbox: Option<&str>,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
) -> Result<Vec<ResolvedAttachment>, Box<dyn std::error::Error + Send + Sync>> {
    use teloxide::net::Download;
    use teloxide::requests::Requester;
    use tokio::io::AsyncWriteExt;

    let tmp_dir = agent_dir.join("tmp/inbox");
    tokio::fs::create_dir_all(&tmp_dir).await?;

    let sandboxed = ssh_config_path.is_some();
    if !sandboxed {
        tokio::fs::create_dir_all(agent_dir.join("inbox")).await?;
    }

    let mut resolved = Vec::with_capacity(attachments.len());

    for (idx, att) in attachments.iter().enumerate() {
        // Check size limit
        if let Some(size) = att.file_size
            && u64::from(size) > TELEGRAM_DOWNLOAD_LIMIT
        {
            let msg = format!(
                "Skipping {} attachment ({:.1} MB) — exceeds 20 MB Telegram download limit.",
                att.kind.as_str(),
                f64::from(size) / (1024.0 * 1024.0),
            );
            if let Err(e) = super::worker::send_tg(bot, chat_id, eff_thread_id, &msg).await {
                tracing::warn!("Failed to notify user about oversized attachment: {e}");
            }
            continue;
        }

        let mime = att
            .mime_type
            .as_deref()
            .unwrap_or("application/octet-stream");
        let ext = mime_to_extension(mime);
        let file_name = format!("{}_{message_id}_{idx}.{ext}", att.kind.as_str());

        // Download from Telegram
        let file = bot
            .get_file(teloxide::types::FileId(att.file_id.clone()))
            .await?;
        let host_path = tmp_dir.join(&file_name);
        let mut dst = tokio::fs::File::create(&host_path).await?;
        bot.download_file(&file.path, &mut dst).await?;
        dst.flush().await?;

        let final_path = if sandboxed {
            // Upload to sandbox, then clean up host temp file
            let sandbox = resolved_sandbox.unwrap();
            rightclaw::openshell::upload_file(sandbox, &host_path, SANDBOX_INBOX).await?;
            if let Err(e) = tokio::fs::remove_file(&host_path).await {
                tracing::warn!("Failed to remove temp file {}: {e}", host_path.display());
            }
            PathBuf::from(format!("{SANDBOX_INBOX}{file_name}"))
        } else {
            // Move to inbox
            let dest = agent_dir.join("inbox").join(&file_name);
            tokio::fs::rename(&host_path, &dest).await?;
            dest
        };

        resolved.push(ResolvedAttachment {
            kind: att.kind,
            path: final_path,
            mime_type: mime.to_owned(),
            filename: att.filename.clone(),
        });
    }

    Ok(resolved)
}

/// Download outbound attachments from sandbox and send to Telegram.
pub async fn send_attachments(
    attachments: &[OutboundAttachment],
    bot: &super::BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    agent_dir: &std::path::Path,
    ssh_config_path: Option<&std::path::Path>,
    resolved_sandbox: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use teloxide::payloads::{
        SendAnimationSetters, SendAudioSetters, SendDocumentSetters, SendPhotoSetters,
        SendStickerSetters, SendVideoNoteSetters, SendVideoSetters, SendVoiceSetters,
    };
    use teloxide::requests::Requester;
    use teloxide::types::{InputFile, MessageId, ThreadId};

    let sandboxed = ssh_config_path.is_some();
    let outbox_prefix = if sandboxed {
        SANDBOX_OUTBOX.to_owned()
    } else {
        agent_dir.join("outbox").to_string_lossy().into_owned()
    };
    let outbox_path = agent_dir.join("outbox");

    // Pre-create tmp/outbox for sandboxed downloads (avoids repeated create_dir_all in loop)
    if sandboxed {
        tokio::fs::create_dir_all(agent_dir.join("tmp/outbox")).await?;
    }

    let mut errors: Vec<String> = Vec::new();
    for att in attachments {
        // Validate path is within outbox
        if !att.path.starts_with(&outbox_prefix) {
            tracing::warn!(
                "Outbound attachment path {} is outside outbox prefix {outbox_prefix} — skipping",
                att.path,
            );
            continue;
        }

        // Resolve to host path
        let host_path = if sandboxed {
            let tmp_dir = agent_dir.join("tmp/outbox");
            let file_name = std::path::Path::new(&att.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let dest = tmp_dir.join(&file_name);
            let sandbox = resolved_sandbox.unwrap();
            rightclaw::openshell::download_file(sandbox, &att.path, &dest).await?;
            dest
        } else {
            // Canonicalize to resolve any `..` components and verify the path
            // truly resides under the outbox directory (string prefix check above
            // is insufficient against paths like `outbox/../../etc/passwd`).
            let canonical = std::fs::canonicalize(PathBuf::from(&att.path)).map_err(|e| {
                tracing::warn!(
                    "Outbound attachment path {} could not be canonicalized: {e} — skipping",
                    att.path,
                );
                e
            });
            let canonical = match canonical {
                Ok(p) => p,
                Err(_) => continue,
            };
            let canonical_outbox = match std::fs::canonicalize(&outbox_path) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("Failed to canonicalize outbox dir: {e} — skipping attachment");
                    continue;
                }
            };
            if !canonical.starts_with(&canonical_outbox) {
                tracing::warn!(
                    "Outbound attachment path {} resolves to {} which is outside outbox — skipping",
                    att.path,
                    canonical.display(),
                );
                continue;
            }
            canonical
        };

        // Check file size against limits
        let metadata = tokio::fs::metadata(&host_path).await?;
        let size = metadata.len();
        let limit = match att.kind {
            OutboundKind::Photo => TELEGRAM_PHOTO_UPLOAD_LIMIT,
            _ => TELEGRAM_FILE_UPLOAD_LIMIT,
        };
        if size > limit {
            tracing::warn!(
                "Outbound {} ({:.1} MB) exceeds upload limit — skipping",
                att.path,
                size as f64 / (1024.0 * 1024.0),
            );
            if sandboxed {
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

        // Kinds that support captions
        let caption = att.caption.as_deref();

        let send_result: Result<_, teloxide::RequestError> = match att.kind {
            OutboundKind::Photo => {
                let mut req = bot.send_photo(chat_id, input_file);
                if let Some(cap) = caption {
                    req = req.caption(cap);
                }
                if let Some(tid) = thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map(|_| ())
            }
            OutboundKind::Document => {
                let mut req = bot.send_document(chat_id, input_file);
                if let Some(cap) = caption {
                    req = req.caption(cap);
                }
                if let Some(tid) = thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map(|_| ())
            }
            OutboundKind::Video => {
                let mut req = bot.send_video(chat_id, input_file);
                if let Some(cap) = caption {
                    req = req.caption(cap);
                }
                if let Some(tid) = thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map(|_| ())
            }
            OutboundKind::Audio => {
                let mut req = bot.send_audio(chat_id, input_file);
                if let Some(cap) = caption {
                    req = req.caption(cap);
                }
                if let Some(tid) = thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map(|_| ())
            }
            OutboundKind::Voice => {
                let mut req = bot.send_voice(chat_id, input_file);
                if let Some(cap) = caption {
                    req = req.caption(cap);
                }
                if let Some(tid) = thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map(|_| ())
            }
            OutboundKind::Animation => {
                let mut req = bot.send_animation(chat_id, input_file);
                if let Some(cap) = caption {
                    req = req.caption(cap);
                }
                if let Some(tid) = thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map(|_| ())
            }
            // video_note and sticker don't support captions
            OutboundKind::VideoNote => {
                let mut req = bot.send_video_note(chat_id, input_file);
                if let Some(tid) = thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map(|_| ())
            }
            OutboundKind::Sticker => {
                let mut req = bot.send_sticker(chat_id, input_file);
                if let Some(tid) = thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map(|_| ())
            }
        };

        if let Err(e) = send_result {
            let msg = format!("failed to send {:?} attachment {}: {e:#}", att.kind, att.path);
            tracing::error!("{msg}");
            errors.push(msg);
        }

        // Clean up temp file if sandboxed
        if sandboxed {
            let _ = tokio::fs::remove_file(&host_path).await;
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; ").into())
    }
}

/// Spawn a background task that periodically cleans up old attachment files.
pub fn spawn_cleanup_task(
    agent_dir: std::path::PathBuf,
    ssh_config_path: Option<std::path::PathBuf>,
    resolved_sandbox: Option<String>,
    retention_days: u32,
) {
    tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(CLEANUP_INTERVAL_SECS);
        loop {
            tokio::time::sleep(interval).await;
            if let Err(e) =
                run_cleanup(&agent_dir, ssh_config_path.as_deref(), resolved_sandbox.as_deref(), retention_days)
                    .await
            {
                tracing::warn!("attachment cleanup failed: {e:#}");
            }
        }
    });
}

async fn run_cleanup(
    agent_dir: &std::path::Path,
    ssh_config_path: Option<&std::path::Path>,
    resolved_sandbox: Option<&str>,
    retention_days: u32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ssh_config) = ssh_config_path {
        let ssh_host = rightclaw::openshell::ssh_host_for_sandbox(resolved_sandbox.unwrap());
        let mtime_arg = format!("+{retention_days}");
        // Use find to delete files older than retention_days in sandbox inbox/outbox
        rightclaw::openshell::ssh_exec(
            ssh_config,
            &ssh_host,
            &[
                "find",
                SANDBOX_INBOX,
                SANDBOX_OUTBOX,
                "-type",
                "f",
                "-mtime",
                &mtime_arg,
                "-delete",
            ],
            30,
        )
        .await?;
        // Also clean local tmp dirs (host-side, regardless of sandbox)
        for subdir in &["tmp/inbox", "tmp/outbox"] {
            let dir = agent_dir.join(subdir);
            if dir.exists() {
                cleanup_local_dir(&dir, retention_days).await?;
            }
        }
    } else {
        for subdir in &["inbox", "outbox", "tmp/inbox", "tmp/outbox"] {
            let dir = agent_dir.join(subdir);
            if dir.exists() {
                cleanup_local_dir(&dir, retention_days).await?;
            }
        }
    }
    Ok(())
}

async fn cleanup_local_dir(
    dir: &std::path::Path,
    retention_days: u32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(u64::from(retention_days) * 86400);

    let mut entries = tokio::fs::read_dir(dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let metadata = entry.metadata().await?;
        if !metadata.is_file() {
            continue;
        }
        if let Ok(modified) = metadata.modified()
            && modified < cutoff
        {
            tracing::debug!("cleaning up old attachment: {}", entry.path().display());
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
    Ok(())
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
