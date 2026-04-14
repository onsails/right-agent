# Finding: CC silently drops all MCP tools if any tool has invalid inputSchema

## Date
2026-04-14

## Problem
Claude Code (`claude -p`) connected to our MCP aggregator (status: "connected")
but registered 0 tools. All 14 tools from `tools/list` were silently dropped.

## Root Cause

**Invalid `inputSchema` on `rightmeta__mcp_list` tool.** The tool definition had
`"inputSchema": {}` (empty object) instead of a valid JSON Schema with
`"type": "object"`. CC validates the entire `tools/list` response and silently
drops ALL tools if ANY tool has an invalid schema.

## Investigation Timeline

1. Initial hypothesis: CC refuses plain `http://` MCP servers — **WRONG**.
   Tested on host and in sandbox: CC connects to HTTP MCP servers fine.

2. Protocol version mismatch: rmcp 1.3.0 defaults to `protocolVersion: 2025-06-18`,
   CC sends `2025-03-26`. Tested explicitly: CC loads tools fine with 2025-06-18.
   **Not the issue.**

3. SSE vs JSON response format: Tested both `with_json_response(true)` and
   `with_json_response(false)`. No difference — CC handles both.

4. Stateful vs stateless mode: No difference.

5. **Empty `inputSchema: {}`**: The `rightmeta__mcp_list` tool was defined with
   `Tool::new("rightmeta__mcp_list", "...", serde_json::Map::new())` — producing
   `"inputSchema": {}`. CC expects `"type": "object"` at minimum. Fixing this
   to `{"type": "object"}` immediately loaded all 14 tools.

## Fix

```rust
// Before (broken):
Tool::new("rightmeta__mcp_list", "...", serde_json::Map::new())
// → "inputSchema": {}

// After (fixed):
let mut schema = serde_json::Map::new();
schema.insert("type".into(), serde_json::Value::String("object".into()));
Tool::new("rightmeta__mcp_list", "...", schema)
// → "inputSchema": {"type": "object"}
```

## Key Lessons

- CC silently drops ALL MCP tools if any single tool in `tools/list` has an
  invalid `inputSchema`. No error, no warning, no partial loading.
- `"inputSchema": {}` is not valid — must have `"type": "object"` at minimum.
- rmcp's `Tool::new()` accepts any `serde_json::Map` without validation.
- Always test MCP tool loading after adding new tool definitions.
- Protocol version 2025-06-18 works fine with CC 2.1.91.
- HTTP MCP servers work fine — no HTTPS requirement.
