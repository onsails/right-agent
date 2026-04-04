---
id: SEED-024
status: dormant
planted: 2026-04-04
planted_during: v3.2 MCP OAuth (complete)
trigger_when: agent self-management or skills infrastructure work
scope: Small
---

# SEED-024: MCP server management via rightclaw skill (add/remove/list/auth)

## Why This Matters

MCP management currently lives only in the Telegram bot (`/mcp add`, `/mcp remove`, `/mcp list`, `/mcp auth`) and `rightclaw mcp` CLI. Neither is accessible to agents themselves or to users working inside a Claude Code session.

A rightclaw skill would give a unified interface:
- **Agents** can self-manage their MCP servers (add tools they need, remove unused ones)
- **Users** can manage MCPs from inside any Claude Code session via skill invocation
- Single source of truth — skill calls the same underlying functions as bot/CLI

## When to Surface

**Trigger:** When working on agent self-management capabilities or skills infrastructure

This seed should be presented during `/gsd-new-milestone` when the milestone scope matches any of these conditions:
- Agent self-configuration or autonomy features
- Skills system / skill registry work
- MCP server management improvements

## Scope Estimate

**Small** — the MCP add/remove/list/auth logic already exists in `crates/bot/src/telegram/handler.rs`. The skill would be a thin wrapper exposing the same operations via SKILL.md format.

## Breadcrumbs

Related code and decisions found in the current codebase:

- `crates/bot/src/telegram/handler.rs` — `handle_mcp_list`, `handle_mcp_add`, `handle_mcp_remove` (lines 231-563)
- `crates/bot/src/telegram/dispatch.rs` — `/mcp` command registration
- `crates/rightclaw/src/mcp/` — core MCP module (detect, credentials, oauth, refresh)
- `crates/rightclaw/src/codegen/mcp_config.rs` — `.mcp.json` generation
- `crates/rightclaw-cli/src/main.rs` — `rightclaw mcp` CLI subcommand
- SEED-014: Agent self-restart via MCP (related self-management concept)

## Notes

The bot handler functions are async and take `(bot, msg, agent_dir)` — extracting the core logic into shared functions that both bot and skill can call would be the clean approach. The skill would need access to the agent's working directory to locate `.mcp.json`.
