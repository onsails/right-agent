---
id: SEED-014
status: dormant
planted: 2026-04-02
planted_during: v3.0 Teloxide Bot Runtime (phase 28.2 UAT)
trigger_when: Agent self-management milestone — any work on agent lifecycle control, MCP tooling for agents, or self-modification capabilities
scope: medium
---

# SEED-014: /restart command — agent restarts itself via rightclaw MCP → process-compose

## Why This Matters

Agents can self-modify: add MCP servers, update plugins, change settings. After such edits,
the agent needs to restart to pick up the changes — but it has no way to trigger that itself.
Currently the only option is for the user to `rightclaw restart <agent>` manually or use the
process-compose TUI.

A `/restart` skill backed by a rightclaw MCP tool would let the agent close the loop:
> "I've added the postgres MCP server to your config. Restarting now to apply changes..."

This is the natural complement to agent self-modification capabilities.

## When to Surface

**Trigger:** Agent self-management milestone — surface when working on:
- rightclaw MCP server / agent-facing tools
- Agent lifecycle control (restart, stop, config reload)
- Self-modification flows (agent edits its own IDENTITY.md, agent.yaml, MCP config)

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches any of these conditions:
- rightclaw MCP crate is being created
- Agent self-management or lifecycle tooling is in scope
- Any work on agent-facing skills or tools

## Scope Estimate

**Medium** — A phase or two:
1. rightclaw MCP tool `restart_agent` — reads own process name from env (`RIGHTCLAW_AGENT_NAME`), calls `PcClient::restart_process`
2. `/restart` skill file at `.claude/skills/restart.md` (auto-provisioned by `rightclaw up`)
3. Handle the edge case: agent sends a reply/confirmation before the restart kills its process
4. Tests: mock PC client, verify correct process name passed

## Breadcrumbs

- `crates/rightclaw/src/runtime/pc_client.rs:75` — `restart_process(name: &str)` already exists, just needs to be exposed via MCP
- `crates/rightclaw-cli/src/memory_server.rs` — existing MCP server pattern to follow for a new `rightclaw-mcp` crate
- `crates/rightclaw/src/codegen/process_compose.rs` — process names are derived here; same naming convention needed for `RIGHTCLAW_AGENT_NAME` env var
- `crates/rightclaw/src/codegen/settings.rs` — where per-agent skills are provisioned (add restart skill here)

## Notes

- Process-compose REST API restart endpoint has a known crash bug (see project MEMORY). The safe path is `POST /process/restart/{name}` — test with real PC before shipping.
- Agent needs its own process name at runtime. Simplest: inject `RIGHTCLAW_AGENT_NAME=<name>` env var in the generated process-compose.yaml. Already have agent name at codegen time.
- Consider: should restart be immediate or graceful (finish current CC response first)? Graceful is harder but less jarring. Seed this as a follow-up decision.
