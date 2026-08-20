# Sandbox Supervisor Phase-Aware Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the bot's `SandboxSupervisor` detect a sandbox that has entered OpenShell `Phase: Error` (gateway still reachable) and degrade the agent into a recoverable/diagnosed state, instead of silently staying "Ready" forever.

**Architecture:** Replace the gateway-only reachability probe with a probe that queries the real sandbox phase over gRPC (`GetSandbox.phase`), so `monitor_step` degrades on `SANDBOX_PHASE_ERROR`. Give the long-lived sync task a `SandboxRuntimeHandle` so its repeated "sandbox is not ready" failures reach the supervisor. Add a dedicated `GatewayCause::SandboxError` so the operator-facing Telegram/CLI copy names the real cause. Gap 3 (Error-phase recovery) is **resolved, not a decision point**: OpenShell exposes no pod restart/resume (CLI and gRPC proto both confirmed), and **Right never touches the container runtime directly — OpenShell is the sole abstraction over Docker.** So recovery is detect → degrade with an honest diagnosis → keep retrying `bring_up_sandbox` with backoff, which **auto-resumes the moment OpenShell (a graceful future upgrade) or the operator restores the sandbox to Ready**. No recovery action is added; no sandbox recreation, ever.

**Load-bearing invariant (do not violate):** Right must not shell out to `docker`/`podman` or use a container-runtime API. Reviving a killed/Errored pod is OpenShell's responsibility. `docker start` of the exited container *does* fully recover it with data intact (validated live), but Right deliberately does not use that path.

**Tech Stack:** Rust (edition 2024), tokio, tonic gRPC (`openshell.v1`), `arc-swap`, `thiserror`/`miette`, `right-openshell`, `bot` crate, `right-ui`/`sandbox_copy` for user-facing copy.

---

## Background: verified root cause

Three gaps, all confirmed against current code:

- **Gap 1 — health detection is gateway-reachability, not sandbox phase.**
  `crates/bot/src/sandbox_supervisor.rs::probe_reachable()` (line 458) only does
  `right_openshell::openshell::connect_grpc(...).await.is_ok()` — a connect to the
  *gateway*. `monitor_step` (lines 480-498) calls it before degrading; when a failure
  is reported but the gateway answers, it returns `LoopStep::Continue` and never calls
  `handle.set_unavailable(...)`. After the v0.0.50→v0.0.56 upgrade the gateway stayed
  up while the `right` agent's sandbox pod was `Phase: Error`, so recovery never triggered.

- **Gap 2 — the sync task sees the death but can't signal recovery.**
  `report_suspected_failure()` (`crates/bot/src/sandbox_runtime.rs:69`) is the only
  recovery trigger; it is called from `crates/bot/src/telegram/worker.rs:1531` and
  `crates/bot/src/keepalive.rs:97`. The long-lived `run_sync_task`
  (`crates/bot/src/sync.rs:35-58`) logs `sync cycle failed: ...` every 5 min but holds
  no `SandboxRuntimeHandle`, so the clearest periodic signal never reaches the supervisor.

