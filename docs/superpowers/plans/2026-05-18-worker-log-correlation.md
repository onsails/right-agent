# Worker Log Correlation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make foreground Telegram worker logs unambiguous when multiple topics in the same group chat run concurrently.

**Architecture:** Keep routing behavior unchanged: workers remain keyed by `(chat_id, eff_thread_id)` and Claude sessions remain keyed by `root_session_id`. Add a small invocation log context inside `telegram::worker` and use it on the high-value lifecycle logs: invocation start, stream progress, invocation finish, and Telegram reply send. Update the descriptive session architecture doc so it matches the current host-side worker stream logs.

**Tech Stack:** Rust 2024, `tracing`, `tracing-subscriber` test capture, teloxide worker runtime, SQLite-backed session metadata.

---

## File Structure

- Modify `crates/bot/Cargo.toml`
  - Add `tracing-subscriber = { workspace = true }` under `[dev-dependencies]` so worker unit tests can capture formatted tracing output.
- Modify `crates/bot/src/telegram/worker.rs`
  - Add `InvocationLogContext` near the worker helper functions.
  - Add small log helper functions for the key lifecycle events.
  - Move `turn_id` creation early enough that start/stream/finish logs share the same correlation ID.
  - Add `session_uuid`, `eff_thread_id`, `key`, and `turn_id` to foreground invocation logs.
  - Add `session_uuid` to the Telegram reply send log.
- Modify `docs/architecture/sessions.md`
  - Fix drift: worker sessions now append host-side stream logs.
  - Document the required correlation fields for foreground worker logs.

No database schema, routing, session semantics, or Telegram behavior changes.

---

### Task 1: Add Testable Invocation Log Context

**Files:**
- Modify: `crates/bot/Cargo.toml`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add the dev dependency**

In `crates/bot/Cargo.toml`, add this line under `[dev-dependencies]`:

```toml
tracing-subscriber = { workspace = true }
```

- [ ] **Step 2: Write the failing context tests**

In `crates/bot/src/telegram/worker.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn invocation_log_context_carries_thread_session_and_turn() {
    let ctx = InvocationLogContext::new(
        -1003977763163,
        458,
        "f7d5a319-447f-4e58-ba8f-3c23dd476367".to_owned(),
        42,
    );

    assert_eq!(ctx.chat_id, -1003977763163);
    assert_eq!(ctx.eff_thread_id, 458);
    assert_eq!(ctx.key(), (-1003977763163, 458));
    assert_eq!(ctx.session_uuid, "f7d5a319-447f-4e58-ba8f-3c23dd476367");
    assert_eq!(ctx.turn_id, 42);
}

#[test]
fn invocation_log_context_distinguishes_parallel_topics_in_same_chat() {
    let agenda = InvocationLogContext::new(-1003977763163, 458, "agenda-session".to_owned(), 10);
    let danilo = InvocationLogContext::new(-1003977763163, 2, "danilo-session".to_owned(), 11);

    assert_ne!(agenda.key(), danilo.key());
    assert_eq!(agenda.chat_id, danilo.chat_id);
    assert_ne!(agenda.eff_thread_id, danilo.eff_thread_id);
    assert_ne!(agenda.session_uuid, danilo.session_uuid);
    assert_ne!(agenda.turn_id, danilo.turn_id);
}
```

- [ ] **Step 3: Run the targeted tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-bot invocation_log_context
```

Expected: FAIL with an error like `use of undeclared type InvocationLogContext`.

- [ ] **Step 4: Implement the log context**

In `crates/bot/src/telegram/worker.rs`, above `spawn_worker`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct InvocationLogContext {
    chat_id: i64,
    eff_thread_id: i64,
    session_uuid: String,
    turn_id: u64,
}

impl InvocationLogContext {
    fn new(chat_id: i64, eff_thread_id: i64, session_uuid: String, turn_id: u64) -> Self {
        Self {
            chat_id,
            eff_thread_id,
            session_uuid,
            turn_id,
        }
    }

    fn key(&self) -> SessionKey {
        (self.chat_id, self.eff_thread_id)
    }
}
```

