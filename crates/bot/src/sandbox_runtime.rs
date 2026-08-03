//! Lock-free shared sandbox-backend state. The `SandboxSupervisor` is the
//! sole writer; every other consumer (message worker, dashboard) only reads.

use arc_swap::ArcSwap;
use right_openshell::diagnosis::GatewayDiagnosis;
use right_openshell::sandbox_exec::SandboxExec;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Current usability of the sandbox backend.
#[derive(Clone)]
pub enum SandboxHealth {
    Ready,
    Unavailable { diagnosis: Arc<GatewayDiagnosis> },
}

/// Shared handle. Cheap to clone via `Arc`. Reads are lock-free (`ArcSwap`).
pub struct SandboxRuntimeHandle {
    health: ArcSwap<SandboxHealth>,
    sandbox: ArcSwap<Option<SandboxExec>>,
    affected: Mutex<HashSet<(i64, i64)>>,
    failure_tx: mpsc::Sender<()>,
}

impl SandboxRuntimeHandle {
    /// Build a handle plus the receiver the supervisor owns. Capacity 1 with
    /// `try_send` coalesces bursts of failure reports into a single wake.
    pub fn new(initial: SandboxHealth) -> (Arc<Self>, mpsc::Receiver<()>) {
        let (failure_tx, failure_rx) = mpsc::channel(1);
        let handle = Arc::new(Self {
            health: ArcSwap::from_pointee(initial),
            sandbox: ArcSwap::from_pointee(None),
            affected: Mutex::new(HashSet::new()),
            failure_tx,
        });
        (handle, failure_rx)
    }

    // ---- reads (any consumer) ----

    pub fn health(&self) -> SandboxHealth {
        SandboxHealth::clone(&self.health.load())
    }

    pub fn is_ready(&self) -> bool {
        matches!(**self.health.load(), SandboxHealth::Ready)
    }

    /// Live sandbox handle, `Some` iff health is `Ready` (see the store-order
    /// invariant on `set_ready`/`set_unavailable`). Reserved for the deferred
    /// follow-up letting the dashboard/keepalive read the sandbox from the
    /// handle instead of their startup snapshots; currently exercised only by
    /// tests.
    pub fn current_sandbox(&self) -> Option<SandboxExec> {
        Option::clone(&self.sandbox.load())
    }

    /// Record a chat that saw an "unavailable" reply, for the back-online notice.
    pub fn note_affected(&self, chat_id: i64, eff_thread_id: i64) {
        self.affected
            .lock()
            .expect("affected mutex poisoned")
            .insert((chat_id, eff_thread_id));
    }

    /// Ask the supervisor to verify the backend. Coalesced; never blocks.
    pub fn report_suspected_failure(&self) {
        let _ = self.failure_tx.try_send(());
    }

    // ---- writes (supervisor only) ----

    pub(crate) fn set_ready(&self, sandbox: SandboxExec) {
        // Invariant: Ready ⟹ sandbox is Some. Publish the sandbox before
        // flipping health to Ready so a reader never sees Ready with no sandbox.
        self.sandbox.store(Arc::new(Some(sandbox)));
        self.health.store(Arc::new(SandboxHealth::Ready));
    }

    pub(crate) fn set_unavailable(&self, diagnosis: Arc<GatewayDiagnosis>) {
        // Invariant: Ready ⟹ sandbox is Some. Flip health to Unavailable
        // before clearing the sandbox so a reader never sees Ready with no sandbox.
        self.health
            .store(Arc::new(SandboxHealth::Unavailable { diagnosis }));
        self.sandbox.store(Arc::new(None));
    }

    pub(crate) fn take_affected(&self) -> Vec<(i64, i64)> {
        std::mem::take(&mut *self.affected.lock().expect("affected mutex poisoned"))
            .into_iter()
            .collect()
    }
}

/// Outcome of the pre-invocation sandbox gate.
#[derive(Debug, PartialEq, Eq)]
pub enum GateDecision {
    Proceed,
    Reply { diagnosis: Arc<GatewayDiagnosis> },
}

/// Decide whether a turn may invoke CC. **Fail-closed:** a sandboxed agent
/// with an unavailable backend MUST NOT run (it would otherwise execute on the
/// host with `--dangerously-skip-permissions`).
pub fn sandbox_gate(is_sandboxed: bool, health: &SandboxHealth) -> GateDecision {
    match (is_sandboxed, health) {
        (false, _) | (true, SandboxHealth::Ready) => GateDecision::Proceed,
        (true, SandboxHealth::Unavailable { diagnosis }) => GateDecision::Reply {
            diagnosis: Arc::clone(diagnosis),
        },
    }
}

#[cfg(test)]
#[path = "sandbox_runtime_tests.rs"]
mod tests;
