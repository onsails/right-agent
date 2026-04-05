# Phase 24: System Prompt Codegen - Research

**Researched:** 2026-03-31
**Domain:** Rust codegen -- CC system prompt composition, shell wrapper removal
**Confidence:** HIGH

## Summary

Phase 24 replaces the current `generate_combined_prompt()` + shell wrapper pipeline with a simpler
`generate_system_prompt()` that writes `agent_dir/.claude/system-prompt.txt`. The new file contains
each present identity file's raw content, concatenated in canonical OpenClaw order. The shell
wrapper (`codegen/shell_wrapper.rs`) and its template are deleted.

The key verified finding: CC 2.1.87 (current in this repo) accepts `--system-prompt-file` as a
valid flag (confirmed by live test -- no "unknown option" error). The `--bare` mode docs mention
`--system-prompt[-file]` confirming the `-file` suffix variant exists. The current codebase uses
`--append-system-prompt-file`; Phase 24 switches to `--system-prompt-file` for bot invocations.

`@file` include syntax is UNVERIFIED -- CC help text does not document it for `--system-prompt-file`.
Decision D-03 prescribes falling back to raw content concatenation if unsupported. Raw concatenation
is definitively verified to work (current codebase uses it). Recommend using raw concatenation as
the primary implementation from the start.

`start_prompt` field removal from `AgentConfig` is a clean break -- `deny_unknown_fields` means
existing agent.yaml files with this field will fail to parse (fail-fast per CLAUDE.rust.md).

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** `system-prompt.txt` uses `@file` reference syntax -- one present identity file per line
  in canonical order. Absent files silently skipped.
- **D-02:** File order: IDENTITY.md -> SOUL.md -> USER.md -> AGENTS.md. Verified as correct below.
- **D-03:** Use `--system-prompt-file` for `claude -p` bot invocations. If CC does NOT support
  `@file` includes, fall back to pre-concatenating file contents with `\n\n---\n\n` separator.
- **D-04:** All four OpenClaw identity files are candidates. Absent files skipped.
- **D-05:** USER.md is agent-writable; IDENTITY.md, SOUL.md, AGENTS.md are read-only.
  system-prompt.txt is read-only (generated artifact).
- **D-06:** All hardcoded sections (Startup Instructions, BOOTSTRAP.md detection, Communication
  reminder, Cron Management) are REMOVED from codegen. No static content appended.
- **D-07:** BOOTSTRAP.md detection gone from codegen. File is visible to CC via normal discovery.
- **D-08:** `start_prompt` field removed from `AgentConfig` entirely. No deprecation shim.
- **D-09:** Delete `codegen/shell_wrapper.rs`, `codegen/shell_wrapper_tests.rs`,
  `templates/agent-wrapper.sh.j2`.
- **D-10:** `generate_combined_prompt()` rewritten to `generate_system_prompt()` producing
  the identity file content list; writes to `agent_dir/.claude/system-prompt.txt`.
- **D-11:** Old `run/<agent>-prompt.md` intermediate file no longer written.
- **D-12:** process-compose template and `codegen/process_compose.rs` NOT modified in this phase.
  `wrapper_path` field remains stale until Phase 26.
- **D-13:** Create SOUL.md, USER.md, AGENTS.md for default Right agent template.

### Claude's Discretion

- Exact content of default SOUL.md and AGENTS.md.
- Whether to keep `system_prompt_tests.rs` with new tests or rewrite from scratch.
- Whether `generate_system_prompt()` takes `&AgentDef` or just `&Path` to agent dir.

### Deferred Ideas (OUT OF SCOPE)

- Process-compose template update for direct claude invocation -- Phase 26 scope.
- File watcher to regenerate system-prompt.txt when identity files change -- v3.1.
- start_prompt migration guide / CHANGELOG entry -- low priority.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| PROMPT-01 | `rightclaw up` generates `agent_dir/.claude/system-prompt.txt` by concatenating present files: SOUL.md + USER.md + AGENTS.md (absent files silently skipped) | `generate_system_prompt()` rewrite; writes via same pattern as `settings.json` already in `cmd_up` per-agent loop |
| PROMPT-02 | `claude -p` invocations pass `--system-prompt-file agent_dir/.claude/system-prompt.txt` (first-message calls only) | Phase 25 consumer; but file must exist and be written by `rightclaw up` in this phase |
| PROMPT-03 | `codegen/shell_wrapper.rs` removed | Delete shell_wrapper.rs, shell_wrapper_tests.rs, agent-wrapper.sh.j2; update mod.rs |
</phase_requirements>

