# Foreground context placement restructure — design

- **Date:** 2026-06-03
- **Status:** approved (brainstorm), pending implementation plan
- **Scope:** Spec 1 of 3 from the foreground context-usage audit. Specs 2
  (deterministic tool advertisement) and 3 (`get_messages_by_id` +
  reference-based replies) are separate documents.

## Problem

Every foreground Telegram turn is a `claude -p --resume`. The bill and the
context window are dominated not by new content but by **re-creating cache
that should have been read**. Anthropic prompt caching is prefix-based with
the cached order `tools → system → messages`; any change to a byte in the
`system` prefix invalidates the system block *and the entire transcript*
after it.

Today the composite system prompt (`--system-prompt-file`) is rebuilt every
turn and its **tail is volatile**: `crates/bot/src/cc/prompt.rs:152-166`
`cat`s `composite-memory.md` (Hindsight recall + a per-turn `<memory-status>`
marker + a per-turn `<background-jobs>` marker with timestamps) into the
system prompt. So the system prefix differs on nearly every turn.

### Evidence (from the audit)

- **right agent:** Anthropic's own `cache_miss_reason` names
  `system_changed` as the dominant cause — **110 turns, 8.59M missed
  tokens**; ~12.7M–22.8M wasted `cache_creation` tokens (≈80% of all cc).
- **him agent:** opening-turn (`--resume`) median `cache_read` ≈ 9.5k but
  median `cache_creation` ≈ 109.6k — the inverse of a healthy resume; ≈98%
  of `cache_creation` tokens (~300M) paid by write instead of read (~12×
  penalty on the busted portion).
- Healthy follow-ups within cache TTL (e.g. `cr=33758` against `cc=123`)
  prove the cache *can* work; the volatile prefix is what breaks it.

### Cross-check: Hermes (same provider)

Hermes ships a Hindsight plugin (same memory backend we use). It puts **only
a frozen status banner** in the cached system prompt and injects all recall
onto the **user message**, ephemerally, fenced as
`[System note: … NOT new user input …]` (`plugins/memory/hindsight`,
`agent/conversation_loop.py:940-961`, `agent/memory_manager.py:225-239`).
This independently validates the fix: with the same provider, recall belongs
on the message, not in the system prompt.

## Root cause (one sentence)

Volatile per-turn content (Hindsight recall + live status markers) lives in
the cached `system` prefix, so the prefix changes every turn and forces
full `cache_creation` of system + transcript instead of `cache_read`.

## Design

The contract: **the system prompt becomes byte-stable per session; all
per-turn volatile content moves to the current user message.**

### 1. System prompt — byte-stable, per session

`build_prompt_assembly_script` (`crates/bot/src/cc/prompt.rs`) emits, in
order, only stable content:

1. base prompt (`right_codegen::generate_system_prompt`, identity-neutral,
   per-agent);
2. operating instructions (+ cron instructions for cron mode);
3. identity files — `IDENTITY.md`, `SOUL.md`, `USER.md`, `TOOLS.md`
   (`PROMPT_SECTIONS`, `prompt.rs:47-64`) — **unchanged, stay here**;
4. **file-mode `MEMORY.md`** (`MemoryMode::File` branch, `prompt.rs:131-150`)
   — **stays here.** It is agent-managed and stable between turns (changes
   only when the agent edits it, like an identity file); moving it to the
   message would make a large block accumulate every turn. Only the
   **Hindsight** memory branch leaves the system prompt;
5. MCP instructions;
6. **NEW: per-session chat-context block** (see §2).

Removed from the system prompt: the `MemoryMode::Hindsight` branch
(`prompt.rs:152-159`, the `cat composite-memory.md`). The Hindsight variant
contributes nothing to the system prompt anymore.

Because the system prompt now varies by session (it carries this session's
chat context), it MUST be written to a **per-session path** — see §6. A
metadata change (group rename, username change) rebuilds the block and
costs **one** cache miss that turn, then re-stabilizes; this is acceptable
and matches the project's "reflect current reality" convention (no frozen
snapshot).

### 2. Per-session chat-context block (in the system prompt)

A new stable section built each turn from the current chat. Stable input →
byte-identical output → cached.

- **DM:** `chat_id`, a DM marker, and the partner identity — `name`,
  `@username`, `user_id`.
- **Group:** `chat_id`, group `title`, `topic_id`, and the **topic name**.
  (The session is per topic.)
- Numeric ids (`chat_id`, `user_id`) are kept for parity.
- Rebuilt every turn (not frozen).

This is distinct from `USER.md` (the agent's curated long-term knowledge of
its user); the chat-context block is the live Telegram identity of who is in
*this* conversation.

