//! Behavioral tests for the delivery loop.
//!
//! The pending-fetch / per-job-dedup SQL moved to the Aggregator owner during
//! the single-owner cutover, and its semantics coverage moved with it:
//! `crates/right/src/internal_api_db_tests.rs` exercises the real owner
//! queries (ordering and filters, `NULLIF(target_chat_id, 0)`, `force_notify`
//! reads, supersede with group-OR `force_notify`, and target-snapshot
//! resilience after spec deletion) through the typed routes. The tests below
//! cover bot-owned logic; where the old suite drove a direct
//! `right_db::Connection` fixture, it now drives a [`FakeInternalApi`] and
//! asserts the typed wire contract.

use std::collections::HashSet;
use std::sync::Arc;

use right_mcp::internal_db as wire;

use super::*;
use crate::test_support::{FakeInternalApi, ok, sync_handler};

#[test]
fn attachment_failure_fatal_only_when_no_body_sent() {
    // No body content reached the user yet → retry is safe and required.
    assert!(attachment_failure_is_fatal(0));
    // Body content already delivered → retry would duplicate it, so the
    // attachment failure must be tolerated, not fatal.
    assert!(!attachment_failure_is_fatal(1));
    assert!(!attachment_failure_is_fatal(3));
}

#[test]
fn attachments_only_with_header_keeps_attachment_failure_fatal() {
    // Regression: an attachments-only delivery sends the one-line platform
    // status header standalone before the attachment batch. That header send
    // must NOT count as body content — otherwise an attachment failure would
    // flip non-fatal and the user's payload would be dropped with no retry.
    // The body-content count stays 0, so the failure remains fatal (requeue).
    let body_messages_sent_after_header_only = 0usize;
    assert!(attachment_failure_is_fatal(
        body_messages_sent_after_header_only
    ));
}

#[tokio::test]
async fn delivery_mode_shutdown_flush_skips_idle_gate() {
    assert!(should_wait_for_idle(DeliveryMode::Normal, 10));
    assert!(!should_wait_for_idle(DeliveryMode::ShutdownFlush, 10));
}

#[test]
fn subprocess_deadline_merges_with_caller_control_and_preserves_diagnostic() {
    let token = tokio_util::sync::CancellationToken::new();
    let now = tokio::time::Instant::now();
    let internal_deadline = now + DELIVERY_TIMEOUT;
    let caller_deadline = now + std::time::Duration::from_secs(10);
    let caller_control = DeliveryShutdownControl {
        token: Some(&token),
        deadline: Some(caller_deadline),
    };

    let caller_bounded = delivery_subprocess_control(caller_control, internal_deadline);
    assert_eq!(caller_bounded.shutdown.deadline, Some(caller_deadline));
    assert!(std::ptr::eq(caller_bounded.shutdown.token.unwrap(), &token));
    assert_eq!(
        caller_bounded.deadline_error,
        DELIVERY_INTERRUPTED_BY_SHUTDOWN
    );

    let delivery_bounded = delivery_subprocess_control(
        DeliveryShutdownControl {
            token: Some(&token),
            deadline: None,
        },
        internal_deadline,
    );
    assert_eq!(delivery_bounded.shutdown.deadline, Some(internal_deadline));
    assert!(std::ptr::eq(
        delivery_bounded.shutdown.token.unwrap(),
        &token
    ));
    assert_eq!(delivery_bounded.deadline_error, DELIVERY_TIMEOUT_ERROR);

    let later_caller = delivery_subprocess_control(
        DeliveryShutdownControl {
            token: None,
            deadline: Some(internal_deadline + std::time::Duration::from_secs(1)),
        },
        internal_deadline,
    );
    assert_eq!(later_caller.shutdown.deadline, Some(internal_deadline));
    assert_eq!(later_caller.deadline_error, DELIVERY_TIMEOUT_ERROR);
}

#[tokio::test]
async fn shutdown_deadline_bounds_single_delivery_attempt() {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(10);
    let result = run_or_delivery_shutdown(
        DeliveryShutdownControl {
            token: None,
            deadline: Some(deadline),
        },
        async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            true
        },
    )
    .await;

    assert_eq!(
        result.unwrap_err(),
        DELIVERY_INTERRUPTED_BY_SHUTDOWN.to_owned()
    );
}