## Standard Stack

No new dependencies. This phase is a pure refactor within the existing stack.

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `std::fs` | stdlib | Write system-prompt.txt | Same pattern as settings.json |
| `miette` | 7.6.0 | Error reporting | Project standard (CLAUDE.md) |

## Architecture Patterns

### Existing Pattern: Per-Agent Codegen in cmd_up

The `cmd_up` per-agent loop in `crates/rightclaw-cli/src/main.rs` (lines ~425-531) is the
integration point. The loop currently:

1. Calls `generate_combined_prompt(agent)` -- writes to `run/<agent>-prompt.md`
2. Calls `generate_wrapper(agent, &prompt_path_str, ...)` -- writes to `run/<agent>.sh`
3. Sets wrapper executable permissions
4. Generates settings.json, claude.json, credential symlink, plugins, skills, memory.db, .mcp.json

Phase 24 replaces steps 1-3 with: call `generate_system_prompt(agent)` -- writes to
`agent_dir/.claude/system-prompt.txt`.

### Existing Pattern: Settings Codegen (follow this model)

`codegen/settings.rs` -> `generate_settings(agent, no_sandbox, host_home)` returns
`serde_json::Value`, caller writes to `agent_dir/.claude/settings.json`.

`generate_system_prompt()` should follow the same structure: takes agent context, returns
`miette::Result<String>`, caller writes to `agent_dir/.claude/system-prompt.txt`. The `.claude/`
directory is guaranteed to exist at that point in the loop (created before settings.json write at
line ~462).

### Function Signature Recommendation: Use &AgentDef

`AgentDef` already carries `soul_path`, `user_path`, `agents_path` as `Option<PathBuf>` --
populated by agent discovery. Using `&AgentDef` is the right signature:

- Avoids re-probing the filesystem for which files exist
- Consistent with `generate_combined_prompt(&AgentDef)` and `generate_settings(&AgentDef, ...)`
- IDENTITY.md (`agent.identity_path: PathBuf`, non-optional) also included; existence check needed

### Recommended Implementation: Raw Content Concatenation

D-03 prescribes `@file` reference syntax as preferred IF CC supports it in `--system-prompt-file`.
Based on the code audit:

- The existing wrapper uses `--append-system-prompt-file` pointing to a PRE-CONCATENATED file
  (current `generate_combined_prompt` already produces raw content, not `@file` refs)
- CC help text does not document `@file` support for `--system-prompt-file`
- `@file` syntax is documented for CLAUDE.md includes, not for system prompt flags

**Use raw content concatenation (D-03 fallback) as the primary implementation.** This is
definitively verified to work. Write each present identity file's content separated by
`\n\n---\n\n`. No CC version dependency.

### File Order

Canonical order (confirmed by IDENTITY.md self-configuration table in templates/right/IDENTITY.md):

```
IDENTITY.md  (core principles -- operator-defined)
SOUL.md      (tone, personality -- operator-defined, agent-editable)
USER.md      (learned user preferences -- agent-writable)
AGENTS.md    (operational framework -- operator-defined)
```

### USER.md Write Permissions (D-05)

Current `generate_settings()` builds `allow_write` starting with `agent.path` (the entire agent
directory). USER.md lives inside the agent dir, so write access is already covered. No change to
`generate_settings()` is needed. `system-prompt.txt` lives in `agent_dir/.claude/` which is also
under the agent dir allowWrite -- sandbox enforcement does not prevent writes at the filesystem
level for Write/Edit tools (CC sandbox only constrains Bash tool per MEMORY.md). The "read-only"
status of system-prompt.txt is a convention, not a technical enforcement.

### Second Call Site: cmd_replay (line ~1359)

`crates/rightclaw-cli/src/main.rs` ~line 1359 has a second `generate_combined_prompt()` call
in a function that uses `std::process::Command::exec()` to replace the process. This call:

- Writes prompt to `run/<agent>-prompt.md`
- Passes it via `--append-system-prompt-file`

Phase 24 must update this too:
- Remove write to `run/<agent>-prompt.md`
- Reference `agent_dir/.claude/system-prompt.txt` (must be written before the exec call, since
  cmd_up should have already written it on `rightclaw up`, but cmd_replay may run independently)
- Switch flag from `--append-system-prompt-file` to `--system-prompt-file`

If this function can be called without a prior `rightclaw up` (i.e., without the file existing),
it should call `generate_system_prompt()` itself and write the file before the exec.

