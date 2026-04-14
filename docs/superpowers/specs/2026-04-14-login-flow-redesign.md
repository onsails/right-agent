# Login Flow Redesign: setup-token via Telegram

## Problem

Claude Code CLI's interactive login (`claude auth login`) uses an ink-based TUI that cannot be driven programmatically — PTY writes are ignored, and the local callback server's OAuth codes are bound to a mismatched redirect_uri. Every approach to automate the interactive flow has failed.

## Solution

Use `claude setup-token` — a CLI command that generates a long-lived (1-year) OAuth token. The user runs it on their own machine (where they have a browser), copies the token, and sends it to the Telegram bot. The bot stores the token and passes it as `CLAUDE_CODE_OAUTH_TOKEN` env var to all subsequent `claude -p` invocations.

No PTY, no callback server, no port discovery, no sandbox policy changes.

## Design

### User Flow

1. User sends a message to the bot
2. `claude -p` returns auth error (401/403 / "Not logged in")
3. Bot checks `auth_tokens` table — if token exists, it's stale: delete it
4. Bot sends Telegram message:
   > To authenticate, run this on your machine:
   > ```
   > claude setup-token
   > ```
   > Then send me the token it prints.
5. User runs `claude setup-token`, completes OAuth in browser, copies token
6. User pastes token in Telegram
7. Bot intercepts message as token (via existing `auth_code_tx` intercept slot)
8. Bot saves token to `auth_tokens` table in memory.db
9. Bot retries the original request with `CLAUDE_CODE_OAUTH_TOKEN` set

### Storage

New table in memory.db (via rusqlite_migration):

```sql
CREATE TABLE IF NOT EXISTS auth_tokens (
    token TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

Single row per agent (one agent = one memory.db). UPSERT on save: delete all + insert.

### invoke_cc Changes

In `invoke_cc()` (worker.rs), before spawning `claude -p` via SSH:

1. Open DB, query `SELECT token FROM auth_tokens LIMIT 1`
2. If token exists, add `CLAUDE_CODE_OAUTH_TOKEN=<token>` to the SSH command env

For sandbox mode, the env var is passed through SSH: `ssh ... -- env CLAUDE_CODE_OAUTH_TOKEN=<token> claude -p ...`

For no-sandbox mode, set it on the `Command` directly.

### Auth Error Handling Changes

Replace `spawn_auth_watcher()` (which spawned PTY login) with `spawn_token_request()`:

1. Check `auth_tokens` — if exists, delete (token is stale)
2. Send instruction message to Telegram
3. Store oneshot sender in `auth_code_tx` intercept slot (reuse existing mechanism)
4. Wait for token from Telegram (with 5-min timeout)
5. Save token to `auth_tokens`
6. Send "Token saved. You can continue chatting." to Telegram

### What Gets Deleted

- `login.rs` — entire file gutted. Keep `LoginEvent` enum (simplified), remove everything else (PTY helper, callback server, port discovery, URL parsing)
- `/dev/tty` and `/dev/pts` from policy — not needed
- `urlencoding` dependency — not needed
- All the callback/curl/ss infrastructure

### What Stays

- `is_auth_error()` detection in worker.rs
- `auth_watcher_active` flag (prevents duplicate requests)
- `auth_code_tx` / `InterceptSlots` mechanism in handler.rs (reused for token intercept)
- `LoginEvent` enum (simplified to just Done/Error)

### Edge Cases

| Case | Handling |
|------|----------|
| Token expired (1 year) | Next `claude -p` returns auth error → bot deletes stale token, requests new one |
| User sends invalid token | `claude -p` will fail with auth error on next invocation → same flow |
| User sends message before token | Normal message routing (auth_code_tx slot is empty) |
| Multiple auth errors in parallel | `auth_watcher_active` flag prevents duplicates (existing) |
| No-sandbox mode | Same flow, env var set on Command directly instead of SSH |

### Security

- Token stored in agent's memory.db (per-agent isolation)
- Token never logged (only length logged)
- Token passed via env var, not command-line arg (not visible in `ps`)
- memory.db has mode 0600 on Linux