#[tokio::test]
async fn delivery_shutdown_cancels_pending_future() {
    let shutdown = tokio_util::sync::CancellationToken::new();
    shutdown.cancel();

    let result = run_or_delivery_shutdown(
        DeliveryShutdownControl {
            token: Some(&shutdown),
            deadline: None,
        },
        async { true },
    )
    .await;

    assert_eq!(
        result.unwrap_err(),
        DELIVERY_INTERRUPTED_BY_SHUTDOWN.to_owned()
    );
}

#[tokio::test]
async fn delivery_without_shutdown_waits_for_future() {
    let result = run_or_delivery_shutdown(
        DeliveryShutdownControl {
            token: None,
            deadline: None,
        },
        async { true },
    )
    .await;

    assert_eq!(result, Ok(true));
}

#[tokio::test]
async fn delivery_shutdown_interruption_is_not_retry_failure() {
    assert!(is_delivery_shutdown_interruption(
        DELIVERY_INTERRUPTED_BY_SHUTDOWN
    ));
    assert!(!is_delivery_shutdown_interruption(
        "stdin write: broken pipe"
    ));
}

#[tokio::test]
async fn shutdown_bounded_telegram_send_timeout_is_terminal_not_retryable() {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(10);
    let result = run_telegram_request_with_shutdown(
        DeliveryShutdownControl {
            token: None,
            deadline: Some(deadline),
        },
        false,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok::<_, String>(())
        },
    )
    .await;

    let error = result.unwrap_err();
    assert!(is_delivery_terminal_shutdown_send_error(&error));
    assert!(!is_delivery_shutdown_interruption(&error));
}

#[tokio::test]
async fn expired_shutdown_deadline_does_not_start_fresh_telegram_send() {
    let polled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let was_polled = Arc::clone(&polled);
    let deadline = tokio::time::Instant::now() - std::time::Duration::from_millis(1);

    let result = run_telegram_request_with_shutdown(
        DeliveryShutdownControl {
            token: None,
            deadline: Some(deadline),
        },
        false,
        async move {
            was_polled.store(true, std::sync::atomic::Ordering::SeqCst);
            std::future::pending::<Result<(), String>>().await
        },
    )
    .await;

    assert_eq!(
        result.unwrap_err(),
        DELIVERY_INTERRUPTED_BY_SHUTDOWN.to_owned()
    );
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn cancelled_shutdown_after_prior_send_is_terminal_without_new_send_poll() {
    let polled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let was_polled = Arc::clone(&polled);
    let shutdown = tokio_util::sync::CancellationToken::new();
    shutdown.cancel();

    let result = run_telegram_request_with_shutdown(
        DeliveryShutdownControl {
            token: Some(&shutdown),
            deadline: None,
        },
        true,
        async move {
            was_polled.store(true, std::sync::atomic::Ordering::SeqCst);
            std::future::pending::<Result<(), String>>().await
        },
    )
    .await;

    assert!(is_delivery_terminal_shutdown_send_error(
        &result.unwrap_err()
    ));
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
}

fn pending_dto(id: &str, kind: &str, producer_ref: Option<&str>) -> wire::PendingAsyncResultDto {
    wire::PendingAsyncResultDto {
        id: id.to_owned(),
        kind: kind.to_owned(),
        producer_ref: producer_ref.map(str::to_owned),
        delivery_json: "{\"kind\":\"notify\",\"content\":\"x\"}".to_owned(),
        run_note: "note".to_owned(),
        status: "success".to_owned(),
        target_chat_id: Some(-100),
        target_thread_id: None,
        force_notify: false,
    }
}

fn test_client(socket: &std::path::Path) -> right_mcp::internal_client::InternalClient {
    right_mcp::internal_client::InternalClient::new(socket)
}

#[tokio::test]
async fn fetch_next_pending_skips_in_memory_delivered_oldest() {
    // Semantic mapping: the old suite seeded two `async_runs` rows and read
    // them back through the in-bot SQL helper. The batch fetch now runs
    // owner-side; the bot logic under test is the in-memory dedup skip and
    // the mark-delivered call for the skipped row.
    let fake = FakeInternalApi::start(sync_handler(|route, _body| match route {
        wire::ROUTE_DELIVERY_FETCH_PENDING => ok(wire::DeliveryFetchPendingResponse {
            pending: vec![
                pending_dto("a", "cron", Some("job1")),
                pending_dto("b", "cron", Some("job1")),
            ],
        }),
        wire::ROUTE_DELIVERY_MARK_OUTCOME => ok(wire::OkResponse {}),
        other => panic!("unexpected route {other}"),
    }));
    let client = test_client(fake.socket_path());
    let delivered_in_memory = HashSet::from(["a".to_string()]);

    let pending = fetch_next_pending(&client, "alpha", &delivered_in_memory)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.id, "b");

    let recorded = fake.recorded().await;
    let mark = recorded
        .iter()
        .find(|request| request.route == wire::ROUTE_DELIVERY_MARK_OUTCOME)
        .expect("skipped in-memory row must be marked delivered owner-side");
    assert_eq!(mark.body["run_id"], "a");
    assert_eq!(mark.body["status"], "delivered");
}