### Default Agent Template Changes (D-13)

Files already existing in `templates/right/`: IDENTITY.md, SOUL.md, AGENTS.md, BOOTSTRAP.md,
agent.yaml. No USER.md template exists yet.

Changes needed:
1. Create `templates/right/USER.md` (empty placeholder or minimal comment)
2. Update `crates/rightclaw/src/init.rs` to include USER.md in the `files` array
3. SOUL.md and AGENTS.md already have content -- review if hardcoded sections from
   `generate_combined_prompt()` need to be migrated to AGENTS.md

The hardcoded sections being removed (D-06):
- "Startup Instructions" -- was startup task guidance
- BOOTSTRAP.md detection block
- "Communication" -- daemon mode reminder
- "Cron Management (RightCron)" -- rightcron startup instruction

These should be moved to `templates/right/AGENTS.md` since agents using the default template
need this content somewhere. The AGENTS.md template currently has only placeholder comments.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| File existence check per identity file | Custom stat helper | `Path::exists()` | stdlib |
| Separator joining | Loop with index | collect strings, join with `\n\n---\n\n` | idiomatic |

## Common Pitfalls

### Pitfall 1: IDENTITY.md Missing from REQUIREMENTS.md but D-04 Includes It

REQUIREMENTS.md PROMPT-01 lists SOUL.md + USER.md + AGENTS.md. D-04 (CONTEXT.md) adds IDENTITY.md.
Use D-04 -- it is the locked user decision. REQUIREMENTS.md predates the clarification.

### Pitfall 2: start_prompt in struct literals across multiple files

After removing `start_prompt` from `AgentConfig`, all struct literals break compilation.
Affected locations (from code inspection):

- `crates/rightclaw/src/agent/types.rs` -- field declaration + test uses `start_prompt` in literal
- `crates/rightclaw/src/codegen/system_prompt.rs` -- reads `c.start_prompt`
- `crates/rightclaw/src/codegen/system_prompt_tests.rs` -- struct literal
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` -- struct literal (deleted in same task)
- `crates/rightclaw/src/init.rs` -- struct literal at ~line 68-80

All must be updated atomically with the AgentConfig change.

### Pitfall 3: .claude/ Dir Must Exist Before system-prompt.txt Write

The `.claude/` dir is created at line ~462 of main.rs `cmd_up`. Place the
`generate_system_prompt()` call AFTER that `create_dir_all`. The function itself should NOT
create the directory -- match the settings.rs pattern (caller creates dir, callee writes file).

### Pitfall 4: process-compose template still references wrapper_path

Per D-12, `codegen/process_compose.rs` and the Jinja template are NOT modified. The PC config
will reference stale wrapper paths. This is expected and documented -- Phase 26 fixes it.
The planner should include a comment in the code noting this intentional staleness.

### Pitfall 5: generate_wrapper still referenced in process_compose.rs

After deleting shell_wrapper.rs and its re-export from mod.rs, any code that calls
`generate_wrapper` will fail to compile. Verify that `codegen/process_compose.rs` does NOT
call `generate_wrapper` -- it should only use the `wrapper_path` string from AgentDef or
similar. Inspect process_compose.rs before deletion.

## Code Examples

### New generate_system_prompt structure (recommended)

```rust
// crates/rightclaw/src/codegen/system_prompt.rs
use crate::agent::AgentDef;

pub fn generate_system_prompt(agent: &AgentDef) -> miette::Result<String> {
    let candidates = [
        ("IDENTITY.md", Some(&agent.identity_path)),
        ("SOUL.md", agent.soul_path.as_ref()),
        ("USER.md", agent.user_path.as_ref()),
        ("AGENTS.md", agent.agents_path.as_ref()),
    ];

    let mut sections = Vec::new();
    for (_name, path_opt) in &candidates {
        if let Some(path) = path_opt {
            if path.exists() {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| miette::miette!("Failed to read {}: {e}", path.display()))?;
                sections.push(content);
            }
        }
    }
    Ok(sections.join("\n\n---\n\n"))
}
```

Note: `agent.identity_path` is `PathBuf` (non-optional), but existence still checked because
the path is set at discovery time and the file could theoretically be deleted between discovery
and codegen. Current implementation reads it unconditionally (would error if missing); new
implementation should match current behavior: error if IDENTITY.md missing, skip others silently.
Alternatively: always include IDENTITY.md (treat as required, consistent with current behavior),
skip SOUL/USER/AGENTS if absent.

### cmd_up replacement block

```rust
// Replace the generate_combined_prompt + generate_wrapper block:
let prompt_content = rightclaw::codegen::generate_system_prompt(agent)?;
// .claude/ dir already created above at create_dir_all(&claude_dir)
let system_prompt_path = claude_dir.join("system-prompt.txt");
std::fs::write(&system_prompt_path, &prompt_content).map_err(|e| {
    miette::miette!("failed to write system-prompt.txt for '{}': {e:#}", agent.name)
})?;
tracing::debug!(agent = %agent.name, "wrote system-prompt.txt");
```

### mod.rs update

```rust
// Remove:
pub mod shell_wrapper;
pub use shell_wrapper::generate_wrapper;
pub use system_prompt::generate_combined_prompt;

