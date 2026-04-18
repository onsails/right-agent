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
    #[serde(default)]
    pub media_group_id: Option<String>,
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

/// Category of Telegram media-group album. A `None` from [`GroupKind::of`] means
/// the attachment kind (voice / video_note / sticker / animation) cannot live in
/// any media group and must be sent individually.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupKind {
    /// Photos and videos can mix in the same album.
    PhotoVideo,
    /// Documents form a documents-only album.
    Document,
    /// Audios form an audios-only album.
    Audio,
}

impl GroupKind {
    pub(crate) fn of(kind: &OutboundKind) -> Option<Self> {
        match kind {
            OutboundKind::Photo | OutboundKind::Video => Some(Self::PhotoVideo),
            OutboundKind::Document => Some(Self::Document),
            OutboundKind::Audio => Some(Self::Audio),
            OutboundKind::Voice
            | OutboundKind::VideoNote
            | OutboundKind::Sticker
            | OutboundKind::Animation => None,
        }
    }
}

/// Outcome of classifying a candidate media group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GroupPlan {
    /// 2–10 compatible items: one `sendMediaGroup` call.
    SendAsGroup(GroupKind),
    /// More than 10 compatible same-kind items: split into consecutive
    /// chunks. Each chunk is a list of indices into the input slice. The last
    /// chunk may be size 1 — the caller must fall back to an individual send
    /// for a size-1 chunk because `sendMediaGroup` rejects it.
    Split {
        chunks: Vec<Vec<usize>>,
        kind: GroupKind,
        reason: String,
    },
    /// Incompatible mix, size 0 or 1, or oversize-with-incompatible-mix: the
    /// caller falls back to individual sends for every item in the group.
    Degrade { reason: String },
}

/// Maximum items per Telegram media group.
const MEDIA_GROUP_MAX: usize = 10;

pub(crate) fn classify_media_group(items: &[&OutboundAttachment]) -> GroupPlan {
    if items.len() < 2 {
        return GroupPlan::Degrade {
            reason: format!("group of {}", items.len()),
        };
    }

    // All items must share a single GroupKind; any ungroupable item → degrade.
    let Some(first) = GroupKind::of(&items[0].kind) else {
        return GroupPlan::Degrade {
            reason: format!(
                "incompatible types: {:?} cannot appear in a media group",
                items[0].kind
            ),
        };
    };
    for it in &items[1..] {
        match GroupKind::of(&it.kind) {
            Some(k) if k == first => (),
            _ => {
                let summary: Vec<_> = items.iter().map(|i| i.kind).collect();
                return GroupPlan::Degrade {
                    reason: format!("incompatible types {summary:?} in one media group"),
                };
            }
        }
    }

    if items.len() <= MEDIA_GROUP_MAX {
        return GroupPlan::SendAsGroup(first);
    }

    let chunks: Vec<Vec<usize>> = (0..items.len())
        .collect::<Vec<_>>()
        .chunks(MEDIA_GROUP_MAX)
        .map(<[usize]>::to_vec)
        .collect();
    GroupPlan::Split {
        chunks,
        kind: first,
        reason: format!(
            "group of {} exceeds Telegram limit of {MEDIA_GROUP_MAX}",
            items.len()
        ),
    }
}

/// Fold every non-empty caption into the first slot, separated by blank lines,
/// and blank the rest. Telegram only shows the first item's caption in a media
/// group; without folding, later captions would be silently dropped.
pub(crate) fn merge_group_captions(captions: &mut [Option<String>]) {
    let parts: Vec<String> = captions
        .iter_mut()
        .filter_map(|c| c.take().filter(|s| !s.is_empty()))
        .collect();
    if let Some(slot) = captions.first_mut() {
        *slot = if parts.is_empty() {
            None
        } else {
            let joined = parts.join("\n\n");
            let char_count = joined.chars().count();
            if char_count > TELEGRAM_CAPTION_LIMIT {
                // Truncate to (limit - 1) chars, then append the ellipsis character.
                let truncated: String = joined
                    .char_indices()
                    .take(TELEGRAM_CAPTION_LIMIT - 1)
                    .map(|(_, c)| c)
                    .collect();
                let result = format!("{truncated}…");
                tracing::warn!(
                    original_chars = char_count,
                    limit = TELEGRAM_CAPTION_LIMIT,
                    "media-group caption exceeded Telegram limit; truncated to {} chars",
                    result.chars().count()
                );
                Some(result)
            } else {
                Some(joined)
            }
        };
    }
}

/// One Telegram API call the bot must make to honour a reply's attachments.
/// `Single` reuses the per-type `send_*` path; `Group` becomes one
/// `sendMediaGroup`.
#[derive(Debug)]
pub(crate) enum OutboundSend {
    Single(OutboundAttachment),
    Group {
        kind: GroupKind,
        items: Vec<OutboundAttachment>,
    },
}