#[tokio::test]
async fn select_delivery_candidate_does_not_deduplicate_background_rows() {
    // Semantic mapping: the old suite asserted two background rows stayed
    // `pending` in `async_runs` after candidate selection. Background runs
    // have no per-job dedup, so the bot must select the fetched row without
    // issuing any owner request at all.
    let fake = FakeInternalApi::start(sync_handler(|route, _body| {
        panic!("background candidate selection must not call the owner: {route}")
    }));
    let client = test_client(fake.socket_path());

    let pending = pending_from_dto(pending_dto("bg-old", "background", None));
    let (selected, skipped) = select_delivery_candidate(&client, "alpha", pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.id, "bg-old");
    assert_eq!(skipped, 0);
    assert!(fake.recorded().await.is_empty());
}

#[tokio::test]
async fn select_delivery_candidate_maps_owner_dedup_response() {
    // The cron path delegates dedup to the owner; the bot maps the winning
    // DTO and superseded count through unchanged.
    let fake = FakeInternalApi::start(sync_handler(|route, body| {
        assert_eq!(route, wire::ROUTE_DELIVERY_DEDUPLICATE_JOB);
        assert_eq!(body["producer_ref"], "job1");
        ok(wire::DeliveryDeduplicateJobResponse {
            candidate: Some(pending_dto("b", "cron", Some("job1"))),
            superseded: 1,
        })
    }));
    let client = test_client(fake.socket_path());

    let pending = pending_from_dto(pending_dto("a", "cron", Some("job1")));
    let (selected, skipped) = select_delivery_candidate(&client, "alpha", pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.id, "b");
    assert_eq!(skipped, 1);
}

fn yaml_pending(kind: &str, status: &str, job: Option<&str>, content: &str) -> PendingAsyncResult {
    PendingAsyncResult {
        id: "abc".into(),
        kind: kind.into(),
        producer_ref: job.map(str::to_owned),
        delivery_json: format!("{{\"kind\":\"notify\",\"content\":\"{content}\"}}"),
        run_note: "Checked 5 pairs".into(),
        status: status.into(),
        target_chat_id: None,
        target_thread_id: None,
        force_notify: false,
    }
}

#[tokio::test]
async fn format_async_yaml_basic_cron() {
    let pending = yaml_pending("cron", "success", Some("health-check"), "BTC up 2%");
    let output = format_async_yaml(&pending, 2).unwrap();
    // Instruction prefix assertions
    assert!(output.starts_with("You are delivering a cron job result"));
    assert!(output.contains("place it VERBATIM in your reply's `content` field"));
    assert!(output.contains("Do NOT call `mcp__right__send_message`"));
    assert!(output.contains("never repeat the content text in a caption"));
    assert!(output.contains("Here is the YAML report of the cron job:"));
    // YAML content assertions
    assert!(output.contains("job: \"health-check\""));
    assert!(output.contains("runs_total: 3"));
    assert!(output.contains("skipped_runs: 2"));
    assert!(output.contains("BTC up 2%"));
    assert!(output.contains("Checked 5 pairs"));
}

#[tokio::test]
async fn format_async_yaml_no_skipped() {
    let pending = yaml_pending("cron", "success", Some("job1"), "hello");
    let output = format_async_yaml(&pending, 0).unwrap();
    assert!(output.starts_with("You are delivering a cron job result"));
    assert!(output.contains("runs_total: 1"));
    assert!(!output.contains("skipped_runs"));
}

