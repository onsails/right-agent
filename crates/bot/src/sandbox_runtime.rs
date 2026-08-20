//! Lock-free shared sandbox-backend state. After construction the
//! `SandboxSupervisor` is the sole writer; every other consumer (message
//! worker, cron, delivery, curator, upgrade, dashboard) only reads, and reads
//! the sandbox handle *per unit of work*: recovery publishes a brand-new
//! `SandboxHandle` whose SDK connection belongs to the new VM, so a snapshot
//! taken at startup would address a deleted one.

use arc_swap::ArcSwap;
use right_sandbox::SandboxDiagnosis;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Current usability of the sandbox backend.
#[derive(Clone)]
pub enum SandboxHealth {
    Ready,
    Unavailable { diagnosis: Arc<SandboxDiagnosis> },
}

/// Shared handle. Cheap to clone via `Arc`. Reads are lock-free (`ArcSwap`).
pub struct SandboxRuntimeHandle {
    health: ArcSwap<SandboxHealth>,
    sandbox: ArcSwap<Option<crate::sandbox::Sandbox>>,
    affected: Mutex<HashSet<(i64, i64)>>,
    failure_tx: mpsc::Sender<()>,
    /// Counts `current_sandbox()` calls so tests can prove a consumer resolves
    /// the sandbox when it uses it rather than snapshotting it at construction
    /// (the staleness bug this indirection exists to prevent). Test-only: no
    /// production reader.
    #[cfg(test)]
    sandbox_reads: std::sync::atomic::AtomicUsize,
}

impl SandboxRuntimeHandle {
    /// Build a handle plus the receiver the supervisor owns. Capacity 1 with
    /// `try_send` coalesces bursts of failure reports into a single wake.
    ///
    /// `initial` is bring-up's outcome — the live sandbox, or the diagnosis
    /// explaining why there is none. Seeding health and sandbox together here
    /// is what makes the supervisor the *only* writer afterwards.
    pub fn new(
        initial: Result<crate::sandbox::Sandbox, Arc<SandboxDiagnosis>>,
    ) -> (Arc<Self>, mpsc::Receiver<()>) {
        let (failure_tx, failure_rx) = mpsc::channel(1);
        let (health, sandbox) = match initial {
            Ok(sandbox) => (SandboxHealth::Ready, Some(sandbox)),
            Err(diagnosis) => (SandboxHealth::Unavailable { diagnosis }, None),
        };
        let handle = Arc::new(Self {
            health: ArcSwap::from_pointee(health),
            sandbox: ArcSwap::from_pointee(sandbox),
            affected: Mutex::new(HashSet::new()),
            failure_tx,
            #[cfg(test)]
            sandbox_reads: std::sync::atomic::AtomicUsize::new(0),
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
    /// invariant on `set_ready`/`set_unavailable`).
    ///
    /// Call this once per unit of work — turn, cron job, delivery, dashboard
    /// request — and hold the result only for that unit: recovery replaces the
    /// handle, and the retired one talks to a VM that may no longer exist.
    pub fn current_sandbox(&self) -> Option<crate::sandbox::Sandbox> {
        #[cfg(test)]
        self.sandbox_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Option::clone(&self.sandbox.load())
    }

    /// How many times [`Self::current_sandbox`] has been called.
    #[cfg(test)]
    pub(crate) fn sandbox_reads(&self) -> usize {
        self.sandbox_reads
            .load(std::sync::atomic::Ordering::Relaxed)
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

    pub(crate) fn set_ready(&self, sandbox: crate::sandbox::Sandbox) {
        // Invariant: Ready ⟹ sandbox is Some. Publish the sandbox before
        // flipping health to Ready so a reader never sees Ready with no sandbox.
        self.sandbox.store(Arc::new(Some(sandbox)));
        self.health.store(Arc::new(SandboxHealth::Ready));
    }

    pub(crate) fn set_unavailable(&self, diagnosis: Arc<SandboxDiagnosis>) {
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
    Reply { diagnosis: Arc<SandboxDiagnosis> },
}

/// Decide whether a turn may invoke CC. **Fail-closed:** every agent is
/// sandboxed, so an unavailable backend MUST NOT run a turn (it would
/// otherwise execute on the host with `--dangerously-skip-permissions`).
pub fn sandbox_gate(health: &SandboxHealth) -> GateDecision {
    match health {
        SandboxHealth::Ready => GateDecision::Proceed,
        SandboxHealth::Unavailable { diagnosis } => GateDecision::Reply {
            diagnosis: Arc::clone(diagnosis),
        },
    }
}

#[cfg(test)]
#[path = "sandbox_runtime_tests.rs"]
mod tests;
