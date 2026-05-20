---
name: right-memory
description: Manage your long-term memory powered by Hindsight
---

# Memory Management

Your memory is powered by Hindsight. It works in two ways:

**Automatic:** Your conversations are retained and relevant context
is recalled before each interaction. You don't need to do anything
for this to work.

## Remember request router

When the user says "remember", "save this", or "don't forget", first classify what kind of persistent fact it is. The word "remember" means "persist this", not "call `mcp__right__memory_retain`".

- Tool/API/environment rules -> `TOOLS.md`
- Stable user facts, preferences, timezone, communication style -> `USER.md`
- Your voice, autonomy, pushback, escalation boundaries -> `SOUL.md`
- Core identity, principles, security posture -> `IDENTITY.md`
- Reusable procedures -> learned skill
- Narrow durable context with no better home -> memory

After this routing, memory is the fallback for facts with no better home.

**Explicit tools** — use when automatic isn't enough:

- `mcp__right__memory_retain(content, context)` — save a fact permanently
  - `context` is a short label: "user preference", "session correction",
    "api format", "mistake to avoid"
  - Example: after hitting an unexpected API shape, retain the correct
    payload structure with context "api format"

- `mcp__right__memory_recall(query)` — search your memory
  - Use before answering questions about past work or making decisions
    that might have prior context
  - Returns ranked results from semantic + keyword + graph search

- `mcp__right__memory_reflect(query)` — deep analysis across memories
  - Use for synthesizing patterns, comparing past decisions,
    understanding evolution of a project
  - More expensive than recall — use when you need reasoning, not lookup

## What belongs in memory

Memory is for facts that don't have a home in your files
(`TOOLS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`):

- Granular or time-stamped observations too narrow for `USER.md`
- Corrections specific to one session's context
- Cross-session conversational context transcripts won't reconstruct

## What does NOT belong in memory

Route these to the correct file instead of calling `mcp__right__memory_retain`:

- "Use tool X for task Y" → `TOOLS.md` (static, always in prompt;
  semantic recall may miss it when the query doesn't name the tool)
- Stable user preferences → `USER.md`
- Your identity / values / tone → `IDENTITY.md` / `SOUL.md`
- Reusable procedures → save as a skill
- Task progress or completed-work logs → session transcripts already
  cover these

## Write declaratively, not imperatively

Memory is re-read as context on future turns. Imperative phrasing
("Always do X") gets interpreted as a directive and can override the
user's current request.

- `"User prefers pytest-xdist for parallel tests"` ✓
- `"Always run tests with pytest -n 4"` ✗
- `"API foo returns 422 when `input` is used instead of `arguments`"` ✓
- `"Use `arguments` for API foo"` ✗ (this is a rule — goes in `TOOLS.md`)

## Red flags — route elsewhere instead of retaining

- User says "remember to use …" about a tool → `TOOLS.md`
- You discovered a user's stable preference → `USER.md`

If the fact is a rule ("when X, do Y"), it belongs in a file that's
always in your prompt — not in memory that may or may not surface it.
