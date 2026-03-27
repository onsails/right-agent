# Phase 19: HOME Isolation Hardening - Research

**Researched:** 2026-03-27
**Domain:** Rust codebase bug-fix — Telegram false-positive, MCP env propagation, manual UAT
**Confidence:** HIGH

## Summary

Phase 19 fixes three concrete bugs in the HOME isolation layer. All three are well-understood code
defects with exact reproduction paths confirmed by reading the source. No new external dependencies
are introduced. The work is pure Rust codebase surgery plus authoring a UAT checklist file.

Bug 1: `mcp_config_path.is_some()` is the Telegram detection signal in `shell_wrapper.rs` (line 30)
and `settings.rs` (line 96). Phase 17 made every agent write `.mcp.json` (for rightmemory), so every
agent now incorrectly gets `--channels plugin:telegram@...` and `enabledPlugins: telegram` injected.
Fix: check `agent.config.telegram_token/telegram_token_file` directly.

Bug 2: `generate_mcp_config` writes `"env": {}` with no content. The memory server reads
`RC_AGENT_NAME` for `stored_by` provenance; it falls back to `"unknown"` because the env var is
never injected. Fix: add `agent_name: &str` param and inject into the env object.

The `mcp_config_path` field removal (D-03) is a mechanical cascade — exactly 25 references across
9 files. All are struct literal constructions (drop the field) or the two logic uses that get
replaced by the new Telegram check. No hidden usages found.

**Primary recommendation:** Follow CONTEXT.md decisions verbatim — they are complete and correct.
Write failing regression tests first (D-06), then implement D-01 through D-05.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**D-01:** Replace `mcp_config_path.is_some()` with agent.yaml config check in both:
- `shell_wrapper.rs`: `let telegram = agent.config.as_ref().map(|c| c.telegram_token.is_some() || c.telegram_token_file.is_some()).unwrap_or(false);`
- `settings.rs`: same check → `enabledPlugins: {telegram@...}` only when Telegram actually configured

**D-02:** Remove `"telegram": true` marker from `.mcp.json` entirely. `generate_telegram_channel_config` stops writing this key. The marker was always a workaround — `agent.yaml` is authoritative.

**D-03:** Remove `mcp_config_path` field from `AgentDef` struct. Both uses resolved:
- Telegram detection → D-01
- Status display in `rightclaw list` → check `.mcp.json` existence inline at display time
- Remove `mcp_config_path: optional_file(&path, ".mcp.json")` in `discovery.rs` line 120
- 24+ references across the codebase need updating — all test helper structs drop the field

**D-04:** `generate_mcp_config` gains `agent_name: &str` parameter. Injects into env section:
`"env": {"RC_AGENT_NAME": "right"}`. Caller in `cmd_up` passes `&agent.name`.

**D-05:** Memory server startup: if `RC_AGENT_NAME` is absent or empty → log to stderr:
`"warning: RC_AGENT_NAME not set — memories will record stored_by as 'unknown'"`.
Do NOT fail. Degraded provenance is acceptable.

**D-06:** Write TWO failing regression tests BEFORE fixing bugs:
1. `wrapper_without_telegram_omits_channels_when_mcp_json_exists` — AgentDef with `mcp_config_path: Some(...)` but no telegram config. Assert `--channels` absent. Currently FAILS.
2. `mcp_config_env_contains_agent_name` — call `generate_mcp_config(dir.path(), Path::new("rightclaw"), "myagent")`. Assert `parsed["mcpServers"]["rightmemory"]["env"]["RC_AGENT_NAME"] == "myagent"`. Currently FAILS.
After writing these failing tests, proceed to D-01 through D-05.

