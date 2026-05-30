# MCP Health Reconciler — Design

**Date:** 2026-05-31
**Status:** Approved for planning
**Scope:** `right-mcp`, `right` (bot startup wiring)

## Problem

`BackendStatus` for an external MCP server (`ProxyBackend.status`,
`crates/right-mcp/src/proxy.rs:310`) is **purely event-driven and
in-memory**. It changes on exactly four events:

1. Startup `connect()` success → `Connected` (`proxy.rs:418`).
2. `connect()` hits `"Auth required"` → `NeedsAuth` (`proxy.rs:439`).
3. A `tools_call` returns `"Auth required"` → `NeedsAuth` (`proxy.rs:490`).
4. A permanent OAuth refresh failure → `NeedsAuth`
   (`refresh.rs:467`, `reconnect.rs:266`).

Nothing else ever touches it. Two failure modes follow:

- **Non-auth death is invisible.** A 502, timeout, or dropped connection
  during `tools_call` returns `CallToolFailed` and leaves status
  `Connected` (`proxy.rs:495`). `handle_mcp_list`
  (`crates/right/src/internal_api.rs:975`) returns the live in-memory
  status verbatim, so the dashboard shows a green "connected" pill for a
  server that is actually dead.
- **Recovery is invisible.** A backend that was `Unreachable` at startup
  is never re-probed. It stays `unreachable` forever — even after the
  upstream recovers. Worse, `tools_call` *short-circuits* on `Unreachable`
  (`proxy.rs:451`) and returns an error without contacting the upstream,
  so the **agent gives up without trying.**

This second mode is the confirmed root cause of the 2026-05-27 incident
with agent `agent-b`: Composio's gateway flapped (unreachable → 502 →
recovered), but the displayed status (and the agent's willingness to call
it) only tracked reality through incidental restart/refresh events, not
through any active check. The agent declared "composio unreachable,
giving up" and later recited stale failure state as current fact.

### What this is NOT

The OAuth-expiry detection path **already works** and is verified live:
an expired token with a dead refresh token (`invalid_grant`) is detected
proactively at startup via `refresh_due_in == ZERO` → reconnect →
permanent-failure → `NeedsAuth`, and `mcp_list` correctly reports
`needs_auth`. This design does not touch that path. The dashboard read
path and Vue rendering are also correct — `McpView` renders whatever
status string it is given. The defect is solely that the **status value
itself goes stale** on the Connected↔Unreachable axis.

## Goal

Make `BackendStatus` reflect upstream reality for **both** readers — the
dashboard (`mcp_list`) and the agent (`tools_call`) — independent of
whether the agent happens to call a tool or whether anyone has the
dashboard open. Concretely:

- A `Connected` server that dies (non-auth) flips to `Unreachable`.
- An `Unreachable` server that recovers flips back to `Connected` (the
  `agent-b` regression).
- A server whose probe reveals auth death flips to `NeedsAuth`.
- `tool_count` stays current as upstream tool sets change.

## Approach: periodic health reconciler

A per-agent background task that probes each external backend on an
adaptive interval and writes its true status into the **same**
`Arc<ProxyBackend>` map the dashboard and agent read. This is the only
option that keeps status honest for an idle dashboard *and* fixes the
agent's `Unreachable` give-up, because it is the single thing that
re-probes a down server to detect recovery.

Rejected alternatives:
- **Probe-on-read in `mcp_list`:** fresh for the dashboard, but leaves the
  agent's `tools_call` short-circuit stale (the functional bug unfixed)
  and blocks the dashboard on N upstream round-trips.
- **Event-driven only** (flip on `tools_call` failure, reconnect on use):
  fixes the agent path with zero idle traffic, but an idle dashboard
  never sees recovery until traffic flows.

The reconciler folds in the event-driven improvements implicitly: because
it owns recovery, the agent no longer needs to wait for a tool call to
discover a server is back.

## Component & placement

New module `crates/right-mcp/src/health.rs` exposing:

```rust
pub async fn run_health_reconciler(
    proxies: Arc<RwLock<HashMap<String, Arc<ProxyBackend>>>>,
    http_client: reqwest::Client,
) -> /* never returns; process-lifetime */
```

Spawned once per agent in `crates/right/src/main.rs` at the existing
per-agent startup block (alongside `run_refresh_scheduler`, `main.rs:1250`)
as a sibling `tokio::spawn`. All required handles are already in scope
there: `registry.proxies`, `http_client`, `agent_dir`.

Rationale for `right-mcp` (not `right`): it manipulates `ProxyBackend`
status and belongs with the MCP-connection-lifecycle cluster
(`proxy.rs` / `refresh.rs` / `reconnect.rs`). No new crate, no new
long-lived state types — it holds only a clone of `proxies` (also held by
the dispatcher for process lifetime) plus loop-local timing/strike state.

