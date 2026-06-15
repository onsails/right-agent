# Focus update notification - design

- **Date:** 2026-06-15
- **Status:** Approved for planning
- **Scope:** Small behavior change on the existing operator focus save path.

## Goal

When an operator saves conversation focus through the `/set_focus` Mini App, the
bot sends a short confirmation message into the same Telegram conversation scope
whose focus was changed. This gives the group, topic, or DM an auditable signal
that standing context changed.

Telegram bots cannot create native Telegram service events, so this feature
sends a normal bot message that is service-like in wording.

## Current behavior

`/set_focus` is only a launcher. In DMs it sends the Mini App button directly;
in groups and topics it sends a deep-link button that bounces through DM before
opening the Mini App. The actual write happens later in
`crates/bot/src/telegram/dashboard/focus.rs::handle_update`, which calls
`right_db::thread_focus::set_operator`.

The notification belongs on the successful `PATCH /dashboard/{agent}/api/v1/focus`
path, not on `/set_focus`, because `/set_focus` does not prove that focus was
saved.

## Non-goals

- No notification when merely opening `/set_focus`.
- No agent-facing MCP notification for `mcp__right__thread_focus_set`; this
  feature covers operator-set focus from the Mini App only.
- No native Telegram service event; the Bot API does not support that.
- No retry queue or durable outbox in this iteration.
- No prompt, memory, or focus data-model changes.

## Recommended approach

After `handle_update` validates auth and scope token, trims the submitted
operator focus, enforces the existing length cap, and successfully writes the DB
row, it sends a Telegram message to `(chat_id, thread_id)` from the request:

- non-empty value: `Focus set: <trimmed focus>`
- cleared value: `Focus cleared`

`thread_id == 0` sends to the chat normally. Non-zero thread ids use
`message_thread_id`, matching existing bot delivery paths.

## Alternatives considered

**Send from `PATCH /focus` after DB write.** This is the selected design. It is
the only point where the server knows the persisted value and scope.

**Queue a best-effort async notification.** This would keep the Mini App save
response fast, but Telegram failures would be visible only in logs. That is a
poor fit for a user-visible confirmation.

**Send from `/set_focus`.** This is wrong because the command only opens the UI.
It would announce a focus change even if the user closes the Mini App or the
save fails.

## Components

Add a small notification boundary to dashboard state rather than calling the
network directly from route tests:

- production state carries a notifier backed by the existing `BotType` clone;
- tests use a fake notifier that records attempted sends and can simulate
  failure;
- the focus route depends on the notifier abstraction, not on a live Telegram
  API call.

This keeps behavior testable without a real Bot API request. The production
notifier should reuse existing Telegram send conventions: target chat id,
optional `message_thread_id`, bounded send timeout if the local helper already
provides one, and structured warning logs on failure.

## Error handling

DB write remains authoritative:

- if validation or DB write fails, no notification is attempted;
- if DB write succeeds and notification succeeds, return the existing success
  JSON shape;
- if DB write succeeds but Telegram delivery fails, return an error response
  with explicit detail such as `Focus saved, but notification could not be sent`
  and log the underlying Telegram error.

The DB write is not rolled back after a send failure. Telegram delivery is an
external side effect and rollback would make the saved UI state harder to reason
about.

## Testing

Use TDD for implementation:

1. Add focused dashboard route tests that prove successful save sends the fake
   notification to the requested `chat_id` and `thread_id`.
2. Add a focused clear test that sends `Focus cleared`.
3. Add a failure test where the fake notifier fails after the DB write; assert
   the API reports notification failure and the DB row still contains the saved
   focus.
4. Keep existing tests for trimming, clearing, scope-token rejection, and length
   cap intact.

Implementation verification should run the targeted bot dashboard tests first.
Final code completion still requires the workspace commands mandated by
`AGENTS.md`: `devenv shell -- cargo nextest run --workspace` and
`devenv shell -- cargo test --doc --workspace`.

## Documentation impact

`ARCHITECTURE.md` should not change unless implementation introduces a new
contract or invariant. The existing focus architecture remains intact: operator
focus is still written by the dashboard and injected into the system prompt on
foreground turns.