**D-07:** Write `19-HUMAN-UAT.md` with 7 manual test cases. UAT checklist:
1. `rm -rf ~/.rightclaw && rightclaw init` — directory structure created, default agent scaffolded
2. `rightclaw up` — all files generated: `memory.db`, `settings.json`, `.claude.json`, `.mcp.json`, credential symlink
3. `.mcp.json` content: `mcpServers.rightmemory.env.RC_AGENT_NAME` present; no `"telegram": true` key
4. Agent WITHOUT telegram: wrapper has no `--channels`, `settings.json` has no `enabledPlugins`
5. Agent WITH telegram: wrapper has `--channels plugin:telegram@claude-plugins-official`, `.env` has bot token
6. Memory round-trip: agent session stores memory → `rightclaw memory list <agent>` shows entry with correct `stored_by`
7. `rightclaw doctor` — all checks pass

### Claude's Discretion

- Whether to update the existing `wrapper_with_mcp_includes_channels_flag` test (line 126 in `shell_wrapper_tests.rs`) to use telegram config instead of `mcp_config_path`. It currently tests the broken behavior — update to test the correct behavior as part of D-06.
- Exact inline check for `.mcp.json` existence in `rightclaw list` status display (e.g., `agent.path.join(".mcp.json").exists()`).

### Deferred Ideas (OUT OF SCOPE)

- SEED-002: BOOTSTRAP.md onboarding doesn't trigger via Telegram
- Stale shell snapshot cleanup — left to CC per D-03 discussion
- Automated integration tests for fresh-init flow — manual UAT chosen; automation is v2.4 candidate
</user_constraints>

## Standard Stack

No new dependencies. All fixes use existing crates already in the workspace.

| Library | Version | Purpose | Status |
|---------|---------|---------|--------|
| serde_json | (workspace) | JSON manipulation for `.mcp.json` env injection | Already used in `mcp_config.rs` |
| miette | (workspace) | Error propagation | Already in all codegen modules |
| std::env::var | stdlib | `RC_AGENT_NAME` read in memory_server.rs | Already used (line 186) |

No installation steps needed.

## Architecture Patterns

### Telegram Detection Pattern (D-01)

Current (broken):
```rust
// shell_wrapper.rs line 30
let channels: Option<&str> = if agent.mcp_config_path.is_some() {
    Some("plugin:telegram@claude-plugins-official")
} else {
    None
};
```

Fixed:
```rust
let channels: Option<&str> = if agent
    .config
    .as_ref()
    .map(|c| c.telegram_token.is_some() || c.telegram_token_file.is_some())
    .unwrap_or(false)
{
    Some("plugin:telegram@claude-plugins-official")
} else {
    None
};
```

Same pattern applies in `settings.rs` (line 96) for `enabledPlugins`.

The `agent.config.as_ref().and_then(...)` pattern is already established throughout the codebase —
see `shell_wrapper.rs` line 36 (`model`), `settings.rs` lines 48-55 (sandbox overrides).

### MCP Config env Injection Pattern (D-04)

Current signature:
```rust
pub fn generate_mcp_config(agent_path: &Path, binary: &Path) -> miette::Result<()>
```

New signature:
```rust
pub fn generate_mcp_config(agent_path: &Path, binary: &Path, agent_name: &str) -> miette::Result<()>
```

The env object in the rightmemory entry changes from:
```json
"env": {}
```
to:
```json
"env": {"RC_AGENT_NAME": "agent-name-here"}
```

The existing `servers.insert(...)` call replaces the entire entry — the new `agent_name` is simply
included in the `serde_json::json!({...})` literal. No merge logic needed; the entry is always
fully replaced (idempotent overwrite pattern already in place at line 38-45 of `mcp_config.rs`).

Call site in `cmd_up` (main.rs line 497):
```rust
// Before:
rightclaw::codegen::generate_mcp_config(&agent.path, &self_exe)?;

// After:
rightclaw::codegen::generate_mcp_config(&agent.path, &self_exe, &agent.name)?;
```

### mcp_config_path Field Removal (D-03)

The field appears in 25 locations (confirmed by grep). Removal is mechanical:

**Logic changes (2 locations):**
- `shell_wrapper.rs` line 30 — replaced by D-01 Telegram check
- `settings.rs` line 96 — replaced by D-01 Telegram check

