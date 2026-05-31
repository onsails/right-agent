# MCP `set-headers` Decoupling + Reconnect Observability — Design

**Date:** 2026-06-01
**Status:** Approved for planning
**Scope:** `right` (`internal_api.rs`), `right-mcp` (`proxy.rs`, `health.rs`, `internal_client.rs`), `right-dashboard` (`McpView.vue`, `mcpViewModel.ts`, `types.ts`)

## Problem

Adding an HTTP header to an external MCP server through the Telegram Mini
App can silently fail to take effect, and when an external server is
stuck `unreachable` there is no way to find out why without trawling host
process logs. Both were hit live on agent `right` with an Obsidian MCP
server (`http://openclaw.owl-skate.ts.net:27123/mcp`).

Three coupled defects:

1. **`set-headers` gates persistence on a live connection.**
   `handle_mcp_set_headers` (`crates/right/src/internal_api.rs:618`) builds
   a replacement `ProxyBackend`, calls `replacement.connect()`
   (`internal_api.rs:670`), and **returns an error without persisting the
   header or swapping the backend** if that connect fails. Persisting a
   credential has no dependency on upstream reachability, yet a
   temporarily-down server (e.g. Obsidian's Local REST/MCP server only
   runs while the desktop app is open on that node) makes the save
   impossible. The user sees the server still `unavailable` and assumes
   the header was applied; in reality `mcp_servers.auth_type` is unchanged
   and `mcp_http_headers` is empty.

2. **Non-auth `connect()` failures are invisible.**
   `ProxyBackend::connect()` (`crates/right-mcp/src/proxy.rs:373`) logs
   only on success (`info`, line 446) and on auth-required failure
   (`warn`, line 460). The `InitFailed` / `ListToolsFailed` paths
   (lines 395, 410) return the error with **no log at all**. The health
   reconciler's `Unreachable` branch then discards it entirely
   (`let _ = … connect()`, `crates/right-mcp/src/health.rs:125`). A backend
   can sit `unreachable` indefinitely, re-probed every 20s, with **zero
   log output** — which is exactly what made the Obsidian case
   undebuggable. This violates the AGENTS.md rule "errors must propagate
   to logs, never be silently swallowed."

3. **The dashboard shows status without cause.** `/mcp-list`
   (`internal_api.rs:906`) returns only the status string
   (`connected` / `unreachable` / `needs_auth`). `McpView` can render that
   a server is down, but never *why* or *when last tried*.

### What this is NOT

