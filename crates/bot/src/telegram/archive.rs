use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use right_db::conversation::{ConversationMessage, ConversationRole, archive_message};
use teloxide::types::{ChatKind, Message};

use super::mention::{AddressKind, BotIdentity, is_bot_addressed};
use super::session::effective_thread_id;

const MAX_ARCHIVE_WRITES: usize = 4;

static ARCHIVE_WRITE_PERMITS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(MAX_ARCHIVE_WRITES)));

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

pub(crate) fn should_archive_seen_group_message(msg: &Message) -> bool {
    !matches!(msg.chat.kind, ChatKind::Private(_))
}

pub(crate) fn archive_content(msg: &Message) -> Option<String> {
    let mut parts = Vec::new();

    if let Some(content) = msg.text().or(msg.caption()).map(str::trim)
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
    if !matches!(msg.chat.kind, ChatKind::Private(_)) {
        return;
    }

    archive_user_message(agent_dir, msg, address.is_some(), true);
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
            chat_id: msg.chat.id.0,
            thread_id: effective_thread_id(msg),
            message_id: msg.id.0,
            sender_user_id: msg.from.as_ref().map(|user| user.id.0 as i64),
            sender_name: msg.from.as_ref().map(|user| user.full_name()),
            addressed_to_bot,
            routed_to_agent,
        })
    }
}

fn spawn_archive_write(payload: ArchivePayload) {
    let permit = match Arc::clone(&ARCHIVE_WRITE_PERMITS).try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            tracing::warn!(
                chat_id = payload.chat_id,
                thread_id = payload.thread_id,
                message_id = payload.message_id,
                "telegram archive dropped: writer concurrency limit reached"
            );
            return;
        }
    };

    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        write_archive_payload(payload);
    });
}

fn write_archive_payload(payload: ArchivePayload) {
    let conn = match right_db::open_connection(&payload.agent_dir, false) {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!(
                chat_id = payload.chat_id,
                thread_id = payload.thread_id,
                message_id = payload.message_id,
                "telegram archive open_connection failed: {e:#}"
            );
            return;
        }
    };

    let message = ConversationMessage {
        platform: "telegram",
        chat_id: payload.chat_id,
        thread_id: payload.thread_id,
        message_id: Some(payload.message_id),
        sender_user_id: payload.sender_user_id,
        sender_name: payload.sender_name.as_deref(),
        addressed_to_bot: payload.addressed_to_bot,
        routed_to_agent: payload.routed_to_agent,
        root_session_id: None,
        turn_id: None,
        role: ConversationRole::User,
        content: &payload.content,
    };

    if let Err(e) = archive_message(&conn, message) {
        tracing::warn!(
            chat_id = payload.chat_id,
            thread_id = payload.thread_id,
            message_id = payload.message_id,
            "telegram archive_message failed: {e:#}"
        );
    }
}

#[cfg(test)]
mod tests {
    use teloxide::types::Message;

    use super::super::mention::{AddressKind, BotIdentity};

    fn message(payload: serde_json::Value) -> Message {
        serde_json::from_value(payload).unwrap()
    }

    fn bot_identity() -> BotIdentity {
        BotIdentity {
            username: "rightaww_bot".to_string(),
            user_id: 999,
        }
    }

    fn archived_row(
        agent_dir: &std::path::Path,
        chat_id: i64,
        message_id: i32,
    ) -> Option<(String, i64)> {
        let conn = right_db::open_connection(agent_dir, false).unwrap();
        conn.query_row(
            "SELECT content, routed_to_agent
             FROM conversation_messages
             WHERE platform = 'telegram'
               AND chat_id = ?1
               AND message_id = ?2
               AND role = 'user'",
            (chat_id, message_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()
    }

    async fn poll_archived_row(
        agent_dir: &std::path::Path,
        chat_id: i64,
        message_id: i32,
    ) -> Option<(String, i64)> {
        poll_archived_row_for(
            agent_dir,
            chat_id,
            message_id,
            std::time::Duration::from_secs(2),
        )
        .await
    }

    async fn poll_archived_row_for(
        agent_dir: &std::path::Path,
        chat_id: i64,
        message_id: i32,
        timeout: std::time::Duration,
    ) -> Option<(String, i64)> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(row) = archived_row(agent_dir, chat_id, message_id) {
                return Some(row);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[test]
    fn archive_content_uses_text() {
        let msg = message(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": 42, "type": "private", "first_name": "User"},
            "from": {"id": 42, "is_bot": false, "first_name": "User"},
            "text": "  hello archive  "
        }));

        assert_eq!(
            super::archive_content(&msg),
            Some("hello archive".to_string())
        );
    }

    #[test]
    fn archive_content_uses_caption() {
        let msg = message(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": -1001, "type": "supergroup", "title": "g"},
            "from": {"id": 42, "is_bot": false, "first_name": "User"},
            "caption": "  caption  ",
            "photo": [{
                "file_id": "AgAD",
                "file_unique_id": "u",
                "width": 1,
                "height": 1
            }]
        }));