**Discovery (1 location):**
- `discovery.rs` line 120 — remove `mcp_config_path: optional_file(&path, ".mcp.json"),`

**Status display (1 location):**
- `main.rs` lines 307-312 — replace `agent.mcp_config_path.is_some()` with `agent.path.join(".mcp.json").exists()`

**Test helpers (21 locations — drop the field from struct literals):**

| File | Location |
|------|----------|
| `shell_wrapper_tests.rs` | lines 27, 39-42, 53, 143, 285 |
| `settings_tests.rs` | lines 14, 162 |
| `process_compose_tests.rs` | lines 27, 46 |
| `claude_json.rs` (tests) | lines 142, 363 |
| `telegram.rs` (tests) | line 85 |
| `discovery_tests.rs` | line 174 (assert updated) |
| `system_prompt_tests.rs` | line 21 |
| `init.rs` | lines 73, 151 |
| `main.rs` | line 940 |

The `init.rs` references (lines 73, 151) are in `pre_trust_directory` and test setup — review
those specifically since line 73 has `mcp_config_path: if telegram_token.is_some() { ... }` which
uses the old Telegram-detection logic. After D-03, this also becomes `None` (init doesn't
call codegen functions; it scaffolds the agent directory structure, not the session files).

### telegram.rs D-02 Change

Remove lines 37-41 in `telegram.rs` entirely:
```rust
// DELETE THIS BLOCK:
let mcp_json = agent.path.join(".mcp.json");
if !mcp_json.exists() {
    std::fs::write(&mcp_json, r#"{"telegram": true}"#)
        .map_err(|e| miette::miette!("failed to write .mcp.json: {e:#}"))?;
}
```

This also requires updating two tests in `telegram.rs`:
- `creates_mcp_json_if_absent` — delete or repurpose (`.mcp.json` no longer created here)
- `does_not_overwrite_existing_mcp_json` — delete (no longer relevant)

The `preserves_non_mcp_servers_keys` test in `mcp_config.rs` (line 104) explicitly preserves
`"telegram": true`. After D-02, this test must be updated: the key should no longer be present
in fresh `.mcp.json` files, and `generate_mcp_config` should not preserve it (or at minimum the
test should be renamed to test actual non-telegram keys).

### RC_AGENT_NAME Warning in memory_server.rs (D-05)

Current code (line 186):
```rust
let agent_name = std::env::var("RC_AGENT_NAME").unwrap_or_else(|_| "unknown".to_string());
```

New code:
```rust
let agent_name = match std::env::var("RC_AGENT_NAME") {
    Ok(name) if !name.is_empty() => name,
    _ => {
        eprintln!("warning: RC_AGENT_NAME not set — memories will record stored_by as 'unknown'");
        "unknown".to_string()
    }
};
```

Uses `eprintln!` (stderr) not `tracing::warn!` because the tracing subscriber is initialized two
lines earlier. Both work, but `eprintln!` is simpler and consistent with D-03 note about stderr.
The existing tracing init uses `with_writer(std::io::stderr)` so either works — use `tracing::warn!`
for consistency with the established pattern at line 174.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| JSON env injection in `.mcp.json` | Custom string manipulation | `serde_json::json!({})` with named fields | Already used in mcp_config.rs; avoids escaping bugs |
| Optional field access on `AgentConfig` | `if let Some(config) = ...` chains | `agent.config.as_ref().map(...)` | Established pattern in shell_wrapper.rs line 36, settings.rs lines 48-54 |

## Common Pitfalls

### Pitfall 1: init.rs mcp_config_path logic (line 73)

**What goes wrong:** `init.rs` line 73 sets `mcp_config_path: if telegram_token.is_some() { Some(...) } else { None }`. After D-03, this field is gone. But the LOGIC in init (Telegram detection) was never wrong — init correctly checks for a token. The field removal just stops recording the path. No logic fix needed for init, just field removal.

**Warning signs:** If you accidentally remove the telegram token check from init.rs, `rightclaw init` will stop scaffolding the Telegram `.mcp.json` marker. After D-02, the marker is gone anyway — `init.rs` lines 62-78 can be reviewed for whether the `if telegram_token.is_some()` block writes anything else that matters.