- [ ] **Step 5: Run the targeted tests to verify they pass**

Run:

```bash
devenv shell -- cargo test -p right-bot invocation_log_context
```

Expected: PASS for both new tests.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/Cargo.toml crates/bot/src/telegram/worker.rs
git commit -m "test(bot): cover worker invocation log context"
```

---

### Task 2: Add Correlation Fields to Foreground Worker Logs

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write failing log-helper tests**

In `crates/bot/src/telegram/worker.rs`, inside `#[cfg(test)] mod tests`, add this test writer and tests:

```rust
#[derive(Clone)]
struct SharedLogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_worker_log<F>(f: F) -> String
where
    F: FnOnce(),
{
    let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = SharedLogWriter(std::sync::Arc::clone(&buffer));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_target(false)
        .without_time()
        .finish();

    tracing::subscriber::with_default(subscriber, f);

    let bytes = buffer.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn invoking_claude_log_includes_topic_session_and_turn() {
    let ctx = InvocationLogContext::new(
        -1003977763163,
        458,
        "f7d5a319-447f-4e58-ba8f-3c23dd476367".to_owned(),
        42,
    );

    let log = capture_worker_log(|| log_invoking_claude(&ctx, false, true));

    assert!(log.contains("invoking claude -p"), "{log}");
    assert!(log.contains("chat_id=-1003977763163"), "{log}");
    assert!(log.contains("eff_thread_id=458"), "{log}");
    assert!(log.contains("key=(-1003977763163, 458)"), "{log}");
    assert!(
        log.contains("session_uuid=f7d5a319-447f-4e58-ba8f-3c23dd476367"),
        "{log}"
    );
    assert!(log.contains("turn_id=42"), "{log}");
    assert!(log.contains("is_first_call=false"), "{log}");
    assert!(log.contains("sandboxed=true"), "{log}");
}

#[test]
fn stream_update_log_includes_topic_session_and_assistant_turn() {
    let ctx = InvocationLogContext::new(-1003977763163, 2, "2f4a29c9".to_owned(), 43);

    let log = capture_worker_log(|| log_stream_update(&ctx, 5, "tool call"));

    assert!(log.contains("tool call"), "{log}");
    assert!(log.contains("chat_id=-1003977763163"), "{log}");
    assert!(log.contains("eff_thread_id=2"), "{log}");
    assert!(log.contains("key=(-1003977763163, 2)"), "{log}");
    assert!(log.contains("session_uuid=2f4a29c9"), "{log}");
    assert!(log.contains("turn_id=43"), "{log}");
    assert!(log.contains("assistant_turn=5"), "{log}");
}

#[test]
fn claude_finished_log_includes_topic_session_turn_and_stream_log() {
    let ctx = InvocationLogContext::new(-1003977763163, 458, "f7d5a319".to_owned(), 44);
    let stream_log = std::path::Path::new("/tmp/f7d5a319.ndjson");

    let log = capture_worker_log(|| {
        log_claude_finished(&ctx, 0, false, false, false, stream_log, true);
    });

    assert!(log.contains("claude -p finished"), "{log}");
    assert!(log.contains("chat_id=-1003977763163"), "{log}");
    assert!(log.contains("eff_thread_id=458"), "{log}");
    assert!(log.contains("key=(-1003977763163, 458)"), "{log}");
    assert!(log.contains("session_uuid=f7d5a319"), "{log}");
    assert!(log.contains("turn_id=44"), "{log}");
    assert!(log.contains("exit_code=0"), "{log}");
    assert!(log.contains("stream_log=/tmp/f7d5a319.ndjson"), "{log}");
}
```

