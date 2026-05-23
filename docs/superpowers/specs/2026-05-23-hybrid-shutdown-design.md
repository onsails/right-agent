# Hybrid Shutdown Design

## Context

`Ctrl+C` currently behaves as process shutdown for the bot. The Telegram dispatcher
stops, active worker tasks are dropped, and their `ProcessGroupChild` guards kill
the in-flight Claude process group. This is operationally clean but user-hostile:
Telegram can be left with a stale `Working...` anchor, active foreground work is
lost, and cron output may never be delivered if shutdown interrupts the wrong
phase.

Right Agent already has most of the primitives needed for a better shutdown:

- Foreground Telegram turns can be converted into `async_runs` background
  continuations through the same path as the `Background it` button.
- Cron jobs persist `async_runs kind='cron'` rows before launching Claude.
- Async delivery can relay pending `async_runs` notifications to Telegram.

The missing behavior is coordinated shutdown policy across these pieces.

## Goal

On `SIGINT` or `SIGTERM`, shutdown should be graceful from Telegram's point of
view without making the operator wait for arbitrary model/tool runtimes.

The selected policy is hybrid shutdown:

1. Stop accepting new Telegram work immediately.
2. Convert active foreground Telegram turns into background continuations.
3. Let already-running cron jobs drain for a bounded timeout.
4. Flush already-complete Telegram deliveries once, bounded.
5. Persist unfinished work as explicit interrupted/failed state for restart-time
   recovery and notification.

## Non-Goals

- Do not implement an unbounded "drain until everything finishes" shutdown for
  terminal `Ctrl+C`.
- Do not change the normal `Stop` button semantics.
- Do not change Claude Code invocation arguments except where needed to reuse
  existing background-continuation behavior.
- Do not introduce manual sandbox recovery steps.

## Foreground Telegram Turns

Foreground turns currently install a per-turn `stop_token` and the Telegram
`Background it` callback uses that token to request background handoff. Shutdown
should use equivalent semantics, but with a distinct reason so logs and banners
are clear.

Behavior:

- `run_telegram` receives the process shutdown signal and stops dispatching new
  Telegram updates.
- Active workers receive a shutdown/background request instead of being dropped
  immediately.
- Each active `invoke_cc` creates an `async_runs kind='background'` row before
  killing the foreground Claude process.
- The worker starts the background continuation using `--resume <main_session_id>
  --fork-session --session-id <run_id>`.
- The handoff is considered successful only after the background continuation
  emits a matching `system/init`.
- The thinking anchor is edited to a shutdown-specific banner:
  `Shutting down - continuing in background...`.
- If handoff setup fails, the thinking anchor is edited to a failure banner and
  the background row is marked failed with pending delivery.

This should reuse the existing background handoff machinery rather than creating
a parallel implementation.

## Cron Jobs

Cron schedulers must stop creating new runs during shutdown. Jobs already
running keep the existing bounded drain behavior, currently 60 seconds via
`SHUTDOWN_JOB_TIMEOUT`.

Behavior:

- Running cron jobs that finish inside the timeout persist output normally.
- Completed notify outputs remain eligible for shutdown delivery flush.
- Running cron jobs that exceed the timeout are marked failed/interrupted, not
  left as ambiguous success and not silently abandoned.
- The failure payload should identify shutdown interruption explicitly, include
  the run id and job name, and preserve the log path when available.
- If the cron run has a target chat, delivery remains pending so the user can be
  notified either during shutdown flush or after restart.
- Lock files are removed only when the job is terminal or explicitly marked
  interrupted by shutdown.

The implementation must be careful not to consume a `JoinHandle` through
`tokio::time::timeout` in a way that prevents explicit abort/marking.

## Async Delivery

Normal async delivery respects chat-idle politeness. Shutdown is different: the
process is exiting, so already-complete notifications should be flushed once
without waiting for the idle threshold.

Behavior:

- The regular delivery loop stops polling after shutdown begins.
- A shutdown-only final flush selects already-pending deliverable rows.
- The final flush ignores idle-delay politeness.
- The final flush is bounded by a short timeout.
- Failed sends are not treated as delivered; rows remain pending or retryable
  for the next boot according to existing delivery state rules.

This flush is only for rows that are already terminal and delivery-ready. It must
not wait for foreground background continuations or cron jobs that have not
reached terminal state.

## Recovery

Startup recovery should continue to repair states that can be left behind by
abrupt exits.

Required states:

- Background rows stuck at `status='queued'` and `handoff_state='queued'` remain
  recoverable through existing interrupted-handoff recovery.
- Background rows that failed during shutdown handoff are terminal failed rows
  with pending delivery.
- Cron rows interrupted by shutdown are terminal failed rows with explicit
  shutdown error payload.
- Cron rows still `running` after an ungraceful kill are not solved by this spec
  unless implementation can prove process ownership. Do not infer stale running
  recovery from timestamps alone.

## Observability

Logs should distinguish shutdown paths from user Stop, user Background, timeout,
and subprocess failure:

- signal received and shutdown mode entered
- foreground handoff requested by shutdown
- foreground handoff confirmed or failed
- cron drain started, finished, timed out, or interrupted
- delivery shutdown flush started and completed

Telegram banners should be concise and avoid implying completion before the
background or cron result is actually available.

## Testing

Use targeted tests while implementing and a full workspace test before claiming
completion.

Required tests:

- Unit test foreground shutdown classification: shutdown maps to background
  handoff, not Stop.
- Unit test shutdown banner text and reason selection.
- Unit test cron interrupted-row persistence, including status, error payload,
  and delivery status when target chat is known.
- Unit test delivery shutdown flush selection ignores idle delay but only sends
  terminal delivery-ready rows.
- Async integration-style test with fake handles/channels for shutdown ordering:
  dispatcher stops accepting work, foreground handoff is requested, cron drain is
  bounded, delivery flush runs after terminal rows are available.

Final verification:

```bash
devenv shell -- cargo test --workspace
```

## Documentation Updates

Implementation must update:

- `docs/architecture/sessions.md` for foreground shutdown/background handoff,
  cron interruption, and delivery flush semantics.
- `docs/architecture/lifecycle.md` for the `right bot` shutdown flow.
- `ARCHITECTURE.md` only if the implementation changes prescriptive contracts or
  module ownership boundaries.
