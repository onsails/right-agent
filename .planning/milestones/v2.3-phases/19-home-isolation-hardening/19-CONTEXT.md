# Phase 19: HOME Isolation Hardening - Context

**Gathered:** 2026-03-27
**Status:** Ready for planning

<domain>
## Phase Boundary

Fix three HOME isolation gaps introduced by prior phases:

1. **Telegram false-positive** — `mcp_config_path.is_some()` was the Telegram signal. Phase 17 now writes `.mcp.json` to ALL agents (for rightmemory). Every agent incorrectly gets `--channels plugin:telegram@...` and `enabledPlugins: telegram` injected.

2. **RC_AGENT_NAME missing** — `generate_mcp_config` writes `"env": {}`. Memory server reads `RC_AGENT_NAME` for stored_by provenance; falls back to `"unknown"`. Agent name is never recorded on stored memories.

3. **Comprehensive fresh-init UAT** — Phases 16 UAT has 5 pending tests. Need end-to-end validation from a clean state covering all v2.3 capabilities.

Shell snapshot stale-file cleanup is OUT OF SCOPE — pre-creation (commit 1364435) is sufficient. Leave CC to manage its own snapshot lifecycle.

</domain>

<decisions>
## Implementation Decisions

### Telegram Detection Fix

- **D-01:** Replace `mcp_config_path.is_some()` with agent.yaml config check in both:
  - `shell_wrapper.rs`: `let telegram = agent.config.as_ref().map(|c| c.telegram_token.is_some() || c.telegram_token_file.is_some()).unwrap_or(false);`
  - `settings.rs`: same check → `enabledPlugins: {telegram@...}` only when Telegram actually configured

- **D-02:** Remove `"telegram": true` marker from `.mcp.json` entirely. The `generate_telegram_channel_config` function in `telegram.rs` should stop writing this key to `.mcp.json`. The marker was always a workaround — rightclaw has the authoritative config in `agent.yaml`.

- **D-03:** Remove `mcp_config_path` field from `AgentDef` struct. It served two purposes (Telegram detection + status display), both now resolved differently:
  - Telegram detection → D-01 (use agent.config)
  - Status display in `rightclaw list` → check `.mcp.json` existence inline at display time
  - Discovery: remove the `mcp_config_path: optional_file(&path, ".mcp.json")` line
  - 24 references across the codebase need updating — all test helper structs drop the field

### RC_AGENT_NAME Propagation

- **D-04:** `generate_mcp_config` gains `agent_name: &str` parameter. Injects into env section:
  ```json
  "env": {"RC_AGENT_NAME": "right"}
  ```
  Caller in `cmd_up` passes `&agent.name`.

- **D-05:** Memory server startup: if `RC_AGENT_NAME` is absent or empty → log to stderr:
  `"warning: RC_AGENT_NAME not set — memories will record stored_by as 'unknown'"`
  Do NOT fail. Degraded provenance is acceptable; crashing is not.

  Rationale: each agent has its own `memory.db` so agent-level attribution is structural.
  `stored_by` is tool-level provenance. Losing it is annoying, not catastrophic.

### Regression Tests (TDD — per CLAUDE.md mandate)

- **D-06:** Write TWO failing regression tests BEFORE fixing bugs:

  1. `wrapper_without_telegram_omits_channels_when_mcp_json_exists` — construct AgentDef with `mcp_config_path: Some(...)` but no telegram config in `AgentConfig`. Assert `--channels` is absent in wrapper output. This test currently FAILS (bug demonstration).

  2. `mcp_config_env_contains_agent_name` — call `generate_mcp_config(dir.path(), Path::new("rightclaw"), "myagent")`. Assert `parsed["mcpServers"]["rightmemory"]["env"]["RC_AGENT_NAME"] == "myagent"`. Currently FAILS (env is `{}`).

  After writing these failing tests, proceed to implement D-01 through D-05.

### Fresh-Init UAT

- **D-07:** Write `19-HUMAN-UAT.md` with 7 manual test cases (see below). This file is the UAT document for Phase 19. Phases 16 pending tests are subsumed here (DB and doctor checks covered in test 2 and 7).

  UAT test checklist:
  1. `rm -rf ~/.rightclaw && rightclaw init` — directory structure created, default agent scaffolded
  2. `rightclaw up` — all files generated: `memory.db`, `settings.json`, `.claude.json`, `.mcp.json`, credential symlink
  3. `.mcp.json` content: `mcpServers.rightmemory.env.RC_AGENT_NAME` present; no `"telegram": true` key
  4. Agent WITHOUT telegram: wrapper has no `--channels`, `settings.json` has no `enabledPlugins`
  5. Agent WITH telegram: wrapper has `--channels plugin:telegram@claude-plugins-official`, `.env` has bot token
  6. Memory round-trip: agent session stores memory via MCP → `rightclaw memory list <agent>` shows entry with correct `stored_by` (agent name, not "unknown")
  7. `rightclaw doctor` — all checks pass (bubblewrap, socat, sqlite3, git)

### Claude's Discretion

