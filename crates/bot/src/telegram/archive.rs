use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use super::mention::{AddressKind, BotIdentity, is_bot_addressed};
use super::msg_ext;
use super::session::effective_thread_id;
use frankenstein::types::Message;
use right_mcp::internal_db as ipc;

// Why 4: bounded so a Telegram traffic burst cannot queue unbounded
// blocking-pool tasks; SQLite WAL handles small write concurrency well.
const MAX_ARCHIVE_WRITES: usize = 4;
// Why 6s: a hair longer than the connection-level `busy_timeout` (5s) so a
// truly stuck writer surfaces as a logged failure rather than an indefinite
// hang on the blocking pool.
const ARCHIVE_WRITE_RETRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);
// Why 25ms: short enough that contention from another short write resolves
// within one or two sleeps; long enough that we are not spinning at busy-wait
// speed on the blocking pool.
const ARCHIVE_WRITE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

static ARCHIVE_WRITE_PERMITS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(MAX_ARCHIVE_WRITES)));

#[derive(Clone, Copy)]
struct ArchiveLogMeta {
    chat_id: i64,
    thread_id: i64,
    message_id: Option<i32>,
}

struct ArchivePayload {
    agent_dir: PathBuf,
    content: String,
    chat_id: i64,
    thread_id: i64,
    message_id: i32,
    sender_user_id: Option<i64>,
    sender_name: Option<String>,
    addressed_to_bot: bool,
    routed_to_agent: bool,
}

struct AssistantArchivePayload {
    agent_dir: PathBuf,
    agent_name: String,
    chat_id: i64,
    thread_id: i64,
    session_uuid: String,
    turn_id: u64,
    content: String,
}

pub(crate) fn should_archive_seen_group_message(msg: &Message) -> bool {
    !msg_ext::is_private(&msg.chat)
}

pub(crate) fn archive_content(msg: &Message) -> Option<String> {
    let mut parts = Vec::new();

    if let Some(content) = msg_ext::text_or_caption(msg).map(str::trim)
        && !content.is_empty()
    {
        parts.push(content.to_string());
    }

    parts.extend(
        super::attachments::extract_attachments(msg)
            .into_iter()
            .map(|attachment| match attachment.filename {
                Some(filename) => format!("[{}: {filename}]", attachment.kind.as_str()),
                None => format!("[{}]", attachment.kind.as_str()),
            }),
    );

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

pub(crate) fn archive_seen_group_message(agent_dir: &Path, identity: &BotIdentity, msg: &Message) {
    if !should_archive_seen_group_message(msg) {
        return;
    }

    let addressed_to_bot = is_bot_addressed(msg, identity).is_some();
    archive_user_message(agent_dir, msg, addressed_to_bot, false);
}

pub(crate) fn archive_routed_dm_message(
    agent_dir: &Path,
    msg: &Message,
    address: Option<AddressKind>,
) {
    if !msg_ext::is_private(&msg.chat) {
        return;
    }

    archive_user_message(agent_dir, msg, address.is_some(), true);
}

pub(crate) fn archive_channel_post(agent_dir: &Path, msg: &Message) {
    archive_user_message(agent_dir, msg, false, false);
}

fn archive_user_message(
    agent_dir: &Path,
    msg: &Message,
    addressed_to_bot: bool,
    routed_to_agent: bool,
) {
    let Some(payload) =
        ArchivePayload::from_message(agent_dir, msg, addressed_to_bot, routed_to_agent)
    else {
        return;
    };

    spawn_archive_write(payload);
}

impl ArchivePayload {
    fn from_message(
        agent_dir: &Path,
        msg: &Message,
        addressed_to_bot: bool,
        routed_to_agent: bool,
    ) -> Option<Self> {
        Some(Self {
            agent_dir: agent_dir.to_path_buf(),
            content: archive_content(msg)?,
            chat_id: msg.chat.id,
            thread_id: effective_thread_id(msg),
            message_id: msg.message_id,
            sender_user_id: msg.from.as_ref().map(|user| user.id as i64),
            sender_name: msg
                .from
                .as_ref()
                .map(|user| msg_ext::full_name(user))
                .or_else(|| {
                    msg.sender_chat
                        .as_ref()
                        .and_then(|chat| msg_ext::chat_title(chat).map(str::to_owned))
                }),
            addressed_to_bot,
            routed_to_agent,
        })
    }
}

pub(crate) fn archive_assistant_message(
    agent_dir: &Path,
    agent_name: &str,
    chat_id: i64,
    thread_id: i64,
    session_uuid: &str,
    turn_id: u64,
    content: String,
) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }
    let payload = AssistantArchivePayload {
        agent_dir: agent_dir.to_path_buf(),
        agent_name: agent_name.to_owned(),
        chat_id,
        thread_id,
        session_uuid: session_uuid.to_owned(),
        turn_id,
        content: trimmed.to_owned(),
    };
    let meta = ArchiveLogMeta {
        chat_id: payload.chat_id,
        thread_id: payload.thread_id,
        message_id: None,
    };
    with_archive_permit(meta, move || write_assistant_payload(payload));
}

