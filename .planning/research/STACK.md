# Stack Research: v3.0 Teloxide Bot Runtime

**Domain:** Rust Telegram bot + cron runtime + claude -p session management
**Researched:** 2026-03-31
**Confidence:** HIGH (all versions confirmed via crates.io)

## Scope

Delta-research for the v3.0 milestone. Covers ONLY the four new capability areas:
1. teloxide Telegram bot (replaces CC channels)
2. `claude -p --resume` session management
3. File watching on `crons/` directory
4. Cron schedule parsing and execution

Validated existing stack (tokio, serde, reqwest, rusqlite, minijinja, etc.) is NOT re-evaluated.

---

## New Dependencies

### 1. Telegram Bot — teloxide

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| teloxide | 0.17 | Telegram bot framework | Dominant Rust Telegram bot library. 173K downloads. Long polling is built-in (no webhook infra needed). Handler model is tokio-native filter chains — maps cleanly to "dispatch message to agent session" pattern. Active maintenance: 0.17.0 released July 2025 with Telegram Bot API 9.1 support. Minimum features `["macros"]` — no storage or webhook features needed. |

```toml
teloxide = { version = "0.17", features = ["macros"] }
```

**Features to use:** `macros` only. Do NOT enable `sqlite-storage`, `redis-storage`, or `webhooks-axum` — all unnecessary for this use case. Session mapping is in the existing `memory.db` via a new `telegram_sessions` table.

**Long polling pattern:** `Dispatcher::builder(bot, schema).build().dispatch().await` runs the update loop natively on the tokio runtime. Each bot process is a separate process-compose entry, one per agent.

**Breaking changes in 0.17.0 to know about:** `TransactionPartnerUser` restructured with `kind` field, `ChatFullInfoPublicKind::Supergroup` now boxed. Not relevant to basic message handling — these are Telegram API types that don't affect the `Message → text → dispatch to claude` flow.

---

### 2. claude -p Session Management

No new Rust crate needed. This is a `std::process::Command` invocation pattern.

**Exact flags (verified against official Claude Code docs):**

```bash
# First invocation — capture session ID
claude -p "prompt" \
  --output-format json \
  --append-system-prompt-file /path/to/system-prompt.txt \
  --allowedTools "Bash,Read,Edit,Write" \
  | jq -r '.session_id'

# Resume session
claude -p "follow-up prompt" \
  --resume "$session_id" \
  --output-format json \
  --allowedTools "Bash,Read,Edit,Write"
```

**Key behavioral facts (HIGH confidence, from official docs):**

- `--output-format json` returns a JSON object with `session_id` field at top level
- `--resume <session_id>` continues a specific conversation by ID
- `--continue` resumes the most recent session (risky in concurrent multi-agent context — avoid)
- `--bare` skips auto-discovery of hooks/skills/MCP/CLAUDE.md — use for deterministic scripted calls. Official docs state it will become the default `-p` mode in a future release. **Recommendation: add `--bare` to cron executions** to avoid agent's local CLAUDE.md interfering with scheduled task invocations.
- `--append-system-prompt-file` works in `-p` mode — use for system-prompt.txt composed from SOUL.md + USER.md + AGENTS.md

**Session ID storage:** Store `(telegram_thread_id, session_id)` in `memory.db` `telegram_sessions` table. Look up on each incoming message; create new session (first invocation without `--resume`) if none exists.

