# Switchable response modes (addressed / all) per topic or group

**Date:** 2026-06-06
**Status:** Design approved, ready for implementation plan

## Problem

In a group, the bot answers a message only when it is *addressed*: an
`@mention`, a reply to one of its messages, or a `/command`
(`crates/bot/src/telegram/filter.rs:55`,
`crates/bot/src/telegram/mention.rs:25`). Plain text is dropped even in an
"opened" group — `is_group_open` only relaxes the trusted-sender check, it does
**not** lift the addressing requirement.

A trusted user wants to let the bot participate in a chosen topic (or the whole
group) without `@mentioning` it every time, and to keep the strict
addressed-only behaviour elsewhere — under explicit, per-scope control.

### Pre-existing bug this design must fix

Telegram models forum topics on the reply mechanism: a message in a named topic
references the topic's root `forum_topic_created` service message, whose author
is whoever created the topic. When the **bot** created the topic, every message
in it looks like a reply to the bot, so `is_bot_addressed`
(`mention.rs:31-36`) returns `GroupReplyToBot` and the bot answers everything —
but only in topics it created, and never in the General topic. This produces the
inconsistent, topic-dependent behaviour the feature must replace with a single
explicit control. There is currently no guard for the topic-root service
message in the addressing path (verified: no `forum_topic_created` /
`is_topic_message` reference in the filter/mention path).

## Goals

- A trusted user can set, per scope, one of two response modes:
  - `addressed` (default): respond only when addressed (mention / reply /
    command). Untrusted senders must address the bot.
  - `all`: respond to **every** message in the scope, from **any** participant,
    no addressing required (choice A — convenience over strictness).
- Scope is either a single topic (including General) or the whole group.
- Consistent behaviour across all topics: after the bug fix, the default is
  `addressed` everywhere, including General and bot-created topics.
- Deployable to already-running agents with zero manual steps and no sandbox
  recreation.

## Non-goals

- No DM modes (DMs are always answered for trusted users; mode applies to
  groups/supergroups only).
- No per-user mode (mode is per-scope, not per-sender).
- No dashboard surface in this iteration — operational toggles like `allow_all`
  are slash commands; `/mode` follows that precedent.

## Model and precedence

- `ResponseMode = Addressed | All`. Default `Addressed`.
- Scope key = `(chat_id, effective_thread_id)`; General normalises to thread
  `0` (`crates/bot/src/telegram/session.rs:21`).
- Lookup order: explicit topic entry → group default → built-in `Addressed`.
- `All` takes effect **only in an opened group** — `open` (membership in the
  allowlist `groups` list) remains the master engagement gate. Mode is a
  refinement *within* an opened group. A closed group is unchanged: a trusted
  sender still gets through when addressed; mode entries (if any linger) are
  ignored while the group is closed.

## Storage: `allowlist.yaml` v1 → v2

Mode is "how the bot engages in this chat", which is the allowlist's job. The
file is already in memory behind an `RwLock`, hot-reloaded by a watcher
(`allowlist.rs:422`), and read on exactly the filter path. No new file, no DB on
the hot path (`filter.rs` is a synchronous in-memory closure; an async DB read
there would force a cache layer for no benefit).

```yaml
version: 2
users: [...]
groups:
  - id: -100123
    label: Dev Team
    opened_by: null
    opened_at: 2026-06-06T00:00:00Z
    mode: addressed                       # group default; absent ⇒ addressed
    topics:
      - { thread_id: 0, mode: all }       # General
      - { thread_id: 8, mode: addressed }
```

Changes in `crates/right-agent/src/agent/allowlist.rs`:

- New `ResponseMode` enum (serialize lowercase `addressed` / `all`), default
  `Addressed`.
- `AllowedGroup` gains `mode: ResponseMode` (`#[serde(default)]`) and
  `topics: Vec<TopicMode>` (`#[serde(default)]`), where
  `TopicMode { thread_id: i64, mode: ResponseMode }`. Both default, so **v1
  files load as `addressed` everywhere** and behaviour is preserved.
- `parse_yaml` accepts version `1` **or** `2`, upgrades in memory, and
  `serialize_yaml` always writes version `2`. `deny_unknown_fields` stays (the
  added fields are known). `CURRENT_VERSION = 2`.
- `serialize_yaml` extended to emit `mode` and the `topics` block
  deterministically (mode omitted when `Addressed` to keep v1-equivalent files
  byte-stable where possible; topics omitted when empty).
- New methods on `AllowlistState`:
  - `response_mode(chat_id, thread_id) -> ResponseMode` (precedence above).
  - `set_group_mode(chat_id, mode)`, `set_topic_mode(chat_id, thread_id,
    mode)`, `clear_topic_mode(chat_id, thread_id)`. Setting a mode on a group
    not yet present is rejected by the command layer (group must be open first).

## Gating: `crates/bot/src/telegram/filter.rs`

In the group/supergroup arm of `make_routing_filter`, compute
`mode = state.response_mode(chat_id, effective_thread_id(&msg))` while the read
lock is held.