/// Partition a reply's attachments into the ordered sends the bot must perform.
/// Returns the list of sends and a list of WARN strings describing any group
/// that had to be degraded or split — the caller logs them.
pub(crate) fn partition_sends(attachments: &[OutboundAttachment]) -> (Vec<OutboundSend>, Vec<String>) {
    use std::collections::BTreeMap;

    // Collect indices per group, preserving first-occurrence order via a
    // secondary Vec (BTreeMap orders by key, which is not what we want).
    let mut group_order: Vec<String> = Vec::new();
    let mut group_indices: BTreeMap<String, Vec<usize>> = BTreeMap::new();

    for (i, a) in attachments.iter().enumerate() {
        if let Some(id) = &a.media_group_id {
            if !group_indices.contains_key(id) {
                group_order.push(id.clone());
            }
            group_indices.entry(id.clone()).or_default().push(i);
        }
    }

    // Build a timeline: every single keeps its original position; every group
    // replaces the position of its first member (anchor). Later group members
    // are "GroupMember" and emitted with the anchor, not at their original
    // positions.
    #[derive(Clone)]
    enum Slot {
        Single,
        GroupAnchor(String),
        GroupMember,
    }
    let mut slots: Vec<Slot> = vec![Slot::Single; attachments.len()];
    for id in &group_order {
        let indices = &group_indices[id];
        for (n, idx) in indices.iter().enumerate() {
            slots[*idx] = if n == 0 {
                Slot::GroupAnchor(id.clone())
            } else {
                Slot::GroupMember
            };
        }
    }

    let mut warnings: Vec<String> = Vec::new();
    let mut sends: Vec<OutboundSend> = Vec::new();

    for (i, slot) in slots.iter().enumerate() {
        match slot {
            Slot::Single => sends.push(OutboundSend::Single(attachments[i].clone())),
            Slot::GroupAnchor(id) => {
                let indices = &group_indices[id];
                let group_items: Vec<&OutboundAttachment> =
                    indices.iter().map(|&idx| &attachments[idx]).collect();
                let plan = classify_media_group(&group_items);
                match plan {
                    GroupPlan::SendAsGroup(kind) => {
                        let mut items: Vec<OutboundAttachment> =
                            indices.iter().map(|&idx| attachments[idx].clone()).collect();
                        let mut caps: Vec<Option<String>> =
                            items.iter().map(|it| it.caption.clone()).collect();
                        merge_group_captions(&mut caps);
                        for (it, c) in items.iter_mut().zip(caps.into_iter()) {
                            it.caption = c;
                        }
                        sends.push(OutboundSend::Group { kind, items });
                    }
                    GroupPlan::Split { chunks, kind, reason } => {
                        warnings.push(format!(
                            "media_group_id={id:?}: {reason} — splitting into ≤10-item chunks"
                        ));
                        for chunk in chunks {
                            if chunk.len() < 2 {
                                // size-1 trailing chunk: emit as Single
                                let src_idx = indices[chunk[0]];
                                sends.push(OutboundSend::Single(attachments[src_idx].clone()));
                            } else {
                                let mut items: Vec<OutboundAttachment> = chunk
                                    .iter()
                                    .map(|&local| attachments[indices[local]].clone())
                                    .collect();
                                let mut caps: Vec<Option<String>> =
                                    items.iter().map(|it| it.caption.clone()).collect();
                                merge_group_captions(&mut caps);
                                for (it, c) in items.iter_mut().zip(caps.into_iter()) {
                                    it.caption = c;
                                }
                                sends.push(OutboundSend::Group { kind, items });
                            }
                        }
                    }
                    GroupPlan::Degrade { reason } => {
                        warnings.push(format!(
                            "media_group_id={id:?}: {reason} — falling back to individual sends"
                        ));
                        for &idx in indices {
                            sends.push(OutboundSend::Single(attachments[idx].clone()));
                        }
                    }
                }
            }
            Slot::GroupMember => { /* emitted with the anchor */ }
        }
    }

    (sends, warnings)
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
/// Maximum characters in a Telegram message/media-group caption.
pub const TELEGRAM_CAPTION_LIMIT: usize = 1024;

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

/// Chat kind + identity for the incoming message. DM emits no attribution block;
/// Group emits a `chat:` block in the prompt.
#[derive(Debug, Clone)]
pub enum ChatContext {
    Private,
    Group {
        id: i64,
        title: Option<String>,
        topic_id: Option<i64>,
    },
}

/// Body of the replied-to message — populated only when the user's message is
/// a Telegram reply AND the reply target is not the bot's own message.
#[derive(Debug, Clone)]
pub struct ReplyToBody {
    pub author: MessageAuthor,
    pub text: Option<String>,
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
    pub chat: ChatContext,
    pub reply_to_body: Option<ReplyToBody>,
}

/// Format input for CC stdin.
///
/// Always formats as YAML with `messages:` root key containing author metadata.
///
/// Returns None if there is nothing to send.
pub fn format_cc_input(msgs: &[InputMessage]) -> Option<String> {
    if msgs.is_empty() {
        return None;
    }

    // Check if all messages have no text and no attachments
    if msgs.iter().all(|m| m.text.is_none() && m.attachments.is_empty()) {
        return None;
    }

    use std::fmt::Write;
    let mut out = String::with_capacity(512);
    out.push_str("messages:\n");
    for m in msgs {
        writeln!(out, "  - id: {}", m.message_id).expect("infallible");
        writeln!(out, "    ts: \"{}\"", m.timestamp.format("%Y-%m-%dT%H:%M:%SZ"))
            .expect("infallible");

        // Author block (always present)
        out.push_str("    author:\n");
        writeln!(out, "      name: \"{}\"", yaml_escape_string(&m.author.name))
            .expect("infallible");
        if let Some(ref username) = m.author.username {
            writeln!(out, "      username: \"{}\"", yaml_escape_string(username))
                .expect("infallible");
        }
        if let Some(user_id) = m.author.user_id {
            writeln!(out, "      user_id: {user_id}").expect("infallible");
        }

        // Chat block (group only; DM stays unchanged).
        if let ChatContext::Group { id, title, topic_id } = &m.chat {
            out.push_str("    chat:\n");
            writeln!(out, "      kind: group").expect("infallible");
            writeln!(out, "      id: {id}").expect("infallible");
            if let Some(t) = title {
                writeln!(out, "      title: \"{}\"", yaml_escape_string(t))
                    .expect("infallible");
            }
            if let Some(tid) = topic_id {
                writeln!(out, "      topic_id: {tid}").expect("infallible");
            }
        }

        // Forward info (only if forwarded)
        if let Some(ref fwd) = m.forward_info {
            out.push_str("    forward_from:\n");
            writeln!(out, "      name: \"{}\"", yaml_escape_string(&fwd.from.name))
                .expect("infallible");
            if let Some(ref username) = fwd.from.username {
                writeln!(out, "      username: \"{}\"", yaml_escape_string(username))
                    .expect("infallible");
            }
            if let Some(user_id) = fwd.from.user_id {
                writeln!(out, "      user_id: {user_id}").expect("infallible");
            }
            writeln!(out, "    forward_date: \"{}\"", fwd.date.format("%Y-%m-%dT%H:%M:%SZ"))
                .expect("infallible");
        }

        // Reply-to (only if reply)
        if let Some(reply_id) = m.reply_to_id {
            writeln!(out, "    reply_to_id: {reply_id}").expect("infallible");
        }

        // Reply-to body: present only when the user replied to a non-bot message.
        if let Some(ref r) = m.reply_to_body {
            out.push_str("    reply_to:\n");
            out.push_str("      author:\n");
            writeln!(out, "        name: \"{}\"", yaml_escape_string(&r.author.name))
                .expect("infallible");
            if let Some(ref un) = r.author.username {
                writeln!(out, "        username: \"{}\"", yaml_escape_string(un))
                    .expect("infallible");
            }
            if let Some(uid) = r.author.user_id {
                writeln!(out, "        user_id: {uid}").expect("infallible");
            }
            if let Some(ref t) = r.text {
                writeln!(out, "      text: \"{}\"", yaml_escape_string(t))
                    .expect("infallible");
            }
        }

        // Text
        if let Some(ref text) = m.text {
            let escaped = yaml_escape_string(text);
            writeln!(out, "    text: \"{escaped}\"").expect("infallible");
        }

        // Attachments
        if !m.attachments.is_empty() {
            out.push_str("    attachments:\n");
            for att in &m.attachments {
                writeln!(out, "      - type: {}", att.kind.as_str()).expect("infallible");
                writeln!(out, "        path: {}", att.path.display()).expect("infallible");
                writeln!(out, "        mime_type: {}", att.mime_type).expect("infallible");
                if let Some(ref fname) = att.filename {
                    let escaped = yaml_escape_string(fname);
                    writeln!(out, "        filename: \"{escaped}\"").expect("infallible");
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
    let sandboxed = ssh_config_path.is_some();
    let outbox_prefix = if sandboxed {
        SANDBOX_OUTBOX.to_owned()
    } else {
        agent_dir.join("outbox").to_string_lossy().into_owned()
    };

    // Pre-create tmp/outbox for sandboxed downloads (avoids repeated create_dir_all in loop)
    if sandboxed {
        tokio::fs::create_dir_all(agent_dir.join("tmp/outbox")).await?;
    }

    // Canonicalize the outbox dir once per call (non-sandboxed only). Eliminates
    // N repeated blocking syscalls in groups of ≤10.
    let outbox_canonical = if sandboxed {
        None
    } else {
        match std::fs::canonicalize(agent_dir.join("outbox")) {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!(
                    "Failed to canonicalize outbox dir {}: {e} — all outbound attachments will be skipped",
                    agent_dir.join("outbox").display(),
                );
                None
            }
        }
    };

    let ctx = SendCtx {
        bot,
        chat_id,
        eff_thread_id,
        agent_dir,
        resolved_sandbox,
        sandboxed,
        outbox_prefix: &outbox_prefix,
        outbox_canonical,
    };

    let (sends, warnings) = partition_sends(attachments);
    for w in &warnings {
        tracing::warn!("{w}");
    }

    let mut errors: Vec<String> = Vec::new();
    for send in &sends {
        let result: Result<(), teloxide::RequestError> = match send {
            OutboundSend::Single(att) => send_single(att, &ctx).await,
            OutboundSend::Group { kind: _, items } => send_group(items, &ctx).await,
        };
        if let Err(e) = result {
            let label = match send {
                OutboundSend::Single(att) => format!("{:?} attachment {}", att.kind, att.path),
                OutboundSend::Group { kind, items } => {
                    format!("{kind:?} media group of {} items", items.len())
                }
            };
            let msg = format!(
                "failed to send {label}: {}",
                rightclaw::error::display_error_chain(&e),
            );
            tracing::error!("{msg}");
            errors.push(msg);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; ").into())
    }
}

/// Shared context passed to per-send helpers. Built once per `send_attachments`
/// call. `outbox_canonical` is pre-computed in non-sandboxed mode so we don't
/// re-canonicalize the outbox dir per attachment.
struct SendCtx<'a> {
    bot: &'a super::BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    agent_dir: &'a std::path::Path,
    resolved_sandbox: Option<&'a str>,
    sandboxed: bool,
    outbox_prefix: &'a str,
    outbox_canonical: Option<PathBuf>,
}

/// Validate an outbound attachment, fetch its host path, and enforce the size
/// limit. Returns `Some(path)` on success; logs WARN and returns `None` on any
/// failure. On sandboxed paths a size/metadata failure deletes the just-downloaded
/// temp file so callers never see orphans.
///
/// `log_suffix` is appended to each WARN message so the caller's path shape
/// ("skipping" vs "skipping media group") is visible without duplicating the
/// whole message.
async fn resolve_host_path(
    att: &OutboundAttachment,
    ctx: &SendCtx<'_>,
    log_suffix: &str,
) -> Option<PathBuf> {
    if !att.path.starts_with(ctx.outbox_prefix) {
        tracing::warn!(
            "Outbound attachment path {} is outside outbox prefix {} — {log_suffix}",
            att.path,
            ctx.outbox_prefix,
        );
        return None;
    }

    let host: PathBuf = if ctx.sandboxed {
        let tmp_dir = ctx.agent_dir.join("tmp/outbox");
        let file_name = std::path::Path::new(&att.path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let dest = tmp_dir.join(&file_name);
        let sandbox = ctx.resolved_sandbox.unwrap();
        if let Err(e) = rightclaw::openshell::download_file(sandbox, &att.path, &dest).await {
            tracing::warn!(
                "download_file failed for {}: {:#} — {log_suffix}",
                att.path,
                e,
            );
            return None;
        }
        dest
    } else {
        let Some(outbox_c) = ctx.outbox_canonical.as_deref() else {
            tracing::warn!(
                "outbox dir not canonicalizable — {log_suffix} (path {})",
                att.path,
            );
            return None;
        };
        let canonical = match tokio::fs::canonicalize(&att.path).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    "Outbound attachment path {} could not be canonicalized: {e} — {log_suffix}",
                    att.path,
                );
                return None;
            }
        };
        if !canonical.starts_with(outbox_c) {
            tracing::warn!(
                "Outbound attachment path {} resolves to {} which is outside outbox — {log_suffix}",
                att.path,
                canonical.display(),
            );
            return None;
        }
        canonical
    };

    let meta = match tokio::fs::metadata(&host).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("metadata failed for {}: {e} — {log_suffix}", host.display());
            if ctx.sandboxed {
                if let Err(e) = tokio::fs::remove_file(&host).await
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!("failed to remove temp file {}: {e}", host.display());
                }
            }
            return None;
        }
    };
    let limit = match att.kind {
        OutboundKind::Photo => TELEGRAM_PHOTO_UPLOAD_LIMIT,
        _ => TELEGRAM_FILE_UPLOAD_LIMIT,
    };
    if meta.len() > limit {
        tracing::warn!(
            "Outbound {} ({:.1} MB) exceeds upload limit — {log_suffix}",
            att.path,
            meta.len() as f64 / (1024.0 * 1024.0),
        );
        if ctx.sandboxed {
            if let Err(e) = tokio::fs::remove_file(&host).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("failed to remove temp file {}: {e}", host.display());
            }
        }
        return None;
    }

    Some(host)
}

