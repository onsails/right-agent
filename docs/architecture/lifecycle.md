# Lifecycle and runtime flows

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Agent Lifecycle

```
right init  /  right agent init <name>
  ├─ `agent init` runs an interactive wizard (sandbox mode, network policy,
  │   telegram, chat IDs, stt, memory) and writes sandbox config + policy.yaml
  │   to the agent dir. `init` skips the wizard and also writes
  │   ~/.right/config.yaml + detects Telegram token / cloudflared tunnel.
  │   Permissive network policy is generated as hostless public `allowed_ips`;
  │   restrictive policy uses scoped DNS wildcard endpoints.
  │   Right MCP host access starts as a bootstrap unresolved endpoint; after
  │   sandbox READY, Right resolves host.openshell.internal inside the sandbox
  │   and hot-applies exact IPv4 /32 and IPv6 /128 allowed_ips.
  │   `right-config` owns global config loading, saving, and path helpers.
  ├─ Create ~/.right/agents/<name>/ with template files
  ├─ Write BOOTSTRAP.md, TOOLS.md, agent.yaml
  │   (IDENTITY.md, USER.md created later by bootstrap CC session;
  │   SOUL.md is created later by the bootstrap CC session from user choices)
  ├─ Generate .claude/settings.json, .claude.json
  └─ Symlink credentials from ~/.claude/

right up [--agents x,y] [--detach] [--no-sandbox]
  ├─ Discover agents from agents/ directory
  ├─ Per agent: resolve secret for token map (generate if missing)
  ├─ Generate agent-tokens.json
  ├─ Generate process-compose.yaml (minijinja)
  ├─ Generate cloudflared config, including `/dashboard/<agent>/.*` rules,
  │   and record whether content changed
  └─ Launch process-compose (TUI or detached)

right reload / running agent register / running agent destroy
  ├─ Discover agents from agents/ directory
  ├─ Run cross-agent codegen and record whether cloudflared config content changed
  ├─ POST /project/configuration to process-compose
  ├─ If cloudflared config changed: restart `cloudflared` via process-compose
  └─ Notify aggregator reload path when applicable

right bot --agent <name>  (spawned by process-compose)
  ├─ Resolve token, open data.db
  ├─ Per-agent codegen:
  │   ├─ settings.json, schemas
  │   ├─ .claude.json, credentials symlink, mcp.json
  │   ├─ TOOLS.md, skills install, policy.yaml
  │   └─ data.db init, git init, secret generation
  ├─ Clear Telegram webhook, verify bot identity
  ├─ Sandbox lifecycle (`right-openshell`):
  │   ├─ Check if sandbox exists via gRPC → reuse with exact multi-IP policy hot-reload
  │   ├─ Or create new: prepare staging dir, spawn sandbox, wait for READY
  │   └─ Generate SSH config for sandbox exec
  ├─ Initial sync (blocking): `right-platform-store` deploys platform files to /sandbox/.platform/ (content-addressed + symlinks)
  ├─ Identity mirror sync: pull IDENTITY.md / SOUL.md / USER.md from /sandbox
  │   into host agent_dir/ when present
  ├─ Start background sync task (every 5 min — `right-platform-store` re-deploys /sandbox/.platform/, GC stale entries)
  ├─ Start Claude health loop:
  │   ├─ immediate startup Haiku probe with strict MCP config
  │   ├─ hourly Haiku probe for Claude OAuth keepalive + agent-facing MCP init
  │   └─ stale `right` MCP needs-auth cache repair when `system/init` is unhealthy
  ├─ Start cron engine and refresh scheduler
  ├─ Start bot-owned UDS server with OAuth callback, progress, healthz,
  │   dashboard, and nested Telegram webhook routes; dashboard serves
  │   `/dashboard/<agent>/` static assets, explicit v1 read APIs for bootstrap,
  │   overview, activity, knowledge, usage, identity, health, and authenticated
  │   MCP management APIs, plus the learned-skill pin/unpin route.
  │   Health endpoints are explicit probes: overview reports injected status and
  │   never runs doctor or sandbox commands implicitly.
  ├─ Clear stale Telegram per-chat command scopes for current allowlist ids
  │   and legacy `allowed_chat_ids`, then register current command autocomplete
  │   in Default, AllPrivateChats, and AllGroupChats scopes; `/mcp` opens the
  │   dashboard MCP view
  ├─ Start teloxide long-polling dispatcher
  └─ On SIGINT/SIGTERM:
      ├─ Stop accepting Telegram updates
      ├─ Cancel workers that have not started foreground Claude work
      ├─ Request shutdown background handoff for active foreground turns
      ├─ Wait briefly for foreground handoff gates to drain
      ├─ Stop cron schedulers and bounded-drain running cron jobs
      ├─ Mark owned timed-out cron runs as shutdown-interrupted failures
      ├─ Wait up to the async-delivery shutdown deadline for the normal
      │   delivery loop to exit; abort and skip explicit flush if it cannot
      │   finish
      ├─ Flush already-ready async deliveries without idle-delay politeness
      │   when the normal loop exits cleanly
      └─ Tear down SSH control master and exit

Per message:
  ├─ Extract text + attachments from Telegram message
  ├─ Check if token request waiting for auth token → forward to intercept slot
  ├─ Route to worker task via DashMap<(chat_id, thread_id), Sender>
  ├─ Worker: debounce 500ms → download attachments → upload to sandbox inbox
  ├─ Format input: single text → raw string, multi/attachments → YAML
  ├─ Fail-closed sandbox gate: sandboxed agent + `SandboxHealth::Unavailable` →
  │   send cause-specific HTML message to Telegram, record affected chat, skip CC.
  │   Non-sandboxed agents pass through unconditionally.
  ├─ Pipe input to claude -p via stdin (SSH or direct)
  │   ├─ First message: --session-id <uuid> (new session)
  │   ├─ Subsequent: --resume <root_session_id> (persistent session)
  │   └─ Sessions persist across messages — agent retains full CC context
  ├─ Observe Claude Code `system/init`; if `right` MCP is unhealthy, schedule
  │   cache repair asynchronously without interrupting or retrying the turn
  ├─ If foreground exits via 600s timeout or 🌙 Background button:
  │   ├─ Insert async_runs row with kind='background', source_session_id =
  │   │   <main_session_id>, run_session_id = <run_id>
  │   ├─ Immediately fork Claude with --resume <main_session_id>
  │   │   --fork-session --session-id <run_id>
  │   ├─ Edit thinking message to per-reason banner ("⏱ Foreground hit 10-min
  │   │   limit — continuing in background…" / "🌙 Working in background…")
  │   └─ Worker returns; debounce frees, user can send next message
  ├─ Parse reply JSON with typed attachments
  ├─ Record accepted `used_skill_receipts` into `skill_lifecycle` in data.db
  ├─ Send text reply to Telegram
  ├─ Download outbound attachments from sandbox outbox → send to Telegram
  └─ Periodic cleanup: hourly, configurable retention (default 7 days)

Config change (right agent config):
  ├─ Writes agent.yaml
  ├─ Detects filesystem policy change via `right-openshell` gRPC GetSandboxPolicyStatus
  │   ├─ Network-only change: config_watcher → bot restart → hot-reload
  │   └─ Filesystem change: sandbox migration (below)
  ├─ config_watcher detects change (2s debounce)
  ├─ Bot exits with code 2
  ├─ process-compose restarts bot (on_failure policy)
  └─ Bot re-runs per-agent codegen with new config → resolves host alias in sandbox and applies fresh policy

Sandbox migration (filesystem policy change):
  ├─ Backup sandbox-only (SSH tar czpf)
  ├─ Create new sandbox right-<agent>-<YYYYMMDD-HHMM> with bootstrap policy
  ├─ Wait for READY + SSH ready
  ├─ Resolve host.openshell.internal inside the new sandbox
  ├─ Hot-apply exact Right MCP allowed_ips via openshell policy set --wait
  ├─ Restore files via SSH tar xzpf
  ├─ Write sandbox.name to agent.yaml
  ├─ Delete old sandbox (best-effort)
  └─ config_watcher restarts bot → picks up new sandbox

right agent backup <name> [--sandbox-only] [--include-rebuildable]
  ├─ Sandbox mode: SSH tar /sandbox/ → sandbox.tar.gz
  │   └─ Default excludes: sandbox/.cache, sandbox/.venv, sandbox/.npm, sandbox/.uv
  ├─ --include-rebuildable: include those rebuildable dirs for forensic backup
  ├─ No-sandbox mode: tar agent dir → sandbox.tar.gz, excluding data.db and data.db-* sidecars
  ├─ Full mode: + agent.yaml, allowlist.yaml, policy.yaml, VACUUM INTO data.db
  └─ Stored at ~/.right/backups/<agent>/<YYYYMMDD-HHMM>/; destroy --backup uses the same DB exclude contract

right agent rebootstrap <name> [-y]
  ├─ Confirm (yes/no) unless -y
  ├─ Stop <name>-bot via process-compose REST API (best-effort)
  ├─ Backup IDENTITY.md / SOUL.md / USER.md (host + sandbox copies)
  │   to ~/.right/backups/<agent>/rebootstrap-<YYYYMMDD-HHMM>/
  ├─ rm -f the same files from /sandbox/ via `right-openshell` gRPC exec_in_sandbox
  ├─ Remove host copies, write fresh BOOTSTRAP.md from BOOTSTRAP_INSTRUCTIONS
  ├─ UPDATE sessions SET is_active = 0 WHERE is_active = 1 in data.db
  └─ Restart <name>-bot if we stopped it

right agent init <name> --from-backup <path>
  ├─ Validate: agent must not exist, backup has sandbox.tar.gz + agent.yaml
  ├─ Read backup.json when present, or infer legacy source from backup path
  ├─ Resolve restore binding mode for clone-sensitive implicit defaults
  ├─ Fail before creating target agent state if binding mode is required
  ├─ Restore config/control-plane files to new agent dir (agent.yaml, allowlist.yaml, policy.yaml, data.db when present)
  ├─ Remove restored data.db-* sidecars; discard tar-extracted data.db; the canonical DB snapshot is backup/data.db only
  ├─ Normalize restored agent.yaml before codegen/sandbox creation
  ├─ Regenerate bootstrap policy before sandbox creation; copied policy IPs are treated as stale generated state
  ├─ Warn when clone restore copies explicit external state (Telegram, MCP, cron)
  ├─ Create new sandbox with timestamped name
  ├─ Resolve host.openshell.internal inside the new sandbox and hot-apply exact IPv4 /32 and IPv6 /128 allowed_ips
  ├─ Restore sandbox files via SSH tar
  │   └─ On upload failure: delete new sandbox best-effort, remove partial
  │      agent dir, and report the remote tar stderr when available
  ├─ Write sandbox.name to agent.yaml
  ├─ Reconcile host identity mirror from /sandbox
  └─ Run codegen + initial sync

Sandboxed identity files are restored from `sandbox.tar.gz` into `/sandbox`.
After restore and again on bot startup, Right Agent reconciles `IDENTITY.md`,
`SOUL.md`, and `USER.md` from `/sandbox` into the host `agent_dir/` mirror.
This mirror is required for control-plane checks, but sandboxed prompt assembly
reads `/sandbox` directly.

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

The `agent.yaml` STT schema (`right-agent-config::SttConfig` and
`right-agent-config::WhisperModel`) is owned by `right-agent-config`;
host-side model cache and ffmpeg helpers are owned by `right-stt`.

Models live at `~/.right/cache/whisper/ggml-<model>.bin` and are
downloaded at `right up` (skipped if ffmpeg is missing). Default model
is `small`; per-agent override via `agent.yaml`:

    stt:
      enabled: true
      model: small   # tiny | base | small | medium | large-v3

When ffmpeg is missing or the model file is absent, the bot still runs;
voice messages produce an error marker that the agent relays to the user.

## Learned skill lifecycle

Learned skill package content remains file-based. A learned package exists at
`.claude/skills/<skill_name>/SKILL.md`; the MCP finish path verifies that file
before accepting successful create/update finishes.

Lifecycle metadata is database-backed. `data.db.skill_lifecycle` is mutable
current state for active/stale/archived status, `created_by` provenance
(`foreground`, `probe_writer`, `curator`, `bundled`), use/patch counters,
activity timestamps, absorption target, and the pin flag. `skill_learning_events`
is append-only audit history for learning-tool start/finish calls.

Foreground usage is detected only from accepted `used_skill_receipts` in normal
assistant replies. The worker de-duplicates receipts per foreground turn and
bumps lifecycle usage for `rightx-*` packages.

The probe-writer and curator write skill files directly and report changes
through `mcp__right__skill_learning_start` /
`mcp__right__skill_learning_finish`. Successful finishes insert audit events and
update `skill_lifecycle`; background learning kinds do not send Telegram
learning messages. Curator automatic transitions read and write
`skill_lifecycle` rows and skip pinned, foreground-created, bundled, and already
archived rows.

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
