//! Tests for Telegram message archiving.
//!
//! Semantic mapping: archive writes moved from spawned in-bot `data.db`
//! inserts to spawned typed owner IPC (`archive-message`). Tests that used to
//! poll a real SQLite file for the written row now poll the `FakeInternalApi`
//! request log for the exact `ArchiveMessageRequest` DTO; the assertions on
//! content, role, routing flags, and chat/thread/message ids are unchanged.
//! Owner-side row mapping (DTO → `conversation_messages` columns) is covered
//! in `crates/right/src/internal_api_db_tests.rs`.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

static ARCHIVE_TEST_MUTEX: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

use frankenstein::types::Message;
use right_mcp::internal_db as wire;

use super::super::mention::{AddressKind, BotIdentity};
use crate::test_support::{FakeInternalApi, ok, sync_handler};

/// Home layout fixture so `crate::db::client_for_agent_dir` resolves
/// `<home>/run/internal.sock` for `<home>/agents/alpha`.
struct ArchiveFixture {
    _home: tempfile::TempDir,
    agent_dir: PathBuf,
    fake: FakeInternalApi,
}

fn archive_fixture(handler: crate::test_support::Handler) -> ArchiveFixture {
    let home = tempfile::tempdir().expect("home tempdir");
    let agent_dir = home.path().join("agents").join("alpha");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    let run_dir = home.path().join("run");
    std::fs::create_dir_all(&run_dir).expect("run dir");
    let fake = FakeInternalApi::start_at(run_dir.join("internal.sock"), handler);
    ArchiveFixture {
        _home: home,
        agent_dir,
        fake,
    }
}

fn ok_handler() -> crate::test_support::Handler {
    sync_handler(|route, _body| {
        assert_eq!(route, wire::ROUTE_ARCHIVE_MESSAGE);
        ok(wire::ArchiveMessageResponse {
            id: 1,
            inserted: true,
        })
    })
}

fn message(payload: serde_json::Value) -> Message {
    serde_json::from_value(payload).unwrap()
}

fn bot_identity() -> BotIdentity {
    BotIdentity {
        username: "rightaww_bot".to_string(),
        user_id: 999,
    }
}

#[test]
fn archive_payload_falls_back_to_sender_chat_for_channel_posts() {
    let dir = tempfile::tempdir().unwrap();
    let msg: Message = serde_json::from_value(serde_json::json!({
        "message_id": 7, "date": 0,
        "chat": {"id": -1001234567890_i64, "type": "channel", "title": "RiskOff"},
        "sender_chat": {"id": -1001234567890_i64, "type": "channel", "title": "RiskOff"},
        "text": "hello channel"
    }))
    .unwrap();
    let payload = super::ArchivePayload::from_message(dir.path(), &msg, false, false).unwrap();
    assert_eq!(payload.sender_user_id, None);
    assert_eq!(payload.sender_name.as_deref(), Some("RiskOff"));
}

#[test]
fn archive_payload_prefers_from_over_sender_chat() {
    let dir = tempfile::tempdir().unwrap();
    let msg = message(serde_json::json!({
        "message_id": 8,
        "date": 0,
        "chat": {"id": -1001234567890_i64, "type": "channel", "title": "RiskOff"},
        "from": {"id": 42, "is_bot": false, "first_name": "User"},
        "sender_chat": {"id": -1001234567890_i64, "type": "channel", "title": "Chan"},
        "text": "both senders"
    }));
    let payload = super::ArchivePayload::from_message(dir.path(), &msg, false, false).unwrap();
    assert_eq!(payload.sender_name.as_deref(), Some("User"));
}

#[tokio::test]
async fn archive_outbound_channel_post_writes_assistant_row() {
    let fixture = archive_fixture(ok_handler());

    super::archive_outbound_channel_post(&fixture.agent_dir, -100, 7, "published post")
        .await
        .expect("archive outbound channel post");

    let recorded = fixture.fake.recorded().await;
    assert_eq!(recorded.len(), 1);
    let message = &recorded[0].body["message"];
    assert_eq!(message["role"], "assistant");
    assert_eq!(message["content"], "published post");
    assert_eq!(message["chat_id"], -100);
    assert_eq!(message["thread_id"], 0);
    assert_eq!(message["message_id"], 7);
    assert_eq!(message["routed_to_agent"], true);
}

