# System Prompt Restructure: Compiled-In Operating Instructions

## Problem

Template changes don't propagate to existing agents. The `templates/right/AGENTS.md` is copied to the agent directory only during `rightclaw init`. Subsequent `rightclaw up` / bot restarts read from the agent directory copy, which is stale. This caused a production bug where updated MCP management instructions never reached the running agent.

The root issue: platform-owned instructions (identical for every agent) and agent-owned configuration (per-agent customization) are mixed in one file (`AGENTS.md`). Platform instructions should update with the binary; agent configuration should stay on disk.

## Solution

Split the template directory into two categories:

- `prompt/` — content compiled into the binary via `include_str!()`, injected directly into the system prompt at assembly time. Updates take effect on `cargo build` + restart. No file sync needed.
- `agent/` — content written to the agent directory on `rightclaw init`. Agent-owned, editable, synced to sandbox.

## Template Directory Structure

```
templates/right/
├── prompt/
│   └── OPERATING_INSTRUCTIONS.md   # Platform instructions → system prompt
├── agent/
│   ├── AGENTS.md                   # Per-agent (subagents, routing, skills)
│   ├── TOOLS.md                    # Per-agent (tools, env notes) — empty
│   ├── BOOTSTRAP.md                # Bootstrap flow + file structure examples
│   └── agent.yaml                  # Agent configuration
```

### Deleted Files

- `templates/right/IDENTITY.md` — structure moves into BOOTSTRAP.md
- `templates/right/SOUL.md` — structure moves into BOOTSTRAP.md
- `templates/right/USER.md` — structure moves into BOOTSTRAP.md
- `templates/right/AGENTS.md` — split into OPERATING_INSTRUCTIONS.md + reduced AGENTS.md
- `templates/right/BOOTSTRAP.md` — moves to `agent/BOOTSTRAP.md`

## File Contents

### `prompt/OPERATING_INSTRUCTIONS.md`

Contains sections 1–8 from the current AGENTS.md. These are platform-owned instructions identical across all agents:

1. **Your Files** — list of agent-owned files (IDENTITY.md, SOUL.md, USER.md, TOOLS.md, AGENTS.md) with descriptions of what each controls and when to update them
2. **Memory** — how to use MCP memory tools (store_record, query, search, delete)
3. **MCP Management** — explicit instructions that agents CANNOT manage MCP servers; user must use Telegram `/mcp` commands
4. **Communication** — Telegram formatting rules (supported/unsupported markdown)
5. **Message Input Format** — YAML schema for incoming messages with attachments
6. **Sending Attachments** — outbox path, size limits
7. **Cron Management** — run `/rightcron` on startup, use skill for management
8. **Core Skills** — placeholder section for documenting core skills

Section 1 changes from "Identity Files" to "Your Files" and adds TOOLS.md and AGENTS.md:

```markdown
## Your Files

These files are yours. Update them as you evolve.

- `IDENTITY.md` — your name, nature, vibe, emoji
- `SOUL.md` — your personality, values, boundaries
- `USER.md` — what you know about the human
- `TOOLS.md` — your tools, environment notes, integrations
- `AGENTS.md` — your subagents, task routing, installed skills

Update USER.md when you discover meaningful new facts about the user.
Never interview the user — pick up signals naturally through conversation.
```

### `agent/BOOTSTRAP.md`

Current content plus expanded file structure examples from the deleted IDENTITY.md, SOUL.md, USER.md templates. This gives the agent concrete structural guidance during bootstrap.

The expanded "Files to Create" section:

```markdown
## Files to Create

### IDENTITY.md

Name, creature type, vibe, emoji. Use this structure:

- Opening line: "You are {name} -- a {nature} ..." (one sentence)
- **Who you are**: 3–5 bullet points about capabilities and constraints
- **Key principles**: numbered list of core values
- **How you work**: bullet list of operational details (sandbox, scheduling, timestamps)
- **Self-configuration**: table mapping user requests to which file to edit

### SOUL.md

Personality based on chosen vibe. Use this structure:

- **Tone & Style**: bullet list (concise/verbose, formal/casual, emoji policy, language matching)
- **Personality**: bullet list of behavioral traits (helpful but not sycophantic, opinionated about quality, pragmatic, transparent)

### USER.md

What you learned about the human. Start with:

- Preferred name
- Communication style
- Timezone (if mentioned)
- Recurring context and interests
```