async fn send_single(
    att: &OutboundAttachment,
    ctx: &SendCtx<'_>,
) -> Result<(), teloxide::RequestError> {
    use teloxide::payloads::{
        SendAnimationSetters, SendAudioSetters, SendDocumentSetters, SendPhotoSetters,
        SendStickerSetters, SendVideoNoteSetters, SendVideoSetters, SendVoiceSetters,
    };
    use teloxide::requests::Requester;
    use teloxide::types::{InputFile, MessageId, ThreadId};

    let Some(host_path) = resolve_host_path(att, ctx, "skipping").await else {
        return Ok(());
    };

    let input_file = InputFile::file(&host_path);
    let thread_id = if ctx.eff_thread_id != 0 {
        Some(ThreadId(MessageId(ctx.eff_thread_id as i32)))
    } else {
        None
    };

    let caption = att.caption.as_deref();

    let send_result: Result<_, teloxide::RequestError> = match att.kind {
        OutboundKind::Photo => {
            let mut req = ctx.bot.send_photo(ctx.chat_id, input_file);
            if let Some(cap) = caption {
                req = req.caption(cap);
            }
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }
        OutboundKind::Document => {
            let mut req = ctx.bot.send_document(ctx.chat_id, input_file);
            if let Some(cap) = caption {
                req = req.caption(cap);
            }
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }
        OutboundKind::Video => {
            let mut req = ctx.bot.send_video(ctx.chat_id, input_file);
            if let Some(cap) = caption {
                req = req.caption(cap);
            }
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }
        OutboundKind::Audio => {
            let mut req = ctx.bot.send_audio(ctx.chat_id, input_file);
            if let Some(cap) = caption {
                req = req.caption(cap);
            }
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }
        OutboundKind::Voice => {
            let mut req = ctx.bot.send_voice(ctx.chat_id, input_file);
            if let Some(cap) = caption {
                req = req.caption(cap);
            }
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }
        OutboundKind::Animation => {
            let mut req = ctx.bot.send_animation(ctx.chat_id, input_file);
            if let Some(cap) = caption {
                req = req.caption(cap);
            }
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }
        OutboundKind::VideoNote => {
            let mut req = ctx.bot.send_video_note(ctx.chat_id, input_file);
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }
        OutboundKind::Sticker => {
            let mut req = ctx.bot.send_sticker(ctx.chat_id, input_file);
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            req.await.map(|_| ())
        }
    };

    if ctx.sandboxed {
        if let Err(e) = tokio::fs::remove_file(&host_path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("failed to remove temp file {}: {e}", host_path.display());
        }
    }
    send_result
}

