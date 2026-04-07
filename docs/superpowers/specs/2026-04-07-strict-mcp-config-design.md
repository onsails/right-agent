# Strict MCP Config: Block Cloud MCPs, Guarantee rightmemory

**Date:** 2026-04-07
**Status:** Approved
**Supersedes:** Partial assumptions in 2026-04-06-mcp-isolation-token-refresh-design.md (line 120: "No --strict-mcp-config needed")

## Problem

Two bugs discovered via Claude debug log analysis on the sandbox:

1. **Cloud MCPs load uncontrolled.** CC connects to 6 cloud MCP servers (Blockscout, Canva, Crypto.com, Figma, Gmail, Google Calendar) via `tengu_claudeai_mcp_connectors` feature flag in `.claude.json`. OpenShell sandbox isolation doesn't prevent this — CC initiates the connections from inside the sandbox. Cost: ~3s per `claude -p` invocation for MCP startup + ~60 deferred tools bloating the system prompt, causing unnecessary ToolSearch API round-trips.

2. **`.mcp.json` missing after sandbox reuse.** The file is uploaded only during sandbox creation (staging dir). `sync_cycle` uploads settings.json, reply-schema.json, skills, and .claude.json — but not `.mcp.json`. On sandbox reuse (the common path — sandboxes are persistent), rightmemory MCP is absent.

## Solution

### Change 1: `--strict-mcp-config` in `invoke_cc`

**File:** `crates/bot/src/telegram/worker.rs` — `invoke_cc()`

Add `--mcp-config <path> --strict-mcp-config` to `claude_args` in both execution branches:

- **Sandbox (SSH):** `--mcp-config /sandbox/.mcp.json --strict-mcp-config`
- **Direct (--no-sandbox):** `--mcp-config <agent_dir>/.mcp.json --strict-mcp-config`

Insert after `--dangerously-skip-permissions`, before `--output-format`.

`--strict-mcp-config` tells CC to use ONLY servers from the specified `--mcp-config` path, ignoring cloud/account MCP connectors entirely.

### Change 2: `.mcp.json` in `sync_cycle`

**File:** `crates/bot/src/sync.rs` — `sync_cycle()`

Add `.mcp.json` upload between step 2 (reply-schema.json) and step 3 (skills):

```rust
let mcp_json = agent_dir.join(".mcp.json");
if mcp_json.exists() {
    rightclaw::openshell::upload_file(sandbox, &mcp_json, "/sandbox/")
        .await
        .map_err(|e| miette::miette!("sync .mcp.json: {e:#}"))?;
}
```

Destination is `/sandbox/` (project root, not `.claude/`). CC reads `.mcp.json` from project root.

This also covers `initial_sync` since it calls `sync_cycle` internally.

## Token Refresh Flow (unchanged, verified correct)

```
OAuth callback (host)
  → Bearer token written to .mcp.json (host-side)
  → RefreshEntry sent to refresh scheduler

Refresh scheduler (host)
  → Proactive refresh 10 min before expiry
  → Updates Bearer in .mcp.json (host-side)
  → Immediately re-uploads .mcp.json to sandbox via openshell upload_file

sync_cycle (every 5 min)
  → Re-uploads .mcp.json to sandbox (safety net if refresh upload failed)
```

Two delivery paths for updated tokens: immediate (refresh.rs) and periodic (sync.rs). No changes needed.

## Out of Scope

- `ENABLE_CLAUDEAI_MCP_SERVERS=false` env var — redundant with `--strict-mcp-config`
- Changes to `generate_mcp_config_http()` — generation is correct
- Changes to `refresh.rs` — already re-uploads immediately after refresh
- Changes to `oauth_callback.rs` — already writes Bearer to `.mcp.json`
- Changes to `login.rs` — orthogonal concern

## Expected Impact

- **-3s per `claude -p` call** (no cloud MCP server initialization)
- **-60 deferred tools** in system prompt → fewer spurious ToolSearch round-trips
- **rightmemory always available** after sandbox reuse
- **Deterministic MCP set** — only what's in `.mcp.json`, nothing else

## Files Changed

| File | Change |
|------|--------|
| `crates/bot/src/telegram/worker.rs` | Add `--mcp-config` + `--strict-mcp-config` to both SSH and direct branches |
| `crates/bot/src/sync.rs` | Add `.mcp.json` upload to `sync_cycle()` |
