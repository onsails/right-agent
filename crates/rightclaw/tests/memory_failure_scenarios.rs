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
