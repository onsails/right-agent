---
id: SEED-015
status: dormant
planted: 2026-04-02
planted_during: v3.0 Teloxide Bot Runtime (phase 28.2 UAT)
trigger_when: Next milestone — any work touching process-compose codegen or agent startup
scope: tiny
---

# SEED-015: Add MCP_CONNECTION_NONBLOCKING=1 to generated process-compose environment

## Why This Matters

CC waits for all MCP connections to be established before starting. With
`MCP_CONNECTION_NONBLOCKING=1`, CC starts immediately and connects to MCP servers
in the background — agent is responsive from first second instead of hanging on
slow/unavailable MCP servers.

One env var, one line in the template. Zero risk.

## When to Surface

**Trigger:** Next milestone — surface whenever touching `templates/process-compose.yaml.j2`
or agent startup/codegen work.

## Scope Estimate

**Tiny** — One line in the Jinja2 template:
```yaml
      - MCP_CONNECTION_NONBLOCKING=1
```

## Breadcrumbs

- `templates/process-compose.yaml.j2:10–17` — `environment:` block where the line goes
- `crates/rightclaw/src/codegen/process_compose.rs:8` — template is embedded here via `include_str!`
- `crates/rightclaw/src/agent/types.rs:83` — `env_vars` field exists on AgentDef if per-agent override ever needed

## Notes

Verify the env var name hasn't been renamed in newer CC releases before shipping.
Consider making it opt-out via `agent.yaml` (`mcp_nonblocking: false`) if an agent
needs synchronous MCP startup for correctness — but default should be `true`.
