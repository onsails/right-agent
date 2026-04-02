# Phase 26: PC Cutover - Context

**Gathered:** 2026-04-01
**Status:** Ready for planning

<domain>
## Phase Boundary

Wire teloxide bot processes into `rightclaw up` lifecycle; remove all CC channels infrastructure
atomically. Specifically:

1. `rightclaw up` generates `<agent>-bot` process-compose entries for agents with a Telegram token.
2. CC interactive session entries are removed from process-compose entirely — bot replaces them.
3. Channels-related calls (`ensure_bun_installed`, `ensure_telegram_plugin_installed`,
   `generate_telegram_channel_config`) are removed from `cmd_up`.
4. Bot calls `deleteWebhook` on startup (fatal error if it fails).
5. `rightclaw doctor` warns when a configured agent token has an active Telegram webhook.

**Out of scope:**
- Cron runtime (Phase 27)
- Cronsync SKILL rewrite (Phase 28)
- `telegram-core` crate refactor (deferred idea)
- Any new agent capabilities

</domain>

<decisions>
## Implementation Decisions

### Process-compose structure

- **D-01:** After Phase 26, `process-compose.yaml` contains **only bot entries** — one
  `<agent>-bot` process per agent with `telegram_token` / `telegram_token_file` set.
  Agents without a Telegram token get no process-compose entry at all.

- **D-02:** CC interactive session entries are **removed entirely**. The old CC persistent
  session is not replaced — `claude -p` is invoked per-message by the bot process. The
  `ProcessAgent` struct, `process_compose.rs`, and the Jinja2 template are refactored to
  generate bot entries only.

- **D-03:** Bot process entry shape (PC-01):
  ```yaml
  <agent>-bot:
    command: "rightclaw bot --agent <name>"
    working_dir: "<agent_path>"
    environment:
      - RC_AGENT_DIR=<agent_path>
      - RC_AGENT_NAME=<agent_name>
      - RC_TELEGRAM_TOKEN=<token>       # or RC_TELEGRAM_TOKEN_FILE=<path>
    availability:
      restart: "on_failure"
      backoff_seconds: 5
      max_restarts: 3
    shutdown:
      signal: 15
      timeout_seconds: 30
  ```
  No `is_interactive` field (PC-02). `RC_TELEGRAM_TOKEN` used when token is inline;
  `RC_TELEGRAM_TOKEN_FILE` used when `telegram_token_file` is set in agent.yaml
  (file path resolved to absolute, relative to agent dir).

- **D-04:** `is_interactive` removed from the Jinja2 template entirely (PC-02). No entries use TTY.

- **D-05:** `generate_process_compose` function signature changes: takes `agents: &[AgentDef]`
  and `run_dir: &Path` as before, but also needs to pass the current executable path for the
  `rightclaw bot` command — OR the function resolves `current_exe()` internally. **Claude's
  Discretion**: implementation detail.

### Channels cleanup

- **D-06:** Remove from `cmd_up`:
  - `ensure_bun_installed()` call
  - `ensure_telegram_plugin_installed()` call
  - `generate_telegram_channel_config(agent)` call
  - The `any_telegram` detection block that gates them.

  Old `.claude/channels/telegram/` directories on disk are left untouched (non-production env,
  CC no longer reads them without the `--channels` flag).

### deleteWebhook

- **D-07:** Bot calls `deleteWebhook` in `run_async()` (in `crates/bot/src/lib.rs`) before
  starting the teloxide dispatcher. Uses teloxide's built-in `bot.delete_webhook()`.

- **D-08:** `deleteWebhook` failure is a **fatal error** — propagate `Err`, let process-compose
  restart the bot. Rationale: if deleteWebhook fails, long-polling will compete with an active
  webhook and messages will be silently dropped.

### Doctor webhook check

- **D-09:** `rightclaw doctor` checks each agent's configured Telegram token via Telegram API's
  `getWebhookInfo` endpoint. Implementation uses **`reqwest`** (already in `rightclaw` crate
  dependencies) with a blocking call via `tokio::runtime::Runtime::new()`.

- **D-10:** Check is **non-fatal / warn** — if the HTTP call fails (timeout, network error),
  the check is skipped with a `DoctorStatus::Pass` or `Warn` stating "webhook check skipped".
  Only warns (not fails) when a webhook IS found with a non-empty URL.

