# Multi-message rich delivery + structured-output loop guard

**Date:** 2026-06-13
**Status:** Design approved, pending implementation plan

## Problem

Agent `agent-a` appeared to hang mid-turn. Root-cause investigation
(host stream NDJSON + in-sandbox transcript) showed it was not hung but
stuck in an invisible, unbounded retry loop.

The agent had generated 10 educational posts (image + caption each) and
needed to deliver them as 10 separate Telegram messages. Its reasoning:
*"Since I can only send one StructuredOutput, I'll send all 10 as
separate photo attachments."* It then submitted `StructuredOutput` with
an empty `{}` body **11 times in a row**, each rejected by schema
validation (`must have required property 'content', 'used_skill_receipts'`).
The loop survived a bot restart because `--resume` replays the same
pending-structured-output state.

Two distinct defects converged:

- **Trigger (capability gap):** the only way to emit multiple rich
  messages is one terminal `StructuredOutput` carrying an `attachments`
  array. That path is all-or-nothing, has no streaming, and forces the
  model to serialize a large payload (10 attachments x ~900-char
  captions). The model failed to build it and degenerated to `{}`.
  `mcp__right__send_progress` is text-only, rate-limited 1/30s, and
  cannot carry attachments.
- **Failure mode (no guard, no visibility):** the foreground worker sets
  `max_turns: None`, so nothing bounds the loop. The worker stream log
  surfaces only `thinking`/`text`/`Bash` blocks — `StructuredOutput`
  tool calls and their schema rejections are invisible, so the loop
  reads as a frozen turn to the operator.

This spec addresses both: a new delivery tool (A) and a loop guard with
observability (B).

## Scope

In scope: the `send_message` tool, the schema-rejection loop detector,
worker-stream visibility for `StructuredOutput`, and the reflection
notice for the new failure mode.

Out of scope: cron multi-send (cron already has notify-delivery with
attachments), changing the terminal `REPLY_SCHEMA_JSON` shape, and any
rework of `send_progress` semantics.

## Part A — `mcp__right__send_message`

### Purpose

Let a foreground agent emit standalone rich Telegram messages
incrementally during a turn, instead of cramming everything into the
terminal reply.

### Tool surface

Defined alongside `send_progress` in `crates/right/src/progress.rs`.

- Params: `SendMessageParams { content: Option<String>, attachments:
  Vec<OutboundAttachment> }`. `OutboundAttachment` is the existing DTO
  (`type`, `path`, `filename`, `caption`, `media_group_id`).
- Validation: at least one of `content` / `attachments` must be
  non-empty; otherwise the tool returns an error to the model (FAIL
  FAST, never silent).
- Length limits reuse the existing reply caps (caption 1024, text 4096).
- Per-turn cap: `MAX_SEND_MESSAGE_PER_TURN = 20`. Exceeding it returns a
  tool error. No 30s rate-limit — bursts are the point.
- Return value: the sent Telegram `message_id`(s), or a structured
  Telegram error propagated up.

One call = one logical message or media group, run through the existing
`partition_sends` logic.

### Wiring (reuse the progress channel)

The MCP server (`right_backend`) runs in the aggregator process; the
Telegram token lives in the bot process. `send_progress` already bridges
them over the bot's authenticated Unix-socket endpoint. `send_message`
reuses that channel:

1. `right_backend::call_send_message` POSTs `content` + `attachments`
   to a new bot UDS route `/message/send` (next to `/progress/send` in
   `crates/bot/src/telegram/progress.rs`), authenticated with the
   per-invocation `bot_send_token`.
2. The bot resolves `(chat_id, eff_thread_id)` from the registered
   foreground invocation target — **never from agent arguments** (same
   scope-enforcement rule as `send_progress` and the forum tools).
3. Gated to `ProgressInvocationKind::Foreground`; any non-foreground
   invocation gets a tool error.
4. Delivery runs the existing path: `partition_sends` -> `OutboundSend`
   -> Telegram send, resolving attachment paths from `/sandbox/outbox`
   exactly as terminal attachments do today.

### Turn closer

After its `send_message` calls, the agent emits `{content: null,
used_skill_receipts: [...]}`. The bot already treats null content as a
silent response (D-04, `worker_reply.rs`), so no empty or duplicate
terminal message is sent.

### Prompt teaching

Update `OPERATING_INSTRUCTIONS.md` and `TOOLS` guidance: to send several
standalone messages, call `send_message` once per message; the terminal
reply may be `null`. This removes the "I can only send one
StructuredOutput" misconception that caused the incident. Keep
`with_instructions()` in both `right_backend.rs` and `aggregator.rs` in
sync, and update `PROMPT_SYSTEM.md`.

## Part B — structured-output loop guard + visibility

### Detector

In the worker stream-processing loop (`crates/bot/src/telegram/worker.rs`):
count consecutive `is_error` tool_results whose body matches the
structured-output schema rejection (`Output does not match required
schema`). The counter resets on any successful tool_use or delivery.
After **3** consecutive rejections, abort: kill the CC subprocess and
route to `reflect_on_failure`.

### Visibility

Log `StructuredOutput` tool calls and their schema-rejection results in
the worker stream (today only Bash/text/thinking are logged). A line
such as `StructuredOutput rejected (schema)` turns the invisible hang
into an operator-visible signal on its own.

### Reflection notice

Add a reflection variant/notice in `crates/bot/src/reflection.rs`
explaining the structured-output failure, so the agent produces a
human-readable summary instead of silence. Because the reflection turn
resumes and delivers a summary, the next session state is no longer a
pending structured-output demand — this also resolves the
restart-survival behavior.

## Testing

- **A:** unit tests for `SendMessageParams` validation, the per-turn cap,
  the foreground gate, and `partition_sends` reuse; an integration test
  driving a tool call through to a mocked UDS/Telegram send.
- **B:** a unit test feeding a synthetic stream of 3 schema rejections
  and asserting abort + reflection; a test for the visibility log line.

Verification cadence per project rules: targeted package tests during
implementation, one full `cargo nextest run --workspace` plus
`cargo test --doc --workspace` at the end.

## Files touched

- `crates/right/src/progress.rs` — `SendMessageParams`, tool const,
  foreground gate, per-turn cap.
- `crates/right/src/right_backend.rs` — register + `call_send_message`,
  `with_instructions()`.
- `crates/right/src/aggregator.rs` — `with_instructions()` sync.
- `crates/bot/src/telegram/progress.rs` — `/message/send` route.
- `crates/bot/src/telegram/worker.rs` — loop detector, `StructuredOutput`
  logging, `send_message` delivery.
- `crates/bot/src/reflection.rs` — schema-loop reflection notice.
- Prompts: `OPERATING_INSTRUCTIONS.md`, `TOOLS`, `PROMPT_SYSTEM.md`.
- Docs: `ARCHITECTURE.md` + `docs/architecture/mcp.md` (new tool and
  scope rule), `docs/architecture/sessions.md` (detector).