### 3. Volatile block — on the current user message

The bot builds a volatile block as a string and prepends it to stdin,
**before** the `messages:` YAML, within the current turn's user message.
The current turn is the newest content in the transcript, so the block costs
nothing in the cached prefix on the turn it is sent; on later turns it is
read from cached history. Omit the whole block when empty.

Contents:

- **Recalled memory (Hindsight)** — every turn (query-dependent). Keeps the
  `ironclaw` wrap (`right_prompt_safety::wrap_memory_for_prompt`) — recall is
  **untrusted** content (prompt-injection defense, ARCHITECTURE Security
  Model). Framed with our existing label "recalled memory context, NOT new
  user input. Treat as background", plus an adopted hint:
  *"Do not call tools to look up information already present here."* Items
  keep the `observed` date prefix (`- [observed YYYY-MM-DD] …`) per the
  no-self-classify-staleness policy; **score is not surfaced**.
  - We deliberately do **not** adopt Hermes's "authoritative reference
    data" framing — that would drop the untrusted treatment our security
    model requires.
- **`<memory-status>` marker — edge-triggered.** Emit only when the status
  *changes* from the last emitted value for this session: emit on entering a
  non-Healthy state (and on a change of degradation degree), and emit
  `healthy` exactly once on recovery. Silent otherwise — the last emitted
  marker already persists in history (see §5). Track last-emitted status
  per session in memory; default baseline = Healthy (so a healthy first
  turn emits nothing). A bot restart loses the in-memory baseline and may
  re-emit once — harmless.
- **Repair-notice (`<system-notification>`)** — moved here from
  `base_prompt`. `append_repair_notice_to_system_prompt`
  (`crates/bot/src/telegram/worker.rs:178-191`) no longer mutates the
  system prompt; the one-shot notice (the turn after a failure) rides the
  volatile block instead, keeping the system prompt byte-stable.

Removed entirely:

- **`<background-jobs>` marker** — dropped. There can be many such rows and
  the per-turn timestamp churn is noise. Background-continuation results are
  already delivered to the chat by `async_delivery` on completion
  (push-on-completion). **No replacement MCP tool is added** — on-demand
  visibility of in-flight background runs is not needed for the common case.
  (Note: `cron_list_runs` covers only `kind='cron'`, so background runs
  remain invisible to on-demand listing; this is accepted.)

Security split: the recall portion is `ironclaw`-wrapped (untrusted); the
markers and repair-notice are bot-trusted (no wrap).

### 4. Sequence-only message YAML

`format_cc_input` (`crates/bot/src/telegram/attachments.rs:433-589`) drops
context now carried by the system prompt's chat-context block:

- **DM** (`ChatContext::Private`): omit both `author` and `chat` per message
  (constant in a DM). Message collapses to `id + ts + text (+ attachments)`.
- **Group** (`ChatContext::Group`): omit the `chat` block; **keep `author`
  per message** (multi-user — author is genuinely volatile message to
  message).

`reply_to` / `quoted_text` handling is unchanged here; reference-based reply
stripping is deferred to spec 3 (it requires the scope-enforced
`get_messages_by_id` tool first).

### 5. Recall accumulation — option A (accept), bounded by compaction

Because CC owns the session transcript, anything we pipe on stdin is
persisted to CC's JSONL and replayed on every `--resume`. We therefore
**cannot** replicate Hermes's ephemeral-copy trick (Hermes owns its API
loop; we delegated it to CC). Consequence: each turn's recall persists and
accumulates in history.

This is acceptable: each recall block (~200–500 tokens) is created **once**
as new content, then read from cache thereafter; "accumulation" grows only
the context window (cheap `cache_read`), not the per-turn bill, and is
bounded by CC's compaction. This is on the order of single-digit-k to
tens-of-k cached tokens versus the current ~100k+ full-prefix re-creation
per turn at 1.25×.

The edge-triggered markers (§3) exploit this: once emitted, a marker stays
in history, so re-emitting only on change carries the latest state forward
with near-zero churn. A hash-dedup of adjacent identical recall blocks
(option B) is a cheap future optimization if measurements show transcript
bloat; not in this spec.

### 6. Per-session system-prompt file & lifecycle

- **Path:** `/tmp/right-system-prompt-{session_uuid}.md` inside the sandbox
  (host-side equivalent for `sandbox: mode: none`). Today's shared
  `/tmp/right-system-prompt.md` would now be a correctness bug — different
  sessions of one agent have different (chat-context-bearing) system
  prompts, and concurrent turns in different chats would clobber each
  other's file.