#[allow(clippy::too_many_arguments)]
async fn send_group(
    items: &[OutboundAttachment],
    ctx: &SendCtx<'_>,
) -> Result<(), teloxide::RequestError> {
    use teloxide::payloads::SendMediaGroupSetters;
    use teloxide::requests::Requester;
    use teloxide::types::{
        InputFile, InputMedia, InputMediaAudio, InputMediaDocument, InputMediaPhoto,
        InputMediaVideo, MessageId, ThreadId,
    };

    // All-or-nothing: Telegram's sendMediaGroup requires the full set in one
    // call. If any member fails path validation, download, metadata read, or
    // the size check, the whole group is aborted — already-downloaded temp
    // files are deleted, and the user sees no message for this group. A WARN
    // record goes to the bot log identifying the offending path. We prefer
    // this over partial sends because a partial album would be surprising
    // (user asked for 5 photos, got a silent-cropped album of 3).
    let mut host_paths: Vec<PathBuf> = Vec::with_capacity(items.len());
    for att in items {
        match resolve_host_path(att, ctx, "skipping media group").await {
            Some(p) => host_paths.push(p),
            None => {
                cleanup_host_paths(&host_paths, ctx.sandboxed).await;
                return Ok(());
            }
        }
    }

    let media: Vec<InputMedia> = items
        .iter()
        .zip(host_paths.iter())
        .map(|(att, host)| {
            let file = InputFile::file(host);
            let cap = att.caption.clone();
            match att.kind {
                OutboundKind::Photo => {
                    let mut m = InputMediaPhoto::new(file);
                    if let Some(c) = cap {
                        m = m.caption(c);
                    }
                    InputMedia::Photo(m)
                }
                OutboundKind::Video => {
                    let mut m = InputMediaVideo::new(file);
                    if let Some(c) = cap {
                        m = m.caption(c);
                    }
                    InputMedia::Video(m)
                }
                OutboundKind::Document => {
                    let mut m = InputMediaDocument::new(file);
                    if let Some(c) = cap {
                        m = m.caption(c);
                    }
                    InputMedia::Document(m)
                }
                OutboundKind::Audio => {
                    let mut m = InputMediaAudio::new(file);
                    if let Some(c) = cap {
                        m = m.caption(c);
                    }
                    InputMedia::Audio(m)
                }
                // Classifier rejects these kinds from groups, so this branch is
                // unreachable in practice. Log a loud error and fall back to a
                // Document to keep the bot alive if the classifier is ever
                // changed. This MUST NOT silently swallow data.
                _ => {
                    tracing::error!(
                        "send_group received ungroupable kind {:?} for {} — classifier bug",
                        att.kind,
                        att.path,
                    );
                    InputMedia::Document(InputMediaDocument::new(file))
                }
            }
        })
        .collect();

    let thread_id = if ctx.eff_thread_id != 0 {
        Some(ThreadId(MessageId(ctx.eff_thread_id as i32)))
    } else {
        None
    };

    let mut req = ctx.bot.send_media_group(ctx.chat_id, media);
    if let Some(tid) = thread_id {
        req = req.message_thread_id(tid);
    }
    let result = req.await.map(|_| ());

    cleanup_host_paths(&host_paths, ctx.sandboxed).await;
    result
}

