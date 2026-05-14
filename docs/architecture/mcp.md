# MCP Aggregator and token refresh

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## MCP Token Refresh

```
OAuth callback (bot) → POST /set-token to Aggregator (Unix socket)
  → Aggregator updates DynamicAuthClient.token in-memory
  → Aggregator saves to mcp_servers SQLite table (auth_token, expires_at, etc.)
  → Aggregator schedules refresh timer (see "Refresh margin" below)
  → on timer: POST refresh_token to token_endpoint
  → classify outcome (success / Transient / Permanent)
  → on success: update DynamicAuthClient.token in-memory, persist to SQLite
    (db_update_oauth_token), reset retry counter, reschedule next refresh
  → no .mcp.json writes, no sandbox uploads
```

### Refresh margin

`refresh.rs::refresh_due_in` schedules each refresh at
`expires_at - min(1h, remaining_lifetime / 2)`. The 1-hour upper bound (vs.
the previous flat 10 min) gives long-lived tokens room for transient
outages — laptops dropping Wi-Fi for a few minutes still get multiple retry
windows before the token actually dies. The `lifetime / 2` clamp keeps
short-lived tokens (TTL under ~2h) from busy-looping the scheduler.

### Failure classification

`reconnect.rs::do_refresh_cancellable` classifies every token-endpoint
failure into one of two variants of `RefreshFailure`:

| Class | Triggered by | Scheduler response |
|-------|--------------|--------------------|
| `Transient` | Network error, 5xx, 408, 429 | Reschedule with exponential backoff, retry indefinitely. Backend status unchanged (stays `Connected` from the user's perspective). |
| `Permanent` | Non-recoverable 4xx (typically `invalid_grant` / `invalid_client`) | Flip backend to `NeedsAuth`, drop the timer, clear retry counter. User must re-OAuth via `/mcp auth <server>`. |

Transient backoff schedule (`refresh.rs::transient_backoff_secs`,
1-indexed): `60, 120, 300, 600, 1200, 1800` seconds, capped at 1800s
(30 min) for all subsequent attempts. The counter resets on success and
on any new `RefreshMessage::NewEntry` (e.g. after `/mcp auth`), so a
stale counter from prior failures can't push the next retry past the
60-second first step.

### Tool-call 401 detection

A token can also die mid-session: the upstream MCP rejects a tool call
even though the local refresh timer hasn't fired yet (clock skew, server-
side revocation, etc.). `proxy.rs::ProxyBackend::tools_call` catches this
by inspecting the rmcp error string via `proxy.rs::is_upstream_auth_error`
— rmcp surfaces 401s from `StreamableHttpClient` as a `TransportSend`
error whose `Display` contains `"Auth required"`. On match the backend
flips to `NeedsAuth` and returns `ProxyError::NeedsAuth` (not opaque
`tool_failed`), so `mcp_list` reports the truth instead of `connected`.

### Post-refresh reconnect

When the scheduler completes a successful refresh while the backend was
`NeedsAuth` (set by either a permanent-failure flip that has since
recovered, or a tool-call 401), the rmcp session is almost certainly
dead. The `was_needs_auth` branch in `refresh.rs::run_refresh_scheduler`
spawns `backend.connect(http)` in the background to re-establish the
session before the next tool call.

### Scheduler concurrency

`refresh.rs::run_refresh_scheduler` is a single `tokio::select!` loop with
three arms: inbound `rx.recv()`, an in-flight `JoinSet::join_next()`, and a
timer wake-up. Refresh attempts are **spawned** into the `JoinSet`, not
awaited inline — this keeps the `rx.recv()` arm responsive while a refresh
runs. An exhausting-backoff path (~210s) on one server would otherwise
starve `RemoveServer` for a different server, or block a `NewEntry`
arriving from a just-completed `/mcp auth` for minutes.

Two correctness handles keep stale results from polluting state:

1. **Per-server `CancellationToken`s** (`cancel_tokens`): both
   `RemoveServer` and a superseding `NewEntry` cancel the in-flight
   refresh, so `do_refresh_cancellable` aborts at its next pre-attempt
   check or interrupts a backoff sleep.
2. **Per-server generation counters** (`generations`): `NewEntry` bumps
   the counter; each spawned task is tagged with the generation it saw
   at spawn time. The join_next handler discards results whose tag
   doesn't match the current generation. This defends against the
   `do_refresh_cancellable` Ok path, which returns immediately on a
   successful HTTP response without re-checking the cancel token — so
   a NewEntry that races a near-completion refresh could otherwise see
   the stale Ok overwrite the freshly-rotated credentials.

### `ProxyBackend` status transitions

`BackendStatus` is `Connected | NeedsAuth | Unreachable`. Refresh- and
tool-call-driven transitions:

| Trigger | From | To |
|---------|------|----|
| Transient refresh failure | `Connected` | `Connected` (unchanged; retry pending) |
| Permanent refresh failure | any | `NeedsAuth` |
| Tool-call upstream 401 (`Auth required`) | `Connected` | `NeedsAuth` |
| Successful refresh | `NeedsAuth` | `NeedsAuth` → background `connect()` → `Connected` |
| Successful refresh | `Connected` | `Connected` (no reconnect needed) |

(Initial connect-time transitions — `Unreachable` → `Connected` on
successful `connect()` — are unchanged from before this branch.)

## MCP Aggregator

The Aggregator replaces HttpMemoryServer as the MCP endpoint. One shared process
serves all agents on TCP :8100/mcp with per-agent Bearer token authentication.

Tool routing:
  - No `__` prefix → RightBackend (built-in tools, unprefixed)
  - `rightmeta__` prefix → Aggregator management (read-only: mcp_list)
  - `{server}__` prefix → ProxyBackend (forwarded to upstream MCP)

Internal REST API on Unix socket (~/.right/run/internal.sock):
  - POST /mcp-add — register external MCP server
  - POST /mcp-remove — remove external MCP server
  - POST /set-token — deliver OAuth tokens after authentication
  - POST /mcp-list — list MCP servers with status
  - POST /mcp-instructions — fetch MCP server instructions markdown

Telegram bot uses InternalClient (hyper UDS) to call these endpoints.
Agents cannot reach the Unix socket from inside the sandbox.
