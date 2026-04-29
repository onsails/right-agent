---
name: rightmemory
description: Manage your long-term memory via MEMORY.md
---

# Memory Management

Your long-term memory is stored in `MEMORY.md` in your home directory.
This file is automatically injected into your system prompt at the start
of each interaction (truncated to 200 lines).

## How to use

Use Claude Code's built-in `Edit` and `Write` tools to manage MEMORY.md:

- **Add an entry:** Append a new line or section to MEMORY.md
- **Update an entry:** Edit an existing line to correct or refine it
- **Remove stale entries:** Delete lines that are no longer relevant

## What belongs in MEMORY.md

Things that don't have a home in your other files (`TOOLS.md`,
`IDENTITY.md`, `SOUL.md`, `USER.md`):

- Granular or time-stamped observations too narrow for `USER.md`
- Session-specific corrections worth carrying to the next turn
- Cross-session context transcripts won't reconstruct

## What does NOT belong here

Route these to the correct file instead of writing to MEMORY.md:

- "Use tool X for task Y" → `TOOLS.md`
- Stable user preferences → `USER.md`
- Your identity / values / tone → `IDENTITY.md` / `SOUL.md`
- Reusable procedures → save as a skill
- Task progress, TODO state, completed-work logs — transcripts cover these

## Write declaratively, not imperatively

MEMORY.md is re-read as context on every turn. Imperative phrasing
("Always do X") gets interpreted as a directive and can override the
user's current request.

- `"User prefers pytest-xdist for parallel tests"` ✓
- `"Always run tests with pytest -n 4"` ✗
- `"API foo returns 422 when `input` is used instead of `arguments`"` ✓
- `"Use `arguments` for API foo"` ✗ (this is a rule — goes in `TOOLS.md`)

## Keep it concise

MEMORY.md is truncated to **200 lines** in your prompt. Periodically:
- Remove entries no longer relevant
- Consolidate related entries into single lines
- Remove duplicates
- Move tool rules / user preferences / etc. to their proper homes
