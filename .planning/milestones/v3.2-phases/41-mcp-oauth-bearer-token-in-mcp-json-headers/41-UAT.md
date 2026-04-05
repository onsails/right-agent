---
status: testing
phase: 41-mcp-oauth-bearer-token-in-mcp-json-headers
source: [41-01-SUMMARY.md, 41-02-SUMMARY.md]
started: 2026-04-05T20:30:00Z
updated: 2026-04-05T20:30:00Z
---

## Current Test

number: 1
name: /mcp auth writes Bearer token to .mcp.json
expected: |
  Send `/mcp auth notion` in Telegram. After OAuth completes, check agent's .mcp.json — it should have `"headers": {"Authorization": "Bearer <token>"}` on the notion server entry.
awaiting: user response

## Tests

### 1. /mcp auth writes Bearer token to .mcp.json
expected: Send `/mcp auth notion` in Telegram. After OAuth completes, agent's .mcp.json should have `"headers": {"Authorization": "Bearer <token>"}` on the notion server entry. No write to .credentials.json.
result: [pending]

### 2. /mcp list shows ✅ after auth
expected: Send `/mcp list` after OAuth. Notion should show `✅ notion  —  present` (detected from Authorization header, not credentials file).
result: [pending]

### 3. CC agent sees Notion MCP
expected: Ask the agent a Notion question in Telegram (e.g. "which pages do you see in notion?"). Agent should connect to Notion MCP and respond with actual data — NOT say "I don't have a Notion integration."
result: [pending]

### 4. No agent restart after OAuth
expected: After `/mcp auth` completes, bot logs should show token written message but NO "agent restart requested" line.
result: [pending]

### 5. /mcp list shows ❌ before auth
expected: `/mcp remove notion` then `/mcp add notion https://mcp.notion.com/mcp` then `/mcp list`. Notion should show `❌ notion  —  missing` (no Authorization header yet).
result: [pending]

### 6. rightclaw mcp status CLI matches
expected: Run `rightclaw mcp status` — should show same auth state as /mcp list (reads .mcp.json headers, not .credentials.json).
result: [pending]

## Summary

total: 6
passed: 0
issues: 0
pending: 6
skipped: 0
blocked: 0

## Gaps

