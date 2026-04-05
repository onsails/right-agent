## Core Skills

<!-- Add your skills here. Example: -->
<!-- - `/my-skill` -- description of what it does -->

## Subagents

<!-- Define your subagents here. Each subagent is a specialized worker with its own permissions. -->
<!-- Example: -->
<!-- ### reviewer -->
<!-- Code review. Read-only fs, git log, posts comments via MCP GitHub. -->

## Task Routing

<!-- Define how tasks get routed to subagents. -->
<!-- If no subagent fits -- handle it directly in the main session. -->

## Installed Skills

Check `skills/installed.json` for ClawHub-installed skills.
Scan `.claude/skills/` for manually installed skills.

## MCP Management

To install, remove, or authorize MCP servers at runtime, use the `rightmemory` MCP tools:

- `mcp_add(name, url)` — add an HTTP MCP server to `.claude.json`
- `mcp_remove(name)` — remove an MCP server (rightmemory itself is protected)
- `mcp_list()` — list all configured MCP servers (no tokens exposed)
- `mcp_auth(server_name)` — get the OAuth authorization URL for a server; send the link to the user via Telegram to complete auth

Never edit `.claude.json` directly — always use these tools.

## Communication

You are running as a daemon with no terminal access.
ALWAYS use the remote channel (reply MCP tool) to communicate with the user.
Never output to console — the user cannot see it.

## Cron Management (RightCron)

**On startup:** Run `/rightcron` immediately. It will bootstrap the reconciler
and recover any persisted jobs. Do this before responding to the user.

**For user requests:** When the user wants to manage cron jobs, scheduled tasks,
or recurring tasks, ALWAYS use the /rightcron skill. NEVER call CronCreate
directly — always write a YAML spec first, then reconcile.
