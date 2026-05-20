# Design: Immediate Working Anchor

## Problem

Foreground Telegram turns can sit visibly idle before Claude Code emits its
first displayable stream event. The worker currently sends the `Working...`
thinking anchor only inside the stream-event branch. When Claude has a long
time-to-first-token, especially after a cache miss on a large resumed session,
the user sees no durable Telegram status or Stop/Background controls.

Observed case:

- User message archived at `2026-05-20T20:46:25Z`.
- `claude -p` invoked at `2026-05-20T20:46:26.408Z`.
- First thinking event logged at `2026-05-20T20:47:06.817Z`.
- Result reported `ttft_ms=40033`, `cache_creation_input_tokens=190890`,
  `cache_read_input_tokens=0`.

The platform routing path was fast. The visible delay came from upstream Claude
TTFT plus the worker waiting for a displayable stream event before creating the
Telegram anchor.

## Decision

Create the Telegram `Working...` anchor immediately after the Claude subprocess
is successfully started, stdin is written, the stop token is registered, and
thinking visibility state is initialized.

This preserves the debounce and batching behavior. The anchor represents a real
running Claude invocation, not a message still waiting in preflight.

## Alternatives Considered

### A. Anchor After Claude Start

This is the selected approach. It usually appears about 0.5-1 second after the
Telegram update, once debounce and invocation setup complete. Stop and
Background controls are valid as soon as the anchor appears.

Trade-off: attachment download, prompt assembly, and subprocess spawn still
happen before the anchor. That is acceptable because this design targets the
long idle gap after a real Claude invocation starts.

### B. Anchor Before Claude Start

This would be slightly faster but would create false UI for work that may still
fail in attachment download, prompt assembly, session lookup, or subprocess
spawn. It also complicates cleanup for errors that happen before stop/background
state exists.

### C. Keep Only Typing Action Until First Stream Event

This keeps chat quieter but fails the user-visible status requirement. Telegram
typing actions are transient and do not expose Stop or Background controls.

## Components

### Worker Anchor Helper

Add a small helper near the existing thinking UI helpers in
`crates/bot/src/telegram/worker.rs`.

Responsibilities:

- Render the initial text with existing `thinking_anchor_text`.
- Build the keyboard with existing `working_keyboard`.
- Send to the correct chat and topic.
- Return `Option<MessageId>`.
- Log Telegram send failures and continue the run.

The helper must not create new state. It consumes the visibility state and ring
buffer already owned by `invoke_cc`.

### Invocation Flow

Adjust `invoke_cc` in `crates/bot/src/telegram/worker.rs`:

1. Spawn `claude -p`.
2. Write stdin and close stdin.
3. Insert the stop token.
4. Initialize thinking visibility.
5. Create the initial anchor immediately.
6. Start reading stdout line by line.
7. On displayable stream events, update the ring buffer and edit the existing
   anchor through the current UI path.

If the anchor send fails, `thinking_msg_id` remains `None`. The stream loop may
retry anchor creation on the first displayable event using existing behavior.

### Stream Loop

The stream loop should no longer rely on the first displayable event to create
the normal anchor. It should still support fallback creation when the immediate
send failed.

Completion behavior stays the same:

- Expanded successful runs edit the final thinking summary and remove keyboard.
- Collapsed successful runs delete the anchor.
- Stopped runs edit the anchor with stopped status.
- Timeout, reflection, and background handoff keep their existing ownership
  rules.

### Observability

Add lightweight logging for result timing and cache shape when parsing the final
result event:

- `duration_ms`
- `duration_api_ms`
- `ttft_ms`
- `cache_creation_input_tokens`
- `cache_read_input_tokens`
- `input_tokens`
- `output_tokens`
- cache miss reason when present

This should be best-effort logging only. Missing fields are normal across Claude
Code versions and must not fail the run.

## Data Flow

```text
Telegram message
  -> worker debounce closes
  -> worker invokes Claude
  -> stop token and visibility state are registered
  -> Telegram anchor is sent as Working...
  -> Claude stream emits events later
  -> worker edits the same anchor with live preview or final state
```

## Error Handling

- Telegram send failure for the immediate anchor: log and continue.
- Missing stdout handle after spawn: existing error path remains responsible for
  finishing progress state and returning an error.
- Stop/Background click after anchor creation: existing token and request maps
  remain authoritative.
- Subprocess spawn failure: no anchor is created, because no real invocation
  exists yet.
- Result JSON missing timing/cache fields: omit those fields from the log.

## Tests

Implementation should follow the existing worker helper-test style.

Focused tests:

- Initial anchor text remains `Working...` in collapsed mode.
- Initial anchor text uses `format_thinking_message` in expanded mode.
- The worker stream event path does not create a second anchor when
  `thinking_msg_id` already exists.
- The fallback stream-event path can still create an anchor when the immediate
  send failed.
- Result timing/cache extraction tolerates missing fields.

Verification cadence:

- Run the narrowest useful `right-bot` test filter during implementation.
- Run `devenv shell -- cargo test -p right-bot` after the coherent feature
  slice.
- Before declaring implementation complete, run
  `devenv shell -- cargo test --workspace`.

## Documentation

Update `docs/architecture/sessions.md` during implementation. It currently
describes thinking messages but not the immediate post-spawn anchor timing. The
doc should state that foreground workers create the anchor after a real Claude
subprocess starts, before the first stream event arrives.
