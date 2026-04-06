# Phase 43: Init Detection + Up Revalidation - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions captured in CONTEXT.md — this log preserves the discussion.

**Date:** 2026-04-06
**Phase:** 43-init-detection-up-revalidation
**Mode:** discuss
**Areas discussed:** MCP binary discovery, Partial detection handling, Config write restructuring, Up revalidation scope

## Gray Areas Presented

| Area | Selected? |
|------|-----------|
| MCP binary discovery | Yes |
| Partial detection handling | Yes |
| Config write restructuring | Yes |
| Up revalidation scope | Yes |

## Decisions Made

### MCP binary discovery
- **Asked:** How should chrome-devtools-mcp binary be discovered at init?
- **Answer:** `which::which` first, then standard fallback paths: `/usr/local/bin`, `~/.npm-global/bin`; macOS also checks `brew --prefix`
- **Note:** User initially asked about using `npx` — rejected per prior v3.4 research decision ("Never use npx in .mcp.json — absolute path to globally-installed binary only")

### Partial detection handling
- **Asked:** Chrome found but chrome-devtools-mcp not found — what should happen?
- **Answer:** Warn + skip Chrome config entirely. Chrome section only written when BOTH paths resolved.

### Config write restructuring
- **Asked:** How to write Chrome config when cloudflared isn't present (current code early-returns)?
- **Answer:** Collect both chrome_cfg and tunnel_cfg as local vars, write single `GlobalConfig` at end of cmd_init(). Remove write from inside tunnel block.

### Up revalidation scope
- **Asked:** What should trigger skip injection on `rightclaw up`?
- **Answer:** Both paths must exist — if either `chrome_path` or `mcp_binary_path` missing, warn + skip injection for this run.

## Corrections / Overrides

- **npx rejected:** User suggested using npx for chrome-devtools-mcp — corrected per prior research decision. npx breaks sandbox isolation and introduces network dependency.
