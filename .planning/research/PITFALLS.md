# Pitfalls Research: v3.0 Teloxide Bot Runtime

**Domain:** Adding per-agent teloxide Telegram bots + claude -p session continuity + Rust cron runtime to an existing multi-agent Rust system (RightClaw)
**Researched:** 2026-03-31
**Confidence:** HIGH for teloxide/Telegram API behavior (official docs + GitHub issues); HIGH for claude -p session bugs (confirmed GitHub issues); MEDIUM for notify crate patterns (forum posts + docs); HIGH for tokio subprocess lifecycle (official tokio docs + known issue tracker)

---

## Critical Pitfalls

### Pitfall 1: `claude -p --resume` Broken When CLAUDE_CONFIG_DIR Is Set

**What goes wrong:**
RightClaw sets `CLAUDE_CONFIG_DIR` (or uses `HOME=$AGENT_DIR`) per agent so that each agent's `.claude/` directory is isolated. The `--resume SESSION_ID` CLI flag only searches `~/.claude/projects/` (the host user's default path) — it completely ignores `CLAUDE_CONFIG_DIR`. Sessions are saved to the correct per-agent location, but cannot be resumed from it.

Concrete failure sequence:
1. Agent boots, `claude -p "Hello" --output-format json` runs with `CLAUDE_CONFIG_DIR=/home/user/.rightclaw/agents/right/.claude`
2. Session saved to `/home/user/.rightclaw/agents/right/.claude/projects/.../abc123.jsonl`
3. Next Telegram message: `claude -p "Continue" --resume abc123 --output-format json`
4. Error: "No conversation found with session ID: abc123"

