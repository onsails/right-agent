# MCP Aggregator and token refresh

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## MCP Dashboard Management Flow

Telegram `/mcp` opens the Telegram Mini App dashboard on the MCP view; Telegram
no longer implements MCP add/auth/remove/list subcommands. The add flow is
URL-first: the dashboard collects server name + URL, runs detection, then shows
`OAuth`, `Headers`, and `URL as-is` choices. Detection is advisory.

No upstream MCP server is registered until the user saves a chosen mode.
`OAuth` registers the bare URL as `auth_type=oauth` and uses the dashboard OAuth
start route to return an authorization URL. `Headers` stores configured HTTP
header credentials through the dashboard headers route. Header values are
write-only secrets; list APIs return header names only. `URL as-is` registers
the exact original URL without token/header injection, preserving query-string
credentials.

OAuth-capable HTTP MCP servers can advertise a canonical resource URI through
RFC 9728 protected-resource metadata. Right persists that resource with the
OAuth state and sends it during authorization-code exchange and refresh-token
requests. Rows created before `oauth_resource` existed fall back to the
canonicalized MCP server URL.

URL validation has two modes. Public detection accepts network-routable HTTP and
HTTPS URLs, excludes loopback/private/link-local hosts, and returns a short
`Plain HTTP: trusted/encrypted networks only.` warning when plain HTTP is
registered. Explicit user-managed registration allows HTTP/HTTPS while broad
private/link-local ranges remain rejected.
Dashboard MCP detection applies the same public-network URL policy to every
OAuth discovery fetch, including the original MCP probe, RFC 9728
`resource_metadata` URLs, synthesized well-known URLs, and authorization-server
metadata URLs. Literal private or localhost IP URLs are rejected before fetch;
obvious localhost domain aliases such as `localhost` and `localhost.` are also
rejected before fetch. Other domain names are allowed through the dashboard
detection client's guarded DNS resolver.

Dashboard OAuth start is a Mini-App-authenticated API flow. The route lists
registered MCP servers through the Aggregator internal API, finds the selected
server URL, requires a configured tunnel hostname, discovers OAuth metadata,
optionally performs Dynamic Client Registration, stores an in-memory
`PendingAuth` keyed by generated state, stores a matching transient dashboard
OAuth status under the same state value, and returns the authorization URL plus
`flow_id`. The OAuth callback records completion in that transient status store
instead of sending Telegram DMs. The dashboard polls
`/dashboard/<agent>/api/v1/mcp/oauth/<flow_id>/status` until terminal status,
then refreshes the MCP server list. Bot restarts lose in-flight status; the
dashboard treats that as `unknown`, and the user starts OAuth again.
The callback URI is always `https://<tunnel-hostname>/oauth/<agent>/callback`;
tokens, client secrets, and PKCE verifiers remain out of dashboard responses.

## MCP Token Refresh

```
OAuth callback (bot) → POST /set-token to Aggregator (Unix socket)
  → Aggregator updates DynamicAuthClient.token in-memory
  → Aggregator saves token fields and oauth_resource to mcp_servers SQLite table
  → Aggregator schedules refresh timer (see "Refresh margin" below)
  → Aggregator retries MCP reconnect readiness with the fresh token
    and returns success only if that reconnect succeeds
  → on timer: POST refresh_token + resource to token_endpoint
  → classify outcome (success / Transient / Permanent)
  → on success: update DynamicAuthClient.token in-memory, persist to SQLite
    (db_update_oauth_token), reset retry counter, reschedule next refresh
  → no .mcp.json writes, no sandbox uploads
```

### OAuth callback readiness

The bot callback handler only acknowledges that the provider redirected with an
authorization code. It does not tell the user that the MCP server is ready until
the background token exchange and Aggregator `/set-token` call finish.

