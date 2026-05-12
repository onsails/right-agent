# Design: Show Thinking Toggle

## Problem

Telegram thinking messages currently have a fixed visibility mode for each
agent run:

- Direct chats follow `show_thinking`.
- Group chats suppress live thinking and show a static `Working...` anchor.

That is too rigid. Users need a button on the running Telegram message that can
reveal the live event preview for the current run without changing persistent
agent configuration.

## Decision

Add a per-message thinking visibility toggle. The toggle affects only the active
Claude Code invocation for that chat/thread. It does not write `agent.yaml`, does
not hot-reload config, and does not change the default for future turns.

Initial state still depends on context:

| Context | Initial message | Toggle buttons |
| --- | --- | --- |
| Direct chat, `show_thinking: true` | Live event preview | `Hide thinking`, `Stop`, `Background` |
| Direct chat, `show_thinking: false` | Static `Working...` | `Show thinking`, `Stop`, `Background` |
| Group chat | Static `Working...` | `Show thinking`, `Stop`, `Background` |

Group chats remain quiet by default regardless of `show_thinking`. After a user
taps `Show thinking` in a group, the worker shows the live event preview for that
run. Group chats do not need a `Hide thinking` button after expansion; hiding is
not useful enough there to justify another group-chat control path.

## Components

### Worker Keyboard

Replace the current fixed `working_keyboard(chat_id, eff_thread_id)` helper with
a helper that receives a display mode:

- collapsed direct/group: `Show thinking`, `Stop`, `Background`
- expanded direct: `Hide thinking`, `Stop`, `Background`
- expanded group: `Stop`, `Background`

The existing stop and background callback data stay unchanged.

Thinking toggle callback data should be explicit and scoped to the active
chat/thread, for example:

```text
think:{chat_id}:{eff_thread_id}:show
think:{chat_id}:{eff_thread_id}:hide
```

### Active Visibility State

Add process-local state keyed by `(chat_id, eff_thread_id)` for the active run.
The state records whether the current thinking anchor is expanded or collapsed,
plus a monotonically increasing version number. Callback handlers bump the
version whenever they change visibility; the worker remembers the last rendered
version.

The worker initializes the state when it inserts the stop token:

- direct chat: `expanded = ctx.show_thinking`
- group chat: `expanded = false`

The worker removes the visibility entry when it removes the stop token.

### Callback Handler

Add a callback handler for `think:` callback data.

Behavior:

- Parse `think:{chat_id}:{eff_thread_id}:{show|hide}`.
- If no active run exists for that key, answer `Already finished`.
- If active, update the visibility state and answer with a short confirmation.
- Invalid callback data is answered without mutating state.

The handler does not send or edit Telegram messages directly. The worker owns the
thinking message lifecycle and applies the new visibility state on the next UI
tick. This avoids racing Telegram edits from two tasks and avoids waiting for a
new Claude stream event during long-running tool calls.

### Worker Rendering

When the worker receives a displayable stream event:

- If no thinking message exists, send either live preview or static `Working...`
  based on the active visibility state.
- If a thinking message already exists, update the ring buffer and usage state;
  rendering is handled by the UI update path below.
- Live preview uses the existing `format_thinking_message` path.
- Collapsed mode uses the existing static `Working...` anchor.
- Every edit preserves the appropriate keyboard for the current state.

The worker also owns a small periodic UI tick while Claude is running. On each
tick, if a thinking message exists, it edits the message when either condition is
true:

- the visibility state version changed since the last render
- the message is expanded and the existing two-second live-update throttle
  permits a refresh

This makes `Show thinking` visible promptly even when Claude is blocked inside a
long tool call and no new stream event is arriving.

On stop, background, timeout, reflection, and normal completion, keep the current
post-completion ownership rules. The only change is that final text selection
uses the active visibility state rather than only `ctx.show_thinking && !is_group`.

## Data Flow

```text
Worker starts run
  -> inserts stop token
  -> initializes thinking visibility for (chat_id, thread_id)

User taps Show thinking / Hide thinking
  -> Telegram callback query
  -> callback handler parses key and desired mode
  -> handler updates in-memory visibility state
  -> worker observes state on next UI tick
  -> thinking message switches between static anchor and live preview

Worker finishes run
  -> removes stop token
  -> removes visibility state
```

## Error Handling

- Callback after completion: answer `Already finished`.
- Malformed callback data: answer the callback and do not mutate state.
- Telegram edit failure: keep current best-effort behavior; log through existing
  paths where edits are already ignored.
- Bot restart during a run: existing process cleanup applies; in-memory visibility
  state is intentionally not persisted.

## Tests

Add focused unit tests around pure helpers and callback parsing/state:

- Keyboard helper renders the correct buttons for collapsed direct, expanded
  direct, collapsed group, and expanded group.
- Toggle callback data parses valid `show` and `hide` actions.
- Malformed toggle callback data is rejected.
- Visibility state starts from config in direct chats and starts collapsed in
  groups.
- Visibility state version increments when callbacks change the mode.
- Group expanded keyboard omits `Hide thinking`.

Worker integration with Telegram should stay at helper-level tests unless an
existing test harness can exercise message edits without network calls.

## Documentation

Update `docs/architecture/sessions.md` during implementation. It currently says
group chats suppress live thinking and direct chats follow `show_thinking`; after
this change it should describe the per-message toggle and the group default.
