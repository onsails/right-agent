---
id: SEED-018
status: dormant
planted: 2026-04-02
planted_during: v3.0 Teloxide Bot Runtime (phase 28.2 UAT)
trigger_when: Next milestone — any work on MCP, agent defaults, or memory
scope: small
---

# SEED-018: Fix rightmemory — rename key to rightclaw, grant permissions, add skill

## Why This Matters

An agent tried `mcp__rightmemory__recall` and got a permission prompt — no one approved it
because headless agents can't. Three compounding problems:

1. **Key name mismatch**: `.mcp.json` key is `"rightmemory"` → tools are `mcp__rightmemory__*`,
   but `server_info.name = "rightclaw"` (per `memory_server.rs:375` test). The plan was to
   rename the key to `"rightclaw"` so tool names become `mcp__rightclaw__*` — never happened.

2. **No permissions.allow**: `settings.json` has no `permissions` block at all — agents
   can't use their own memory tools without a human approving each call. Fix: add
   `"mcp__rightmemory__*"` (or `"mcp__rightclaw__*"` after rename) to `permissions.allow`.

3. **No memory skill**: There's no `rightmemory/SKILL.md` skill that teaches agents *when
   and how* to use memory tools. BOOTSTRAP.md doesn't mention memory. Agents discover memory
   exists only by accident.

## When to Surface

**Trigger:** Next milestone — surface whenever touching agent defaults, `.mcp.json` codegen,
`settings.rs`, or any memory-related work.

## Scope Estimate

**Small** — Three sub-tasks, each straightforward:

### 1. Rename `.mcp.json` key: `rightmemory` → `rightclaw`
- `crates/rightclaw/src/codegen/mcp_config.rs:39` — change key string `"rightmemory"` to `"rightclaw"`
- Update all tests in the same file
- `rightclaw up` re-generates `.mcp.json` on start → migration automatic for live agents
- Tools become `mcp__rightclaw__recall`, `mcp__rightclaw__store`, etc.

### 2. Add `mcp__rightclaw__*` to `permissions.allow`
- `crates/rightclaw/src/codegen/settings.rs:67` — add `permissions` block
- (Note: SEED-016 also adds WebFetch/WebSearch here — do both together)

### 3. Add `rightmemory/SKILL.md` built-in skill
- `crates/rightclaw/src/codegen/skills.rs` — add new skill entry
- Skill content: when to recall (session start), when to store (important facts, decisions),
  tool names, examples
- Also mention in BOOTSTRAP.md that memory tools are available

## Breadcrumbs

- `crates/rightclaw/src/codegen/mcp_config.rs:39` — key `"rightmemory"` to rename
- `crates/rightclaw/src/codegen/settings.rs:67` — `serde_json::json!` block for permissions
- `crates/rightclaw/src/codegen/skills.rs:1–15` — `install_builtin_skills` — add rightmemory skill here
- `crates/rightclaw-cli/src/memory_server.rs:244–250` — `get_info()` returns name `"rightclaw"` already
- `templates/right/BOOTSTRAP.md` — no mention of memory tools at all

## Notes

- After rename, existing agent `.mcp.json` files have stale `rightmemory` key — `rightclaw up`
  re-runs `generate_mcp_config` which overwrites the entry, so migration is automatic.
  BUT: if an agent has approved `mcp__rightmemory__*` permissions manually in settings.json,
  those become stale. Low risk since permissions.allow is managed by rightclaw, not the user.
- Coordinate with SEED-016 (default permissions block) — implement together to avoid touching
  `settings.rs` twice.