- [ ] **Step 2: Run the log-helper tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-bot log_includes_topic_session
```

Expected: FAIL with missing `log_invoking_claude`, `log_stream_update`, and `log_claude_finished`.

- [ ] **Step 3: Implement the log helper functions**

In `crates/bot/src/telegram/worker.rs`, below `InvocationLogContext`, add:

```rust
fn log_invoking_claude(ctx: &InvocationLogContext, is_first_call: bool, sandboxed: bool) {
    tracing::info!(
        chat_id = ctx.chat_id,
        eff_thread_id = ctx.eff_thread_id,
        key = ?ctx.key(),
        session_uuid = %ctx.session_uuid,
        turn_id = ctx.turn_id,
        is_first_call,
        sandboxed,
        "invoking claude -p"
    );
}

fn log_stream_update(ctx: &InvocationLogContext, assistant_turn: u32, formatted: &str) {
    tracing::info!(
        chat_id = ctx.chat_id,
        eff_thread_id = ctx.eff_thread_id,
        key = ?ctx.key(),
        session_uuid = %ctx.session_uuid,
        turn_id = ctx.turn_id,
        assistant_turn,
        "{formatted}"
    );
}

fn log_claude_finished(
    ctx: &InvocationLogContext,
    exit_code: i32,
    timed_out: bool,
    stopped: bool,
    was_bg_request: bool,
    stream_log_path: &std::path::Path,
    sandboxed: bool,
) {
    tracing::info!(
        chat_id = ctx.chat_id,
        eff_thread_id = ctx.eff_thread_id,
        key = ?ctx.key(),
        session_uuid = %ctx.session_uuid,
        turn_id = ctx.turn_id,
        exit_code,
        timed_out,
        stopped,
        was_bg_request,
        stream_log = %stream_log_path.display(),
        sandboxed,
        "claude -p finished"
    );
}
```

- [ ] **Step 4: Wire `InvocationLogContext` into `invoke_cc`**

In `invoke_cc`, create the turn ID and log context before spawning the child. Replace the existing `let sandboxed` / `tracing::info!` block around `invoking claude -p` with:

```rust
let sandboxed = ctx.ssh_config_path.is_some();
let turn_id = super::next_turn_id();
let log_ctx = InvocationLogContext::new(chat_id, eff_thread_id, session_uuid.clone(), turn_id);
log_invoking_claude(&log_ctx, is_first_call, sandboxed);
```

Then remove the later duplicate line:

```rust
let turn_id = super::next_turn_id();
```

Keep the existing stop-token insert, but it now uses the earlier `turn_id`:

```rust
ctx.stop_tokens
    .insert((chat_id, eff_thread_id), (turn_id, stop_token.clone()));