#[tokio::test]
async fn format_async_yaml_uses_cron_failure_instruction_when_status_failed() {
    let pending = yaml_pending(
        "cron",
        "failed",
        Some("watcher"),
        "Partial data fetched then hit budget",
    );
    let out = format_async_yaml(&pending, 0).unwrap();
    assert!(out.contains("did not complete successfully"));
    assert!(!out.contains("send it VERBATIM"));
}

#[tokio::test]
async fn format_async_yaml_uses_cron_success_instruction_when_status_success() {
    let pending = yaml_pending("cron", "success", Some("watcher"), "BTC up 2%");
    let out = format_async_yaml(&pending, 0).unwrap();
    assert!(out.contains("VERBATIM"));
}

#[tokio::test]
async fn format_async_yaml_rejects_silent_delivery_json() {
    let pending = PendingAsyncResult {
        delivery_json: r#"{"kind":"silent","reason":"No changes"}"#.into(),
        ..yaml_pending("cron", "success", Some("watcher"), "")
    };
    let err = format_async_yaml(&pending, 0).unwrap_err();
    assert!(
        err.to_string().contains("not a notify decision"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn format_async_yaml_background_includes_background_instruction_and_content() {
    let pending = PendingAsyncResult {
        target_chat_id: Some(-100),
        ..yaml_pending(
            "background",
            "success",
            None,
            "Finished the answer in background",
        )
    };

    let out = format_async_yaml(&pending, 0).unwrap();
    assert!(out.starts_with("You are delivering a background task result"));
    assert!(out.contains("background_result:"));
    assert!(out.contains("label: \"background\""));
    assert!(out.contains("Finished the answer in background"));
    assert!(!out.contains("cron_result:"));
}

#[tokio::test]
async fn format_async_yaml_background_uses_failure_instruction_when_status_failed() {
    let pending = PendingAsyncResult {
        target_chat_id: Some(-100),
        ..yaml_pending(
            "background",
            "failed",
            Some("custom-bg"),
            "Background work failed",
        )
    };

    let out = format_async_yaml(&pending, 0).unwrap();
    assert!(out.contains("background task below did not complete successfully"));
    assert!(out.contains("label: \"custom-bg\""));
    assert!(out.contains("Background work failed"));
}

#[tokio::test]
async fn delivery_invocation_uses_configured_agent_model() {
    let args = build_delivery_invocation_args(
        "/sandbox/mcp.json".into(),
        r#"{"type":"object"}"#.into(),
        Some("claude-opus-4-8[1m]".into()),
        Some("session-1".into()),
        None,
    );

    let model_pos = args
        .iter()
        .position(|arg| arg == "--model")
        .expect("configured model must be passed to Claude");
    assert_eq!(args[model_pos + 1], "claude-opus-4-8[1m]");
    assert!(
        !args.iter().any(|arg| arg == "claude-haiku-4-5-20251001"),
        "delivery must not override the configured agent model with Haiku"
    );
}

fn fake_allowlist(users: &[i64], groups: &[i64]) -> right_agent::agent::allowlist::AllowlistState {
    use right_agent::agent::allowlist::{
        AllowedGroup, AllowedUser, AllowlistState, GroupKind, ResponseMode,
    };
    let now = chrono::Utc::now();
    let mut state = AllowlistState::default();
    for &id in users {
        state.add_user(AllowedUser {
            id,
            label: None,
            added_by: None,
            added_at: now,
        });
    }
    for &id in groups {
        state.add_group(AllowedGroup {
            id,
            label: None,
            opened_by: None,
            opened_at: now,
            mode: ResponseMode::Addressed,
            topics: Vec::new(),
            kind: GroupKind::Group,
        });
    }
    state
}

#[test]
fn null_target_classifies_as_no_target() {
    // Semantic mapping: the old suite fetched a legacy row through SQL and
    // classified the result. Fetching is owner-side now (covered in
    // `internal_api_db_tests.rs`); classification itself is a pure bot
    // function over the DTO-shaped struct, exercised here directly. The
    // `target_chat_id = 0` → `None` normalization lives in the owner query
    // (`NULLIF(target_chat_id, 0)`), also covered owner-side.
    let pending = PendingAsyncResult {
        target_chat_id: None,
        ..yaml_pending("cron", "success", Some("legacy"), "x")
    };
    let outcome = classify_pending_target(&pending, &fake_allowlist(&[], &[]));
    assert!(
        matches!(outcome, TargetClassification::NoTarget),
        "got: {outcome:?}"
    );
}

#[test]
fn target_not_in_allowlist_classifies_as_denied() {
    let pending = PendingAsyncResult {
        target_chat_id: Some(-777),
        ..yaml_pending("cron", "success", Some("agenda"), "x")
    };
    let outcome = classify_pending_target(&pending, &fake_allowlist(&[100], &[-200]));
    assert!(
        matches!(outcome, TargetClassification::Denied),
        "got: {outcome:?}"
    );
}

#[test]
fn target_in_allowlist_classifies_as_ready() {
    let pending = PendingAsyncResult {
        target_chat_id: Some(-200),
        target_thread_id: Some(5),
        ..yaml_pending("cron", "success", Some("agenda"), "x")
    };
    let outcome = classify_pending_target(&pending, &fake_allowlist(&[], &[-200]));
    assert!(
        matches!(
            outcome,
            TargetClassification::Ready {
                chat_id: -200,
                thread_id: Some(5)
            }
        ),
        "got: {outcome:?}"
    );
}

#[tokio::test]
async fn empty_delivery_send_report_is_rejected() {
    let report = DeliverySendReport {
        text_messages_sent: 0,
        attachment_batches_sent: 0,
    };

    let err = ensure_delivery_send_report_non_empty(report).unwrap_err();
    assert!(err.contains("empty delivery reply"));
}

#[test]
fn force_notify_skips_idle_gate() {
    // Non-forced, recently active chat → held.
    assert!(should_hold_delivery(false, DeliveryMode::Normal, 10));
    // Forced → never held, even when active.
    assert!(!should_hold_delivery(true, DeliveryMode::Normal, 10));
    // Idle long enough → not held regardless.
    assert!(!should_hold_delivery(
        false,
        DeliveryMode::Normal,
        IDLE_THRESHOLD_SECS + 1
    ));
}

fn test_pending(
    kind: &str,
    status: &str,
    job: Option<&str>,
    force_notify: bool,
) -> PendingAsyncResult {
    PendingAsyncResult {
        id: "x".into(),
        kind: kind.into(),
        producer_ref: job.map(|s| s.to_string()),
        delivery_json: "{}".into(),
        run_note: String::new(),
        status: status.into(),
        target_chat_id: Some(1),
        target_thread_id: None,
        force_notify,
    }
}

#[test]
fn header_success_scheduled() {
    let p = test_pending("cron", "success", Some("sources-update"), false);
    assert_eq!(render_delivery_header(&p), "✓ sources-update · success");
}

#[test]
fn header_success_manual() {
    let p = test_pending("cron", "success", Some("sources-update"), true);
    assert_eq!(
        render_delivery_header(&p),
        "✓ sources-update · manual run · success"
    );
}

#[test]
fn header_failed() {
    let p = test_pending("cron", "failed", Some("sources-update"), false);
    assert_eq!(render_delivery_header(&p), "✗ sources-update · failed");
}

#[test]
fn header_background_label_fallback() {
    let p = test_pending("background", "success", None, false);
    assert_eq!(render_delivery_header(&p), "✓ background task · success");
}

#[test]
fn header_background_slug_normalized() {
    // Real background runs carry `producer_ref = Some("background")`; the raw
    // slug must not surface — present "background task" instead.
    let p = test_pending("background", "success", Some("background"), false);
    assert_eq!(render_delivery_header(&p), "✓ background task · success");
}

#[test]
fn header_preserves_label_as_literal_plain_text() {
    let p = test_pending("cron", "success", Some("a<b>&c"), false);
    assert_eq!(render_delivery_header(&p), "✓ a<b>&c · success");
}

#[test]
fn prepend_header_separates_with_blank_lines() {
    let out = prepend_delivery_header("✓ job · success", "body text");
    assert_eq!(out, "✓ job · success\n\nbody text");
}

#[test]
fn prepend_header_handles_empty_body() {
    let out = prepend_delivery_header("✓ job · success", "");
    assert_eq!(out, "✓ job · success");
}
