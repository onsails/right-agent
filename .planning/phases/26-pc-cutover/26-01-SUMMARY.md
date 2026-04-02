---
phase: 26-pc-cutover
plan: 01
subsystem: codegen
tags: [process-compose, telegram, codegen, refactor]
dependency_graph:
  requires: []
  provides: [generate_process_compose(agents, exe_path), BotProcessAgent, bot-only PC template]
  affects: [rightclaw-cli cmd_up, templates/process-compose.yaml.j2]
tech_stack:
  added: []
  patterns: [bot-only PC generation, filter-by-telegram-token, no-cc-channels]
key_files:
  created: []
  modified:
    - crates/rightclaw/src/codegen/process_compose.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - crates/rightclaw/src/codegen/mod.rs
    - crates/rightclaw/src/codegen/telegram.rs
    - templates/process-compose.yaml.j2
    - crates/rightclaw-cli/src/main.rs
decisions:
  - "BotProcessAgent replaces ProcessAgent: token_inline/token_file mutually exclusive; token_file resolved to abs path at codegen time"
  - "generate_process_compose filters out non-telegram agents entirely (returns empty processes section if none)"
  - "resolve_telegram_token promoted to pub(crate) for Plan 02 doctor use"
  - "Stale CLI integration test for generate_telegram_channel_config removed since function is no longer re-exported"
metrics:
  duration: ~15min
  completed: "2026-04-01"
  tasks_completed: 2
  files_modified: 6
requirements: [PC-01, PC-02, PC-03]
---

# Phase 26 Plan 01: PC Cutover — Bot-Only Process-Compose Codegen

**One-liner:** Replaced CC interactive session codegen with bot-only process-compose entries: `<name>-bot:` per Telegram-enabled agent, `rightclaw bot --agent <name>` command, RC_AGENT_DIR/RC_AGENT_NAME/RC_TELEGRAM_TOKEN env, no `is_interactive`.

## What Was Built

Refactored `process_compose.rs` to generate bot-only entries. `BotProcessAgent` struct holds token_inline/token_file (mutually exclusive), exe_path, working_dir. Filter: only agents with `telegram_token` or `telegram_token_file` in config. Agents without a token are excluded entirely from the output.

Template rewritten to produce `<name>-bot:` process keys with `rightclaw bot --agent <name>` command and environment block. `is_interactive` removed. `cmd_up` cleaned of all CC channels infrastructure: the `any_telegram` guard + `ensure_bun_installed` + `ensure_telegram_plugin_installed` + per-agent `generate_telegram_channel_config` calls all removed. Early-exit added when no agents have Telegram tokens.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | BotProcessAgent + template + mod.rs exports | 9a24eaa | process_compose.rs, process_compose_tests.rs, mod.rs, telegram.rs, templates/process-compose.yaml.j2 |
| 2 | Update cmd_up callsite; remove channels block | 1f3c69e | main.rs |

## Verification Results

- `cargo test --workspace`: 307 passed, 1 pre-existing failure (`test_status_no_running_instance`)
- `rg "is_interactive" templates/`: no matches
- `rg "ensure_bun_installed|ensure_telegram_plugin_installed|generate_telegram_channel_config" crates/rightclaw-cli/`: no matches
- `rg "generate_process_compose.*run_dir" crates/`: no matches
- All 15 `codegen::process_compose` tests pass

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Removed stale CLI integration test referencing de-exported function**
- **Found during:** Task 2 build (cargo test --workspace)
- **Issue:** `telegram_config_not_created_when_no_telegram_fields` test in `rightclaw-cli/src/main.rs` called `rightclaw::codegen::generate_telegram_channel_config` which is no longer re-exported from `codegen/mod.rs`
- **Fix:** Removed the test — its functionality is already covered by tests in `telegram.rs`
- **Files modified:** crates/rightclaw-cli/src/main.rs
- **Commit:** 1f3c69e (part of Task 2 commit)

## Known Stubs

None — all data paths are fully wired. The bot process entries resolve real agent paths and tokens from AgentDef at codegen time.

## Self-Check: PASSED

Files exist:
- crates/rightclaw/src/codegen/process_compose.rs: FOUND
- crates/rightclaw/src/codegen/process_compose_tests.rs: FOUND
- templates/process-compose.yaml.j2: FOUND

Commits exist:
- 9a24eaa: FOUND
- 1f3c69e: FOUND
