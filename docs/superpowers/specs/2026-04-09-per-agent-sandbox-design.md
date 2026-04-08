# Per-Agent Sandbox Configuration

**Date:** 2026-04-09
**Status:** Approved

## Problem

Sandbox enforcement is currently global — `--no-sandbox` on `rightclaw up` applies to all agents. Some agents (e.g., computer-use, Chrome automation) need direct host access while others should run inside OpenShell containers. No way to mix sandboxed and unsandboxed agents in the same invocation.

## Decision

Remove the global `--no-sandbox` flag. Sandbox mode is configured per-agent in `agent.yaml`. No global overrides for anything — each agent is self-contained.

## Data Model

### agent.yaml schema

```yaml
# Sandboxed agent (default when sandbox section omitted)
sandbox:
  mode: openshell
  policy_file: policy.yaml  # relative to agent dir, required when mode=openshell

# Unsandboxed agent (host access)
sandbox:
  mode: none
```

### Rust types

Replace `SandboxOverrides` with:

```rust
#[derive(Debug, Deserialize, Default)]
pub struct SandboxConfig {
    #[serde(default = "default_sandbox_mode")]
    pub mode: SandboxMode,
    pub policy_file: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    #[default]
    Openshell,
    None,
}
```

**Validation:** When `mode == Openshell` and `policy_file` is `None` or the referenced file doesn't exist, fail with a miette diagnostic pointing at agent.yaml.

**Default:** `mode: openshell` — secure by default, opt out explicitly.

## CLI Changes

### Remove global `--no-sandbox`

- Remove from `Up` command
- Remove from `Bot` command
- Remove `no_sandbox` from `ProcessComposeConfig`

### Add `rightclaw agent init <name>`

Interactive wizard that creates a new agent:

1. Agent name (argument, validated: alphanumeric + hyphens)
2. Sandbox mode: `openshell` or `none`?
3. If `openshell`: network policy — `restrictive` or `permissive`? → generates `policy.yaml` in agent dir
4. Telegram token (optional)
5. Model selection (optional)
6. Writes `agent.yaml`, `IDENTITY.md`, `SOUL.md`, `USER.md`, and if openshell, `policy.yaml`

`rightclaw init` becomes: project scaffold (dirs, config.yaml) + `agent_init("right")`. Single implementation, reused.

### Move `rightclaw list` → `rightclaw agent list`

## Runtime Changes

### `rightclaw up`

- **OpenShell preflight:** only run if at least one agent has `mode: openshell`. Skip entirely if all agents are `none`.
- **Policy validation:** no generation — just verify `policy_file` exists for openshell agents.
- **process-compose.yaml:** each agent entry gets sandbox mode from its own config. Per-agent `sandbox_mode` and `policy_path` in template instead of one global flag.

### `rightclaw bot`

Bot reads `agent.yaml` → `sandbox.mode` instead of CLI `--no-sandbox` flag. Remove `BotArgs.no_sandbox`.

- `mode: openshell` → OpenShell lifecycle (gRPC, spawn/reuse, SSH config, policy hot-reload)
- `mode: none` → direct `claude` binary execution, HOME override

### process-compose template

Per-agent sandbox config instead of global flag. Remove `RC_SANDBOX_MODE` and `RC_SANDBOX_POLICY` env vars — bot resolves everything from agent.yaml.

### MCP config

Determined by sandbox mode, not a separate setting:
- `mode: openshell` → HTTP bridge MCP config (`host.docker.internal`)
- `mode: none` → direct stdio MCP config

### Policy file location

Policies live in agent dirs: `agents/<name>/policy.yaml`. No longer generated into `run/policies/`. The `codegen/policy.rs` module becomes a wizard helper that writes `policy.yaml` during `agent init`.

## Migration & Cleanup

### Remove

- `--no-sandbox` flag from `Up` and `Bot` CLI commands
- `SandboxOverrides` struct → replaced by `SandboxConfig`
- `RC_SANDBOX_MODE` env var
- `RC_SANDBOX_POLICY` env var
- `no_sandbox` field from `ProcessComposeConfig`
- Global policy generation in `cmd_up()`

### Move

- Policy generation logic → `agent init` wizard
- `rightclaw list` → `rightclaw agent list`

### Breaking change

Existing agents without `sandbox.mode` default to `openshell` and require a `policy.yaml` in their agent dir. `rightclaw up` fails with a diagnostic explaining what to add. This is intentional — forces explicit configuration.

## Tests

- Process-compose generation: per-agent sandbox config instead of global flag
- Validation: openshell without policy_file → miette error
- Mixed-mode: some openshell, some none in same agent set
- Bot: reads sandbox mode from agent config, not CLI flag
- Agent init wizard: generates correct files for both modes
