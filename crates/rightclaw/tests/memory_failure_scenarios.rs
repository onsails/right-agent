//! Integration scenarios covering memory failure handling.

use rightclaw::memory::hindsight::RetainItem;
use rightclaw::memory::resilient::{POLICY_AUTO_RETAIN, POLICY_MCP_RECALL};
use rightclaw::memory::{MemoryStatus, ResilientError};

mod common;

#[tokio::test]
async fn outage_queues_retain_and_degrades_status() {
    let (_h, url) = common::mock::always(500, r#"{"error":"boom"}"#).await;
    let wrapper = common::wrap(&url, "bot").await;

    let err = wrapper
        .retain(
            "turn-1",
            None,
            Some("doc-1"),
            Some("append"),
            None,
            POLICY_AUTO_RETAIN,
        )
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
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0))
        .unwrap();
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

    let _ = wrapper
        .retain("x", None, None, None, None, POLICY_AUTO_RETAIN)
        .await;

    assert_eq!(wrapper.client_drops_24h().await, 1);
    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

use common::switch::{ResponseSwitch, server};

#[tokio::test]
async fn recovery_drains_queue_after_breaker_closes() {
    let sw = ResponseSwitch::new(500, r#"{"error":"boom"}"#);
    let (_h, url) = server(sw.clone()).await;
    let wrapper = common::wrap(&url, "bot").await;

    for i in 0..6 {
        let _ = wrapper
            .retain(
                &format!("turn-{i}"),
                None,
                Some("doc"),
                Some("append"),
                None,
                POLICY_AUTO_RETAIN,
            )
            .await;
    }

    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();
    let queued: i64 = conn
        .query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0))
        .unwrap();
    assert!(queued > 0, "expected non-empty queue");

    // Flip mock to success. Wait past breaker open timer then drain.
    sw.set(200, r#"{"success":true,"operation_id":"op-1"}"#)
        .await;
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
    })
    .await;

    assert!(
        report.deleted > 0,
        "drain should have deleted at least one entry"
    );
}

#[tokio::test]
async fn drain_poison_pill_deleted_good_records_still_processed() {
    let (_h, url) = common::mock::always(200, r#"{"success":true}"#).await;
    let wrapper = common::wrap(&url, "bot").await;
    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();

    rightclaw::memory::retain_queue::enqueue(&conn, "bot", "POISON", None, None, None, None)
        .unwrap();
    rightclaw::memory::retain_queue::enqueue(&conn, "bot", "GOOD", None, None, None, None).unwrap();

    let report = rightclaw::memory::retain_queue::drain_tick(&conn, |items| async move {
        if items[0].content == "POISON" {
            Err(rightclaw::memory::ErrorKind::Client)
        } else {
            Ok(())
        }
    })
    .await;

    assert_eq!(report.dropped_client, 1);
    assert_eq!(report.deleted, 1);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn queue_eviction_at_cap() {
    let (_h, url) = common::mock::always(200, r#"{"success":true}"#).await;
    let wrapper = common::wrap(&url, "bot").await;
    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();

    for i in 0..(rightclaw::memory::retain_queue::QUEUE_CAP + 5) {
        let c = format!("row-{i}");
        rightclaw::memory::retain_queue::enqueue(&conn, "bot", &c, None, None, None, None).unwrap();
    }

    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n as usize, rightclaw::memory::retain_queue::QUEUE_CAP);
    let first_gone: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pending_retains WHERE content = 'row-0'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(first_gone, 0, "row-0 should have been evicted");
}

#[tokio::test]
async fn two_wrappers_have_independent_breakers() {
    let (_h1, url_bad) = common::mock::always(500, r#"{"error":"x"}"#).await;
    let (_h2, url_ok) = common::mock::always(200, r#"{"results":[]}"#).await;

    let bot_wrapper = common::wrap(&url_bad, "bot").await;
    let agg_wrapper = common::wrap(&url_ok, "aggregator").await;

    for _ in 0..6 {
        let _ = bot_wrapper.recall("q", None, None, POLICY_MCP_RECALL).await;
    }
    assert!(matches!(
        bot_wrapper.status(),
        MemoryStatus::Degraded { .. }
    ));

    let res = agg_wrapper.recall("q", None, None, POLICY_MCP_RECALL).await;
    assert!(res.is_ok(), "independent wrapper must still serve");
    assert!(matches!(agg_wrapper.status(), MemoryStatus::Healthy));
}
