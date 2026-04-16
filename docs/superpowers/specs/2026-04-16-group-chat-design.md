# Group Chat Support

## Overview

Extend the RightClaw Telegram bot to work in group chats. The bot listens in groups it is added to, responds to members according to a dynamic allowlist, and persists per-agent allowlist state in a new bot-managed `allowlist.yaml` file.

DM behavior is unchanged. Group behavior is layered on top of the existing session model, which already keys by `(chat_id, effective_thread_id)`.

## Goals

- Bot responds to `@mention` and reply-to-bot messages in groups it has been added to.
- Two-level trust model:
  - **Trusted users** (global allowlist): can interact with the bot in any chat (DM or group).
  - **Opened groups** (per-group allowlist): any member may mention/reply the bot in these groups.
- Allowlist is mutable at runtime via Telegram commands — no bot restart.
- Telegram privacy mode can remain enabled (mention-only fits the default behavior).
- Single source of truth: `agents/<name>/allowlist.yaml`.
- Backward compatibility for existing agents whose `agent.yaml` contains `allowed_chat_ids`.

## Non-Goals

- **Per-group memory isolation.** Discussed and rejected. Memory exfiltration through auto-recall is a real vector but is dominated by broader tool-surface vectors (Write to file → read from another context, MCP data exfiltration). Closing only the memory vector is "a lock on an open window." Per-tool/per-MCP restriction in groups is a separate future design.
- **Free-response groups** (bot answers every message without mention). Out of scope for this design — only mention/reply routing.
- **Live "thinking" status messages in groups.** Edits are noisy in group context. DM-only for now.
- **Attachments inside reply-target messages.** Only the triggering message's attachments are processed in this design; reply-target attachments are text-only.
- **Session-management commands in groups** (`/new`, `/list`, `/switch`, `/reset`, `/mcp`, `/doctor`). DM-only. The group has one implicit session per `(chat_id, thread_id)`; first message creates it, subsequent messages resume.
- **Cross-thread permission granularity in supergroups.** `allow_all` applies to the whole group; supergroup topics only add a `topic:<thread_id>` tag to memory retains for observability.
- **Automatic deactivation** when the last trusted user leaves a group. Revocation is always explicit via `/deny_all`.
- **Rewriting `agent.yaml`** to remove legacy fields during migration. User-managed file stays untouched; a WARN log nudges cleanup.

---

## Response Rules

For each incoming Telegram message the bot computes whether to route it to the worker:

| Context              | Sender trusted? | Group in `allow_all`? | Mention / reply-to-bot? | Route to worker? |
|----------------------|-----------------|-----------------------|-------------------------|------------------|
| DM                   | yes             | —                     | —                       | ✅               |
| DM                   | no              | —                     | —                       | ❌ silent        |
| Group                | yes             | —                     | yes                     | ✅               |
| Group                | yes             | —                     | no                      | ❌ silent        |
| Group                | no              | yes                   | yes                     | ✅               |
| Group                | no              | yes                   | no                      | ❌ silent        |
| Group                | no              | no                    | any                     | ❌ silent        |

"Silent" = no reply, no ack, no log entry at WARN level. Consistent with current `allowed_chat_ids` filter behavior.

---

## Mention / Reply Detection (groups)

A message counts as addressed to the bot iff **any** of:

1. Message text/caption contains `@<bot_username>` — parsed from Telegram `MessageEntity { kind: Mention }`.
2. A `MessageEntity { kind: TextMention }` points at the bot's `user_id`.
3. `reply_to_message.from.id == bot_user_id`.
4. The message is a slash command targeted at the bot (Telegram forces `@botname` suffix in groups for disambiguation when multiple bots are present).

Pre-processing of the prompt text:

- Strip the leading `@<bot_username>` fragment from the message body (and from `reply_to_message` if the mention happens to also appear there).
- Prepend a group-attribution line (§ Prompt Shape).

## Prompt Shape

### DM

Unchanged — raw text piped to CC, YAML wrapping only when multi-message batch or attachments present (as today in `telegram-attachments-design.md`).