## Reconcile tick — state machine

Probe = `client.peer().list_all_tools()` on the live rmcp session — the
cheapest real round-trip that exercises both transport and auth (the same
call `connect()` makes). Per-backend logic each time a backend is due:

| Current status | Action | Outcome → new status |
|---|---|---|
| `Connected`   | `probe_live()` on live session | `Alive` → stay `Connected` (refresh tool cache, reset strikes); `AuthRequired` → `NeedsAuth` (immediate); `Dead` → strike++; flip to `Unreachable` only on strike 3 |
| `Unreachable` | `connect()` | success → `Connected` (set by `connect()`); `AuthRequired` → `NeedsAuth` (set by `connect()`); fail → stay `Unreachable` |
| `NeedsAuth`   | **skip** | unchanged |

### Why `NeedsAuth` is skipped (load-bearing)

A server needing auth can only be fixed by a user re-OAuth (dashboard) or
the refresh scheduler landing a new token — both already call `connect()`
and flip it out of `NeedsAuth`. Probing it would hammer a known-dead
credential and risk masking the "user must act" state. Auth recovery
stays owned by refresh/reconnect; the health reconciler owns only the
**Connected↔Unreachable** axis, plus *demoting* to `NeedsAuth` when a
probe reveals auth death.

### Debounce (outage flip)

- Failure counter lives in the reconciler's loop-local
  `HashMap<String, u32>`, **not** on `ProxyBackend` — backend stays a pure
  status holder; all status policy stays in one place.
- **3 strikes** of consecutive `Dead` probes before `Connected →
  Unreachable`. Counter resets to 0 on any `Alive` probe.
- `AuthRequired` is **exempt** from debounce — one clear signal, flips to
  `NeedsAuth` immediately.
- No session to probe (`client = None` on a `Connected` backend — should
  not happen, but `tools_call` guards it as `NoSession`) → treated as a
  `Dead` probe outcome → counts as a strike.

### Adaptive cadence

Per-backend next-wake scheduled from the status just observed:

| Status | Cadence | Rationale |
|---|---|---|
| `Connected`   | **120s** | healthy; the event path (`tools_call`) catches death between ticks; this is a light backstop + tool-cache refresh |
| `Unreachable` | **20s**  | the only thing that catches recovery; the agent is blocked until it flips |
| `NeedsAuth`   | never    | skipped |

Latency consequences (intended asymmetry):
- Recovery (`Unreachable→Connected`): no debounce → ~20s.
- Outage (`Connected→Unreachable`): 3 strikes × 120s → ~6min worst case,
  but in practice the agent's next `tool_call` flips a dead `Connected`
  server via the existing event path well before that. The reconciler is
  the backstop, not the primary outage detector.

Intervals are module consts; overridable via env at the spawn site only
(bot-internal runtime, like the refresh scheduler — no CLI surface).

### Tool-cache refresh

A successful `Connected` probe writes the returned tools to
`cached_tools`, so `tool_count` on the dashboard and the agent's available
tool set track reality (e.g. Composio gaining/losing an integration).
Negligible extra cost — `list_all_tools()` already returns the data.

## New `ProxyBackend` method

`connect()` is too heavy to run every interval on healthy servers — it
re-initializes the rmcp session and rewrites instructions to SQLite. So
the `Connected`-probe branch needs a lightweight liveness call:

```rust
pub enum ProbeOutcome {
    Alive,
    AuthRequired,
    Dead(String),
}

impl ProxyBackend {
    /// Probe the live session: list tools, refresh `cached_tools` on success.
    /// Returns the outcome; does NOT mutate `status` — the caller's debounce
    /// owns the flip decision so all status policy stays in one place.
    pub async fn probe_live(&self) -> ProbeOutcome { ... }
}
```

- Reads the existing `client` session; if `None` → `Dead`.
- Calls `peer().list_all_tools()`; on success writes `cached_tools`
  (filtered the same way `connect()` filters: drop names containing `__`)
  and returns `Alive`.
- On error: `is_upstream_auth_error(msg)` → `AuthRequired`, else
  `Dead(msg)`.
- Never writes `status`.

The `Unreachable→reconnect` branch reuses the existing `connect()` and
lets it set status as it already does (`Connected` / `NeedsAuth`
internally) — no new method needed there. The existing auth-flip writers
in `tools_call` / `connect` are unchanged — they are synchronous reactions
to live traffic, orthogonal to the periodic axis.

Total `right-mcp` surface: one new module (`health.rs`), one new method
(`probe_live`) + `ProbeOutcome` enum, one spawn line in `main.rs`.

## Concurrency, shutdown, error handling

