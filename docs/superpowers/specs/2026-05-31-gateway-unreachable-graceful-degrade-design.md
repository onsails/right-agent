# Sandbox-backend graceful degrade — design

**Date:** 2026-05-31
**Status:** Design (pending review)
**Scope:** `right-openshell`, `right-bot` (`crates/bot`), dashboard read path

## Problem

When the OpenShell gateway is unreachable, the bot dies at startup and
`process-compose` restart-loops it forever. No operator ever learns why.

Observed failure:

```
INFO  right_bot: registered MCP server ...
Error:   × gRPC connect to https://127.0.0.1:8080 failed: transport error
(exit 1 -> restart -> repeat)
```

Root cause: `right_openshell::openshell::preflight_check()` only checks
that OpenShell is *installed* and that mTLS certs *exist on disk*. It does
**not** verify the gateway is reachable. With the Docker daemon down the
certs still exist, so `preflight_check()` returns `Ready`, and the bot
falls through to `connect_grpc(&mtls_dir).await?` at `crates/bot/src/lib.rs:703`,
which surfaces a bare `transport error` and exits. The most common real
cause is "Docker Desktop is not running" (`openshell doctor check` reports
`Docker ............. FAILED`), but nothing in the bot says so.

## Goals

1. Replace the bare `transport error` with a **clear, cause-specific log
   diagnosis** plus likely fixes.
2. Surface a **clear Telegram message** to the user with the likely fix,
   leading with the user-visible consequence.
3. **Never restart-loop on a sandbox-backend problem again.** The bot stays
   up, degrades, and **auto-recovers** once the operator fixes the cause.
4. Notify affected chats **"sandbox back online"** when recovery completes.

## Non-goals

- Active health polling of the gateway. We react to direct signals
  (a failed gRPC connect), never to heuristics or timers-as-proxy.
- Changing the happy-path startup timing. When bring-up succeeds on the
  first attempt, startup behaves exactly as today (blocks until the sandbox
  is synced before serving Telegram).
- Host-execution fallback for a sandboxed agent. Degrade **fails closed**
  (see Security).

## Decisions (from brainstorming)

- **Startup behavior:** stay up, degrade. Reactive per-message error reply;
  auto-recover.
- **Fix precision:** probe the actual cause (`openshell doctor check` /
  `openshell status`) and tailor the fix.
- **Recovery:** a background supervisor retries the full bring-up with
  backoff; sends "back online" to affected chats on success.
- **Uniform degrade:** *all* operator-fixable sandbox-backend problems
  degrade through the same path — not only connect failures. `NotInstalled`,
  `NoGateway`, `BrokenGateway` (missing certs), version-too-old, and
  sandbox-not-found each become a tailored, recoverable diagnosis. Genuine
  agent.yaml misconfiguration that cannot self-heal (e.g. unresolved
  `sandbox.policy_file`) stays a hard startup error.
- **Reliability over surgical change:** one authoritative owner of sandbox
  lifecycle; lock-free reads everywhere else.

## Architecture

### Component 1 — cause diagnosis (`right-openshell`)

New module `src/diagnosis.rs`:

```rust
pub enum GatewayCause {
    NotInstalled,
    DockerDown,
    GatewayNotStarted,   // Docker up, gateway not listening
    BrokenCerts(PathBuf),
    VersionTooOld { found: String, min: String },
    SandboxNotFound { sandbox: String },
    Unreachable,         // connect failed, cause unclassified
}

pub struct GatewayDiagnosis {
    pub cause: GatewayCause,
    /// One consequence-first sentence.
    pub summary: String,
    /// Ordered, operator-actionable fixes.
    pub fixes: Vec<String>,
}

/// Diagnose a *connect* failure by probing the backend. Async because it
/// shells `openshell doctor check` / `openshell status`. The connect error
/// text is not needed — the probe output identifies the cause directly.
pub async fn diagnose_gateway() -> GatewayDiagnosis;
```

Post-connect failures (`VersionTooOld`, `SandboxNotFound`) are *not* routed
through `diagnose_gateway()` — the bring-up step that failed already knows the
cause and constructs the `GatewayDiagnosis` directly. Only the connect step,
whose raw error is the opaque `transport error`, needs the probe.

