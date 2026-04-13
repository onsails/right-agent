# Fix MCP tool name prefix mismatch + cleanup

## Context

CC prefixes all MCP tools with `mcp__{server}__`. Server registered as `"right"` in `.mcp.json` -> agent sees tools as `mcp__right__store_record`, `mcp__right__mcp_list`, etc.

But all skills, templates, prompts, and codegen reference bare names (`store_record`, `mcp_list`). Agent can't match instructions to real tool names -> ToolSearch spiral -> wasted turns -> broken workflows (e.g. Composio OAuth endpoint not found because 10/20 turns burned on ToolSearch).

No convention exists to keep tool names in sync when they change.

## Changes

### 1. Add prefix to agent-facing tool references

Every place an agent reads tool names needs `mcp__right__` prefix.

**Skills:**
- `skills/rightmcp/SKILL.md` — `mcp_list()` -> `mcp__right__mcp_list()`
- `skills/rightcron/SKILL.md` — all 7 cron tools: `cron_create` -> `mcp__right__cron_create`, etc.

**Templates (compiled into system prompt):**
- `templates/right/prompt/OPERATING_INSTRUCTIONS.md:23-26` — 4 memory tools
- `templates/right/agent/BOOTSTRAP.md:63` — `bootstrap_done`

**Codegen (Rust string literals):**
- `crates/rightclaw/src/codegen/agent_def.rs:135` — `mcp_list` in base prompt

**Docs:**
- `PROMPT_SYSTEM.md:162,186` — `bootstrap_done` and tool list references

**Deprecated stdio server (low priority but fix for consistency):**
- `crates/rightclaw-cli/src/memory_server.rs:433-451` — `with_instructions()` lists all 13 bare names

**NOT changed** (server-side, bare names correct here):
- `crates/rightclaw-cli/src/right_backend.rs` — tool definitions and dispatch match
- `crates/rightclaw-cli/src/aggregator.rs` — routing logic

### 2. Delete dead `identity/` directory

`identity/AGENTS.md`, `identity/IDENTITY.md`, `identity/SOUL.md` — no Rust code references them. Templates live in `templates/right/`. Just `rm -rf identity/`.

### 3. Add CLAUDE.md convention

Add to Conventions section in `CLAUDE.md`:

> **MCP tool names in agent-facing text**: CC prefixes MCP tools as `mcp__{server}__{tool}`. The RightClaw server is `"right"`, so agents see `mcp__right__<tool>`. All skills, templates, prompts, and codegen that reference tool names for agents must use the full prefixed form. When adding, removing, or renaming tools, update references in: `skills/`, `templates/right/`, `crates/rightclaw/src/codegen/agent_def.rs`, `PROMPT_SYSTEM.md`.

### 4. Update existing CLAUDE.md `with_instructions()` convention

Current rule references `memory_server.rs` — verify it's still accurate or update to reflect that aggregator is the primary server now.

## Execution order

1. Skills + templates + codegen (the actual fix)
2. CLAUDE.md convention (prevent recurrence)
3. Delete `identity/` (cleanup)
4. PROMPT_SYSTEM.md (docs sync)

## Verification

1. `cargo check --workspace` — codegen string changes compile
2. `cargo test --workspace` — no test assertions on old bare names
3. Grep for remaining bare tool names in agent-facing files: `rg '\bstore_record\b|\bmcp_list\b|\bcron_create\b|\bbootstrap_done\b' skills/ templates/ PROMPT_SYSTEM.md` — should only match non-agent-facing contexts (Rust dispatch code, test assertions)
4. Deploy to sandbox, send agent a message like "add composio" — verify agent calls `mcp__right__mcp_list` on first try without ToolSearch spiral