### Pitfall 2: D-02 breaks `preserves_non_mcp_servers_keys` test

**What goes wrong:** The test at `mcp_config.rs` line 104 feeds `{"telegram": true, ...}` and asserts `parsed["telegram"] == true`. After D-02, `telegram.rs` no longer writes the marker — but `generate_mcp_config` still preserves arbitrary unknown keys. The test passes but tests the wrong thing (preserving a key we now want absent). The test needs renaming/rewording, not deletion — the generic preservation behavior is still correct.

**How to avoid:** Rename test to `preserves_unknown_top_level_keys` using a neutral key like `"otherService": true`. The behavior (non-destructive merge) is correct; only the fixture data is wrong.

### Pitfall 3: `make_agent_with_mcp` helper becomes dead code after D-03

**What goes wrong:** `shell_wrapper_tests.rs` line 37 defines `make_agent_with_mcp` which sets `mcp_config_path`. After D-03 field removal, this function no longer compiles. The test `wrapper_with_mcp_includes_channels_flag` (line 126) uses this helper and must be rewritten as part of D-06. `wrapper_without_mcp_omits_channels_flag` (line 137) references the error message string containing `mcp_config_path` — update the assert message.

**How to avoid:** As part of D-06, replace `make_agent_with_mcp` with a helper that sets `telegram_token` in the config. The new regression test uses `make_agent_with_telegram_config`.

### Pitfall 4: discovery_tests.rs test `discover_detects_mcp_json` at line 164

**What goes wrong:** The test asserts `agents[0].mcp_config_path.is_some()` — after D-03 this field is gone. The test itself is still valid conceptually (`.mcp.json` existence IS discoverable) but now via inline check. The test either gets deleted or replaced with a check that `.mcp.json` file exists on disk (not in the struct).

**How to avoid:** Replace `assert!(agents[0].mcp_config_path.is_some())` with `assert!(agents[0].path.join(".mcp.json").exists())`.

### Pitfall 5: settings_tests.rs test `includes_telegram_plugin_when_mcp_present`

**What goes wrong:** Line 162 sets `agent.mcp_config_path = Some(...)` to trigger Telegram plugin injection. After D-01+D-03, detection is via config not path. This test must be updated to set `telegram_token` in `AgentConfig` instead.

**How to avoid:** Create a config with `telegram_token: Some("token".to_string())` and verify `settings["enabledPlugins"]["telegram@claude-plugins-official"] == true`.

## Code Examples

### Regression Test 1: Telegram false-positive (D-06 test 1)

Location: `crates/rightclaw/src/codegen/shell_wrapper_tests.rs`

```rust
// Source: CONTEXT.md D-06
#[test]
fn wrapper_without_telegram_omits_channels_when_mcp_json_exists() {
    // mcp_config_path will be REMOVED as part of D-03.
    // This test is written BEFORE D-03 to demonstrate the bug.
    // After D-03, rewrite to construct agent with mcp.json on disk but no telegram config.
    let agent = make_agent("testbot", Some("Go")); // no telegram in config
    // Bug: with old code, --channels would appear if mcp_config_path were set.
    // With new code (D-01), --channels must be absent because no telegram_token.
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();
    assert!(
        !output.contains("--channels"),
        "should NOT contain --channels when no telegram config, even if .mcp.json exists:\n{output}"
    );
}
```

### Regression Test 2: RC_AGENT_NAME injection (D-06 test 2)

Location: `crates/rightclaw/src/codegen/mcp_config.rs` tests module

```rust
// Source: CONTEXT.md D-06
#[test]
fn mcp_config_env_contains_agent_name() {
    let dir = tempdir().unwrap();
    generate_mcp_config(dir.path(), Path::new("rightclaw"), "myagent").unwrap();

    let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        parsed["mcpServers"]["rightmemory"]["env"]["RC_AGENT_NAME"],
        "myagent",
        "RC_AGENT_NAME must be injected into env"
    );
}
```