/// Archive a channel post sent through the MCP UDS endpoint so `channel_read`
/// can include the agent's own publication.
pub(crate) async fn archive_outbound_channel_post(
    agent_dir: &Path,
    chat_id: i64,
    message_id: i32,
    content: &str,
) -> anyhow::Result<()> {
    let (client, agent) = crate::db::client_for_agent_dir(agent_dir)?;
    client
        .archive_message(&ipc::ArchiveMessageRequest {
            agent,
            request_id: crate::db::request_id(),
            message: ipc::ConversationMessageDto {
                platform: "telegram".to_owned(),
                chat_id,
                thread_id: 0,
                message_id: Some(message_id),
                sender_user_id: None,
                sender_name: None,
                addressed_to_bot: false,
                routed_to_agent: true,
                root_session_id: None,
                turn_id: None,
                role: "assistant".to_owned(),
                content: content.to_owned(),
            },
        })
        .await?;
    Ok(())
}

fn spawn_archive_write(payload: ArchivePayload) {
    let meta = ArchiveLogMeta {
        chat_id: payload.chat_id,
        thread_id: payload.thread_id,
        message_id: Some(payload.message_id),
    };
    with_archive_permit(meta, move || write_archive_payload(payload));
}

fn with_archive_permit<F, Fut>(meta: ArchiveLogMeta, work: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let permit = match Arc::clone(&ARCHIVE_WRITE_PERMITS).try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            tracing::warn!(
                chat_id = meta.chat_id,
                thread_id = meta.thread_id,
                message_id = meta.message_id,
                "telegram archive dropped: writer concurrency limit reached"
            );
            return;
        }
    };

    tokio::spawn(async move {
        let _permit = permit;
        work().await;
    });
}

async fn write_archive_payload(payload: ArchivePayload) {
    let meta = ArchiveLogMeta {
        chat_id: payload.chat_id,
        thread_id: payload.thread_id,
        message_id: Some(payload.message_id),
    };
    retry_archive_user_db_write(meta, "telegram archive", &payload).await;
}

async fn write_assistant_payload(payload: AssistantArchivePayload) {
    let meta = ArchiveLogMeta {
        chat_id: payload.chat_id,
        thread_id: payload.thread_id,
        message_id: None,
    };
    retry_archive_assistant_db_write(meta, "assistant archive", &payload).await;
}

fn archive_error_is_transient(error: &ipc::InternalDbError) -> bool {
    matches!(
        error,
        ipc::InternalDbError::Transport(_)
            | ipc::InternalDbError::Server {
                category: ipc::DbErrorCategory::Unavailable
                    | ipc::DbErrorCategory::NotReady
                    | ipc::DbErrorCategory::Transient,
                ..
            }
    )
}

async fn archive_with_retry(
    meta: ArchiveLogMeta,
    operation: &'static str,
    agent_dir: &Path,
    message: ipc::ConversationMessageDto,
) {
    let (client, agent) = match crate::db::client_for_agent_dir(agent_dir) {
        Ok(pair) => pair,
        Err(error) => {
            tracing::warn!(
                chat_id = meta.chat_id,
                thread_id = meta.thread_id,
                message_id = meta.message_id,
                operation,
                "archive internal client resolution failed: {error:#}"
            );
            return;
        }
    };
    // Reuse one request id across lost-response retries so owner-side
    // idempotency cannot duplicate an archive row.
    let request = ipc::ArchiveMessageRequest {
        agent,
        request_id: crate::db::request_id(),
        message,
    };
    let deadline = std::time::Instant::now() + ARCHIVE_WRITE_RETRY_TIMEOUT;
    let mut attempts = 0usize;
    loop {
        attempts += 1;
        match client.archive_message(&request).await {
            Ok(_) => return,
            Err(error)
                if archive_error_is_transient(&error) && std::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(ARCHIVE_WRITE_RETRY_DELAY).await;
            }
            Err(error) => {
                tracing::warn!(
                    chat_id = meta.chat_id,
                    thread_id = meta.thread_id,
                    message_id = meta.message_id,
                    attempts,
                    operation,
                    "archive owner write failed: {error:#}"
                );
                return;
            }
        }
    }
}

async fn retry_archive_user_db_write(
    meta: ArchiveLogMeta,
    operation: &'static str,
    payload: &ArchivePayload,
) {
    archive_with_retry(
        meta,
        operation,
        &payload.agent_dir,
        ipc::ConversationMessageDto {
            platform: "telegram".to_owned(),
            chat_id: payload.chat_id,
            thread_id: payload.thread_id,
            message_id: Some(payload.message_id),
            sender_user_id: payload.sender_user_id,
            sender_name: payload.sender_name.clone(),
            addressed_to_bot: payload.addressed_to_bot,
            routed_to_agent: payload.routed_to_agent,
            root_session_id: None,
            turn_id: None,
            role: "user".to_owned(),
            content: payload.content.clone(),
        },
    )
    .await;
}

async fn retry_archive_assistant_db_write(
    meta: ArchiveLogMeta,
    operation: &'static str,
    payload: &AssistantArchivePayload,
) {
    archive_with_retry(
        meta,
        operation,
        &payload.agent_dir,
        ipc::ConversationMessageDto {
            platform: "telegram".to_owned(),
            chat_id: payload.chat_id,
            thread_id: payload.thread_id,
            message_id: None,
            sender_user_id: None,
            sender_name: Some(payload.agent_name.clone()),
            addressed_to_bot: false,
            routed_to_agent: true,
            root_session_id: Some(payload.session_uuid.clone()),
            turn_id: Some(payload.turn_id),
            role: "assistant".to_owned(),
            content: payload.content.clone(),
        },
    )
    .await;
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