`/set-token` persists the fresh OAuth state and schedules refresh before probing
upstream readiness. It then runs a bounded `ProxyBackend::connect()` retry loop
with the new token. A 5xx initialize failure is retried; if all attempts fail,
the backend is marked `Unreachable` and `/set-token` returns `502`. If
readiness fails, the dashboard OAuth status reports a failure instead of
showing a false success. If the reconnect failure is an upstream auth error
(`"Auth required"`), the backend remains `NeedsAuth` and `/set-token` returns
`401`.

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

Refresh requests are cancellation-aware while the HTTP request is in flight
and while sleeping between transient retries. Removing a server or replacing
its OAuth entry does not wait for a slow token endpoint or client timeout.

| Class | Triggered by | Scheduler response |
|-------|--------------|--------------------|
| `Transient` | Network error, 5xx, 408, 429, Cloudflare 403 challenge page | Reschedule with exponential backoff, retry indefinitely. Backend status unchanged (stays `Connected` from the user's perspective). |
| `Permanent` | Non-recoverable 4xx (typically `invalid_grant` / `invalid_client`) | Flip backend to `NeedsAuth`, drop the timer, clear retry counter. User must re-OAuth from the dashboard MCP view. |

Transient backoff schedule (`refresh.rs::transient_backoff_secs`,
1-indexed): `60, 120, 300, 600, 1200, 1800` seconds, capped at 1800s
(30 min) for all subsequent attempts. The counter resets on success and
on any new `RefreshMessage::NewEntry` (e.g. after dashboard OAuth), so a
stale counter from prior failures can't push the next retry past the
60-second first step.

### Tool-call 401 detection

A token can also die mid-session: the upstream MCP rejects a tool call
even though the local refresh timer hasn't fired yet (clock skew, server-side
revocation, or stale credentials). `proxy.rs::ProxyBackend::tools_call` catches
this by inspecting the rmcp error string via
`proxy.rs::is_upstream_auth_error` — rmcp surfaces 401s from
`StreamableHttpClient` as a `TransportSend` error whose `Display` contains
`"Auth required"`. On match the backend flips to `NeedsAuth` and returns
`ProxyError::NeedsAuth` (not opaque `tool_failed`), so `mcp_list` reports the
truth instead of `connected`.

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
arriving from a just-completed dashboard OAuth flow for minutes.

Two correctness handles keep stale results from polluting state:

1. **Per-server `CancellationToken`s** (`cancel_tokens`): both
   `RemoveServer` and a superseding `NewEntry` cancel the in-flight
   refresh, so `do_refresh_cancellable` aborts an in-flight token request,
   the next pre-attempt check, or a backoff sleep.
2. **Per-server generation counters** (`generations`): `NewEntry` bumps
   the counter; each spawned task is tagged with the generation it saw
   at spawn time. The join_next handler discards results whose tag
   doesn't match the current generation. This defends against the
   `do_refresh_cancellable` Ok path, which returns immediately on a
   successful HTTP response without re-checking the cancel token — so
   a NewEntry that races a near-completion refresh could otherwise see
   the stale Ok overwrite the freshly-rotated credentials.

### `ProxyBackend` status transitions

`BackendStatus` is `Connected | NeedsAuth | Unreachable`. Refresh-,
tool-call-, and health-reconciler-driven transitions:

| Trigger | From | To |
|---------|------|----|
| Transient refresh failure | `Connected` | `Connected` (unchanged; retry pending) |
| Permanent refresh failure | any | `NeedsAuth` |
| `ProxyBackend::connect()` initialize/list_tools auth failure | any | `NeedsAuth` |
| `/set-token` reconnect 5xx exhausted | any | `Unreachable` |
| `/set-token` reconnect auth failure | any | `NeedsAuth` |
| Tool-call upstream 401 (`Auth required`) | `Connected` | `NeedsAuth` |
| Successful refresh | `NeedsAuth` | `NeedsAuth` → background `connect()` → `Connected` |
| Successful refresh | `Connected` | `Connected` (no reconnect needed) |
| Health reconciler: 3 consecutive Dead probes | `Connected` | `Unreachable` |
| Health reconciler: probe returns `Auth required` | `Connected` | `NeedsAuth` |
| Health reconciler: reconnect succeeds | `Unreachable` | `Connected` |

