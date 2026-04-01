---
gsd_state_version: 1.0
milestone: v3.0
milestone_name: Teloxide Bot Runtime
status: verifying
stopped_at: Completed 28-cronsync-skill-rewrite-01-PLAN.md
last_updated: "2026-04-01T20:32:05.580Z"
last_activity: 2026-04-01
progress:
  total_phases: 8
  completed_phases: 8
  total_plans: 17
  completed_plans: 17
  percent: 14
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-03-26)

**Core value:** Run multiple autonomous Claude Code agents safely -- each sandboxed by native OS-level isolation, orchestrated by a single CLI command.
**Current focus:** Phase 28 — cronsync-skill-rewrite

## Current Position

Phase: 28
Plan: Not started
Status: Phase complete — ready for verification
Last activity: 2026-04-01

Progress: [█░░░░░░░░░] 14%

## Performance Metrics

*Carried from v2.2 for reference:*

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| Phase 11 | 2 | ~18min | ~9min |
| Phase 12 | 1 | — | — |
| Phase 13 | 1 | — | — |
| Phase 14 | 1 | — | — |
| Phase 15 | 1 | — | — |
| Phase 16-db-foundation P02 | 5 | 2 tasks | 11 files |
| Phase 16-db-foundation P01 | 3 | 2 tasks | 9 files |
| Phase 16 P03 | 90 | 2 tasks | 2 files |
| Phase 17 P01 | 4 | 2 tasks | 5 files |
| Phase 17 P02 | 455 | 2 tasks | 8 files |
| Phase 18-cli-inspection P01 | 4 | 2 tasks | 3 files |
| Phase 18-cli-inspection P02 | 3 | 2 tasks | 1 files |
| Phase 19-home-isolation-hardening P01 | 7 | 2 tasks | 15 files |
| Phase 23-bot-skeleton P01 | 12 | 1 tasks | 8 files |
| Phase 23-bot-skeleton P02 | 4 | 2 tasks | 9 files |
| Phase 24-system-prompt-codegen P03 | 7 | 2 tasks | 3 files |
| Phase 24-system-prompt-codegen P02 | 155 | 2 tasks | 2 files |
| Phase 25-telegram-handler-cc-dispatch P01 | 25 | 2 tasks | 4 files |
| Phase 25-telegram-handler-cc-dispatch P02 | 7 | 2 tasks | 4 files |
| Phase 25-telegram-handler-cc-dispatch P03 | 228 | 2 tasks | 5 files |
| Phase 25.5-agent-definition-codegen P01 | 212 | 2 tasks | 6 files |
| Phase 25.5-agent-definition-codegen P02 | 3 | 1 tasks | 2 files |
| Phase 26-pc-cutover P02 | 460 | 2 tasks | 2 files |
| Phase 26-pc-cutover P01 | 15 | 2 tasks | 6 files |
| Phase 27-cron-runtime P01 | 222 | 2 tasks | 7 files |
| Phase 27-cron-runtime P02 | 3 | 1 tasks | 1 files |
| Phase 28-cronsync-skill-rewrite P01 | 2 | 2 tasks | 1 files |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions relevant to v2.3:

- [v2.3 research]: Use rusqlite 0.39 + rusqlite_migration 2.5 (sync-only; tokio-rusqlite rejected)
- [v2.3 research]: FTS5 virtual table in V1 schema even if skill uses LIKE in v2.3 — avoids costly retrofit
- [v2.3 research]: memory.db lives in agent root (not .claude/), never referenced by MEMORY.md
- [v2.3 research]: Injection scanning deferred to Phase 17 with dedicated research before implementation
- [Phase 16-02]: SEC-02 enforced by removing memory_path from AgentDef struct entirely — no MEMORY.md connection at type level
- [Phase 16-02]: Task 2 system_prompt default was pre-completed by plan 16-01 (commit e11f9ff)
- [Phase 16-db-foundation]: rusqlite 0.39 bundled + rusqlite_migration 2.5 for per-agent SQLite memory; WAL mode + FTS5 + ABORT triggers in V1 schema
- [Phase 16]: sqlite3 check uses inline Warn override pattern — matches RESEARCH.md Pattern 5
- [Phase 17]: Use str::contains() on lowercased input over 15-pattern list — no regex crate, matches SEC-01 research
- [Phase 17]: open_connection() returns live Connection for store ops; open_db() retained for cmd_up callers
- [Phase 17]: Injection guard is first line of store_memory() — structural guarantee cannot be bypassed
- [Phase 17]: Use ServerInfo::new().with_instructions() — InitializeResult is #[non_exhaustive] in rmcp 1.3
- [Phase 17]: run_memory_server() returns miette::Result — no anyhow in CLI crate, miette is project standard
- [Phase 17]: cargo update required before build — rmcp-macros 1.3.0 not in stale local crates.io index
- [Phase 18-cli-inspection]: list_memories uses ORDER BY created_at DESC, id DESC for deterministic pagination when timestamps tie
- [Phase 18-cli-inspection]: hard_delete_memory checks existence without deleted_at filter — operators can hard-delete soft-deleted rows
- [Phase 18-cli-inspection]: search_memories unchanged (LIMIT 50); search_memories_paged is separate function for CLI pagination
- [Phase 18-cli-inspection]: cmd_memory_delete fetches entry preview via direct SQL including soft-deleted rows — operators see what they are hard-deleting
- [Phase 18-cli-inspection]: resolve_agent_db centralizes agent-dir and memory.db validation for all cmd_memory_* functions
- [Phase 19-home-isolation-hardening]: Telegram detection reads agent.config.telegram_token/telegram_token_file; mcp_config_path removed as unreliable proxy
- [Phase 19-home-isolation-hardening]: generate_mcp_config gains agent_name param; RC_AGENT_NAME injected into rightmemory env section for memory provenance
- [Phase 23-bot-skeleton]: allowed_chat_ids: Vec<i64> uses serde(default) — empty vec is secure default (blocks all messages), not Option
- [Phase 23-bot-skeleton]: teloxide features=[macros, throttle, cache-me] — default-features=false to avoid ctrlc_handler (Phase 23 owns signal handling)
- [Phase 23-bot-skeleton]: AgentConfig no Default impl — parse_agent_config None uses explicit struct literal fallback
- [Phase 23-03]: bot::run() converted to pub async fn — avoids nested tokio runtime collision with #[tokio::main] CLI main; callers .await it directly
- [Phase 24-system-prompt-codegen]: D-13: USER.md is a minimal placeholder — agent fills it through interaction
- [Phase 24-system-prompt-codegen]: D-06: Communication and Cron Management sections moved from hardcoded codegen to AGENTS.md template
- [Phase 24-system-prompt-codegen]: D-10/D-11: cmd_up writes agent_dir/.claude/system-prompt.txt via generate_system_prompt; no run/<agent>-prompt.md or shell wrapper written
- [Phase 24-system-prompt-codegen]: cmd_pair writes system-prompt.txt itself before exec for standalone correctness
- [Phase 25-01]: ThreadId in teloxide 0.17 wraps MessageId(i32) — match pattern must destructure both layers: Some(ThreadId(MessageId(n)))
- [Phase 25-01]: tokio-util 0.7 rt feature (not sync) enables CancellationToken via tokio/sync transitively
- [Phase 25-telegram-handler-cc-dispatch]: Use 'y' not 'x' in stderr truncation test — 'exit' contains 'x' causing collision
- [Phase 25-telegram-handler-cc-dispatch]: parse_reply_tool uses serde_json::Value directly, no typed CcOutput struct needed
- [Phase 25-telegram-handler-cc-dispatch]: teloxide 0.13 reply uses ReplyParameters not reply_to_message_id method
- [Phase 25-03]: DashMap guard released before .await in handle_message — sender cloned out before send
- [Phase 25-03]: kill_on_drop(true) in invoke_cc sufficient for BOT-04 — no children registry needed
- [Phase 25.5-agent-definition-codegen]: agent_def.rs model field uses 'inherit' when AgentConfig.model is None — matches CC native agent convention
- [Phase 25.5-agent-definition-codegen]: No tools: field in frontmatter per D-05 — CC infers tools from sandbox settings
- [Phase 25.5-agent-definition-codegen]: [Phase 25.5-02]: --agent first call only (AGDEF-02/03); --json-schema both calls (D-01); ReplyOutput gains Debug+Deserialize; agent_name from agent_dir.file_name() at WorkerContext construction
- [Phase 26-pc-cutover]: Use teloxide::requests::Requester as _ in scoped block for delete_webhook; inline token resolver in doctor.rs with TODO for Plan 01 pub(crate) migration
- [Phase 26-pc-cutover]: BotProcessAgent replaces ProcessAgent: token_inline/token_file mutually exclusive; token_file resolved to abs path at codegen time
- [Phase 26-pc-cutover]: generate_process_compose filters out non-telegram agents entirely; early-exit in cmd_up when no bot agents
- [Phase 27-cron-runtime]: D-01: --agent <name> invocation style for cron jobs (not --system-prompt-file) — matches AGDEF-02
- [Phase 27-cron-runtime]: D-04: cron_runs V3 migration in memory.db + log files at crons/logs/; rusqlite::Connection opened per-job (not passed from lib.rs)
- [Phase 27-cron-runtime]: Implementation::new('rightclaw', CARGO_PKG_VERSION) for MCP server rename — cleaner than struct update syntax
- [Phase 27-cron-runtime]: server_info is non-Optional in rmcp 1.3 InitializeResult — direct field access in tests
- [Phase 28-cronsync-skill-rewrite]: cronsync SKILL.md: reactive-only activation (no bootstrap), MCP observability section added, UTC timezone correction, all CC tool refs removed

### Roadmap Evolution

- Phase 19 added: HOME Isolation Hardening — plugin sharing, shell snapshot cleanup, fresh-init UAT
- Phase 28.1 inserted after Phase 28: v3.0 UAT — manual end-to-end testing of Teloxide bot, cron runtime, and rightclaw up flow (URGENT)
- Phase 28.2 inserted after Phase 28.1: v3.0 UAT Fix — teloxide missing native-tls (bot restart loop) and doctor nested tokio runtime panic (URGENT)

### Pending Todos

None yet.

### Blockers/Concerns

- Phase 17 (injection scanning): Practical Rust implementation patterns sparse — needs research pass before coding SEC-01
- OAuth broken under HOME override on Linux -- ANTHROPIC_API_KEY required for headless (carry-over from v2.2)

### Quick Tasks Completed

| # | Description | Date | Commit | Directory |
|---|-------------|------|--------|-----------|
| 260326-us1 | Replace is_tty with is_interactive in process-compose template | 2026-03-26 | 427f5e1 | [260326-us1-replace-is-tty-with-is-interactive-in-pr](./quick/260326-us1-replace-is-tty-with-is-interactive-in-pr/) |
| 260327-04d | Fix rightmemory MCP binary path — use absolute path from current_exe() | 2026-03-27 | fb5972e | [260327-04d-fix-rightmemory-mcp-binary-path-use-abso](./quick/260327-04d-fix-rightmemory-mcp-binary-path-use-abso/) |

## Session Continuity

Last session: 2026-04-01T20:27:43.320Z
Stopped at: Completed 28-cronsync-skill-rewrite-01-PLAN.md
Resume file: None