- **D-11:** Token resolution for doctor check: reads `telegram_token` / `telegram_token_file`
  from `agent.yaml` directly (same logic as `codegen::telegram::resolve_telegram_token`).
  If no token configured — skip check for that agent.

### Claude's Discretion

- Whether `generate_process_compose` resolves `current_exe()` internally or receives it as a parameter.
- Whether token is injected as `RC_TELEGRAM_TOKEN` (inline) or `RC_TELEGRAM_TOKEN_FILE` (file path)
  in the env block — follow the precedence already established in `telegram::resolve_token`.
- Test structure for the reworked `process_compose_tests.rs`.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` §PC-01..PC-05 — exact success criteria for this phase

### Prior phase decisions
- `.planning/phases/25.5-agent-definition-codegen/25.5-CONTEXT.md` — D-01..D-10 (agent def codegen,
  reply schema, worker invocation pattern) — Phase 26 must not break AGDEF-01..05
- `.planning/phases/25-telegram-handler-cc-dispatch/25-CONTEXT.md` — bot architecture decisions,
  `resolve_token` priority chain (D-13)

### Existing code to modify
- `crates/rightclaw/src/codegen/process_compose.rs` — primary change target; `ProcessAgent` struct,
  template rendering, `generate_process_compose` signature
- `templates/process-compose.yaml.j2` — refactor to bot-only entries, remove `is_interactive`
- `crates/rightclaw-cli/src/main.rs` (`cmd_up`) — remove channels block (3 calls + any_telegram guard)
- `crates/bot/src/lib.rs` (`run_async`) — add `deleteWebhook` before dispatcher start
- `crates/rightclaw/src/doctor.rs` — add webhook check per agent with `reqwest`
- `crates/rightclaw/src/codegen/telegram.rs` — may be deleted or kept (review whether
  `generate_telegram_channel_config` is called from anywhere else after cleanup)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `telegram::resolve_token()` in `crates/bot/src/telegram/mod.rs` — priority chain for token
  resolution; doctor check should use equivalent logic from `codegen/telegram.rs`
  (`resolve_telegram_token`)
- `teloxide` in `bot` crate: `bot.delete_webhook()` is the built-in method for PC-04
- `reqwest` + `tokio` already in `rightclaw` crate — no new deps for doctor webhook check
- `DoctorCheck` struct in `doctor.rs` — existing pattern for adding new checks (name, status, detail, fix)

### Established Patterns
- `generate_process_compose(agents, run_dir)` — existing signature; `ProcessAgent` struct becomes
  `BotProcessAgent` or similar; template switches from agent-centric to bot-centric
- `cmd_up` channels block is self-contained (`any_telegram` + 3 calls) — easy to remove atomically
- `run_doctor()` iterates checks, never short-circuits; new webhook check follows same pattern

### Integration Points
- `codegen/mod.rs` — remove `pub use telegram::generate_telegram_channel_config` export if deleted
- `process_compose_tests.rs` — all existing tests become invalid (no more `.sh` paths, no more
  `is_interactive`); full rewrite expected
- `bot/src/lib.rs::run_async` — `delete_webhook` call before `telegram::run_telegram()`

</code_context>

<specifics>
## Specific Ideas

- Bot entry name is `<agent>-bot` (hyphenated), not `<agent>_bot` — consistent with process-compose
  naming conventions and the requirement spec (PC-01).
- `rightclaw bot` command uses `current_exe()` to ensure the binary is found even when not on PATH
  (same pattern as `.mcp.json` entry added in Phase 17).
- For the doctor webhook check, `getWebhookInfo` returns `{"ok": true, "result": {"url": ""}}` when
  no webhook is set — warn only when `result.url` is non-empty.

</specifics>

<deferred>
## Deferred Ideas

### telegram-core crate
Current crate structure causes friction: `teloxide` is in `bot` crate, but `rightclaw` (core)
needs Telegram utilities for doctor checks. A `telegram-core` crate exposing token resolution,
webhook utils, and lightweight Telegram API types would decouple this. Deferred — not needed
for Phase 26 since `reqwest` in `doctor.rs` is sufficient.

### PROMPT-03 resolution
`codegen/shell_wrapper.rs` is listed as pending in REQUIREMENTS.md but the file does not exist
in the codebase. Phase 26 removes the last user of `wrapper_path` (the old PC template), which
effectively completes PROMPT-03. Planner should verify and mark it done.

</deferred>

---

*Phase: 26-pc-cutover*
*Context gathered: 2026-04-01*
