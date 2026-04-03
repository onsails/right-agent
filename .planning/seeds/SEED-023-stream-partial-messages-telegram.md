---
id: SEED-023
status: dormant
planted: 2026-04-03
planted_during: v3.1 (post-completion)
trigger_when: Telegram UX improvements or streaming features
scope: Small
---

# SEED-023: Stream partial messages to Telegram via --include-partial-messages

## Why This Matters

Currently `invoke_cc()` runs `claude -p` and waits for the full response (`wait_with_output`), then sends one Telegram message. For long-running agent tasks (code generation, multi-step reasoning), the user stares at "typing..." for minutes with zero feedback.

CC supports `--include-partial-messages` flag with `--output-format stream-json` which emits partial assistant messages as they're generated. We can stream these to Telegram by editing the message in-place (Telegram `editMessageText` API), giving users real-time visibility into what the agent is doing.

## When to Surface

**Trigger:** When working on Telegram UX, response latency, or streaming capabilities.

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches any of these conditions:
- Telegram bot UX improvements
- Streaming or real-time response features
- Agent response latency reduction
- User experience for long-running tasks

## Scope Estimate

**Small** — The pieces are straightforward:

1. Switch from `wait_with_output` to streaming stdout line-by-line
2. Parse `stream-json` events, filter for partial assistant messages
3. Debounce + `editMessageText` to update Telegram message in-place
4. Final message replaces partial with complete response (including reply tool output)

### Implementation Sketch

```
// Instead of:
cmd.arg("--output-format").arg("json");
let output = wait_with_timeout(child, CC_TIMEOUT_SECS).await?;

// Do:
cmd.arg("--output-format").arg("stream-json");
cmd.arg("--include-partial-messages");
// Read stdout lines, parse JSON events, send Telegram edits
```

### Design Decisions

- **Debounce interval**: Telegram rate-limits `editMessageText` (~30 msg/sec per chat). 1-2 second debounce should work.
- **Message splitting**: Telegram has 4096 char limit. If partial exceeds it, send new message or truncate with "...".
- **Error handling**: If stream breaks mid-way, send whatever we have + error suffix.
- **reply-schema compatibility**: `--json-schema` may not work with `stream-json` — need to verify. Final message in stream should still contain the structured reply tool call.

## Breadcrumbs

- `crates/bot/src/telegram/worker.rs:334-429` — `invoke_cc()` function (current non-streaming implementation)
- `crates/bot/src/telegram/worker.rs:405` — `Stdio::piped()` already set on stdout
- `crates/bot/src/telegram/worker.rs:422` — `wait_with_timeout` is the blocking call to replace
- teloxide `EditMessageText` API for in-place message updates
- CC flag: `--include-partial-messages` (with `--output-format stream-json`)

## Notes

- `--verbose` + `--output-format json` already produces stream-json array — but we intentionally avoid it (line 377 comment). `stream-json` output format is the correct approach.
- Teloxide's `Bot::edit_message_text()` is the method for updating messages in-place.
- Consider a "thinking..." indicator message sent immediately on receive, then edited with partial content as it arrives.