### Health reconciler

Refresh and tool-call transitions are event-driven: a backend that dies
with a non-auth error (502/timeout) stays `Connected` until the next tool
call, and a backend stuck `Unreachable` is never re-probed for recovery on
its own. The per-agent background reconciler
(`right_mcp::health::run_health_reconciler`, spawned per agent at bot
startup beside the refresh scheduler) closes that gap by periodically
probing each backend so the dashboard and the agent see honest status
without waiting for a tool call.

It probes on an adaptive cadence — `Connected` every 120s (light backstop),
`Unreachable` every 20s (the only recovery path), `NeedsAuth` never (owned
by refresh/reconnect). A `Connected` probe is a lightweight
`ProxyBackend::probe_live()` (lists tools on the live session, refreshes the
tool cache, 10s timeout, never writes status); an `Unreachable` probe is a
full `connect()` attempt. Outage detection is debounced — three consecutive
Dead probes flip `Connected → Unreachable`; a single `Alive` resets the
strike count; an `Auth required` probe flips to `NeedsAuth` immediately. The
status-transition policy is the pure `decide_connected` function so it is
unit-tested without I/O. Each tick snapshots the shared proxies map under a
brief read lock, drops it, probes due backends concurrently, then applies
decisions serially; strike/schedule state is pruned for backends removed
from the map at runtime.

### `set-headers` decoupling + connect observability

`handle_mcp_set_headers` persists the header credential *before* it
connects: it builds the background-connect client, writes the headers via
`db_set_http_headers`, swaps a fresh `Unreachable` `ProxyBackend` (carrying
`AuthMethod::Headers`) into the proxies map, returns `200` immediately, and
only then `tokio::spawn`s a best-effort `connect()`. A temporarily-down
upstream therefore still saves the credential; the health reconciler later
re-probes with the new headers and self-heals to `Connected`. The old
test-then-save gate (which returned `502` and discarded the credential on a
failed connect) is gone.

Every `ProxyBackend::connect()` outcome is recorded in memory:
`last_attempt_at`, `last_success_at`, and `last_connect_error` (the error is
passed through `redact_query_strings` so a `query_string`-auth token quoted
in a transport error never reaches logs or the dashboard). The two
non-auth failure arms also `tracing::debug!` the redacted detail. These
fields are surfaced per server through `/mcp-list` (RFC3339 timestamps) and
rendered by the dashboard `McpView` as the failure cause + "last tried"
line; they are disposable runtime state, not persisted across restarts.

## MCP Aggregator

The Aggregator replaces HttpMemoryServer as the MCP endpoint. One shared process
serves all agents on TCP :8100/mcp with per-agent Bearer token authentication.

In OpenShell mode, sandboxed agents reach the host-side aggregator through
`http://host.openshell.internal:8100/mcp` in `/sandbox/mcp.json`. Credentials,
OAuth state, and external MCP sessions stay on the host. The sandbox policy is
generated in two phases: bootstrap policy before sandbox creation omits guessed
Right MCP `allowed_ips`; after the sandbox is READY, Right resolves
`host.openshell.internal` from inside that sandbox and hot-applies exact IPv4
`/32` and IPv6 `/128` entries. This repeats on bot startup so restored agents
self-heal when moved to a different host. OpenShell `forward` and service
exposure are not used for this path because they expose sandbox services
outward, while Right MCP needs sandbox-to-host access.

## Agent-Facing MCP Health

The dashboard MCP server list reports Aggregator backend status through the
internal Unix-socket API. It does not prove that a specific Claude Code process
loaded the same MCP tool registry. Agent turns are checked separately through
Claude Code's `system/init` stream-json event, which lists the MCP servers
visible to that process.

The bot runs a periodic Haiku health probe using the same strict MCP config
path as real turns (`/sandbox/mcp.json` in OpenShell mode, host `mcp.json` in
no-sandbox mode). If `system/init` reports the built-in `right` server as
`needs-auth` or omits it, the bot removes Claude Code's stale
`.claude/mcp-needs-auth-cache.json`, redeploys platform files, and probes once
more. The repair never recreates sandboxes, rewrites external MCP credentials,
or changes the user's Claude session.