The file is both:
- **Content source**: `include_str!()` compiled into binary, injected into system prompt when bootstrap mode is active
- **Existence flag**: written to agent dir on init, deleted on bootstrap completion. `agent_dir.join("BOOTSTRAP.md").exists()` determines bootstrap mode.

### `agent/AGENTS.md`

Reduced to per-agent configuration sections only:

```markdown
# Agent Configuration

## Subagents

<!-- Define your subagents here. Each subagent is a specialized worker. -->
<!-- Example: -->
<!-- ### reviewer -->
<!-- Code review. Read-only fs, git log, posts comments via MCP GitHub. -->

## Task Routing

<!-- Define how tasks get routed to subagents. -->
<!-- If no subagent fits — handle it directly in the main session. -->

## Installed Skills

Check `skills/installed.json` for ClawHub-installed skills.
Scan `.claude/skills/` for manually installed skills.
```

### `agent/TOOLS.md`

Empty file. Created on init as a placeholder. Agent-owned — the agent writes tool documentation here as it adds/configures tools.

## System Prompt Assembly

### Current Structure

```
[Base prompt — generate_system_prompt()]
## Your Identity          ← cat IDENTITY.md
## Your Personality       ← cat SOUL.md
## Your User              ← cat USER.md
## Operating Instructions ← cat AGENTS.md (all sections)
## Environment and Tools  ← cat TOOLS.md
## MCP Server Instructions ← from API
```

### New Structure (Normal Mode)

```
[Base prompt — generate_system_prompt(), unchanged]
## Operating Instructions ← compiled-in OPERATING_INSTRUCTIONS.md constant
## Your Identity          ← cat IDENTITY.md (unchanged)
## Your Personality       ← cat SOUL.md (unchanged)
## Your User              ← cat USER.md (unchanged)
## Agent Configuration    ← cat AGENTS.md (if non-empty, per-agent sections)
## Environment and Tools  ← cat TOOLS.md (unchanged)
## MCP Server Instructions ← from API (unchanged)
```

Operating Instructions moves before identity files because it's platform context the model needs first. Identity/personality are agent-specific context that builds on the platform instructions.

### New Structure (Bootstrap Mode)

```
[Base prompt — generate_system_prompt(), unchanged]
## Bootstrap Instructions ← compiled-in BOOTSTRAP.md constant
```

No change in structure — only the content source changes from file read to compiled-in constant.

### Bootstrap Mode Detection

Unchanged. `ctx.agent_dir.join("BOOTSTRAP.md").exists()` determines bootstrap mode. The file is written to agent dir on init (from `agent/BOOTSTRAP.md` template) and deleted on bootstrap completion.

## Code Changes

### `crates/rightclaw/src/codegen/agent_def.rs`

New compiled-in constants:

```rust
pub const OPERATING_INSTRUCTIONS: &str =
    include_str!("../../../templates/right/prompt/OPERATING_INSTRUCTIONS.md");

pub const BOOTSTRAP_INSTRUCTIONS: &str =
    include_str!("../../../templates/right/agent/BOOTSTRAP.md");
```

Remove `"BOOTSTRAP.md"` from `CONTENT_MD_FILES`. Its content for the system prompt now comes from the compiled-in constant, and the file on disk is only an existence flag (host-side only). The sandbox no longer needs a copy — prompt assembly injects the constant directly, and bootstrap mode detection uses `ctx.agent_dir.join("BOOTSTRAP.md").exists()` on the host. The `bootstrap_done` MCP tool deletes the host-side file; the sandbox never modifies it.

AGENTS.md stays in `CONTENT_MD_FILES` — it's still synced to the sandbox as a per-agent configuration file.

Updated `CONTENT_MD_FILES`:

```rust
pub const CONTENT_MD_FILES: &[&str] = &[
    "AGENTS.md",
    "TOOLS.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
];
```

### `crates/rightclaw/src/init.rs`

Update `include_str!` paths:

```rust
const DEFAULT_AGENTS: &str = include_str!("../../../templates/right/agent/AGENTS.md");
const DEFAULT_BOOTSTRAP: &str = include_str!("../../../templates/right/agent/BOOTSTRAP.md");
const DEFAULT_AGENT_YAML: &str = include_str!("../../../templates/right/agent/agent.yaml");
```