### Group

Attribution is injected so the agent knows it's in a group and who is talking. Unlike DM, groups **always** use YAML input (even single-message, no-attachment case) because attribution metadata must be carried structurally:

```yaml
messages:
  - id: 12345
    chat:
      kind: group
      id: -1001234567890
      title: "Dev Team"
      topic_id: 7           # supergroup topic, omitted otherwise
    author:
      user_id: 987654
      first_name: "Alice"
      username: "alice"
    reply_to:               # present only if reply-to-non-bot message exists
      author:
        user_id: 123
        first_name: "Bob"
        username: "bob"
      text: "here is the function: foo()"
    text: "what does this do?"
    attachments: [...]      # same schema as DM
```

- `@botname` is stripped from `text` before serialization.
- Reply-target author metadata is included so the agent can address them by name if it replies.
- Reply-target **attachments are not downloaded** in this design (deferred).

---

## Commands

All commands below require the caller to be a trusted user (§ Storage). Non-trusted senders get silent ignore, same as any other message.

| Command                        | Valid contexts | Effect                                                                                  |
|--------------------------------|----------------|-----------------------------------------------------------------------------------------|
| `/allow` (reply)               | DM, Group      | Add `reply_to_message.from.id` to `users`.                                              |
| `/allow @user`                 | DM, Group      | Works **only** when Telegram provides a `TextMention` entity with embedded `user_id` (typically inserted via Telegram's autocomplete). A plain text `@username` (no entity-level user_id) returns `✗ cannot resolve @username — reply to their message or use numeric user_id`. |
| `/allow <user_id>`             | DM, Group      | Add by explicit numeric ID.                                                             |
| `/deny` (reply) / `/deny @user` (TextMention only) / `/deny <user_id>` | DM, Group | Remove from `users`. **Self-deny rejected** with an ack message. |
| `/allowed`                     | DM, Group      | Reply with a compact list: trusted users + opened groups (with labels, when & by whom). |
| `/allow_all`                   | Group only     | Add this `chat_id` to `groups`. Ack as reply-to in the chat.                            |
| `/deny_all`                    | Group only     | Remove this `chat_id` from `groups`.                                                    |

### Command Routing Rules

- Commands are never routed to the CC worker. They are handled synchronously inside the bot.
- In groups, only commands where the `@botname` suffix is present (or Telegram auto-suffixed them) count as bot-directed. A stray `/allow` aimed at another bot in the group is ignored.
- `/allow_all` and `/deny_all` outside a group context reply with a short error: `"this command is only valid in group chats"`.
- `/allow` against a bot's own user_id or other bot accounts is rejected with an ack.
- Existing DM-only commands (`/start`, `/reset`, `/new`, `/list`, `/switch`, `/mcp`, `/doctor`) are silently ignored in groups.

### Ack Messages

Bot acknowledges successful mutations with a short reply-to-the-command message:

- `✓ allowed user {label or username or id}`
- `✓ user removed`
- `✓ group opened`
- `✓ group closed`
- `✗ cannot deny yourself — add another trusted user first`
- `✗ user already in allowlist` (informational, not an error)

---

## Storage: `allowlist.yaml`

### Location

`~/.rightclaw/agents/<name>/allowlist.yaml`

### Ownership

Bot-managed. User does not hand-edit under normal operation (like `process-compose.yaml`, `policy.yaml`). CLI and Telegram commands are the supported write paths.

### Schema

```yaml
# Bot-managed. Edit via /allow, /deny, /allow_all, /deny_all, or
# `rightclaw agent allow|deny|allow_all|deny_all`.
version: 1
users:
  - id: 123456
    label: andrey             # nullable; last-known first_name/username
    added_by: null            # null = bootstrap/migration; else user_id of adder
    added_at: 2026-04-16T12:00:00Z
  - id: 789012
    label: alice
    added_by: 123456
    added_at: 2026-04-17T09:30:00Z
groups:
  - id: -1001234567890
    label: "Dev Team"         # nullable; last-known chat.title
    opened_by: 123456         # nullable; null = bootstrap/migration
    opened_at: 2026-04-17T10:00:00Z
```

`version` is reserved for forward compatibility. Missing `version` defaults to `1`. Parser rejects unknown future values with a clear error.

### Writes

- **Atomic.** Write to `allowlist.yaml.tmp` in the same directory, `fsync`, rename over the target. No partial-file window.
- Full file rewrite each time. No attempt to preserve comments (there are none; header comment regenerated from a constant).
- `label`, `added_at`, `opened_at` are refreshed best-effort from incoming Telegram metadata (user first_name / chat title) on command invocation.
- **Concurrency.** Both the bot's command handlers and the CLI mutation commands acquire a short-lived lockfile at `allowlist.yaml.lock` (blocking, short timeout) before the read-modify-write sequence. The `notify` watcher ignores events while the lockfile is held by this process to avoid self-feedback.

### Reads

- Bot loads `allowlist.yaml` at startup into `Arc<RwLock<AllowlistState>>`.
- **File watcher** (`notify` crate) on `allowlist.yaml` — on change, re-parse and swap the `RwLock` contents. No bot restart. Debounce 200 ms.
- Filter code consults the in-memory cache with a read lock.

### Bootstrap (Fresh Agent)

- `rightclaw agent init` wizard adds a new interactive step:
  > "Your Telegram user ID (first trusted user, optional — you can skip and add later via CLI):"
- If provided, write `allowlist.yaml` with a single user entry, `added_by: null`.
- If skipped, do not create `allowlist.yaml` at init time; bot creates it empty on first startup.

### CLI Commands (Recovery / Pre-start Editing)

- `rightclaw agent allow <agent> <user_id> [--label <name>]`
- `rightclaw agent deny <agent> <user_id>`
- `rightclaw agent allow_all <agent> <chat_id> [--label <title>]`
- `rightclaw agent deny_all <agent> <chat_id>`
- `rightclaw agent allowed <agent>` — dump current state to stdout (formatted table).

All four mutation commands acquire a short-lived lockfile (`allowlist.yaml.lock`) to avoid racing with the bot's writer. The `notify` watcher in the bot picks up the change automatically.

### Migration (Existing Agents)

At bot startup, **before** any message handling begins:

1. If `allowlist.yaml` exists → use it; skip migration.
2. Else, parse `agent.yaml`. If it has `allowed_chat_ids: [...]`:
   - Split by sign: `id > 0` → `users`, `id < 0` → `groups`.
   - Write `allowlist.yaml` with all entries, `added_by: null` / `opened_by: null` (null indicates migrated/bootstrap origin), `added_at` / `opened_at: now()`, `label: null`.
   - Log at INFO level: `migrated N users, M groups from agent.yaml::allowed_chat_ids; consider removing the legacy field`.
3. If neither exists, create `allowlist.yaml` with empty `users: []` and `groups: []`.

On every subsequent startup, if `agent.yaml` still has `allowed_chat_ids`, log one WARN: `legacy allowed_chat_ids field ignored; source of truth is allowlist.yaml`.

### Empty Allowlist Safety

If `users: []` and bot receives a DM, bot stays silent. Protects against random-DM hijacking on unclaimed agents. Operator must add the first trusted user via `rightclaw agent allow` or during init.

---

## Memory & Session

### Memory (unchanged bank model)

Single Hindsight bank per agent (`bank_id = agent_name`). No per-group isolation.

**Retain tags — expanded:**

- DM: `["chat:<chat_id>"]` (as today)
- Group: `["chat:<chat_id>", "user:<sender_user_id>"]`, plus `"topic:<thread_id>"` when `message_thread_id` is present in a supergroup.

**Recall tags — unchanged:** `tags_match: "any"` with `["chat:<chat_id>"]`. Topic and user tags are observability-only in this design, not gates.

### Sessions

Existing keying already fits: sessions are keyed by `(chat_id, effective_thread_id)`. No code change needed here.

- First routed message in a new `(chat_id, thread_id)` → `--session-id <new-uuid>`.
- Subsequent routed messages in the same `(chat_id, thread_id)` → `--resume <root-uuid>` (as today).
- Session management UI is DM-only (§ Non-Goals).

---

## UX Details

- **Bot replies in groups are Telegram replies** to the triggering message (sets `reply_to_message_id`). Makes the conversation thread readable in the group UI.
- **Attachments out** (CC-generated) in groups are sent as reply-to the triggering message, same as DM but with the reply relation.
- **Debounce** — per-session 500 ms debounce is unchanged; batching across multiple triggering messages in the same session still applies.
- **Live thinking message** — off in groups. `show_thinking` flag in `agent.yaml` applies to DM only. A future `show_thinking_in_groups: false` flag could be added if needed.
- **Rate limiting / anti-spam** — out of scope; inherits Telegram's and teloxide's existing behavior.

---

## Threat Model

**In scope:**

- Random users DM-ing the bot on an unclaimed agent → rejected (empty allowlist is safe default).
- Bot getting added to random groups → silent until a trusted user issues `/allow_all`.
- Non-trusted members trying to use bot commands or mentions in a closed group → silent.

**Out of scope (documented risks):**

- Once a group is opened with `/allow_all`, members can trigger arbitrary CC actions (file reads, MCP calls, memory retains). This is the same trust surface as a trusted DM partner. Anyone opening a group is implicitly extending that trust.
- Persistent-state exfiltration: a group member asks CC to `Write` sensitive info into a file; a DM user later asks CC to `Read` it. Not prevented by this design. Mitigation belongs in a separate "group tool restriction" feature.
- Auto-recall in groups may surface DM-originated memories (since memory is shared across contexts). Accepted. Per threat model, group membership is a trust event.

---

## Implementation Map

New/changed files (approximate; final shape is for the plan):

- `crates/rightclaw/src/agent/allowlist.rs` (new): `AllowlistState`, `AllowlistFile`, load/write/migrate.
- `crates/rightclaw/src/agent/types.rs`: drop `allowed_chat_ids` from `TelegramConfig` struct (keep permissive parsing for migration read-path).
- `crates/bot/src/telegram/filter.rs`: replace `HashSet<i64>` filter with routing against `Arc<RwLock<AllowlistState>>` + mention detection.
- `crates/bot/src/telegram/handler.rs`: add branches for `/allow`, `/deny`, `/allowed`, `/allow_all`, `/deny_all` before the worker-routing path. Gate existing commands on DM context.
- `crates/bot/src/telegram/mention.rs` (new): detect mention / reply-to-bot; strip `@botname`; extract reply target.
- `crates/bot/src/telegram/prompt.rs`: extend YAML wrapping to include group attribution + reply-target fields.
- `crates/bot/src/telegram/worker.rs`: tag retain calls with `user:` and `topic:` when in a group; no change to recall.
- `crates/rightclaw-cli/src/main.rs`: `rightclaw agent allow|deny|allow_all|deny_all|allowed` subcommands.
- `crates/bot/src/lib.rs`: allowlist migration at startup, `notify` watcher setup.
- `crates/rightclaw/src/init.rs`: wizard step for first trusted user.

Tests:

- Unit: allowlist parsing, migration from `allowed_chat_ids`, write-atomicity on the happy path.
- Integration: command routing matrix (trusted/non-trusted × DM/group/opened-group × mention/no-mention), self-deny rejection, empty-allowlist silence.

## Migration Checklist (for operators)

- On first bot startup after upgrade, check logs for `migrated ... from allowed_chat_ids`.
- Verify `allowlist.yaml` appeared next to `agent.yaml` and contains the expected entries.
- Remove `allowed_chat_ids` from `agent.yaml` when convenient (bot will keep WARN'ing until done).
- Add the bot to a group, DM `/allow_all` from a trusted account inside the group, verify `✓ group opened` reply.
