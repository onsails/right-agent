# MCP Agent-Facing Self-Heal

## Context

Telegram `/mcp list` reports Aggregator backend health through the internal API. Agent turns use a separate path: `claude -p` starts inside the sandbox with `--mcp-config /sandbox/mcp.json --strict-mcp-config`, then Claude Code emits a `system/init` event with the MCP servers it actually loaded.

The observed failure was a split-brain state:

- Aggregator showed `right`, `composio`, and `browser-use` as connected.
- Claude Code `system/init` showed `right` as `needs-auth`.
- Claude Code exposed only MCP auth helper tools, so the model concluded that Composio and other tools were unavailable.
- The sandbox contained `.claude/mcp-needs-auth-cache.json` with a stale `right` entry.

The platform must repair this automatically. Operators must not delete sandboxes, recreate sessions, or manually edit MCP config files.

## Goals

- Periodically verify the same agent-facing MCP path used by real turns.
- Reuse the existing Haiku keepalive loop instead of adding a second scheduler.
- Repair stale Claude Code MCP auth cache without changing the user's Claude session.
- Let a user turn that observes the problem trigger the same repair path asynchronously.
- Keep `/mcp list` as Aggregator status, but stop treating it as proof that Claude Code loaded the same MCP tools.

## Non-Goals

- No fresh Claude session creation.
- No automatic retry or interruption of the current user turn.
- No sandbox deletion or recreation.
- No manual credential repair.
- No Telegram noise for successful background repair.

## Approach

Replace the current token-only keepalive ping with a combined Claude health probe. The loop still uses Haiku and still keeps Claude OAuth warm, but it now starts Claude Code with the same MCP config surface as real agent turns and parses the first `system/init` event.

Recommended command shape:

```text
claude -p
  --model haiku
  --max-turns 1
  --no-session-persistence
  --mcp-config /sandbox/mcp.json
  --strict-mcp-config
  --output-format stream-json
  -- "Reply exactly OK. Do not use tools."
```

For non-sandbox agents, use the existing local MCP config path from `mcp_config_path()`.

If `right` is connected, the probe lets Haiku complete the short response so the old keepalive behavior still happens. If `right` is `needs-auth`, missing, or otherwise unhealthy, the probe stops early and triggers repair.

## Components

### Stream Init Parser

Add a parser near the existing stream helpers:

- Input: one Claude Code stream-json line.
- Only handles `{"type":"system","subtype":"init",...}`.
- Reads `mcp_servers`.
- Returns the status for server `right`.
- Treats missing `right` as unhealthy.

This parser is shared by the health probe and Telegram worker observation.

### Claude Health Loop

Conceptually rename the current keepalive module to Claude health. The implementation may keep the file name if the patch is smaller, but the module responsibility becomes broader:

- periodic hourly probe;
- immediate startup probe after initial sandbox sync;
- Haiku token keepalive;
- agent-facing MCP init check;
- repair trigger on unhealthy `right`.

The loop must not log secrets. It should log agent name, sandbox name, `right` status, probe attempt, and repair outcome.

### Repair Path

Repair is a bounded operation protected by a per-agent mutex or atomic in-flight flag:

1. Remove stale Claude Code cache:
   - sandbox: `/sandbox/.claude/mcp-needs-auth-cache.json`
   - no-sandbox: `<agent_dir>/.claude/mcp-needs-auth-cache.json`
2. Run the existing platform sync path so `/sandbox/mcp.json` and related symlinks are current.
3. Repeat the health probe once.
4. If the second probe is healthy, mark an in-memory "MCP repaired" flag.
5. If the second probe is still unhealthy, log a platform health failure and stop.

Repair never deletes a sandbox and never edits external MCP credentials directly.

### User Turn Observation

During normal Telegram worker stream processing, inspect `system/init` with the same parser.

If the current user turn sees unhealthy `right`:

- do not kill Claude;
- do not retry the turn;
- do not change the session;
- do not alter the user's response flow;
- asynchronously trigger the same repair path used by the health loop.

This accepts that the current response may still say tools are unavailable. The fix targets future `claude -p` processes.

### Next-Turn Notification

After a successful repair, the next normal user turn should receive a short system notification in the prompt:

```text
Right MCP stale needs-auth cache was repaired. Use current MCP tool availability, not previous disconnected status.
```

This notification is one-shot and in-memory. It does not fork, replace, or clear the Claude session.

## Data Flow

Healthy path:

1. Timer fires.
2. Haiku health probe starts with strict MCP config.
3. Probe reads `system/init`.
4. `right = connected`.
5. Probe completes short `OK`.

Background repair path:

1. Timer or startup probe sees `right = needs-auth`.
2. Health loop stops the probe.
3. Repair deletes the stale MCP needs-auth cache.
4. Repair runs platform sync.
5. Repair repeats probe once.
6. Success sets next-turn notification flag; failure logs health failure.

User-turn observation path:

1. User message starts normal `claude -p --resume`.
2. Worker reads `system/init`.
3. If `right` is unhealthy, worker schedules repair in the background.
4. Current stream continues unchanged.
5. Later turns benefit from the repaired cache.

## Error Handling

- Probe spawn failure: warn and keep the loop alive.
- Missing `system/init`: warn; do not repair unless a clear unhealthy MCP init is observed.
- Cache removal failure: warn with path and error; continue to sync and probe once if possible.
- Sync failure: error; do not retry more than once in the same repair trigger.
- Second unhealthy probe: error; leave future timer ticks able to retry.
- Concurrent triggers: collapse into one repair run.

## Testing

- Unit tests for parsing `system/init`:
  - `right = connected`;
  - `right = needs-auth`;
  - missing `right`;
  - non-init lines ignored.
- Unit tests for health decision:
  - healthy init does not repair;
  - unhealthy init triggers exactly one repair;
  - second failure does not loop.
- Unit test for concurrent repair trigger collapsing.
- Worker stream observation test:
  - unhealthy init schedules repair;
  - current stream handling is not interrupted.
- Keep existing keepalive interval behavior unless intentionally changed.

Final implementation verification:

```text
devenv shell -- cargo test --workspace
```

## Acceptance Criteria

- A stale `.claude/mcp-needs-auth-cache.json` entry for `right` is repaired by the periodic Haiku health loop.
- A normal user turn that observes unhealthy `right` triggers repair without retrying or replacing the session.
- `/mcp list` can remain Aggregator-facing and green while the health loop independently repairs Claude Code's agent-facing cache.
- No secrets are printed in logs.
- No sandbox or session is deleted or recreated.
