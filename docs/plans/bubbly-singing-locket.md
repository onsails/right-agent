# Prioritize OAuth endpoints in MCP search instructions

## Context

When a user asks the agent to add an MCP server (e.g. "add Composio MCP"), the agent
searches the web and picks the first URL it finds. For Composio, this is the API-key
endpoint (`backend.composio.dev/v3/mcp/{SERVER_ID}?user_id=...`) rather than the
OAuth-capable endpoint (`mcp.composio.dev`).

The agent correctly follows its instructions — but those instructions say "find the MCP
endpoint" generically, without prioritizing OAuth. Since `handle_mcp_add` short-circuits
to `query_string` auth when the URL has `?params`, OAuth discovery is never attempted.

A spec already exists: `docs/superpowers/specs/2026-04-13-mcp-search-strategy-design.md`.
This plan applies it.

## File

**Modify:** `templates/right/prompt/OPERATING_INSTRUCTIONS.md` — lines 46-58

## Change

Replace the current "When the user asks to connect an MCP server" block:

```markdown
**When the user asks to connect an MCP server:**

1. **Find the MCP endpoint.** Search for the service's Claude Code, Codex,
   or Claude Desktop integration docs — these typically describe an MCP endpoint
   (streamable HTTP or SSE). Search queries like
   `"<service> MCP Claude Code"` or `"<service> MCP server"` work best.

2. **Tell the user to run:** `/mcp add <name> <url>`
   The system auto-detects the authentication method (OAuth, Bearer token,
   custom header, or API key in URL) and handles the setup flow.

3. **NEVER ask the user for API keys or tokens directly** — the `/mcp add` flow
   handles credential collection when needed.
```

With:

```markdown
**When the user asks to connect an MCP server:**

1. **Find the OAuth endpoint first.** Search for the service's Claude Code, Codex,
   or Claude Desktop integration docs — these typically describe an OAuth-capable
   MCP endpoint (streamable HTTP or SSE). Search queries like
   `"<service> MCP Claude Code"` or `"<service> MCP OAuth"` work best.

2. **If OAuth endpoint found** — tell the user to run:
   `/mcp add <name> <url>` then `/mcp auth <name>`

3. **If no OAuth endpoint exists** — look for an API-key endpoint
   (a URL that embeds or requires a key/token). Tell the user to run:
   `/mcp add <name> <url>`
   The system will prompt for credentials if needed.

4. **NEVER ask the user for API keys or tokens directly** — either `/mcp auth`
   handles authentication, or the key is part of the URL the user provides.
```

## What stays the same

- Compiled-in via `include_str!` in `agent_def.rs:23` — no code changes needed
- Existing test (`operating_instructions_constant_is_non_empty`) checks for `## MCP Management` header — unaffected
- `PROMPT_SYSTEM.md` references the file path, doesn't duplicate content — no update needed
- `handle_mcp_add` logic unchanged — this is purely a prompt/instruction fix

## Verification

1. `devenv shell -- cargo test -p rightclaw --lib` — existing tests pass
2. Deploy and ask agent "add Composio MCP" — it should search for OAuth endpoint first, find `mcp.composio.dev`, and suggest `/mcp add composio <oauth-url>` + `/mcp auth composio`