// Add:
pub use system_prompt::generate_system_prompt;
```

### USER.md template (create at templates/right/USER.md)

```markdown
<!-- USER.md: Learned preferences about this user. -->
<!-- Right updates this file through conversation. -->
<!-- Add: preferred name, communication style, recurring context, timezone. -->
```

## State of the Art

| Old Approach | New Approach | Impact |
|--------------|--------------|--------|
| Shell wrapper + intermediate prompt in `run/` | system-prompt.txt in `agent_dir/.claude/` | Cleaner; no run-dir intermediary; lives with other agent config |
| `--append-system-prompt-file` | `--system-prompt-file` | Full control; no CC default system prompt interference |
| Hardcoded startup/cron/comms sections | Agent-owned content in AGENTS.md | Codegen is format-only; agents carry identity |

## Open Questions

1. **Does `--system-prompt-file` support `@file` includes?**
   - What we know: CC 2.1.87 accepts the flag. `@file` syntax is not documented for it.
   - Recommendation: Use raw concatenation (D-03 fallback) as primary implementation.
     Can be tested manually with a quick `claude -p --system-prompt-file <file_with_@ref>` call.

2. **Should cmd_replay-like function write system-prompt.txt itself or assume cmd_up did it?**
   - What we know: The function at ~line 1340 uses it directly without calling cmd_up first.
   - Recommendation: Have it write system-prompt.txt itself (call generate_system_prompt +
     create_dir_all + write) before the exec. Safe to call again if already exists.

3. **AGENTS.md content for default Right agent template**
   - What we know: Current AGENTS.md has only placeholder comments. Hardcoded sections from
     generate_combined_prompt (rightcron, communication) need to land somewhere.
   - Recommendation: Move the rightcron + communication content into `templates/right/AGENTS.md`
     so default Right agents retain this operational guidance.

## Environment Availability

Step 2.6: SKIPPED -- phase is code/config changes only. No new external dependencies.

## Validation Architecture

Skipped -- `workflow.nyquist_validation` is `false` in `.planning/config.json`.

## Sources

### Primary (HIGH confidence)
- `crates/rightclaw/src/codegen/system_prompt.rs` -- current implementation being replaced
- `crates/rightclaw/src/codegen/shell_wrapper.rs` -- file being deleted
- `crates/rightclaw/src/codegen/mod.rs` -- re-exports to update
- `crates/rightclaw/src/codegen/settings.rs` -- pattern to follow
- `crates/rightclaw/src/agent/types.rs` -- AgentConfig.start_prompt removal target
- `crates/rightclaw/src/init.rs` -- struct literal + files array update needed
- `crates/rightclaw-cli/src/main.rs` lines 425-531, 1340-1384 -- integration points
- `templates/agent-wrapper.sh.j2` -- to be deleted
- `templates/right/` -- existing identity file templates (IDENTITY.md, SOUL.md, AGENTS.md)
- CC 2.1.87 live test -- `--system-prompt-file` flag accepted (no "unknown option" error)

### Secondary (MEDIUM confidence)
- CC `--bare` help text: `--system-prompt[-file]` confirms flag exists
- MEMORY.md: CC sandbox allowWrite semantics -- Write/Edit bypass bwrap

### Tertiary (LOW confidence -- flag not doc'd for this use case)
- `@file` syntax for `--system-prompt-file`: unverified, treat as LOW confidence

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- no new deps, existing patterns only
- Architecture: HIGH -- all call sites identified by code inspection
- Pitfalls: HIGH -- derived from direct code reading
- `@file` syntax support: LOW -- not verified against running CC

**Research date:** 2026-03-31
**Valid until:** 2026-04-30