This does not fix the underlying intermittency of the upstream (a node
that sleeps is the user's environment, out of scope). It does not change
the `needs_auth` semantics: a server that actively rejects a credential
(HTTP 401) is still parked and **not** auto-retried by the reconciler.
It does not touch the OAuth or `bearer` write paths beyond the shared
`connect()` observability change.

## Goal

- Saving a header **always persists** the credential, regardless of
  whether the upstream is currently reachable.
- A header saved against a temporarily-down server **self-heals**: the
  reconciler reconnects with the new credential once the server returns.
- Every `connect()` outcome is **traceable** — in logs and, for the
  failure cause, in the live `/mcp-list` status the dashboard already
  consumes.

## Design

### 1. Decouple persistence from connection (`handle_mcp_set_headers`)

New order in `handle_mcp_set_headers` (`internal_api.rs:618`):

1. Validate headers (empty → 400, unchanged).
2. **`db_set_http_headers` first.** This is the only step whose failure is
   a request error (→ 500); the credential write does not depend on the
   upstream.
3. Build the replacement `ProxyBackend` with the new headers and **swap it
   into `proxies` immediately**, in its constructor-default `Unreachable`
   state.
4. Return `{ ok: true }` immediately — the connection outcome is reported
   through live `/mcp-list` status, not this response. The response stays
   fast and the live status is the single source of truth.
5. `tokio::spawn` a single best-effort `connect()` so the happy path
   connects in ~1s instead of waiting for the next reconcile tick. This is
   safe: `connect()` is serialized by `connect_mutex` (`proxy.rs:380`), so
   it cannot race the reconciler's probe.

Resulting status (`connect()` already sets it internally):

| connect outcome | status | reconciler behaviour |
|---|---|---|
| success | `connected` | — |
| transport / init failure | `unreachable` | re-probes every 20s **with the new headers** → self-heals (defect 1 + recovery) |
| HTTP 401 | `needs_auth` | parked, never auto-retried (a rejected credential will not fix itself; the user must correct it) |

The old "test-then-save, discard-on-failure" branch (`internal_api.rs:670-679`)
is removed.

### 2. Reconnect observability (`connect()` + reconciler)

- **`connect()` logs every failure.** On the `InitFailed` / `ListToolsFailed`
  paths, emit `debug!` with `server`, the URL host:port, the `phase`
  (`initialize` / `list_tools`), and the full error chain (`{e:#}`) before
  returning. (Auth path already `warn`s; success already `info`s.) Dev bots
  run with `--debug`, so this makes every attempt visible.
- **Per-backend last-attempt fields.** Add to `ProxyBackend` three
  in-memory fields behind `RwLock`:
  `last_attempt_at: Option<chrono::DateTime<chrono::Utc>>`,
  `last_success_at: Option<chrono::DateTime<chrono::Utc>>`,
  `last_connect_error: Option<String>` (chrono is already a `right-mcp`
  dependency). Written on every `connect()` outcome. The error text is
  `{e:#}` (transport/protocol detail: host:port, status codes) — **never**
  header values or tokens. Because `query_string`-auth embeds the token in
  the URL and rmcp transport errors can quote the URL, the recorded/logged
  error is passed through a `redact_query_strings` helper that strips `?…`
  query portions from URL-like substrings.
- **Reconciler logs transitions, not every probe.** Replace
  `let _ = … connect()` (`health.rs:125`) with a captured result. A failed
  reconnect that leaves the backend `Unreachable` logs at `debug` (no 20s
  `warn` spam); state changes (`unreachable → connected`,
  `connected → unreachable`) keep their existing `info`/`warn`.

### 3. Surface cause via `/mcp-list` → dashboard

- Extend `McpServerStatus` in **both** copies that must stay in sync —
  `crates/right/src/internal_api.rs:108` (serialize) and
  `crates/right-mcp/src/internal_client.rs:440` (deserialize) — with
  `last_connect_error: Option<String>`, `last_attempt_at: Option<String>`
  (RFC3339), `last_success_at: Option<String>`, each
  `#[serde(default, skip_serializing_if = "Option::is_none")]` for
  backward compatibility.
- `handle_mcp_list` (`internal_api.rs:906`) reads them from the backend.
- Dashboard: the "human-readable status + reason + last-tried-ago"
  derivation goes in `mcpViewModel.ts` (pure, unit-tested per the
  dashboard-primitives rule); `McpView.vue` renders it; `types.ts` gains
  the fields. No secret ever reaches the UI.

## Data & Compatibility

- last-attempt state is **in-memory only** — no `data.db` migration; it is
  disposable runtime state, consistent with the existing `BackendStatus`
  model.
- New `McpServerStatus` fields are additive and `#[serde(default)]` — older
  clients and persisted payloads deserialize unchanged.
- No sandbox recreation, no new codegen category, no policy change.
- Side effect: an existing server on the legacy singular
  `auth_type = 'header'` is naturally migrated to plural `'headers'` by the
  first successful `set-headers` save. No special handling required.

## Error Handling

- DB write failure in `set-headers` → 500 (the credential did not save).
- Header validation failure → 400 (unchanged).
- `connect()` failures **never** surface as a `set-headers` request error;
  they are reflected only in live status + logs (decision B).

## Testing

TDD, narrowest-first, per `AGENTS.rust.md` cadence. Targeted package tests
during development; one mandatory `cargo test --workspace` at the end.

- **Regression (the core bug):** `set-headers` against an unreachable
  server persists the headers, returns `ok`, and leaves the backend in a
  retryable (`unreachable`) state — it no longer returns 502 nor discards
  the credential.
- **`proxy`:** a `connect()` transport failure records `last_connect_error`
  and `last_attempt_at`; a success records `last_success_at`.
- **`health`:** a failed reconnect leaves the backend `unreachable` without
  a `warn` (debug-only); recovery still flips to `connected` (existing
  recovery test stays green).
- **Dashboard:** `mcpViewModel` unit test for the `unreachable` + reason +
  last-tried rendering; `McpView` SSR test.

## Non-Goals

- Fixing upstream intermittency (sleeping node / app-not-running).
- Changing `needs_auth` parking behaviour.
- Persisting last-attempt telemetry across restarts.