#[tokio::test]
async fn archive_content_uses_text() {
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

#[tokio::test]
async fn archive_content_uses_caption() {
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

#[tokio::test]
async fn archive_content_records_media_without_text() {
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

#[tokio::test]
async fn group_messages_are_archivable_before_routing() {
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
    let _guard = ARCHIVE_TEST_MUTEX.lock().await;
    let fixture = archive_fixture(ok_handler());
    let msg = message(serde_json::json!({
        "message_id": 11,
        "date": 0,
        "chat": {"id": -1001, "type": "supergroup", "title": "g"},
        "from": {"id": 42, "is_bot": false, "first_name": "User"},
        "text": "  group archive  "
    }));

    super::archive_seen_group_message(&fixture.agent_dir, &bot_identity(), &msg);

    let recorded = fixture
        .fake
        .wait_for_requests(1, Duration::from_secs(6))
        .await;
    assert_eq!(recorded.len(), 1, "archive request should be sent");
    let message = &recorded[0].body["message"];
    assert_eq!(message["content"], "group archive");
    assert_eq!(message["role"], "user");
    assert_eq!(message["chat_id"], -1001);
    assert_eq!(message["message_id"], 11);
    assert_eq!(message["routed_to_agent"], false);
}

#[tokio::test]
async fn routed_dm_archive_persists_routed_message_row() {
    let _guard = ARCHIVE_TEST_MUTEX.lock().await;
    let fixture = archive_fixture(ok_handler());
    let msg = message(serde_json::json!({
        "message_id": 12,
        "date": 0,
        "chat": {"id": 42, "type": "private", "first_name": "User"},
        "from": {"id": 42, "is_bot": false, "first_name": "User"},
        "text": "dm archive"
    }));

    super::archive_routed_dm_message(&fixture.agent_dir, &msg, Some(AddressKind::DirectMessage));

    let recorded = fixture
        .fake
        .wait_for_requests(1, Duration::from_secs(6))
        .await;
    assert_eq!(recorded.len(), 1, "archive request should be sent");
    let message = &recorded[0].body["message"];
    assert_eq!(message["content"], "dm archive");
    assert_eq!(message["role"], "user");
    assert_eq!(message["chat_id"], 42);
    assert_eq!(message["routed_to_agent"], true);
    assert_eq!(message["addressed_to_bot"], true);
}

#[tokio::test]
async fn private_message_seen_by_group_archive_does_not_persist() {
    let _guard = ARCHIVE_TEST_MUTEX.lock().await;
    let fixture = archive_fixture(ok_handler());
    let msg = message(serde_json::json!({
        "message_id": 13,
        "date": 0,
        "chat": {"id": 42, "type": "private", "first_name": "User"},
        "from": {"id": 42, "is_bot": false, "first_name": "User"},
        "text": "private"
    }));

    super::archive_seen_group_message(&fixture.agent_dir, &bot_identity(), &msg);

    assert!(
        fixture
            .fake
            .wait_for_requests(1, Duration::from_millis(100))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn archive_seen_group_message_does_not_wait_for_owner_response() {
    let _guard = ARCHIVE_TEST_MUTEX.lock().await;
    // Semantic mapping: the old test held a SQLite write lock and asserted
    // the archive helper never blocks on database contention. Writes go to
    // the owner over IPC now, so the equivalent contract is that the helper
    // returns promptly even when the owner is slow to respond; the spawned
    // write still lands afterwards.
    let fixture = archive_fixture(sync_handler(|route, _body| {
        assert_eq!(route, wire::ROUTE_ARCHIVE_MESSAGE);
        std::thread::sleep(Duration::from_millis(500));
        ok(wire::ArchiveMessageResponse {
            id: 1,
            inserted: true,
        })
    }));
    let msg = message(serde_json::json!({
        "message_id": 14,
        "date": 0,
        "chat": {"id": -1001, "type": "supergroup", "title": "g"},
        "from": {"id": 42, "is_bot": false, "first_name": "User"},
        "text": "contended"
    }));

    let started = std::time::Instant::now();
    super::archive_seen_group_message(&fixture.agent_dir, &bot_identity(), &msg);
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "archive helper must not block on a slow owner"
    );

    let recorded = fixture
        .fake
        .wait_for_requests(1, Duration::from_secs(6))
        .await;
    assert_eq!(recorded.len(), 1, "archive write should land after return");
    assert_eq!(recorded[0].body["message"]["message_id"], 14);
}
