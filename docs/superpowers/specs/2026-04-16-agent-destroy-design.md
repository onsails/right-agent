# Agent Destroy Command

## Problem

No way to remove an agent from RightClaw. Users must manually delete directories, sandboxes, and worry about process-compose state. Need a single command that cleanly tears down all agent artifacts.

## CLI Interface

```
rightclaw agent destroy <name> [--backup] [--force]
```

- `<name>` — agent name (required)
- `--backup` — create backup before destroying
- `--force` — skip interactive prompts (for scripting)

## Interactive Flow (no `--force`)

1. **Validate** — agent exists in `~/.rightclaw/agents/<name>/`, parse `agent.yaml`
2. **Summary** — print what will be destroyed:
   - Agent directory path and size on disk
   - Sandbox name if `sandbox.mode: openshell`, or "no sandbox" for `mode: none`
   - `data.db` presence and size
   - Whether process-compose is running and agent process is active
3. **Backup prompt** — "Create backup before destroying?" (default: no)
4. If yes — run backup, print backup location
5. **Final confirmation** — red-styled `inquire::Confirm`: "Permanently destroy agent '<name>'? This cannot be undone." (default: no). Use `inquire::ui::RenderConfig` with red prompt color.
6. Execute destruction

With `--force`: skip steps 2–5. `--backup --force` creates backup without asking.

## Architecture

### Core function (library crate)

Location: `crates/rightclaw/src/agent/destroy.rs`

```rust
pub struct DestroyOptions {
    pub agent_name: String,
    pub backup: bool,
    pub pc_port: u16,
}

pub struct DestroyResult {
    pub agent_stopped: bool,
    pub sandbox_deleted: bool,
    pub backup_path: Option<PathBuf>,
    pub dir_removed: bool,
    pub pc_reloaded: bool,
}

pub async fn destroy_agent(
    home: &Path,
    options: &DestroyOptions,
) -> miette::Result<DestroyResult>;
```

### CLI handler (CLI crate)

Location: `crates/rightclaw-cli/src/main.rs`

`cmd_agent_destroy()` handles:
- Agent discovery and validation
- Interactive prompts (inquire) when `--force` is not set
- Calls `destroy_agent()` with resolved options
- Prints human-readable summary from `DestroyResult`

## Execution Steps

Order matters — later steps depend on earlier ones succeeding or being skipped gracefully.

### 1. Stop agent via process-compose (non-fatal)

- Attempt `PcClient::health_check()` to detect if PC is running
- If running: `PcClient::stop_process("{name}-bot")` — graceful stop
- If PC unreachable: skip (platform not running, nothing to stop)
- Failure: warn and continue

### 2. Backup (if requested, fatal)

- Reuse existing backup logic (extract shared function from `cmd_agent_backup`)
- Sandboxed agents: sandbox tar + agent config files
- Non-sandboxed agents: agent directory tar
- Output to `~/.rightclaw/backups/<name>/<YYYYMMDD-HHMM>/`
- Failure: abort — do not destroy without successful backup if user requested one

### 3. Delete sandbox (non-fatal, sandboxed agents only)

- If `sandbox.mode: openshell`: call `delete_sandbox()` with resolved sandbox name
- If `sandbox.mode: none`: skip entirely
- Already best-effort (existing function warns on failure)
- Failure: warn and continue — sandbox may already be gone

### 4. Remove agent directory (fatal)

- `std::fs::remove_dir_all(~/.rightclaw/agents/<name>/)`
- Failure: abort with error

### 5. Reload process-compose (non-fatal)

- If PC is reachable: regenerate `process-compose.yaml` (agent no longer discovered by `discover_agents`), call `reload_configuration()` so PC drops the removed process
- If PC unreachable: skip — next `rightclaw up` will generate correct config
- Failure: warn and continue

### 6. Return result

`DestroyResult` struct with booleans reflecting what happened. CLI prints summary.

## What is NOT destroyed

- Existing backups in `~/.rightclaw/backups/<name>/` — untouched
- Other agents — unaffected
- Global config (`~/.rightclaw/config.yaml`) — untouched
- Logs in `~/.rightclaw/logs/` — untouched (they rotate naturally)

## Platform States

The command handles three platform states:

| State | PC running? | Agent running? | Behavior |
|-------|------------|----------------|----------|
| Active | yes | yes | Stop via PC → destroy → reload PC |
| Inactive | yes | no (crashed/stopped) | Skip stop → destroy → reload PC |
| Down | no | no | Skip stop → destroy → skip reload |

## Non-sandbox Mode

Agents with `sandbox: mode: none` skip sandbox-related steps entirely. The destroy flow is: stop via PC (if running) → optional backup (dir tar only) → remove agent directory → reload PC (if running).

## Error Strategy

- Steps 1, 3, 5 are **non-fatal**: warn via `tracing::warn!`, reflect in `DestroyResult` booleans, continue
- Step 2 (backup) is **fatal if requested**: if user asked for backup and it fails, abort before any deletion
- Step 4 (dir removal) is **fatal**: if we can't remove the directory, the destroy failed

## Testing

- Unit test: `DestroyResult` construction
- Integration test: destroy non-sandboxed agent (create temp agent dir, destroy, verify gone)
- Integration test: destroy with `--force` flag (no TTY needed)
- Integration test: destroy nonexistent agent returns error
