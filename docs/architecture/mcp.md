# MCP Aggregator and token refresh

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## MCP Auth Choice Flow

`/mcp add <name> <url>` treats detection as advice, not authority. The bot
parses the original URL, derives a bare URL for probes, runs OAuth discovery
when the bare URL is public and has no query string, and runs auth-header
classification only when OAuth was not discovered. It then shows inline buttons
for `OAuth`, `Header`, and `URL as-is`, marking the recommendation.

No upstream MCP server is registered until the user clicks a button. `OAuth`
registers the bare URL as `auth_type=oauth` and asks the user to run
`/mcp auth <server>`. `Header` prompts for a token using the detected bearer or
custom-header recommendation; the user can override with `HeaderName: token`.
`URL as-is` registers the exact original URL without token/header injection,
preserving query-string credentials.

URL validation has two modes. Public detection accepts network-routable HTTP and
HTTPS URLs, excludes loopback/private/link-local hosts, and returns a short
`Plain HTTP: trusted/encrypted networks only.` warning when plain HTTP is
registered. Telegram renders that warning through `telegram::tg`, not as raw
`Warning:` prose. Explicit user-managed registration allows HTTP/HTTPS while
broad private/link-local ranges remain rejected.

## MCP Token Refresh

```
OAuth callback (bot) → POST /set-token to Aggregator (Unix socket)
  → Aggregator updates DynamicAuthClient.token in-memory
  → Aggregator saves token fields to mcp_servers SQLite table
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

`/mcp list` reports Aggregator backend status through the internal Unix-socket
API. It does not prove that a specific Claude Code process loaded the same MCP
tool registry. Agent turns are checked separately through Claude Code's
`system/init` stream-json event, which lists the MCP servers visible to that
process.

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
  - POST /set-token — deliver OAuth tokens after authentication
  - POST /mcp-list — list MCP servers with status
  - POST /mcp-instructions — fetch MCP server instructions markdown
  - POST /progress/register — register a foreground invocation that may send progress
  - POST /progress/unregister — remove that invocation when the foreground turn ends

Telegram bot uses InternalClient (hyper UDS) to call these endpoints.
Agents cannot reach the Unix socket from inside the sandbox.

## Foreground Progress Tool

`mcp__right__send_progress` is a built-in RightBackend tool routed through the
aggregator. It is available only when the current MCP request carries
`X-Right-Invocation`, which the worker injects by writing a per-invocation MCP
config and uploading it into the sandbox before starting Claude Code.

The bot registers the invocation with the aggregator over
`/progress/register`. The registration stores the bot UDS path and a separate
send token. On tool call, the aggregator validates that the invocation is
active, foreground-only, and not within the 30-second per-invocation rate
limit, then calls the bot's `POST /progress/send` endpoint. Telegram send
failures surface as tool-level `progress_send_failed` errors.

Cron, delivery, reflection, and background-continuation calls pass
foreground-only tools (`mcp__right__send_progress`,
`mcp__right__skill_learning_start`, and
`mcp__right__skill_learning_finish`) via `--disallowedTools`; they have no live
foreground invocation and must use their structured output delivery path.

## Learned Skill MCP Tools

`mcp__right__skill_learning_start` and
`mcp__right__skill_learning_finish` are built-in RightBackend tools. They are
metadata/progress/receipt tools: the active agent writes skill package files
directly under `.claude/skills/<skill_name>/`; MCP validates the skill name,
records learning events in `data.db`, verifies successful finishes by checking
`.claude/skills/<skill_name>/SKILL.md`, and sends foreground learning messages
through the existing bot UDS delivery path. In OpenShell mode that existence
check runs inside the sandbox; in `sandbox: none` mode it checks the host agent
directory. The receipt text is authored by the LLM and passed as the
`message` argument to `mcp__right__skill_learning_finish`.

Create and update both require `rightx-*`. The learning flow never patches
custom/manual/hub/core/platform/bundled/codegen-owned non-`rightx-*` skills.

Stage 2 background learned-skill review runs after `learning_episodes`
selection and is report-only. Background review invocations do not expose or
call `mcp__right__skill_learning_start` or
`mcp__right__skill_learning_finish`; those remain foreground learning protocol
tools. The reviewer records `skill_review_reports` and sends Telegram only for
high-confidence create/update candidates with `user_notice`. Candidate evidence
must cite at least one observable `msg:*` or non-thinking `exec:*` ref from the
selected episode. The reviewer prompt includes candidate decision rules:
candidates must be reusable across future sessions, one-off task narrative must
not become a skill, transient tool failures must not become persistent negative
claims, and existing `rightx-*` skills should be updated before creating new
candidates.
