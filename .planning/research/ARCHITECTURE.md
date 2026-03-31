# Architecture: Teloxide Bot Runtime Integration

**Project:** RightClaw v3.0
**Researched:** 2026-03-31
**Milestone:** Teloxide Bot Runtime — replacing CC channels with per-agent Rust bots

---

## Questions Answered

1. What changes to process-compose.yaml template (bot process instead of claude session)?
2. How does the bot process know which agent dir to use (env var injection)?
3. How does `claude -p --resume` work with sandbox settings.json (does it re-apply)?
4. Cron file watcher integration with tokio runtime?
5. Build order — what must ship first?

---

## 1. process-compose.yaml Template Changes

### What Changes

The current template (`templates/process-compose.yaml.j2`) runs a shell wrapper per agent:

```yaml
processes:
  {{ agent.name }}:
    command: "{{ agent.wrapper_path }}"      # shell script → exec claude
    working_dir: "{{ agent.working_dir }}"
    is_interactive: true                     # REQUIRED for CC interactive mode
```

The new template runs the teloxide bot binary directly:

```yaml
processes:
  {{ agent.name }}:
    command: "{{ agent.bot_binary_path }}"
    working_dir: "{{ agent.working_dir }}"
    # is_interactive: NOT needed — bot is not a TTY process
    environment:
      - "RC_AGENT_DIR={{ agent.working_dir }}"
      - "RC_AGENT_NAME={{ agent.name }}"
      - "RC_TELEGRAM_TOKEN={{ agent.telegram_token }}"
    availability:
      restart: "{{ agent.restart_policy }}"
      backoff_seconds: {{ agent.backoff_seconds }}
      max_restarts: {{ agent.max_restarts }}
    shutdown:
      signal: 15
      timeout_seconds: 30
```

### What Is Removed

- `is_interactive: true` — only needed for CC's TTY requirement; bot processes are daemons
- Shell wrapper file generation (`run/<agent>.sh`) — no longer needed
- `combined_prompt_path` / `--append-system-prompt-file` CLI arg injection — the bot composes system prompt from `agent_dir/.claude/system-prompt.txt`

### What Is Added

