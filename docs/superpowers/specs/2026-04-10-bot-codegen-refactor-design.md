# Bot Codegen Refactor

**Date:** 2026-04-10
**Status:** Approved

## Problem

Per-agent codegen (settings.json, agent defs, policy.yaml, mcp.json, TOOLS.md, skills, schemas) runs in `rightclaw up` before process-compose launches. After launch, `up` exits. When agent config changes at runtime (`rightclaw agent config`), the bot restarts but cannot regenerate these files because codegen lives in `up`. This causes stale artifacts -- most critically, policy.yaml not reflecting network_policy changes in agent.yaml.

## Design

### Responsibility Split

**`rightclaw up` (orchestrator):**
1. Discover agents (parse agent.yaml -- name, token, restart policy, sandbox mode only)
2. Generate `agent-tokens.json` (cross-agent bearer tokens for right MCP auth)
3. Validate at least one agent has Telegram token
4. Generate `process-compose.yaml` (minijinja)
5. Generate cloudflared config (if tunnel configured)
6. Launch process-compose

**`rightclaw bot` (per-agent, at startup):**
1. Parse agent.yaml (already does this)
2. Run per-agent codegen (NEW):
   - `.claude/settings.json`, `.claude/settings.local.json`
   - `.claude/agents/{name}.md`, `.claude/agents/{name}-bootstrap.md`
   - `.claude/system-prompt.md`
   - Copy identity files (AGENTS.md, BOOTSTRAP.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md, TOOLS.md) into `.claude/agents/`
   - `.claude.json` (trust + onboarding)
   - `.claude/.credentials.json` symlink
   - `mcp.json`
   - `TOOLS.md` (generate_tools_md)
   - `.claude/reply-schema.json`, `.claude/bootstrap-schema.json`
   - `.claude/shell-snapshots/` directory
   - Built-in skills install (rightskills, rightcron)
   - `memory.db` init
   - `agent.yaml` secret generation (if missing)
   - `git init` (if missing)
   - `policy.yaml` -- regenerated from `network_policy` field in agent.yaml
3. Sandbox setup (already does this -- now with fresh policy.yaml)
4. Start Telegram dispatcher (already does this)

### Code Changes

#### New function: `run_single_agent_codegen()`

Extract per-agent codegen from `pipeline.rs::run_agent_codegen()` into a standalone function:

```rust
/// Run codegen for a single agent. Called by bot at startup.
pub fn run_single_agent_codegen(
    home: &Path,
    agent: &AgentDef,
    exe_path: &Path,
    debug: bool,
) -> miette::Result<()>
```

This function contains the per-agent loop body from the current `run_agent_codegen()`, plus policy.yaml generation (currently only in `init`).

#### Simplify `run_agent_codegen()`

After extraction, `run_agent_codegen()` (called by `up`) becomes:
- Generate `agent-tokens.json` (cross-agent)
- Generate `process-compose.yaml`
- Generate cloudflared config
- Write `runtime-state.json`

All per-agent codegen calls are removed.

#### Bot calls codegen at startup

In `crates/bot/src/lib.rs::run()`, after parsing agent.yaml and before sandbox setup:

```rust
// Per-agent codegen: regenerate all derived files from agent.yaml + identity files.
let agent_def = rightclaw::agent::discover_single_agent(&agent_dir)?;
rightclaw::codegen::run_single_agent_codegen(&home, &agent_def, &self_exe, args.debug)?;
```

#### policy.yaml generation in codegen

`run_single_agent_codegen()` calls `generate_policy()` and writes `policy.yaml` based on `network_policy` from agent.yaml. This replaces the current behavior where policy.yaml is only generated during `init` and never updated.

### Config Change Flow

```
rightclaw agent config
  -> writes agent.yaml
  -> config_watcher detects change
  -> bot exits with code 2
  -> process-compose restarts bot (on_failure policy)
  -> bot starts
  -> run_single_agent_codegen() generates fresh files including policy.yaml
  -> apply_policy() applies new policy to sandbox
  -> ready
```

### What Stays the Same

- `rightclaw init` / `rightclaw agent init` -- still creates initial skeleton (agent.yaml, template identity files, initial policy.yaml)
- Sandbox creation -- still in `agent init`, not moved to bot
- config_watcher + exit(2) restart mechanism -- already implemented
- `initial_sync` + background sync -- unchanged

### Edge Cases

- **First run after `init`, before `up`:** Bot won't run yet (PC not launched). `init` creates enough for the agent dir to exist. `up` generates PC yaml and launches. Bot does codegen on first startup.
- **Multiple agents:** Each bot instance does codegen independently for its own agent. No cross-agent coordination needed (agent-tokens.json handled by `up`).
- **Codegen failure in bot:** Bot fails to start, exits non-zero, PC logs error. Same as any other startup failure.
- **`up` without prior `init`:** `up` needs agent.yaml to exist for PC yaml generation. Currently fails with an error -- unchanged.
