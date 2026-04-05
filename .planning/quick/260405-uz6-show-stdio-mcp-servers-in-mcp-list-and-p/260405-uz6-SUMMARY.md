# Quick Task 260405-uz6: Summary

## What Changed

### 1. Stdio MCP servers now appear in `/mcp list` and CLI `mcp status`

Previously, `mcp_auth_status()` in `detect.rs` skipped stdio servers (command+args, no URL). This meant the rightclaw-managed `rightmemory` MCP server was invisible to users.

**Changes:**
- Added `ServerKind` enum (`Http` | `Stdio`) to `detect.rs`
- Added `kind` field to `ServerStatus` struct
- Stdio servers are now included with `state: Present` (they don't need auth) and `url` set to the command name
- Bot displays stdio servers as `rightmemory (.mcp.json) -- stdio`
- CLI displays stdio servers as `<agent>  stdio rightmemory [.mcp.json]`

### 2. `/mcp remove` protects `rightmemory` from deletion

Added `PROTECTED_MCP_SERVER` constant (`"rightmemory"`) in `mcp/mod.rs`. The bot's `/mcp remove` handler rejects attempts to remove this server with: "Cannot remove 'rightmemory' — required for core functionality."

### Tests
- Renamed `stdio_server_skipped` → `stdio_server_included` (verifies kind, state, url)
- Added `mixed_http_and_stdio_both_listed` test
- All 66 tests pass, clippy clean