Add TOOLS.md to the files written on init:

```rust
let files: &[(&str, &str)] = &[
    ("AGENTS.md", DEFAULT_AGENTS),
    ("BOOTSTRAP.md", DEFAULT_BOOTSTRAP),
    ("TOOLS.md", ""),
    ("agent.yaml", DEFAULT_AGENT_YAML),
];
```

Remove the separate TOOLS.md creation from `pipeline.rs` since init now handles it.

### `crates/bot/src/telegram/worker.rs`

#### `build_sandbox_prompt_assembly_script`

Replace file-based sections with compiled-in content injection:

**Bootstrap mode**: Instead of `cat /sandbox/.claude/agents/BOOTSTRAP.md`, use `printf '%s'` with the `BOOTSTRAP_INSTRUCTIONS` constant (same pattern as MCP instructions injection).

**Normal mode**: Replace `cat /sandbox/.claude/agents/AGENTS.md` section with `printf '%s'` of `OPERATING_INSTRUCTIONS` constant. Keep `cat` for IDENTITY.md, SOUL.md, USER.md, TOOLS.md (agent-owned files that live in sandbox). Add optional `cat` for AGENTS.md (per-agent configuration, if it has non-comment content).

New section ordering in the shell script:
1. `printf '%s'` base prompt (unchanged)
2. `printf '%s'` OPERATING_INSTRUCTIONS (new — compiled-in)
3. `cat /sandbox/IDENTITY.md` (unchanged)
4. `cat /sandbox/SOUL.md` (unchanged)
5. `cat /sandbox/USER.md` (unchanged)
6. `cat /sandbox/.claude/agents/AGENTS.md` (per-agent config, optional)
7. `cat /sandbox/.claude/agents/TOOLS.md` (unchanged)
8. `printf '%s'` MCP instructions (unchanged — from API)

#### `assemble_host_system_prompt`

Same logic, host-side:

**Bootstrap mode**: Use `BOOTSTRAP_INSTRUCTIONS` constant instead of reading file.

**Normal mode**: Prepend `OPERATING_INSTRUCTIONS` constant. Keep file reads for IDENTITY.md, SOUL.md, USER.md, TOOLS.md. Add optional read for AGENTS.md.

#### Function signatures

`bootstrap_mode: bool` parameter stays — the functions still need to know whether to include identity files or bootstrap content. The only change is the content source: compiled-in constant instead of file read.

### `crates/bot/src/sync.rs`

Remove BOOTSTRAP.md from forward sync. It's no longer needed in the sandbox — content comes from compiled-in constant, and bootstrap mode detection uses the host-side file.

AGENTS.md stays in sync (per-agent configuration needs to reach the sandbox for `cat`).

### `PROMPT_SYSTEM.md`

Update prompt structure documentation to reflect the new assembly. Document that OPERATING_INSTRUCTIONS.md and BOOTSTRAP.md are compiled into the binary.

### `ARCHITECTURE.md`

Update:
- Template directory structure under "Directory Layout"
- Note that operating instructions are compiled-in
- Update prompt assembly description

## Migration

Existing agents have the old full AGENTS.md on disk. After this change:
- The operating instructions (sections 1–8) come from the compiled-in constant — always fresh.
- The old AGENTS.md on disk still has the full content, but sections 1–8 are now redundant (they're in the constant). The per-agent sections (subagents, routing, skills) are the empty comment placeholders, which is exactly what the new template provides.
- No migration script needed. The redundant sections in old AGENTS.md files are harmless — they'll appear in "Agent Configuration" section of the prompt, duplicating some instructions. This is cosmetic and resolves naturally when the user re-inits or manually updates the file.

If clean migration is desired: on bot startup, codegen could detect old-format AGENTS.md (contains `## Memory` or `## MCP Management` sections) and replace it with the new reduced template. This is optional.

## Testing

- Unit tests for `build_sandbox_prompt_assembly_script`: verify OPERATING_INSTRUCTIONS content appears in output, AGENTS.md is optional, bootstrap mode uses BOOTSTRAP_INSTRUCTIONS constant
- Unit tests for `assemble_host_system_prompt`: same verification
- Unit test: compiled-in constants are non-empty and contain expected section headers
- Integration: verify prompt assembly produces correct composite prompt in both modes
- Verify BOOTSTRAP.md existence flag still works for bootstrap mode detection