```

- [ ] **Step 5: Replace stream progress logs**

In the stream event branch, replace:

```rust
tracing::info!(?chat_id, turn = total_assistant_events, "{formatted}");
```

with:

```rust
log_stream_update(&log_ctx, total_assistant_events, &formatted);
```

- [ ] **Step 6: Replace the finish log**

Replace the existing `tracing::info!` block for `"claude -p finished"` with:

```rust
log_claude_finished(
    &log_ctx,
    exit_code,
    timed_out,
    stopped,
    was_bg_request,
    &stream_log_path,
    sandboxed,
);
```

- [ ] **Step 7: Add correlation fields to nearby warnings/errors**

For the high-signal warning/error logs in `invoke_cc`, add `eff_thread_id`, `key`, `session_uuid`, and `turn_id` fields. Update these call sites:

```rust
tracing::warn!(
    chat_id = log_ctx.chat_id,
    eff_thread_id = log_ctx.eff_thread_id,
    key = ?log_ctx.key(),
    session_uuid = %log_ctx.session_uuid,
    turn_id = log_ctx.turn_id,
    "stream read error: {e:#}"
);
```

```rust
tracing::warn!(
    chat_id = log_ctx.chat_id,
    eff_thread_id = log_ctx.eff_thread_id,
    key = ?log_ctx.key(),
    session_uuid = %log_ctx.session_uuid,
    turn_id = log_ctx.turn_id,
    child_pid = child.id(),
    "deadline fired ({}s) — sending SIGKILL to claude -p",
    CC_TIMEOUT_SECS,
);
```

```rust
tracing::info!(
    chat_id = log_ctx.chat_id,
    eff_thread_id = log_ctx.eff_thread_id,
    key = ?log_ctx.key(),
    session_uuid = %log_ctx.session_uuid,
    turn_id = log_ctx.turn_id,
    child_pid = child.id(),
    "stop_token cancelled — sending SIGKILL to claude -p",
);
```

```rust
tracing::warn!(
    chat_id = log_ctx.chat_id,
    eff_thread_id = log_ctx.eff_thread_id,
    key = ?log_ctx.key(),
    session_uuid = %log_ctx.session_uuid,
    turn_id = log_ctx.turn_id,
    child_pid,
    "child.wait failed: {e:#}"
);
```

```rust
tracing::error!(
    chat_id = log_ctx.chat_id,
    eff_thread_id = log_ctx.eff_thread_id,
    key = ?log_ctx.key(),
    session_uuid = %log_ctx.session_uuid,
    turn_id = log_ctx.turn_id,
    child_pid,
    elapsed_ms = wait_started.elapsed().as_millis() as u64,
    "child.wait timed out — slave is wedged; ProcessGroupChild::Drop will killpg on return",
);
```

```rust
tracing::warn!(
    chat_id = log_ctx.chat_id,
    eff_thread_id = log_ctx.eff_thread_id,
    key = ?log_ctx.key(),
    session_uuid = %log_ctx.session_uuid,
    turn_id = log_ctx.turn_id,
    stderr = %stderr_str,
    "CC stderr"
);
```

- [ ] **Step 8: Add session UUID to Telegram reply send logs**

In `spawn_worker`, replace the existing `"sending reply to Telegram"` log with:

```rust
tracing::info!(
    ?key,
    chat_id,
    eff_thread_id,
    session_uuid = %session_uuid,
    content_len = content.len(),
    html_len = html.len(),
    parts = parts.len(),
    ?reply_to,
    "sending reply to Telegram"
);
```

- [ ] **Step 9: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot invocation_log_context
devenv shell -- cargo test -p right-bot log_includes_topic_session
```

Expected: both commands PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/bot/Cargo.toml crates/bot/src/telegram/worker.rs
git commit -m "fix(bot): correlate worker logs by topic and session"
```

---

### Task 3: Update Session Architecture Documentation

**Files:**
- Modify: `docs/architecture/sessions.md`

- [ ] **Step 1: Update the Stream Logging section**

In `docs/architecture/sessions.md`, replace the current Stream Logging opening paragraph:

```markdown
CC is invoked with `--verbose --output-format stream-json`. Worker reads stdout
line-by-line via `tokio::io::AsyncBufReadExt`. For cron jobs, stdout is tee'd into
an NDJSON log inside the sandbox at `/sandbox/crons/logs/{job_name}-{run_id}.ndjson`
(agents can read these directly via `Read`). Per-job retention keeps the last 10 logs.
Worker sessions do not write stream logs.
```

with:

```markdown
CC is invoked with `--verbose --output-format stream-json`. Worker reads stdout
line-by-line via `tokio::io::AsyncBufReadExt`. Foreground worker sessions append
the raw stream JSON to host-side `~/.right/logs/streams/<session-uuid>.ndjson`;
the file name matches the active `sessions.root_session_id`.