The brittle part — turning `openshell doctor check` / `status` text into a
`GatewayCause` — lives in a **pure function** so it is unit-testable from
captured CLI output, mirroring the existing
`gateway_endpoint_from_status_output`:

```rust
pub(crate) fn classify_doctor_output(doctor: &str, status: &str) -> GatewayCause;
```

`fixes()` text is data, also unit-tested per cause.

`preflight_check()` keeps its current `OpenShellStatus` result; its
`NotInstalled` / `NoGateway` / `BrokenGateway` variants now feed
`GatewayDiagnosis` construction instead of producing bespoke `miette` errors
in `lib.rs`.

### Component 2 — `SandboxRuntimeHandle` (`right-bot`)

New module `crates/bot/src/sandbox_runtime.rs`. A single `Arc`-shared handle;
the supervisor is the only writer.

```rust
pub enum SandboxHealth {
    Ready,
    Unavailable { diagnosis: Arc<GatewayDiagnosis> },
}

pub struct SandboxRuntimeHandle {
    /// Hot-path read on every inbound message — lock-free.
    health: ArcSwap<SandboxHealth>,
    /// Present iff health == Ready. Read by dashboard + (defensively) invocation.
    sandbox: ArcSwap<Option<SandboxExec>>,
    /// (chat_id, eff_thread_id) that received an "unavailable" reply this outage.
    /// Drained when recovery sends the back-online notice.
    affected: Mutex<HashSet<(ChatId, i64)>>,
    /// Wakes the supervisor when a path reports a suspected gateway failure.
    failure_tx: mpsc::Sender<()>,
}
```

Read API: `health()`, `current_sandbox()`, `note_affected(chat, thread)`,
`report_suspected_failure()`. Write API (`pub(crate)`, supervisor only):
`set_ready(sandbox)`, `set_unavailable(diagnosis)`, `take_affected()`.

Threaded into:
- `AgentSettings` (built at `dispatch.rs:209` and `dispatch.rs:681`) — read by
  the message worker.
- `DashboardState` — replaces the startup-captured `sandbox_exec: Option<SandboxExec>`
  with a live read from the handle, so the dashboard reflects current state and
  the existing `openshell-gateway` doctor card can show the diagnosis + fixes.

### Component 3 — `SandboxSupervisor` (`right-bot`)

New module `crates/bot/src/sandbox_supervisor.rs`. One task; owns the
authoritative write side of `SandboxRuntimeHandle` and the sync-task handle.

State machine:

```
            startup bring_up()           +-------- report_suspected_failure
                  |                       |         (verified via 1 gRPC probe)
                  v                       v
   +----------> Ready --------------> Unavailable{diagnosis}
   |              |                       |
   |       spawn sync task          cancel sync task
   |       (initial_sync first)     retry bring_up() w/ backoff (~15s, capped)
   |              |                       |
   +---- back-online notice <------------+
         to affected chats
```

`bring_up()` is the **extracted** sandbox-init sequence currently inlined in
`lib.rs` (`connect_grpc` -> `openshell_preflight` -> `is_sandbox_ready` ->
`resolve_sandbox_id` -> `resolve_host_ips` -> policy regen + apply ->
`initial_sync` -> `reverse_sync_md` -> spawn `run_sync_task`). It returns
`Result<SandboxExec, GatewayDiagnosis>`: any operator-fixable failure maps to
a diagnosis; only non-self-healing config errors propagate as hard errors.

**Startup wiring (`lib.rs`):**
- Call `bring_up()` once, synchronously, as today.
  - `Ok(sandbox)` -> `set_ready`, spawn supervisor in monitor mode. Happy path
    timing unchanged (still blocks on `initial_sync`).
  - `Err(diagnosis)` -> **log the diagnosis at ERROR** (replaces the bare
    `transport error`), `set_unavailable(diagnosis)`, spawn supervisor in
    recovery mode, **continue** booting Telegram.
- Hard config errors (unresolved policy path, etc.) still abort startup.

The supervisor never polls a healthy gateway. While `Ready` it only acts on a
`report_suspected_failure()` wake, which it confirms with a single
`connect_grpc` probe before flipping to `Unavailable` (direct signal, not a
timer heuristic).

