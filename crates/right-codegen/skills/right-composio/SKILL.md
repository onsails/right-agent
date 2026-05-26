---
name: right-composio
description: >-
  Use BEFORE calling any mcp__right__composio__* tool — especially
  COMPOSIO_SEARCH_TOOLS, COMPOSIO_MULTI_EXECUTE_TOOL, or
  COMPOSIO_REMOTE_WORKBENCH — and whenever a request involves Notion,
  Gmail, Google Calendar, Google Drive, Slack, GitHub, Linear, or any
  other service routed through Composio. Sessions that skip this
  skill burn 300-500K context tokens on repeated tool searches and
  inline payloads ($200+ per session observed in cache-write cost).
  Read once per session, on resume, and any turn that mentions a
  Composio-fronted service.
---

# /right-composio — Composio MCP playbook

**Read this BEFORE your first `mcp__right__composio__*` call this
session, and again on resume.** The defaults (no workbench, fresh
search every turn, one tool per call) will bury you in context tokens.

Composio is a gateway: one MCP server fronts 250+ external services
(Notion, Gmail, Calendar, Slack, GitHub, ...). Tool surface is narrow
(~7 meta-tools) but responses can be huge. Two biggest context risks:
dumping list/search/fetch payloads into context, and looping single
tool calls when one MULTI_EXECUTE would do.

## When to Activate

- The user's request maps to a Composio-fronted service.
- You're about to invoke `mcp__right__composio__*` and need to decide:
  workbench yes/no, MULTI_EXECUTE vs single, search_tools first?
- If composio is not in `mcp__right__mcp_list`, this skill does not
  apply — ask the user to open `/mcp` and add Composio in the dashboard.

## Workbench discipline

`mcp__right__composio__COMPOSIO_MULTI_EXECUTE_TOOL` has a
`sync_response_to_workbench` field. `true` → response stored in
Composio's remote workbench, you get a reference. `false` (default)
→ full payload lands in your context.

**`sync_response_to_workbench: true` when:**
- Tool slug contains `_LIST_`, `_SEARCH_`, `_FETCH_`, `_GET_ALL`,
  `_PAGES`, `_THREADS` (collections).
- Batching 2+ tools in one MULTI_EXECUTE call.
- Expecting prose bodies (email content, Notion page text).
- Follow-up MULTI_EXECUTE will act on the result — pass the
  workbench reference via `session_id`.

**`sync_response_to_workbench: false` (or omit) when:**
- Single write/update returning only an id or status
  (`NOTION_INSERT_ROW_DATABASE`, `GMAIL_SEND_EMAIL`,
  `CALENDAR_CREATE_EVENT`).
- Single read of one known record where the body IS the user's
  answer (`NOTION_FETCH_PAGE` by id when the user asked "what's on
  that page").
- Next step in this turn branches on the result AND the result
  is small.

When in doubt: workbench on. Pull with
`mcp__right__composio__COMPOSIO_REMOTE_WORKBENCH` later.

## Tool-selection patterns

- **Unknown toolkit slug?** Call
  `mcp__right__composio__COMPOSIO_SEARCH_TOOLS` ONCE per slug per
  session. Each search drops 20-50KB of schemas into your context —
  30 redundant searches = 580KB of bloat. After you learn a slug
  (e.g. `NOTION_UPDATE_PAGE`, `GMAIL_SEND_EMAIL`), reuse it from your
  turn-by-turn context; do NOT re-search "to be sure". Slugs are
  stable within a session. If you genuinely forgot one, ask the user
  rather than re-searching the same toolkit.
- **Multiple ops on same toolkit?** One MULTI_EXECUTE with a `tools`
  array beats N separate calls.
- **Non-trivial query/transform on a result?**
  `mcp__right__composio__COMPOSIO_REMOTE_BASH_TOOL` on workbench
  data beats pulling-and-parsing in context.

## Pitfalls

- **`input` vs `arguments`:** per-tool args go under `arguments`, not
  `input`. "Required at" / "missing fields" errors = your fault.
- **Connection errors:** `has_active_connection: false` is a
  toolkit-level Composio↔external auth, not MCP-transport auth.
  Call `mcp__right__composio__COMPOSIO_MANAGE_CONNECTIONS` as the
  upstream tells you. Do NOT suggest MCP dashboard re-auth for Composio. (See
  "MCP Error Diagnosis → Trust upstream diagnostics" in your main
  prompt.)
