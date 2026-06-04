use super::{GateDecision, SandboxHealth, SandboxRuntimeHandle, sandbox_gate};
use right_openshell::diagnosis::GatewayCause;
use std::sync::Arc;

fn unavailable() -> SandboxHealth {
    SandboxHealth::Unavailable {
        diagnosis: Arc::new(GatewayCause::DockerDown.diagnose()),
    }
}

#[test]
fn new_handle_starts_unavailable() {
    let (h, _rx) = SandboxRuntimeHandle::new(unavailable());
    assert!(matches!(h.health(), SandboxHealth::Unavailable { .. }));
    assert!(h.current_sandbox().is_none());
}

#[test]
fn set_unavailable_then_health_reflects_it() {
    let (h, _rx) = SandboxRuntimeHandle::new(SandboxHealth::Ready);
    h.set_unavailable(Arc::new(GatewayCause::GatewayNotStarted.diagnose()));
    match h.health() {
        SandboxHealth::Unavailable { diagnosis } => {
            assert_eq!(diagnosis.cause, GatewayCause::GatewayNotStarted)
        }
        _ => panic!("expected Unavailable"),
    }
}

#[test]
fn note_affected_dedupes_and_take_drains() {
    let (h, _rx) = SandboxRuntimeHandle::new(unavailable());
    h.note_affected(teloxide::types::ChatId(7), 0);
    h.note_affected(teloxide::types::ChatId(7), 0); // dup
    h.note_affected(teloxide::types::ChatId(7), 42);
    let drained = h.take_affected();
    assert_eq!(drained.len(), 2);
    assert!(h.take_affected().is_empty()); // drained
}

#[tokio::test]
async fn report_suspected_failure_wakes_supervisor() {
    let (h, mut rx) = SandboxRuntimeHandle::new(SandboxHealth::Ready);
    h.report_suspected_failure();
    // Non-blocking: a signal is queued.
    assert!(rx.try_recv().is_ok());
}

#[tokio::test]
async fn sync_failure_reporter_wakes_supervisor() {
    let (h, mut rx) = SandboxRuntimeHandle::new(SandboxHealth::Ready);
    let reporter: Option<std::sync::Arc<SandboxRuntimeHandle>> = Some(h.clone());
    crate::sync::report_sync_failure(reporter.as_deref());
    assert!(rx.try_recv().is_ok());
}

#[tokio::test]
async fn sync_failure_reporter_is_noop_without_handle() {
    crate::sync::report_sync_failure(None);
}

#[test]
fn non_sandboxed_always_proceeds() {
    assert_eq!(
        sandbox_gate(false, &SandboxHealth::Ready),
        GateDecision::Proceed
    );
    assert_eq!(sandbox_gate(false, &unavailable()), GateDecision::Proceed);
}

#[test]
fn sandboxed_and_ready_proceeds() {
    assert_eq!(
        sandbox_gate(true, &SandboxHealth::Ready),
        GateDecision::Proceed
    );
}

#[test]
fn sandboxed_and_unavailable_replies_never_proceeds() {
    match sandbox_gate(true, &unavailable()) {
        GateDecision::Reply { diagnosis } => assert_eq!(diagnosis.cause, GatewayCause::DockerDown),
        GateDecision::Proceed => {
            panic!("FAIL-CLOSED VIOLATION: sandboxed agent must not run on host")
        }
    }
}
