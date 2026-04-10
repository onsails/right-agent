# Stream Logging + Live Thinking + Timeout Diagnostics

## Problem

CC subprocess timed out after 120s with zero diagnostics — no way to know what CC was doing. Logs show only "invoking claude -p" and "timed out". Need visibility into CC execution.

## Solution

Switch from `--output-format json` to `--verbose --output-format stream-json`. Read stdout line-by-line as events arrive. Show live "thinking" indicator in Telegram. Log full stream to per-session file.

## Stream Processing

### Worker reads stdout line-by-line

```
SSH stdout → BufReader::lines() → for each line:
  1. Write to stream log file (append)
  2. Parse as JSON, extract event type
  3. If assistant event → update ring buffer (last 10)
  4. If result event → save as final result
  5. Every 2s → edit Telegram thinking message with latest status
```

### Ring buffer

In-memory ring buffer of last 10 parsed events. Used for:
- Live thinking message updates
- Timeout diagnostics (dump to Telegram on timeout)

Not the full stdout — that goes to the stream log file only.

### Stream log files

Path: `~/.rightclaw/logs/streams/<session-uuid>.ndjson`

Raw NDJSON, one event per line. Written as events arrive (write-through, no buffering).
Useful for post-mortem debugging. Can be fed to tools later.

## Live Thinking Message

### Format

```
⏳ Turn 3/30 | $0.12
─────────────
🔧 Bash: curl https://composio.dev/docs
📝 "Изучаю документацию Composio..."
🔧 Read: /sandbox/mcp.json
📝 "Нашёл конфигурацию, проверяю..."
🔧 Bash: claude mcp add composio
```

- Header: turn counter (current/max), cost so far
- Body: last 5 events, formatted as icons + short description
- Updated via Telegram `editMessageText`

### Event formatting

| Event type | Icon | Format |
|-----------|------|--------|
| tool_use (Bash) | 🔧 | `Bash: <command truncated to 50 chars>` |
| tool_use (Read) | 📖 | `Read: <file_path>` |
| tool_use (Write/Edit) | ✏️ | `Edit: <file_path>` |
| tool_use (other) | 🔧 | `<tool_name>: <input truncated>` |
| text | 📝 | `"<text truncated to 60 chars>..."` |
| thinking | 💭 | `thinking...` |

### Rate limiting

Telegram edit API: max ~30 req/min per chat. Update thinking message at most once per 2 seconds. Buffer events between updates.

### Lifecycle

1. First assistant event arrives → send thinking message to Telegram
2. Subsequent events → edit thinking message (throttled to 2s)
3. Result event arrives → send final reply as NEW message, keep thinking message

Thinking message stays in chat (shows what agent did). Final reply is a separate message.

### Configuration

```yaml
# agent.yaml
show_thinking: true  # default: true
```

When `false`: no thinking message sent, stream still logged to file.

## CC Execution Controls

### Replace process timeout with CC-native controls

| Control | Flag | Default | Configurable |
|---------|------|---------|-------------|
| Max turns | `--max-turns` | 30 | `agent.yaml: max_turns` |
| Max budget | `--max-budget-usd` | 1.0 | `agent.yaml: max_budget_usd` |
| Process timeout (safety net) | worker-side | 600s | hardcoded |

CC-native controls produce a normal `type: "result"` with `terminal_reason` field (`"max_turns"`, `"max_budget"`). The process timeout is only for truly hung processes.

### agent.yaml additions

```yaml
max_turns: 30
max_budget_usd: 1.0
show_thinking: true
```

All three added to AgentConfig and `rightclaw agent init` wizard.

## Timeout Handling

### CC-native limit hit (max_turns / max_budget)

Normal result JSON with `terminal_reason`. Worker handles it like any response — sends content to Telegram. Thinking message already shows progress.

### Process timeout (600s safety net)

```
⚠️ Agent timed out (600s safety limit). Last activity:
─────────────
🔧 Bash: curl https://...
📝 "Fetching Composio docs..."
🔧 Read: /tmp/response.json

Stream log: ~/.rightclaw/logs/streams/<uuid>.ndjson
```

Dumps last 5 events from ring buffer + path to full stream log.

## CLI Flags Change

Before:
```
claude -p --output-format json ...
```

After:
```
claude -p --verbose --output-format stream-json --max-turns 30 --max-budget-usd 1.0 ...
```

## parse_reply_output — No Changes

The result line from stream-json has the same format as `--output-format json` output. Worker extracts the `type: "result"` line and passes it to `parse_reply_output` as-is.

## Files to Modify

| File | Change |
|------|--------|
| `worker.rs` | Stream reading loop, ring buffer, stream log writer, thinking message lifecycle, --verbose --output-format stream-json, --max-turns, --max-budget-usd, 600s safety timeout |
| `agent/types.rs` | `max_turns: Option<u32>`, `max_budget_usd: Option<f64>`, `show_thinking: Option<bool>` in AgentConfig |
| `init.rs` | Agent init wizard prompts for max_turns, max_budget_usd, show_thinking |
| `ARCHITECTURE.md` | Stream logging + thinking message section |
| `PROMPT_SYSTEM.md` | Update CC invocation flags (--verbose --output-format stream-json --max-turns --max-budget-usd) |
