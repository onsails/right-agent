---
name: right-composio
description: >-
  Use when the user's request maps to a Composio-fronted service
  (Notion, Gmail, Calendar, Slack, GitHub, etc.) and you're about to
  call mcp__right__composio__*. Covers workbench-vs-context discipline,
  MULTI_EXECUTE batching, and search_tools discovery. Activate ONLY
  when composio is in your MCP list.
---

# /right-composio — Composio MCP playbook

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
  apply — ask the user to `/mcp add composio <url>`.

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

- **Unknown toolkit slug?** Always
  `mcp__right__composio__COMPOSIO_SEARCH_TOOLS` first. Don't guess —
  slugs change.
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
  upstream tells you. Do NOT suggest `/mcp auth composio`. (See
  "MCP Error Diagnosis → Trust upstream diagnostics" in your main
  prompt.)
