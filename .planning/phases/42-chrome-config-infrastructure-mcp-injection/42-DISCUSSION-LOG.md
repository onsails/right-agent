# Phase 42: Chrome Config Infrastructure + MCP Injection - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions captured in CONTEXT.md — this log preserves the analysis.

**Date:** 2026-04-06
**Phase:** 42-chrome-config-infrastructure-mcp-injection
**Mode:** discuss

## Gray Areas Identified

| Area | Description |
|------|-------------|
| MCP binary path resolution | How rightclaw up finds the chrome-devtools-mcp binary (absolute path required by INJECT-01) |

## Discussion

### MCP binary path resolution

**Question asked:** INJECT-01 requires an absolute path to chrome-devtools-mcp in .mcp.json. How should rightclaw up resolve it?

**Options presented:**
- Auto-discover via `which` at up time
- Explicit config field in config.yaml
- `which` with config override

**User decision:** `mcp_binary_path` stored in `config.yaml` under `chrome.mcp_binary_path`. Set by `rightclaw init` (Phase 43) via `which chrome-devtools-mcp` first, falling back to standard path locations on Linux and macOS. Phase 42 only reads the stored value.

## Pre-decided (from requirements/codebase)

All other implementation details were clear from REQUIREMENTS.md and existing code patterns:
- Args list (INJECT-02): fully specified
- Additive merge pattern (SBOX-02): Vec::extend already in settings.rs
- Config struct pattern: follow TunnelConfig/RawTunnelConfig
- Injection points in cmd_up(): generate_settings() (~line 618) and generate_mcp_config() (~line 701)