- Whether to update the existing `wrapper_with_mcp_includes_channels_flag` test (line 126 in `shell_wrapper_tests.rs`) to use telegram config instead of `mcp_config_path`. It currently tests the broken behavior — update to test the correct behavior as part of D-06.
- Exact inline check for `.mcp.json` existence in `rightclaw list` status display (e.g., `agent.path.join(".mcp.json").exists()`).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Files to modify (bug fixes)
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — line 30: `mcp_config_path.is_some()` → D-01 Telegram check
- `crates/rightclaw/src/codegen/settings.rs` — line 96: `mcp_config_path.is_some()` → D-01 Telegram check
- `crates/rightclaw/src/codegen/mcp_config.rs` — add `agent_name` param, inject `RC_AGENT_NAME` into env (D-04)
- `crates/rightclaw/src/codegen/telegram.rs` — remove `{"telegram": true}` write to `.mcp.json` (D-02)
- `crates/rightclaw/src/agent/types.rs` — remove `mcp_config_path` from `AgentDef` (D-03)
- `crates/rightclaw/src/agent/discovery.rs` — line 120: remove `mcp_config_path: optional_file(...)` (D-03)
- `crates/rightclaw-cli/src/memory_server.rs` — line 186: add warning if RC_AGENT_NAME absent (D-05)
- `crates/rightclaw-cli/src/main.rs` — line 307: status display inline check + update `generate_mcp_config` call (D-03, D-04)

### Test files to update (field removal cascade)
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` — update `wrapper_with_mcp_includes_channels_flag` to use telegram config; add regression test D-06/test-1
- `crates/rightclaw/src/codegen/settings_tests.rs` — remove `mcp_config_path` from `make_agent_def()`; add regression test D-06/test-2 (via mcp_config.rs tests)
- `crates/rightclaw/src/codegen/process_compose_tests.rs` — remove `mcp_config_path: None` from `AgentDef` constructors
- `crates/rightclaw/src/codegen/claude_json.rs` — same field removal
- `crates/rightclaw/src/agent/discovery_tests.rs` — update test at line 174

### Prior phase context
- `.planning/phases/17-memory-skill/17-CONTEXT.md` — MCP server pattern, generate_mcp_config origin (SKILL-05)
- `.planning/phases/09-agent-environment-setup/09-CONTEXT.md` — Telegram per-agent config decisions (D-04 through D-08)

No external specs — requirements fully captured in decisions above.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `optional_file(&path, ".mcp.json")` in `discovery.rs` — helper that returns `Option<PathBuf>`. After D-03, MCP existence is checked inline; this helper is reused for other optional files.
- `agent.config.as_ref().and_then(...)` — established pattern for accessing optional AgentConfig fields. D-01 follows this pattern.
- `std::io::stderr` + `tracing::warn!` — established stderr warning pattern in memory_server startup. D-05 uses same approach.

### Established Patterns
- `generate_wrapper(agent, prompt_path, debug_log)` call signature in `cmd_up` — the `generate_mcp_config` call gets an additional `&agent.name` parameter (D-04). Same pattern as existing callers that pass agent data.
- `serde_json::json!({"env": {"RC_AGENT_NAME": name}})` merge into existing object — `mcp_config.rs` already does non-destructive merge of `mcpServers` key; same approach for `env` within the entry.
- `rightclaw list` inline file existence check — no dedicated function needed; `agent.path.join(".mcp.json").exists()` in the status formatting block.

### Integration Points
- `cmd_up` per-agent loop calls `generate_mcp_config(&agent.path, &self_exe)` at step 11 → becomes `generate_mcp_config(&agent.path, &self_exe, &agent.name)` (D-04)
- 24 references to `mcp_config_path` across 8 files — all need updating for D-03 field removal
- `wrapper_with_mcp_includes_channels_flag` test currently validates the buggy behavior — it will be rewritten as part of D-06

</code_context>

<specifics>
## Specific Notes

- The `"telegram": true` key in `.mcp.json` was a detection workaround added in Phase 9 (`generate_telegram_channel_config` line 37). After D-01/D-02, this workaround is removed entirely — detection is via `agent.config`, not filesystem markers.
- The existing `mcp_config.rs` test `preserves_non_mcp_servers_keys` explicitly tests that `"telegram": true` is preserved in `.mcp.json`. After D-02, this test should be updated/removed — we no longer want to preserve or write this key.
- `RC_AGENT_NAME` absence warning: stderr only (MCP transport uses stdout for JSON-RPC per D-03 in Phase 17 context).
- The `mcp_config_path` field removal touches 24 references but the changes are mechanical: remove the field from `AgentDef`, remove the assignment in `discovery.rs`, drop from test helper structs. No logic changes except D-01/D-03 codegen.

</specifics>

<deferred>
## Deferred Ideas

- SEED-002: BOOTSTRAP.md onboarding doesn't trigger via Telegram — separate investigation, not HOME isolation
- Stale shell snapshot cleanup — left to CC per D-03 discussion; can revisit if accumulation causes issues
- Automated integration tests for fresh-init flow — manual UAT chosen for Phase 19; automation is v2.4 candidate

</deferred>

---

*Phase: 19-home-isolation-hardening*
*Context gathered: 2026-03-27*
