# Telegram Media Group Aggregation Design

**Date:** 2026-04-27
**Status:** Draft

## Problem

When a user sends multiple files (photos / documents / videos) in one Telegram
"album", Telegram delivers them as **N separate `Message` updates** that share
a `media_group_id`. The caption (and therefore any `@bot` mention entity) sits
on **exactly one** of those messages — typically the first.

The bot's group-chat routing filter
(`crates/bot/src/telegram/filter.rs:35`) demands that every individual message
be addressed to the bot (mention / reply-to-bot / `BotCommand` entity). The two
caption-less siblings fail that check and are dropped silently in the
"group non-mention dropped" branch. Only the message bearing the caption
reaches `handle_message`.

Concrete reproduction from `~/.right/logs/him.log.2026-04-27`:

```
16:03:55.337156  text_preview=None  entities=0   ← attachment, no caption (dropped at filter)
16:03:55.337566  text_preview=None  entities=0   ← attachment, no caption (dropped at filter)
16:03:55.337878  text_preview="@rightaww_bot ..."     ← attachment + caption (routed)
...
16:04:04 📖 Read /sandbox/inbox/document_335_0.png    ← agent sees only one file
```

Agent later replies to user: "у меня только одно письмо от Vonder…". Two
attachments were lost.

The Telegram Bot API exposes no group size and offers no atomicity or ordering
guarantee — the standard solution across all known wrappers is a short
timeout-based aggregator on the receiving side.

## Goal

When a user sends a media group addressed to the bot, the agent must receive
**all** siblings in the same logical turn. Privacy guarantees for non-addressed
group traffic must be preserved.

## Non-goals