- `All` **and** `group_open` → always return `Some(RoutingDecision { address,
  sender_trusted, group_open })`, where `address` is whatever `is_bot_addressed`
  found (may be `None`). Untrusted, unaddressed senders are admitted.
- Otherwise (`Addressed`, or group not open) → unchanged logic: drop if
  `!sender_trusted && !group_open`; then drop if unaddressed and not an album
  sibling / forward.

`RoutingDecision.address` is already `Option`, and the worker already handles
`address: None` (album/forward path), so an unaddressed `All`-mode text needs no
new downstream handling. Confirm in the plan that the worker's prompt-build path
treats `address: None` plain text identically to an album sibling (no mention to
strip, full text used).

## Commands + callbacks (mirror `/model`)

Pattern source: `crates/bot/src/telegram/model_command.rs`
(`render_keyboard` → `handle_model` → `handle_model_callback`), registered in
`dispatch.rs:593` via `Update::filter_callback_query()` with a callback-data
prefix filter.

- `/mode` — shows the **current topic's** effective mode and an inline keyboard
  `[📨 Addressed] [💬 All] [↩︎ Inherit group]`. Callback data
  `mode:addressed | mode:all | mode:clear`. Scope `(chat_id, thread_id)` is
  derived from `q.message` (where the keyboard was shown) — General included via
  thread `0`.
- `/mode_group` — shows the **group default** and `[📨 Addressed] [💬 All]`.
  Callback data `modegroup:addressed | modegroup:all`.
- Both commands and both callbacks: trusted sender only, group/supergroup only,
  group must be open. Callback data carries no chat/thread id (taken from the
  message) and re-checks trusted on tap — no forgery, no stale-scope risk.
- On tap: locked read-modify-write of `allowlist.yaml`,
  `answer_callback_query` within the ~3s spinner window, then edit the menu
  message to reflect the new state.
- Register two new `BotCommand` variants and two callback-prefix filters
  (`mode:`, `modegroup:`) alongside the existing `model:` wiring in
  `dispatch.rs`.

### CLI mirror

For parity with `allow_all` (already mirrored as `right agent allow_all`):

- `right agent mode <chat_id> [thread_id] <addressed|all|clear>`
- `right agent mode-group <chat_id> <addressed|all>`

Same locked-RMW writers. This keeps the allowlist's existing
"editable out-of-band" property.

## Bug fix: `forum_topic_created` in `is_bot_addressed`

In `crates/bot/src/telegram/mention.rs:31-36`, do not treat a reply as
addressing when the replied-to message is the topic-root `forum_topic_created`
service message. Use the teloxide accessor (`reply.forum_topic_created()` /
the `ForumTopicCreated` message kind — exact accessor to be confirmed against
the pinned teloxide version in the plan). Real replies to actual bot messages
still return `GroupReplyToBot`.

This makes `Addressed` behave identically in every topic, including bot-created
ones and General, which is the consistency requirement.

## Upgrade / compatibility

- `allowlist.yaml` is bot-managed with an existing hot-reload watcher. Running
  agents need no action: a v1 file loads as `addressed` everywhere (current
  behaviour). The first `/mode` write upgrades the file to v2 in place. No
  sandbox recreation, no `right agent init`.

## Security note (accepted trade-off)

Choice **A** means an `All`-mode scope answers any participant without a
mention. Consequences: chat noise, model-budget consumption, and a
prompt-injection surface from untrusted senders' free-form text. This is an
explicit, per-scope, trusted-user-only opt-in; the default stays `addressed`
and the group must already be open. Recorded here as a deliberate decision, not
an oversight.

## Testing

Targeted tests during development; full `cargo test --workspace` as the final
gate.

- `allowlist` (`right-agent`): v1 upgrade-on-read; v2 round-trip; `response_mode`
  precedence (topic > group > default); `set_group_mode` / `set_topic_mode` /
  `clear_topic_mode`; serialize stability for an all-`addressed` file.
- `filter`: `All` admits an untrusted, unaddressed text; `Addressed` drops it;
  `All` is inert when the group is closed; existing addressed/album/forward
  tests still pass.
- `mention`: a reply whose target is a `forum_topic_created` service message is
  **not** treated as addressing; a real reply to a bot message still is.
- command/callback: keyboard render for each scope; trusted gate (untrusted tap
  → "Not allowed", no write); callback parse for `mode:*` / `modegroup:*`;
  RMW persists and the menu message is edited.

## Files touched

- `crates/right-agent/src/agent/allowlist.rs` (+ `allowlist_tests.rs`): schema
  v2, `ResponseMode`, lookup/setters, parse/serialize.
- `crates/bot/src/telegram/filter.rs`: mode-aware gating.
- `crates/bot/src/telegram/mention.rs`: topic-root reply guard.
- `crates/bot/src/telegram/mode_command.rs` (new): `/mode`, `/mode_group`,
  callbacks, keyboard render.
- `crates/bot/src/telegram/dispatch.rs`: command variants + callback filters.
- CLI: `right agent mode` / `mode-group` subcommands.
