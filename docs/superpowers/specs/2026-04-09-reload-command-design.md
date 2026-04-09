# Design: `rightclaw reload` Command

## Problem

Adding new agents to a running rightclaw requires `rightclaw down` + `rightclaw up` — a full restart that kills all running agents. There's no way to hot-add agents or re-sync configuration without downtime.

## Solution

Two changes:

1. **`rightclaw reload`** — regenerate all codegen artifacts and hot-update the running process-compose instance via its REST API.
2. **`agent init` hint** — after creating a new agent, suggest running `rightclaw reload` if the system is running.

## Design

### `rightclaw reload` command

**CLI interface:**

```
rightclaw reload [--agents x,y]
```

**Behavior:**

1. Health-check process-compose on port 18927. If not running, error: `"nothing running, use 'rightclaw up'"`.
2. Discover all agents from `agents/` directory (full scan, same as `up`).
3. If `--agents` filter provided, restrict **codegen** to those agents only. The process-compose yaml is always generated from the full agent set (PC does a full state diff — omitting agents would cause them to be stopped).
4. Run per-agent codegen for selected agents (settings.json, .claude.json, .mcp.json, agent_def.md, reply-schema.json, skills install, memory DB init, agent secret, credential symlink).
5. Regenerate `~/.rightclaw/run/process-compose.yaml` from all discovered agents (minijinja template).
6. `POST /project/configuration` to process-compose REST API (empty body) — tells PC to re-read its yaml from disk. PC diffs against running state: starts new processes, stops removed ones, restarts changed ones.
7. Print summary of active agents.

**Error handling:**

- PC not running → miette error with help text pointing to `rightclaw up`.
- Agent name in `--agents` not found → same error as `up` (list available agents).
- PC reload POST fails → propagate HTTP error with status and body.

### Codegen extraction

The per-agent codegen loop currently lives inline in `cmd_up` (`crates/rightclaw-cli/src/main.rs`, ~lines 700-960). Extract into a shared function in the core crate:

```rust
// crates/rightclaw/src/codegen/pipeline.rs

pub fn run_agent_codegen(
    home: &Path,
    agents: &[AgentDef],
    global_cfg: &GlobalConfig,
) -> miette::Result<()>
```

This function encapsulates:
- Per-agent settings.json, agent_def.md, reply-schema.json generation
- Memory DB initialization
- Agent secret generation + mcp.json with Bearer token
- Built-in skills installation
- .claude.json with trust entries
- Credential symlink creation
- process-compose.yaml generation (minijinja)

**After extraction:**
- `cmd_up` = `run_agent_codegen(...)` + launch PC
- `cmd_reload` = health check + `run_agent_codegen(...)` + `POST /project/configuration`

**Not extracted** (stays in `cmd_up` only): PC launch logic, port availability checks, detach mode, stale process detection.

### PcClient addition

One new method on `PcClient`:

```rust
/// Tell process-compose to re-read its configuration from disk.
pub async fn reload_configuration(&self) -> miette::Result<()>
```

Implementation: `POST /project/configuration` with empty body. Error on non-2xx response.

### `agent init` UX change

After successful `rightclaw agent init <name>`, print:

```
Agent '<name>' created at ~/.rightclaw/agents/<name>/

If rightclaw is running, apply changes with:
  rightclaw reload
```

Always print the hint — no runtime detection of whether PC is running. Simple, no side effects.

### `--agents` filter semantics

The `--agents` flag on `reload` controls **codegen scope**, not **PC yaml scope**:

| Step | `--agents` provided | No filter |
|------|-------------------|-----------|
| Agent discovery | All agents | All agents |
| Codegen | Only named agents | All agents |
| PC yaml generation | All agents | All agents |
| PC reload | Full state diff | Full state diff |

This ensures that omitted agents aren't accidentally stopped by PC's diff logic, while still allowing targeted codegen re-runs for efficiency.

## Non-goals

- Auto-detecting running state in `agent init` and calling reload automatically.
- Modeling PC's full Project struct in Rust (we use `POST /project/configuration` which re-reads from disk, not `POST /project` which accepts JSON).
- Graceful handling of PC restart bugs (the existing `cmd_restart` workaround remains separate).

## Dependencies

- process-compose v1.94.0+ (already installed) — `POST /project/configuration` endpoint.
- No new crate dependencies.