This is a confirmed, tracked CC bug (issue #16103) — closed as "not planned" in Feb 2026. It will not be fixed upstream.

**Why it happens:**
The `--resume` path resolver is hardcoded to `~/.claude/projects/`. The CC team does not plan to support CLAUDE_CONFIG_DIR for resume because they consider it an edge case.

**Prevention:**
- Do NOT use `CLAUDE_CONFIG_DIR` for session continuity. Use the HOME isolation approach instead — set `HOME=$AGENT_DIR` so `~/.claude/` resolves naturally to the agent dir. `--resume` searches `$HOME/.claude/projects/` which is the agent dir.
- If you must use `CLAUDE_CONFIG_DIR`, work around by symlinking: `$AGENT_DIR/.claude/projects → $CLAUDE_CONFIG_DIR/projects`. This defeats isolation purpose. Avoid.
- The `telegram_sessions` table in `memory.db` must store session IDs derived from per-agent HOME sessions, not CLAUDE_CONFIG_DIR sessions.

**Warning signs:**
- `rightclaw up` sets `CLAUDE_CONFIG_DIR` in the agent env rather than `HOME`
- Session IDs stored in `telegram_sessions` can't be resumed manually
- `claude --resume SESSION_ID` works in interactive mode but fails when called from the bot subprocess

**Phase to address:** Phase 1 (bot architecture / subprocess invocation design). This constraint dictates the entire session isolation approach.

---

### Pitfall 2: `claude -p` Returns a Different `session_id` on Resume

**What goes wrong:**
When using `claude -p --resume OLD_SESSION_ID --output-format json`, the returned JSON contains a **new, different `session_id`** in the init message — not the original one. The session context (conversation history) is preserved correctly, but the session identifier changes on every resume.

This breaks a naive session tracking model:
```rust
// BROKEN: session_id changes after each resume
let new_id = run_claude_p(prompt, Some(&stored_session_id)).await?;
// new_id != stored_session_id — so we update the DB
// But next resume of new_id will yield yet another ID
// Result: telegram_sessions table grows unboundedly with orphaned IDs
```

**Why it happens:**
CC creates a new JSONL file for each resume invocation (appending to the parent session's transcript chain internally). The external session_id returned is the ID of the new JSONL file, not the original parent. This is a confirmed bug (issue #8069), unfixed.

**Prevention:**
- Store the session ID returned by the **first** `claude -p` invocation for a given thread (when `--resume` was NOT used). This is the canonical "root session ID" for that thread.
- Always resume using that root session ID. Every time you call `--resume <root_id>`, the resumed session will have a new returned ID — ignore it. Never update `telegram_sessions` with the new ID returned from a resume call.
- Only update `telegram_sessions` when `--resume` was NOT used (i.e., first message in a new thread). This is when a genuinely new canonical session is created.
- Schema implication: `telegram_sessions` must have a `root_session_id TEXT` column (the resumable one) separate from any `last_returned_session_id`.

**Warning signs:**
- `telegram_sessions` row count grows faster than thread count
- Each Telegram message creates a new session_id in the DB
- "Session not found" errors after a few rounds of messaging

**Phase to address:** Phase 1 (telegram_sessions schema design). The column semantics must be explicit before writing a single INSERT.

---

### Pitfall 3: Teloxide `Throttle` Adaptor Has a Deadlock Under Specific Load Patterns

**What goes wrong:**
`Bot::new(token).throttle(Limits::default())` is the recommended approach to respect Telegram rate limits. However, the `Throttle` adaptor has a documented deadlock (issue #516) that manifests when:
1. Messages are prepared but not sent (e.g., bot shuts down mid-reply)
2. The internal worker task blocks on an unsent message queue
3. New requests queue behind the blocked worker, producing 100% CPU + frozen bot

Additionally, adaptor **ordering matters** in a way the documentation does not make prominent:
- **WRONG:** `Throttle<CacheMe<Bot>>` — Throttle sees CacheMe-wrapped requests, misses some chat_id mapping
- **CORRECT:** `CacheMe<Throttle<Bot>>` — Throttle is innermost, sees raw Bot calls with real chat_ids

Using wrong ordering causes Throttle to miscalculate per-chat limits, leading to unexpected 429 errors.

**Why it happens:**
The worker task spawned by `Throttle` can block indefinitely if its internal state machine encounters a prepared-but-unsent message. This is an existing bug without a fix in the crate. The ordering sensitivity is documented only in a GitHub issue (#649), not in the main docs.

**Prevention:**
- Use `Throttle` but add a watchdog: if the bot goes unresponsive for >30 seconds, restart the process via process-compose.
- Adaptor order: `CacheMe::new(Throttle::new_with_limits(Bot::new(token), Limits::default()))`.
- Do not pre-build message builders without sending them — build and send in one future chain.
- For the multi-agent case (each agent has its own bot token), each bot instance runs in its own process, so one stuck Throttle does not affect others.

**Warning signs:**
- Bot consumes 100% CPU but sends no messages
- process-compose shows bot process alive but Telegram messages pile up unanswered
- `tokio-console` shows throttle.rs task stuck in "running" state indefinitely

**Phase to address:** Phase 1 (bot process setup). Wrap in process-compose restart policy with `restart: on-failure` and `backoff_seconds: 5`. Do not rely on Throttle's worker to self-recover.

---

### Pitfall 4: Telegram General Topic (`thread_id = 1`) Rejects `message_thread_id` in Bot API

**What goes wrong:**
For session keying, the plan is `thread_id → session_uuid`. When a user messages in the **General** topic of a forum supergroup, the incoming message has `message_thread_id = 1`. But when the bot replies, passing `message_thread_id: Some(1)` to `send_message()` returns a Telegram API error — the Bot API rejects `message_thread_id = 1` for the General topic (you must omit it, not pass 1).

This creates an asymmetry: the session is keyed on `(chat_id, thread_id=1)` but replies fail unless you special-case `thread_id=1`.

**Why it happens:**
The General topic predates the forum feature and has special handling in the Telegram API. It behaves like a normal supergroup at the API level — replies don't use the thread_id parameter. The Bot API documentation notes this but it's easy to miss.

**Prevention:**
- In the session key derivation: normalize `thread_id = Some(1)` to `thread_id = None` before storing in `telegram_sessions`. General topic is keyed as `(chat_id, None)`.
- In the reply path: if `thread_id == Some(1)`, omit `message_thread_id` from the send request.
- Write a helper:
  ```rust
  fn effective_thread_id(raw: Option<ThreadId>) -> Option<ThreadId> {
      raw.filter(|id| id.0 != 1)
  }
  ```
- Test with a real forum supergroup — this cannot be caught in unit tests.

**Warning signs:**
- Bot works in DMs and non-forum groups but fails in General topic of a forum group
- Telegram API errors of type `TOPIC_CLOSED` or `REPLY_TO_INVALID` when thread_id = 1
- Session IDs exist in DB for thread_id=1 but replies never deliver

**Phase to address:** Phase 1 (message routing / session key derivation). Must be in the initial design, not a Phase 2 fix.

---

### Pitfall 5: Telegram Thread IDs Are Permanent, But Topics Can Be Deleted and Recreated With Different IDs

**What goes wrong:**
Thread IDs are used as session keys. If a user deletes a topic and creates a new one with the same name, the new topic has a different `thread_id` (because it equals the service message ID of `messageActionTopicCreate`). The old session is now orphaned — the bot will start a new session for the same "conceptual" conversation.

Less obviously: if the bot has accumulated conversation history under the old session_id, and the user recreates the topic expecting continuity, they get a fresh Claude session with no memory of prior context. From the user's perspective, the bot "forgot everything."

**Why it happens:**
Thread IDs are server-assigned per service message, not per topic name. Topic name and thread ID are independent. There is no Telegram API way to detect topic deletion/recreation from the bot's perspective.

**Prevention:**
- This is fundamentally a user expectation problem, not a code bug. Document it: "Deleting and recreating a topic starts a fresh conversation."
- Do NOT try to match topics by name across `thread_id` changes — this is unreliable (multiple topics can have the same name).
- Keep orphaned session rows in `telegram_sessions` (don't delete on topic close/deletion) — the old session JSONL files are still on disk and can be inspected if needed.
- Consider displaying the session age on first bot reply in a new thread: "Starting new conversation (previous context not linked to this topic)."

**Warning signs:**
- Users report bot "forgetting" conversations after recreating a topic
- `telegram_sessions` has entries with thread_ids that no longer exist in the group
- Users trying to use topic names as stable identifiers

**Phase to address:** Phase 1 (documentation + session key design). Accept the limitation, document it.

---

### Pitfall 6: Concurrent `claude -p` Invocations on the Same Session Corrupt Context

**What goes wrong:**
A user sends two rapid Telegram messages before the first `claude -p` response completes. Both messages trigger a `claude -p --resume SESSION_ID` subprocess. Both subprocesses read the same session JSONL simultaneously, then both write their response to the same session. The result:

1. Both see the same "last message" as context
2. Both write new turns to the session file
3. Session JSONL becomes interleaved or inconsistent
4. Future resumes start from a corrupted context state

This is especially likely with Telegram's "typing" behavior — users often send multiple short messages in sequence.

**Why it happens:**
`claude -p` does not implement any file-level locking on the session JSONL. Multiple concurrent invocations on the same session path are not safe.

**Prevention:**
- Implement a per-session mutex in the bot process. Before spawning `claude -p`, acquire a tokio `Mutex` keyed by `(chat_id, thread_id)`. Release after the subprocess completes and the response is sent.
- Queue incoming messages per session: use a `tokio::sync::mpsc` channel per active session. Messages are processed serially per thread.
- Do NOT spawn a new subprocess for each message independently. Instead, maintain a "session worker" task per active thread that processes messages sequentially.
- Consider a per-session debounce: if a second message arrives within 2 seconds, batch them into a single `claude -p` call.

**Warning signs:**
- Two messages sent quickly → two responses that don't reference each other
- Session JSONL contains repeated turns or out-of-order messages
- `claude -p --resume` returns responses that reference only one of two rapid messages

**Phase to address:** Phase 1 (message dispatch architecture). This must be the fundamental design — a message queue per session, not fire-and-forget subprocess per message.

---

## Moderate Pitfalls

### Pitfall 7: Telegram 429 Rate Limit — Group Chats Are Stricter Than DMs

**What goes wrong:**
Telegram imposes **20 messages/minute** to the same group, vs. ~1 message/second for DMs. For a multi-agent RightClaw deployment where multiple agents share a single group but have separate topics, all agents share the group's 20 msg/min budget. If Agent A sends 15 messages in a minute (e.g., a verbose task output), Agent B's replies in a different topic hit 429 for the rest of that minute.

The `Throttle` adaptor tracks per-chat-id limits, but all topics in the same group share one chat_id. Throttle will not help here because it throttles per chat_id correctly — but the budget is genuinely shared.

**Prevention:**
- Know the limits: 30 msg/sec across all chats, 20 msg/min per group/channel, no published limit for DMs (practical ~1/sec).
- Always handle 429 with RetryAfter: in teloxide, implement a custom error handler that catches `ApiError::RetryAfter(secs)` and re-queues the message after `secs` seconds.
- For long agent outputs, split across multiple calls with enforced 3-second delay between group messages.
- For cron-triggered broadcasts (all agents posting status at the same time), stagger start times across agents.

**Warning signs:**
- Multiple agents in the same group all go silent simultaneously
- Logs show 429 errors followed by immediate retry (no backoff)
- Cron-triggered messages bunch at exactly :00 seconds of each hour

**Phase to address:** Phase 1 (bot error handler design) + Phase 2 (cron scheduling — stagger by agent index).

---

### Pitfall 8: stdout Buffering in `claude -p` Subprocess Blocks on Large Output

**What goes wrong:**
When `claude -p` is spawned via `tokio::process::Command` with `.stdout(Stdio::piped())`, the subprocess writes to a pipe buffer. On Linux, the default pipe buffer is 64KB. If `claude -p` produces more than 64KB of output before the parent reads it, the subprocess blocks waiting for the pipe to drain. The parent is waiting for `child.wait()` or `child.wait_with_output()`. This produces a deadlock: parent waits for child to exit, child waits for parent to read stdout.

This is a classic subprocess deadlock — not hypothetical. Claude responses can easily exceed 64KB for code generation tasks.

**Prevention:**
- Never use `child.wait()` after piping stdout — it does not drain the pipe.
- Use `child.wait_with_output()` which "simultaneously waits for the child to exit and collects all remaining output" (tokio docs). This is the correct pattern.
- Or: spawn a dedicated task to read stdout while the main task awaits exit:
  ```rust
  let stdout_task = tokio::spawn(async move {
      let mut buf = String::new();
      stdout.read_to_string(&mut buf).await?;
      Ok::<_, io::Error>(buf)
  });
  child.wait().await?;
  let output = stdout_task.await??;
  ```
- Always set stdin to `Stdio::null()` unless you need to send input — leaving stdin open can also cause hangs if the child waits for EOF.

**Warning signs:**
- Bot goes silent on long claude -p responses but works for short ones
- process-compose shows bot subprocess in "running" state indefinitely after a code generation task
- 64KB is a common threshold — if responses under that length work but longer ones hang, this is the cause

**Phase to address:** Phase 1 (subprocess spawning code). Use `wait_with_output()` from day one.

---

### Pitfall 9: Zombie `claude -p` Processes When Bot Shuts Down Unexpectedly

**What goes wrong:**
If the teloxide bot process is killed (SIGKILL via process-compose timeout, or OOM), any in-flight `claude -p` child processes become orphans. On Linux, orphans are reparented to PID 1 (init/systemd) which will eventually reap them. But:
1. The claude process may continue running, consuming CPU and tokens, producing output nobody reads.
2. The session JSONL is written by the orphaned process — a future resume may see a partial or inconsistent state.
3. Multiple restarts can accumulate zombie claude processes if process-compose is aggressive with restart policies.

**Why it happens:**
`tokio::process::Child` has `kill_on_drop` which defaults to false. When the parent task is cancelled (e.g., bot shutdown), the child continues running unless explicitly killed.

**Prevention:**
- Set `.kill_on_drop(true)` on the `Command` builder — this sends SIGKILL to the child when the `Child` handle is dropped.
- In the shutdown handler (SIGTERM/SIGINT), explicitly wait for in-flight claude processes to complete before exiting (with a 30-second timeout).
- Implement graceful shutdown: when the bot receives a stop signal, stop accepting new messages, wait for active sessions to complete, then exit.
- Never use `kill_on_drop` as the primary cleanup — it's a safety net. Graceful shutdown is the primary path.

**Warning signs:**
- `ps aux | grep claude` shows multiple claude processes after a bot restart
- process-compose shows bot restarted 3× but there are 3× the expected claude processes
- Memory consumption grows over days without increasing message count

**Phase to address:** Phase 1 (process lifecycle). Implement graceful shutdown handling in the bot main loop.

---

### Pitfall 10: `notify` Crate — crossbeam-channel vs tokio Conflict

**What goes wrong:**
The cron file watcher uses `notify` (or `notify-debouncer-mini` / `notify-debouncer-full`). Both debouncers enable the `crossbeam-channel` feature by default for their event channels. Inside a tokio runtime, mixing blocking crossbeam channel reads with async code can block tokio worker threads. If the file watcher callback blocks on `crossbeam::channel::recv()` inside a tokio task, the tokio thread pool starves.

**Why it happens:**
`notify-debouncer-mini` defaults to `crossbeam-channel = true`. The debouncer's callback runs in a background thread (fine), but the `Receiver` side is often polled in the async context where it doesn't belong.

**Prevention:**
- Disable the crossbeam feature:
  ```toml
  notify-debouncer-mini = { version = "0.3", default-features = false }
  ```
- Use `tokio::sync::mpsc` to forward events from the notify callback into the async world:
  ```rust
  let (tx, mut rx) = tokio::sync::mpsc::channel(100);
  let mut debouncer = new_debouncer(Duration::from_secs(2), move |res| {
      let _ = tx.blocking_send(res);
  })?;
  // In tokio task:
  while let Some(events) = rx.recv().await { ... }
  ```
- Alternatively, use `tokio-debouncer` crate which is built specifically for tokio integration.
- Consider `notify-debouncer-full` for the rename-stitching feature (useful when cron YAML files are written atomically via rename-over-tmp-file pattern).

**Warning signs:**
- Cron file changes trigger correctly in dev but sometimes miss events under load
- tokio task count stays low but tokio threads are all "blocked" (visible via tokio-console)
- File watch callback appears to hang when the system is processing heavy claude -p tasks

**Phase to address:** Phase 2 (cron runtime — file watcher setup). Disable crossbeam at dependency declaration time.

---

### Pitfall 11: File Watcher Fires Multiple Events Per Single File Save (YAML Editors)

**What goes wrong:**
Text editors write files in multiple operations: truncate, write content, fsync, rename. Each step can fire a separate `notify` event. Without debouncing, a single cron YAML edit triggers 3-6 watcher callbacks in rapid succession, causing the cron reconciler to run 3-6 times for one save. This is harmless if reconciliation is idempotent — but if there is any state mutation on each reconcile (e.g., cancelling and rescheduling a timer), rapid multi-fire causes churn.

Some editors (vim, emacs) write to a temp file first, then rename into place. The `notify` crate may fire a `Remove` event (old path), then a `Create` event (new path via rename) rather than a `Modify` event. The reconciler must handle `Remove + Create` for the same logical file as an "update" — not as "delete this cron job, create a new one."

**Prevention:**
- Use `notify-debouncer-full` specifically because it matches rename From/To pairs and emits a single `Rename` event. For the move-into-place pattern, this is critical.
- Set debounce window to at least 500ms — most editors finish their write sequence within this window.
- Make the reconciler purely idempotent: given the same set of cron YAML files, always produce the same set of scheduled tasks. Running reconciliation 6 times must be equivalent to running it once.
- Do not track "previously scheduled task IDs" as mutable state — recompute the full task set on every reconcile from the current file contents.

**Warning signs:**
- Cron task appears to restart itself 3-5 times after a single file edit
- Editing a YAML file causes a brief task execution gap (task cancelled and rescheduled)
- vim/emacs edits behave differently from `echo > file` writes

**Phase to address:** Phase 2 (cron file watcher). Use `notify-debouncer-full` from the start.

---

### Pitfall 12: Polling and Webhook Cannot Both Be Active for the Same Bot Token

**What goes wrong:**
If `rightclaw` currently uses Claude Code's `--channels plugin:telegram` (which uses long polling), and the new teloxide bot also uses long polling for the same token, one of them will fail silently — Telegram only delivers updates to one poller at a time. The first connection wins; the second gets no updates.

More insidiously: if the old CC channel is not fully cleaned up (e.g., a lingering process), the teloxide bot appears to work (no errors) but receives no messages. This is hard to diagnose because the bot starts without errors.

**Prevention:**
- The migration from CC channels to teloxide must be a hard cutover — not a gradual parallel deployment.
- Before starting teloxide polling: call `getMe` and `deleteWebhook` on the token to ensure no webhook is registered from a prior deployment.
- Remove `--channels` flag from agent shell wrappers in the same `rightclaw up` that starts teloxide processes — no intermediate state where both are active.
- Add a doctor check: `GET /getWebhookInfo` for each configured bot token; warn if a webhook is registered but teloxide is configured for polling.

**Warning signs:**
- teloxide bot starts with no errors but messages are not received
- No errors in teloxide logs but user messages disappear
- `curl https://api.telegram.org/bot<TOKEN>/getWebhookInfo` shows an active webhook URL

**Phase to address:** Phase 1 (migration design). Define the cutover procedure before writing any bot code.

---

## Minor Pitfalls

### Pitfall 13: `--output-format json` Requires All Output on stdout — Stderr Breaks Parsing

**What goes wrong:**
`claude -p --output-format json` writes the JSON envelope to stdout. However, CC also sometimes writes warnings, deprecation notices, or diagnostic messages to stderr. If the Rust subprocess reader captures both stdout and stderr in the same buffer (e.g., `stderr(Stdio::inherit())` goes to the terminal, but `stdout(Stdio::piped())` is read for JSON), this is fine. But if stderr is redirected to stdout via shell redirection or piped together, JSON parsing of the combined output fails.

**Prevention:**
- Always use separate `Stdio::piped()` for stdout and `Stdio::piped()` (or `Stdio::inherit()`) for stderr independently. Never use `2>&1` for a subprocess that must emit machine-readable JSON.
- Log stderr separately (to the agent's process-compose log stream).

**Phase to address:** Phase 1 (subprocess spawning). Simple discipline issue.

---

### Pitfall 14: `--bare` Mode vs. MCP Server Loading

**What goes wrong:**
The official CC docs now recommend `--bare` for scripted invocations (`-p` mode). `--bare` skips loading `.mcp.json`, which means the `rightmemory` MCP server is not available to `claude -p` in bare mode. If the bot invokes `claude -p --bare`, the agent loses access to its memory tools.

**Prevention:**
- Do NOT use `--bare` if the agent needs MCP tools.
- If startup time is a concern (bare is faster), explicitly pass the MCP config: `--mcp-config <agent_dir>/.mcp.json` instead of using `--bare`.
- Note that without `--bare`, `claude -p` loads all of `~/.claude/` context including CLAUDE.md — which may inject unexpected user-level instructions. For a bot-invoked session, this may or may not be desired.

**Phase to address:** Phase 1 (claude -p invocation flags). Make the decision explicit and document it.

---

### Pitfall 15: Telegram `message_thread_id` Not Set for Direct Messages — Session Key Must Handle NULL

**What goes wrong:**
For private DMs (not group topics), `message_thread_id` is never set. If the session key is naively `format!("{}:{}", chat_id, thread_id.unwrap())`, DMs cause a panic or require a special case. The `telegram_sessions` table must allow `thread_id` to be NULL and treat `(chat_id, NULL)` as a valid distinct key from `(chat_id, Some(0))`.

**Prevention:**
- `telegram_sessions` schema: `thread_id INTEGER NULL` with a UNIQUE constraint on `(chat_id, thread_id)` — this correctly treats NULLs as distinct (SQLite NULL != NULL in UNIQUE).
- Wait — SQLite UNIQUE with NULLs: SQLite treats each NULL as distinct for UNIQUE purposes, so `(chat1, NULL)` and `(chat1, NULL)` would both be insertable. This means two DM sessions with the same user could be created. Use `thread_id INTEGER NOT NULL DEFAULT 0` instead, with 0 meaning "no thread".
- Helper: `let thread_key = message.thread_id.map(|id| id.0).unwrap_or(0);`

**Phase to address:** Phase 1 (telegram_sessions schema). Simple but gets wrong easily.

---

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| teloxide + tokio | Using `tokio::main` with `Throttle` worker tasks that block | Use `#[tokio::main]` with multi-thread flavor; Throttle needs multiple tokio threads |
| claude -p + session resume | Updating stored session_id after each --resume call | Store only the root session_id (from first -p call); never update it from resume responses |
| notify + tokio | Default crossbeam-channel in debouncers blocks tokio threads | Disable crossbeam feature; use `blocking_send` into tokio mpsc |
| process-compose + teloxide process | process-compose `is_tty: true` needed for Claude Code but breaks teloxide | Teloxide does NOT need `is_tty: true`; it reads from API, not a TTY |
| Telegram polling + CC channels | Both polling same token simultaneously | Hard cutover required; teloxide and CC channels are mutually exclusive per token |
| claude -p stdout | Using `child.wait()` after piping stdout | Always use `child.wait_with_output()` to avoid 64KB pipe deadlock |
| Throttle adaptor ordering | `Throttle<CacheMe<Bot>>` | Correct order: `CacheMe<Throttle<Bot>>` — Throttle must be innermost |
| General topic replies | Passing `message_thread_id = Some(1)` | Omit thread_id for General topic; map Some(1) → None |
| SQLite telegram_sessions + DMs | Nullable thread_id with UNIQUE causing duplicate DM sessions | Use `thread_id NOT NULL DEFAULT 0` where 0 = no thread |
| Kill signal to bot | process-compose sends SIGKILL after timeout | Set `kill_on_drop(true)` on claude -p Child handles; implement graceful shutdown with timeout |

---

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|-------------|---------------|------------|
| Bot process architecture | Concurrent messages to same session | Per-session message queue (mpsc) before writing any bot dispatch code |
| Session key design | resume returning new session_id | Root-session-id-only storage; verify bug behavior with actual CC version in use |
| Session key design | CLAUDE_CONFIG_DIR break for --resume | Use HOME isolation, not CLAUDE_CONFIG_DIR |
| telegram_sessions schema | NULL thread_id for DMs | Use NOT NULL DEFAULT 0; test DM + group + forum in same run |
| telegram_sessions schema | thread_id=1 General topic | Normalize thread_id=1 to 0 in key derivation |
| Teloxide setup | Throttle deadlock | Add process-compose restart policy; test with message bursts |
| Teloxide setup | Polling + CC channels conflict | Doctor check for active webhook; enforce cutover sequence in rightclaw up |
| Subprocess spawning | stdout pipe deadlock | wait_with_output() always; test with 100KB+ response |
| Subprocess lifecycle | Zombie processes on restart | kill_on_drop(true) + graceful shutdown |
| Cron file watcher | crossbeam blocking tokio | Disable crossbeam at cargo dependency declaration |
| Cron file watcher | Editor rename-into-place pattern | Use notify-debouncer-full; reconciler must be idempotent |
| Rate limiting | Group 20 msg/min shared across agent topics | Stagger cron-triggered messages; implement RetryAfter handler |

---

## "Looks Done But Isn't" Checklist

- [ ] Send two Telegram messages within 1 second to the same thread — verify only one `claude -p` runs at a time (serialized queue)
- [ ] Start a session in a per-agent HOME context, kill the bot, restart it, verify `--resume OLD_SESSION_ID` works (not using CLAUDE_CONFIG_DIR)
- [ ] Verify stored session_id does NOT change after resume — it stays as the root ID
- [ ] Post a message in General topic (thread_id=1) — verify bot replies successfully (no API error)
- [ ] Post 25 messages in a group in quick succession — verify 429 handling with RetryAfter wait
- [ ] Start both CC channels and teloxide on the same token — verify only one receives messages (forced test of cutover logic)
- [ ] Generate a 100KB claude -p response — verify bot does not hang (pipe deadlock test)
- [ ] Kill bot process mid-response — verify no orphan claude processes remain (`ps aux | grep claude`)
- [ ] Edit a cron YAML file with vim — verify reconciler runs exactly once (debounce test)
- [ ] Disable crossbeam, verify notify events still fire correctly under tokio load

---

## Sources

### Claude Code Session Bugs (HIGH confidence — confirmed GitHub issues)
- [CC Issue #16103: --resume ignores CLAUDE_CONFIG_DIR](https://github.com/anthropics/claude-code/issues/16103) — closed "not planned" Feb 2026
- [CC Issue #1967: Resuming by session ID broken in print mode](https://github.com/anthropics/claude-code/issues/1967) — bug confirmed, fix status unclear
- [CC Issue #8069: SDK resume gives different session_id](https://github.com/anthropics/claude-code/issues/8069) — confirmed bug, unfixed
- [CC Headless Mode Docs](https://code.claude.com/docs/en/headless) — official --output-format json, --resume, --bare documentation

### Teloxide (HIGH confidence — official docs + GitHub issues)
- [Teloxide Throttle docs](https://docs.rs/teloxide/latest/teloxide/adaptors/struct.Throttle.html) — adaptor ordering, ChatId limitation
- [Teloxide Issue #516: Deadlock with Throttle](https://github.com/teloxide/teloxide/issues/516) — confirmed, no fix
- [Teloxide Issue #649: Adaptor ordering is not documented](https://github.com/teloxide/teloxide/issues/649) — ordering requirement
- [Teloxide CHANGELOG](https://github.com/teloxide/teloxide/blob/master/CHANGELOG.md) — v0.17.0 (Jul 2025), v0.16.0 (Jun 2025), v0.15.0 (Apr 2025) breaking changes

### Telegram Bot API (HIGH confidence — official documentation)
- [Telegram Forum API](https://core.telegram.org/api/forum) — topic creation, thread IDs, General topic behavior
- [grammY Flood Control Guide](https://grammy.dev/advanced/flood) — rate limits (30 msg/sec global, 20 msg/min group), RetryAfter handling
- [Telegram Bot API Reference](https://core.telegram.org/bots/api) — message_thread_id field, forum_topic handling

### Tokio Subprocess (HIGH confidence — official tokio docs)
- [tokio::process::Child](https://docs.rs/tokio/latest/tokio/process/struct.Child.html) — wait_with_output(), kill_on_drop()
- [tokio Issue #2685: Command leaves zombies when Child future is dropped](https://github.com/tokio-rs/tokio/issues/2685) — confirmed behavior

### notify Crate (MEDIUM confidence — forum posts + docs)
- [notify-rs GitHub](https://github.com/notify-rs/notify) — debouncer-mini vs debouncer-full comparison
- [notify-debouncer-mini docs](https://docs.rs/notify-debouncer-mini/latest/notify_debouncer_mini/) — crossbeam feature flag
- [notify-debouncer-full docs](https://docs.rs/notify_debouncer_full/latest/notify_debouncer_full/) — rename stitching
- [Rust forum: Problem with notify crate v6.1](https://users.rust-lang.org/t/problem-with-notify-crate-v6-1/99877) — duplicate events, PollWatcher panic

---
*Pitfalls research for: v3.0 Teloxide Bot Runtime — per-agent teloxide bots + claude -p session continuity + Rust cron runtime*
*Researched: 2026-03-31*
