---
phase: 19-home-isolation-hardening
plan: "02"
subsystem: uat
tags: [uat, telegram, fresh-init, e2e]
dependency_graph:
  requires: ["19-01"]
  provides:
    - "Human-verified fresh-init flow"
    - "Telegram channel config validated end-to-end"
  affects: []
tech_stack:
  added: []
  patterns: []
key_files:
  created:
    - .planning/phases/19-home-isolation-hardening/19-HUMAN-UAT.md
  modified: []
decisions:
  - "UAT uncovered 3 additional bugs beyond Plan 01 scope — all fixed inline before sign-off"
  - "Plugin symlink pattern mirrors credential symlink: agent/.claude/plugins → ~/.claude/plugins"
  - "init.rs writes telegram token to agent-level .claude/channels/telegram/.env (not host-level)"
  - "telegram_token_file in agent.yaml references dotenv file; resolve_telegram_token strips TELEGRAM_BOT_TOKEN= prefix"
metrics:
  duration_minutes: 45
  completed_date: "2026-03-27"
  tasks_completed: 2
  files_modified: 5
---

# Phase 19 Plan 02: Fresh-Init Human UAT Summary

UAT document created with 7 test cases covering fresh-init through doctor. Human verification revealed 3 additional bugs — all fixed before sign-off.

## What Was Built

### Task 1: UAT document
Created `.planning/phases/19-home-isolation-hardening/19-HUMAN-UAT.md` with 7 tests:
1. Fresh Init — directory structure
2. File Generation — memory.db, settings.json, .claude.json, .mcp.json
3. .mcp.json Content — RC_AGENT_NAME present, no legacy telegram marker
4. No Telegram Agent — no --channels or enabledPlugins
5. With Telegram Agent — channels injected, .env with bot token
6. Memory Round-trip — stored_by shows agent name
7. Doctor — all checks pass

### Task 2: Human verification (checkpoint)
Human executed UAT. Three bugs discovered and fixed during verification:

**Bug 1 — Plugin symlink missing (462ef80):**
CC isolates its plugin registry per-HOME. Agent's `.claude/plugins/installed_plugins.json` was empty → "plugin not installed" for telegram. Fix: `create_plugins_symlink` symlinks agent's plugins dir to host's `~/.claude/plugins/`.

**Bug 2 — Init writes telegram to wrong dir (0317667):**
`init.rs` wrote `.env` to host-level `~/.claude/channels/telegram/` but agents use isolated HOME. Token was invisible to the agent. Fix: write to `agent_dir/.claude/channels/telegram/` and add `telegram_token_file: .claude/channels/telegram/.env` to agent.yaml.

**Bug 3 — Dotenv prefix doubled (b13850c):**
`resolve_telegram_token` read the entire `.env` (including `TELEGRAM_BOT_TOKEN=` prefix) as the token value. `generate_telegram_channel_config` then wrote `TELEGRAM_BOT_TOKEN=TELEGRAM_BOT_TOKEN=xxx`. Bot authenticated with invalid token, silently failed. Fix: strip `TELEGRAM_BOT_TOKEN=` prefix in `resolve_telegram_token`.

After all fixes: human confirmed all 7 UAT tests pass.

## Commits

| Hash | Message |
|------|---------|
| 05fbb97 | docs(19-02): write fresh-init human UAT with 7 test cases |
| 506f974 | docs(19-02): fix UAT test commands |
| 462ef80 | fix(up): symlink agent .claude/plugins to host plugins |
| 0317667 | fix(init): write telegram config to agent dir, record token_file in agent.yaml |
| b13850c | fix(telegram): strip TELEGRAM_BOT_TOKEN= prefix when reading dotenv-format token file |

## Self-Check: PASSED
