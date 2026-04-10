# Agent SSH Command Design

## Summary

Add `rightclaw agent ssh <name> [-- <command>...]` CLI command that lets users SSH into running agent sandboxes. Additionally, inject agent awareness into the system prompt so sandboxed agents know to suggest this command when they encounter operations requiring interactive terminals.

## Motivation

Agents running in OpenShell sandboxes cannot perform interactive operations (TUI prompts, password input, interactive CLI flows like `gh auth login`). Users currently have no easy way to get a shell on an agent's sandbox. This command bridges that gap and teaches agents to delegate interactive work to the user.

## CLI Command

### Usage

```
rightclaw agent ssh <name>            # interactive shell
rightclaw agent ssh <name> -- <cmd>   # run command and exit
```

### Implementation

1. Discover agent by name via existing `discover_agents()`
2. Validate `sandbox.mode == openshell` — error if `none`
3. Check agent is running via process-compose REST API (`runtime::client`)
4. Locate SSH config at `<agent_dir>/ssh_config`
5. Unix `exec` into `ssh -F <config> openshell-rightclaw-<name> [cmd]`

Uses `std::process::Command::exec()` (from `std::os::unix::process::CommandExt`) to replace the rightclaw process with SSH, giving the user a native interactive terminal.

### Clap Definition

New variant in the existing `AgentCommand` enum:

```rust
/// SSH into an agent's sandbox
Ssh {
    /// Agent name
    name: String,
    /// Command to run (optional)
    #[arg(last = true)]
    command: Vec<String>,
},
```

### Error Cases

All errors via `miette` diagnostics, consistent with existing CLI:

| Condition | Message |
|-----------|---------|
| Agent not found | `Agent '<name>' not found` |
| Sandbox mode = none | `Agent '<name>' runs without sandbox, SSH not available` |
| Agent not running | `Agent '<name>' is not running. Start it with: rightclaw up` |
| SSH config missing | `SSH config not found at <path>. Try restarting the agent.` |

## Agent Awareness

### System Prompt Addition

Added to `generate_system_prompt()` in `codegen/agent_def.rs`, only for agents with `sandbox: openshell`:

```markdown
## User SSH Access

If an operation requires an interactive terminal (TUI, interactive prompts,
password input) that you cannot perform from within your sandbox — tell the
user to run:

  rightclaw agent ssh <AGENT_NAME>
  rightclaw agent ssh <AGENT_NAME> -- <command>

Examples:
- `gh auth login`
- `gcloud auth login`
- `npm login`
- Any command with interactive prompts or TUI

Always provide the exact command with the `--` separator when passing a specific command.
```

`<AGENT_NAME>` is substituted from the agent config at generation time.

## Files to Modify

| File | Change |
|------|--------|
| `crates/rightclaw-cli/src/main.rs` | Add `Ssh` variant to `AgentCommand`, implement handler |
| `crates/rightclaw/src/codegen/agent_def.rs` | Add SSH awareness block to `generate_system_prompt()` |

## Testing

- Unit test: system prompt contains SSH block for openshell agents, absent for sandbox=none
- Integration test: `rightclaw agent ssh nonexistent` returns appropriate error
- Manual test: SSH into running sandboxed agent, run interactive command
