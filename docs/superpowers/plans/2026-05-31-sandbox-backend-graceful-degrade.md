# Sandbox-backend Graceful Degrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the OpenShell sandbox backend is unreachable, the bot stays up, replies with a cause-specific fix, and auto-recovers — instead of restart-looping with a bare `transport error`.

**Architecture:** A pure cause-diagnosis layer in `right-openshell` classifies backend failures into actionable `GatewayDiagnosis`. In `right-bot`, a single `SandboxSupervisor` task owns sandbox lifecycle and writes a lock-free `SandboxRuntimeHandle` (health + `SandboxExec` + affected-chat set). The message worker reads health and **fails closed** (never runs sandboxed CC on the host). On degrade the supervisor retries bring-up with backoff and notifies affected chats when the backend returns.

**Tech Stack:** Rust 2024, tonic gRPC, `arc_swap::ArcSwap`, tokio, teloxide, miette/thiserror.

**Spec:** `docs/superpowers/specs/2026-05-31-gateway-unreachable-graceful-degrade-design.md`

**Worktree:** Create one before starting (see Task 0). Baseline-test, then implement task-by-task. Targeted tests during; full `cargo test --workspace` at the end (mandatory).

**Command prefix:** All cargo commands run as `devenv shell -- cargo …`.

---

## Task 0: Worktree + baseline

**Files:** none (setup)

- [ ] **Step 1: Create the worktree**

Run:
```bash
git worktree add .worktrees/sandbox-degrade -b feat/sandbox-graceful-degrade
cd .worktrees/sandbox-degrade
```

- [ ] **Step 2: Baseline the two crates we'll touch**

Run: `devenv shell -- cargo test -p right-openshell -p right-bot 2>&1 | tail -30`
Expected: PASS (record any pre-existing failures verbatim — they are not ours to fix).

---

## Task 1: `GatewayCause` + `GatewayDiagnosis` + pure classifier (`right-openshell`)

**Files:**
- Create: `crates/right-openshell/src/diagnosis.rs`
- Create: `crates/right-openshell/src/diagnosis_tests.rs`
- Modify: `crates/right-openshell/src/lib.rs` (add `pub mod diagnosis;`)

- [ ] **Step 1: Write the failing tests**

Create `crates/right-openshell/src/diagnosis_tests.rs`:

```rust
use super::diagnosis::{GatewayCause, classify_doctor_output};

const DOCTOR_DOCKER_FAILED: &str = "Checking system prerequisites...\n\n  Docker ............. FAILED\n\nError:   × docker info failed: Cannot connect to the Docker daemon";
const DOCTOR_OK: &str = "Checking system prerequisites...\n\n  Docker ............. OK\n";
const STATUS_REFUSED: &str =
    "Server Status\n\n  Gateway: openshell\n  Server: https://127.0.0.1:17670\nError:   × client error (Connect)\n  ╰─▶ Connection refused (os error 61)";
const STATUS_OK: &str = "Server Status\n\n  Gateway: openshell\n  Server: https://127.0.0.1:17670\n  Version: 0.0.50";

#[test]
fn docker_failed_classifies_as_docker_down() {
    assert_eq!(
        classify_doctor_output(DOCTOR_DOCKER_FAILED, STATUS_REFUSED),
        GatewayCause::DockerDown
    );
}

#[test]
fn docker_ok_but_server_refused_classifies_as_gateway_not_started() {
    assert_eq!(
        classify_doctor_output(DOCTOR_OK, STATUS_REFUSED),
        GatewayCause::GatewayNotStarted
    );
}

#[test]
fn docker_ok_and_server_ok_classifies_as_unreachable() {
    // Connect failed yet probes look healthy — race or transient; generic advice.
    assert_eq!(
        classify_doctor_output(DOCTOR_OK, STATUS_OK),
        GatewayCause::Unreachable
    );
}

#[test]
fn empty_probe_output_classifies_as_unreachable() {
    assert_eq!(classify_doctor_output("", ""), GatewayCause::Unreachable);
}

#[test]
fn docker_down_diagnosis_summary_and_fixes_are_actionable() {
    let d = GatewayCause::DockerDown.diagnose();
    assert!(d.summary.to_lowercase().contains("docker"));
    assert!(!d.fixes.is_empty());
    assert!(d.fixes[0].to_lowercase().contains("docker"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell diagnosis 2>&1 | tail -20`
Expected: FAIL — `module diagnosis` / `classify_doctor_output` not found.

- [ ] **Step 3: Implement the module**

Create `crates/right-openshell/src/diagnosis.rs`:

```rust
//! Cause-specific diagnosis for sandbox-backend (OpenShell gateway) failures.
//!
//! The connect error itself is opaque (`transport error`); the cause is found
//! by probing `openshell doctor check` + `openshell status`. Classification is
//! a pure function over captured CLI text so it is unit-testable.

use std::path::PathBuf;

/// Why the sandbox backend is unusable. Every variant is operator-fixable
/// without recreating the sandbox or restarting the bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayCause {
    NotInstalled,
    DockerDown,
    /// Docker is up but the gateway is not listening.
    GatewayNotStarted,
    BrokenCerts(PathBuf),
    VersionTooOld { found: String, min: String },
    SandboxNotFound { sandbox: String },
    /// Connect failed but probes are inconclusive (race/transient).
    Unreachable,
}

/// A human-facing, cause-specific diagnosis: one consequence-first summary
/// plus ordered, actionable fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayDiagnosis {
    pub cause: GatewayCause,
    pub summary: String,
    pub fixes: Vec<String>,
}

impl GatewayCause {
    /// Build the operator-facing diagnosis for this cause.
    pub fn diagnose(self) -> GatewayDiagnosis {
        let (summary, fixes): (&str, Vec<&str>) = match &self {
            GatewayCause::NotInstalled => (
                "the sandbox backend isn't installed",
                vec!["Install OpenShell from https://github.com/NVIDIA/OpenShell"],
            ),
            GatewayCause::DockerDown => (
                "Docker isn't running",
                vec!["Start Docker, then I'll reconnect automatically"],
            ),
            GatewayCause::GatewayNotStarted => (
                "the sandbox gateway isn't running",
                vec!["Run: openshell gateway start"],
            ),
            GatewayCause::BrokenCerts(_) => (
                "the sandbox gateway is missing its security certificates",
                vec!["Run: openshell gateway destroy && openshell gateway start"],
            ),
            GatewayCause::VersionTooOld { .. } => (
                "the installed OpenShell is too old",
                vec!["Upgrade OpenShell, then restart the gateway"],
            ),
            GatewayCause::SandboxNotFound { .. } => (
                "my sandbox doesn't exist yet",
                vec!["Create it on the host with: right init"],
            ),
            GatewayCause::Unreachable => (
                "I can't reach the sandbox backend",
                vec![
                    "Check Docker is running",
                    "Run: openshell gateway start",
                    "Check OpenShell version (openshell --version)",
                ],
            ),
        };
        // Enrich messages that carry data.
        let summary = match &self {
            GatewayCause::VersionTooOld { found, min } => {
                format!("the installed OpenShell ({found}) is older than the required {min}")
            }
            GatewayCause::SandboxNotFound { sandbox } => {
                format!("my sandbox '{sandbox}' doesn't exist yet")
            }
            _ => summary.to_owned(),
        };
        GatewayDiagnosis {
            cause: self,
            summary,
            fixes: fixes.into_iter().map(str::to_owned).collect(),
        }
    }
}

/// Classify a connect failure from `openshell doctor check` + `openshell status`
/// output. Pure: no I/O. Brittle CLI-wording lives here and nowhere else.
pub fn classify_doctor_output(doctor: &str, status: &str) -> GatewayCause {
    let doctor_l = doctor.to_lowercase();
    if doctor_l.contains("docker") && doctor_l.contains("failed") {
        return GatewayCause::DockerDown;
    }
    // Docker OK but the gateway server refused the connection.
    let status_l = status.to_lowercase();
    if status_l.contains("connection refused") || status_l.contains("connect)") {
        return GatewayCause::GatewayNotStarted;
    }
    GatewayCause::Unreachable
}

#[cfg(test)]
#[path = "diagnosis_tests.rs"]
mod tests;
```

Add to `crates/right-openshell/src/lib.rs` (with the other `pub mod` lines):

```rust
pub mod diagnosis;
```

- [ ] **Step 4: Run to verify it passes**

Run: `devenv shell -- cargo test -p right-openshell diagnosis 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/diagnosis.rs crates/right-openshell/src/diagnosis_tests.rs crates/right-openshell/src/lib.rs
git commit -m "feat(openshell): cause-specific gateway diagnosis (pure classifier)"
```

---

## Task 2: `diagnose_gateway()` — async probe wrapper (`right-openshell`)

**Files:**
- Modify: `crates/right-openshell/src/diagnosis.rs`

- [ ] **Step 1: Add the async probe (no new unit test — it shells the CLI; classification is already covered)**

Append to `crates/right-openshell/src/diagnosis.rs`:

