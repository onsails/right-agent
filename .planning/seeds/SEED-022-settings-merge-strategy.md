---
id: SEED-022
status: dormant
planted: 2026-04-03
planted_during: v3.1 (post-completion)
trigger_when: agent autonomy improvements — plugins, MCP, self-configuration
scope: Medium
---

# SEED-022: Merge strategy for agent-modified settings.json

## Why This Matters

`rightclaw up` regenerates `.claude/settings.json` from scratch every launch (`agent.yaml` is source of truth). If an agent modifies its own settings during runtime — e.g., user asks agent to enable a plugin, add an MCP server, or adjust permissions — those changes are silently wiped on next restart.

This breaks a natural user expectation: "I told my agent to add X, why is X gone after restart?"

## When to Surface

**Trigger:** When working on agent autonomy — plugins, MCP servers, self-configuration capabilities.

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches any of these conditions:
- Agent self-configuration or autonomy features
- Plugin/MCP management
- Settings or configuration system refactoring
- Agent persistence improvements

## Scope Estimate

**Medium** — Needs design decisions about which keys are mergeable vs rightclaw-owned, conflict resolution strategy, and tests for edge cases.

### Design Questions to Resolve

1. **Which keys are agent-writable?** Sandbox settings must stay rightclaw-owned (security boundary). But `enabledPlugins`, `mcpServers`, custom permissions could be agent-writable.
2. **Merge direction:** Read existing settings.json before generating, overlay rightclaw-owned keys, preserve agent-added keys? Or maintain a separate `settings.agent.json` that gets merged?
3. **Conflict resolution:** What if agent added a domain to `allowedDomains` that rightclaw also generates? Deduplicate? Union?
4. **Source of truth split:** `agent.yaml` owns sandbox/security. Agent owns plugins/MCP/preferences. Clear boundary needed.

### Possible Approaches

- **A) Deep merge**: Read existing, overlay rightclaw keys, preserve unknown keys. Simple but fragile — hard to know what's "unknown" vs "removed by user in agent.yaml".
- **B) Split files**: `settings.json` = rightclaw-owned (regenerated). `settings.local.json` = agent-writable (preserved). CC would need to support layered settings (may not).
- **C) Allowlist merge**: Explicitly list which top-level keys agents can modify. Read existing, preserve only allowlisted keys, regenerate everything else.
- **D) Agent.yaml feedback loop**: Agent writes changes to `agent.yaml` via MCP tool instead of settings.json directly. `rightclaw up` regenerates from updated agent.yaml. Single source of truth preserved.

Approach D is the most architecturally clean but requires an MCP tool for agents to modify their own agent.yaml.

## Breadcrumbs

- `crates/rightclaw/src/codegen/settings.rs` — current generation logic (full overwrite)
- `crates/rightclaw/src/codegen/settings_tests.rs` — existing tests for settings generation
- `crates/rightclaw/src/init.rs:68` — where settings.json is written during init
- `crates/rightclaw-cli/tests/home_isolation.rs` — integration tests reading settings.json
- `crates/rightclaw/src/doctor.rs:168` — doctor validates settings.json structure
- SEED-008 (managed-settings) — related but different: system-level vs project-level settings

## Notes

- CC's `.claude/settings.json` is a flat JSON file — no native support for layered configs
- `CLAUDE_CONFIG_DIR` env var can redirect `.claude/` but doesn't help with merge
- Current `generate_settings()` returns `serde_json::Value` — easy to add merge logic on top
- Related to SEED-008: managed-settings is system-wide, this is per-agent runtime state
