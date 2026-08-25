//! Integration scenarios covering memory failure handling.
//!
//! The former test harness passed an `agent_db_path` into
//! `ResilientHindsight` and inspected `pending_retains` with direct SQL. The
//! production interface now accepts an injected `PendingRetainSink`; these
//! tests use `InMemoryRetainQueue`, the lease-semantic reference
//! implementation for tests. Assertions map one-for-one: SQL row counts become
//! `RetainLeaseQueue::stats`, direct inserts become `PendingRetainSink::enqueue`,
//! and `drain_tick` becomes `drain_claimed`. SQL owner-adapter behavior is
//! covered separately in `right-memory::retain_queue` and the right owner API
//! tests.

use right_memory::hindsight::RetainItem;
use right_memory::resilient::{POLICY_AUTO_RETAIN, POLICY_MCP_RECALL};
use right_memory::retain_queue::drain_claimed;
use right_memory::retain_sink::{NewPendingRetain, PendingRetainSink, RetainLeaseQueue};
use right_memory::{MemoryStatus, ResilientError};

mod common;

fn queued(source: &str, content: &str) -> NewPendingRetain {
    NewPendingRetain {
        source: source.to_owned(),
        content: content.to_owned(),
        context: None,
        document_id: None,
        update_mode: None,
        tags: None,
    }
}

#[tokio::test]
async fn outage_queues_retain_and_degrades_status() {
    let (_h, url) = common::mock::always(500, r#"{"error":"boom"}"#).await;
    let (wrapper, queue) = common::wrap(&url, "bot").await;

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
    let n = queue.stats().await.unwrap().count;
    assert!(n >= 1, "expected queue non-empty, got {n}");
}

#[tokio::test]
async fn auth_failure_sets_auth_failed_status() {
    let (_h, url) = common::mock::always(401, r#"{"error":"bad key"}"#).await;
    let (wrapper, _queue) = common::wrap(&url, "bot").await;

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
    let (wrapper, queue) = common::wrap(&url, "bot").await;

    let _ = wrapper
        .retain("x", None, None, None, None, POLICY_AUTO_RETAIN)
        .await;

    assert_eq!(wrapper.client_drops_24h().await, 1);
    assert_eq!(queue.stats().await.unwrap().count, 0);
}

use common::switch::{ResponseSwitch, server};

#[tokio::test]
async fn recovery_drains_queue_after_breaker_closes() {
    let switch = ResponseSwitch::new(500, r#"{"error":"boom"}"#);
    let (_h, url) = server(switch.clone()).await;
    let (wrapper, queue) = common::wrap(&url, "bot").await;

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
    assert!(
        queue.stats().await.unwrap().count > 0,
        "expected non-empty queue"
    );

    // Flip mock to success. Wait past breaker open timer then drain.
    switch
        .set(200, r#"{"success":true,"operation_id":"op-1"}"#)
        .await;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    tokio::task::yield_now().await;
    tokio::time::resume();

    let report = drain_claimed(queue.as_ref(), |items| {
        let wrapper = &wrapper;
        async move {
            let item = RetainItem {
                content: items[0].content.clone(),
                context: items[0].context.clone(),
                document_id: items[0].document_id.clone(),
                update_mode: items[0].update_mode.clone(),
                tags: items[0].tags.clone(),
            };
            wrapper.drain_retain_item(&item).await
        }
    })
    .await;

    assert!(report.deleted > 0, "drain should delete at least one entry");
}

#[tokio::test]
async fn drain_poison_pill_deleted_good_records_still_processed() {
    let (_h, url) = common::mock::always(200, r#"{"success":true}"#).await;
    let (_wrapper, queue) = common::wrap(&url, "bot").await;

    queue.enqueue(queued("bot", "POISON")).await.unwrap();
    queue.enqueue(queued("bot", "GOOD")).await.unwrap();

    let report = drain_claimed(queue.as_ref(), |items| async move {
        if items[0].content == "POISON" {
            Err(right_memory::ErrorKind::Client)
        } else {
            Ok(())
        }
    })
    .await;

    assert_eq!(report.dropped_client, 1);
    assert_eq!(report.deleted, 1);
    assert_eq!(queue.stats().await.unwrap().count, 0);
}

#[tokio::test]
async fn queue_eviction_at_cap() {
    let (_h, url) = common::mock::always(200, r#"{"success":true}"#).await;
    let (_wrapper, queue) = common::wrap(&url, "bot").await;

    for i in 0..(right_memory::retain_queue::QUEUE_CAP + 5) {
        queue
            .enqueue(queued("bot", &format!("row-{i}")))
            .await
            .unwrap();
    }

    assert_eq!(
        queue.stats().await.unwrap().count,
        right_memory::retain_queue::QUEUE_CAP
    );
    let claim = queue
        .claim_batch(
            right_memory::retain_queue::QUEUE_CAP,
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert!(
        claim.items.iter().all(|item| item.content != "row-0"),
        "row-0 should have been evicted"
    );
}

#[tokio::test]
async fn two_wrappers_have_independent_breakers() {
    let (_h1, url_bad) = common::mock::always(500, r#"{"error":"x"}"#).await;
    let (_h2, url_ok) = common::mock::always(200, r#"{"results":[]}"#).await;

    let (bot_wrapper, _bot_queue) = common::wrap(&url_bad, "bot").await;
    let (aggregator_wrapper, _aggregator_queue) = common::wrap(&url_ok, "aggregator").await;

    for _ in 0..6 {
        let _ = bot_wrapper.recall("q", None, None, POLICY_MCP_RECALL).await;
    }
    assert!(matches!(
        bot_wrapper.status(),
        MemoryStatus::Degraded { .. }
    ));

    let result = aggregator_wrapper
        .recall("q", None, None, POLICY_MCP_RECALL)
        .await;
    assert!(result.is_ok(), "independent wrapper must still serve");
    assert!(matches!(aggregator_wrapper.status(), MemoryStatus::Healthy));
}