Normal user turns also observe `system/init`. If a turn sees unhealthy `right`,
it schedules the same repair path asynchronously but does not kill, retry, or
replace the current turn.

Tool routing:
  - No `__` prefix → RightBackend (built-in tools, unprefixed)
  - `rightmeta__` prefix → Aggregator management (read-only: mcp_list)
  - `{server}__` prefix → ProxyBackend (forwarded to upstream MCP)

Internal REST API on Unix socket (~/.right/run/internal.sock):
  - POST /mcp-add — register external MCP server
  - POST /mcp-remove — remove external MCP server
  - POST /mcp-set-headers — replace stored HTTP header credentials for an
    external MCP server
  - POST /set-token — deliver OAuth tokens after authentication
  - POST /mcp-list — list MCP servers with status
  - POST /mcp-instructions — fetch MCP server instructions markdown
  - POST /progress/register — register an invocation for foreground progress,
    foreground learning, probe-writer learning, curator learning, or search scope
  - POST /progress/unregister — remove that invocation when the run ends

Telegram dashboard routes use InternalClient (hyper UDS) to call these
endpoints; the `/mcp` Telegram command only opens the dashboard MCP view.
Dashboard MCP management routes do not edit MCP config files or credential
stores directly.
Agents cannot reach the Unix socket from inside the sandbox.

## Invocation-scoped MCP tools

`mcp__right__send_progress` is a built-in RightBackend tool routed through the
aggregator. It is available only when the current MCP request carries
`X-Right-Invocation`, which the worker injects by writing a per-invocation MCP
config and uploading it into the sandbox before starting Claude Code.

The bot registers the invocation with the aggregator over
`/progress/register`. Foreground turns register as `Foreground`, probe-writer
runs as `ProbeWriter`, and curator runs as `Curator`; all learning-capable
invocations must use the per-invocation MCP config carrying
`X-Right-Invocation`. The registration stores the bot UDS path and a separate
send token. On `send_progress`, the aggregator validates that the invocation is
active, `Foreground`, and not within the 30-second per-invocation rate limit,
then calls the bot's `POST /progress/send` endpoint. Telegram send failures
surface as tool-level `progress_send_failed` errors.

Cron, delivery, reflection, and background-continuation calls pass
invocation-scoped tools (`mcp__right__send_progress`,
`mcp__right__skill_learning_start`, and
`mcp__right__skill_learning_finish`) via `--disallowedTools`. `send_progress`
is foreground-only. Learning tools are available only to registered
`Foreground`, `ProbeWriter`, and `Curator` invocations, so ordinary background
calls must use their structured output delivery path.

The learning prefilter is stricter: it omits MCP config and passes
`--tools ""`, so no MCP or Claude Code tools are available.

## Learned Skill MCP Tools

`mcp__right__skill_learning_start` and
`mcp__right__skill_learning_finish` are built-in RightBackend tools. They are
metadata/progress/receipt tools: the active agent writes skill package files
directly under `.claude/skills/<skill_name>/`; MCP validates the skill name,
records append-only `skill_learning_events`, updates mutable
`skill_lifecycle` rows on successful finishes, verifies successful finishes by
checking `.claude/skills/<skill_name>/SKILL.md`, and sends learning messages
only for `Foreground` invocations. `ProbeWriter` and `Curator` invocations
record lifecycle/events without Telegram learning-message delivery. In
OpenShell mode that existence check runs inside the sandbox; in `sandbox: none`
mode it checks the host agent directory. The receipt text is authored by the
LLM and passed as the `message` argument to
`mcp__right__skill_learning_finish`.

Create and update both require `rightx-*`. The learning flow never patches
custom/manual/hub/core/platform/bundled/codegen-owned non-`rightx-*` skills.

The removed Stage 2 background review is not a learning-capable invocation
kind and does not expose learning MCP tools.
