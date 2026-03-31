# Phase 23: Bot Skeleton - Context

**Gathered:** 2026-03-31
**Status:** Ready for planning

<domain>
## Phase Boundary

Create the `rightclaw bot --agent <name>` subcommand as a long-running process: resolves agent dir, opens memory.db, reads agent config, and starts a teloxide no-op dispatcher. Wires full SIGTERM shutdown infrastructure (even with empty subprocess list). No message handling, no claude -p invocation — that is Phase 25.

</domain>

<decisions>
## Implementation Decisions

### Crate Structure

- **D-01:** New `crates/bot` workspace crate (`rightclaw-bot` package). Teloxide and all bot-related code lives here. Must NOT be added to `rightclaw` (core lib) or `rightclaw-cli`. Rationale: single-responsibility — core lib stays lean; future non-Telegram bot support (Slack, webhooks, etc.) will also live in `crates/bot` under separate modules.
- **D-02:** `rightclaw-cli` gains a `Bot` variant in its `Commands` enum and delegates to `rightclaw-bot`. Dependency: `rightclaw-cli` → `rightclaw-bot` → `rightclaw`.

### allowed_chat_ids Field

- **D-03:** `allowed_chat_ids: Vec<i64>` field added to `AgentConfig` in `rightclaw` crate (not bot crate — it's parsed as standard agent.yaml config). Type is `i64` — Telegram chat IDs are signed 64-bit integers.
- **D-04:** `#[serde(default)]` on the field — absent from agent.yaml = empty vec.
- **D-05:** Empty vec = **block all** (secure default). Bot starts successfully with empty list, but emits a `tracing::warn!` at startup: "allowed_chat_ids is empty — all incoming messages will be dropped". Operator cannot miss it, but the process doesn't crash (valid config for a bot that only sends/doesn't receive).
- **D-06:** Filtering logic: incoming update's chat_id checked against the set. If not present → silently drop (no reply, no log at INFO level). Per BOT-05.

### SIGTERM Shutdown Machinery

- **D-07:** **Full structure wired in Phase 23**, even though no subprocesses exist yet. `Arc<Mutex<Vec<tokio::process::Child>>>` shared state. SIGTERM handler iterates the vec and calls `child.kill().await` on each before exiting. Phase 25 adds to the vec — no structural changes needed then.
- **D-08:** Use `tokio::signal::unix::signal(SignalKind::terminate())` for SIGTERM + `tokio::signal::ctrl_c()` for SIGINT. Both trigger the same shutdown path.
- **D-09:** Graceful shutdown sequence: signal received → iterate in-flight children and kill each → wait for teloxide dispatcher to stop → process exits 0.

### Agent Directory Resolution

- **D-10:** `rightclaw bot --agent <name>` resolves agent dir from RIGHTCLAW_HOME via the same logic as `rightclaw up` (search `$RIGHTCLAW_HOME/agents/<name>/`). No requirement for `RC_AGENT_DIR` in env for local development.
- **D-11:** `RC_AGENT_DIR` environment variable is honoured as an **override** — if set, skips dir search and uses it directly. This is how process-compose (Phase 26, PC-01) will inject the path.
- **D-12:** `--home` CLI flag (already on root `Cli` struct) controls RIGHTCLAW_HOME for resolution.

### Bot Token Resolution

- **D-13:** Token resolution order: (1) `RC_TELEGRAM_TOKEN` env var, (2) `RC_TELEGRAM_TOKEN_FILE` env var (read file contents), (3) `agent.yaml` `telegram_token_file` field, (4) `agent.yaml` `telegram_token` field. First non-empty value wins. Error if none found.
- **D-14:** `RC_TELEGRAM_TOKEN` and `RC_TELEGRAM_TOKEN_FILE` are injected by process-compose in Phase 26 (PC-01). For Phase 23 local testing, set manually.

### Teloxide Adaptor Ordering