```rust
/// Diagnose a *connect* failure by probing the backend with
/// `openshell doctor check` and `openshell status`. Falls back to
/// `Unreachable` if the CLI cannot be run. Never returns an error — a
/// diagnosis is always producible.
pub async fn diagnose_gateway() -> GatewayDiagnosis {
    async fn run(args: &[&str]) -> String {
        match tokio::process::Command::new("openshell")
            .args(args)
            .env("NO_COLOR", "1")
            .output()
            .await
        {
            Ok(o) => {
                let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
                s.push('\n');
                s.push_str(&String::from_utf8_lossy(&o.stderr));
                s
            }
            Err(_) => String::new(),
        }
    }
    let doctor = run(&["doctor", "check"]).await;
    let status = run(&["status"]).await;
    classify_doctor_output(&doctor, &status).diagnose()
}
```

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p right-openshell 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/right-openshell/src/diagnosis.rs
git commit -m "feat(openshell): diagnose_gateway() probes doctor/status on connect failure"
```

---

## Task 3: `SandboxHealth` + `SandboxRuntimeHandle` (`right-bot`)

**Files:**
- Create: `crates/bot/src/sandbox_runtime.rs`
- Create: `crates/bot/src/sandbox_runtime_tests.rs`
- Modify: `crates/bot/src/lib.rs` (add `pub mod sandbox_runtime;`)

- [ ] **Step 1: Write the failing tests**

Create `crates/bot/src/sandbox_runtime_tests.rs`:

```rust
use super::sandbox_runtime::{SandboxHealth, SandboxRuntimeHandle};
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo test -p right-bot sandbox_runtime 2>&1 | tail -20`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the module**

Create `crates/bot/src/sandbox_runtime.rs`:

```rust
//! Lock-free shared sandbox-backend state. The `SandboxSupervisor` is the
//! sole writer; every other consumer (message worker, dashboard) only reads.

use arc_swap::ArcSwap;
use right_openshell::diagnosis::GatewayDiagnosis;
use right_openshell::sandbox_exec::SandboxExec;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use teloxide::types::ChatId;
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

    pub fn current_sandbox(&self) -> Option<SandboxExec> {
        Option::clone(&self.sandbox.load())
    }

    /// Record a chat that saw an "unavailable" reply, for the back-online notice.
    pub fn note_affected(&self, chat: ChatId, eff_thread_id: i64) {
        self.affected
            .lock()
            .expect("affected mutex poisoned")
            .insert((chat.0, eff_thread_id));
    }

    /// Ask the supervisor to verify the backend. Coalesced; never blocks.
    pub fn report_suspected_failure(&self) {
        let _ = self.failure_tx.try_send(());
    }

    // ---- writes (supervisor only) ----

    pub(crate) fn set_ready(&self, sandbox: SandboxExec) {
        self.sandbox.store(Arc::new(Some(sandbox)));
        self.health.store(Arc::new(SandboxHealth::Ready));
    }

    pub(crate) fn set_unavailable(&self, diagnosis: Arc<GatewayDiagnosis>) {
        self.sandbox.store(Arc::new(None));
        self.health
            .store(Arc::new(SandboxHealth::Unavailable { diagnosis }));
    }

    pub(crate) fn take_affected(&self) -> Vec<(ChatId, i64)> {
        std::mem::take(&mut *self.affected.lock().expect("affected mutex poisoned"))
            .into_iter()
            .map(|(c, t)| (ChatId(c), t))
            .collect()
    }
}

#[cfg(test)]
#[path = "sandbox_runtime_tests.rs"]
mod tests;
```

Add to `crates/bot/src/lib.rs` (near the other `pub mod`/`mod` declarations):

```rust
pub mod sandbox_runtime;
```

> Note: confirm `right_openshell::sandbox_exec::SandboxExec` is the correct path and that `SandboxExec` derives/implements `Clone` (it is cloned at `lib.rs:910`+ today, so it does). If `Clone` is missing, wrap in `Arc<SandboxExec>` inside the `ArcSwap` instead.

- [ ] **Step 4: Run to verify it passes**

Run: `devenv shell -- cargo test -p right-bot sandbox_runtime 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/sandbox_runtime.rs crates/bot/src/sandbox_runtime_tests.rs crates/bot/src/lib.rs
git commit -m "feat(bot): SandboxRuntimeHandle — lock-free sandbox health/exec/affected state"
```

---

## Task 4: Fail-closed gate decision (pure) (`right-bot`)

**Files:**
- Modify: `crates/bot/src/sandbox_runtime.rs`
- Modify: `crates/bot/src/sandbox_runtime_tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/bot/src/sandbox_runtime_tests.rs`:

```rust
use super::sandbox_runtime::{GateDecision, sandbox_gate};

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
        GateDecision::Proceed => panic!("FAIL-CLOSED VIOLATION: sandboxed agent must not run on host"),
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo test -p right-bot sandbox_gate 2>&1 | tail -20`
Expected: FAIL — `GateDecision` / `sandbox_gate` not found.

- [ ] **Step 3: Implement**

Append to `crates/bot/src/sandbox_runtime.rs` (above the `#[cfg(test)]` block):