### Component 4 — message gate (`right-bot` worker) — fail closed

Before dispatching a sandboxed CC turn, the worker reads `handle.health()`:

- `Ready` -> proceed.
- `Unavailable { diagnosis }` -> `note_affected(chat, thread)`, reply via
  `send_tg` with the formatted diagnosis, **skip the CC invocation entirely**.

`ssh_config_path` is a stable generated file path (`generate_ssh_config`,
`lib.rs:815`) and is *not* a readiness signal — an empty path would make a
sandboxed agent's `claude -p` run on the **host** with
`--dangerously-skip-permissions`. The gate therefore keys on config
`is_sandboxed` **+** `SandboxHealth`, never on path presence. CC for a
sandboxed agent runs only when health is `Ready`.

If a turn fails mid-session with a gateway-class error, the failure path calls
`report_suspected_failure()` so the supervisor verifies and degrades uniformly.

### Component 5 — Telegram copy

Consequence-first, HTML-escaped (`ParseMode::Html`), no `Failed:` prefix,
preserves `eff_thread_id`. Example (Docker down):

> ⚠️ I can't run right now — my secure sandbox backend is offline.
> Likely cause: Docker isn't running.
> Fix: start Docker — I'll reconnect automatically within ~15s.

Back-online notice, sent once per affected `(chat, thread)` on recovery:

> ✅ Sandbox back online — I'm ready.

Copy is generated from `GatewayDiagnosis` (`summary` + first fix), so each
cause yields its own message.

## Data flow

```
startup - bring_up() -+- Ok ----- set_ready --- serve (happy path, unchanged)
                      +- Err(d) - log ERROR - set_unavailable - serve degraded
                                                     |
inbound msg - health()? -- Ready --> dispatch CC
                          +-Unavailable - note_affected + reply(d) - skip CC
                                                     |
supervisor (recovery) - retry bring_up() w/ backoff - Ok - set_ready
                                                          + spawn sync task
                                                          + back-online to affected
```

## Security

- Degrade **fails closed**: a sandboxed agent never executes CC on the host.
  The gate enforces this on `is_sandboxed + health`, independent of
  `ssh_config_path`.
- No credentials or gateway internals enter the diagnosis text or logs;
  `summary`/`fixes` are static, cause-derived strings.

## Testing

Pure-function units (no live gateway):
- `classify_doctor_output` — captured `openshell doctor check` + `status`
  samples -> expected `GatewayCause` (Docker down, gateway not started,
  version too old, unclassified).
- `GatewayDiagnosis::fixes()` / summary per cause.
- Message-gate decision: `(is_sandboxed, SandboxHealth)` -> `Reply | Proceed`,
  including the fail-closed assertion that a sandboxed + `Unavailable` state
  never yields `Proceed`.
- `SandboxRuntimeHandle` transitions: affected-set populate/drain, health swap.
- Copy: HTML-escaping + `eff_thread_id` preservation.

Live gateway up->down->up is impractical in CI; the bring-up failure path is
covered by the pure layers above. Any test that does touch a live sandbox uses
`TestSandbox` and the `ci_openshell_` / `#[ignore = "ci-openshell: ..."]`
convention.

**Verification cadence:** targeted `cargo test -p right-openshell` and
`-p right-bot <filter>` after each red/green slice; one
`devenv shell -- cargo test --workspace` at the end (mandatory).

## Docs to update on implementation (cite-on-touch)

- `docs/architecture/sandbox.md` — bot-startup sandbox sequence now degrades
  instead of hard-failing; supervisor + recovery.
- `docs/architecture/lifecycle.md` — per-message flow gains the health gate.
- `ARCHITECTURE.md` — only if a new invariant lands (e.g. "sandboxed CC runs
  only when SandboxHealth == Ready; degrade fails closed"). Keep to <=3
  sentences if added.
- `PROMPT_SYSTEM.md` — no change expected (no prompt-surface change).

## Open risk

The `SandboxExec` move behind `ArcSwap` touches the dashboard router and sync
task, which capture it at startup today. The supervisor becomes the single
owner; consumers load on demand. Comprehensive existing tests plus the new
unit suite cover the seams; the workspace test is the final gate.
