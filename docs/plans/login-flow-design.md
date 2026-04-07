# Login Flow Design: Auth Error Detection + Telegram-Driven Login

## Problem

Claude Code inside OpenShell sandbox returns 403/401 when not authenticated. User has no way to log in — the sandbox is headless, no browser, no interactive terminal access from Telegram.

## Solution

Detect auth errors from CC output, spawn an interactive `claude auth login` process inside the sandbox via process-compose, scrape the OAuth URL from its logs, and send it as a clickable link in Telegram. A watcher probes auth status periodically and cleans up once login succeeds.

## Components

### 1. Auth Error Detection (`worker.rs`)

New pure function:

```rust
fn is_auth_error(stdout: &str) -> bool
```

Parses CC stdout JSON. Returns true when `is_error: true` AND `result` contains any of:
- `"403"`
- `"401"`
- `"Failed to authenticate"`
- `"Not logged in"`
- `"Please run /login"`

Called in `invoke_cc` after detecting non-zero exit code. When true AND sandboxed, returns a special auth-error variant instead of generic error reply.

When true AND not sandboxed, returns a plain Telegram message: "🔑 Claude needs to log in. Run `claude` in your terminal to authenticate."

### 2. Login Process in PC Template

Pre-declared in `process-compose.yaml.j2` for each sandbox-enabled agent:

```yaml
  login-{agent}:
    command: "ssh -t -F {home}/run/ssh/rightclaw-{agent}.ssh-config openshell-rightclaw-{agent} -- claude auth login"
    is_tty: true
    disabled: true
    availability:
      restart: "no"
    shutdown:
      signal: 15
      timeout_seconds: 10
```

- `disabled: true` — not started at boot, only on demand via PC API.
- `is_tty: true` — `claude auth login` needs TTY to produce the OAuth URL.
- `restart: "no"` — one-shot. Watcher kills it after successful auth.
- SSH config path and host alias are deterministic at template generation time.

Template changes:
- `BotProcessAgent` gets `home_dir: String` field (for SSH config path).
- New `login-{agent}` block rendered per agent when `!no_sandbox`.

### 3. PC Client Extensions (`pc_client.rs`)

Add two methods:

```rust
/// Start a disabled/stopped process.
pub async fn start_process(&self, name: &str) -> miette::Result<()>
// POST /process/start/{name}

/// Read process logs.
pub async fn get_process_logs(&self, name: &str, limit: usize) -> miette::Result<Vec<String>>
// GET /process/logs/{name}/0/{limit}
```

`stop_process` already exists. `start_process` is the missing counterpart.

### 4. Auth Watcher Task

Spawned from worker when auth error detected in sandbox mode. Lives as a tokio task.

**Guard:** `Arc<AtomicBool>` per agent — prevents duplicate watchers. Set to `true` when watcher starts, `false` when it exits. Stored in `WorkerContext`.

**Phase 1 — URL extraction (poll PC logs):**
- Every 2s, call `get_process_logs("login-{agent}", 50)`
- Scan lines for URL pattern: `https://` containing `anthropic` or `claude`
- Once found, send to Telegram as clickable link
- Timeout: 30s. If no URL found, warn in Telegram: "Could not extract login URL. Check process-compose TUI for login-{agent}."

**Phase 2 — Auth probe (poll CC inside sandbox):**
- Every 10s, run `claude -p --dangerously-skip-permissions --output-format json -- "say ok"` via SSH
- Check exit code: 0 = auth works
- On success:
  1. Stop `login-{agent}` via PC API
  2. Send Telegram: "✅ Logged in successfully. You can continue chatting."
  3. Log at INFO level
- Timeout: 5 minutes. Give up, log warning, notify Telegram.

**Cleanup:** On exit (success or timeout), set AtomicBool back to false.

### 5. Environment Wiring

Bot process needs PC API port to call back. Add to template:

```yaml
- RC_PC_PORT={{ pc_port }}
```

`pc_port` defaults to `18927` (existing `PC_PORT` constant). Read in bot startup, store in `WorkerContext`.

### 6. Telegram Message Flow

**Auth error detected (sandbox):**
> 🔑 Claude needs to log in. Starting login session...

**URL extracted from logs:**
> 🔑 Open this link to authenticate:\
> https://console.anthropic.com/oauth/...

**URL extraction timeout:**
> 🔑 Could not extract login URL automatically. Open the process-compose TUI and find **login-right** to authenticate.

**Auth probe success:**
> ✅ Logged in successfully. You can continue chatting.

**Auth probe timeout (5 min):**
> ⚠️ Login timed out. Send another message to retry.

**Auth error (no-sandbox):**
> 🔑 Claude needs to log in. Run `claude` in your terminal to authenticate.

## Data Flow

```
User sends message
    → worker invoke_cc()
    → CC returns exit 1, stdout has "403"
    → is_auth_error(stdout) = true
    → sandboxed?
        YES:
            → check auth_watcher_active AtomicBool
            → if false:
                → start "login-{agent}" via PC API
                → spawn auth_watcher task
                → send "Starting login session..." to Telegram
            → if true:
                → send "Login already in progress..." to Telegram
        NO:
            → send "Run claude in terminal" to Telegram

auth_watcher task:
    Phase 1: poll PC logs → extract URL → send to Telegram
    Phase 2: probe claude -p "say ok" via SSH → loop
        → success: stop login process, notify, exit
        → timeout: warn, exit
```

## Files Modified

| File | Change |
|------|--------|
| `crates/bot/src/telegram/worker.rs` | `is_auth_error()`, auth watcher spawn logic in `invoke_cc`, watcher task function |
| `crates/rightclaw/src/runtime/pc_client.rs` | `start_process()`, `get_process_logs()` |
| `crates/rightclaw/src/codegen/process_compose.rs` | `home_dir` field in `BotProcessAgent`, pass to template |
| `templates/process-compose.yaml.j2` | `login-{agent}` block, `RC_PC_PORT` env var |
| `crates/bot/src/lib.rs` | Read `RC_PC_PORT` env, thread through to WorkerContext |
| `crates/bot/src/telegram/handler.rs` | Thread `pc_port` through dptree injection |
| `crates/bot/src/telegram/dispatch.rs` | Accept and pass `pc_port` |

## Testing

- `is_auth_error()` — unit tests with various CC stdout patterns (403, 401, normal error, success)
- `get_process_logs()` parsing — unit test for JSON response
- Template rendering — assert `login-{agent}` block present when sandbox, absent when no-sandbox
- Auth watcher URL extraction — unit test with sample log lines
- PC client `start_process` — integration test pattern (same as existing methods)

## Open Questions

1. Does `claude auth login` inside a sandbox with `ssh -t` actually produce a URL on stdout? Needs smoke test. Fallback: `claude` interactive mode with `/login`.
2. Does the OAuth callback URL work from outside the sandbox? The redirect might go to localhost inside the container. May need cloudflared tunnel to be reachable.
3. PC API auth — if `PC_API_TOKEN` is set, all PC REST calls need the token header. Current `PcClient` doesn't handle this. May need to add if PC is started with auth.