```rust
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
```

- [ ] **Step 4: Run to verify it passes**

Run: `devenv shell -- cargo test -p right-bot sandbox 2>&1 | tail -20`
Expected: PASS (all Task 3 + Task 4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/sandbox_runtime.rs crates/bot/src/sandbox_runtime_tests.rs
git commit -m "feat(bot): fail-closed sandbox gate decision"
```

---

## Task 5: Diagnosis → Telegram copy (`right-bot`)

**Files:**
- Create: `crates/bot/src/sandbox_copy.rs`
- Create: `crates/bot/src/sandbox_copy_tests.rs`
- Modify: `crates/bot/src/lib.rs` (add `mod sandbox_copy;`)

- [ ] **Step 1: Write the failing tests**

Create `crates/bot/src/sandbox_copy_tests.rs`:

```rust
use super::sandbox_copy::{back_online_message, unavailable_message};
use right_openshell::diagnosis::GatewayCause;

#[test]
fn unavailable_message_leads_with_consequence_and_includes_fix() {
    let d = GatewayCause::DockerDown.diagnose();
    let msg = unavailable_message(&d);
    assert!(msg.starts_with("⚠️"));
    assert!(msg.to_lowercase().contains("offline"));
    assert!(msg.to_lowercase().contains("docker"));
    // No raw CLI-style prefixes.
    assert!(!msg.contains("Failed:"));
    assert!(!msg.contains("Error:"));
}

#[test]
fn unavailable_message_html_escapes_dynamic_text() {
    let d = GatewayCause::SandboxNotFound {
        sandbox: "a<b>&c".to_owned(),
    }
    .diagnose();
    let msg = unavailable_message(&d);
    assert!(msg.contains("a&lt;b&gt;&amp;c"));
    assert!(!msg.contains("a<b>"));
}

#[test]
fn back_online_message_is_positive_and_short() {
    let msg = back_online_message();
    assert!(msg.starts_with("✅"));
    assert!(msg.to_lowercase().contains("online"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo test -p right-bot sandbox_copy 2>&1 | tail -20`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement**

Create `crates/bot/src/sandbox_copy.rs`:

```rust
//! Telegram-facing copy for sandbox-backend outages. Consequence-first,
//! HTML-escaped for `ParseMode::Html`, no raw CLI prefixes.

use crate::cc::markdown_utils::html_escape;
use right_openshell::diagnosis::GatewayDiagnosis;

/// Message shown when a sandboxed turn is blocked by an unavailable backend.
pub fn unavailable_message(d: &GatewayDiagnosis) -> String {
    let summary = html_escape(&d.summary);
    let fix = d.fixes.first().map(|f| html_escape(f)).unwrap_or_default();
    format!(
        "⚠️ I can't run right now — my secure sandbox backend is offline.\n\
         Likely cause: {summary}.\n\
         Fix: {fix}."
    )
}

/// Sent once per affected chat when the backend recovers.
pub fn back_online_message() -> String {
    "✅ Sandbox back online — I'm ready.".to_owned()
}
```

Add to `crates/bot/src/lib.rs`:

```rust
mod sandbox_copy;
```

> Note: confirm `crate::cc::markdown_utils::html_escape` is public to the crate (it is `use`d at `worker.rs:23`). If not, use the same escaping helper that path resolves to.

- [ ] **Step 4: Run to verify it passes**

Run: `devenv shell -- cargo test -p right-bot sandbox_copy 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/sandbox_copy.rs crates/bot/src/sandbox_copy_tests.rs crates/bot/src/lib.rs
git commit -m "feat(bot): sandbox outage / back-online Telegram copy"
```

---

## Task 6: Extract `bring_up_sandbox()` from `lib.rs`

This is a **relocation refactor**, not new logic. The sandbox-init sequence at `crates/bot/src/lib.rs:666–908` moves into one function that returns `Result<SandboxExec, GatewayDiagnosis>` instead of crashing on connect failure.

**Files:**
- Create: `crates/bot/src/sandbox_supervisor.rs`
- Modify: `crates/bot/src/lib.rs` (extract the block; add `mod sandbox_supervisor;`)

- [ ] **Step 1: Define the extraction contract (no behavior change yet)**

Create `crates/bot/src/sandbox_supervisor.rs` with the function shell and the failure mapping. Move the body of the `is_sandboxed` block (`lib.rs:676–876`, ending just before the `(Some(config_path), Some((mtls_dir, sandbox_id)))` tuple) into it. Capture the variables it reads as parameters:

```rust
//! Owns the sandbox-backend lifecycle: first bring-up, degrade, recovery.

use crate::sandbox_runtime::{SandboxHealth, SandboxRuntimeHandle};
use right_openshell::diagnosis::{GatewayCause, GatewayDiagnosis};
use right_openshell::sandbox_exec::SandboxExec;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Inputs the bring-up sequence needs. Borrowed from the startup scope.
pub struct BringUpCtx<'a> {
    pub agent: &'a str,
    pub home: &'a Path,
    pub agent_dir: &'a Path,
    pub resolved_sandbox: &'a str,
    pub policy_path: &'a Path,
    pub network_policy: right_codegen::policy::NetworkPolicy, // match the real type at lib.rs:744
    pub config: &'a right_agent::agent::types::AgentConfig,   // match the real config type
}

/// Result of one bring-up attempt: the live SandboxExec, or a diagnosis to
/// degrade on. Genuine non-self-healing config errors are returned as `Err`
/// of the outer `miette` result (hard-fail), not as a diagnosis.
pub async fn bring_up_sandbox(
    ctx: &BringUpCtx<'_>,
) -> miette::Result<Result<SandboxExec, GatewayDiagnosis>> {
    // 1) preflight_check(): NotInstalled/NoGateway/BrokenGateway -> Ok(Err(diagnosis))
    // 2) connect_grpc(): on Err -> Ok(Err(diagnose_gateway().await))
    // 3) openshell_preflight(): version too old -> Ok(Err(GatewayCause::VersionTooOld{..}.diagnose()))
    // 4) is_sandbox_ready()==false -> Ok(Err(GatewayCause::SandboxNotFound{ sandbox }.diagnose()))
    // 5) resolve_sandbox_id / resolve_host_ips / policy drift+apply / generate_ssh_config /
    //    clean_stale_control_master / provider reconcile / initial_sync / reverse_sync_md
    //    -- relocated verbatim from lib.rs:731–876 + 891–925.
    //    Filesystem-drift detected (lib.rs:787) stays a hard miette::Err (cannot self-heal).
    // 6) build SandboxExec and return Ok(Ok(sandbox)).
    todo!("relocate lib.rs:676..908 here, mapping the listed steps")
}
```

Map each existing early-return to the new contract:
- `OpenShellStatus::NotInstalled` → `Ok(Err(GatewayCause::NotInstalled.diagnose()))`
- `OpenShellStatus::NoGateway(_)` → `Ok(Err(GatewayCause::GatewayNotStarted.diagnose()))`
- `OpenShellStatus::BrokenGateway(dir)` → `Ok(Err(GatewayCause::BrokenCerts(dir).diagnose()))`
- `connect_grpc(..).await` `Err` (was `?` at `lib.rs:703`) → `Ok(Err(right_openshell::diagnosis::diagnose_gateway().await))`
- `openshell_preflight` `Err` (was hard-fail at `lib.rs:708`) → `Ok(Err(GatewayCause::VersionTooOld{ found, min }.diagnose()))` (extract `found`/`min` from the preflight error; if not available, use `GatewayCause::Unreachable.diagnose()` and add a `// TODO follow-up: thread version strings`).
- `!sandbox_exists` (was hard-fail at `lib.rs:717`) → `Ok(Err(GatewayCause::SandboxNotFound{ sandbox: resolved_sandbox.to_owned() }.diagnose()))`
- Filesystem **drift** (`lib.rs:787`) → keep as `Err(miette::miette!(...))` (hard-fail; cannot self-heal without migration).

> The `initial_sync` + `run_sync_task` spawn currently live *after* the block (`lib.rs:904+`). Move `initial_sync` + `reverse_sync_md` into `bring_up_sandbox` (they must complete before `Ready`). The `run_sync_task` spawn is owned by the supervisor in Task 7 — return the constructed `SandboxExec` so the supervisor can spawn it.

- [ ] **Step 2: Compile-check the signature against real types**

Run: `devenv shell -- cargo check -p right-bot 2>&1 | tail -30`
Expected: errors only inside the `todo!()` body. Fix `BringUpCtx` field types to match the real `network_policy` (`config.network_policy` at `lib.rs:744`) and config type until the struct compiles.

- [ ] **Step 3: Relocate the body**

Replace `todo!()` with the relocated code per the step-1 mapping. Add `mod sandbox_supervisor;` to `lib.rs`. In `lib.rs`, the `is_sandboxed` block becomes a call (wired fully in Task 8); for now keep `lib.rs` compiling by calling `bring_up_sandbox` and `?`-propagating + `.expect`-ing the inner `Ok` to preserve current behavior temporarily:

```rust
let sandbox_exec = match sandbox_supervisor::bring_up_sandbox(&bring_up_ctx).await? {
    Ok(sb) => sb,
    Err(diag) => {
        // Temporary: Task 8 replaces this with degrade. Keep old crash semantics for now.
        return Err(miette::miette!("{}", diag.summary));
    }
};
```

- [ ] **Step 4: Verify the crate builds and existing tests pass**

Run: `devenv shell -- cargo test -p right-bot 2>&1 | tail -30`
Expected: PASS (no behavior change vs baseline).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/sandbox_supervisor.rs crates/bot/src/lib.rs
git commit -m "refactor(bot): extract bring_up_sandbox() returning diagnosis on backend failure"
```

---

## Task 7: `SandboxSupervisor` task — recovery + monitor + back-online

**Files:**
- Modify: `crates/bot/src/sandbox_supervisor.rs`

- [ ] **Step 1: Implement the supervisor loop**

Append to `crates/bot/src/sandbox_supervisor.rs`:

```rust
use tokio::sync::mpsc;

const RECOVERY_BACKOFF: &[u64] = &[5, 10, 15, 15, 30]; // seconds; last value repeats

/// Owns sandbox lifecycle. Runs for the bot's life. When `Unavailable`, retries
/// `bring_up_sandbox` with backoff. When `Ready`, sleeps until a verified
/// failure report flips it back. On every Unavailable→Ready transition, spawns
/// the sync task and notifies affected chats.
#[allow(clippy::too_many_arguments)]
pub fn spawn_supervisor(
    handle: Arc<SandboxRuntimeHandle>,
    mut failure_rx: mpsc::Receiver<()>,
    bot: crate::telegram::BotType,
    // owned clones of everything BringUpCtx borrows (agent, home, agent_dir,
    // resolved_sandbox, policy_path, network_policy, config), plus shutdown.
    deps: SupervisorDeps,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut sync_task: Option<tokio::task::JoinHandle<()>> = None;
        loop {
            let ready = handle.is_ready();
            if ready {
                // Monitor: wait for a failure report or shutdown.
                tokio::select! {
                    _ = deps.shutdown.cancelled() => break,
                    msg = failure_rx.recv() => {
                        if msg.is_none() { break; }
                        // Verify with a single direct probe before degrading.
                        if probe_reachable(&deps).await { continue; }
                        let diag = right_openshell::diagnosis::diagnose_gateway().await;
                        tracing::error!(agent = %deps.agent, cause = ?diag.cause, "{}", diag.summary);
                        handle.set_unavailable(Arc::new(diag));
                        if let Some(t) = sync_task.take() { t.abort(); }
                    }
                }
            } else {
                // Recovery: retry bring-up with backoff.
                let ctx = deps.bring_up_ctx();
                match bring_up_sandbox(&ctx).await {
                    Ok(Ok(sandbox)) => {
                        handle.set_ready(sandbox.clone());
                        sync_task = Some(spawn_sync_task(&deps, sandbox));
                        notify_back_online(&handle, &bot).await;
                        tracing::info!(agent = %deps.agent, "sandbox backend recovered");
                    }
                    Ok(Err(diag)) => {
                        handle.set_unavailable(Arc::new(diag));
                    }
                    Err(e) => {
                        tracing::error!(agent = %deps.agent, "unrecoverable sandbox error: {e:#}");
                        // Hard config error during recovery: keep degraded, stop retrying.
                        break;
                    }
                }
                let attempt = deps.attempt_and_increment();
                let secs = RECOVERY_BACKOFF[attempt.min(RECOVERY_BACKOFF.len() - 1)];
                tokio::select! {
                    _ = deps.shutdown.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => {}
                }
            }
        }
    })
}

async fn notify_back_online(handle: &Arc<SandboxRuntimeHandle>, bot: &crate::telegram::BotType) {
    for (chat, thread) in handle.take_affected() {
        let _ = crate::telegram::worker::send_tg(
            bot,
            chat,
            thread,
            &crate::sandbox_copy::back_online_message(),
        )
        .await;
    }
}
```

> Implement the small helpers `SupervisorDeps` (owned clones + `bring_up_ctx()` + `attempt_and_increment()` counter), `probe_reachable(&deps)` (a `connect_grpc` + cheap RPC, `true` on success), and `spawn_sync_task(&deps, sandbox)` (relocate the `run_sync_task` spawn from `lib.rs:925`). Make `send_tg` `pub(crate)` (it already is, `worker.rs:2235`).

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p right-bot 2>&1 | tail -30`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/sandbox_supervisor.rs
git commit -m "feat(bot): SandboxSupervisor — recovery backoff, monitor, back-online notice"
```

---

## Task 8: Wire startup degrade + thread the handle

**Files:**
- Modify: `crates/bot/src/lib.rs` (degrade instead of crash; spawn supervisor; pass handle down)
- Modify: `crates/bot/src/telegram/dispatch.rs:209` and `:681` (add `sandbox_runtime` to `AgentSettings`)
- Modify: `crates/bot/src/telegram/handler.rs:74` (add field to `AgentSettings`)
- Modify: `crates/bot/src/telegram/dashboard.rs` (`DashboardState.sandbox_exec` → live read from handle)

- [ ] **Step 1: Add the field to `AgentSettings`**

In `crates/bot/src/telegram/handler.rs` `struct AgentSettings` (after `resolved_sandbox`):

```rust
    /// Shared sandbox-backend health/exec state. Read before every sandboxed turn.
    pub sandbox_runtime: std::sync::Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
```

- [ ] **Step 2: Construct the handle at startup and degrade**

In `crates/bot/src/lib.rs`, replace the temporary Task-6 crash block with:

```rust
let initial_health = match sandbox_supervisor::bring_up_sandbox(&bring_up_ctx).await? {
    Ok(sandbox) => crate::sandbox_runtime::SandboxHealth::Ready_with(sandbox), // see note
    Err(diag) => {
        tracing::error!(agent = %args.agent, cause = ?diag.cause,
            fixes = ?diag.fixes, "sandbox backend unavailable — starting degraded: {}", diag.summary);
        crate::sandbox_runtime::SandboxHealth::Unavailable { diagnosis: std::sync::Arc::new(diag) }
    }
};
let (sandbox_runtime, failure_rx) = crate::sandbox_runtime::SandboxRuntimeHandle::new(
    matches!(initial_health, crate::sandbox_runtime::SandboxHealth::Ready { .. })
        .then_some(crate::sandbox_runtime::SandboxHealth::Ready)
        .unwrap_or(initial_health),
);
```

> Simpler than `Ready_with`: on the `Ok(sandbox)` branch call `sandbox_runtime.set_ready(sandbox)` *after* constructing the handle, and spawn the sync task there (or always let the supervisor own it by starting the supervisor in monitor mode). Pick one ownership model: **recommended** — always construct the handle as `Unavailable`/`Ready` flag only, then `set_ready(sandbox)` + spawn supervisor; the supervisor's monitor branch owns the sync task uniformly. Keep `health_sandbox_exec`/`sync_handle` derived from `sandbox_runtime.current_sandbox()`.

Then spawn the supervisor (always, for monitor + recovery) and thread `sandbox_runtime` into the `run_telegram` call so it reaches `AgentSettings`.

- [ ] **Step 3: Pass it into both `AgentSettings` constructions**

`crates/bot/src/telegram/dispatch.rs:209` — add `sandbox_runtime,` (plumb the param through `run_telegram`/`build_dispatcher` signatures as needed).
`crates/bot/src/telegram/dispatch.rs:681` (test) — add:

```rust
            sandbox_runtime: {
                let (h, _rx) = crate::sandbox_runtime::SandboxRuntimeHandle::new(
                    crate::sandbox_runtime::SandboxHealth::Ready,
                );
                h
            },
```

- [ ] **Step 4: Dashboard reads live**

In `crates/bot/src/telegram/dashboard.rs`, change `DashboardState.sandbox_exec` to hold the `Arc<SandboxRuntimeHandle>` and resolve `current_sandbox()` per request (replace the startup-captured `dashboard_sandbox_exec` clone at `lib.rs:945`). Keep the field name change minimal; update all readers.

- [ ] **Step 5: Verify the crate builds and tests pass**

Run: `devenv shell -- cargo test -p right-bot 2>&1 | tail -30`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/lib.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dashboard.rs
git commit -m "feat(bot): degrade on sandbox bring-up failure; spawn supervisor; thread handle"
```

---

## Task 9: Wire the fail-closed gate + mid-session report

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (gate before `invoke_cc` at `:1443`; report on gateway-class failure)

- [ ] **Step 1: Gate before invoke_cc**

In `crates/bot/src/telegram/worker.rs`, immediately before the `// Invoke claude -p` block at `:1431`, insert:

```rust
            // Fail-closed sandbox gate: never run a sandboxed turn on the host.
            {
                use crate::sandbox_runtime::{GateDecision, sandbox_gate};
                let is_sandboxed = ctx.settings.resolved_sandbox.is_some();
                if let GateDecision::Reply { diagnosis } =
                    sandbox_gate(is_sandboxed, &ctx.settings.sandbox_runtime.health())
                {
                    ctx.settings
                        .sandbox_runtime
                        .note_affected(tg_chat_id, eff_thread_id);
                    let _ = send_tg(
                        &ctx.bot,
                        tg_chat_id,
                        eff_thread_id,
                        &crate::sandbox_copy::unavailable_message(&diagnosis),
                    )
                    .await;
                    typing_task.await.ok();
                    continue; // skip this batch; do not invoke CC
                }
            }
```

> Confirm `send_tg` is sent with `ParseMode::Html`. `send_tg` (`:2235`) currently does *not* set HTML mode. Add an HTML-aware variant or set parse mode there; the copy contains escaped HTML entities and emoji, so plain mode would show `&lt;`. Cheapest correct fix: a `send_tg_html` sibling that sets `ParseMode::Html`, used here.

- [ ] **Step 2: Report gateway-class failures mid-session**

In the `Err(failure)` arm of `invoke_cc` (`worker.rs:1471`), when the failure indicates the sandbox/SSH was unreachable, call:

```rust
                    ctx.settings.sandbox_runtime.report_suspected_failure();
```

Place it where the failure variant is known (do not over-trigger on ordinary CC errors — only SSH/transport failures). If distinguishing is non-trivial, scope this step to a follow-up and leave a `// TODO` — the startup + recovery path already satisfies the spec's primary goal.

- [ ] **Step 3: Verify**

Run: `devenv shell -- cargo test -p right-bot 2>&1 | tail -30`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): fail-closed sandbox gate before CC; report mid-session backend failure"
```

---

## Task 10: Dashboard doctor card surfaces the diagnosis

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs:871` (the `openshell-gateway` check)

