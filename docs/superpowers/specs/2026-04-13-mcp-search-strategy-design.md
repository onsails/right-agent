# MCP Search Strategy for OPERATING_INSTRUCTIONS.md

## Problem

When a user asks the agent to add an MCP server, the agent searches for generic MCP URLs
and finds API-key endpoints (URLs with embedded tokens/server IDs). These don't work with
`/mcp auth` OAuth flow. The agent should prioritize OAuth-capable endpoints.

## Solution

Replace the "When the user asks to connect an MCP server" block in
`templates/right/prompt/OPERATING_INSTRUCTIONS.md` with a search strategy
that prioritizes OAuth endpoints over API-key endpoints.

## Current Text (replace)

```markdown
**When the user asks to connect an MCP server:**
1. Help them find the correct MCP URL (search docs if needed)
2. Tell them to run: `/mcp add <name> <url>`
3. If the server requires OAuth, tell them to also run: `/mcp auth <name>`
4. NEVER ask the user for API keys or tokens directly — `/mcp auth` handles authentication
```

## New Text

```markdown
**When the user asks to connect an MCP server:**

1. **Find the OAuth endpoint first.** Search for the service's Claude Code, Codex,
   or Claude Desktop integration docs — these typically describe an OAuth-capable
   MCP endpoint (streamable HTTP or SSE). Search queries like
   `"<service> MCP Claude Code"` or `"<service> MCP OAuth"` work best.

2. **If OAuth endpoint found:**
   - Tell the user to run: `/mcp add <name> <url>`
   - Then: `/mcp auth <name>`

3. **If no OAuth endpoint exists** — look for an API-key endpoint
   (a URL that embeds or requires a key/token). Tell the user to run:
   `/mcp add <name> <url>`
   No `/mcp auth` needed — the key is in the URL itself.

4. **NEVER ask the user for API keys or tokens directly** — either `/mcp auth`
   handles authentication, or the key is part of the URL the user provides.
```

## Files Changed

- `templates/right/prompt/OPERATING_INSTRUCTIONS.md` — replace MCP search instructions

## Testing

Deploy, ask agent to add Composio MCP. It should search for "Composio MCP Claude Code"
first, find the OAuth endpoint, and suggest `/mcp add composio <oauth-url>` + `/mcp auth composio`.