**Known issue:** CC `--resume` bug (GitHub issue #15837) where context is not fully restored. This is a CC upstream issue; design the system to be tolerant — if session context is lost, agents should be able to reconstruct from SOUL.md + USER.md + AGENTS.md system prompt. The stateless `-p` model means each call is independently valid even if history is unavailable.

---

### 3. File Watching — notify + notify-debouncer-full

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| notify | 8.2 | Cross-platform filesystem events | Latest stable (8.2.0 released 2025-08-03). Uses inotify on Linux, FSEvents on macOS. `recommended_watcher()` picks the best backend automatically. Breaking change in 8.0: MSRV raised to 1.77, `notify-types` dependency updated to 2.0. |
| notify-debouncer-full | 0.7 | Debounce rapid file events | Wraps notify 8.2 (^8.2.0 dep). Required because editors write YAML files as multiple rapid events (create, modify, close). Without debouncing, the cron loader fires 3-5 times per save. 9M+ downloads. |

```toml
notify = "8.2"
notify-debouncer-full = "0.7"
```

**Integration pattern with tokio:** notify uses a synchronous channel internally. Bridge to tokio via `tokio::sync::mpsc` + a spawned blocking task:

```rust
let (tx, mut rx) = tokio::sync::mpsc::channel(16);
let mut debouncer = new_debouncer(Duration::from_millis(500), None, move |res| {
    let _ = tx.blocking_send(res);
})?;
debouncer.watcher().watch(&crons_dir, RecursiveMode::NonRecursive)?;

tokio::spawn(async move {
    while let Some(events) = rx.recv().await {
        // reload cron specs
    }
});
```

**Watch mode:** `RecursiveMode::NonRecursive` on the `crons/` directory. No subdirectory recursion needed — cron specs are flat YAML files directly in `crons/`.

**Do NOT use notify-debouncer-mini:** It is the older, simpler debouncer. `notify-debouncer-full` provides richer event information (file path, event kind) needed to distinguish create/modify/delete for cron reconciliation.

---

### 4. Cron Schedule Parsing

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| cron | 0.16 | Parse cron expressions, compute next run time | Lightweight, focused, no async overhead. 7,740 downloads (latest 0.16.0 published 2026-03-25). Supports standard 7-field format: `sec min hour day-of-month month day-of-week year`. Returns `chrono::DateTime<Utc>` for next execution. No background runtime — pure computation. |

```toml
cron = "0.16"
```

**Why `cron` instead of `tokio-cron-scheduler`:**

`tokio-cron-scheduler` (0.15.1) is a heavier framework that embeds its own background scheduler loop, optional Postgres/NATS persistence, and manages job state. RightClaw already has a cron runtime design (tokio tasks + file watcher); it needs **only schedule parsing** — not a second scheduler framework competing with the existing runtime.

The `cron` crate is the right primitive: parse an expression string → get next `DateTime<Utc>` → `tokio::time::sleep_until(next.into())`. Full control, no hidden state, no framework to fight.

**Usage pattern:**

```rust
use cron::Schedule;
use std::str::FromStr;
use chrono::Utc;

let schedule = Schedule::from_str("0 30 9 * * Mon-Fri *")?;
let next = schedule.upcoming(Utc).next()
    .ok_or_else(|| anyhow!("no upcoming execution"))?;

tokio::time::sleep_until(next.into()).await;
// invoke claude -p
```

**chrono dependency:** `cron` 0.16 depends on `chrono`. Likely already in workspace transitively (via teloxide or other deps). If not, add `chrono = "0.4"` to the crate that drives scheduling.

---

## What NOT to Add

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `tokio-cron-scheduler` | Framework overhead — embeds own scheduler loop, Postgres/NATS storage, job UUID management. RightClaw owns the cron runtime; needs only expression parsing. | `cron` crate (pure parsing) + custom tokio task loop |
| `notify-debouncer-mini` | Older/simpler API, less event detail | `notify-debouncer-full` |
| `notify` 9.x rc | Release candidate, not stable | `notify` 8.2 (stable) |
| `teloxide` storage features (`sqlite-storage`, `redis-storage`) | Teloxide dialogue storage is for multi-step bot conversations. Session mapping belongs in existing `memory.db`. | `telegram_sessions` table in rusqlite |
| `teloxide` `webhooks-axum` feature | Requires running an HTTP server and public URL. Long polling is simpler and sufficient for per-agent bots. | Default long polling via `dispatch()` |
| `axum` / `tower` | No HTTP server needed — bot uses long polling | Not needed |
| `pretty_env_logger` | Already using `tracing-subscriber` | Existing tracing setup |

---

## Cargo.toml Delta

Add to the crate that hosts the bot and cron runtime (likely a new `rightclaw-bot` subcrate or added to `rightclaw` crate):

```toml
[dependencies]
teloxide = { version = "0.17", features = ["macros"] }
notify = "8.2"
notify-debouncer-full = "0.7"
cron = "0.16"
# chrono is a transitive dep of cron; add explicitly if not already present:
# chrono = "0.4"
```

No dev dependencies needed for these additions.

---

## Architecture Notes

**teloxide bot process model:** One `rightclaw-bot` binary per agent. process-compose entry per agent. Each bot process:
1. Reads `TELEGRAM_BOT_TOKEN` from env (set by rightclaw up via agent.yaml)
2. Reads `RC_AGENT_NAME` to know which agent dir / memory.db to use
3. On message: looks up `thread_id → session_id` in `telegram_sessions` table
4. Invokes `claude -p <message> [--resume <session_id>] --output-format json --bare`
5. Stores new `session_id` if first message in thread
6. Replies to Telegram with the text response from JSON output

**Cron runtime model:** Same binary (or separate tokio task in `rightclaw-bot`):
1. File watcher on `~/.rightclaw/agents/<name>/crons/` via notify-debouncer-full
2. In-memory cron registry (HashMap of spec-file-name → Schedule + last-run)
3. `tokio::time::interval`-based tick loop (e.g. every 30s): check which jobs are due
4. For each due job: invoke `claude -p <cron-prompt> --bare --allowedTools "Bash,Read,Edit,Write"`
5. On file-change event: reload changed spec files, update registry

**System prompt composition** (no new crates needed): Read SOUL.md + USER.md + AGENTS.md from agent dir, concatenate with separators, write to `agent/.claude/system-prompt.txt` on `rightclaw up`. `claude -p` call uses `--append-system-prompt-file` pointing to that file.

---

## Sources

- [teloxide 0.17.0 on crates.io](https://crates.io/crates/teloxide) — version confirmed 2026-03-31
- [teloxide README on GitHub](https://github.com/teloxide/teloxide/blob/master/README.md) — minimal feature config
- [teloxide 0.17.0 CHANGELOG](https://github.com/teloxide/teloxide/blob/master/CHANGELOG.md) — breaking changes reviewed
- [notify 8.2.0 on crates.io](https://crates.io/crates/notify) — version confirmed 2026-03-31, latest stable
- [notify 8.2.0 docs.rs](https://docs.rs/notify/8.2.0/notify/) — API and tokio integration pattern
- [notify-debouncer-full 0.7.0 on crates.io](https://crates.io/crates/notify-debouncer-full) — version confirmed 2026-03-31, requires notify ^8.2.0
- [cron 0.16.0 on crates.io](https://crates.io/crates/cron) — version confirmed 2026-03-31 (published 2026-03-25)
- [tokio-cron-scheduler 0.15.1 on crates.io](https://crates.io/crates/tokio-cron-scheduler) — reviewed and rejected
- [Claude Code headless/CLI docs](https://code.claude.com/docs/en/headless) — --resume, --output-format json, --bare flags confirmed HIGH confidence
- [Claude Code session management (DeepWiki)](https://deepwiki.com/anthropics/claude-code/3.3-session-and-conversation-management) — session storage path `~/.claude/sessions/`
- [CC --resume bug GitHub #15837](https://github.com/anthropics/claude-code/issues/15837) — context restoration issue known upstream

---
*Stack research for: RightClaw v3.0 Teloxide Bot Runtime*
*Researched: 2026-03-31*
