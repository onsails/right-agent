# Lifecycle and runtime flows

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `CLAUDE.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Agent Lifecycle

```
right init  /  right agent init <name>
  ├─ `agent init` runs an interactive wizard (sandbox mode, network policy,
  │   telegram, chat IDs, stt, memory) and writes sandbox config + policy.yaml
  │   to the agent dir. `init` skips the wizard and also writes
  │   ~/.right/config.yaml + detects Telegram token / cloudflared tunnel.
  ├─ Create ~/.right/agents/<name>/ with template files
  ├─ Write BOOTSTRAP.md, TOOLS.md, agent.yaml
  │   (IDENTITY.md, SOUL.md, USER.md created later by bootstrap CC session)
  ├─ Generate .claude/settings.json, .claude.json
  └─ Symlink credentials from ~/.claude/

right up [--agents x,y] [--detach] [--no-sandbox]
  ├─ Discover agents from agents/ directory
  ├─ Per agent: resolve secret for token map (generate if missing)
  ├─ Generate agent-tokens.json
  ├─ Generate process-compose.yaml (minijinja)
  ├─ Generate cloudflared config (if tunnel)
  └─ Launch process-compose (TUI or detached)

right bot --agent <name>  (spawned by process-compose)
  ├─ Resolve token, open data.db
  ├─ Per-agent codegen:
  │   ├─ settings.json, schemas
  │   ├─ .claude.json, credentials symlink, mcp.json
  │   ├─ TOOLS.md, skills install, policy.yaml
  │   └─ data.db init, git init, secret generation
  ├─ Clear Telegram webhook, verify bot identity
  ├─ Sandbox lifecycle:
  │   ├─ Check if sandbox exists via gRPC → reuse with policy hot-reload
  │   ├─ Or create new: prepare staging dir, spawn sandbox, wait for READY
  │   └─ Generate SSH config for sandbox exec
  ├─ Initial sync (blocking): deploy platform files to /sandbox/.platform/ (content-addressed + symlinks)
  ├─ Start background sync task (every 5 min — re-deploys /sandbox/.platform/, GC stale entries)
  ├─ Start cron engine, OAuth callback server, refresh scheduler
  └─ Start teloxide long-polling dispatcher

