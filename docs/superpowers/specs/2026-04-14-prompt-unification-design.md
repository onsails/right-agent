# Prompt Assembly Unification

Remove dead `--agent` code, unify system prompt assembly into a single shell template, stop forward-syncing agent-managed files, fix bootstrap file creation path.

## Problem

Four related issues:

1. **Dead code**: Codegen generates `.claude/agents/{name}.md` agent definitions with `@` file references, copies IDENTITY/SOUL/USER/MEMORY.md into `.claude/agents/`, deploys them to `/platform/` via platform_store — but `--agent` flag is never passed to `claude -p`. All of this code does nothing.

2. **Bootstrap writes to wrong location**: The bootstrap agent sees copies of identity files in `.claude/agents/` (from codegen) and writes new IDENTITY.md, SOUL.md, USER.md there instead of `/sandbox/` (CWD). The system prompt assembly reads from `/sandbox/*.md`, so the files are never picked up.

3. **Duplicated prompt assembly**: Two functions do the same thing differently — `build_sandbox_prompt_assembly_script()` (shell, for sandbox) and `assemble_host_system_prompt()` (Rust string concat, for no-sandbox). Same logic, two code paths to maintain.

4. **Unnecessary forward sync**: `initial_sync` uploads IDENTITY/SOUL/USER/MEMORY.md from host to sandbox on every startup, overwriting sandbox copies. If the agent edited files since last `reverse_sync`, the edits are lost. Agent-managed files should not be forward-synced.

## Testing Proof: `--agent` Does Not Work

Testing confirmed that `--agent` with `@` file references does NOT inline file content into the system prompt. Instead, CC's model uses the Read tool to fetch files at runtime — consuming turns, breaking prompt caching, and behaving unpredictably. This is why we use `--system-prompt-file` exclusively.

## Design

### 1. Remove `.claude/agents/` Dead Code

**Delete from `agent_def.rs`:**
- `generate_agent_definition()`
- `generate_bootstrap_definition()`
- `CONTENT_MD_FILES` constant
- Related tests in `agent_def_tests.rs`

**Delete from `codegen/mod.rs`:**
- Re-exports: `generate_agent_definition`, `generate_bootstrap_definition`, `CONTENT_MD_FILES`

**Delete from `pipeline.rs` (`run_single_agent_codegen`):**
- Lines 69-114: agent def generation, bootstrap def generation, `.claude/agents/` dir creation, CONTENT_MD_FILES copy
- Related tests

**Delete from `main.rs`:**
- `rightclaw init` (~559-633): agent def codegen + `verify_sandbox_files` for `.claude/agents/`
- `rightclaw agent init` (~844-929): same
- `rightclaw agent exec` (~2181-2214): agent def generation + `--agent` flag usage

**Fix `rightclaw agent exec`:**
- Currently uses `--agent` flag (line 2209) — the only place in the codebase that does
- `--agent` doesn't work (proven by testing: `@` refs not inlined, wastes turns)
- Replace with: assemble system prompt on host (same shell template as no-sandbox mode), pass `--system-prompt-file`

**Delete from `platform_store.rs`:**
- Lines 107-134: scanning `.claude/agents/` and deploying agent def files to `/platform/`

**Delete from integration tests:**
- `cli_integration.rs`: assertions for `.claude/agents/right.md`, `right-bootstrap.md`
- `openshell_tests.rs`: staging dir setup for `.claude/agents/`
- `platform_store_tests.rs`: agent def entries in manifest

### 2. Unify Prompt Assembly

Replace two functions with one: a Rust function that generates a shell script parameterized by `root_path`.

**New function** `build_prompt_assembly_script()`:

Parameters:
- `base_prompt: &str` — from `generate_system_prompt()`
- `bootstrap_mode: bool`
- `root_path: &str` — `/sandbox` (sandbox) or absolute `agent_dir` path (no-sandbox)
- `prompt_file: &str` — `/tmp/rightclaw-system-prompt.md` (sandbox) or `agent_dir/.claude/composite-system-prompt.md`
- `workdir: &str` — `/sandbox` (sandbox) or `agent_dir` (no-sandbox)
- `claude_args: &[String]`
- `mcp_instructions: Option<&str>`