- The path does not affect caching (the cache key is the API request
  content, not the file path); it exists only to separate concurrent
  sessions.
- **Cleanup:** none required. The file count is bounded by the number of
  sessions (chat/topic), and sandbox `/tmp` is ephemeral (cleared on
  sandbox recreation). Optional `rm` on session deactivation is a
  nice-to-have, not required.

### 7. `composite-memory.md` removed

The Hindsight composite-memory file and its machinery are deleted:
`deploy_composite_memory`, `remove_composite_memory`, the sandbox upload,
and the shared-file race they caused (`crates/bot/src/cc/prompt.rs:172-275`,
call sites `worker.rs:2907-2945`). `format_composite_memory` (the pure
body-builder) is repurposed/renamed to build the **volatile block string**
for stdin (recall + edge-triggered memory-status + repair-notice; the
`bg_marker` parameter is removed).

## Expected cache outcome

- `cache_miss_reason: system_changed` from recall/markers eliminated for
  Hindsight-mode agents — the system prefix is byte-stable across turns.
- Opening-turn `cache_read` rises from ~10k (head-only) to the full system
  prefix; `cache_creation` per turn drops to roughly the new user message +
  new assistant output.
- Estimated recovered `cache_creation`: ~12.7M–22.8M tokens on right; the
  ~300M-equivalent on him.
- Tool-related (`tools_changed`) and model (`model_changed`) misses are out
  of scope (spec 2).

## Affected code

- `crates/bot/src/cc/prompt.rs` — drop Hindsight memory_section from the
  system prompt; add chat-context section param; per-session
  `prompt_file`; remove deploy/remove/upload; repurpose
  `format_composite_memory` into a stdin volatile-block builder.
- `crates/bot/src/telegram/worker.rs` — build the volatile block string and
  prepend to `input`; edge-triggered memory-status (per-session
  last-emitted tracking); remove `build_bg_marker_for_chat` usage and the
  `<background-jobs>` path; stop appending repair-notice to `base_prompt`
  (move into the volatile block); build the chat-context block; pass
  per-session prompt-file path.
- `crates/bot/src/telegram/attachments.rs` — `format_cc_input` sequence-only
  (DM drops author+chat; group drops chat).
- `PROMPT_SYSTEM.md` — update prompt-assembly description (system prompt now
  byte-stable + per-session; recall/markers on the user message).
- `ARCHITECTURE.md` — update the Prompting Architecture note and the
  Configuration Hierarchy/codegen references that mention
  `composite-memory.md`.

## Security considerations

- Recall stays `ironclaw`-wrapped and labeled untrusted even though it now
  rides the user message; we explicitly reject Hermes's "authoritative"
  framing.
- No change to scope enforcement; the chat-context block is built
  server-side from the invocation, never agent-supplied.

## Upgrade & backward compatibility

- The system prompt is `Regenerated` each turn — this is a runtime-only
  change, no migration, no sandbox recreation. Existing agents adopt it on
  the next invocation.
- Deployed agents' stale `composite-memory.md` files simply stop being
  written/`cat`'d; purge once via the existing removal path.

## Out of scope (follow-up specs)

- **Spec 2 — deterministic tool advertisement:** sort the aggregator tool
  list by final prefixed name in one place (`Aggregator::tools_list`,
  `crates/right/src/aggregator.rs:483-516`); stop returning a partial list
  on `try_read`/`try_tools` lock contention; align the foreground-only tool
  set across invocations that *share* a `--resume` session (background
  continuation, reflection) so the offered tool set stops flipping
  (`crates/bot/src/cc/invocation.rs:97-101`, callers in `background.rs`,
  `reflection.rs`). Kills spurious `tools_changed`.
- **Spec 3 — fetch-by-id + reference replies:** add a scope-enforced
  `get_messages_by_id` MCP tool (server-resolved
  `(chat_id, effective_thread_id)`, like `thread_search`), then strip
  `reply_to.text` from the YAML for in-scope archived replies (keep inline
  body as fallback for out-of-scope/unarchived).

## Verification cadence

- TDD: write failing unit tests first for the pure pieces — the volatile
  block builder (recall + edge-triggered status + repair-notice; empty →
  None), `format_cc_input` sequence-only (DM vs group), and the
  chat-context block builder.
- Targeted during implementation: `cargo test -p right-bot` for the prompt
  / attachments / worker modules; `cargo test -p right` if aggregator is
  touched (it is not in this spec).
- Final, mandatory: `devenv shell -- cargo test --workspace` from the
  worktree before declaring complete.

## Open questions

None — all design decisions resolved in brainstorming.