Per message:
  ├─ Extract text + attachments from Telegram message
  ├─ Check if token request waiting for auth token → forward to intercept slot
  ├─ Route to worker task via DashMap<(chat_id, thread_id), Sender>
  ├─ Worker: debounce 500ms → download attachments → upload to sandbox inbox
  ├─ Format input: single text → raw string, multi/attachments → YAML
  ├─ Pipe input to claude -p via stdin (SSH or direct)
  │   ├─ First message: --session-id <uuid> (new session)
  │   ├─ Subsequent: --resume <root_session_id> (persistent session)
  │   └─ Sessions persist across messages — agent retains full CC context
  ├─ If foreground exits via 600s timeout or 🌙 Background button:
  │   ├─ Insert cron_specs row with schedule_kind=BackgroundContinuation
  │   │   { fork_from: <main_session_id> } (encoded as `@bg:<uuid>`) and
  │   │   the continuation prompt body
  │   ├─ Edit thinking message to per-reason banner ("⏱ Foreground hit 10-min
  │   │   limit — continuing in background…" / "🌙 Working in background…")
  │   └─ Worker returns; debounce frees, user can send next message
  ├─ Parse reply JSON with typed attachments
  ├─ Send text reply to Telegram
  ├─ Download outbound attachments from sandbox outbox → send to Telegram
  └─ Periodic cleanup: hourly, configurable retention (default 7 days)

Config change (right agent config):
  ├─ Writes agent.yaml
  ├─ Detects filesystem policy change via gRPC GetSandboxPolicyStatus
  │   ├─ Network-only change: config_watcher → bot restart → hot-reload
  │   └─ Filesystem change: sandbox migration (below)
  ├─ config_watcher detects change (2s debounce)
  ├─ Bot exits with code 2
  ├─ process-compose restarts bot (on_failure policy)
  └─ Bot re-runs per-agent codegen with new config → applies fresh policy

Sandbox migration (filesystem policy change):
  ├─ Backup sandbox-only (SSH tar czpf)
  ├─ Create new sandbox right-<agent>-<YYYYMMDD-HHMM> with new policy
  ├─ Wait for READY + SSH ready
  ├─ Restore files via SSH tar xzpf
  ├─ Write sandbox.name to agent.yaml
  ├─ Delete old sandbox (best-effort)
  └─ config_watcher restarts bot → picks up new sandbox

right agent backup <name> [--sandbox-only]
  ├─ Sandbox mode: SSH tar /sandbox/ → sandbox.tar.gz
  ├─ No-sandbox mode: tar agent dir → sandbox.tar.gz
  ├─ Full mode: + agent.yaml, policy.yaml, VACUUM INTO data.db
  └─ Stored at ~/.right/backups/<agent>/<YYYYMMDD-HHMM>/

right agent rebootstrap <name> [-y]
  ├─ Confirm (yes/no) unless -y
  ├─ Stop <name>-bot via process-compose REST API (best-effort)
  ├─ Backup IDENTITY.md / SOUL.md / USER.md (host + sandbox copies)
  │   to ~/.right/backups/<agent>/rebootstrap-<YYYYMMDD-HHMM>/
  ├─ rm -f the same files from /sandbox/ via gRPC exec_in_sandbox
  ├─ Remove host copies, write fresh BOOTSTRAP.md from BOOTSTRAP_INSTRUCTIONS
  ├─ UPDATE sessions SET is_active = 0 WHERE is_active = 1 in data.db
  └─ Restart <name>-bot if we stopped it

right agent init <name> --from-backup <path>
  ├─ Validate: agent must not exist, backup has sandbox.tar.gz + agent.yaml
  ├─ Restore config files to new agent dir
  ├─ Create new sandbox with timestamped name
  ├─ Restore sandbox files via SSH tar
  ├─ Write sandbox.name to agent.yaml
  └─ Run codegen + initial sync

right down
  └─ POST /project/stop to process-compose REST API
```

## Voice transcription

`voice` and `video_note` Telegram attachments are transcribed on the host
inside `download_attachments` when `agent.yaml`'s `stt.enabled` is true and
ffmpeg is present. The transcript is wrapped in a Russian marker
(`[Пользователь надиктовал...]` / `[Пользователь записал кружок...]`) and
prepended to the user-message text. The original audio file is dropped on
the host — it never reaches the sandbox.

Models live at `~/.right/cache/whisper/ggml-<model>.bin` and are
downloaded at `right up` (skipped if ffmpeg is missing). Default model
is `small`; per-agent override via `agent.yaml`:

    stt:
      enabled: true
      model: small   # tiny | base | small | medium | large-v3

When ffmpeg is missing or the model file is absent, the bot still runs;
voice messages produce an error marker that the agent relays to the user.

## Login Flow (setup-token)

When `claude -p` returns 403/401 (auth error):

```
1. is_auth_error() detects auth failure in CC JSON output
2. spawn_token_request() — tokio task:
   ├─ Send "Claude needs authentication" notification to Telegram
   ├─ Send setup-token instructions to Telegram
   ├─ Delete stale token from auth_tokens table (if any)
   ├─ Create oneshot channel, store sender in auth_code_tx intercept slot
   ├─ Wait for token from Telegram (5-min timeout)
   ├─ Telegram handler intercepts next message as token
   ├─ Save token to auth_tokens table in data.db
   └─ Send "Token saved" confirmation to Telegram
3. On next claude -p: load token from auth_tokens, inject as
   CLAUDE_CODE_OAUTH_TOKEN env var (sandbox: export in shell script,
   no-sandbox: cmd.env())
4. On error/timeout: notify user, reset auth_watcher_active flag
```
