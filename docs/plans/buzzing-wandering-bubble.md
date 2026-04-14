# Fix cron: auth token, stream logging, failure notifications

## Context

Cron jobs fail silently due to three compounding bugs:
- `CLAUDE_CODE_OAUTH_TOKEN` not injected into cron's `claude -p` invocation (both SSH and direct paths)
- Cron uses `--output-format json` (no thinking, no streaming) while worker uses `stream-json --verbose`
- Failed cron runs never notify Telegram: `notify_json` only populated on success + delivery query filters `WHERE status = 'success'`

## Files to modify

| File | Change |
|------|--------|
| `crates/bot/src/cron.rs` | Auth token injection, stream-json output, failure notification population |
| `crates/bot/src/cron_delivery.rs` | Remove `status = 'success'` filter, handle failure rows |

## Changes

### 1. Auth token injection (`cron.rs`, lines 187-227)

Mirror worker.rs pattern. `execute_job` already has `agent_dir` available.

**SSH path** (line 195): prepend `export CLAUDE_CODE_OAUTH_TOKEN='...'` to script string, same as `worker.rs:880-883`:
```rust
let mut script = String::new();
if let Some(token) = crate::login::load_auth_token(agent_dir) {
    let escaped = token.replace('\'', "'\\''");
    script.push_str(&format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n"));
}
script.push_str(&format!("cd /sandbox && {claude_cmd}"));
```

**Direct path** (after line 225): add `c.env("CLAUDE_CODE_OAUTH_TOKEN", &token)` same as `worker.rs:911-912`.

### 2. Stream logging (`cron.rs`, lines 180-264)

**a)** Change output format args (line 180-181):
```
--verbose --output-format stream-json
```
Remove `--json-schema` — stream-json doesn't use it (structured output comes via `structured_output` field in the result event).

Wait — cron uses `--json-schema` for `CronReplyOutput`. With `stream-json`, the final `result` event still contains `structured_output`. Verify: keep `--json-schema` since CC respects it in both modes.

**b)** Replace `wait_with_output()` (line 237-252) with line-by-line streaming:
- Take stdout, wrap in `BufReader`, read lines
- Write each line to NDJSON log file at `~/.rightclaw/logs/streams/<job>-<run_id>.ndjson`
- Collect lines into buffer for final parsing
- Keep stderr piped for error capture

**c)** Update `parse_cron_output` (line 379-395): currently expects single JSON blob. With stream-json, need to find the last `{"type":"result",...}` line from the NDJSON stream. Extract from collected lines.

### 3. Failure notifications

**a)** `cron.rs` — after the success branch (line 288), add failure handling:
```rust
if output.status.success() {
    // ... existing success path, parse structured output ...
} else {
    // Build failure notification from stderr + raw stdout
    let error_msg = format!("Cron job `{}` failed (exit code {})\n\n{}",
        job_name,
        output.status.code().unwrap_or(-1),
        stderr_summary);
    let notify = CronNotify { content: error_msg, attachments: None };
    let notify_json = serde_json::to_string(&notify).ok();
    update_with_notify(&conn, &run_id, "unknown", notify_json);
}
```

Note: with stream-json, "output" is no longer `wait_with_output()` — adapt to use collected lines and the result event. If result event has `is_error: true`, treat as failure. If process exited non-zero without a result event, build error from stderr.

**b)** `cron_delivery.rs` — change query (line 25):
```sql
WHERE status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL
```

Same for `deduplicate_job()` (line 66) and update query (line 89).

## Execution order

1. Auth token injection — smallest, independent change
2. Failure notifications — independent of streaming change
3. Stream logging — largest change, touches output parsing

## Verification

1. `cargo check --workspace` after each step
2. Check existing tests: `cargo test -p rightclaw-bot` — `cron.rs` has `parse_cron_output` tests that need updating for stream-json parsing
3. Manual: trigger a cron job via Telegram, verify:
   - Auth token present in sandbox (check stream log for successful CC init)
   - NDJSON stream log created in `~/.rightclaw/logs/streams/`
   - Thinking events visible in stream log
   - Intentional failure (e.g. bad prompt) delivers Telegram notification
