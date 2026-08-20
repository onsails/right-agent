# Composio auth disambiguation (agnostic prompt fix)

**Date:** 2026-05-06
**Status:** approved

## Problem

Agent `him` told the user to run `/mcp auth composio` after a Composio
tool returned `has_active_connection: false` for Google Docs. That advice
was wrong on two levels:

- `/mcp auth <server>` re-authorizes the **Right Agent ↔ MCP server**
  transport. The Composio MCP token is fine (refreshed every 50 min,
  last refresh logged at 14:58 UTC, valid until 15:58 UTC).
- The `has_active_connection: false` flag describes a **Composio ↔
  downstream app (Google Docs)** OAuth state, managed entirely inside
  Composio. `/mcp auth composio` cannot fix it.

The Composio MCP server already returned a perfectly clear instruction
in its tool payload:

> `"No Active connection for toolkit=googledocs. You MUST call
> COMPOSIO_MANAGE_CONNECTIONS (toolkit=\"googledocs\") to create a
> connection BEFORE executing any googledocs tools."`

The agent ignored that `status_message` because the
`OPERATING_INSTRUCTIONS.md` "MCP Error Diagnosis" table tells it to
react to the broad `unauthorized | forbidden | auth | 401 | 403` pattern
by suggesting `/mcp auth <server>`. The "No Active connection" string
matches loosely on "auth", so the rule fires inside payloads where it
shouldn't.

## Goal

Stop the prompt from overriding upstream's own diagnostic message. Keep
the change vendor-agnostic — no Composio-specific tool names in the
instructions.

## Non-goals

- Aggregator code changes (no rewriting upstream payloads).
- Per-server `instructions` override mechanism (would be useful long
  term, deferred — see "Future work").
- `tools/list_changed` notification handling. The Composio MCP server
  declares `tools.listChanged: true` in its capabilities and Right
  Agent's proxy uses a no-op `()` ClientHandler that ignores it.
  Tracked separately; not in this spec.
- Cleanup of the duplicate `/templates/` directory at repo root (the
  authoritative copy used via `include_str!` is
  `crates/right-agent/templates/`). Tracked separately as a hygiene
  TODO.

## Design

Single-file edit to
`crates/right-agent/templates/right/prompt/OPERATING_INSTRUCTIONS.md`,
section **MCP Error Diagnosis** (lines 179–196).

### Change 1 — Narrow the existing auth-error rule

Replace the broad pattern

> `"unauthorized", "forbidden", "auth", 401, 403`

with the precise MCP-transport signals only:

> HTTP 401/403 from MCP transport, OR the error string
> `"Authentication required for '<server>'. Use /mcp auth <server>"`
> (raised by Right Agent's `ProxyError::NeedsAuth`)

Action remains "Tell the user to run `/mcp auth <server>`".

Rationale: that error string is the canonical signal Right Agent itself
emits (see `crates/right-agent/src/mcp/proxy.rs:53`). HTTP 401/403 from
the MCP transport itself is the other unambiguous case. Loose keyword
matching on tool payload contents is what caused this bug.

### Change 2 — Add a "trust upstream diagnostics" rule

Insert a new row in the table:

| Error pattern | Meaning | Action |
|---|---|---|
| Tool response payload contains a status/instruction field (e.g. `status_message`, `error.message`, `instructions`) telling you what to do next | Upstream tool already diagnosed the issue and prescribed the fix | Follow the upstream instruction verbatim. Do NOT translate it into `/mcp auth` advice. |

### Change 3 — Add a clarifying note under the table

> **Trust upstream diagnostics.** When a tool's own response payload
> tells you what action to take ("call X to set up connection", "visit
> URL Y to authorize", etc.), follow it as-is. `/mcp auth` is a Right
> Agent CLI command for re-authorizing the MCP transport — it is not a
> fix-all for any authentication-shaped error inside tool responses.

## Verification

After bot restart (`right restart him`):

1. In the `aibots` chat, ask `him` to read the same Google Doc that
   triggered the original misdirection.
2. Expected behavior:
   - Agent calls `COMPOSIO_MANAGE_CONNECTIONS` (as Composio's own
     `status_message` instructs), OR
   - Agent relays Composio's `status_message` verbatim to the user.
   - Agent does **not** suggest `/mcp auth composio`.
3. Inspect `~/.right/logs/him.log.<date>`:
   `rg "/mcp auth composio" ~/.right/logs/him.log.<date>` should show
   no new entries in `📝` (assistant text) lines after the restart
   timestamp.

This is a "let's see if it works" verification — the prompt change is
heuristic and behavior depends on the model. If the agent still
misroutes, escalate to option B from brainstorming (a Composio-specific
hardcoded block in the system prompt via `with_instructions()`).

## Risks

- **False negative:** narrowing the auth rule may miss real cases where
  an MCP server returns a 401-like response without our canonical
  `ProxyError::NeedsAuth` wrapper. Mitigation: the `ProxyError::NeedsAuth`
  path covers all OAuth servers managed by Right Agent, which is the
  population that needs `/mcp auth`. Pure non-OAuth servers cannot be
  re-authorized via `/mcp auth` anyway.
- **Ambiguity in "status field":** the agent has to decide whether a
  given payload field counts as "an instruction telling you what to
  do". The clarifying note frames the test in terms of action verbs
  ("call X", "visit Y"), which should be discriminative enough in
  practice. If not, iterate on phrasing.

## Out of scope / future work

- **Option B/C (per-server instruction overrides):** the
  `mcp_servers.instructions` column already exists and is populated
  from upstream `peer_info().instructions`. Composio currently sends
  no instructions string. A `/mcp set-hint <name> "..."` command +
  inclusion in the aggregated system prompt would let operators
  attach guidance to any MCP server without prompt changes.
  Deferred — only worth building once we see a second federation MCP
  with the same problem.
- **`tools/list_changed` handling:** Composio's MCP advertises this
  capability but Right Agent's proxy ignores server→client
  notifications (`()` ClientHandler at `proxy.rs:286`). Doesn't
  affect the current bug because Composio's tool count is fixed at 7
  meta-tools regardless of connected apps.
- **Duplicate `/templates/` at repo root:** identical files but only
  the `crates/right-agent/templates/` copy is referenced by
  `include_str!`. Hygiene cleanup, separate change.
