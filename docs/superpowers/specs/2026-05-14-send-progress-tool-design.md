# Send Progress Tool Design

## Context

Issue #55 asks for an MCP tool that lets an agent send interim progress
updates during long-running Telegram tasks. Today users only receive the live
thinking anchor and the final response. The thinking anchor is system-owned:
it exists for Stop/Background controls and stream previews. It is not the
right surface for agent-authored progress.

The important constraint is that the current MCP request path authenticates
only the agent. It does not carry the active Telegram chat, topic, or Claude
session. A progress tool must not infer a target from agent-wide mutable state,
and it must not let the agent pass arbitrary `chat_id` or `thread_id`.

## Goals

- Add `mcp__right__send_progress` for agent-authored foreground progress.
- Deliver progress as separate Telegram messages in the current chat/topic.
- Support only the current foreground worker invocation.
- Deny progress from cron, background continuations, cron delivery, and
  reflection.
- Rate-limit accepted progress to one message per 30 seconds per invocation.
- Return tool-level errors for unavailable progress, rate limits, invalid
  input, and Telegram delivery failures.
- Teach agents in the system prompt to use progress only for complex or
  long-running work, not routine quick tasks.

## Non-Goals

- No `chat_id` or `thread_id` parameters.
- No cross-chat progress sends.
- No persisted progress state.
- No replacement for the final structured reply.
- No use of progress from cron jobs; cron output remains the structured
  `notify` contract.
- No attempt to auto-generate progress from every tool call or stream event.

## Chosen Approach

Use a per-foreground-invocation capability carried as an MCP HTTP header.

Before a foreground `claude -p` turn starts, the worker generates an
`invocation_id` and a random bot-send token. It registers the invocation with
the aggregator:

- agent name
- invocation id
- root session id
- target Telegram `chat_id`
- effective Telegram `thread_id`
- kind: `foreground`
- last accepted progress timestamp
- bot-send token

The worker also uses a turn-specific MCP config for that invocation. The `right`
HTTP MCP server entry includes the existing Bearer token and an additional
header:

```json
{
  "X-Right-Invocation": "<invocation_id>"
}
```

The tool shape is deliberately narrow:

```text
mcp__right__send_progress(message: string)
```

The aggregator authenticates the agent by Bearer token as it does today, reads
`X-Right-Invocation` from the MCP HTTP request, validates the active registry
entry, applies the rate limit, then asks the owning bot process to send the
message via its local Unix socket. The bot validates the bot-send token before
sending. The token is never exposed to the agent; the agent receives only the
MCP invocation header.

This avoids agent-wide "current target" state. Two foreground turns for the
same agent in different chats cannot leak progress into each other because the
invocation id is bound to one registered target.

## Components

### Aggregator

The aggregator owns the authorization decision for `send_progress` because it
already terminates MCP HTTP requests and resolves the agent from the Bearer
token.

Add a process-local progress registry keyed by `(agent, invocation_id)`.
Registry entries are inserted and removed through internal API endpoints used
by the bot:

- `POST /progress/register`
- `POST /progress/unregister`

The registry stores only delivery metadata and timestamps. It must not store
Telegram tokens or message contents beyond logs already emitted. It stores the
per-invocation bot-send token needed to authenticate the aggregator's
bot-socket call.

`RightBackend` exposes a new `send_progress` tool. The call receives a small
request context from the aggregator containing the authenticated agent and
optional invocation id. If that context is missing or invalid, the tool returns
`progress_unavailable`.

### Bot UDS Endpoint

The bot remains the only process that talks to Telegram. Add a local UDS route
on the per-agent bot socket:

- `POST /progress/send`

The request is accepted only when it includes the random bot-send token
registered for that active invocation. This protects the route even though the
bot socket also serves other local endpoints and the public tunnel routes only
selected paths.

The request includes:

- invocation id
- bot-send token
- Markdown message

The bot stores its own process-local active progress map keyed by invocation
id. That map contains the target chat/thread and token. The bot resolves the
Telegram target from this local map, not from the send request.

The bot converts Markdown through the existing Telegram Markdown-to-HTML path,
splits long HTML messages with the existing splitter, sends separate Telegram
messages to the registered chat/topic, and returns success or a structured
delivery error.

### MCP Config

The static generated `mcp.json` remains the normal per-agent config. Foreground
worker turns need a per-turn config overlay or temporary config file that is
identical except for the `X-Right-Invocation` header.

Cron, background continuations, delivery, and reflection continue using the
static MCP config and therefore never carry an invocation id.

### Claude Invocation Deny Layer

Server-side validation is the real boundary. In addition, cron, delivery, and
reflection invocations should add `mcp__right__send_progress` to
`ClaudeInvocation.disallowed_tools`.

Before implementation, verify that Claude Code accepts MCP tool names in the
`mcp__right__send_progress` format for `--disallowedTools`. If it does not,
keep the server-side boundary and document the limitation.