async fn cleanup_host_paths(paths: &[std::path::PathBuf], sandboxed: bool) {
    if !sandboxed {
        return;
    }
    for p in paths {
        if let Err(e) = tokio::fs::remove_file(p).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("failed to remove temp file {}: {e}", p.display());
        }
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

    fn test_author() -> MessageAuthor {
        MessageAuthor {
            name: "Test User".into(),
            username: None,
            user_id: Some(1),
        }
    }

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
    fn outbound_attachment_deserialize_without_media_group_id_defaults_none() {
        let json = r#"{"type":"photo","path":"/sandbox/outbox/a.jpg"}"#;
        let att: OutboundAttachment = serde_json::from_str(json).unwrap();
        assert!(att.media_group_id.is_none());
    }

    #[test]
    fn outbound_attachment_deserialize_with_media_group_id() {
        let json = r#"{"type":"photo","path":"/sandbox/outbox/a.jpg","media_group_id":"shots"}"#;
        let att: OutboundAttachment = serde_json::from_str(json).unwrap();
        assert_eq!(att.media_group_id.as_deref(), Some("shots"));
    }

    #[test]
    fn format_cc_input_single_text_returns_yaml() {
        let msgs = vec![InputMessage {
            message_id: 1,
            text: Some("hello world".into()),
            timestamp: DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            attachments: vec![],
            author: test_author(),
            forward_info: None,
            reply_to_id: None,
            chat: ChatContext::Private,
            reply_to_body: None,
        }];
        let result = format_cc_input(&msgs).unwrap();
        assert!(result.starts_with("messages:\n"));
        assert!(result.contains("    text: \"hello world\"\n"));
        assert!(result.contains("    author:\n"));
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
            author: test_author(),
            forward_info: None,
            reply_to_id: None,
            chat: ChatContext::Private,
            reply_to_body: None,
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
                author: test_author(),
                forward_info: None,
                reply_to_id: None,
                chat: ChatContext::Private,
                reply_to_body: None,
            },
            InputMessage {
                message_id: 2,
                text: Some("second".into()),
                timestamp: ts,
                attachments: vec![],
                author: test_author(),
                forward_info: None,
                reply_to_id: None,
                chat: ChatContext::Private,
                reply_to_body: None,
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
            author: test_author(),
            forward_info: None,
            reply_to_id: None,
            chat: ChatContext::Private,
            reply_to_body: None,
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
            author: test_author(),
            forward_info: None,
            reply_to_id: None,
            chat: ChatContext::Private,
            reply_to_body: None,
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
                author: test_author(),
                forward_info: None,
                reply_to_id: None,
                chat: ChatContext::Private,
                reply_to_body: None,
            },
            InputMessage {
                message_id: 2,
                text: Some("line1\nline2\ttab \"quoted\"".into()),
                timestamp: Utc::now(),
                attachments: vec![],
                author: test_author(),
                forward_info: None,
                reply_to_id: None,
                chat: ChatContext::Private,
                reply_to_body: None,
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

    #[test]
    fn group_kind_from_outbound_kind_covers_all_variants() {
        use OutboundKind::*;
        assert_eq!(GroupKind::of(&Photo), Some(GroupKind::PhotoVideo));
        assert_eq!(GroupKind::of(&Video), Some(GroupKind::PhotoVideo));
        assert_eq!(GroupKind::of(&Document), Some(GroupKind::Document));
        assert_eq!(GroupKind::of(&Audio), Some(GroupKind::Audio));
        assert_eq!(GroupKind::of(&Voice), None);
        assert_eq!(GroupKind::of(&VideoNote), None);
        assert_eq!(GroupKind::of(&Sticker), None);
        assert_eq!(GroupKind::of(&Animation), None);
    }

    fn att(kind: OutboundKind) -> OutboundAttachment {
        OutboundAttachment {
            kind,
            path: format!("/sandbox/outbox/{}.bin", kind_to_ext(kind)),
            filename: None,
            caption: None,
            media_group_id: Some("g".into()),
        }
    }

    fn kind_to_ext(k: OutboundKind) -> &'static str {
        match k {
            OutboundKind::Photo => "jpg",
            OutboundKind::Video => "mp4",
            OutboundKind::Document => "pdf",
            OutboundKind::Audio => "mp3",
            OutboundKind::Voice => "ogg",
            OutboundKind::VideoNote => "mp4",
            OutboundKind::Sticker => "webp",
            OutboundKind::Animation => "gif",
        }
    }

    fn atts(kinds: &[OutboundKind]) -> Vec<OutboundAttachment> {
        kinds.iter().copied().map(att).collect()
    }

    fn refs(v: &[OutboundAttachment]) -> Vec<&OutboundAttachment> {
        v.iter().collect()
    }

    #[test]
    fn classify_two_photos_sends_as_group() {
        let items = atts(&[OutboundKind::Photo, OutboundKind::Photo]);
        assert_eq!(
            classify_media_group(&refs(&items)),
            GroupPlan::SendAsGroup(GroupKind::PhotoVideo),
        );
    }

    #[test]
    fn classify_photo_and_video_mix_sends_as_group() {
        let items = atts(&[OutboundKind::Photo, OutboundKind::Video]);
        assert_eq!(
            classify_media_group(&refs(&items)),
            GroupPlan::SendAsGroup(GroupKind::PhotoVideo),
        );
    }

    #[test]
    fn classify_two_documents_sends_as_group() {
        let items = atts(&[OutboundKind::Document, OutboundKind::Document]);
        assert_eq!(
            classify_media_group(&refs(&items)),
            GroupPlan::SendAsGroup(GroupKind::Document),
        );
    }

    #[test]
    fn classify_two_audios_sends_as_group() {
        let items = atts(&[OutboundKind::Audio, OutboundKind::Audio]);
        assert_eq!(
            classify_media_group(&refs(&items)),
            GroupPlan::SendAsGroup(GroupKind::Audio),
        );
    }

    #[test]
    fn classify_photo_and_voice_degrades() {
        let items = atts(&[OutboundKind::Photo, OutboundKind::Voice]);
        match classify_media_group(&refs(&items)) {
            GroupPlan::Degrade { reason } => assert!(reason.contains("incompatible")),
            other => panic!("expected Degrade, got {other:?}"),
        }
    }

    #[test]
    fn classify_photo_and_document_degrades() {
        let items = atts(&[OutboundKind::Photo, OutboundKind::Document]);
        match classify_media_group(&refs(&items)) {
            GroupPlan::Degrade { reason } => assert!(reason.contains("incompatible")),
            other => panic!("expected Degrade, got {other:?}"),
        }
    }

    #[test]
    fn classify_single_item_degrades() {
        let items = atts(&[OutboundKind::Photo]);
        match classify_media_group(&refs(&items)) {
            GroupPlan::Degrade { reason } => assert!(reason.contains("group of 1")),
            other => panic!("expected Degrade, got {other:?}"),
        }
    }

    #[test]
    fn classify_empty_group_degrades() {
        let items: Vec<OutboundAttachment> = vec![];
        match classify_media_group(&refs(&items)) {
            GroupPlan::Degrade { .. } => (),
            other => panic!("expected Degrade, got {other:?}"),
        }
    }

    #[test]
    fn classify_eleven_photos_splits_into_chunks() {
        let items = atts(&[OutboundKind::Photo; 11]);
        match classify_media_group(&refs(&items)) {
            GroupPlan::Split { chunks, kind, .. } => {
                assert_eq!(kind, GroupKind::PhotoVideo);
                assert_eq!(chunks.len(), 2, "expected 2 chunks (10 + 1)");
                assert_eq!(chunks[0], (0..10).collect::<Vec<_>>());
                assert_eq!(chunks[1], vec![10]);
            }
            other => panic!("expected Split, got {other:?}"),
        }
    }

    #[test]
    fn classify_twenty_five_photos_splits_into_three_chunks() {
        let items = atts(&[OutboundKind::Photo; 25]);
        match classify_media_group(&refs(&items)) {
            GroupPlan::Split { chunks, .. } => {
                assert_eq!(chunks.len(), 3);
                assert_eq!(chunks[0].len(), 10);
                assert_eq!(chunks[1].len(), 10);
                assert_eq!(chunks[2].len(), 5);
            }
            other => panic!("expected Split, got {other:?}"),
        }
    }

    #[test]
    fn classify_exactly_ten_photos_sends_as_group() {
        // Boundary: `len <= MEDIA_GROUP_MAX` — off-by-one guard.
        let items = atts(&[OutboundKind::Photo; 10]);
        assert_eq!(
            classify_media_group(&refs(&items)),
            GroupPlan::SendAsGroup(GroupKind::PhotoVideo),
        );
    }

    #[test]
    fn partition_empty_input_is_safe() {
        let (sends, warnings) = partition_sends(&[]);
        assert!(sends.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn classify_eleven_mixed_with_voice_degrades() {
        let mut kinds = vec![OutboundKind::Photo; 10];
        kinds.push(OutboundKind::Voice);
        let items = atts(&kinds);
        match classify_media_group(&refs(&items)) {
            GroupPlan::Degrade { reason } => assert!(reason.contains("incompatible")),
            other => panic!("expected Degrade, got {other:?}"),
        }
    }

    #[test]
    fn merge_captions_first_only_is_preserved() {
        let mut caps = vec![Some("first".to_owned()), None, None];
        merge_group_captions(&mut caps);
        assert_eq!(caps, vec![Some("first".to_owned()), None, None]);
    }

    #[test]
    fn merge_captions_all_none_stays_none() {
        let mut caps: Vec<Option<String>> = vec![None, None, None];
        merge_group_captions(&mut caps);
        assert_eq!(caps, vec![None, None, None]);
    }

    #[test]
    fn merge_captions_later_items_fold_into_first() {
        let mut caps = vec![Some("a".to_owned()), None, Some("b".to_owned())];
        merge_group_captions(&mut caps);
        assert_eq!(caps, vec![Some("a\n\nb".to_owned()), None, None]);
    }

    #[test]
    fn merge_captions_only_later_item_moves_to_first() {
        let mut caps = vec![None, Some("only".to_owned())];
        merge_group_captions(&mut caps);
        assert_eq!(caps, vec![Some("only".to_owned()), None]);
    }

    #[test]
    fn merge_captions_all_three_set_joined() {
        let mut caps = vec![
            Some("a".to_owned()),
            Some("b".to_owned()),
            Some("c".to_owned()),
        ];
        merge_group_captions(&mut caps);
        assert_eq!(caps, vec![Some("a\n\nb\n\nc".to_owned()), None, None]);
    }

    #[test]
    fn merge_captions_truncates_when_over_telegram_limit() {
        // 10 captions × 150 chars each → 1500 chars of content + 9 "\n\n" = 1518
        // Well over the 1024 limit; should be truncated with ellipsis.
        let mut caps: Vec<Option<String>> = (0..10)
            .map(|i| Some("x".repeat(150) + &format!("#{i}")))
            .collect();
        merge_group_captions(&mut caps);
        let first = caps[0].as_deref().expect("first slot must be set");
        assert!(
            first.chars().count() <= TELEGRAM_CAPTION_LIMIT,
            "caption too long: {} chars",
            first.chars().count()
        );
        assert!(first.ends_with('…'), "truncated caption must end with …");
        for tail in &caps[1..] {
            assert!(tail.is_none());
        }
    }

    #[test]
    fn merge_captions_under_limit_is_not_truncated() {
        let mut caps = vec![Some("short".to_owned()), Some("also short".to_owned())];
        merge_group_captions(&mut caps);
        assert_eq!(caps[0].as_deref(), Some("short\n\nalso short"));
        assert!(!caps[0].as_deref().unwrap().ends_with('…'));
    }

    #[test]
    fn merge_captions_truncation_is_char_safe() {
        // Russian / emoji caption, each copy is 500+ chars via chars().count() —
        // byte length is 2-4x that. Byte-slice truncation would panic mid-codepoint.
        let cyrillic: String = "а".repeat(600);
        let mut caps = vec![Some(cyrillic.clone()), Some(cyrillic.clone()), Some(cyrillic)];
        merge_group_captions(&mut caps);
        let first = caps[0].as_deref().unwrap();
        assert!(first.chars().count() <= TELEGRAM_CAPTION_LIMIT);
        // No panic = char-boundary-safe truncation.
    }

    #[test]
    fn format_cc_input_includes_author() {
        let ts = DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let msgs = vec![InputMessage {
            message_id: 1,
            text: Some("hello".into()),
            timestamp: ts,
            attachments: vec![],
            author: MessageAuthor {
                name: "\u{0410}\u{043d}\u{0434}\u{0440}\u{0435}\u{0439} \u{041a}\u{0443}\u{0437}\u{043d}\u{0435}\u{0446}\u{043e}\u{0432}".into(),
                username: Some("@right".into()),
                user_id: Some(12345678),
            },
            forward_info: None,
            reply_to_id: None,
            chat: ChatContext::Private,
            reply_to_body: None,
        }];
        let result = format_cc_input(&msgs).unwrap();
        assert!(result.starts_with("messages:\n"), "should be YAML, not raw text");
        assert!(result.contains("    author:\n"));
        assert!(result.contains("      name: \"\u{0410}\u{043d}\u{0434}\u{0440}\u{0435}\u{0439} \u{041a}\u{0443}\u{0437}\u{043d}\u{0435}\u{0446}\u{043e}\u{0432}\"\n"));
        assert!(result.contains("      username: \"@right\"\n"));
        assert!(result.contains("      user_id: 12345678\n"));
    }

    #[test]
    fn format_cc_input_includes_forward_info() {
        let ts = DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let fwd_date = DateTime::parse_from_rfc3339("2026-04-07T20:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let msgs = vec![InputMessage {
            message_id: 1,
            text: Some("forwarded text".into()),
            timestamp: ts,
            attachments: vec![],
            author: MessageAuthor {
                name: "\u{0410}\u{043d}\u{0434}\u{0440}\u{0435}\u{0439} \u{041a}\u{0443}\u{0437}\u{043d}\u{0435}\u{0446}\u{043e}\u{0432}".into(),
                username: Some("@right".into()),
                user_id: Some(12345678),
            },
            forward_info: Some(ForwardInfo {
                from: MessageAuthor {
                    name: "\u{041c}\u{0438}\u{0448}\u{0430} \u{041f}\u{0435}\u{0442}\u{0440}\u{043e}\u{0432}".into(),
                    username: Some("@mishapetrov".into()),
                    user_id: Some(12345678),
                },
                date: fwd_date,
            }),
            reply_to_id: None,
            chat: ChatContext::Private,
            reply_to_body: None,
        }];
        let result = format_cc_input(&msgs).unwrap();
        assert!(result.contains("    forward_from:\n"));
        assert!(result.contains("      name: \"\u{041c}\u{0438}\u{0448}\u{0430} \u{041f}\u{0435}\u{0442}\u{0440}\u{043e}\u{0432}\"\n"));
        assert!(result.contains("      username: \"@mishapetrov\"\n"));
        assert!(result.contains("      user_id: 12345678\n"));
        assert!(result.contains("    forward_date: \"2026-04-07T20:00:00Z\"\n"));
    }

    #[test]
    fn format_cc_input_includes_reply_to_id() {
        let ts = Utc::now();
        let msgs = vec![InputMessage {
            message_id: 5,
            text: Some("replying".into()),
            timestamp: ts,
            attachments: vec![],
            author: MessageAuthor {
                name: "\u{0410}\u{043d}\u{0434}\u{0440}\u{0435}\u{0439}".into(),
                username: None,
                user_id: Some(12345678),
            },
            forward_info: None,
            reply_to_id: Some(3),
            chat: ChatContext::Private,
            reply_to_body: None,
        }];
        let result = format_cc_input(&msgs).unwrap();
        assert!(result.contains("    reply_to_id: 3\n"));
    }

    fn att_with(kind: OutboundKind, group: Option<&str>, caption: Option<&str>) -> OutboundAttachment {
        OutboundAttachment {
            kind,
            path: format!("/sandbox/outbox/{}-{}.bin", kind_to_ext(kind), caption.unwrap_or("x")),
            filename: None,
            caption: caption.map(str::to_owned),
            media_group_id: group.map(str::to_owned),
        }
    }

    #[test]
    fn partition_no_group_ids_produces_all_singles() {
        let atts = vec![
            att_with(OutboundKind::Photo, None, None),
            att_with(OutboundKind::Document, None, None),
        ];
        let (sends, warnings) = partition_sends(&atts);
        assert_eq!(sends.len(), 2);
        assert!(warnings.is_empty());
        assert!(matches!(sends[0], OutboundSend::Single(_)));
        assert!(matches!(sends[1], OutboundSend::Single(_)));
    }

    #[test]
    fn partition_two_photo_group_produces_one_group_send() {
        let atts = vec![
            att_with(OutboundKind::Photo, Some("shots"), Some("a")),
            att_with(OutboundKind::Photo, Some("shots"), Some("b")),
        ];
        let (sends, warnings) = partition_sends(&atts);
        assert!(warnings.is_empty());
        assert_eq!(sends.len(), 1);
        match &sends[0] {
            OutboundSend::Group { kind, items } => {
                assert_eq!(*kind, GroupKind::PhotoVideo);
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].caption.as_deref(), Some("a\n\nb"));
                assert!(items[1].caption.is_none());
            }
            other => panic!("expected OutboundSend::Group, got {other:?}"),
        }
    }

    #[test]
    fn partition_group_preserves_first_occurrence_order() {
        // Reply order: group A item, single, group A item, group B item, group B item
        let atts = vec![
            att_with(OutboundKind::Photo, Some("a"), None),
            att_with(OutboundKind::Document, None, None),
            att_with(OutboundKind::Photo, Some("a"), None),
            att_with(OutboundKind::Document, Some("b"), None),
            att_with(OutboundKind::Document, Some("b"), None),
        ];
        let (sends, warnings) = partition_sends(&atts);
        assert!(warnings.is_empty());
        // Expected send order: group "a" (where it first appeared), then single,
        // then group "b".
        assert_eq!(sends.len(), 3);
        assert!(matches!(sends[0], OutboundSend::Group { kind: GroupKind::PhotoVideo, .. }));
        assert!(matches!(sends[1], OutboundSend::Single(_)));
        assert!(matches!(sends[2], OutboundSend::Group { kind: GroupKind::Document, .. }));
    }

    #[test]
    fn partition_incompatible_group_degrades_and_warns() {
        let atts = vec![
            att_with(OutboundKind::Photo, Some("bad"), None),
            att_with(OutboundKind::Voice, Some("bad"), None),
        ];
        let (sends, warnings) = partition_sends(&atts);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("bad"), "warning must mention group id, got: {}", warnings[0]);
        assert_eq!(sends.len(), 2, "both items fall back to Single");
        assert!(sends.iter().all(|s| matches!(s, OutboundSend::Single(_))));
    }

    #[test]
    fn partition_lone_group_member_degrades_and_warns() {
        let atts = vec![
            att_with(OutboundKind::Photo, Some("only"), None),
            att_with(OutboundKind::Document, None, None),
        ];
        let (sends, warnings) = partition_sends(&atts);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("only"));
        assert_eq!(sends.len(), 2);
        assert!(sends.iter().all(|s| matches!(s, OutboundSend::Single(_))));
    }

    #[test]
    fn partition_split_oversize_group_yields_multiple_group_sends_plus_trailing_single() {
        let atts: Vec<OutboundAttachment> = (0..11)
            .map(|_| att_with(OutboundKind::Photo, Some("big"), None))
            .collect();
        let (sends, warnings) = partition_sends(&atts);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("big"));
        // 11 → one group of 10 + one trailing single.
        assert_eq!(sends.len(), 2);
        match &sends[0] {
            OutboundSend::Group { items, .. } => assert_eq!(items.len(), 10),
            other => panic!("expected Group first, got {other:?}"),
        }
        assert!(matches!(sends[1], OutboundSend::Single(_)));
    }

    #[test]
    fn partition_split_oversize_group_merges_captions_per_chunk() {
        // 11 photos in a group — split into a chunk of 10 + 1 trailing single.
        // Give every photo a distinct caption and assert the first item of the
        // 10-chunk carries all 10 captions joined with "\n\n" and the trailing
        // single retains only its own caption.
        let captions: Vec<String> = (0..11).map(|i| format!("c{i}")).collect();
        let atts: Vec<OutboundAttachment> = captions
            .iter()
            .map(|c| att_with(OutboundKind::Photo, Some("big"), Some(c)))
            .collect();
        let (sends, warnings) = partition_sends(&atts);
        assert_eq!(warnings.len(), 1);
        assert_eq!(sends.len(), 2);

        // First send: 10-item Group, first item's caption = "c0\n\nc1\n\n...\n\nc9".
        match &sends[0] {
            OutboundSend::Group { items, .. } => {
                assert_eq!(items.len(), 10);
                let expected: String = (0..10)
                    .map(|i| format!("c{i}"))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                assert_eq!(items[0].caption.as_deref(), Some(expected.as_str()));
                for it in &items[1..] {
                    assert!(it.caption.is_none(), "non-first items must be blanked");
                }
            }
            other => panic!("expected Group, got {other:?}"),
        }

        // Second send: trailing Single for the 11th photo — caption "c10".
        match &sends[1] {
            OutboundSend::Single(att) => {
                assert_eq!(att.caption.as_deref(), Some("c10"));
            }
            other => panic!("expected Single, got {other:?}"),
        }
    }

    #[test]
    fn format_cc_input_hidden_user_forward_omits_missing_fields() {
        let ts = Utc::now();
        let fwd_date = Utc::now();
        let msgs = vec![InputMessage {
            message_id: 1,
            text: Some("secret".into()),
            timestamp: ts,
            attachments: vec![],
            author: MessageAuthor {
                name: "\u{0410}\u{043d}\u{0434}\u{0440}\u{0435}\u{0439}".into(),
                username: None,
                user_id: Some(12345678),
            },
            forward_info: Some(ForwardInfo {
                from: MessageAuthor {
                    name: "Hidden Person".into(),
                    username: None,
                    user_id: None,
                },
                date: fwd_date,
            }),
            reply_to_id: None,
            chat: ChatContext::Private,
            reply_to_body: None,
        }];
        let result = format_cc_input(&msgs).unwrap();
        assert!(result.contains("      name: \"Hidden Person\"\n"));
        // username and user_id lines should NOT be present under forward_from
        let fwd_idx = result.find("    forward_from:\n").unwrap();
        let after_fwd = &result[fwd_idx..];
        let fwd_block_end = after_fwd.find("    forward_date:").unwrap();
        let fwd_block = &after_fwd[..fwd_block_end];
        assert!(!fwd_block.contains("username:"));
        assert!(!fwd_block.contains("user_id:"));
    }
}

