//! Integration scenarios covering memory failure handling.

use rightclaw::memory::hindsight::{HindsightClient, RetainItem};
use rightclaw::memory::resilient::{
    POLICY_AUTO_RETAIN, POLICY_MCP_RECALL,
};
use rightclaw::memory::{MemoryStatus, ResilientError};

mod common;

#[tokio::test]
async fn outage_queues_retain_and_degrades_status() {
    let (_h, url) = common::mock::always(500, r#"{"error":"boom"}"#).await;
    let wrapper = common::wrap(&url, "bot").await;

    let err = wrapper
        .retain("turn-1", None, Some("doc-1"), Some("append"), None, POLICY_AUTO_RETAIN)
        .await
        .unwrap_err();
    assert!(matches!(err, ResilientError::Upstream(_)));

    // Trip the breaker with more transient failures.
    for _ in 0..4 {
        let _ = wrapper
            .retain("more", None, None, None, None, POLICY_AUTO_RETAIN)
            .await;
    }

    assert!(matches!(wrapper.status(), MemoryStatus::Degraded { .. }));

    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0)).unwrap();
    assert!(n >= 1, "expected queue non-empty, got {n}");
}

#[tokio::test]
async fn auth_failure_sets_auth_failed_status() {
    let (_h, url) = common::mock::always(401, r#"{"error":"bad key"}"#).await;
    let wrapper = common::wrap(&url, "bot").await;

    let err = wrapper
        .recall("q", None, None, POLICY_MCP_RECALL)
        .await
        .unwrap_err();
    assert!(matches!(err, ResilientError::Upstream(_)));
    assert!(matches!(wrapper.status(), MemoryStatus::AuthFailed { .. }));
}

#[tokio::test]
async fn client_error_drops_record_bumps_counter_no_enqueue() {
    let (_h, url) = common::mock::always(400, r#"{"error":"bad payload"}"#).await;
    let wrapper = common::wrap(&url, "bot").await;

    let _ = wrapper.retain("x", None, None, None, None, POLICY_AUTO_RETAIN).await;

    assert_eq!(wrapper.client_drops_24h().await, 1);
    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

use common::switch::{server, ResponseSwitch};

#[tokio::test]
async fn recovery_drains_queue_after_breaker_closes() {
    let sw = ResponseSwitch::new(500, r#"{"error":"boom"}"#);
    let (_h, url) = server(sw.clone()).await;
    let wrapper = common::wrap(&url, "bot").await;

    for i in 0..6 {
        let _ = wrapper
            .retain(&format!("turn-{i}"), None, Some("doc"), Some("append"), None, POLICY_AUTO_RETAIN)
            .await;
    }

    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();
    let queued: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0)).unwrap();
    assert!(queued > 0, "expected non-empty queue");

    // Flip mock to success. Wait past breaker open timer then drain.
    sw.set(200, r#"{"success":true,"operation_id":"op-1"}"#).await;
    tokio::time::sleep(std::time::Duration::from_secs(31)).await;

    let report = rightclaw::memory::retain_queue::drain_tick(&conn, |items| {
        let w = &wrapper;
        async move {
            let item = RetainItem {
                content: items[0].content.clone(),
                context: items[0].context.clone(),
                document_id: items[0].document_id.clone(),
                update_mode: items[0].update_mode.clone(),
                tags: items[0].tags.clone(),
            };
            w.drain_retain_item(&item).await
        }
    }).await;

    assert!(report.deleted > 0, "drain should have deleted at least one entry");
}