        assert_eq!(
            super::archive_content(&msg),
            Some("caption\n[photo]".to_string())
        );
    }

    #[test]
    fn archive_content_records_media_without_text() {
        let msg = message(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": -1001, "type": "supergroup", "title": "g"},
            "from": {"id": 42, "is_bot": false, "first_name": "User"},
            "document": {
                "file_id": "BAAD",
                "file_unique_id": "uniq",
                "file_name": "plan.pdf",
                "mime_type": "application/pdf",
                "file_size": 1024
            }
        }));

        assert_eq!(
            super::archive_content(&msg),
            Some("[document: plan.pdf]".to_string())
        );
    }

    #[test]
    fn group_messages_are_archivable_before_routing() {
        let msg = message(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": -1001, "type": "supergroup", "title": "g"},
            "from": {"id": 42, "is_bot": false, "first_name": "User"},
            "text": "unaddressed group message"
        }));

        assert!(super::should_archive_seen_group_message(&msg));
    }

    #[tokio::test]
    async fn group_archive_persists_unrouted_message_row() {
        let dir = tempfile::tempdir().unwrap();
        right_db::open_connection(dir.path(), true).unwrap();
        let msg = message(serde_json::json!({
            "message_id": 11,
            "date": 0,
            "chat": {"id": -1001, "type": "supergroup", "title": "g"},
            "from": {"id": 42, "is_bot": false, "first_name": "User"},
            "text": "  group archive  "
        }));

        super::archive_seen_group_message(dir.path(), &bot_identity(), &msg);

        let row = poll_archived_row(dir.path(), -1001, 11)
            .await
            .expect("archive row should be written");
        assert_eq!(row, ("group archive".to_string(), 0));
    }

    #[tokio::test]
    async fn routed_dm_archive_persists_routed_message_row() {
        let dir = tempfile::tempdir().unwrap();
        right_db::open_connection(dir.path(), true).unwrap();
        let msg = message(serde_json::json!({
            "message_id": 12,
            "date": 0,
            "chat": {"id": 42, "type": "private", "first_name": "User"},
            "from": {"id": 42, "is_bot": false, "first_name": "User"},
            "text": "dm archive"
        }));

        super::archive_routed_dm_message(dir.path(), &msg, Some(AddressKind::DirectMessage));

        let row = poll_archived_row(dir.path(), 42, 12)
            .await
            .expect("archive row should be written");
        assert_eq!(row, ("dm archive".to_string(), 1));
    }

    #[tokio::test]
    async fn private_message_seen_by_group_archive_does_not_persist() {
        let dir = tempfile::tempdir().unwrap();
        right_db::open_connection(dir.path(), true).unwrap();
        let msg = message(serde_json::json!({
            "message_id": 13,
            "date": 0,
            "chat": {"id": 42, "type": "private", "first_name": "User"},
            "from": {"id": 42, "is_bot": false, "first_name": "User"},
            "text": "private"
        }));

        super::archive_seen_group_message(dir.path(), &bot_identity(), &msg);

        assert!(
            poll_archived_row_for(dir.path(), 42, 13, std::time::Duration::from_millis(100))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn archive_seen_group_message_does_not_wait_for_locked_db() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = right_db::open_connection(dir.path(), true).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO conversation_messages (
                platform, chat_id, thread_id, message_id, sender_user_id, sender_name,
                addressed_to_bot, routed_to_agent, root_session_id, turn_id, role, content
             ) VALUES (
                'telegram', -1001, 0, 98, 42, 'User',
                0, 0, NULL, NULL, 'user', 'lock holder'
             )",
            [],
        )
        .unwrap();
        let msg = message(serde_json::json!({
            "message_id": 14,
            "date": 0,
            "chat": {"id": -1001, "type": "supergroup", "title": "g"},
            "from": {"id": 42, "is_bot": false, "first_name": "User"},
            "text": "contended"
        }));

        let started = std::time::Instant::now();
        super::archive_seen_group_message(dir.path(), &bot_identity(), &msg);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(250),
            "archive helper must not block on SQLite contention"
        );

        drop(tx);
        poll_archived_row(dir.path(), -1001, 14)
            .await
            .expect("archive row should be written after lock release");
    }
}