#[cfg(test)]
mod group_format_tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 4, 16, 12, 0, 0).unwrap()
    }

    #[test]
    fn dm_single_message_emits_yaml_with_no_chat_block() {
        let m = InputMessage {
            message_id: 1,
            text: Some("hi".into()),
            timestamp: now(),
            attachments: vec![],
            author: MessageAuthor {
                name: "Alice".into(),
                username: Some("@alice".into()),
                user_id: Some(42),
            },
            forward_info: None,
            reply_to_id: None,
            chat: ChatContext::Private,
            reply_to_body: None,
        };
        let yaml = format_cc_input(&[m]).unwrap();
        assert!(yaml.contains("messages:"));
        assert!(!yaml.contains("chat:"), "DM must not emit chat block, got: {yaml}");
    }

    #[test]
    fn group_message_emits_chat_block_and_topic() {
        let m = InputMessage {
            message_id: 9,
            text: Some("what does foo do".into()),
            timestamp: now(),
            attachments: vec![],
            author: MessageAuthor {
                name: "Alice".into(),
                username: Some("@alice".into()),
                user_id: Some(42),
            },
            forward_info: None,
            reply_to_id: None,
            chat: ChatContext::Group {
                id: -1001,
                title: Some("Dev".into()),
                topic_id: Some(7),
            },
            reply_to_body: None,
        };
        let yaml = format_cc_input(&[m]).unwrap();
        assert!(yaml.contains("chat:"), "got: {yaml}");
        assert!(yaml.contains("kind: group"));
        assert!(yaml.contains("id: -1001"));
        assert!(yaml.contains("title:"));
        assert!(yaml.contains("topic_id: 7"));
    }

    #[test]
    fn group_message_with_reply_to_body_emits_reply_to_block() {
        let m = InputMessage {
            message_id: 10,
            text: Some("explain this".into()),
            timestamp: now(),
            attachments: vec![],
            author: MessageAuthor {
                name: "Bob".into(),
                username: None,
                user_id: Some(43),
            },
            forward_info: None,
            reply_to_id: Some(5),
            chat: ChatContext::Group {
                id: -1001,
                title: None,
                topic_id: None,
            },
            reply_to_body: Some(ReplyToBody {
                author: MessageAuthor {
                    name: "Alice".into(),
                    username: Some("@alice".into()),
                    user_id: Some(42),
                },
                text: Some("here is the function: foo()".into()),
            }),
        };
        let yaml = format_cc_input(&[m]).unwrap();
        assert!(yaml.contains("reply_to:"), "got: {yaml}");
        assert!(yaml.contains("here is the function: foo()"));
    }
}
