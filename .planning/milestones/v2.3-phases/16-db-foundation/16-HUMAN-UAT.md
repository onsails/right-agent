---
status: partial
phase: 16-db-foundation
source: [16-01-SUMMARY.md, 16-02-SUMMARY.md, 16-03-SUMMARY.md]
started: 2026-03-27T09:35:00Z
updated: 2026-03-27T09:35:00Z
---

## Current Test

[awaiting human testing]

## Tests

### 1. DB created on first `rightclaw up`
expected: After `rightclaw up`, each agent directory contains `memory.db`. File did not exist before launch.
result: [pending]

### 2. DB schema is correct
expected: `sqlite3 <agent-dir>/memory.db ".schema"` shows `memories`, `memory_events`, and `memories_fts` tables with correct columns.
result: [pending]

### 3. WAL mode enabled
expected: `sqlite3 <agent-dir>/memory.db "PRAGMA journal_mode;"` returns `wal`.
result: [pending]

### 4. DB persists across restart
expected: After `rightclaw down` + `rightclaw up`, `memory.db` still exists and prior entries are intact.
result: [pending]

### 5. Doctor reports sqlite3 availability
expected: `rightclaw doctor` reports sqlite3 check result (pass or actionable warning).
result: [pending]

## Summary

total: 5
passed: 0
issues: 0
pending: 5
skipped: 0
blocked: 0

## Gaps