- `environment:` block in process-compose template with RC_ env vars
- `RC_AGENT_DIR` — absolute path to agent directory (bot's source of truth)
- `RC_AGENT_NAME` — agent name (for memory DB lookup, logging)
- `RC_TELEGRAM_TOKEN` — resolved token value (or `RC_TELEGRAM_TOKEN_FILE` pointing to file path)

### The `ProcessAgent` Rust Struct Change

`codegen/process_compose.rs` currently populates `ProcessAgent { name, wrapper_path, working_dir, ... }`.

`wrapper_path` is replaced by `bot_binary_path`. Recommendation: use `rightclaw bot --agent <name>` as a subcommand (same binary). `bot_binary_path` = result of `current_exe()`, same pattern already used for `.mcp.json` MCP server entry. No separate install step.

---

## 2. Env Var Injection: How the Bot Finds Its Agent Dir

### The Pattern

process-compose injects `RC_AGENT_DIR` as a process-level environment variable. The bot reads it at startup:

```rust
let agent_dir = std::env::var("RC_AGENT_DIR")
    .map(PathBuf::from)
    .expect("RC_AGENT_DIR must be set by process-compose");
```

From `RC_AGENT_DIR` the bot resolves everything:
- `agent_dir/SOUL.md` — personality
- `agent_dir/USER.md` — user context (optional)
- `agent_dir/AGENTS.md` — operational framework
- `agent_dir/.claude/system-prompt.txt` — pre-composed system prompt (written by `rightclaw up`)
- `agent_dir/memory.db` — SQLite for session mapping + memories
- `agent_dir/crons/` — cron spec directory (watched by file watcher)

### Token Resolution

Two paths from `agent.yaml`:
1. `telegram_token_file` — relative path within agent dir (preferred, no secret visible in PC YAML)
2. `telegram_token` — inline value injected as `RC_TELEGRAM_TOKEN` env var

`rightclaw up` writes either `RC_TELEGRAM_TOKEN_FILE` or `RC_TELEGRAM_TOKEN` into the process-compose environment block based on which field is configured in `agent.yaml`. The bot reads one of these at startup.

**Security note:** Inline tokens in env blocks are visible in the process-compose TUI (`environment:` section). Prefer `telegram_token_file`.

---

## 3. `claude -p --resume` with Sandbox settings.json

### How CC Loads settings.json

CC loads settings from these scopes (lowest to highest priority):
1. User scope: `~/.claude/settings.json` where `~` = `HOME` env var at process startup
2. Project scope: `<cwd>/.claude/settings.json`
3. Local scope: `<cwd>/.claude/settings.local.json`
4. Managed scope: system-level (`/etc/claude-code/managed-settings.json`)

Under the existing HOME override (`HOME=$AGENT_DIR`), the agent's `.claude/settings.json` is loaded as **both** user scope (via `~`) and project scope (via cwd). Both resolve to the same file. This is intentional and already works.

### What Happens with `claude -p --resume <uuid> "message"`

1. Bot sets `HOME=$AGENT_DIR`, `cwd=$AGENT_DIR` before spawning CC subprocess
2. CC loads `$AGENT_DIR/.claude/settings.json` — sandbox config **is applied**
3. Session data is loaded from `$AGENT_DIR/.claude/projects/<encoded-cwd>/<uuid>.jsonl`
4. Sandbox applies in `-p` mode exactly as in interactive mode — no behavioral difference for settings loading

**Critical:** Session storage path is `$HOME/.claude/projects/<encoded-cwd>/`. With `HOME=$AGENT_DIR`, sessions live inside the agent directory — isolated and correct. If the agent directory is renamed, existing sessions are orphaned (same issue exists today for interactive CC).

### Known Bug: `claude -p --resume` (MEDIUM confidence, verify before relying on)

GitHub issue #1967 (marked closed June 2025): `claude -p -r <session_id> "prompt"` fails with "No conversation found with session ID". Fix was merged but community reports regression in v1.0.51+.

**Mitigation options:**
- Test `claude -p --resume <uuid> "prompt"` with the deployed CC version before building thread continuity on it
- Fallback: `claude -p --session-id <uuid> "prompt"` — creates a new session with a deterministic UUID. This does NOT resume conversation history, but the UUID can serve as a stable key for the telegram_sessions mapping. Each `claude -p` call is stateless; the bot manages conversational state by injecting history into the prompt.
- Second fallback: `claude -c -p "prompt"` (continue most recent in cwd) — less precise, only viable for single-thread-per-agent scenarios

### Sandbox in `-p` Mode

Sandbox settings in `settings.json` apply in print mode. The `skipDangerousModePermissionPrompt`, `autoMemoryEnabled: false`, and filesystem restrictions all apply identically. No changes needed to `codegen/settings.rs` — it already generates correct sandbox config that works for both modes.

---

## 4. Cron File Watcher: tokio + notify

### Architecture

The teloxide bot process runs two concurrent tokio tasks:
1. **Bot dispatcher** — handles incoming Telegram updates via long-polling
2. **Cron runtime** — evaluates schedules, executes jobs at due time, watches spec files for changes

### File Watcher Pattern

`notify` crate (v7.x, current) + `tokio::sync::mpsc` channel as sync→async bridge:

```rust
// sync→async bridge for notify's sync EventHandler callback
let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::unbounded_channel();
let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
    let _ = watcher_tx.blocking_send(event);  // blocking_send is correct for sync→async
})?;
watcher.watch(&crons_dir, notify::RecursiveMode::NonRecursive)?;

// Cron runtime tokio task
tokio::spawn(async move {
    loop {
        tokio::select! {
            event = watcher_rx.recv() => {
                if let Some(Ok(_)) = event {
                    reload_cron_specs(&crons_dir, &mut scheduler).await;
                }
            }
            _ = scheduler.next_tick() => {
                execute_due_jobs(&agent_dir).await;
            }
        }
    }
});
```

`notify::recommended_watcher` selects the best OS primitive automatically: inotify on Linux, FSEvents on macOS, poll fallback. `blocking_send` is the correct pattern for sync→async boundary crossing in notify v7.

### Cron Execution

Each cron job invokes `claude -p` as a subprocess:
```
claude -p
  --system-prompt-file $AGENT_DIR/.claude/system-prompt.txt
  --session-id <deterministic-uuid-from-job-name>
  --dangerously-skip-permissions
  -- "<job prompt>"
```

The deterministic UUID (e.g., `uuid5(NAMESPACE, agent_name + job_name)`) keeps cron calls pseudostateful without requiring session resume reliability.

### Cronsync SKILL.md Changes

In v3.0, Cronsync SKILL.md is reduced to **file management only**: creating, editing, and deleting YAML spec files in `agent_dir/crons/`. The Rust runtime handles parsing, scheduling, file watching, and CC subprocess execution.

The CC-native `CronCreate/CronList/CronDelete` tools are no longer needed. This removes the v2.5 complexity: inline bootstrap on main thread (BOOT-01/BOOT-02), CHECK/RECONCILE split, CRITICAL guard. All of that was a workaround for CC's CronCreate being main-thread-only — it disappears entirely when cron management moves to Rust.

---

## 5. Build Order

### Dependency Graph

```
Phase A: DB schema — telegram_sessions migration
          ↓
Phase B: rightclaw-bot crate + agent dir loading
          |
          +-→ Phase C: System prompt composition (SOUL+USER+AGENTS → system-prompt.txt)
          |             [parallel with Phase B skeleton]
          ↓
Phase D: Telegram message handler → claude -p invocation + session mapping
          ↓
Phase E: process-compose template change + rightclaw up wiring
          ↓
Phase F: Cron runtime — tokio task + notify file watcher
          ↓
Phase G: Cronsync SKILL.md — file-management-only rewrite
```

### Phase A Must Ship First: DB Migration (telegram_sessions)

New V2 migration on top of existing V1 schema (via `rusqlite_migration`):

```sql
-- V2: telegram session mapping for teloxide bot
CREATE TABLE IF NOT EXISTS telegram_sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id       TEXT NOT NULL,
    thread_id     TEXT,             -- NULL for DMs and non-forum group chats
    session_uuid  TEXT NOT NULL,    -- UUID passed to claude --session-id / --resume
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(chat_id, thread_id)
);
```

`UNIQUE(chat_id, thread_id)` with nullable `thread_id` handles all three cases:
- DM: `(chat_id=user_id, thread_id=NULL)` — unique per user
- Forum group topic: `(chat_id=group_id, thread_id=topic_id)` — unique per topic
- Regular group: `(chat_id=group_id, thread_id=NULL)` — shared session for whole group

### Phase B: rightclaw-bot as CLI Subcommand

New subcommand `rightclaw bot --agent <name>` in `rightclaw-cli`. This reuses the existing binary — `bot_binary_path = current_exe()` in process-compose codegen, same pattern as the `memory-server` subcommand.

New crate `crates/rightclaw-bot/` or inline in `rightclaw` lib crate. Responsibilities:
- Reads `RC_AGENT_DIR`, `RC_AGENT_NAME`, `RC_TELEGRAM_TOKEN[_FILE]`
- Opens `memory.db` for session mapping
- Loads `system-prompt.txt`
- Runs teloxide dispatcher + cron runtime as concurrent tokio tasks

### Phase C: System Prompt Composition

`rightclaw up` gains a new codegen step: compose `agent_dir/.claude/system-prompt.txt` from `SOUL.md` + `USER.md` (if present) + `AGENTS.md` (if present).

`codegen/system_prompt.rs` is modified: `generate_system_prompt_txt(agent)` writes to `agent_dir/.claude/system-prompt.txt`. The old `generate_combined_prompt()` function (which built the `--append-system-prompt-file` content) is removed or repurposed.

The RightCron section of the current combined prompt is removed — cron management moves to Rust runtime.

### Phase E: process-compose Template + rightclaw up Wiring

`templates/process-compose.yaml.j2` updated. `ProcessAgent` struct gains `bot_binary_path`, `telegram_token`, drops `wrapper_path`. Shell wrapper generation loop in `cmd_up` is removed. System-prompt.txt composition is added to the per-agent loop.

### Phases F and G: Cron and Cronsync

These can ship independently of Telegram functionality. The cron runtime in Phase F depends on Phase B (bot process structure) but not on Phase D (Telegram handler). Cronsync SKILL.md Phase G depends on the Rust cron API being stable.

---

## Component Boundaries After v3.0

| Component | Responsibility | Status |
|-----------|---------------|--------|
| `rightclaw` CLI | Discovery, codegen, up/down/status | Modified |
| `rightclaw bot` subcommand (NEW) | Telegram dispatcher, CC invocation, cron runtime | New |
| `codegen/process_compose.rs` | PC YAML with bot binary + env block | Modified |
| `codegen/system_prompt.rs` | Compose SOUL+USER+AGENTS → system-prompt.txt | Modified |
| `codegen/shell_wrapper.rs` | Shell wrapper generation | Removed |
| `memory/` | SQLite store, V2 migration adds telegram_sessions | +migration |
| `memory_server.rs` (MCP) | MCP stdio server for CC memory tools | Unchanged |
| process-compose.yaml | Orchestrates bot processes (not CC sessions) | Regenerated |

---

## Data Flow: Message Handling

```
Telegram update
    ↓ teloxide dispatcher (RC_AGENT_DIR/memory.db)
    ↓ lookup (chat_id, thread_id) → session_uuid
    ↓ if new: gen UUID v4, INSERT telegram_sessions
    ↓ if existing: SELECT session_uuid, UPDATE last_used_at
    ↓
    spawn: claude -p
      --system-prompt-file $AGENT_DIR/.claude/system-prompt.txt
      --session-id <uuid>      (new thread)  OR
      --resume <uuid>          (existing, if reliable — verify first)
      --dangerously-skip-permissions
      cwd=$AGENT_DIR, HOME=$AGENT_DIR
      -- "<user message>"
    ↓
    read stdout → bot.send_message(chat_id, response)
```

---

## Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| `claude -p --resume` broken in some CC versions | HIGH | Test on target CC version; fallback to `--session-id` (stateless per call) |
| Session path changes if agent dir renamed | LOW | Document constraint; not blocking |
| process-compose env block exposes inline tokens in TUI | MEDIUM | Enforce `telegram_token_file` over `telegram_token` for production |
| Teloxide version compatibility with tokio 1.50 | LOW | teloxide 0.17.0 requires `tokio ^1.39`; 1.50 satisfies this |
| notify v7 watcher callback is sync; must not block | MEDIUM | Use `unbounded_channel` + `blocking_send` pattern, never do I/O in callback |

---

## Sources

- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference) — `--resume`, `--session-id`, `-p`, `--system-prompt-file` flags — HIGH confidence, official docs
- [Claude Code settings — scope hierarchy](https://code.claude.com/docs/en/settings) — project settings loading under HOME override — HIGH confidence
- [Bug #1967: Resuming by session ID in print mode](https://github.com/anthropics/claude-code/issues/1967) — MEDIUM confidence, issue closed but regression reported in v1.0.51+
- [teloxide 0.17.0 on docs.rs](https://docs.rs/teloxide/latest/teloxide/) — tokio ^1.39 dep, dispatcher architecture — HIGH confidence
- [teloxide Message struct — thread_id field](https://docs.rs/teloxide/latest/teloxide/types/struct.Message.html) — HIGH confidence
- [notify crate on docs.rs](https://docs.rs/notify) — EventHandler, recommended_watcher — MEDIUM confidence (version not pinned in search)
- Session storage path encoding pattern — MEDIUM confidence, multiple community sources agree