## Data Flow

Foreground worker:

1. Resolve or create the active session UUID.
2. Generate a fresh invocation UUID.
3. Generate a fresh bot-send token.
4. Store the invocation in the bot-local active progress map.
5. Register the invocation with the aggregator as `kind = foreground`.
6. Build the turn-specific MCP config with `X-Right-Invocation`.
7. Invoke `claude -p`.
8. Agent calls `mcp__right__send_progress({ "message": "..." })`.
9. Aggregator authenticates the Bearer token and reads the invocation header.
10. Aggregator validates registry entry, kind, ownership, and rate limit.
11. Aggregator calls the bot UDS progress endpoint with invocation id,
    bot-send token, and message.
12. Bot validates the token, resolves the target from its local active map, and
    sends a separate Telegram message to the registered chat/topic.
13. Worker unregisters the invocation from both bot and aggregator during
    cleanup after normal exit, stop, background handoff, timeout, or failure.

Cron, background continuation, delivery, and reflection:

1. Invocation starts without `X-Right-Invocation`.
2. `mcp__right__send_progress` either is denied by Claude Code or reaches the
   server without an invocation.
3. Aggregator returns `progress_unavailable`.

## Tool Behavior

Input:

- `message`: required string, non-empty after trimming, max 2000 characters
  before Markdown conversion.

Success response:

```json
{
  "status": "sent"
}
```

Tool-level errors:

- `progress_unavailable`: no invocation header, unknown invocation, wrong
  agent, wrong kind, already unregistered invocation, or bot rejection due to
  unknown invocation id or invalid bot-send token.
- `rate_limited`: last accepted progress for this invocation was less than 30
  seconds ago. Include `retry_after_secs`.
- `invalid_argument`: empty/whitespace message or message longer than 2000
  characters.
- `upstream_unreachable`: aggregator cannot reach the bot UDS.
- `telegram_send_failed`: bot UDS was reachable but Telegram delivery failed.

Rate limit is per invocation. There is no total count limit.

## Prompt and UX

Update normal-mode system instructions:

- Progress messages are available through `mcp__right__send_progress`.
- Use progress only for complex or long-running work: deep research,
  multi-step automation, sequential or parallel subagents, long tool chains, or
  tasks likely to take noticeable time.
- Do not send progress for routine quick tasks.
- Do not narrate every tool call.
- Keep progress short, factual, and user-facing.
- Do not call more often than every 30 seconds.
- Progress does not replace the required final response.

Update cron-mode instructions:

- Progress messages are unavailable in cron.
- Cron results must use the structured `notify` output contract.
- Do not attempt to send progress while executing cron or background
  continuation work.

Prompt and agent-facing references that mention Right MCP tools must use full
Claude Code tool names such as `mcp__right__send_progress`.

## Documentation

Update:

- `PROMPT_SYSTEM.md`
- `docs/architecture/mcp.md`
- `docs/architecture/sessions.md` if the invocation registry changes session
  or worker behavior materially
- `ARCHITECTURE.md` only if the architecture contract changes need a
  prescriptive rule

Also update `with_instructions()` in both:

- `crates/right/src/memory_server.rs`
- `crates/right/src/aggregator.rs`

## Error Handling

Errors must be explicit and observable.

The aggregator returns operation errors with the existing MCP tool-error
convention rather than infrastructure errors where possible. Infrastructure
failures that prevent dispatch can still surface as MCP internal errors, but
Telegram send failures should be mapped to `telegram_send_failed` so the agent
can report accurately.

The bot logs Telegram delivery failures with agent, invocation id, chat id, and
thread id. It must not log Telegram tokens. Message content may be included in
debug-level logs only if existing Telegram send paths already do so; otherwise
log length and metadata instead.

Registry cleanup is best-effort. Unregister in worker cleanup, and also expire
old entries conservatively so a crashed worker does not leave stale progress
capabilities active.

## Testing

Regression-first tests:

- Tool list includes `send_progress` with a valid object input schema.
- `send_progress` without invocation context returns `progress_unavailable`.
- Unknown or unregistered invocation returns `progress_unavailable`.
- Non-foreground invocation kind returns `progress_unavailable`.
- Second accepted call within 30 seconds returns `rate_limited` with
  `retry_after_secs`.
- Bot UDS connection failure maps to `upstream_unreachable`.
- Bot UDS rejects a missing or invalid bot-send token.
- Bot/Telegram delivery failure maps to `telegram_send_failed`.
- Foreground worker MCP config includes `X-Right-Invocation`.
- Cron, delivery, and reflection invocations attempt to disallow
  `mcp__right__send_progress`.
- Prompt tests assert normal-mode guidance for progress.
- Prompt tests assert cron-mode denial and structured `notify` guidance.

Verification:

- Targeted tests for `right`, `right-mcp`, and `right-bot` modules touched by
  the implementation.
- `cargo test --workspace` when feasible.
- Final `cargo build --workspace`.
