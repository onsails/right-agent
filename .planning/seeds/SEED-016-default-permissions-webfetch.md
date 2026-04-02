---
id: SEED-016
status: dormant
planted: 2026-04-02
planted_during: v3.0 Teloxide Bot Runtime (phase 28.2 UAT)
trigger_when: Next milestone — any work touching settings.rs codegen or agent defaults
scope: small
---

# SEED-016: Add default permissions block (WebFetch + others) to generated settings.json

## Why This Matters

Currently `generate_settings()` produces no `permissions` block — so agents must manually
approve every WebFetch call at runtime, or the operator has to add `--dangerously-skip-permissions`
bypasses. Agents that do research, fetch docs, or call external APIs need WebFetch silently
allowed from the start.

Without a default `permissions.allow`, a headless agent (`claude -p`) simply can't use
WebFetch — it has no TTY to approve the prompt.

## When to Surface

**Trigger:** Next milestone — surface whenever touching `crates/rightclaw/src/codegen/settings.rs`
or agent default configuration.

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches:
- Agent capability defaults / settings codegen
- Any work on what agents can do out of the box
- Onboarding / "new agent works immediately" improvements

## Scope Estimate

**Small** — Add `permissions` block to the `serde_json::json!` literal in `settings.rs`,
audit which tools make sense as defaults, add to `SandboxOverrides` schema in `agent.yaml`
for per-agent opt-out/extension, update tests.

## Proposed Defaults

Starting point — needs audit before shipping:
```json
"permissions": {
  "allow": [
    "WebFetch",
    "WebSearch"
  ],
  "deny": []
}
```

Candidates to evaluate:
- `WebFetch` — fetch URLs, docs, APIs ✅ obvious yes
- `WebSearch` — search the web ✅ probably yes
- `Bash(curl:*)` — may overlap with WebFetch, evaluate
- `mcp__*` — blanket MCP tool allow, or keep per-tool

## Breadcrumbs

- `crates/rightclaw/src/codegen/settings.rs:67` — `serde_json::json!({...})` block where `permissions` key goes
- `crates/rightclaw/src/codegen/settings.rs:6` — `DEFAULT_ALLOWED_DOMAINS` — parallel pattern for a `DEFAULT_ALLOWED_TOOLS` const
- `crates/rightclaw/src/agent/types.rs:83` — `env_vars` / `SandboxOverrides` struct — add `allow_tools: Vec<String>` here for per-agent override
- `crates/rightclaw/src/codegen/settings_tests.rs` — update assertions to include permissions block

## Notes

CC `permissions.allow` format: array of strings. Tool names match CC tool names exactly
(`"WebFetch"`, `"WebSearch"`, `"Bash(git log:*)"`, `"mcp__rightmemory__*"` etc).
Verify current CC docs for exact format — it may have changed.
