# Phase 19: HOME Isolation Hardening - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-27
**Phase:** 19-home-isolation-hardening
**Areas discussed:** Telegram detection fix, RC_AGENT_NAME propagation, Shell snapshot cleanup, Fresh-init UAT scope, AgentDef.mcp_config_path removal

---

## Telegram Detection Fix

| Option | Description | Selected |
|--------|-------------|----------|
| Use agent.config directly | Check agent.config telegram fields in codegen; remove "telegram: true" from .mcp.json | ✓ |
| Parse .mcp.json content | Content-based detection at discovery time | |
| Add telegram_enabled to AgentDef | New explicit bool field from agent.yaml | |

**User's choice:** Use agent.config directly (with preview confirming the implementation pattern)
**Notes:** Remove `"telegram": true` marker entirely — it was always a workaround. `mcp_config_path` stays for other purposes initially.

---

## RC_AGENT_NAME Propagation

| Option | Description | Selected |
|--------|-------------|----------|
| MCP config env section | generate_mcp_config injects RC_AGENT_NAME into env object | ✓ |
| Shell wrapper export | export RC_AGENT_NAME in wrapper template; inherited via env | |

**User's choice:** MCP config env section
**Notes:** User asked: should memory-server fail loudly if RC_AGENT_NAME not set?
Clarification provided: `stored_by` is tool-level provenance, not agent-level (DB is already per-agent structurally). Loss is annoying not catastrophic — warn on stderr, don't fail. Consistent with project's degraded-but-functional pattern.

---

## Shell Snapshot Cleanup

| Option | Description | Selected |
|--------|-------------|----------|
| Delete all on each up | Purge shell-snapshots/ contents before agents start | |
| Leave it to CC | Pre-creation already prevents hard error; CC manages lifecycle | ✓ |
| Delete files older than 24h | Time-based cleanup | |

**User's choice:** Leave it to CC
**Notes:** Pre-creation (commit 1364435) is sufficient. Stale snapshots are harmless.

---

## Fresh-Init UAT Scope

| Option | Description | Selected |
|--------|-------------|----------|
| Manual test guide only | 7-test HUMAN-UAT.md | ✓ |
| Automated integration tests | Rust assert_cmd tests | |
| Both | Automated + manual | |

**User's choice:** Manual test guide only (with the 7-test preview checklist confirmed)
**Notes:** Phases 16 pending UAT tests are subsumed into this guide. Automated regression tests for specific bugs (D-06) are separate from UAT and added per TDD mandate.

---

## AgentDef.mcp_config_path Removal

| Option | Description | Selected |
|--------|-------------|----------|
| Keep it | mcp_config_path stays; only remove from Telegram codegen paths | |
| Remove from AgentDef | Field removed; status display uses inline .mcp.json check | ✓ |

**User's choice:** Remove from AgentDef
**Notes:** mcp_config_path was always semantically "Telegram marker", not "has MCP config". Removing it eliminates the bug vector. 24 references need updating — mechanical changes.

---

## Claude's Discretion

- Update or remove `wrapper_with_mcp_includes_channels_flag` test that validates buggy behavior
- Exact inline check for `.mcp.json` existence in `rightclaw list` status display
- Whether `mcp_config.rs` test `preserves_non_mcp_servers_keys` needs updating after "telegram: true" removal

## Deferred Ideas

- SEED-002 (BOOTSTRAP.md / Telegram onboarding) — separate investigation
- Stale shell snapshot cleanup — revisit if accumulation causes issues in practice
- Automated fresh-init integration tests — v2.4 candidate