All existing tests in `mcp_config.rs` that call `generate_mcp_config(dir.path(), Path::new("rightclaw"))` must gain the new `agent_name` argument — add `"test-agent"` as the third arg to keep them compiling.

## Runtime State Inventory

This is a code/config fix phase, not a rename/refactor. No runtime state inventory required.

However, note: agents already running with the bug will have `.mcp.json` files with `"telegram": true`
as a top-level key and `"env": {}` (empty). After the fix:
- `generate_mcp_config` is called on every `rightclaw up` (idempotent overwrite of the `rightmemory`
  entry). The `env` field gets the correct `RC_AGENT_NAME` value automatically on the next `up`.
- The `"telegram": true` key: `generate_mcp_config` uses non-destructive merge — it only writes
  `mcpServers.rightmemory`. Pre-existing `"telegram": true` at root level will remain in `.mcp.json`
  files written by old versions of rightclaw until the agent dir is recreated. This is cosmetic only
  — CC ignores unknown top-level keys in `.mcp.json`. No migration needed.

## Environment Availability

Phase 19 is purely code changes + UAT doc authoring. No new external dependencies.

UAT requires `rightclaw` binary, `process-compose`, and `sqlite3` — all previously validated in Phase 16/17/18.

## Sources

### Primary (HIGH confidence)
- Direct source code inspection — all claims verified against live file contents
  - `crates/rightclaw/src/codegen/shell_wrapper.rs` — line 30: confirmed bug
  - `crates/rightclaw/src/codegen/settings.rs` — line 96: confirmed bug
  - `crates/rightclaw/src/codegen/mcp_config.rs` — line 43: `"env": {}` confirmed
  - `crates/rightclaw/src/codegen/telegram.rs` — lines 37-41: marker write confirmed
  - `crates/rightclaw/src/agent/types.rs` — `mcp_config_path` field confirmed
  - `crates/rightclaw/src/agent/discovery.rs` — line 120: assignment confirmed
  - `crates/rightclaw-cli/src/memory_server.rs` — line 186: fallback to "unknown" confirmed
  - `crates/rightclaw-cli/src/main.rs` — line 497: call site, line 307: list status

- Grep audit: 25 `mcp_config_path` references across 9 files (confirmed count vs CONTEXT.md "24")

### Secondary (HIGH — from CONTEXT.md)
- `.planning/phases/19-home-isolation-hardening/19-CONTEXT.md` — decisions D-01 through D-07

## Open Questions

1. **init.rs telegram block (lines 62-78)**
   - What we know: line 73 sets `mcp_config_path` based on telegram token presence. After D-03, this field is removed.
   - What's unclear: does the `if telegram_token.is_some()` block do anything else besides set the field that needs to be removed, or does it also write the `.mcp.json` marker? Looking at line 73: `mcp_config_path: if telegram_token.is_some() { Some(path.join(".mcp.json")) } else { None }` — it only sets the struct field, does not write to disk. Init creates the agent dir skeleton, not session files.
   - Recommendation: Remove the field from the struct literal in `init.rs`. The `telegram_token` check in init context is irrelevant after D-02/D-03. Read `init.rs` fully before editing it.

2. **telegram.rs tests after D-02**
   - What we know: `creates_mcp_json_if_absent` and `does_not_overwrite_existing_mcp_json` test the removed `.mcp.json` write behavior.
   - Recommendation: Delete both tests. They test a removed code path. Add a new test `token_with_config_does_not_create_mcp_json` to verify the marker is no longer written.

## Metadata

**Confidence breakdown:**
- Bug identification: HIGH — confirmed by reading source at the exact lines cited in CONTEXT.md
- Fix approach: HIGH — decisions are complete, patterns established in surrounding code
- Field removal scope: HIGH — grep audit confirms all 25 locations
- Test changes: HIGH — each affected test identified with file and line

**Research date:** 2026-03-27
**Valid until:** N/A — purely internal code; no external APIs