Generated script structure:
```sh
{ printf '{base_prompt}'
  printf '\n## Operating Instructions\n'
  printf '%s\n' '{operating_instructions}'
  if [ -f {root}/IDENTITY.md ]; then
    printf '\n## Your Identity\n'
    cat {root}/IDENTITY.md
    printf '\n'
  fi
  if [ -f {root}/SOUL.md ]; then
    printf '\n## Your Personality and Values\n'
    cat {root}/SOUL.md
    printf '\n'
  fi
  if [ -f {root}/USER.md ]; then
    printf '\n## Your User\n'
    cat {root}/USER.md
    printf '\n'
  fi
  if [ -f {root}/AGENTS.md ]; then
    printf '\n## Agent Configuration\n'
    cat {root}/AGENTS.md
    printf '\n'
  fi
  if [ -f {root}/TOOLS.md ]; then
    printf '\n## Environment and Tools\n'
    cat {root}/TOOLS.md
    printf '\n'
  fi
  printf '{mcp_instructions}'
} > {prompt_file}
cd {workdir} && {claude_cmd} --system-prompt-file {prompt_file}
```

Bootstrap mode replaces the Operating Instructions + file sections with just:
```sh
printf '\n## Bootstrap Instructions\n'
printf '%s\n' '{bootstrap_instructions}'
```

**Execution:**
- Sandbox: `ssh -F {config} {host} -- {script}`
- No-sandbox: `bash -c {script}` (or `tokio::process::Command::new("bash").arg("-c").arg(script)`)

**Delete:**
- `build_sandbox_prompt_assembly_script()` — replaced
- `assemble_host_system_prompt()` — replaced

### 3. Remove Forward Sync of Agent-Managed Files

**In `sync.rs` `initial_sync()`:**
- Remove upload of `CONTENT_MD_FILES` (IDENTITY, SOUL, USER, MEMORY) to sandbox
- Remove upload of AGENTS, TOOLS "only if missing" to sandbox
- Keep: `sync_cycle` call (platform store for settings, schemas, skills), `ensure_local_bin_in_path`, `.claude.json` verification

**In `sync.rs`:**
- Remove `use rightclaw::codegen::CONTENT_MD_FILES` import

**In `sync.rs` `REVERSE_SYNC_FILES`:**
- Remove MEMORY.md if present (memory is Claude auto-memory, not a synced file)

**MEMORY.md cleanup:**
- Not part of prompt assembly (already excluded)
- Not needed in forward or reverse sync
- Remove from all sync lists

### 4. Fix Bootstrap File Path

**In `templates/right/agent/BOOTSTRAP.md`:**

Change the "Files to Create" section to explicitly state the path:

```markdown
## Files to Create

Write all files in your current working directory using the Write tool.
Do NOT write them to `.claude/agents/` or any subdirectory.

### IDENTITY.md
...
### SOUL.md
...
### USER.md
...
```

## Files Changed

| File | Change |
|------|--------|
| `crates/rightclaw/src/codegen/agent_def.rs` | Remove `generate_agent_definition`, `generate_bootstrap_definition`, `CONTENT_MD_FILES` |
| `crates/rightclaw/src/codegen/agent_def_tests.rs` | Remove tests for deleted functions |
| `crates/rightclaw/src/codegen/mod.rs` | Remove re-exports |
| `crates/rightclaw/src/codegen/pipeline.rs` | Remove agent def generation + copy, update tests |
| `crates/rightclaw/src/platform_store.rs` | Remove `.claude/agents/` scanning |
| `crates/rightclaw/src/platform_store_tests.rs` | Update tests |
| `crates/rightclaw-cli/src/main.rs` | Remove agent def codegen from init, agent init; remove verify_sandbox_files; rewrite agent exec to use `--system-prompt-file` |
| `crates/rightclaw-cli/tests/cli_integration.rs` | Remove `.claude/agents/` assertions |
| `crates/rightclaw/src/openshell_tests.rs` | Remove `.claude/agents/` staging setup |
| `crates/bot/src/telegram/worker.rs` | Replace two assembly functions with one unified `build_prompt_assembly_script()` |
| `crates/bot/src/sync.rs` | Remove forward sync of agent files, remove MEMORY.md from reverse sync |
| `templates/right/agent/BOOTSTRAP.md` | Add explicit file path instructions |
| `PROMPT_SYSTEM.md` | Update to reflect unified assembly |
| `ARCHITECTURE.md` | Remove `.claude/agents/` references |

## What Stays

- `generate_system_prompt()` — base prompt generation
- `OPERATING_INSTRUCTIONS`, `BOOTSTRAP_INSTRUCTIONS` — compiled-in constants
- `REPLY_SCHEMA_JSON`, `BOOTSTRAP_SCHEMA_JSON`, `CRON_SCHEMA_JSON` — schemas
- `reverse_sync_md()` — backup agent files to host (minus MEMORY.md)
- `should_accept_bootstrap()` — server-side bootstrap validation
- Platform store sync for settings, schemas, skills (non-agent files)