- [ ] **Step 1: Drive the card from live health**

At `dashboard.rs:871`, replace the static `name: "openshell-gateway"` check construction so its status/fix come from `state.sandbox_runtime.health()`:

```rust
                let (ok, fix) = match state.sandbox_runtime.health() {
                    crate::sandbox_runtime::SandboxHealth::Ready => (true, None),
                    crate::sandbox_runtime::SandboxHealth::Unavailable { diagnosis } => {
                        (false, Some(diagnosis.fixes.join("; ")))
                    }
                };
```

Wire `ok`/`fix` into the existing check struct fields (keep the existing field names at `:871–874`).

- [ ] **Step 2: Verify**

Run: `devenv shell -- cargo test -p right-bot dashboard 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/dashboard.rs
git commit -m "feat(dashboard): openshell-gateway card reflects live sandbox diagnosis"
```

---

## Task 11: Docs + final verification

**Files:**
- Modify: `docs/architecture/sandbox.md`
- Modify: `docs/architecture/lifecycle.md`
- Modify: `ARCHITECTURE.md` (one invariant only, if warranted)

- [ ] **Step 1: Update `docs/architecture/sandbox.md`**

Add a "Graceful degrade" subsection: bot-startup bring-up now returns a `GatewayDiagnosis` on operator-fixable failure instead of crashing; the `SandboxSupervisor` owns recovery (backoff) and the back-online notice; mid-session failures route through `report_suspected_failure` + one verified probe.

