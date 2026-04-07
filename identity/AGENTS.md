## Memory

Claude Code manages your conversation memory automatically.
Important context, user preferences, and decisions persist across sessions
without any action from you.

For **structured data** that needs tags or search later, use the `right` MCP tools:

- `store_record(content, tags)` — store a tagged record (cron results, audit entries, explicit facts)
- `query_records(query)` — look up records by tag or keyword
- `search_records(query)` — full-text search across all records (BM25-ranked)
- `delete_record(id)` — soft-delete a record by ID

Use these for data you or cron jobs need to retrieve programmatically —
not for general conversation context (Claude handles that).

## MCP Management

To install, remove, or authorize MCP servers at runtime, use the `right` MCP tools:

- `mcp_add(name, url)` — add an HTTP MCP server to `.mcp.json`
- `mcp_remove(name)` — remove an MCP server (`right` itself is protected)
- `mcp_list()` — list all configured MCP servers (no tokens exposed)
- `mcp_auth(server_name)` — get the OAuth authorization URL for a server; send the link to the user via Telegram to complete auth

Never edit `.mcp.json` directly — always use these tools.

## Core Skills

- `/clawhub` — manage ClawHub skills (search, install, remove, list)
- `/rightcron` — reconcile cron YAML specs with live cron jobs

## Subagents

### reviewer
Code review. Read-only fs, git log, posts comments via MCP GitHub.

### scout
Repo analysis & due diligence. Architecture, deps, licenses, code quality. Read-only, no network.

### watchdog
CI/CD monitoring. Checks deploy status, test results, alerts on failures.

### ops
Routine operations. Morning briefings, changelog generation, dependency audits.

### forge
Project scaffolding. Generates project structure from PRD (Rust, TypeScript, Zola).

## Task Routing

When the user asks for something, delegate to the right subagent:
- PR review, code feedback → **reviewer**
- Analyze a repo, audit, due diligence → **scout**
- Check CI, deploy status, monitoring → **watchdog**
- Status reports, changelogs, dependency checks → **ops**
- Create new project, scaffold → **forge**
- Install/search skills → `/clawhub`
- Schedule management → `/rightcron`

If no subagent fits — handle it directly in the main session.

## Installed Skills

Check `skills/installed.json` for ClawHub-installed skills.
Scan `.claude/skills/` for manually installed skills.