- **D-15:** `CacheMe<Throttle<Bot>>` — CacheMe wraps Throttle, NOT the other way around. Required per BOT-03 to prevent teloxide issue #516 deadlock.

### DB Open

- **D-16:** Bot opens `memory.db` on startup via `memory::open_db(agent_dir.join("memory.db"))`. If DB doesn't exist, `open_db` creates it (existing behaviour from Phase 16). Startup error if DB cannot be opened.

### Claude's Discretion

- Exact module layout inside `crates/bot/src/` — reasonable to have `lib.rs`, `bot.rs`, `config.rs` etc.
- Whether to use `tokio_util::sync::CancellationToken` vs `AtomicBool` for shutdown signalling — Claude decides based on what integrates cleanest with teloxide dispatcher's shutdown mechanism.
- Whether `allowed_chat_ids` becomes a `HashSet<i64>` internally for O(1) lookup — Claude decides.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` §BOT-01, BOT-03, BOT-04, BOT-05 — exact requirements for Phase 23

### Existing code that bot crate depends on
- `crates/rightclaw/src/agent/types.rs` — `AgentConfig`, `AgentDef`; `allowed_chat_ids` field must be added here
- `crates/rightclaw/src/agent/discovery.rs` — `parse_agent_config`, `discover_agents`, `validate_agent_name`; resolution logic mirrors `rightclaw up`
- `crates/rightclaw/src/memory/mod.rs` — `open_db` / `open_connection`; bot calls this on startup
- `crates/rightclaw/src/config.rs` — `resolve_home`; bot uses this for RIGHTCLAW_HOME resolution
- `crates/rightclaw-cli/src/main.rs` — existing `Commands` enum; add `Bot` variant here

### Prior phase context
- `.planning/phases/22-db-schema/22-CONTEXT.md` — telegram_sessions schema decisions (referenced by Phase 25 CRUD)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `config::resolve_home()`: resolves RIGHTCLAW_HOME — bot uses this directly
- `agent::parse_agent_config()`: reads agent.yaml into `AgentConfig` — bot calls this after dir resolution
- `memory::open_db()`: opens SQLite with WAL + migrations — bot calls this on startup
- `agent::validate_agent_name()`: validates the `--agent <name>` value before path construction

### Established Patterns
- `AgentConfig` has `#[serde(deny_unknown_fields)]` — adding `allowed_chat_ids` with `#[serde(default)]` is backward compatible (absent field = empty vec)
- Error handling: `miette::Result` at binary boundaries, `thiserror` for library error types
- Tracing: `tracing::info!/warn!/error!` throughout — bot follows same pattern

### Integration Points
- `rightclaw-cli/src/main.rs` `Commands` enum: add `Bot { agent: String }` variant
- `Cargo.toml` workspace: add `crates/bot` to `members`
- `AgentConfig` in `rightclaw` crate: add `allowed_chat_ids: Vec<i64>` field

</code_context>

<specifics>
## Specific Ideas

- User explicitly wants `crates/bot` to be forward-compatible with non-Telegram bots — keep Telegram-specific code in a `telegram/` submodule inside the crate, not at the top level. Future Slack/webhook bots get their own submodules.
- The "silent drop" on unlisted chat_ids (no reply, no log) is intentional per BOT-05 — don't log at INFO/DEBUG either, as it would spam logs if a public bot receives many unwanted messages.

</specifics>

<deferred>
## Deferred Ideas

- Non-Telegram bot support (Slack, webhooks) — future phases, will live in `crates/bot` when needed
- Streaming responses / edit-in-place — v3.1 per REQUIREMENTS.md Future Requirements

### Reviewed Todos (not folded)
- "Document CC gotcha — Telegram messages dropped while agent is streaming" — docs-only todo, not in scope for Phase 23. Remains pending for later.

</deferred>

---

*Phase: 23-bot-skeleton*
*Context gathered: 2026-03-31*