- [ ] **Step 2: Update `docs/architecture/lifecycle.md`**

In the per-message flow, note the fail-closed sandbox gate before CC invocation.

- [ ] **Step 3: Update `ARCHITECTURE.md` (only if adding an invariant)**

If kept ≤3 sentences, add under Security Model:

> Sandboxed CC runs only when `SandboxHealth == Ready`. Degrade fails closed — a sandboxed agent never falls back to host execution. The `SandboxSupervisor` is the sole writer of sandbox health.

Verify the 40k character budget is not exceeded:
Run: `wc -c ARCHITECTURE.md` (must stay < 40000).

- [ ] **Step 4: Final full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace 2>&1 | tail -40`
Expected: PASS (or only pre-existing baseline failures recorded in Task 0).

- [ ] **Step 5: Clippy on touched crates**

Run: `devenv shell -- cargo clippy -p right-openshell -p right-bot --all-targets 2>&1 | tail -30`
Expected: no new warnings.

- [ ] **Step 6: Commit**

```bash
git add docs/architecture/sandbox.md docs/architecture/lifecycle.md ARCHITECTURE.md
git commit -m "docs(architecture): sandbox graceful-degrade + fail-closed gate invariant"
```

---

## Self-review notes (coverage vs spec)

- Goal 1 (clear log diagnosis): Task 6 step-3 mapping + Task 8 step-2 ERROR log.
- Goal 2 (clear Telegram message): Tasks 5 + 9.
- Goal 3 (no restart-loop; auto-recover): Tasks 6 (degrade, not crash) + 7 (recovery) + 8 (continue boot).
- Goal 4 (back-online notice): Task 7 `notify_back_online` + Task 3 affected-set.
- Uniform degrade across causes: Task 6 mapping (NotInstalled / NoGateway / BrokenCerts / VersionTooOld / SandboxNotFound / connect).
- Fail-closed: Task 4 (pure) + Task 9 (wired) — asserted by `sandboxed_and_unavailable_replies_never_proceeds`.
- Single authoritative owner: Task 7 supervisor is the only writer; `set_*` are `pub(crate)`.

**Known follow-ups flagged inline (not blockers):** version-string threading for `VersionTooOld` (Task 6); precise gateway-class failure detection at the mid-session report site (Task 9 step 2).
```