**Locking.** The reconciler clones `Arc<ProxyBackend>` handles out of
`proxies` under a brief read lock, then **drops the lock before probing**
— never holds the map lock across an upstream round-trip (a hanging
upstream must not block `mcp_list` reads or the agent's tool dispatch).
Each backend's own `RwLock<status>` / `connect_mutex` handles per-backend
serialization, so a reconcile probe and a concurrent `tools_call` cannot
corrupt status. The `Unreachable→connect()` branch goes through
`connect_mutex` for free (it already serializes reconnect vs.
dashboard-OAuth).

**Per-tick fan-out.** Probe all due backends concurrently (`join_all`);
N is tiny (1–3 servers) and one slow upstream must not delay the others.

**Per-probe timeout.** 10s. A black-holed connection cannot wedge the
tick; a timeout counts as a `Dead` strike.

**Shutdown.** No cancellation token. The task loops for process lifetime,
matching the refresh scheduler's fire-and-forget model — it holds only a
clone of `proxies` (also held by the dispatcher, which lives for process
lifetime). Bot graceful-restart tears down the whole process.

**Error handling (FAIL FAST compliance).** A probe *failure* is an
expected, modeled outcome (`ProbeOutcome::Dead` / `AuthRequired`), not an
error to propagate — detecting dead servers is the entire point. The
reconcile loop never silently swallows: every probe outcome logs at
debug, every status **transition** logs at info/warn so the flap is
explicit in `right-mcp-server` logs, e.g.
`composio: connected → unreachable (strike 3/3)` and
`composio: unreachable → connected`. Genuinely unexpected errors
(none in the probe path — it touches no DB) would propagate normally.

## Testing strategy

Pure MCP-proxy tests, no live OpenShell (no sandbox involved). TDD
red/green per AGENTS.rust.md.

**Unit — `probe_live` (`proxy.rs` tests, wiremock upstream):**
- live session + 200 + tools → `Alive`, `cached_tools` updated (assert
  count changed).
- upstream `"Auth required"` → `AuthRequired`, status untouched.
- upstream 502 / timeout → `Dead`, status untouched.
- no session (`client = None`) → `Dead`.

**Unit — reconciler state machine (`health.rs` tests, `tokio::time::pause`):**
- `Connected` + 3 consecutive `Dead` → flips `Unreachable` **on strike 3,
  not 2** (assert the off-by-one explicitly).
- `Connected` + 2 `Dead` then `Alive` → counter resets, stays `Connected`.
- `Connected` + `AuthRequired` → immediate `NeedsAuth`, no debounce.
- `Unreachable` + successful `connect()` → `Connected`. **Named
  `unreachable_backend_recovers_to_connected_on_probe` — this is the
  May-27 regression anchor.**
- `NeedsAuth` → never probed (assert the mock upstream received zero
  requests).
- adaptive cadence: assert an `Unreachable` backend is re-probed at ~20s
  and a `Connected` one at ~120s via `tokio::time::advance`.
- concurrency: map read-lock released before probe — a probe in flight on
  one backend does not block another's tick (two mock servers, one
  delayed).

**No dashboard frontend tests** — `McpView` already renders whatever
status string it receives; this change only makes that string truthful.
No Vue change.

**Cadence:**
- Worktree start: baseline `devenv shell -- cargo test -p right-mcp`,
  record pre-existing failures.
- During: targeted `-p right-mcp <filter>` red/green loops.
- End (mandatory): `devenv shell -- cargo test --workspace` from inside
  the worktree.

## Decisions summary

| Decision | Choice |
|---|---|
| Component | `right-mcp/src/health.rs`, `run_health_reconciler` |
| Spawn site | per-agent in `main.rs:1250`, sibling of refresh scheduler |
| Axis owned | Connected↔Unreachable; demote-to-NeedsAuth on auth probe |
| `NeedsAuth` | never probed (refresh/reconnect own auth recovery) |
| New method | `ProxyBackend::probe_live() -> ProbeOutcome` (no status write) |
| Outage debounce | 3 strikes, counter in reconciler loop state |
| Auth flip | immediate, debounce-exempt |
| Cadence | adaptive: Connected 120s, Unreachable 20s, NeedsAuth never |
| Probe timeout | 10s |
| Fan-out | concurrent per tick; map lock dropped before probing |
| Shutdown | process-lifetime, no cancel token |
| Frontend | unchanged |

## Non-goals

- Per-sub-integration auth status for opaque hub MCPs (Composio→Notion).
  Composio remains one opaque server with one status; surfacing Notion's
  state inside Composio is a separate, harder problem deferred out.
- Proactive revocation detection for static-key (`header`/`query_string`)
  servers. The periodic probe catches a revoked key only insofar as the
  upstream rejects `list_all_tools()` (→ `Dead`, or `AuthRequired` if it
  surfaces `"Auth required"`); a server that still serves a cached tool
  list under a revoked key would read `Alive`. No dedicated key-validity
  check is added.
- Any change to the OAuth refresh / reconnect path (already correct).
- Any frontend polling change (out of scope; backend truth only).