- Aggregating media groups across long pauses ("send 3 photos now, send 3 more
  in 30 seconds, treat as one message"). One CC turn per media group.
- Aggregating media groups across CC executions ("send media group A, while CC
  runs send media group B, treat A+B as one input"). Each batch is one CC turn.
- Changing privacy semantics for ordinary text messages in open groups —
  `@mention` / reply / command remains required for non-media-group traffic.
- Inbound voice/video_note merging — those are not media groups in the
  Telegram sense.

## Scope

In scope:

- `crates/bot/src/telegram/filter.rs` — relax the addressed-check for
  media-group siblings only.
- `crates/bot/src/telegram/handler.rs` — propagate `media_group_id` and the
  optional address kind into `DebounceMsg`.
- `crates/bot/src/telegram/worker.rs` — adaptive debounce window when the
  current batch contains any media-group sibling, plus a final
  "addressed?" gate before invoking CC.

Out of scope:

- Outbound `sendMediaGroup` — already implemented in `attachments.rs`.
- Cron / login flow — neither receives Telegram media groups.

## Design

### Routing filter changes

`RoutingDecision.address` becomes `Option<AddressKind>`. The filter still
returns `Option<RoutingDecision>` and the **trust gate is unchanged**: a
sender that is neither in the trusted-users list nor in an open group is
dropped immediately, exactly as today.

For trusted-or-open group chats the new logic is:

```text
if msg.media_group_id is Some:
    address := is_bot_addressed(msg, identity)   // Option<AddressKind>
    return Some(RoutingDecision { address, sender_trusted, group_open })

else (no media_group_id):
    addr := is_bot_addressed(msg, identity)
    if addr is None: return None                 // today's behaviour
    return Some(RoutingDecision { address: Some(addr), sender_trusted, group_open })
```

DM behaviour is unchanged.

The net effect: media-group siblings without their own mention slip through
the filter, but only inside groups where the sender is already permitted.
A trusted user typing a plain text message in an open group without an
`@mention` is still dropped.

### `DebounceMsg` shape

Two field changes in `crates/bot/src/telegram/worker.rs`:

- `address: AddressKind` → `address: Option<AddressKind>`
- new field `media_group_id: Option<String>` (extracted in `handle_message`)

`handle_message` (`crates/bot/src/telegram/handler.rs:293`) populates both
fields from `msg.media_group_id` and `decision.address`.

### Adaptive debounce window

Today the worker uses a fixed 500 ms window from the first message. Telegram
delivers media-group siblings in 100–500 ms typically but can be slower
under load, so a fixed-500 ms window can split a single album across two
batches.

New behaviour in `worker.rs`:

- **No media-group sibling in batch:** unchanged — fixed 500 ms from first
  arrival. Single-message latency is preserved.
- **Any media-group sibling present (including arrivals after the first
  message):** switch the inner loop to *idle timeout* mode — close the
  batch when no new message has arrived for 1000 ms, with a hard cap of
  2500 ms from the first arrival.

Pseudocode for the inner debounce loop:

```text
let mut hard_deadline = first_arrival + 2500ms
let mut idle_deadline = first_arrival + 500ms       // initial = today's window
let mut media_group_seen = first.media_group_id.is_some()

loop {
    let now_deadline = if media_group_seen {
        min(idle_deadline, hard_deadline)
    } else {
        idle_deadline
    }
    select! {
        msg = rx.recv() => {
            push msg into batch
            if msg.media_group_id.is_some() {
                media_group_seen = true
            }
            if media_group_seen {
                idle_deadline = now() + 1000ms
            }
            // non-media-group: idle_deadline unchanged (still 500 ms from first)
        }
        _ = sleep_until(now_deadline) => break,
    }
}
```

The hard cap (2500 ms) prevents a slow / pathological producer from holding
the worker indefinitely.

### Post-debounce address gate

After the window closes, before downloading attachments and invoking CC, the
worker checks the assembled batch:

```text
if is_group(batch) && batch.iter().all(|m| m.address.is_none()):
    log debug "media-group batch had no addressed sibling, dropping"
    continue (next debounce cycle)
```

`is_group` is the already-computed `ChatContext::Group { .. }` discriminator.
DM batches always have `address = Some(DirectMessage)` so the gate trivially
passes for them.

This means an unaddressed media group from a trusted user in an open chat
hits the worker, gets aggregated, and is dropped without invoking CC —
zero subprocess cost, one log line.

### What this fixes

| Scenario | Before | After |
|---|---|---|
| Album of 3 files with `@bot` caption on item 1 | 1 file delivered | All 3 in one CC turn |
| Album with no caption + follow-up text `@bot ...` | Only the text | Album + text in one CC turn |
| Text `@bot ...` + follow-up album with no caption | Only the text | Text + album in one CC turn (if within 2.5 s) |
| Trusted user typing plain text in open group | Dropped | Dropped (privacy preserved) |
| Trusted user posting a 10-photo album with no mention | Dropped at filter | Dropped at worker after aggregation, no CC |
| Single text with `@mention` | 500 ms debounce | 500 ms debounce (unchanged) |

### Interaction with existing queue / backpressure

The worker's `mpsc::channel(32)` already serialises all input for a
`(chat_id, thread_id)` SessionKey. Variant C does not change this:

- A solo addressed file arriving **inside** the debounce window joins the
  current batch (existing behaviour generalised by the wider window).
- A solo addressed file arriving **while CC is running** sits in the
  channel and is picked up on the next `rx.recv()` after CC returns —
  exactly as today.
- An unaddressed solo file is still dropped at the filter, never reaching
  the queue.

No new races, no lost messages, no out-of-order processing.

## Testing

Unit tests, all in existing files:

- `filter.rs::tests`
  - media-group sibling without mention in trusted/open group → `Some(_)` with `address: None`
  - media-group sibling without mention from untrusted sender in non-open group → `None`
  - non-media-group group message without mention → `None` (regression guard for privacy)
  - DM unchanged

- `mention.rs::tests` — no changes; existing coverage already exercises
  `is_bot_addressed` independently.

- `worker.rs` debounce — extract the batching loop into a pure function that
  consumes a `Receiver<DebounceMsg>` and a `now()`-injectable clock so the
  following can run with `tokio::time::pause()`:
  - 3 messages, all `media_group_id = Some("g")`, arrivals at 0/200/400 ms →
    one batch of 3, closes at ≈ 1400 ms (idle 1000 from last arrival).
  - 4 messages, `media_group_id = Some("g")`, arrivals at 0/600/1200/1800
    ms → one batch of 4 (hard cap not hit).
  - 5 messages spaced 700 ms each with `media_group_id = Some` → batch
    closes at hard cap 2500 ms; remaining message becomes batch 2.
  - 2 messages without `media_group_id`, second arrives at 800 ms → two
    batches (regression guard against widening for non-album traffic).
  - Mixed: text without `media_group_id` at T=0, album sibling at T=200 ms
    → batch widens at T=200, closes at idle of last arrival.

- Worker post-debounce gate:
  - Batch of 3 in group, all `address: None` → no CC invocation, debug
    log "dropped".
  - Batch of 3 in group, one `address: Some(_)` → CC invoked.
  - Batch of 1 in DM with `address: Some(DirectMessage)` → CC invoked
    (regression guard).

Regression test for the original Vonder bug — purely at the filter level
since the worker batch path is covered by the unit tests above:
construct three `Message` fixtures sharing a `media_group_id`, one with
caption + `@mention` entity and two with no caption; assert the filter
emits `Some(_)` for all three when sender is trusted.

## Migration

None. No on-disk format changes, no config changes, no schema migrations.
The change ships on the next bot restart (`Regenerated(BotRestart)` —
this is purely Rust code).

Already-deployed agents pick this up automatically when their bot
process restarts (process-compose `on_failure` policy or a manual
`right restart <agent>`).

## Risks

- **Slow Telegram delivery edge cases.** The 2500 ms hard cap means an
  album whose last sibling is delayed beyond that point will arrive in
  two batches. CC sees them as two consecutive turns; the agent can still
  reason about both. The user-visible cost is one extra reply — strictly
  better than today's lost-attachments outcome.
- **Worker channel pressure in busy open groups.** Album bursts from
  trusted users now reach the worker even when not addressed. The bound
  is still 32 in `mpsc::channel(32)`; in the worst case `handle_message`
  applies backpressure. This is the same backpressure path that already
  exists for ordinary message bursts — no new failure mode.
- **`address: AddressKind` → `Option<AddressKind>` is a breaking change in
  `DebounceMsg`.** `DebounceMsg` is module-private to `crates/bot`; no
  external crate depends on it. All callsites are within
  `crates/bot/src/telegram/`.

## Alternatives considered

- **Filter-side buffer + re-injection.** Hold media-group siblings in a
  `DashMap<(chat_id, media_group_id), Vec<Message>>` until a 1500 ms idle
  timer fires, then evaluate addressedness. Rejected: teloxide's
  `filter_map` is synchronous, and there is no clean re-injection path
  back into the dispatcher pipeline. Doable via direct `worker_map`
  pushes that bypass `handle_message`, but that duplicates the
  `DebounceMsg` construction logic and concentrates state outside the
  natural per-session boundary.

- **Pass all trusted-sender group traffic to the worker; check
  addressedness only at batch finalisation.** Rejected: this would
  spawn workers and consume backpressure budget for ordinary chatter
  in open groups, and the address gate would have to drop large numbers
  of unaddressed batches. The narrower trigger ("only media-group
  siblings bypass the per-message mention check") is sufficient.

- **Always extend debounce to 1000 ms idle.** Rejected: regresses
  single-message latency for the vastly more common non-album case
  by 500 ms.

## References

- Bug log: `~/.right/logs/him.log.2026-04-27` lines 137-152
- Filter code: `crates/bot/src/telegram/filter.rs:20-60`
- Mention code: `crates/bot/src/telegram/mention.rs:25-81`
- Worker debounce: `crates/bot/src/telegram/worker.rs:340-399`
- Handler routing: `crates/bot/src/telegram/handler.rs:248-305`
- Telegram Bot API — `Message.media_group_id` (no atomicity or size
  guarantees): https://core.telegram.org/bots/api#message
- Standard timeout-aggregator pattern in third-party libraries:
  https://github.com/tdlib/telegram-bot-api/issues/339