- **Gap 3 (resolved) — Error phase has no in-OpenShell recovery, and Right must not work around it.**
  Investigation outcome, verified on a live v0.0.56 gateway: a gateway upgrade **SIGKILLs**
  the sandbox container (`Exit 137`, OOM=false; the container's `RestartPolicy=unless-stopped`
  is suppressed because the kill counts as an explicit stop), and OpenShell exposes **no**
  restart/resume — neither in the CLI (`sandbox` has only create/get/list/delete/exec/
  connect/upload/download) nor in the gRPC proto (only `CreateSandbox`/`GetSandbox`/
  `ListSandboxes`/`DeleteSandbox`/`Exec`/`GetSandboxConfig`/`WatchSandbox`/token RPCs).
  Container data survives in the stopped writable layer (`/sandbox` is **not** on a volume),
  and `docker start` of the exited container fully recovers it (validated on both agents).
  **Right deliberately does NOT use that path — OpenShell is the sole abstraction over the
  container runtime.** So `bring_up_sandbox` (which reuses the existing sandbox;
  `is_sandbox_ready` at `openshell.rs:122` returns `false` for an Error pod) keeps failing
  and the supervisor stays degraded, **retrying with backoff and auto-resuming the instant the
  sandbox is Ready again** — when a future graceful OpenShell upgrade, or a manual operator
  action, restores it. Recreation remains FORBIDDEN (ARCHITECTURE: "Never delete sandboxes for
  recovery").

## Code facts the implementer must know (read before starting)

- `SandboxReadiness`, its `is_error()`/`is_ready()`/`describe()` methods, and
  `get_sandbox_readiness()` are **private** in `crates/right-openshell/src/openshell.rs`
  (lines 401-455). The phase consts `SANDBOX_PHASE_READY`/`SANDBOX_PHASE_ERROR` are
  private (lines 18-21). Task 1 adds a **public** phase-query function; do not make the
  struct itself public.
- `GatewayCause` (`crates/right-openshell/src/diagnosis.rs:12-27`) has NO sandbox-phase
  variant. Task 2 adds `SandboxError { sandbox: String }`.
- The supervisor loop future is `!Send` and runs on a `LocalSet` via `spawn_blocking`
  (`run_supervisor` / `spawn_supervisor`, lines 547-605). Preserve that — do not add
  `Send` bounds or move work off the LocalSet.
- Health writes (`set_ready`/`set_unavailable`) are `pub(crate)` and supervisor-only by
  contract (`sandbox_runtime.rs:73-88` comment "writes (supervisor only)"). The sync task
  must NOT call them; it only calls the public `report_suspected_failure()`.
- `connect_grpc` / `default_mtls_dir` are public
  (`openshell.rs:247` / `:97`). `OpenShellClient<Channel>` is the gRPC client type.
- `diagnose_gateway()` (`diagnosis.rs:121`) is the fallback diagnosis used by
  `monitor_step` today; keep it as the fallback when the phase probe itself can't run.
- User-facing outage copy lives in `crates/bot/src/sandbox_copy.rs`
  (`unavailable_message`, `back_online_message`) and is HTML-escaped for `ParseMode::Html`.

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/right-openshell/src/openshell.rs` | Public `sandbox_phase_status()` returning a typed phase result | Modify |
| `crates/right-openshell/src/diagnosis.rs` | New `GatewayCause::SandboxError` variant + diagnosis text | Modify |
| `crates/right-openshell/src/diagnosis_tests.rs` | Test diagnosis for the new variant | Modify |
| `crates/bot/src/sandbox_supervisor.rs` | Phase-aware `probe_reachable` rename + `monitor_step` degrade logic; wire handle into sync task | Modify |
| `crates/bot/src/sandbox_runtime.rs` | (no API change; document the sync-task reporter path) | Read only |
| `crates/bot/src/sync.rs` | Optional `SandboxRuntimeHandle` reporter on sync-cycle failure | Modify |
| `crates/bot/src/sandbox_supervisor_phase_tests.rs` (new) | Pure-logic tests for the phase→degrade decision | Create |

---

## Task 1: Expose a public sandbox-phase query in `right-openshell`

The supervisor needs the real pod phase over gRPC. Today the only public phase helper is
`is_sandbox_ready` (bool, collapses Error into "not ready") and `wait_for_ready` (a polling
loop). Add a single-shot, non-collapsing query that distinguishes Ready / Error / Other /
NotFound so callers can decide policy. This respects "Readiness polling diagnostics: do not
collapse OpenShell status into a bare boolean."

**Files:**
- Modify: `crates/right-openshell/src/openshell.rs` (add public enum + function near the
  private `get_sandbox_readiness`, ~line 432)

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` block at the bottom of
`crates/right-openshell/src/openshell.rs` (find it with
`rg -n "mod tests" crates/right-openshell/src/openshell.rs`). This test exercises the pure
classifier helper, not gRPC:

```rust
    #[test]
    fn phase_status_classifies_phase_ints() {
        assert_eq!(
            SandboxPhaseStatus::from_phase(SANDBOX_PHASE_READY, "ok".to_owned()),
            SandboxPhaseStatus::Ready
        );
        match SandboxPhaseStatus::from_phase(SANDBOX_PHASE_ERROR, "boom".to_owned()) {
            SandboxPhaseStatus::Error { detail } => assert_eq!(detail, "boom"),
            other => panic!("expected Error, got {other:?}"),
        }
        match SandboxPhaseStatus::from_phase(
            SandboxPhase::Provisioning as i32,
            "prov".to_owned(),
        ) {
            SandboxPhaseStatus::Other { phase, detail } => {
                assert_eq!(phase, "PROVISIONING");
                assert_eq!(detail, "prov");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell phase_status_classifies_phase_ints`
Expected: FAIL to compile — `SandboxPhaseStatus` not found.

- [ ] **Step 3: Write minimal implementation**

Add this public type and constructor **above** `get_sandbox_readiness` (after the private
`SandboxReadiness` impl, ~line 430). Keep `SandboxReadiness` private and unchanged.

```rust
/// Non-collapsing single-shot sandbox phase classification. Distinguishes the
/// states callers must act on differently: `Ready` (usable), `Error` (terminal
/// pod failure — recovery policy decided by the caller), `NotFound` (gone),
/// and every other transient phase. Carries the human-readable status detail so
/// diagnostics and logs preserve the OpenShell phase/status (see ARCHITECTURE
/// "Readiness polling diagnostics").
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SandboxPhaseStatus {
    Ready,
    Error { detail: String },
    Other { phase: String, detail: String },
    NotFound,
}

impl SandboxPhaseStatus {
    /// Pure classifier over a raw phase int + status summary. Separated from the
    /// gRPC call so it is unit-testable without a live gateway.
    fn from_phase(phase: i32, detail: String) -> Self {
        if phase == SANDBOX_PHASE_READY {
            SandboxPhaseStatus::Ready
        } else if phase == SANDBOX_PHASE_ERROR {
            SandboxPhaseStatus::Error { detail }
        } else {
            SandboxPhaseStatus::Other {
                phase: sandbox_phase_name(phase).to_owned(),
                detail,
            }
        }
    }
}

/// Query the current sandbox phase over gRPC. Propagates RPC errors (FAIL FAST);
/// maps gRPC `NotFound` to `SandboxPhaseStatus::NotFound` rather than an error,
/// because "the sandbox is gone" is a phase the caller acts on, not a transport
/// failure.
pub async fn sandbox_phase_status(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> miette::Result<SandboxPhaseStatus> {
    match get_sandbox_readiness(client, name).await? {
        Some(readiness) => Ok(SandboxPhaseStatus::from_phase(
            readiness.phase,
            readiness.describe(),
        )),
        None => Ok(SandboxPhaseStatus::NotFound),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-openshell phase_status_classifies_phase_ints`
Expected: PASS.

- [ ] **Step 5: Build the crate to confirm no warnings/unused**

Run: `devenv shell -- cargo build -p right-openshell`
Expected: builds clean. (`from_phase` is `dead_code`-clean because the test uses it; the
`pub` function uses `describe()`.)

- [ ] **Step 6: Commit**

```bash
git add crates/right-openshell/src/openshell.rs
git commit -m "feat(openshell): add public sandbox_phase_status query"
```

---

## Task 2: Add `GatewayCause::SandboxError` diagnosis

When the supervisor degrades because the pod is in `Phase: Error`, the operator-facing
message must name that cause — not the generic "I can't reach the sandbox backend"
(`Unreachable`), which would mislead the operator into restarting Docker.

**Files:**
- Modify: `crates/right-openshell/src/diagnosis.rs:12-90`
- Modify: `crates/right-openshell/src/diagnosis_tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/right-openshell/src/diagnosis_tests.rs` (open it first to match the existing
test style; it imports `super::*`):

```rust
    #[test]
    fn sandbox_error_diagnosis_names_the_sandbox_and_is_recovery_oriented() {
        let d = GatewayCause::SandboxError {
            sandbox: "right-him-1".to_owned(),
        }
        .diagnose();
        assert_eq!(
            d.cause,
            GatewayCause::SandboxError {
                sandbox: "right-him-1".to_owned()
            }
        );
        assert!(d.summary.contains("right-him-1"));
        assert!(!d.fixes.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell sandbox_error_diagnosis_names_the_sandbox`
Expected: FAIL to compile — no `SandboxError` variant.

- [ ] **Step 3: Write minimal implementation**

In `crates/right-openshell/src/diagnosis.rs`, add the variant to the enum (after
`SandboxNotFound`, ~line 24):

```rust
    /// The sandbox exists but its pod entered OpenShell `Phase: Error` (e.g. after
    /// a gateway upgrade). The gateway is still reachable, so this is distinct from
    /// `Unreachable`. Recovery policy is decided by the supervisor (see the
    /// SandboxError handling in sandbox_supervisor.rs).
    SandboxError {
        sandbox: String,
    },
```

In `diagnose()`, add a default `(summary, fixes)` arm in the first `match &self` (before the
closing `}`, alongside the other arms, ~line 73):

```rust
            GatewayCause::SandboxError { .. } => (
                "my secure sandbox stopped responding",
                vec![
                    "I'll reconnect automatically as soon as the sandbox is back. \
                     If it stays down, the sandbox container needs to be restarted \
                     in OpenShell — your data is preserved.",
                ],
            ),
```

And add an enriching arm in the second `match &self` that builds the data-carrying summary
(~line 80, alongside `SandboxNotFound`):

```rust
            GatewayCause::SandboxError { sandbox } => {
                format!("my secure sandbox '{sandbox}' is in an error state")
            }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-openshell sandbox_error_diagnosis_names_the_sandbox`
Expected: PASS.

- [ ] **Step 5: Confirm no non-exhaustive-match breakage**

Run: `devenv shell -- cargo build -p right-openshell`
Expected: builds clean. (`GatewayCause` matches in `diagnose()` are the only exhaustive ones;
`classify_doctor_output` only constructs variants, never matches all.) If any consumer in
`bot` matches `GatewayCause` exhaustively, that surfaces in Task 4's build — handle it there.

- [ ] **Step 6: Commit**

```bash
git add crates/right-openshell/src/diagnosis.rs crates/right-openshell/src/diagnosis_tests.rs
git commit -m "feat(openshell): add SandboxError gateway-diagnosis cause"
```

---

## Task 3: Make `monitor_step` degrade on `Phase: Error` (Gap 1)

Rename the gateway-only `probe_reachable` to a phase-aware probe that returns a typed
decision, and have `monitor_step` degrade with the right diagnosis when the pod is in Error.

This is the core fix. The probe runs only after a failure report (same trigger point as
today), so it adds no steady-state gRPC traffic. Keep the LocalSet/`!Send` shape: the probe
is `async` and awaits gRPC through the captured runtime handle, exactly like the existing
`probe_reachable`.

**Files:**
- Modify: `crates/bot/src/sandbox_supervisor.rs:455-499`
- Create: `crates/bot/src/sandbox_supervisor_phase_tests.rs`
- Modify: `crates/bot/src/sandbox_supervisor.rs` (add `#[cfg(test)] #[path = ...] mod` at end)

- [ ] **Step 1: Write the failing test (pure decision logic)**

The probe does I/O (gRPC), so extract the **decision** into a pure function
`degrade_decision(probe: ProbeOutcome, sandbox: &str) -> Option<GatewayDiagnosis>` and test
that. Create `crates/bot/src/sandbox_supervisor_phase_tests.rs`:

```rust
use super::{ProbeOutcome, degrade_decision};
use right_openshell::diagnosis::GatewayCause;

#[test]
fn ready_phase_does_not_degrade() {
    assert!(degrade_decision(ProbeOutcome::Ready, "right-him-1").is_none());
}

#[test]
fn error_phase_degrades_with_sandbox_error_cause() {
    let diag = degrade_decision(
        ProbeOutcome::Error {
            detail: "phase=ERROR status=...".to_owned(),
        },
        "right-him-1",
    )
    .expect("Error phase must degrade");
    assert_eq!(
        diag.cause,
        GatewayCause::SandboxError {
            sandbox: "right-him-1".to_owned()
        }
    );
}

#[test]
fn gateway_unreachable_degrades_with_gateway_diagnosis() {
    // When the gateway itself can't be reached, the probe can't read a phase;
    // we fall back to the gateway diagnosis (e.g. DockerDown/Unreachable).
    let diag = degrade_decision(
        ProbeOutcome::GatewayDiagnosis(GatewayCause::DockerDown.diagnose()),
        "right-him-1",
    )
    .expect("gateway failure must degrade");
    assert_eq!(diag.cause, GatewayCause::DockerDown);
}

#[test]
fn transient_other_phase_does_not_degrade() {
    // PROVISIONING/UNKNOWN etc. are transient; a single failure report should not
    // flip a sandbox that is merely mid-transition.
    assert!(
        degrade_decision(
            ProbeOutcome::Other {
                phase: "PROVISIONING".to_owned(),
                detail: "prov".to_owned(),
            },
            "right-him-1"
        )
        .is_none()
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p bot degrade_decision`
Expected: FAIL to compile — `ProbeOutcome` / `degrade_decision` not found, module not wired.

- [ ] **Step 3: Write minimal implementation**

In `crates/bot/src/sandbox_supervisor.rs`, replace `probe_reachable` (lines 455-462) with the
typed probe + pure decision, and add the test-module hook. First add the new types and
functions (replace the whole `probe_reachable` block):

```rust
use right_openshell::diagnosis::{GatewayCause, GatewayDiagnosis, diagnose_gateway};
use right_openshell::openshell::SandboxPhaseStatus;

/// Result of verifying a reported failure: either the sandbox's real phase, or a
/// gateway-level diagnosis when we couldn't even read the phase.
enum ProbeOutcome {
    Ready,
    Error { detail: String },
    Other { phase: String, detail: String },
    GatewayDiagnosis(GatewayDiagnosis),
}

/// Pure mapping from a verified probe outcome to a degrade diagnosis (or `None`
/// to keep running). `Error` → `SandboxError`; gateway failure → its diagnosis;
/// `Ready`/`Other`/`NotFound`-handled-upstream → no degrade.
fn degrade_decision(outcome: ProbeOutcome, sandbox: &str) -> Option<GatewayDiagnosis> {
    match outcome {
        ProbeOutcome::Ready => None,
        ProbeOutcome::Other { phase, detail } => {
            tracing::debug!(%phase, %detail, "sandbox in transient phase; not degrading");
            None
        }
        ProbeOutcome::Error { detail } => {
            tracing::warn!(%detail, "sandbox is in ERROR phase");
            Some(
                GatewayCause::SandboxError {
                    sandbox: sandbox.to_owned(),
                }
                .diagnose(),
            )
        }
        ProbeOutcome::GatewayDiagnosis(diag) => Some(diag),
    }
}

/// Verify a reported failure by reading the sandbox's real phase over gRPC. If
/// the gateway itself is unreachable, fall back to a gateway diagnosis. A
/// `NotFound` sandbox is treated as a gateway-level `SandboxNotFound` diagnosis.
async fn probe_phase(sandbox: &str) -> ProbeOutcome {
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = match right_openshell::openshell::connect_grpc(&mtls_dir).await {
        Ok(c) => c,
        Err(_) => return ProbeOutcome::GatewayDiagnosis(diagnose_gateway().await),
    };
    match right_openshell::openshell::sandbox_phase_status(&mut client, sandbox).await {
        Ok(SandboxPhaseStatus::Ready) => ProbeOutcome::Ready,
        Ok(SandboxPhaseStatus::Error { detail }) => ProbeOutcome::Error { detail },
        Ok(SandboxPhaseStatus::Other { phase, detail }) => ProbeOutcome::Other { phase, detail },
        Ok(SandboxPhaseStatus::NotFound) => ProbeOutcome::GatewayDiagnosis(
            GatewayCause::SandboxNotFound {
                sandbox: sandbox.to_owned(),
            }
            .diagnose(),
        ),
        Err(e) => {
            // Phase RPC failed though the channel connected — inconclusive.
            // Fall back to the gateway diagnosis rather than degrading on a
            // transient RPC hiccup.
            tracing::warn!("sandbox phase probe failed: {e:#}");
            ProbeOutcome::GatewayDiagnosis(diagnose_gateway().await)
        }
    }
}
```

Then rewrite `monitor_step`'s failure arm (lines 482-497) to use them:

```rust
        msg = failure_rx.recv() => {
            if msg.is_none() {
                return LoopStep::Break;
            }
            // Verify with a single phase probe before degrading. A transient
            // worker error against a still-Ready sandbox must not flip health.
            let outcome = probe_phase(&deps.resolved_sandbox).await;
            let Some(diag) = degrade_decision(outcome, &deps.resolved_sandbox) else {
                return LoopStep::Continue;
            };
            tracing::error!(agent = %deps.agent, cause = ?diag.cause, "{}", diag.summary);
            handle.set_unavailable(Arc::new(diag));
            if let Some(t) = sync_task.take() {
                t.abort();
            }
            LoopStep::Continue
        }
```

Remove the now-unused top-of-file imports only if your new `use` lines duplicate them: the
file already imports `diagnose_gateway` and `GatewayDiagnosis` via
`use right_openshell::diagnosis::{GatewayCause, GatewayDiagnosis, diagnose_gateway};` at
line 12 — do NOT add a second `use`. Add only `use right_openshell::openshell::SandboxPhaseStatus;`
and extend the existing line-12 import with `GatewayCause` if it is not already present
(it currently imports `GatewayCause, GatewayDiagnosis, diagnose_gateway` — confirm with
`rg -n "use right_openshell::diagnosis" crates/bot/src/sandbox_supervisor.rs`).

Finally, add the test-module hook at the very end of the file:

```rust
#[cfg(test)]
#[path = "sandbox_supervisor_phase_tests.rs"]
mod phase_tests;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p bot degrade_decision`
Expected: PASS (4 tests).

- [ ] **Step 5: Build the bot crate (catch the exhaustive-match risk from Task 2)**

Run: `devenv shell -- cargo build -p bot`
Expected: builds clean. If a `match GatewayCause` elsewhere in `bot` is now non-exhaustive,
add a `SandboxError { .. }` arm there mirroring the nearest existing arm's behavior, then
rebuild. (`crates/bot/src/sandbox_copy.rs` uses the `GatewayDiagnosis` fields, not a
`GatewayCause` match, so it is unaffected.)

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/sandbox_supervisor.rs crates/bot/src/sandbox_supervisor_phase_tests.rs
git commit -m "fix(supervisor): degrade on sandbox Phase: Error, not just gateway loss"
```

---

## Task 4: Route sync-task failures to the supervisor (Gap 2)

The periodic sync task is the most reliable detector of a dead sandbox ("sandbox is not
ready" every 5 min) but currently can't signal recovery. Give it an optional
`SandboxRuntimeHandle` and call `report_suspected_failure()` on a sync-cycle failure. The
supervisor's Task-3 phase probe then verifies and degrades. The sync task NEVER writes health
directly (that is supervisor-only by contract).

**Files:**
- Modify: `crates/bot/src/sync.rs:35-58` (`run_sync_task` signature + failure arm)
- Modify: `crates/bot/src/sandbox_supervisor.rs:447-453` (`spawn_sync_task` passes the handle)

- [ ] **Step 1: Write the failing test**

`run_sync_task` is a loop driving real gRPC, so test the **reporting decision** in isolation.
Extract the report into a tiny pure-ish helper and test that the handle receives the wake.
Add to `crates/bot/src/sandbox_runtime_tests.rs` (it already imports the handle types):

```rust
#[tokio::test]
async fn sync_failure_reporter_wakes_supervisor() {
    let (h, mut rx) = SandboxRuntimeHandle::new(SandboxHealth::Ready);
    // The sync task holds an Option<Arc<Handle>>; on cycle failure it reports.
    let reporter: Option<std::sync::Arc<SandboxRuntimeHandle>> = Some(h.clone());
    crate::sync::report_sync_failure(reporter.as_deref());
    assert!(rx.try_recv().is_ok());
}

#[tokio::test]
async fn sync_failure_reporter_is_noop_without_handle() {
    // No handle (e.g. mode: none agent) → no panic, nothing to assert beyond that.
    crate::sync::report_sync_failure(None);
}
```

Note: the test references `SandboxHealth` — add it to the `use super::{...}` line at the top
of `sandbox_runtime_tests.rs` if absent (`rg -n "use super" crates/bot/src/sandbox_runtime_tests.rs`).
`SandboxRuntimeHandle::clone` works because the handle is wrapped in `Arc` by `new`; if `h`
is `Arc<SandboxRuntimeHandle>` already, `h.clone()` clones the `Arc`.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p bot sync_failure_reporter`
Expected: FAIL to compile — `report_sync_failure` not found.

- [ ] **Step 3: Write minimal implementation**

In `crates/bot/src/sync.rs`, add the reporter helper near the top (after the imports):

```rust
use std::sync::Arc;

use crate::sandbox_runtime::SandboxRuntimeHandle;

/// Ask the supervisor to verify the backend after a sync-cycle failure. No-op for
/// agents without a runtime handle (e.g. `sandbox: mode: none`). Coalesced and
/// non-blocking; the supervisor's phase probe decides whether to actually degrade.
pub(crate) fn report_sync_failure(handle: Option<&SandboxRuntimeHandle>) {
    if let Some(h) = handle {
        h.report_suspected_failure();
    }
}
```

Change `run_sync_task`'s signature to accept the optional handle and call the reporter in the
failure arm (replace lines 35-58):

```rust
pub(crate) async fn run_sync_task(
    agent_dir: PathBuf,
    sbox: right_openshell::sandbox_exec::SandboxExec,
    sandbox_runtime: Option<Arc<SandboxRuntimeHandle>>,
    shutdown: CancellationToken,
) {
    let mut tick = interval(SYNC_INTERVAL);
    tick.tick().await; // consume immediate tick

    loop {
        tokio::select! {
            _ = tick.tick() => {
                tracing::debug!(sandbox = %sbox.sandbox_name(), "sync: starting cycle");

                if let Err(e) = sync_cycle(&agent_dir, &sbox).await {
                    tracing::error!(sandbox = %sbox.sandbox_name(), "sync cycle failed: {e:#}");
                    // Surface the failure to the supervisor so a dead sandbox is
                    // verified + degraded instead of only logged every 5 minutes.
                    report_sync_failure(sandbox_runtime.as_deref());
                }
            }
            _ = shutdown.cancelled() => {
                tracing::info!(sandbox = %sbox.sandbox_name(), "sync task shutting down");
                break;
            }
        }
    }
}
```

In `crates/bot/src/sandbox_supervisor.rs`, update `spawn_sync_task` (lines 447-453) to pass
the handle. The supervisor already holds the `Arc<SandboxRuntimeHandle>` as `handle` in
`run_supervisor`; thread it through. Change `spawn_sync_task`:

```rust
fn spawn_sync_task(
    deps: &SupervisorDeps,
    handle: &Arc<SandboxRuntimeHandle>,
    sandbox: SandboxExec,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(sync::run_sync_task(
        deps.agent_dir.clone(),
        sandbox,
        Some(Arc::clone(handle)),
        deps.shutdown.clone(),
    ))
}
```

Update its one caller in `recovery_step` (line 516) from
`*sync_task = Some(spawn_sync_task(deps, bring_up.sandbox));` to
`*sync_task = Some(spawn_sync_task(deps, handle, bring_up.sandbox));`.

The **initial** sync task is spawned outside the supervisor (the seed
`initial_sync_task` passed into `run_supervisor`). Find its spawn site
(`rg -n "run_sync_task" crates/bot/src/lib.rs` or wherever the initial task is created) and
pass the same `Arc<SandboxRuntimeHandle>` there as the new third argument. If the initial
spawn happens before the handle exists, pass `None` — the periodic task replacing it on first
recovery will carry the handle. Confirm the call site with the rg above and update it to the
4-arg form.

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p bot sync_failure_reporter`
Expected: PASS (2 tests).

- [ ] **Step 5: Build the bot crate**

Run: `devenv shell -- cargo build -p bot`
Expected: builds clean. Fix any remaining `run_sync_task(` call sites the rg surfaced to the
4-arg form.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/sync.rs crates/bot/src/sandbox_supervisor.rs
git commit -m "fix(sync): report sandbox failure to supervisor on sync-cycle failure"
```

---

## Task 5: Error-phase recovery — retry-and-auto-resume (Gap 3, resolved)

The Gap-3 decision is **already made** (see Background): OpenShell exposes no restart/resume
(CLI + gRPC confirmed) and Right must not touch the container runtime directly. **There is no
recovery action to add.** After Tasks 3-4 the supervisor detects Error, degrades into
`Unavailable { SandboxError }`, aborts the sync task, and notifies affected chats. The existing
`recovery_step` then loops `bring_up_sandbox` with backoff; because bring-up **reuses** the
existing sandbox, it auto-resumes the instant the sandbox returns to Ready (graceful future
OpenShell upgrade, or a manual operator restart). This is the maximal self-heal achievable
without Right reviving the pod itself.

This task is **verification-only**: confirm the loop keeps retrying (never `Break`s on a
`SandboxError` degrade) and that the copy is honest. Add no new mechanism, no
container-runtime calls, no recreation.

- [ ] **Step 1: Confirm the retry loop self-heals (does not hard-Break on Error)**

Read `recovery_step` (lines 505-540): `Ok(Err(diag))` → `set_unavailable` + backoff (keep
retrying); only a hard `Err(e)` returns `LoopStep::Break`. Confirm `bring_up_sandbox` returns
`Ok(Err(_))` (not `Err(_)`) for an Error/NotFound phase, so the supervisor retries
indefinitely and reconnects automatically once the sandbox is Ready. If a code change is
needed to keep an `Error`-phase bring-up on the recoverable (`Ok(Err)`) path, make it; add a
test that an Error-phase bring-up keeps the handle `Unavailable` and does **not** Break.

- [ ] **Step 2: Confirm the operator copy is honest**

The `GatewayCause::SandboxError` copy from Task 2 must say recovery is automatic once the
sandbox returns, and that a persistent outage needs the sandbox restarted in OpenShell (data
preserved) — never imply Right will revive the container itself. Verify the
`diagnosis_tests.rs` assertion matches.

- [ ] **Step 3: No separate commit unless Step 1 required a code change**

If Step 1 changed code, commit it; otherwise this task folds into Task 3's verification and
adds nothing. **Never** add a `docker`/`podman` call or a sandbox recreation path here.

---

## Out of scope (explicit) / future tracks

- **Right never touches Docker/podman directly.** OpenShell is the sole abstraction over the
  container runtime. `docker start` recovers an upgrade-killed sandbox (validated, data intact)
  but is deliberately NOT used — reviving a killed pod is OpenShell's responsibility (a future
  graceful upgrade or an upstream restart RPC) or a manual operator step. Do not add a
  container-runtime dependency to Right under any task.
- **Durability of `/sandbox` (separate spec, not this plan).** `/sandbox` lives in the
  container's writable layer, not a volume, so `docker rm`/`prune`/disk-cleanup = permanent
  data loss, which neither detection nor retry can help. Mitigations to decide in their own
  spec: (a) upstream — get OpenShell's container driver to mount `/sandbox` on a named volume;
  (b) Right-side scheduled `right agent backup --sandbox-only` as a data-loss safety net. Out
  of scope here; flag as a follow-up.
- **No proactive phase-poll timer.** keepalive already runs periodically; once it degrades on
  Error (Gaps 1-2), it *is* the periodic heartbeat that catches an idle agent's dead sandbox.
  A separate poll loop would be redundant — do not add one.

## Task 6: Update architecture docs (cite-on-touch)

This change touches the sandbox-supervisor subsystem and the gateway-diagnosis contract, so
the satellite doc must be updated (ARCHITECTURE "Cite-on-touch (mandatory)").

**Files:**
- Modify: `docs/architecture/sandbox.md` (or the supervisor-owning satellite — confirm with
  `rg -l "monitor_step|SandboxSupervisor|probe_reachable" docs/architecture/`)
- Modify: `ARCHITECTURE.md` ONLY if a load-bearing rule changed (e.g. "the supervisor MUST
  verify via sandbox phase, not gateway reachability"). Keep it ≤3 sentences; respect the 40k
  budget — cut elsewhere if needed.

- [ ] **Step 1: Locate the supervisor satellite doc**

Run: `rg -l "SandboxSupervisor|sandbox_supervisor|probe_reachable|monitor_step" docs/architecture/`
Expected: identifies the file (likely `docs/architecture/sandbox.md` or `lifecycle.md`).

- [ ] **Step 2: Document the phase-aware probe + sync-task reporter**

Update the located satellite to state: the supervisor verifies a reported failure by reading
the **sandbox phase** (`sandbox_phase_status`), degrades on `SANDBOX_PHASE_ERROR` with a
`SandboxError` diagnosis, and the periodic sync task reports failures via
`report_suspected_failure` (never writes health directly). Record the Gap-3 decision outcome
from Task 5.

- [ ] **Step 3: Add the one-line rule to ARCHITECTURE.md only if load-bearing**

If you keep the rule, add at most: "The sandbox supervisor MUST verify a reported failure by
reading the real sandbox phase over gRPC (`sandbox_phase_status`), not by gateway
reachability; only the supervisor writes `SandboxRuntimeHandle` health." Place it near the
"Sandboxed CC fails closed" / "Self-healing platform" sections.

- [ ] **Step 4: Commit**

```bash
git add docs/architecture ARCHITECTURE.md
git commit -m "docs(architecture): phase-aware sandbox supervisor recovery"
```

---

## Task 7: Final full-workspace verification

Per AGENTS.md verification cadence, run the full suite exactly once at the end. Do NOT run
full-workspace tests after every task.

- [ ] **Step 1: Run the full workspace test suite**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Known-flaky tests (cc/invocation pid race, dashboard warn-count) may flake
under parallel load — re-run those isolated before blaming this change (see global memory
"Flaky tests under parallel load").

- [ ] **Step 2: Final debug build**

Run: `devenv shell -- cargo build --workspace`
Expected: builds clean.

- [ ] **Step 3: Final commit (if any doc/test tidy-ups remain)**

```bash
git add -A
git commit -m "test: full workspace verification for phase-aware sandbox recovery"
```

---

## Self-review notes

- **Spec coverage:** Gap 1 → Task 3; Gap 2 → Task 4; Gap 3 → Task 5 (resolved:
  retry-and-auto-resume, no recovery action, no container-runtime dependency);
  diagnosis/copy → Task 2; out-of-scope/future tracks (durability, no docker side-channel,
  no poll timer) → "Out of scope" section; docs → Task 6; final verification → Task 7.
- **Load-bearing invariant:** Right never calls `docker`/`podman` or a container-runtime API;
  OpenShell is the sole abstraction. Reviving a killed pod is OpenShell's job; Right detects,
  degrades honestly, and auto-resumes via the existing reuse+retry loop.
- **Type consistency:** `SandboxPhaseStatus` (Task 1) is consumed by `probe_phase` (Task 3);
  `ProbeOutcome`/`degrade_decision` names are used identically in the test (Task 3 Step 1) and
  impl (Step 3); `GatewayCause::SandboxError { sandbox: String }` is constructed identically in
  Tasks 2, 3, and tests; `report_sync_failure(Option<&SandboxRuntimeHandle>)` signature matches
  between sync.rs and its tests; `run_sync_task` is updated to 4 args at every call site.
- **Conventions encoded:** TDD red→green per task; targeted `-p` tests midstream, one
  `--workspace` at the end (Task 7); FAIL FAST + `{:#}` anyhow/miette formatting; gRPC-only
  (no CLI) for the phase query; supervisor-only health writes preserved; `!Send` LocalSet shape
  preserved; live-sandbox tests (if any in Task 5 Option A) use `TestSandbox`, not `#[ignore]`.
- **No sandbox recreation** anywhere; Gap 3 explicitly forbids it.