For cron jobs, stdout is tee'd into an NDJSON log inside the sandbox at
`/sandbox/crons/logs/{job_name}-{run_id}.ndjson` (agents can read these directly
via `Read`). Per-job retention keeps the last 10 logs.
```

- [ ] **Step 2: Document foreground log correlation fields**

Immediately after the paragraph above, add:

```markdown
Foreground worker lifecycle logs must be attributable to a Telegram topic and a
Claude session. High-value logs (`invoking claude -p`, stream progress events,
`claude -p finished`, and `sending reply to Telegram`) include `chat_id`,
`eff_thread_id`, `key=(chat_id, eff_thread_id)`, `session_uuid`, and, for
per-invocation logs, `turn_id`. This is required because multiple Telegram
topics in the same group share one `chat_id` and may run concurrently.
```

- [ ] **Step 3: Verify the doc contains no stale claim**

Run:

```bash
devenv shell -- rg -n 'Worker sessions do not write stream logs|Foreground worker lifecycle logs' docs/architecture/sessions.md
```

Expected:

```text
docs/architecture/sessions.md:<line>:Foreground worker lifecycle logs must be attributable to a Telegram topic and a
```

and no match for `Worker sessions do not write stream logs`.

- [ ] **Step 4: Commit**

```bash
git add docs/architecture/sessions.md
git commit -m "docs: document worker stream log correlation"
```

---

### Task 4: Final Verification

**Files:**
- Verify: `crates/bot/Cargo.toml`
- Verify: `crates/bot/src/telegram/worker.rs`
- Verify: `docs/architecture/sessions.md`

- [ ] **Step 1: Run targeted worker tests**

Run:

```bash
devenv shell -- cargo test -p right-bot invocation_log_context
devenv shell -- cargo test -p right-bot log_includes_topic_session
devenv shell -- cargo test -p right-bot thinking_anchor_text
```

Expected: PASS.

- [ ] **Step 2: Run a package-level bot test check**

Run:

```bash
devenv shell -- cargo test -p right-bot
```

Expected: PASS. If there are pre-existing failures, capture the exact failing test names and confirm the new targeted tests still pass.

- [ ] **Step 3: Run the mandatory full workspace test suite**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. This is mandatory before claiming the implementation complete.

- [ ] **Step 4: Inspect git diff for scope**

Run:

```bash
devenv shell -- git diff -- crates/bot/Cargo.toml crates/bot/src/telegram/worker.rs docs/architecture/sessions.md
```

Expected: diff is limited to log correlation, tests, and the session logging doc. Do not touch the existing unrelated `crates/bot/src/cc/stream.rs` working-tree change unless it is already part of your assigned work.

- [ ] **Step 5: Final commit if needed**

If any verification-only edits were needed:

```bash
git add crates/bot/Cargo.toml crates/bot/src/telegram/worker.rs docs/architecture/sessions.md
git commit -m "fix(bot): complete worker log correlation"
```

---

## Self-Review

Spec coverage:

- Distinguish concurrent Telegram topics in the same group: Task 2 adds `eff_thread_id`, `key`, `session_uuid`, and `turn_id` to lifecycle logs.
- Avoid routing/session behavior changes: File Structure states no routing, DB, or Telegram behavior changes; tasks only touch logging, tests, and docs.
- Preserve observability for the actual confusion case: Task 2 covers `invoking claude -p`, stream progress, `claude -p finished`, and `sending reply to Telegram`.
- Keep architecture docs current: Task 3 updates `docs/architecture/sessions.md`.
- Verify with tests: Task 4 includes targeted, package-level, and full workspace tests.

Placeholder scan:

- No `TBD`, `TODO`, `implement later`, or unspecified edge handling remains.
- Every code-changing step includes exact code.
- Every test step includes exact commands and expected outcomes.

Type consistency:

- `InvocationLogContext::new(chat_id, eff_thread_id, session_uuid, turn_id)` is used consistently.
- `SessionKey` remains `(i64, i64)`.
- `turn_id` remains the existing `u64` value returned by `super::next_turn_id()`.
- `session_uuid` remains a `String` cloned from the active session lookup.
